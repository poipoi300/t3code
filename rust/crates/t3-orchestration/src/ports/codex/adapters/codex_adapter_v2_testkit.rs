#![cfg(feature = "codex-adapter")]

use super::codex_adapter_v2::{self as mapping, Mutation, Projection};

use crate::{
    adapters::codex::{
        CodexAdapterOptions, CodexAdapterV2, CodexSessionRuntime, native_error, protocol,
    },
    provider::*,
    testing::{ManualClock, SequenceIds},
};

use serde_json::{Value, json};

use std::{collections::BTreeMap, sync::Arc};

use t3_codex_app_server::{self as native, wire::Params};

use t3_contracts::{
    base_schemas::*,
    model_selection::ModelSelection,
    orchestration_v2::*,
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf},
    sync::{mpsc, oneshot},
};

use tokio_util::sync::CancellationToken;

pub const NOW: &str = "2026-08-29T00:00:00.000Z";

pub fn open_input() -> OpenSession {
    OpenSession {
        thread_id: ThreadId("thread-codex".into()),
        provider_session_id: ProviderSessionId("session-codex".into()),
        model_selection: selection(),
        runtime_policy: policy(),
        prompt_suggestions: None,
        resume_from_session: None,
        initial_native_thread_id: None,
        initial_provider_item_identity_version: None,
    }
}

pub fn selection() -> ModelSelection {
    ModelSelection {
        instance_id: ProviderInstanceId("codex".into()),
        model: "gpt-6-luna".into(),
        options: None,
    }
}

pub fn policy() -> RuntimePolicy {
    RuntimePolicy {
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: Some("<workspace>".into()),
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    }
}

pub fn options() -> CodexAdapterOptions {
    CodexAdapterOptions::new(
        ProviderInstanceId("codex".into()),
        Arc::new(ManualClock::new(NOW)),
        Arc::new(SequenceIds::new("codex")),
    )
}

pub fn thread(native: &str) -> ProviderResult<OrchestrationV2ProviderThread> {
    mapping::decode(json!({
    "id":mapping::derived("provider-thread","native-thread",native),"driver":"codex","providerInstanceId":"codex","providerSessionId":"session-codex","appThreadId":"thread-codex","ownerNodeId":null,"nativeThreadRef":mapping::native_ref(native),"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[],"forkedFrom":null,"createdAt":NOW,"updatedAt":NOW}
    ))
}

pub fn turn_input(native: &str, ordinal: u64, prompt: &str) -> ProviderResult<TurnInput> {
    let selection = selection();

    let mut app: Value = serde_json::from_str(
        include_str!("../../../../../../fixtures/samples/OrchestrationV2AppThread.jsonl")
            .lines()
            .next()
            .ok_or_else(|| protocol("Missing app-thread fixture"))?,
    )
    .map_err(|e| protocol(e.to_string()))?;

    app["id"] = json!("thread-codex");

    app["title"] = json!("Codex test");

    app["modelSelection"] = json!(selection);

    app["providerInstanceId"] = json!("codex");

    app["lineage"] = json!({
    "parentThreadId":null,"relationshipToParent":null,"rootThreadId":"thread-codex"}
    );

    Ok(TurnInput {
        app_thread: mapping::decode(app)?,
        thread_id: ThreadId("thread-codex".into()),
        run_id: RunId(format!("run-{ordinal}")),
        run_ordinal: ordinal,
        native_thread_has_turns: Some(ordinal > 1),
        provider_turn_ordinal: ordinal,
        restart_continuation_of_run_id: None,
        attempt_id: RunAttemptId(format!("attempt-{ordinal}")),
        root_node_id: NodeId(format!("root-{ordinal}")),
        provider_thread: thread(native)?,
        message: TurnMessage {
            message_id: MessageId(format!("message-{ordinal}")),
            text: prompt.into(),
            attachments: vec![],
            created_by: OrchestrationV2Actor::User,
            creation_source: Some(OrchestrationV2CreationSource::Web),
            scheduled_task_id: None,
            sender_thread_id: None,
        },
        model_selection: selection,
        runtime_policy: policy(),
        app_context: vec![],
    })
}

pub fn native_turn(id: &str, status: &str) -> Value {
    json!({
    "id":id,"items":[],"itemsView":"notLoaded","status":status,"error":null,"startedAt":1782622440,"completedAt":if status=="inProgress"{
    Value::Null}
    else{
    json!(1782622450)}
    ,"durationMs":null}
    )
}

pub fn initialize_response() -> Value {
    json!({
    "userAgent":"T3 Code/0.156.1","codexHome":"<codex-home>","platformFamily":"unix","platformOs":"linux"}
    )
}

pub struct WirePeer {
    pub reader: BufReader<ReadHalf<DuplexStream>>,
    pub writer: WriteHalf<DuplexStream>,
}

impl WirePeer {
    pub async fn read(&mut self) -> ProviderResult<Value> {
        let mut line = String::new();

        if self
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| protocol(e.to_string()))?
            == 0
        {
            return Err(protocol("Unexpected replay EOF"));
        }

        serde_json::from_str(&line).map_err(|e| protocol(e.to_string()))
    }

    pub async fn write(&mut self, value: &Value) -> ProviderResult<()> {
        let mut bytes = serde_json::to_vec(value).map_err(|e| protocol(e.to_string()))?;

        bytes.push(b'\n');

        self.writer
            .write_all(&bytes)
            .await
            .map_err(|e| protocol(e.to_string()))
    }

    pub async fn respond(&mut self, request: &Value, result: Value) -> ProviderResult<()> {
        self.write(&json!({
        "id":request["id"],"result":result}
        ))
        .await
    }
}

pub fn pipes() -> (native::Client, native::Events, WirePeer) {
    let (agent, peer) = tokio::io::duplex(256 * 1024);

    let (reader, writer) = tokio::io::split(agent);

    let (peer_reader, peer_writer) = tokio::io::split(peer);

    let (client, events) =
        native::Client::connect(reader, writer, native::ClientOptions::default());

    (
        client,
        events,
        WirePeer {
            reader: BufReader::new(peer_reader),
            writer: peer_writer,
        },
    )
}

pub struct Harness {
    pub runtime: Arc<CodexSessionRuntime>,
    pub peer: WirePeer,
    pub events: EventSubscription,
}

impl Harness {
    pub async fn open() -> ProviderResult<Self> {
        Self::with_options(options()).await
    }

    pub async fn with_options(options: CodexAdapterOptions) -> ProviderResult<Self> {
        Self::with_session(options, open_input()).await
    }

    pub async fn with_session(
        options: CodexAdapterOptions,
        input: OpenSession,
    ) -> ProviderResult<Self> {
        let (client, events, mut peer) = pipes();

        let adapter = CodexAdapterV2::new(options);

        let open = adapter.open_connected(input, client, events, CancellationToken::new());

        let server = async {
            let request = peer.read().await?;

            if request["method"] != "initialize" {
                return Err(protocol("Expected initialize"));
            }

            peer.respond(&request, initialize_response()).await?;

            let request = peer.read().await?;

            if request["method"] != "initialized" {
                return Err(protocol("Expected initialized"));
            }

            Ok(())
        };

        let (runtime, server) = tokio::join!(open, server);

        server?;

        let runtime = runtime?;

        let events = runtime.subscribe().await?;

        Ok(Self {
            runtime,
            peer,
            events,
        })
    }

    pub async fn start(&mut self, turn: &str) -> ProviderResult<()> {
        let runtime = self.runtime.clone();

        let input = turn_input("native-thread", 1, "work")?;

        let server = async {
            let config = self.peer.read().await?;

            if config["method"] != "config/read" {
                return Err(protocol("Expected config/read"));
            }

            self.peer
                .respond(
                    &config,
                    json!({
                    "config":{
                    }
                    ,"origins":{
                    }
                    }
                    ),
                )
                .await?;

            let request = self.peer.read().await?;

            if request["method"] != "turn/start" {
                return Err(protocol("Expected turn/start"));
            }

            self.peer
                .write(&json!({
                "method":"turn/started","params":{
                "threadId":"native-thread","turn":native_turn(turn,"inProgress")}
                }
                ))
                .await?;

            self.peer
                .respond(
                    &request,
                    json!({
                    "turn":native_turn(turn,"inProgress")}
                    ),
                )
                .await
        };

        let (result, server) = tokio::join!(runtime.start_turn(input), server);

        server?;

        result
    }

