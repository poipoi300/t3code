//! Composition root of the orchestration runtime (TS `runtimeLayer.ts` plus the orchestration
//! phases of `serverRuntimeStartup.ts`). [`OrchestrationRuntime::start`] is the one entry point
//! a host calls: it wires the packages' services behind the seam traits, runs provider recovery
//! before admitting commands, starts the effect worker, and [`OrchestrationRuntime::shutdown`]
//! stops everything in order.
use crate::{
    api::*,
    decision::engine::{MessageContextPreparer, PureDecisionEngine},
    effects::{EffectExecutionOptions, TitleKind},
    error::{Error, Result},
    ports::{
        checkpoints_forks::{
            checkpoint_capture_service::CheckpointCaptureService,
            checkpoint_rollback_service::{
                CheckpointRollbackService, ResolvedRollbackPolicy, RollbackWorkspaceRecovery,
            },
            checkpoint_service::{CheckpointMetadata, CheckpointService, WorkspaceLocks},
            thread_launch_service::{
                LaunchEnvironment, LaunchInput, LaunchResult, LaunchServices, ThreadLaunchService,
            },
        },
        commit_worker::effect_worker::ServiceExecutor,
        decisions::{runtime_policy::resolve_base, thread_command_executor::KeyedLanes},
        runs_ingestion::{
            provider_event_ingestor::ProviderEventIngestor,
            provider_turn_control_service::ProviderTurnControlService,
            provider_turn_start_service::ProviderTurnStartService,
            run_execution_service::RunExecutionService,
            run_finalization_service::{
                FinalizationObserver, FinalizationTarget, RunFinalizationService,
            },
            runtime_request_service::RuntimeRequestService,
            thread_message_intake::ThreadMessageIntake,
        },
        sessions_recovery::{
            legacy::legacy_v1_thread_importer::LegacyV1ThreadImporter,
            provider_runtime_recovery_service::ProviderRuntimeRecoveryService,
            provider_session_manager::{
                ProviderSessionManager, SessionManagerOptions, SessionPersistence, SessionServices,
            },
            runtime_layer::RuntimeLayer,
        },
        thread_project::{
            project_commands::{
                ProjectCommand, ProjectCommandState, encode_project_command_rejection,
                plan_project_command,
            },
            pull_request_watch_reactor::{PullRequestWatchReactor, PullRequestWatchServices},
            thread_management_service::{
                NoLegacyTranscript, ThreadManagementService, ThreadManagementServices,
                ThreadStopService, ThreadStopServices,
            },
            thread_settlement_service::{ThreadSettlementService, ThreadSettlementServices},
            thread_title_regeneration_service::ThreadTitleRegenerationService,
        },
    },
    projections::{
        store::ProjectionStore,
        write::{PullRequestIdentityRegistry, RegistryProjectionWriter},
    },
    provider::{ConfiguredRuntimePolicy, RuntimeResponse},
    runtime::{
        commit::EventSink,
        dispatch::{
            CommandInputs, ContextCommandInputs, DispatchServices, Dispatcher,
            ProjectionCommandInputs, RestartCommandInputs, RuntimeCommandInputs,
            SettingsCommandInputs,
        },
        ingestion::ProviderCommitter,
        worker::{EffectWorker as OutboxWorker, OutboxSignals, WorkerOptions},
    },
    stores::{
        StoreDatabase, projects,
        receipts::{AggregateId, CommandReceipt, ReceiptStatus},
    },
    transport::service::TransportService,
};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
use t3_contracts::settings::ServerSettings;
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use unicode_segmentation::UnicodeSegmentation;

/// A checkpoint store for the runtime. Package 07 still reads ref naming, repository probing
/// and ref deletion through its own `CheckpointMetadata`; once those move onto
/// `CheckpointBackend` (seam change of 2026-11-13) this becomes `CheckpointBackend` alone.
pub trait CheckpointPort: CheckpointBackend + CheckpointMetadata {}
impl<T: CheckpointBackend + CheckpointMetadata> CheckpointPort for T {}

