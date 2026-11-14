#![cfg(feature = "claude-adapter")]

use crate::{
    adapters::claude::*,
    api::{Clock, IdGenerator},
    provider::*,
};
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use t3_claude_cli::{Session, SessionOptions, launch::LaunchOptions};
use t3_contracts::{
    base_schemas::*,
    model_selection::ModelSelection,
    orchestration_v2::*,
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub const FIXTURE_DATE: &str = "2026-01-02T03:04:05.000Z";

struct NativeReplayState {
    entries: Vec<Value>,
    cursor: usize,
    prompt_ids: std::collections::BTreeMap<String, String>,
    failure: Option<String>,
}
pub struct ClaudeReplayConnector {
    state: Arc<Mutex<NativeReplayState>>,
    gate: crate::testing::replay::ReplayGate,
    tasks: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    failure: tokio::sync::watch::Sender<Option<String>>,
}
impl ClaudeReplayConnector {
    pub fn new(
        transcript: &crate::testing::replay::ReplayTranscript,
        gate: crate::testing::replay::ReplayGate,
    ) -> ProviderResult<Self> {
        if transcript.provider.0 != "claudeAgent"
            || transcript.protocol.0 != "claude-agent-sdk.query"
        {
            return Err(operation(
                ProviderOperation::Open,
                "Expected a Claude SDK query recording",
            ));
        }
        let entries = serde_json::to_value(&transcript.entries)
            .map_err(|_| operation(ProviderOperation::Open, "Invalid Claude replay entries"))?
            .as_array()
            .cloned()
            .ok_or(ProviderError::SessionStopped)?;
        Ok(Self {
            state: Arc::new(Mutex::new(NativeReplayState {
                entries,
                cursor: 0,
                prompt_ids: Default::default(),
                failure: None,
            })),
            gate,
            tasks: Default::default(),
            failure: tokio::sync::watch::channel(None).0,
        })
    }
    pub fn assert_complete(&self) -> ProviderResult<()> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(failure) = &state.failure {
            return Err(operation(ProviderOperation::Stream, failure));
        }
        if state.cursor != state.entries.len() {
            return Err(operation(
                ProviderOperation::Stream,
                format!(
                    "Claude replay stopped at entry {} of {}",
                    state.cursor,
                    state.entries.len()
                ),
            ));
        }
        Ok(())
    }
    pub async fn join(&self) {
        for task in std::mem::take(&mut *self.tasks.lock().await) {
            let _ = task.await;
        }
    }
    pub async fn failed(&self) -> ProviderError {
        let mut failure = self.failure.subscribe();
        loop {
            if let Some(detail) = failure.borrow().clone() {
                return operation(ProviderOperation::Stream, detail);
            }
            if failure.changed().await.is_err() {
                return ProviderError::SessionStopped;
            }
        }
    }
}
fn replay_lock(state: &Mutex<NativeReplayState>) -> std::sync::MutexGuard<'_, NativeReplayState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}
fn remap_prompt_ids(value: &mut Value, ids: &std::collections::BTreeMap<String, String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                remap_prompt_ids(value, ids);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if matches!(key.as_str(), "user_message_uuid" | "uuid")
                    && let Some(id) = value.as_str().and_then(|id| ids.get(id))
                {
                    *value = json!(id);
                } else if key == "user_message_uuids" {
                    if let Some(values) = value.as_array_mut() {
                        for value in values {
                            if let Some(id) = value.as_str().and_then(|id| ids.get(id)) {
                                *value = json!(id);
                            }
                        }
                    }
                } else {
                    remap_prompt_ids(value, ids);
                }
            }
        }
        _ => {}
    }
}
async fn replay_native_pipe(
    state: Arc<Mutex<NativeReplayState>>,
    gate: crate::testing::replay::ReplayGate,
    peer: tokio::io::DuplexStream,
    cancellation: CancellationToken,
) -> ProviderResult<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (read, mut write) = tokio::io::split(peer);
    let mut lines = BufReader::new(read).lines();
    let read_frame = |line: Option<String>| -> ProviderResult<Value> {
        serde_json::from_str(&line.ok_or(ProviderError::SessionStopped)?)
            .map_err(|_| operation(ProviderOperation::Stream, "Invalid native replay frame"))
    };
    let init = tokio::select! {_=cancellation.cancelled()=>return Ok(()),line=lines.next_line()=>read_frame(line.map_err(|_|ProviderError::SessionStopped)?)?};
    if init["request"]["subtype"] != "initialize" {
        return Err(operation(
            ProviderOperation::Open,
            "Expected native initialize",
        ));
    }
    let response = json!({"type":"control_response","response":{"subtype":"success","request_id":init["request_id"],"response":{}}});
    let mut bytes = serde_json::to_vec(&response).map_err(|_| ProviderError::SessionStopped)?;
    bytes.push(b'\n');
    write
        .write_all(&bytes)
        .await
        .map_err(|_| ProviderError::SessionStopped)?;
    loop {
        let entry = {
            let state = replay_lock(&state);
            state.entries.get(state.cursor).cloned()
        };
        let Some(entry) = entry else {
            cancellation.cancelled().await;
            return Ok(());
        };
        match entry["type"].as_str() {
            Some("runtime_exit") => {
                replay_lock(&state).cursor += 1;
                return Ok(());
            }
            Some("emit_inbound") => {
                gate.before_emit(entry["label"].as_str(), &cancellation)
                    .await;
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                let mut frame = entry["frame"].clone();
                {
                    let mut state = replay_lock(&state);
                    remap_prompt_ids(&mut frame, &state.prompt_ids);
                    state.cursor += 1;
                }
                if frame["type"] == "permission.request" {
                    let id = format!("permission:{}", replay_lock(&state).cursor);
                    let options = &frame["options"];
                    frame = json!({"type":"control_request","request_id":id,"request":{"subtype":"can_use_tool","tool_name":frame["toolName"],"input":frame["input"],"tool_use_id":options["toolUseID"],"permission_suggestions":options["suggestions"],"title":options["title"],"description":options["description"],"agent_id":options["agentID"]}});
                }
                let mut bytes =
                    serde_json::to_vec(&frame).map_err(|_| ProviderError::SessionStopped)?;
                bytes.push(b'\n');
                write
                    .write_all(&bytes)
                    .await
                    .map_err(|_| ProviderError::SessionStopped)?;
            }
            Some("expect_outbound") => {
                let actual = tokio::select! {_=cancellation.cancelled()=>return Ok(()),line=lines.next_line()=>read_frame(line.map_err(|_|ProviderError::SessionStopped)?)?};
                let expected = &entry["frame"];
                match expected["type"].as_str() {
                    Some("prompt.offer") => {
                        if actual["type"] != "user"
                            || actual["message"]["content"]
                                != expected["message"]["message"]["content"]
                        {
                            return Err(operation(
                                ProviderOperation::Start,
                                "Claude replay prompt mismatch",
                            ));
                        }
                        if let (Some(recorded), Some(native)) = (
                            expected["message"]["uuid"].as_str(),
                            actual["uuid"].as_str(),
                        ) {
                            replay_lock(&state)
                                .prompt_ids
                                .insert(recorded.into(), native.into());
                        }
                    }
                    Some("permission.response") => {
                        if actual["type"] != "control_response"
                            || !crate::testing::replay::matches_expected(
                                &expected["result"],
                                &actual["response"]["response"],
                                "",
                            )
                        {
                            return Err(operation(
                                ProviderOperation::Respond,
                                "Claude replay permission response mismatch",
                            ));
                        }
                    }
                    Some("query.interrupt") => {
                        if actual["request"]["subtype"] != "interrupt" {
                            return Err(operation(
                                ProviderOperation::Interrupt,
                                "Claude replay interrupt mismatch",
                            ));
                        }
                        let response = json!({"type":"control_response","response":{"subtype":"success","request_id":actual["request_id"],"response":{}}});
                        let mut bytes = serde_json::to_vec(&response)
                            .map_err(|_| ProviderError::SessionStopped)?;
                        bytes.push(b'\n');
                        write
                            .write_all(&bytes)
                            .await
                            .map_err(|_| ProviderError::SessionStopped)?;
                    }
                    _ => {
                        return Err(ProviderError::Unsupported(
                            "Claude replay operation requires native transcript fixture state",
                        ));
                    }
                }
                replay_lock(&state).cursor += 1;
            }
            _ => {
                return Err(operation(
                    ProviderOperation::Stream,
                    "Unknown Claude replay entry",
                ));
            }
        }
    }
}
impl ClaudeConnector for ClaudeReplayConnector {
    fn connect<'a>(
        &'a self,
        options: &'a LaunchOptions,
        cancellation: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ProviderResult<ClaudeConnection>> + Send + 'a>> {
        Box::pin(async move {
            {
                let mut state = replay_lock(&self.state);
                let entry = state
                    .entries
                    .get(state.cursor)
                    .ok_or(ProviderError::SessionStopped)?;
                let expected = &entry["frame"];
                if entry["type"] != "expect_outbound" || expected["type"] != "query.open" {
                    return Err(operation(
                        ProviderOperation::Open,
                        "Expected recorded query.open",
                    ));
                }
                if expected["options"]["model"].as_str() != options.model.as_deref()
                    || expected["options"]["permissionMode"].as_str()
                        != options.permission_mode.as_ref().map(|p| p.as_str())
                {
                    return Err(operation(
                        ProviderOperation::Open,
                        "Claude replay model/permission mode mismatch",
                    ));
                }
                state.cursor += 1;
            }
            let (client, peer) = tokio::io::duplex(64 * 1024);
            let (read, write) = tokio::io::split(client);
            let state = self.state.clone();
            let gate = self.gate.clone();
            let failure = self.failure.clone();
            let task = tokio::spawn(async move {
                if let Err(error) =
                    replay_native_pipe(state.clone(), gate, peer, cancellation).await
                {
                    let detail = error.to_string();
                    replay_lock(&state).failure = Some(detail.clone());
                    failure.send_replace(Some(detail));
                }
            });
            self.tasks.lock().await.push(task);
            Ok(ClaudeConnection {
                session: Session::connect(read, write, SessionOptions::default()),
                process: None,
            })
        })
    }
}

