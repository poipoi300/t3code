//! The composed runtime end to end on package 12's Replay provider: a client creates a project
//! and a thread, sends a message, watches the turn complete through the thread subscription, and
//! a restart on the same database recovers to a settled thread. The managed ceiling and a
//! provider's offered modes are checked on the same runtime. Requires `acp-adapter`.
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{
    managed_policy::ManagedPolicyDocument,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::RuntimeMode,
    settings::ServerSettings,
};
use t3_orchestration::{
    adapters::acp::{DEV_REPLAY_INSTANCE_ID, dev_replay_registration},
    api::*,
    composition::{CheckpointPort, OrchestrationRuntime, RandomIds, RuntimePorts, StaticSettings},
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
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-11-13T12:00:00.000Z";
const THREAD: &str = "thread:runtime";
/// The Replay scenario that answers without approvals.
const MODEL: &str = "multi-turn";

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

async fn start(
    database: &Path,
    checkpoints: Arc<dyn CheckpointPort>,
) -> Result<OrchestrationRuntime> {
    start_with(database, checkpoints, NOW, |adapter| adapter, |_| {}).await
}

/// `start` at `now`, with the Replay adapter wrapped and the ports adjusted.
async fn start_with(
    database: &Path,
    checkpoints: Arc<dyn CheckpointPort>,
    now: &str,
    wrap: impl FnOnce(Arc<dyn ProviderAdapter>) -> Arc<dyn ProviderAdapter>,
    configure: impl FnOnce(&mut RuntimePorts),
) -> Result<OrchestrationRuntime> {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(now));
    let mut replay = dev_replay_registration(clock.clone(), Arc::new(RandomIds), false);
    replay.adapter = wrap(replay.adapter);
    let registry = AdapterRegistry::new(vec![replay]);
    let mut ports = RuntimePorts::new(
        StoreDatabase(t3_persistence::Database::open(database)?),
        Arc::new(registry),
        checkpoints,
        clock,
    );
    // As the dev server configures Replay: an instance only exists once it is in settings.
    let mut settings = serde_json::to_value(ServerSettings::default())?;
    settings["providerInstances"][DEV_REPLAY_INSTANCE_ID] =
        json!({"driver":"acpRegistry","displayName":"Replay (dev)","enabled":true});
    ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
    configure(&mut ports);
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
        Self::repository_with(root, None).await
    }

    /// `repository`, with the backend keeping each scope's binding in `bindings`.
    #[cfg(feature = "git-backend")]
    async fn repository_with(
        root: &Path,
        bindings: Option<Arc<dyn RepositoryBindingStore>>,
    ) -> Result<Self> {
        let path = root.join("workspace");
        let home = root.join("home");
        std::fs::create_dir(&path)?;
        std::fs::create_dir(&home)?;
        let environment = git_environment(&home);
        let git = t3_git::Git::new(path.clone()).with_environment(environment);
        let workspace = Self {
            path,
            checkpoints: git_checkpoints(&home, bindings),
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

/// Git configuration isolated under `home`, away from the developer's own.
#[cfg(feature = "git-backend")]
fn git_environment(
    home: &Path,
) -> std::collections::BTreeMap<std::ffi::OsString, Option<std::ffi::OsString>> {
    let mut environment = std::collections::BTreeMap::new();
    for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
        environment.insert(key.into(), Some(home.as_os_str().to_owned()));
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
    environment
}

/// Package 07's real checkpoint backend with `home`'s configuration, keeping each scope's
/// binding in `bindings`.
#[cfg(feature = "git-backend")]
fn git_checkpoints(
    home: &Path,
    bindings: Option<Arc<dyn RepositoryBindingStore>>,
) -> Arc<dyn CheckpointPort> {
    let environment = git_environment(home);
    let mut backend =
        t3_orchestration::runtime::checkpoints::GitCheckpointBackend::new(move |cwd| {
            t3_git::Git::new(cwd).with_environment(environment.clone())
        });
    if let Some(bindings) = bindings {
        backend = backend.with_repository_bindings(bindings);
    }
    Arc::new(backend)
}

/// Reads the thread stream until a run settles; checks the turn's visible milestones.
async fn await_completed_turn(subscription: &mut ThreadSubscription) -> Result<RunId> {
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
    assert!(reply.is_some_and(|text| !text.is_empty()));
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
    directory: &Path,
    workspace: &Workspace,
) -> Result<OrchestrationV2ThreadProjection> {
    let database = directory.join("statev2.sqlite");
    let thread_id = ThreadId(THREAD.into());

    let runtime = start(&database, workspace.checkpoints.clone()).await?;
    runtime
        .project_command(serde_json::from_value(json!({"type":"project.create","commandId":"command:project","projectId":"project:runtime","title":"Runtime","workspaceRoot":workspace.path}))?)
        .await?;
    runtime
        .dispatch_command(command(json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":"project:runtime","title":"Runtime thread","modelSelection":{"instanceId":DEV_REPLAY_INSTANCE_ID,"model":MODEL},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}))?)
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
    let run_id = await_completed_turn(&mut subscription).await?;
    subscription.close().await?;
    runtime.shutdown().await?;
    drop(runtime);

    let restarted = start(&database, workspace.checkpoints.clone()).await?;
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
    client_turn_and_restart(directory.path(), &workspace).await?;
    Ok(())
}

/// The same turn in a Git repository: finalization captures a real checkpoint ref.
#[cfg(feature = "git-backend")]
#[tokio::test]
async fn a_turn_in_a_git_repository_captures_its_checkpoint() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::repository(directory.path()).await?;
    let projection = client_turn_and_restart(directory.path(), &workspace).await?;
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

const CEILING_REFUSAL: &str = "Acme limits threads to Auto-accept edits.";

fn managed_ceiling() -> Result<ManagedPolicyDocument> {
    Ok(serde_json::from_value(json!({
        "schemaVersion": 1,
        "organization": "Acme",
        "limits": {"maxRuntimeMode": "auto-accept-edits"},
    }))?)
}

async fn create_project(runtime: &OrchestrationRuntime, workspace: &Workspace) -> Result<()> {
    runtime
        .project_command(serde_json::from_value(json!({"type":"project.create","commandId":"command:project","projectId":"project:runtime","title":"Runtime","workspaceRoot":workspace.path}))?)
        .await?;
    Ok(())
}

fn create_thread(id: &str, mode: &str, model: &str) -> Result<OrchestrationV2ServerCommand> {
    command(
        json!({"type":"thread.create","commandId":format!("command:{id}"),"threadId":id,"projectId":"project:runtime","title":"Runtime thread","modelSelection":{"instanceId":DEV_REPLAY_INSTANCE_ID,"model":model},"runtimeMode":mode,"interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
    )
}

fn send(thread: &str, id: &str) -> Result<OrchestrationV2ServerCommand> {
    command(
        json!({"type":"message.dispatch","commandId":format!("command:{id}"),"threadId":thread,"messageId":format!("message:{id}"),"text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
    )
}

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result {
        Err(Error::Rejected { reason, .. }) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The managed document alone, with the default policy, caps thread creation and mode changes.
#[tokio::test]
async fn a_managed_ceiling_refuses_creating_or_switching_a_thread_above_it() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let policy = managed_ceiling()?;
    let runtime = start_with(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
        NOW,
        |adapter| adapter,
        |ports| ports.managed_policy = Some(policy),
    )
    .await?;
    create_project(&runtime, &workspace).await?;
    let refused = runtime
        .dispatch_command(create_thread("thread:full", "full-access", MODEL)?)
        .await;
    assert_eq!(refusal(refused), CEILING_REFUSAL);
    runtime
        .dispatch_command(create_thread(THREAD, "approval-required", MODEL)?)
        .await?;
    let raise = |mode: &str| {
        command(
            json!({"type":"thread.runtime-mode.set","commandId":format!("command:{mode}"),"threadId":THREAD,"runtimeMode":mode}),
        )
    };
    let refused = runtime.dispatch_command(raise("full-access")?).await;
    assert_eq!(refusal(refused), CEILING_REFUSAL);
    runtime
        .dispatch_command(raise("auto-accept-edits")?)
        .await?;
    runtime.shutdown().await?;
    Ok(())
}

/// A thread created full-access before the policy: after a restart under the policy its next
/// message is refused with the turn text, and lowering the thread's mode lets the turn run.
/// (A turn start pending at shutdown does not survive: recovery cancels it with its run.)
#[tokio::test]
async fn a_thread_above_a_new_ceiling_is_refused_after_a_restart_until_its_mode_is_lowered()
-> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("statev2.sqlite");
    let workspace = Workspace::plain(directory.path())?;
    let runtime = start(&database, workspace.checkpoints.clone()).await?;
    create_project(&runtime, &workspace).await?;
    runtime
        .dispatch_command(create_thread(THREAD, "full-access", MODEL)?)
        .await?;
    runtime.shutdown().await?;
    drop(runtime);

    let policy = managed_ceiling()?;
    let restarted = start_with(
        &database,
        workspace.checkpoints.clone(),
        NOW,
        |adapter| adapter,
        |ports| ports.managed_policy = Some(policy),
    )
    .await?;
    let refused = restarted.dispatch_command(send(THREAD, "refused")?).await;
    assert_eq!(
        refusal(refused),
        format!("{CEILING_REFUSAL} Switch this thread's mode to continue.")
    );
    restarted
        .dispatch_command(command(json!({"type":"thread.runtime-mode.set","commandId":"command:lower","threadId":THREAD,"runtimeMode":"approval-required"}))?)
        .await?;
    let mut subscription = restarted
        .transport
        .subscribe_thread(
            serde_json::from_value(json!({"threadId":THREAD}))?,
            LiveStreamLimits::default(),
        )
        .await?;
    restarted.dispatch_command(send(THREAD, "lowered")?).await?;
    await_completed_turn(&mut subscription).await?;
    subscription.close().await?;
    restarted.shutdown().await?;
    Ok(())
}

/// A session open that waits until released, so it holds the worker's only slot.
struct Hold {
    thread: ThreadId,
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: tokio::sync::watch::Receiver<bool>,
}

/// The Replay adapter declaring a mode list; records the runtime mode and the ceiling each
/// session opens with.
struct Probe {
    inner: Arc<dyn ProviderAdapter>,
    modes: Vec<RuntimeMode>,
    opened: std::sync::Mutex<Vec<(RuntimeMode, Option<RuntimeMode>)>>,
    hold: Option<Hold>,
    /// Asked at each session open; its answers are kept in `checked`.
    check: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    checked: std::sync::Mutex<Vec<bool>>,
}

impl Probe {
    fn new(inner: Arc<dyn ProviderAdapter>, modes: Vec<RuntimeMode>) -> Self {
        Self {
            inner,
            modes,
            opened: Default::default(),
            hold: None,
            check: None,
            checked: Default::default(),
        }
    }
    async fn record(&self, input: &OpenSession) {
        if let Some(hold) = self
            .hold
            .as_ref()
            .filter(|hold| hold.thread == input.thread_id)
        {
            let _ = hold.entered.send(());
            let mut release = hold.release.clone();
            let _ = release.wait_for(|released| *released).await;
        }
        if let Some(check) = &self.check {
            self.checked.lock().expect("checked lock").push(check());
        }
        self.opened.lock().expect("modes lock").push((
            input.runtime_policy.runtime_mode,
            input.runtime_policy.max_runtime_mode,
        ));
    }
}

const ALL_MODES: [RuntimeMode; 4] = [
    RuntimeMode::ApprovalRequired,
    RuntimeMode::AutoAcceptEdits,
    RuntimeMode::Auto,
    RuntimeMode::FullAccess,
];

#[async_trait]
impl ProviderAdapter for Probe {
    fn instance_id(&self) -> &ProviderInstanceId {
        self.inner.instance_id()
    }
    fn driver(&self) -> ProviderDriverKind {
        self.inner.driver()
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        self.inner.capabilities().await
    }
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        self.inner.plan_selection_transition(input).await
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.record(&input).await;
        self.inner.open_session(input, cancellation).await
    }
    async fn open_session_with_policy(
        &self,
        input: OpenSession,
        options: RuntimePolicyOptions,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.record(&input).await;
        self.inner
            .open_session_with_policy(input, options, cancellation)
            .await
    }
    fn supported_runtime_modes(&self) -> Option<Vec<RuntimeMode>> {
        Some(self.modes.clone())
    }
}

/// With no host provider snapshot, an `auto` thread on a provider offering only
/// approval-required and full access runs as approval-required, as TypeScript's snapshot makes it.
#[tokio::test]
async fn a_mode_the_provider_does_not_offer_runs_as_approval_required() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let wrapped = std::sync::Mutex::new(None::<Arc<Probe>>);
    let runtime = start_with(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
        NOW,
        |inner| {
            // Muse's offered modes (`provider-muse/src/server/status.ts`).
            let adapter = Arc::new(Probe::new(
                inner,
                vec![RuntimeMode::ApprovalRequired, RuntimeMode::FullAccess],
            ));
            *wrapped.lock().expect("adapter lock") = Some(adapter.clone());
            adapter
        },
        |_| {},
    )
    .await?;
    let adapter = wrapped
        .lock()
        .expect("adapter lock")
        .clone()
        .expect("adapter wrapped");
    create_project(&runtime, &workspace).await?;
    runtime
        .dispatch_command(create_thread(THREAD, "auto", MODEL)?)
        .await?;
    let mut subscription = runtime
        .transport
        .subscribe_thread(
            serde_json::from_value(json!({"threadId":THREAD}))?,
            LiveStreamLimits::default(),
        )
        .await?;
    runtime.dispatch_command(send(THREAD, "auto")?).await?;
    await_completed_turn(&mut subscription).await?;
    subscription.close().await?;
    runtime.shutdown().await?;
    assert_eq!(
        *adapter.opened.lock().expect("modes lock"),
        vec![(RuntimeMode::ApprovalRequired, None)]
    );
    Ok(())
}

/// The managed ceiling reaches the adapter as `max_runtime_mode` (the Claude adapter filters
/// `setMode` suggestions on it), with the thread's own mode below it.
#[tokio::test]
async fn the_managed_ceiling_reaches_the_session_the_provider_opens() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let policy = managed_ceiling()?;
    let wrapped = std::sync::Mutex::new(None::<Arc<Probe>>);
    let runtime = start_with(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
        NOW,
        |inner| {
            let adapter = Arc::new(Probe::new(inner, ALL_MODES.to_vec()));
            *wrapped.lock().expect("adapter lock") = Some(adapter.clone());
            adapter
        },
        |ports| ports.managed_policy = Some(policy),
    )
    .await?;
    let adapter = wrapped
        .lock()
        .expect("adapter lock")
        .clone()
        .expect("adapter wrapped");
    create_project(&runtime, &workspace).await?;
    runtime
        .dispatch_command(create_thread(THREAD, "approval-required", MODEL)?)
        .await?;
    let mut subscription = runtime
        .transport
        .subscribe_thread(
            serde_json::from_value(json!({"threadId":THREAD}))?,
            LiveStreamLimits::default(),
        )
        .await?;
    runtime.dispatch_command(send(THREAD, "capped")?).await?;
    await_completed_turn(&mut subscription).await?;
    subscription.close().await?;
    runtime.shutdown().await?;
    assert_eq!(
        *adapter.opened.lock().expect("modes lock"),
        vec![(
            RuntimeMode::ApprovalRequired,
            Some(RuntimeMode::AutoAcceptEdits)
        )]
    );
    Ok(())
}

/// The turn-start backstop through the composed runtime: a run was admitted while its thread
/// was within the ceiling, then the stored mode rose above it without dispatch (as data written
/// before the policy or by another writer). The turn start refuses it and settles it failed
/// with TypeScript's error item, before any session opens.
#[tokio::test]
async fn a_turn_start_above_the_ceiling_settles_failed_through_composition() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let policy = managed_ceiling()?;
    let held = ThreadId("thread:held".into());
    let late = ThreadId("thread:late".into());
    let (entered, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release, release_rx) = tokio::sync::watch::channel(false);
    let wrapped = std::sync::Mutex::new(None::<Arc<Probe>>);
    let runtime = start_with(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
        NOW,
        |inner| {
            let mut adapter = Probe::new(inner, ALL_MODES.to_vec());
            adapter.hold = Some(Hold {
                thread: held.clone(),
                entered,
                release: release_rx,
            });
            let adapter = Arc::new(adapter);
            *wrapped.lock().expect("adapter lock") = Some(adapter.clone());
            adapter
        },
        |ports| {
            ports.managed_policy = Some(policy);
            ports.worker.concurrency = 1;
        },
    )
    .await?;
    let adapter = wrapped
        .lock()
        .expect("adapter lock")
        .clone()
        .expect("adapter wrapped");
    create_project(&runtime, &workspace).await?;
    for thread in [&held, &late] {
        runtime
            .dispatch_command(create_thread(&thread.0, "approval-required", MODEL)?)
            .await?;
    }
    runtime.dispatch_command(send(&held.0, "held")?).await?;
    entered_rx.recv().await.expect("the held session opens");
    runtime.dispatch_command(send(&late.0, "late")?).await?;
    let mut thread = runtime
        .projections
        .thread(late.clone())
        .await?
        .expect("late thread");
    thread.runtime_mode = RuntimeMode::FullAccess;
    let event = json!({"id":"event:raised","threadId":late,"type":"thread.runtime-mode-updated","occurredAt":NOW,"payload":thread});
    runtime
        .events
        .commit(CommitRequest {
            receipt: None,
            guard: CommitGuard::None,
            decision: Decision {
                events: vec![serde_json::from_value(event)?],
                ..Default::default()
            },
        })
        .await?;
    let mut subscription = runtime
        .transport
        .subscribe_thread(
            serde_json::from_value(json!({"threadId":late}))?,
            LiveStreamLimits::default(),
        )
        .await?;
    release.send_replace(true);
    let mut settled = None;
    while settled.is_none() {
        let batch = subscription.pull().await?.expect("thread stream ended");
        for item in batch {
            let item = serde_json::to_value(&item.value)?;
            let event = &item["event"];
            if event["type"] == "run.updated"
                && SETTLED.contains(&event["payload"]["status"].as_str().unwrap_or_default())
            {
                settled = Some(event["payload"]["status"].clone());
            }
        }
    }
    subscription.close().await?;
    assert_eq!(settled, Some(json!("failed")));
    let projection = runtime
        .projections
        .projection(late.clone())
        .await?
        .expect("late thread");
    let errors: Vec<_> = projection
        .turn_items
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|item| item["type"] == "error")
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["title"], "Thread mode is above the policy limit");
    assert_eq!(
        errors[0]["failure"]["message"],
        format!("{CEILING_REFUSAL} Switch this thread's mode to continue.")
    );
    runtime.shutdown().await?;
    assert_eq!(
        *adapter.opened.lock().expect("modes lock"),
        vec![(
            RuntimeMode::ApprovalRequired,
            Some(RuntimeMode::AutoAcceptEdits)
        )],
        "only the held thread opened a session"
    );
    Ok(())
}

/// A project-root launch touches only the project checks: every Git, branch and setup call
/// fails, so a launch that reached one would fail the test.
struct RootLaunch;

#[async_trait]
impl t3_orchestration::ports::checkpoints_forks::thread_launch_service::LaunchEnvironment
    for RootLaunch
{
    async fn is_scratch(&self, _: &ProjectId) -> Result<bool> {
        Ok(false)
    }
    async fn reject_during_clone(&self, _: &ProjectId) -> Result<()> {
        Ok(())
    }
    async fn assert_provider_enabled(
        &self,
        _: &ProjectId,
        _: &CommandId,
        _: &ProviderInstanceId,
    ) -> Result<()> {
        Ok(())
    }
    async fn folder_for_thread(
        &self,
        _: &ProjectId,
        _: &ThreadId,
        _: &str,
    ) -> Result<Option<PathBuf>> {
        Ok(None)
    }
    async fn repository_root(&self, _: PathBuf) -> Result<Option<PathBuf>> {
        Err(Error::Unsupported("root launch: repository_root"))
    }
    async fn has_commit(&self, _: PathBuf, _: &str) -> Result<bool> {
        Err(Error::Unsupported("root launch: has_commit"))
    }
    async fn has_ref(&self, _: PathBuf, _: &str) -> Result<bool> {
        Err(Error::Unsupported("root launch: has_ref"))
    }
    async fn remote_for_base(&self, _: PathBuf, _: &str) -> Result<Option<String>> {
        Err(Error::Unsupported("root launch: remote_for_base"))
    }
    async fn local_branches(&self, _: PathBuf) -> Result<Vec<String>> {
        Err(Error::Unsupported("root launch: local_branches"))
    }
    async fn temporary_branch(&self, _: &ProjectId) -> Result<String> {
        Err(Error::Unsupported("root launch: temporary_branch"))
    }
    async fn generate_branch(
        &self,
        _: &ProjectId,
        _: PathBuf,
        _: &t3_orchestration::ports::checkpoints_forks::thread_launch_service::InitialMessage,
        _: CancellationToken,
    ) -> Result<t3_orchestration::ports::checkpoints_forks::thread_launch_service::GeneratedBranch>
    {
        Err(Error::Unsupported("root launch: generate_branch"))
    }
    async fn rename_branch(
        &self,
        _: PathBuf,
        _: &str,
        _: t3_orchestration::ports::checkpoints_forks::thread_launch_service::GeneratedBranch,
        _: CancellationToken,
    ) -> Result<String> {
        Err(Error::Unsupported("root launch: rename_branch"))
    }
    async fn setup(
        &self,
        _: t3_orchestration::ports::checkpoints_forks::thread_launch_service::SetupInput,
        _: CancellationToken,
    ) -> Result<t3_orchestration::ports::checkpoints_forks::thread_launch_service::SetupRun> {
        // No project setup script.
        Ok(t3_orchestration::ports::checkpoints_forks::thread_launch_service::SetupRun::Skipped)
    }
    async fn close_terminal(&self, _: &ThreadId, _: &str) -> Result<()> {
        Err(Error::Unsupported("root launch: close_terminal"))
    }
}

/// The project root as the launch workspace; worktrees are refused.
struct RootWorkspace(PathBuf);

#[async_trait]
impl WorkspaceBackend for RootWorkspace {
    async fn prepare(
        &self,
        _: ProjectId,
        _: ThreadId,
        strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        _: CancellationToken,
    ) -> Result<t3_orchestration::api::Workspace> {
        match strategy {
            OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(_) => {
                Ok(t3_orchestration::api::Workspace {
                    cwd: self.0.clone(),
                    branch: None,
                    worktree_path: None,
                })
            }
            OrchestrationV2ThreadLaunchWorkspaceStrategy::ExistingWorktree(existing) => {
                Ok(t3_orchestration::api::Workspace {
                    cwd: PathBuf::from(&existing.worktree_path.0),
                    branch: existing.branch.flatten().map(|branch| branch.0),
                    worktree_path: Some(PathBuf::from(&existing.worktree_path.0)),
                })
            }
            _ => Err(Error::Unsupported("root workspace: new worktrees")),
        }
    }
    async fn remove(
        &self,
        _: t3_orchestration::api::Workspace,
        _: CancellationToken,
    ) -> Result<()> {
        Ok(())
    }
    async fn current_branch(&self, _: PathBuf) -> Result<Option<String>> {
        Ok(None)
    }
}

/// `orchestration.launchThread` from a client: a project-root launch with a first message
/// creates the thread as the user from `web` and runs its turn.
#[tokio::test]
async fn a_client_launch_creates_the_thread_and_runs_its_first_turn() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let runtime = start_with(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
        NOW,
        |adapter| adapter,
        |ports| {
            ports.launch = Some(Arc::new(RootLaunch));
            ports.workspaces = Arc::new(RootWorkspace(workspace.path.clone()));
        },
    )
    .await?;
    create_project(&runtime, &workspace).await?;
    let input = serde_json::from_value(
        json!({"commandId":"command:launch","threadId":THREAD,"projectId":"project:runtime","title":"Launched","modelSelection":{"instanceId":DEV_REPLAY_INSTANCE_ID,"model":MODEL},"runtimeMode":"approval-required","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"Hello","attachments":[]}}),
    )?;
    let mut committed = runtime.events.subscribe();
    let launched = runtime.launch_client_thread(input).await?;
    assert_eq!(launched.thread_id, ThreadId(THREAD.into()));
    assert!(!launched.resumed);
    let thread = serde_json::to_value(&launched.projection.thread)?;
    assert_eq!(thread["createdBy"], "user");
    assert_eq!(thread["creationSource"], "web");
    loop {
        let projection = runtime
            .projections
            .projection(ThreadId(THREAD.into()))
            .await?
            .expect("launched thread");
        let statuses = serde_json::to_value(&projection.runs)?;
        if statuses[0]["status"] == "completed" {
            break;
        }
        assert!(
            !SETTLED.contains(&statuses[0]["status"].as_str().unwrap_or_default()),
            "{statuses}"
        );
        committed.recv().await.map_err(|error| Error::Service {
            operation: "launch test",
            detail: error.to_string(),
        })?;
    }
    runtime.shutdown().await?;
    Ok(())
}

/// A plain-file workflow reader for the test: identity is the file's length.
struct PlainWorkflowFiles;
struct PlainOpenFile(PathBuf);
#[async_trait]
impl t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowOpenFile
    for PlainOpenFile
{
    async fn stat(
        &self,
    ) -> std::io::Result<
        t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowFileStat,
    > {
        let metadata = std::fs::metadata(&self.0)?;
        Ok(
            t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowFileStat {
                size: metadata.len(),
                identity:
                    t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowFileIdentity {
                        device: 1,
                        inode: metadata.len(),
                    },
                regular: metadata.is_file(),
            },
        )
    }
    async fn read_at(&self, offset: u64, length: usize) -> std::io::Result<Vec<u8>> {
        let bytes = std::fs::read(&self.0)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        Ok(bytes[start..(start + length).min(bytes.len())].to_vec())
    }
}
#[async_trait]
impl t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowFileBackend
    for PlainWorkflowFiles
{
    async fn realpath(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
    async fn open_nonblocking(
        &self,
        path: &Path,
    ) -> std::io::Result<
        Arc<dyn t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowOpenFile>,
    > {
        Ok(Arc::new(PlainOpenFile(path.to_path_buf())))
    }
    async fn lstat_identity(
        &self,
        path: &Path,
    ) -> std::io::Result<
        t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowFileIdentity,
    > {
        Ok(
            t3_orchestration::ports::runs_ingestion::workflow_file_read::WorkflowFileIdentity {
                device: 1,
                inode: std::fs::symlink_metadata(path)?.len(),
            },
        )
    }
}

/// `orchestration.getWorkflowScript` reads a `.js` script under the projects root and refuses
/// one outside it; without a host reader the method is unsupported.
#[tokio::test]
async fn workflow_scripts_are_read_only_under_the_projects_root() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let projects = directory.path().join("projects");
    std::fs::create_dir_all(projects.join("one"))?;
    let script = projects.join("one").join("flow.js");
    std::fs::write(&script, "export default 1;\n")?;
    let outside = directory.path().join("outside.js");
    std::fs::write(&outside, "export default 2;\n")?;
    let unbound = start(
        &directory.path().join("unbound.sqlite"),
        workspace.checkpoints.clone(),
    )
    .await?;
    let request = |path: &Path| -> Result<OrchestrationV2GetWorkflowScriptInput> {
        Ok(serde_json::from_value(
            json!({"threadId":THREAD,"scriptPath":path.to_string_lossy()}),
        )?)
    };
    assert!(matches!(
        unbound.workflow_script(request(&script)?).await,
        Err(Error::Unsupported(_))
    ));
    unbound.shutdown().await?;
    let root = projects.clone();
    let runtime = start_with(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
        NOW,
        |adapter| adapter,
        |ports| {
            ports.workflow_files = Some(t3_orchestration::composition::WorkflowFiles {
                backend: Arc::new(PlainWorkflowFiles),
                projects_root: root,
            })
        },
    )
    .await?;
    let read = runtime
        .workflow_script(request(&script)?)
        .await?
        .expect("a script under the root");
    assert_eq!(read.contents, "export default 1;\n");
    assert!(!read.truncated);
    let refused = runtime
        .workflow_script(request(&outside)?)
        .await?
        .expect_err("a script outside the root");
    assert_eq!(
        serde_json::to_value(&refused.reason)?,
        json!("outside-root")
    );
    runtime.shutdown().await?;
    Ok(())
}

fn caller(
    thread: Option<&str>,
    runtime_mode: RuntimeMode,
    link_fence: Option<serde_json::Value>,
) -> Result<t3_orchestration::composition::CommandCaller> {
    Ok(t3_orchestration::composition::CommandCaller {
        thread_id: thread.map(|id| ThreadId(id.into())),
        limit: t3_orchestration::ports::decisions::dispatch_mode_limit::DispatchModes {
            runtime_mode,
            interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode::Default,
        },
        link_fence: link_fence.map(serde_json::from_value).transpose()?,
    })
}

fn set_mode(thread: &str, mode: &str, id: &str) -> Result<OrchestrationV2ServerCommand> {
    command(
        json!({"type":"thread.runtime-mode.set","commandId":format!("command:{id}"),"threadId":thread,"runtimeMode":mode}),
    )
}

fn refused<T: std::fmt::Debug>(result: Result<T>) -> t3_orchestration::error::CallerRefusal {
    match result {
        Err(Error::CallerRefused { refusal, .. }) => refusal,
        other => panic!("expected a caller refusal, got {other:?}"),
    }
}

/// An agent's caller may not reach above its own modes: not a thread already broader than it,
/// nor a thread it would make broader (TS `DispatchModeLimit`, `resolveRuntimeMode`).
#[tokio::test]
async fn a_caller_cannot_change_or_raise_a_thread_above_its_own_modes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let runtime = start(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
    )
    .await?;
    create_project(&runtime, &workspace).await?;
    runtime
        .dispatch_command(create_thread("thread:caller", "approval-required", MODEL)?)
        .await?;
    runtime
        .dispatch_command(create_thread("thread:broad", "full-access", MODEL)?)
        .await?;
    runtime
        .dispatch_command(create_thread("thread:narrow", "approval-required", MODEL)?)
        .await?;
    let commands = runtime.commands_for(caller(
        Some("thread:caller"),
        RuntimeMode::ApprovalRequired,
        None,
    )?);
    let above = refused(
        commands
            .dispatch(set_mode(
                "thread:broad",
                "approval-required",
                "lower-broad",
            )?)
            .await,
    );
    assert_eq!(above.code(), "runtime_mode_escalation_denied");
    assert_eq!(
        above.to_string(),
        "Thread thread:broad now runs in full-access/default mode, above what this caller may change."
    );
    let escalation = refused(
        commands
            .dispatch(set_mode("thread:narrow", "full-access", "raise-narrow")?)
            .await,
    );
    assert_eq!(escalation.code(), "runtime_mode_escalation_denied");
    assert_eq!(
        escalation.to_string(),
        "Child runtime mode full-access is broader than parent mode approval-required."
    );
    let created = refused(
        commands
            .dispatch(create_thread("thread:child", "auto", MODEL)?)
            .await,
    );
    assert_eq!(
        created.to_string(),
        "Child runtime mode auto is broader than parent mode approval-required."
    );
    // Within its modes the caller acts through the same dispatcher, with a receipt.
    commands
        .dispatch(create_thread("thread:allowed", "approval-required", MODEL)?)
        .await?;
    runtime.shutdown().await?;
    Ok(())
}

/// A caller fenced to a link changes only threads that link started (TS `assertSameLink`);
/// its own thread is exempt, and a thread the link started is allowed.
#[tokio::test]
async fn a_fenced_caller_changes_only_threads_its_link_started() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let workspace = Workspace::plain(directory.path())?;
    let runtime = start(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
    )
    .await?;
    create_project(&runtime, &workspace).await?;
    let origin = json!({"sessionId":"link:laptop","label":"Laptop"});
    runtime
        .dispatch_command(create_thread("thread:user", "approval-required", MODEL)?)
        .await?;
    let mut linked =
        serde_json::to_value(create_thread("thread:linked", "approval-required", MODEL)?)?;
    linked["linkOrigin"] = origin.clone();
    runtime.dispatch_command(command(linked)?).await?;
    let commands = runtime.commands_for(caller(None, RuntimeMode::FullAccess, Some(origin))?);
    let outside = refused(
        commands
            .dispatch(set_mode("thread:user", "approval-required", "user")?)
            .await,
    );
    assert_eq!(outside.code(), "capability_denied");
    assert_eq!(
        outside.to_string(),
        "This work was started from Laptop through a linked environment, so it may change only threads that link started. A link paired with full access is not limited this way."
    );
    commands
        .dispatch(set_mode("thread:linked", "auto-accept-edits", "linked")?)
        .await?;
    runtime.shutdown().await?;
    Ok(())
}

