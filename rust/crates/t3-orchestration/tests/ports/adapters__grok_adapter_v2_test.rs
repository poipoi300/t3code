#![cfg(feature = "acp-adapter")]
#![allow(
    dead_code,
    reason = "Native conformance entry points await the named provider and host bindings."
)]

use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use t3_orchestration::{
    adapters::acp::{AcpAdapter, PermissionDisposition, permission_disposition},
    api::Clock,
    error::Result,
    ports::acp_replay::{
        adapters::acp_adapter_v2_testkit::make_turn_input,
        testkit::provider_replay_harness::{AcpFlavorAdapterFactory, ProviderReplayHarness},
    },
    provider::*,
    testing::{ManualClock, SequenceIds, TEST_TIME, replay::ReplayGate},
};
use tokio_util::sync::CancellationToken;

fn policy(mode: RuntimeMode, approval: Option<Value>, sandbox: Option<Value>) -> RuntimePolicy {
    RuntimePolicy {
        runtime_mode: mode,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: Some("/workspace".into()),
        approval_policy: approval,
        sandbox_policy: sandbox,
        max_runtime_mode: None,
    }
}
fn permission(kind: &str) -> Value {
    json!({"sessionId":"session-1","options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"allow-always","name":"Allow always","kind":"allow_always"},{"optionId":"reject-once","name":"Reject","kind":"reject_once"}],"toolCall":{"toolCallId":"tool-1","title":"Test tool","kind":kind}})
}

// Local shim: bind these probes to the actual Grok flavor and a capturing, failing native spawner.
#[async_trait::async_trait]
pub trait GrokNativeTestHooks: Send + Sync {
    fn prompt_failure(
        &self,
        code: Option<i64>,
        message: &str,
    ) -> t3_contracts::orchestration_v2::OrchestrationV2ProviderFailure;
    fn terminalizes_completed_tool(&self, tool: &Value) -> bool;
    fn permission_disposition(
        &self,
        policy: &RuntimePolicy,
        request: &Value,
    ) -> PermissionDisposition;
    fn supported_runtime_modes(&self) -> Vec<RuntimeMode>;
    async fn launch_args(
        &self,
        policy: RuntimePolicy,
        host_platform: &str,
    ) -> Result<Vec<Vec<String>>>;
}

pub async fn grok_native_flavor_and_launch_conformance(
    native: &dyn GrokNativeTestHooks,
) -> Result<()> {
    let failure = serde_json::to_value(
        native.prompt_failure(Some(-32003), "Grok usage limit reached. Try again later."),
    )?;
    assert_eq!(failure["class"], "usage_limit");
    assert_eq!(failure["code"], "-32003");
    assert_eq!(
        failure["message"],
        "Grok usage limit reached. Try again later."
    );
    for (code, message) in [
        (Some(-32603), "Internal error"),
        (None, "Rate limit mentioned in an ordinary error"),
    ] {
        assert_eq!(
            serde_json::to_value(native.prompt_failure(code, message))?["class"],
            "provider_error"
        );
    }
    let foreground = json!({"toolCallId":"foreground-1","title":"Terminal","status":"inProgress","data":{"rawInput":{"command":"true"},"rawOutput":{"type":"Bash","exit_code":0}}});
    let monitor = json!({"toolCallId":"monitor-1","title":"Monitor","status":"inProgress","data":{"rawInput":{"variant":"Monitor","command":"sleep 30"},"rawOutput":{"type":"Monitor","taskId":"019f44b8-8e98-7c80-a40e-df1e26a5f9e3"}}});
    let subagent = json!({"toolCallId":"subagent-1","title":"Task","status":"inProgress","data":{"rawInput":{"description":"Inspect interrupt handling","prompt":"Review the adapter.","subagent_type":"generalPurpose"}}});
    assert!(native.terminalizes_completed_tool(&foreground));
    assert!(!native.terminalizes_completed_tool(&monitor));
    assert!(!native.terminalizes_completed_tool(&subagent));
    assert_eq!(
        native.permission_disposition(&policy(RuntimeMode::Auto, None, None), &permission("read")),
        PermissionDisposition::Ask
    );
    assert_eq!(
        native.permission_disposition(
            &policy(
                RuntimeMode::Auto,
                Some(json!("never")),
                Some(json!({"type":"readOnly"}))
            ),
            &permission("execute")
        ),
        PermissionDisposition::Deny
    );
    assert_eq!(
        native.permission_disposition(
            &policy(RuntimeMode::ApprovalRequired, None, None),
            &permission("edit")
        ),
        PermissionDisposition::Ask
    );
    for (mode, args) in [
        (
            RuntimeMode::ApprovalRequired,
            vec!["--permission-mode", "default", "agent", "stdio"],
        ),
        (
            RuntimeMode::Auto,
            vec!["--permission-mode", "auto", "agent", "stdio"],
        ),
        (
            RuntimeMode::FullAccess,
            vec!["agent", "--always-approve", "stdio"],
        ),
    ] {
        assert_eq!(
            native
                .launch_args(policy(mode, None, None), "darwin")
                .await?,
            vec![args]
        );
    }
    let supported = native.supported_runtime_modes();
    let mode = t3_orchestration::ports::decisions::runtime_policy::provider_runtime_mode(
        RuntimeMode::AutoAcceptEdits,
        Some(&supported),
    );
    assert_eq!(mode, RuntimeMode::ApprovalRequired);
    assert_eq!(
        native
            .launch_args(policy(mode, None, None), "darwin")
            .await?,
        vec![vec!["--permission-mode", "default", "agent", "stdio"]]
    );
    Ok(())
}

#[test]
fn grok_acp_permission_policy_rejects_escalation_and_honors_resolved_access() {
    let read_only = policy(
        RuntimeMode::FullAccess,
        Some(json!("never")),
        Some(json!({"type":"readOnly"})),
    );
    for kind in ["execute", "edit"] {
        assert_eq!(
            permission_disposition(&read_only, kind, None),
            PermissionDisposition::Deny
        );
    }
    assert_eq!(
        permission_disposition(&read_only, "read", None),
        PermissionDisposition::Allow
    );
    assert_eq!(
        permission_disposition(
            &policy(
                RuntimeMode::FullAccess,
                Some(json!("never")),
                Some(json!({"type":"dangerFullAccess"}))
            ),
            "execute",
            None
        ),
        PermissionDisposition::Allow
    );
    assert_eq!(
        permission_disposition(
            &policy(RuntimeMode::ApprovalRequired, None, None),
            "edit",
            None
        ),
        PermissionDisposition::Ask
    );
    assert_eq!(
        permission_disposition(
            &policy(RuntimeMode::ApprovalRequired, None, None),
            "read",
            None
        ),
        PermissionDisposition::Allow
    );
}

pub async fn grok_native_multiselect_stays_live(
    factory: &dyn AcpFlavorAdapterFactory,
) -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let instance = ProviderInstanceId("grok-multi-select".into());
    let mut base = AcpAdapter::replay(
        instance.clone(),
        Arc::new(ManualClock::new(TEST_TIME)),
        Arc::new(SequenceIds::new("grok-question")),
        false,
    );
    base.driver = ProviderDriverKind("grok".into());
    let question = "Which changes should be included?";
    let transcript = serde_json::from_value(
        json!({"provider":"grok","protocol":"acp.ndjson-jsonrpc","version":"1","scenario":"grok-multi-select","entries":[
            {"type":"expect_outbound","label":"initialize","frame":{"kind":"request","method":"initialize","params":"<any>"}},
            {"type":"emit_inbound","label":"initialized","frame":{"kind":"response","method":"initialize","result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":false}},"authMethods":[{"id":"replay","name":"Replay"}]}}},
            {"type":"expect_outbound","label":"session.new","frame":{"kind":"request","method":"session/new","params":"<any>"}},
            {"type":"emit_inbound","label":"session.created","frame":{"kind":"response","method":"session/new","result":{"sessionId":"grok-multi-select-session","models":{"currentModelId":"grok-build","availableModels":[{"modelId":"grok-build","name":"Grok Build"}]}}}},
            {"type":"expect_outbound","label":"prompt","frame":{"kind":"request","method":"session/prompt","params":"<any>"}},
            {"type":"emit_inbound","label":"question","frame":{"kind":"request","method":"_x.ai/ask_user_question","params":{"method":"x.ai/ask_user_question","params":{"sessionId":"grok-multi-select-session","toolCallId":"grok-question","mode":"default","questions":[{"id":"changes","question":question,"multiSelect":true,"options":[{"label":"Tests","description":"Cover behavior"},{"label":"Docs","description":"Explain usage"}]}]}}}},
            {"type":"expect_outbound","label":"selected answers","frame":{"kind":"response","method":"_x.ai/ask_user_question","result":{"outcome":"accepted","answers":{question:["Tests","Docs"]}}}},
            {"type":"emit_inbound","label":"complete","frame":{"kind":"response","method":"session/prompt","result":{"stopReason":"end_turn"}}}
        ]}),
    )?;
    let harness = ProviderReplayHarness::with_flavor_factory(
        transcript,
        base,
        ReplayGate::default(),
        factory,
    )
    .await?;
    let selection = serde_json::from_value::<t3_contracts::model_selection::ModelSelection>(
        json!({"instanceId":instance,"model":"grok-build"}),
    )?;
    let mut policy = policy(RuntimeMode::ApprovalRequired, None, None);
    policy.cwd = Some(workspace.path().into());
    let thread_id = ThreadId("grok-multi-select-thread".into());
    let runtime = harness
        .adapter
        .open_session(
            OpenSession {
                thread_id: thread_id.clone(),
                provider_session_id: ProviderSessionId("grok-multi-select-session".into()),
                model_selection: selection.clone(),
                runtime_policy: policy.clone(),
                prompt_suggestions: None,
                resume_from_session: None,
                initial_native_thread_id: None,
                initial_provider_item_identity_version: None,
            },
            CancellationToken::new(),
        )
        .await?;
    let result=async {
        let mut events=runtime.subscribe().await?;
        let thread=runtime.ensure_thread(EnsureThread {thread_id,title:None,model_selection:selection.clone(),runtime_policy:policy.clone(),provider_session_id:None,existing_provider_thread:None,force_new_native_thread:false}).await?;
        runtime.start_turn(make_turn_input(thread,selection,policy,ManualClock::new(TEST_TIME).now(),1,"Ask which changes to include.")?).await?;
        let mut first_question=None;
        let mut pending=None;
        let mut response=None;
        loop {
            match events.events.recv().await.ok_or(ProviderError::SubscriptionClosed)??.kind {
                ProviderEventKind::TurnItem {item,..} => {
                    let item=serde_json::to_value(item)?;
                    if item["type"]=="user_input_request" && first_question.is_none() { first_question=item["questions"].as_array().and_then(|questions|questions.first()).cloned(); }
                },
                ProviderEventKind::RuntimeRequest {request,..} if request.status==t3_contracts::orchestration_v2::RuntimeRequestStatus::Pending => {
                    let request=serde_json::to_value(&request)?;
                    assert_eq!(request["kind"],"user_input");assert_eq!(request["responseCapability"]["type"],"live");
                    pending=Some(RuntimeRequestId(request["id"].as_str().expect("request ID").into()));
                },
                ProviderEventKind::Terminal {..} => break,
                _=>{},
            }
            if response.is_none() && first_question.is_some() && let Some(id)=pending.take() {
                assert_eq!(first_question.as_ref().expect("question arrived"),&json!({"id":"changes","header":"Question","question":question,"multiSelect":true,"options":[{"label":"Tests","description":"Cover behavior"},{"label":"Docs","description":"Explain usage"}]}));
                let runtime=runtime.clone();
                response=Some(tokio::spawn(async move { runtime.respond(RuntimeResponse {request_id:id,decision:None,feedback:None,answers:Some(serde_json::from_value(json!({"changes":["Tests","Docs"]})).expect("answer object"))}).await }));
            }
        }
        response.expect("live question was answered").await.map_err(|error| t3_orchestration::error::Error::Service {operation:"Grok answer",detail:error.to_string()})??;
        Ok::<_,t3_orchestration::error::Error>(())
    }.await;
    let close = runtime.close().await;
    result?;
    close?;
    harness.assert_complete()?;
    Ok(())
}
