#![cfg(feature = "acp-adapter")]

use serde_json::json;
use std::sync::Arc;
use t3_contracts::{
    base_schemas::*,
    model_selection::ModelSelection,
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use t3_orchestration::{
    adapters::acp::AcpAdapter,
    api::Clock,
    ports::acp_replay::{
        adapters::{
            acp_adapter_v2_testkit::make_turn_input,
            acp_registry_adapter_v2_testkit::make_acp_registry_replay_harness,
        },
        testkit::provider_replay_fixtures::find_replay_fixture,
    },
    provider::*,
    testing::{ManualClock, SequenceIds, TEST_TIME, replay::ReplayGate},
};
use tokio_util::sync::CancellationToken;

#[tokio::test(start_paused = true)]
async fn source_registry_recording_runs_through_acp_and_preserves_complete_text() {
    let recording = find_replay_fixture("simple", "acpRegistry")
        .unwrap()
        .unwrap();
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let mut adapter = AcpAdapter::replay(
        ProviderInstanceId("registry-replay".into()),
        clock.clone(),
        Arc::new(SequenceIds::new("strict")),
        false,
    );
    adapter.initialize = json!({"protocolVersion":2,"info":{"name":"t3-code","version":"1"},"capabilities":{},"clientInfo":{"name":"t3-code","version":"1"},"clientCapabilities":{}});
    let harness =
        make_acp_registry_replay_harness(recording.transcript, adapter, ReplayGate::default())
            .unwrap();
    let policy = RuntimePolicy {
        runtime_mode: RuntimeMode::ApprovalRequired,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: Some(".".into()),
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    };
    let selection: ModelSelection =
        serde_json::from_value(json!({"instanceId":"registry-replay","model":"grok-build"}))
            .unwrap();
    let runtime = harness
        .adapter
        .open_session(
            OpenSession {
                thread_id: ThreadId("thread:strict".into()),
                provider_session_id: ProviderSessionId("session:strict".into()),
                model_selection: selection.clone(),
                runtime_policy: policy.clone(),
                prompt_suggestions: None,
                resume_from_session: None,
                initial_native_thread_id: None,
                initial_provider_item_identity_version: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut events = runtime.subscribe().await.unwrap();
    let thread = runtime
        .ensure_thread(EnsureThread {
            thread_id: ThreadId("thread:strict".into()),
            title: None,
            model_selection: selection.clone(),
            runtime_policy: policy.clone(),
            provider_session_id: None,
            existing_provider_thread: None,
            force_new_native_thread: false,
        })
        .await
        .unwrap();
    runtime.start_turn(make_turn_input(thread.clone(),selection,policy,clock.now(),1,"<t3_code_instructions>\nReplay test instructions\n</t3_code_instructions>\n\n<user_request>\nRespond with the following text: fixture simple ok\n</user_request>").unwrap()).await.unwrap();
    let mut messages = Vec::new();
    let mut reasoning = false;
    loop {
        match events.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::Message(message) if !message.streaming => {
                messages.push(message.text)
            }
            ProviderEventKind::TurnItem { item, .. } => {
                reasoning |= serde_json::to_value(item).unwrap()["type"] == "reasoning"
            }
            ProviderEventKind::Terminal { outcome, .. } => {
                assert!(matches!(outcome, TerminalOutcome::Completed { .. }));
                break;
            }
            _ => {}
        }
    }
    assert_eq!(messages, ["fixture simple ok"]);
    assert!(reasoning);
    harness.assert_complete().unwrap();
    assert!(
        runtime
            .snapshot(thread)
            .await
            .unwrap()
            .messages
            .iter()
            .all(|message| !message.streaming)
    );
    runtime.close().await.unwrap();
}

#[test]
fn registry_replay_refuses_non_acp_recordings_and_wrong_adapter_drivers() {
    let recording = find_replay_fixture("simple", "codex").unwrap().unwrap();
    let adapter = || {
        AcpAdapter::replay(
            ProviderInstanceId("registry-replay".into()),
            Arc::new(ManualClock::new(TEST_TIME)),
            Arc::new(SequenceIds::new("strict")),
            false,
        )
    };
    assert!(
        make_acp_registry_replay_harness(recording.transcript, adapter(), ReplayGate::default())
            .is_err()
    );
    let recording = find_replay_fixture("simple", "acpRegistry")
        .unwrap()
        .unwrap();
    let mut wrong = adapter();
    wrong.driver.0 = "codex".into();
    assert!(
        make_acp_registry_replay_harness(recording.transcript, wrong, ReplayGate::default())
            .is_err()
    );
}
