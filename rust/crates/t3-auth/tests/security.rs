#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use serde_json::json;
use t3_auth::{
    AuthConfig, EnvironmentAuth, Error, Request, SessionStore,
    authorization::*,
    oauth::{AuthorizationError, McpOAuth},
    pairing::*,
    utils::*,
};
use t3_contracts::auth::*;

#[tokio::test]
async fn empty_signing_keys_are_refused_at_all_loaders() {
    for name in ["server-signing-key", "mcp-oauth-signing-key", "direct-key"] {
        let h = Harness::new().await;
        h.auth.secrets.set(name, b"").await.unwrap();
        assert!(
            PayloadSigner::from_store(&h.auth.secrets, name)
                .await
                .is_err(),
            "empty {name}"
        );
        if name == "server-signing-key" {
            assert!(
                SessionStore::new(
                    h.db.clone(),
                    &h.auth.secrets,
                    h.auth.config().clone(),
                    h.clock.clone()
                )
                .await
                .is_err()
            );
            assert!(
                EnvironmentAuth::new(
                    h.db.clone(),
                    h.auth.secrets.clone(),
                    h.auth.config().clone(),
                    None,
                    h.clock.clone()
                )
                .await
                .is_err()
            );
        } else if name == "mcp-oauth-signing-key" {
            assert!(
                McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn empty_bootstrap_secret_is_a_configuration_error() {
    let h = Harness::new().await;
    let config = AuthConfig {
        desktop_bootstrap_secret: Some(String::new()),
        ..h.auth.config().clone()
    };
    assert!(
        EnvironmentAuth::new(
            h.db.clone(),
            h.auth.secrets.clone(),
            config,
            None,
            h.clock.clone()
        )
        .await
        .is_err()
    );
}

#[test]
fn guarded_fixture_methods_reject_malformed_payloads() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/rpc.json")).unwrap();
    let schemas: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/contracts.schema.json")).unwrap();
    fn properties(
        schema: &serde_json::Value,
        definitions: &serde_json::Value,
        names: &mut std::collections::BTreeSet<String>,
    ) {
        if let Some(reference) = schema["$ref"].as_str() {
            properties(
                &definitions[reference.strip_prefix("#/$defs/").unwrap()],
                definitions,
                names,
            );
        }
        if let Some(fields) = schema["properties"].as_object() {
            names.extend(fields.keys().cloned());
        }
        for union in ["allOf", "anyOf", "oneOf"] {
            if let Some(members) = schema[union].as_array() {
                for member in members {
                    properties(member, definitions, names);
                }
            }
        }
    }
    let mut checked = 0;
    for method in fixture.as_array().unwrap() {
        if method["group"] == "WsRpcGroup" && !method["requiredScope"].is_null() {
            let tag = method["tag"].as_str().unwrap();
            assert!(
                matches!(
                    authorize_rpc(STANDARD_SCOPES, tag, &json!([])),
                    Err(Error::InvalidPayload(_))
                ),
                "{tag}"
            );
            let mut fields = std::collections::BTreeSet::new();
            properties(&method["payload"], &schemas["$defs"], &mut fields);
            if !fields.is_empty() {
                let malformed: serde_json::Value = fields
                    .into_iter()
                    .map(|field| (field, json!(123456)))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
                assert!(
                    matches!(
                        authorize_rpc(STANDARD_SCOPES, tag, &malformed),
                        Err(Error::InvalidPayload(_))
                    ),
                    "typed object for {tag}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 51);
}

#[tokio::test]
async fn original_case_origin_and_authorization_cannot_bypass_guards() {
    let h = Harness::config(AuthConfig {
        dev_url: Some("http://localhost:5173".into()),
        dev_auth_token: Some("private-dev-token".into()),
        ..Default::default()
    })
    .await;
    let dev = h.auth.sessions.dev_auth().unwrap();
    let mut request = Request {
        method: "GET".into(),
        url: "/ws".into(),
        headers: [
            ("Host".into(), "localhost:3773".into()),
            ("Origin".into(), "https://evil.example".into()),
        ]
        .into(),
        cookies: [(dev.cookie_name.clone(), dev.credential.clone())].into(),
        ..Default::default()
    };
    assert!(h.auth.authenticate_http(&request).await.is_err());
    request.headers.remove("Origin");
    request
        .headers
        .insert("AUTHORIZATION".into(), "Basic invalid".into());
    assert!(matches!(
        h.auth.authenticate_http(&request).await,
        Err(Error::MissingCredential)
    ));
}

#[test]
fn secret_debug_is_redacted() {
    let cookie = t3_auth::environment::Cookie::session(
        "session".into(),
        "SENSITIVE_COOKIE".into(),
        t3_auth::clock::iso(NOW).unwrap(),
        false,
    );
    assert!(!format!("{cookie:?}").contains("SENSITIVE_COOKIE"));
    let error = AuthorizationError::Redirect {
        error: "invalid_request",
        description: "invalid".into(),
        redirect_uri: "https://example.test".into(),
        state: Some("SENSITIVE_STATE".into()),
    };
    assert!(!format!("{error:?}").contains("SENSITIVE_STATE"));
}

#[tokio::test]
async fn overflow_ttls_return_date_errors() {
    let h = Harness::new().await;
    let session = h.issue("ttl").await;
    assert!(matches!(
        h.auth
            .sessions
            .issue_websocket_token(&session.session_id, Some(i64::MAX), None, None)
            .await,
        Err(Error::Date(_))
    ));
    assert!(matches!(
        h.auth
            .pairing
            .issue_one_time_token(
                t3_auth::authorization::STANDARD_SCOPES,
                IssuePairing {
                    scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                    ttl_ms: Some(i64::MAX),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Date(_))
    ));
    h.clock.set(i64::MAX - 1);
    assert!(matches!(
        t3_auth::PairingGrantStore::new(h.db.clone(), h.auth.config().clone(), h.clock.clone()),
        Err(Error::Date(_))
    ));
}

#[tokio::test]
async fn trailing_token_segments_are_rejected_and_empty_jkt_is_unbound() {
    let h = Harness::new().await;
    let session = h.issue("claims").await;
    assert!(
        h.auth
            .sessions
            .verify(&format!("{}.extra", session.token))
            .await
            .is_err()
    );
    let ticket = h
        .auth
        .sessions
        .issue_websocket_token(&session.session_id, None, None, None)
        .await
        .unwrap();
    assert!(
        h.auth
            .sessions
            .verify_websocket_token(&format!("{}.extra", ticket.ticket.0))
            .await
            .is_err()
    );
    let signer = PayloadSigner::from_store(&h.auth.secrets, "server-signing-key")
        .await
        .unwrap();
    for (token, ws) in [(session.token, false), (ticket.ticket.0, true)] {
        let body = token.split('.').next().unwrap();
        let mut claims: serde_json::Value = serde_json::from_slice(&unb64(body).unwrap()).unwrap();
        claims["jkt"] = json!("");
        let body = b64(serde_json::to_vec(&claims).unwrap());
        let token = format!("{body}.{}", signer.sign(&body));
        let result = if ws {
            h.auth.sessions.verify_websocket_token(&token).await
        } else {
            h.auth.sessions.verify(&token).await
        }
        .unwrap();
        assert!(result.proof_key_thumbprint.is_none());
    }
}

#[tokio::test]
async fn forwarded_host_is_not_trusted_by_default() {
    let h = Harness::new().await;
    let session = h.issue("origin").await;
    let mut request = cookie_request(&h.auth, &session.token);
    request
        .headers
        .insert("origin".into(), "https://evil.example".into());
    request
        .headers
        .insert("x-forwarded-host".into(), "evil.example".into());
    assert!(h.auth.authenticate_http(&request).await.is_err());
}

#[test]
fn every_dynamic_guard_rejects_wrong_shapes() {
    assert!(matches!(
        required_scope_for_device_list(&json!({"updateTool": ["hub"]})),
        Err(Error::InvalidPayload(_))
    ));
    for (method, payload) in [
        ("device.list", json!({"updateTool":["hub"]})),
        ("device.list", json!({"retryHostId":5})),
        (
            "git.runStackedAction",
            json!({"cwd":"/repo","actionId":"a","threadId":"t","action":["create_pr"]}),
        ),
        (
            "git.preparePullRequestThread",
            json!({"cwd":"/repo","reference":"1","mode":["worktree"],"threadId":"t"}),
        ),
        ("assets.createUrl", json!({"resource":{"_tag":"unknown"}})),
        (
            "server.updateSettings",
            json!({"patch":{},"providerInstanceMutation":[]}),
        ),
        (
            "projects.mutate",
            json!({"type":"project.update","scripts":5}),
        ),
    ] {
        assert!(
            matches!(
                authorize_rpc(STANDARD_SCOPES, method, &payload),
                Err(Error::InvalidPayload(_))
            ),
            "{method}"
        );
    }
}

#[test]
fn delegation_requires_access_write_and_every_delegated_scope() {
    assert!(
        validate_delegation(
            &[AuthEnvironmentScope::OrchestrationRead],
            &[AuthEnvironmentScope::OrchestrationRead]
        )
        .is_err()
    );
    assert!(
        validate_delegation(
            &[
                AuthEnvironmentScope::AccessWrite,
                AuthEnvironmentScope::OrchestrationRead
            ],
            &[AuthEnvironmentScope::FilesystemWrite]
        )
        .is_err()
    );
}

#[tokio::test]
async fn empty_keys_and_configured_secrets_return_typed_errors() {
    assert!(matches!(
        PayloadSigner::new(vec![]),
        Err(Error::EmptySecret)
    ));
    assert!(matches!(
        current_desktop_token("", NOW),
        Err(Error::EmptySecret)
    ));
    assert!(matches!(
        valid_desktop_token("", "forged", NOW),
        Err(Error::EmptySecret)
    ));
    let h = Harness::new().await;
    for config in [
        AuthConfig {
            desktop_bootstrap_secret: Some(String::new()),
            ..Default::default()
        },
        AuthConfig {
            desktop_bootstrap_token: Some(String::new()),
            ..Default::default()
        },
        AuthConfig {
            dev_auth_token: Some(String::new()),
            ..Default::default()
        },
    ] {
        assert!(matches!(
            t3_auth::PairingGrantStore::new(h.db.clone(), config.clone(), h.clock.clone()),
            Err(Error::EmptySecret)
        ));
        assert!(matches!(
            SessionStore::new(
                h.db.clone(),
                &h.auth.secrets,
                config.clone(),
                h.clock.clone()
            )
            .await,
            Err(Error::EmptySecret)
        ));
        assert!(matches!(
            EnvironmentAuth::new(
                h.db.clone(),
                h.auth.secrets.clone(),
                config,
                None,
                h.clock.clone()
            )
            .await,
            Err(Error::EmptySecret)
        ));
    }
    assert!(PayloadSigner::new(vec![1]).is_ok());
    assert!(current_desktop_token(" ", NOW).is_ok());
    assert!(valid_desktop_token(" ", &current_desktop_token(" ", NOW).unwrap(), NOW).unwrap());
}

#[tokio::test]
async fn pairing_issuance_validates_delegation_in_one_call_and_never_defaults_scopes() {
    use AuthEnvironmentScope as S;
    let h = Harness::new().await;
    for (caller, input) in [
        (
            vec![S::OrchestrationRead],
            pairing_input(Some(vec![S::OrchestrationRead]), None),
        ),
        (
            vec![S::AccessWrite, S::OrchestrationRead],
            pairing_input(Some(vec![S::FilesystemWrite]), None),
        ),
        (
            vec![S::AccessWrite, S::OrchestrationRead],
            pairing_input(None, Some(AuthMcpClientAccess::FullAccess)),
        ),
        (STANDARD_SCOPES.to_vec(), pairing_input(None, None)),
    ] {
        assert!(
            h.auth
                .issue_pairing_credential_for(&caller, input)
                .await
                .is_err()
        );
    }
    assert!(matches!(
        h.auth
            .pairing
            .issue_one_time_token(STANDARD_SCOPES, IssuePairing::default())
            .await,
        Err(Error::InvalidScope)
    ));
    let narrow = [S::AccessWrite, S::OrchestrationRead];
    let code = h
        .auth
        .issue_pairing_credential_for(
            &narrow,
            pairing_input(Some(vec![S::OrchestrationRead]), None),
        )
        .await
        .unwrap();
    let grant = h
        .auth
        .pairing
        .consume(&code.credential.0, ConsumePairing::default())
        .await
        .unwrap();
    assert_eq!(grant.scopes, vec![S::OrchestrationRead]);
    assert!(h.auth.list_pairing_links(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn live_peer_link_setting_is_observed_without_reconstructing_oauth() {
    let h = Harness::new().await;
    let (setting, changes) = tokio::sync::watch::channel(true);
    let oauth = McpOAuth::new(h.auth.clone(), changes).await.unwrap();
    let registration = || {
        serde_json::from_value(json!({"redirect_uris":["https://example.test/callback"],"software_id":"t3code-peer-link"})).unwrap()
    };
    assert!(oauth.register(registration()).is_ok());
    setting.send_replace(false);
    assert!(oauth.register(registration()).is_err());
    setting.send_replace(true);
    assert!(oauth.register(registration()).is_ok());
}

#[tokio::test]
async fn cookie_secure_attribute_tracks_https_for_normal_dev_and_expired_cookies() {
    for dev in [false, true] {
        let h = Harness::config(AuthConfig {
            dev_url: dev.then(|| "https://localhost:5173".into()),
            dev_auth_token: dev.then(|| "private-dev-token".into()),
            ..Default::default()
        })
        .await;
        for (protocol, forwarded, secure) in [
            ("http", None, false),
            ("https", None, true),
            ("http", Some("https"), true),
        ] {
            let mut request = Request {
                protocol: Some(protocol.into()),
                ..Default::default()
            };
            if let Some(value) = forwarded {
                request
                    .headers
                    .insert("X-Forwarded-Proto".into(), value.into());
            }
            let code = if dev {
                "private-dev-token".into()
            } else {
                h.auth
                    .startup_pairing_credential()
                    .await
                    .unwrap()
                    .credential
                    .0
            };
            let session = h
                .auth
                .create_browser_session(&request, &code, metadata(), None)
                .await
                .unwrap();
            assert!(session.cookies.iter().all(|cookie| cookie.secure == secure));
        }
    }
}

#[tokio::test]
async fn cross_process_revocation_wakes_live_waiter_before_expiry() {
    let h = Harness::new().await;
    let path = h.temp.path().join("state.sqlite");
    let other_db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
        .await
        .unwrap()
        .unwrap();
    let other = SessionStore::new(
        other_db,
        &h.auth.secrets,
        h.auth.config().clone(),
        h.clock.clone(),
    )
    .await
    .unwrap();
    let session = h.issue("socket").await;
    let invalidation = h
        .auth
        .sessions
        .watch_invalidation(&session.session_id, None)
        .await
        .unwrap();
    let mut waits = h.clock.waiting.subscribe();
    let waiting = tokio::spawn(invalidation.wait());
    waits.changed().await.unwrap();
    assert_eq!(
        *waits.borrow_and_update(),
        Some(NOW + 1000),
        "live sessions must recheck the shared database promptly"
    );
    other.revoke(&session.session_id).await.unwrap();
    h.clock.advance(1000);
    waiting.await.unwrap().unwrap();
}

#[tokio::test]
async fn bootstrap_credentials_are_checked_before_recording_proof_replay_state() {
    let h = Harness::config(AuthConfig {
        desktop_bootstrap_token: Some("desktop".into()),
        ..Default::default()
    })
    .await;
    let request = bootstrap_request(Some("key".into()));
    let markers = || {
        std::fs::read_dir(h.auth.secrets.directory())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("dpop-proof-")
            })
            .count()
    };
    assert!(
        h.auth
            .exchange_bootstrap(&request, "invalid", None, metadata())
            .await
            .is_err()
    );
    assert_eq!(markers(), 0);
    let issued = h
        .auth
        .exchange_bootstrap(&request, "desktop", None, metadata())
        .await
        .unwrap();
    assert_eq!(issued.token_type, AuthTokenType::DPoP);
    assert_eq!(markers(), 1);
    assert!(
        h.auth
            .exchange_bootstrap(&request, "desktop", None, metadata())
            .await
            .is_err()
    );
    assert_eq!(h.auth.sessions.list_active().await.unwrap().len(), 1);
}

#[tokio::test]
async fn duplicate_case_variants_remain_ambiguous_and_never_enable_dev_fallback() {
    let h = Harness::config(AuthConfig {
        dev_url: Some("http://localhost:5173".into()),
        dev_auth_token: Some("dev".into()),
        ..Default::default()
    })
    .await;
    let dev = h.auth.sessions.dev_auth().unwrap();
    let request = Request {
        cookies: [(dev.cookie_name.clone(), dev.credential.clone())].into(),
        headers: [
            ("Authorization".into(), "invalid".into()),
            ("aUtHoRiZaTiOn".into(), "".into()),
        ]
        .into(),
        ..Default::default()
    };
    assert!(matches!(
        h.auth.authenticate_http(&request).await,
        Err(Error::MissingCredential)
    ));
}
