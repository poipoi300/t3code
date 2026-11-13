//! The composed runtime end to end: a client creates a project and a thread, sends a message,
//! watches the turn complete through the thread subscription, and a restart on the same
//! database recovers to a settled thread.
//!
//! The provider sits behind the real adapter registry: package 12's Replay adapter when the
//! `acp-adapter` feature is on, and otherwise a scripted adapter that replays one completed
//! turn on `start_turn`.
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use t3_contracts::{
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    settings::ServerSettings,
};
use t3_orchestration::{
    api::*,
    composition::{CheckpointPort, OrchestrationRuntime, RuntimePorts, StaticSettings},
    error::{Error, Result},
    ports::{
        checkpoints_forks::checkpoint_service::CheckpointMetadata,
        sessions_recovery::provider_adapter_registry::AdapterRegistry,
        transport::live_stream_budget::LiveStreamLimits,
    },
    provider::*,
    stores::StoreDatabase,
    testing::ManualClock,
    transport::service::ThreadSubscription,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-11-13T12:00:00.000Z";
const THREAD: &str = "thread:runtime";

fn sample(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples")
        .join(format!("{name}.jsonl"));
    let text = std::fs::read_to_string(path).expect("sample fixture");
    serde_json::from_str(text.lines().next().expect("sample line")).expect("sample json")
}

fn expanded<T: Clone>(id: &IdOrIndex<T>) -> ProviderResult<T> {
    match id {
        IdOrIndex::Id(id) => Ok(id.clone()),
        _ => Err(ProviderError::Unsupported("compact id in scripted adapter")),
    }
}

fn provider_error(error: serde_json::Error) -> ProviderError {
    ProviderError::Operation {
        operation: ProviderOperation::Open,
        detail: error.to_string(),
    }
}

struct ScriptedAdapter {
    instance: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
}

impl ScriptedAdapter {
    fn new() -> Self {
        Self {
            instance: ProviderInstanceId("replay".into()),
            capabilities: serde_json::from_value(
                sample("OrchestrationV2ProviderSession")["capabilities"].clone(),
            )
            .expect("capabilities"),
        }
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedAdapter {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance
    }
    fn driver(&self) -> ProviderDriverKind {
        ProviderDriverKind("replay".into())
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        Ok(self.capabilities.clone())
    }
    async fn plan_selection_transition(
        &self,
        _input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        Ok(SelectionTransitionPlan::ApplyOnNextTurn)
    }
    async fn open_session(
        &self,
        input: OpenSession,
        _cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        let mut session = sample("OrchestrationV2ProviderSession");
        session["id"] = json!(input.provider_session_id);
        session["driver"] = "replay".into();
        session["providerInstanceId"] = "replay".into();
        session["status"] = "ready".into();
        session["model"] = json!(input.model_selection.model);
        session["capabilities"] =
            serde_json::to_value(&self.capabilities).map_err(provider_error)?;
        Ok(Arc::new(ScriptedSession {
            session: serde_json::from_value(session).map_err(provider_error)?,
            subscribers: Mutex::new(Vec::new()),
        }))
    }
}

struct ScriptedSession {
    session: OrchestrationV2ProviderSession,
    subscribers: Mutex<Vec<mpsc::Sender<ProviderResult<ProviderEvent>>>>,
}

impl ScriptedSession {
    fn event(kind: ProviderEventKind) -> ProviderEvent {
        ProviderEvent {
            driver: ProviderDriverKind("replay".into()),
            kind,
        }
    }

    /// One completed turn: started, an assistant reply, completed, terminal.
    fn turn_events(input: &TurnInput) -> ProviderResult<Vec<ProviderEvent>> {
        let provider_thread_id = expanded(&input.provider_thread.id)?;
        let provider_turn_id = ProviderTurnId(format!("provider-turn:{}", input.run_id.0));
        let turn = |status: &str, completed: Option<&str>| -> ProviderResult<ProviderEvent> {
            let turn = json!({"id":provider_turn_id,"providerThreadId":provider_thread_id,"nodeId":input.root_node_id,"runAttemptId":input.attempt_id,"nativeTurnRef":null,"ordinal":input.provider_turn_ordinal,"status":status,"startedAt":NOW,"completedAt":completed});
            Ok(Self::event(ProviderEventKind::Turn {
                provider_turn: Box::new(serde_json::from_value(turn).map_err(provider_error)?),
                thread_id: None,
            }))
        };
        let reply = json!({"createdBy":"agent","creationSource":"server","id":format!("message:reply:{}", input.run_id.0),"threadId":input.thread_id,"runId":input.run_id,"nodeId":input.root_node_id,"role":"assistant","text":"Replayed reply","attachments":[],"streaming":false,"createdAt":NOW,"updatedAt":NOW});
        Ok(vec![
            turn("running", None)?,
            Self::event(ProviderEventKind::Message(Box::new(
                serde_json::from_value(reply).map_err(provider_error)?,
            ))),
            turn("completed", Some(NOW))?,
            Self::event(ProviderEventKind::Terminal {
                provider_thread_id,
                provider_turn_id: provider_turn_id.clone(),
                run_ordinal: input.run_ordinal,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            }),
        ])
    }
}

#[async_trait]
impl SessionRuntime for ScriptedSession {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        true
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        let (sender, events) = mpsc::channel(64);
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(sender);
        Ok(EventSubscription {
            events,
            cancellation: CancellationToken::new(),
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        Ok(())
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let mut thread = match input.existing_provider_thread {
            Some(thread) => serde_json::to_value(thread).map_err(provider_error)?,
            None => {
                let mut thread = sample("OrchestrationV2ProviderThread");
                thread["id"] = format!("provider-thread:{}", input.thread_id.0).into();
                thread["ownerNodeId"] = Value::Null;
                thread["lastRunOrdinal"] = Value::Null;
                thread["createdAt"] = NOW.into();
                thread["updatedAt"] = NOW.into();
                thread
            }
        };
        thread["driver"] = "replay".into();
        thread["providerInstanceId"] = "replay".into();
        thread["providerSessionId"] = json!(self.session.id);
        thread["appThreadId"] = json!(input.thread_id);
        thread["nativeThreadRef"] = json!({"driver":"replay","nativeId":format!("native:{}", input.thread_id.0),"strength":"strong"});
        serde_json::from_value(thread).map_err(provider_error)
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        Ok(input.provider_thread)
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        let events = Self::turn_events(&input)?;
        let subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        tokio::spawn(async move {
            for event in events {
                for subscriber in &subscribers {
                    let _ = subscriber.send(Ok(event.clone())).await;
                }
            }
        });
        Ok(())
    }
    async fn steer_turn(&self, _input: SteerInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("steer_turn"))
    }
    async fn interrupt_turn(&self, _input: InterruptInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("interrupt_turn"))
    }
    async fn respond(&self, _input: RuntimeResponse) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("respond"))
    }
    async fn snapshot(
        &self,
        _thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("snapshot"))
    }
    async fn rollback(&self, _input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("rollback"))
    }
    async fn fork(&self, _input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        Err(ProviderError::Unsupported("fork"))
    }
}

