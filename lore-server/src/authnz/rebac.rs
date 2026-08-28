// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT
use lore_proto::RebacApiClient as RebacApiGrpcClient;
use lore_proto::rebac::ConfirmResourceDeletedRequest;
use lore_proto::rebac::ConfirmResourceDeletedResponse;
use lore_proto::rebac::CreateResourceRequest;
use lore_proto::rebac::CreateResourceResponse;
use lore_proto::rebac::DeleteResourceRequest;
use lore_proto::rebac::DeleteResourceResponse;
use lore_telemetry::InstrumentProvider;
use lore_telemetry::LabelArray;
use lore_telemetry::METRICS_OPERATION_LATENCY_METRIC_NAME;
use lore_telemetry::timed;
use lore_telemetry::timer::TimedResult;
use lore_transport::grpc::CorrelationInterceptor;
use opentelemetry::KeyValue;
use smallvec::SmallVec;
use tokio::time::{Duration, sleep};
use tonic::Request;
use tonic::Response;
use tonic::Status;
use tonic::codegen::InterceptedService;
use tonic::transport::ClientTlsConfig;

use crate::grpc::ServerResultExt;

pub type RebacApiResult<T> = Result<Response<T>, Status>;

#[async_trait::async_trait]
pub trait RebacApiClient {
    async fn create_resource(
        &mut self,
        request: Request<CreateResourceRequest>,
    ) -> RebacApiResult<CreateResourceResponse>;

    async fn delete_resource(
        &mut self,
        request: Request<DeleteResourceRequest>,
    ) -> RebacApiResult<DeleteResourceResponse>;

    async fn confirm_resource_deleted(
        &mut self,
        request: Request<ConfirmResourceDeletedRequest>,
    ) -> RebacApiResult<ConfirmResourceDeletedResponse>;
}

pub struct RebacClientHelper {
    client:
        RebacApiGrpcClient<InterceptedService<tonic::transport::Channel, CorrelationInterceptor>>,
}

impl RebacClientHelper {
    async fn new(auth_url: String) -> Result<RebacClientHelper, Status> {
        // Authentication exchange is public; relationship mutation is not.
        // Production uses the colocated Auth Gateway through the host-only
        // ReBAC port. The fallback keeps local development compatible with
        // the combined test service.
        let mut rebac_url = std::env::var("LORE_REBAC_URL").unwrap_or(auth_url);
        // ReBAC is canonically :8087. If the URL has no explicit port
        // (for example http://127.0.0.1), append :8087 rather than dialing :80.
        if rebac_url.starts_with("http://") {
            if rebac_url.ends_with(":80") || rebac_url.ends_with(":80/") {
                rebac_url = rebac_url.replace(":80", ":8087");
            } else if !rebac_url.matches(':').count().ge(&2) {
                // http://host without port → http://host:8087
                rebac_url = format!("{}:8087", rebac_url.trim_end_matches('/'));
            }
        }
        tracing::debug!(%rebac_url, "ReBAC endpoint");
        let mut endpoint = tonic::transport::Endpoint::from_shared(rebac_url.clone())
            .warn_map_err(|_| Status::internal("Failed to create rebac endpoint"))?;
        if rebac_url.starts_with("https://") {
            endpoint = endpoint
                .tls_config(
                    ClientTlsConfig::new()
                        .assume_http2(true)
                        .with_native_roots(),
                )
                .warn_map_err(|_| Status::internal("Failed to configure TLS for rebac"))?;
        }
        let max_attempts = std::env::var("LORE_REBAC_CONNECT_MAX_ATTEMPTS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(8);
        for attempt in 1..=max_attempts {
            match endpoint.clone().connect().await {
                Ok(channel) => {
                    let client =
                        RebacApiGrpcClient::with_interceptor(channel, CorrelationInterceptor);
                    return Ok(RebacClientHelper { client });
                }
                Err(error) if attempt < max_attempts => {
                    let backoff_ms = 250_u64.saturating_mul(1_u64 << (attempt - 1).min(4));
                    tracing::warn!(%rebac_url, attempt, max_attempts, backoff_ms, %error, "ReBAC unavailable; retrying connection");
                    sleep(Duration::from_millis(backoff_ms)).await;
                }
                Err(_) => return Err(Status::internal("Failed to connect to rebac service")),
            }
        }
        unreachable!("ReBAC retry loop returns on success or its final failure")
    }
}

#[async_trait::async_trait]
impl RebacApiClient for RebacClientHelper {
    async fn create_resource(
        &mut self,
        request: Request<CreateResourceRequest>,
    ) -> RebacApiResult<CreateResourceResponse> {
        timed!(
            self.latency_histogram_ms(METRICS_OPERATION_LATENCY_METRIC_NAME),
            &self.get_labels_for_operation_context("create_resource"),
            self.client.create_resource(request).await
        )
        .result
    }

    async fn delete_resource(
        &mut self,
        request: Request<DeleteResourceRequest>,
    ) -> RebacApiResult<DeleteResourceResponse> {
        timed!(
            self.latency_histogram_ms(METRICS_OPERATION_LATENCY_METRIC_NAME),
            &self.get_labels_for_operation_context("delete_resource"),
            self.client.delete_resource(request).await
        )
        .result
    }

    async fn confirm_resource_deleted(
        &mut self,
        request: Request<ConfirmResourceDeletedRequest>,
    ) -> RebacApiResult<ConfirmResourceDeletedResponse> {
        timed!(
            self.latency_histogram_ms(METRICS_OPERATION_LATENCY_METRIC_NAME),
            &self.get_labels_for_operation_context("confirm_resource_deleted"),
            self.client.confirm_resource_deleted(request).await
        )
        .result
    }
}

pub async fn grpc_get_rebac_client(auth_url: String) -> Result<RebacClientHelper, Status> {
    RebacClientHelper::new(auth_url).await
}

impl InstrumentProvider for RebacClientHelper {
    fn namespace(&self) -> &'static str {
        "urc.authnz.rebac"
    }
}
