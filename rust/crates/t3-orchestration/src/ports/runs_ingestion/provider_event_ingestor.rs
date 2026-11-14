use crate::ports::decisions::model_switch::model_selections_equal;
use crate::ports::decisions::thread_command_executor::KeyedLanes;
use crate::{
    api::*,
    error::{Error, Result},
    provider::{ProviderEvent, ProviderEventKind, SteerStatus, TerminalOutcome},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex},
};
use t3_contracts::{orchestration_v2_id_table::IdOrIndex, provider_instance::ProviderInstanceId};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, thiserror::Error)]
#[error("provider event normalization for {provider_session_id:?}/{thread_id:?}: {cause}")]
pub struct ProviderEventNormalizeError {
    pub provider_session_id: ProviderSessionId,
    pub thread_id: ThreadId,
    pub provider_event: ProviderEvent,
    #[source]
    pub cause: Box<Error>,
}

#[derive(Debug, thiserror::Error)]
#[error("publishing {event_count} provider events for {provider_session_id:?}: {cause}")]
pub struct ProviderEventPublishError {
    pub provider_session_id: ProviderSessionId,
    pub event_count: usize,
    #[source]
    pub cause: Box<Error>,
}

#[derive(Debug, Clone)]
pub struct IngestInput {
    pub session_id: ProviderSessionId,
    pub instance_id: ProviderInstanceId,
    pub thread_id: ThreadId,
    pub run_id: Option<RunId>,
    pub node_id: Option<NodeId>,
    pub raw_event_id: Option<RawEventId>,
    pub event: ProviderEvent,
}

impl IngestInput {
    pub fn domain_event(
        &self,
        ids: &dyn IdGenerator,
        now: &IsoDateTime,
        event_type: &str,
        payload: Value,
        identity: Value,
    ) -> Result<OrchestrationV2DomainEvent> {
        let mut event = json!({
            "id": ids.next(IdKind::Event), "type": event_type,
            "threadId": self.thread_id, "driver": self.event.driver,
            "providerInstanceId": self.instance_id, "occurredAt": now, "payload": payload,
        });
        for (key, value) in [
            (
                "runId",
                self.run_id.as_ref().map(t3_contracts::stored::to_value),
            ),
            (
                "nodeId",
                self.node_id.as_ref().map(t3_contracts::stored::to_value),
            ),
            (
                "rawEventId",
                self.raw_event_id
                    .as_ref()
                    .map(t3_contracts::stored::to_value),
            ),
        ] {
            if let Some(value) = value {
                event[key] = value?;
            }
        }
        if let Some(identity) = identity.as_object() {
            for (key, value) in identity {
                // TypeScript uses ?? for payload identities; null falls back to the run input.
                if !value.is_null() {
                    event[key] = value.clone();
                }
            }
        }
        Ok(serde_json::from_value(event)?)
    }
}

pub fn expanded_id<T: Clone>(id: &IdOrIndex<T>) -> Result<T> {
    match id {
        IdOrIndex::Id(id) => Ok(id.clone()),
        IdOrIndex::Index(_) => Err(Error::Decode {
            entity: "provider event identity",
            detail: "compact IDs must be expanded before ingestion".into(),
        }),
    }
}

// seam-shim: server binds instance snapshot endpoint and the shared proxy-hub reset resolver.
#[async_trait]
pub trait UsageLimitResets: Send + Sync {
    async fn endpoint(&self, instance: ProviderInstanceId) -> Result<Option<String>>;
    async fn reset_at(
        &self,
        driver: t3_contracts::provider_instance::ProviderDriverKind,
        endpoint: String,
        model: Option<String>,
    ) -> Result<Option<IsoDateTime>>;
}

pub struct ProviderEventIngestor {
    pub projections: Arc<dyn ProjectionReader>,
    pub committer: Arc<dyn EventCommitter>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub lanes: Arc<KeyedLanes>,
    pub workspace: Option<Arc<dyn WorkspaceBackend>>,
    pub agent_scope: Option<Arc<dyn AgentScope>>,
    pub telemetry: Option<Arc<dyn ProviderTelemetry>>,
    pub usage_limit_resets: Option<Arc<dyn UsageLimitResets>>,
    completed_turns: Mutex<CompletedTurns>,
    telemetry_tasks: TaskTracker,
    telemetry_cancellation: CancellationToken,
    telemetry_closed: Mutex<bool>,
}

#[derive(Default)]
struct CompletedTurns {
    keys: HashSet<(ProviderInstanceId, ProviderTurnId)>,
    order: VecDeque<(ProviderInstanceId, ProviderTurnId)>,
}

impl CompletedTurns {
    fn insert(&mut self, key: (ProviderInstanceId, ProviderTurnId)) -> bool {
        if !self.keys.insert(key.clone()) {
            return false;
        }
        self.order.push_back(key);
        if self.order.len() > 4096
            && let Some(oldest) = self.order.pop_front()
        {
            self.keys.remove(&oldest);
        }
        true
    }
}