/// What the host injects. [`RuntimePorts::new`] takes the four ports with no stand-in; every
/// other port starts as one that fails with a typed `Unsupported`, and the host replaces the
/// fields it can supply.
pub struct RuntimePorts {
    pub database: StoreDatabase,
    pub providers: Arc<dyn ProviderRegistry>,
    pub checkpoints: Arc<dyn CheckpointPort>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub settings: Arc<dyn SettingsService>,
    pub policy: Arc<dyn RuntimePolicyResolver>,
    pub attachments: Arc<dyn AttachmentBackend>,
    pub cleanup: Arc<dyn ResourceCleanup>,
    pub workspaces: Arc<dyn WorkspaceBackend>,
    /// Thread-owned agent processes (kill, OOM detection). Without it force-stop cannot kill.
    pub agent_scope: Option<Arc<dyn AgentScope>>,
    /// Usage refresh and completed-turn analytics.
    pub telemetry: Option<Arc<dyn ProviderTelemetry>>,
    pub titles: Option<Arc<dyn TitleGenerator>>,
    /// Resolves composer message context for `message.dispatch`; without it a message that
    /// carries context is decided without it.
    pub message_context: Option<Arc<dyn MessageContextPreparer>>,
    /// ANSI terminal fold (TS `terminalOutput.ts`); without it there is no command-output hub.
    pub terminal_output:
        Option<Arc<dyn crate::ports::transport::command_output_hub::TerminalOutputFold>>,
    /// Shared tokenizer, matcher, ranking and citations; without it no cross-thread search.
    pub search_policy: Option<Arc<dyn crate::ports::transport::thread_search::ThreadSearchPolicy>>,
    /// Shared rendered-text helpers for in-thread find; without it `search` is unsupported.
    /// One store (and so one find cache) serves the runtime.
    pub thread_find_text: Option<Arc<dyn crate::ports::transport::thread_find::ThreadFindText>>,
    /// Without it `launch_thread` is unsupported.
    pub launch: Option<Arc<dyn LaunchEnvironment>>,
    /// Forge URL and repository rules for projected PR identities; without it projections
    /// keep identities as persisted (no host alias or authority normalisation).
    pub pull_request_identities: Option<Arc<dyn PullRequestIdentityRegistry>>,
    /// Forge reads for the PR watch reactor; without it no watch runs.
    pub pull_requests: Option<Arc<dyn PullRequestBackend>>,
    /// Without it provider-thread rollback effects are unsupported.
    pub rollback_workspaces: Option<Arc<dyn RollbackWorkspaceRecovery>>,
    pub finalization_observer: Arc<dyn FinalizationObserver>,
    pub sessions: SessionManagerOptions,
    pub worker: WorkerOptions,
}

impl RuntimePorts {
    pub fn new(
        database: StoreDatabase,
        providers: Arc<dyn ProviderRegistry>,
        checkpoints: Arc<dyn CheckpointPort>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            database,
            providers,
            checkpoints,
            clock,
            ids: Arc::new(RandomIds),
            settings: Arc::new(Unbound),
            policy: Arc::new(BaseRuntimePolicy),
            attachments: Arc::new(Unbound),
            cleanup: Arc::new(Unbound),
            workspaces: Arc::new(Unbound),
            agent_scope: None,
            telemetry: None,
            titles: None,
            message_context: None,
            thread_find_text: None,
            terminal_output: None,
            search_policy: None,
            launch: None,
            pull_requests: None,
            pull_request_identities: None,
            rollback_workspaces: None,
            finalization_observer: Arc::new(Unbound),
            sessions: SessionManagerOptions::default(),
            worker: WorkerOptions::default(),
        }
    }
}