    pub async fn collect_until(
        &mut self,
        mut condition: impl FnMut(&ProviderEvent) -> bool,
    ) -> ProviderResult<Vec<ProviderEvent>> {
        let mut events = vec![];

        loop {
            let event = self
                .events
                .events
                .recv()
                .await
                .ok_or(ProviderError::SubscriptionClosed)??;

            let done = condition(&event);

            events.push(event);

            if done {
                return Ok(events);
            }
        }
    }
}

async fn mutate<T: Send + 'static>(
    control: &mpsc::Sender<Mutation>,
    operation: impl FnOnce(&mut Projection) -> ProviderResult<(T, Vec<ProviderEvent>)> + Send + 'static,
) -> ProviderResult<T> {
    let (send, receive) = oneshot::channel();

    control
        .send(Box::new(move |state| match operation(state) {
            Ok((result, events)) => (
                events,
                Box::new(move || {
                    let _ = send.send(Ok(result));
                }),
            ),
            Err(error) => (
                vec![],
                Box::new(move || {
                    let _ = send.send(Err(error));
                }),
            ),
        }))
        .await
        .map_err(|_| ProviderError::SessionStopped)?;

    receive.await.map_err(|_| ProviderError::SessionStopped)?
}

pub struct ReplayResult {
    pub events: Vec<ProviderEvent>,
    pub outbound_count: usize,
    pub diagnostics: Vec<String>,
}

pub struct FixtureExpectation {
    pub terminal_statuses: &'static [&'static str],
    pub assistant_texts: &'static [&'static str],
    pub item_types: &'static [&'static str],
    pub request_kinds: &'static [&'static str],
    pub subagent_results: Option<&'static [&'static str]>,
}

pub fn fixture_expectation(name: &str) -> Option<&'static FixtureExpectation> {
    use crate::ports::codex::testkit::fixtures;

    match name {
        "simple" => Some(&fixtures::simple::codex_output::EXPECTATION),
        "codex_model_reroute" => Some(&fixtures::codex_model_reroute::output::EXPECTATION),
        "message_steering" => Some(&fixtures::message_steering::codex_output::EXPECTATION),
        "multi_turn" => Some(&fixtures::multi_turn::codex_output::EXPECTATION),
        "queued_turn" => Some(&fixtures::queued_turn::codex_output::EXPECTATION),
        "plan_questions" => Some(&fixtures::plan_questions::codex_output::EXPECTATION),
        "proposed_plan" => Some(&fixtures::proposed_plan::codex_output::EXPECTATION),
        "subagent" => Some(&fixtures::subagent::codex_output::EXPECTATION),
        "subagent_continue" => Some(&fixtures::subagent_continue::codex_output::EXPECTATION),
        "subagent_v2" => Some(&fixtures::subagent_v2::codex_output::EXPECTATION),
        "subagent_v2_approval" => Some(&fixtures::subagent_v2_approval::codex_output::EXPECTATION),
        "subagent_v2_nested" => Some(&fixtures::subagent_v2_nested::codex_output::EXPECTATION),
        "subagent_v2_nested_approval" => {
            Some(&fixtures::subagent_v2_nested_approval::codex_output::EXPECTATION)
        }

        "thread_rollback" => Some(&fixtures::thread_rollback::codex_output::EXPECTATION),
        "thread_rollback_after_restart" => {
            Some(&fixtures::thread_rollback_after_restart::codex_output::EXPECTATION)
        }

        "thread_rollback_to_stopped_turn" => {
            Some(&fixtures::thread_rollback_to_stopped_turn::codex_output::EXPECTATION)
        }

        "todo_list" => Some(&fixtures::todo_list::codex_output::EXPECTATION),
        "tool_call_restricted_granular" => {
            Some(&fixtures::tool_call_restricted_granular::codex_output::EXPECTATION)
        }

        "tool_call_workspace_never" => {
            Some(&fixtures::tool_call_workspace_never::codex_output::EXPECTATION)
        }

        "turn_interrupt" => Some(&fixtures::turn_interrupt::codex_output::EXPECTATION),
        "turn_interrupt_mid_tool" => {
            Some(&fixtures::turn_interrupt_mid_tool::codex_output::EXPECTATION)
        }

        "web_search" => Some(&fixtures::web_search::codex_output::EXPECTATION),
        _ => None,
    }
}

pub fn verify_fixture(result: &ReplayResult, expected: &FixtureExpectation) -> ProviderResult<()> {
    let mut items = BTreeMap::new();

    let mut nodes = BTreeMap::new();

    let mut roots = std::collections::BTreeSet::new();

    let mut requests = BTreeMap::new();

    let mut subagents = BTreeMap::new();

    let mut threads = BTreeMap::new();

    let mut terminals = vec![];

    for event in &result.events {
        if let ProviderEventKind::Turn { provider_turn, .. } = &event.kind {
            roots.insert(
                serde_json::to_value(&provider_turn.node_id)
                    .map_err(|e| protocol(e.to_string()))?
                    .to_string(),
            );
        }

        let (table, value) = match &event.kind {
            ProviderEventKind::TurnItem { item, .. } => (&mut items, serde_json::to_value(item)),
            ProviderEventKind::Node(node) => (&mut nodes, serde_json::to_value(node)),
            ProviderEventKind::RuntimeRequest { request, .. } => {
                (&mut requests, serde_json::to_value(request))
            }

            ProviderEventKind::Subagent(task) => (&mut subagents, serde_json::to_value(task)),
            ProviderEventKind::AppThread { app_thread, .. } => {
                (&mut threads, serde_json::to_value(app_thread))
            }

            ProviderEventKind::Terminal { outcome, .. } => {
                terminals.push(match outcome {
                    TerminalOutcome::Completed { .. } => "completed",
                    TerminalOutcome::Interrupted { .. } => "interrupted",
                    TerminalOutcome::Cancelled { .. } => "cancelled",
                    TerminalOutcome::Failed { .. } => "failed",
                });

                continue;
            }

            _ => continue,
        };

        let value = value.map_err(|error| protocol(error.to_string()))?;

        table.insert(value["id"].to_string(), value);
    }

    if terminals != expected.terminal_statuses {
        return Err(protocol(format!(
            "Terminal statuses: expected {:?}, received {terminals:?}",
            expected.terminal_statuses
        )));
    }

    for text in expected.assistant_texts {
        if !items.values().any(|item| {
            item["type"] == "assistant_message"
                && item["streaming"] == false
                && item["text"]
                    .as_str()
                    .is_some_and(|value| value.contains(text))
        }) {
            return Err(protocol(format!(
                "Missing completed assistant text: {text}"
            )));
        }
    }

    for kind in expected.item_types {
        if !items.values().any(|item| item["type"] == *kind) {
            return Err(protocol(format!("Missing turn item type: {kind}")));
        }
    }

    let mut kinds: Vec<_> = requests
        .values()
        .map(|request| request["kind"].as_str().unwrap_or_default())
        .collect();

    kinds.sort_unstable();

    let mut expected_kinds = expected.request_kinds.to_vec();

    expected_kinds.sort_unstable();

    if kinds != expected_kinds
        || requests
            .values()
            .any(|request| request["status"] != "resolved")
    {
        return Err(protocol(format!(
            "Expected resolved requests {expected_kinds:?}, received {requests:?}"
        )));
    }

    if let Some(expected_results) = expected.subagent_results {
        if subagents.len() != expected_results.len() {
            return Err(protocol(format!(
                "Expected {} subagents, received {}",
                expected_results.len(),
                subagents.len()
            )));
        }

        for expected in expected_results {
            if !subagents.values().any(|task| {
                task["status"] == "completed"
                    && task["result"]
                        .as_str()
                        .is_some_and(|value| value.contains(expected))
            }) {
                return Err(protocol(format!(
                    "Missing completed subagent result: {expected}"
                )));
            }
        }
    }

    for item in items.values() {
        if !nodes.contains_key(&item["nodeId"].to_string())
            && !roots.contains(&item["nodeId"].to_string())
        {
            return Err(protocol(format!(
                "Turn item {} has no execution node",
                item["id"]
            )));
        }
    }

    for task in subagents.values() {
        let child = threads
            .get(&task["childThreadId"].to_string())
            .ok_or_else(|| protocol("Subagent has no app-owned child thread"))?;

        if task["origin"] != "provider_native"
            || task["createdBy"] != "agent"
            || task["driver"] != "codex"
            || child["lineage"]["parentThreadId"] != task["threadId"]
            || child["lineage"]["relationshipToParent"] != "subagent"
            || child["activeProviderThreadId"] != task["providerThreadId"]
        {
            return Err(protocol("Subagent ownership or child lineage mismatch"));
        }

        let parent_item = items
            .values()
            .find(|item| item["type"] == "subagent" && item["subagentId"] == task["id"])
            .ok_or_else(|| protocol("Subagent has no parent turn item"))?;

        if parent_item["result"] != task["result"] {
            return Err(protocol(
                "Subagent result differs from its parent turn item",
            ));
        }
    }

    for request in requests.values() {
        let node = nodes
            .get(&request["nodeId"].to_string())
            .ok_or_else(|| protocol("Request has no execution node"))?;

        let waiting = result.events.iter().position(|event| matches!(&event.kind, ProviderEventKind::TurnItem{
item,..}
 if serde_json::to_value(item).ok().is_some_and(|item| item["requestId"] == request["id"] && item["status"] == "waiting")));

        let pending = result.events.iter().position(|event| matches!(&event.kind, ProviderEventKind::RuntimeRequest{
request: value,..}
 if serde_json::to_value(value).ok().is_some_and(|value| value["id"] == request["id"] && value["status"] == "pending")));

        if !matches!((waiting,pending), (Some(waiting), Some(pending)) if waiting < pending)
            || node["status"] != "completed"
        {
            return Err(protocol(
                "Request must follow its waiting item and resolve its node",
            ));
        }
    }

    Ok(())
}

