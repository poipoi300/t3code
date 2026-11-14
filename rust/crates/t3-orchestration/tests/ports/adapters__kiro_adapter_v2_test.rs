#![cfg(feature = "acp-adapter")]

use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::{ProviderApprovalDecision, ProviderInteractionMode, RuntimeMode},
};
use t3_orchestration::{
    adapters::acp::AcpAdapter,
    api::Clock,
    ports::acp_replay::{
        adapters::acp_adapter_v2_testkit::make_turn_input,
        testkit::provider_replay_harness::ProviderReplayHarness,
    },
    provider::*,
    testing::{ManualClock, SequenceIds, TEST_TIME, replay::ReplayGate},
};
use tokio_util::sync::CancellationToken;

struct Observation {
    cards: Vec<Value>,
    pending: usize,
    failure: Option<Value>,
    completed: bool,
}
fn options() -> Value {
    json!([
        {"optionId":"accept","name":"Allow","kind":"allow_once"},
        {"optionId":"always-accept","name":"Always allow","kind":"allow_always"},
        {"optionId":"reject","name":"Deny","kind":"reject_once"}
    ])
}
async fn probe(
    driver: &str,
    mode: RuntimeMode,
    method: &str,
    params: Value,
    response: Value,
    decision: ProviderApprovalDecision,
) -> Result<Observation, Box<dyn std::error::Error>> {
    probe_with_grant(driver, mode, method, params, response, decision, None).await
}
struct InspectGrant;
#[async_trait::async_trait]
impl t3_orchestration::adapters::acp::AcpClientEffects for InspectGrant {
    async fn request(
        &self,
        request: t3_acp::ClientRequest,
        context: t3_orchestration::adapters::acp::AcpEffectContext,
    ) -> Result<Value, t3_acp::RpcError> {
        match request {
            t3_acp::ClientRequest::TerminalCreate(_) => {
                Ok(json!({"executeApproved":context.execute_approved}))
            }
            _ => Err(t3_acp::RpcError::invalid_params()),
        }
    }
}
async fn probe_with_grant(
    driver: &str,
    mode: RuntimeMode,
    method: &str,
    params: Value,
    response: Value,
    decision: ProviderApprovalDecision,
    next_turn_grant: Option<bool>,
) -> Result<Observation, Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let mut transcript: Value = json!({"provider":driver,"protocol":"acp.ndjson-jsonrpc","version":"1","scenario":"approval-security","entries":[
        {"type":"expect_outbound","frame":{"kind":"request","method":"initialize","params":"<any>"}},
        {"type":"emit_inbound","frame":{"kind":"response","method":"initialize","result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}},
        {"type":"expect_outbound","frame":{"kind":"request","method":"session/new","params":"<any>"}},
        {"type":"emit_inbound","frame":{"kind":"response","method":"session/new","result":{"sessionId":"approval-session"}}},
        {"type":"expect_outbound","frame":{"kind":"request","method":"session/prompt","params":"<any>"}},
        {"type":"emit_inbound","frame":{"kind":"request","method":method,"params":params}},
        {"type":"expect_outbound","frame":{"kind":"response","method":method,"result":response}},
        {"type":"emit_inbound","frame":{"kind":"response","method":"session/prompt","result":{"stopReason":"end_turn"}}}
    ]});
    if let Some(grant) = next_turn_grant {
        transcript["entries"].as_array_mut().ok_or("entries")?.extend([
            json!({"type":"expect_outbound","frame":{"kind":"request","method":"session/prompt","params":"<any>"}}),
            json!({"type":"emit_inbound","frame":{"kind":"request","method":"terminal/create","params":{"sessionId":"approval-session","command":"never-executed"}}}),
            json!({"type":"expect_outbound","frame":{"kind":"response","method":"terminal/create","result":{"executeApproved":grant}}}),
            json!({"type":"emit_inbound","frame":{"kind":"response","method":"session/prompt","result":{"stopReason":"end_turn"}}}),
        ]);
    }
    let transcript = serde_json::from_value(transcript)?;
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let mut adapter = AcpAdapter::replay(
        ProviderInstanceId("approval".into()),
        clock.clone(),
        Arc::new(SequenceIds::new("approval")),
        false,
    );
    adapter.driver = ProviderDriverKind(driver.into());
    adapter.effects = Arc::new(InspectGrant);
    let harness = ProviderReplayHarness::new(transcript, adapter, ReplayGate::default(), false)?;
    let selection = serde_json::from_value::<t3_contracts::model_selection::ModelSelection>(
        json!({"instanceId":"approval","model":"default"}),
    )?;
    let policy = RuntimePolicy {
        runtime_mode: mode,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: Some(workspace.path().into()),
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    };
    let runtime = harness
        .adapter
        .open_session(
            OpenSession {
                thread_id: ThreadId("approval-thread".into()),
                provider_session_id: ProviderSessionId("approval-session".into()),
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
    let mut responders = Vec::new();
    let result = async {
        let mut events = runtime.subscribe().await?;
        let thread = runtime
            .ensure_thread(EnsureThread {
                thread_id: ThreadId("approval-thread".into()),
                title: None,
                model_selection: selection.clone(),
                runtime_policy: policy.clone(),
                provider_session_id: None,
                existing_provider_thread: None,
                force_new_native_thread: false,
            })
            .await?;
        runtime
            .start_turn(make_turn_input(
                thread.clone(),
                selection.clone(),
                policy.clone(),
                clock.now(),
                1,
                "Approval security regression",
            )?)
            .await?;
        let mut cards = Vec::new();
        let mut pending = 0;
        let completed = loop {
            match events
                .events
                .recv()
                .await
                .ok_or(ProviderError::SubscriptionClosed)??
                .kind
            {
                ProviderEventKind::TurnItem { item, .. } => {
                    let item = serde_json::to_value(item)?;
                    if matches!(
                        item["type"].as_str(),
                        Some("approval_request" | "user_input_request")
                    ) {
                        cards.push(item);
                    }
                }
                ProviderEventKind::RuntimeRequest { request, .. }
                    if serde_json::to_value(&request)?["status"] == "pending" =>
                {
                    pending += 1;
                    let id = serde_json::to_value(request)?["id"]
                        .as_str()
                        .ok_or("request ID")?
                        .to_owned();
                    let runtime = runtime.clone();
                    responders.push(tokio::spawn(async move {
                        runtime
                            .respond(RuntimeResponse {
                                request_id: RuntimeRequestId(id),
                                decision: Some(decision),
                                feedback: None,
                                answers: None,
                            })
                            .await
                    }));
                }
                ProviderEventKind::Terminal { outcome, .. } => {
                    break matches!(outcome, TerminalOutcome::Completed { .. });
                }
                _ => {}
            }
        };
        let completed = if next_turn_grant.is_some() && completed {
            runtime
                .start_turn(make_turn_input(
                    thread,
                    selection,
                    policy,
                    clock.now(),
                    2,
                    "Check grant scope",
                )?)
                .await?;
            loop {
                if let ProviderEventKind::Terminal { outcome, .. } = events
                    .events
                    .recv()
                    .await
                    .ok_or(ProviderError::SubscriptionClosed)??
                    .kind
                {
                    break matches!(outcome, TerminalOutcome::Completed { .. });
                }
            }
        } else {
            completed
        };
        Ok::<_, Box<dyn std::error::Error>>((cards, pending, completed))
    }
    .await;
    let close = runtime.close().await;
    for responder in responders {
        let _ = responder.await?;
    }
    let (cards, pending, completed) = result?;
    let failure = harness
        .progress
        .borrow()
        .failure
        .clone()
        .or_else(|| close.err().map(|error| json!(error.to_string())));
    Ok(Observation {
        cards,
        pending,
        failure,
        completed,
    })
}
fn request(options: Value, kind: &str) -> Value {
    json!({"sessionId":"approval-session","toolCall":{"toolCallId":"call-1","title":"Write File","kind":kind},"options":options})
}
fn selected(id: &str) -> Value {
    json!({"outcome":{"outcome":"selected","optionId":id}})
}
fn assert_success(observed: &Observation) {
    assert!(observed.failure.is_none(), "{:?}", observed.failure);
    assert!(observed.completed);
}

#[tokio::test(start_paused = true)]
async fn vendor_cards_downgrade_stale_session_answers_before_selecting_native_options()
-> Result<(), Box<dyn std::error::Error>> {
    for (driver, mut native, expected, session_label) in [
        ("kiro", options(), "accept", None),
        ("grok", options(), "accept", None),
        (
            "grok",
            options(),
            "allow-edits-session",
            Some("Allow all edits this session"),
        ),
        (
            "kiro",
            json!([{"optionId":"always-accept","name":"Always allow","kind":"allow_always"},{"optionId":"reject","name":"Deny","kind":"reject_once"}]),
            "always-accept",
            Some("Always allow"),
        ),
    ] {
        if expected == "allow-edits-session" {
            native[1]["optionId"] = json!(expected);
        }
        let observed = probe(
            driver,
            RuntimeMode::ApprovalRequired,
            "session/request_permission",
            request(native, "edit"),
            selected(expected),
            ProviderApprovalDecision::AcceptForSession,
        )
        .await?;
        assert_success(&observed);
        assert_eq!(observed.pending, 1);
        let card = observed
            .cards
            .iter()
            .find(|item| item["type"] == "approval_request")
            .ok_or("approval card")?;
        let choices = card["options"].as_array().ok_or("vendor choices")?;
        assert_eq!(
            choices
                .iter()
                .find(|option| option["decision"] == "acceptForSession")
                .and_then(|option| option["label"].as_str()),
            session_label,
            "{driver}"
        );
        assert!(choices.iter().any(|option| option["decision"] == "cancel"));
    }
    let unavailable = probe(
        "grok",
        RuntimeMode::ApprovalRequired,
        "session/request_permission",
        request(
            json!([{"kind":"allow_always","optionId":"workspace","name":"Workspace rule"}]),
            "edit",
        ),
        json!({"outcome":{"outcome":"cancelled"}}),
        ProviderApprovalDecision::AcceptForSession,
    )
    .await?;
    assert_success(&unavailable);
    assert_eq!(unavailable.pending, 1);
    let mut mixed = options();
    mixed.as_array_mut().ok_or("options")?.push(
        json!({"kind":"allow_always","optionId":"allow-edits-session","name":"Session edits"}),
    );
    let scoped = probe(
        "grok",
        RuntimeMode::ApprovalRequired,
        "session/request_permission",
        request(mixed, "edit"),
        selected("allow-edits-session"),
        ProviderApprovalDecision::AcceptForSession,
    )
    .await?;
    assert_success(&scoped);
    let forged = probe(
        "grok",
        RuntimeMode::ApprovalRequired,
        "session/request_permission",
        request(options(), "edit"),
        selected("reject"),
        ProviderApprovalDecision::AcceptAlways,
    )
    .await?;
    assert_success(&forged);
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn decline_with_only_reject_always_cancels_without_persisting_a_rule()
-> Result<(), Box<dyn std::error::Error>> {
    let observed = probe(
        "acpRegistry",
        RuntimeMode::ApprovalRequired,
        "session/request_permission",
        request(
            json!([{"optionId":"persistent-denial","name":"Never allow","kind":"reject_always"}]),
            "edit",
        ),
        json!({"outcome":{"outcome":"cancelled"}}),
        ProviderApprovalDecision::Decline,
    )
    .await?;
    assert_success(&observed);
    assert_eq!(observed.pending, 1);
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn unoffered_or_unknown_runtime_modes_ask_before_writes()
-> Result<(), Box<dyn std::error::Error>> {
    for (driver, mode) in [
        ("muse", RuntimeMode::Auto),
        ("pi", RuntimeMode::Auto),
        ("unknown-acp", RuntimeMode::FullAccess),
        ("fx", RuntimeMode::Auto),
        ("grok", RuntimeMode::Auto),
    ] {
        let observed = probe(
            driver,
            mode,
            "session/request_permission",
            request(options(), "edit"),
            selected("reject"),
            ProviderApprovalDecision::Decline,
        )
        .await?;
        assert_success(&observed);
        assert_eq!(observed.pending, 1, "{driver}");
    }
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn mcp_approval_elicitations_auto_allow_only_the_recognized_non_supervised_form()
-> Result<(), Box<dyn std::error::Error>> {
    for (driver, mode, mcp, pending, expected) in [
        (
            "acpRegistry",
            RuntimeMode::Auto,
            true,
            0,
            json!({"action":"accept","content":{}}),
        ),
        (
            "acpRegistry",
            RuntimeMode::FullAccess,
            true,
            0,
            json!({"action":"accept","content":{}}),
        ),
        (
            "acpRegistry",
            RuntimeMode::ApprovalRequired,
            true,
            1,
            json!({"action":"cancel"}),
        ),
        (
            "acpRegistry",
            RuntimeMode::Auto,
            false,
            1,
            json!({"action":"cancel"}),
        ),
        (
            "muse",
            RuntimeMode::Auto,
            true,
            1,
            json!({"action":"cancel"}),
        ),
        ("pi", RuntimeMode::Auto, true, 1, json!({"action":"cancel"})),
        (
            "unknown-acp",
            RuntimeMode::FullAccess,
            true,
            1,
            json!({"action":"cancel"}),
        ),
    ] {
        let mut params = json!({"sessionId":"approval-session","mode":"form","message":"Approve MCP tool","requestedSchema":{"type":"object","properties":{"approve":{"type":"boolean"}},"required":["approve"]}});
        if mcp {
            params["_meta"] = json!({"codex_approval_kind":"mcp_tool_call"});
        }
        let observed = probe(
            driver,
            mode,
            "elicitation/create",
            params,
            expected,
            ProviderApprovalDecision::Cancel,
        )
        .await?;
        assert_success(&observed);
        assert_eq!(observed.pending, pending);
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn mcp_auto_approval_still_requires_an_active_turn() -> Result<(), Box<dyn std::error::Error>>
{
    let transcript = serde_json::from_value(
        json!({"provider":"acpRegistry","protocol":"acp.ndjson-jsonrpc","version":"1","scenario":"mcp-outside-turn","entries":[
            {"type":"expect_outbound","frame":{"kind":"request","method":"initialize","params":"<any>"}},
            {"type":"emit_inbound","frame":{"kind":"response","method":"initialize","result":{"protocolVersion":1}}},
            {"type":"expect_outbound","frame":{"kind":"request","method":"session/new","params":"<any>"}},
            {"type":"emit_inbound","frame":{"kind":"response","method":"session/new","result":{"sessionId":"approval-session"}}},
            {"type":"emit_inbound","frame":{"kind":"request","method":"elicitation/create","params":{"sessionId":"approval-session","mode":"form","_meta":{"codex_approval_kind":"mcp_tool_call"},"message":"Approve MCP tool","requestedSchema":{"type":"object","properties":{}}}}},
            {"type":"expect_outbound","frame":{"kind":"response","method":"elicitation/create","error":{"code":-32602,"message":"Invalid params"}}}
        ]}),
    )?;
    let adapter = AcpAdapter::replay(
        ProviderInstanceId("approval".into()),
        Arc::new(ManualClock::new(TEST_TIME)),
        Arc::new(SequenceIds::new("mcp-outside-turn")),
        false,
    );
    let harness = ProviderReplayHarness::new(transcript, adapter, ReplayGate::default(), false)?;
    let selection = serde_json::from_value::<t3_contracts::model_selection::ModelSelection>(
        json!({"instanceId":"approval","model":"default"}),
    )?;
    let policy = RuntimePolicy {
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: None,
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    };
    let runtime = harness
        .adapter
        .open_session(
            OpenSession {
                thread_id: ThreadId("approval-thread".into()),
                provider_session_id: ProviderSessionId("approval-session".into()),
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
    let result = async {
        runtime
            .ensure_thread(EnsureThread {
                thread_id: ThreadId("approval-thread".into()),
                title: None,
                model_selection: selection,
                runtime_policy: policy,
                provider_session_id: None,
                existing_provider_thread: None,
                force_new_native_thread: false,
            })
            .await?;
        let mut progress = harness.progress.clone();
        progress
            .wait_for(|status| status.failure.is_some() || status.cursor == status.total)
            .await?;
        harness.assert_complete()?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    let closed = runtime.close().await;
    result?;
    closed?;
    Ok(())
}

#[tokio::test]
async fn vendor_approval_cards_equal_the_actual_typescript_scope_rules()
-> Result<(), Box<dyn std::error::Error>> {
    use t3_orchestration::adapters::acp::{
        PermissionDisposition, approval_options, permission_disposition_for_driver,
    };
    let candidates = json!([
        {"kind":"allow_once","optionId":"once","name":"Once"},
        {"kind":"allow_always","optionId":"workspace","name":"Workspace rule"},
        {"kind":"allow_always","optionId":" allow-edits-session ","name":"Session edits"},
        {"kind":"reject_once","optionId":"deny","name":"Deny"},
        {"kind":"reject_always","optionId":"never","name":"Never"}
    ]);
    let mut requests = Vec::new();
    for driver in ["kiro", "grok"] {
        for mask in 0..32 {
            let choices: Vec<_> = candidates
                .as_array()
                .ok_or("options")?
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, option)| option.clone())
                .collect();
            requests.push(json!({"driver":driver,"request":request(json!(choices),"edit")}));
        }
        for id in ["", " \u{feff}", "valid"] {
            requests.push(json!({"driver":driver,"request":request(json!([{"kind":"allow_once","optionId":id,"name":"Once"},{"kind":"allow_always","optionId":"always","name":"Always"},{"kind":"reject_once","optionId":id,"name":"Deny"}]),"edit")}));
        }
    }
    let mut process=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {stripTypeScriptTypes} from 'node:module';
import {readFileSync} from 'node:fs';
const extract=(path,name)=>{const source=readFileSync(path,'utf8');const start=source.indexOf(`export function ${name}(`);const end=source.indexOf('\n}',start);if(start<0||end<0)throw new Error(name);return source.slice(start,end+2);};
const source=readFileSync('packages/provider-grok/src/server/acpSupport.ts','utf8');
const start=source.indexOf('const GROK_ALLOW_EDITS_SESSION_OPTION_ID');
const code=source.slice(start,source.indexOf(';',start)+1)+'\n'+extract('packages/provider-kiro/src/server/acpSupport.ts','kiroApprovalOptions')+'\n'+extract('packages/provider-grok/src/server/acpSupport.ts','grokApprovalOptions');
const {kiroApprovalOptions,grokApprovalOptions}=await import('data:text/javascript,'+encodeURIComponent(stripTypeScriptTypes(code)));
const rows=JSON.parse(readFileSync(0,'utf8'));
process.stdout.write(JSON.stringify(rows.map(({driver,request})=>(driver==='kiro'?kiroApprovalOptions:grokApprovalOptions)(request))));
"#]);
    process.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    process.stdin = Some(serde_json::to_vec(&requests)?);
    let expected: Vec<Value> = serde_json::from_slice(&t3_process::run(process).await?.stdout)?;
    assert_eq!(expected.len(), requests.len());
    for (input, expected) in requests.iter().zip(expected) {
        assert_eq!(
            serde_json::to_value(approval_options(
                input["driver"].as_str().ok_or("driver")?,
                &input["request"]
            ))?,
            expected,
            "{input}"
        );
    }
    let policy = RuntimePolicy {
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: None,
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    };
    let offered = [RuntimeMode::ApprovalRequired, RuntimeMode::FullAccess];
    for kind in ["read", "edit", "execute"] {
        let always = request(
            json!([{"kind":"allow_always","optionId":"workspace","name":"Always"}]),
            kind,
        );
        assert_eq!(
            permission_disposition_for_driver("kiro", &policy, &always, Some(&offered)),
            PermissionDisposition::Ask
        );
        assert_eq!(
            permission_disposition_for_driver(
                "kiro",
                &policy,
                &request(options(), kind),
                Some(&offered)
            ),
            PermissionDisposition::Allow
        );
    }
    Ok(())
}

#[tokio::test]
async fn mcp_approval_disposition_equals_typescript_for_modes_overrides_and_request_ids()
-> Result<(), Box<dyn std::error::Error>> {
    use t3_orchestration::adapters::acp::{PermissionDisposition, mcp_tool_approval_disposition};
    let mut rows = Vec::new();
    for mode in [
        "approval-required",
        "auto-accept-edits",
        "auto",
        "full-access",
    ] {
        for approval in [None, Some("never"), Some("on-request")] {
            for form in [false, true] {
                for metadata in [None, Some("other"), Some("mcp_tool_call")] {
                    for id in [None, Some("ordinary"), Some("mcp_tool_call_approval_123")] {
                        let mut policy = json!({"runtimeMode":mode,"cwd":null});
                        if let Some(approval) = approval {
                            policy["approvalPolicy"] = json!(approval);
                        }
                        let mut request = json!({"mode":if form {"form"}else{"url"}});
                        if let Some(metadata) = metadata {
                            request["_meta"] = json!({"codex_approval_kind":metadata});
                        }
                        rows.push(json!({"policy":policy,"request":request,"id":id}));
                    }
                }
            }
        }
    }
    let mut process=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {readFileSync} from 'node:fs';
import {acpMcpToolApprovalElicitationDisposition} from './packages/provider-acp/src/server/clientPolicy.ts';
process.stdout.write(JSON.stringify(JSON.parse(readFileSync(0,'utf8')).map(({policy,request,id})=>acpMcpToolApprovalElicitationDisposition(policy,request,id??undefined))));
"#]);
    process.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    process.stdin = Some(serde_json::to_vec(&rows)?);
    let expected: Vec<Value> = serde_json::from_slice(&t3_process::run(process).await?.stdout)?;
    assert_eq!(expected.len(), rows.len());
    for (input, expected) in rows.iter().zip(expected) {
        let policy = RuntimePolicy {
            runtime_mode: serde_json::from_value(input["policy"]["runtimeMode"].clone())?,
            interaction_mode: ProviderInteractionMode::Default,
            cwd: None,
            approval_policy: input["policy"].get("approvalPolicy").cloned(),
            sandbox_policy: None,
            max_runtime_mode: None,
        };
        let actual =
            match mcp_tool_approval_disposition(&policy, &input["request"], input["id"].as_str()) {
                Some(PermissionDisposition::Allow) => json!("allow"),
                Some(PermissionDisposition::Ask) => json!("ask"),
                None => Value::Null,
                other => panic!("unexpected MCP disposition {other:?}"),
            };
        assert_eq!(actual, expected, "{input}");
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stale_vendor_session_answers_do_not_leak_execute_grants_into_the_next_turn()
-> Result<(), Box<dyn std::error::Error>> {
    for (driver, selected_id, grant) in [
        ("kiro", "accept", false),
        ("grok", "accept", false),
        ("acpRegistry", "always-accept", true),
    ] {
        let observed = probe_with_grant(
            driver,
            RuntimeMode::ApprovalRequired,
            "session/request_permission",
            request(options(), "execute"),
            selected(selected_id),
            ProviderApprovalDecision::AcceptForSession,
            Some(grant),
        )
        .await?;
        assert_success(&observed);
        assert_eq!(observed.pending, 1);
    }
    Ok(())
}

#[test]
fn unknown_mode_lists_cannot_enable_overrides_or_weaken_denials() {
    use t3_orchestration::adapters::acp::{
        PermissionDisposition, permission_disposition_for_driver,
    };
    let mut policy = RuntimePolicy {
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: None,
        approval_policy: Some(json!("never")),
        sandbox_policy: None,
        max_runtime_mode: None,
    };
    for modes in [None, Some([].as_slice())] {
        assert_eq!(
            permission_disposition_for_driver("muse", &policy, &request(options(), "edit"), modes),
            PermissionDisposition::Ask
        );
        policy.sandbox_policy = Some(json!({"type":"readOnly"}));
        assert_eq!(
            permission_disposition_for_driver("muse", &policy, &request(options(), "edit"), modes),
            PermissionDisposition::Deny
        );
        policy.sandbox_policy = None;
    }
}

#[tokio::test]
async fn native_mode_declarations_equal_vendor_status_and_launch_lists()
-> Result<(), Box<dyn std::error::Error>> {
    let mut process=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {readFileSync} from 'node:fs';
const rows=[];
for(const [driver,file,marker] of [
    ['kiro','packages/provider-kiro/src/server/acpSupport.ts','export const KIRO_SUPPORTED_RUNTIME_MODES'],
    ['grok','packages/provider-grok/src/server/acpSupport.ts','export const GROK_SUPPORTED_RUNTIME_MODES'],
    ['fx','packages/provider-fx/src/server/acpSupport.ts','export const FX_SUPPORTED_RUNTIME_MODES'],
    ['muse','packages/provider-muse/src/server/status.ts','supportedRuntimeModes:'],
    ['pi','packages/provider-pi/src/server/status.ts','supportedRuntimeModes:'],
]) {
    const source=readFileSync(file,'utf8');const start=source.indexOf(marker);const begin=source.indexOf('[',start);const end=source.indexOf(']',begin);
    if(start<0||begin<0||end<0)throw new Error(driver);
    rows.push([driver,JSON.parse(source.slice(begin,end+1).replace(/,\s*]/g,']'))]);
}
process.stdout.write(JSON.stringify(rows));
"#]);
    process.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let expected: Vec<(String, Vec<RuntimeMode>)> =
        serde_json::from_slice(&t3_process::run(process).await?.stdout)?;
    let mut adapter = AcpAdapter::replay(
        ProviderInstanceId("approval".into()),
        Arc::new(ManualClock::new(TEST_TIME)),
        Arc::new(SequenceIds::new("approval")),
        false,
    );
    for (driver, modes) in expected {
        adapter.driver = ProviderDriverKind(driver.clone());
        assert_eq!(
            adapter
                .supported_runtime_modes
                .get(&ProviderDriverKind(driver.clone())),
            Some(&modes),
            "{driver}"
        );
        assert_eq!(
            ProviderAdapter::supported_runtime_modes(&adapter),
            Some(modes),
            "registry policy must receive {driver}'s declared modes"
        );
    }
    adapter.driver = ProviderDriverKind("unknown-acp".into());
    assert_eq!(ProviderAdapter::supported_runtime_modes(&adapter), None);
    Ok(())
}