/// A started runtime. Reads go through `transport`; commands through [`Self::dispatch_command`],
/// [`Self::launch_thread`] and [`Self::project_command`]; stop operations through `threads`.
pub struct OrchestrationRuntime {
    pub transport: Arc<TransportService>,
    pub intake: ThreadMessageIntake,
    /// Thread lifecycle: `stop_thread`, `interrupt_thread`, `send_to_thread`, `wait_for_thread`.
    pub threads: ThreadManagementService,
    /// `stop_workflow`, `force_stop_thread`.
    pub stop: ThreadStopService,
    /// Live provider sessions; `subscribe_prompt_suggestion` for prompt suggestions.
    pub sessions: ProviderSessionManager,
    pub projections: Arc<ProjectionStore>,
    pub events: EventSink,
    recovery: RecoverySummary,
    database: StoreDatabase,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    dispatcher: Dispatcher,
    layer: Arc<RuntimeLayer>,
    worker: OutboxWorker,
    worker_task: Mutex<Option<JoinHandle<Result<()>>>>,
    operations: Arc<ComposedThreads>,
    execution: Arc<RunExecutionService>,
    ingestor: Arc<ProviderEventIngestor>,
    launches: Option<ThreadLaunchService>,
    project_lane: tokio::sync::Mutex<()>,
    settlement: Arc<ThreadSettlementService>,
    watch: Option<Arc<PullRequestWatchReactor>>,
    /// Legacy V1 import: shells are reconciled before admission; the host drives
    /// `import_pending_transcripts` and its lifecycle events after start.
    pub legacy: Arc<LegacyV1ThreadImporter>,
    /// `orchestration.subscribeCommandOutput`; present when `terminal_output` was supplied.
    pub command_output: Option<crate::ports::transport::command_output_hub::CommandOutputHub>,
    /// `orchestration.searchThreads`; present when `search_policy` was supplied.
    pub thread_search: Option<Arc<crate::ports::transport::thread_search::ThreadSearch>>,
}

