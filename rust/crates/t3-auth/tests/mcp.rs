#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use serde_json::{Value, json};
use t3_auth::{
    Error, SessionStore,
    session::IssueSession,
    utils::{PayloadSigner, b64, unb64},
};
use t3_contracts::auth::{
    AuthEnvironmentScope as Scope, AuthMcpClientAccess as Access, ServerAuthSessionMethod as Method,
};

const RESOURCE: &str = "https://box.example/mcp";

async fn mint(h: &Harness, access: Access) -> t3_auth::session::IssuedSession {
    h.auth
        .issue_mcp_session(RESOURCE, "Client".into(), access, false, metadata())
        .await
        .unwrap()
}

async fn change_claim(h: &Harness, token: &str, key: &str, value: Option<Value>) -> String {
    let mut claims: Value =
        serde_json::from_slice(&unb64(token.split_once('.').unwrap().0).unwrap()).unwrap();
    match value {
        Some(value) => {
            claims[key] = value;
        }
        None => {
            claims.as_object_mut().unwrap().remove(key);
        }
    }
    let body = b64(serde_json::to_vec(&claims).unwrap());
    let signer = PayloadSigner::from_store(&h.auth.secrets, "server-signing-key")
        .await
        .unwrap();
    format!("{body}.{}", signer.sign(&body))
}

