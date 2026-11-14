use crate::{
    api::*,
    effects::EffectExecutionOptions,
    error::{Error, Result},
    ports::decisions::{
        dispatch_mode_limit::*, orchestrator::command_identity, thread_command_executor::KeyedLanes,
    },
    stores::{
        StoreDatabase,
        receipts::{self, AggregateId, CommandReceipt, ReceiptStatus},
    },
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::OnceCell;

// The host preserves provider-core failure curation, redaction, bounds and capability classification.
pub trait QueueStartFailureFormatter: Send + Sync {
    fn failure(&self, cause: &Error) -> Result<OrchestrationV2ProviderFailure>;
}

/// TS `failQueuedStart`'s `makeProviderFailure({cause, code: "queued_start_failed", class:
/// "unknown"})` for the causes a queued start meets (dispatch refusals carry plain text, which
/// provider-core does not curate): its default message. Rust's capability refusal is plain
/// text too, so TS's `context_handoff_unsupported` code is not distinguished here.
fn default_queue_start_failure() -> Result<OrchestrationV2ProviderFailure> {
    Ok(serde_json::from_value(serde_json::json!({
        "class": "unknown",
        "message": "Provider turn failed.",
        "code": "queued_start_failed",
        "retryable": null,
    }))?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    Success,
    Failure,
    Interrupt,
}

pub trait DispatchObserver: Send + Sync {
    fn record(&self, command_type: &str, outcome: DispatchOutcome, duration: std::time::Duration);
}

struct DispatchMeasurement {
    observer: Arc<dyn DispatchObserver>,
    command_type: String,
    started: std::time::Instant,
    outcome: DispatchOutcome,
}

impl Drop for DispatchMeasurement {
    fn drop(&mut self) {
        self.observer.record(
            &self.command_type,
            if std::thread::panicking() {
                DispatchOutcome::Failure
            } else {
                self.outcome
            },
            self.started.elapsed(),
        );
    }
}

pub struct PreparedCommand {
    pub context: PreparedDecisionContext,
    pub guard: CommitGuard,
}

#[async_trait]
pub trait CommandInputs: Send + Sync {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        None
    }

    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>>;
    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand>;
}

pub struct ProjectionCommandInputs {
    pub reader: Arc<dyn ProjectionReader>,
    pub providers: Arc<dyn ProviderRegistry>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

pub struct ContextCommandInputs {
    pub base: Arc<dyn CommandInputs>,
    pub preparer: Arc<dyn crate::decision::engine::MessageContextPreparer>,
}
#[async_trait]
impl CommandInputs for ContextCommandInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        self.base.reader()
    }
    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        self.base.lock_keys(command).await
    }
    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand> {
        let mut prepared = self.base.prepare(command).await?;
        let continuation =
            crate::ports::decisions::orchestrator::metadata_update_continuation(command)?;
        let context = if let Some(continuation) = &continuation {
            let mut updated = prepared.context.clone();
            if let Some(projection) = updated.context.projection.as_mut() {
                let metadata = crate::decision::engine::stored_value(command)?;
                if let Some(path) = metadata.get("worktreePath") {
                    projection.thread.worktree_path = serde_json::from_value(path.clone())?;
                }
                if let Some(branch) = metadata.get("branch") {
                    projection.thread.branch = serde_json::from_value(branch.clone())?;
                }
            }
            self.preparer.prepare(&updated, continuation).await?
        } else {
            self.preparer.prepare(&prepared.context, command).await?
        };
        prepared.context.inputs.message_context = context;
        Ok(prepared)
    }
}

pub struct RestartCommandInputs {
    pub base: Arc<dyn CommandInputs>,
    pub database: StoreDatabase,
}
#[async_trait]
impl CommandInputs for RestartCommandInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        self.base.reader()
    }
    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        self.base.lock_keys(command).await
    }
    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand> {
        let mut prepared = self.base.prepare(command).await?;
        if matches!(
            command_identity(command)?.1.as_str(),
            "thread.stop" | "run.interrupt"
        ) {
            let observations = load_restart_continuations(
                &self.database,
                &prepared.context.inputs.related_projections,
            )
            .await?;
            prepared.context.inputs.restart_continuations = observations;
        }
        Ok(prepared)
    }
}

pub async fn load_restart_continuations(
    database: &StoreDatabase,
    projections: &[OrchestrationV2ThreadProjection],
) -> Result<crate::ports::decisions::orchestrator::RestartContinuations> {
    let threads = projections
        .iter()
        .map(|projection| {
            Ok((
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &projection.thread.id,
                )?,
                projection
                    .runs
                    .iter()
                    .map(|run| {
                        crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                            &run.id,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    database
        .read(move |db| {
            threads
                .into_iter()
                .map(|(thread, runs)| {
                    let mut pending = Vec::new();
                    for run in runs {
                        if crate::stores::outbox::get(
                            db,
                            &format!("effect:restart-continuation:{}", run.0),
                        )?
                        .is_some_and(|effect| {
                            matches!(
                                effect.status,
                                crate::effects::EffectStatus::Pending
                                    | crate::effects::EffectStatus::Running
                            )
                        }) {
                            pending.push(run);
                        }
                    }
                    Ok((thread, pending))
                })
                .collect()
        })
        .await
}

pub struct SettingsCommandInputs {
    pub base: Arc<dyn CommandInputs>,
    pub settings: Arc<dyn SettingsService>,
}
#[async_trait]
impl CommandInputs for SettingsCommandInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        self.base.reader()
    }
    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        self.base.lock_keys(command).await
    }
    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand> {
        let mut prepared = self.base.prepare(command).await?;
        if let Some(projection) = &prepared.context.context.projection {
            let settings = self.settings.get().await?;
            let project = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                &projection.thread.project_id,
            )?;
            let mut instances = vec![
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &projection.thread.provider_instance_id,
                )?,
            ];
            instances.extend(
                prepared
                    .context
                    .inputs
                    .provider_metadata
                    .iter()
                    .map(|(id, _)| id.clone()),
            );
            instances.sort_by(|a, b| a.0.cmp(&b.0));
            instances.dedup();
            for instance in instances {
                let enabled=crate::ports::thread_project::thread_management_service::provider_instance_enabled(&settings,&project,&instance)?;
                prepared
                    .context
                    .inputs
                    .provider_enabled
                    .push((instance, enabled));
            }
        }
        Ok(prepared)
    }
}

pub struct RuntimeCommandInputs {
    pub base: ProjectionCommandInputs,
    pub sessions: Arc<dyn SessionDirectory>,
    pub policy: Arc<dyn RuntimePolicyResolver>,
}

#[async_trait]
impl CommandInputs for RuntimeCommandInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        Some(self.base.reader.as_ref())
    }

    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        self.base.lock_keys(command).await
    }
    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand> {
        let mut prepared = self.base.prepare(command).await?;
        let (command_id, kind, _) = command_identity(command)?;
        let continuation =
            crate::ports::decisions::orchestrator::metadata_update_continuation(command)?;
        if let Some(projection) = &prepared.context.context.projection {
            if matches!(
                kind.as_str(),
                "message.dispatch" | "prepared-run.release" | "checkpoint.rollback"
            ) || continuation.is_some()
            {
                let value = crate::decision::engine::stored_value(
                    continuation.as_ref().unwrap_or(command),
                )?;
                let selection =
                    effective_selection(&value, Some(projection))?.ok_or(Error::Stale)?;
                let mut thread = projection.thread.clone();
                if continuation.is_some() {
                    let metadata = crate::decision::engine::stored_value(command)?;
                    if let Some(path) = metadata.get("worktreePath") {
                        thread.worktree_path = serde_json::from_value(path.clone())?;
                    }
                }
                // TS `runtimePolicyCeilingCause`: refused like the dispatch ceiling, unrecorded.
                let resolved = self
                    .policy
                    .resolve(thread, selection)
                    .await
                    .map_err(|error| {
                        if !crate::ports::decisions::runtime_policy::is_above_ceiling(&error) {
                            return error;
                        }
                        let reason = match error {
                            Error::Service { detail, .. } => detail,
                            other => other.to_string(),
                        };
                        Error::Rejected { command_id, reason }
                    })?;
                prepared.context.inputs.runtime_policy = Some(resolved);
            }
            let mut sessions = std::collections::BTreeMap::new();
            for provider in &projection.provider_threads {
                let Some(session_id) = &provider.provider_session_id else {
                    continue;
                };
                let session_id =
                    crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                        session_id,
                    )?;
                if !sessions.contains_key(&session_id) {
                    sessions.insert(
                        session_id.clone(),
                        self.sessions.get(session_id.clone()).await?,
                    );
                }
                let probe = matches!(
                    kind.as_str(),
                    "run.interrupt"
                        | "thread.stop"
                        | "thread.background-work.settle"
                        | "thread.settle"
                        | "thread.auto-settle"
                        | "thread.metadata.update"
                        | "thread.runtime-mode.set"
                        | "thread.model-selection.set"
                        | "provider.switch"
                ) || kind == "message.dispatch"
                    && crate::decision::engine::stored_value(command)?["freshContext"] == true;
                if probe
                    && let Some(runtime) = sessions.get(&session_id).and_then(Option::as_ref)
                    && let Some(extensions) = runtime.extensions()
                {
                    prepared.context.inputs.background_work.push((
                        crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                            &provider.id,
                        )?,
                        extensions
                            .pending_background_work(
                                if runtime
                                    .session()
                                    .capabilities
                                    .sessions
                                    .supports_multiple_provider_threads_per_session
                                {
                                    Some(provider.clone())
                                } else {
                                    None
                                },
                                true,
                            )
                            .await
                            .unwrap_or(true),
                    ));
                }
            }
            for session in &projection.provider_sessions {
                let id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &session.id,
                )?;
                if !sessions.contains_key(&id) {
                    sessions.insert(id.clone(), self.sessions.get(id).await?);
                }
            }
            if let Some(current) = prepared.context.context.projection.as_mut() {
                for runtime in sessions.values().filter_map(Option::as_ref) {
                    if let Some(existing) = current
                        .provider_sessions
                        .iter_mut()
                        .find(|session| session.id == runtime.session().id)
                    {
                        *existing = runtime.session().clone();
                    } else {
                        current.provider_sessions.push(runtime.session().clone());
                    }
                }
                if matches!(
                    kind.as_str(),
                    "message.dispatch"
                        | "queued-message.promote-to-steer"
                        | "thread.model-selection.set"
                        | "provider.switch"
                ) {
                    let value = crate::decision::engine::stored_value(command)?;
                    let target = effective_selection(&value, Some(current))?.ok_or(Error::Stale)?;
                    let running = current.runs.iter().find(|run| {
                        matches!(run.status, OrchestrationV2RunStatus::Running)
                            && (value["dispatchMode"]["targetRunId"]
                                == crate::decision::engine::stored_value(&run.id)
                                    .unwrap_or_default()
                                || value["targetRunId"]
                                    == crate::decision::engine::stored_value(&run.id)
                                        .unwrap_or_default())
                    });
                    let original = running
                        .map(|run| run.model_selection.clone())
                        .unwrap_or_else(|| current.thread.model_selection.clone());
                    // The decision compares the running run's provider instance, as TS does.
                    let instance = match running {
                        Some(run) => {
                            crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                                &run.provider_instance_id,
                            )?
                        }
                        None => original.instance_id.clone(),
                    };
                    if instance == target.instance_id
                        && !crate::ports::decisions::model_switch::model_selections_equal(
                            &original, &target,
                        )
                        && let Some(session) = current.provider_sessions.iter().find(|session| {
                            session.provider_instance_id
                                == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                                    instance.clone(),
                                )
                        })
                    {
                        prepared.context.inputs.selection_transition = Some(
                            self.base
                                .providers
                                .adapter(instance.clone())
                                .await?
                                .plan_selection_transition(
                                    crate::provider::SelectionTransitionInput {
                                        current: original,
                                        target,
                                        session_capabilities: session.capabilities.clone(),
                                    },
                                )
                                .await?,
                        );
                    }
                }
            }
            prepared.context.inputs.selection_sessions = sessions
                .iter()
                .filter(|(_, runtime)| {
                    runtime
                        .as_ref()
                        .and_then(|runtime| runtime.extensions())
                        .is_some_and(|extensions| extensions.capabilities().apply_selection)
                })
                .map(|(id, _)| id.clone())
                .collect();
            prepared.context.inputs.live_sessions = sessions
                .into_iter()
                .map(|(id, runtime)| (id, runtime.is_some()))
                .collect();
        }
        Ok(prepared)
    }
}