impl OrchestrationRuntime {
    /// Wires the runtime, recovers provider state left by the previous process, then starts the
    /// effect worker. Commands are admitted only after recovery succeeds.
    pub async fn start(ports: RuntimePorts) -> Result<Self> {
        let RuntimePorts {
            database,
            providers,
            checkpoints,
            clock,
            ids,
            settings,
            policy,
            attachments,
            cleanup,
            workspaces,
            agent_scope,
            telemetry,
            titles,
            message_context,
            thread_find_text,
            terminal_output,
            search_policy,
            launch,
            pull_requests,
            pull_request_identities,
            rollback_workspaces,
            finalization_observer,
            sessions: session_options,
            worker: worker_options,
        } = ports;
        let projections = Arc::new(match thread_find_text {
            Some(text) => ProjectionStore::new(database.clone()).with_thread_find_text(text),
            None => ProjectionStore::new(database.clone()),
        });
        let reader: Arc<dyn ProjectionReader> = projections.clone();
        let signals = OutboxSignals::default();
        let writer: Arc<dyn ProjectionWriter> = match pull_request_identities {
            Some(registry) => Arc::new(RegistryProjectionWriter::new(registry)),
            None => projections.clone(),
        };
        let sink = EventSink::new(database.clone(), writer, clock.clone(), signals.clone());
        let events: Arc<dyn EventCommitter> = Arc::new(sink.clone());
        let events_for_services = events.clone();
        let settings_for_services = settings.clone();
        let lanes = Arc::new(KeyedLanes::default());
        let mut ingestor = ProviderEventIngestor::new(
            reader.clone(),
            Arc::new(ProviderCommitter(sink.clone())),
            clock.clone(),
            ids.clone(),
            lanes.clone(),
        );
        ingestor.agent_scope = agent_scope.clone();
        ingestor.telemetry = telemetry;
        let ingestor = Arc::new(ingestor);
        // Run execution ingests run events; the manager would ingest every event again.
        let sessions = ProviderSessionManager::with_options(
            providers.clone(),
            None,
            session_options,
            Some(SessionServices {
                persistence: Some(SessionPersistence {
                    reader: reader.clone(),
                    sink: events.clone(),
                    clock: clock.clone(),
                    ids: ids.clone(),
                }),
                credentials: None,
            }),
        );
        let session_directory: Arc<dyn SessionDirectory> = Arc::new(sessions.clone());
        let recovery = Arc::new(ProviderRuntimeRecoveryService::new(
            database.clone(),
            reader.clone(),
            events.clone(),
            session_directory.clone(),
            clock.clone(),
            ids.clone(),
            signals.clone(),
        ));
        // Package 04's input loaders, outermost last: live sessions and runtime policy over the
        // projection reads, then provider settings, restart continuations and message context.
        let inputs: Arc<dyn CommandInputs> = Arc::new(RuntimeCommandInputs {
            base: ProjectionCommandInputs {
                reader: reader.clone(),
                providers: providers.clone(),
                clock: clock.clone(),
                ids: ids.clone(),
            },
            sessions: session_directory.clone(),
            policy: policy.clone(),
        });
        let inputs: Arc<dyn CommandInputs> = Arc::new(SettingsCommandInputs {
            base: inputs,
            settings: settings.clone(),
        });
        let inputs: Arc<dyn CommandInputs> = Arc::new(RestartCommandInputs {
            base: inputs,
            database: database.clone(),
        });
        let inputs: Arc<dyn CommandInputs> = match message_context {
            Some(preparer) => Arc::new(ContextCommandInputs {
                base: inputs,
                preparer,
            }),
            None => inputs,
        };
        let dispatcher = Dispatcher::with_command_lanes(
            DispatchServices {
                database: database.clone(),
                inputs,
                engine: Arc::new(PureDecisionEngine),
                committer: events.clone(),
                recovery: Arc::new(StartupRecovered(recovery.clone())),
                clock: clock.clone(),
            },
            None,
            lanes.clone(),
        );
        let raw_dispatcher: Arc<dyn CommandDispatcher> = Arc::new(dispatcher.clone());

        let checkpoint_backend: Arc<dyn CheckpointBackend> = checkpoints.clone();
        let checkpoint_metadata: Arc<dyn CheckpointMetadata> = checkpoints;
        let checkpoint_service = Arc::new(CheckpointService::new(
            checkpoint_backend.clone(),
            checkpoint_metadata,
            // Not the process-wide workspace lease: rollback holds that lease while it restores.
            Arc::new(WorkspaceLocks::default()),
        ));
        let execution = Arc::new(RunExecutionService::new(
            ingestor.clone(),
            Some(checkpoint_backend),
        ));
        let operations = Arc::new(ComposedThreads {
            projections: reader.clone(),
            policy: policy.clone(),
            turn_start: ProviderTurnStartService {
                projections: reader.clone(),
                committer: events.clone(),
                sessions: session_directory.clone(),
                execution: execution.clone(),
                clock: clock.clone(),
                ids: ids.clone(),
            },
            finalization: RunFinalizationService {
                capture: Arc::new(CheckpointCaptureService {
                    checkpoints: checkpoint_service.clone(),
                    projections: reader.clone(),
                    events: events.clone(),
                    ids: ids.clone(),
                    clock: clock.clone(),
                }),
                projections: reader.clone(),
                observer: finalization_observer,
            },
            management: OnceLock::new(),
            titles: OnceLock::new(),
            cancellation: CancellationToken::new(),
        });
        let rollback: Arc<dyn CheckpointRollback> = match rollback_workspaces {
            Some(workspace_recovery) => Arc::new(CheckpointRollbackService {
                command_lanes: lanes.clone(),
                checkpoints: checkpoint_service,
                projections: reader.clone(),
                sessions: session_directory.clone(),
                providers: providers.clone(),
                policy: Arc::new(ResolvedRollbackPolicy(policy)),
                workspace_recovery,
                events: events.clone(),
                database: database.clone(),
                ids: ids.clone(),
                clock: clock.clone(),
            }),
            None => Arc::new(Unbound),
        };
        let executor = Arc::new(ServiceExecutor {
            threads: operations.clone(),
            dispatcher: raw_dispatcher.clone(),
            projections: reader.clone(),
            sessions: session_directory.clone(),
            control: Arc::new(ProviderTurnControlService {
                projections: reader.clone(),
                sessions: session_directory.clone(),
                events: events.clone(),
            }),
            rollback,
            requests: Arc::new(Responder(RuntimeRequestService {
                projections: reader.clone(),
                sessions: session_directory.clone(),
            })),
            cleanup,
        });
        let worker = OutboxWorker::new(
            database.clone(),
            executor,
            clock.clone(),
            signals.clone(),
            worker_options,
        );
        let layer = Arc::new(RuntimeLayer::new(
            raw_dispatcher,
            recovery,
            Arc::new(worker.clone()),
            session_directory.clone(),
        ));
        // Client-facing commands pass the layer's admission gate; effect-driven ones do not.
        let threads = ThreadManagementService::new(ThreadManagementServices {
            reader: reader.clone(),
            dispatcher: layer.clone(),
            committer: events.clone(),
            database: database.clone(),
            settings: settings.clone(),
            hydrator: Arc::new(NoLegacyTranscript),
            signals,
            clock: clock.clone(),
        });
        let _ = operations.management.set(threads.clone());
        let stop = ThreadStopService::new(ThreadStopServices {
            management: threads.clone(),
            sessions: session_directory,
            agent_scope: agent_scope.unwrap_or_else(|| Arc::new(Unbound)),
            gate: lanes,
        });
        let _ = operations.titles.set(titles.map(|generator| {
            ThreadTitleRegenerationService::new(
                threads.clone(),
                database.clone(),
                settings,
                generator,
            )
        }));
        let launches = launch.map(|environment| {
            ThreadLaunchService::new(LaunchServices {
                database: database.clone(),
                projections: reader.clone(),
                events: events.clone(),
                commands: Arc::new(dispatcher.clone()),
                workspaces,
                environment,
                ids: ids.clone(),
                clock: clock.clone(),
            })
        });
        let intake = ThreadMessageIntake {
            threads: threads.clone(),
            launches: launches.clone(),
            attachments,
            projections: reader.clone(),
        };
        let command_output = terminal_output.map(|terminal| {
            crate::ports::transport::command_output_hub::CommandOutputHub::new(
                reader.clone(),
                events_for_services.clone(),
                terminal,
            )
        });
        let thread_search = search_policy.map(|policy| {
            Arc::new(crate::ports::transport::thread_search::ThreadSearch::new(
                database.clone(),
                policy,
            ))
        });
        let transport = Arc::new(TransportService::new(reader, events, database.clone()));

        let legacy = Arc::new(LegacyV1ThreadImporter::new(
            database.clone(),
            events_for_services.clone(),
            clock.clone(),
            ids.clone(),
        ));
        let settlement = Arc::new(ThreadSettlementService::new(ThreadSettlementServices {
            projections: projections.clone(),
            dispatcher: Arc::new(dispatcher.clone()),
            committer: events_for_services.clone(),
            settings: settings_for_services,
            clock: clock.clone(),
            ids: ids.clone(),
        }));
        let watch = pull_requests.map(|pull_requests| {
            Arc::new(PullRequestWatchReactor::new(PullRequestWatchServices {
                projections: projections.clone(),
                commands: Arc::new(dispatcher.clone()),
                pull_requests,
                clock: clock.clone(),
                ids: ids.clone(),
            }))
        });
        // TS start-up order: legacy shells, provider recovery, effect worker.
        legacy.reconcile_shells().await?;
        let recovery = layer.start().await?;
        let daemon = worker.clone();
        let worker_task = tokio::spawn(async move { daemon.run_daemon().await });
        settlement.start().await?;
        if let Some(watch) = &watch {
            watch.start().await?;
        }
        Ok(Self {
            transport,
            intake,
            threads,
            stop,
            sessions,
            projections,
            events: sink,
            recovery,
            database,
            clock,
            ids,
            dispatcher,
            layer,
            worker,
            worker_task: Mutex::new(Some(worker_task)),
            operations,
            execution,
            ingestor,
            launches,
            project_lane: tokio::sync::Mutex::new(()),
            settlement,
            watch,
            legacy,
            command_output,
            thread_search,
        })
    }

