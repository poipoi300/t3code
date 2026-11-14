#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, RwLock},
};
use t3_contracts::{
    base_schemas::{EnvironmentId, ThreadId},
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use t3_mcp::*;

#[derive(Default)]
struct Store(RwLock<HashMap<String, ThreadShell>>);
#[async_trait]
impl AccessStore for Store {
    async fn thread_shell(&self, id: &ThreadId) -> Result<Option<ThreadShell>> {
        Ok(self.0.read().unwrap().get(&id.0).cloned())
    }
}
fn shell(id: &str, mode: RuntimeMode, plan: bool) -> ThreadShell {
    ThreadShell {
        id: ThreadId(id.into()),
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
        provider_instance_id: Some(ProviderInstanceId("codex".into())),
        background_subagent_providers: vec![],
        link_origin: None,
    }
}
fn scope(thread: Option<&str>, access: ClientAccess) -> InvocationScope {
    InvocationScope {
        environment_id: EnvironmentId("environment".into()),
        request_namespace: "test".into(),
        issued_at: 0,
        thread: thread.map(|id| ThreadCaller {
            thread_id: ThreadId(id.into()),
            provider_session_id: "test".into(),
            provider_instance_id: ProviderInstanceId("codex".into()),
            subagent: false,
        }),
        client: thread.is_none().then(|| ClientCaller {
            session_id: "test".into(),
            label: "Test".into(),
            access,
            linked: false,
            narrowed_to: None,
            sender_thread: None,
        }),
        capabilities: BTreeSet::from([Capability::Orchestration]),
        linked_environments: Some(true),
    }
}
fn code(result: Result<CallContext>) -> String {
    match result {
        Ok(_) => "ran".into(),
        Err(McpError::Failure { code, .. }) => code,
        Err(error) => panic!("{error}"),
    }
}

fn tool_for(rule: AccessRule) -> &'static str {
    match rule {
        AccessRule::Reads => "t3_thread_read",
        AccessRule::ReadsAsCaller => "preview_status",
        AccessRule::ActsAsCaller => "send_to_user",
        AccessRule::Writes => "delete_scheduled_task",
        AccessRule::WritesWithCapability => "write_pull_request_guides",
        AccessRule::WritesThreads => "t3_thread_send",
        AccessRule::StartsThreads => "t3_thread_launch",
        AccessRule::WritesEnvironment => "t3_theme_set",
    }
}
async fn authorize(
    store: Arc<Store>,
    scope: InvocationScope,
    rule: AccessRule,
    tool: &str,
    input: &Value,
    remote: bool,
) -> Result<CallContext> {
    let actual = access::tool_rule(tool, input).unwrap();
    assert_eq!(actual, rule, "{tool}");
    access::authorize(store, scope, actual, tool, input, remote).await
}