/// Under a managed ceiling a caller's write cannot take a thread above it, whatever the
/// caller's own modes, and a thread already above it is out of the caller's reach.
#[tokio::test]
async fn a_callers_write_stays_under_the_managed_ceiling() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("statev2.sqlite");
    let workspace = Workspace::plain(directory.path())?;
    let runtime = start(&database, workspace.checkpoints.clone()).await?;
    create_project(&runtime, &workspace).await?;
    runtime
        .dispatch_command(create_thread("thread:old", "full-access", MODEL)?)
        .await?;
    runtime.shutdown().await?;
    drop(runtime);
    let policy = managed_ceiling()?;
    let runtime = start_with(
        &database,
        workspace.checkpoints.clone(),
        NOW,
        |adapter| adapter,
        |ports| ports.managed_policy = Some(policy),
    )
    .await?;
    runtime
        .dispatch_command(create_thread(THREAD, "approval-required", MODEL)?)
        .await?;
    let commands = runtime.commands_for(caller(None, RuntimeMode::FullAccess, None)?);
    let raised = commands
        .dispatch(set_mode(THREAD, "full-access", "raise")?)
        .await;
    assert_eq!(refusal(raised), CEILING_REFUSAL);
    let old = refused(
        commands
            .dispatch(set_mode("thread:old", "approval-required", "old")?)
            .await,
    );
    assert_eq!(
        old.to_string(),
        "Thread thread:old now runs in full-access/default mode, above what this caller may change."
    );
    runtime.shutdown().await?;
    Ok(())
}