/// A workspace that is not a Git repository: capture is skipped, as for a plain folder.
struct NoRepository;

#[async_trait]
impl CheckpointBackend for NoRepository {
    async fn capture(
        &self,
        _input: CaptureCheckpoint,
        _cancellation: CancellationToken,
    ) -> Result<CheckpointRef> {
        Err(Error::Unsupported("capture outside a repository"))
    }
    async fn lookup(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _ordinal: u64,
    ) -> Result<Option<CheckpointRef>> {
        Ok(None)
    }
    async fn diff(
        &self,
        _cwd: PathBuf,
        _from: CheckpointRef,
        _to: CheckpointRef,
    ) -> Result<CheckpointDiff> {
        Err(Error::Unsupported("diff outside a repository"))
    }
    async fn restore(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _ordinal: u64,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(Error::Unsupported("restore outside a repository"))
    }
    async fn prune(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _preserve: Vec<u64>,
    ) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl CheckpointMetadata for NoRepository {
    fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
        CheckpointRef(format!("refs/t3/test/{}/{ordinal}", scope.0))
    }
    async fn is_repository(&self, _cwd: PathBuf) -> Result<bool> {
        Ok(false)
    }
    async fn delete_refs(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        Ok(())
    }
}

/// The provider behind the real adapter registry.
#[derive(Clone, Copy)]
enum Provider {
    /// The scripted adapter above, for builds without the ACP adapter.
    Scripted,
    /// Package 12's Replay adapter (`devReplay`, the dev server's free provider).
    #[cfg(feature = "acp-adapter")]
    Replay,
}

impl Provider {
    fn instance(self) -> &'static str {
        match self {
            Self::Scripted => "replay",
            #[cfg(feature = "acp-adapter")]
            Self::Replay => t3_orchestration::adapters::acp::DEV_REPLAY_INSTANCE_ID,
        }
    }
    fn driver(self) -> &'static str {
        match self {
            Self::Scripted => "replay",
            #[cfg(feature = "acp-adapter")]
            Self::Replay => "acpRegistry",
        }
    }
    /// `multi-turn` replays an approval-free conversation.
    fn model(self) -> &'static str {
        match self {
            Self::Scripted => "replay-model",
            #[cfg(feature = "acp-adapter")]
            Self::Replay => "multi-turn",
        }
    }
    fn registry(
        self,
        // Only the Replay registration takes the clock.
        #[cfg_attr(not(feature = "acp-adapter"), allow(unused_variables))] clock: Arc<dyn Clock>,
    ) -> AdapterRegistry {
        match self {
            Self::Scripted => {
                AdapterRegistry::from_adapters(vec![Arc::new(ScriptedAdapter::new())])
            }
            #[cfg(feature = "acp-adapter")]
            Self::Replay => AdapterRegistry::new(vec![
                t3_orchestration::adapters::acp::dev_replay_registration(
                    clock,
                    Arc::new(t3_orchestration::composition::RandomIds),
                    false,
                ),
            ]),
        }
    }
}