impl ProviderEventIngestor {
    pub fn prepare_snapshot(
        &self,
        input: IngestInput,
        guard: CommitGuard,
        command_id: Option<CommandId>,
    ) -> Result<crate::ports::commit_worker::provider_commit_batch::PreparedCommit> {
        use crate::ports::commit_worker::provider_commit_batch::{PreparedCommit, is_snapshot};
        if !is_snapshot(&input.event) {
            return Err(Error::Unsupported(
                "prepare_snapshot requires a provider snapshot",
            ));
        }
        let ids = self.ids.clone();
        let clock = self.clock.clone();
        Ok(PreparedCommit {
            request: CommitRequest {
                receipt: None,
                decision: Decision::default(),
                guard,
            },
            options: CommitOptions {
                command_id,
                guard_pending_user_input_cancellations: true,
                ..Default::default()
            },
            prepare: Some(Box::new(move |tx, mut request| {
                let now = clock.now();
                let (kind, payload, identity) = match &input.event.kind {
                    ProviderEventKind::Node(node) => {
                        let prior = crate::projections::read::records(
                            tx,
                            &expanded_id(&node.thread_id)?,
                            &[RecordKind::Nodes],
                            &RecordFilter {
                                node_ids: Some(vec![expanded_id(&node.id)?]),
                                ..Default::default()
                            },
                        )?;
                        if prior.records.iter().any(|records|matches!(records,ProjectionRecordSet::Nodes(nodes) if nodes.contains(node))) {return Ok(request);}
                        (
                            "node.updated",
                            t3_contracts::stored::to_value(node)?,
                            json!({"threadId":node.thread_id,"runId":node.run_id,"nodeId":node.id}),
                        )
                    }
                    ProviderEventKind::Message(message) => (
                        "message.updated",
                        t3_contracts::stored::to_value(message)?,
                        json!({"threadId":message.thread_id,"runId":message.run_id,"nodeId":message.node_id}),
                    ),
                    ProviderEventKind::TurnItem { item, .. } => {
                        let mut payload = t3_contracts::stored::to_value(item)?;
                        bound_item(&mut payload);
                        let identity = json!({"threadId":payload["threadId"],"runId":payload["runId"],"nodeId":payload["nodeId"]});
                        ("turn-item.updated", payload, identity)
                    }
                    ProviderEventKind::Plan(plan) => {
                        let mut plan = *plan.clone();
                        if let OrchestrationV2PlanArtifact::TodoList(list) = &mut plan {
                            let previous: Option<OrchestrationV2PlanArtifact> =
                                crate::projections::read::one(
                                    tx,
                                    "SELECT payload_json FROM orchestration_v2_projection_plans WHERE thread_id=?1 AND plan_id=?2",
                                    t3_persistence::rusqlite::params![
                                        expanded_id(&list.thread_id)?.0,
                                        expanded_id(&list.id)?.0
                                    ],
                                )?;
                            with_plan_step_durations(
                                list,
                                match &previous {
                                    Some(OrchestrationV2PlanArtifact::TodoList(list)) => Some(list),
                                    _ => None,
                                },
                                &now,
                            );
                        }
                        let payload = t3_contracts::stored::to_value(&plan)?;
                        let identity = json!({"threadId":payload["threadId"],"runId":payload["runId"],"nodeId":payload["nodeId"]});
                        ("plan.updated", payload, identity)
                    }
                    _ => unreachable!("snapshot kind checked before admission"),
                };
                request.decision.events.push(input.domain_event(
                    ids.as_ref(),
                    &now,
                    kind,
                    payload,
                    identity,
                )?);
                Ok(request)
            })),
        })
    }

    pub fn new(
        projections: Arc<dyn ProjectionReader>,
        committer: Arc<dyn EventCommitter>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        lanes: Arc<KeyedLanes>,
    ) -> Self {
        Self {
            projections,
            committer,
            clock,
            ids,
            lanes,
            workspace: None,
            agent_scope: None,
            telemetry: None,
            usage_limit_resets: None,
            completed_turns: Mutex::new(CompletedTurns::default()),
            telemetry_tasks: TaskTracker::new(),
            telemetry_cancellation: CancellationToken::new(),
            telemetry_closed: Mutex::new(false),
        }
    }

    pub async fn shutdown(&self) {
        {
            let mut closed = self
                .telemetry_closed
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *closed = true;
            self.telemetry_tasks.close();
            self.telemetry_cancellation.cancel();
        }
        self.telemetry_tasks.wait().await;
    }

