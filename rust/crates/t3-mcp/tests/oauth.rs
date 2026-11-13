#![cfg(feature = "auth")]
#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{HeaderMap, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_auth::{
    AuthConfig, EnvironmentAuth, ServerSecretStore, clock::TokioClock, session::IssueSession,
};
use t3_contracts::{
    auth::*,
    base_schemas::{EnvironmentId, ThreadId},
};
use t3_mcp::{oauth::OAuthAuthenticator, transport::*, *};
use tower::ServiceExt;

struct Store;
#[async_trait]
impl AccessStore for Store {
    async fn thread_shell(&self, _: &ThreadId) -> Result<Option<ThreadShell>> {
        panic!("refused call reached services")
    }
}
struct Harness {
    auth: EnvironmentAuth,
    _temp: tempfile::TempDir,
}
impl Harness {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        let db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
            .await
            .unwrap()
            .unwrap();
        let secrets = ServerSecretStore::open(temp.path().join("secrets"))
            .await
            .unwrap();
        let config = AuthConfig {
            environment_id: "environment".into(),
            state_dir: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let auth =
            EnvironmentAuth::new(db, secrets, config, None, TokioClock::at(1_767_225_600_000))
                .await
                .unwrap();
        Self { auth, _temp: temp }
    }
    fn verifier(&self) -> OAuthAuthenticator {
        OAuthAuthenticator::new(self.auth.clone(), "https://example.test/mcp").unwrap()
    }
    async fn issue(&self, scopes: Vec<AuthEnvironmentScope>) -> t3_auth::session::IssuedSession {
        self.auth
            .sessions
            .issue(IssueSession {
                subject: Some("mcp-client".into()),
                method: Some(ServerAuthSessionMethod::BearerAccessToken),
                scopes: Some(scopes),
                ..Default::default()
            })
            .await
            .unwrap()
    }
    async fn verify(&self, token: &str, url: &str) -> Option<AuthenticatedClient> {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        headers.insert("host", "example.test".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        self.verifier()
            .authenticate(&headers, &url.parse().unwrap())
            .await
    }
    async fn with_claim(&self, token: &str, key: &str, value: Value) -> String {
        let mut claims: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(token.split_once('.').unwrap().0)
                .unwrap(),
        )
        .unwrap();
        claims[key] = value;
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let signer =
            t3_auth::utils::PayloadSigner::from_store(&self.auth.secrets, "server-signing-key")
                .await
                .unwrap();
        format!("{body}.{}", signer.sign(&body))
    }
}

#[tokio::test(start_paused = true)]
async fn oauth_sessions_validate_signature_recipient_resource_expiry_and_revocation() {
    let h = Harness::new().await;
    let issued = h
        .auth
        .issue_mcp_session(
            "https://example.test/mcp",
            "MCP client".into(),
            AuthMcpClientAccess::FullAccess,
            false,
            t3_auth::utils::default_metadata(),
        )
        .await
        .unwrap();
    assert!(h.verify(&issued.token, "/mcp").await.is_some());
    let legacy = h.issue(vec![AuthEnvironmentScope::OrchestrationRead]).await;
    assert!(h.auth.sessions.verify(&legacy.token).await.is_ok());
    assert!(
        h.verify(&legacy.token, "/mcp").await.is_none(),
        "Generic sessions without a signed audience must be refused."
    );
    let token = h
        .with_claim(&issued.token, "resource", json!("https://example.test/mcp"))
        .await;
    let verified = h.verify(&token, "/mcp").await.unwrap();
    assert_eq!(
        verified.scope.client.unwrap().access,
        ClientAccess::FullAccess
    );
    for (key, value) in [
        ("aud", json!("https://other.test/mcp")),
        ("resource", json!("https://example.test/other")),
        ("exp", json!(1_767_225_600_000.0)),
        ("sub", json!("ordinary-user")),
        ("method", json!("browser-session-cookie")),
        ("kind", json!("websocket")),
        ("jkt", json!("bound-key")),
        ("sid", json!("unknown-session")),
        ("method", json!("dpop-access-token")),
    ] {
        let token = h.with_claim(&token, key, value).await;
        assert!(h.verify(&token, "/mcp").await.is_none(), "{key}");
    }
    assert!(h.verify(&token, "/other").await.is_none());
    let mut tampered = token.clone();
    tampered.push('x');
    assert!(h.verify(&tampered, "/mcp").await.is_none());
    assert!(Harness::new().await.verify(&token, "/mcp").await.is_none());
    let array = h
        .with_claim(&token, "aud", json!(["https://example.test/mcp"]))
        .await;
    assert!(h.verify(&array, "/mcp").await.is_none());
    h.auth.sessions.revoke(&issued.session_id).await.unwrap();
    assert!(h.verify(&token, "/mcp").await.is_none());
    let short = h
        .auth
        .sessions
        .issue(IssueSession {
            ttl_ms: Some(10),
            subject: Some("mcp-client".into()),
            method: Some(ServerAuthSessionMethod::BearerAccessToken),
            scopes: Some(vec![AuthEnvironmentScope::OrchestrationRead]),
            ..Default::default()
        })
        .await
        .unwrap();
    let short = h
        .with_claim(&short.token, "aud", json!("https://example.test/mcp"))
        .await;
    let longer_claim = h
        .with_claim(&short, "exp", json!(1_767_225_600_000.0 + 1_000_000.0))
        .await;
    assert!(h.verify(&short, "/mcp").await.is_some());
    assert!(h.verify(&longer_claim, "/mcp").await.is_some());
    tokio::time::advance(std::time::Duration::from_millis(10)).await;
    assert!(h.verify(&short, "/mcp").await.is_none());
    assert!(
        h.verify(&longer_claim, "/mcp").await.is_none(),
        "Persisted expiry must also be checked."
    );
}

