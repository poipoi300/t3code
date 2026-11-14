#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use t3_mcp::{access::tool_rule, catalog::catalog};

mod calls {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex, RwLock};
    use t3_contracts::{
        base_schemas::ThreadId,
        provider_policy::{ProviderInteractionMode, RuntimeMode},
    };
    use t3_mcp::*;

    struct Store(RwLock<ThreadShell>);
    #[async_trait]
    impl AccessStore for Store {
        async fn thread_shell(&self, id: &ThreadId) -> Result<Option<ThreadShell>> {
            assert_eq!(
                id.0, "caller",
                "remote target IDs must never be read locally"
            );
            Ok(Some(self.0.read().unwrap().clone()))
        }
    }
    fn scope() -> InvocationScope {
        serde_json::from_value(json!({"environmentId":"local","requestNamespace":"security","issuedAt":0,"capabilities":["orchestration","worktree","pull-requests"],"linkedEnvironments":true,"thread":{"threadId":"caller","providerInstanceId":"codex","providerSessionId":"session","subagent":false}})).unwrap()
    }
    fn store() -> Arc<Store> {
        Arc::new(Store(RwLock::new(ThreadShell {
            id: ThreadId("caller".into()),
            modes: Modes::full(),
            deleted: false,
            archived: false,
            active: true,
            provider_instance_id: Some(t3_contracts::provider_instance::ProviderInstanceId(
                "codex".into(),
            )),
            background_subagent_providers: vec![],
            link_origin: None,
        })))
    }
    #[derive(Default)]
    struct Peer(Mutex<Vec<CallContext>>);
    #[async_trait]
    impl server::PeerService for Peer {
        async fn call(
            &self,
            context: CallContext,
            environment: &str,
            _tool: &str,
            input: Value,
        ) -> Result<Value> {
            assert_eq!(environment, "remote");
            assert_eq!(input["environmentId"], "remote");
            self.0.lock().unwrap().push(context);
            Ok(json!({"forwarded":true}))
        }
    }
    fn refusal(result: &Value, code: &str) {
        assert_eq!(result["isError"], true, "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap_or_else(|error| panic!("{result}: {error}"))
                ["code"],
            code,
            "{result}"
        );
    }

    #[tokio::test]
    async fn forwarding_loads_caller_and_preserves_modes_but_refuses_link_fences_and_capabilities()
    {
        let store = store();
        let peer = Arc::new(Peer::default());
        let mut server = McpServer::new(Services::default(), store.clone());
        server.peers = Some(peer.clone());
        let input = json!({"environmentId":"remote","threadId":"remote-thread"});
        {
            let mut caller = store.0.write().unwrap();
            caller.modes = Modes {
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: ProviderInteractionMode::Plan,
            };
            caller.active = false;
        }
        assert_eq!(
            server
                .call(scope(), "t3_thread_read", input.clone())
                .await
                .unwrap()["isError"],
            false
        );
        {
            let calls = peer.0.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0].limits.interaction_mode,
                ProviderInteractionMode::Plan
            );
            assert_eq!(calls[0].limits.runtime_mode, RuntimeMode::ApprovalRequired);
            assert_eq!(calls[0].caller.as_ref().unwrap().id.0, "caller");
        }
        store.0.write().unwrap().link_origin = Some(LinkOrigin {
            session_id: "origin".into(),
            label: "Origin".into(),
            access: ClientAccess::ApprovalRequired,
        });
        let denied = server
            .call(scope(), "t3_thread_read", input.clone())
            .await
            .unwrap();
        refusal(&denied, "capability_denied");
        assert!(
            denied
                .to_string()
                .contains("cannot use this environment's links")
        );
        store
            .0
            .write()
            .unwrap()
            .link_origin
            .as_mut()
            .unwrap()
            .access = ClientAccess::FullAccess;
        assert_eq!(
            server
                .call(scope(), "t3_thread_read", input.clone())
                .await
                .unwrap()["isError"],
            false
        );
        assert_eq!(
            peer.0.lock().unwrap()[1]
                .link_origin
                .as_ref()
                .unwrap()
                .access,
            ClientAccess::FullAccess
        );
        refusal(
            &server
                .call(
                    scope(),
                    "t3_thread_update",
                    json!({"environmentId":"remote","threadId":"remote-thread","action":"rename","title":"Hello"}),
                )
                .await
                .unwrap(),
            "parent_not_active",
        );
        store.0.write().unwrap().link_origin = None;
        store.0.write().unwrap().active = true;
        store.0.write().unwrap().modes = Modes::full();
        for (tool, input, capability) in [
            ("t3_thread_read", input.clone(), Capability::Orchestration),
            ("t3_worktree_list", input.clone(), Capability::Worktree),
            (
                "list_thread_pull_requests",
                input.clone(),
                Capability::PullRequests,
            ),
        ] {
            let mut caller = scope();
            caller.capabilities.remove(&capability);
            let result = server.call(caller, tool, input).await.unwrap();
            assert_eq!(result["isError"], true, "{tool}: {result}");
            match capability {
                Capability::Orchestration => refusal(&result, "capability_denied"),
                Capability::Worktree => {
                    let error: Value =
                        serde_json::from_str(result["content"][0]["text"].as_str().unwrap())
                            .unwrap();
                    assert_eq!(error["_tag"], "McpCapabilityUnavailableError");
                    assert_eq!(error["capability"], "worktree");
                }
                Capability::PullRequests => assert_eq!(
                    result["content"][0]["text"],
                    catalog().capability_messages["generic"]
                        .replace("__CAPABILITY__", "pull-requests")
                ),
                _ => unreachable!(),
            }
        }
        let mut external = scope();
        external.thread = None;
        external.client = Some(ClientCaller {
            session_id: "external".into(),
            label: "External".into(),
            access: ClientAccess::ApprovalRequired,
            linked: true,
            narrowed_to: None,
            sender_thread: None,
        });
        refusal(
            &server
                .call(external.clone(), "t3_thread_read", input.clone())
                .await
                .unwrap(),
            "capability_denied",
        );
        external.client.as_mut().unwrap().linked = false;
        external.client.as_mut().unwrap().access = ClientAccess::ReadOnly;
        refusal(
            &server
                .call(
                    external,
                    "t3_thread_update",
                    json!({"environmentId":"remote","threadId":"remote-thread","action":"rename","title":"Hello"}),
                )
                .await
                .unwrap(),
            "capability_denied",
        );
        store.0.write().unwrap().deleted = true;
        refusal(
            &server.call(scope(), "t3_thread_read", input).await.unwrap(),
            "thread_not_found",
        );
        assert_eq!(peer.0.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn link_routing_off_uses_the_typescript_tool_message_and_subagents_cannot_call_hidden_tools()
     {
        let peer = Arc::new(Peer::default());
        let mut server = McpServer::new(Services::default(), store());
        server.peers = Some(peer.clone());
        let mut caller = scope();
        caller.linked_environments = Some(false);
        let result = server
            .call(
                caller,
                "t3_thread_read",
                json!({"environmentId":"remote","threadId":"remote-thread"}),
            )
            .await
            .unwrap();
        refusal(&result, "capability_denied");
        let error: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            error["message"],
            instructions::fixture()["linkedEnvironmentsOff"]["message"]
        );
        let mut caller = scope();
        caller.thread.as_mut().unwrap().subagent = true;
        for tool in ["watch_pull_request", "unwatch_pull_request"] {
            assert!(
                server
                    .call(
                        caller.clone(),
                        tool,
                        json!({"threadId":"caller","url":"https://github.com/org/repo/pull/1"})
                    )
                    .await
                    .is_err()
            );
        }
        assert!(peer.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn html_paths_refuse_deleted_or_below_full_access_callers_before_host_execution() {
        let store = store();
        let server = McpServer::new(Services::default(), store.clone());
        store.0.write().unwrap().modes.runtime_mode = RuntimeMode::ApprovalRequired;
        let result = server
            .call(scope(), "html_preview", json!({"path":"private.html"}))
            .await
            .unwrap();
        assert_eq!(result["isError"], true);
        assert!(result.to_string().contains("only full-access threads"));
        store.0.write().unwrap().modes = Modes::full();
        store.0.write().unwrap().deleted = true;
        let result = server
            .call(scope(), "html_preview", json!({"path":"private.html"}))
            .await
            .unwrap();
        assert!(
            result.to_string().contains("calling thread was not found"),
            "{result}"
        );
    }
}

#[test]
fn every_access_mapping_matches_the_real_typescript_declarations() {
    fn leaves(declaration: &Value) -> std::collections::BTreeSet<String> {
        if declaration["kind"] == "either" {
            leaves(&declaration["first"])
                .union(&leaves(&declaration["second"]))
                .cloned()
                .collect()
        } else {
            std::collections::BTreeSet::from([declaration["kind"].as_str().unwrap().to_owned()])
        }
    }
    let bindings: Value = serde_json::from_str(include_str!("../fixtures/bindings.json")).unwrap();
    assert_eq!(bindings.as_object().unwrap().len(), catalog().tools.len());
    for tool in &catalog().tools {
        let binding = &bindings[&tool.name];
        assert!(
            binding["accessDeclaration"]["kind"].is_string(),
            "{} has no declaration",
            tool.name
        );
        let mut observed = std::collections::BTreeSet::new();
        for case in binding["accessCases"].as_array().unwrap() {
            let rule = tool_rule(&tool.name, &case["input"])
                .unwrap_or_else(|error| panic!("{}: {error}", tool.name));
            observed.insert(
                serde_json::to_value(rule)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            assert_eq!(
                serde_json::to_value(rule).unwrap(),
                case["rule"],
                "{} {}",
                tool.name,
                case["input"]
            );
        }
        assert_eq!(
            observed,
            leaves(&binding["accessDeclaration"]),
            "{}: a TypeScript declaration branch has no matching Rust witness",
            tool.name
        );
    }
    for name in ["new_upstream_mutation", "preview_new_mutation", ""] {
        assert!(tool_rule(name, &json!({})).is_err());
    }
}

#[test]
fn cargo_detects_stale_source_or_fixture_snapshots() {
    use sha2::{Digest, Sha256};
    let manifest: Value =
        serde_json::from_str(include_str!("../fixtures/source-hashes.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for (path, expected) in manifest.as_object().unwrap() {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("{path}: {error}; regenerate t3-mcp fixtures"));
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(source.replace("\r\n", "\n").as_bytes())
            ),
            expected.as_str().unwrap(),
            "{path} changed; run t3-mcp/export.ts before Cargo tests"
        );
    }
}
