#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use serde_json::json;
use t3_auth::{
    AuthConfig, EnvironmentAuth, Error, authorization::STANDARD_SCOPES, clock::Clock,
    environment::*, pairing::*, session::IssueSession,
};
use t3_contracts::{auth::*, base_schemas::TrimmedNonEmptyString};

#[tokio::test]
async fn dev_cookie_selection_never_overrides_scoped_or_rejected_primary_credentials() {
    let h = Harness::config(AuthConfig {
        dev_url: Some("http://localhost:5173".into()),
        dev_auth_token: Some("dev-token".into()),
        ..Default::default()
    })
    .await;
    let dev = h
        .auth
        .create_browser_session(&Request::default(), "dev-token", metadata(), None)
        .await
        .unwrap();
    assert_eq!(dev.cookies.len(), 2);
    assert!(dev.cookies[0].name.starts_with("t3_dev_session_"));
    assert_eq!(dev.cookies[1].value, "");
    assert_eq!(dev.cookies[1].max_age_seconds, Some(0));
    assert!(dev.cookies[0].http_only);
    assert_eq!(dev.cookies[0].same_site, "lax");
    assert_eq!(dev.cookies[0].path, "/");
    assert!(!dev.cookies[0].secure);
    let p = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: vec![AuthEnvironmentScope::OrchestrationRead],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let scoped = h
        .auth
        .create_browser_session(&Request::default(), &p.credential.0, metadata(), None)
        .await
        .unwrap();
    assert_eq!(scoped.cookies.len(), 1);
    let mut request = cookie_request(&h.auth, &scoped.session_token);
    request
        .cookies
        .insert(dev.cookies[0].name.clone(), "dev-token".into());
    let verified = h.auth.authenticate_http(&request).await.unwrap();
    assert_eq!(
        verified.scopes,
        vec![AuthEnvironmentScope::OrchestrationRead]
    );
    h.auth.sessions.revoke(&verified.session_id).await.unwrap();
    assert!(h.auth.authenticate_http(&request).await.is_err());
    request.cookies.remove(&h.auth.sessions.cookie_name());
    assert_eq!(
        h.auth.authenticate_http(&request).await.unwrap().scopes,
        STANDARD_SCOPES
    );
    for header in [
        "Bearer invalid",
        "bearer invalid",
        "bEaReR invalid",
        "Basic invalid",
        "",
    ] {
        request
            .headers
            .insert("authorization".into(), header.into());
        assert!(h.auth.authenticate_http(&request).await.is_err());
    }
    request.headers.remove("authorization");
    assert!(h.auth.authenticate_http(&request).await.is_ok());
    let seed = h.auth.sessions.verify("dev-token").await.unwrap();
    let startup = h.auth.startup_pairing_credential().await.unwrap();
    assert_eq!(startup.credential.0, "dev-token");
    h.auth.sessions.revoke(&seed.session_id).await.unwrap();
    let recovery = h.auth.startup_pairing_credential().await.unwrap();
    assert_ne!(recovery.credential.0, "dev-token");
    assert!(
        h.auth
            .create_browser_session(
                &Request::default(),
                &recovery.credential.0,
                metadata(),
                None
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn primary_and_legacy_cookie_priority_and_origin_guards_apply_to_http_and_sockets() {
    let h = Harness::config(AuthConfig {
        host: Some("192.168.1.50".into()),
        trust_forwarded_host: true,
        dev_url: Some("http://localhost:5733".into()),
        ..Default::default()
    })
    .await;
    let bearer = h.auth.issue_session(None, None, None, None).await.unwrap();
    let mut request = token_request(&bearer.token);
    request.cookies.insert("t3_session".into(), "stale".into());
    request
        .cookies
        .insert(h.auth.sessions.cookie_name(), "".into());
    for scheme in ["Bearer", "bearer", "bEaReR"] {
        request
            .headers
            .insert("authorization".into(), format!("{scheme} {}", bearer.token));
        request
            .headers
            .insert("origin".into(), "http://other:8080".into());
        assert!(h.auth.authenticate_http(&request).await.is_ok());
        assert!(
            h.auth
                .authenticate_websocket(&request, AuthWebSocketTicketUse::Rpc)
                .await
                .is_ok()
        );
    }
    let p = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let browser = h
        .auth
        .create_browser_session(&Request::default(), &p.credential.0, metadata(), None)
        .await
        .unwrap();
    let request = cookie_request(&h.auth, &browser.session_token);
    for headers in [
        json!({"origin":"http://192.168.1.50:3773"}),
        json!({"origin":"https://box.tail1234.ts.net","x-forwarded-host":"box.tail1234.ts.net"}),
        json!({"origin":"t3code://app"}),
        json!({"origin":"t3code-dev://app"}),
        json!({"origin":"http://localhost:5733"}),
        json!({}),
        json!({"origin":"http://foreign:9999","sec-fetch-site":"same-origin"}),
    ] {
        let mut request = request.clone();
        for (k, v) in headers.as_object().unwrap() {
            request
                .headers
                .insert(k.clone(), v.as_str().unwrap().into());
        }
        assert!(h.auth.authenticate_http(&request).await.is_ok());
        assert!(
            h.auth
                .authenticate_websocket(&request, AuthWebSocketTicketUse::Rpc)
                .await
                .is_ok()
        );
    }
    for origin in ["http://192.168.1.50:8080", "null", "http://localhost:3000"] {
        let mut request = request.clone();
        request.headers.insert("origin".into(), origin.into());
        assert!(h.auth.authenticate_http(&request).await.is_err());
        assert!(
            h.auth
                .authenticate_websocket(&request, AuthWebSocketTicketUse::Rpc)
                .await
                .is_err()
        );
        assert!(!h.auth.session_state(&request).await.unwrap().authenticated);
    }
    let remote = Harness::config(AuthConfig {
        host: Some("0.0.0.0".into()),
        ..Default::default()
    })
    .await;
    let s = remote.issue("legacy").await;
    let mut req = cookie_request(&remote.auth, &s.token);
    req.cookies.remove(&remote.auth.sessions.cookie_name());
    req.cookies.insert("t3_session".into(), s.token);
    assert!(remote.auth.authenticate_http(&req).await.is_ok());
}

#[tokio::test]
async fn exchanges_narrow_scopes_preserve_rejected_codes_and_rotate_desktop_sessions_only() {
    let h = Harness::config(AuthConfig {
        desktop_bootstrap_token: Some("desktop".into()),
        ..Default::default()
    })
    .await;
    let p = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: vec![AuthEnvironmentScope::OrchestrationRead],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        h.auth
            .exchange_bootstrap(
                &bootstrap_request(None),
                &p.credential.0,
                Some(vec![AuthEnvironmentScope::AccessWrite]),
                metadata()
            )
            .await,
        Err(Error::ScopeNotGranted)
    ));
    assert_eq!(h.auth.list_pairing_links(None).await.unwrap().len(), 1);
    assert!(h.auth.sessions.list_active().await.unwrap().is_empty());
    let token = h
        .auth
        .exchange_bootstrap(
            &bootstrap_request(None),
            &p.credential.0,
            Some(vec![
                AuthEnvironmentScope::OrchestrationRead,
                AuthEnvironmentScope::AccessWrite,
                AuthEnvironmentScope::OrchestrationRead,
            ]),
            metadata(),
        )
        .await
        .unwrap();
    assert_eq!(token.scope.0, "orchestration:read");
    assert!(
        h.auth
            .exchange_bootstrap(&bootstrap_request(None), &p.credential.0, None, metadata())
            .await
            .is_err()
    );
    let browser = h
        .auth
        .create_browser_session(&Request::default(), "desktop", metadata(), None)
        .await
        .unwrap();
    let browser_id = h
        .auth
        .sessions
        .verify(&browser.session_token)
        .await
        .unwrap()
        .session_id;
    let first = h
        .auth
        .exchange_bootstrap(
            &bootstrap_request(None),
            "desktop",
            Some(vec![AuthEnvironmentScope::OrchestrationRead]),
            metadata(),
        )
        .await
        .unwrap();
    let second = h
        .auth
        .exchange_bootstrap(&bootstrap_request(None), "desktop", None, metadata())
        .await
        .unwrap();
    assert!(h.auth.sessions.verify(&first.access_token.0).await.is_err());
    assert!(h.auth.sessions.verify(&second.access_token.0).await.is_ok());
    assert!(
        h.auth
            .sessions
            .get(&browser_id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none()
    );
    assert!(h.auth.sessions.verify(&token.access_token.0).await.is_ok());
    let third = h
        .auth
        .exchange_bootstrap(
            &bootstrap_request(Some("key".into())),
            "desktop",
            None,
            metadata(),
        )
        .await
        .unwrap();
    assert_eq!(third.token_type, AuthTokenType::DPoP);
    let verified = h.auth.sessions.verify(&third.access_token.0).await.unwrap();
    assert_eq!(
        t3_auth::clock::epoch(&verified.expires_at).unwrap() - h.clock.now_ms(),
        3600000
    );
}

#[tokio::test]
async fn reusable_dev_exchanges_issue_independent_scoped_child_sessions() {
    let h = Harness::config(AuthConfig {
        dev_url: Some("http://localhost:1".into()),
        dev_auth_token: Some("dev".into()),
        ..Default::default()
    })
    .await;
    let mut tokens = Vec::new();
    for key in [None, None, Some("key".into())] {
        let t = h
            .auth
            .exchange_bootstrap(
                &bootstrap_request(key),
                "dev",
                Some(vec![AuthEnvironmentScope::OrchestrationRead]),
                metadata(),
            )
            .await
            .unwrap();
        assert_ne!(t.access_token.0, "dev");
        let session = h.auth.sessions.verify(&t.access_token.0).await.unwrap();
        assert_eq!(session.subject, "reusable-dev-token-child");
        tokens.push(t.access_token.0);
    }
    assert_ne!(tokens[0], tokens[1]);
    assert_eq!(
        h.auth.sessions.verify("dev").await.unwrap().subject,
        "reusable-dev-token"
    );
    let before = h.auth.sessions.list_active().await.unwrap();
    for scopes in [vec![], vec![AuthEnvironmentScope::ReviewWrite]] {
        assert!(matches!(
            h.auth
                .exchange_bootstrap(&bootstrap_request(None), "dev", Some(scopes), metadata())
                .await,
            Err(Error::ScopeNotGranted)
        ));
    }
    assert_eq!(h.auth.sessions.list_active().await.unwrap(), before);
}

#[tokio::test]
async fn startup_urls_hide_admin_subject_and_use_standard_scopes_and_dev_lifetime() {
    let h = Harness::new().await;
    let url = h
        .auth
        .startup_pairing_url("https://example.test/base?token=old&keep=1#old")
        .await
        .unwrap();
    let url = url::Url::parse(&url).unwrap();
    assert_eq!(url.path(), "/pair");
    assert_eq!(url.query(), Some("keep=1"));
    let token = url::form_urlencoded::parse(url.fragment().unwrap().as_bytes())
        .find(|(k, _)| k == "token")
        .unwrap()
        .1
        .into_owned();
    assert!(h.auth.list_pairing_links(None).await.unwrap().is_empty());
    let browser = h
        .auth
        .create_browser_session(&Request::default(), &token, metadata(), None)
        .await
        .unwrap();
    let s = h
        .auth
        .sessions
        .verify(&browser.session_token)
        .await
        .unwrap();
    assert_eq!(s.subject, "administrative-bootstrap");
    assert_eq!(s.scopes, STANDARD_SCOPES);
    let h = Harness::config(AuthConfig {
        dev_url: Some("http://localhost:1".into()),
        ..Default::default()
    })
    .await;
    let p = h.auth.startup_pairing_credential().await.unwrap();
    assert_eq!(
        t3_auth::clock::epoch(&p.expires_at).unwrap() - h.clock.now_ms(),
        86400000
    );
    let cli = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        t3_auth::clock::epoch(&cli.expires_at).unwrap() - h.clock.now_ms(),
        300000
    );
}

#[tokio::test]
async fn browser_repair_replaces_only_previous_browser_and_admin_lists_never_expose_tokens() {
    let h = Harness::new().await;
    let p = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let first = h
        .auth
        .create_browser_session(&Request::default(), &p.credential.0, metadata(), None)
        .await
        .unwrap();
    let previous = h.auth.sessions.verify(&first.session_token).await.unwrap();
    let unrelated = h.issue("one-time-token").await;
    let p = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: vec![AuthEnvironmentScope::OrchestrationRead],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let second = h
        .auth
        .create_browser_session(
            &Request::default(),
            &p.credential.0,
            metadata(),
            Some(&first.session_token),
        )
        .await
        .unwrap();
    assert!(h.auth.sessions.verify(&first.session_token).await.is_err());
    assert!(h.auth.sessions.verify(&unrelated.token).await.is_ok());
    assert!(h.auth.sessions.verify(&second.session_token).await.is_ok());
    let listed = h
        .auth
        .list_client_sessions(Some(&unrelated.session_id))
        .await
        .unwrap();
    assert!(listed.iter().any(|s| s.current));
    assert!(listed.iter().all(|s| s.permissions.is_some()));
    assert!(listed.iter().all(|s| s.session_id != previous.session_id));
    assert!(
        serde_json::to_value(listed)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("token").is_none())
    );
    let bot = h
        .auth
        .issue_session(None, None, Some("deploy-bot".into()), None)
        .await
        .unwrap();
    assert_eq!(bot.client.device_type, AuthClientMetadataDeviceType::Bot);
    assert_eq!(bot.client.label.unwrap().0, "deploy-bot");
}