/// Replays wire frames through the real client and the production event projector.
pub async fn replay_projected(recording: &str) -> ProviderResult<ReplayResult> {
    let entries: Vec<Value> = recording
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .map_err(|e| protocol(e.to_string()))?;

    if entries
        .first()
        .is_none_or(|e| e["type"] != "transcript_start" || e["protocol"] != "codex.app-server")
    {
        return Err(protocol("Expected a Codex transcript header"));
    }

    let mut result = ReplayResult {
        events: vec![],
        outbound_count: 0,
        diagnostics: vec![],
    };

    let mut segments = vec![];

    let mut segment = vec![];

    for entry in entries.into_iter().skip(1) {
        if entry["type"] == "runtime_exit" {
            segments.push(std::mem::take(&mut segment));
        } else {
            segment.push(entry);
        }
    }

    if !segment.is_empty() {
        segments.push(segment);
    }

    for segment in segments {
        let (client, events, mut peer) = pipes();

        let stop = CancellationToken::new();

        let (send, mut receive) = mpsc::channel(128);

        let (control, commands) = mpsc::channel(32);

        let opts = options();

        let projection = Projection::new(
            opts.clock,
            opts.ids,
            open_input(),
            ProviderInstanceId("codex".into()),
        );

        let worker = tokio::spawn(mapping::run(
            projection,
            events,
            commands,
            send,
            stop.clone(),
        ));

        let (pending_send, mut pending_receive) = mpsc::channel(32);

        let collector = tokio::spawn(async move {
            let mut events = vec![];

            let mut diagnostics = vec![];

            while let Some(event) = receive.recv().await {
                match event {
                    Ok(event) => {
                        if matches!(&event.kind,ProviderEventKind::RuntimeRequest{
request,..}
 if request.status==RuntimeRequestStatus::Pending)
                        {
                            let _ = pending_send.send(()).await;
                        }

                        events.push(event);
                    }

                    Err(error) => diagnostics.push(error.to_string()),
                }
            }

            (events, diagnostics)
        });

        let mut requests = vec![];

        let mut ids = BTreeMap::new();

        let mut ordinal = 0;

        for entry in segment {
            match entry["type"].as_str() {
                Some("expect_outbound") => {
                    let expected = &entry["frame"];

                    if let Some(method) = expected["method"].as_str() {
                        if method == "turn/start" {
                            ordinal += 1;

                            let mut input = turn_input(
                                expected["params"]["threadId"]
                                    .as_str()
                                    .ok_or_else(|| protocol("Turn start has no thread id"))?,
                                ordinal,
                                "recorded",
                            )?;

                            if let Some(model) = expected["params"]["model"].as_str() {
                                input.model_selection.model = model.into();
                            }

                            mutate(&control, move |state| state.prepare_start(input)).await?;
                        }

                        if expected.get("id").is_some() {
                            let client = client.clone();

                            let method = method.to_owned();

                            let params = expected
                                .get("params")
                                .cloned()
                                .map_or(Params::Missing, Params::Present);

                            requests.push(tokio::spawn(async move {
                                client.request_raw(&method, params).await
                            }));
                        } else {
                            client
                                .notify(
                                    method,
                                    expected
                                        .get("params")
                                        .cloned()
                                        .map_or(Params::Missing, Params::Present),
                                )
                                .await
                                .map_err(native_error)?;
                        }
                    } else {
                        pending_receive
                            .recv()
                            .await
                            .ok_or_else(|| protocol("Recorded request was not surfaced"))?;

                        let expected = expected.clone();

                        let (response, input) =
                            mutate(&control, move |state| state.recorded_response(&expected))
                                .await?;

                        response.send(input.clone()).await?;

                        mutate(&control, move |state| {
                            state.finish_response(input.request_id.clone(), input, true)
                        })
                        .await?;
                    }

                    let actual = peer.read().await?;

                    if let Some(id) = expected.get("id")
                        && expected.get("method").is_some()
                    {
                        ids.insert(id.to_string(), actual["id"].clone());
                    }

                    let mut normalized = actual.clone();

                    if expected.get("id").is_some() {
                        normalized["id"] = expected["id"].clone();
                    }

                    if &normalized != expected {
                        return Err(protocol(format!(
                            "Recorded outbound mismatch: expected {expected}, received {normalized}"
                        )));
                    }

                    result.outbound_count += 1;
                }

                Some("emit_inbound") => {
                    let mut frame = entry["frame"].clone();

                    if frame.get("method").is_none()
                        && let Some(id) = ids.get(&frame["id"].to_string())
                    {
                        frame["id"] = id.clone();
                    }

                    peer.write(&frame).await?;
                }

                _ => return Err(protocol("Unsupported Codex replay entry")),
            }
        }

        let barrier_client = client.clone();

        let barrier = tokio::spawn(async move {
            barrier_client
                .request_raw("test/replayBarrier", Params::Missing)
                .await
        });

        let request = peer.read().await?;

        peer.respond(&request, json!({})).await?;

        barrier
            .await
            .map_err(|e| protocol(e.to_string()))?
            .map_err(native_error)?;

        mutate(&control, |state| Ok(((), state.flush(true)?))).await?;

        for request in requests {
            if let Err(error) = request.await.map_err(|e| protocol(e.to_string()))? {
                result.diagnostics.push(error.to_string());
            }
        }

        stop.cancel();

        let _ = client.shutdown().await;

        worker.await.map_err(|e| protocol(e.to_string()))?;

        let (events, diagnostics) = collector.await.map_err(|e| protocol(e.to_string()))?;

        result.events.extend(events);

        result.diagnostics.extend(diagnostics);
    }

    Ok(result)
}