    pub async fn normalize(&self, input: &IngestInput) -> Result<Vec<OrchestrationV2DomainEvent>> {
        let now = self.clock.now();
        let make = |kind, payload, identity| {
            input.domain_event(self.ids.as_ref(), &now, kind, payload, identity)
        };
        let mut events = Vec::new();
        let (kind, payload, identity) = match &input.event.kind {
            ProviderEventKind::AppThread {
                app_thread,
                create_only,
            } => {
                let thread_id = expanded_id(&app_thread.id)?;
                let existing = self.projections.thread(thread_id.clone()).await?;
                let (kind, payload) = match existing {
                    Some(mut saved) => {
                        if *create_only == Some(true)
                            || (saved.title == app_thread.title
                                && model_selections_equal(
                                    &saved.model_selection,
                                    &app_thread.model_selection,
                                ))
                        {
                            return Ok(events);
                        }
                        saved.title = app_thread.title.clone();
                        saved.model_selection = app_thread.model_selection.clone();
                        saved.updated_at = now.clone();
                        (
                            "thread.metadata-updated",
                            t3_contracts::stored::to_value(&saved)?,
                        )
                    }
                    None => (
                        "thread.created",
                        t3_contracts::stored::to_value(&app_thread)?,
                    ),
                };
                (kind, payload, json!({"threadId":thread_id}))
            }
            ProviderEventKind::Session(session) => (
                "provider-session.updated",
                t3_contracts::stored::to_value(&session)?,
                json!({}),
            ),
            ProviderEventKind::Thread {
                provider_thread,
                retained_native_turn_ids,
                ..
            } => {
                let thread_id = provider_thread
                    .app_thread_id
                    .as_ref()
                    .map(expanded_id)
                    .transpose()?
                    .unwrap_or_else(|| input.thread_id.clone());
                events.push(make(
                    "provider-thread.updated",
                    t3_contracts::stored::to_value(&provider_thread)?,
                    json!({"threadId":thread_id}),
                )?);
                if let Some(retained) = retained_native_turn_ids {
                    events.extend(
                        self.rewind_events(
                            input,
                            thread_id,
                            expanded_id(&provider_thread.id)?,
                            retained,
                            &now,
                        )
                        .await?,
                    );
                }
                return Ok(events);
            }
            ProviderEventKind::Turn {
                provider_turn,
                thread_id,
            } => {
                if matches!(
                    provider_turn.status,
                    ProviderTurnStatus::Completed
                        | ProviderTurnStatus::Failed
                        | ProviderTurnStatus::Cancelled
                        | ProviderTurnStatus::Interrupted
                ) {
                    events.extend(
                        self.dismiss_native_inputs(
                            input,
                            expanded_id(&provider_turn.id)?,
                            thread_id.clone().unwrap_or_else(|| input.thread_id.clone()),
                            &now,
                        )
                        .await?,
                    );
                }
                (
                    "provider-turn.updated",
                    t3_contracts::stored::to_value(&provider_turn)?,
                    json!({"threadId":thread_id, "nodeId":provider_turn.node_id}),
                )
            }
            ProviderEventKind::Node(node) => {
                let records = self
                    .projections
                    .records(
                        expanded_id(&node.thread_id)?,
                        vec![RecordKind::Nodes],
                        RecordFilter {
                            node_ids: Some(vec![expanded_id(&node.id)?]),
                            ..Default::default()
                        },
                    )
                    .await?;
                if records.records.iter().any(
                    |set| matches!(set, ProjectionRecordSet::Nodes(nodes) if nodes.contains(node)),
                ) {
                    return Ok(events);
                }
                (
                    "node.updated",
                    t3_contracts::stored::to_value(&node)?,
                    json!({"threadId":node.thread_id,"runId":node.run_id,"nodeId":node.id}),
                )
            }
            ProviderEventKind::Subagent(agent) => (
                "subagent.updated",
                t3_contracts::stored::to_value(&agent)?,
                json!({"threadId":agent.thread_id,"runId":agent.run_id,"nodeId":agent.id}),
            ),
            ProviderEventKind::Message(message) => (
                "message.updated",
                t3_contracts::stored::to_value(&message)?,
                json!({"threadId":message.thread_id,"runId":message.run_id,"nodeId":message.node_id}),
            ),
            ProviderEventKind::TurnItem { item, .. } => {
                let mut payload = t3_contracts::stored::to_value(&item)?;
                bound_item(&mut payload);
                let identity = json!({"threadId":payload["threadId"],"runId":payload["runId"],"nodeId":payload["nodeId"]});
                ("turn-item.updated", payload, identity)
            }
            ProviderEventKind::RuntimeRequest { request, thread_id } => (
                "runtime-request.updated",
                t3_contracts::stored::to_value(&request)?,
                json!({"threadId":thread_id,"nodeId":request.node_id}),
            ),
            ProviderEventKind::Plan(plan) => {
                let mut plan = *plan.clone();
                if let OrchestrationV2PlanArtifact::TodoList(list) = &mut plan {
                    let previous = self
                        .projections
                        .plan(expanded_id(&list.thread_id)?, expanded_id(&list.id)?)
                        .await?;
                    with_plan_step_durations(
                        list,
                        match &previous {
                            Some(OrchestrationV2PlanArtifact::TodoList(list)) => Some(list),
                            _ => None,
                        },
                        &now,
                    );
                }
                let payload = t3_contracts::stored::to_value(&plan)?;
                let identity = json!({"threadId":payload["threadId"],"runId":payload["runId"],"nodeId":payload["nodeId"]});
                ("plan.updated", payload, identity)
            }
            ProviderEventKind::Steer {
                thread_id,
                message_id,
                status,
                ..
            } => {
                let item_id = TurnItemId(derive_id("turn-item", &["message", &message_id.0]));
                let Some(OrchestrationV2TurnItem::UserMessage(mut item)) = self
                    .projections
                    .turn_item(thread_id.clone(), item_id)
                    .await?
                else {
                    return Ok(events);
                };
                let status = match status {
                    SteerStatus::Pending => OrchestrationV2TurnItemStatus::Pending,
                    SteerStatus::Completed => OrchestrationV2TurnItemStatus::Completed,
                    SteerStatus::Cancelled => OrchestrationV2TurnItemStatus::Cancelled,
                };
                if item.status == status
                    || !matches!(
                        item.status,
                        OrchestrationV2TurnItemStatus::Pending
                            | OrchestrationV2TurnItemStatus::Completed
                    )
                {
                    return Ok(events);
                }
                item.completed_at =
                    (status != OrchestrationV2TurnItemStatus::Pending).then(|| now.clone());
                item.status = status;
                item.updated_at = now.clone();
                let identity =
                    json!({"threadId":item.thread_id,"runId":item.run_id,"nodeId":item.node_id});
                (
                    "turn-item.updated",
                    t3_contracts::stored::to_value(&OrchestrationV2TurnItem::UserMessage(item))?,
                    identity,
                )
            }
            ProviderEventKind::BackgroundWork { .. }
            | ProviderEventKind::CommandOutput { .. }
            | ProviderEventKind::PromptSuggestion { .. } => return Ok(events),
            ProviderEventKind::Terminal {
                provider_turn_id, ..
            } => {
                events.extend(
                    self.dismiss_native_inputs(
                        input,
                        provider_turn_id.clone(),
                        input.thread_id.clone(),
                        &now,
                    )
                    .await?,
                );
                let mut terminal = input.event.clone();
                if let ProviderEventKind::Terminal {
                    outcome: TerminalOutcome::Failed { failure, .. },
                    ..
                } = &mut terminal.kind
                    && let Some(scope) = &self.agent_scope
                    && scope.oom_killed(input.thread_id.clone()).await?
                {
                    failure.message = t3_contracts::base_schemas::TrimmedNonEmptyString(
                        "Killed: out of memory.".into(),
                    );
                    failure.code = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                        "oom_kill".into(),
                    ));
                }
                if let ProviderEventKind::Terminal {
                    outcome: TerminalOutcome::Failed { failure, .. },
                    ..
                } = &mut terminal.kind
                    && failure.class == OrchestrationV2ProviderFailureClass::UsageLimit
                    && failure.reset_at.as_ref().and_then(Option::as_ref).is_none()
                    && let Some(resets) = &self.usage_limit_resets
                    && let Some(endpoint) = resets.endpoint(input.instance_id.clone()).await?
                {
                    let model = async {
                        if let Some(run_id) = &input.run_id
                            && let Some(run) = self
                                .projections
                                .run(input.thread_id.clone(), run_id.clone())
                                .await?
                        {
                            return Ok::<_, Error>(Some(run.model_selection.model));
                        }
                        Ok(self
                            .projections
                            .thread(input.thread_id.clone())
                            .await?
                            .map(|thread| thread.model_selection.model))
                    }
                    .await
                    .unwrap_or(None);
                    if let Some(reset) = resets
                        .reset_at(input.event.driver.clone(), endpoint, model)
                        .await?
                    {
                        failure.reset_at = Some(Some(reset));
                    }
                }
                let Some(item) = terminal_failure_item(
                    &terminal,
                    &input.thread_id,
                    input.run_id.as_ref(),
                    input.node_id.as_ref(),
                    &now,
                )?
                else {
                    return Ok(events);
                };
                (
                    "turn-item.updated",
                    t3_contracts::stored::to_value(&item)?,
                    json!({}),
                )
            }
        };
        events.push(make(kind, payload, identity)?);
        Ok(events)
    }

    pub async fn ingest_normalized(
        &self,
        input: &IngestInput,
        guard: CommitGuard,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        self.ingest_normalized_with_context(input, guard, IngestionContext::default())
            .await
    }

    pub async fn ingest_normalized_with_context(
        &self,
        input: &IngestInput,
        guard: CommitGuard,
        context: IngestionContext,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        // Native child creation shares the command lane through normalization and commit.
        let _guards = if let ProviderEventKind::AppThread { app_thread, .. } = &input.event.kind {
            self.lanes.acquire([expanded_id(&app_thread.id)?.0]).await
        } else {
            Vec::new()
        };
        let events = self.normalize(input).await?;
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let result = self
            .commit_with_provenance(
                CommitRequest {
                    receipt: None,
                    decision: Decision {
                        events,
                        ..Default::default()
                    },
                    guard,
                },
                context.command_id,
            )
            .await?;
        if !result.committed {
            return Ok(Vec::new());
        }
        let mut stored = result.events;
        if stored.is_empty() {
            return Ok(stored);
        }
        if let Some(telemetry) = &self.telemetry {
            if matches!(&input.event.kind, ProviderEventKind::Terminal { outcome: TerminalOutcome::Failed { failure, .. }, .. }
                if failure.class == OrchestrationV2ProviderFailureClass::UsageLimit)
            {
                let closed = self
                    .telemetry_closed
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if !*closed {
                    let telemetry = telemetry.clone();
                    let instance_id = input.instance_id.clone();
                    let cancellation = self.telemetry_cancellation.child_token();
                    self.telemetry_tasks.spawn(async move {
                        tokio::select! {
                            _ = cancellation.cancelled() => {},
                            _ = telemetry.refresh_usage(instance_id, cancellation.clone()) => {},
                        }
                    });
                }
            }
            if let ProviderEventKind::Turn { provider_turn, .. } = &input.event.kind
                && matches!(
                    provider_turn.status,
                    ProviderTurnStatus::Completed
                        | ProviderTurnStatus::Failed
                        | ProviderTurnStatus::Interrupted
                        | ProviderTurnStatus::Cancelled
                )
            {
                let key = (input.instance_id.clone(), expanded_id(&provider_turn.id)?);
                let report = self
                    .completed_turns
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(key);
                if report {
                    let _ = telemetry
                        .turn_completed(
                            CompletedProviderTurn {
                                driver: input.event.driver.clone(),
                                provider_turn: *provider_turn.clone(),
                                context: context.analytics,
                            },
                            self.telemetry_cancellation.child_token(),
                        )
                        .await;
                }
            }
        }
        if let ProviderEventKind::Subagent(agent) = &input.event.kind {
            stored.extend(self.sync_subagent_model(input, agent).await?);
        }
        if let Some(workspace) = &self.workspace {
            let child = stored.iter().find_map(|stored| match &stored.event {
                OrchestrationV2DomainEvent::ThreadCreated(event) => Some(&event.payload),
                _ => None,
            });
            if let Some(child) = child
                && let (Some(cwd), Some(branch)) = (&child.worktree_path, &child.branch)
                && let Ok(Some(current)) = workspace
                    .current_branch(std::path::PathBuf::from(&cwd.0))
                    .await
                && current != branch.0
            {
                let mut payload = t3_contracts::stored::to_value(&child)?;
                let now = self.clock.now();
                payload["branch"] = current.into();
                payload["updatedAt"] = json!(now);
                let event = input.domain_event(
                    self.ids.as_ref(),
                    &now,
                    "thread.metadata-updated",
                    payload,
                    json!({"threadId":child.id}),
                )?;
                if let Ok(result) = self
                    .committer
                    .commit(CommitRequest {
                        receipt: None,
                        decision: Decision {
                            events: vec![event],
                            ..Default::default()
                        },
                        guard: CommitGuard::None,
                    })
                    .await
                {
                    stored.extend(result.events);
                }
            }
        }
        Ok(stored)
    }

    async fn sync_subagent_model(
        &self,
        input: &IngestInput,
        agent: &OrchestrationV2Subagent,
    ) -> Result<Vec<OrchestrationV2StoredEvent>> {
        if agent.origin != SubagentOrigin::ProviderNative
            || agent.child_thread_id.is_none()
            || agent.model.is_none() && agent.model_selection.is_none()
        {
            return Ok(Vec::new());
        }
        let Some(child_id) = agent
            .child_thread_id
            .as_ref()
            .map(expanded_id)
            .transpose()?
        else {
            return Ok(Vec::new());
        };
        let _guards = self.lanes.acquire([child_id.0.clone()]).await;
        let Some(mut child) = self.projections.thread(child_id).await? else {
            return Ok(Vec::new());
        };
        let selection =
            agent
                .model_selection
                .clone()
                .flatten()
                .unwrap_or_else(|| match &agent.model {
                    Some(model) if model != &child.model_selection.model => {
                        let mut selection = child.model_selection.clone();
                        selection.model = model.clone();
                        selection.options = None;
                        selection
                    }
                    _ => child.model_selection.clone(),
                });
        if model_selections_equal(&child.model_selection, &selection) {
            return Ok(Vec::new());
        }
        child.model_selection = selection;
        child.updated_at = self.clock.now();
        let event = input.domain_event(
            self.ids.as_ref(),
            &child.updated_at,
            "thread.model-selection-updated",
            t3_contracts::stored::to_value(&child)?,
            json!({"threadId":child.id}),
        )?;
        Ok(self
            .commit_with_retry(CommitRequest {
                receipt: None,
                decision: Decision {
                    events: vec![event],
                    ..Default::default()
                },
                guard: CommitGuard::None,
            })
            .await?
            .events)
    }

    pub async fn commit_with_retry(&self, request: CommitRequest) -> Result<CommitResult> {
        self.commit_with_provenance(request, None).await
    }

    async fn commit_with_provenance(
        &self,
        request: CommitRequest,
        command_id: Option<CommandId>,
    ) -> Result<CommitResult> {
        let guarded = command_id.is_some();
        self.commit_with_retry_options(
            request,
            CommitOptions {
                command_id,
                guard_pending_user_input_cancellations: guarded,
                ..Default::default()
            },
        )
        .await
    }

    pub async fn commit_with_retry_options(
        &self,
        request: CommitRequest,
        options: CommitOptions,
    ) -> Result<CommitResult> {
        for attempt in 0..3 {
            let result = self
                .committer
                .commit_with_options(request.clone(), options.clone())
                .await;
            match result {
                Err(Error::Sql(t3_persistence::rusqlite::Error::SqliteFailure(error, _)))
                    if error.code == t3_persistence::rusqlite::ErrorCode::DatabaseBusy
                        && attempt < 2 => {}
                result => return result,
            }
        }
        unreachable!("the final attempt returns its result")
    }

    async fn dismiss_native_inputs(
        &self,
        input: &IngestInput,
        turn_id: ProviderTurnId,
        thread_id: ThreadId,
        now: &IsoDateTime,
    ) -> Result<Vec<OrchestrationV2DomainEvent>> {
        let records = self
            .projections
            .records(
                thread_id.clone(),
                vec![
                    RecordKind::RuntimeRequests,
                    RecordKind::Nodes,
                    RecordKind::TurnItems,
                ],
                RecordFilter {
                    runtime_request_statuses: Some(vec![RuntimeRequestStatus::Pending]),
                    ..Default::default()
                },
            )
            .await?;
        let mut requests = Vec::new();
        let mut nodes = Vec::new();
        let mut items = Vec::new();
        for set in records.records {
            match set {
                ProjectionRecordSet::RuntimeRequests(values) => requests.extend(values),
                ProjectionRecordSet::Nodes(values) => nodes.extend(values),
                ProjectionRecordSet::TurnItems(values) => items.extend(values),
                _ => {}
            }
        }
        let mut pending = Vec::new();
        for request in requests {
            if request.status == RuntimeRequestStatus::Pending
                && request.provider_turn_id == Some(IdOrIndex::Id(turn_id.clone()))
                && matches!(request.kind, RuntimeRequestKind::Member2(ref kind) if **kind == RuntimeRequestKindMember2::UserInput)
                && !matches!(
                    request.response_capability,
                    RuntimeRequestResponseCapability::Message(_)
                )
            {
                pending.push(request);
            }
        }
        let mut events = Vec::new();
        for request in &pending {
            let mut payload = t3_contracts::stored::to_value(&request)?;
            payload["status"] = "cancelled".into();
            payload["resolvedAt"] = json!(now);
            events.push(input.domain_event(
                self.ids.as_ref(),
                now,
                "runtime-request.updated",
                payload,
                json!({"threadId":thread_id,"nodeId":request.node_id}),
            )?);
        }
        for node in nodes
            .into_iter()
            .filter(|node| pending.iter().any(|request| request.node_id == node.id))
        {
            let mut payload = t3_contracts::stored::to_value(&node)?;
            payload["status"] = "cancelled".into();
            payload["completedAt"] = json!(now);
            events.push(input.domain_event(
                self.ids.as_ref(),
                now,
                "node.updated",
                payload,
                json!({"threadId":thread_id,"runId":node.run_id,"nodeId":node.id}),
            )?);
        }
        for item in items {
            let OrchestrationV2TurnItem::UserInputRequest(item) = item else {
                continue;
            };
            if item.provider_turn_id != Some(IdOrIndex::Id(turn_id.clone()))
                || !pending.iter().any(|request| request.id == item.request_id)
            {
                continue;
            }
            let identity = json!({"threadId":thread_id,"runId":item.run_id,"nodeId":item.node_id});
            let mut payload =
                t3_contracts::stored::to_value(&OrchestrationV2TurnItem::UserInputRequest(item))?;
            payload["status"] = "cancelled".into();
            payload["completedAt"] = json!(now);
            payload["updatedAt"] = json!(now);
            events.push(input.domain_event(
                self.ids.as_ref(),
                now,
                "turn-item.updated",
                payload,
                identity,
            )?);
        }
        Ok(events)
    }

    async fn rewind_events(
        &self,
        input: &IngestInput,
        thread_id: ThreadId,
        provider_thread_id: ProviderThreadId,
        retained: &[String],
        now: &IsoDateTime,
    ) -> Result<Vec<OrchestrationV2DomainEvent>> {
        let records = self
            .projections
            .records(
                thread_id.clone(),
                vec![
                    RecordKind::Runs,
                    RecordKind::Attempts,
                    RecordKind::ProviderTurns,
                    RecordKind::Nodes,
                    RecordKind::Checkpoints,
                ],
                RecordFilter::default(),
            )
            .await?;
        let mut runs = Vec::new();
        let mut attempts = Vec::new();
        let mut turns = Vec::new();
        let mut nodes = Vec::new();
        let mut checkpoints = Vec::new();
        for set in records.records {
            match set {
                ProjectionRecordSet::Runs(values) => runs.extend(values),
                ProjectionRecordSet::Attempts(values) => attempts.extend(values),
                ProjectionRecordSet::ProviderTurns(values) => turns.extend(values),
                ProjectionRecordSet::Nodes(values) => nodes.extend(values),
                ProjectionRecordSet::Checkpoints(values) => checkpoints.extend(values),
                _ => {}
            }
        }
        let rewound = runs_off_native_branch(
            &runs,
            &attempts,
            &turns,
            &provider_thread_id,
            retained,
            input.run_id.as_ref(),
        );
        let mut events = Vec::new();
        for mut checkpoint in checkpoints {
            if checkpoint.status != CheckpointStatus::Ready
                || !rewound
                    .iter()
                    .any(|run| checkpoint.run_id.as_ref() == Some(&run.id))
            {
                continue;
            }
            checkpoint.status = CheckpointStatus::Stale;
            let mut event = input.domain_event(
                self.ids.as_ref(),
                now,
                "checkpoint.captured",
                t3_contracts::stored::to_value(&checkpoint)?,
                json!({"threadId":thread_id,"runId":checkpoint.run_id,"nodeId":checkpoint.node_id}),
            )?;
            clear_provider_metadata(&mut event, false)?;
            events.push(event);
        }
        for run in rewound {
            let node = nodes
                .iter()
                .find(|node| run.root_node_id.as_ref() == Some(&node.id));
            let mut payload = t3_contracts::stored::to_value(&run)?;
            payload["status"] = "rolled_back".into();
            payload["completedAt"] = json!(now);
            let identity = json!({"threadId":thread_id,"runId":run.id,"nodeId":node.map(|node| &node.id),"providerInstanceId":run.provider_instance_id});
            let mut event = input.domain_event(
                self.ids.as_ref(),
                now,
                "run.updated",
                payload,
                identity.clone(),
            )?;
            clear_provider_metadata(&mut event, node.is_none())?;
            events.push(event);
            if let Some(node) = node {
                let mut payload = t3_contracts::stored::to_value(&node)?;
                payload["status"] = "rolled_back".into();
                payload["completedAt"] = json!(now);
                let mut event = input.domain_event(
                    self.ids.as_ref(),
                    now,
                    "node.updated",
                    payload,
                    identity,
                )?;
                clear_provider_metadata(&mut event, false)?;
                events.push(event);
            }
        }
        Ok(events)
    }
}