/// Review t3-git H-1: a thread's worktree is bound, and the binding saved, before its first turn
/// opens a provider session there.
#[cfg(feature = "git-backend")]
#[tokio::test]
async fn a_launched_worktree_is_bound_before_its_first_turn() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store_path = directory
        .path()
        .join("state")
        .join("repository-bindings.json");
    let store = Arc::new(
        t3_orchestration::runtime::repository_bindings::FileRepositoryBindings::new(
            store_path.clone(),
        ),
    );
    let workspace = Workspace::repository_with(directory.path(), Some(store)).await?;
    let worktree = directory.path().join("feature");
    workspace
        .git(&[
            "worktree",
            "add",
            "-b",
            "feature",
            &worktree.to_string_lossy(),
        ])
        .await?;
    let key = worktree.to_string_lossy().into_owned();
    let bound = move || {
        std::fs::read(&store_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|saved| saved["bindings"].get(&key).is_some())
    };
    let wrapped = std::sync::Mutex::new(None::<Arc<Probe>>);
    let root = workspace.path.clone();
    let runtime = start_with(
        &directory.path().join("statev2.sqlite"),
        workspace.checkpoints.clone(),
        NOW,
        |inner| {
            let mut adapter = Probe::new(inner, ALL_MODES.to_vec());
            adapter.check = Some(Box::new(bound));
            let adapter = Arc::new(adapter);
            *wrapped.lock().expect("adapter lock") = Some(adapter.clone());
            adapter
        },
        |ports| {
            ports.launch = Some(Arc::new(RootLaunch));
            ports.workspaces = Arc::new(RootWorkspace(root));
        },
    )
    .await?;
    let adapter = wrapped
        .lock()
        .expect("adapter lock")
        .clone()
        .expect("adapter wrapped");
    create_project(&runtime, &workspace).await?;
    let input = serde_json::from_value(
        json!({"commandId":"command:launch","threadId":THREAD,"projectId":"project:runtime","title":"Worktree","modelSelection":{"instanceId":DEV_REPLAY_INSTANCE_ID,"model":MODEL},"runtimeMode":"approval-required","interactionMode":"default","workspaceStrategy":{"type":"existing_worktree","worktreePath":worktree,"branch":"feature"},"initialMessage":{"text":"Hello","attachments":[]}}),
    )?;
    let mut committed = runtime.events.subscribe();
    runtime.launch_client_thread(input).await?;
    loop {
        let projection = runtime
            .projections
            .projection(ThreadId(THREAD.into()))
            .await?
            .expect("launched thread");
        let statuses = serde_json::to_value(&projection.runs)?;
        if statuses[0]["status"] == "completed" {
            break;
        }
        assert!(
            !SETTLED.contains(&statuses[0]["status"].as_str().unwrap_or_default()),
            "{}",
            serde_json::to_value(&projection.turn_items)?
        );
        committed.recv().await.map_err(|error| Error::Service {
            operation: "worktree launch test",
            detail: error.to_string(),
        })?;
    }
    runtime.shutdown().await?;
    assert_eq!(*adapter.checked.lock().expect("checked lock"), vec![true]);
    Ok(())
}