#[tokio::test]
async fn typescript_access_matrix_for_modes_liveness_clients_and_background_owners() {
    use AccessRule::*;
    use ClientAccess::*;
    let store = Arc::new(Store::default());
    for (id, mode, plan) in [
        ("supervised", RuntimeMode::ApprovalRequired, false),
        ("plan", RuntimeMode::ApprovalRequired, true),
        ("full", RuntimeMode::FullAccess, false),
        ("ended", RuntimeMode::Auto, false),
        ("background", RuntimeMode::Auto, false),
        ("wrong-owner", RuntimeMode::Auto, false),
        ("archived", RuntimeMode::Auto, false),
        ("unknown-owner", RuntimeMode::Auto, false),
    ] {
        let mut entry = shell(id, mode, plan);
        if matches!(
            id,
            "ended" | "background" | "wrong-owner" | "archived" | "unknown-owner"
        ) {
            entry.active = false;
        }
        if matches!(id, "background" | "archived") {
            entry
                .background_subagent_providers
                .push(ProviderInstanceId("codex".into()));
        }
        if id == "wrong-owner" {
            entry
                .background_subagent_providers
                .push(ProviderInstanceId("claude".into()));
        }
        if id == "archived" {
            entry.archived = true;
        }
        store.0.write().unwrap().insert(id.into(), entry);
    }
    let thread = |id| scope(Some(id), ClientAccess::ApprovalRequired);
    let client = |access| scope(None, access);
    let cases: Vec<(AccessRule, InvocationScope, Value, &str)> = vec![
        (Reads, thread("ended"), json!({}), "ran"),
        (Reads, client(ReadOnly), json!({}), "ran"),
        (ReadsAsCaller, thread("ended"), json!({}), "ran"),
        (
            ReadsAsCaller,
            client(ClientAccess::FullAccess),
            json!({}),
            "thread_credential_required",
        ),
        (ActsAsCaller, thread("supervised"), json!({}), "ran"),
        (
            ActsAsCaller,
            thread("ended"),
            json!({}),
            "parent_not_active",
        ),
        (ActsAsCaller, thread("background"), json!({}), "ran"),
        (
            ActsAsCaller,
            thread("wrong-owner"),
            json!({}),
            "parent_not_active",
        ),
        (
            ActsAsCaller,
            thread("archived"),
            json!({}),
            "parent_not_active",
        ),
        (
            ActsAsCaller,
            thread("unknown-owner"),
            json!({}),
            "parent_not_active",
        ),
        (
            ActsAsCaller,
            client(ClientAccess::FullAccess),
            json!({}),
            "thread_credential_required",
        ),
        (Writes, thread("background"), json!({}), "ran"),
        (Writes, thread("ended"), json!({}), "parent_not_active"),
        (
            WritesThreads,
            thread("supervised"),
            json!({"threadId":"plan"}),
            "ran",
        ),
        (
            WritesThreads,
            thread("supervised"),
            json!({"threadId":"full"}),
            "runtime_mode_escalation_denied",
        ),
        (
            WritesThreads,
            thread("plan"),
            json!({"threadId":"supervised"}),
            "interaction_mode_escalation_denied",
        ),
        (
            WritesThreads,
            thread("full"),
            json!({"threadId":"supervised"}),
            "ran",
        ),
        (
            WritesThreads,
            client(ClientAccess::ApprovalRequired),
            json!({"threadId":"full"}),
            "runtime_mode_escalation_denied",
        ),
        (
            WritesThreads,
            thread("supervised"),
            json!({"threadId":"missing"}),
            "ran",
        ),
        (
            StartsThreads,
            thread("plan"),
            json!({"interactionMode":"default"}),
            "interaction_mode_escalation_denied",
        ),
        (
            StartsThreads,
            thread("supervised"),
            json!({"runtimeMode":"full-access"}),
            "runtime_mode_escalation_denied",
        ),
        (WritesEnvironment, thread("full"), json!({}), "ran"),
        (
            WritesEnvironment,
            thread("supervised"),
            json!({}),
            "capability_denied",
        ),
        (
            WritesEnvironment,
            thread("plan"),
            json!({}),
            "capability_denied",
        ),
        (
            WritesEnvironment,
            client(ClientAccess::FullAccess),
            json!({}),
            "ran",
        ),
    ];
    for (rule, scope, input, expected) in cases {
        assert_eq!(
            code(authorize(store.clone(), scope, rule, tool_for(rule), &input, false).await),
            expected,
            "{rule:?}"
        );
    }
    for rule in [
        Writes,
        WritesThreads,
        StartsThreads,
        WritesEnvironment,
        WritesWithCapability,
    ] {
        assert_eq!(
            code(
                authorize(
                    store.clone(),
                    client(ReadOnly),
                    rule,
                    tool_for(rule),
                    &json!({}),
                    false
                )
                .await
            ),
            "capability_denied"
        );
    }
    let mut preview = thread("full");
    preview.capabilities = BTreeSet::from([Capability::Preview]);
    assert_eq!(
        code(
            authorize(
                store.clone(),
                preview.clone(),
                Writes,
                tool_for(AccessRule::Writes),
                &json!({}),
                false
            )
            .await
        ),
        "capability_denied"
    );
    assert_eq!(
        code(
            authorize(
                store.clone(),
                preview,
                WritesWithCapability,
                tool_for(AccessRule::WritesWithCapability),
                &json!({}),
                false
            )
            .await
        ),
        "ran"
    );
}