#[tokio::test]
async fn admin_session_listing_prioritizes_access_managers_then_connections_then_issue_time() {
    let h = Harness::new().await;
    let admin = h.issue("admin").await;
    h.clock.advance(1);
    let reader = h
        .auth
        .sessions
        .issue(IssueSession {
            scopes: Some(vec![AuthEnvironmentScope::OrchestrationRead]),
            ..Default::default()
        })
        .await
        .unwrap();
    h.auth
        .sessions
        .mark_connected(&reader.session_id)
        .await
        .unwrap();
    assert_eq!(
        h.auth.list_client_sessions(None).await.unwrap()[0].session_id,
        admin.session_id
    );
}

#[tokio::test]
async fn managed_policy_refuses_pairing_and_revokes_only_disallowed_subjects_and_remote_startup_clients()
 {
    let h = Harness::new().await;
    let phone = h.issue("one-time-token").await;
    let connect = h.issue("cloud-connect").await;
    let bot = h.issue("cli-issued-session").await;
    let startup = |ip: &str| {
        let mut m = metadata();
        m.ip_address = Some(TrimmedNonEmptyString(ip.into()));
        IssueSession {
            subject: Some("administrative-bootstrap".into()),
            client: Some(m),
            ..Default::default()
        }
    };
    let remote = h
        .auth
        .sessions
        .issue(startup("192.168.1.20"))
        .await
        .unwrap();
    let local = h.auth.sessions.issue(startup("127.0.0.1")).await.unwrap();
    h.auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    h.auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                mcp_approval: Some(AuthMcpClientAccess::Auto),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let policy=serde_json::from_value(json!({"schemaVersion":1,"organization":"Example Corp","limits":{"clients":["desktop","local-browser"]}})).unwrap();
    let restricted = EnvironmentAuth::new(
        h.db.clone(),
        h.auth.secrets.clone(),
        h.auth.config().clone(),
        Some(policy),
        h.clock.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        restricted.initial_policy_revocations(),
        PolicyRevocations {
            sessions: 3,
            pairing_links: 2
        }
    );
    assert_eq!(
        restricted.enforce_policy().await.unwrap(),
        PolicyRevocations::default()
    );
    assert_eq!(
        restricted
            .create_pairing_link(
                t3_auth::authorization::STANDARD_SCOPES,
                IssuePairing {
                    scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                    ..Default::default()
                }
            )
            .await
            .err()
            .unwrap()
            .to_string(),
        "Example Corp's policy does not allow remote clients, so this server cannot create pairing links."
    );
    assert!(restricted.startup_pairing_credential().await.is_ok());
    for session in [phone, connect, remote] {
        assert!(h.auth.sessions.verify(&session.token).await.is_err());
    }
    for session in [bot, local] {
        assert!(h.auth.sessions.verify(&session.token).await.is_ok());
    }
    assert_eq!(h.auth.pairing.list_active().await.unwrap().len(), 1);
    let h = Harness::new().await;
    let phone = h.issue("one-time-token").await;
    let connect = h.issue("cloud-connect").await;
    let policy = serde_json::from_value(
        json!({"schemaVersion":1,"organization":"Example Corp","limits":{"connect":false}}),
    )
    .unwrap();
    EnvironmentAuth::new(
        h.db.clone(),
        h.auth.secrets.clone(),
        h.auth.config().clone(),
        Some(policy),
        h.clock.clone(),
    )
    .await
    .unwrap();
    assert!(h.auth.sessions.verify(&phone.token).await.is_ok());
    assert!(h.auth.sessions.verify(&connect.token).await.is_err());
}

#[test]
fn oauth_scope_parser_filters_retired_unknown_permissions_and_dpop_challenges() {
    assert!(parse_requested_scopes("review:write").is_err());
    assert!(parse_requested_scopes("orchestration:read  access:write").is_err());
    assert_eq!(
        parse_requested_scopes("orchestration:read unknown orchestration:read").unwrap(),
        vec![AuthEnvironmentScope::OrchestrationRead]
    );
    for scheme in ["DPoP", "dpop"] {
        let mut request = token_request("bad");
        request
            .headers
            .insert("authorization".into(), format!("{scheme} bad"));
        assert!(dpop_challenge(&request));
    }
    assert_eq!(
        CREDENTIAL_RESPONSE_HEADERS,
        [("cache-control", "no-store"), ("pragma", "no-cache")]
    );
}