    /// The dispatcher effect-driven work uses (no admission gate); scenario harnesses wrap it.
    pub fn command_dispatcher(&self) -> Arc<dyn CommandDispatcher> {
        Arc::new(self.dispatcher.clone())
    }

    /// The live effect worker; `drain` awaits queued and executing effects.
    pub fn effect_worker(&self) -> Arc<dyn EffectWorker> {
        Arc::new(self.worker.clone())
    }

    /// What start-up recovery did to state the previous process left.
    pub fn recovery(&self) -> &RecoverySummary {
        &self.recovery
    }

    /// `orchestration.dispatchCommand` (TS `ThreadMessageIntake.dispatchCommand`).
    pub async fn dispatch_command(
        &self,
        command: OrchestrationV2ServerCommand,
    ) -> Result<CommandReceipt> {
        self.intake.dispatch_command(command).await
    }

    /// `orchestration.launchThread` (TS `ThreadMessageIntake.launchThread`).
    pub async fn launch_thread(&self, input: LaunchInput) -> Result<LaunchResult> {
        self.intake.launch_thread(input).await
    }

    /// Project create, update and delete: the commit TS `ProjectService` performs after
    /// planning with `ProjectCommands`. A planner rejection is `Error::Rejected` whose reason is
    /// the encoded `ProjectCommandRejection`.
    pub async fn project_command(&self, command: ProjectCommand) -> Result<CommandReceipt> {
        let value = serde_json::to_value(&command)?;
        let command_id: CommandId = serde_json::from_value(value["commandId"].clone())?;
        let project_id: ProjectId = serde_json::from_value(value["projectId"].clone())?;
        let command_type = value["type"].as_str().unwrap_or_default().to_owned();
        let workspace_root = value["workspaceRoot"].as_str().map(str::to_owned);
        // Planning reads must see every earlier project commit, including other projects'.
        let _lane = self.project_lane.lock().await;
        let id = project_id.clone();
        let (project, workspace_owner) = self
            .database
            .read(move |db| {
                let owner = match &workspace_root {
                    Some(root) => projects::find_active_by_workspace_root(db, root)?,
                    None => None,
                };
                Ok((projects::get(db, &id, false)?, owner))
            })
            .await?;
        let now = self.clock.now();
        let planned = plan_project_command(
            &command,
            ProjectCommandState {
                project: project.as_ref(),
                workspace_owner: workspace_owner.as_ref(),
            },
            EventId(self.ids.next(IdKind::Event)),
            &now,
            |text| text.graphemes(true).count(),
        )?;
        let event = match planned {
            Ok(event) => event,
            Err(rejection) => {
                return Err(Error::Rejected {
                    command_id,
                    reason: encode_project_command_rejection(&rejection)?,
                });
            }
        };
        self.dispatcher
            .commit_project(
                event,
                CommandReceipt {
                    command_id,
                    aggregate: AggregateId::Project(project_id),
                    command_type,
                    accepted_at: now.0,
                    result_sequence: 0,
                    status: ReceiptStatus::Accepted,
                    error: None,
                },
            )
            .await
    }

