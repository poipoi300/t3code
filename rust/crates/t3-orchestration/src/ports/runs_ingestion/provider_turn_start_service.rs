use super::{
    context_handoff_delivery::{
        self, DeliveryInput, HandoffBudget, HandoffPersistence, HistoryInjection,
        PortableHandoffBudget, PreparedDelivery,
    },
    context_handoff_service::{
        ContextHandoffService, HandoffContent, PrepareHandoff, is_side_chat_context_handoff,
        read_fork_history_items, visible_delta_run_ordinals,
    },
    provider_event_ingestor::{derive_id, expanded_id},
    run_execution_service::{
        RootRunState, RootTurnPreparation, RunExecutionHandle, RunExecutionService, StartRootRun,
    },
};
use crate::{
    api::*,
    error::{Error, Result},
    provider::*,
};
use async_trait::async_trait;
use serde_json::json;
use std::{collections::HashSet, sync::Arc};
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

struct HandoffWriter {
    committer: Arc<dyn EventCommitter>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    state: RootRunState,
}

#[async_trait]
impl HandoffPersistence for HandoffWriter {
    async fn persist(&self, mut handoff: OrchestrationV2ContextHandoff) -> Result<()> {
        let now = self.clock.now();
        handoff.updated_at = now.clone();
        let event = crate::ports::checkpoints_forks::checkpoint_capture_service::domain_event(
            self.ids.as_ref(),
            &expanded_id(&self.state.run.thread_id)?,
            &now,
            "context-handoff.updated",
            handoff,
            json!({"runId":self.state.run.id,"providerInstanceId":self.state.run.provider_instance_id}),
        )?;
        self.committer
            .commit(CommitRequest {
                receipt: None,
                guard: CommitGuard::None,
                decision: Decision {
                    events: vec![event],
                    ..Default::default()
                },
            })
            .await?;
        Ok(())
    }
}

struct HandoffStart {
    handoffs: Vec<OrchestrationV2ContextHandoff>,
    side_chat: Option<OrchestrationV2ContextHandoff>,
    budget: u64,
    delivered: HashSet<TurnItemId>,
    persistence: Arc<dyn HandoffPersistence>,
    runtime: Arc<dyn SessionRuntime>,
    restart_note: String,
    note_continuation: bool,
}

struct NativeInjection<'a> {
    runtime: &'a dyn SessionRuntime,
    thread: &'a OrchestrationV2ProviderThread,
}
#[async_trait]
impl HistoryInjection for NativeInjection<'_> {
    async fn inject(
        &self,
        messages: &[OrchestrationV2HistoricalMessage],
        context: &str,
    ) -> Result<bool> {
        Ok(self
            .runtime
            .extensions()
            .ok_or(ProviderError::Unsupported("inject_history"))?
            .inject_history(self.thread.clone(), messages.to_vec(), context.into())
            .await?)
    }
}

#[async_trait]
impl RootTurnPreparation for HandoffStart {
    async fn prepare(
        &self,
        mut turn: TurnInput,
        compact: bool,
    ) -> Result<(TurnInput, PreparedDelivery)> {
        let injection = NativeInjection {
            runtime: self.runtime.as_ref(),
            thread: &turn.provider_thread,
        };
        let inject = self
            .runtime
            .extensions()
            .is_some_and(|extension| extension.capabilities().inject_history);
        let goal = native_goals(self.runtime.as_ref())
            && is_goal(&turn.message.text, &turn.message.attachments);
        let mut delivery = context_handoff_delivery::deliver_context_handoffs(
            DeliveryInput {
                handoffs: &self.handoffs,
                provider_thread: &turn.provider_thread,
                budget: self.budget,
                defer_inline: compact || goal,
                already_delivered: &self.delivered,
                inject: inject.then_some(&injection as &dyn HistoryInjection),
            },
            &PortableHandoffBudget,
            self.persistence.clone(),
        )
        .await?;
        let side_context = self.side_chat.as_ref().filter(|_| !compact && !goal);
        let context = [
            delivery.context.as_str(),
            side_context.map_or("", |handoff| handoff.summary_text.as_str()),
            self.restart_note.as_str(),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
        if !goal {
            turn.message.text =
                context_handoff_delivery::with_inline_context(&context, &turn.message.text);
        }
        if self.note_continuation {
            turn.restart_continuation_of_run_id = None;
        }
        if let Some(handoff) = side_context
            && let Some(native) = turn
                .provider_thread
                .native_thread_ref
                .as_ref()
                .and_then(|reference| reference.native_id.as_ref())
        {
            let mut handoff = handoff.clone();
            handoff.to_provider_thread_id = turn.provider_thread.id.clone();
            handoff.delivery = Some(Some(ContextHandoffDelivery {
                native_thread_id: native.0.clone(),
                status: ContextHandoffDeliveryStatus::Inline,
                item_ids: vec![],
                omitted_item_ids: None,
            }));
            delivery.record_after_acceptance(handoff);
        }
        Ok((turn, delivery))
    }
}

struct ForkPlan {
    input: ForkInput,
    busy: bool,
    source_thread: OrchestrationV2AppThread,
    source_runs: Vec<OrchestrationV2Run>,
    source_ordinal: u64,
}

struct ForkFallback<'a> {
    plan: &'a ForkPlan,
    transfer: &'a serde_json::Value,
    state: &'a RootRunState,
    runtime: &'a dyn SessionRuntime,
    policy: &'a RuntimePolicy,
    options: &'a RuntimePolicyOptions,
    title: &'a str,
    failure: Option<&'a ProviderError>,
}