#[async_trait]
impl EventIngestor for ProviderEventIngestor {
    async fn ingest_with_context(
        &self,
        session_id: ProviderSessionId,
        instance_id: ProviderInstanceId,
        thread_id: ThreadId,
        event: ProviderEvent,
        context: IngestionContext,
    ) -> Result<()> {
        self.ingest_normalized_with_context(
            &IngestInput {
                session_id,
                instance_id,
                thread_id,
                event,
                run_id: None,
                node_id: None,
                raw_event_id: None,
            },
            CommitGuard::None,
            context,
        )
        .await?;
        Ok(())
    }
    async fn ingest(
        &self,
        session_id: ProviderSessionId,
        instance_id: ProviderInstanceId,
        thread_id: ThreadId,
        event: ProviderEvent,
    ) -> Result<()> {
        self.ingest_normalized(
            &IngestInput {
                session_id,
                instance_id,
                thread_id,
                event,
                run_id: None,
                node_id: None,
                raw_event_id: None,
            },
            CommitGuard::None,
        )
        .await?;
        Ok(())
    }
}

fn clear_provider_metadata(
    event: &mut OrchestrationV2DomainEvent,
    remove_node: bool,
) -> Result<()> {
    let mut value = t3_contracts::stored::to_value(&*event)?;
    if let Some(object) = value.as_object_mut() {
        object.shift_remove("driver");
        object.shift_remove("rawEventId");
        if remove_node {
            object.shift_remove("nodeId");
        }
    }
    *event = serde_json::from_value(value)?;
    Ok(())
}