pub fn fixture(name: &str) -> ProviderResult<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/server/src/orchestration-v2/testkit/fixtures");

    std::fs::read_to_string(path.join(name).join("codex_transcript.ndjson"))
        .map_err(|e| protocol(e.to_string()))
}

pub use crate::ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult;

pub fn assert_scenario_output(
    name: &str,
    result: &ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::codex::testkit::fixtures;

    match name {

        "codex_model_reroute"=>fixtures::codex_model_reroute::output::assert_codex_model_reroute_output(result,transcript),
        "message_steering"=>fixtures::message_steering::codex_output::assert_message_steering_output(result,transcript),
        "multi_turn"=>fixtures::multi_turn::codex_output::assert_multi_turn_output(result,transcript),
        "plan_questions"=>fixtures::plan_questions::codex_output::assert_codex_plan_questions_output(result,transcript),
        "proposed_plan"=>fixtures::proposed_plan::codex_output::assert_proposed_plan_output(result,transcript),
        "queued_cancelled_while_active"=>fixtures::queued_cancelled_while_active::codex_output::assert_queued_cancelled_while_active_output(result,transcript),
        "queued_turn"=>fixtures::queued_turn::codex_output::assert_queued_turn_output(result,transcript),
        "simple"=>fixtures::simple::codex_output::assert_simple_output(result,transcript),
        "subagent"=>fixtures::subagent::codex_output::assert_subagent_output(result,transcript),
        "subagent_continue"=>fixtures::subagent_continue::codex_output::assert_subagent_continue_output(result,transcript),
        "subagent_v2"=>fixtures::subagent_v2::codex_output::assert_subagent_v2_output(result,transcript),
        "subagent_v2_approval"=>fixtures::subagent_v2_approval::codex_output::assert_subagent_v2_approval_output(result,transcript),
        "subagent_v2_nested"=>fixtures::subagent_v2_nested::codex_output::assert_subagent_v2_nested_output(result,transcript),
        "subagent_v2_nested_approval"=>fixtures::subagent_v2_nested_approval::codex_output::assert_subagent_v2_nested_approval_output(result,transcript),
        "thread_rollback"=>fixtures::thread_rollback::codex_output::assert_thread_rollback_output(result,transcript),
        "thread_rollback_after_restart"=>fixtures::thread_rollback_after_restart::codex_output::assert_thread_rollback_after_restart_output(result,transcript),
        "thread_rollback_to_stopped_turn"=>fixtures::thread_rollback_to_stopped_turn::codex_output::assert_thread_rollback_to_stopped_turn_output(result,transcript),
        "todo_list"=>fixtures::todo_list::codex_output::assert_todo_list_output(result,transcript),
        "tool_call_restricted_granular"=>fixtures::tool_call_restricted_granular::codex_output::assert_tool_call_restricted_granular_output(result,transcript),
        "tool_call_workspace_never"=>fixtures::tool_call_workspace_never::codex_output::assert_tool_call_workspace_never_output(result,transcript),
        "turn_interrupt"=>fixtures::turn_interrupt::codex_output::assert_turn_interrupt_output(result,transcript),
        "turn_interrupt_mid_tool"=>fixtures::turn_interrupt_mid_tool::codex_output::assert_turn_interrupt_mid_tool_output(result,transcript),
        "web_search"=>fixtures::web_search::codex_output::assert_web_search_output(result,transcript),
        _=>Err(crate::error::Error::Service{
operation:"Codex scenario assertion",detail:format!("Unknown fixture {name}")}
)
    }
}

pub fn scenario_details(
    name: &str,
    result: &ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
    projection: &Value,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared::*;

    require(transcript.provider.0 == "codex", "Codex scenario driver")?;

    let row = |table: &str, index: usize| {
        rows(projection, table)
            .nth(index)
            .cloned()
            .unwrap_or(Value::Null)
    };

    match name {

        "simple"=>{
}
,
        "multi_turn"|"queued_turn"=>{

            require(rows(projection,"turnItems").filter(|item|item["type"]=="user_message").count()==2,"two durable user messages")?;

            if name=="queued_turn" {
let events=serde_json::to_value(&result.domain_events)?;
let events:Vec<_>=events.as_array().into_iter().flatten().filter(|event|matches!(event["type"].as_str(),Some("run.created"|"run.updated")) && event["runId"]==projection["runs"][1]["id"]).collect();
require(events.first().is_some_and(|event|event["type"]=="run.created" && event["payload"]["status"]=="queued") && events.iter().any(|event|event["payload"]["status"]=="running"),"queued run must transition from created/queued to running")?;
}

        }
,
        "message_steering"=>require(rows(projection,"runs").count()==1 && rows(projection,"providerTurns").count()==1,"steering stays in the active run/native turn")?,
        "plan_questions"|"plan_questions_base"=>{
}
,
        "proposed_plan"=>{
let plan=rows(projection,"plans").rfind(|plan|plan["kind"]=="proposed_plan");
require(plan.is_some_and(|plan|plan["status"]=="active" && plan["markdown"].as_str().is_some_and(|text|text.to_lowercase().contains("replay") && text.to_lowercase().contains("fixture"))),"completed proposal stays active and describes the replay fixture")?;
require(rows(projection,"turnItems").rfind(|item|item["type"]=="proposed_plan").is_some_and(|item|item["streaming"]==false),"final proposal does not stream")?;
}
,
        "todo_list"=>{
let plan=rows(projection,"plans").rfind(|plan|plan["kind"]=="todo_list");
require(plan.is_some_and(|plan|values(plan,"steps","status")==vec![json!("completed");
3]),"all three todo steps complete")?;
}
,
        "tool_call_restricted_granular"=>require(rows(projection,"turnItems").find(|item|item["type"]=="approval_request").is_some_and(|item|item["prompt"]=="add /home/replay-user/t3code/packages/effect-codex-app-server/.codex-probe-write-action.txt"),"file approval preserves native path and operation")?,
        "tool_call_workspace_never"=>require(!rows(projection,"turnItems").any(|item|item["type"]=="approval_request"),"never approval mode creates no approval artifacts")?,
        "web_search"=>{
let searches:Vec<_>=rows(projection,"turnItems").filter(|item|item["type"]=="web_search").collect();
require(searches.len()==1 && searches[0]["status"]=="completed" && searches[0]["patterns"].as_array().is_some_and(|patterns|patterns.contains(&json!(crate::ports::codex::testkit::fixtures::web_search::codex_output::WEB_SEARCH_QUERY))),"one completed search preserves its query")?;
}
,
        "codex_model_reroute"=>{
require(row("providerTurns",0)["turnTokenUsage"]["models"]==json!(["gpt-6-astra"]),"first turn records the reroute model")?;
require(row("providerTurns",1)["turnTokenUsage"].get("models").is_none(),"reroute does not set the next turn model")?;
}
,
        "queued_cancelled_while_active"=>{

            let active=row("runs",0);
let cancelled=row("runs",1);
require(active.is_object() && cancelled.is_object() && cancelled["startedAt"].is_null(),"cancelled queued run never starts")?;

            require(rows(projection,"messages").find(|message|message["id"]==cancelled["userMessageId"]).is_some_and(|message|message["createdBy"]=="agent" && message["creationSource"]=="server"),"queued message retains its server/agent provenance")?;

            let shell=result.captured_shell_snapshots.get("active-run-over-cancelled-latest").map(serde_json::to_value).transpose()?.unwrap_or(Value::Null);
let thread=rows(&shell,"threads").find(|thread|thread["id"]==projection["thread"]["id"]).cloned().unwrap_or(Value::Null);

            require(thread.is_object() && thread["latestRunId"]==active["id"] && thread["activeRunId"]==active["id"] && thread["status"]=="running" && thread["activityRunStatus"]=="running" && thread["activityRunStartedAt"]==active.get("startedAt").filter(|value|!value.is_null()).unwrap_or(&active["requestedAt"]).clone() && thread["pendingBackgroundTasks"]==json!([]),"active run stays visible over a newer cancelled queued run")?;

        }
,
        "thread_rollback"|"thread_rollback_after_restart"|"thread_rollback_to_stopped_turn"=>{

            if name=="thread_rollback_after_restart" {
crate::ports::codex::testkit::fixtures::thread_rollback::codex_output::assert_thread_rollback_output(result,transcript)?;
}

            let recall=rows(projection,"turnItems").rfind(|item|item["type"]=="assistant_message").cloned().unwrap_or(Value::Null);
let text=recall["text"].as_str().unwrap_or_default();

            require(text.contains("first turn complete") && !text.contains("second turn"),"rollback recall excludes the removed turn")?;

            if name=="thread_rollback_to_stopped_turn" {
let stopped=rows(projection,"runs").find(|run|run["ordinal"]==2).cloned().unwrap_or(Value::Null);
let checkpoint=rows(projection,"checkpoints").find(|checkpoint|checkpoint["appRunOrdinal"]==2).cloned().unwrap_or(Value::Null);
require(checkpoint["status"]=="ready" && checkpoint["runId"]==stopped["id"] && stopped["checkpointId"]==checkpoint["id"],"interrupted run checkpoint stays a valid rollback boundary")?;
require(text.contains("interrupt fixture tool started"),"rollback retains stopped-turn conversation")?;
}

            else {
require(rows(projection,"checkpoints").count()>=2 && rows(projection,"runs").any(|run|run["status"]=="rolled_back"),"rollback remains visible with ready checkpoints")?;
}

            let methods=recorded_methods(transcript)?;
let revert=methods.iter().position(|method|method=="thread/revert");
if name!="thread_rollback" {
require(revert.is_some(),"rollback reaches native thread/revert")?;
}

            if name=="thread_rollback_after_restart" {
let revert=revert.unwrap_or(0);
require(revert>1 && methods[..revert].iter().rposition(|method|method=="thread/resume")==Some(revert-2),"fresh server resumes immediately before turns/list and revert")?;
}

        }
,
        "turn_interrupt"|"turn_interrupt_mid_tool"=>{

            let request=rows(projection,"turnItems").find(|item|item["type"]=="run_interrupt_request").cloned().unwrap_or(Value::Null);
let response=rows(projection,"turnItems").find(|item|item["type"]=="run_interrupt_result").cloned().unwrap_or(Value::Null);
require(request.is_object() && response.is_object() && request["status"]=="completed" && response["status"]=="interrupted" && response["parentItemId"]==request["id"],"interrupt request/result are linked and terminal")?;

            require(values(projection,"attempts","status")==json!(["interrupted"]).as_array().cloned().unwrap_or_default() && row("providerThreads",0)["status"]=="idle" && matches!(row("providerTurns",0)["status"].as_str(),Some("interrupted"|"cancelled")),"interrupted attempt/provider lifecycle")?;

            if name=="turn_interrupt" {
require(values(projection,"nodes","status")==vec![json!("interrupted")],"root execution node interrupted")?;
}

            else {
let command=rows(projection,"turnItems").find(|item|item["type"]=="command_execution").cloned().unwrap_or(Value::Null);
require(command["status"]=="interrupted" && !command["completedAt"].is_null() && command["input"].as_str().is_some_and(|input|input.contains("node -e")),"interrupted command has the native input and completion time")?;
require(!rows(projection,"turnItems").any(|item|item["type"]=="command_execution" && item["status"]=="running") && rows(projection,"nodes").filter(|node|node["kind"]=="tool_call").all(|node|node["status"]!="running" && !node["completedAt"].is_null()),"Stop leaves no running tool artifacts")?;
assert_interrupt_recording(transcript)?;
}

        }
,
        name if name.starts_with("subagent")=>assert_subagent_scenario(name,result,projection)?,
        _=>require(false,format!("unmapped Codex scenario detail {name}"))?,
    }

    Ok(())
}

