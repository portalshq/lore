// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::jwk::JwkSet;
use lore_telemetry::InstrumentProvider;
use lore_telemetry::LabelArray;
use lore_telemetry::METRICS_OPERATION_LATENCY_METRIC_NAME;
use lore_telemetry::timed;
use lore_telemetry::timer::TimedResult;
use lore_transport::grpc::user_agent;
use opentelemetry::KeyValue;
use serde::Deserialize;
use smallvec::SmallVec;
use thiserror::Error;
use tracing::warn;

#[derive(Clone)]
struct JWKServiceKey {
    #[allow(dead_code)]
    jwk: Jwk,
    decoding_key: DecodingKey,
    algorithm: jsonwebtoken::Algorithm,
}

#[derive(Clone, Deserialize, Debug)]
pub struct JWKServiceSettings {
    pub endpoint: String,
    #[serde(default = "default_refresh_interval_seconds")]
    pub refresh_interval_seconds: u64,
    #[serde(default = "default_max_stale_seconds")]
    pub max_stale_seconds: u64,
}

fn default_refresh_interval_seconds() -> u64 {
    60
}
fn default_max_stale_seconds() -> u64 {
    600
}

impl Default for JWKServiceSettings {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            refresh_interval_seconds: default_refresh_interval_seconds(),
            max_stale_seconds: default_max_stale_seconds(),
        }
    }
}

#[derive(Error, Debug)]
pub enum JWKServiceError {
    #[error("Internal Error")]
    InternalError,
    #[error("Could not parse jwks endpoint response")]
    ParseError(#[from] serde_json::Error),
    #[error("Could not decode jwk key")]
    DecodingError(#[from] jsonwebtoken::errors::Error),
    #[error("Key for kid not found")]
    NotFound,
}

#[async_trait]
pub trait JWKService: Send + Sync {
    /// Get the public key for the specified key id. Note: this may potentially result in a network
    /// call if the key for key id is not already cached locally by the implementer of this trait.
    async fn get_key(
        &self,
        kid: &str,
    ) -> Result<(DecodingKey, jsonwebtoken::Algorithm), JWKServiceError>;
}

#[derive(Clone)]
pub struct JwkServiceImpl {
    // allow to be refetched from different threads if needed
    cached_set: Arc<tokio::sync::RwLock<HashMap<String, JWKServiceKey>>>,
    last_successful_refresh: Arc<tokio::sync::RwLock<Option<Instant>>>,
    #[allow(dead_code)]
    settings: JWKServiceSettings,
}

impl JwkServiceImpl {
    pub fn new(settings: JWKServiceSettings) -> Self {
        JwkServiceImpl {
            cached_set: Default::default(),
            last_successful_refresh: Default::default(),
            settings,
        }
    }

    async fn get_cached_key(
        &self,
        kid: &str,
    ) -> Result<(DecodingKey, jsonwebtoken::Algorithm), JWKServiceError> {
        let keys = self.cached_set.read().await;
        let res = keys.get(kid).ok_or(JWKServiceError::NotFound)?;
        Ok((res.decoding_key.clone(), res.algorithm))
    }

    /// Fetch the latest keys and replace the local cache. If `desired` is not-`None`,
    /// short-circuits if the key id is already present in the local cache.
    pub async fn fetch_new_keys(&self, desired: Option<&str>) -> Result<(), JWKServiceError> {
        if let Some(desired) = desired {
            let cache = self.cached_set.read().await;
            let fresh = self
                .last_successful_refresh
                .read()
                .await
                .is_some_and(|instant| {
                    instant.elapsed() <= Duration::from_secs(self.settings.refresh_interval_seconds)
                });
            if fresh && cache.contains_key(desired) {
                return Ok(());
            }
        }

        let client = reqwest::Client::builder()
            .user_agent(user_agent())
            .build()
            .map_err(|e| {
                warn!("Failed to construct HTTP client: {e:?}");
                JWKServiceError::InternalError
            })?;

        let response = timed!(
            self.latency_histogram_ms(METRICS_OPERATION_LATENCY_METRIC_NAME),
            &self.get_labels_for_operation_context("get_keys"),
            {
                client
                    .get(&self.settings.endpoint)
                    .send()
                    .await
                    .map_err(|e| {
                        warn!("Failed to fetch JWKS endpoint: {e:?}");
                        JWKServiceError::InternalError
                    })
            }
        )
        .result?;

        let status = response.status();
        let response_body = response.text().await.map_err(|e| {
            warn!("Failed to get response body from JWKS endpoint result: {e:?}");
            JWKServiceError::InternalError
        })?;

        if !status.is_success() {
            warn!("JWKS endpoint returned error. Status: {status}, response: {response_body}");

            return Err(JWKServiceError::InternalError);
        }

        let new_jwks: JwkSet = serde_json::from_str(response_body.as_str()).map_err(|e| {
            warn!("Failed to parse JWKS response: {response_body}");
            JWKServiceError::ParseError(e)
        })?;

        let mut new_set = HashMap::new();

        for jwk in new_jwks.keys {
            let kid = jwk
                .common
                .key_id
                .as_ref()
                .ok_or(JWKServiceError::InternalError)?;

            let algorithm = jwk
                .common
                .key_algorithm
                .ok_or(JWKServiceError::InternalError)?;
            let algorithm = jsonwebtoken::Algorithm::from_str(&algorithm.to_string())
                .map_err(JWKServiceError::DecodingError)?;

            new_set.insert(
                kid.clone(),
                JWKServiceKey {
                    decoding_key: DecodingKey::from_jwk(&jwk)
                        .map_err(JWKServiceError::DecodingError)?,
                    jwk,
                    algorithm,
                },
            );
        }

        *self.cached_set.write().await = new_set;
        *self.last_successful_refresh.write().await = Some(Instant::now());

        Ok(())
    }
}

#[async_trait]
impl JWKService for JwkServiceImpl {
    async fn get_key(
        &self,
        kid: &str,
    ) -> Result<(DecodingKey, jsonwebtoken::Algorithm), JWKServiceError> {
        let cached = self.get_cached_key(kid).await;
        let age = self
            .last_successful_refresh
            .read()
            .await
            .map(|instant| instant.elapsed());
        let refresh_after = Duration::from_secs(self.settings.refresh_interval_seconds);
        if cached.is_ok() && age.is_some_and(|age| age <= refresh_after) {
            return cached;
        }

        match self.fetch_new_keys(None).await {
            Ok(()) => self.get_cached_key(kid).await,
            Err(error) => {
                let max_stale = Duration::from_secs(self.settings.max_stale_seconds);
                if cached.is_ok() && age.is_some_and(|age| age <= max_stale) {
                    warn!("JWKS refresh failed; using a bounded stale key: {error:?}");
                    cached
                } else {
                    Err(error)
                }
            }
        }
    }
}

impl InstrumentProvider for JwkServiceImpl {
    fn namespace(&self) -> &'static str {
        "urc.auth.jwk_service"
    }
}
