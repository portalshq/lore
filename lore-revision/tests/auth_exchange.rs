// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT
/// Auth exchange integration tests.
///
/// These tests verify the `Authentication` trait's error handling patterns
/// that the orchestration layer depends on for identity probing and
/// authorization exchange. They use `MockAuthentication` registered in the
/// global authentication registry.
///
/// The original `AuthExchange` trait tests (identity selection, domain
/// filtering) operated on an in-memory mock of the token store. Those
/// scenarios are now covered by:
/// - Unit tests in `protocol::tests` (mock trait method responses)
/// - Smoke tests (end-to-end with real token store)
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use lore_base::error::NotAuthenticated;
    use lore_base::error::NotAuthorized;
    use lore_base::error::NotSupported;
    use lore_revision::lore::RepositoryId;
    use lore_transport::AuthSession;
    use lore_transport::Authentication;
    use lore_transport::AuthenticationToken;
    use lore_transport::AuthorizationToken;
    use lore_transport::ProtocolError;
    use lore_transport::ResolvedUser;
    use lore_transport::auth::authentication;

    struct TestAuthentication {
        refresh_not_sent: bool,
        refresh_delay: Option<std::time::Duration>,
        refresh_result:
            Option<Box<dyn Fn() -> Result<AuthenticationToken, ProtocolError> + Send + Sync>>,
        exchange_result:
            Box<dyn Fn(RepositoryId) -> Result<AuthorizationToken, ProtocolError> + Send + Sync>,
    }

    impl TestAuthentication {
        fn always_succeed() -> Self {
            Self {
                refresh_not_sent: false,
                refresh_delay: None,
                refresh_result: None,
                exchange_result: Box::new(|_| {
                    Ok(AuthorizationToken {
                        token: "authz-token".into(),
                        expires_ms: u64::MAX,
                        acceptable_root_domains: vec![],
                    })
                }),
            }
        }

        fn always_not_authorized() -> Self {
            Self {
                refresh_not_sent: false,
                refresh_delay: None,
                refresh_result: None,
                exchange_result: Box::new(|_| Err(ProtocolError::from(NotAuthorized))),
            }
        }

        fn always_not_authenticated() -> Self {
            Self {
                refresh_not_sent: false,
                refresh_delay: None,
                refresh_result: None,
                exchange_result: Box::new(|_| Err(ProtocolError::from(NotAuthenticated))),
            }
        }
    }

    #[async_trait]
    impl Authentication for TestAuthentication {
        async fn refresh_authentication_with_outcome(
            &self,
            auth_url: &str,
            refresh: &str,
            correlation: &str,
        ) -> Result<AuthenticationToken, lore_transport::RefreshFailure> {
            if self.refresh_not_sent {
                return Err(lore_transport::RefreshFailure::NotSent(
                    ProtocolError::from(lore_base::error::Disconnected),
                ));
            }
            self.refresh_authentication(auth_url, refresh, correlation)
                .await
                .map_err(lore_transport::RefreshFailure::Uncertain)
        }
        async fn revoke_refresh(
            &self,
            _auth_url: &str,
            _refresh_token: &str,
            _correlation_id: &str,
        ) -> Result<(), ProtocolError> {
            Ok(())
        }
        async fn start_auth_session(
            &self,
            _auth_url: &str,
            _client_state: &str,
            _correlation_id: &str,
        ) -> Result<AuthSession, ProtocolError> {
            Err(ProtocolError::from(NotSupported {
                operation: "start_auth_session".into(),
            }))
        }

        async fn poll_auth_session(
            &self,
            _auth_url: &str,
            _client_state: &str,
            _session_code: &str,
            _correlation_id: &str,
        ) -> Result<Option<AuthenticationToken>, ProtocolError> {
            Ok(None)
        }

        async fn exchange_external_token(
            &self,
            _auth_url: &str,
            _token: &str,
            _token_type: &str,
            _correlation_id: &str,
        ) -> Result<AuthenticationToken, ProtocolError> {
            Err(ProtocolError::from(NotSupported {
                operation: "exchange_external_token".into(),
            }))
        }

        async fn refresh_authentication(
            &self,
            _auth_url: &str,
            _refresh_token: &str,
            _correlation_id: &str,
        ) -> Result<AuthenticationToken, ProtocolError> {
            if let Some(delay) = self.refresh_delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(refresh) = &self.refresh_result {
                return refresh();
            }
            Err(ProtocolError::from(NotSupported {
                operation: "refresh_authentication".into(),
            }))
        }

        async fn exchange_for_repository(
            &self,
            _auth_url: &str,
            _authn_token: &str,
            repository: RepositoryId,
            _correlation_id: &str,
        ) -> Result<AuthorizationToken, ProtocolError> {
            (self.exchange_result)(repository)
        }

        async fn exchange_for_custom_resource(
            &self,
            _auth_url: &str,
            _authn_token: &str,
            _resource_id: &str,
            _correlation_id: &str,
        ) -> Result<AuthorizationToken, ProtocolError> {
            (self.exchange_result)(RepositoryId::default())
        }

        async fn get_user_info(
            &self,
            _auth_url: &str,
            _authz_token: &str,
            _repository: RepositoryId,
            user_ids: &[String],
            _correlation_id: &str,
        ) -> Result<Vec<ResolvedUser>, ProtocolError> {
            Ok(user_ids
                .iter()
                .map(|id| ResolvedUser {
                    user_id: id.clone(),
                    user_name: format!("User {id}"),
                })
                .collect())
        }

        async fn get_user_id(
            &self,
            _auth_url: &str,
            _authz_token: &str,
            _repository: RepositoryId,
            display_name: &str,
            _correlation_id: &str,
        ) -> Result<Option<ResolvedUser>, ProtocolError> {
            Ok(Some(ResolvedUser {
                user_id: format!("id-for-{display_name}"),
                user_name: display_name.to_string(),
            }))
        }
    }

    #[test]
    fn test_auth_registration_and_lookup() {
        let scheme = "test-auth-exchange";
        let mock = Arc::new(TestAuthentication::always_succeed());
        authentication::add(scheme, mock).unwrap();

        let found = authentication::find(&format!("{scheme}://auth.test.com"));
        assert!(found.is_ok());
    }

    /// Run only with an isolated, disposable nonproduction credential store.
    #[tokio::test]
    #[ignore = "requires isolated LORE_AUTH_PATH and explicit nonproduction fallback store"]
    async fn renewal_serializes_and_preserves_identity() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use lore_credential::token_store;
        use lore_transport::auth::exchange::authentication_token;
        use std::sync::atomic::{AtomicUsize, Ordering};
        assert_eq!(std::env::var("LORE_AUTH_STORE").unwrap(), "fallback");
        assert_ne!(std::env::var("LORE_SECURITY_MODE").unwrap(), "strict");
        assert!(
            std::env::var("LORE_AUTH_PATH")
                .unwrap()
                .contains("portals-renewal-test")
        );
        let jwt = |expiry: u64| {
            let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
            let body = URL_SAFE_NO_PAD.encode(serde_json::json!({"iss":"https://auth.test.com","sub":"renewal-user","name":"Test","aud":["test.com"],"exp":expiry}).to_string());
            format!("{header}.{body}.{}", URL_SAFE_NO_PAD.encode([0_u8; 32]))
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let old = jwt(now - 1);
        let fresh = jwt(now + 900);
        let count = Arc::new(AtomicUsize::new(0));
        let mut provider = TestAuthentication::always_succeed();
        let calls = count.clone();
        let returned = fresh.clone();
        provider.refresh_result = Some(Box::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(AuthenticationToken {
                token: returned.clone(),
                user_id: "renewal-user".into(),
                user_name: "Test".into(),
                expires_ms: (now + 900) * 1000,
                acceptable_root_domains: vec!["test.com".into()],
                refresh_token: Some("next-test-refresh".into()),
            })
        }));
        authentication::add("renewal-test", Arc::new(provider)).unwrap();
        let url = "renewal-test://auth.test.com";
        token_store::store_user_session(
            url,
            "renewal-user",
            &old,
            vec!["test.com".into()],
            Some("initial-test-refresh"),
        )
        .await
        .unwrap();
        let (a, b) = tokio::join!(
            authentication_token(url, "renewal-user", "test.com"),
            authentication_token(url, "renewal-user", "test.com")
        );
        assert_eq!(a.unwrap(), fresh);
        assert_eq!(b.unwrap(), fresh);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            token_store::load_refresh_token(url, "renewal-user")
                .await
                .unwrap(),
            "next-test-refresh"
        );
        assert!(
            authentication_token(url, "renewal-user", "evil.example")
                .await
                .is_err()
        );

        let mut unavailable = TestAuthentication::always_succeed();
        let uncertain_calls = Arc::new(AtomicUsize::new(0));
        let observed_calls = uncertain_calls.clone();
        unavailable.refresh_result = Some(Box::new(move || {
            observed_calls.fetch_add(1, Ordering::SeqCst);
            Err(ProtocolError::from(lore_base::error::Disconnected))
        }));
        authentication::add("renewal-offline", Arc::new(unavailable)).unwrap();
        let url = "renewal-offline://auth.test.com";
        token_store::store_user_session(
            url,
            "renewal-user",
            &old,
            vec!["test.com".into()],
            Some("retain-test-refresh"),
        )
        .await
        .unwrap();
        assert!(
            authentication_token(url, "renewal-user", "test.com")
                .await
                .is_err()
        );
        assert_eq!(
            token_store::load_refresh_token(url, "renewal-user")
                .await
                .unwrap(),
            "retain-test-refresh"
        );
        assert!(
            authentication_token(url, "renewal-user", "test.com")
                .await
                .is_err()
        );
        assert_eq!(
            uncertain_calls.load(Ordering::SeqCst),
            1,
            "An uncertain refresh must not be replayed"
        );
        let mut unsent = TestAuthentication::always_succeed();
        unsent.refresh_not_sent = true;
        authentication::add("renewal-unsent", Arc::new(unsent)).unwrap();
        let unsent_url = "renewal-unsent://auth.test.com";
        token_store::store_user_session(
            unsent_url,
            "renewal-user",
            &old,
            vec!["test.com".into()],
            Some("unsent-test-refresh"),
        )
        .await
        .unwrap();
        for _ in 0..2 {
            assert!(
                authentication_token(unsent_url, "renewal-user", "test.com")
                    .await
                    .is_err()
            );
        }
        let mut guard = token_store::lock_refresh(unsent_url, "renewal-user")
            .await
            .unwrap();
        token_store::begin_refresh_attempt(&mut guard, "unsent-test-refresh")
            .expect("Unsent attempts must remain retryable");
        token_store::clear_refresh_attempt(&mut guard).unwrap();
        drop(guard);

        let mut rejected = TestAuthentication::always_succeed();
        rejected.refresh_result = Some(Box::new(|| Err(ProtocolError::from(NotAuthenticated))));
        authentication::add("renewal-rejected", Arc::new(rejected)).unwrap();
        let url = "renewal-rejected://auth.test.com";
        token_store::store_user_session(
            url,
            "renewal-user",
            &old,
            vec!["test.com".into()],
            Some("rejected-test-refresh"),
        )
        .await
        .unwrap();
        assert!(
            authentication_token(url, "renewal-user", "test.com")
                .await
                .is_err()
        );
        assert!(
            token_store::load_refresh_token(url, "renewal-user")
                .await
                .is_err()
        );
        for (scheme, malformed) in [
            ("renewal-malformed", true),
            ("renewal-missing-refresh", false),
        ] {
            let mut provider = TestAuthentication::always_succeed();
            let returned = fresh.clone();
            provider.refresh_result = Some(Box::new(move || {
                Ok(AuthenticationToken {
                    token: if malformed {
                        "not-a-jwt".into()
                    } else {
                        returned.clone()
                    },
                    user_id: "renewal-user".into(),
                    user_name: "Test".into(),
                    expires_ms: (now + 900) * 1000,
                    acceptable_root_domains: vec!["test.com".into()],
                    refresh_token: None,
                })
            }));
            authentication::add(scheme, Arc::new(provider)).unwrap();
            let endpoint = format!("{scheme}://auth.test.com");
            token_store::store_user_session(
                &endpoint,
                "renewal-user",
                &old,
                vec!["test.com".into()],
                Some("original-test-refresh"),
            )
            .await
            .unwrap();
            assert!(
                authentication_token(&endpoint, "renewal-user", "test.com")
                    .await
                    .is_err()
            );
            assert_eq!(
                token_store::load_refresh_token(&endpoint, "renewal-user")
                    .await
                    .unwrap(),
                "original-test-refresh"
            );
        }
        let mut delayed = TestAuthentication::always_succeed();
        delayed.refresh_delay = Some(std::time::Duration::from_secs(30));
        authentication::add("renewal-timeout", Arc::new(delayed)).unwrap();
        let endpoint = "renewal-timeout://auth.test.com";
        token_store::store_user_session(
            endpoint,
            "renewal-user",
            &old,
            vec!["test.com".into()],
            Some("timeout-test-refresh"),
        )
        .await
        .unwrap();
        assert!(
            authentication_token(endpoint, "renewal-user", "test.com")
                .await
                .is_err()
        );
        assert_eq!(
            token_store::load_refresh_token(endpoint, "renewal-user")
                .await
                .unwrap(),
            "timeout-test-refresh"
        );
    }

    #[tokio::test]
    async fn exchange_for_repository_success_returns_token() {
        let scheme = "test-exchange-success";
        authentication::add(scheme, Arc::new(TestAuthentication::always_succeed())).unwrap();

        let auth = authentication::find(&format!("{scheme}://auth.test.com")).unwrap();
        let result = auth
            .exchange_for_repository(
                &format!("{scheme}://auth.test.com"),
                "authn-tok",
                RepositoryId::default(),
                "corr",
            )
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().token, "authz-token");
    }

    #[tokio::test]
    async fn exchange_for_custom_resource_success_returns_token() {
        let scheme = "test-exchange-custom-success";
        authentication::add(scheme, Arc::new(TestAuthentication::always_succeed())).unwrap();

        let auth = authentication::find(&format!("{scheme}://auth.test.com")).unwrap();
        let result = auth
            .exchange_for_custom_resource(
                &format!("{scheme}://auth.test.com"),
                "authn-tok",
                "bespoke-urc:uefn:some-stream",
                "corr",
            )
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().token, "authz-token");
    }

    #[tokio::test]
    async fn exchange_not_authorized_is_matchable() {
        let scheme = "test-exchange-not-authz";
        authentication::add(
            scheme,
            Arc::new(TestAuthentication::always_not_authorized()),
        )
        .unwrap();

        let auth = authentication::find(&format!("{scheme}://auth.test.com")).unwrap();
        let result = auth
            .exchange_for_repository(
                &format!("{scheme}://auth.test.com"),
                "authn-tok",
                RepositoryId::default(),
                "corr",
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_not_authorized());
    }

    #[tokio::test]
    async fn exchange_not_authenticated_is_matchable() {
        let scheme = "test-exchange-not-authn";
        authentication::add(
            scheme,
            Arc::new(TestAuthentication::always_not_authenticated()),
        )
        .unwrap();

        let auth = authentication::find(&format!("{scheme}://auth.test.com")).unwrap();
        let result = auth
            .exchange_for_repository(
                &format!("{scheme}://auth.test.com"),
                "authn-tok",
                RepositoryId::default(),
                "corr",
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_not_authenticated());
    }

    #[tokio::test]
    async fn get_user_info_returns_resolved_users() {
        let scheme = "test-userinfo";
        authentication::add(scheme, Arc::new(TestAuthentication::always_succeed())).unwrap();

        let auth = authentication::find(&format!("{scheme}://auth.test.com")).unwrap();
        let users = auth
            .get_user_info(
                &format!("{scheme}://auth.test.com"),
                "authz-tok",
                RepositoryId::default(),
                &["u1".into(), "u2".into()],
                "corr",
            )
            .await
            .unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].user_id, "u1");
        assert_eq!(users[1].user_name, "User u2");
    }

    #[tokio::test]
    async fn get_user_id_returns_resolved_user() {
        let scheme = "test-userid";
        authentication::add(scheme, Arc::new(TestAuthentication::always_succeed())).unwrap();

        let auth = authentication::find(&format!("{scheme}://auth.test.com")).unwrap();
        let user = auth
            .get_user_id(
                &format!("{scheme}://auth.test.com"),
                "authz-tok",
                RepositoryId::default(),
                "Alice",
                "corr",
            )
            .await
            .unwrap();
        assert!(user.is_some());
        assert_eq!(user.unwrap().user_id, "id-for-Alice");
    }

    #[tokio::test]
    async fn not_supported_interactive_login() {
        let scheme = "test-no-interactive";
        authentication::add(scheme, Arc::new(TestAuthentication::always_succeed())).unwrap();

        let auth = authentication::find(&format!("{scheme}://auth.test.com")).unwrap();
        let result = auth
            .start_auth_session(&format!("{scheme}://auth.test.com"), "state", "corr")
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_not_supported());
    }
}