pub fn fixture_recording_root() -> std::path::PathBuf {
    let source = std::path::Path::new(file!());
    let manifest = source
        .is_absolute()
        .then(|| source.ancestors().nth(5))
        .flatten()
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    manifest.join("../../../apps/server/src/orchestration-v2/testkit/fixtures")
}

pub fn fixture_turn_input() -> ProviderResult<TurnInput> {
    let mut app: Value = serde_json::from_str(
        include_str!("../../../../../../fixtures/samples/OrchestrationV2AppThread.jsonl")
            .lines()
            .next()
            .ok_or(ProviderError::SessionStopped)?,
    )
    .map_err(|_| ProviderError::SessionStopped)?;
    app["id"] = json!("thread");
    let thread:OrchestrationV2ProviderThread=serde_json::from_value(json!({"id":"provider-thread","driver":"claudeAgent","providerInstanceId":"claudeAgent",
        "providerSessionId":"session","appThreadId":"thread","ownerNodeId":null,"nativeThreadRef":{"driver":"claudeAgent","nativeId":"00000000-0000-4000-8000-000000000001","strength":"strong"},
        "nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[],"forkedFrom":null,"createdAt":FIXTURE_DATE,"updatedAt":FIXTURE_DATE}))
        .map_err(|_|ProviderError::SessionStopped)?;
    Ok(TurnInput {
        app_thread: serde_json::from_value(app).map_err(|_| ProviderError::SessionStopped)?,
        thread_id: ThreadId("thread".into()),
        run_id: RunId("run".into()),
        run_ordinal: 1,
        native_thread_has_turns: Some(false),
        provider_turn_ordinal: 1,
        restart_continuation_of_run_id: None,
        attempt_id: RunAttemptId("attempt".into()),
        root_node_id: NodeId("root".into()),
        provider_thread: thread,
        message: TurnMessage {
            message_id: MessageId("00000000-0000-4000-8000-000000000002".into()),
            text: "Hello".into(),
            attachments: vec![],
            created_by: OrchestrationV2Actor::User,
            creation_source: Some(OrchestrationV2CreationSource::Web),
            scheduled_task_id: None,
            sender_thread_id: None,
        },
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId("claudeAgent".into()),
            model: "claude-sonnet-4-6".into(),
            options: None,
        },
        runtime_policy: RuntimePolicy {
            runtime_mode: RuntimeMode::FullAccess,
            interaction_mode: ProviderInteractionMode::Default,
            cwd: None,
            approval_policy: None,
            sandbox_policy: None,
            max_runtime_mode: None,
        },
        app_context: vec![],
        inline_context_length: None,
    })
}

pub struct PipeConnector(Mutex<Option<Session>>);
impl PipeConnector {
    pub fn new(session: Session) -> Self {
        Self(Mutex::new(Some(session)))
    }
}
impl ClaudeConnector for PipeConnector {
    fn connect<'a>(
        &'a self,
        _: &'a LaunchOptions,
        _: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ProviderResult<ClaudeConnection>> + Send + 'a>> {
        let session = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        Box::pin(async move {
            Ok(ClaudeConnection {
                session: session.ok_or(ProviderError::SessionStopped)?,
                process: None,
            })
        })
    }
}

pub fn fixture_projection(clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> ClaudeProjection {
    ClaudeProjection::new(
        ProviderInstanceId("claudeAgent".into()),
        ProviderSessionId("session".into()),
        clock,
        ids,
    )
}

pub async fn replay_messages(
    recording: &str,
    projection: &mut ClaudeProjection,
    mut input: TurnInput,
) -> ProviderResult<Vec<ProviderEvent>> {
    let records = recording
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line).map_err(|_| ProviderError::Protocol {
                reason: None,
                detail: "Invalid Claude recording".into(),
                payload: None,
            })
        })
        .collect::<ProviderResult<Vec<_>>>()?;
    let prompts = records
        .iter()
        .filter(|record| {
            record["type"] == "expect_outbound" && record["frame"]["type"] == "prompt.offer"
        })
        .map(|record| record["frame"]["message"].clone())
        .collect::<Vec<_>>();
    let apply_prompt = |input: &mut TurnInput, index: usize| {
        if let Some(prompt) = prompts.get(index) {
            if let Some(uuid) = prompt["uuid"].as_str() {
                input.message.message_id = MessageId(uuid.into());
            }
            if let Some(text) = prompt["message"]["content"].as_str() {
                input.message.text = text.into();
            }
        }
    };
    let mut prompt_index = 0;
    apply_prompt(&mut input, prompt_index);
    let (client, mut server) = tokio::io::duplex(16 * 1024);
    let (read, write) = tokio::io::split(client);
    let mut session = Session::connect(read, write, SessionOptions::default());
    let mut events = projection.begin(input.clone())?;
    for record in &records {
        if record["type"] == "expect_outbound" && record["frame"]["type"] == "query.interrupt" {
            projection.interrupt();
        }
        if record["type"] == "runtime_exit" {
            events.extend(projection.query_ended()?);
        }
        if record["type"] != "emit_inbound" {
            continue;
        }
        let mut bytes =
            serde_json::to_vec(&record["frame"]).map_err(|_| ProviderError::SessionStopped)?;
        bytes.push(b'\n');
        let receive = async {
            loop {
                match session
                    .next_event()
                    .await
                    .ok_or(ProviderError::SessionStopped)?
                    .map_err(|_| ProviderError::SessionStopped)?
                {
                    t3_claude_cli::Event::Message(message) => return Ok(message),
                    t3_claude_cli::Event::Request(_) => {
                        return Err(ProviderError::Unsupported(
                            "Use scripted replay controls for permission callbacks",
                        ));
                    }
                    _ => {}
                }
            }
        };
        let (written, message) = tokio::join!(server.write_all(&bytes), receive);
        written.map_err(|_| ProviderError::SessionStopped)?;
        let message = message?;
        if projection.active_turn_id().is_none()
            && matches!(
                message.raw()["type"].as_str(),
                Some("assistant" | "stream_event")
            )
            && message.raw()["parent_tool_use_id"].is_null()
        {
            input.run_ordinal += 1;
            input.provider_turn_ordinal += 1;
            input.run_id = RunId(format!("run:{}", input.run_ordinal));
            prompt_index += 1;
            apply_prompt(&mut input, prompt_index);
            events.extend(projection.begin(input.clone())?);
        }
        events.extend(projection.handle_message(&message).map_err(|error| {
            ProviderError::Protocol {
                reason: None,
                detail: format!(
                    "Replay frame {} / {}: {error:?}",
                    message.raw()["type"],
                    message.raw()["subtype"]
                ),
                payload: None,
            }
        })?);
    }
    server
        .shutdown()
        .await
        .map_err(|_| ProviderError::SessionStopped)?;
    events.extend(projection.query_ended()?);
    session
        .shutdown()
        .await
        .map_err(|_| ProviderError::SessionStopped)?;
    Ok(events)
}