/// Plain `git` with `home`'s isolated configuration, as an agent runs it in its folder.
#[cfg(feature = "git-backend")]
fn agent_git(home: &Path, cwd: &Path, args: &[&str]) -> Result<()> {
    let mut command = std::process::Command::new("git");
    command.current_dir(cwd).args(args);
    for (key, value) in git_environment(home) {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
    ] {
        command.env_remove(key);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(Error::Service {
            operation: "agent git",
            detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// The runtime over `root`'s database and Git backend, launching threads into existing worktrees.
#[cfg(feature = "git-backend")]
async fn worktree_runtime(
    root: &Path,
    checkpoints: Arc<dyn CheckpointPort>,
) -> Result<OrchestrationRuntime> {
    let workspace = root.join("workspace");
    start_with(
        &root.join("statev2.sqlite"),
        checkpoints,
        NOW,
        |adapter| adapter,
        |ports| {
            ports.launch = Some(Arc::new(RootLaunch));
            ports.workspaces = Arc::new(RootWorkspace(workspace));
        },
    )
    .await
}

/// Waits until the thread has `count` runs and all of them are settled.
#[cfg(feature = "git-backend")]
async fn settled_runs(
    runtime: &OrchestrationRuntime,
    count: usize,
) -> Result<OrchestrationV2ThreadProjection> {
    let mut committed = runtime.events.subscribe();
    loop {
        let projection = runtime
            .projections
            .projection(ThreadId(THREAD.into()))
            .await?
            .expect("thread");
        let runs = serde_json::to_value(&projection.runs)?;
        let runs = runs.as_array().cloned().unwrap_or_default();
        if runs.len() == count
            && runs
                .iter()
                .all(|run| SETTLED.contains(&run["status"].as_str().unwrap_or_default()))
        {
            return Ok(projection);
        }
        committed.recv().await.map_err(|error| Error::Service {
            operation: "settled runs",
            detail: error.to_string(),
        })?;
    }
}

#[cfg(feature = "git-backend")]
const PLANTED_SEND_CHILD: &str = "T3_ORCHESTRATION_PLANTED_SEND_CHILD";

/// Re-review 4 F1: a send never rebinds a worktree thread's folder. After a restart, a
/// repository planted in place of the worktree's `.git` keeps being refused: the saved binding is
/// unchanged and the planted repository's filters never run as the server user.
#[cfg(feature = "git-backend")]
#[tokio::test]
async fn a_send_after_a_restart_never_rebinds_a_planted_worktree_repository() -> Result<()> {
    if let Some(root) = std::env::var_os(PLANTED_SEND_CHILD) {
        return send_after_restart(PathBuf::from(root)).await;
    }
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let store_path = root.join("state").join("repository-bindings.json");
    let workspace = Workspace::repository_with(
        root,
        Some(Arc::new(
            t3_orchestration::runtime::repository_bindings::FileRepositoryBindings::new(
                store_path.clone(),
            ),
        )),
    )
    .await?;
    let worktree = root.join("feature");
    workspace
        .git(&[
            "worktree",
            "add",
            "-b",
            "feature",
            &worktree.to_string_lossy(),
        ])
        .await?;
    let runtime = worktree_runtime(root, workspace.checkpoints.clone()).await?;
    create_project(&runtime, &workspace).await?;
    runtime
        .launch_client_thread(serde_json::from_value(json!({"commandId":"command:launch","threadId":THREAD,"projectId":"project:runtime","title":"Worktree","modelSelection":{"instanceId":DEV_REPLAY_INSTANCE_ID,"model":MODEL},"runtimeMode":"approval-required","interactionMode":"default","workspaceStrategy":{"type":"existing_worktree","worktreePath":worktree,"branch":"feature"},"initialMessage":{"text":"Hello","attachments":[]}}))?)
        .await?;
    settled_runs(&runtime, 1).await?;
    runtime.shutdown().await?;
    drop(runtime);
    let saved = std::fs::read(&store_path)?;
    // The agent replaces the worktree's `.git` pointer with a repository of its own whose clean
    // filter runs whatever it likes.
    let home = root.join("home");
    std::fs::remove_file(worktree.join(".git"))?;
    let marker = root.join("filter-ran");
    agent_git(&home, &worktree, &["init", "--initial-branch=main"])?;
    let target = marker.to_string_lossy().replace('\\', "/");
    agent_git(
        &home,
        &worktree,
        &[
            "config",
            "filter.planted.clean",
            &format!("touch '{target}'; cat"),
        ],
    )?;
    std::fs::write(worktree.join(".gitattributes"), "* filter=planted\n")?;
    agent_git(&home, &worktree, &["add", "-A"])?;
    agent_git(&home, &worktree, &["commit", "-m", "planted"])?;
    std::fs::remove_file(&marker)?;
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "a_send_after_a_restart_never_rebinds_a_planted_worktree_repository",
            "--exact",
            "--nocapture",
        ])
        .env(PLANTED_SEND_CHILD, root)
        .output()?;
    assert!(
        !marker.exists(),
        "the planted repository's filter ran as the server user"
    );
    assert_eq!(
        String::from_utf8_lossy(&std::fs::read(&store_path)?),
        String::from_utf8_lossy(&saved),
        "the send rebound the worktree"
    );
    assert!(
        output.status.success(),
        "after restart: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

/// The after-restart half, in a fresh process: t3-git's first-use memory is empty, so only the
/// saved binding stands between the send and the planted repository.
#[cfg(feature = "git-backend")]
async fn send_after_restart(root: PathBuf) -> Result<()> {
    let checkpoints = git_checkpoints(
        &root.join("home"),
        Some(Arc::new(
            t3_orchestration::runtime::repository_bindings::FileRepositoryBindings::new(
                root.join("state").join("repository-bindings.json"),
            ),
        )),
    );
    let runtime = worktree_runtime(&root, checkpoints).await?;
    runtime
        .threads
        .send_to_thread(serde_json::from_value(json!({"projectId":"project:runtime","commandId":"command:after-restart","threadId":THREAD,"messageId":"message:after-restart","text":"Hello","attachments":[],"mode":"auto","freshContext":false,"modelSelection":null,"scheduledTaskId":null,"senderThreadId":null,"linkedSender":null,"createdBy":"user","creationSource":"web"}))?)
        .await?;
    let projection = settled_runs(&runtime, 2).await?;
    let projection = serde_json::to_value(&projection)?;
    let latest = projection["runs"]
        .as_array()
        .and_then(|runs| runs.iter().max_by_key(|run| run["ordinal"].as_u64()))
        .expect("the second run")["id"]
        .clone();
    assert!(
        !projection["checkpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|checkpoint| checkpoint["runId"] == latest && checkpoint["status"] == "ready"),
        "the turn's checkpoint was taken in the planted repository: {}",
        projection["checkpoints"]
    );
    runtime.shutdown().await?;
    Ok(())
}