async fn start(
    database: &Path,
    provider: Provider,
    checkpoints: Arc<dyn CheckpointPort>,
) -> Result<OrchestrationRuntime> {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(NOW));
    let mut ports = RuntimePorts::new(
        StoreDatabase(t3_persistence::Database::open(database)?),
        Arc::new(provider.registry(clock.clone())),
        checkpoints,
        clock,
    );
    // Like the dev server's Replay provider: an instance only exists once it is configured.
    let mut settings = serde_json::to_value(ServerSettings::default())?;
    settings["providerInstances"][provider.instance()] =
        json!({"driver":provider.driver(),"enabled":true});
    ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
    OrchestrationRuntime::start(ports).await
}

fn command(value: Value) -> Result<OrchestrationV2ServerCommand> {
    Ok(serde_json::from_value(value)?)
}

const SETTLED: [&str; 5] = [
    "completed",
    "interrupted",
    "failed",
    "cancelled",
    "rolled_back",
];

/// The project's workspace folder and the checkpoint store over it.
struct Workspace {
    path: PathBuf,
    checkpoints: Arc<dyn CheckpointPort>,
    #[cfg(feature = "git-backend")]
    git: Option<t3_git::Git>,
}

impl Workspace {
    /// A plain folder: capture is skipped, as for any non-repository workspace.
    fn plain(root: &Path) -> Result<Self> {
        let path = root.join("workspace");
        std::fs::create_dir(&path)?;
        Ok(Self {
            path,
            checkpoints: Arc::new(NoRepository),
            #[cfg(feature = "git-backend")]
            git: None,
        })
    }

    /// A Git repository isolated from the developer's Git configuration, with package 07's
    /// real checkpoint backend over it.
    #[cfg(feature = "git-backend")]
    async fn repository(root: &Path) -> Result<Self> {
        use std::ffi::OsString;
        let path = root.join("workspace");
        let home = root.join("home");
        std::fs::create_dir(&path)?;
        std::fs::create_dir(&home)?;
        let mut environment = std::collections::BTreeMap::<OsString, Option<OsString>>::new();
        for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
            environment.insert(key.into(), Some(home.clone().into_os_string()));
        }
        for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
            environment.insert(key.into(), Some(home.join("no-config").into_os_string()));
        }
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_COUNT", "0"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
        ] {
            environment.insert(key.into(), Some(value.into()));
        }
        for key in ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG", "GIT_TEMPLATE_DIR"] {
            environment.insert(key.into(), None);
        }
        let factory_environment = environment.clone();
        let backend =
            t3_orchestration::runtime::checkpoints::GitCheckpointBackend::new(move |cwd| {
                t3_git::Git::new(cwd).with_environment(factory_environment.clone())
            });
        let git = t3_git::Git::new(path.clone()).with_environment(environment);
        let workspace = Self {
            path,
            checkpoints: Arc::new(backend),
            git: Some(git),
        };
        workspace.git(&["init", "--initial-branch=main"]).await?;
        workspace.git(&["config", "core.autocrlf", "false"]).await?;
        std::fs::write(workspace.path.join("README.md"), "workspace\n")?;
        workspace.git(&["add", "README.md"]).await?;
        workspace.git(&["commit", "-m", "initial"]).await?;
        Ok(workspace)
    }

    #[cfg(feature = "git-backend")]
    async fn git(&self, args: &[&str]) -> Result<String> {
        let git = self
            .git
            .as_ref()
            .ok_or(Error::Unsupported("plain workspace"))?;
        Ok(git
            .execute(args, Default::default())
            .await
            .map_err(|error| Error::Service {
                operation: "test git",
                detail: format!("{error:?}"),
            })?
            .stdout_text())
    }
}

