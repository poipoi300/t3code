#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use serde_json::json;
use t3_auth::{
    Error,
    authorization::{STANDARD_SCOPES, mcp_scopes},
    oauth::*,
    pairing::*,
};
use t3_contracts::auth::*;
const ORIGIN: &str = "https://box.example.ts.net";
const REDIRECT: &str = "http://localhost:51234/callback";
fn urls() -> OAuthUrls {
    OAuthUrls {
        issuer: ORIGIN.into(),
        resource: format!("{ORIGIN}/mcp"),
    }
}
fn registration(uri: &str, peer: bool) -> AuthMcpClientRegistration {
    serde_json::from_value(json!({"client_name":"Claude Code","redirect_uris":[uri],"token_endpoint_auth_method":"client_secret_post","software_id":if peer {PEER_LINK_SOFTWARE_ID} else {"other"}})).unwrap()
}
fn authorization(client: &McpOAuthClient, redirect: &str) -> AuthMcpAuthorizationRequest {
    serde_json::from_value(json!({"client_id":client.client_id,"redirect_uri":redirect,"response_type":"code","code_challenge":t3_auth::utils::b64(t3_auth::utils::sha256("a".repeat(64))),"code_challenge_method":"S256","state":"state-1","resource":format!("{ORIGIN}/mcp")})).unwrap()
}
fn token(
    client: &McpOAuthClient,
    code: &str,
    redirect: &str,
    verifier: &str,
) -> AuthMcpTokenRequest {
    serde_json::from_value(json!({"grant_type":"authorization_code","code":code,"client_id":client.client_id,"redirect_uri":redirect,"code_verifier":verifier})).unwrap()
}
fn decision(code: &str, access: AuthMcpClientAccess) -> AuthMcpApprovalDecision {
    serde_json::from_value(json!({"_tag":"pairing-code","code":code,"access":access})).unwrap()
}
fn callback_code(uri: &str) -> String {
    url::Url::parse(uri)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned()
}
fn access_levels() -> [AuthMcpClientAccess; 5] {
    [
        AuthMcpClientAccess::ReadOnly,
        AuthMcpClientAccess::ApprovalRequired,
        AuthMcpClientAccess::AutoAcceptEdits,
        AuthMcpClientAccess::Auto,
        AuthMcpClientAccess::FullAccess,
    ]
}

#[test]
fn redirects_and_resource_matching_follow_oauth_loopback_rules() {
    for (left, right, result) in [
        (
            "http://localhost/callback",
            "http://localhost:9/callback",
            true,
        ),
        (
            "http://127.0.0.1:1/callback",
            "http://127.0.0.1:2/callback",
            true,
        ),
        ("http://[::1]:1/callback", "http://[::1]:2/callback", true),
        (
            "http://localhost/callback",
            "http://127.0.0.1/callback",
            false,
        ),
        (
            "http://localhost/callback",
            "https://localhost/callback",
            false,
        ),
        (
            "https://example.test/cb",
            "https://example.test:443/cb",
            true,
        ),
        (
            "https://example.test/cb",
            "https://example.test:9/cb",
            false,
        ),
        (
            "http://localhost/cb?q=1",
            "http://localhost:9/cb?q=2",
            false,
        ),
        ("https://x.test/cb#", "https://x.test/cb", false),
    ] {
        assert_eq!(redirect_matches(left, right), result, "{left} {right}");
    }
    assert!(same_resource(&urls(), None));
    assert!(same_resource(
        &urls(),
        Some(&format!("{ORIGIN}/mcp/?query=x"))
    ));
    assert!(!same_resource(&urls(), Some(&format!("{ORIGIN}/other"))));
}