#[tokio::test]
async fn signed_mcp_claims_are_checked_independently() {
    let h = Harness::new().await;
    let issued = mint(&h, Access::Auto).await;
    let verified = h
        .auth
        .sessions
        .verify_mcp(&issued.token, RESOURCE, &[])
        .await
        .unwrap();
    assert_eq!(verified.oauth_audience.as_deref(), Some(RESOURCE));
    assert_eq!(verified.audience, None);
    for (key, value, label) in [
        ("aud", None, "missing audience"),
        (
            "aud",
            Some(json!("https://other.example/mcp")),
            "other origin",
        ),
        (
            "aud",
            Some(json!("https://box.example/other")),
            "other endpoint",
        ),
        (
            "aud",
            Some(json!([RESOURCE, "https://other.example/mcp"])),
            "multiple resources",
        ),
        (
            "resource",
            Some(json!("https://other.example/mcp")),
            "mismatched resource indicator",
        ),
        ("exp", Some(json!(NOW)), "signed expiry"),
        ("sub", Some(json!("browser")), "non-MCP subject"),
        (
            "method",
            Some(json!("browser-session-cookie")),
            "browser method",
        ),
        ("method", Some(json!("dpop-access-token")), "DPoP method"),
        ("kind", Some(json!("websocket")), "WebSocket token"),
        ("jkt", Some(json!("bound-key")), "proof-bound bearer"),
        ("sid", Some(json!("unknown-session")), "unknown session"),
    ] {
        let token = change_claim(&h, &issued.token, key, value).await;
        assert!(
            h.auth
                .sessions
                .verify_mcp(&token, RESOURCE, &[])
                .await
                .is_err(),
            "accepted {label}"
        );
    }
    let explicit_resource =
        change_claim(&h, &issued.token, "resource", Some(json!(RESOURCE))).await;
    assert!(
        h.auth
            .sessions
            .verify_mcp(&explicit_resource, RESOURCE, &[])
            .await
            .is_ok()
    );
    for resource in ["https://other.example/mcp", "https://box.example/other"] {
        assert!(
            h.auth
                .sessions
                .verify_mcp(&issued.token, resource, &[])
                .await
                .is_err()
        );
    }
    let tampered = format!("{}x", issued.token);
    assert!(
        h.auth
            .sessions
            .verify_mcp(&tampered, RESOURCE, &[])
            .await
            .is_err()
    );
    assert!(
        h.auth
            .sessions
            .verify_mcp(&issued.token, RESOURCE, &[])
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn persisted_mcp_state_is_checked_independently_of_signed_claims() {
    for (column, value, label) in [
        ("revoked_at", "2026-01-01T00:00:00.000Z", "revocation"),
        ("expires_at", "2026-01-01T00:00:00.000Z", "row expiry"),
        ("subject", "browser", "row subject"),
        ("method", "browser-session-cookie", "row method"),
        ("scopes", "[]", "missing read scope"),
    ] {
        let h = Harness::new().await;
        let issued = mint(&h, Access::Auto).await;
        let id = issued.session_id.0;
        h.db.writer(move |db| {
            db.execute(
                &format!("UPDATE auth_sessions SET {column}=?1 WHERE session_id=?2"),
                [value, &id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(
            h.auth
                .sessions
                .verify_mcp(&issued.token, RESOURCE, &[])
                .await
                .is_err(),
            "accepted {label}"
        );
    }
}

#[tokio::test]
async fn mcp_requires_each_requested_scope_from_the_live_row() {
    let h = Harness::new().await;
    let issued = mint(&h, Access::FullAccess).await;
    for scope in [
        Scope::OrchestrationOperate,
        Scope::SettingsWrite,
        Scope::SourceControlWrite,
    ] {
        assert!(
            h.auth
                .sessions
                .verify_mcp(&issued.token, RESOURCE, &[scope])
                .await
                .is_ok()
        );
        let id = issued.session_id.0.clone();
        let scopes: Vec<_> = t3_auth::authorization::mcp_scopes(Access::FullAccess)
            .into_iter()
            .filter(|s| *s != scope)
            .collect();
        let serialized = serde_json::to_string(&scopes).unwrap();
        h.db.writer(move |db| {
            db.execute(
                "UPDATE auth_sessions SET scopes=?1 WHERE session_id=?2",
                [serialized, id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(matches!(
            h.auth
                .sessions
                .verify_mcp(&issued.token, RESOURCE, &[scope])
                .await,
            Err(Error::ScopeNotGranted)
        ));
        let id = issued.session_id.0.clone();
        let serialized =
            serde_json::to_string(&t3_auth::authorization::mcp_scopes(Access::FullAccess)).unwrap();
        h.db.writer(move |db| {
            db.execute(
                "UPDATE auth_sessions SET scopes=?1 WHERE session_id=?2",
                [serialized, id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }
    let read_only = mint(&h, Access::ReadOnly).await;
    assert!(
        h.auth
            .sessions
            .verify_mcp(&read_only.token, RESOURCE, &[])
            .await
            .is_ok()
    );
    assert!(matches!(
        h.auth
            .sessions
            .verify_mcp(&read_only.token, RESOURCE, &[Scope::OrchestrationOperate])
            .await,
        Err(Error::ScopeNotGranted)
    ));
}

#[tokio::test]
async fn another_environments_key_cannot_authenticate_even_with_the_same_database() {
    let h = Harness::new().await;
    let issued = mint(&h, Access::Auto).await;
    let secrets = t3_auth::ServerSecretStore::open(h.temp.path().join("other-secrets"))
        .await
        .unwrap();
    let mut config = h.auth.config().clone();
    config.environment_id = "another-environment".into();
    let other = SessionStore::new(h.db.clone(), &secrets, config, h.clock.clone())
        .await
        .unwrap();
    assert!(other.get(&issued.session_id).await.unwrap().is_some());
    credential_failure(
        other.verify_mcp(&issued.token, RESOURCE, &[]).await,
        t3_auth::error::CredentialFailure::InvalidSessionTokenSignatureError,
    );
}

#[tokio::test]
async fn mcp_resource_configuration_is_canonical_and_errors_do_not_expose_input() {
    let h = Harness::new().await;
    let issued = h
        .auth
        .issue_mcp_session(
            "https://BOX.example:443/mcp",
            "Client".into(),
            Access::ReadOnly,
            false,
            metadata(),
        )
        .await
        .unwrap();
    assert!(
        h.auth
            .sessions
            .verify_mcp(&issued.token, RESOURCE, &[])
            .await
            .is_ok()
    );
    for resource in [
        "https://box.example/secret-endpoint",
        "https://box.example/mcp?secret-query",
        "https://box.example/mcp#secret-fragment",
        "https://secret-user:secret-password@box.example/mcp",
        "file:///secret-path",
    ] {
        let error = h
            .auth
            .issue_mcp_session(
                resource,
                "Client".into(),
                Access::ReadOnly,
                false,
                metadata(),
            )
            .await
            .err()
            .unwrap();
        assert!(!format!("{error:?} {error}").contains("secret-"));
        assert!(
            h.auth
                .sessions
                .verify_mcp(&issued.token, resource, &[])
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn mcp_rejects_audience_less_legacy_sessions_without_breaking_generic_verification() {
    let h = Harness::new().await;
    let legacy = h
        .auth
        .sessions
        .issue(IssueSession {
            subject: Some("mcp-client".into()),
            method: Some(Method::BearerAccessToken),
            scopes: Some(vec![Scope::OrchestrationRead]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(h.auth.sessions.verify(&legacy.token).await.is_ok());
    let mut request = token_request(&legacy.token);
    request.url = RESOURCE.into();
    assert!(h.auth.authenticate_mcp(&request).await.is_err());
}
