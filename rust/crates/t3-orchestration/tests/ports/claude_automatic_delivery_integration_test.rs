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
