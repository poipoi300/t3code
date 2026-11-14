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
                ProviderSessionManager, SessionCredentials, SessionManagerOptions,
                SessionPersistence, SessionServices,
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
            CommandInputs, ContextCommandInputs, DispatchServices, DispatchThreadOperations,
            Dispatcher, ProjectionCommandInputs, RestartCommandInputs, RuntimeCommandInputs,
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
    /// Recreates a thread's missing worktree before a send (TS `restoreMissingWorktree`);
    /// without it a send to a thread whose worktree was deleted fails at turn start.
    pub worktree_restore: Option<Arc<dyn ThreadWorktreeRestore>>,
    /// MCP credentials for provider sessions (package 05's `SessionCredentials`, with
    /// `prepare_reserved` for managed leases); without it sessions start without them.
    pub credentials: Option<Arc<dyn SessionCredentials>>,
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
    /// The managed policy's runtime-mode ceiling for thread creation and mode changes; load it
    /// from the same `ManagedPolicyDocument` given to 04's `ProjectRuntimePolicy::configured`.
    pub runtime_mode_ceiling:
        Option<crate::ports::decisions::dispatch_mode_limit::RuntimeModeCeiling>,
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
            worktree_restore: None,
            credentials: None,
            telemetry: None,
            titles: None,
            message_context: None,
            thread_find_text: Some(Arc::new(t3_shared::find_text::NativeThreadFindText)),
            terminal_output: Some(Arc::new(
                t3_shared::terminal_output::NativeTerminalOutputFold,
            )),
            search_policy: Some(Arc::new(
                t3_shared::search_policy::NativeThreadSearchPolicy::default(),
            )),
            launch: None,
            pull_requests: None,
            pull_request_identities: None,
            rollback_workspaces: None,
            finalization_observer: Arc::new(Unbound),
            runtime_mode_ceiling: None,
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
    /// The dispatcher's thread lanes; a host service that mutates threads must share them.
    thread_lanes: Arc<KeyedLanes>,
    dispatcher: Dispatcher,
    layer: Arc<RuntimeLayer>,
    worker: OutboxWorker,
    worker_task: Mutex<Option<JoinHandle<Result<()>>>>,
    operations: Arc<ComposedThreads>,
    ingestor: Arc<ProviderEventIngestor>,
    launches: Option<ThreadLaunchService>,
    project_lane: tokio::sync::Mutex<()>,
    continuation_service: Arc<
        crate::ports::sessions_recovery::provider_continuation_service::ProviderContinuationService,
    >,
    continuation_task: Mutex<Option<JoinHandle<Result<()>>>>,
    delegated_recovery: crate::runtime::dispatch::DelegatedRecoveryReport,
    lifecycle: Lifecycle,
    settlement: Arc<ThreadSettlementService>,
    watch: Option<Arc<PullRequestWatchReactor>>,
    /// Legacy V1 import: shells are reconciled before admission; the host drives
    /// `import_pending_transcripts` and its lifecycle events after start.
    pub legacy: Arc<LegacyV1ThreadImporter>,
    /// `orchestration.getTurnDiff` and `getFullThreadDiff` (package 07).
    pub checkpoint_diffs:
        Arc<crate::ports::checkpoints_forks::checkpoint_diff_query::CheckpointDiffQuery>,
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
            worktree_restore,
            credentials,
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
            runtime_mode_ceiling,
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
        // The manager ingests only session-scoped artifacts (requests and recovered commands that
        // can arrive before a run subscribes); run events reach run execution alone.
        let sessions = ProviderSessionManager::with_options(
            providers.clone(),
            Some(ingestor.clone() as Arc<dyn EventIngestor>),
            session_options,
            Some(SessionServices {
                persistence: Some(SessionPersistence {
                    reader: reader.clone(),
                    sink: events.clone(),
                    clock: clock.clone(),
                    ids: ids.clone(),
                }),
                credentials,
            }),
        );
        let session_directory: Arc<dyn SessionDirectory> = Arc::new(sessions.clone());
        let recovery = Arc::new(
            ProviderRuntimeRecoveryService::new(
                database.clone(),
                reader.clone(),
                events.clone(),
                session_directory.clone(),
                clock.clone(),
                ids.clone(),
                signals.clone(),
            )
            .with_restart_settings(settings.clone()),
        );
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
            runtime_mode_ceiling,
            lanes.clone(),
        );
        let raw_dispatcher: Arc<dyn CommandDispatcher> = Arc::new(dispatcher.clone());

        let checkpoint_backend: Arc<dyn CheckpointBackend> = checkpoints.clone();
        let checkpoint_metadata: Arc<dyn CheckpointMetadata> = checkpoints;
        let checkpoint_diffs = Arc::new(
            crate::ports::checkpoints_forks::checkpoint_diff_query::CheckpointDiffQuery::new(
                reader.clone(),
                checkpoint_metadata.clone(),
            ),
        );
        let checkpoint_service = Arc::new(CheckpointService::new(
            checkpoint_backend.clone(),
            checkpoint_metadata,
            // Not the process-wide workspace lease: rollback holds that lease while it restores.
            Arc::new(WorkspaceLocks::default()),
        ));
        let execution = Arc::new(
            RunExecutionService::new(ingestor.clone(), Some(checkpoint_backend))
                .with_baseline_checkpoints(checkpoint_service.clone()),
        );
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
            restart: OnceLock::new(),
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
        let effect_operations: Arc<dyn ThreadOperations> = Arc::new(DispatchThreadOperations {
            base: operations.clone(),
            dispatcher: dispatcher.clone(),
        });
        let executor = Arc::new(ServiceExecutor {
            threads: effect_operations.clone(),
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
        let layer = Arc::new(
            RuntimeLayer::new(
                raw_dispatcher,
                recovery,
                Arc::new(worker.clone()),
                session_directory.clone(),
            )
            .with_run_execution(execution.clone()),
        );
        // Client-facing commands pass the layer's admission gate; effect-driven ones do not.
        let threads = ThreadManagementService::new(ThreadManagementServices {
            reader: reader.clone(),
            dispatcher: layer.clone(),
            committer: events.clone(),
            database: database.clone(),
            settings: settings.clone(),
            hydrator: Arc::new(NoLegacyTranscript),
            worktree_restore: worktree_restore.unwrap_or_else(|| Arc::new(NoWorktreeRestore)),
            signals,
            clock: clock.clone(),
        });
        let _ = operations.management.set(threads.clone());
        let stop = ThreadStopService::new(ThreadStopServices {
            management: threads.clone(),
            sessions: session_directory,
            agent_scope: agent_scope.unwrap_or_else(|| Arc::new(Unbound)),
            gate: lanes.clone(),
        });
        let _ = operations.restart.set(
            crate::ports::runs_ingestion::restart_continuation::RestartContinuationService {
                threads: threads.clone(),
                settings: settings.clone(),
                operations: effect_operations,
                clock: clock.clone(),
                dispatch_policy: None,
            },
        );
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
        // After provider recovery and before admission (package 04): continuation delivery,
        // then the delegated-task recovery sweeps, then the committed-event consumers.
        let continuations = dispatcher.prepare_continuations(
            projections.clone(),
            ids.clone(),
            std::num::NonZeroUsize::new(64).unwrap_or(std::num::NonZeroUsize::MIN),
        )?;
        let continuation_service = continuations.service;
        let runner = continuation_service.clone();
        let requests = continuations.requests;
        let continuation_task = tokio::spawn(async move { runner.run(requests).await });
        let delegated_recovery = dispatcher.recover_delegated_tasks().await;
        let lifecycle = Lifecycle::start(&sink, &dispatcher).await?;
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
            thread_lanes: lanes,
            dispatcher,
            layer,
            worker,
            worker_task: Mutex::new(Some(worker_task)),
            operations,
            ingestor,
            launches,
            project_lane: tokio::sync::Mutex::new(()),
            continuation_service,
            continuation_task: Mutex::new(Some(continuation_task)),
            delegated_recovery,
            lifecycle,
            settlement,
            watch,
            legacy,
            checkpoint_diffs,
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

    /// The dispatcher's thread lanes, for host services (such as project deletion) whose
    /// cascades must not race thread commands.
    /// The dispatcher's thread lanes, for host services (`ProjectService`) that must serialize
    /// with thread commands.
    pub fn thread_lanes(&self) -> Arc<KeyedLanes> {
        self.thread_lanes.clone()
    }

    /// The runtime's clock and id generator, for host services that commit alongside it.
    /// The clock and id generator the runtime was started with.
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }
    pub fn ids(&self) -> Arc<dyn IdGenerator> {
        self.ids.clone()
    }

    /// The start-up delegated-task recovery sweeps; the host logs each failure.
    pub fn delegated_recovery(&self) -> &crate::runtime::dispatch::DelegatedRecoveryReport {
        &self.delegated_recovery
    }

    /// Failures of the committed-event consumers since start (most recent last, at most 64);
    /// each consumer keeps running after one.
    pub fn lifecycle_failures(&self) -> Vec<(&'static str, String)> {
        self.lifecycle.failures()
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

    /// `orchestration.stopWorkflow`. Failures the RPC types are `Error::StopWorkflow`; anything
    /// else is a defect, as in TS.
    pub async fn stop_workflow(&self, input: OrchestrationV2StopWorkflowInput) -> Result<()> {
        let thread_id = input.thread_id.clone();
        let subagent_id = input.subagent_id.clone();
        self.stop
            .stop_workflow(input, None)
            .await
            .map_err(|error| typed_stop_workflow_error(error, thread_id, subagent_id))
    }

    /// `orchestration.forceStopThread`. Failures the RPC types are `Error::ForceStopThread`.
    pub async fn force_stop_thread(
        &self,
        input: OrchestrationV2ForceStopThreadInput,
    ) -> Result<OrchestrationV2ForceStopThreadResult> {
        let thread_id = input.thread_id.clone();
        self.stop
            .force_stop_thread(input)
            .await
            .map_err(|error| typed_force_stop_error(error, thread_id))
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

    /// Closes admission, stops the settlement, watch and launch producers, stops the effect worker,
    /// then the title, stop and thread-management services, then run execution, provider sessions
    /// and recovery reconciliation, then drains ingestion and pending commits. Returns the first
    /// failure after attempting every step.
    pub async fn shutdown(&self) -> Result<()> {
        // Package 05 and 08's order: close admission, stop producers, stop the worker, stop the
        // services its effects use, then sessions (with run execution) and pending commits.
        self.layer.close_admission().await;
        if let Some(watch) = &self.watch {
            watch.shutdown().await;
        }
        self.settlement.shutdown().await;
        if let Some(launches) = &self.launches {
            launches.shutdown().await;
        }
        self.lifecycle.shutdown().await;
        self.continuation_service.shutdown().await;
        let mut failure = None;
        let continuation = self
            .continuation_task
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(task) = continuation {
            join_task(task, &mut failure).await;
        }
        if let Err(error) = self.layer.shutdown_worker().await {
            failure.get_or_insert(error);
        }
        let task = self
            .worker_task
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(task) = task {
            join_task(task, &mut failure).await;
        }
        if let Some(Some(titles)) = self.operations.titles.get() {
            titles.shutdown().await;
        }
        self.stop.shutdown().await;
        self.threads.shutdown().await;
        self.operations.cancellation.cancel();
        if let Err(error) = self.layer.shutdown_sessions().await {
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
    restart:
        OnceLock<crate::ports::runs_ingestion::restart_continuation::RestartContinuationService>,
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
    async fn continue_run(&self, thread_id: ThreadId, source_run_id: RunId) -> Result<()> {
        self.restart
            .get()
            .ok_or(Error::Service {
                operation: "OrchestrationRuntime.threads",
                detail: "Restart continuation is not wired yet.".into(),
            })?
            .continue_restarted_run(thread_id, source_run_id)
            .await
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

async fn join_task(task: JoinHandle<Result<()>>, failure: &mut Option<Error>) {
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

/// Committed-event consumers feeding the dispatcher (package 04): run updates drive retries,
/// queue drain and delivery finalization; provider-turn updates release deferred steers;
/// subagent and provider-thread updates track holding rosters and terminal child work. Each
/// reads the sink's durable stream from the sequence current at start, so none is missed.
struct Lifecycle {
    cancellation: CancellationToken,
    tasks: tokio_util::task::TaskTracker,
    failures: Arc<Mutex<std::collections::VecDeque<(&'static str, String)>>>,
}

impl Lifecycle {
    async fn start(sink: &EventSink, dispatcher: &Dispatcher) -> Result<Self> {
        let after = sink.latest_sequence(None).await?;
        let lifecycle = Self {
            cancellation: CancellationToken::new(),
            tasks: tokio_util::task::TaskTracker::new(),
            failures: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        };
        for event_type in [
            "run.updated",
            "provider-turn.updated",
            "subagent.updated",
            "provider-thread.updated",
        ] {
            let mut stream = sink
                .stream(crate::runtime::commit::StreamOptions {
                    thread_id: None,
                    after_sequence: after,
                    event_type: Some(event_type.into()),
                    bounded: false,
                })
                .await?;
            let dispatcher = dispatcher.clone();
            let cancellation = lifecycle.cancellation.clone();
            let failures = lifecycle.failures.clone();
            lifecycle.tasks.spawn(async move {
                loop {
                    let event = tokio::select! {
                        _ = cancellation.cancelled() => return,
                        event = stream.next() => event,
                    };
                    let result = match event {
                        Ok(Some(event)) => match event_type {
                            "run.updated" => dispatcher.handle_run_update(&event).await,
                            "provider-turn.updated" => {
                                dispatcher.handle_provider_turn_update(&event).await
                            }
                            _ => dispatcher.handle_child_work_update(&event).await,
                        },
                        Ok(None) => return,
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result {
                        let mut failures = failures.lock().unwrap_or_else(|e| e.into_inner());
                        if failures.len() == 64 {
                            failures.pop_front();
                        }
                        failures.push_back((event_type, error.to_string()));
                    }
                }
            });
        }
        lifecycle.tasks.close();
        Ok(lifecycle)
    }

    fn failures(&self) -> Vec<(&'static str, String)> {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    async fn shutdown(&self) {
        self.cancellation.cancel();
        self.tasks.wait().await;
    }
}

// Package 08 encodes the TS failure reason in its stop errors' static operation code; once it
// returns `Error::StopWorkflow` / `Error::ForceStopThread` itself these pass through unchanged.
fn typed_stop_workflow_error(error: Error, thread_id: ThreadId, subagent_id: NodeId) -> Error {
    let Error::Service { operation, detail } = &error else {
        return error;
    };
    let reason = match *operation {
        "StopWorkflow.not-running" => StopWorkflowErrorReason::NotRunning,
        "StopWorkflow.unsupported" => StopWorkflowErrorReason::Unsupported,
        "StopWorkflow.unavailable" => StopWorkflowErrorReason::Unavailable,
        "StopWorkflow.stop-failed" => StopWorkflowErrorReason::StopFailed,
        _ => return error,
    };
    Error::StopWorkflow(Box::new(OrchestrationV2StopWorkflowError {
        _tag: StopWorkflowErrorTag::OrchestrationV2StopWorkflowError,
        thread_id,
        subagent_id,
        reason,
        cause: Some(Some(serde_json::Value::String(detail.clone()))),
    }))
}

fn typed_force_stop_error(error: Error, thread_id: ThreadId) -> Error {
    let Error::Service { operation, detail } = &error else {
        return error;
    };
    let reason = match *operation {
        "ForceStop.superseded" => Some(Some(ForceStopThreadErrorReason::Superseded)),
        "ForceStop.clock" | "ForceStop.release" => None,
        _ => return error,
    };
    Error::ForceStopThread(Box::new(OrchestrationV2ForceStopThreadError {
        _tag: ForceStopThreadErrorTag::OrchestrationV2ForceStopThreadError,
        thread_id,
        reason,
        cause: Some(Some(serde_json::Value::String(detail.clone()))),
    }))
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

/// No worktree recreation before a send (TS's test layer): a missing worktree then fails the
/// turn start on its own, as it did before upstream added the restore.
pub struct NoWorktreeRestore;

#[async_trait]
impl ThreadWorktreeRestore for NoWorktreeRestore {
    async fn restore(&self, _thread: OrchestrationV2AppThread) -> Result<()> {
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn service(operation: &'static str) -> Error {
        Error::Service {
            operation,
            detail: "detail".into(),
        }
    }

    #[test]
    fn stop_failures_carry_the_wire_reason_and_other_errors_pass_through() {
        let thread = || ThreadId("thread".into());
        let node = || NodeId("node".into());
        for (operation, reason) in [
            (
                "StopWorkflow.not-running",
                StopWorkflowErrorReason::NotRunning,
            ),
            (
                "StopWorkflow.unsupported",
                StopWorkflowErrorReason::Unsupported,
            ),
            (
                "StopWorkflow.unavailable",
                StopWorkflowErrorReason::Unavailable,
            ),
            (
                "StopWorkflow.stop-failed",
                StopWorkflowErrorReason::StopFailed,
            ),
        ] {
            let Error::StopWorkflow(error) =
                typed_stop_workflow_error(service(operation), thread(), node())
            else {
                panic!("{operation} was not typed");
            };
            assert_eq!(error.reason, reason);
            assert_eq!(error.subagent_id, node());
        }
        assert!(matches!(
            typed_stop_workflow_error(service("Dispatch.failed"), thread(), node()),
            Error::Service { .. }
        ));
        let Error::ForceStopThread(error) =
            typed_force_stop_error(service("ForceStop.superseded"), thread())
        else {
            panic!("superseded was not typed");
        };
        assert_eq!(
            error.reason,
            Some(Some(ForceStopThreadErrorReason::Superseded))
        );
        let Error::ForceStopThread(error) =
            typed_force_stop_error(service("ForceStop.release"), thread())
        else {
            panic!("release was not typed");
        };
        assert_eq!(error.reason, None);
        assert!(matches!(
            typed_force_stop_error(Error::Cancelled, thread()),
            Error::Cancelled
        ));
    }
}