pub fn runs_off_native_branch<'a>(
    runs: &'a [OrchestrationV2Run],
    attempts: &[OrchestrationV2RunAttempt],
    turns: &[OrchestrationV2ProviderTurn],
    provider_thread_id: &ProviderThreadId,
    retained: &[String],
    reporting_run_id: Option<&RunId>,
) -> Vec<&'a OrchestrationV2Run> {
    let retained: HashSet<_> = retained.iter().map(String::as_str).collect();
    let settled: Vec<_> = runs
        .iter()
        .filter(|run| {
            run.provider_thread_id == Some(IdOrIndex::Id(provider_thread_id.clone()))
                && reporting_run_id
                    .map(|id| run.id != IdOrIndex::Id(id.clone()))
                    .unwrap_or(true)
                && matches!(
                    run.status,
                    OrchestrationV2RunStatus::Completed
                        | OrchestrationV2RunStatus::Interrupted
                        | OrchestrationV2RunStatus::Failed
                )
        })
        .map(|run| {
            let native_id = attempts
                .iter()
                .find(|attempt| Some(&attempt.id) == run.active_attempt_id.as_ref())
                .and_then(|attempt| {
                    turns.iter().find(|turn| {
                        Some(&turn.id) == attempt.provider_turn_id.as_ref()
                            || turn.run_attempt_id.as_ref() == Some(&attempt.id)
                    })
                })
                .and_then(|turn| turn.native_turn_ref.as_ref())
                .filter(|reference| reference.strength == OrchestrationV2NativeRefStrength::Strong)
                .and_then(|reference| reference.native_id.as_ref())
                .map(|id| id.0.as_str());
            (run, native_id)
        })
        .collect();
    let last_retained = settled
        .iter()
        .filter(|(_, native)| native.is_some_and(|id| retained.contains(id)))
        .map(|(run, _)| run.ordinal)
        .max()
        .unwrap_or(0);
    settled
        .into_iter()
        .filter(|(run, native)| {
            native
                .map(|id| !retained.contains(id))
                .unwrap_or(run.ordinal > last_retained)
        })
        .map(|(run, _)| run)
        .collect()
}