#[tokio::test]
async fn linked_writes_are_fenced_reads_are_open_and_remote_caller_checks_do_not_read_remote_threads()
 {
    let store = Arc::new(Store::default());
    let mut target = shell("target", RuntimeMode::ApprovalRequired, false);
    store
        .0
        .write()
        .unwrap()
        .insert("target".into(), target.clone());
    let mut linked = scope(None, ClientAccess::ApprovalRequired);
    linked.client.as_mut().unwrap().linked = true;
    for rule in [
        AccessRule::Writes,
        AccessRule::WritesWithCapability,
        AccessRule::WritesEnvironment,
    ] {
        assert_eq!(
            code(
                authorize(
                    store.clone(),
                    linked.clone(),
                    rule,
                    tool_for(rule),
                    &json!({}),
                    false
                )
                .await
            ),
            "capability_denied"
        );
    }
    assert_eq!(
        code(
            authorize(
                store.clone(),
                linked.clone(),
                AccessRule::Reads,
                tool_for(AccessRule::Reads),
                &json!({}),
                false
            )
            .await
        ),
        "ran"
    );
    assert_eq!(
        code(
            authorize(
                store.clone(),
                linked.clone(),
                AccessRule::WritesThreads,
                tool_for(AccessRule::WritesThreads),
                &json!({"threadId":"target"}),
                false
            )
            .await
        ),
        "capability_denied"
    );
    target.link_origin = Some(LinkOrigin {
        session_id: "test".into(),
        label: "Test".into(),
        access: ClientAccess::ApprovalRequired,
    });
    store.0.write().unwrap().insert("target".into(), target);
    assert_eq!(
        code(
            authorize(
                store.clone(),
                linked.clone(),
                AccessRule::WritesThreads,
                tool_for(AccessRule::WritesThreads),
                &json!({"threadId":"target"}),
                false
            )
            .await
        ),
        "ran"
    );
    assert_eq!(
        code(
            authorize(
                store,
                linked,
                AccessRule::WritesThreads,
                tool_for(AccessRule::WritesThreads),
                &json!({"threadId":"remote-unknown"}),
                true
            )
            .await
        ),
        "ran"
    );
}

struct RacingService {
    store: Arc<Store>,
}
#[async_trait]
impl services::OrchestratorService for RacingService {
    async fn t3_thread_update(
        &self,
        context: CallContext,
        _input: DecodedInput,
    ) -> Result<t3_contracts::thread_metadata_mcp::ThreadMetadataMcpUpdateResult> {
        let mut records = self.store.0.write().unwrap();
        let target = records.get_mut("target").unwrap();
        target.modes.runtime_mode = RuntimeMode::FullAccess;
        // The real orchestrator performs this check under its command lock.
        context
            .check_dispatch(target)
            .map_err(|_| McpError::failure("orchestration_error", "The rename failed."))?;
        panic!("a rejected write must never run")
    }
}

#[tokio::test]
async fn mode_raise_after_precheck_is_refused_under_lock_and_survives_handler_error_wrapping() {
    let store = Arc::new(Store::default());
    store.0.write().unwrap().insert(
        "target".into(),
        shell("target", RuntimeMode::ApprovalRequired, false),
    );
    let services = Services {
        orchestrator: Arc::new(RacingService {
            store: store.clone(),
        }),
        ..Services::default()
    };
    let server = McpServer::new(services, store.clone());
    let result = server
        .call(
            scope(None, ClientAccess::ApprovalRequired),
            "t3_thread_update",
            json!({"threadId":"target","action":"rename","title":"Renamed"}),
        )
        .await
        .unwrap();
    assert_eq!(result["isError"], true, "{result}");
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        payload["code"], "runtime_mode_escalation_denied",
        "{payload}"
    );
    assert_eq!(
        store.0.read().unwrap()["target"].modes.runtime_mode,
        RuntimeMode::FullAccess
    );
}