async fn call(
    router: axum::Router,
    name: &str,
    token: Option<&str>,
    input: Value,
) -> (http::StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "example.test")
        .header("x-forwarded-proto", "https")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-method", "tools/call")
        .header("mcp-name", name)
        .header("mcp-protocol-version", "2026-07-28");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response=router.oneshot(request.body(axum::body::Body::from(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":input,"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"test","version":"1"}
    }}}).to_string())).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn every_tool_family_refuses_unauthenticated_and_wrong_scope_calls_before_services() {
    let h = Harness::new().await;
    let token = h.issue(vec![AuthEnvironmentScope::SettingsWrite]).await;
    let token = h
        .with_claim(&token.token, "aud", json!("https://example.test/mcp"))
        .await;
    let token = h
        .with_claim(
            &token,
            "scopes",
            json!([
                "orchestration:read",
                "orchestration:operate",
                "settings:write",
                "source-control:write"
            ]),
        )
        .await;
    assert!(
        h.verify(&token, "/mcp").await.is_none(),
        "Signed scope claims cannot supply the missing live read grant."
    );
    let router = router(
        Arc::new(McpServer::new(Services::default(), Arc::new(Store))),
        Arc::new(HttpState {
            registry: Arc::new(SessionRegistry::new(
                EnvironmentId("environment".into()),
                None,
            )),
            clients: Some(Arc::new(h.verifier())),
            linked_environments_enabled: std::sync::atomic::AtomicBool::new(true),
            runtime_ceiling: None,
        }),
    );
    let witnesses: Value = serde_json::from_str(include_str!("../fixtures/decoders.json")).unwrap();
    for tool in &t3_mcp::catalog::catalog().tools {
        let input = witnesses[&tool.name][0]["input"].clone();
        let (status, _) = call(router.clone(), &tool.name, None, input.clone()).await;
        assert_eq!(status, http::StatusCode::UNAUTHORIZED, "{}", tool.name);
        let (status, value) = call(router.clone(), &tool.name, Some(&token), input).await;
        assert_eq!(
            status,
            http::StatusCode::UNAUTHORIZED,
            "{} {value}",
            tool.name
        );
    }
    for (name, input, missing) in [
        (
            "t3_queue",
            json!({"action":"edit"}),
            AuthEnvironmentScope::OrchestrationOperate,
        ),
        (
            "t3_environment_preferences_update",
            json!({}),
            AuthEnvironmentScope::SettingsWrite,
        ),
        (
            "t3_worktree_handoff",
            json!({}),
            AuthEnvironmentScope::SourceControlWrite,
        ),
    ] {
        let granted = [
            AuthEnvironmentScope::OrchestrationRead,
            AuthEnvironmentScope::OrchestrationOperate,
            AuthEnvironmentScope::SettingsWrite,
            AuthEnvironmentScope::SourceControlWrite,
        ]
        .into_iter()
        .filter(|s| *s != missing)
        .collect::<Vec<_>>();
        assert!(t3_mcp::authorization::authorize_tool(&granted, name, &input).is_err());
        let issued = h.issue(granted).await;
        let token = h
            .with_claim(&issued.token, "aud", json!("https://example.test/mcp"))
            .await;
        assert!(h.verify(&token, "/mcp").await.is_some());
        let (status, value) = call(router.clone(), name, Some(&token), input).await;
        assert_eq!(status, http::StatusCode::OK, "{name}: {value}");
        assert_eq!(value["result"]["isError"], true, "{name}: {value}");
        assert!(
            value["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("missing required scope"),
            "{name}: {value}"
        );
    }
    assert!(t3_mcp::authorization::required_scopes("unknown", &json!({})).is_err());
}