pub fn build_claude_fixture_input(
    fixture: &crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorReplayFixture,
) -> crate::error::Result<
    crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput,
> {
    match fixture.build_input.as_str() {
        "claudeBackgroundMonitorWakeInput" => crate::ports::claude::testkit::fixtures::claude_background_monitor_wake::input::claude_background_monitor_wake_input(),
        "claudeBackgroundSubagentAfterRootInput" => crate::ports::claude::testkit::fixtures::claude_background_subagent_after_root::input::claude_background_subagent_after_root_input(),
        "claudeBackgroundSubagentLifecycleInput" => crate::ports::claude::testkit::fixtures::claude_background_subagent_lifecycle::input::claude_background_subagent_lifecycle_input(),
        "claudeBackgroundTaskAfterRootInput" => crate::ports::claude::testkit::fixtures::claude_background_task_after_root::input::claude_background_task_after_root_input(),
        "claudeBackgroundTaskInterruptInput" => crate::ports::claude::testkit::fixtures::claude_background_task_interrupt::input::claude_background_task_interrupt_input(),
        "claudeBackgroundTaskWakeInput" => crate::ports::claude::testkit::fixtures::claude_background_task_wake::input::claude_background_task_wake_input(),
        "claudeBackgroundWakeBeforeQueuedPromptInput" => crate::ports::claude::testkit::fixtures::claude_background_wake_before_queued_prompt::input::claude_background_wake_before_queued_prompt_input(),
        "claudeBackgroundWakeBeforeQueuedPromptNoEchoInput" => crate::ports::claude::testkit::fixtures::claude_background_wake_before_queued_prompt_no_echo::input::claude_background_wake_before_queued_prompt_no_echo_input(),
        "claudeCompactAfterPeerTurnInput" => crate::ports::claude::testkit::fixtures::claude_compact_after_peer_turn::input::claude_compact_after_peer_turn_input(),
        "claudeCompactAfterPeerTurnNoEchoInput" => crate::ports::claude::testkit::fixtures::claude_compact_after_peer_turn::input::claude_compact_after_peer_turn_no_echo_input(),
        "claudeCompactAfterResumeWakeInput" => crate::ports::claude::testkit::fixtures::claude_compact_after_resume_wake::input::claude_compact_after_resume_wake_input(),
        "claudeIdleResumeInput" => crate::ports::claude::testkit::fixtures::claude_idle_resume::input::claude_idle_resume_input(),
        "claudeLocalBashTaskInput" => crate::ports::claude::testkit::fixtures::claude_local_bash_task::input::claude_local_bash_task_input(),
        "claudeMcpToolPresentationInput" => crate::ports::claude::testkit::fixtures::claude_mcp_tool_presentation::input::claude_mcp_tool_presentation_input(),
        "claudeNestedBackgroundSubagentWakeInput" => crate::ports::claude::testkit::fixtures::claude_nested_background_subagent_wake::input::claude_nested_background_subagent_wake_input(),
        "claudeNestedSubagentModelInput" => crate::ports::claude::testkit::fixtures::claude_nested_subagent_model::input::claude_nested_subagent_model_input(),
        "claudeResultIsErrorInput" => crate::ports::claude::testkit::fixtures::claude_result_is_error::input::claude_result_is_error_input(),
        _ => fixture.build_owned_input(),
    }
}

