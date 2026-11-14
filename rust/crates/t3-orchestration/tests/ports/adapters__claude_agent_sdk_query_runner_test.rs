#![cfg(feature = "claude-adapter")]

use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use t3_claude_cli::{Session, SessionOptions, launch::LaunchOptions};
use t3_contracts::{
    base_schemas::*, provider_instance::ProviderInstanceId,
    provider_policy::ProviderApprovalDecision,
};
use t3_orchestration::{
    adapters::claude::*,
    ports::claude::adapters::claude_adapter_v2_testkit::*,
    provider::*,
    testing::{ManualClock, SequenceIds},
};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio_util::sync::CancellationToken;

struct Peer {
    read: Lines<BufReader<ReadHalf<DuplexStream>>>,
    write: WriteHalf<DuplexStream>,
}
impl Peer {
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.write.write_all(&bytes).await.unwrap();
    }
    async fn read(&mut self) -> Value {
        serde_json::from_str(&self.read.next_line().await.unwrap().expect("peer frame")).unwrap()
    }
}
async fn running() -> (Arc<dyn SessionRuntime>, EventSubscription, Peer) {
    running_with_host(ClaudeHostOptions::default()).await
}
async fn running_with_host(
    host: ClaudeHostOptions,
) -> (Arc<dyn SessionRuntime>, EventSubscription, Peer) {
    let (client, server) = tokio::io::duplex(65536);
    let (read, write) = tokio::io::split(client);
    let (peer_read, peer_write) = tokio::io::split(server);
    let connector = Arc::new(PipeConnector::new(Session::connect(
        read,
        write,
        SessionOptions::default(),
    )));
    let input = fixture_turn_input().unwrap();
    let adapter = ClaudeAdapterV2::new(ClaudeAdapterOptions {
        instance_id: ProviderInstanceId("claudeAgent".into()),
        launch: LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..LaunchOptions::default()
        },
        connector,
        clock: Arc::new(ManualClock::new(FIXTURE_DATE)),
        ids: Arc::new(SequenceIds::new("runtime")),
        new_native_session_id: Arc::new(|| "00000000-0000-4000-8000-000000000001".into()),
        mcp_servers: BTreeMap::new(),
        transcript_root: None,
        attachments: Arc::new(DirectoryClaudeAttachments::default()),
        host,
    });
    let runtime = adapter
        .open_session(
            OpenSession {
                thread_id: input.thread_id.clone(),
                provider_session_id: ProviderSessionId("session".into()),
                model_selection: input.model_selection.clone(),
                runtime_policy: input.runtime_policy.clone(),
                prompt_suggestions: None,
                resume_from_session: None,
                initial_native_thread_id: None,
                initial_provider_item_identity_version: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let subscription = runtime.subscribe().await.unwrap();
    let mut peer = Peer {
        read: BufReader::new(peer_read).lines(),
        write: peer_write,
    };
    let handle = runtime.clone();
    let start = tokio::spawn(async move { handle.start_turn(input).await });
    let init = peer.read().await;
    assert_eq!(init["request"]["subtype"], "initialize");
    assert!(
        init["request"].get("systemPrompt").is_none(),
        "default chat sessions retain Claude Code's system prompt"
    );
    peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":init["request_id"],"response":{}}})).await;
    let user = peer.read().await;
    assert_eq!(user["type"], "user");
    start.await.unwrap().unwrap();
    (runtime, subscription, peer)
}

#[tokio::test]
async fn native_background_wake_waits_for_an_app_run_and_never_offers_a_second_prompt() {
    use t3_contracts::orchestration_v2::{OrchestrationV2Actor, OrchestrationV2CreationSource};
    let (send, mut requests) = tokio::sync::mpsc::channel(4);
    let host = ClaudeHostOptions {
        continuation_requests: Some(send),
        ..Default::default()
    };
    let (runtime, mut subscription, mut peer) = running_with_host(host).await;
    peer.send(json!({"type":"system","subtype":"background_tasks_changed","uuid":"roster","tasks":[{"task_id":"background","task_type":"local_bash","description":"Build"}]})).await;
    let result = |id: &str, text: &str| json!({"type":"result","subtype":"success","uuid":id,"result":text,"is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}});
    peer.send(result("root-result", "STARTED")).await;
    loop {
        if matches!(
            subscription.events.recv().await.unwrap().unwrap().kind,
            ProviderEventKind::Terminal { .. }
        ) {
            break;
        }
    }
    peer.send(json!({"type":"system","subtype":"background_tasks_changed","uuid":"empty-roster","tasks":[]})).await;
    peer.send(json!({"type":"system","subtype":"task_notification","task_id":"background","status":"completed","summary":"Build complete","uuid":"notice"})).await;
    peer.send(control("wake-barrier", "Bash", json!({"command":"pwd"})))
        .await;
    let barrier = peer.read().await;
    assert_eq!(barrier["response"]["response"]["behavior"], "deny");
    assert!(
        requests.try_recv().is_err(),
        "a completed opaque task does not prove that a native wake began"
    );
    assert!(
        runtime
            .extensions()
            .unwrap()
            .pending_background_work(None, true)
            .await
            .unwrap()
    );
    peer.send(json!({"type":"assistant","uuid":"wake-assistant","parent_tool_use_id":null,"message":{"id":"wake-api","role":"assistant","content":[{"type":"text","text":"WAKE_DONE"}]}})).await;
    let request = requests.recv().await.unwrap();
    assert_eq!(
        serde_json::to_value(request.notification).unwrap()["summary"],
        "Command \"Build\" finished"
    );
    assert_eq!(request.detail.as_deref(), Some("Build complete"));
    let mut wake = fixture_turn_input().unwrap();
    wake.run_id = RunId("wake-run".into());
    wake.run_ordinal = 2;
    wake.provider_turn_ordinal = 2;
    wake.message.created_by = OrchestrationV2Actor::Agent;
    wake.message.creation_source = Some(OrchestrationV2CreationSource::Provider);
    runtime.start_turn(wake).await.unwrap();
    peer.send(result("wake-result", "WAKE_DONE")).await;
    let mut saw_wake = false;
    loop {
        match subscription.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::Message(message) if message.text == "WAKE_DONE" => {
                assert_eq!(serde_json::to_value(message.run_id).unwrap(), "wake-run");
                saw_wake = true;
            }
            ProviderEventKind::Terminal { run_ordinal, .. } => {
                assert_eq!(run_ordinal, 2);
                break;
            }
            _ => {}
        }
    }
    assert!(saw_wake);
    assert!(
        !runtime
            .extensions()
            .unwrap()
            .pending_background_work(None, true)
            .await
            .unwrap()
    );
    runtime.close().await.unwrap();
    assert!(
        peer.read.next_line().await.unwrap().is_none(),
        "native wake must not send another user prompt"
    );
}
async fn request(subscription: &mut EventSubscription) -> RuntimeRequestId {
    loop {
        let event = subscription
            .events
            .recv()
            .await
            .expect("provider event")
            .unwrap();
        if let ProviderEventKind::RuntimeRequest { request, .. } = event.kind
            && request.status == t3_contracts::orchestration_v2::RuntimeRequestStatus::Pending
        {
            return serde_json::from_value(serde_json::to_value(request.id).unwrap()).unwrap();
        }
    }
}
fn control(id: &str, tool: &str, input: Value) -> Value {
    json!({"type":"control_request","request_id":id,"request":{"subtype":"can_use_tool","tool_name":tool,"input":input,"tool_use_id":format!("tool-{id}")}})
}
#[tokio::test]
async fn full_access_callback_requires_explicit_decision_and_preserves_tool_id() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control("permission", "Bash", json!({"command":"pwd"})))
        .await;
    let id = request(&mut subscription).await;
    assert!(
        runtime
            .respond(RuntimeResponse {
                request_id: id.clone(),
                decision: None,
                feedback: None,
                answers: None
            })
            .await
            .is_err()
    );
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::Decline),
            feedback: Some("No".into()),
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(reply["response"]["request_id"], "permission");
    assert_eq!(reply["response"]["response"]["behavior"], "deny");
    assert_eq!(
        reply["response"]["response"]["toolUseID"],
        "tool-permission"
    );
    runtime.close().await.unwrap();
}
#[tokio::test]
async fn independent_callbacks_are_correlated_and_only_explicit_allow_grants_access() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control(
        "first",
        "Write",
        json!({"file_path":"a","content":"one"}),
    ))
    .await;
    let first = request(&mut subscription).await;
    peer.send(control(
        "second",
        "Write",
        json!({"file_path":"b","content":"two"}),
    ))
    .await;
    let second = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: second,
            decision: Some(ProviderApprovalDecision::Accept),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(reply["response"]["request_id"], "second");
    assert_eq!(reply["response"]["response"]["behavior"], "allow");
    assert_eq!(
        reply["response"]["response"]["updatedInput"]["file_path"],
        "b"
    );
    runtime
        .respond(RuntimeResponse {
            request_id: first,
            decision: Some(ProviderApprovalDecision::Cancel),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(reply["response"]["request_id"], "first");
    assert_eq!(reply["response"]["response"]["behavior"], "deny");
    assert_eq!(reply["response"]["response"]["interrupt"], true);
    runtime.close().await.unwrap();
}
#[tokio::test]
async fn user_questions_and_plan_exit_remain_pending_until_seam_response() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control(
        "questions",
        "AskUserQuestion",
        json!({"questions":[{"question":"Choose","options":[]}]}),
    ))
    .await;
    let id = request(&mut subscription).await;
    assert!(
        runtime
            .respond(RuntimeResponse {
                request_id: id.clone(),
                decision: Some(ProviderApprovalDecision::Accept),
                feedback: None,
                answers: None
            })
            .await
            .is_err()
    );
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: None,
            feedback: None,
            answers: Some(
                [("Choose".into(), json!(["One", "Two"]))]
                    .into_iter()
                    .collect(),
            ),
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(
        reply["response"]["response"]["updatedInput"]["answers"],
        json!({"Choose":"One, Two"})
    );
    peer.send(control(
        "plan",
        "ExitPlanMode",
        json!({"plan":"# Proposed plan"}),
    ))
    .await;
    let id = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::Decline),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    assert_eq!(
        peer.read().await["response"]["response"]["behavior"],
        "deny"
    );
    peer.send(control(
        "cancel-question",
        "AskUserQuestion",
        json!({"questions":[{"question":"Choose","options":[]}]}),
    ))
    .await;
    let id = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::Cancel),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let response = peer.read().await;
    assert_eq!(response["response"]["response"]["behavior"], "deny");
    assert_eq!(response["response"]["response"]["interrupt"], true);
    runtime.close().await.unwrap();
}
#[tokio::test]
async fn cancelled_and_closed_callbacks_never_receive_allow() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control("cancelled", "Bash", json!({"command":"pwd"})))
        .await;
    let id = request(&mut subscription).await;
    peer.send(json!({"type":"control_cancel_request","request_id":"cancelled"}))
        .await;
    loop {
        let event = subscription.events.recv().await.unwrap().unwrap();
        if matches!(event.kind,ProviderEventKind::RuntimeRequest{request,..} if request.status==t3_contracts::orchestration_v2::RuntimeRequestStatus::Cancelled)
        {
            break;
        }
    }
    assert!(
        runtime
            .respond(RuntimeResponse {
                request_id: id,
                decision: Some(ProviderApprovalDecision::Accept),
                feedback: None,
                answers: None
            })
            .await
            .is_err()
    );
    peer.send(control("closed", "Bash", json!({"command":"pwd"})))
        .await;
    let _ = request(&mut subscription).await;
    runtime.close().await.unwrap();
    let mut bytes = String::new();
    use tokio::io::AsyncReadExt;
    peer.read
        .into_inner()
        .read_to_string(&mut bytes)
        .await
        .unwrap();
    assert!(!bytes.contains("\"allow\""));
}
#[tokio::test]
async fn unknown_frame_is_nonfatal_and_a_result_does_not_close_session_input() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(json!({"type":"future_frame","future":true}))
        .await;
    peer.send(json!({"type":"result","subtype":"success","uuid":"result","session_id":"session","result":"Done","is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}})).await;
    loop {
        let event = subscription.events.recv().await.unwrap().unwrap();
        if matches!(event.kind, ProviderEventKind::Terminal { .. }) {
            break;
        }
    }
    let handle = runtime.clone();
    let mut input = fixture_turn_input().unwrap();
    input.run_ordinal = 2;
    input.provider_turn_ordinal = 2;
    input.run_id = RunId("run:2".into());
    input.message.text = "Again".into();
    input.message.message_id = MessageId("00000000-0000-4000-8000-000000000003".into());
    let start = tokio::spawn(async move { handle.start_turn(input).await });
    let user = peer.read().await;
    assert_eq!(user["type"], "user");
    assert_eq!(user["message"]["content"], "Again");
    start.await.unwrap().unwrap();
    drop(peer);
    loop {
        if let ProviderEventKind::Terminal {
            disposition,
            outcome,
            ..
        } = subscription.events.recv().await.unwrap().unwrap().kind
        {
            assert_eq!(disposition, ThreadDisposition::Broken);
            assert!(matches!(outcome, TerminalOutcome::Failed { .. }));
            break;
        }
    }
    runtime.close().await.unwrap();
}

#[tokio::test]
async fn reused_query_applies_model_and_effort_before_the_next_prompt() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(
        json!({"type":"result", "subtype":"success", "uuid":"result", "session_id":"session",
        "result":"Done", "is_error":false, "num_turns":1, "duration_ms":1, "duration_api_ms":1,
        "total_cost_usd":0, "usage":{"input_tokens":1,"output_tokens":1}}),
    )
    .await;
    loop {
        if matches!(
            subscription.events.recv().await.unwrap().unwrap().kind,
            ProviderEventKind::Terminal { .. }
        ) {
            break;
        }
    }
    let mut input = fixture_turn_input().unwrap();
    input.run_ordinal = 2;
    input.provider_turn_ordinal = 2;
    input.model_selection.model = "claude-opus-4-6".into();
    let handle = runtime.clone();
    let mut start = tokio::spawn(async move {
        handle
            .start_turn_with_policy(
                input,
                RuntimePolicyOptions {
                    reasoning_effort: Some("high".into()),
                },
            )
            .await
    });
    let model = tokio::select! {
        result = &mut start => panic!("start returned before applying selection: {result:?}"),
        model = peer.read() => model,
    };
    assert_eq!(model["request"]["subtype"], "set_model");
    assert_eq!(model["request"]["model"], "claude-opus-4-6");
    peer.send(json!({"type":"control_response", "response":{"subtype":"success", "request_id":model["request_id"], "response":{}}})).await;
    let effort = peer.read().await;
    assert_eq!(effort["request"]["subtype"], "apply_flag_settings");
    assert_eq!(effort["request"]["settings"]["effortLevel"], "high");
    peer.send(json!({"type":"control_response", "response":{"subtype":"success", "request_id":effort["request_id"], "response":{}}})).await;
    assert_eq!(peer.read().await["type"], "user");
    start.await.unwrap().unwrap();
    runtime.close().await.unwrap();
}

#[tokio::test]
async fn background_work_outlives_root_completion_and_task_controls_target_its_session() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(json!({"type":"system", "subtype":"background_tasks_changed", "session_id":"session", "uuid":"roster",
        "tasks":[{"task_id":"background", "task_type":"local_bash", "description":"Build"}]})).await;
    peer.send(
        json!({"type":"result", "subtype":"success", "uuid":"result", "session_id":"session",
        "result":"Started", "is_error":false, "num_turns":1, "duration_ms":1, "duration_api_ms":1,
        "total_cost_usd":0, "usage":{"input_tokens":1,"output_tokens":1}}),
    )
    .await;
    loop {
        if matches!(
            subscription.events.recv().await.unwrap().unwrap().kind,
            ProviderEventKind::Terminal { .. }
        ) {
            break;
        }
    }
    let thread = fixture_turn_input().unwrap().provider_thread;
    assert!(
        runtime
            .extensions()
            .unwrap()
            .pending_background_work(Some(thread.clone()), true)
            .await
            .unwrap()
    );
    let mut wrong = thread.clone();
    wrong.native_thread_ref.as_mut().unwrap().native_id = Some(
        t3_contracts::base_schemas::TrimmedNonEmptyString("wrong-session".into()),
    );
    assert!(
        runtime
            .extensions()
            .unwrap()
            .stop_task(wrong, "background".into())
            .await
            .is_err()
    );
    let handle = runtime.clone();
    let stop = tokio::spawn(async move {
        handle
            .extensions()
            .unwrap()
            .stop_task(thread, "background".into())
            .await
    });
    let control = peer.read().await;
    assert_eq!(control["request"]["subtype"], "stop_task");
    assert_eq!(control["request"]["task_id"], "background");
    peer.send(json!({"type":"control_response", "response":{"subtype":"success", "request_id":control["request_id"], "response":{}}})).await;
    stop.await.unwrap().unwrap();
    runtime.close().await.unwrap();
}

