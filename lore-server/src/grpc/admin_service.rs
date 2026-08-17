// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use lore_base::version::LORE_LIBRARY_VERSION;
use lore_proto::rpc::HostInfo;
use lore_proto::rpc::ServerInfoRequest;
use lore_proto::rpc::ServerInfoResponse;
use lore_proto::rpc::admin_service_server::AdminService;
use lore_revision::notification::NotificationSender;
use sysinfo::CpuRefreshKind;
use sysinfo::MemoryRefreshKind;
use sysinfo::RefreshKind;
use tonic::Request;
use tonic::Response;
use tonic::Status;
use tracing::info;
use tracing::instrument;
use tracing::warn;

use super::handlers::obliterate;
use super::timeout_grpc;
use crate::auth::jwt::JwtVerifier;
use crate::hooks::HookDispatcher;

pub struct LoreAdminService {
    server_info: ServerInfoResponse,
    immutable_store: Arc<dyn lore_storage::ImmutableStore>,
    mutable_store: Arc<dyn lore_storage::MutableStore>,
    jwt_verifier: Arc<Option<JwtVerifier>>,
    notification: Arc<dyn NotificationSender>,
    hook_dispatcher: Arc<HookDispatcher>,
    rpc_timeout: Duration,
    enabled: bool,
}

impl LoreAdminService {
    pub fn new(
        settings: HashMap<String, String>,
        features: Vec<String>,
        immutable_store: Arc<dyn lore_storage::ImmutableStore>,
        mutable_store: Arc<dyn lore_storage::MutableStore>,
        notification: Arc<dyn NotificationSender>,
        hook_dispatcher: Arc<HookDispatcher>,
    ) -> Self {
        let mut sys =
            sysinfo::System::new_with_specifics(RefreshKind::everything().without_processes());

        sys.refresh_cpu_specifics(CpuRefreshKind::nothing().with_frequency());
        sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());

        let cores = sys.cpus().len();
        let cpu = sys
            .cpus()
            .iter()
            .next()
            .map_or("unknown".to_string(), |cpu| {
                format!(
                    "{} ({} cores) {:.1}ghz",
                    cpu.brand(),
                    cores,
                    cpu.frequency() as f64 / 1000f64
                )
            });

        let ram = format!("{} GiB", sys.total_memory() / 1024 / 1024 / 1024);

        Self {
            server_info: ServerInfoResponse {
                version: LORE_LIBRARY_VERSION.to_string(),
                features,
                settings,
                host: Some(HostInfo {
                    arch: env!("VERGEN_RUSTC_HOST_TRIPLE").to_string(),
                    cpu,
                    ram,
                    hostname: sysinfo::System::host_name().unwrap_or("unknown".to_string()),
                    environment: std::env::var("LORE_ENV").unwrap_or("unknown".to_string()),
                }),
            },
            immutable_store,
            mutable_store,
            jwt_verifier: Arc::new(None),
            notification,
            hook_dispatcher,
            rpc_timeout: Duration::from_secs(60),
            enabled: true,
        }
    }

    pub fn set_jwt_verifier(&mut self, jwt_verifier: Option<JwtVerifier>) {
        if jwt_verifier.is_none() {
            warn!(
                "No JWT verifier - Admin Service RPCs including obliterate will be unauthenticated"
            );
        }
        self.jwt_verifier = Arc::new(jwt_verifier);
    }

    pub fn set_rpc_timeout(&mut self, rpc_timeout: Duration) {
        self.rpc_timeout = rpc_timeout;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn require_enabled(&self) -> Result<(), Status> {
        if self.enabled {
            Ok(())
        } else {
            Err(Status::unimplemented("AdminService is disabled"))
        }
    }
}

#[tonic::async_trait]
impl AdminService for LoreAdminService {
    #[instrument(name = "AdminService::ServerInfo", skip_all)]
    async fn server_info(
        &self,
        _request: Request<ServerInfoRequest>,
    ) -> Result<Response<ServerInfoResponse>, Status> {
        self.require_enabled()?;
        info!("Request for ServerInfo");

        Ok(Response::new(self.server_info.clone()))
    }

    async fn obliterate(
        &self,
        request: Request<lore_proto::ObliterateRequest>,
    ) -> Result<Response<lore_proto::ObliterateResponse>, Status> {
        self.require_enabled()?;
        timeout_grpc(
            self.rpc_timeout,
            obliterate::handler(
                request,
                self.immutable_store.clone(),
                self.mutable_store.clone(),
                self.notification.clone(),
                &self.hook_dispatcher,
                &self.jwt_verifier,
            ),
        )
        .await
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;
    use crate::notification::testing::MockNotificationSender;
    use crate::store::test_store_create;
    use tonic::Code;

    #[tokio::test]
    async fn disabled_admin_service_returns_unimplemented_before_store_access() {
        let (immutable, mutable, _) = test_store_create().await.expect("test stores");
        let mut service = LoreAdminService::new(
            HashMap::new(),
            Vec::new(),
            immutable,
            mutable,
            Arc::new(MockNotificationSender::new()),
            Arc::new(HookDispatcher::empty()),
        );
        service.set_enabled(false);

        let error = service
            .server_info(Request::new(ServerInfoRequest {}))
            .await
            .expect_err("disabled AdminService must fail");
        assert_eq!(error.code(), Code::Unimplemented);
    }
}
