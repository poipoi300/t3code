#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use t3_contracts::{
    base_schemas::{EnvironmentId, ThreadId},
    provider_instance::ProviderInstanceId,
};
use t3_mcp::{catalog::catalog, exposure::ToolExposure, instructions, *};

fn scope(subagent: bool, links: bool, external: bool) -> InvocationScope {
    InvocationScope {
        environment_id: EnvironmentId("environment-fixture".into()),
        request_namespace: "fixture".into(),
        issued_at: 0,
        capabilities: BTreeSet::from([
            Capability::Preview,
            Capability::Device,
            Capability::Orchestration,
            Capability::Worktree,
            Capability::PullRequests,
        ]),
        thread: (!external).then(|| ThreadCaller {
            thread_id: ThreadId("thread-fixture".into()),
            provider_instance_id: ProviderInstanceId("codex".into()),
            provider_session_id: "fixture".into(),
            subagent,
        }),
        client: external.then(|| ClientCaller {
            session_id: "fixture".into(),
            label: "fixture".into(),
            access: ClientAccess::ReadOnly,
            linked: false,
            narrowed_to: None,
            sender_thread: None,
        }),
        linked_environments: Some(links),
    }
}

#[test]
fn tool_list_matches_real_typescript_http_in_every_access_mode() {
    assert_eq!(catalog().tools.len(), 72);
    for (key, expected) in &catalog().modes {
        let parts = key.split('/').collect::<Vec<_>>();
        let scope = scope(
            parts[0] == "subagent",
            parts[1] == "links",
            parts[0] == "client",
        );
        let actual = ToolExposure::default().list(&scope, parts[2] == "deferred");
        assert_eq!(actual["tools"], expected["tools"], "{key}");
        for tool in actual["tools"].as_array().unwrap() {
            let sdk: rmcp::model::Tool = serde_json::from_value(tool.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(sdk).unwrap(),
                *tool,
                "{}",
                tool["name"]
            );
        }
    }
}

#[test]
fn instruction_matrix_matches_extracted_provider_builders() {
    let info = instructions::RuntimeInfo {
        harness: "__HARNESS__",
        model: "__MODEL__",
        model_name: "__MODEL_NAME__",
        reasoning_effort: "__EFFORT__",
        pull_request_linking: true,
    };
    assert_eq!(
        instructions::runtime(&info),
        instructions::fixture()["runtime"]
    );
    for browser in [false, true] {
        for device in [false, true] {
            let runtime = instructions::RuntimeInfo {
                harness: "Codex",
                ..info
            };
            assert_eq!(
                instructions::codex(&runtime, browser, device),
                instructions::fixture()[format!("codex/{browser}/{device}")]
            );
        }
    }
    for mcp in [false, true] {
        for device in [false, true] {
            assert_eq!(
                instructions::claude(mcp, device),
                instructions::fixture()[format!("claude/{mcp}/{device}")]
            );
            for plan in [false, true] {
                let state = instructions::AcpState {
                    plan,
                    has_mcp: mcp,
                    device,
                };
                assert_eq!(
                    instructions::acp("__PROMPT__", state, None),
                    instructions::fixture()[format!(
                        "acp/{}/{mcp}/{device}",
                        if plan { "plan" } else { "default" }
                    )]
                );
                assert_eq!(instructions::acp(" /help", state, None), " /help");
                assert_eq!(
                    instructions::acp("unchanged", state, Some(state)),
                    "unchanged"
                );
            }
        }
    }
}

#[test]
fn decodes_full_union_not_the_merged_advertisement() {
    assert!(decode_input("preview_act", json!({"action":"type"})).is_err());
    let input = decode_input(
        "preview_act",
        json!({"action":"click","locator":"aria-ref=1","x":3,"y":4}),
    )
    .unwrap();
    assert_eq!(input.value()["action"], "click");
    assert!(
        decode_input("preview_snapshot", json!({"madeUp":true}))
            .unwrap_err()
            .to_string()
            .contains("Unknown argument \"madeUp\"")
    );
    assert_eq!(
        decode_input(
            "t3_thread_read",
            json!({"threadId":"thread","limit":999,"runLimit":999,"maxCharsPerItem":999999})
        )
        .unwrap()
        .value()["limit"],
        100
    );
    assert_eq!(
        decode_input("preview_snapshot", json!({"includeImage":null}))
            .unwrap()
            .value(),
        &json!({})
    );
}

#[test]
fn decoded_inputs_match_typescript_codec_witnesses() {
    let cases: Value = serde_json::from_str(include_str!("../fixtures/decoders.json")).unwrap();
    for (name, cases) in cases.as_object().unwrap() {
        for case in cases.as_array().unwrap() {
            if case["invalidAction"] == true || case["invalid"] == true {
                assert!(decode_input(name, case["input"].clone()).is_err());
                continue;
            }
            let decoded = decode_input(name, case["input"].clone())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(
                decoded.value() == &case["decoded"],
                "{name}: actual {} expected {}",
                decoded.value(),
                case["decoded"]
            );
        }
    }
}

