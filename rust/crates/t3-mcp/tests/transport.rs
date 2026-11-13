#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use axum::{Router, body::Body};
use http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};
use t3_contracts::{base_schemas::ThreadId, provider_policy::RuntimeMode};
use t3_mcp::{services::EnvironmentService, transport::*, *};
use tower::ServiceExt;

struct Store;
#[async_trait]
impl AccessStore for Store {
    async fn thread_shell(&self, _: &ThreadId) -> Result<Option<ThreadShell>> {
        Ok(None)
    }
}

struct Echo;
#[async_trait]
impl EnvironmentService for Echo {
    async fn t3_environment_read(&self, context: CallContext, _: DecodedInput) -> Result<Value> {
        Ok(json!({"scope":context.scope,"limits":context.limits}))
    }
}

struct Clients;
#[async_trait]
impl ClientAuthenticator for Clients {
    async fn authenticate(
        &self,
        headers: &HeaderMap,
        _: &http::Uri,
    ) -> Option<AuthenticatedClient> {
        let token = bearer(headers)?;
        if !matches!(token, "client.signature" | "linked.signature") {
            return None;
        }
        Some(AuthenticatedClient {
            scopes: vec![t3_contracts::auth::AuthEnvironmentScope::OrchestrationRead],
            scope: serde_json::from_value(json!({
                "environmentId":"environment", "requestNamespace":"client", "issuedAt":0,
                "capabilities":["orchestration"], "client":{
                    "sessionId":"client", "label":"Client", "access":"auto",
                    "linked":token == "linked.signature"
                }
            }))
            .unwrap(),
        })
    }
}

fn app(links: bool) -> Router {
    router(
        Arc::new(McpServer::new(
            Services {
                environment: Arc::new(Echo),
                ..Services::default()
            },
            Arc::new(Store),
        )),
        Arc::new(HttpState {
            registry: Arc::new(SessionRegistry::new(
                t3_contracts::base_schemas::EnvironmentId("environment".into()),
                None,
            )),
            clients: Some(Arc::new(Clients)),
            linked_environments_enabled: AtomicBool::new(links),
            runtime_ceiling: Some(RuntimeMode::AutoAcceptEdits),
        }),
    )
}

async fn call(app: Router, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "example.test")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-method", "tools/call")
        .header("mcp-name", "t3_environment_read")
        .header("mcp-protocol-version", "2026-07-28");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .oneshot(
            request
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{
                            "name":"t3_environment_read", "arguments":{}, "_meta":{
                                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                                "io.modelcontextprotocol/clientCapabilities":{},
                                "io.modelcontextprotocol/clientInfo":{"name":"test","version":"1"}
                            }
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn oauth_challenges_preserve_the_public_https_origin_and_exclude_provider_tokens() {
    for (authorization, challenge) in [
        (
            None,
            "Bearer resource_metadata=\"https://example.test/.well-known/oauth-protected-resource/mcp\"",
        ),
        (
            Some("Bearer stale.signature"),
            "Bearer resource_metadata=\"https://example.test/.well-known/oauth-protected-resource/mcp\", error=\"invalid_token\"",
        ),
        (
            Some("Bearer provider-token"),
            "Bearer error=\"invalid_token\"",
        ),
        (
            Some("Basic credentials"),
            "Bearer resource_metadata=\"https://example.test/.well-known/oauth-protected-resource/mcp\"",
        ),
        (
            Some("Bearer "),
            "Bearer resource_metadata=\"https://example.test/.well-known/oauth-protected-resource/mcp\"",
        ),
    ] {
        let mut headers = vec![("x-forwarded-proto", "https")];
        if let Some(authorization) = authorization {
            headers.push(("authorization", authorization));
        }
        let (status, headers, body) = call(app(true), &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["www-authenticate"], challenge);
        assert_eq!(
            body,
            json!({"error":"invalid_mcp_credential", "message":"A valid T3 Code MCP credential is required."})
        );
    }
}

#[tokio::test]
async fn client_mode_limits_sender_headers_and_links_setting_are_applied_per_request() {
    let router = app(true);
    for (limit, runtime, interaction) in [
        ("full-access/default", "auto-accept-edits", "default"),
        ("approval-required/plan", "approval-required", "plan"),
    ] {
        let (status, _, body) = call(
            router.clone(),
            &[
                ("authorization", "bEaReR linked.signature"),
                ("t3-mode-limit", limit),
                ("t3-sender-thread", "other/thread%2Fid/Title%20%F0%9F%8C%8D"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let client = &body["result"]["structuredContent"]["scope"]["client"];
        assert_eq!(client["narrowedTo"]["runtimeMode"], runtime);
        assert_eq!(client["narrowedTo"]["interactionMode"], interaction);
        assert_eq!(
            client["senderThread"],
            json!({"environmentId":"other","threadId":"thread/id","title":"Title 🌍"})
        );
    }
    for sender in ["a/b/%ZZ", "a/b/%FF", "a/b/c/d", "/b/c"] {
        let (_, _, body) = call(
            router.clone(),
            &[
                ("authorization", "Bearer linked.signature"),
                ("t3-sender-thread", sender),
            ],
        )
        .await;
        assert!(
            body["result"]["structuredContent"]["scope"]["client"]
                .get("senderThread")
                .is_none(),
            "{body}"
        );
    }
    let (_, _, body) = call(
        router.clone(),
        &[
            ("authorization", "Bearer client.signature"),
            ("t3-sender-thread", "a/b/Title"),
        ],
    )
    .await;
    assert!(
        body["result"]["structuredContent"]["scope"]["client"]
            .get("senderThread")
            .is_none()
    );
    for limit in ["auto", "auto/default/extra", "auto/unknown"] {
        let (status, headers, body) = call(
            router.clone(),
            &[
                ("authorization", "Bearer client.signature"),
                ("t3-mode-limit", limit),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(body["error"], "invalid_mode_limit");
    }
    let (status, _, body) = call(app(false), &[("authorization", "Bearer linked.signature")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "linked_environments_off");
}