pub fn with_plan_step_durations(
    plan: &mut PlanArtifactTodoList,
    previous: Option<&PlanArtifactTodoList>,
    now: &IsoDateTime,
) {
    let previous_step = |step: &OrchestrationV2PlanStep| {
        previous
            .and_then(|plan| plan.steps.iter().rev().find(|prior| prior.id == step.id))
            .filter(|prior| prior.text == step.text)
    };
    let has_new_completion = plan.steps.iter().any(|step| {
        step.status == PlanStepStatus::Completed
            && previous_step(step).is_none_or(|prior| prior.status != PlanStepStatus::Completed)
    });
    let mut fallback_consumed = false;
    for step in &mut plan.steps {
        let prior = previous_step(step);
        step.duration_anchor_at = None;
        step.duration_ms = None;
        match step.status {
            PlanStepStatus::Completed => {
                if let Some(prior) = prior.filter(|prior| prior.status == PlanStepStatus::Completed)
                {
                    step.duration_ms = prior.duration_ms;
                    continue;
                }
                let anchor = if prior.is_some_and(|prior| prior.status == PlanStepStatus::Running)
                    || !fallback_consumed
                {
                    prior.and_then(|prior| {
                        prior.duration_anchor_at.as_ref().and_then(Option::as_ref)
                    })
                } else {
                    Some(now)
                };
                fallback_consumed = true;
                if let Some(duration) = anchor
                    .and_then(|anchor| epoch_millis(&now.0).zip(epoch_millis(&anchor.0)))
                    .map(|(now, anchor)| now.saturating_sub(anchor).max(0) as u64)
                    .filter(|duration| *duration > 0)
                {
                    step.duration_ms = Some(Some(duration));
                }
            }
            PlanStepStatus::Running => {
                step.duration_anchor_at = Some(Some(
                    prior
                        .filter(|prior| prior.status == PlanStepStatus::Running)
                        .and_then(|prior| prior.duration_anchor_at.clone().flatten())
                        .unwrap_or_else(|| now.clone()),
                ))
            }
            PlanStepStatus::Pending => {
                step.duration_anchor_at = Some(Some(
                    if has_new_completion
                        || prior.is_none_or(|prior| prior.status != PlanStepStatus::Pending)
                    {
                        now.clone()
                    } else {
                        prior
                            .and_then(|prior| prior.duration_anchor_at.clone().flatten())
                            .unwrap_or_else(|| now.clone())
                    },
                ))
            }
        }
    }
}