#[tokio::test]
async fn discovery_registration_validation_and_stateless_client_ids() {
    let h = Harness::new().await;
    let oauth = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
        .await
        .unwrap();
    let mut request = token_request("unused");
    request.url = "/mcp".into();
    request
        .headers
        .insert("x-forwarded-proto".into(), "https".into());
    let derived = OAuthUrls::from_request(&request);
    assert_eq!(derived.resource, "https://192.168.1.50:3773/mcp");
    assert_eq!(
        protected_resource_metadata(&derived).unwrap().resource,
        derived.resource
    );
    assert_eq!(
        authorization_server_metadata(&derived).unwrap().issuer,
        derived.issuer
    );
    for uri in [
        "http://bot.example/callback",
        "https://user:pass@bot.example/callback",
        "https://bot.example/callback#",
        "http://localhost/callback#",
        "javascript:alert(1)",
    ] {
        assert!(matches!(
            oauth.register(registration(uri, false)),
            Err(Error::OAuth {
                error: "invalid_redirect_uri",
                ..
            })
        ));
    }
    let client = oauth.register(registration(REDIRECT, false)).unwrap();
    assert_eq!(
        client.registration().unwrap().token_endpoint_auth_method,
        AuthNoneMethod::None
    );
    assert!(oauth.parse_client_id(&client.client_id).is_some());
    let restarted = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
        .await
        .unwrap();
    assert!(restarted.parse_client_id(&client.client_id).is_some());
    let mut bad = authorization(&client, REDIRECT);
    bad.client_id = Some(format!(
        "{}.forged",
        client.client_id.split('.').next().unwrap()
    ));
    assert!(matches!(
        oauth.validate_authorization(&urls(), bad),
        Err(AuthorizationError::Page(_))
    ));
    assert!(matches!(
        oauth.validate_authorization(
            &urls(),
            authorization(&client, "http://localhost/elsewhere")
        ),
        Err(AuthorizationError::Page(_))
    ));
    let mut bad = authorization(&client, REDIRECT);
    bad.response_type = Some("token".into());
    let error = oauth.validate_authorization(&urls(), bad).err().unwrap();
    assert!(
        error
            .redirect(ORIGIN)
            .unwrap()
            .contains("unsupported_response_type")
    );
    let mut bad = authorization(&client, REDIRECT);
    bad.code_challenge_method = Some("plain".into());
    assert!(matches!(
        oauth.validate_authorization(&urls(), bad),
        Err(AuthorizationError::Redirect {
            error: "invalid_request",
            ..
        })
    ));
    let mut bad = authorization(&client, REDIRECT);
    bad.resource = Some("https://other/mcp".into());
    assert!(matches!(
        oauth.validate_authorization(&urls(), bad),
        Err(AuthorizationError::Redirect {
            error: "invalid_target",
            ..
        })
    ));
    let disabled = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(false).1)
        .await
        .unwrap();
    assert!(disabled.register(registration(REDIRECT, true)).is_err());
    assert!(
        oauth
            .register(registration(REDIRECT, true))
            .unwrap()
            .peer_link
    );
}