struct DeliveryContext {
    budget: u64,
    delivered: HashSet<TurnItemId>,
    covered: HashSet<TurnItemId>,
    usage: Option<t3_contracts::provider_runtime::ThreadTokenUsageSnapshot>,
    native_has_turns: bool,
}

struct ResumeInput<'a> {
    projection: &'a OrchestrationV2ThreadProjection,
    state: &'a RootRunState,
    runtime: &'a dyn SessionRuntime,
    policy: &'a RuntimePolicy,
    options: &'a RuntimePolicyOptions,
    message: &'a TurnMessage,
    handoffs: Vec<OrchestrationV2ContextHandoff>,
}

struct ResumeOutcome {
    provider: OrchestrationV2ProviderThread,
    handoffs: Vec<OrchestrationV2ContextHandoff>,
    notice: Option<serde_json::Value>,
    recovered: bool,
}

struct PreviewHandoffWriter;
#[async_trait]
impl HandoffPersistence for PreviewHandoffWriter {
    async fn persist(&self, _: OrchestrationV2ContextHandoff) -> Result<()> {
        Ok(())
    }
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
        let runs = &projection.runs;
        let turns = &projection.provider_turns;
        let mut related_threads = Vec::new();
        let mut related_providers = Vec::new();
        for agent in projection
            .subagents
            .iter()
            .filter(|agent| agent.status == OrchestrationV2TurnItemStatus::Completed)
        {
            if let Some(id) = &agent.child_thread_id {
                related_threads.push(expanded_id(id)?);
            }
            if let Some(id) = &agent.provider_thread_id {
                related_providers.push(expanded_id(id)?);
            }
        }
        let (mut handoffs, side_chat) = pending_handoffs(&projection, &state)?;
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
            .iter()
            .find(|session| session.id == IdOrIndex::Id(session_id.clone()))
            .cloned();
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
            }; self.sessions.attach_with_policy(expanded_id(&state.run.provider_instance_id)?, open, policy_options.clone()).await } => result,
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
        let compactions = projection
            .messages
            .iter()
            .filter(|message| {
                is_compaction(&message.text, &message.attachments)
                    || (native_goals(runtime.as_ref())
                        && is_goal(&message.text, &message.attachments))
            })
            .map(|message| expanded_id(&message.id))
            .collect::<Result<HashSet<_>>>()?;
        let cancelled_work =
            super::restart_background_note::pending_restart_cancelled_background_work(
                &state.run,
                &projection.runs,
                &projection.provider_turns,
                &projection.attempts,
                &compactions,
            );
        let restart_note = if cancelled_work.is_empty() {
            String::new()
        } else {
            super::restart_background_note::restart_cancelled_background_work_note(&cancelled_work)
        };
        let note_continuation = super::restart_background_note::is_restart_note_continuation(
            &state.run,
            &projection.runs,
            &projection.provider_turns,
            &projection.attempts,
        );
        let mut budget_message = prepared_message.clone();
        if !restart_note.is_empty() {
            budget_message.text = format!("{restart_note}\n\n{}", budget_message.text);
        }
        let fork_plan = match &native_fork_transfer {
            Some(transfer) => Some(
                self.native_fork_input(transfer, &state, runtime_policy.clone())
                    .await?,
            ),
            None => None,
        };
        let mut recovery_notice = None;
        let mut resume_recovered = false;
        let resumed = if fork_plan.is_none() && state.provider_thread.native_thread_ref.is_some() {
            let resume = self.resume_or_recover(ResumeInput {
                projection: &projection,
                state: &state,
                runtime: runtime.as_ref(),
                policy: &runtime_policy,
                options: &policy_options,
                message: &budget_message,
                handoffs: handoffs.clone(),
            });
            match tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                result = resume => result,
            } {
                Ok(outcome) => {
                    handoffs = outcome.handoffs;
                    recovery_notice = outcome.notice;
                    resume_recovered = outcome.recovered;
                    Some(outcome.provider)
                }
                Err(Error::Stale) => return Ok(None),
                Err(error) => {
                    return self
                        .failed(
                            &state,
                            "provider-thread-load-failure",
                            "Provider turn failed to start",
                            error,
                            will_retry,
                        )
                        .await;
                }
            }
        } else {
            None
        };
        let loaded = if let Some(resumed) = resumed {
            Some(Ok(resumed))
        } else if fork_plan.as_ref().is_some_and(|plan| plan.busy) {
            None
        } else {
            Some(tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                result = async {
                    if let Some(plan)=&fork_plan {
                        let fork = plan.input.clone();
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
            })
        };
        let mut portable_fork = None;
        let loaded = match loaded {
            Some(Ok(loaded)) => loaded,
            failure
                if fork_plan.is_some()
                    && (failure.is_none() || !will_retry)
                    && !matches!(&failure, Some(Err(ProviderError::Cancelled))) =>
            {
                if !self.current(&state).await? {
                    return Ok(None);
                }
                let native_error = failure.and_then(std::result::Result::err);
                let fallback = self.portable_fork(ForkFallback {
                    plan: fork_plan.as_ref().expect("fork plan checked"),
                    transfer: native_fork_transfer.as_ref().expect("fork transfer exists"),
                    state: &state,
                    runtime: runtime.as_ref(),
                    policy: &runtime_policy,
                    options: &policy_options,
                    title: &projection.thread.title.0,
                    failure: native_error.as_ref(),
                });
                let (loaded, handoff) = match tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Err(Error::Cancelled),
                    result = fallback => result,
                } {
                    Ok(result) => result,
                    Err(Error::Stale) => return Ok(None),
                    Err(error) => {
                        return self
                            .failed(
                                &state,
                                "provider-thread-load-failure",
                                "Provider turn failed to start",
                                error,
                                will_retry,
                            )
                            .await;
                    }
                };
                handoffs.insert(0, handoff.clone());
                portable_fork = Some(handoff);
                loaded
            }
            Some(Err(error)) => {
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
            None => unreachable!("only busy forks skip native loading"),
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
        running.provider_thread.forked_from = if let Some(handoff) = &portable_fork {
            if !running.provider_thread.handoff_ids.contains(&handoff.id) {
                running.provider_thread.handoff_ids.push(handoff.id.clone());
            }
            None
        } else {
            state.provider_thread.forked_from.clone()
        };
        running.provider_thread.created_at = state.provider_thread.created_at.clone();
        running.provider_thread.updated_at = now.clone();
        running.provider_thread.status = ProviderThreadStatus::Active;
        let delivery_context = self
            .delivery_budget(
                &projection,
                &running,
                runtime.as_ref(),
                &runtime_policy,
                &budget_message,
            )
            .await?;
        let native_has_turns = delivery_context.native_has_turns;
        running.provider_thread.context_usage = delivery_context.usage.clone().map(Some);
        if !resume_recovered {
            handoffs.extend(
                self.retry_handoff(&projection, &running, &delivery_context)
                    .await?,
            );
        }
        let preparation = if handoffs.is_empty()
            && side_chat.is_none()
            && restart_note.is_empty()
            && !note_continuation
        {
            None
        } else {
            Some(Arc::new(HandoffStart {
                handoffs,
                side_chat,
                budget: delivery_context.budget,
                delivered: delivery_context.delivered,
                runtime: runtime.clone(),
                restart_note,
                note_continuation,
                persistence: Arc::new(HandoffWriter {
                    committer: self.committer.clone(),
                    clock: self.clock.clone(),
                    ids: self.ids.clone(),
                    state: running.clone(),
                }),
            }) as Arc<dyn RootTurnPreparation>)
        };
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
        if portable_fork.is_none()
            && let Some(mut transfer) = native_fork_transfer
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
        if let Some(notice) = recovery_notice {
            events.push(self.event(&state, "turn-item.updated", notice, &now)?);
        }
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
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
        self.execution
            .start_root_run_with_preparation(
                StartRootRun {
                    state: running.clone(),
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
                        provider_thread: running.provider_thread.clone(),
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
                preparation,
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
    ) -> Result<ForkPlan> {
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
        let busy = runs.iter().any(|run| {
            run.ordinal > source_run.ordinal
                && matches!(
                    run.status,
                    OrchestrationV2RunStatus::Preparing
                        | OrchestrationV2RunStatus::Starting
                        | OrchestrationV2RunStatus::Running
                )
        });
        let source_ordinal = source_run.ordinal;
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
        Ok(ForkPlan {
            busy,
            source_thread: source.thread,
            source_runs: runs,
            source_ordinal,
            input: ForkInput {
                source_provider_thread: provider,
                source_provider_turns: Some(turns),
                provider_turn_id,
                target_thread_id: expanded_id(&state.run.thread_id)?,
                owner_node_id: None,
                model_selection: Some(state.run.model_selection.clone()),
                runtime_policy: Some(policy),
            },
        })
    }

    async fn retry_handoff(
        &self,
        projection: &OrchestrationV2ThreadProjection,
        state: &RootRunState,
        context: &DeliveryContext,
    ) -> Result<Vec<OrchestrationV2ContextHandoff>> {
        let missed: Vec<_> = projection
            .runs
            .iter()
            .filter(|run| {
                run.ordinal < state.run.ordinal
                    && run.provider_thread_id == state.run.provider_thread_id
                    && matches!(
                        run.status,
                        OrchestrationV2RunStatus::Failed | OrchestrationV2RunStatus::Interrupted
                    )
                    && !projection
                        .provider_turns
                        .iter()
                        .any(|turn| turn.run_attempt_id == run.active_attempt_id)
            })
            .collect();
        if missed.is_empty() {
            return Ok(vec![]);
        }
        let run_ids = missed
            .iter()
            .map(|run| expanded_id(&run.id))
            .collect::<Result<Vec<_>>>()?;
        let records = self
            .projections
            .records(
                expanded_id(&state.run.thread_id)?,
                vec![RecordKind::TurnItems],
                RecordFilter {
                    run_ids: Some(run_ids.clone()),
                    turn_item_run_ids: Some(run_ids.into_iter().map(Some).collect()),
                    ..Default::default()
                },
            )
            .await?;
        let mut items = vec![];
        for records in records.records {
            if let ProjectionRecordSet::TurnItems(values) = records {
                for item in values {
                    let value = t3_contracts::stored::to_value(&item)?;
                    let id: TurnItemId = serde_json::from_value(value["id"].clone())?;
                    if !context.covered.contains(&id)
                        && PortableHandoffBudget.historical_message(&item).is_some()
                    {
                        items.push(item);
                    }
                }
            }
        }
        if items.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![
            ContextHandoffService {
                ids: self.ids.clone(),
                budget: Arc::new(PortableHandoffBudget),
                token_cap: handoff_token_cap(),
            }
            .prepare(PrepareHandoff {
                thread_id: expanded_id(&state.run.thread_id)?,
                target_run_id: expanded_id(&state.run.id)?,
                transfer_id: None,
                from_provider_thread_ids: vec![expanded_id(&state.provider_thread.id)?],
                to_provider_thread_id: expanded_id(&state.provider_thread.id)?,
                covered_run_ordinals: ContextHandoffCoveredRunOrdinals {
                    r#from: missed
                        .iter()
                        .map(|run| run.ordinal)
                        .min()
                        .expect("nonempty missed runs"),
                    to: missed
                        .iter()
                        .map(|run| run.ordinal)
                        .max()
                        .expect("nonempty missed runs"),
                },
                items,
                runs: projection.runs.clone(),
                created_at: self.clock.now(),
                content: HandoffContent::Provider {
                    strategy: ContextHandoffStrategy::DeltaSinceTargetLastSeen,
                },
            })?,
        ])
    }

    async fn resume_or_recover(&self, input: ResumeInput<'_>) -> Result<ResumeOutcome> {
        let ResumeInput {
            projection,
            state,
            runtime,
            policy,
            options,
            message,
            mut handoffs,
        } = input;
        let native = state
            .provider_thread
            .native_thread_ref
            .as_ref()
            .and_then(|reference| reference.native_id.as_ref());
        let uncertain = projection.context_handoffs.iter().any(|handoff| {
            handoff.to_provider_thread_id == state.provider_thread.id
                && handoff
                    .delivery
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|delivery| {
                        Some(delivery.native_thread_id.as_str()) == native.map(|id| id.0.as_str())
                            && delivery.status == ContextHandoffDeliveryStatus::Pending
                    })
        });
        let resumed = if uncertain {
            None
        } else {
            let resume = ResumeThread {
                provider_thread: state.provider_thread.clone(),
                thread_id: Some(expanded_id(&state.run.thread_id)?),
                model_selection: Some(state.run.model_selection.clone()),
                runtime_policy: Some(policy.clone()),
            };
            Some(if options.reasoning_effort.is_some() {
                runtime
                    .resume_thread_with_policy(resume, options.clone())
                    .await
            } else {
                runtime.resume_thread(resume).await
            })
        };
        let mut recovery = None;
        if let Some(Ok(resumed)) = &resumed {
            let mut candidate = state.clone();
            candidate.provider_thread = resumed.clone();
            candidate.provider_thread.id = state.provider_thread.id.clone();
            let context = self
                .delivery_budget(projection, &candidate, runtime, policy, message)
                .await?;
            let retry = self.retry_handoff(projection, &candidate, &context).await?;
            let mut preview_handoffs = handoffs.clone();
            preview_handoffs.extend(retry);
            if handoffs_fit(
                &preview_handoffs,
                &candidate.provider_thread,
                &context,
                is_compaction(&message.text, &message.attachments)
                    || (native_goals(runtime) && is_goal(&message.text, &message.attachments)),
            )
            .await?
            {
                return Ok(ResumeOutcome {
                    provider: resumed.clone(),
                    handoffs,
                    notice: None,
                    recovered: false,
                });
            }
            let handoff = self.recovery_handoff(projection, state).await?;
            let mut fresh = candidate.clone();
            fresh.provider_thread.native_thread_ref = None;
            let fresh_context = self
                .delivery_budget(projection, &fresh, runtime, policy, message)
                .await?;
            let fresh_handoffs = with_recovery_handoff(&handoffs, &handoff, &state.run.id);
            if !handoffs_fit(
                &fresh_handoffs,
                &fresh.provider_thread,
                &fresh_context,
                false,
            )
            .await?
            {
                return Ok(ResumeOutcome {
                    provider: resumed.clone(),
                    handoffs,
                    notice: None,
                    recovered: false,
                });
            }
            if let Some(extension) = runtime.extensions()
                && extension
                    .pending_background_work(Some(resumed.clone()), true)
                    .await?
            {
                return Err(start_error(
                    &expanded_id(&state.run.id)?,
                    "The target provider still has background work running. Wait for it to finish or stop it before retrying the handoff.",
                ));
            }
            recovery = Some(handoff);
        }
        if let Some(Err(error @ (ProviderError::ThreadInUse(_) | ProviderError::Cancelled))) =
            resumed
        {
            return Err(error.into());
        }
        if !self.current(state).await? {
            return Err(Error::Stale);
        }
        let mut existing = state.provider_thread.clone();
        existing.native_thread_ref = None;
        let ensure = EnsureThread {
            thread_id: expanded_id(&state.run.thread_id)?,
            title: Some(projection.thread.title.0.clone()),
            model_selection: state.run.model_selection.clone(),
            runtime_policy: policy.clone(),
            provider_session_id: state
                .provider_thread
                .provider_session_id
                .as_ref()
                .map(expanded_id)
                .transpose()?,
            existing_provider_thread: Some(existing),
            force_new_native_thread: true,
        };
        let provider = if options.reasoning_effort.is_some() {
            runtime
                .ensure_thread_with_policy(ensure, options.clone())
                .await?
        } else {
            runtime.ensure_thread(ensure).await?
        };
        let handoff = match recovery {
            Some(handoff) => handoff,
            None => self.recovery_handoff(projection, state).await?,
        };
        handoffs = with_recovery_handoff(&handoffs, &handoff, &state.run.id);
        let now = self.clock.now();
        let transfer = json!({"id":handoff.transfer_id,"type":"provider_handoff","sourceThreadId":state.run.thread_id,
            "targetThreadId":state.run.thread_id,"sourcePoint":{"threadId":state.run.thread_id},"basePoint":null,
            "sourceProviderInstanceId":state.provider_thread.provider_instance_id,"targetProviderInstanceId":state.run.provider_instance_id,
            "targetRunId":state.run.id,"status":"resolved_portable","resolution":{"strategy":"portable_context","contextHandoffId":handoff.id},
            "createdBy":"system","error":null,"createdAt":now,"updatedAt":now,"consumedAt":null});
        let ordinal = self
            .projections
            .next_turn_item_ordinal(
                expanded_id(&state.run.thread_id)?,
                Some(expanded_id(&state.run.id)?),
            )
            .await?;
        let previous = projection
            .runs
            .iter()
            .filter(|run| {
                run.ordinal < state.run.ordinal
                    && run.provider_thread_id == state.run.provider_thread_id
                    && run.started_at.is_some()
            })
            .max_by_key(|run| run.ordinal);
        let notice = json!({"id":derive_id("turn-item", &["run",&expanded_id(&state.run.id)?.0,"signal","resume-fallback-handoff"]),
            "threadId":state.run.thread_id,"runId":state.run.id,"nodeId":state.root_node.id,"providerThreadId":state.provider_thread.id,
            "providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":"Recovered context",
            "startedAt":now,"completedAt":now,"updatedAt":now,"type":"handoff","contextHandoffId":handoff.id,
            "fromProviderThreadIds":handoff.from_provider_thread_ids,"toProviderThreadId":handoff.to_provider_thread_id,
            "fromProviderInstanceIds":[state.provider_thread.provider_instance_id],"toProviderInstanceId":state.run.provider_instance_id,
            "fromModelSelections":previous.map(|run|vec![&run.model_selection]).unwrap_or_default(),"toModel":state.run.model_selection.model,
            "strategy":handoff.strategy,"summary":handoff.summary_text});
        if !self
            .commit(
                state,
                vec![
                    self.event(
                        state,
                        "context-handoff.updated",
                        t3_contracts::stored::to_value(&handoff)?,
                        &now,
                    )?,
                    self.event(state, "context-transfer.updated", transfer, &now)?,
                ],
            )
            .await?
        {
            return Err(Error::Stale);
        }
        Ok(ResumeOutcome {
            provider,
            handoffs,
            notice: Some(notice),
            recovered: true,
        })
    }

    async fn recovery_handoff(
        &self,
        projection: &OrchestrationV2ThreadProjection,
        state: &RootRunState,
    ) -> Result<OrchestrationV2ContextHandoff> {
        let items = read_fork_history_items(
            self.projections.as_ref(),
            &projection.thread,
            state.run.ordinal.saturating_sub(1).max(1),
        )
        .await?;
        let items = items
            .into_iter()
            .filter(|item| {
                t3_contracts::stored::to_value(item).is_ok_and(|value| {
                    value["runId"]
                        != t3_contracts::stored::to_value(&state.run.id)
                            .expect("typed ID serializes")
                })
            })
            .collect();
        ContextHandoffService {
            ids: self.ids.clone(),
            budget: Arc::new(PortableHandoffBudget),
            token_cap: handoff_token_cap(),
        }
        .prepare(PrepareHandoff {
            thread_id: expanded_id(&state.run.thread_id)?,
            target_run_id: expanded_id(&state.run.id)?,
            transfer_id: Some(ContextTransferId(self.ids.next(IdKind::ContextTransfer))),
            from_provider_thread_ids: vec![expanded_id(&state.provider_thread.id)?],
            to_provider_thread_id: expanded_id(&state.provider_thread.id)?,
            covered_run_ordinals: ContextHandoffCoveredRunOrdinals {
                r#from: 1,
                to: state.run.ordinal.saturating_sub(1).max(1),
            },
            items,
            runs: projection.runs.clone(),
            created_at: self.clock.now(),
            content: HandoffContent::Provider {
                strategy: ContextHandoffStrategy::FullThreadSummary,
            },
        })
    }

    async fn portable_fork(
        &self,
        input: ForkFallback<'_>,
    ) -> Result<(OrchestrationV2ProviderThread, OrchestrationV2ContextHandoff)> {
        let ForkFallback {
            plan,
            transfer,
            state,
            runtime,
            policy,
            options,
            title,
            failure,
        } = input;
        let items = read_fork_history_items(
            self.projections.as_ref(),
            &plan.source_thread,
            plan.source_ordinal,
        )
        .await?;
        let now = self.clock.now();
        let handoff = ContextHandoffService {
            ids: self.ids.clone(),
            budget: Arc::new(PortableHandoffBudget),
            token_cap: handoff_token_cap(),
        }
        .prepare(PrepareHandoff {
            thread_id: expanded_id(&state.run.thread_id)?,
            target_run_id: expanded_id(&state.run.id)?,
            transfer_id: Some(serde_json::from_value(transfer["id"].clone())?),
            from_provider_thread_ids: vec![expanded_id(&plan.input.source_provider_thread.id)?],
            to_provider_thread_id: expanded_id(&state.provider_thread.id)?,
            covered_run_ordinals: visible_delta_run_ordinals(&plan.source_runs, &items)?,
            items,
            runs: plan.source_runs.clone(),
            created_at: now.clone(),
            content: HandoffContent::Provider {
                strategy: ContextHandoffStrategy::FullThreadSummary,
            },
        })?;
        let mut existing = state.provider_thread.clone();
        existing.forked_from = None;
        if failure.is_some() {
            existing.native_thread_ref = None;
        }
        let ensure = EnsureThread {
            thread_id: expanded_id(&state.run.thread_id)?,
            title: Some(title.into()),
            model_selection: state.run.model_selection.clone(),
            runtime_policy: policy.clone(),
            provider_session_id: state
                .provider_thread
                .provider_session_id
                .as_ref()
                .map(expanded_id)
                .transpose()?,
            existing_provider_thread: Some(existing),
            force_new_native_thread: failure.is_some(),
        };
        let loaded = if options.reasoning_effort.is_some() {
            runtime
                .ensure_thread_with_policy(ensure, options.clone())
                .await?
        } else {
            runtime.ensure_thread(ensure).await?
        };
        let mut transfer = transfer.clone();
        transfer["targetProviderInstanceId"] =
            t3_contracts::stored::to_value(&state.run.provider_instance_id)?;
        transfer["status"] = "resolved_portable".into();
        transfer["resolution"] =
            json!({"strategy":"portable_context","contextHandoffId":handoff.id});
        transfer["error"] = serde_json::Value::Null;
        transfer["updatedAt"] = json!(now);
        let mut events = vec![
            self.event(
                state,
                "context-handoff.updated",
                t3_contracts::stored::to_value(&handoff)?,
                &now,
            )?,
            self.event(state, "context-transfer.updated", transfer, &now)?,
        ];
        if let Some(error) = failure {
            let run_id = expanded_id(&state.run.id)?;
            let ordinal = self
                .projections
                .next_turn_item_ordinal(expanded_id(&state.run.thread_id)?, Some(run_id.clone()))
                .await?;
            let item = json!({"id":derive_id("turn-item", &["run",&run_id.0,"signal","native-fork-fallback"]),
                "threadId":state.run.thread_id,"runId":state.run.id,"nodeId":state.root_node.id,"providerThreadId":state.provider_thread.id,
                "providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed",
                "title":"Native fork failed","startedAt":now,"completedAt":now,"updatedAt":now,"type":"system_notice",
                "message":format!("Could not fork the source conversation natively ({error}). Started a fresh session with its history.")});
            events.push(self.event(state, "turn-item.updated", item, &now)?);
        }
        if !self.commit(state, events).await? {
            return Err(Error::Stale);
        }
        Ok((loaded, handoff))
    }

    async fn delivery_budget(
        &self,
        projection: &OrchestrationV2ThreadProjection,
        state: &RootRunState,
        runtime: &dyn SessionRuntime,
        policy: &RuntimePolicy,
        message: &TurnMessage,
    ) -> Result<DeliveryContext> {
        let previous = projection
            .provider_threads
            .iter()
            .find(|thread| thread.id == state.provider_thread.id)
            .unwrap_or(&state.provider_thread);
        let native = |thread: &OrchestrationV2ProviderThread| {
            thread
                .native_thread_ref
                .as_ref()
                .and_then(|reference| reference.native_id.as_ref())
                .map(|id| id.0.clone())
        };
        let same_native = native(previous) == native(&state.provider_thread);
        let native_id = native(&state.provider_thread);
        let settled: Vec<_> = projection
            .context_handoffs
            .iter()
            .filter(|handoff| {
                handoff.to_provider_thread_id == state.provider_thread.id
                    && handoff
                        .delivery
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|delivery| {
                            Some(&delivery.native_thread_id) == native_id.as_ref()
                                && delivery.status != ContextHandoffDeliveryStatus::Pending
                        })
            })
            .collect();
        let delivered: HashSet<_> = settled
            .iter()
            .flat_map(|handoff| {
                &handoff
                    .delivery
                    .as_ref()
                    .and_then(Option::as_ref)
                    .expect("settled delivery")
                    .item_ids
            })
            .cloned()
            .collect();
        let mut covered = delivered.clone();
        covered.extend(
            settled
                .iter()
                .flat_map(|handoff| handoff.delivery.as_ref().and_then(Option::as_ref))
                .flat_map(|delivery| delivery.omitted_item_ids.as_ref().and_then(Option::as_ref))
                .flatten()
                .cloned(),
        );
        let accepted: Vec<_> = projection
            .attempts
            .iter()
            .filter(|attempt| {
                attempt.provider_thread_id == state.provider_thread.id
                    && projection
                        .provider_turns
                        .iter()
                        .any(|turn| turn.run_attempt_id.as_ref() == Some(&attempt.id))
            })
            .collect();
        let measured = projection
            .provider_turns
            .iter()
            .filter_map(|turn| {
                if turn.provider_thread_id != previous.id {
                    return None;
                }
                let usage = turn.token_usage.as_ref().and_then(Option::as_ref)?;
                let attempt = projection.attempts.iter().find(|attempt| {
                    Some(&attempt.id) == turn.run_attempt_id.as_ref()
                        && attempt.provider_thread_id == previous.id
                        && attempt.root_node_id == turn.node_id
                        && attempt
                            .native_thread_id
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_some_and(|id| {
                                expanded_id(id).ok().as_ref() == native(previous).as_ref()
                            })
                })?;
                let run = projection
                    .runs
                    .iter()
                    .find(|run| run.id == attempt.run_id)?;
                Some((usage, &run.model_selection))
            })
            .max_by(|(left, _), (right, _)| left.updated_at.cmp(&right.updated_at));
        let previous_selection = measured.map(|(_, selection)| selection).or_else(|| {
            projection
                .runs
                .iter()
                .filter(|run| {
                    run.ordinal < state.run.ordinal
                        && run.started_at.is_some()
                        && run.provider_thread_id.as_ref() == Some(&previous.id)
                })
                .max_by_key(|run| run.ordinal)
                .map(|run| &run.model_selection)
        });
        let same_selection = previous_selection.is_none_or(|selection| {
            crate::ports::decisions::model_switch::model_selections_equal(
                selection,
                &state.run.model_selection,
            )
        });
        let extensions = runtime.extensions();
        let known_window = match extensions {
            Some(extension) => {
                extension
                    .model_context_window(state.run.model_selection.clone(), policy.cwd.clone())
                    .await?
            }
            None => None,
        };
        let reuse = same_selection
            || previous_selection.is_some_and(|selection| {
                extensions.is_some_and(|extension| {
                    extension.can_reuse_context_usage(selection, &state.run.model_selection)
                })
            });
        let mut usage = state
            .provider_thread
            .context_usage
            .clone()
            .flatten()
            .or_else(|| previous.context_usage.clone().flatten());
        if let Some((measured, _)) = measured {
            let mut value = usage
                .as_ref()
                .map(t3_contracts::stored::to_value)
                .transpose()?
                .unwrap_or_else(|| json!({}));
            value["usedTokens"] = json!(measured.used_tokens);
            if let Some(max) = measured.max_tokens.flatten().filter(|max| *max > 0) {
                value["maxTokens"] = json!(max);
            }
            usage = Some(serde_json::from_value(value)?);
        }
        let usage = context_handoff_delivery::context_usage_for_handoff(
            same_native,
            same_selection,
            reuse,
            usage,
            known_window,
        );
        let mut estimate = 0;
        if same_native && usage.is_none() {
            let items = read_fork_history_items(
                self.projections.as_ref(),
                &projection.thread,
                state.run.ordinal.saturating_sub(1).max(1),
            )
            .await?;
            for item in items {
                let value = t3_contracts::stored::to_value(&item)?;
                if value["runId"] == t3_contracts::stored::to_value(&state.run.id)? {
                    continue;
                }
                let run_id: Option<RunId> = serde_json::from_value(value["runId"].clone())?;
                let item_id: TurnItemId = serde_json::from_value(value["id"].clone())?;
                let accepted_native = value["providerThreadId"]
                    == t3_contracts::stored::to_value(&state.provider_thread.id)?
                    && run_id.as_ref().is_some_and(|run| {
                        accepted.iter().any(|attempt| {
                            if expanded_id(&attempt.run_id).ok().as_ref() != Some(run) {
                                return false;
                            }
                            match attempt.native_thread_id.as_ref().and_then(Option::as_ref) {
                                Some(id) => expanded_id(id).ok().as_ref() == native_id.as_ref(),
                                None => {
                                    !covered.contains(&item_id)
                                        && !settled.iter().any(|handoff| {
                                            handoff.strategy
                                                == ContextHandoffStrategy::FullThreadSummary
                                                && handoff
                                                    .from_provider_thread_ids
                                                    .contains(&previous.id)
                                                && projection.runs.iter().any(|source| {
                                                    expanded_id(&source.id).ok().as_ref()
                                                        == Some(run)
                                                        && source.ordinal
                                                            >= handoff.covered_run_ordinals.r#from
                                                        && source.ordinal
                                                            <= handoff.covered_run_ordinals.to
                                                })
                                        })
                                }
                            }
                        })
                    });
                if !accepted_native && !delivered.contains(&item_id) {
                    continue;
                }
                if let Some(historical) = PortableHandoffBudget.historical_message(&item) {
                    estimate += context_handoff_delivery::estimate_text_tokens(&historical.text);
                }
                if accepted_native && value["type"] == "user_message" {
                    estimate += context_handoff_delivery::attachment_token_allowance(
                        &serde_json::from_value::<
                            Vec<t3_contracts::chat_attachment::ChatAttachment>,
                        >(value["attachments"].clone())?,
                    );
                }
            }
        }
        let mut budget_thread = state.provider_thread.clone();
        budget_thread.context_usage = usage.map(Some);
        let budget = context_handoff_delivery::handoff_budget(
            handoff_token_cap(),
            &message.text,
            &message.attachments,
            &budget_thread,
            estimate,
            known_window,
        );
        let native_has_turns = accepted.iter().any(|attempt| {
            attempt
                .native_thread_id
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|id| expanded_id(id).ok().as_ref() == native_id.as_ref())
        }) || (same_native
            && accepted.iter().any(|attempt| {
                attempt
                    .native_thread_id
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none()
            })
            && !accepted.iter().any(|attempt| {
                attempt
                    .native_thread_id
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some()
            }));
        Ok(DeliveryContext {
            budget,
            delivered,
            covered,
            usage: budget_thread.context_usage.flatten(),
            native_has_turns,
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

fn handoff_token_cap() -> u64 {
    std::env::var("T3CODE_CONTEXT_HANDOFF_TOKEN_CAP")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .map_or(
            context_handoff_delivery::DEFAULT_HANDOFF_TOKEN_CAP,
            |value| value.clamp(1_024, 64_000) as u64,
        )
}

fn is_compaction(
    text: &str,
    attachments: &[t3_contracts::chat_attachment::ChatAttachment],
) -> bool {
    attachments.is_empty() && t3_contracts::wire::trim_js(text).eq_ignore_ascii_case("/compact")
}

fn is_goal(text: &str, attachments: &[t3_contracts::chat_attachment::ChatAttachment]) -> bool {
    attachments.is_empty()
        && t3_contracts::wire::trim_js(text)
            .strip_prefix("/goal")
            .is_some_and(|argument| {
                argument.is_empty()
                    || argument.chars().next().is_some_and(|ch| {
                        let mut bytes = [0; 4];
                        t3_contracts::wire::trim_js(ch.encode_utf8(&mut bytes)).is_empty()
                    })
            })
}

fn native_goals(runtime: &dyn SessionRuntime) -> bool {
    matches!(runtime.session().driver.0.as_str(), "codex" | "claudeAgent")
        || runtime.session().capabilities.turns.supports_native_goals == Some(Some(true))
}

async fn handoffs_fit(
    handoffs: &[OrchestrationV2ContextHandoff],
    provider: &OrchestrationV2ProviderThread,
    context: &DeliveryContext,
    defer: bool,
) -> Result<bool> {
    match context_handoff_delivery::deliver_context_handoffs(
        DeliveryInput {
            handoffs,
            provider_thread: provider,
            budget: context.budget,
            defer_inline: defer,
            already_delivered: &context.delivered,
            inject: None,
        },
        &PortableHandoffBudget,
        Arc::new(PreviewHandoffWriter),
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(Error::Service {
            operation: "ContextHandoffDelivery",
            detail,
        }) if detail == "ContextHandoffBudgetError" => Ok(false),
        Err(error) => Err(error),
    }
}

fn with_recovery_handoff(
    handoffs: &[OrchestrationV2ContextHandoff],
    recovery: &OrchestrationV2ContextHandoff,
    run: &IdOrIndex<RunId>,
) -> Vec<OrchestrationV2ContextHandoff> {
    handoffs
        .iter()
        .filter(|handoff| &handoff.target_run_id != run)
        .cloned()
        .chain(std::iter::once(recovery.clone()))
        .chain(
            handoffs
                .iter()
                .filter(|handoff| &handoff.target_run_id == run)
                .cloned(),
        )
        .collect()
}

fn pending_handoffs(
    projection: &OrchestrationV2ThreadProjection,
    state: &RootRunState,
) -> Result<(
    Vec<OrchestrationV2ContextHandoff>,
    Option<OrchestrationV2ContextHandoff>,
)> {
    let subagent_transfers = projection
        .context_transfers
        .iter()
        .map(t3_contracts::stored::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|transfer| transfer["type"] == "subagent_result")
        .map(|transfer| transfer["id"].clone())
        .collect::<Vec<_>>();
    let side_chat = projection.thread.lineage.side_chat == Some(Some(true));
    let mut history = vec![];
    let mut side = None;
    for handoff in &projection.context_handoffs {
        let is_side = side_chat && is_side_chat_context_handoff(handoff);
        let pending = handoff.status == ContextHandoffStatus::Ready
            && (handoff.target_run_id == state.run.id
                || ((is_side || handoff.to_provider_thread_id == state.provider_thread.id)
                    && projection.runs.iter().any(|run| {
                        run.id == handoff.target_run_id
                            && (matches!(
                                run.status,
                                OrchestrationV2RunStatus::Failed
                                    | OrchestrationV2RunStatus::Interrupted
                                    | OrchestrationV2RunStatus::Cancelled
                            ) || (run.status == OrchestrationV2RunStatus::Completed
                                && handoff.delivery.as_ref().and_then(Option::as_ref).is_none()
                                && projection.messages.iter().any(|message| {
                                    message.id == run.user_message_id
                                        && (is_compaction(&message.text, &message.attachments)
                                            || is_goal(&message.text, &message.attachments))
                                })))
                    })));
        if !pending {
            continue;
        }
        if is_side {
            if side.is_none() && handoff.delivery.as_ref().and_then(Option::as_ref).is_none() {
                side = Some(handoff.clone());
            }
        } else if !handoff
            .transfer_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|id| {
                subagent_transfers
                    .contains(&t3_contracts::stored::to_value(id).expect("typed ID serializes"))
            })
        {
            history.push(handoff.clone());
        }
    }
    Ok((history, side))
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
