#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use t3_contracts::{
    base_schemas::{EnvironmentId, ThreadId},
    orchestrator_mcp::{
        ORCHESTRATOR_MCP_RUNNING_TASK_NEXT, OrchestratorMcpRunningTaskNext,
        OrchestratorMcpThreadWaitResult,
    },
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use t3_mcp::{
    catalog::{advertised, catalog},
    instructions, *,
};

struct Store(RwLock<ThreadShell>);
#[async_trait]
impl AccessStore for Store {
    async fn thread_shell(&self, id: &ThreadId) -> Result<Option<ThreadShell>> {
        Ok(Some(ThreadShell {
            id: id.clone(),
            ..self.0.read().unwrap().clone()
        }))
    }
}
fn store(mode: RuntimeMode, plan: bool) -> Arc<Store> {
    Arc::new(Store(RwLock::new(ThreadShell {
        id: ThreadId("caller".into()),
        modes: Modes {
            runtime_mode: mode,
            interaction_mode: if plan {
                ProviderInteractionMode::Plan
            } else {
                ProviderInteractionMode::Default
            },
        },
        deleted: false,
        archived: false,
        active: true,
        provider_instance_id: Some(ProviderInstanceId("claudeAgent".into())),
        background_subagent_providers: vec![],
        link_origin: None,
    })))
}
fn scope() -> InvocationScope {
    InvocationScope {
        environment_id: EnvironmentId("env".into()),
        request_namespace: "session".into(),
        issued_at: 0,
        capabilities: std::collections::BTreeSet::from([Capability::Orchestration]),
        thread: Some(ThreadCaller {
            thread_id: ThreadId("caller".into()),
            provider_session_id: "session".into(),
            provider_instance_id: ProviderInstanceId("claudeAgent".into()),
            subagent: false,
        }),
        client: None,
        linked_environments: None,
    }
}
fn failure(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn send_to_user_preserves_mid_turn_markdown_in_plan_and_read_only_sandboxes() {
    let text = "  Draft:\n```rust\nlet emoji = \"😀\";\n```\n";
    for plan in [false, true] {
        let store = store(RuntimeMode::ApprovalRequired, plan);
        let server = McpServer::new(Services::default(), store.clone());
        let result = server
            .call(scope(), "send_to_user", json!({"message":text}))
            .await
            .unwrap();
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["sentMessage"], text);
        assert_eq!(
            result["structuredContent"]["message"],
            instructions::fixture()["sendToUserAcknowledgement"]
        );
        assert_eq!(
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
            result["structuredContent"]
        );
        store.0.write().unwrap().active = false;
        let denied = server
            .call(scope(), "send_to_user", json!({"message":text}))
            .await
            .unwrap();
        assert_eq!(denied["isError"], true);
        assert_eq!(failure(&denied)["_tag"], "OrchestratorMcpFailure");
        assert_eq!(failure(&denied)["code"], "parent_not_active");
    }
    let tool = advertised("send_to_user").unwrap();
    assert_eq!(
        tool.annotations,
        json!({"title":"Send a message to the user","readOnlyHint":true,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false})
    );
    assert_eq!(tool.meta.unwrap()["anthropic/alwaysLoad"], true);
}

#[test]
fn send_to_user_length_is_javascript_utf16_without_trimming() {
    assert!(serde_json::from_str::<Value>(r#"{"message":"\ud800"}"#).is_err());
    let paired: Value = serde_json::from_str(r#"{"message":"\ud83d\ude00"}"#).unwrap();
    assert_eq!(
        decode_input("send_to_user", paired).unwrap().value()["message"],
        "😀"
    );
    for message in [" ".into(), "a".repeat(4000), "😀".repeat(2000)] {
        let input = decode_input("send_to_user", json!({"message":message})).unwrap();
        assert_eq!(input.value()["message"], message);
    }
    for message in ["".into(), "a".repeat(4001), "😀".repeat(2000) + "x"] {
        assert!(decode_input("send_to_user", json!({"message":message})).is_err());
    }
}

#[test]
fn context_budgets_and_always_load_metadata_match_the_current_typescript() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/server/src/provider/agentContextBudget.json");
    let budget: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        instructions::claude(true, false).len(),
        budget["claudeSystemAppendBytes"].as_u64().unwrap() as usize
    );
    assert_eq!(
        instructions::codex(
            &instructions::RuntimeInfo {
                harness: "Codex",
                model: "gpt-6.1-sol",
                model_name: "GPT-6.1 Sol",
                reasoning_effort: "high",
                pull_request_linking: true
            },
            true,
            true
        )
        .as_object()
        .unwrap()
        .values()
        .map(|entry| entry["value"].as_str().unwrap().len())
        .sum::<usize>(),
        budget["codexAdditionalContextBytes"].as_u64().unwrap() as usize
    );
    let tools=catalog().tools.iter().filter(|tool|tool.name!="tool_search").map(|tool|json!({"name":tool.name,"description":tool.description,"inputSchema":tool.input_schema})).collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_vec(&tools).unwrap().len(),
        budget["mcpToolListJsonBytes"].as_u64().unwrap() as usize
    );
    let always_load = catalog()
        .tools
        .iter()
        .filter(|tool| {
            tool.meta
                .as_ref()
                .is_some_and(|meta| meta["anthropic/alwaysLoad"] == true)
        })
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(always_load, vec!["request_secret", "send_to_user"]);
}

#[test]
fn running_task_next_accepts_other_versions_but_emits_current_guidance() {
    assert_eq!(
        ORCHESTRATOR_MCP_RUNNING_TASK_NEXT,
        instructions::fixture()["runningTaskNext"]
    );
    let next = "Guidance written by a different server version.";
    let text: OrchestratorMcpRunningTaskNext = serde_json::from_value(json!(next)).unwrap();
    assert_eq!(text, next);
    let result: OrchestratorMcpThreadWaitResult = serde_json::from_value(
        json!({"threadId":"thread","runId":null,"status":"idle","timedOut":false,"next":next}),
    )
    .unwrap();
    assert_eq!(result.next.flatten().unwrap(), next);
}

#[tokio::test]
async fn theme_mutations_require_environment_authority_and_valid_input() {
    let store = store(RuntimeMode::FullAccess, true);
    let server = McpServer::new(Services::default(), store.clone());
    let denied = server
        .call(scope(), "t3_theme_set", json!({"appearance":"dark"}))
        .await
        .unwrap();
    assert_eq!(failure(&denied)["code"], "capability_denied");
    store.0.write().unwrap().modes.interaction_mode = ProviderInteractionMode::Default;
    store.0.write().unwrap().link_origin = Some(LinkOrigin {
        session_id: "peer".into(),
        label: "Peer".into(),
        access: ClientAccess::Auto,
    });
    let linked = server
        .call(scope(), "t3_theme_set", json!({"appearance":"dark"}))
        .await
        .unwrap();
    assert_eq!(failure(&linked)["code"], "capability_denied");
    store.0.write().unwrap().link_origin = None;
    for input in [json!({}), json!({"theme":{}})] {
        let result = server.call(scope(), "t3_theme_set", input).await.unwrap();
        assert_eq!(result["isError"], true);
    }
    let input = decode_input("t3_theme_set", json!({"appearance":"dark"})).unwrap();
    let context = access::authorize(
        store.clone(),
        scope(),
        AccessRule::WritesEnvironment,
        "t3_theme_set",
        input.value(),
        false,
    )
    .await
    .unwrap();
    store.0.write().unwrap().active = false;
    assert!(context.recheck_environment().await.is_err());
}
