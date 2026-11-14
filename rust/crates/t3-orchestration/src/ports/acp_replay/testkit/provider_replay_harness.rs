#![cfg(feature = "acp-adapter")]

use crate::{
    adapters::acp::{AcpAdapter, StrictReplayTransportFactory},
    error::{Error, Result},
    ports::acp_replay::testkit::orchestrator_scenario::{
        Scenario, ScenarioResult, ScenarioServices, run_scenario,
    },
    provider::{ProviderAdapter, ProviderError, ProviderResult},
    testing::replay::{ReplayError, ReplayGate, ReplayStatus, ReplayTranscript, decode_acp},
};
use std::sync::Arc;
use tokio::sync::watch;

pub struct ProviderReplayHarness {
    pub adapter: Arc<dyn ProviderAdapter>,
    pub gate: ReplayGate,
    pub progress: watch::Receiver<ReplayStatus>,
}

impl ProviderReplayHarness {
    pub fn new(
        transcript: ReplayTranscript,
        mut adapter: AcpAdapter,
        gate: ReplayGate,
        retarget_provider: bool,
    ) -> std::result::Result<Self, ReplayError> {
        let transcript = decode_acp(transcript, &adapter.driver.0, retarget_provider)?;
        let (status, progress) = watch::channel(ReplayStatus {
            scenario: transcript.scenario.0.clone(),
            cursor: 0,
            total: transcript.entries.len(),
            failure: None,
        });
        adapter.transport = Arc::new(StrictReplayTransportFactory {
            transcript,
            gate: gate.clone(),
            status,
        });
        Ok(Self {
            adapter: Arc::new(adapter),
            gate,
            progress,
        })
    }

    pub fn assert_complete(&self) -> Result<()> {
        let status = self.progress.borrow();
        status.assert_complete().map_err(|error| {
            Error::Provider(ProviderError::Protocol {
                reason: None,
                detail: error.to_string(),
                payload: serde_json::to_value(&*status).ok(),
            })
        })
    }

    pub async fn with_flavor_factory(
        transcript: ReplayTranscript,
        mut base: AcpAdapter,
        gate: ReplayGate,
        factory: &dyn AcpFlavorAdapterFactory,
    ) -> ProviderResult<Self> {
        let transcript = decode_acp(transcript, &base.driver.0, false).map_err(|error| {
            ProviderError::Protocol {
                reason: None,
                detail: error.to_string(),
                payload: None,
            }
        })?;
        let (status, progress) = watch::channel(ReplayStatus {
            scenario: transcript.scenario.0.clone(),
            cursor: 0,
            total: transcript.entries.len(),
            failure: None,
        });
        base.transport = Arc::new(StrictReplayTransportFactory {
            transcript,
            gate: gate.clone(),
            status,
        });
        let adapter = factory.create(base, gate.clone()).await?;
        Ok(Self {
            adapter,
            gate,
            progress,
        })
    }

    pub async fn run(
        &self,
        scenario: Scenario,
        services: &ScenarioServices,
    ) -> Result<ScenarioResult> {
        let services = ScenarioServices {
            gate: self.gate.clone(),
            ..services.clone()
        };
        let result = run_scenario(scenario, &services).await?;
        self.assert_complete()?;
        Ok(result)
    }
}

// Local shim: native vendor policy/mapping wrappers are owned by their provider packages.
#[async_trait::async_trait]
pub trait AcpFlavorAdapterFactory: Send + Sync {
    async fn create(
        &self,
        base: AcpAdapter,
        gate: ReplayGate,
    ) -> ProviderResult<Arc<dyn ProviderAdapter>>;
}

struct AdmittedRuntime(Arc<crate::composition::OrchestrationRuntime>);
#[async_trait::async_trait]
impl crate::api::CommandDispatcher for AdmittedRuntime {
    async fn dispatch(
        &self,
        command: t3_contracts::orchestration_v2::OrchestrationV2ServerCommand,
    ) -> Result<crate::stores::receipts::CommandReceipt> {
        self.0.dispatch_command(command).await
    }
}

pub struct ReplayFixtureHostInput {
    pub database: crate::stores::StoreDatabase,
    pub providers: Arc<dyn crate::api::ProviderRegistry>,
    pub clock: Arc<crate::testing::ManualClock>,
    pub ids: Arc<dyn crate::api::IdGenerator>,
    pub workspace: std::path::PathBuf,
    pub runtime_policy_override: serde_json::Value,
    pub run_continuation_worker: bool,
    pub provider_batch_observer:
        Option<super::provider_replay_fixtures::native::ProviderBatchObserver>,
}

