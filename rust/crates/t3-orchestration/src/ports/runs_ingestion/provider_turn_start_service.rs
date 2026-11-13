use super::{
    provider_event_ingestor::{derive_id, expanded_id},
    run_execution_service::{RootRunState, RunExecutionHandle, RunExecutionService, StartRootRun},
};
use crate::{
    api::*,
    error::{Error, Result},
    provider::*,
};
use serde_json::json;
use std::sync::Arc;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use tokio_util::sync::CancellationToken;

pub struct ProviderTurnStartService {
    pub projections: Arc<dyn ProjectionReader>,
    pub committer: Arc<dyn EventCommitter>,
    pub sessions: Arc<dyn SessionDirectory>,
    pub execution: Arc<RunExecutionService>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

impl ProviderTurnStartService {
    pub async fn start(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        runtime_policy: RuntimePolicy,
        will_retry: bool,
        cancellation: CancellationToken,
    ) -> Result<Option<RunExecutionHandle>> {
        self.start_configured(
            thread_id,
            run_id,
            ConfiguredRuntimePolicy {
                policy: runtime_policy,
                options: RuntimePolicyOptions::default(),
            },
            will_retry,
            cancellation,
        )
        .await
    }

    pub async fn start_configured(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        configured: ConfiguredRuntimePolicy,
        will_retry: bool,
        cancellation: CancellationToken,
    ) -> Result<Option<RunExecutionHandle>> {
        let ConfiguredRuntimePolicy {
            policy: runtime_policy,
            options: policy_options,
        } = configured;
        let context = self
            .projections
            .turn_start_context(thread_id.clone(), run_id.clone())
            .await?;
        let projection = context.projection;
        let Some(run) = projection
            .runs
            .iter()
            .find(|run| run.id == IdOrIndex::Id(run_id.clone()))
            .cloned()
        else {
            return Ok(None);
        };
        if run.status != OrchestrationV2RunStatus::Starting {
            return Ok(None);
        }
        let missing = || start_error(&run_id, "Run is missing its execution projection state.");
        let root_node = projection
            .nodes
            .iter()
            .find(|node| Some(&node.id) == run.root_node_id.as_ref())
            .cloned()
            .ok_or_else(missing)?;
        let attempt = projection
            .attempts
            .iter()
            .find(|attempt| Some(&attempt.id) == run.active_attempt_id.as_ref())
            .cloned()
            .ok_or_else(missing)?;
        let provider_thread = projection
            .provider_threads
            .iter()
            .find(|thread| Some(&thread.id) == run.provider_thread_id.as_ref())
            .cloned()
            .ok_or_else(missing)?;
        let message = projection
            .messages
            .iter()
            .find(|message| message.id == run.user_message_id)
            .ok_or_else(missing)?;
        let session_id = provider_thread
            .provider_session_id
            .as_ref()
            .map(expanded_id)
            .transpose()?
            .ok_or_else(missing)?;
        let scope = projection
            .checkpoint_scopes
            .iter()
            .find(|scope| Some(&scope.id) == root_node.checkpoint_scope_id.as_ref())
            .cloned()
            .ok_or_else(missing)?;
        if run.thread_id != IdOrIndex::Id(thread_id.clone())
            || root_node.thread_id != run.thread_id
            || root_node.run_id.as_ref() != Some(&run.id)
            || attempt.run_id != run.id
            || attempt.root_node_id != root_node.id
            || attempt.provider_thread_id != provider_thread.id
            || message.thread_id != run.thread_id
        {
            return Err(missing());
        }
        let state = RootRunState {
            run,
            root_node,
            attempt,
            provider_thread,
            checkpoint_scope: scope,
        };
        if !context.has_conversation
            && message.attachments.is_empty()
            && t3_contracts::wire::trim_js(&message.text).eq_ignore_ascii_case("/compact")
        {
            return self.empty_compaction(&state).await;
        }
        let prepared_message = turn_message(message)?;
        let recovery = match self.projections.runtime_recovery(thread_id.clone()).await {
            Ok(recovery) => recovery,
            Err(error) => {
                return self
                    .failed(
                        &state,
                        "runtime-recovery-read-failure",
                        "Couldn't read thread state",
                        error,
                        will_retry,
                    )
                    .await;
            }
        };
        let runs = projection.runs;
        let turns = projection.provider_turns;
        let mut related_threads = Vec::new();
        let mut related_providers = Vec::new();
        for agent in projection
            .subagents
            .into_iter()
            .filter(|agent| agent.status == OrchestrationV2TurnItemStatus::Completed)
        {
            if let Some(id) = agent.child_thread_id {
                related_threads.push(expanded_id(&id)?);
            }
            if let Some(id) = agent.provider_thread_id {
                related_providers.push(expanded_id(&id)?);
            }
        }
        if projection.context_handoffs.iter().any(|handoff| {
            handoff.status == ContextHandoffStatus::Ready
                && handoff.delivery.as_ref().and_then(Option::as_ref).is_none()
        }) {
            // seam-shim: ContextHandoffService delivery is composed after its package-owned port.
            return Err(Error::Unsupported(
                "ProviderTurnStartService.context_handoff_delivery",
            ));
        }
        let native_fork_transfer = projection
            .context_transfers
            .iter()
            .map(t3_contracts::stored::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .find(|transfer| {
                transfer["type"] == "fork"
                    && transfer["targetThreadId"] == json!(thread_id)
                    && transfer["targetRunId"] == json!(state.run.id)
                    && transfer["status"] == "pending"
                    && transfer["resolution"].is_null()
            });
        let inherited = super::run_execution_service::select_inherited_background_turn_items(
            &thread_id,
            &expanded_id(&state.provider_thread.id)?,
            state.run.ordinal,
            &recovery.runs,
            &recovery.turn_items,
        )?;
        let existing_session = projection
            .provider_sessions
            .into_iter()
            .find(|session| session.id == IdOrIndex::Id(session_id.clone()));
        if !self.current(&state).await? {
            return Ok(None);
        }
        let abandoned: Vec<_> = turns
            .iter()
            .filter(|turn| {
                turn.provider_thread_id == state.provider_thread.id
                    && turn.status == ProviderTurnStatus::Running
            })
            .filter_map(|turn| {
                runs.iter()
                    .find(|previous| {
                        previous.ordinal < state.run.ordinal
                            && previous.status == OrchestrationV2RunStatus::Failed
                            && previous.provider_thread_id == state.run.provider_thread_id
                            && turn.run_attempt_id == previous.active_attempt_id
                            && Some(&turn.node_id) == previous.root_node_id.as_ref()
                    })
                    .map(|previous| (turn, previous))
            })
            .collect();
        if let Some((last, _)) = abandoned.last() {
            if let Some(runtime) = self.sessions.get(session_id.clone()).await?
                && let Err(error) = runtime
                    .interrupt_turn(InterruptInput {
                        provider_thread: state.provider_thread.clone(),
                        provider_turn_id: expanded_id(&last.id)?,
                        request_runtime_restart: true,
                    })
                    .await
            {
                return self
                    .failed(
                        &state,
                        "provider-turn-recovery-failure",
                        "Provider turn recovery failed",
                        error.into(),
                        will_retry,
                    )
                    .await;
            }
            let now = self.clock.now();
            let mut events = Vec::new();
            for (turn, previous) in abandoned {
                let mut turn = turn.clone();
                turn.status = ProviderTurnStatus::Interrupted;
                turn.completed_at = Some(now.clone());
                events.push(crate::ports::checkpoints_forks::checkpoint_capture_service::domain_event(
                    self.ids.as_ref(), &thread_id, &now, "provider-turn.updated", turn.clone(),
                    json!({"runId":previous.id,"nodeId":turn.node_id,"providerInstanceId":previous.provider_instance_id}),
                )?);
            }
            if !self.commit(&state, events).await? {
                return Ok(None);
            }
        }
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let opened = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::Cancelled),
            result = async { let open = OpenSession {
                thread_id: thread_id.clone(), provider_session_id: session_id.clone(),
                model_selection: state.run.model_selection.clone(), runtime_policy: runtime_policy.clone(),
                resume_from_session: existing_session, prompt_suggestions: None,
                initial_native_thread_id: state.provider_thread.native_thread_ref.as_ref().and_then(|reference| reference.native_id.as_ref().map(|id| id.0.clone())),
                initial_provider_item_identity_version: state.provider_thread.native_metadata.as_ref().and_then(Option::as_ref)
                    .and_then(|metadata| metadata.item_identity_version.as_ref()).and_then(Option::as_ref)
                    .map(|version| u64::from(version.clone()) as u8),
            }; if policy_options.reasoning_effort.is_some() {
                self.sessions.attach_with_policy(expanded_id(&state.run.provider_instance_id)?, open, policy_options.clone()).await
            } else { self.sessions.attach(expanded_id(&state.run.provider_instance_id)?, open).await } } => result,
        };
        let runtime = match opened {
            Ok(runtime) => runtime,
            Err(error) => {
                return self
                    .failed(
                        &state,
                        "provider-session-open-failure",
                        "Provider session failed to open",
                        error,
                        will_retry,
                    )
                    .await;
            }
        };
        if !self.current(&state).await? {
            return Ok(None);
        }
        let native_fork = match &native_fork_transfer {
            Some(transfer) => Some(
                self.native_fork_input(transfer, &state, runtime_policy.clone())
                    .await?,
            ),
            None => None,
        };
        let loaded = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::Cancelled),
            result = async {
                if let Some(fork)=native_fork {
                    if policy_options.reasoning_effort.is_some(){runtime.fork_with_policy(fork,policy_options.clone()).await}else{runtime.fork(fork).await}
                } else if state.provider_thread.native_thread_ref.is_some() {
                    let resume = ResumeThread {
                        provider_thread: state.provider_thread.clone(), thread_id: Some(thread_id.clone()),
                        model_selection: Some(state.run.model_selection.clone()), runtime_policy: Some(runtime_policy.clone()),
                    };
                    if policy_options.reasoning_effort.is_some() {runtime.resume_thread_with_policy(resume,policy_options.clone()).await}else{runtime.resume_thread(resume).await}
                } else {
                    let ensure = EnsureThread {
                        thread_id: thread_id.clone(), title: Some(projection.thread.title.0.clone()),
                        model_selection: state.run.model_selection.clone(), runtime_policy: runtime_policy.clone(),
                        provider_session_id: Some(session_id.clone()), existing_provider_thread: Some(state.provider_thread.clone()),
                        force_new_native_thread: false,
                    };
                    if policy_options.reasoning_effort.is_some() {runtime.ensure_thread_with_policy(ensure,policy_options.clone()).await}else{runtime.ensure_thread(ensure).await}
                }
            } => result,
        };
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                return self
                    .failed(
                        &state,
                        "provider-thread-load-failure",
                        "Provider turn failed to start",
                        error.into(),
                        will_retry,
                    )
                    .await;
            }
        };
        let now = self.clock.now();
        let mut running = state.clone();
        running.run.status = OrchestrationV2RunStatus::Running;
        running.run.started_at = Some(now.clone());
        running.attempt.status = RunAttemptStatus::Running;
        running.attempt.started_at = Some(now.clone());
        if let Some(native) = loaded
            .native_thread_ref
            .as_ref()
            .and_then(|reference| reference.native_id.as_ref())
        {
            running.attempt.native_thread_id = Some(Some(IdOrIndex::Id(native.0.clone())));
        }
        running.root_node.status = ExecutionNodeStatus::Running;
        running.root_node.started_at = Some(now.clone());
        running.provider_thread = loaded;
        running.provider_thread.id = state.provider_thread.id.clone();
        running.provider_thread.driver = runtime.session().driver.clone();
        running.provider_thread.provider_instance_id = state.run.provider_instance_id.clone();
        running.provider_thread.provider_session_id = Some(IdOrIndex::Id(session_id));
        running.provider_thread.app_thread_id = Some(IdOrIndex::Id(thread_id.clone()));
        running.provider_thread.owner_node_id = state.provider_thread.owner_node_id.clone();
        running.provider_thread.first_run_ordinal = state
            .provider_thread
            .first_run_ordinal
            .or(Some(state.run.ordinal));
        running.provider_thread.last_run_ordinal = Some(state.run.ordinal);
        running.provider_thread.handoff_ids = state.provider_thread.handoff_ids.clone();
        running.provider_thread.forked_from = state.provider_thread.forked_from.clone();
        running.provider_thread.created_at = state.provider_thread.created_at.clone();
        running.provider_thread.updated_at = now.clone();
        running.provider_thread.status = ProviderThreadStatus::Active;
        let mut events: Vec<_> = [
            (
                "provider-session.updated",
                t3_contracts::stored::to_value(runtime.session())?,
            ),
            (
                "provider-thread.updated",
                t3_contracts::stored::to_value(&running.provider_thread)?,
            ),
            ("run.updated", t3_contracts::stored::to_value(&running.run)?),
            (
                "run-attempt.updated",
                t3_contracts::stored::to_value(&running.attempt)?,
            ),
            (
                "node.updated",
                t3_contracts::stored::to_value(&running.root_node)?,
            ),
        ]
        .into_iter()
        .map(|(kind, payload)| self.event(&state, kind, payload, &now))
        .collect::<Result<_>>()?;
        if let Some(mut transfer) = native_fork_transfer
            && let Some(native) = &running.provider_thread.native_thread_ref
        {
            transfer["targetProviderInstanceId"] =
                t3_contracts::stored::to_value(&state.run.provider_instance_id)?;
            transfer["targetRunId"] = t3_contracts::stored::to_value(&state.run.id)?;
            transfer["status"] = "consumed".into();
            transfer["resolution"] = json!({"strategy":"native_fork","providerThreadRef":t3_contracts::stored::to_value(native)?});
            transfer["error"] = serde_json::Value::Null;
            transfer["updatedAt"] = json!(now);
            transfer["consumedAt"] = json!(now);
            events.push(self.event(&state, "context-transfer.updated", transfer, &now)?);
        }
        if !self.commit(&state, events).await? {
            return Ok(None);
        }
        let ordinal = turns
            .iter()
            .filter(|turn| turn.provider_thread_id == state.provider_thread.id)
            .map(|turn| turn.ordinal)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| start_error(&run_id, "Provider turn ordinal overflow."))?;
        let native_has_turns = turns
            .iter()
            .any(|turn| turn.provider_thread_id == state.provider_thread.id);
        self.execution
            .start_root_run_with_policy(
                StartRootRun {
                    state: running,
                    turn: TurnInput {
                        app_thread: projection.thread,
                        thread_id,
                        run_id,
                        run_ordinal: state.run.ordinal,
                        native_thread_has_turns: Some(native_has_turns),
                        provider_turn_ordinal: ordinal,
                        restart_continuation_of_run_id: state
                            .run
                            .restart_continuation_of_run_id
                            .clone()
                            .flatten()
                            .map(|id| expanded_id(&id))
                            .transpose()?,
                        attempt_id: expanded_id(&state.attempt.id)?,
                        root_node_id: expanded_id(&state.root_node.id)?,
                        provider_thread: state.provider_thread,
                        message: prepared_message,
                        model_selection: state.run.model_selection,
                        runtime_policy,
                        app_context: Vec::new(),
                    },
                    session: runtime,
                    inherited_background_items: inherited,
                    related_thread_ids: related_threads,
                    related_provider_thread_ids: related_providers,
                },
                policy_options,
                cancellation,
            )
            .await
            .map(Some)
    }

    async fn native_fork_input(
        &self,
        transfer: &serde_json::Value,
        state: &RootRunState,
        policy: RuntimePolicy,
    ) -> Result<ForkInput> {
        let source = self
            .projections
            .records(
                serde_json::from_value(transfer["sourceThreadId"].clone())?,
                vec![
                    RecordKind::Runs,
                    RecordKind::ProviderThreads,
                    RecordKind::Attempts,
                    RecordKind::ProviderTurns,
                ],
                RecordFilter::default(),
            )
            .await?;
        let mut runs = vec![];
        let mut providers = vec![];
        let mut attempts = vec![];
        let mut turns = vec![];
        for records in source.records {
            match records {
                ProjectionRecordSet::Runs(value) => runs = value,
                ProjectionRecordSet::ProviderThreads(value) => providers = value,
                ProjectionRecordSet::Attempts(value) => attempts = value,
                ProjectionRecordSet::ProviderTurns(value) => turns = value,
                _ => {}
            }
        }
        let source_run_id: RunId =
            serde_json::from_value(transfer["sourcePoint"]["runId"].clone())?;
        let target_run_id = expanded_id(&state.run.id)?;
        let source_run = runs
            .iter()
            .find(|run| run.id == IdOrIndex::Id(source_run_id.clone()))
            .ok_or_else(|| {
                start_error(
                    &target_run_id,
                    "Native fork transfer has no source provider execution.",
                )
            })?;
        if runs.iter().any(|run| {
            run.ordinal > source_run.ordinal
                && matches!(
                    run.status,
                    OrchestrationV2RunStatus::Preparing
                        | OrchestrationV2RunStatus::Starting
                        | OrchestrationV2RunStatus::Running
                )
        }) {
            // seam-shim: busy native forks require the provider-core budgeted portable fallback.
            return Err(Error::Unsupported(
                "ProviderTurnStartService.portable_fork_handoff_budget",
            ));
        }
        let provider = providers
            .into_iter()
            .find(|provider| Some(&provider.id) == source_run.provider_thread_id.as_ref())
            .ok_or_else(|| {
                start_error(
                    &source_run_id,
                    "Native fork transfer has no source provider execution.",
                )
            })?;
        let attempt = attempts
            .iter()
            .find(|attempt| Some(&attempt.id) == source_run.active_attempt_id.as_ref());
        let turn = attempt.and_then(|attempt| {
            turns
                .iter()
                .filter(|turn| turn.run_attempt_id.as_ref() == Some(&attempt.id))
                .max_by_key(|turn| turn.ordinal)
                .or_else(|| {
                    turns
                        .iter()
                        .find(|turn| Some(&turn.id) == attempt.provider_turn_id.as_ref())
                })
        });
        let provider_turn_id = turn.map(|turn| expanded_id(&turn.id)).transpose()?;
        let rolled_back = attempts
            .iter()
            .filter(|attempt| {
                runs.iter().any(|run| {
                    run.id == attempt.run_id && run.status == OrchestrationV2RunStatus::RolledBack
                })
            })
            .map(|attempt| &attempt.id)
            .collect::<Vec<_>>();
        let turns = turns
            .into_iter()
            .filter(|turn| {
                turn.run_attempt_id
                    .as_ref()
                    .is_none_or(|id| !rolled_back.contains(&id))
            })
            .collect();
        Ok(ForkInput {
            source_provider_thread: provider,
            source_provider_turns: Some(turns),
            provider_turn_id,
            target_thread_id: expanded_id(&state.run.thread_id)?,
            owner_node_id: None,
            model_selection: Some(state.run.model_selection.clone()),
            runtime_policy: Some(policy),
        })
    }

    async fn empty_compaction(&self, state: &RootRunState) -> Result<Option<RunExecutionHandle>> {
        let now = self.clock.now();
        let run_id = expanded_id(&state.run.id)?;
        let ordinal = self
            .projections
            .next_turn_item_ordinal(expanded_id(&state.run.thread_id)?, Some(run_id.clone()))
            .await?;
        let item = json!({"id":derive_id("turn-item", &["run", &run_id.0,"signal","empty-compaction"]),
            "threadId":state.run.thread_id,"runId":run_id,"nodeId":state.root_node.id,"providerThreadId":state.provider_thread.id,
            "providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"failed",
            "title":"Cannot compact an empty thread","startedAt":now,"completedAt":now,"updatedAt":now,"type":"error",
            "failure":{"class":"validation_error","message":"Start a conversation before compacting this thread.","code":null,"retryable":null}});
        let mut events = vec![self.event(state, "turn-item.updated", item, &now)?];
        for (kind, mut payload) in [
            ("run.updated", t3_contracts::stored::to_value(&state.run)?),
            (
                "run-attempt.updated",
                t3_contracts::stored::to_value(&state.attempt)?,
            ),
            (
                "node.updated",
                t3_contracts::stored::to_value(&state.root_node)?,
            ),
        ] {
            payload["status"] = "failed".into();
            payload["startedAt"] = json!(now);
            payload["completedAt"] = json!(now);
            events.push(self.event(state, kind, payload, &now)?);
        }
        let mut thread = state.provider_thread.clone();
        thread.status = if thread.native_thread_ref.is_some() {
            ProviderThreadStatus::Idle
        } else {
            ProviderThreadStatus::NotLoaded
        };
        thread.updated_at = now.clone();
        events.push(self.event(
            state,
            "provider-thread.updated",
            t3_contracts::stored::to_value(&thread)?,
            &now,
        )?);
        self.commit(state, events).await?;
        Ok(None)
    }

    async fn current(&self, state: &RootRunState) -> Result<bool> {
        Ok(self
            .projections
            .run(
                expanded_id(&state.run.thread_id)?,
                expanded_id(&state.run.id)?,
            )
            .await?
            .is_some_and(|run| {
                run.active_attempt_id.as_ref() == Some(&state.attempt.id)
                    && run.status == OrchestrationV2RunStatus::Starting
            }))
    }

    fn event(
        &self,
        state: &RootRunState,
        kind: &str,
        payload: serde_json::Value,
        now: &IsoDateTime,
    ) -> Result<OrchestrationV2DomainEvent> {
        crate::ports::checkpoints_forks::checkpoint_capture_service::domain_event(
            self.ids.as_ref(),
            &expanded_id(&state.run.thread_id)?,
            now,
            kind,
            payload,
            json!({"runId":state.run.id,"nodeId":state.root_node.id,"providerInstanceId":state.run.provider_instance_id}),
        )
    }

    async fn commit(
        &self,
        state: &RootRunState,
        events: Vec<OrchestrationV2DomainEvent>,
    ) -> Result<bool> {
        Ok(self
            .committer
            .commit(CommitRequest {
                receipt: None,
                decision: Decision {
                    events,
                    ..Default::default()
                },
                guard: CommitGuard::RunCurrent {
                    thread_id: expanded_id(&state.run.thread_id)?,
                    run_id: expanded_id(&state.run.id)?,
                    attempt_id: expanded_id(&state.attempt.id)?,
                    status: OrchestrationV2RunStatus::Starting,
                },
            })
            .await?
            .committed)
    }

    async fn failed(
        &self,
        state: &RootRunState,
        signal: &str,
        title: &str,
        error: Error,
        will_retry: bool,
    ) -> Result<Option<RunExecutionHandle>> {
        if matches!(
            error,
            Error::Cancelled | Error::Provider(ProviderError::Cancelled)
        ) || will_retry && !matches!(error, Error::Provider(ProviderError::RequiresSignIn))
        {
            return Err(error);
        }
        let now = self.clock.now();
        let ordinal = self
            .projections
            .next_turn_item_ordinal(
                expanded_id(&state.run.thread_id)?,
                Some(expanded_id(&state.run.id)?),
            )
            .await?;
        let run_id = expanded_id(&state.run.id)?;
        let item = json!({"id":derive_id("turn-item", &["run",&run_id.0,"signal",signal]),
            "threadId":state.run.thread_id,"runId":run_id,"nodeId":state.root_node.id,
            "providerThreadId":state.provider_thread.id,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":ordinal,"status":"failed","title":title,"startedAt":now,"completedAt":now,"updatedAt":now,
            "type":"error","failure":{"class":"provider_error","message":provider_failure_message(&error),"code":null,"retryable":null}});
        let mut events = vec![self.event(state, "turn-item.updated", item, &now)?];
        for (kind, mut payload) in [
            ("run.updated", json!(state.run)),
            ("run-attempt.updated", json!(state.attempt)),
            ("node.updated", json!(state.root_node)),
        ] {
            payload["status"] = "failed".into();
            payload["completedAt"] = json!(now);
            events.push(self.event(state, kind, payload, &now)?);
        }
        self.commit(state, events).await?;
        Ok(None)
    }
}