pub use crate::ports::decisions::orchestrator::epoch_millis;

pub fn derive_id(prefix: &str, parts: &[&str]) -> String {
    let mut id = prefix.to_owned();
    for part in parts {
        id.push(':');
        for byte in part.bytes() {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                id.push(char::from(byte));
            } else {
                use std::fmt::Write;
                let _ = write!(id, "%{byte:02X}");
            }
        }
    }
    id
}

pub fn failure_item(
    input: &IngestInput,
    now: &IsoDateTime,
) -> Result<Option<OrchestrationV2TurnItem>> {
    terminal_failure_item(
        &input.event,
        &input.thread_id,
        input.run_id.as_ref(),
        input.node_id.as_ref(),
        now,
    )
}

pub fn terminal_failure_item(
    event: &ProviderEvent,
    thread_id: &ThreadId,
    run_id: Option<&RunId>,
    node_id: Option<&NodeId>,
    now: &IsoDateTime,
) -> Result<Option<OrchestrationV2TurnItem>> {
    let ProviderEventKind::Terminal {
        provider_thread_id,
        provider_turn_id,
        outcome:
            TerminalOutcome::Failed {
                failure,
                failure_item_ordinal,
                retry,
                retry_started_at,
                ..
            },
        ..
    } = &event.kind
    else {
        return Ok(None);
    };
    let driver = t3_contracts::stored::to_value(&event.driver)?;
    let driver = driver.as_str().ok_or_else(|| Error::Decode {
        entity: "provider driver",
        detail: "expected string".into(),
    })?;
    let mut item = json!({"id":derive_id("turn-item", &["provider",driver,"native-item",&format!("terminal-failure:{}",provider_turn_id.0)]), "threadId":thread_id,"runId":run_id,"nodeId":node_id,"providerThreadId":provider_thread_id,"providerTurnId":provider_turn_id,"nativeItemRef":null,"parentItemId":null,"ordinal":failure_item_ordinal,"status":"failed","title":if t3_contracts::stored::to_value(&failure)?["class"] == "usage_limit" {"Usage limit reached"} else {"Provider error"},"startedAt":retry_started_at.as_ref().unwrap_or(now),"completedAt":now,"updatedAt":now,"type":"error","failure":failure});
    if let Some(retry) = retry {
        item["retry"] = t3_contracts::stored::to_value(&retry)?;
    }
    Ok(Some(serde_json::from_value(item)?))
}