// Local shim: the host supplies checkpoints, managed policy and native continuation services.
#[async_trait::async_trait]
pub trait ReplayFixtureHostFactory: Send + Sync {
    async fn prepare_workspace(&self, fixture: &str, workspace: &std::path::Path) -> Result<()>;
    async fn runtime_ports(
        &self,
        input: ReplayFixtureHostInput,
    ) -> Result<crate::composition::RuntimePorts>;
}

pub async fn start_fixture_runtime(
    input: ReplayFixtureHostInput,
    host: &dyn ReplayFixtureHostFactory,
) -> Result<Arc<crate::composition::OrchestrationRuntime>> {
    Ok(Arc::new(
        crate::composition::OrchestrationRuntime::start(host.runtime_ports(input).await?).await?,
    ))
}

pub fn scenario_services_for_runtime(
    runtime: Arc<crate::composition::OrchestrationRuntime>,
    database: crate::stores::StoreDatabase,
    clock: Arc<crate::testing::ManualClock>,
    gate: ReplayGate,
) -> ScenarioServices {
    let worker = runtime.effect_worker();
    ScenarioServices {
        database,
        dispatcher: Arc::new(AdmittedRuntime(runtime.clone())),
        committer: Arc::new(runtime.events.clone()),
        projections: runtime.projections.clone(),
        worker,
        clock,
        gate,
    }
}

pub mod event_tap {
    use super::super::provider_replay_fixtures::native::ProviderEventObserver;
    use crate::provider::*;
    use std::sync::Arc;
    use t3_contracts::{
        orchestration_v2::*,
        provider_instance::{ProviderDriverKind, ProviderInstanceId},
    };
    use tokio::{
        sync::{Mutex, mpsc},
        task::JoinHandle,
    };
    use tokio_util::sync::CancellationToken;