fn start_error(run_id: &RunId, detail: &str) -> Error {
    Error::Service {
        operation: "ProviderTurnStartService.start",
        detail: format!("Run {}: {detail}", run_id.0),
    }
}

pub fn provider_failure_message(error: &Error) -> String {
    match error {
        Error::Provider(
            ProviderError::Protocol { detail, .. } | ProviderError::Operation { detail, .. },
        ) => detail.clone(),
        _ => error.to_string(),
    }
}

pub fn turn_message(message: &OrchestrationV2ConversationMessage) -> Result<TurnMessage> {
    if message.text.contains("t3-context://")
        || message
            .context
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|context| !context.records.is_empty())
    {
        return Err(Error::Unsupported(
            "composer context projection for provider input",
        ));
    }
    Ok(TurnMessage {
        message_id: expanded_id(&message.id)?,
        text: message.text.clone(),
        attachments: message.attachments.clone(),
        created_by: message.created_by.clone(),
        creation_source: Some(message.creation_source.clone()),
        scheduled_task_id: message
            .scheduled_task_id
            .clone()
            .flatten()
            .map(|id| expanded_id(&id))
            .transpose()?,
        sender_thread_id: message
            .sender_thread_id
            .clone()
            .flatten()
            .map(|id| expanded_id(&id))
            .transpose()?,
    })
}