fn bound_item(item: &mut Value) {
    if item["type"] == "file_change" {
        for key in ["diffStr", "oldStr", "newStr"] {
            if let Some(value) = item[key].as_str().filter(|value| value.len() > 32768) {
                let marker = "\n… output truncated for transport";
                let mut end = 32768 - marker.len();
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                let prefix = value[..end]
                    .strip_suffix('\u{fffd}')
                    .unwrap_or(&value[..end]);
                item[key] = format!("{prefix}{marker}").into();
            }
        }
    }
    if item["type"] == "dynamic_tool"
        && let Some(output) = item.get_mut("output")
    {
        strip_unserved_images(output);
    }
}

fn strip_unserved_images(output: &mut Value) {
    let blocks = if let Some(blocks) = output.as_array() {
        blocks.iter().collect::<Vec<_>>()
    } else if let Some(blocks) = output.get("content").and_then(Value::as_array) {
        blocks.iter().collect()
    } else {
        vec![&*output]
    };
    let mut served = Vec::new();
    for block in blocks {
        if block["type"] != "image" {
            continue;
        }
        let source = block.get("source").filter(|source| source.is_object());
        if source.is_some_and(|source| source["type"] != "base64") {
            continue;
        }
        let mime = source
            .map(|source| &source["media_type"])
            .unwrap_or(&block["mimeType"]);
        if !mime.as_str().is_some_and(|mime| {
            ["image/png", "image/jpeg", "image/webp", "image/gif"]
                .contains(&mime.to_lowercase().as_str())
        }) {
            continue;
        }
        served.push(block as *const Value as usize);
        if served.len() == 8 {
            break;
        }
    }
    strip_image_value(output, None, &served, 0);
}

fn strip_image_value(value: &mut Value, key: Option<&str>, served: &[usize], depth: usize) {
    if depth > 32 {
        return;
    }
    let is_served = served.contains(&(value as *const Value as usize));
    match value {
        Value::Array(values) => {
            for value in values {
                strip_image_value(value, None, served, depth + 1);
            }
        }
        Value::Object(record) => {
            let data = record
                .get("source")
                .filter(|source| source.is_object())
                .map(|source| &source["data"])
                .or_else(|| record.get("data"));
            if record.get("type").is_some_and(|kind| kind == "image")
                && is_served
                && data
                    .and_then(Value::as_str)
                    .is_none_or(|data| data.len() <= (10_usize * 1024 * 1024).div_ceil(3) * 4)
            {
                return;
            }
            let image_mime = ["mimeType", "mime_type", "media_type", "type"]
                .iter()
                .any(|key| {
                    record
                        .get(*key)
                        .and_then(Value::as_str)
                        .is_some_and(|mime| mime.to_lowercase().starts_with("image/"))
                });
            let image_body = record.get("type").is_some_and(|kind| kind == "image")
                || key == Some("image")
                || image_mime
                    && record.get("type").is_none_or(|kind| {
                        kind == "base64"
                            || kind
                                .as_str()
                                .is_some_and(|kind| kind.to_lowercase().starts_with("image/"))
                    });
            let mut removed = None;
            record.retain(|key, value| {
                if image_body
                    && ["data", "blob", "base64"].contains(&key.as_str())
                    && value.as_str().is_some_and(is_base64_body)
                {
                    removed = value
                        .as_str()
                        .map(|body| body.trim_end_matches('=').len() * 3 / 4);
                    false
                } else {
                    strip_image_value(value, Some(key), served, depth + 1);
                    true
                }
            });
            if let Some(length) = removed {
                record.insert("sizeBytes".into(), json!(length));
            }
        }
        _ => {}
    }
}

fn is_base64_body(value: &str) -> bool {
    value.len() >= 64
        && value.bytes().take_while(|byte| *byte != b'=').count() > 0
        && value
            .trim_end_matches('=')
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+/_-".contains(&byte))
        && value.len() - value.trim_end_matches('=').len() <= 2
}
