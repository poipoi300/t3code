#![cfg(feature = "claude-adapter")]

use super::{provider_event_ingestor_test::Harness, provider_turn_start_service_test::seed};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use t3_claude_cli::{Session, SessionOptions, launch::LaunchOptions};
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_orchestration::{
    adapters::claude::*,
    api::*,
    error::{Error, Result},
    ports::{
        claude::adapters::claude_adapter_v2_testkit::{PipeConnector, fixture_turn_input},
        runs_ingestion::{
            provider_event_ingestor::IngestInput,
            provider_turn_control_service::ProviderTurnControlService,
        },
    },
    provider::*,
    testing::SequenceIds,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

async fn native_frame(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    frame: Value,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(&frame)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    Ok(())
}

async fn next_native_user(
    lines: &mut tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    writer: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
) -> Result<Value> {
    loop {
        let frame: Value =
            serde_json::from_str(&lines.next_line().await?.expect("native transport ended"))?;
        if frame["type"] == "user" {
            return Ok(frame);
        }
        assert_eq!(
            frame["type"], "control_request",
            "unexpected native frame {frame}"
        );
        assert!(
            matches!(
                frame["request"]["subtype"].as_str(),
                Some("initialize" | "set_model" | "apply_flag_settings" | "set_permission_mode")
            ),
            "automatic delivery must not interrupt: {frame}"
        );
        native_frame(writer,json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{}}})).await?;
    }
}

async fn committed_event(
    events: &mut tokio::sync::broadcast::Receiver<ApplicationStoredEvent>,
    predicate: impl Fn(&Value) -> bool,
) -> Result<Value> {
    loop {
        let stored = events.recv().await.map_err(|error| Error::Service {
            operation: "automatic delivery event stream",
            detail: error.to_string(),
        })?;
        if let ApplicationStoredEvent::Orchestration(stored) = stored {
            let event = t3_contracts::stored::to_value(&stored.event)?;
            if predicate(&event) {
                return Ok(event);
            }
        }
    }
}