    /// Stops the settlement and watch producers and the client-facing services, then the runtime
    /// layer (admission, provider preparation, effect worker, sessions), then waits for run
    /// execution and pending commits. Returns the first failure after attempting every step.
    pub async fn shutdown(&self) -> Result<()> {
        if let Some(watch) = &self.watch {
            watch.shutdown().await;
        }
        self.settlement.shutdown().await;
        if let Some(launches) = &self.launches {
            launches.shutdown().await;
        }
        // Package 08's order puts the worker before these three; the runtime layer stops the
        // worker and the sessions together, so they close first and in-flight effects retry.
        if let Some(Some(titles)) = self.operations.titles.get() {
            titles.shutdown().await;
        }
        self.stop.shutdown().await;
        self.threads.shutdown().await;
        let mut failure = self.layer.shutdown().await.err();
        let task = self
            .worker_task
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(task) = task {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(error) => {
                    failure.get_or_insert(Error::Service {
                        operation: "OrchestrationRuntime.shutdown",
                        detail: error.to_string(),
                    });
                }
            }
        }
        self.operations.cancellation.cancel();
        if let Err(error) = self.execution.shutdown().await {
            failure.get_or_insert(error);
        }
        self.ingestor.shutdown().await;
        self.events.drain().await;
        failure.map_or(Ok(()), Err)
    }
}

/// The thread operations the effect worker routes to: turn start (06), run finalization and
/// checkpoint capture (06/07), title generation and delegated stops (08).
struct ComposedThreads {
    projections: Arc<dyn ProjectionReader>,
    policy: Arc<dyn RuntimePolicyResolver>,
    turn_start: ProviderTurnStartService,
    finalization: RunFinalizationService,
    // Set right after construction: thread management dispatches through the runtime layer,
    // which needs the worker that routes to these operations.
    management: OnceLock<ThreadManagementService>,
    titles: OnceLock<Option<ThreadTitleRegenerationService>>,
    cancellation: CancellationToken,
}

impl ComposedThreads {
    fn management(&self) -> Result<&ThreadManagementService> {
        self.management.get().ok_or(Error::Service {
            operation: "OrchestrationRuntime.threads",
            detail: "Thread management is not wired yet.".into(),
        })
    }
}

