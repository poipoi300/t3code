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
    failure: Option<String>,
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
    let workspace = tempfile::tempdir()?;
    let transcript = serde_json::from_value(
        json!({"provider":driver,"protocol":"acp.ndjson-jsonrpc","version":"1","scenario":"approval-security","entries":[
            {"type":"expect_outbound","frame":{"kind":"request","method":"initialize","params":"<any>"}},
            {"type":"emit_inbound","frame":{"kind":"response","method":"initialize","result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}},
            {"type":"expect_outbound","frame":{"kind":"request","method":"session/new","params":"<any>"}},
            {"type":"emit_inbound","frame":{"kind":"response","method":"session/new","result":{"sessionId":"approval-session"}}},
            {"type":"expect_outbound","frame":{"kind":"request","method":"session/prompt","params":"<any>"}},
            {"type":"emit_inbound","frame":{"kind":"request","method":method,"params":params}},
            {"type":"expect_outbound","frame":{"kind":"response","method":method,"result":response}},
            {"type":"emit_inbound","frame":{"kind":"response","method":"session/prompt","result":{"stopReason":"end_turn"}}}
        ]}),
    )?;
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let mut adapter = AcpAdapter::replay(
        ProviderInstanceId("approval".into()),
        clock.clone(),
        Arc::new(SequenceIds::new("approval")),
        false,
    );
    adapter.driver = ProviderDriverKind(driver.into());
    let harness = ProviderReplayHarness::new(transcript, adapter, ReplayGate::default(), false)?;
    let selection = serde_json::from_value(json!({"instanceId":"approval","model":"default"}))?;
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
                thread,
                selection,
                policy,
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
        .or_else(|| close.err().map(|error| error.to_string()));
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
    for (mode, mcp, pending, expected) in [
        (
            RuntimeMode::Auto,
            true,
            0,
            json!({"action":"accept","content":{}}),
        ),
        (
            RuntimeMode::FullAccess,
            true,
            0,
            json!({"action":"accept","content":{}}),
        ),
        (
            RuntimeMode::ApprovalRequired,
            true,
            1,
            json!({"action":"cancel"}),
        ),
        (RuntimeMode::Auto, false, 1, json!({"action":"cancel"})),
    ] {
        let mut params = json!({"sessionId":"approval-session","mode":"form","message":"Approve MCP tool","requestedSchema":{"type":"object","properties":{"approve":{"type":"boolean"}},"required":["approve"]}});
        if mcp {
            params["_meta"] = json!({"codex_approval_kind":"mcp_tool_call"});
        }
        let observed = probe(
            "acpRegistry",
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