fn recorded_methods(
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<Vec<String>> {
    let entries = serde_json::to_value(&transcript.entries)?;

    Ok(entries
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["type"] == "expect_outbound")
        .filter_map(|entry| entry["frame"]["method"].as_str().map(str::to_owned))
        .collect())
}

fn assert_interrupt_recording(
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared::require;

    let entries = serde_json::to_value(&transcript.entries)?;

    let entries = entries.as_array().cloned().unwrap_or_default();

    let command = entries.iter().position(|entry| {
        entry["type"] == "emit_inbound"
            && entry["frame"]["method"] == "item/started"
            && entry["frame"]["params"]["item"]["type"] == "commandExecution"
    });

    let interrupt = entries.iter().position(|entry| {
        entry["type"] == "expect_outbound" && entry["frame"]["method"] == "turn/interrupt"
    });

    let complete = entries.iter().position(|entry| {
        entry["type"] == "emit_inbound" && entry["frame"]["method"] == "turn/completed"
    });

    let terminate = entries.iter().position(|entry| {
        entry["type"] == "expect_outbound"
            && entry["frame"]["method"] == "thread/backgroundTerminals/terminate"
    });

    require(
        matches!((command,interrupt,complete,terminate),(Some(command),Some(interrupt),Some(_),Some(terminate)) if interrupt>command && terminate>interrupt),
        "recording interrupts after command start and then terminates its terminal",
    )?;

    if let (Some(command), Some(terminate)) = (command, terminate) {
        let item = &entries[command]["frame"]["params"]["item"];

        let params = &entries[terminate]["frame"]["params"];

        require(
            params["threadId"] == entries[command]["frame"]["params"]["threadId"]
                && params["processId"] == item["processId"],
            "termination targets the recorded command process/thread",
        )?;
    }

    Ok(())
}

pub fn assert_question_id(
    result: &ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
    question: &str,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared::*;

    let projection = projection_value_for(result, &transcript.scenario.0)?;

    require(
        rows(&projection, "turnItems")
            .find(|item| item["type"] == "user_input_request")
            .is_some_and(|item| item["questions"][0]["id"] == question),
        "native question ID is preserved",
    )
}

fn scenario_projection(result: &ScenarioResult, id: &Value) -> crate::error::Result<Value> {
    let key = id.as_str().ok_or_else(|| crate::error::Error::Service {
        operation: "Codex fixture",
        detail: "missing child thread ID".into(),
    })?;

    let projection = result
        .projections
        .get(key)
        .ok_or_else(|| crate::error::Error::Service {
            operation: "Codex fixture",
            detail: format!("missing child projection {key}"),
        })?;

    Ok(serde_json::to_value(projection)?)
}