#[test]
fn deferred_search_is_session_scoped_and_not_a_call_gate() {
    let exposure = ToolExposure::default();
    let scope = scope(false, true, false);
    assert_eq!(
        exposure.list(&scope, true)["tools"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let found = exposure.search("fixture", "device_screenshot");
    assert_eq!(found["tools"][0]["name"], "device_screenshot");
    assert!(
        exposure.list(&scope, true)["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "device_screenshot")
    );
    let mut another = scope.clone();
    another.thread.as_mut().unwrap().provider_session_id = "other".into();
    assert_eq!(
        exposure.list(&another, true)["tools"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(
        !exposure.list(&scope, false)["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "tool_search")
    );
}

struct Store;
#[async_trait]
impl AccessStore for Store {
    async fn thread_shell(&self, id: &ThreadId) -> Result<Option<ThreadShell>> {
        Ok(Some(ThreadShell {
            id: id.clone(),
            modes: Modes::full(),
            deleted: false,
            archived: false,
            active: true,
            provider_instance_id: Some(ProviderInstanceId("codex".into())),
            background_subagent_providers: vec![],
            link_origin: None,
        }))
    }
}

async fn request(
    router: axum::Router,
    token: Option<&str>,
    session: Option<&str>,
    version: &str,
    method: &str,
    params: Value,
) -> (http::StatusCode, http::HeaderMap, Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(token) = token {
        request = request.header("authorization", token);
    }
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    if !version.is_empty() {
        request = request
            .header("mcp-protocol-version", version)
            .header("mcp-method", method);
    }
    let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let response = router
        .oneshot(
            request
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let value = if text.is_empty() {
        Value::Null
    } else if text.starts_with("event:") || text.starts_with("data:") {
        let line = text
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .next_back()
            .unwrap();
        serde_json::from_str(line).unwrap()
    } else {
        serde_json::from_str(&text).unwrap_or_else(|_| json!({"raw":text}))
    };
    (status, headers, value)
}

#[tokio::test]
async fn real_sdk_http_lists_the_exact_fixture_and_negotiates_both_revisions() {
    let registry = Arc::new(SessionRegistry::new(
        EnvironmentId("environment-fixture".into()),
        None,
    ));
    let credential = registry
        .issue(CredentialRequest {
            thread_id: ThreadId("thread-fixture".into()),
            provider_instance_id: ProviderInstanceId("codex".into()),
            browser_tools_available: true,
            capabilities: Some(scope(false, true, false).capabilities),
            subagent: false,
        })
        .unwrap();
    let server = Arc::new(McpServer::new(Services::default(), Arc::new(Store)));
    let router = transport::router(
        server,
        Arc::new(transport::HttpState {
            registry,
            clients: None,
            linked_environments_enabled: std::sync::atomic::AtomicBool::new(true),
            runtime_ceiling: None,
        }),
    );
    let params = json!({"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"fixture","version":"1"}}});
    let (status, _, body) = request(
        router.clone(),
        Some(&credential.authorization_header),
        None,
        "2026-07-28",
        "tools/list",
        params.clone(),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{body}");
    assert_eq!(
        body["result"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        catalog().modes["thread/links/full"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<BTreeSet<_>>()
    );
    for (actual, expected) in body["result"]["tools"].as_array().unwrap().iter().zip(
        catalog().modes["thread/links/full"]["tools"]
            .as_array()
            .unwrap(),
    ) {
        assert!(actual == expected, "{} differs", actual["name"]);
    }
    for key in ["_meta", "resultType", "ttlMs", "cacheScope"] {
        assert_eq!(
            body["result"][key],
            catalog().modes["thread/links/full"][key],
            "{key}"
        );
    }
    let (status,headers,body)=request(router.clone(),Some(&credential.authorization_header),None,"","initialize",json!({"protocolVersion":"2099-01-01","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}})).await;
    assert_eq!(status, http::StatusCode::OK, "{body}");
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    let session = headers.get("mcp-session-id").unwrap().to_str().unwrap();
    let (status, _, body) = request(
        router.clone(),
        Some(&credential.authorization_header),
        Some(session),
        "2025-06-18",
        "tools/list",
        json!({}),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{body}");
    assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 71);
    assert!(
        body["result"] == catalog().legacy_modes["thread/links/full"],
        "legacy tools/list differs"
    );
    use tower::ServiceExt;
    let terminated = router
        .clone()
        .oneshot(
            http::Request::builder()
                .method("DELETE")
                .uri("/mcp")
                .header("host", "127.0.0.1")
                .header("authorization", &credential.authorization_header)
                .header("mcp-session-id", session)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(terminated.status(), http::StatusCode::NO_CONTENT);
    let (status, _, _) = request(
        router.clone(),
        Some(&credential.authorization_header),
        Some(session),
        "2025-06-18",
        "tools/list",
        json!({}),
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    let (status, _, body) = request(router, None, None, "2026-07-28", "tools/list", params).await;
    assert_eq!(status, http::StatusCode::UNAUTHORIZED, "{body}");
}
