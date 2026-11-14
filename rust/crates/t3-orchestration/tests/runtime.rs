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
    managed_policy::ManagedPolicyDocument, provider_policy::RuntimeMode,
    settings::ServerSettings,
};
use t3_orchestration::{
    adapters::acp::{DEV_REPLAY_INSTANCE_ID, dev_replay_registration},
    api::*,
    composition::{CheckpointPort, OrchestrationRuntime, RandomIds, RuntimePorts, StaticSettings},
    effects::{EffectRequest, EffectStatus},
    error::{Error, Result},
    provider::*,
    ports::{
        checkpoints_forks::checkpoint_service::CheckpointMetadata,
        sessions_recovery::provider_adapter_registry::AdapterRegistry,
        transport::live_stream_budget::LiveStreamLimits,
    },
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