#[tokio::test]
async fn live_selection_updates_flags_and_defers_process_bound_changes() {
    let host = ClaudeHostOptions {
        compile_model: Some(Arc::new(|selection| {
            let options = serde_json::to_value(&selection.options).unwrap();
            let mut model = ClaudeCompiledModel {
                model: selection.model.clone(),
                effort: Some("high".into()),
                settings: json!({}),
            };
            for option in options.as_array().into_iter().flatten() {
                match option["id"].as_str() {
                    Some("effort") => {
                        model.effort = option["value"]
                            .as_str()
                            .filter(|value| *value != "default")
                            .map(str::to_owned)
                    }
                    Some("thinking") => {
                        model.settings["alwaysThinkingEnabled"] = option["value"].clone()
                    }
                    Some("fastMode" | "ultracode") => {
                        model.settings[option["id"].as_str().unwrap()] = option["value"].clone()
                    }
                    _ => {}
                }
            }
            Ok(model)
        })),
        ..Default::default()
    };
    let (runtime, mut events, mut peer) = running_with_host(host).await;
    let thread = fixture_turn_input().unwrap().provider_thread;
    let selection = |options| {
        serde_json::from_value(
            json!({"instanceId":"claudeAgent","model":"claude-sonnet-4-6","options":options}),
        )
        .unwrap()
    };
    let extensions = runtime.extensions().unwrap();
    for (options, expected) in [
        (
            json!([{"id":"effort","value":"medium"}]),
            json!({"effortLevel":"medium","fastMode":null,"ultracode":null}),
        ),
        (
            json!([{"id":"effort","value":"medium"},{"id":"fastMode","value":true},{"id":"ultracode","value":false}]),
            json!({"effortLevel":"medium","fastMode":true,"ultracode":false}),
        ),
    ] {
        let handle = runtime.clone();
        let thread = thread.clone();
        let next = selection(options);
        let mut apply = tokio::spawn(async move {
            handle
                .extensions()
                .unwrap()
                .apply_selection(thread, next)
                .await
        });
        let flags = tokio::select! {result = &mut apply => panic!("live flags returned before native acknowledgment: {result:?}"), frame = peer.read() => frame};
        assert_eq!(flags["request"]["subtype"], "apply_flag_settings");
        assert_eq!(flags["request"]["settings"], expected);
        peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":flags["request_id"],"response":{}}})).await;
        apply.await.unwrap().unwrap();
    }
    for next in [
        selection(
            json!([{"id":"effort","value":"medium"},{"id":"fastMode","value":true},{"id":"ultracode","value":false}]),
        ),
        selection(json!([{"id":"effort","value":"default"}])),
        selection(json!([{"id":"effort","value":"medium"},{"id":"thinking","value":false}])),
        serde_json::from_value(json!({"instanceId":"claudeAgent","model":"claude-opus-4-6"}))
            .unwrap(),
    ] {
        tokio::select! {result = extensions.apply_selection(thread.clone(), next) => result.unwrap(), frame = peer.read() => panic!("process-bound/no-op change sent a live frame: {frame}")};
    }
    peer.send(json!({"type":"result","subtype":"success","uuid":"live-flags-end","result":"Done","is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}})).await;
    let mut turns = std::collections::BTreeSet::new();
    loop {
        match events.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::Turn { provider_turn, .. } => {
                turns.insert(serde_json::to_string(&provider_turn.id).unwrap());
            }
            ProviderEventKind::Terminal { run_ordinal, .. } => {
                assert_eq!(run_ordinal, 1);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(turns.len(), 1);
    runtime.close().await.unwrap();
    assert!(
        peer.read.next_line().await.unwrap().is_none(),
        "live updates must not send a user prompt or interrupt"
    );
}