#[tokio::test]
async fn explicit_and_promoted_steering_reach_the_native_query_at_now_priority_and_persist_tool_cancellation()
-> Result<()> {
    use t3_orchestration::{
        composition::{OrchestrationRuntime, RuntimePorts, StaticSettings},
        ports::{
            runs_ingestion::provider_turn_start_service_testkit::NoRepositoryCheckpoints,
            sessions_recovery::provider_adapter_registry::AdapterRegistry,
        },
        testing::{ManualClock, TEST_TIME, TestDatabase},
    };
    let refusal = "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.";
    let mut observed = vec![];
    for promoted in [false, true] {
        let workspace = tempfile::tempdir()?;
        let database = TestDatabase::memory()?;
        let clock = Arc::new(ManualClock::new(TEST_TIME));
        let ids = Arc::new(SequenceIds::new("automatic"));
        let (client, peer) = tokio::io::duplex(65_536);
        let (read, write) = tokio::io::split(client);
        let (read_peer, mut write_peer) = tokio::io::split(peer);
        let mut lines = BufReader::new(read_peer).lines();
        let native = "00000000-0000-4000-8000-000000000001";
        let registration = claude_registration(
            ClaudeAdapterOptions {
                instance_id: ProviderInstanceId("claudeAgent".into()),
                launch: LaunchOptions {
                    cli_path: Some("fixture-claude".into()),
                    ..Default::default()
                },
                connector: Arc::new(PipeConnector::new(Session::connect(
                    read,
                    write,
                    SessionOptions::default(),
                ))),
                clock: clock.clone(),
                ids: ids.clone(),
                new_native_session_id: Arc::new(move || native.into()),
                mcp_servers: BTreeMap::new(),
                transcript_root: None,
                attachments: Arc::new(DirectoryClaudeAttachments::default()),
                host: ClaudeHostOptions::default(),
            },
            "claude:auto-delivery".into(),
            true,
            vec![],
        );
        let mut ports = RuntimePorts::new(
            database.stores.clone(),
            Arc::new(AdapterRegistry::new(vec![registration])),
            Arc::new(NoRepositoryCheckpoints),
            clock,
        );
        ports.ids = ids;
        let mut settings = serde_json::to_value(t3_contracts::settings::ServerSettings::default())?;
        settings["providerInstances"]["claudeAgent"] =
            json!({"driver":"claudeAgent","enabled":true});
        ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
        let runtime = OrchestrationRuntime::start(ports).await?;
        runtime.project_command(serde_json::from_value(json!({"type":"project.create","commandId":"project:create","projectId":"project:auto","title":"Automatic delivery","workspaceRoot":workspace.path()}))?).await?;
        runtime.dispatch_command(serde_json::from_value(json!({"type":"thread.create","commandId":"thread:create","threadId":"thread:auto","projectId":"project:auto","title":"Automatic delivery","modelSelection":{"instanceId":"claudeAgent","model":"claude-sonnet-4-6"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":workspace.path(),"createdBy":"user","creationSource":"web"}))?).await?;
        let mut events = EventCommitter::subscribe(&runtime.events);
        runtime.dispatch_command(serde_json::from_value(json!({"type":"message.dispatch","commandId":"message:first","threadId":"thread:auto","messageId":"message:first","text":"Build and check all four tasks.","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}))?).await?;
        let first = next_native_user(&mut lines, &mut write_peer).await?;
        assert!(first["priority"].is_null());
        let running = committed_event(&mut events, |event| {
            event["type"] == "provider-turn.updated" && event["payload"]["status"] == "running"
        })
        .await?;
        let attempt = running["payload"]["runAttemptId"].clone();
        let projection = runtime
            .projections
            .projection(ThreadId("thread:auto".into()))
            .await?
            .unwrap();
        let run = projection
            .runs
            .iter()
            .find(|run| t3_contracts::stored::to_value(&run.active_attempt_id).unwrap() == attempt)
            .unwrap();
        let run_id = t3_contracts::stored::to_value(&run.id)?;
        let mut tools =
            vec![json!({"type":"tool_use","id":"build","name":"Bash","input":{"command":"make"}})];
        tools.extend((1..=4).map(|index|json!({"type":"tool_use","id":format!("status-{index}"),"name":"mcp__t3-code__task_status","input":{"taskId":format!("status-{index}")}})));
        native_frame(&mut write_peer,json!({"type":"assistant","uuid":"batch","session_id":native,"parent_tool_use_id":null,
            "message":{"id":"tool-batch","role":"assistant","type":"message","model":"claude-sonnet-4-6","content":tools,
                "stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}})).await?;
        committed_event(&mut events, |event| {
            event["type"] == "turn-item.updated"
                && event["payload"]["nativeItemRef"]["nativeId"] == "status-4"
        })
        .await?;
        let dispatch_mode = if promoted {
            json!({"type":"queue_after_active"})
        } else {
            json!({"type":"steer_active","targetRunId":run_id})
        };
        runtime.dispatch_command(serde_json::from_value(json!({"type":"message.dispatch","commandId":"message:notice","threadId":"thread:auto","messageId":"message:notice","text":"Change direction now.","attachments":[],"dispatchMode":dispatch_mode,"createdBy":"user","creationSource":"web"}))?).await?;
        if promoted {
            let projection = runtime
                .projections
                .projection(ThreadId("thread:auto".into()))
                .await?
                .unwrap();
            let queued = projection
                .runs
                .iter()
                .find(|run| run.status == OrchestrationV2RunStatus::Queued)
                .unwrap();
            runtime.dispatch_command(serde_json::from_value(json!({"type":"queued-message.promote-to-steer","commandId":"queue:promote","threadId":"thread:auto","queuedRunId":queued.id,"targetRunId":run_id}))?).await?;
        }
        let steer = next_native_user(&mut lines, &mut write_peer).await?;
        assert_eq!(steer["priority"], "now");
        for id in ["build", "status-1", "status-2", "status-3", "status-4"] {
            let cancelled = id != "build";
            let mut result = json!({"type":"user","uuid":format!("result-{id}"),"session_id":native,"parent_tool_use_id":null,
                "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"is_error":cancelled,"content":if cancelled {refusal} else {"build OK"}}]}});
            if cancelled {
                result["tool_result_meta"] = json!([{"id":id,"non_execution_kind":"cancelled"}]);
            }
            native_frame(&mut write_peer, result).await?;
        }
        for aborted in [true, false] {
            native_frame(&mut write_peer,json!({"type":"result","subtype":"success","uuid":if aborted {"aborted"} else {"completed"},"session_id":native,
                "is_error":false,"num_turns":1,"result":"","stop_reason":"end_turn","terminal_reason":if aborted {"aborted_tools"} else {"completed"},
                "permission_denials":[],"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1},"modelUsage":{}})).await?;
        }
        committed_event(&mut events, |event| {
            event["type"] == "run.updated"
                && event["payload"]["id"] == run_id
                && event["payload"]["status"] == "completed"
        })
        .await?;
        let projection = runtime
            .projections
            .projection(ThreadId("thread:auto".into()))
            .await?
            .unwrap();
        let reads: Vec<_> = projection
            .turn_items
            .iter()
            .map(t3_contracts::stored::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|item| {
                item["nativeItemRef"]["nativeId"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("status-"))
            })
            .collect();
        assert_eq!(reads.len(), 4);
        for read in reads {
            observed.push((promoted, read));
        }
        if promoted {
            assert!(
                projection
                    .runs
                    .iter()
                    .any(|run| run.status == OrchestrationV2RunStatus::Cancelled)
            );
            assert!(
                projection
                    .turn_items
                    .iter()
                    .map(t3_contracts::stored::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .iter()
                    .any(|item| item["type"] == "user_message"
                        && item["inputIntent"] == "promoted_queued_to_steer")
            );
        }
        runtime.shutdown().await?;
    }
    assert_eq!(observed.len(), 8);
    for (promoted, read) in observed {
        assert_eq!(
            read["status"], "cancelled",
            "promoted={promoted}, metadata={}",
            read["toolNonExecutionKind"]
        );
        assert_eq!(read["toolNonExecutionKind"], "cancelled");
        assert_eq!(read["output"], refusal);
    }
    Ok(())
}

struct LiveSession(Arc<dyn SessionRuntime>);
#[async_trait]
impl SessionDirectory for LiveSession {
    async fn get(&self, id: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>> {
        Ok(
            (self.0.session().id == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id))
                .then(|| self.0.clone()),
        )
    }
    async fn attach(
        &self,
        _instance: ProviderInstanceId,
        _input: OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>> {
        Err(Error::Unsupported("test session already open"))
    }
    async fn detach(&self, _id: ProviderSessionId, _revoke: bool) -> Result<()> {
        Ok(self.0.close().await?)
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(self.0.close().await?)
    }
}

#[tokio::test]
async fn live_effort_change_reaches_the_running_claude_query_without_interrupting_or_starting_a_turn()
-> Result<()> {
    let h = Harness::new().await?;
    let mut state = seed(&h).await?;
    let high = json!({"instanceId":"claudeAgent","model":"claude-sonnet-4-6","options":[{"id":"effort","value":"high"}]});
    let mut provider = t3_contracts::stored::to_value(&state.provider_thread)?;
    provider["driver"] = "claudeAgent".into();
    provider["providerInstanceId"] = "claudeAgent".into();
    provider["nativeThreadRef"] = json!({"driver":"claudeAgent","nativeId":"00000000-0000-4000-8000-000000000001","strength":"strong"});
    state.provider_thread = serde_json::from_value(provider.clone())?;
    h.write("provider-thread.updated", provider).await?;
    state.run.status = OrchestrationV2RunStatus::Running;
    state.run.model_selection = serde_json::from_value(high.clone())?;
    h.write("run.updated", t3_contracts::stored::to_value(&state.run)?)
        .await?;
    let (client, peer) = tokio::io::duplex(65536);
    let (read, write) = tokio::io::split(client);
    let (read_peer, mut write_peer) = tokio::io::split(peer);
    let mut lines = BufReader::new(read_peer).lines();
    let adapter = ClaudeAdapterV2::new(ClaudeAdapterOptions {
        instance_id: ProviderInstanceId("claudeAgent".into()),
        launch: LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        connector: Arc::new(PipeConnector::new(Session::connect(
            read,
            write,
            SessionOptions::default(),
        ))),
        clock: h.clock.clone(),
        ids: Arc::new(SequenceIds::new("claude-delivery")),
        new_native_session_id: Arc::new(|| "00000000-0000-4000-8000-000000000001".into()),
        mcp_servers: BTreeMap::new(),
        transcript_root: None,
        attachments: Arc::new(DirectoryClaudeAttachments::default()),
        host: ClaudeHostOptions::default(),
    });
    let mut input = fixture_turn_input()?;
    input.thread_id = ThreadId("thread:test".into());
    input.run_id = RunId("run:test".into());
    input.attempt_id = RunAttemptId("attempt:test".into());
    input.root_node_id = NodeId("node:test".into());
    input.provider_thread = state.provider_thread.clone();
    input.model_selection = serde_json::from_value(high)?;
    let runtime = adapter
        .open_session(
            OpenSession {
                thread_id: input.thread_id.clone(),
                provider_session_id: ProviderSessionId("session:test".into()),
                model_selection: input.model_selection.clone(),
                runtime_policy: input.runtime_policy.clone(),
                prompt_suggestions: None,
                resume_from_session: None,
                initial_native_thread_id: None,
                initial_provider_item_identity_version: None,
            },
            CancellationToken::new(),
        )
        .await?;
    let mut events = runtime.subscribe().await?;
    let start_runtime = runtime.clone();
    let start = tokio::spawn(async move { start_runtime.start_turn(input).await });
    let init: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    assert_eq!(init["request"]["subtype"], "initialize");
    let mut response = serde_json::to_vec(
        &json!({"type":"control_response","response":{"subtype":"success","request_id":init["request_id"],"response":{}}}),
    )?;
    response.push(b'\n');
    write_peer.write_all(&response).await?;
    let user: Value = serde_json::from_str(&lines.next_line().await?.unwrap())?;
    assert_eq!(user["type"], "user");
    start.await.unwrap()?;
    loop {
        let event = events.events.recv().await.unwrap()?;
        let running = matches!(&event.kind,ProviderEventKind::Turn{provider_turn,..} if provider_turn.status==ProviderTurnStatus::Running);
        h.ingestor
            .ingest_normalized(
                &IngestInput {
                    session_id: ProviderSessionId("session:test".into()),
                    instance_id: ProviderInstanceId("claudeAgent".into()),
                    thread_id: ThreadId("thread:test".into()),
                    run_id: Some(RunId("run:test".into())),
                    node_id: Some(NodeId("node:test".into())),
                    raw_event_id: None,
                    event,
                },
                CommitGuard::None,
            )
            .await?;
        if running {
            break;
        }
    }
    let control = ProviderTurnControlService {
        projections: h.reader.clone(),
        sessions: Arc::new(LiveSession(runtime.clone())),
        events: h.sink.clone(),
    };
    let mut applying = tokio::spawn(async move {
        control.apply_selection(ApplyModelSelection{thread_id:ThreadId("thread:test".into()),provider_session_id:ProviderSessionId("session:test".into()),provider_thread_id:ProviderThreadId("pt:test".into()),model_selection:serde_json::from_value(json!({"instanceId":"claudeAgent","model":"claude-sonnet-4-6","options":[{"id":"effort","value":"medium"}]}))?},CancellationToken::new()).await
    });
    let flags = tokio::select! {
        result=&mut applying=>{runtime.close().await?;panic!("live effort returned before applying flags: {result:?}")},
        frame=lines.next_line()=>serde_json::from_str::<Value>(&frame?.unwrap())?,
    };
    assert_eq!(flags["request"]["subtype"], "apply_flag_settings");
    assert_eq!(
        flags["request"]["settings"],
        json!({"effortLevel":"medium","fastMode":null,"ultracode":null})
    );
    let mut response = serde_json::to_vec(
        &json!({"type":"control_response","response":{"subtype":"success","request_id":flags["request_id"],"response":{}}}),
    )?;
    response.push(b'\n');
    write_peer.write_all(&response).await?;
    applying.await.unwrap()?;
    assert_eq!(
        h.reader
            .run(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .unwrap()
            .status,
        OrchestrationV2RunStatus::Running
    );
    runtime.close().await?;
    Ok(())
}