fn assert_subagent_scenario(
    name: &str,
    result: &ScenarioResult,
    projection: &Value,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared::*;

    let shell = serde_json::to_value(&result.shell_snapshot)?;

    let tasks: Vec<_> = rows(projection, "subagents").collect();

    let continued = name == "subagent_continue";

    let nested = name.contains("nested");

    let approval = name.contains("approval");

    require(
        tasks.len() == if name == "subagent" { 2 } else { 1 },
        "root has only its direct native children",
    )?;

    require(
        rows(projection, "runs").count() == if continued { 2 } else { 1 },
        "native turns do not become app runs",
    )?;

    require(
        !rows(projection, "turnItems").any(|item| item["type"] == "command_execution"),
        "child commands stay out of the root thread",
    )?;

    if name != "subagent_continue" && !approval {
        require(
            rows(&shell, "threads").count()
                == if name == "subagent" {
                    3
                } else if nested {
                    4
                } else {
                    2
                },
            "shell includes the native child threads",
        )?;
    }

    let root_id = &projection["thread"]["id"];

    for parent in result.projections.values() {
        let parent = serde_json::to_value(parent)?;

        for task in rows(&parent, "subagents") {
            require(
                task["origin"] == "provider_native"
                    && task["createdBy"] == "agent"
                    && task["driver"] == "codex"
                    && task["status"] == "completed"
                    && !task["completedAt"].is_null()
                    && !task["childThreadId"].is_null()
                    && !task["providerThreadId"].is_null()
                    && !task["nativeTaskRef"].is_null(),
                "native task ownership and terminal lifecycle",
            )?;

            let parent_item = rows(&parent, "turnItems")
                .find(|item| item["type"] == "subagent" && item["subagentId"] == task["id"]);

            require(
                parent_item.is_some_and(|item| item["result"] == task["result"]),
                "parent lifecycle item mirrors the child result",
            )?;

            let provider = rows(&parent, "providerThreads")
                .find(|thread| thread["id"] == task["providerThreadId"])
                .cloned()
                .unwrap_or(Value::Null);

            require(
                provider.is_object()
                    && provider["appThreadId"] == task["childThreadId"]
                    && provider["ownerNodeId"].is_null(),
                "child provider thread belongs to its app thread",
            )?;

            let child = scenario_projection(result, &task["childThreadId"])?;

            require(
                child["thread"]["lineage"]["parentThreadId"] == parent["thread"]["id"]
                    && child["thread"]["lineage"]["relationshipToParent"] == "subagent"
                    && child["thread"]["lineage"]["rootThreadId"] == *root_id
                    && child["thread"]["activeProviderThreadId"] == provider["id"]
                    && rows(&child, "runs").count() == 0
                    && rows(&child, "providerThreads").any(|thread| thread["id"] == provider["id"]),
                format!(
                    "native child lineage and runless ownership: child {}; parent {}; root {}; provider {}",
                    child["thread"], parent["thread"]["id"], root_id, provider["id"]
                ),
            )?;

            require(
                rows(&child, "providerTurns").count() == if continued { 2 } else { 1 },
                "child native turn count",
            )?;

            if name == "subagent" || continued {
                let prompt = task["prompt"].as_str().unwrap_or_default();

                assert_user_messages_include(&child, &[prompt])?;
            }

            require(
                rows(&child, "turnItems").any(|item| {
                    item["type"] == "assistant_message"
                        && item["text"].as_str().is_some_and(|text| {
                            text.contains(
                                &task["result"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .chars()
                                    .take(40)
                                    .collect::<String>(),
                            )
                        })
                }),
                "child conversation contains its native final result",
            )?;

            if nested && !approval {
                let title = task["title"].as_str().unwrap_or_default();

                require(
                    title.starts_with("/root/")
                        && !title.ends_with('/')
                        && title.split('/').skip(2).all(|part| !part.is_empty()),
                    "v2 child title is an agent path",
                )?;

                require(
                    rows(&parent, "turnItems")
                        .filter(|item| item["type"] == "subagent")
                        .count()
                        == 1,
                    "each thread renders only its direct child",
                )?;
            }
        }
    }

    match name {
        "subagent" => {
            require(
                tasks.iter().any(|task| {
                    task["prompt"].as_str().is_some_and(|prompt| {
                        prompt.contains("package.json") && !prompt.contains("tsconfig.json")
                    }) && task["result"]
                        .as_str()
                        .is_some_and(|text| text.contains("effect-codex-app-server"))
                }),
                "package child result matches its distinct prompt",
            )?;

            require(
                tasks.iter().any(|task| {
                    task["prompt"].as_str().is_some_and(|prompt| {
                        prompt.contains("tsconfig.json") && !prompt.contains("package.json")
                    }) && task["result"]
                        .as_str()
                        .is_some_and(|text| text.contains("../../tsconfig.base.json"))
                }),
                "tsconfig child result matches its distinct prompt",
            )?;

            for task in &tasks {
                assert_turn_item_types(
                    &scenario_projection(result, &task["childThreadId"])?,
                    &["user_message", "command_execution", "assistant_message"],
                )?;
            }

            let provider = rows(projection, "providerThreads")
                .find(|thread| thread["ownerNodeId"].is_null() && thread["appThreadId"] == *root_id)
                .cloned()
                .unwrap_or(Value::Null);

            require(
                provider.is_object()
                    && projection["thread"]["activeProviderThreadId"] == provider["id"],
                "root active provider thread remains the root's",
            )?;
        }

        "subagent_continue" => {
            let task = tasks[0];

            require(
                task["result"] == "continued subagent response"
                    && task["runId"] == projection["runs"][1]["id"],
                "resumed child attaches to the continuing parent run",
            )?;

            let child = scenario_projection(result, &task["childThreadId"])?;

            assert_user_messages_include(&child, &[SUBAGENT_CONTINUE_CHILD_PROMPT])?;

            for text in ["initial subagent response", "continued subagent response"] {
                require(
                    rows(&child, "turnItems")
                        .any(|item| item["type"] == "assistant_message" && item["text"] == text),
                    "child preserves both native turn replies",
                )?;
            }
        }

        "subagent_v2" => {
            let task = tasks[0];

            let title = task["title"].as_str().unwrap_or_default();

            require(
                title.starts_with("/root/")
                    && !title[6..].contains('/')
                    && !title[6..].is_empty()
                    && task["prompt"] == ""
                    && task["result"] == "Hello.",
                "v2 root child path, implicit prompt and result",
            )?;

            let child = scenario_projection(result, &task["childThreadId"])?;

            assert_turn_item_types(&child, &["assistant_message"])?;

            require(
                child["providerTurns"][0]["status"] == "completed",
                "child completes after its root",
            )?;

            let root_usage = &projection["providerTurns"][0]["turnTokenUsage"];

            let child_usage = &child["providerTurns"][0]["turnTokenUsage"];

            require(
                root_usage["usageStatus"] == "complete"
                    && root_usage["outputTokens"] == 69
                    && root_usage["models"] == json!(["gpt-6-luna"])
                    && child_usage["usageStatus"] == "complete"
                    && child_usage["outputTokens"] == 6
                    && child_usage.get("models").is_none(),
                "root and child retain separate usage and model provenance",
            )?;
        }

        "subagent_v2_nested" => {
            let mut parent = projection.clone();

            let mut path = "/root".to_owned();

            for depth in 0..3 {
                let children: Vec<_> = rows(&parent, "subagents").collect();

                require(children.len() == 1, "three-level child chain")?;

                let task = children[0];

                let title = task["title"].as_str().unwrap_or_default();

                let prefix = format!("{path}/");

                require(
                    title.starts_with(&prefix)
                        && !title[prefix.len()..].is_empty()
                        && !title[prefix.len()..].contains('/')
                        && task["result"] == "Hello.",
                    "each native path is one child of its direct parent's path",
                )?;

                path = title.into();

                parent = scenario_projection(result, &task["childThreadId"])?;

                assert_turn_item_types(
                    &parent,
                    if depth < 2 {
                        &["subagent", "assistant_message"]
                    } else {
                        &["assistant_message"]
                    },
                )?;
            }

            require(
                rows(&parent, "subagents").count() == 0
                    && rows(&parent, "turnItems").any(|item| {
                        item["type"] == "assistant_message" && item["text"] == "Hello."
                    }),
                "leaf has the final answer and no children",
            )?;
        }

        "subagent_v2_approval" | "subagent_v2_nested_approval" => {
            let top = tasks[0];

            require(
                top["result"] == "Written.",
                "top-level approved task result",
            )?;

            let child = scenario_projection(result, &top["childThreadId"])?;

            let leaf = if nested {
                let task = rows(&child, "subagents")
                    .next()
                    .cloned()
                    .unwrap_or(Value::Null);

                require(
                    rows(&child, "subagents").count() == 1
                        && task["status"] == "completed"
                        && task["result"] == "Written.",
                    "approved grandchild result",
                )?;

                scenario_projection(result, &task["childThreadId"])?
            } else {
                child.clone()
            };

            let requests: Vec<_> = rows(projection, "runtimeRequests").collect();

            require(requests.len() == 1, "one approval lifted to the root")?;

            let request = requests[0];

            let run = &projection["runs"][0];

            require(
                request["kind"] == "command"
                    && request["status"] == "resolved"
                    && request["decision"] == "accept"
                    && rows(projection, "providerTurns")
                        .find(|turn| turn["id"] == request["providerTurnId"])
                        .is_some_and(|turn| turn["runAttemptId"] == run["activeAttemptId"]),
                "root provider turn owns the approval",
            )?;

            let node = rows(projection, "nodes")
                .find(|node| node["id"] == request["nodeId"])
                .cloned()
                .unwrap_or(Value::Null);

            require(
                node["kind"] == "approval_request"
                    && node["threadId"] == *root_id
                    && node["runId"] == run["id"]
                    && node["parentNodeId"] == top["id"]
                    && node["status"] == "completed",
                "approval node hangs off the nearest root-thread subagent ancestor",
            )?;

            let approvals: Vec<_> = rows(projection, "turnItems")
                .filter(|item| item["type"] == "approval_request")
                .collect();

            require(
                approvals.len() == 1
                    && approvals[0]["runId"] == run["id"]
                    && approvals[0]["status"] == "completed"
                    && approvals[0]["requestId"] == request["id"]
                    && approvals[0]["prompt"].as_str().is_some_and(|prompt| {
                        prompt.contains(if nested {
                            "nested-approval.txt"
                        } else {
                            "subagent-approval.txt"
                        })
                    }),
                "one completed approval item preserves native prompt and request identity",
            )?;

            let events = serde_json::to_value(&result.domain_events)?;

            let events = events.as_array().cloned().unwrap_or_default();

            let waiting = events.iter().position(|event| {
                event["type"] == "turn-item.updated"
                    && event["payload"]["id"] == approvals[0]["id"]
                    && event["payload"]["status"] == "waiting"
            });

            let pending = events.iter().position(|event| {
                event["type"] == "runtime-request.updated"
                    && event["payload"]["id"] == request["id"]
                    && event["payload"]["status"] == "pending"
            });

            require(
                matches!((waiting,pending),(Some(waiting),Some(pending)) if pending>waiting),
                "request becomes actionable after its waiting artifact",
            )?;

            let key = if nested {
                "nested-subagent-approval-pending"
            } else {
                "subagent-approval-pending"
            };

            let captured = result
                .captured_shell_snapshots
                .get(key)
                .map(serde_json::to_value)
                .transpose()?
                .unwrap_or(Value::Null);

            require(
                rows(&captured, "threads")
                    .find(|thread| thread["id"] == *root_id)
                    .is_some_and(|thread| thread["pendingRuntimeRequest"]["id"] == request["id"]),
                "pending shell surfaces only the root's actionable approval",
            )?;

            for descendant in [&child, &leaf] {
                require(
                    rows(&captured, "threads")
                        .find(|thread| thread["id"] == descendant["thread"]["id"])
                        .is_some_and(|thread| thread["pendingRuntimeRequest"].is_null()),
                    "child shell exists without duplicated root approval",
                )?;

                require(
                    rows(descendant, "runtimeRequests").count() == 0
                        && !rows(descendant, "turnItems")
                            .any(|item| item["type"] == "approval_request"),
                    "child histories contain no duplicate approval",
                )?;
            }

            require(
                rows(&leaf, "turnItems")
                    .find(|item| item["type"] == "command_execution")
                    .is_some_and(|item| item["status"] == "completed")
                    && rows(&leaf, "turnItems").any(|item| {
                        item["type"] == "assistant_message"
                            && item["text"]
                                .as_str()
                                .is_some_and(|text| text.contains("Written."))
                    }),
                "approved leaf command and reply complete",
            )?;
        }

        _ => require(false, "unknown native-child fixture")?,
    }

    Ok(())
}

pub struct CodexReplayAdapter {
    adapter: CodexAdapterV2,
    transcript: Arc<Vec<Value>>,
    ignored_config: Arc<std::collections::BTreeSet<String>>,
    cursor: Arc<std::sync::Mutex<usize>>,
    pub gate: crate::testing::replay::ReplayGate,
    pub progress: tokio::sync::watch::Receiver<crate::testing::replay::ReplayStatus>,
    status: tokio::sync::watch::Sender<crate::testing::replay::ReplayStatus>,
    tasks: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    stop: CancellationToken,
}

impl CodexReplayAdapter {
    pub fn new(
        options: CodexAdapterOptions,
        transcript: crate::testing::replay::ReplayTranscript,
        gate: crate::testing::replay::ReplayGate,
    ) -> ProviderResult<Self> {
        if transcript.provider.0 != "codex" || transcript.protocol.0 != "codex.app-server" {
            return Err(protocol("Expected a Codex app-server replay transcript"));
        }

        let entries = serde_json::to_value(&transcript.entries)
            .map_err(|e| protocol(e.to_string()))?
            .as_array()
            .cloned()
            .ok_or_else(|| protocol("Expected replay entries"))?;

        let ignored_config = transcript
            .metadata
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|metadata| metadata.get("recorderThreadConfigKeys"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let (status, progress) =
            tokio::sync::watch::channel(crate::testing::replay::ReplayStatus {
                scenario: transcript.scenario.0,
                cursor: 0,
                total: entries.len(),
                failure: None,
            });

        Ok(Self {
            adapter: CodexAdapterV2::new(options),
            transcript: Arc::new(entries),
            ignored_config: Arc::new(ignored_config),
            cursor: Arc::new(std::sync::Mutex::new(0)),
            gate,
            progress,
            status,
            tasks: tokio::sync::Mutex::new(vec![]),
            stop: CancellationToken::new(),
        })
    }

    pub fn assert_complete(&self) -> ProviderResult<()> {
        self.progress
            .borrow()
            .assert_complete()
            .map_err(|error| protocol(error.to_string()))
    }

    pub async fn shutdown(&self) {
        self.gate.release_all();

        self.stop.cancel();

        for task in std::mem::take(&mut *self.tasks.lock().await) {
            let _ = task.await;
        }
    }
}

impl Drop for CodexReplayAdapter {
    fn drop(&mut self) {
        self.gate.release_all();

        self.stop.cancel();
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for CodexReplayAdapter {
    fn instance_id(&self) -> &ProviderInstanceId {
        self.adapter.instance_id()
    }

    fn driver(&self) -> t3_contracts::provider_instance::ProviderDriverKind {
        self.adapter.driver()
    }

    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        self.adapter.capabilities().await
    }

    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        self.adapter.plan_selection_transition(input).await
    }

    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        let (client, events, peer) = pipes();

        let transcript = self.transcript.clone();

        let ignored_config = self.ignored_config.clone();
        let cursor = self.cursor.clone();

        let status = self.status.clone();

        let gate = self.gate.clone();

        let stop = self.stop.child_token();

        let workspace = input
            .runtime_policy
            .cwd
            .as_ref()
            .map(|cwd| cwd.to_string_lossy().into_owned())
            .unwrap_or_default();

        self.tasks.lock().await.push(tokio::spawn(async move {
            let result = run_wire_replay(
                peer,
                WireReplayConfig { transcript:&transcript, ignored_config:&ignored_config, workspace:&workspace },
                cursor,
                status.clone(),
                gate,
                &stop,
            )
            .await;

            if let Err(error) = result {
                status.send_modify(|status| {
                    status.failure = Some(json!({
                    "error":format!("{error:?}")}
                    ))
                });
            }
        }));

        self.adapter
            .open_connected(input, client, events, cancellation)
            .await
            .map(|runtime| runtime as Arc<dyn SessionRuntime>)
    }
}

struct WireReplayConfig<'a> {
    transcript: &'a [Value],
    ignored_config: &'a std::collections::BTreeSet<String>,
    workspace: &'a str,
}
async fn run_wire_replay(
    mut peer: WirePeer,
    config: WireReplayConfig<'_>,
    cursor: Arc<std::sync::Mutex<usize>>,
    status: tokio::sync::watch::Sender<crate::testing::replay::ReplayStatus>,
    gate: crate::testing::replay::ReplayGate,
    stop: &CancellationToken,
) -> ProviderResult<()> {
    let WireReplayConfig { transcript, ignored_config, workspace } = config;
    let mut ids: BTreeMap<String, Value> = BTreeMap::new();
    let mut children = std::collections::BTreeSet::new();
    for entry in transcript {
        if entry["type"] != "emit_inbound" {
            continue;
        }
        let item = &entry["frame"]["params"]["item"];
        if item["type"] == "subAgentActivity" {
            if let Some(id) = item["agentThreadId"].as_str() {
                children.insert(id.to_owned());
            }
        } else if item["type"] == "collabAgentToolCall" {
            children.extend(
                item["receiverThreadIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
    }
    loop {
        let index = *cursor.lock().unwrap_or_else(|e| e.into_inner());

        let Some(entry) = transcript.get(index) else {
            let actual = read_codex_replay_outbound(&mut peer, &children, stop).await?;
            return if stop.is_cancelled() {
                Ok(())
            } else {
                Err(protocol(format!(
                    "Codex replay exhausted; received {actual}"
                )))
            };
        };

        match entry["type"].as_str() {
            Some("runtime_exit") => {
                *cursor.lock().unwrap_or_else(|e| e.into_inner()) += 1;

                status.send_modify(|status| status.cursor = index + 1);

                return Ok(());
            }

            Some("emit_inbound") => {
                gate.before_emit(entry["label"].as_str(), stop).await;

                if stop.is_cancelled() {
                    return Ok(());
                }

                if let Some(delay) = entry["afterMs"].as_u64().filter(|delay| *delay > 0) {
                    tokio::select! { _=stop.cancelled()=>return Ok(()), _=tokio::time::sleep(std::time::Duration::from_millis(delay))=>{} }
                }
                let mut frame = entry["frame"].clone();
                normalize_legacy_codex_inbound(&mut frame);
                if frame.get("method").is_none()
                    && let Some(id) = ids.get(&frame["id"].to_string())
                {
                    frame["id"] = id.clone();
                }

                tokio::select! {
                _=stop.cancelled()=>return Ok(()),result=peer.write(&frame)=>result?};
            }

            Some("expect_outbound") => {
                let actual = read_codex_replay_outbound(&mut peer, &children, stop).await?;
                if stop.is_cancelled() {
                    return Ok(());
                }

                let expected = &entry["frame"];

                let mut normalized = actual.clone();

                if expected.get("method").is_some() && expected.get("id").is_some() {
                    ids.insert(expected["id"].to_string(), actual["id"].clone());

                    normalized["id"] = expected["id"].clone();
                }

                if normalize_codex_replay_frame(expected.clone(), workspace, ignored_config)
                    != normalize_codex_replay_frame(normalized, workspace, ignored_config)
                {
                    return Err(protocol(format!(
                        "Codex replay entry {index}: expected {expected}; received {actual}"
                    )));
                }
            }

            _ => return Err(protocol("Unsupported Codex replay entry")),
        }

        *cursor.lock().unwrap_or_else(|e| e.into_inner()) += 1;

        status.send_modify(|status| status.cursor = index + 1);
    }
}

async fn read_codex_replay_outbound(
    peer: &mut WirePeer,
    children: &std::collections::BTreeSet<String>,
    stop: &CancellationToken,
) -> ProviderResult<Value> {
    loop {
        let frame = tokio::select! { _=stop.cancelled()=>return Ok(Value::Null), frame=peer.read()=>frame? };
        let params = &frame["params"];
        let metadata = match frame["method"].as_str() {
            Some("thread/read") => params["includeTurns"] == false,
            Some("thread/resume") => params["excludeTurns"] == true,
            _ => false,
        };
        if metadata
            && params["threadId"]
                .as_str()
                .is_some_and(|id| children.contains(id))
        {
            peer.respond(
                &frame,
                json!({"thread":{"id":params["threadId"]},"model":null}),
            )
            .await?;
        } else {
            return Ok(frame);
        }
    }
}

fn normalize_legacy_codex_inbound(frame: &mut Value) {
    if let Some(values) = frame.as_array_mut() {
        for value in values {
            normalize_legacy_codex_inbound(value);
        }
    }
    let Some(object) = frame.as_object_mut() else {
        return;
    };
    for value in object.values_mut() {
        normalize_legacy_codex_inbound(value);
    }
    if object.get("id").is_some_and(Value::is_string)
        && object.contains_key("modelProvider")
        && object.contains_key("status")
    {
        let id = object["id"].clone();
        object.entry("sessionId").or_insert(id);
        object.entry("projectId").or_insert(Value::Null);
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if let Some(params) = object.get_mut("params").and_then(Value::as_object_mut) {
        if method == "item/started" || method.ends_with("/requestApproval") {
            params.entry("startedAtMs").or_insert(json!(0));
        }
        if method == "item/completed" {
            params.entry("completedAtMs").or_insert(json!(0));
        }
        if method == "item/tool/requestUserInput" {
            params.entry("isBlocking").or_insert(json!(true));
        }
    }
}

fn normalize_codex_replay_frame(
    mut frame: Value,
    workspace: &str,
    ignored_config: &std::collections::BTreeSet<String>,
) -> Value {
    fn normalize(value: &mut Value, workspace: &str) {
        match value {
            Value::String(text) => {
                if !workspace.is_empty() {
                    *text = text.replace(workspace, "<workspace>");
                }

                if text.starts_with("Context handoff (")
                    && let (Some(end), Some(user)) =
                        (text.find(":\n"), text.find("\n\nUser message:\n"))
                    && end < user
                {
                    *text = format!("{}<dynamic-summary>{}", &text[..end + 2], &text[user..]);
                }
            }

            Value::Array(values) => {
                for value in values {
                    normalize(value, workspace);
                }
            }

            Value::Object(values) => {
                for value in values.values_mut() {
                    normalize(value, workspace);
                }
            }

            _ => {}
        }
    }

    normalize(&mut frame, workspace);

    if frame["method"] == "initialize" {
        frame["params"]["clientInfo"]["version"] = json!("<ignored>");
    }

    if matches!(
        frame["method"].as_str(),
        Some("thread/start" | "thread/resume" | "thread/fork")
    ) && let Some(params) = frame["params"].as_object_mut()
    {
        params.shift_remove("cwd");

        params.shift_remove("model");

        if let Some(config) = params.get_mut("config").and_then(Value::as_object_mut) {
            config.retain(|key, _| {
                key != "mcp_servers"
                    && !key.starts_with("mcp_servers.")
                    && key != "shell_environment_policy.set.T3CODE_THREAD_ID"
                    && !ignored_config.contains(key)
            });
        }
    }

    if frame["method"] == "turn/start"
        && let Some(params) = frame["params"].as_object_mut()
    {
        if params.get("approvalPolicy") == Some(&json!("never")) {
            params.shift_remove("approvalPolicy");
        }

        if params
            .get("sandboxPolicy")
            .is_some_and(|policy| policy["type"] == "dangerFullAccess")
        {
            params.shift_remove("sandboxPolicy");
        }

        if let Some(settings) = params
            .get_mut("collaborationMode")
            .and_then(|mode| mode.get_mut("settings"))
            .and_then(Value::as_object_mut)
        {
            settings.insert("developer_instructions".into(), json!("<ignored>"));
        }
    }

    frame
}

// Local composition shim: callers inject the shared scenario services from their real runtime.
pub async fn run_codex_scenario(
    name: &str,
    input: &crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput,
    transcript: &crate::testing::replay::ReplayTranscript,
    services: &crate::ports::acp_replay::testkit::orchestrator_scenario::ScenarioServices,
    ids: &crate::ports::acp_replay::testkit::deterministic_runtime::SeededIds,
) -> crate::error::Result<ScenarioResult> {
    use crate::ports::acp_replay::testkit::{fixtures::shared, orchestrator_scenario};

    let scenario = shared::materialize_fixture_input(
        &transcript.scenario.0,
        input,
        "codex",
        &shared::codex_model_selection()?,
        ids,
    )?
    .into_scenario(name)?;

    let result = orchestrator_scenario::run_scenario(scenario, services).await?;

    assert_scenario_output(name, &result, transcript)?;

    Ok(result)
}