    pub fn observe_adapter(
        inner: Arc<dyn ProviderAdapter>,
        observer: ProviderEventObserver,
    ) -> Arc<dyn ProviderAdapter> {
        Arc::new(ObservedAdapter { inner, observer })
    }
    struct ObservedAdapter {
        inner: Arc<dyn ProviderAdapter>,
        observer: ProviderEventObserver,
    }
    impl ObservedAdapter {
        fn session(&self, inner: Arc<dyn SessionRuntime>) -> Arc<dyn SessionRuntime> {
            Arc::new(ObservedSession {
                inner,
                observer: self.observer.clone(),
                stop: CancellationToken::new(),
                tasks: Mutex::new(Vec::new()),
            })
        }
    }
    #[async_trait::async_trait]
    impl ProviderAdapter for ObservedAdapter {
        fn instance_id(&self) -> &ProviderInstanceId {
            self.inner.instance_id()
        }
        fn supported_runtime_modes(
            &self,
        ) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
            self.inner.supported_runtime_modes()
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
            Ok(self.session(self.inner.open_session(input, cancellation).await?))
        }
        async fn open_session_with_policy(
            &self,
            input: OpenSession,
            options: RuntimePolicyOptions,
            cancellation: CancellationToken,
        ) -> ProviderResult<Arc<dyn SessionRuntime>> {
            Ok(self.session(
                self.inner
                    .open_session_with_policy(input, options, cancellation)
                    .await?,
            ))
        }
    }
    struct ObservedSession {
        inner: Arc<dyn SessionRuntime>,
        observer: ProviderEventObserver,
        stop: CancellationToken,
        tasks: Mutex<Vec<JoinHandle<()>>>,
    }
    impl Drop for ObservedSession {
        fn drop(&mut self) {
            self.stop.cancel();
        }
    }
    #[async_trait::async_trait]
    impl SessionRuntime for ObservedSession {
        fn session(&self) -> &OrchestrationV2ProviderSession {
            self.inner.session()
        }
        fn can_resume_after_restart(&self) -> bool {
            self.inner.can_resume_after_restart()
        }
        fn background_tasks_end_with_process(&self) -> bool {
            self.inner.background_tasks_end_with_process()
        }
        fn extensions(&self) -> Option<&dyn SessionExtensions> {
            self.inner.extensions()
        }
        async fn subscribe(&self) -> ProviderResult<EventSubscription> {
            if self.stop.is_cancelled() {
                return Err(ProviderError::SessionStopped);
            }
            let mut source = self.inner.subscribe().await?;
            let cancellation = self.stop.child_token();
            let stopped = cancellation.clone();
            let observer = self.observer.clone();
            let (sender, events) = mpsc::channel(64);
            self.tasks.lock().await.push(tokio::spawn(async move {
                loop {
                    let event = tokio::select! { biased; _ = stopped.cancelled() => break, event = source.events.recv() => event };
                    let Some(event) = event else { break; };
                    let failed = event.is_err();
                    if let Ok(event) = &event { observer(event); }
                    if tokio::select! { biased; _ = stopped.cancelled() => true, result = sender.send(event) => result.is_err() } { break; }
                    if failed { break; }
                }
            }));
            Ok(EventSubscription {
                events,
                cancellation,
            })
        }
        async fn close(&self) -> ProviderResult<()> {
            self.stop.cancel();
            let closed = self.inner.close().await;
            let tasks = std::mem::take(&mut *self.tasks.lock().await);
            let mut failed = None;
            for task in tasks {
                if let Err(error) = task.await {
                    failed = Some(error.to_string());
                }
            }
            closed?;
            if let Some(detail) = failed {
                return Err(ProviderError::Protocol {
                    reason: None,
                    detail,
                    payload: None,
                });
            }
            Ok(())
        }
        async fn ensure_thread(
            &self,
            input: EnsureThread,
        ) -> ProviderResult<OrchestrationV2ProviderThread> {
            self.inner.ensure_thread(input).await
        }
        async fn ensure_thread_with_policy(
            &self,
            input: EnsureThread,
            options: RuntimePolicyOptions,
        ) -> ProviderResult<OrchestrationV2ProviderThread> {
            self.inner.ensure_thread_with_policy(input, options).await
        }
        async fn resume_thread(
            &self,
            input: ResumeThread,
        ) -> ProviderResult<OrchestrationV2ProviderThread> {
            self.inner.resume_thread(input).await
        }
        async fn resume_thread_with_policy(
            &self,
            input: ResumeThread,
            options: RuntimePolicyOptions,
        ) -> ProviderResult<OrchestrationV2ProviderThread> {
            self.inner.resume_thread_with_policy(input, options).await
        }
        async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
            self.inner.start_turn(input).await
        }
        async fn start_turn_with_policy(
            &self,
            input: TurnInput,
            options: RuntimePolicyOptions,
        ) -> ProviderResult<()> {
            self.inner.start_turn_with_policy(input, options).await
        }
        async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
            self.inner.steer_turn(input).await
        }
        async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
            self.inner.interrupt_turn(input).await
        }
        async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
            self.inner.respond(input).await
        }
        async fn snapshot(
            &self,
            thread: OrchestrationV2ProviderThread,
        ) -> ProviderResult<ThreadSnapshot> {
            self.inner.snapshot(thread).await
        }
        async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
            self.inner.rollback(input).await
        }
        async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
            self.inner.fork(input).await
        }
        async fn fork_with_policy(
            &self,
            input: ForkInput,
            options: RuntimePolicyOptions,
        ) -> ProviderResult<OrchestrationV2ProviderThread> {
            self.inner.fork_with_policy(input, options).await
        }
    }
}

#[async_trait::async_trait]
impl super::provider_replay_fixtures::native::FixtureAdapter for ProviderReplayHarness {
    fn adapter(&self) -> Arc<dyn ProviderAdapter> {
        self.adapter.clone()
    }
    fn assert_complete(&self) -> Result<()> {
        Self::assert_complete(self)
    }
    async fn shutdown(&self) -> Result<()> {
        self.gate.release_all();
        Ok(())
    }
}

pub struct AcpFixtureFactory<'a> {
    pub native: &'a dyn AcpFlavorAdapterFactory,
    pub instructions: &'a (dyn Fn(&str, &str) -> String + Send + Sync),
    pub assertions:
        &'a (dyn Fn(&str, &ScenarioResult, &ReplayTranscript) -> Result<()> + Send + Sync),
}
#[async_trait::async_trait]
impl super::provider_replay_fixtures::native::FixtureAdapterFactory for AcpFixtureFactory<'_> {
    async fn create(
        &self,
        input: super::provider_replay_fixtures::native::FixtureAdapterInput,
    ) -> Result<Arc<dyn super::provider_replay_fixtures::native::FixtureAdapter>> {
        let mut base = AcpAdapter::replay(
            input.variant.model_selection.instance_id.clone(),
            input.clock,
            input.ids,
            false,
        );
        base.driver = input.variant.driver;
        let mut harness = ProviderReplayHarness::with_flavor_factory(
            input.transcript,
            base,
            input.gate,
            self.native,
        )
        .await?;
        if let Some(observer) = input.observe {
            harness.adapter = event_tap::observe_adapter(harness.adapter, observer);
        }
        Ok(Arc::new(harness))
    }
    fn runtime_instructions(&self, harness: &str, model: &str) -> String {
        (self.instructions)(harness, model)
    }
    fn materialize_workspace(
        &self,
        transcript: ReplayTranscript,
        _workspace: &std::path::Path,
    ) -> Result<ReplayTranscript> {
        Ok(transcript)
    }
    fn assert_output(
        &self,
        name: &str,
        result: &ScenarioResult,
        transcript: &ReplayTranscript,
    ) -> Result<()> {
        (self.assertions)(name, result, transcript)
    }
}