#[async_trait]
impl ThreadOperations for ComposedThreads {
    async fn start_with_options(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        options: EffectExecutionOptions,
    ) -> Result<()> {
        let thread = self
            .projections
            .thread(thread_id.clone())
            .await?
            .ok_or_else(|| Error::ThreadNotFound(thread_id.clone()))?;
        let model_selection = match self
            .projections
            .run(thread_id.clone(), run_id.clone())
            .await?
        {
            Some(run) => run.model_selection,
            None => thread.model_selection.clone(),
        };
        let policy = self.policy.resolve(thread, model_selection).await?;
        // The returned handle only reports settlement; the execution scope owns the run.
        self.turn_start
            .start(
                thread_id,
                run_id,
                policy.policy,
                options.will_retry,
                self.cancellation.child_token(),
            )
            .await?;
        Ok(())
    }
    async fn start(&self, thread_id: ThreadId, run_id: RunId) -> Result<()> {
        self.start_with_options(thread_id, run_id, EffectExecutionOptions::default())
            .await
    }
    async fn finalize(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        scope_id: CheckpointScopeId,
    ) -> Result<()> {
        self.finalization
            .finalize(
                FinalizationTarget {
                    thread_id,
                    run_id,
                    scope_id,
                },
                self.cancellation.child_token(),
            )
            .await
            .map_err(|error| *error.cause)
    }
    async fn continue_run(&self, _thread_id: ThreadId, _source_run_id: RunId) -> Result<()> {
        Err(Error::Unsupported("ThreadOperations.continue_run"))
    }
    async fn stop_delegated(&self, _thread_id: ThreadId, _reason: Option<String>) -> Result<()> {
        Err(Error::Unsupported(
            "ThreadOperations.stop_delegated without the effect's command id",
        ))
    }
    async fn generate_title(&self, _thread_id: ThreadId, _kind: TitleKind) -> Result<()> {
        Err(Error::Unsupported(
            "ThreadOperations.generate_title without the effect's command id",
        ))
    }
    async fn stop_delegated_for_effect(
        &self,
        thread_id: ThreadId,
        command_id: CommandId,
        reason: Option<String>,
    ) -> Result<()> {
        self.management()?
            .stop_delegated_tasks(thread_id, command_id, reason, None)
            .await
    }
    async fn generate_title_for_effect(
        &self,
        thread_id: ThreadId,
        command_id: CommandId,
        kind: TitleKind,
    ) -> Result<()> {
        let titles = self
            .titles
            .get()
            .and_then(Option::as_ref)
            .ok_or(Error::Unsupported("TitleGenerator"))?;
        titles
            .execute(thread_id, command_id, kind, self.cancellation.child_token())
            .await
    }
}

/// Recovery runs once in [`OrchestrationRuntime::start`], before admission; the dispatcher's
/// own first-dispatch recovery must not run it a second time on a live runtime.
struct StartupRecovered(Arc<ProviderRuntimeRecoveryService>);

#[async_trait]
impl RecoveryService for StartupRecovered {
    async fn recover(&self) -> Result<RecoverySummary> {
        Ok(RecoverySummary::default())
    }
    async fn reconcile(&self, thread_id: Option<ThreadId>) -> Result<RecoverySummary> {
        self.0.reconcile(thread_id).await
    }
    async fn prepare_for_shutdown(&self) -> Result<()> {
        self.0.prepare_for_shutdown().await
    }
}

struct Responder(RuntimeRequestService);

#[async_trait]
impl RuntimeRequestResponder for Responder {
    async fn respond(
        &self,
        thread_id: ThreadId,
        session_id: ProviderSessionId,
        response: RuntimeResponse,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        self.0
            .respond(thread_id, session_id, response)
            .await
            .map_err(|error| Error::RuntimeResponse(Box::new(error)))
    }
}

/// The thread's own runtime and interaction modes (`runtime_policy::resolve_base`), without the
/// project or managed-ceiling resolution package 04 owns. No reasoning-effort override.
pub struct BaseRuntimePolicy;