#[tokio::test]
async fn oauth_pkce_exchange_limits_tokens_to_mcp_and_preserves_access_and_peer_flags() {
    for (uri, peer) in [
        (REDIRECT, false),
        ("https://bot.example/oauth/callback", false),
        (REDIRECT, true),
    ] {
        let h = Harness::new().await;
        let oauth = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
            .await
            .unwrap();
        let client = oauth.register(registration(uri, peer)).unwrap();
        let authorization = oauth
            .validate_authorization(&urls(), authorization(&client, uri))
            .unwrap();
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
        let approved = oauth
            .approve(
                &token_request("unused"),
                &authorization,
                decision(&p.credential.0, AuthMcpClientAccess::Auto),
            )
            .await
            .unwrap();
        assert_eq!(
            approved.granted,
            Some(ApprovalGrant {
                access: AuthMcpClientAccess::Auto,
                method: ApprovalMethod::PairingCode
            })
        );
        let url = url::Url::parse(&approved.redirect).unwrap();
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "state" && v == "state-1")
        );
        assert!(url.query_pairs().any(|(k, v)| k == "iss" && v == ORIGIN));
        let code = callback_code(&approved.redirect);
        let result = oauth
            .exchange_code(
                &token_request("unused"),
                &urls(),
                token(&client, &code, uri, &"a".repeat(64)),
            )
            .await
            .unwrap();
        let claims: serde_json::Value = serde_json::from_slice(
            &t3_auth::utils::unb64(result.access_token.split_once('.').unwrap().0).unwrap(),
        )
        .unwrap();
        assert_eq!(claims["aud"].as_str(), Some(urls().resource.as_str()));
        assert!(
            oauth
                .exchange_code(
                    &token_request("unused"),
                    &urls(),
                    token(&client, &code, uri, &"a".repeat(64))
                )
                .await
                .is_err()
        );
        for scheme in ["Bearer", "bearer", "bEaReR"] {
            let mut request = token_request(&result.access_token);
            request.url = urls().resource;
            request.headers.insert(
                "authorization".into(),
                format!("{scheme} {}", result.access_token),
            );
            let signed = h.auth.authenticate_mcp(&request).await.unwrap();
            assert_eq!(signed.access, AuthMcpClientAccess::Auto);
            assert_eq!(signed.peer_link, peer);
            assert_eq!(signed.label, "Claude Code");
            assert!(!h.auth.session_state(&request).await.unwrap().authenticated);
            assert!(
                h.auth
                    .authenticate_websocket(&request, AuthWebSocketTicketUse::Rpc)
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_pkce_expired_and_raced_codes_are_spent_exactly_once() {
    let h = Harness::new().await;
    let oauth = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
        .await
        .unwrap();
    let client = oauth.register(registration(REDIRECT, false)).unwrap();
    let authz = oauth
        .validate_authorization(&urls(), authorization(&client, REDIRECT))
        .unwrap();
    for failure in [
        "pkce", "expired", "redirect", "resource", "client", "origin", "race",
    ] {
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
        let approved = oauth
            .approve(
                &token_request("unused"),
                &authz,
                decision(&p.credential.0, AuthMcpClientAccess::ApprovalRequired),
            )
            .await
            .unwrap();
        let code = callback_code(&approved.redirect);
        if failure == "race" {
            let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let mut tasks = Vec::new();
            for _ in 0..2 {
                let oauth = oauth.clone();
                let barrier = barrier.clone();
                let input = token(&client, &code, REDIRECT, &"a".repeat(64));
                tasks.push(tokio::spawn(async move {
                    barrier.wait().await;
                    oauth
                        .exchange_code(&token_request("unused"), &urls(), input)
                        .await
                }));
            }
            let b = tasks.pop().unwrap().await.unwrap();
            let a = tasks.pop().unwrap().await.unwrap();
            assert_ne!(a.is_ok(), b.is_ok());
        } else {
            let mut input = token(&client, &code, REDIRECT, &"a".repeat(64));
            let mut exchange_urls = urls();
            match failure {
                "pkce" => input.code_verifier = Some("b".repeat(64)),
                "expired" => h.clock.advance(60000),
                "redirect" => input.redirect_uri = Some("http://localhost:9999/callback".into()),
                "resource" => input.resource = Some("https://other/mcp".into()),
                "client" => {
                    let mut other = registration(REDIRECT, false);
                    other.client_name = Some("Another client".into());
                    input.client_id = Some(oauth.register(other).unwrap().client_id);
                }
                "origin" => exchange_urls.resource = "https://other/mcp".into(),
                _ => panic!(),
            }
            assert!(
                oauth
                    .exchange_code(&token_request("unused"), &exchange_urls, input)
                    .await
                    .is_err(),
                "accepted {failure}"
            );
        }
        assert!(
            oauth
                .exchange_code(
                    &token_request("unused"),
                    &urls(),
                    token(&client, &code, REDIRECT, &"a".repeat(64))
                )
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn approval_codes_grant_only_their_own_app_level_and_rejected_scopes_allow_one_lower_retry() {
    let h = Harness::new().await;
    let oauth = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
        .await
        .unwrap();
    let client = oauth.register(registration(REDIRECT, false)).unwrap();
    let authz = oauth
        .validate_authorization(&urls(), authorization(&client, REDIRECT))
        .unwrap();
    for access in access_levels() {
        let p = h
            .auth
            .create_pairing_link(
                t3_auth::authorization::STANDARD_SCOPES,
                IssuePairing {
                    scopes: mcp_scopes(access),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            oauth
                .approve(
                    &token_request("unused"),
                    &authz,
                    decision(&p.credential.0, access)
                )
                .await
                .is_ok()
        );
    }
    let p = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: mcp_scopes(AuthMcpClientAccess::ReadOnly),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        h.auth
            .consume_mcp_approval_code(&p.credential.0, AuthMcpClientAccess::FullAccess)
            .await,
        Err(Error::McpApprovalCode {
            reason: t3_auth::error::McpApprovalReason::InsufficientScope
        })
    ));
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let auth = h.auth.clone();
        let code = p.credential.0.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            auth.consume_mcp_approval_code(&code, AuthMcpClientAccess::ReadOnly)
                .await
        }));
    }
    let b = tasks.pop().unwrap().await.unwrap();
    let a = tasks.pop().unwrap().await.unwrap();
    assert_ne!(a.is_ok(), b.is_ok());
    let p = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                mcp_approval: Some(AuthMcpClientAccess::ApprovalRequired),
                scopes: mcp_scopes(AuthMcpClientAccess::ApprovalRequired),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        oauth
            .approve(
                &token_request("unused"),
                &authz,
                decision(&p.credential.0, AuthMcpClientAccess::Auto)
            )
            .await
            .is_err()
    );
    let peer = oauth.register(registration(REDIRECT, true)).unwrap();
    let peer_authz = oauth
        .validate_authorization(&urls(), authorization(&peer, REDIRECT))
        .unwrap();
    assert!(
        oauth
            .approve(
                &token_request("unused"),
                &peer_authz,
                decision(&p.credential.0, AuthMcpClientAccess::ApprovalRequired)
            )
            .await
            .is_err()
    );
    assert!(
        oauth
            .approve(
                &token_request("unused"),
                &authz,
                decision(
                    &p.credential.0.to_lowercase(),
                    AuthMcpClientAccess::ApprovalRequired
                )
            )
            .await
            .is_ok()
    );
    assert!(
        oauth
            .approve(
                &token_request("unused"),
                &authz,
                decision(&p.credential.0, AuthMcpClientAccess::ApprovalRequired)
            )
            .await
            .is_err()
    );
    let denied = oauth.deny(&authz).unwrap();
    assert!(denied.contains("access_denied"));
    let bound = h
        .auth
        .create_pairing_link(
            t3_auth::authorization::STANDARD_SCOPES,
            IssuePairing {
                scopes: t3_auth::authorization::STANDARD_SCOPES.to_vec(),
                proof_key_thumbprint: Some(bootstrap_thumbprint()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        oauth
            .approve(
                &token_request("unused"),
                &authz,
                decision(&bound.credential.0, AuthMcpClientAccess::Auto)
            )
            .await
            .is_err()
    );
    assert!(
        h.auth
            .exchange_bootstrap(
                &bootstrap_request(Some("key".into())),
                &bound.credential.0,
                None,
                metadata()
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn csrf_approvals_require_held_scopes_and_bind_to_holder_and_authorization() {
    let h = Harness::new().await;
    let oauth = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
        .await
        .unwrap();
    let client = oauth.register(registration(REDIRECT, false)).unwrap();
    let authz = oauth
        .validate_authorization(&urls(), authorization(&client, REDIRECT))
        .unwrap();
    for scopes in [
        STANDARD_SCOPES.to_vec(),
        vec![
            AuthEnvironmentScope::AccessWrite,
            AuthEnvironmentScope::OrchestrationRead,
        ],
        vec![
            AuthEnvironmentScope::AccessRead,
            AuthEnvironmentScope::AccessWrite,
        ],
    ] {
        let session = h
            .auth
            .issue_session(None, Some(scopes.clone()), None, None)
            .await
            .unwrap();
        let request = token_request(&session.token);
        let approving = oauth.approving_session(&request, &authz).await;
        assert_eq!(
            approving.is_some(),
            scopes.contains(&AuthEnvironmentScope::OrchestrationRead)
        );
        for access in access_levels() {
            let csrf = approving
                .as_ref()
                .map(|s| s.csrf_token.as_str())
                .unwrap_or("forged");
            let decision = serde_json::from_value(
                json!({"_tag":"browser-session","access":access,"csrfToken":csrf}),
            )
            .unwrap();
            let allowed = mcp_scopes(access).iter().all(|s| scopes.contains(s));
            assert_eq!(
                oauth.approve(&request, &authz, decision).await.is_ok(),
                allowed
            );
        }
        let decision = serde_json::from_value(
            json!({"_tag":"browser-session","access":"read-only","csrfToken":"forged"}),
        )
        .unwrap();
        assert!(oauth.approve(&request, &authz, decision).await.is_err());
        if let Some(approving) = approving {
            let other = h.auth.issue_session(None, None, None, None).await.unwrap();
            let decision=serde_json::from_value(json!({"_tag":"browser-session","access":"read-only","csrfToken":approving.csrf_token})).unwrap();
            assert!(
                oauth
                    .approve(&token_request(&other.token), &authz, decision)
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn failed_mcp_session_issuance_retains_its_internal_cause() {
    let h = Harness::new().await;
    let oauth = McpOAuth::new(h.auth.clone(), tokio::sync::watch::channel(true).1)
        .await
        .unwrap();
    let client = oauth.register(registration(REDIRECT, false)).unwrap();
    let authz = oauth
        .validate_authorization(&urls(), authorization(&client, REDIRECT))
        .unwrap();
    let p = h
        .auth
        .create_pairing_link(
            STANDARD_SCOPES,
            IssuePairing {
                scopes: STANDARD_SCOPES.to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let approved = oauth
        .approve(
            &token_request("unused"),
            &authz,
            decision(&p.credential.0, AuthMcpClientAccess::Auto),
        )
        .await
        .unwrap();
    h.db.writer(|db| { db.execute_batch("CREATE TRIGGER reject_mcp_session BEFORE INSERT ON auth_sessions BEGIN SELECT RAISE(ABORT,'mcp insert failure'); END")?; Ok(()) }).await.unwrap();
    let result = oauth
        .exchange_code(
            &token_request("unused"),
            &urls(),
            token(
                &client,
                &callback_code(&approved.redirect),
                REDIRECT,
                &"a".repeat(64),
            ),
        )
        .await;
    assert!(
        matches!(result, Err(Error::OAuthSessionIssue { source }) if matches!(*source, Error::Persistence(_)))
    );
    assert!(!format!("{approved:?}").contains(&callback_code(&approved.redirect)));
}