impl ProjectionCommandInputs {
    async fn native_children(
        &self,
        thread: ThreadId,
    ) -> Result<Vec<OrchestrationV2ThreadProjection>> {
        let mut pending = vec![thread.clone()];
        let mut visited = vec![thread.clone()];
        let mut children = Vec::new();
        while let Some(id) = pending.pop() {
            let projection = match self
                .reader
                .records(
                    id.clone(),
                    vec![
                        RecordKind::Subagents,
                        RecordKind::Nodes,
                        RecordKind::ProviderTurns,
                        RecordKind::TurnItems,
                    ],
                    RecordFilter {
                        omit_settled_subagents: true,
                        omit_settled_leaf_nodes: true,
                        turn_item_types: Some(vec![
                            "command_execution".into(),
                            "dynamic_tool".into(),
                            "subagent".into(),
                            "assistant_message".into(),
                            "reasoning".into(),
                        ]),
                        turn_item_statuses: Some(vec![
                            OrchestrationV2TurnItemStatus::Pending,
                            OrchestrationV2TurnItemStatus::Running,
                            OrchestrationV2TurnItemStatus::Waiting,
                        ]),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(records) => projection_from_records(records),
                Err(Error::ThreadNotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            for task in &projection.subagents {
                let value = crate::decision::engine::stored_value(task)?;
                if value["origin"] != "provider_native" {
                    continue;
                }
                if let Some(child) = value["childThreadId"].as_str() {
                    let child = ThreadId(child.into());
                    if !visited.contains(&child) {
                        visited.push(child.clone());
                        pending.push(child);
                    }
                }
            }
            if id != thread {
                children.push(projection);
            }
        }
        Ok(children)
    }

    async fn orphan_children(
        &self,
        thread: ThreadId,
    ) -> Result<Vec<OrchestrationV2ThreadProjection>> {
        let parent = match self
            .reader
            .records(thread, vec![RecordKind::Subagents], RecordFilter::default())
            .await
        {
            Ok(records) => projection_from_records(records),
            Err(Error::ThreadNotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut children = Vec::new();
        for task in parent.subagents {
            let task = crate::decision::engine::stored_value(task)?;
            if task["origin"] != "app_owned"
                || task["childThreadId"].is_null()
                || !matches!(
                    task["status"].as_str(),
                    Some("pending" | "running" | "waiting")
                )
            {
                continue;
            }
            if children
                .iter()
                .any(|child: &OrchestrationV2ThreadProjection| {
                    crate::decision::engine::stored_value(&child.thread.id)
                        .ok()
                        .as_ref()
                        == Some(&task["childThreadId"])
                })
            {
                continue;
            }
            let id = serde_json::from_value(task["childThreadId"].clone())?;
            match self
                .reader
                .records(
                    id,
                    vec![
                        RecordKind::Runs,
                        RecordKind::ProviderThreads,
                        RecordKind::RuntimeRequests,
                        RecordKind::TurnItems,
                    ],
                    RecordFilter {
                        turn_item_types: Some(vec![
                            "command_execution".into(),
                            "dynamic_tool".into(),
                            "subagent".into(),
                        ]),
                        turn_item_statuses: Some(vec![
                            OrchestrationV2TurnItemStatus::Pending,
                            OrchestrationV2TurnItemStatus::Running,
                            OrchestrationV2TurnItemStatus::Waiting,
                        ]),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(records) => children.push(projection_from_records(records)),
                Err(Error::ThreadNotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(children)
    }

    async fn side_chats(&self, thread: ThreadId) -> Result<Vec<ThreadId>> {
        let records = match self
            .reader
            .records(
                thread.clone(),
                vec![RecordKind::ContextTransfers],
                RecordFilter::default(),
            )
            .await
        {
            Ok(records) => records,
            Err(Error::ThreadNotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let records = crate::decision::engine::stored_value(projection_from_records(records))?;
        let mut ids = Vec::new();
        for transfer in records["contextTransfers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|transfer| {
                transfer["type"] == "fork"
                    && transfer["sourceThreadId"] == serde_json::json!(thread)
            })
        {
            let id: ThreadId = serde_json::from_value(transfer["targetThreadId"].clone())?;
            if let Some(child) = self.reader.thread(id.clone()).await?
                && child.lineage.side_chat == Some(Some(true))
                && child.deleted_at.is_none()
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}

#[async_trait]
impl CommandInputs for ProjectionCommandInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        Some(self.reader.as_ref())
    }

    async fn lock_keys(&self, command: &OrchestrationV2ServerCommand) -> Result<Vec<ThreadId>> {
        let continuation =
            crate::ports::decisions::orchestrator::metadata_update_continuation(command)?;
        let command = continuation.as_ref().unwrap_or(command);
        let (_, kind, thread_id) = command_identity(command)?;
        let value = crate::decision::engine::stored_value(command)?;
        let mut keys = Vec::new();
        if let Some(source) = value["sourcePlanRef"]["threadId"].as_str() {
            keys.push(ThreadId(source.into()));
        }

        if matches!(
            kind.as_str(),
            "run.interrupt" | "thread.stop" | "thread.background-work.settle"
        ) {
            for child in self.native_children(thread_id.clone()).await? {
                keys.push(
                    crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                        &child.thread.id,
                    )?,
                );
            }
        }

        if matches!(kind.as_str(), "thread.stop" | "run.interrupt") {
            keys.extend(
                self.orphan_children(thread_id.clone())
                    .await?
                    .into_iter()
                    .map(|child| {
                        crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                            &child.thread.id,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        if kind == "thread.delete" {
            keys.extend(self.side_chats(thread_id.clone()).await?);
        }

        for name in [
            "sourceThreadId",
            "targetThreadId",
            "parentThreadId",
            "childThreadId",
        ] {
            if let Some(id) = value[name].as_str() {
                keys.push(ThreadId(id.to_owned()));
            }
        }
        if matches!(
            kind.as_str(),
            "message.dispatch" | "runtime-request.respond"
        ) && let Some(thread) = self.reader.thread(thread_id.clone()).await?
            && let Some(parent) = thread.lineage.parent_thread_id
        {
            keys.push(parent);
        }
        if kind == "subagent.stop"
            || kind == "thread.background-work.settle" && value.get("subagent").is_some()
        {
            let tasks = projection_from_records(
                self.reader
                    .records(
                        thread_id,
                        vec![RecordKind::Subagents],
                        RecordFilter::default(),
                    )
                    .await?,
            );
            if let Some(child) = tasks
                .subagents
                .iter()
                .find(|task| {
                    crate::decision::engine::stored_value(&task.id)
                        .ok()
                        .as_ref()
                        == Some(if kind == "subagent.stop" {
                            &value["subagentId"]
                        } else {
                            &value["subagent"]["id"]
                        })
                })
                .and_then(|task| task.child_thread_id.as_ref())
            {
                keys.push(
                    crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(child)?,
                );
            }
        }
        Ok(keys)
    }

    async fn prepare(&self, command: &OrchestrationV2ServerCommand) -> Result<PreparedCommand> {
        let continuation =
            crate::ports::decisions::orchestrator::metadata_update_continuation(command)?;
        let command = continuation.as_ref().unwrap_or(command);
        let (_, kind, thread_id) = command_identity(command)?;
        let value = crate::decision::engine::stored_value(command)?;
        let runtime_response = matches!(
            kind.as_str(),
            "runtime-request.respond"
                | "thread.user-input.dismiss"
                | "runtime-request.delivery.fail"
        );
        let mut inputs = DecisionInputs::default();
        let mut projection = if matches!(kind.as_str(), "thread.fork" | "thread.merge_back") {
            let source = self
                .reader
                .records(
                    serde_json::from_value(value["sourceThreadId"].clone())?,
                    vec![
                        RecordKind::Runs,
                        RecordKind::Checkpoints,
                        RecordKind::ProviderThreads,
                        RecordKind::ProviderTurns,
                        RecordKind::Attempts,
                        RecordKind::ContextTransfers,
                    ],
                    RecordFilter::default(),
                )
                .await?;
            inputs
                .related_projections
                .push(projection_from_records(source));
            if kind == "thread.fork" {
                self.reader
                    .thread(thread_id.clone())
                    .await?
                    .map(empty_projection)
            } else {
                Some(projection_from_records(
                    self.reader
                        .records(
                            thread_id.clone(),
                            vec![RecordKind::Runs, RecordKind::ContextTransfers],
                            RecordFilter::default(),
                        )
                        .await?,
                ))
            }
        } else if kind == "thread.create" {
            self.reader
                .thread(thread_id.clone())
                .await?
                .map(empty_projection)
        } else if runtime_response {
            let control = self
                .reader
                .control(ControlQuery::RuntimeResponse(serde_json::from_value(
                    value["requestId"].clone(),
                )?))
                .await?;
            match control.filter(|context| {
                context.thread.id
                    == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(thread_id.clone())
            }) {
                Some(context) => {
                    let message_answer = context.request.as_ref().is_some_and(|request| {
                        crate::decision::engine::stored_value(request)
                            .ok()
                            .is_some_and(|request| {
                                request["responseCapability"]["type"] == "message"
                            })
                    }) && kind == "runtime-request.respond"
                        && !matches!(value["decision"].as_str(), Some("decline" | "cancel"));
                    if message_answer {
                        Some(projection_from_records(
                            self.reader
                                .records(
                                    thread_id.clone(),
                                    vec![
                                        RecordKind::Runs,
                                        RecordKind::Nodes,
                                        RecordKind::Attempts,
                                        RecordKind::Messages,
                                        RecordKind::TurnItems,
                                        RecordKind::RuntimeRequests,
                                        RecordKind::ProviderSessions,
                                        RecordKind::ProviderThreads,
                                        RecordKind::ProviderTurns,
                                        RecordKind::ContextTransfers,
                                        RecordKind::Plans,
                                    ],
                                    RecordFilter::default(),
                                )
                                .await?,
                        ))
                    } else {
                        Some(projection_from_control(context))
                    }
                }
                None => self
                    .reader
                    .thread(thread_id.clone())
                    .await?
                    .map(empty_projection),
            }
        } else {
            let (fields, filter) = crate::decision::engine::required_records(command)?;
            let records = if matches!(
                kind.as_str(),
                "message.dispatch"
                    | "queued-run.edit"
                    | "queued-run.reorder"
                    | "queue.resume"
                    | "queued-run.start"
                    | "queued-message.promote-to-steer"
                    | "queued-run.cancel"
                    | "notification.delivery.accept"
                    | "delegated_task.remote.complete"
                    | "delegated_task.wake-policy"
                    | "delegated_task.completion-delivery.acknowledge"
                    | "delegated_task.completion-delivery.dispose"
            ) {
                self.reader
                    .query_records(ProjectionRecordQuery {
                        thread_id: thread_id.clone(),
                        fields,
                        filter,
                        messages: MessageFilter {
                            roles: Some(vec![ConversationMessageRole::User]),
                            run_ids: if kind == "queued-run.edit" {
                                Some(vec![serde_json::from_value(value["runId"].clone())?])
                            } else {
                                None
                            },
                            ..Default::default()
                        },
                        run_statuses: None,
                        include_latest_run: false,
                    })
                    .await?
            } else {
                self.reader
                    .records(thread_id.clone(), fields, filter)
                    .await?
            };
            Some(projection_from_records(records))
        };
        if kind == "thread.snooze"
            && let Some(current) = projection.as_mut()
            && let Some(latest) =
                crate::ports::decisions::model_switch::latest_executed_run(&current.runs)
                    .filter(|run| run.status == OrchestrationV2RunStatus::Failed)
        {
            let id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                &latest.id,
            )?;
            let evidence = projection_from_records(
                self.reader
                    .records(
                        thread_id.clone(),
                        vec![RecordKind::TurnItems, RecordKind::ProviderSessions],
                        RecordFilter {
                            turn_item_types: Some(vec!["error".into()]),
                            turn_item_run_ids: Some(vec![Some(id)]),
                            ..Default::default()
                        },
                    )
                    .await?,
            );
            current.turn_items.extend(evidence.turn_items);
            current.provider_sessions = evidence.provider_sessions;
        }
        if matches!(
            kind.as_str(),
            "message.dispatch" | "runtime-request.respond"
        ) && let Some(child) = &projection
            && child.thread.lineage.relationship_to_parent
                == Some(AppThreadLineageRelationshipToParent::Subagent)
            && let Some(parent) = child.thread.lineage.parent_thread_id.clone()
        {
            let mut filter = RecordFilter::default();
            let child_value = crate::decision::engine::stored_value(&child.thread)?;
            filter.subagent_ids = Some(if child_value["forkedFrom"]["type"] == "node" {
                vec![serde_json::from_value(
                    child_value["forkedFrom"]["nodeId"].clone(),
                )?]
            } else {
                Vec::new()
            });
            if !value["notification"].is_null()
                && let Some(task) = child
                    .runs
                    .iter()
                    .filter(|run| {
                        run.delegated_task_id
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_some()
                    })
                    .max_by_key(|run| run.ordinal)
                    .and_then(|run| run.delegated_task_id.as_ref().and_then(Option::as_ref))
            {
                filter.subagent_ids.as_mut().ok_or(Error::Stale)?.push(
                    crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(task)?,
                );
            }
            inputs.related_projections.push(projection_from_records(
                self.reader
                    .records(
                        parent.clone(),
                        vec![
                            RecordKind::Runs,
                            RecordKind::Subagents,
                            RecordKind::ProviderTurns,
                            RecordKind::Attempts,
                        ],
                        filter,
                    )
                    .await?,
            ));
            inputs.next_item_ordinals.push((
                parent.clone(),
                None,
                self.reader.next_turn_item_ordinal(parent, None).await?,
            ));
        }
        if let Some(reference) = value.get("sourcePlanRef") {
            let source: ThreadId = serde_json::from_value(reference["threadId"].clone())?;
            if let Some(plan) = self
                .reader
                .plan(
                    source.clone(),
                    serde_json::from_value(reference["planId"].clone())?,
                )
                .await?
            {
                if source == thread_id {
                    if let Some(projection) = projection.as_mut() {
                        projection.plans.push(plan);
                    }
                } else if let Some(thread) = self.reader.thread(source).await? {
                    let mut related = empty_projection(thread);
                    related.plans.push(plan);
                    inputs.related_projections.push(related);
                }
            }
        }
        if matches!(
            kind.as_str(),
            "run.interrupt" | "thread.stop" | "thread.background-work.settle"
        ) {
            inputs
                .related_projections
                .extend(self.native_children(thread_id.clone()).await?);
        }
        if matches!(kind.as_str(), "thread.stop" | "run.interrupt") {
            inputs
                .related_projections
                .extend(self.orphan_children(thread_id.clone()).await?);
        }
        if kind == "thread.created.record" {
            let target = self
                .reader
                .records(
                    serde_json::from_value(value["targetThreadId"].clone())?,
                    vec![RecordKind::Runs],
                    RecordFilter::default(),
                )
                .await?;
            inputs
                .related_projections
                .push(projection_from_records(target));
        }
        if (kind == "subagent.stop"
            || kind == "thread.background-work.settle" && value.get("subagent").is_some())
            && let Some(child) = projection
                .as_ref()
                .and_then(|projection| {
                    projection.subagents.iter().find(|task| {
                        crate::decision::engine::stored_value(&task.id)
                            .ok()
                            .as_ref()
                            == Some(if kind == "subagent.stop" {
                                &value["subagentId"]
                            } else {
                                &value["subagent"]["id"]
                            })
                    })
                })
                .and_then(|task| task.child_thread_id.as_ref())
        {
            let child =
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(child)?;
            if self.reader.thread(child.clone()).await?.is_some() {
                inputs.related_projections.push(projection_from_records(
                    self.reader
                        .records(
                            child,
                            if kind == "subagent.stop" {
                                vec![RecordKind::Runs]
                            } else {
                                vec![
                                    RecordKind::Nodes,
                                    RecordKind::ProviderTurns,
                                    RecordKind::TurnItems,
                                ]
                            },
                            RecordFilter::default(),
                        )
                        .await?,
                ));
            }
        }
        if (kind == "runtime-request.respond" && value.get("feedback").is_some())
            || matches!(
                kind.as_str(),
                "runtime-request.delivery.fail"
                    | "prepared-run.fail"
                    | "secret_request.record"
                    | "thread.created.record"
                    | "delegated_task.remote.request"
                    | "message.dispatch"
            )
        {
            inputs.next_item_ordinals.push((
                thread_id.clone(),
                None,
                self.reader
                    .next_turn_item_ordinal(thread_id.clone(), None)
                    .await?,
            ));
        }
        if kind == "thread.mark-unread" {
            inputs.shell_completions.push((
                thread_id.clone(),
                self.reader
                    .shell(thread_id.clone())
                    .await?
                    .and_then(|shell| shell.latest_run_completed_at.flatten()),
            ));
        }
        let capabilities = if matches!(
            kind.as_str(),
            "message.dispatch"
                | "queued-message.promote-to-steer"
                | "queued-run.edit"
                | "checkpoint.rollback"
                | "thread.model-selection.set"
                | "provider.switch"
        ) || kind == "runtime-request.respond"
            && projection.as_ref().is_some_and(|projection| {
                projection.runtime_requests.iter().any(|request| {
                    crate::decision::engine::stored_value(request)
                        .ok()
                        .is_some_and(|request| request["responseCapability"]["type"] == "message")
                })
            })
            || (matches!(
                kind.as_str(),
                "link_request.record" | "thread.pull-request-watch.sync" | "delegated_task.request"
            ) && value.get("wake").is_some()
                || kind == "delegated_task.request")
        {
            let selection = effective_selection(&value, projection.as_ref())?
                .map(crate::decision::engine::stored_value)
                .transpose()?;
            if let Some(instance) =
                selection.and_then(|selection| selection["instanceId"].as_str().map(str::to_owned))
            {
                let instance = t3_contracts::provider_instance::ProviderInstanceId(instance);
                let metadata = self.providers.metadata(instance.clone()).await?;
                let capabilities = metadata.capabilities.clone();
                inputs.provider_metadata.push((instance, metadata));
                if let Some(projection) = &projection {
                    let current =
                        crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                            &projection.thread.provider_instance_id,
                        )?;
                    if !inputs
                        .provider_metadata
                        .iter()
                        .any(|(instance, _)| instance == &current)
                        && let Ok(metadata) = self.providers.metadata(current.clone()).await
                    {
                        inputs.provider_metadata.push((current, metadata));
                    }
                }

                Some(capabilities)
            } else {
                None
            }
        } else {
            None
        };
        if kind == "thread.delete" {
            inputs.attachment_ids.push((
                thread_id.clone(),
                self.reader.attachment_ids(thread_id.clone()).await?,
            ));
            for child in self.side_chats(thread_id.clone()).await? {
                let (fields, filter) = crate::decision::engine::required_records(command)?;
                inputs.related_projections.push(projection_from_records(
                    self.reader.records(child.clone(), fields, filter).await?,
                ));
                inputs
                    .attachment_ids
                    .push((child.clone(), self.reader.attachment_ids(child).await?));
            }
        }
        let related_count = inputs
            .related_projections
            .iter()
            .map(|projection| {
                projection.runs.len() * 2
                    + projection.attempts.len()
                    + projection.nodes.len()
                    + projection.runtime_requests.len()
                    + projection.subagents.len()
                    + projection.provider_sessions.len()
                    + 1
            })
            .sum::<usize>();
        let event_count = related_count
            + projection.as_ref().map_or(16, |projection| {
                32 + 4
                    * (projection.runs.len()
                        + projection.attempts.len()
                        + projection.nodes.len()
                        + projection.subagents.len()
                        + projection.provider_sessions.len()
                        + projection.provider_threads.len()
                        + projection.provider_turns.len()
                        + projection.runtime_requests.len()
                        + projection.turn_items.len()
                        + projection.context_transfers.len())
            });
        let mut ids = Vec::with_capacity(event_count + 24);
        for _ in 0..event_count {
            ids.push((IdKind::Event, self.ids.next(IdKind::Event)));
        }
        for kind in [
            IdKind::Run,
            IdKind::RunAttempt,
            IdKind::Node,
            IdKind::ProviderSession,
            IdKind::ProviderThread,
            IdKind::ProviderTurn,
            IdKind::Checkpoint,
            IdKind::CheckpointScope,
            IdKind::ContextHandoff,
            IdKind::RuntimeRequest,
            IdKind::Thread,
        ] {
            ids.push((kind, self.ids.next(kind)));
        }
        for kind in [IdKind::Message, IdKind::ContextTransfer] {
            for _ in 0..projection
                .as_ref()
                .map_or(1, |projection| projection.subagents.len() + 1)
            {
                ids.push((kind, self.ids.next(kind)));
            }
        }
        for _ in 0..8 {
            ids.push((IdKind::TurnItem, self.ids.next(IdKind::TurnItem)));
        }
        Ok(PreparedCommand {
            context: DecisionContext {
                projection,
                now: self.clock.now(),
                ids,
                capabilities,
            }
            .with_inputs(inputs),
            guard: CommitGuard::None,
        })
    }
}

fn effective_selection(
    value: &serde_json::Value,
    projection: Option<&OrchestrationV2ThreadProjection>,
) -> Result<Option<t3_contracts::model_selection::ModelSelection>> {
    if let Some(selection) = value
        .get("modelSelection")
        .filter(|selection| !selection.is_null())
    {
        return Ok(Some(serde_json::from_value(selection.clone())?));
    }
    let Some(projection) = projection else {
        return Ok(None);
    };
    if matches!(
        value["type"].as_str(),
        Some("queued-run.edit" | "queued-message.promote-to-steer")
    ) && let Some(run) = projection.runs.iter().find(|run| {
        crate::decision::engine::stored_value(&run.id)
            .ok()
            .is_some_and(|id| id == value["runId"] || id == value["queuedRunId"])
    }) {
        return crate::ports::decisions::orchestrator::queued_run_model_selection(projection, run)
            .map(Some);
    }
    if value["type"] == "message.dispatch"
        && value["notification"].is_null()
        && projection.runs.iter().any(|run| {
            matches!(
                run.status,
                OrchestrationV2RunStatus::Preparing
                    | OrchestrationV2RunStatus::Starting
                    | OrchestrationV2RunStatus::Running
                    | OrchestrationV2RunStatus::Waiting
            )
        })
        && !crate::ports::decisions::orchestrator::dispatch_follows_current(value)
    {
        return crate::ports::decisions::orchestrator::queue_tail_model_selection(projection)
            .map(Some);
    }
    Ok(Some(projection.thread.model_selection.clone()))
}

fn projection_from_control(context: ControlContext) -> OrchestrationV2ThreadProjection {
    let mut projection = empty_projection(context.thread);
    projection.runs.extend(context.run);
    projection.attempts.extend(context.attempt);
    projection.nodes.extend(context.node);
    projection
        .provider_sessions
        .extend(context.provider_session);
    projection.provider_threads.extend(context.provider_thread);
    projection.provider_turns.extend(context.provider_turn);
    projection.runtime_requests.extend(context.request);
    projection.messages.extend(context.message);
    projection.turn_items.extend(context.item);
    projection
}

fn empty_projection(thread: OrchestrationV2AppThread) -> OrchestrationV2ThreadProjection {
    OrchestrationV2ThreadProjection {
        updated_at: thread.updated_at.clone(),
        thread,
        runs: Vec::new(),
        attempts: Vec::new(),
        nodes: Vec::new(),
        subagents: Vec::new(),
        provider_sessions: Vec::new(),
        provider_threads: Vec::new(),
        provider_turns: Vec::new(),
        runtime_requests: Vec::new(),
        messages: Vec::new(),
        plans: Vec::new(),
        turn_items: Vec::new(),
        checkpoint_scopes: Vec::new(),
        checkpoints: Vec::new(),
        context_handoffs: Vec::new(),
        context_transfers: Vec::new(),
        visible_turn_items: Vec::new(),
    }
}

pub fn projection_from_records(records: ProjectionRecords) -> OrchestrationV2ThreadProjection {
    let mut projection = empty_projection(records.thread);
    for record in records.records {
        match record {
            ProjectionRecordSet::Runs(value) => projection.runs = value,
            ProjectionRecordSet::Attempts(value) => projection.attempts = value,
            ProjectionRecordSet::Nodes(value) => projection.nodes = value,
            ProjectionRecordSet::Subagents(value) => projection.subagents = value,
            ProjectionRecordSet::ProviderSessions(value) => projection.provider_sessions = value,
            ProjectionRecordSet::ProviderThreads(value) => projection.provider_threads = value,
            ProjectionRecordSet::ProviderTurns(value) => projection.provider_turns = value,
            ProjectionRecordSet::RuntimeRequests(value) => projection.runtime_requests = value,
            ProjectionRecordSet::Messages(value) => projection.messages = value,
            ProjectionRecordSet::Plans(value) => projection.plans = value,
            ProjectionRecordSet::TurnItems(value) => projection.turn_items = value,
            ProjectionRecordSet::CheckpointScopes(value) => projection.checkpoint_scopes = value,
            ProjectionRecordSet::Checkpoints(value) => projection.checkpoints = value,
            ProjectionRecordSet::ContextHandoffs(value) => projection.context_handoffs = value,
            ProjectionRecordSet::ContextTransfers(value) => projection.context_transfers = value,
        }
    }
    projection
}

pub struct DispatchServices {
    pub database: StoreDatabase,
    pub inputs: Arc<dyn CommandInputs>,
    pub engine: Arc<dyn DecisionEngine>,
    pub committer: Arc<dyn EventCommitter>,
    pub recovery: Arc<dyn RecoveryService>,
    pub clock: Arc<dyn Clock>,
}

struct Inner {
    services: DispatchServices,
    lanes: Arc<KeyedLanes>,
    intake: KeyedLanes,
    projects: KeyedLanes,
    ready: OnceCell<()>,
    ceiling: Option<RuntimeModeCeiling>,
    queue_failure_formatter: std::sync::OnceLock<Arc<dyn QueueStartFailureFormatter>>,
    dispatch_observer: std::sync::OnceLock<Arc<dyn DispatchObserver>>,
    background_seen: std::sync::Mutex<std::collections::HashSet<String>>,
    continuations: std::sync::OnceLock<
        tokio::sync::mpsc::Sender<
            crate::ports::sessions_recovery::provider_continuation_service::ContinuationRequest,
        >,
    >,
}

#[derive(Clone)]
pub struct Dispatcher(Arc<Inner>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedRecoveryPhase {
    SubagentResults,
    DelegatedCompletions,
}

#[derive(Debug)]
pub struct DelegatedRecoveryFailure {
    pub phase: DelegatedRecoveryPhase,
    pub thread_id: Option<ThreadId>,
    pub cause: Error,
}

#[derive(Debug, Default)]
pub struct DelegatedRecoveryReport {
    pub subagent_result_threads: usize,
    pub completion_threads: usize,
    pub failures: Vec<DelegatedRecoveryFailure>,
}

pub struct DispatchThreadOperations {
    pub base: Arc<dyn ThreadOperations>,
    pub dispatcher: Dispatcher,
}

pub struct PreparedContinuations {
    pub service: Arc<
        crate::ports::sessions_recovery::provider_continuation_service::ProviderContinuationService,
    >,
    pub requests: tokio::sync::mpsc::Receiver<
        crate::ports::sessions_recovery::provider_continuation_service::ContinuationRequest,
    >,
}

#[async_trait]
impl ThreadOperations for DispatchThreadOperations {
    async fn start(&self, thread: ThreadId, run: RunId) -> Result<()> {
        self.base.start(thread, run).await
    }
    async fn start_with_options(
        &self,
        thread: ThreadId,
        run: RunId,
        options: EffectExecutionOptions,
    ) -> Result<()> {
        self.base.start_with_options(thread, run, options).await
    }
    async fn finalize(&self, thread: ThreadId, run: RunId, scope: CheckpointScopeId) -> Result<()> {
        self.base.finalize(thread, run, scope).await
    }
    async fn continue_run(&self, thread: ThreadId, source: RunId) -> Result<()> {
        self.base.continue_run(thread, source).await
    }
    async fn continue_run_with_options(
        &self,
        thread: ThreadId,
        source: RunId,
        options: EffectExecutionOptions,
    ) -> Result<()> {
        self.base
            .continue_run_with_options(thread, source, options)
            .await
    }
    async fn stop_delegated(&self, thread: ThreadId, reason: Option<String>) -> Result<()> {
        self.base.stop_delegated(thread, reason).await
    }
    async fn stop_delegated_for_effect(
        &self,
        thread: ThreadId,
        command: CommandId,
        reason: Option<String>,
    ) -> Result<()> {
        self.base
            .stop_delegated_for_effect(thread, command, reason)
            .await
    }
    async fn generate_title(
        &self,
        thread: ThreadId,
        kind: crate::effects::TitleKind,
    ) -> Result<()> {
        self.base.generate_title(thread, kind).await
    }
    async fn generate_title_for_effect(
        &self,
        thread: ThreadId,
        command: CommandId,
        kind: crate::effects::TitleKind,
    ) -> Result<()> {
        self.base
            .generate_title_for_effect(thread, command, kind)
            .await
    }
    async fn recover_delegated_task(
        &self,
        thread: ThreadId,
        source: Option<RunId>,
        completed: Option<RunId>,
    ) -> Result<()> {
        self.dispatcher
            .recover_delegated_task(thread, source, completed)
            .await
    }
    async fn resume_queue(&self, thread: ThreadId, selected: Option<RunId>) -> Result<()> {
        self.dispatcher.resume_queue(thread, selected).await
    }
}

impl Dispatcher {
    pub fn prepare_continuations(
        &self,
        reader: Arc<dyn ProjectionReader>,
        ids: Arc<dyn IdGenerator>,
        capacity: std::num::NonZeroUsize,
    ) -> Result<PreparedContinuations> {
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity.get());
        self.set_continuation_requests(sender)?;
        let service=crate::ports::sessions_recovery::provider_continuation_service::ProviderContinuationService::new(
            reader,
            Arc::new(self.clone()),self.0.services.committer.clone(),ids,
        );
        Ok(PreparedContinuations {
            service: Arc::new(service),
            requests: receiver,
        })
    }

    pub async fn recover_delegated_tasks(&self) -> DelegatedRecoveryReport {
        let inner = self.0.clone();
        match tokio::spawn(async move { inner.recover_delegated_tasks().await }).await {
            Ok(report) => report,
            Err(cause) => DelegatedRecoveryReport {
                failures: vec![DelegatedRecoveryFailure {
                    phase: DelegatedRecoveryPhase::SubagentResults,
                    thread_id: None,
                    cause: Error::Service {
                        operation: "Dispatcher.recover_delegated_tasks",
                        detail: cause.to_string(),
                    },
                }],
                ..Default::default()
            },
        }
    }

    pub async fn list_active_threads(
        &self,
        project: Option<ProjectId>,
    ) -> Result<Vec<crate::ports::decisions::orchestrator::ActiveThreadListEntry>> {
        let reader = self
            .0
            .services
            .inputs
            .reader()
            .ok_or(Error::Unsupported("active list requires reader"))?;
        read_active_thread_metadata(reader, project).await
    }

    pub async fn delegated_task_result_pending(
        &self,
        thread: ThreadId,
        task: Option<NodeId>,
    ) -> Result<bool> {
        let reader = self
            .0
            .services
            .inputs
            .reader()
            .ok_or(Error::Unsupported("delegated pending requires reader"))?;
        let child = projection_from_records(
            reader
                .query_records(ProjectionRecordQuery {
                    thread_id: thread.clone(),
                    fields: vec![
                        RecordKind::Runs,
                        RecordKind::Messages,
                        RecordKind::Subagents,
                        RecordKind::ProviderThreads,
                        RecordKind::ProviderTurns,
                        RecordKind::Attempts,
                    ],
                    filter: RecordFilter::default(),
                    messages: MessageFilter {
                        roles: Some(vec![ConversationMessageRole::User]),
                        ..Default::default()
                    },
                    run_statuses: None,
                    include_latest_run: false,
                })
                .await?,
        );
        let pending =
            load_restart_continuations(&self.0.services.database, std::slice::from_ref(&child))
                .await?;
        let restarts = pending
            .iter()
            .find(|(id, _)| *id == thread)
            .map(|(_, ids)| ids.as_slice())
            .ok_or(Error::Stale)?;
        crate::ports::decisions::orchestrator::delegated_result_pending(
            &child,
            task.as_ref(),
            restarts,
        )
    }

    pub fn new(services: DispatchServices, ceiling: Option<RuntimeModeCeiling>) -> Self {
        Self::with_command_lanes(services, ceiling, Arc::new(KeyedLanes::default()))
    }

    pub fn with_command_lanes(
        services: DispatchServices,
        ceiling: Option<RuntimeModeCeiling>,
        lanes: Arc<KeyedLanes>,
    ) -> Self {
        Self(Arc::new(Inner {
            services,
            lanes,
            intake: KeyedLanes::default(),
            projects: KeyedLanes::default(),
            ready: OnceCell::new(),
            ceiling,
            continuations: std::sync::OnceLock::new(),
            queue_failure_formatter: std::sync::OnceLock::new(),
            dispatch_observer: std::sync::OnceLock::new(),
            background_seen: std::sync::Mutex::new(std::collections::HashSet::new()),
        }))
    }

    pub fn set_queue_start_failure_formatter(
        &self,
        formatter: Arc<dyn QueueStartFailureFormatter>,
    ) -> Result<()> {
        self.0
            .queue_failure_formatter
            .set(formatter)
            .map_err(|_| Error::Service {
                operation: "Dispatcher.set_queue_start_failure_formatter",
                detail: "Queue failure formatter is already attached.".into(),
            })
    }

    pub fn set_dispatch_observer(&self, observer: Arc<dyn DispatchObserver>) -> Result<()> {
        self.0
            .dispatch_observer
            .set(observer)
            .map_err(|_| Error::Service {
                operation: "Dispatcher.set_dispatch_observer",
                detail: "Dispatch observer is already attached.".into(),
            })
    }

    pub fn set_continuation_requests(
        &self,
        requests: tokio::sync::mpsc::Sender<
            crate::ports::sessions_recovery::provider_continuation_service::ContinuationRequest,
        >,
    ) -> Result<()> {
        self.0
            .continuations
            .set(requests)
            .map_err(|_| Error::Service {
                operation: "Dispatcher.set_continuation_requests",
                detail: "Continuation queue is already attached.".into(),
            })
    }

    pub async fn resume_queue(
        &self,
        thread_id: ThreadId,
        selected_run_id: Option<RunId>,
    ) -> Result<()> {
        let inner = self.0.clone();
        tokio::spawn(async move { inner.resume_queue(thread_id, selected_run_id, None).await })
            .await
            .map_err(|error| Error::Service {
                operation: "Dispatcher.resume_queue",
                detail: error.to_string(),
            })?
    }

    pub async fn handle_run_update(&self, stored: &OrchestrationV2StoredEvent) -> Result<()> {
        let value = crate::decision::engine::stored_value(stored)?;
        if value["event"]["type"] != "run.updated"
            || stored
                .command_id
                .as_ref()
                .is_some_and(|id| id.0.starts_with("command:runtime-reconcile:"))
        {
            return Ok(());
        }
        let event = &value["event"];
        let run = &event["payload"];
        let terminal = matches!(
            run["status"].as_str(),
            Some("completed" | "interrupted" | "failed" | "cancelled" | "rolled_back")
        );
        if !terminal
            && (run["delegatedTaskId"].is_null()
                || !matches!(run["status"].as_str(), Some("queued" | "starting")))
        {
            return Ok(());
        }
        let thread: ThreadId = serde_json::from_value(event["threadId"].clone())?;
        let id: RunId = serde_json::from_value(run["id"].clone())?;
        let inner = self.0.clone();
        tokio::spawn(async move {
            if !terminal {
                return inner.sync_delegated_run(thread, id).await;
            }
            let lanes = inner.lanes.acquire([thread.0.clone()]).await;
            let prepared = inner.prepare_delegated_recovery(thread.clone()).await?;
            let decision = crate::decision::engine::PureDecisionEngine
                .decide_after_run(&prepared.context, &id)?;
            inner.commit_recovery(decision).await?;
            drop(lanes);
            inner
                .resume_queue(thread.clone(), None, Some(id.clone()))
                .await?;
            inner.recover_delegated_task(thread, None, Some(id)).await
        })
        .await
        .map_err(|error| Error::Service {
            operation: "Dispatcher.handle_run_update",
            detail: error.to_string(),
        })?
    }

    pub async fn handle_child_work_update(
        &self,
        stored: &OrchestrationV2StoredEvent,
    ) -> Result<()> {
        let value = crate::decision::engine::stored_value(stored)?;
        let event = &value["event"];
        let payload = &event["payload"];
        let settled = if event["type"] == "subagent.updated" {
            !matches!(
                payload["status"].as_str(),
                Some("pending" | "running" | "waiting")
            )
        } else if event["type"] == "provider-thread.updated" {
            let id = payload["id"].as_str().ok_or(Error::Stale)?.to_owned();
            let holding = payload["pendingBackgroundTasks"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|task| {
                    task["awaited"] == true
                        || matches!(task["kind"].as_str(), Some("subagent" | "background_task"))
                });
            let mut seen = self
                .0
                .background_seen
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if holding {
                seen.insert(id);
                false
            } else {
                seen.remove(&id)
            }
        } else {
            false
        };
        if settled {
            self.recover_delegated_task(
                serde_json::from_value(event["threadId"].clone())?,
                None,
                None,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn handle_provider_turn_update(
        &self,
        stored: &OrchestrationV2StoredEvent,
    ) -> Result<()> {
        let value = crate::decision::engine::stored_value(stored)?;
        let event = &value["event"];
        let turn = &event["payload"];
        if event["type"] != "provider-turn.updated" || turn["status"] != "running" {
            return Ok(());
        }
        let thread: ThreadId = serde_json::from_value(event["threadId"].clone())?;
        let inner = self.0.clone();
        let turn = turn.clone();
        tokio::spawn(async move {
            let _lane=inner.lanes.acquire([thread.0.clone()]).await;
            let reader=inner.services.inputs.reader().ok_or(Error::Unsupported("deferred steer requires reader"))?;
            let projection=projection_from_records(reader.query_records(ProjectionRecordQuery{
                thread_id:thread.clone(),fields:vec![RecordKind::Runs],filter:RecordFilter::default(),messages:MessageFilter::default(),
                run_statuses:Some(vec![OrchestrationV2RunStatus::Queued,OrchestrationV2RunStatus::Running]),include_latest_run:false,
            }).await?);
            let Some(target)=projection.runs.iter().find(|run|run.status==OrchestrationV2RunStatus::Running && crate::decision::engine::stored_value(&run.active_attempt_id).ok().as_ref()==Some(&turn["runAttemptId"])) else {return Ok(());};
            let target=crate::decision::engine::stored_value(target)?;
            for queued in crate::ports::decisions::queued_run_order::queued_runs_in_delivery_order(&projection) {
                let queued=crate::decision::engine::stored_value(queued)?;
                if queued["steerWhenReadyRunId"]!=target["id"] {continue;}
                inner.commit_locked_command(serde_json::from_value(serde_json::json!({"type":"queued-message.promote-to-steer","commandId":format!("command:deferred-steer:{}:{}:{}",queued["id"].as_str().ok_or(Error::Stale)?,target["id"].as_str().ok_or(Error::Stale)?,turn["runAttemptId"].as_str().ok_or(Error::Stale)?),"threadId":thread,"queuedRunId":queued["id"],"targetRunId":target["id"]}))?).await?;
            }
            Ok(())
        }).await.map_err(|error|Error::Service{operation:"Dispatcher.handle_provider_turn_update",detail:error.to_string()})?
    }

    pub async fn recover_delegated_task(
        &self,
        thread_id: ThreadId,
        source_run_id: Option<RunId>,
        completed_run_id: Option<RunId>,
    ) -> Result<()> {
        let inner = self.0.clone();
        tokio::spawn(async move {
            inner
                .recover_delegated_task(thread_id, source_run_id, completed_run_id)
                .await
        })
        .await
        .map_err(|error| Error::Service {
            operation: "Dispatcher.recover_delegated_task",
            detail: error.to_string(),
        })?
    }

    pub async fn dispatch_with_limit(
        &self,
        command: OrchestrationV2ServerCommand,
        limit: Option<DispatchModes>,
    ) -> Result<CommandReceipt> {
        let inner = self.0.clone();
        let (_, command_type, thread_id) = command_identity(&command)?;
        let intake = if command_type == "message.dispatch"
            || crate::ports::decisions::orchestrator::metadata_update_continuation(&command)?
                .is_some()
        {
            inner.intake.acquire([thread_id.0]).await
        } else {
            Vec::new()
        };
        // A caller dropping its future must not release the lane before a blocking commit finishes.
        tokio::spawn(async move {
            let _intake = intake;
            inner.dispatch(command, limit).await
        })
        .await
        .map_err(|error| Error::Service {
            operation: "CommandDispatcher.dispatch",
            detail: error.to_string(),
        })?
    }

    pub async fn commit_project(
        &self,
        event: ApplicationProjectEvent,
        receipt: CommandReceipt,
    ) -> Result<CommandReceipt> {
        let inner = self.0.clone();
        tokio::spawn(async move {
            inner.admit().await?;
            let AggregateId::Project(project) = &receipt.aggregate else {
                return Err(Error::Service {
                    operation: "CommandDispatcher.commit_project",
                    detail: "Expected a project receipt.".into(),
                });
            };
            let _guard = inner.projects.acquire([project.0.clone()]).await;
            if let Some(existing) = inner.receipt(&receipt.command_id).await? {
                return validate_receipt(existing, &receipt.aggregate);
            }
            validate_receipt(
                inner
                    .services
                    .committer
                    .commit_project(event, receipt.clone())
                    .await?,
                &receipt.aggregate,
            )
        })
        .await
        .map_err(|error| Error::Service {
            operation: "CommandDispatcher.commit_project",
            detail: error.to_string(),
        })?
    }
}

#[async_trait]
impl CommandDispatcher for Dispatcher {
    fn command_lanes(&self) -> Result<Arc<KeyedLanes>> {
        Ok(self.0.lanes.clone())
    }
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        self.dispatch_with_limit(command, None).await
    }
}

impl Inner {
    async fn recover_delegated_tasks(self: Arc<Self>) -> DelegatedRecoveryReport {
        let mut report = DelegatedRecoveryReport::default();
        if let Err(cause) = self.admit().await {
            report.failures.push(DelegatedRecoveryFailure {
                phase: DelegatedRecoveryPhase::SubagentResults,
                thread_id: None,
                cause,
            });
            return report;
        }
        for phase in [
            DelegatedRecoveryPhase::SubagentResults,
            DelegatedRecoveryPhase::DelegatedCompletions,
        ] {
            let candidates = self
                .services
                .database
                .read(move |db| {
                    crate::projections::read::recovery_thread_ids(
                        db,
                        match phase {
                            DelegatedRecoveryPhase::SubagentResults => {
                                crate::projections::read::RecoveryKind::SubagentResults
                            }
                            DelegatedRecoveryPhase::DelegatedCompletions => {
                                crate::projections::read::RecoveryKind::DelegatedCompletions
                            }
                        },
                        None,
                        None,
                    )
                })
                .await;
            let threads = match candidates {
                Ok(threads) => threads,
                Err(cause) => {
                    report.failures.push(DelegatedRecoveryFailure {
                        phase,
                        thread_id: None,
                        cause,
                    });
                    continue;
                }
            };
            match phase {
                DelegatedRecoveryPhase::SubagentResults => {
                    report.subagent_result_threads = threads.len()
                }
                DelegatedRecoveryPhase::DelegatedCompletions => {
                    report.completion_threads = threads.len()
                }
            }
            for batch in threads.chunks(8) {
                let mut tasks = tokio::task::JoinSet::new();
                for thread in batch {
                    let thread = thread.clone();
                    let inner = self.clone();
                    tasks.spawn(async move {
                        let result = match phase {
                            DelegatedRecoveryPhase::SubagentResults => {
                                inner
                                    .recover_delegated_task_inner(thread.clone(), None, None, true)
                                    .await
                            }
                            DelegatedRecoveryPhase::DelegatedCompletions => {
                                inner.recover_completion_thread(thread.clone()).await
                            }
                        };
                        (thread, result)
                    });
                }
                while let Some(result) = tasks.join_next().await {
                    match result {
                        Ok((thread, Err(cause))) => {
                            report.failures.push(DelegatedRecoveryFailure {
                                phase,
                                thread_id: Some(thread),
                                cause,
                            })
                        }
                        Err(cause) => report.failures.push(DelegatedRecoveryFailure {
                            phase,
                            thread_id: None,
                            cause: Error::Service {
                                operation: "Dispatcher.recover_delegated_tasks",
                                detail: cause.to_string(),
                            },
                        }),
                        Ok((_, Ok(()))) => {}
                    }
                }
            }
        }
        report
    }

    async fn sync_delegated_run(&self, thread: ThreadId, id: RunId) -> Result<()> {
        let reader = self.services.inputs.reader().ok_or(Error::Unsupported(
            "delegated synchronization requires reader",
        ))?;
        let Some(child) = reader.thread(thread.clone()).await? else {
            return Ok(());
        };
        let Some(parent) = child.lineage.parent_thread_id else {
            return Ok(());
        };
        if child.lineage.relationship_to_parent
            != Some(AppThreadLineageRelationshipToParent::Subagent)
        {
            return Ok(());
        }
        let _lanes = self
            .lanes
            .acquire([thread.0.clone(), parent.0.clone()])
            .await;
        let child = projection_from_records(
            reader
                .records(
                    thread,
                    vec![RecordKind::Runs],
                    RecordFilter {
                        run_ids: Some(vec![id.clone()]),
                        ..Default::default()
                    },
                )
                .await?,
        );
        let prepared = self.prepare_delegated_recovery(parent).await?;
        self.commit_recovery(
            crate::decision::engine::PureDecisionEngine.decide_delegated_sync(
                &prepared.context,
                &child,
                &id,
            )?,
        )
        .await
    }

    async fn recover_delegated_task(
        &self,
        thread_id: ThreadId,
        source: Option<RunId>,
        completed: Option<RunId>,
    ) -> Result<()> {
        self.recover_delegated_task_inner(thread_id, source, completed, false)
            .await
    }

    async fn recover_delegated_task_inner(
        &self,
        thread_id: ThreadId,
        source: Option<RunId>,
        completed: Option<RunId>,
        startup: bool,
    ) -> Result<()> {
        self.admit().await?;
        let reader = self.services.inputs.reader().ok_or(Error::Unsupported(
            "delegated recovery requires the projection reader",
        ))?;
        if let Some(completed) = completed {
            let lane = self.lanes.acquire([thread_id.0.clone()]).await;
            if reader
                .run_message(thread_id.clone(), completed.clone())
                .await?
                .is_some_and(|message| message.delegated_completion.is_some())
            {
                let prepared = self.prepare_delegated_recovery(thread_id.clone()).await?;
                let decision = crate::decision::engine::PureDecisionEngine
                    .decide_delegated_recovery(
                        &prepared.context,
                        None,
                        Some(&completed),
                        None,
                        &[],
                    )?;
                self.commit_recovery(decision).await?;
            }
            drop(lane);
            self.offer_deliveries(thread_id.clone()).await?;
        }
        let Some(thread) = reader.thread(thread_id.clone()).await? else {
            return Ok(());
        };
        let value = crate::decision::engine::stored_value(&thread)?;
        if value["lineage"]["relationshipToParent"] != "subagent"
            || value["forkedFrom"]["type"] != "node"
        {
            return Ok(());
        }
        let parent: ThreadId = serde_json::from_value(value["lineage"]["parentThreadId"].clone())?;
        if !startup
            && source.is_none()
            && !reader
                .pending_delegated_completion(thread_id.clone())
                .await?
        {
            return Ok(());
        }
        let lanes = self
            .lanes
            .acquire([thread_id.0.clone(), parent.0.clone()])
            .await;
        let mut child = projection_from_records(
            reader
                .query_records(ProjectionRecordQuery {
                    thread_id: thread_id.clone(),
                    fields: vec![
                        RecordKind::Runs,
                        RecordKind::Messages,
                        RecordKind::Subagents,
                        RecordKind::ProviderThreads,
                        RecordKind::ProviderTurns,
                        RecordKind::Attempts,
                    ],
                    filter: RecordFilter::default(),
                    messages: MessageFilter {
                        roles: Some(vec![ConversationMessageRole::User]),
                        ..Default::default()
                    },
                    run_statuses: None,
                    include_latest_run: false,
                })
                .await?,
        );
        let run_ids = child
            .runs
            .iter()
            .map(|run| {
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&run.id)
            })
            .collect::<Result<Vec<_>>>()?;
        let pending_ids = run_ids.clone();
        let pending = self
            .services
            .database
            .read(move |db| {
                let mut pending = Vec::new();
                for id in pending_ids {
                    if crate::stores::outbox::get(
                        db,
                        &format!("effect:restart-continuation:{}", id.0),
                    )?
                    .is_some_and(|effect| {
                        matches!(
                            effect.status,
                            crate::effects::EffectStatus::Pending
                                | crate::effects::EffectStatus::Running
                        )
                    }) {
                        pending.push(id);
                    }
                }
                Ok(pending)
            })
            .await?;
        let results = reader
            .query_records(ProjectionRecordQuery {
                thread_id: thread_id.clone(),
                fields: vec![RecordKind::Messages],
                filter: RecordFilter::default(),
                messages: MessageFilter {
                    roles: Some(vec![ConversationMessageRole::Assistant]),
                    run_ids: Some(run_ids.clone()),
                    ..Default::default()
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await?;
        for records in results.records {
            if let ProjectionRecordSet::Messages(messages) = records {
                child.messages.extend(messages);
            }
        }
        child.turn_items = reader.turn_start_history(thread_id, Some(run_ids)).await?;
        let prepared = self.prepare_delegated_recovery(parent.clone()).await?;
        let decision = crate::decision::engine::PureDecisionEngine.decide_delegated_recovery(
            &prepared.context,
            Some(&child),
            None,
            source.as_ref(),
            &pending,
        )?;
        self.commit_recovery(decision).await?;
        self.clean_finished_delegate(&child, source.as_ref(), &pending)
            .await?;
        drop(lanes);
        self.offer_deliveries(parent).await
    }

    async fn recover_completion_thread(&self, thread: ThreadId) -> Result<()> {
        let reader = self.services.inputs.reader().ok_or(Error::Unsupported(
            "delegated recovery requires the projection reader",
        ))?;
        let lanes = self.lanes.acquire([thread.0.clone()]).await;
        let projection = projection_from_records(
            reader
                .records(
                    thread.clone(),
                    vec![RecordKind::Runs],
                    RecordFilter::default(),
                )
                .await?,
        );
        let runs = projection
            .runs
            .iter()
            .map(crate::decision::engine::stored_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let messages: std::collections::HashSet<_> = runs
            .iter()
            .filter_map(|run| run["delegatedCompletion"]["delivery"]["messageId"].as_str())
            .collect();
        let terminal: Vec<RunId> = runs
            .iter()
            .filter(|run| {
                matches!(
                    run["status"].as_str(),
                    Some("completed" | "interrupted" | "failed" | "cancelled" | "rolled_back")
                ) && run["userMessageId"]
                    .as_str()
                    .is_some_and(|id| messages.contains(id))
            })
            .map(|run| serde_json::from_value(run["id"].clone()))
            .collect::<std::result::Result<_, _>>()?;
        for id in terminal {
            let prepared = self.prepare_delegated_recovery(thread.clone()).await?;
            self.commit_recovery(
                crate::decision::engine::PureDecisionEngine.decide_delegated_recovery(
                    &prepared.context,
                    None,
                    Some(&id),
                    None,
                    &[],
                )?,
            )
            .await?;
        }
        drop(lanes);
        self.offer_deliveries(thread).await
    }

    async fn clean_finished_delegate(
        &self,
        child: &OrchestrationV2ThreadProjection,
        settled_source: Option<&RunId>,
        pending: &[RunId],
    ) -> Result<()> {
        let progress = crate::ports::decisions::orchestrator::delegated_task_progress(child)?;
        if progress.state != "result_available" {
            return Ok(());
        }
        let Some(result) = progress.result_run else {
            return Ok(());
        };
        let result_id =
            crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&result.id)?;
        if child.runs.iter().any(|run| {
            let id =
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&run.id)
                    .ok();
            id.as_ref() != settled_source
                && id.is_some_and(|id| pending.contains(&id))
                && (run.id == result.id
                    || crate::ports::decisions::model_switch::run_ran_after(run, result))
        }) {
            return Ok(());
        }
        let thread = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &child.thread.id,
        )?;
        let reader = self
            .services
            .inputs
            .reader()
            .ok_or(Error::Unsupported("delegate cleanup requires reader"))?;
        if matches!(
            result.status,
            OrchestrationV2RunStatus::Interrupted | OrchestrationV2RunStatus::Cancelled
        ) {
            let controls = projection_from_records(
                reader
                    .records(
                        thread.clone(),
                        vec![RecordKind::RuntimeRequests, RecordKind::Nodes],
                        RecordFilter::default(),
                    )
                    .await?,
            );
            let mut chain = Vec::new();
            let mut previous = Some(result);
            while let Some(run) = previous {
                if chain.contains(&run.id) {
                    break;
                }
                chain.push(run.id.clone());
                let value = crate::decision::engine::stored_value(run)?;
                previous = child.runs.iter().find(|run| {
                    crate::decision::engine::stored_value(&run.id).ok().as_ref()
                        == value.get("restartContinuationOfRunId")
                });
            }
            for request in controls.runtime_requests {
                let value = crate::decision::engine::stored_value(&request)?;
                if value["status"] == "pending"
                    && value["kind"] == "user_input"
                    && value["responseCapability"]["type"] == "message"
                    && controls.nodes.iter().any(|node| {
                        node.id == request.node_id
                            && node.run_id.as_ref().is_some_and(|id| chain.contains(id))
                    })
                {
                    self.commit_locked_command(serde_json::from_value(serde_json::json!({
                        "type":"thread.user-input.dismiss","threadId":thread,
                        "commandId":format!("command:system:delegated-task-question:{}", value["id"].as_str().ok_or(Error::Stale)?),
                        "requestId":value["id"]
                    }))?).await?;
                }
            }
        }
        if child.provider_threads.iter().any(|provider| {
            provider
                .pending_background_tasks
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|tasks| !tasks.is_empty())
        }) {
            return Ok(());
        }
        let sessions = reader
            .records(
                thread.clone(),
                vec![RecordKind::ProviderSessions],
                RecordFilter::default(),
            )
            .await?;
        for records in sessions.records {
            let ProjectionRecordSet::ProviderSessions(sessions) = records else {
                continue;
            };
            for session in sessions {
                if matches!(
                    session.status,
                    ProviderSessionStatus::Stopped | ProviderSessionStatus::Error
                ) || session
                    .capabilities
                    .sessions
                    .supports_multiple_provider_threads_per_session
                {
                    continue;
                }
                let id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &session.id,
                )?;
                self.commit_locked_command(serde_json::from_value(serde_json::json!({
                    "type":"provider-session.detach","threadId":thread,
                    "commandId":format!("command:system:delegated-task-release:{}:{}",result_id.0,id.0),
                    "providerSessionId":id,"reason":"Delegated task finished."
                }))?).await?;
            }
        }
        Ok(())
    }

    // The caller owns every affected command lane; dispatch would recursively acquire it.
    async fn commit_locked_command(&self, command: OrchestrationV2ServerCommand) -> Result<()> {
        let (command_id, command_type, thread) = command_identity(&command)?;
        if self.receipt(&command_id).await?.is_some() {
            return Ok(());
        }
        let prepared = self.services.inputs.prepare(&command).await?;
        let decision = self
            .services
            .engine
            .decide_prepared(&prepared.context, &command)?;
        self.services
            .committer
            .commit(CommitRequest {
                receipt: Some(CommandReceipt {
                    command_id,
                    command_type,
                    aggregate: AggregateId::Thread(thread),
                    accepted_at: self.services.clock.now().0,
                    result_sequence: 0,
                    status: ReceiptStatus::Accepted,
                    error: None,
                }),
                decision,
                guard: prepared.guard,
            })
            .await?;
        Ok(())
    }

    async fn prepare_delegated_recovery(&self, thread: ThreadId) -> Result<PreparedCommand> {
        let command = serde_json::from_value(serde_json::json!({
            "type":"delegated_task.completion-delivery.dispose",
            "commandId":format!("command:system:delegated-recovery:{}", thread.0),
            "parentThreadId":thread, "taskId":"recovery-placeholder"
        }))?;
        self.services.inputs.prepare(&command).await
    }

    async fn commit_recovery(&self, decision: Decision) -> Result<()> {
        if !decision.events.is_empty() || !decision.effects.is_empty() {
            self.services
                .committer
                .commit(CommitRequest {
                    receipt: None,
                    decision,
                    guard: CommitGuard::None,
                })
                .await?;
        }
        Ok(())
    }
    async fn fail_queued_start(&self, thread: ThreadId, id: RunId, cause: Error) -> Result<()> {
        if matches!(cause, Error::Unsupported(_)) {
            return Err(cause);
        }
        let failure = match self.queue_failure_formatter.get() {
            Some(formatter) => formatter.failure(&cause)?,
            None => default_queue_start_failure()?,
        };
        let mut prepared = self.prepare_delegated_recovery(thread.clone()).await?;
        let reader = self
            .services
            .inputs
            .reader()
            .ok_or(Error::Unsupported("queued failure requires reader"))?;
        prepared.context.inputs.next_item_ordinals.push((
            thread,
            None,
            reader
                .next_turn_item_ordinal(
                    crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                        &prepared
                            .context
                            .context
                            .projection
                            .as_ref()
                            .ok_or(Error::Stale)?
                            .thread
                            .id,
                    )?,
                    None,
                )
                .await?,
        ));
        let decision = crate::decision::engine::PureDecisionEngine.decide_queued_failure(
            &prepared.context,
            &id,
            failure,
        )?;
        self.commit_recovery(decision).await
    }

    async fn queue_selection(&self, thread: ThreadId) -> Result<OrchestrationV2ThreadProjection> {
        let reader = self
            .services
            .inputs
            .reader()
            .ok_or(Error::Unsupported("queue selection requires reader"))?;
        Ok(projection_from_records(
            reader
                .query_records(ProjectionRecordQuery {
                    thread_id: thread,
                    fields: vec![RecordKind::Runs, RecordKind::Messages],
                    filter: RecordFilter::default(),
                    messages: MessageFilter {
                        roles: Some(vec![ConversationMessageRole::User]),
                        ..Default::default()
                    },
                    run_statuses: None,
                    include_latest_run: false,
                })
                .await?,
        ))
    }

    async fn cancel_stale_question_wakes(
        &self,
        projection: &OrchestrationV2ThreadProjection,
    ) -> Result<bool> {
        let reader = self
            .services
            .inputs
            .reader()
            .ok_or(Error::Unsupported("question wake cleanup requires reader"))?;
        let thread = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &projection.thread.id,
        )?;
        let mut candidates = Vec::new();
        let mut task_ids = Vec::new();
        for run in &projection.runs {
            if run.status != OrchestrationV2RunStatus::Queued {
                continue;
            }
            if let Some(message) = projection
                .messages
                .iter()
                .find(|message| message.id == run.user_message_id)
            {
                let value = crate::decision::engine::stored_value(message)?;
                if value["notification"]["source"]["kind"] == "delegated_task"
                    && value["notification"]["outcome"] == "updated"
                {
                    for id in value["notification"]["source"]["taskIds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        task_ids.push(serde_json::from_value(id.clone())?);
                    }
                    candidates.push((run, message));
                }
            }
        }
        if candidates.is_empty() {
            return Ok(false);
        }
        let parent = projection_from_records(
            reader
                .records(
                    thread.clone(),
                    vec![RecordKind::Subagents],
                    RecordFilter {
                        subagent_ids: Some(task_ids),
                        ..Default::default()
                    },
                )
                .await?,
        );
        let mut cancelled = false;
        for (run, message) in candidates {
            let value = crate::decision::engine::stored_value(message)?;
            let ids = value["notification"]["source"]["taskIds"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let task = parent.subagents.iter().find(|task| {
                crate::decision::engine::stored_value(&task.id)
                    .ok()
                    .is_some_and(|id| ids.contains(&id))
            });
            let child = match task.and_then(|task| task.child_thread_id.as_ref()) {
                Some(child) => match reader
                    .records(
                        crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                            child,
                        )?,
                        vec![RecordKind::RuntimeRequests],
                        RecordFilter {
                            runtime_request_statuses: Some(vec![RuntimeRequestStatus::Pending]),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    Ok(records) => Some(projection_from_records(records)),
                    Err(_) => None,
                },
                None => None,
            };
            if crate::ports::decisions::orchestrator::stale_question_wake(
                &parent,
                message,
                child.as_ref(),
            )? {
                let id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &run.id,
                )?;
                self.commit_locked_command(serde_json::from_value(serde_json::json!({"type":"queued-run.cancel","commandId":format!("command:system:stale-question:{}",id.0),"threadId":thread,"runId":id}))?).await?;
                cancelled = true;
            }
        }
        Ok(cancelled)
    }

    async fn resume_queue(
        &self,
        thread_id: ThreadId,
        selected: Option<RunId>,
        ended: Option<RunId>,
    ) -> Result<()> {
        let _lane = self.lanes.acquire([thread_id.0.clone()]).await;
        loop {
            let mut projection = self.queue_selection(thread_id.clone()).await?;
            if projection.runs.iter().any(|run| {
                matches!(
                    run.status,
                    OrchestrationV2RunStatus::Preparing
                        | OrchestrationV2RunStatus::Starting
                        | OrchestrationV2RunStatus::Running
                        | OrchestrationV2RunStatus::Waiting
                )
            }) {
                return Ok(());
            }
            if self.cancel_stale_question_wakes(&projection).await? {
                projection = self.queue_selection(thread_id.clone()).await?;
            }
            let run = if let Some(selected) = &selected {
                projection.runs.iter().find(|run| {
                    crate::decision::engine::stored_value(&run.id).ok()
                        == Some(serde_json::json!(selected))
                        && run.status == OrchestrationV2RunStatus::Queued
                })
            } else {
                crate::ports::decisions::queued_run_order::queued_runs_in_delivery_order(
                    &projection,
                )
                .into_iter()
                .next()
            };
            let Some(run) = run else {
                return Ok(());
            };
            if run.queue_held == Some(Some(true)) {
                return Ok(());
            }
            if self.ceiling.as_ref().is_some_and(|ceiling| {
                is_runtime_mode_above(projection.thread.runtime_mode, ceiling.max_runtime_mode)
            }) {
                return Ok(());
            }
            let run_id =
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&run.id)?;
            let selection = crate::ports::decisions::orchestrator::queued_run_model_selection(
                &projection,
                run,
            )?;
            let command = serde_json::from_value(
                serde_json::json!({"type":"message.dispatch","commandId":format!("command:system:start-queued:{}",run_id.0),"threadId":thread_id,"messageId":run.user_message_id,"text":"queued start","attachments":[],"modelSelection":selection,"dispatchMode":{"type":"start_immediately"},"createdBy":"agent","creationSource":"server"}),
            )?;
            let mut prepared = match self.services.inputs.prepare(&command).await {
                Ok(prepared) => prepared,
                Err(cause) => {
                    return self
                        .fail_queued_start(thread_id.clone(), run_id, cause)
                        .await;
                }
            };
            let project_id =
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &projection.thread.project_id,
                )?;
            prepared.context.inputs.projects = self
                .services
                .database
                .read(move |db| crate::stores::projects::list(db, Some(&[project_id]), true))
                .await?;
            let decision = match crate::decision::engine::PureDecisionEngine
                .decide_queued_start_after(&prepared.context, &run_id, ended.as_ref())
            {
                Ok(decision) => decision,
                Err(cause) => {
                    return self
                        .fail_queued_start(thread_id.clone(), run_id, cause)
                        .await;
                }
            };
            if decision.events.is_empty() && decision.effects.is_empty() {
                return Ok(());
            }
            let cancelled = decision
                .events
                .iter()
                .filter_map(|event| crate::decision::engine::stored_value(event).ok())
                .any(|event| {
                    event["type"] == "run.updated"
                        && event["payload"]["id"] == serde_json::json!(run_id)
                        && event["payload"]["status"] == "cancelled"
                });
            let result = self
                .services
                .committer
                .commit(CommitRequest {
                    receipt: None,
                    decision,
                    guard: CommitGuard::RunCurrent {
                        thread_id: thread_id.clone(),
                        run_id,
                        attempt_id:
                            crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                                run.active_attempt_id.as_ref().ok_or(Error::Stale)?,
                            )?,
                        status: OrchestrationV2RunStatus::Queued,
                    },
                })
                .await?;
            if result.committed && cancelled && selected.is_none() {
                continue;
            }
            return Ok(());
        }
    }

    async fn offer_deliveries(&self, thread_id: ThreadId) -> Result<()> {
        use crate::ports::sessions_recovery::provider_continuation_service::{
            ContinuationDelivery, ContinuationRequest, DelegatedContinuation, current_delivery,
        };
        let reader = self.services.inputs.reader().ok_or(Error::Unsupported(
            "completion offers require the projection reader",
        ))?;
        let projection = projection_from_records(
            reader
                .records(
                    thread_id.clone(),
                    vec![
                        RecordKind::Runs,
                        RecordKind::Messages,
                        RecordKind::ProviderTurns,
                        RecordKind::ProviderThreads,
                    ],
                    RecordFilter::default(),
                )
                .await?,
        );
        if projection.thread.archived_at.is_some() || projection.thread.deleted_at.is_some() {
            return Ok(());
        }
        for run in &projection.runs {
            let value = crate::decision::engine::stored_value(run)?;
            let delivery = &value["delegatedCompletion"]["delivery"];
            if value["delegatedCompletion"]["disposition"] != "open"
                || delivery["taskIds"].as_array().is_none_or(Vec::is_empty)
            {
                continue;
            }
            let completion = DelegatedContinuation {
                parent_run_id: serde_json::from_value(value["id"].clone())?,
                generation: delivery["generation"].as_u64().ok_or(Error::Stale)?,
                message_id: serde_json::from_value(delivery["messageId"].clone())?,
            };
            if current_delivery(
                &projection.messages,
                &projection.runs,
                &projection.provider_turns,
                &completion,
            )?
            .is_none()
            {
                continue;
            }
            let Some(owner) = projection
                .provider_threads
                .iter()
                .find(|owner| Some(&owner.id) == run.provider_thread_id.as_ref())
            else {
                continue;
            };
            let request = ContinuationRequest {
                thread_id: thread_id.clone(),
                provider_thread_id:
                    crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                        &owner.id,
                    )?,
                driver: owner.driver.clone(),
                detail: None,
                notification: None,
                delegated_completion: Some(completion),
                delivery: ContinuationDelivery::MessageText,
                message_id: None,
                callbacks: None,
            };
            self.continuations
                .get()
                .ok_or(Error::Unsupported(
                    "completion delivery requires the provider continuation queue",
                ))?
                .send(request)
                .await
                .map_err(|_| Error::Cancelled)?;
        }
        Ok(())
    }

    async fn after_commit(
        &self,
        command: &OrchestrationV2ServerCommand,
        replay: bool,
        claimed: bool,
    ) -> Result<()> {
        let continuation =
            crate::ports::decisions::orchestrator::metadata_update_continuation(command)?;
        let command = continuation.as_ref().unwrap_or(command);
        let (_, kind, thread) = command_identity(command)?;
        let value = crate::decision::engine::stored_value(command)?;
        if matches!(kind.as_str(), "queue.resume" | "queued-run.start")
            || !replay && kind == "thread.runtime-mode.set" && self.ceiling.is_some()
        {
            self.resume_queue(
                thread.clone(),
                if kind == "queued-run.start" {
                    Some(serde_json::from_value(value["runId"].clone())?)
                } else {
                    None
                },
                None,
            )
            .await?;
        }
        if !replay
            && (kind == "notification.delivery.accept"
                || matches!(
                    kind.as_str(),
                    "message.dispatch" | "queued-message.promote-to-steer"
                ) && claimed
                || matches!(
                    kind.as_str(),
                    "delegated_task.wake-policy" | "delegated_task.remote.complete"
                )
                || kind == "delegated_task.completion-delivery.acknowledge"
                    && value["observedByRunId"].is_null())
        {
            self.offer_deliveries(thread).await?;
        }
        Ok(())
    }

    async fn admit(&self) -> Result<()> {
        self.ready
            .get_or_try_init(|| async { self.services.recovery.recover().await.map(|_| ()) })
            .await?;
        Ok(())
    }

    async fn receipt(&self, command_id: &CommandId) -> Result<Option<CommandReceipt>> {
        let id = command_id.clone();
        self.services
            .database
            .read(move |db| receipts::get(db, &id))
            .await
    }

    async fn reject(
        &self,
        command_id: CommandId,
        command_type: String,
        aggregate: AggregateId,
        error: Error,
    ) -> Result<CommandReceipt> {
        let reason = match &error {
            Error::Rejected { reason, .. } => reason.clone(),
            _ => error.to_string(),
        };
        let receipt = CommandReceipt {
            command_id,
            aggregate: aggregate.clone(),
            command_type,
            accepted_at: self.services.clock.now().0,
            result_sequence: 0,
            status: ReceiptStatus::Rejected,
            error: Some(reason),
        };
        let committed = self
            .services
            .committer
            .commit(CommitRequest {
                receipt: Some(receipt),
                decision: Decision::default(),
                guard: CommitGuard::None,
            })
            .await?;
        if let Some(receipt) = committed.receipt {
            validate_receipt(receipt, &aggregate)?;
        }
        Err(error)
    }

    async fn dispatch(
        &self,
        command: OrchestrationV2ServerCommand,
        limit: Option<DispatchModes>,
    ) -> Result<CommandReceipt> {
        self.admit().await?;
        let (command_id, command_type, thread_id) = command_identity(&command)?;
        let aggregate = AggregateId::Thread(thread_id.clone());
        let mut keys = self.services.inputs.lock_keys(&command).await?;
        keys.push(thread_id.clone());
        // These guards cover input reads, the pure decision, receipt reservation, and commit.
        let _lanes = loop {
            keys.sort_by(|a, b| a.0.cmp(&b.0));
            keys.dedup();
            let lanes = self.lanes.acquire(keys.iter().map(|id| id.0.clone())).await;
            if !matches!(
                command_type.as_str(),
                "thread.delete" | "run.interrupt" | "thread.stop" | "thread.background-work.settle"
            ) {
                break lanes;
            }
            let discovered = self.services.inputs.lock_keys(&command).await?;
            if discovered.iter().all(|id| keys.contains(id)) {
                break lanes;
            }
            keys.extend(discovered);
            drop(lanes);
        };
        let mut measurement = self
            .dispatch_observer
            .get()
            .map(|observer| DispatchMeasurement {
                observer: observer.clone(),
                command_type: command_type.clone(),
                started: std::time::Instant::now(),
                outcome: DispatchOutcome::Interrupt,
            });
        let result = async {
        if let Some(receipt) = self.receipt(&command_id).await? {
            let receipt = validate_receipt(receipt, &aggregate)?;
            drop(_lanes);
            self.after_commit(&command, true, false).await?;
            return Ok(receipt);
        }
        let mut prepared = self.services.inputs.prepare(&command).await?;
        let needs_project_context = matches!(
            command_type.as_str(),
            "prepared-run.release"
                | "thread.project.adopt-folder"
                | "message.dispatch"
                | "delegated_task.request"
                | "queued-message.promote-to-steer"
                | "link_request.record"
                | "thread.pull-request-watch.sync"
                | "runtime-request.respond"
        ) || crate::ports::decisions::orchestrator::metadata_update_continuation(&command)?.is_some();
        if needs_project_context && let Some(projection) = &prepared.context.context.projection {
            let project_id =
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &projection.thread.project_id,
                )?;
            let mut project_ids = vec![project_id];
            if command_type == "thread.project.adopt-folder" {
                project_ids.push(serde_json::from_value(
                    crate::decision::engine::stored_value(&command)?["projectId"].clone(),
                )?);
            }
            let projects = self
                .services
                .database
                .read(move |db| crate::stores::projects::list(db, Some(&project_ids), true))
                .await?;
            prepared.context.inputs.projects.extend(projects);
        }
        if command_type == "checkpoint.rollback"
            && crate::decision::engine::stored_value(&command)?["restoreFiles"] != false
            && let Some(projection) = &prepared.context.context.projection
        {
            let value = crate::decision::engine::stored_value(&command)?;
            if let Some(scope) = projection.checkpoint_scopes.iter().find(|scope| {
                crate::decision::engine::stored_value(&scope.id)
                    .ok()
                    .as_ref()
                    == Some(&value["scopeId"])
            }) {
                let reader = self.services.inputs.reader().ok_or(Error::Unsupported(
                    "checkpoint restore isolation requires the projection reader",
                ))?;
                prepared.context.inputs.restore_isolated=Some(crate::ports::checkpoints_forks::checkpoint_restore_safety::is_checkpoint_restore_isolated(&projection.thread,scope,reader,&self.services.database).await?);
            }
        }
        let copied_sources = prepared
            .context
            .inputs
            .related_projections
            .iter()
            .filter(|_| matches!(command_type.as_str(), "thread.fork" | "thread.merge_back"));
        for projection in prepared
            .context
            .context
            .projection
            .iter()
            .chain(copied_sources)
        {
            let Some(limit) = narrow_to_ceiling(limit, self.ceiling.as_ref()) else {
                continue;
            };
            if exceeded_dispatch_mode_limit(
                limit,
                DispatchModes {
                    runtime_mode: projection.thread.runtime_mode,
                    interaction_mode: projection.thread.interaction_mode,
                },
            )
            .is_none()
            {
                continue;
            }
            let runtime_mode =
                crate::decision::engine::stored_value(projection.thread.runtime_mode)?;
            let interaction_mode =
                crate::decision::engine::stored_value(projection.thread.interaction_mode)?;
            return Err(Error::Rejected {
                command_id,
                reason: format!(
                    "Thread {} now runs in {}/{} mode, above what this caller may change.",
                    crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                        &projection.thread.id
                    )?
                    .0,
                    runtime_mode.as_str().ok_or(Error::Stale)?,
                    interaction_mode.as_str().ok_or(Error::Stale)?,
                ),
            });
        }
        let planned = self
            .services
            .engine
            .decide_prepared(&prepared.context, &command);
        let decision = match planned {
            Ok(decision) => decision,
            Err(error @ Error::Rejected { .. }) => {
                return self
                    .reject(command_id, command_type, aggregate, error)
                    .await;
            }
            Err(error) => return Err(error),
        };
        if let Some(ceiling) = &self.ceiling {
            let mut modes = std::collections::HashMap::new();
            if let Some(projection) = &prepared.context.context.projection {
                modes.insert(thread_id.clone(), projection.thread.runtime_mode);
            }
            if let Some(refusal) =
                runtime_mode_ceiling_refusal(ceiling.max_runtime_mode, &decision.events, &modes)
            {
                return Err(Error::Rejected {
                    command_id,
                    reason: ceiling.message(refusal.refused),
                });
            }
        }
        if decision.events.is_empty() && !allows_empty_decision(&command_type, &command)? {
            let error = Error::Rejected {
                command_id: command_id.clone(),
                reason: "Command produced no domain events.".into(),
            };
            return self
                .reject(command_id, command_type, aggregate, error)
                .await;
        }
        let receipt = CommandReceipt {
            command_id: command_id.clone(),
            aggregate: aggregate.clone(),
            command_type,
            accepted_at: decision
                .events
                .last()
                .map(crate::decision::engine::stored_value)
                .transpose()?
                .and_then(|event| event["occurredAt"].as_str().map(str::to_owned))
                .unwrap_or_else(|| self.services.clock.now().0),
            result_sequence: 0,
            status: ReceiptStatus::Accepted,
            error: None,
        };
        if decision.events.is_empty()
            && decision.effects.is_empty()
            && decision.cancellations.is_empty()
            && matches!(&prepared.guard, CommitGuard::None)
        {
            let thread_id = thread_id.clone();
            let receipt = self
                .services
                .database
                .transaction(move |tx| {
                    if let Some(existing) = receipts::get(tx, &receipt.command_id)? {
                        return Ok(existing);
                    }
                    let mut receipt = receipt;
                    receipt.result_sequence = crate::stores::event_store::EventStore::new()
                        .latest_agent_sequence(tx, Some(&thread_id))?;
                    if receipts::insert_if_absent(tx, &receipt)? {
                        return Ok(receipt);
                    }
                    receipts::get(tx, &receipt.command_id)?.ok_or(Error::Stale)
                })
                .await?;
            return validate_receipt(receipt, &aggregate);
        }
        let claimed = decision
            .events
            .iter()
            .map(crate::decision::engine::stored_value)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|event| {
                event["type"] == "subagent.updated"
                    && event["payload"]["completionDelivery"]["state"] == "claimed"
            });
        let result = self
            .services
            .committer
            .commit(CommitRequest {
                receipt: Some(receipt),
                decision,
                guard: prepared.guard,
            })
            .await?;
        let receipt = result.receipt.ok_or(Error::Service {
            operation: "CommandDispatcher.dispatch",
            detail: "Commit returned no command receipt.".into(),
        })?;
        let receipt = validate_receipt(receipt, &aggregate)?;
        drop(_lanes);
        self.after_commit(&command, false, claimed).await?;
        Ok(receipt)
        }.await;
        if let Some(measurement) = &mut measurement {
            measurement.outcome = match &result {
                Ok(_) => DispatchOutcome::Success,
                Err(
                    Error::Cancelled | Error::Provider(crate::provider::ProviderError::Cancelled),
                ) => DispatchOutcome::Interrupt,
                Err(_) => DispatchOutcome::Failure,
            };
        }
        result
    }
}

pub fn validate_receipt(
    receipt: CommandReceipt,
    aggregate: &AggregateId,
) -> Result<CommandReceipt> {
    if receipt.status == ReceiptStatus::Rejected {
        return Err(Error::Rejected {
            command_id: receipt.command_id.clone(),
            reason: receipt
                .error
                .clone()
                .unwrap_or_else(|| "Previously rejected.".into()),
        });
    }
    if &receipt.aggregate != aggregate {
        let reason =
            if let (AggregateId::Thread(receipt_thread), AggregateId::Thread(command_thread)) =
                (&receipt.aggregate, aggregate)
            {
                format!(
                    "Command {} was already handled for thread {} and cannot be replayed for {}.",
                    receipt.command_id.0, receipt_thread.0, command_thread.0
                )
            } else {
                "Conflicting receipt is not a thread command receipt.".into()
            };
        return Err(Error::Rejected {
            command_id: receipt.command_id.clone(),
            reason,
        });
    }
    Ok(receipt)
}

fn allows_empty_decision(
    command_type: &str,
    command: &OrchestrationV2ServerCommand,
) -> Result<bool> {
    let value = crate::decision::engine::stored_value(command)?;
    Ok(matches!(
        command_type,
        "thread.background-work.settle"
            | "thread.visit"
            | "thread.pull-request-watch.checks-reopen"
            | "thread.interrupt.unresponsive"
            | "thread.stop"
            | "subagent.stop"
            | "thread.guest-share.close"
    ) || (command_type == "thread.delete" && value["onlyIfUnused"] == true)
        || (command_type == "delegated_task.completion-delivery.acknowledge"
            && value["observedByRunId"].is_null()))
}

// Local shim until ProjectionReader exposes 02's lightweight active-thread metadata method.
async fn read_active_thread_metadata(
    _reader: &dyn ProjectionReader,
    _project: Option<ProjectId>,
) -> Result<Vec<crate::ports::decisions::orchestrator::ActiveThreadListEntry>> {
    Err(Error::Unsupported("ProjectionReader.list_active_threads"))
}
