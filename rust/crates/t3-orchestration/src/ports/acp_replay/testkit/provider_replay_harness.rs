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