#[cfg(feature = "git-backend")]
pub mod git_fixture {
    use super::*;
    use crate::{
        composition::{BaseRuntimePolicy, RuntimePorts, StaticSettings},
        ports::decisions::runtime_policy::{OverriddenRuntimePolicy, RuntimePolicyOverride},
        runtime::checkpoints::{GitCheckpointBackend, GitRollbackWorkspaceRecovery},
    };
    use std::path::Path;

    pub struct GitReplayFixtureHost;

    use crate::api::*;
    use crate::ports::checkpoints_forks::{
        checkpoint_rollback_service::RollbackWorkspaceRecovery,
        checkpoint_service::CheckpointMetadata,
    };
    use tokio_util::sync::CancellationToken;

    // Git process deadlines need live time while scenario timers use a paused clock.
    async fn live<T: Send + 'static>(
        future: impl std::future::Future<Output = Result<T>> + Send + 'static,
    ) -> Result<T> {
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(future)
        })
        .await
        .map_err(|error| Error::Service {
            operation: "fixture live git runtime",
            detail: error.to_string(),
        })?
    }
    struct LiveCheckpointPort(Arc<GitCheckpointBackend>);
    #[async_trait::async_trait]
    impl CheckpointBackend for LiveCheckpointPort {
        async fn capture(
            &self,
            input: CaptureCheckpoint,
            cancellation: CancellationToken,
        ) -> Result<CheckpointRef> {
            let backend = self.0.clone();
            live(async move { backend.capture(input, cancellation).await }).await
        }
        async fn lookup(
            &self,
            scope: OrchestrationV2CheckpointScope,
            ordinal: u64,
        ) -> Result<Option<CheckpointRef>> {
            let backend = self.0.clone();
            live(async move { backend.lookup(scope, ordinal).await }).await
        }
        async fn diff(
            &self,
            cwd: std::path::PathBuf,
            from: CheckpointRef,
            to: CheckpointRef,
        ) -> Result<CheckpointDiff> {
            let backend = self.0.clone();
            live(async move { backend.diff(cwd, from, to).await }).await
        }
        async fn restore(
            &self,
            scope: OrchestrationV2CheckpointScope,
            ordinal: u64,
            cancellation: CancellationToken,
        ) -> Result<()> {
            let backend = self.0.clone();
            live(async move { backend.restore(scope, ordinal, cancellation).await }).await
        }
        async fn prune(
            &self,
            scope: OrchestrationV2CheckpointScope,
            preserve: Vec<u64>,
        ) -> Result<()> {
            let backend = self.0.clone();
            live(async move { backend.prune(scope, preserve).await }).await
        }
        async fn is_git_repository(&self, cwd: std::path::PathBuf) -> Result<bool> {
            let backend = self.0.clone();
            live(async move { backend.is_git_repository(cwd).await }).await
        }
        async fn delete_checkpoint_refs(
            &self,
            scope: OrchestrationV2CheckpointScope,
            checkpoints: Vec<OrchestrationV2Checkpoint>,
        ) -> Result<()> {
            let backend = self.0.clone();
            live(async move { backend.delete_checkpoint_refs(scope, checkpoints).await }).await
        }
        fn checkpoint_ref(&self, scope: &CheckpointScopeId, ordinal: u64) -> Result<CheckpointRef> {
            self.0.checkpoint_ref(scope, ordinal)
        }
    }
    #[async_trait::async_trait]
    impl CheckpointMetadata for LiveCheckpointPort {
        fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
            self.0.reference(scope, ordinal)
        }
        async fn is_repository(&self, cwd: std::path::PathBuf) -> Result<bool> {
            let backend = self.0.clone();
            live(async move { backend.is_repository(cwd).await }).await
        }
        async fn delete_refs(
            &self,
            scope: OrchestrationV2CheckpointScope,
            checkpoints: Vec<OrchestrationV2Checkpoint>,
        ) -> Result<()> {
            let backend = self.0.clone();
            live(async move { backend.delete_refs(scope, checkpoints).await }).await
        }
    }
    struct LiveRollbackRecovery(Arc<GitRollbackWorkspaceRecovery>);
    #[async_trait::async_trait]
    impl RollbackWorkspaceRecovery for LiveRollbackRecovery {
        async fn is_managed(&self, path: &Path) -> Result<bool> {
            self.0.is_managed(path).await
        }
        async fn recreate(
            &self,
            thread: &OrchestrationV2AppThread,
            cancellation: CancellationToken,
        ) -> Result<()> {
            let recovery = self.0.clone();
            let thread = thread.clone();
            live(async move { recovery.recreate(&thread, cancellation).await }).await
        }
    }

    fn environment(workspace: &Path) -> Result<t3_process::Environment> {
        let home = workspace.join(".git").join("t3-fixture-home");
        std::fs::create_dir_all(&home)?;
        let mut environment = t3_process::Environment::new();
        for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
            environment.insert(key.into(), Some(home.clone().into_os_string()));
        }
        for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
            environment.insert(key.into(), Some(home.join("no-config").into_os_string()));
        }
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_COUNT", "0"),
            ("GIT_AUTHOR_NAME", "T3 Code Test"),
            ("GIT_AUTHOR_EMAIL", "t3code-test@example.com"),
            ("GIT_COMMITTER_NAME", "T3 Code Test"),
            ("GIT_COMMITTER_EMAIL", "t3code-test@example.com"),
        ] {
            environment.insert(key.into(), Some(value.into()));
        }
        for key in ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG", "GIT_TEMPLATE_DIR"] {
            environment.insert(key.into(), None);
        }
        Ok(environment)
    }
    fn git_error(error: t3_git::GitError) -> Error {
        Error::Service {
            operation: "replay fixture git",
            detail: format!("{error:?}"),
        }
    }
    #[async_trait::async_trait]
    impl ReplayFixtureHostFactory for GitReplayFixtureHost {
        async fn prepare_workspace(&self, fixture: &str, workspace: &Path) -> Result<()> {
            let readme = workspace.join("README.md");
            if !readme.try_exists()? {
                std::fs::write(readme, format!("# {fixture}\n"))?;
            }
            let git = t3_git::Git::new(workspace).with_environment(environment(workspace)?);
            live(async move {
                for args in [
                    vec!["init"],
                    vec!["config", "core.autocrlf", "false"],
                    vec!["add", "--all"],
                    vec!["commit", "-m", "initial"],
                ] {
                    git.execute(&args, Default::default())
                        .await
                        .map_err(git_error)?;
                }
                Ok(())
            })
            .await
        }
        async fn runtime_ports(&self, input: ReplayFixtureHostInput) -> Result<RuntimePorts> {
            if input.run_continuation_worker {
                return Err(Error::Unsupported(
                    "fixture continuation worker: provider-core host owner",
                ));
            }
            if input.provider_batch_observer.is_some() {
                return Err(Error::Unsupported(
                    "provider batch observation: owners 03/06",
                ));
            }
            let environment = environment(&input.workspace)?;
            let backend = Arc::new(GitCheckpointBackend::new(move |cwd| {
                t3_git::Git::new(cwd).with_environment(environment.clone())
            }));
            let mut settings =
                serde_json::to_value(t3_contracts::settings::ServerSettings::default())?;
            for instance in input.providers.list().await? {
                let metadata = input.providers.metadata(instance.clone()).await?;
                settings["providerInstances"][&instance.0] =
                    serde_json::json!({"driver":metadata.driver,"enabled":metadata.enabled});
            }
            let mut ports = RuntimePorts::new(
                input.database.clone(),
                input.providers,
                Arc::new(LiveCheckpointPort(backend.clone())),
                input.clock,
            );
            ports.ids = input.ids;
            ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
            ports.policy = Arc::new(OverriddenRuntimePolicy {
                base: Arc::new(BaseRuntimePolicy),
                overrides: RuntimePolicyOverride {
                    cwd: Some(input.workspace.clone()),
                    approval_policy: input.runtime_policy_override.get("approvalPolicy").cloned(),
                    sandbox_policy: input.runtime_policy_override.get("sandboxPolicy").cloned(),
                },
                reasoning_effort: None,
            });
            ports.rollback_workspaces = Some(Arc::new(LiveRollbackRecovery(Arc::new(
                GitRollbackWorkspaceRecovery {
                    backend,
                    database: input.database,
                    managed_roots: vec![input.workspace],
                },
            ))));
            Ok(ports)
        }
    }
}