/// Reads the thread stream until a run settles; checks the turn's visible milestones.
async fn await_completed_turn(
    subscription: &mut ThreadSubscription,
    provider: Provider,
) -> Result<RunId> {
    let mut statuses = Vec::new();
    let mut reply = None;
    let run_id = loop {
        let batch = subscription.pull().await?.expect("thread stream ended");
        let mut finished = None;
        for item in batch {
            let item = serde_json::to_value(&item.value)?;
            let event = &item["event"];
            match event["type"].as_str() {
                Some("run.updated") | Some("run.created") => {
                    let status = event["payload"]["status"].as_str().unwrap_or_default();
                    statuses.push(status.to_owned());
                    if SETTLED.contains(&status) {
                        finished = Some(event["payload"]["id"].clone());
                    }
                }
                Some("message.updated") if event["payload"]["role"] == "assistant" => {
                    reply = event["payload"]["text"].as_str().map(str::to_owned);
                }
                _ => {}
            }
        }
        if let Some(run) = finished {
            break serde_json::from_value::<RunId>(run)?;
        }
    };
    match provider {
        Provider::Scripted => assert_eq!(reply.as_deref(), Some("Replayed reply")),
        #[cfg(feature = "acp-adapter")]
        Provider::Replay => assert!(reply.is_some_and(|text| !text.is_empty())),
    }
    assert!(
        statuses.iter().any(|status| status == "running"),
        "{statuses:?}"
    );
    assert_eq!(
        statuses.last().map(String::as_str),
        Some("completed"),
        "{statuses:?}"
    );
    Ok(run_id)
}

/// Creates a project and thread, sends a message, watches the turn complete through the
/// thread subscription, then restarts on the same database: recovery finds nothing to repair
/// and every run is settled.
async fn client_turn_and_restart(
    provider: Provider,
    directory: &Path,
    workspace: &Workspace,
) -> Result<OrchestrationV2ThreadProjection> {
    let database = directory.join("statev2.sqlite");
    let thread_id = ThreadId(THREAD.into());

    let runtime = start(&database, provider, workspace.checkpoints.clone()).await?;
    runtime
        .project_command(serde_json::from_value(json!({"type":"project.create","commandId":"command:project","projectId":"project:runtime","title":"Runtime","workspaceRoot":workspace.path}))?)
        .await?;
    runtime
        .dispatch_command(command(json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":"project:runtime","title":"Runtime thread","modelSelection":{"instanceId":provider.instance(),"model":provider.model()},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}))?)
        .await?;
    let mut subscription = runtime
        .transport
        .subscribe_thread(
            serde_json::from_value(json!({"threadId":THREAD}))?,
            LiveStreamLimits::default(),
        )
        .await?;
    runtime
        .dispatch_command(command(json!({"type":"message.dispatch","commandId":"command:message","threadId":THREAD,"messageId":"message:runtime","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}))?)
        .await?;
    let run_id = await_completed_turn(&mut subscription, provider).await?;
    subscription.close().await?;
    runtime.shutdown().await?;
    drop(runtime);

    let restarted = start(&database, provider, workspace.checkpoints.clone()).await?;
    assert_eq!(restarted.recovery(), &RecoverySummary::default());
    let run = restarted
        .projections
        .run(thread_id.clone(), run_id)
        .await?
        .expect("run survives restart");
    assert_eq!(run.status, OrchestrationV2RunStatus::Completed);
    let projection = restarted
        .projections
        .projection(thread_id)
        .await?
        .expect("thread survives restart");
    for run in &projection.runs {
        let status = serde_json::to_value(&run.status)?;
        assert!(
            SETTLED.contains(&status.as_str().unwrap_or_default()),
            "{status}"
        );
    }
    restarted.shutdown().await?;
    Ok(projection)
}

#[tokio::test]
async fn a_message_runs_a_turn_and_a_restart_recovers_a_settled_thread() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    client_turn_and_restart(Provider::Scripted, directory.path(), &workspace).await?;
    Ok(())
}

/// The same turn in a Git repository: finalization captures a real checkpoint ref.
#[cfg(feature = "git-backend")]
#[tokio::test]
async fn a_turn_in_a_git_repository_captures_its_checkpoint() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::repository(directory.path()).await?;
    let projection =
        client_turn_and_restart(Provider::Scripted, directory.path(), &workspace).await?;
    let captured: Vec<_> = projection
        .checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.status == CheckpointStatus::Ready)
        .collect();
    assert!(!captured.is_empty(), "{:?}", projection.checkpoints);
    for checkpoint in captured {
        workspace
            .git(&[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", checkpoint.r#ref.0),
            ])
            .await?;
    }
    Ok(())
}

#[cfg(feature = "acp-adapter")]
#[tokio::test]
// Package 12: ACP `setup_thread` rejects the decision's not-loaded provider thread (no native
// ref) instead of opening a new native session; the start effect then retries, so this hangs.
#[ignore = "package 12: ACP ensure_thread rejects a not-loaded provider thread"]
async fn a_message_runs_a_replay_turn_and_a_restart_recovers_a_settled_thread() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    client_turn_and_restart(Provider::Replay, directory.path(), &workspace).await?;
    Ok(())
}