#[async_trait]
impl RuntimePolicyResolver for BaseRuntimePolicy {
    async fn resolve(
        &self,
        thread: OrchestrationV2AppThread,
        _model_selection: t3_contracts::model_selection::ModelSelection,
    ) -> Result<ConfiguredRuntimePolicy> {
        Ok(resolve_base(&thread).with_reasoning_effort(None))
    }
}

/// `<prefix>:<uuid v4>`, with the TS `IdAllocator` prefixes.
pub struct RandomIds;

impl IdGenerator for RandomIds {
    fn next(&self, kind: IdKind) -> String {
        let prefix = match kind {
            IdKind::Event => "event",
            IdKind::Command => "command",
            IdKind::Run => "run",
            IdKind::RunAttempt => "run-attempt",
            IdKind::Node => "node",
            IdKind::Message => "message",
            IdKind::ProviderSession => "provider-session",
            IdKind::ProviderThread => "provider-thread",
            IdKind::ProviderTurn => "provider-turn",
            IdKind::TurnItem => "turn-item",
            IdKind::Checkpoint => "checkpoint",
            IdKind::CheckpointScope => "checkpoint-scope",
            IdKind::ContextTransfer => "context-transfer",
            IdKind::ContextHandoff => "context-handoff",
            IdKind::RuntimeRequest => "runtime-request",
            IdKind::Thread => "thread",
        };
        format!("{prefix}:{}", uuid::Uuid::new_v4())
    }
}

/// Fixed settings, for hosts and tests without a settings runtime.
pub struct StaticSettings(watch::Sender<Arc<ServerSettings>>);

impl StaticSettings {
    pub fn new(settings: ServerSettings) -> Self {
        Self(watch::Sender::new(Arc::new(settings)))
    }
}

#[async_trait]
impl SettingsService for StaticSettings {
    async fn get(&self) -> Result<ServerSettings> {
        Ok(self.0.borrow().as_ref().clone())
    }
    fn subscribe(&self) -> Result<watch::Receiver<Arc<ServerSettings>>> {
        Ok(self.0.subscribe())
    }
}

/// Stand-in for a port the host has not supplied: every operation fails with `Unsupported`.
pub struct Unbound;

#[async_trait]
impl SettingsService for Unbound {}

#[async_trait]
impl CheckpointRollback for Unbound {}

#[async_trait]
impl AgentScope for Unbound {}

#[async_trait]
impl ResourceCleanup for Unbound {
    async fn cleanup(&self, _thread_id: ThreadId, _kind: ResourceKind) -> Result<()> {
        Err(Error::Unsupported("ResourceCleanup.cleanup"))
    }
}

#[async_trait]
impl FinalizationObserver for Unbound {
    async fn refresh(&self, _cwd: String, _thread_id: ThreadId, _run_id: RunId) -> Result<()> {
        Err(Error::Unsupported("FinalizationObserver.refresh"))
    }
}

#[async_trait]
impl AttachmentBackend for Unbound {
    async fn resolve(
        &self,
        _thread_id: ThreadId,
        _attachments: Vec<t3_contracts::chat_attachment::ChatAttachment>,
    ) -> Result<Vec<t3_contracts::chat_attachment::ChatAttachment>> {
        Err(Error::Unsupported("AttachmentBackend.resolve"))
    }
    async fn claim(&self, _thread_id: ThreadId, _attachment_ids: Vec<String>) -> Result<()> {
        Err(Error::Unsupported("AttachmentBackend.claim"))
    }
    async fn release(&self, _thread_id: ThreadId, _attachment_ids: Vec<String>) -> Result<()> {
        Err(Error::Unsupported("AttachmentBackend.release"))
    }
}

#[async_trait]
impl WorkspaceBackend for Unbound {
    async fn prepare(
        &self,
        _project_id: ProjectId,
        _thread_id: ThreadId,
        _strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        _cancellation: CancellationToken,
    ) -> Result<Workspace> {
        Err(Error::Unsupported("WorkspaceBackend.prepare"))
    }
    async fn remove(&self, _workspace: Workspace, _cancellation: CancellationToken) -> Result<()> {
        Err(Error::Unsupported("WorkspaceBackend.remove"))
    }
    async fn current_branch(&self, _cwd: PathBuf) -> Result<Option<String>> {
        Err(Error::Unsupported("WorkspaceBackend.current_branch"))
    }
}