pub fn assert_claude_fixture_output(
    name: &str,
    result: &crate::ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    match name {
        "assertClaudeBackgroundMonitorWakeOutput" => crate::ports::claude::testkit::fixtures::claude_background_monitor_wake::output::assert_claude_background_monitor_wake_output(result, transcript),
        "assertClaudeBackgroundSubagentAfterRootOutput" => crate::ports::claude::testkit::fixtures::claude_background_subagent_after_root::output::assert_claude_background_subagent_after_root_output(result, transcript),
        "assertClaudeBackgroundSubagentLifecycleOutput" => crate::ports::claude::testkit::fixtures::claude_background_subagent_lifecycle::output::assert_claude_background_subagent_lifecycle_output(result, transcript),
        "assertClaudeBackgroundTaskAfterRootOutput" => crate::ports::claude::testkit::fixtures::claude_background_task_after_root::output::assert_claude_background_task_after_root_output(result, transcript),
        "assertClaudeBackgroundTaskInterruptOutput" => crate::ports::claude::testkit::fixtures::claude_background_task_interrupt::output::assert_claude_background_task_interrupt_output(result, transcript),
        "assertClaudeBackgroundTaskWakeOutput" => crate::ports::claude::testkit::fixtures::claude_background_task_wake::output::assert_claude_background_task_wake_output(result, transcript),
        "assertClaudeBackgroundWakeBeforeQueuedPromptOutput" => crate::ports::claude::testkit::fixtures::claude_background_wake_before_queued_prompt::output::assert_claude_background_wake_before_queued_prompt_output(result, transcript),
        "assertClaudeBackgroundWakeBeforeQueuedPromptNoEchoOutput" => crate::ports::claude::testkit::fixtures::claude_background_wake_before_queued_prompt_no_echo::output::assert_claude_background_wake_before_queued_prompt_no_echo_output(result, transcript),
        "assertClaudeCompactAfterPeerTurnOutput" => crate::ports::claude::testkit::fixtures::claude_compact_after_peer_turn::output::assert_claude_compact_after_peer_turn_output(result, transcript),
        "assertClaudeCompactAfterPeerTurnNoEchoOutput" => crate::ports::claude::testkit::fixtures::claude_compact_after_peer_turn_no_echo::output::assert_claude_compact_after_peer_turn_no_echo_output(result, transcript),
        "assertClaudeCompactAfterResumeWakeOutput" => crate::ports::claude::testkit::fixtures::claude_compact_after_resume_wake::output::assert_claude_compact_after_resume_wake_output(result, transcript),
        "assertClaudeIdleResumeOutput" => crate::ports::claude::testkit::fixtures::claude_idle_resume::output::assert_claude_idle_resume_output(result, transcript),
        "assertClaudeLocalBashTaskOutput" => crate::ports::claude::testkit::fixtures::claude_local_bash_task::output::assert_claude_local_bash_task_output(result, transcript),
        "assertClaudeMcpToolPresentationOutput" => crate::ports::claude::testkit::fixtures::claude_mcp_tool_presentation::output::assert_claude_mcp_tool_presentation_output(result, transcript),
        "assertClaudeNestedBackgroundSubagentWakeOutput" => crate::ports::claude::testkit::fixtures::claude_nested_background_subagent_wake::output::assert_claude_nested_background_subagent_wake_output(result, transcript),
        "assertClaudeNestedSubagentModelOutput" => crate::ports::claude::testkit::fixtures::claude_nested_subagent_model::output::assert_claude_nested_subagent_model_output(result, transcript),
        "assertClaudeResultIsErrorOutput" => crate::ports::claude::testkit::fixtures::claude_result_is_error::output::assert_claude_result_is_error_output(result, transcript),
        "assertClaudeMessageSteeringOutput" => crate::ports::claude::testkit::fixtures::message_steering::claude_output::assert_claude_message_steering_output(result, transcript),
        "assertMultiTurnClaudeOutput" => crate::ports::claude::testkit::fixtures::multi_turn::claude_output::assert_multi_turn_claude_output(result, transcript),
        "assertSimpleClaudeOutput" => crate::ports::claude::testkit::fixtures::simple::claude_output::assert_simple_claude_output(result, transcript),
        "assertClaudeSubagentOutput" => crate::ports::claude::testkit::fixtures::subagent::claude_output::assert_claude_subagent_output(result, transcript),
        "assertClaudeThreadRollbackOutput" => crate::ports::claude::testkit::fixtures::thread_rollback::claude_output::assert_claude_thread_rollback_output(result, transcript),
        "assertToolCallDeniedWriteClaudeOutput" => crate::ports::claude::testkit::fixtures::tool_call_denied_write::claude_output::assert_tool_call_denied_write_claude_output(result, transcript),
        "assertToolCallReadOnlyClaudeOutput" => crate::ports::claude::testkit::fixtures::tool_call_read_only::claude_output::assert_tool_call_read_only_claude_output(result, transcript),
        "assertToolCallRestrictedGranularClaudeOutput" => crate::ports::claude::testkit::fixtures::tool_call_restricted_granular::claude_output::assert_tool_call_restricted_granular_claude_output(result, transcript),
        "assertToolCallWorkspaceNeverClaudeOutput" => crate::ports::claude::testkit::fixtures::tool_call_workspace_never::claude_output::assert_tool_call_workspace_never_claude_output(result, transcript),
        "assertTurnInterruptClaudeOutput" => crate::ports::claude::testkit::fixtures::turn_interrupt::claude_output::assert_turn_interrupt_claude_output(result, transcript),
        "assertTurnInterruptMidToolClaudeOutput" => crate::ports::claude::testkit::fixtures::turn_interrupt_mid_tool::claude_output::assert_turn_interrupt_mid_tool_claude_output(result, transcript),
        "assertTurnInterruptRestartClaudeOutput" => crate::ports::claude::testkit::fixtures::turn_interrupt_restart::claude_output::assert_turn_interrupt_restart_claude_output(result, transcript),
        "assertClaudeWebSearchOutput" => crate::ports::claude::testkit::fixtures::web_search::claude_output::assert_claude_web_search_output(result, transcript),
        _ => Err(crate::error::Error::Service {
            operation: "Claude replay output",
            detail: format!("Assertion {name} is not owned by the Claude package"),
        }),
    }
}
