use crate::{
    api::*,
    effects::{EffectRequest, PendingEffect, TitleKind},
    error::{Error, Result},
    ports::decisions::{
        guest_share_decider::{self, GuestShareCommand},
        orchestrator::{command_identity, epoch_millis, iso_from_epoch_millis, rejection},
        queued_run_order::queued_runs_in_delivery_order,
    },
};
use serde_json::{Value, json};
use t3_contracts::{provider_instance::ProviderInstanceId, stored};

pub(crate) fn stored_value(value: impl serde::Serialize) -> serde_json::Result<Value> {
    stored::to_value(&value)
}

#[derive(Debug, Clone)]
pub struct PreparedMessageContext {
    pub provider_thread: OrchestrationV2ProviderThread,
    pub handoff: Option<OrchestrationV2ContextHandoff>,
    pub transfers: Vec<OrchestrationV2ContextTransfer>,
    pub released_sessions: Vec<OrchestrationV2ProviderSession>,
}

#[async_trait::async_trait]
pub trait MessageContextPreparer: Send + Sync {
    async fn prepare(
        &self,
        context: &PreparedDecisionContext,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<Option<PreparedMessageContext>>;
}

#[derive(Default)]
pub struct PureDecisionEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandDecision {
    Create,
    GuestShare,
    ThreadMutation,
    SelectionMutation,
    Settlement,
    Detach,
    QueueReorder,
    QueueEdit,
    QueueCancel,
    QueueResume,
    RuntimeResponse,
    UserInputDismiss,
    RuntimeDeliveryFailure,
    RollbackFailure,
    InterruptUnresponsive,
    PreparedRun,
    SecretRecord,
    CreatedThreadRecord,
    Fork,
    MergeBack,
    AdoptFolder,
    RemoteStop,
    SubagentStop,
    RemoteRequest,
    ReopenChecks,
    Message,
    Interrupt,
    Stop,
    BackgroundSettle,
    CompletionResolution,
    RemoteComplete,
    WakePolicy,
    NotificationAccept,
    Delete,
    Rollback,
    LinkRecord,
    DelegatedRequest,
    Promote,
    PullRequest,
}

pub fn command_decision(kind: &str) -> Option<CommandDecision> {
    Some(match kind {
        "thread.create" => CommandDecision::Create,
        "thread.guest-share.open"
        | "thread.guest-share.redeem"
        | "thread.guest-share.close"
        | "thread.guest-message.submit"
        | "thread.guest-message.dismiss" => CommandDecision::GuestShare,
        "thread.unarchive"
        | "thread.unsettle"
        | "thread.snooze"
        | "thread.unsnooze"
        | "thread.auto-settle.set"
        | "thread.pin"
        | "thread.unpin"
        | "thread.pin.reorder"
        | "thread.active.reorder"
        | "thread.tag.set"
        | "thread.mark-unread"
        | "thread.visit"
        | "thread.title.regeneration.complete"
        | "thread.interaction-mode.set" => CommandDecision::ThreadMutation,
        "thread.metadata.update"
        | "thread.runtime-mode.set"
        | "thread.model-selection.set"
        | "provider.switch" => CommandDecision::SelectionMutation,
        "thread.archive" | "thread.settle" | "thread.auto-settle" => CommandDecision::Settlement,
        "provider-session.detach" => CommandDecision::Detach,
        "queued-run.reorder" => CommandDecision::QueueReorder,
        "queued-run.edit" => CommandDecision::QueueEdit,
        "queued-run.cancel" => CommandDecision::QueueCancel,
        "queued-run.start" | "queue.resume" => CommandDecision::QueueResume,
        "runtime-request.respond" => CommandDecision::RuntimeResponse,
        "thread.user-input.dismiss" => CommandDecision::UserInputDismiss,
        "runtime-request.delivery.fail" => CommandDecision::RuntimeDeliveryFailure,
        "checkpoint.rollback.fail" => CommandDecision::RollbackFailure,
        "thread.interrupt.unresponsive" => CommandDecision::InterruptUnresponsive,
        "secret_request.record" => CommandDecision::SecretRecord,
        "thread.created.record" => CommandDecision::CreatedThreadRecord,
        "thread.fork" => CommandDecision::Fork,
        "thread.merge_back" => CommandDecision::MergeBack,
        "thread.project.adopt-folder" => CommandDecision::AdoptFolder,
        "delegated_task.remote.stop" => CommandDecision::RemoteStop,
        "subagent.stop" => CommandDecision::SubagentStop,
        "delegated_task.remote.request" => CommandDecision::RemoteRequest,
        "thread.pull-request-watch.checks-reopen" => CommandDecision::ReopenChecks,
        "message.dispatch" => CommandDecision::Message,
        "run.interrupt" => CommandDecision::Interrupt,
        "thread.stop" => CommandDecision::Stop,
        "thread.background-work.settle" => CommandDecision::BackgroundSettle,
        "delegated_task.remote.complete" => CommandDecision::RemoteComplete,
        "delegated_task.wake-policy" => CommandDecision::WakePolicy,
        "notification.delivery.accept" => CommandDecision::NotificationAccept,
        "thread.delete" => CommandDecision::Delete,
        "checkpoint.rollback" => CommandDecision::Rollback,
        "link_request.record" => CommandDecision::LinkRecord,
        "delegated_task.request" => CommandDecision::DelegatedRequest,
        "queued-message.promote-to-steer" => CommandDecision::Promote,
        "thread.pull-request.link"
        | "thread.pull-request.unlink"
        | "thread.pull-request-link.sync"
        | "thread.pull-request.sync"
        | "thread.pull-request.watch"
        | "thread.pull-request-watch.sync" => CommandDecision::PullRequest,
        "delegated_task.completion-delivery.acknowledge"
        | "delegated_task.completion-delivery.dispose" => CommandDecision::CompletionResolution,
        "prepared-run.release"
        | "prepared-run.progress"
        | "prepared-run.fail"
        | "prepared-run.retry" => CommandDecision::PreparedRun,
        _ => return None,
    })
}

impl DecisionEngine for PureDecisionEngine {
    fn decide(
        &self,
        context: &DecisionContext,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<Decision> {
        self.plan(context, None, command)
    }

    fn decide_prepared(
        &self,
        context: &PreparedDecisionContext,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<Decision> {
        self.plan(&context.context, Some(&context.inputs), command)
    }
}

impl PureDecisionEngine {
    pub fn decide_delegated_recovery(
        &self,
        context: &PreparedDecisionContext,
        child: Option<&OrchestrationV2ThreadProjection>,
        completed: Option<&RunId>,
        settled_source: Option<&RunId>,
        pending_restarts: &[RunId],
    ) -> Result<Decision> {
        let thread = context.context.projection.as_ref().ok_or(Error::Stale)?;
        let thread_id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &thread.thread.id,
        )?;
        let mut plan = Plan {
            context: &context.context,
            inputs: Some(&context.inputs),
            command: json!({"type":"system:delegated-recovery","commandId":format!("command:system:delegated-recovery:{}",thread_id.0),"threadId":thread_id}),
            projection: stored_value(thread)?,
            thread_id,
            decision: Decision::default(),
            id_positions: Vec::new(),
        };
        if let Some(completed) = completed {
            plan.finalize_delivery(completed)?;
        }
        if let Some(child) = child {
            plan.finalize_child_tasks(child, settled_source, pending_restarts)?;
        }
        Ok(plan.decision)
    }

    pub fn decide_queued_start(
        &self,
        context: &PreparedDecisionContext,
        run_id: &RunId,
    ) -> Result<Decision> {
        self.decide_queued_start_after(context, run_id, None)
    }

    pub fn decide_queued_start_after(
        &self,
        context: &PreparedDecisionContext,
        run_id: &RunId,
        ended: Option<&RunId>,
    ) -> Result<Decision> {
        let thread_id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &context
                .context
                .projection
                .as_ref()
                .ok_or(Error::Stale)?
                .thread
                .id,
        )?;
        let mut plan = Plan {
            context: &context.context,
            inputs: Some(&context.inputs),
            command: json!({"type":"message.dispatch","commandId":format!("command:system:start-queued:{}",run_id.0),"threadId":thread_id,"runId":run_id,"endedRunId":ended}),
            projection: stored_value(context.context.projection.as_ref().ok_or(Error::Stale)?)?,
            thread_id,
            decision: Decision::default(),
            id_positions: Vec::new(),
        };
        plan.queue_start()?;
        Ok(plan.decision)
    }

    pub fn decide_delegated_sync(
        &self,
        context: &PreparedDecisionContext,
        child: &OrchestrationV2ThreadProjection,
        id: &RunId,
    ) -> Result<Decision> {
        let projection = context.context.projection.as_ref().ok_or(Error::Stale)?;
        let thread_id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &projection.thread.id,
        )?;
        let mut plan = Plan {
            context: &context.context,
            inputs: Some(&context.inputs),
            command: json!({"type":"system:delegated-sync","threadId":thread_id}),
            projection: stored_value(projection)?,
            thread_id,
            decision: Decision::default(),
            id_positions: Vec::new(),
        };
        plan.sync_child_tasks(child, id)?;
        Ok(plan.decision)
    }

    pub fn decide_queued_failure(
        &self,
        context: &PreparedDecisionContext,
        id: &RunId,
        failure: OrchestrationV2ProviderFailure,
    ) -> Result<Decision> {
        let projection = context.context.projection.as_ref().ok_or(Error::Stale)?;
        let thread_id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &projection.thread.id,
        )?;
        let mut plan = Plan {
            context: &context.context,
            inputs: Some(&context.inputs),
            command: json!({"type":"system:queued-failure","commandId":format!("command:system:queued-start-failure:{}",id.0),"threadId":thread_id}),
            projection: stored_value(projection)?,
            thread_id,
            decision: Decision::default(),
            id_positions: Vec::new(),
        };
        plan.fail_queued(id, stored_value(failure)?)?;
        Ok(plan.decision)
    }

    pub fn decide_after_run(
        &self,
        context: &PreparedDecisionContext,
        id: &RunId,
    ) -> Result<Decision> {
        let projection = context.context.projection.as_ref().ok_or(Error::Stale)?;
        let thread_id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &projection.thread.id,
        )?;
        let mut plan = Plan {
            context: &context.context,
            inputs: Some(&context.inputs),
            command: json!({"type":"system:after-run","threadId":thread_id,"runId":id}),
            projection: stored_value(projection)?,
            thread_id,
            decision: Decision::default(),
            id_positions: Vec::new(),
        };
        plan.after_run(id)?;
        Ok(plan.decision)
    }

    fn plan(
        &self,
        context: &DecisionContext,
        inputs: Option<&DecisionInputs>,
        command: &OrchestrationV2ServerCommand,
    ) -> Result<Decision> {
        let (_, kind, thread_id) = command_identity(command)?;
        let command_value = stored_value(command)?;
        let mut plan = Plan {
            context,
            inputs,
            command: command_value,
            projection: context
                .projection
                .as_ref()
                .map(stored_value)
                .transpose()?
                .unwrap_or(Value::Null),
            thread_id,
            decision: Decision::default(),
            id_positions: Vec::new(),
        };
        match command_decision(&kind).ok_or(Error::Unsupported(
            "command decision requires the remaining Orchestrator port",
        ))? {
            CommandDecision::Create => plan.create()?,
            CommandDecision::GuestShare => plan.guest_share(command)?,
            CommandDecision::ThreadMutation => plan.thread_mutation(&kind)?,
            CommandDecision::SelectionMutation => plan.selection_mutation(&kind)?,
            CommandDecision::Settlement => plan.settlement(&kind)?,
            CommandDecision::Detach => plan.detach()?,
            CommandDecision::QueueReorder => plan.queue_reorder()?,
            CommandDecision::QueueEdit => plan.queue_edit()?,
            CommandDecision::QueueCancel => plan.queue_cancel()?,
            CommandDecision::QueueResume => plan.queue_resume()?,
            CommandDecision::RuntimeResponse => plan.runtime_response()?,
            CommandDecision::UserInputDismiss => plan.user_input_dismiss()?,
            CommandDecision::RuntimeDeliveryFailure => plan.runtime_delivery_failure()?,
            CommandDecision::RollbackFailure => plan.rollback_failure()?,
            CommandDecision::InterruptUnresponsive => plan.interrupt_unresponsive()?,
            CommandDecision::PreparedRun => plan.prepared_run(&kind)?,
            CommandDecision::SecretRecord => plan.secret_record()?,
            CommandDecision::CreatedThreadRecord => plan.created_thread_record()?,
            CommandDecision::Fork => plan.fork()?,
            CommandDecision::MergeBack => plan.merge_back()?,
            CommandDecision::AdoptFolder => plan.adopt_folder()?,
            CommandDecision::RemoteStop => plan.remote_stop()?,
            CommandDecision::SubagentStop => plan.subagent_stop()?,
            CommandDecision::RemoteRequest => plan.remote_request()?,
            CommandDecision::ReopenChecks => plan.reopen_checks()?,
            CommandDecision::Message => plan.message()?,
            CommandDecision::Interrupt => plan.interrupt()?,
            CommandDecision::Stop => plan.stop()?,
            CommandDecision::BackgroundSettle => plan.background_settle()?,
            CommandDecision::CompletionResolution => plan.completion_resolution()?,
            CommandDecision::RemoteComplete => plan.remote_complete()?,
            CommandDecision::WakePolicy => plan.wake_policy()?,
            CommandDecision::NotificationAccept => plan.notification_accept()?,
            CommandDecision::Delete => plan.delete()?,
            CommandDecision::Rollback => plan.rollback()?,
            CommandDecision::LinkRecord => plan.link_record()?,
            CommandDecision::DelegatedRequest => plan.delegated_request()?,
            CommandDecision::Promote => plan.promote()?,
            CommandDecision::PullRequest => plan.pull_request()?,
        }
        if kind == "thread.stop" {
            plan.cancel_idle_orphans()?;
        }
        if kind == "run.interrupt" && plan.command["holdQueue"] == true {
            let saved = plan.decision.clone();
            let projection = plan.projection.clone();
            let ids = plan.id_positions.clone();
            if plan.cancel_idle_orphans().is_err() {
                plan.decision = saved;
                plan.projection = projection;
                plan.id_positions = ids;
            }
            plan.effect(
                "delegated-tasks.stop",
                EffectRequest::StopDelegated {
                    reason: plan.command["reason"].as_str().map(str::to_owned),
                },
            );
        }
        Ok(plan.decision)
    }
}

pub fn required_records(
    command: &OrchestrationV2ServerCommand,
) -> Result<(Vec<RecordKind>, RecordFilter)> {
    let (_, kind, _) = command_identity(command)?;
    let value = stored_value(command)?;
    let mut filter = RecordFilter::default();
    let fields = match kind.as_str() {
        "delegated_task.remote.complete"
        | "delegated_task.wake-policy"
        | "notification.delivery.accept"
        | "run.interrupt"
        | "thread.stop"
        | "thread.background-work.settle"
        | "delegated_task.completion-delivery.acknowledge"
        | "delegated_task.completion-delivery.dispose" => vec![
            RecordKind::Runs,
            RecordKind::Attempts,
            RecordKind::Nodes,
            RecordKind::ProviderSessions,
            RecordKind::ProviderThreads,
            RecordKind::ProviderTurns,
            RecordKind::Messages,
            RecordKind::TurnItems,
            RecordKind::RuntimeRequests,
            RecordKind::Subagents,
            RecordKind::ContextTransfers,
        ],
        "checkpoint.rollback" => vec![
            RecordKind::ProviderThreads,
            RecordKind::Checkpoints,
            RecordKind::CheckpointScopes,
            RecordKind::Runs,
            RecordKind::ProviderTurns,
            RecordKind::Attempts,
        ],
        "thread.delete" => vec![
            RecordKind::Runs,
            RecordKind::Attempts,
            RecordKind::Nodes,
            RecordKind::RuntimeRequests,
            RecordKind::Subagents,
            RecordKind::ProviderSessions,
            RecordKind::Messages,
            RecordKind::ContextTransfers,
        ],
        "link_request.record" => vec![
            RecordKind::Runs,
            RecordKind::Nodes,
            RecordKind::TurnItems,
            RecordKind::Attempts,
            RecordKind::ProviderTurns,
            RecordKind::Messages,
            RecordKind::ProviderThreads,
            RecordKind::ProviderSessions,
            RecordKind::ContextTransfers,
        ],
        "delegated_task.request" | "queued-message.promote-to-steer" => vec![
            RecordKind::Runs,
            RecordKind::Nodes,
            RecordKind::Attempts,
            RecordKind::Messages,
            RecordKind::ProviderSessions,
            RecordKind::ProviderThreads,
            RecordKind::ProviderTurns,
            RecordKind::TurnItems,
            RecordKind::RuntimeRequests,
            RecordKind::Subagents,
            RecordKind::ContextTransfers,
        ],
        "message.dispatch" => vec![
            RecordKind::Runs,
            RecordKind::Attempts,
            RecordKind::Nodes,
            RecordKind::ProviderSessions,
            RecordKind::ProviderThreads,
            RecordKind::ProviderTurns,
            RecordKind::RuntimeRequests,
            RecordKind::Messages,
            RecordKind::TurnItems,
            RecordKind::ContextTransfers,
            RecordKind::CheckpointScopes,
        ],
        "prepared-run.release"
        | "prepared-run.progress"
        | "prepared-run.fail"
        | "prepared-run.retry" => vec![
            RecordKind::Runs,
            RecordKind::Attempts,
            RecordKind::Nodes,
            RecordKind::ProviderThreads,
            RecordKind::TurnItems,
        ],
        "runtime-request.respond"
        | "thread.user-input.dismiss"
        | "runtime-request.delivery.fail" => vec![
            RecordKind::RuntimeRequests,
            RecordKind::Nodes,
            RecordKind::ProviderSessions,
            RecordKind::TurnItems,
        ],
        "thread.interrupt.unresponsive" => vec![
            RecordKind::Runs,
            RecordKind::ProviderTurns,
            RecordKind::TurnItems,
        ],
        "thread.snooze" | "thread.settle" | "thread.auto-settle" => vec![
            RecordKind::Runs,
            RecordKind::RuntimeRequests,
            RecordKind::Messages,
            RecordKind::ProviderSessions,
            RecordKind::ProviderThreads,
            RecordKind::Subagents,
        ],
        "thread.mark-unread" => vec![RecordKind::Runs],
        "provider-session.detach" => vec![RecordKind::ProviderSessions],
        "queued-run.reorder" => vec![RecordKind::Runs, RecordKind::Messages],
        "queued-run.start" | "queue.resume" => {
            filter.turn_item_types = Some(vec!["error".into()]);
            vec![
                RecordKind::Runs,
                RecordKind::Messages,
                RecordKind::TurnItems,
                RecordKind::ProviderSessions,
            ]
        }
        "secret_request.record" | "thread.created.record" => {
            filter.turn_item_types = Some(if kind == "secret_request.record" {
                vec!["secret_request".into()]
            } else {
                Vec::new()
            });
            vec![
                RecordKind::Runs,
                RecordKind::Nodes,
                RecordKind::TurnItems,
                RecordKind::Attempts,
                RecordKind::ProviderTurns,
            ]
        }
        "thread.merge_back" => vec![RecordKind::Runs, RecordKind::ContextTransfers],
        "thread.project.adopt-folder" => vec![RecordKind::Runs, RecordKind::RuntimeRequests],
        "delegated_task.remote.stop" => vec![RecordKind::Subagents],
        "delegated_task.remote.request" => {
            filter.turn_item_types = Some(vec!["run_interrupt_request".into()]);
            vec![
                RecordKind::Runs,
                RecordKind::Subagents,
                RecordKind::Attempts,
                RecordKind::ProviderTurns,
                RecordKind::TurnItems,
            ]
        }
        "subagent.stop" => {
            filter.turn_item_types = Some(vec!["subagent".into()]);
            filter.omit_settled_leaf_nodes = true;
            vec![
                RecordKind::Runs,
                RecordKind::Subagents,
                RecordKind::ProviderThreads,
                RecordKind::ProviderTurns,
                RecordKind::Nodes,
                RecordKind::TurnItems,
            ]
        }
        "queued-run.edit" => {
            filter.turn_item_types = Some(vec!["user_message".into()]);
            let id: RunId = serde_json::from_value(value["runId"].clone())?;
            filter.run_ids = Some(vec![id.clone()]);
            filter.turn_item_run_id = Some(Some(id));
            vec![
                RecordKind::Runs,
                RecordKind::Messages,
                RecordKind::TurnItems,
            ]
        }
        "queued-run.cancel" => vec![
            RecordKind::Runs,
            RecordKind::Messages,
            RecordKind::Nodes,
            RecordKind::Attempts,
            RecordKind::Subagents,
        ],
        "thread.archive" => {
            filter.turn_item_types = Some(vec!["command_execution".into(), "link_request".into()]);
            vec![
                RecordKind::Runs,
                RecordKind::Attempts,
                RecordKind::Nodes,
                RecordKind::ProviderSessions,
                RecordKind::ProviderThreads,
                RecordKind::Subagents,
                RecordKind::Messages,
                RecordKind::TurnItems,
            ]
        }
        "thread.runtime-mode.set" => {
            vec![RecordKind::ProviderSessions, RecordKind::ProviderThreads]
        }
        "thread.model-selection.set" | "provider.switch" => vec![
            RecordKind::Runs,
            RecordKind::ProviderSessions,
            RecordKind::ProviderThreads,
            RecordKind::ProviderTurns,
        ],
        "thread.metadata.update" => {
            let mut fields = Vec::new();
            if value["expectedEmpty"] == true {
                fields.extend([RecordKind::Runs, RecordKind::Messages]);
            }
            if value.get("worktreePath").is_some() {
                fields.extend([RecordKind::ProviderSessions, RecordKind::ProviderThreads]);
            }
            if !value["limitRecovery"].is_null() {
                filter.turn_item_types = Some(vec!["error".into()]);
                fields.extend([
                    RecordKind::Runs,
                    RecordKind::RuntimeRequests,
                    RecordKind::TurnItems,
                ]);
            }
            fields
        }
        _ => Vec::new(),
    };
    if kind == "message.dispatch" {
        filter.turn_item_types = Some(vec![
            "user_message".into(),
            "error".into(),
            "run_interrupt_request".into(),
            "secret_request".into(),
        ]);
        filter.omit_settled_leaf_nodes = true;
        filter.omit_settled_subagents = true;
        filter.omit_settled_subagent_records = true;
        filter.context_transfer_statuses = Some(vec![ContextTransferStatus::Pending]);
    }
    if kind == "queued-message.promote-to-steer" {
        filter.turn_item_types = Some(vec!["user_message".into(), "run_interrupt_request".into()]);
        filter.omit_settled_leaf_nodes = true;
    }
    if kind == "queued-run.cancel" {
        filter.omit_settled_leaf_nodes = true;
    }
    if matches!(
        kind.as_str(),
        "run.interrupt" | "thread.stop" | "thread.background-work.settle"
    ) {
        filter.turn_item_types = Some(vec![
            "command_execution".into(),
            "dynamic_tool".into(),
            "subagent".into(),
            "assistant_message".into(),
            "reasoning".into(),
            "link_request".into(),
        ]);
        filter.turn_item_statuses = Some(vec![
            OrchestrationV2TurnItemStatus::Pending,
            OrchestrationV2TurnItemStatus::Running,
            OrchestrationV2TurnItemStatus::Waiting,
        ]);
        filter.omit_settled_leaf_nodes = true;
    }
    Ok((fields, filter))
}

fn prepared_message_context(inputs: Option<&DecisionInputs>) -> Result<&PreparedMessageContext> {
    inputs
        .and_then(|inputs| inputs.message_context.as_ref())
        .ok_or(Error::Unsupported(
            "message context requires the prepared fork/merge/provider handoff observation",
        ))
}

fn pending_restart_observations(
    inputs: Option<&DecisionInputs>,
) -> Option<&crate::ports::decisions::orchestrator::RestartContinuations> {
    inputs.map(|inputs| &inputs.restart_continuations)
}

// Local shim until DecisionInputs exposes the runtime's extension capabilities.
fn session_supports_selection(_inputs: Option<&DecisionInputs>, _session: &Value) -> Result<bool> {
    Err(Error::Unsupported(
        "live selection application requires the session extension observation",
    ))
}

// Local shim until DecisionInputs exposes the host registry's prepared PR identities.
fn prepared_pull_request_identities(
    _inputs: Option<&DecisionInputs>,
) -> Result<&crate::ports::decisions::orchestrator::PreparedPullRequestIdentities> {
    Err(Error::Unsupported(
        "pull-request decisions require prepared host source-control identities",
    ))
}

struct PreallocatedEventIds {
    ids: Vec<String>,
    cursor: std::sync::atomic::AtomicUsize,
}
impl IdGenerator for PreallocatedEventIds {
    fn next(&self, _: IdKind) -> String {
        self.ids
            .get(
                self.cursor
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            )
            .cloned()
            .unwrap_or_default()
    }
}

struct MessageTarget<'a> {
    run_id: &'a str,
    node_id: &'a str,
    provider_thread_id: &'a str,
    provider_turn_id: Value,
    scope: &'a Value,
    model_selection: &'a Value,
    handoff: bool,
}

struct Plan<'a> {
    context: &'a DecisionContext,
    inputs: Option<&'a DecisionInputs>,
    command: Value,
    projection: Value,
    thread_id: ThreadId,
    decision: Decision,
    id_positions: Vec<(IdKind, usize)>,
}

fn run_for_source_point<'a>(
    source: &'a OrchestrationV2ThreadProjection,
    point: &Value,
) -> Option<&'a OrchestrationV2Run> {
    match point["type"].as_str() {
        Some("run") => source
            .runs
            .iter()
            .find(|run| json!(run.id) == point["runId"]),
        Some("checkpoint") => source
            .checkpoints
            .iter()
            .find(|checkpoint| json!(checkpoint.id) == point["checkpointId"])
            .and_then(|checkpoint| checkpoint.run_id.as_ref())
            .and_then(|id| source.runs.iter().find(|run| &run.id == id)),
        Some("latest_stable") => source
            .runs
            .iter()
            .filter(|run| {
                run.status == OrchestrationV2RunStatus::Completed && run.checkpoint_id.is_some()
            })
            .max_by_key(|run| run.ordinal),
        _ => None,
    }
}

impl Plan<'_> {
    fn fail_queued(&mut self, id: &RunId, failure: Value) -> Result<()> {
        let Some(mut run) = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == json!(id) && run["status"] == "queued")
            .cloned()
        else {
            return Ok(());
        };
        let scope = Self::run_scope(&run);
        for item in self
            .records("turnItems")
            .to_vec()
            .into_iter()
            .filter(|item| item["type"] == "user_message" && item["runId"] == run["id"])
        {
            self.emit("turn-item.updated", item, Some(&scope))?;
        }
        if let Some(mut attempt) = self
            .records("attempts")
            .iter()
            .find(|attempt| attempt["id"] == run["activeAttemptId"])
            .cloned()
        {
            attempt["status"] = json!("failed");
            attempt["completedAt"] = json!(self.context.now);
            self.write_record("attempts", "run-attempt.updated", attempt, &scope)?;
        }
        let root = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == run["rootNodeId"])
            .cloned();
        if let Some(mut node) = root.clone() {
            node["status"] = json!("failed");
            node["completedAt"] = json!(self.context.now);
            self.write_record("nodes", "node.updated", node, &scope)?;
        }
        if let Some(root) = root
            && let Some(provider) = self
                .records("providerThreads")
                .iter()
                .find(|owner| owner["id"] == run["providerThreadId"])
                .cloned()
        {
            let driver = provider["driver"].as_str().ok_or(Error::Stale)?;
            let item_id = crate::ports::decisions::orchestrator::derived_id(
                "turn-item",
                &[
                    "provider",
                    driver,
                    "native-item",
                    &format!("queued-start-failure:{}", id.0),
                ],
            );
            self.emit("turn-item.updated",json!({"id":item_id,"threadId":self.thread_id,"runId":run["id"],"nodeId":root["id"],"providerThreadId":provider["id"],"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal(),"status":"failed","title":"Queued provider could not start","startedAt":self.context.now,"completedAt":self.context.now,"updatedAt":self.context.now,"type":"error","failure":failure}),Some(&scope))?;
        }
        run["status"] = json!("failed");
        run["queuePosition"] = Value::Null;
        run["completedAt"] = json!(self.context.now);
        self.write_record("runs", "run.updated", run, &scope)
    }

    fn after_run(&mut self, id: &RunId) -> Result<()> {
        let projection = self.context.projection.as_ref().ok_or(Error::Stale)?;
        let Some(run) = projection
            .runs
            .iter()
            .find(|run| stored_value(&run.id).ok() == Some(json!(id)))
        else {
            return Ok(());
        };
        if run.status==OrchestrationV2RunStatus::Failed && let Some(delay)=crate::ports::runs_ingestion::restart_background_note::continuation_start_retry_delay_ms(run,&projection.runs,&projection.provider_turns,&projection.turn_items)? {
            self.decision.effects.push(PendingEffect{id:format!("effect:restart-continuation:{}",id.0),command_id:CommandId(format!("command:restart-continuation-retry:{}",id.0)),thread_id:self.thread_id.clone(),request:EffectRequest::Continue{source_run_id:id.clone()},available_at:Some(iso_from_epoch_millis(epoch_millis(&self.context.now.0).ok_or(Error::Stale)?+delay as i64))});
        }
        self.finalize_delivery(id)?;
        if run.status != OrchestrationV2RunStatus::Completed
            || crate::ports::decisions::model_switch::latest_executed_run(&projection.runs)
                .is_none_or(|latest| latest.id != run.id)
            || self
                .records("turnItems")
                .iter()
                .any(|item| item["type"] == "run_interrupt_request" && item["runId"] == json!(id))
            || self.projection["thread"]["worktreePath"].is_null()
                && !stored_value(run)?["workspacePreparation"].is_null()
        {
            return Ok(());
        }
        for mut held in self
            .records("runs")
            .to_vec()
            .into_iter()
            .filter(|run| run["status"] == "queued" && run["queueHeld"] == true)
        {
            if self.records("messages").iter().any(|message| {
                message["id"] == held["userMessageId"]
                    && (message["createdBy"] != "user" || !message["scheduledTaskId"].is_null())
            }) {
                held["queueHeld"] = json!(false);
                let scope = Self::run_scope(&held);
                self.write_record("runs", "run.updated", held, &scope)?;
            }
        }
        Ok(())
    }

    fn finalize_delivery(&mut self, id: &RunId) -> Result<()> {
        let Some(run) = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == json!(id))
            .cloned()
        else {
            return Ok(());
        };
        let Some(message) = self
            .records("messages")
            .iter()
            .find(|message| {
                message["id"] == run["userMessageId"] && !message["delegatedCompletion"].is_null()
            })
            .cloned()
        else {
            return Ok(());
        };
        let ownership = &message["delegatedCompletion"];
        let Some(mut parent) = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == ownership["parentRunId"])
            .cloned()
        else {
            return Ok(());
        };
        let cohort = parent["delegatedCompletion"].clone();
        let delivery = &cohort["delivery"];
        if delivery.is_null()
            || delivery["messageId"] != message["id"]
            || delivery["generation"] != ownership["generation"]
        {
            return Ok(());
        }
        let mut changed = Vec::new();
        let ids = delivery["taskIds"].as_array().cloned().unwrap_or_default();
        for task in self.records("subagents").iter().filter(|task| {
            task["origin"] == "app_owned"
                && task["runId"] == parent["id"]
                && ids.contains(&task["id"])
                && task["completionDelivery"]["state"] == "claimed"
        }) {
            let mut task = task.clone();
            task["completionDelivery"] = json!({"state":if run["status"]=="cancelled"{"pending"}else{"delivered"},"observedByRunId":null});
            changed.push(task);
        }
        let pending: Vec<_> = self
            .records("subagents")
            .iter()
            .filter(|task| {
                task["origin"] == "app_owned"
                    && task["runId"] == parent["id"]
                    && matches!(
                        task["status"].as_str(),
                        Some("completed" | "failed" | "cancelled" | "interrupted")
                    )
                    && (task["completionDelivery"]["state"] == "pending"
                        || changed.iter().any(|changed| {
                            changed["id"] == task["id"]
                                && changed["completionDelivery"]["state"] == "pending"
                        }))
            })
            .map(|task| task["id"].clone())
            .collect();
        let reserve = cohort["disposition"] == "open"
            && self.projection["thread"]["archivedAt"].is_null()
            && self.projection["thread"]["deletedAt"].is_null()
            && !pending.is_empty();
        if reserve {
            let generation = cohort["nextGeneration"].as_u64().ok_or(Error::Stale)?;
            parent["delegatedCompletion"]["nextGeneration"] = json!(generation + 1);
            parent["delegatedCompletion"]["delivery"] = json!({"generation":generation,"messageId":self.allocate(IdKind::Message)?,"taskIds":pending});
            for id in &pending {
                if let Some(task) = changed.iter_mut().find(|task| task["id"] == *id) {
                    task["completionDelivery"]["state"] = json!("claimed");
                } else if let Some(mut task) = self
                    .records("subagents")
                    .iter()
                    .find(|task| task["id"] == *id)
                    .cloned()
                {
                    task["completionDelivery"] = json!({"state":"claimed","observedByRunId":null});
                    changed.push(task);
                }
            }
        } else {
            parent["delegatedCompletion"]["delivery"] = Value::Null;
        }
        for mut task in changed {
            task["updatedAt"] = json!(self.context.now);
            let scope = json!({"runId":parent["id"],"nodeId":task["id"],"driver":task["driver"],"providerInstanceId":task["providerInstanceId"]});
            self.write_record("subagents", "subagent.updated", task, &scope)?;
        }
        let scope = Self::run_scope(&parent);
        self.write_record("runs", "run.updated", parent, &scope)
    }

    fn child_result(&self, child: &Value, run: &Value) -> (String, Option<Value>) {
        let item = child["turnItems"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| {
                item["runId"] == run["id"]
                    && item["type"] == "assistant_message"
                    && item["text"]
                        .as_str()
                        .is_some_and(|text| !t3_contracts::wire::trim_js(text).is_empty())
            })
            .reduce(|latest, item| {
                if item["ordinal"].as_u64() > latest["ordinal"].as_u64() {
                    item
                } else {
                    latest
                }
            });
        let messages: Vec<_> = child["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|message| {
                message["runId"] == run["id"]
                    && message["role"] == "assistant"
                    && message["text"]
                        .as_str()
                        .is_some_and(|text| !t3_contracts::wire::trim_js(text).is_empty())
            })
            .collect();
        let message = messages
            .iter()
            .copied()
            .find(|message| item.is_some_and(|item| item["messageId"] == message["id"]))
            .or_else(|| {
                messages.into_iter().reduce(|latest, message| {
                    let compare = message["updatedAt"]
                        .as_str()
                        .and_then(epoch_millis)
                        .cmp(&latest["updatedAt"].as_str().and_then(epoch_millis))
                        .then_with(|| {
                            message["createdAt"]
                                .as_str()
                                .and_then(epoch_millis)
                                .cmp(&latest["createdAt"].as_str().and_then(epoch_millis))
                        });
                    if compare.is_gt() { message } else { latest }
                })
            });
        let failure = if run["status"] == "failed" {
            child["turnItems"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| item["runId"] == run["id"] && item["type"] == "error")
                .reduce(|latest, item| {
                    if item["ordinal"].as_u64() > latest["ordinal"].as_u64() {
                        item
                    } else {
                        latest
                    }
                })
        } else {
            None
        };
        let report = message
            .and_then(|message| message["text"].as_str())
            .or_else(|| item.and_then(|item| item["text"].as_str()));
        let mut text = if let Some(failure) = failure {
            let failure_text = failure["failure"]["message"].as_str().unwrap_or_default();
            if item.is_some_and(|item| item["ordinal"].as_u64() < failure["ordinal"].as_u64())
                && let Some(report) = report
            {
                format!(
                    "{failure_text}\n\nLast assistant message before the run failed:\n\n{report}"
                )
            } else {
                failure_text.into()
            }
        } else {
            report.map(str::to_owned).unwrap_or_else(|| {
                if run["status"] == "completed" {
                    "Child task completed without an assistant result.".into()
                } else {
                    format!(
                        "Child task ended with status {}.",
                        run["status"].as_str().unwrap_or_default()
                    )
                }
            })
        };
        if let Some(model) = self
            .provider_turn_for_run(child, run)
            .and_then(|turn| turn["model"].as_str().map(str::to_owned))
            && run["modelSelection"]["model"] != model
        {
            text = format!(
                "Requested model {}; the provider reported running {model}.\n\n{text}",
                run["modelSelection"]["model"].as_str().unwrap_or_default()
            );
        }
        (text, failure.or(item).map(|item| item["id"].clone()))
    }

    fn sync_child_tasks(
        &mut self,
        child: &OrchestrationV2ThreadProjection,
        id: &RunId,
    ) -> Result<()> {
        let value = stored_value(child)?;
        if value["thread"]["lineage"]["relationshipToParent"] != "subagent"
            || value["thread"]["lineage"]["parentThreadId"] != json!(self.thread_id)
            || value["thread"]["forkedFrom"]["type"] != "node"
        {
            return Ok(());
        }
        let Some(run) = value["runs"].as_array().and_then(|runs| {
            runs.iter()
                .find(|run| run["id"] == json!(id) && !run["delegatedTaskId"].is_null())
        }) else {
            return Ok(());
        };
        if !matches!(
            run["status"].as_str(),
            Some("queued" | "preparing" | "starting" | "running" | "waiting")
        ) {
            return Ok(());
        }
        let status = if run["status"] == "queued" {
            if run["queueHeld"] == true {
                "idle"
            } else {
                "pending"
            }
        } else if run["status"] == "waiting" {
            "waiting"
        } else {
            "running"
        };
        let Some(mut task) = self
            .records("subagents")
            .iter()
            .find(|task| {
                task["id"] == run["delegatedTaskId"]
                    && task["origin"] == "app_owned"
                    && task["childThreadId"] == value["thread"]["id"]
                    && task["result"].is_null()
                    && task["status"] != status
            })
            .cloned()
        else {
            return Ok(());
        };
        task["status"] = json!(status);
        task["completedAt"] = Value::Null;
        task["updatedAt"] = json!(self.context.now);
        let scope = json!({"runId":task["runId"],"nodeId":task["id"],"providerInstanceId":task["providerInstanceId"]});
        self.write_record("subagents", "subagent.updated", task.clone(), &scope)?;
        if let Some(mut node) = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == task["id"])
            .cloned()
        {
            node["status"] = json!(status);
            node["completedAt"] = Value::Null;
            self.write_record("nodes", "node.updated", node, &scope)?;
        }
        if let Some(mut item) = self
            .records("turnItems")
            .iter()
            .find(|item| item["type"] == "subagent" && item["subagentId"] == task["id"])
            .cloned()
        {
            item["status"] = json!(status);
            item["completedAt"] = Value::Null;
            item["updatedAt"] = json!(self.context.now);
            self.write_record("turnItems", "turn-item.updated", item, &scope)?;
        }
        Ok(())
    }

    fn finalize_child_tasks(
        &mut self,
        child: &OrchestrationV2ThreadProjection,
        settled_source: Option<&RunId>,
        pending_restarts: &[RunId],
    ) -> Result<()> {
        use crate::ports::{
            decisions::orchestrator::delegated_task_progress,
            runs_ingestion::restart_background_note::{delegated_task_result_run, run_outcome},
        };
        let child_value = stored_value(child)?;
        if child_value["thread"]["lineage"]["relationshipToParent"] != "subagent"
            || child_value["thread"]["forkedFrom"]["type"] != "node"
            || child_value["thread"]["lineage"]["parentThreadId"] != json!(self.thread_id)
        {
            return Ok(());
        }
        let progress = delegated_task_progress(child)?;
        for mut task in self
            .records("subagents")
            .to_vec()
            .into_iter()
            .filter(|task| {
                task["origin"] == "app_owned"
                    && task["childThreadId"] == child_value["thread"]["id"]
            })
        {
            let cancelled = task.get("cancelledRunId").is_some();
            let original = task["id"] == child_value["thread"]["forkedFrom"]["nodeId"];
            if original
                && if cancelled {
                    progress.state == "working"
                } else {
                    progress.state != "result_available"
                }
            {
                continue;
            }
            if !task["result"].is_null()
                && self.records("contextTransfers").iter().any(|transfer| {
                    transfer["type"] == "subagent_result"
                        && transfer["sourceThreadId"] == child_value["thread"]["id"]
                })
            {
                continue;
            }
            let task_id: NodeId = serde_json::from_value(task["id"].clone())?;
            let own_runs: Vec<_> = child
                .runs
                .iter()
                .filter(|run| {
                    stored_value(&run.delegated_task_id).ok().as_ref() == Some(&task["id"])
                })
                .cloned()
                .collect();
            let run = if own_runs.is_empty() {
                if original { progress.result_run } else { None }
            } else {
                delegated_task_result_run(&own_runs, &task_id)
            };
            let Some(run) = run else {
                continue;
            };
            let outcome = run_outcome(run);
            let status = match outcome {
                OrchestrationV2RunStatus::Completed => "completed",
                OrchestrationV2RunStatus::Failed => "failed",
                OrchestrationV2RunStatus::Interrupted => "interrupted",
                OrchestrationV2RunStatus::Cancelled | OrchestrationV2RunStatus::RolledBack => {
                    "cancelled"
                }
                _ => continue,
            };
            let source_runs = if own_runs.is_empty() {
                &child.runs
            } else {
                &own_runs
            };
            if source_runs.iter().any(|source| {
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&source.id)
                    .ok()
                    .is_some_and(|id| Some(&id) != settled_source && pending_restarts.contains(&id))
                    && (source.id == run.id
                        || crate::ports::decisions::model_switch::run_ran_after(source, run))
            }) {
                continue;
            }
            let mut run = stored_value(run)?;
            let task_cancelled = task["cancelledRunId"] == run["id"];
            let status = if task_cancelled && status == "completed" {
                "interrupted"
            } else {
                status
            };
            run["status"] = if task_cancelled && status == "interrupted" {
                json!("interrupted")
            } else {
                stored_value(outcome)?
            };
            if self.records("contextTransfers").iter().any(|transfer| {
                transfer["type"] == "subagent_result"
                    && transfer["sourceThreadId"] == child_value["thread"]["id"]
                    && transfer["targetThreadId"] == json!(self.thread_id)
                    && (transfer["sourcePoint"]["runId"] == run["id"]
                        || original && transfer["sourcePoint"].get("runId").is_none())
            }) {
                continue;
            }
            let (text, item_id) = self.child_result(&child_value, &run);
            task["providerThreadId"] = run["providerThreadId"].clone();
            task["status"] = json!(status);
            task["result"] = json!(text);
            task["completedAt"] = json!(self.context.now);
            task["updatedAt"] = json!(self.context.now);
            if task_cancelled {
                task["cancelledRunId"] = run["id"].clone();
                task["completionDelivery"] = json!({"state":"disposed","observedByRunId":null});
            }
            let first = self.decision.events.len();
            let task = self.plan_delivery(task)?;
            let scope = json!({"runId":task["runId"],"nodeId":task["id"],"driver":task["driver"]});
            self.write_record("subagents", "subagent.updated", task.clone(), &scope)?;
            let event = self.decision.events.pop().ok_or(Error::Stale)?;
            self.decision.events.insert(first, event);
            if let Some(mut node) = self
                .records("nodes")
                .iter()
                .find(|node| node["id"] == task["id"])
                .cloned()
            {
                node["status"] = json!(status);
                node["providerThreadId"] = run["providerThreadId"].clone();
                node["completedAt"] = json!(self.context.now);
                self.write_record("nodes", "node.updated", node, &scope)?;
            }
            if let Some(mut item) = self
                .records("turnItems")
                .iter()
                .find(|item| item["type"] == "subagent" && item["subagentId"] == task["id"])
                .cloned()
            {
                item["status"] = json!(status);
                item["result"] = json!(text);
                item["completedAt"] = json!(self.context.now);
                item["updatedAt"] = json!(self.context.now);
                self.write_record("turnItems", "turn-item.updated", item, &scope)?;
            }
            let parent = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == task["runId"]);
            let mut point = json!({"threadId":child_value["thread"]["id"],"runId":run["id"]});
            if let Some(item_id) = item_id {
                point["turnItemId"] = item_id;
            }
            if !run["checkpointId"].is_null() {
                point["checkpointId"] = run["checkpointId"].clone();
            }
            if let Some(provider) =
                child_value["providerThreads"]
                    .as_array()
                    .and_then(|providers| {
                        providers
                            .iter()
                            .find(|provider| provider["id"] == run["providerThreadId"])
                    })
                && !provider["nativeThreadRef"].is_null()
            {
                point["providerThreadRef"] = provider["nativeThreadRef"].clone();
            }
            if let Some(turn) = self.provider_turn_for_run(&child_value, &run)
                && !turn["nativeTurnRef"].is_null()
            {
                point["providerTurnRef"] = turn["nativeTurnRef"].clone();
            }
            let target_instance = parent.map_or(
                self.projection["thread"]["providerInstanceId"].clone(),
                |run| run["providerInstanceId"].clone(),
            );
            let target_run = parent.map_or(Value::Null, |run| run["id"].clone());
            let transfer_id = self.allocate(IdKind::ContextTransfer)?;
            self.write_record("contextTransfers","context-transfer.created",json!({"id":transfer_id,"type":"subagent_result","sourceThreadId":child_value["thread"]["id"],"targetThreadId":self.thread_id,"sourcePoint":point,"basePoint":null,"sourceProviderInstanceId":run["providerInstanceId"],"targetProviderInstanceId":target_instance,"targetRunId":target_run,"status":"consumed","resolution":null,"createdBy":"system","error":null,"createdAt":self.context.now,"updatedAt":self.context.now,"consumedAt":self.context.now}),&json!({"runId":target_run,"providerInstanceId":run["providerInstanceId"]}))?;
        }
        Ok(())
    }

    fn emit_message_context(
        &mut self,
        context: &PreparedMessageContext,
        run: &Value,
        mut provider: Value,
    ) -> Result<Value> {
        let scope = Self::run_scope(run);
        if let Some(handoff) = &context.handoff {
            let value = stored_value(handoff)?;
            if value["threadId"] != json!(self.thread_id)
                || value["targetRunId"] != run["id"]
                || value["toProviderThreadId"] != provider["id"]
            {
                return Err(Error::Service{operation:"DecisionEngine.message-context",detail:"Prepared handoff does not belong to the run and provider thread being started.".into()});
            }
            let mut ids = provider["handoffIds"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !ids.contains(&value["id"]) {
                ids.push(value["id"].clone());
            }
            provider["handoffIds"] = json!(ids);
            self.emit("context-handoff.updated", value, Some(&scope))?;
        }
        for transfer in &context.transfers {
            let value = stored_value(transfer)?;
            let exists = self
                .records("contextTransfers")
                .iter()
                .any(|transfer| transfer["id"] == value["id"]);
            self.emit(
                if exists {
                    "context-transfer.updated"
                } else {
                    "context-transfer.created"
                },
                value,
                Some(&scope),
            )?;
        }
        for session in &context.released_sessions {
            let session = stored_value(session)?;
            if self.session_present(&session["id"])?
                && self.records("providerThreads").iter().any(|provider| {
                    provider["providerSessionId"] == session["id"]
                        && self.inputs.is_some_and(|inputs| {
                            inputs
                                .background_work
                                .iter()
                                .any(|(id, pending)| json!(id) == provider["id"] && *pending)
                        })
                })
            {
                return Err(rejection(
                    &self.command,
                    "The previous provider is still running background agents or commands, and switching would end them. Wait for them to finish, or press Stop, then send the message again.",
                ));
            }
            self.emit("provider-session.detached",json!({"providerSessionId":session["id"],"detachedAt":self.context.now,"reason":"Provider or model selection changed."}),Some(&session))?;
            self.effect(
                &format!(
                    "provider-session.detach:{}",
                    session["id"].as_str().unwrap_or_default()
                ),
                EffectRequest::Detach {
                    provider_session_id: serde_json::from_value(session["id"].clone())?,
                    detail: Some("Provider or model selection changed.".into()),
                    revoke_mcp_credential: None,
                },
            );
        }
        Ok(provider)
    }

    fn fresh_sessions(&self) -> Result<Vec<Value>> {
        let owner = self
            .records("providerThreads")
            .iter()
            .find(|owner| owner["id"] == self.projection["thread"]["activeProviderThreadId"]);
        let session_id = owner
            .map(|owner| owner["providerSessionId"].clone())
            .unwrap_or(Value::Null);
        let mut released = Vec::new();
        for session in self.records("providerSessions").iter().filter(|session| {
            session["id"] == session_id
                && !matches!(session["status"].as_str(), Some("stopped" | "error"))
                && session["capabilities"]["sessions"]["supportsMultipleProviderThreadsPerSession"]
                    != true
        }) {
            if self.session_present(&session["id"])?
                && self
                    .records("providerThreads")
                    .iter()
                    .filter(|owner| owner["providerSessionId"] == session["id"])
                    .any(|owner| {
                        self.inputs.is_some_and(|inputs| {
                            inputs
                                .background_work
                                .iter()
                                .any(|(id, pending)| json!(id) == owner["id"] && *pending)
                        })
                    })
            {
                if self.command.get("scheduledTaskId").is_some() {
                    continue;
                }
                return Err(rejection(
                    &self.command,
                    "The current session is still running background agents or commands, and a fresh session would end them. Wait for them to finish, or press Stop, then send the message again.",
                ));
            }
            released.push(session.clone());
        }
        Ok(released)
    }

    fn detach_fresh_sessions(&mut self, sessions: Vec<Value>) -> Result<()> {
        for session in sessions {
            self.emit("provider-session.detached",json!({"providerSessionId":session["id"],"detachedAt":self.context.now,"reason":"Started a fresh session."}),Some(&session))?;
            self.effect(
                &format!(
                    "provider-session.detach:{}",
                    session["id"].as_str().unwrap_or_default()
                ),
                EffectRequest::Detach {
                    provider_session_id: serde_json::from_value(session["id"].clone())?,
                    detail: Some("Started a fresh session.".into()),
                    revoke_mcp_credential: None,
                },
            );
        }
        Ok(())
    }

    fn queue_start(&mut self) -> Result<()> {
        let mut thread = self.thread()?;
        if !thread["archivedAt"].is_null() || self.records("runs").iter().any(Self::active_run) {
            return Ok(());
        }
        let Some(mut run) = self
            .records("runs")
            .iter()
            .find(|run| {
                run["id"] == self.command["runId"]
                    && run["status"] == "queued"
                    && run["queueHeld"] != true
            })
            .cloned()
        else {
            return Ok(());
        };
        let mut message = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == run["userMessageId"])
            .cloned()
            .ok_or(Error::Unsupported("queued start requires its user message"))?;
        if message["notification"]["source"]["kind"] == "monitor"
            && !message["notification"]["source"]["pullRequest"].is_null()
        {
            use crate::ports::decisions::orchestrator::{
                PullRequestWatchWakeState, current_pull_request_watch_wake,
            };
            match current_pull_request_watch_wake(
                &serde_json::from_value(self.projection.clone())?,
                &serde_json::from_value(message.clone())?,
                prepared_pull_request_identities(self.inputs)?,
            )? {
                PullRequestWatchWakeState::Stale => {
                    self.cancel_queued_run(&run)?;
                    return Ok(());
                }
                PullRequestWatchWakeState::Updated(mut updated) => {
                    updated["updatedAt"] = json!(self.context.now);
                    let scope = Self::run_scope(&run);
                    self.write_record("messages", "message.updated", updated.clone(), &scope)?;
                    message = updated;
                }
                PullRequestWatchWakeState::Unchanged => {}
            }
        }
        let mut root = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == run["rootNodeId"])
            .cloned()
            .ok_or(Error::Unsupported("queued start requires its root node"))?;
        let mut attempt = self
            .records("attempts")
            .iter()
            .find(|attempt| attempt["id"] == run["activeAttemptId"])
            .cloned()
            .ok_or(Error::Unsupported(
                "queued start requires its pending attempt",
            ))?;
        let mut owner = self
            .records("providerThreads")
            .iter()
            .find(|owner| owner["id"] == run["providerThreadId"])
            .cloned()
            .ok_or(Error::Unsupported(
                "queued start requires its provider thread",
            ))?;
        let instance: ProviderInstanceId =
            serde_json::from_value(run["providerInstanceId"].clone())?;
        let metadata = self
            .inputs
            .and_then(|inputs| {
                inputs
                    .provider_metadata
                    .iter()
                    .find(|(id, _)| id == &instance)
                    .map(|(_, metadata)| metadata)
            })
            .ok_or(Error::Unsupported("queued start requires adapter metadata"))?;
        let automatic = !self.command["endedRunId"].is_null();
        if self.usage_limit_holds_queue(
            &run["providerInstanceId"],
            if automatic {
                Some(&self.context.now)
            } else {
                None
            },
        )? {
            return Ok(());
        }
        let latest = self
            .context
            .projection
            .as_ref()
            .and_then(|projection| {
                crate::ports::decisions::model_switch::latest_executed_run(&projection.runs)
            })
            .map(stored_value)
            .transpose()?;
        let failed = automatic
            && latest.as_ref().is_some_and(|latest| {
                latest["id"] == self.command["endedRunId"]
                    && latest["status"] == "failed"
                    && latest["providerInstanceId"] == run["providerInstanceId"]
                    && self
                        .records("turnItems")
                        .iter()
                        .filter(|item| {
                            item["type"] == "error"
                                && item["runId"] == latest["id"]
                                && item["nodeId"] == latest["rootNodeId"]
                        })
                        .max_by_key(|item| item["ordinal"].as_u64())
                        .is_some_and(|item| {
                            !item["failure"]["class"].is_null()
                                && item["failure"]["class"] != "validation_error"
                        })
            });
        if failed {
            for mut queued in self
                .records("runs")
                .to_vec()
                .into_iter()
                .filter(|run| run["status"] == "queued")
            {
                queued["queueHeld"] = json!(true);
                let scope = Self::run_scope(&queued);
                self.write_record("runs", "run.updated", queued, &scope)?;
            }
            return Ok(());
        }
        if !metadata.enabled
            || self.inputs.is_some_and(|inputs| {
                inputs
                    .provider_enabled
                    .iter()
                    .any(|(id, enabled)| id == &instance && !enabled)
            })
            || thread["worktreePath"].is_null()
                && self.records("runs").iter().any(|run| {
                    matches!(run["status"].as_str(), Some("failed" | "interrupted"))
                        && run["workspacePreparation"]["type"] == "worktree"
                })
        {
            for mut held in self
                .records("runs")
                .to_vec()
                .into_iter()
                .filter(|run| run["status"] == "queued")
            {
                held["queueHeld"] = json!(true);
                let scope = Self::run_scope(&held);
                self.write_record("runs", "run.updated", held, &scope)?;
            }
            return Ok(());
        }
        let needs_context = thread["providerInstanceId"] != run["providerInstanceId"]
            && self
                .records("providerThreads")
                .iter()
                .any(|provider| !provider["nativeThreadRef"].is_null())
            || self.records("contextTransfers").iter().any(|transfer| {
                transfer["status"] == "pending"
                    && matches!(transfer["type"].as_str(), Some("fork" | "merge_back"))
            });
        let prepared_context = if needs_context {
            Some(prepared_message_context(self.inputs)?.clone())
        } else {
            None
        };
        if let Some(context) = &prepared_context {
            owner = stored_value(&context.provider_thread)?;
            run["providerThreadId"] = owner["id"].clone();
            if let Some(handoff) = &context.handoff {
                run["contextHandoffId"] = stored_value(&handoff.id)?;
            }
        }
        let sessions_to_detach = if run["freshContext"] == true {
            self.fresh_sessions()?
        } else {
            Vec::new()
        };
        let newer: Vec<_> = self
            .records("runs")
            .iter()
            .filter(|candidate| {
                candidate["status"] != "queued"
                    && candidate["ordinal"].as_u64() > run["ordinal"].as_u64()
            })
            .map(|candidate| candidate["id"].clone())
            .collect();
        if self.records("turnItems").iter().any(|item| {
            newer.contains(&item["runId"])
                && matches!(item["type"].as_str(), Some("user_message" | "notification"))
        }) {
            run["ordinal"] = json!(
                self.records("runs")
                    .iter()
                    .filter_map(|run| run["ordinal"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1
            );
        }
        if owner["providerSessionId"].is_null() {
            owner["providerSessionId"] = json!(if metadata
                .capabilities
                .sessions
                .supports_multiple_provider_threads_per_session
            {
                format!("provider-session:provider-instance:{}:shared", instance.0)
            } else {
                self.allocate(IdKind::ProviderSession)?
            });
        }
        owner["status"] = json!("not_loaded");
        if owner["firstRunOrdinal"].is_null() {
            owner["firstRunOrdinal"] = run["ordinal"].clone();
        }
        owner["lastRunOrdinal"] = run["ordinal"].clone();
        owner["updatedAt"] = json!(self.context.now);
        let scope_record = self
            .records("checkpointScopes")
            .iter()
            .find(|scope| scope["id"] == root["checkpointScopeId"])
            .cloned();
        let checkpoint = if let Some(scope) = scope_record {
            scope
        } else {
            self.root_scope(&run, &root, &owner)?
        };
        root["checkpointScopeId"] = checkpoint["id"].clone();
        root["providerThreadId"] = owner["id"].clone();
        attempt["providerInstanceId"] = run["providerInstanceId"].clone();
        attempt["providerThreadId"] = owner["id"].clone();
        if thread["modelSelection"] != run["modelSelection"] {
            let changed = thread["providerInstanceId"] != run["providerInstanceId"];
            thread["modelSelection"] = run["modelSelection"].clone();
            thread["providerInstanceId"] = run["providerInstanceId"].clone();
            thread["updatedAt"] = json!(self.context.now);
            self.update_thread(
                if changed {
                    "thread.provider-switched"
                } else {
                    "thread.model-selection-updated"
                },
                thread,
            )?;
        }
        let mut wake = message.clone();
        if let Some(restart) = run.get("restartContinuationOfRunId") {
            wake["restartContinuationOfRunId"] = restart.clone();
        }
        let stopped = self.records("turnItems").iter().any(|item| {
            item["type"] == "run_interrupt_request" && item["runId"] == self.command["endedRunId"]
        });
        if let Some(at) = crate::ports::decisions::orchestrator::wake_work_started_at(
            self.context.projection.as_ref().ok_or(Error::Stale)?,
            &wake,
            if automatic && !stopped {
                "automatic-queue"
            } else {
                "explicit-queue"
            },
        )? {
            run["workStartedAt"] = json!(at);
        }
        run["status"] = json!("starting");
        run["queuePosition"] = Value::Null;
        run["startedAt"] = Value::Null;
        let scope = Self::run_scope(&run);
        if let Some(context) = &prepared_context {
            owner = self.emit_message_context(context, &run, owner)?;
        }
        self.write_record(
            "providerThreads",
            "provider-thread.updated",
            owner.clone(),
            &scope,
        )?;
        self.emit("checkpoint-scope.created", checkpoint, Some(&scope))?;
        self.write_record("nodes", "node.updated", root, &scope)?;
        self.write_record("attempts", "run-attempt.updated", attempt, &scope)?;
        self.write_record("runs", "run.updated", run.clone(), &scope)?;
        let saved = self.command.clone();
        self.command = message;
        self.command["messageId"] = self.command["id"].clone();
        self.command["commandId"] = saved["commandId"].clone();
        let message_event = self.decision.events.len();
        self.append_message(
            MessageTarget {
                run_id: run["id"].as_str().ok_or(Error::Stale)?,
                node_id: run["rootNodeId"].as_str().ok_or(Error::Stale)?,
                provider_thread_id: owner["id"].as_str().ok_or(Error::Stale)?,
                provider_turn_id: Value::Null,
                scope: &scope,
                model_selection: &run["modelSelection"],
                handoff: !run["contextHandoffId"].is_null(),
            },
            "queued_turn",
            true,
        )?;
        self.decision.events.remove(message_event);
        if let Some(event) = self.decision.events.last_mut() {
            let mut value = stored_value(&*event)?;
            value["payload"]["ordinal"] = json!(run["ordinal"].as_u64().ok_or(Error::Stale)? * 100);
            *event = serde_json::from_value(value)?;
        }
        self.command = saved;
        self.detach_fresh_sessions(sessions_to_detach)?;
        self.effect(
            &format!(
                "provider-turn.start:{}",
                run["id"].as_str().unwrap_or_default()
            ),
            EffectRequest::Start {
                run_id: serde_json::from_value(run["id"].clone())?,
            },
        );
        Ok(())
    }

    fn delegated_request(&mut self) -> Result<()> {
        let original = self.command.clone();
        let command_id = self.command["commandId"].as_str().ok_or(Error::Stale)?;
        let child_id = crate::ports::decisions::orchestrator::derived_id(
            "thread",
            &["delegated-task", command_id],
        );
        let node_id = crate::ports::decisions::orchestrator::derived_id(
            "node",
            &["delegated-task", command_id],
        );
        let message_id = crate::ports::decisions::orchestrator::derived_id(
            "message",
            &["delegated-task", command_id],
        );
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["parentRunId"] && Self::active_run(run))
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Parent run {} is not active.",
                        self.command["parentRunId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        if !self
            .records("nodes")
            .iter()
            .any(|node| node["id"] == self.command["parentNodeId"] && node["runId"] == run["id"])
            || run["rootNodeId"].is_null()
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Parent node {} is not part of active run {}.",
                    self.command["parentNodeId"].as_str().unwrap_or_default(),
                    run["id"].as_str().unwrap_or_default()
                ),
            ));
        }
        let instance: ProviderInstanceId =
            serde_json::from_value(self.command["modelSelection"]["instanceId"].clone())?;
        let metadata = self
            .inputs
            .and_then(|inputs| {
                inputs
                    .provider_metadata
                    .iter()
                    .find(|(id, _)| *id == instance)
                    .map(|(_, metadata)| metadata)
            })
            .ok_or(Error::Unsupported(
                "delegation requires target adapter metadata",
            ))?;
        let mut child = self.thread()?;
        child["id"] = json!(child_id);
        child["providerInstanceId"] = json!(instance);
        child["modelSelection"] = self.command["modelSelection"].clone();
        child["createdBy"] = self.command["createdBy"].clone();
        child["creationSource"] = self.command["creationSource"].clone();
        child["runtimeMode"] = self.command["runtimeMode"].clone();
        child["interactionMode"] = self.command["interactionMode"].clone();
        child["lineage"] = json!({"parentThreadId":self.thread_id,"relationshipToParent":"subagent","rootThreadId":self.projection["thread"]["lineage"]["rootThreadId"]});
        child["forkedFrom"] = json!({"type":"node","nodeId":node_id});
        child["createdAt"] = self
            .command
            .get("createdAt")
            .cloned()
            .unwrap_or(json!(self.context.now));
        child["updatedAt"] = child["createdAt"].clone();
        child["pullRequests"] = json!([]);
        for field in [
            "titleRegeneration",
            "linkedPullRequest",
            "activeProviderThreadId",
            "archivedAt",
            "settledOverride",
            "settledAt",
            "snoozedUntil",
            "snoozedAt",
            "pinnedAt",
            "pinOrderKey",
            "autoSettleDisabledAt",
            "guestShare",
            "lastVisitedAt",
            "deletedAt",
        ] {
            child[field] = Value::Null;
        }
        child
            .as_object_mut()
            .ok_or(Error::Stale)?
            .shift_remove("historyOrigin");
        if self.command.get("workspaceStrategy").is_some() {
            child["branch"] = Value::Null;
            child["worktreePath"] = Value::Null;
        }
        self.command["driver"] = json!(metadata.driver);
        self.command["childThreadId"] = json!(child_id);
        let start = self.decision.events.len();
        self.remote_request()?;
        let task_item = self
            .decision
            .events
            .last()
            .map(stored_value)
            .transpose()?
            .ok_or(Error::Stale)?;
        child["title"] = task_item["payload"]["title"].clone();
        let child_projection =
            crate::runtime::dispatch::projection_from_records(ProjectionRecords {
                thread: serde_json::from_value(child.clone())?,
                records: Vec::new(),
            });
        let child_context = DecisionContext {
            projection: Some(child_projection),
            now: self.context.now.clone(),
            ids: self.context.ids.clone(),
            capabilities: Some(metadata.capabilities.clone()),
        };
        let mut child_plan = Plan {
            context: &child_context,
            inputs: self.inputs,
            command: json!({"type":"message.dispatch","commandId":original["commandId"],"threadId":child_id,"senderThreadId":self.thread_id,"messageId":message_id,"text":original["task"],"attachments":[],"modelSelection":original["modelSelection"],"dispatchMode":if original.get("workspaceStrategy").is_some(){json!({"type":"defer_start","workspaceStrategy":original["workspaceStrategy"]})}else{json!({"type":"start_immediately"})},"createdBy":original["createdBy"],"creationSource":original["creationSource"]}),
            projection: stored_value(child_context.projection.as_ref().ok_or(Error::Stale)?)?,
            thread_id: ThreadId(child_id.clone()),
            decision: Decision::default(),
            id_positions: self.id_positions.clone(),
        };
        let scope = json!({"driver":metadata.driver,"providerInstanceId":instance});
        child_plan.emit("thread.created", child, Some(&scope))?;
        let created = child_plan.decision.events.remove(0);
        self.decision.events.insert(start, created);
        child_plan.message_inner()?;
        let child_run = child_plan
            .decision
            .events
            .iter()
            .map(stored_value)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .find(|event| event["type"] == "run.created")
            .map(|event| event["payload"].clone())
            .ok_or(Error::Unsupported(
                "delegated child message did not create a run",
            ))?;
        let transfer = child_plan.allocate(IdKind::ContextTransfer)?;
        child_plan.emit("context-transfer.created",json!({"id":transfer,"type":"subagent_spawn","sourceThreadId":self.thread_id,"targetThreadId":child_id,"sourcePoint":{"threadId":self.thread_id,"runId":run["id"],"turnItemId":task_item["payload"]["id"]},"basePoint":null,"sourceProviderInstanceId":run["providerInstanceId"],"targetProviderInstanceId":instance,"targetRunId":child_run["id"],"status":"consumed","resolution":null,"createdBy":original["createdBy"],"error":null,"createdAt":self.context.now,"updatedAt":self.context.now,"consumedAt":self.context.now}),Some(&json!({"runId":child_run["id"],"providerInstanceId":instance})))?;
        self.id_positions = child_plan.id_positions;
        self.decision.events.extend(child_plan.decision.events);
        self.decision.effects.extend(child_plan.decision.effects);
        self.command = original;
        Ok(())
    }

    fn promote(&mut self) -> Result<()> {
        self.thread()?;
        let queued = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["queuedRunId"] && run["status"] == "queued")
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Queued run {} is not queued.",
                        self.command["queuedRunId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        let message = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == queued["userMessageId"])
            .cloned();
        if message.is_none()
            || !self
                .records("nodes")
                .iter()
                .any(|node| node["id"] == queued["rootNodeId"])
            || !self
                .records("attempts")
                .iter()
                .any(|attempt| attempt["id"] == queued["activeAttemptId"])
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Queued run {} is missing message or execution state.",
                    queued["id"].as_str().unwrap_or_default()
                ),
            ));
        }
        let message = message.ok_or(Error::Stale)?;
        if message.get("delegatedCompletion").is_some() || message.get("notification").is_some() {
            return Err(rejection(
                &self.command,
                "Automatic completion deliveries cannot be promoted to Steer.",
            ));
        }
        let goal = self.native_goal(&message, &queued["providerInstanceId"])?;
        let target = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["targetRunId"])
            .cloned();
        let target_message = target.as_ref().and_then(|target| {
            self.records("messages")
                .iter()
                .find(|message| message["id"] == target["userMessageId"])
                .cloned()
        });
        let compact = target_message.as_ref().is_some_and(|message| {
            message["role"] == "user"
                && t3_contracts::wire::trim_js(message["text"].as_str().unwrap_or_default())
                    .eq_ignore_ascii_case("/compact")
                && message["attachments"].as_array().is_none_or(Vec::is_empty)
        });
        if !compact && queued["freshContext"] == true {
            return Err(rejection(
                &self.command,
                "A message that starts a fresh session can't steer the current turn.",
            ));
        }
        if compact || goal.is_some() {
            let mut updated = queued.clone();
            updated["queuePosition"] = json!(1);
            let scope = Self::run_scope(&queued);
            self.write_record("runs", "run.updated", updated, &scope)?;
            let stops = target.as_ref().is_some_and(|target| {
                let status = self
                    .records("providerThreads")
                    .iter()
                    .find(|owner| owner["id"] == target["providerThreadId"])
                    .and_then(|owner| owner["goal"]["status"].as_str());
                let maintenance = target_message.as_ref().is_some_and(|message| {
                    crate::ports::decisions::command_policy::is_native_maintenance_command(
                        message["text"].as_str().unwrap_or_default(),
                        message["attachments"].as_array().map_or(0, Vec::len),
                    )
                });
                matches!(
                    target["status"].as_str(),
                    Some("preparing" | "starting" | "running")
                ) && (compact
                    || crate::ports::decisions::command_policy::goal_command_stops_active_run(
                        goal.unwrap_or_default(),
                        status,
                    ) && !maintenance)
                    && !self.records("turnItems").iter().any(|item| {
                        item["type"] == "run_interrupt_request" && item["runId"] == target["id"]
                    })
            });
            return self.stop_queued_command(
                &queued["id"],
                target
                    .as_ref()
                    .filter(|_| stops)
                    .map(|target| &target["id"]),
                if compact {
                    "Skipped compaction to send a queued message"
                } else {
                    "Stopped to apply /goal"
                },
            );
        }
        if let Some(target) = &target
            && (matches!(target["status"].as_str(), Some("preparing" | "starting"))
                || target["status"] == "running"
                    && !self.records("providerTurns").iter().any(|turn| {
                        turn["runAttemptId"] == target["activeAttemptId"]
                            && turn["status"] == "running"
                    }))
            && !self.records("turnItems").iter().any(|item| {
                item["type"] == "run_interrupt_request" && item["runId"] == target["id"]
            })
        {
            self.validate_steering_message(&message)?;
            self.validate_steering_capability(&target["providerInstanceId"])?;
            let mut updated = queued.clone();
            updated["steerWhenReadyRunId"] = target["id"].clone();
            let scope = Self::run_scope(&queued);
            return self.write_record("runs", "run.updated", updated, &scope);
        }
        self.cancel_queued_run(&queued)?;
        let original = self.command.clone();
        let mut steer = message;
        steer["type"] = json!("queued-message.promote-to-steer");
        steer["commandId"] = original["commandId"].clone();
        steer["messageId"] = steer["id"].clone();
        self.command = steer;
        let result = self.steer_message(
            &json!({"type":"steer_active","targetRunId":original["targetRunId"]}),
            &queued["modelSelection"],
        );
        self.command = original;
        result
    }

    fn pull_request_watch_sync(
        &mut self,
        identities: &crate::ports::decisions::orchestrator::PreparedPullRequestIdentities,
    ) -> Result<()> {
        let thread = self.thread()?;
        let wake = self.command.get("wake").cloned();
        let mut entries = vec![self.command.clone()];
        if let Some(wake) = &wake
            && let Some(value) = wake.get("includesRemarks")
        {
            entries[0]["includesRemarks"] = value.clone();
        }
        for other in self.command["others"].as_array().into_iter().flatten() {
            let mut entry = other.clone();
            entry["type"] = json!("thread.pull-request-watch.sync");
            entry["threadId"] = json!(self.thread_id);
            entry["commandId"] = self.command["commandId"].clone();
            entries.push(entry);
        }
        let inactive = !thread["archivedAt"].is_null()
            || thread["settledOverride"] == "settled"
            || !thread["settledAt"].is_null()
            || thread["lineage"]["relationshipToParent"] == "subagent"
                && thread["creationSource"] == "provider";
        let mut stamps = Vec::new();
        let mut outdated = Vec::new();
        for entry in &entries {
            let key = identities.key(entry)?;
            let link = thread["pullRequests"].as_array().and_then(|links| {
                links.iter().find(|link| {
                    link["source"] != "stack-dismissed"
                        && identities.key(link).ok().as_ref() == Some(&key)
                })
            });
            if link.is_none_or(|link| link["watch"]["startedAt"] != entry["startedAt"])
                || wake.is_some() && inactive
            {
                return Err(rejection(
                    &self.command,
                    "The pull request watch ended or its thread settled while it was read.",
                ));
            }
            let snapshot = link.map(|link| &link["snapshot"]);
            let ends = snapshot.is_some_and(|snapshot| {
                snapshot["state"] == "merged"
                    || snapshot["state"] == "closed"
                        && snapshot["syncedAt"]
                            .as_str()
                            .and_then(epoch_millis)
                            .zip(entry["startedAt"].as_str().and_then(epoch_millis))
                            .is_some_and(|(synced, started)| synced >= started)
            });
            outdated.push(
                wake.as_ref()
                    .is_some_and(|wake| wake["notification"]["source"]["kind"] == "monitor")
                    && !entry["watch"].is_null()
                    && ends,
            );
            let mut stamp = key;
            stamp["watchStartedAt"] = entry["startedAt"].clone();
            stamp["headSha"] = if entry["includesRemarks"] == true {
                Value::Null
            } else {
                entry["watch"]["headSha"].clone()
            };
            if entry["watch"].is_null() {
                stamp["watchEnded"] = json!(true);
            }
            if entries.len() > 1
                && let Some(section) = wake
                    .as_ref()
                    .and_then(|wake| wake["sections"].as_array())
                    .and_then(|sections| sections.get(stamps.len()))
            {
                stamp["section"] = section.clone();
            }
            stamps.push(stamp);
        }
        if entries.len() > 1 && outdated.iter().any(|outdated| *outdated) {
            return Err(rejection(
                &self.command,
                "The pull request watch ended or its thread settled while it was read.",
            ));
        }
        for entry in entries {
            let current = self.projection["thread"].clone();
            let updated = crate::ports::decisions::orchestrator::pull_request_mutation(
                current.clone(),
                &entry,
                &self.context.now,
                identities,
            )?;
            if updated != current {
                let key = identities.key(&entry)?;
                if let Some(link) = updated["pullRequests"]
                    .as_array()
                    .and_then(|links| {
                        links
                            .iter()
                            .find(|link| identities.key(link).ok().as_ref() == Some(&key))
                    })
                    .cloned()
                {
                    self.emit("thread.pull-request-link-updated", link, None)?;
                }
                self.projection["thread"] = updated;
            }
        }
        if let Some(mut wake) = wake
            && !outdated.first().copied().unwrap_or(false)
        {
            if wake["notification"]["source"]["kind"] == "monitor" {
                wake["notification"]["source"] = json!({"kind":"monitor","pullRequest":stamps[0]});
                if stamps.len() > 1 {
                    wake["notification"]["source"]["otherPullRequests"] = json!(&stamps[1..]);
                }
            }
            let original = self.command.clone();
            self.command = json!({"type":"message.dispatch","commandId":original["commandId"],"threadId":self.thread_id,"messageId":wake["messageId"],"text":wake["text"],"notification":wake["notification"],"attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"server"});
            let result = self.message();
            self.command = original;
            result?;
        }
        Ok(())
    }

    fn pull_request(&mut self) -> Result<()> {
        let identities = prepared_pull_request_identities(self.inputs)?;
        if self.command["type"] == "thread.pull-request-watch.sync" {
            return self.pull_request_watch_sync(identities);
        }

        let thread = self.thread()?;
        let updated = crate::ports::decisions::orchestrator::pull_request_mutation(
            thread.clone(),
            &self.command,
            &self.context.now,
            identities,
        )?;
        if matches!(
            self.command["type"].as_str(),
            Some("thread.pull-request-link.sync" | "thread.pull-request-watch.sync")
        ) {
            if updated == thread {
                return Ok(());
            }
            let key = identities.key(&self.command)?;
            let link = updated["pullRequests"]
                .as_array()
                .and_then(|links| {
                    links
                        .iter()
                        .find(|link| identities.key(link).ok().as_ref() == Some(&key))
                })
                .cloned();
            if let Some(link) = link {
                self.emit("thread.pull-request-link-updated", link, None)?;
                self.projection["thread"] = updated;
            }
            return Ok(());
        }
        self.update_thread("thread.pull-request-synced", updated)
    }

    fn delete(&mut self) -> Result<()> {
        if self.command["onlyIfUnused"] == true
            && (!self.records("runs").is_empty() || !self.records("messages").is_empty())
        {
            return Ok(());
        }
        let mut projections = vec![serde_json::from_value::<OrchestrationV2ThreadProjection>(
            self.projection.clone(),
        )?];
        if let Some(inputs) = self.inputs {
            projections.extend(
                inputs
                    .related_projections
                    .iter()
                    .filter(|projection| projection.thread.lineage.side_chat == Some(Some(true)))
                    .cloned(),
            );
        }
        for projection in projections {
            let thread_id =
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &projection.thread.id,
                )?;
            let attachments = self
                .inputs
                .and_then(|inputs| {
                    inputs
                        .attachment_ids
                        .iter()
                        .find(|(thread, _)| thread == &thread_id)
                        .map(|(_, ids)| ids)
                })
                .ok_or(Error::Unsupported(
                    "deletion requires the per-thread attachment inventory",
                ))?;
            let count = 1
                + projection.runs.len() * 2
                + projection.attempts.len()
                + projection.nodes.len()
                + projection.runtime_requests.len()
                + projection.subagents.len()
                + projection.provider_sessions.len();
            let mut ids = Vec::with_capacity(count);
            for _ in 0..count {
                ids.push(self.allocate(IdKind::Event)?);
            }
            let allocated = PreallocatedEventIds {
                ids,
                cursor: std::sync::atomic::AtomicUsize::new(0),
            };
            let mut command_id: CommandId =
                serde_json::from_value(self.command["commandId"].clone())?;
            if thread_id != self.thread_id {
                command_id.0 = format!("{}:side-chat:{}", command_id.0, thread_id.0);
            }
            let result = crate::ports::thread_project::thread_deletion::plan_thread_deletion(
                &command_id,
                &thread_id,
                &projection,
                attachments,
                &self.context.now,
                &allocated,
            )?;
            if allocated.cursor.load(std::sync::atomic::Ordering::Relaxed) > allocated.ids.len() {
                return Err(Error::Unsupported("deletion event preallocation exhausted"));
            }
            self.decision.events.extend(result.events);
            self.decision.effects.extend(result.effects);
            self.decision.cancellations.extend(result.cancellations);
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<()> {
        use crate::ports::decisions::command_policy::CapabilityCheck;
        let mut thread = self.thread()?;
        let owner = self
            .records("providerThreads")
            .iter()
            .find(|owner| owner["id"] == thread["activeProviderThreadId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    "No active provider thread exists for rollback.",
                )
            })?;
        if owner["providerSessionId"].is_null() {
            return Err(rejection(
                &self.command,
                format!(
                    "Provider thread {} has no provider session.",
                    owner["id"].as_str().unwrap_or_default()
                ),
            ));
        }
        let caps = self
            .context
            .capabilities
            .as_ref()
            .ok_or(Error::Unsupported(
                "rollback requires selected adapter capabilities",
            ))?;
        let command_id: CommandId = serde_json::from_value(self.command["commandId"].clone())?;
        let instance: ProviderInstanceId =
            serde_json::from_value(thread["modelSelection"]["instanceId"].clone())?;
        CapabilityCheck {
            command_id: &command_id,
            thread_id: &self.thread_id,
            provider_instance_id: &instance,
            capabilities: caps,
        }
        .ensure_rollback()
        .map_err(|error| rejection(&self.command, error.to_string()))?;
        let checkpoint = self
            .records("checkpoints")
            .iter()
            .find(|checkpoint| checkpoint["id"] == self.command["checkpointId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Checkpoint {} was not found.",
                        self.command["checkpointId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        if checkpoint["status"] != "ready"
            && !(self.command["restoreFiles"] == false && checkpoint["status"] == "missing")
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Checkpoint {} is {} and cannot be restored.",
                    checkpoint["id"].as_str().unwrap_or_default(),
                    checkpoint["status"].as_str().unwrap_or_default()
                ),
            ));
        }
        let scope = self
            .records("checkpointScopes")
            .iter()
            .find(|scope| scope["id"] == checkpoint["scopeId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Checkpoint scope {} was not found.",
                        checkpoint["scopeId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        if scope["id"] != self.command["scopeId"] {
            return Err(rejection(
                &self.command,
                format!(
                    "Checkpoint {} belongs to scope {}, not {}.",
                    checkpoint["id"].as_str().unwrap_or_default(),
                    scope["id"].as_str().unwrap_or_default(),
                    self.command["scopeId"].as_str().unwrap_or_default()
                ),
            ));
        }
        if self.command["restoreFiles"] != false
            && !self
                .inputs
                .and_then(|inputs| inputs.restore_isolated)
                .ok_or(Error::Unsupported(
                    "rollback requires checkpoint restore isolation observation",
                ))?
        {
            return Err(rejection(&self.command,crate::ports::checkpoints_forks::checkpoint_restore_safety::SHARED_WORKSPACE_RESTORE_MESSAGE));
        }
        let ordinal = checkpoint["appRunOrdinal"].as_u64().unwrap_or(0);
        if ordinal > 0 {
            let run = self
                .records("runs")
                .iter()
                .find(|run| run["ordinal"] == ordinal && run["status"] != "rolled_back");
            let turn = run.and_then(|run| self.provider_turn_for_run(&self.projection, run));
            let turn = turn.ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Cannot roll back to checkpoint {}: its provider turn is unavailable.",
                        checkpoint["id"].as_str().unwrap_or_default()
                    ),
                )
            })?;
            if turn["providerThreadId"] != owner["id"] {
                return Err(rejection(
                    &self.command,
                    format!(
                        "Cannot roll back provider thread {} to checkpoint {}: target provider turn {} belongs to provider thread {}.",
                        owner["id"].as_str().unwrap_or_default(),
                        checkpoint["id"].as_str().unwrap_or_default(),
                        turn["id"].as_str().unwrap_or_default(),
                        turn["providerThreadId"].as_str().unwrap_or_default()
                    ),
                ));
            }
        }
        thread["rollbackRequestId"] = self.command["commandId"].clone();
        thread["rollbackFailure"] = Value::Null;
        thread["updatedAt"] = json!(self.context.now);
        self.update_thread("thread.metadata-updated", thread)?;
        self.emit("checkpoint.rollback-requested",json!({"scopeId":scope["id"],"checkpointId":checkpoint["id"],"requestedAt":self.context.now}),None)?;
        self.effect(
            &format!(
                "provider-thread.rollback:{}:{}",
                owner["id"].as_str().unwrap_or_default(),
                checkpoint["id"].as_str().unwrap_or_default()
            ),
            EffectRequest::Rollback {
                restore_files: self.command["restoreFiles"].as_bool(),
                provider_thread_id: serde_json::from_value(owner["id"].clone())?,
                checkpoint_id: serde_json::from_value(checkpoint["id"].clone())?,
                scope_id: serde_json::from_value(scope["id"].clone())?,
            },
        );
        Ok(())
    }

    fn link_record(&mut self) -> Result<()> {
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["runId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Run {} was not found.",
                        self.command["runId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        let existing = self
            .records("turnItems")
            .iter()
            .find(|item| item["id"] == self.command["turnItemId"])
            .cloned();
        if let Some(item) = &existing {
            if item["type"] != "link_request" {
                return Err(rejection(
                    &self.command,
                    format!(
                        "Turn item {} is not a link request.",
                        self.command["turnItemId"].as_str().unwrap_or_default()
                    ),
                ));
            }
            if item["runId"] != self.command["runId"] {
                return Err(rejection(
                    &self.command,
                    format!(
                        "Link request {} belongs to another run.",
                        self.command["turnItemId"].as_str().unwrap_or_default()
                    ),
                ));
            }
        }
        if existing.is_none() {
            if run["rootNodeId"] != self.command["nodeId"] {
                return Err(rejection(
                    &self.command,
                    format!(
                        "Node {} is not the root of run {}.",
                        self.command["nodeId"].as_str().unwrap_or_default(),
                        run["id"].as_str().unwrap_or_default()
                    ),
                ));
            }
            if !matches!(
                run["status"].as_str(),
                Some("preparing" | "starting" | "running")
            ) || self
                .records("turnItems")
                .iter()
                .any(|item| item["runId"] == run["id"] && item["type"] == "run_interrupt_request")
            {
                return Err(rejection(
                    &self.command,
                    format!(
                        "Run {} is not active.",
                        run["id"].as_str().unwrap_or_default()
                    ),
                ));
            }
        }
        let unchanged = existing.as_ref().is_some_and(|item| {
            item["linkStatus"] != "pending" || self.command["linkStatus"] == "pending"
        });
        let pending = self.command["linkStatus"] == "pending";
        let mut item = if unchanged {
            existing.clone().ok_or(Error::Stale)?
        } else {
            let mut item = self.command.clone();
            let fields = item.as_object_mut().ok_or(Error::Stale)?;
            for name in ["commandId", "turnItemId", "wake"] {
                fields.shift_remove(name);
            }
            item["type"] = json!("link_request");
            item["id"] = self.command["turnItemId"].clone();
            item["providerThreadId"] = existing
                .as_ref()
                .map_or(run["providerThreadId"].clone(), |item| {
                    item["providerThreadId"].clone()
                });
            item["providerTurnId"] = existing.as_ref().map_or_else(
                || {
                    self.provider_turn_for_run(&self.projection, &run)
                        .map_or(Value::Null, |turn| turn["id"].clone())
                },
                |item| item["providerTurnId"].clone(),
            );
            item["nativeItemRef"] = Value::Null;
            item["parentItemId"] = Value::Null;
            item["ordinal"] = existing
                .as_ref()
                .map_or(json!(self.next_item_ordinal()), |item| {
                    item["ordinal"].clone()
                });
            item["status"] = json!(if pending {
                "waiting"
            } else if self.command["linkStatus"] == "linked" {
                "completed"
            } else if self.command["linkStatus"] == "failed" {
                "failed"
            } else {
                "cancelled"
            });
            item["title"] = self
                .command
                .get("label")
                .filter(|label| !label.is_null())
                .or_else(|| self.command.get("url").filter(|url| !url.is_null()))
                .cloned()
                .unwrap_or(json!("Link an environment"));
            item["startedAt"] = existing
                .as_ref()
                .map_or(json!(self.context.now), |item| item["startedAt"].clone());
            item["completedAt"] = if pending {
                Value::Null
            } else {
                json!(self.context.now)
            };
            item["updatedAt"] = json!(self.context.now);
            if let Some(existing) = &existing {
                item["nodeId"] = existing["nodeId"].clone();
            }
            item
        };
        let scope = json!({"runId":run["id"],"nodeId":item["nodeId"],"providerInstanceId":run["providerInstanceId"]});
        if !unchanged
            && self.command["linkStatus"] == "linked"
            && !self.projection["thread"]["snoozedUntil"].is_null()
        {
            let mut thread = self.thread()?;
            thread["snoozedUntil"] = Value::Null;
            thread["snoozedAt"] = Value::Null;
            thread["updatedAt"] = json!(self.context.now);
            self.update_thread("thread.unsnoozed", thread)?;
        }
        self.write_record("turnItems", "turn-item.updated", item.take(), &scope)?;
        if !unchanged
            && let Some(wake) = self.command.get("wake").cloned()
            && self.projection["thread"]["archivedAt"].is_null()
            && self.projection["thread"]["deletedAt"].is_null()
        {
            let saved = self.command.clone();
            self.command = json!({"type":"message.dispatch","commandId":saved["commandId"],"threadId":self.thread_id,"messageId":wake["messageId"],"text":wake["text"],"notification":wake["notification"],"attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"server"});
            let result = self.message();
            self.command = saved;
            result?;
        }
        Ok(())
    }

    fn write_record(
        &mut self,
        records: &str,
        kind: &str,
        payload: Value,
        scope: &Value,
    ) -> Result<()> {
        self.emit(kind, payload.clone(), Some(scope))?;
        let entries = self.projection[records]
            .as_array_mut()
            .ok_or(Error::Stale)?;
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry["id"] == payload["id"])
        {
            *entry = payload;
        } else {
            entries.push(payload);
        }
        Ok(())
    }

    fn session_present(&self, id: &Value) -> Result<bool> {
        if id.is_null() {
            return Ok(false);
        }
        self.inputs
            .and_then(|inputs| {
                inputs
                    .live_sessions
                    .iter()
                    .find(|(session, _)| json!(session) == *id)
                    .map(|(_, live)| *live)
            })
            .ok_or(Error::Unsupported(
                "interrupt requires an explicit live-session observation",
            ))
    }

    fn signal(
        &mut self,
        run: &Value,
        turn: Option<&Value>,
        result: bool,
        before_start: bool,
    ) -> Result<()> {
        use crate::ports::decisions::orchestrator::derived_id;
        let id = run["id"].as_str().ok_or(Error::Stale)?;
        let signal = if result {
            "interrupt-result"
        } else {
            "interrupt-request"
        };
        let reason = self.command["reason"].as_str().unwrap_or("Stopped by user");
        let item = json!({"id":derived_id("turn-item", &["run", id, "signal", signal]), "threadId":self.thread_id,"runId":run["id"],"nodeId":run["rootNodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":turn.map(|turn| &turn["id"]),"nativeItemRef":null,"parentItemId":if result {json!(derived_id("turn-item", &["run",id,"signal","interrupt-request"]))} else {Value::Null},"ordinal":if result && !before_start {run["ordinal"].as_u64().unwrap_or(0)*100+98} else {self.next_item_ordinal()},"status":if result {"interrupted"} else {"completed"},"title":if result {"Interrupted"} else {"Interrupt requested"},"startedAt":self.context.now,"completedAt":self.context.now,"updatedAt":self.context.now,"type":if result {"run_interrupt_result"} else {"run_interrupt_request"},"message":if result && before_start {"Run interrupted before provider start"} else {reason}});
        self.write_record(
            "turnItems",
            "turn-item.updated",
            item,
            &Self::run_scope(run),
        )
    }

    fn interrupt_effect(&mut self, owner: &Value, turn: &Value) -> Result<()> {
        let suffix = format!(
            "provider-turn.interrupt:{}",
            turn["id"].as_str().ok_or(Error::Stale)?
        );
        if self.decision.effects.iter().any(|effect| {
            effect.id
                == format!(
                    "effect:{}:{suffix}",
                    self.command["commandId"].as_str().unwrap_or_default()
                )
        }) {
            return Ok(());
        }
        self.effect(
            &suffix,
            EffectRequest::Interrupt {
                provider_session_id: serde_json::from_value(owner["providerSessionId"].clone())?,
                provider_thread_id: serde_json::from_value(owner["id"].clone())?,
                provider_turn_id: serde_json::from_value(turn["id"].clone())?,
                subagent: None,
            },
        );
        Ok(())
    }

    fn interrupt(&mut self) -> Result<()> {
        self.thread()?;
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["runId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Run {} is not interruptible.",
                        self.command["runId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        let root = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == run["rootNodeId"])
            .cloned();
        let owner = self
            .records("providerThreads")
            .iter()
            .find(|owner| owner["id"] == run["providerThreadId"])
            .cloned();
        let (Some(root), Some(owner)) = (root, owner) else {
            return Err(rejection(
                &self.command,
                format!(
                    "Run {} is not interruptible.",
                    run["id"].as_str().unwrap_or_default()
                ),
            ));
        };
        let background = !self.pending_background_items().is_empty()
            || self.records("providerThreads").iter().any(|provider| {
                provider["pendingBackgroundTasks"]
                    .as_array()
                    .is_some_and(|tasks| !tasks.is_empty())
            });
        let turn = self
            .records("providerTurns")
            .iter()
            .rev()
            .find(|turn| {
                turn["runAttemptId"] == run["activeAttemptId"]
                    && (run["status"] == "running" || turn["status"] == "running" || background)
            })
            .or_else(|| {
                if background {
                    self.records("providerTurns")
                        .iter()
                        .rev()
                        .find(|turn| turn["providerThreadId"] == owner["id"])
                } else {
                    None
                }
            })
            .cloned();
        if turn.is_none()
            && matches!(
                run["status"].as_str(),
                Some("preparing" | "starting" | "running")
            )
        {
            let mut attempt = self
                .records("attempts")
                .iter()
                .find(|attempt| attempt["id"] == run["activeAttemptId"])
                .cloned()
                .ok_or_else(|| {
                    rejection(
                        &self.command,
                        format!(
                            "Run {} has no active attempt to interrupt.",
                            run["id"].as_str().unwrap_or_default()
                        ),
                    )
                })?;
            self.signal(&run, None, false, true)?;
            self.signal(&run, None, true, true)?;
            let scope = Self::run_scope(&run);
            for mut item in self
                .records("turnItems")
                .to_vec()
                .into_iter()
                .filter(|item| {
                    item["runId"] == run["id"]
                        && item["type"] == "command_execution"
                        && item["input"] == "Preparing workspace"
                        && item["status"] == "running"
                })
            {
                item["status"] = json!("interrupted");
                item["title"] = json!("Workspace preparation interrupted");
                item["output"] = self
                    .command
                    .get("reason")
                    .cloned()
                    .unwrap_or(json!("Interrupted before provider start"));
                item["completedAt"] = json!(self.context.now);
                item["updatedAt"] = json!(self.context.now);
                self.write_record("turnItems", "turn-item.updated", item, &scope)?;
            }
            attempt["status"] = json!("interrupted");
            attempt["completedAt"] = json!(self.context.now);
            self.write_record("attempts", "run-attempt.updated", attempt, &scope)?;
            let mut ended = root;
            ended["status"] = json!("interrupted");
            ended["completedAt"] = json!(self.context.now);
            self.write_record("nodes", "node.updated", ended, &scope)?;
            let mut ended = run.clone();
            ended["status"] = json!("interrupted");
            ended["completedAt"] = json!(self.context.now);
            self.write_record("runs", "run.updated", ended, &scope)?;
            for previous in self
                .records("providerTurns")
                .to_vec()
                .into_iter()
                .filter(|turn| {
                    turn["status"] == "running"
                        && self.records("attempts").iter().any(|attempt| {
                            attempt["id"] == turn["runAttemptId"] && attempt["runId"] == run["id"]
                        })
                })
                .collect::<Vec<_>>()
            {
                if let Some(owner) = self
                    .records("providerThreads")
                    .iter()
                    .find(|owner| owner["id"] == previous["providerThreadId"])
                    .cloned()
                    && self.session_present(&owner["providerSessionId"])?
                {
                    self.interrupt_effect(&owner, &previous)?;
                }
            }
            self.stop_remaining(&run)?;
            self.decision.cancellations.push(CancelEffects {
                thread_id: self.thread_id.clone(),
                effect_types: vec!["provider-turn.start".into(), "provider-turn.restart".into()],
                reason: format!(
                    "Run {} was interrupted before its provider turn started.",
                    run["id"].as_str().unwrap_or_default()
                ),
            });
            if background || self.command["holdQueue"] == true {
                self.interrupt_other_background(&owner)?;
            }
            return Ok(());
        }
        let turn = turn.ok_or_else(|| {
            rejection(
                &self.command,
                format!(
                    "Run {} is not interruptible.",
                    run["id"].as_str().unwrap_or_default()
                ),
            )
        })?;
        let live = self.session_present(&owner["providerSessionId"])?;
        if live {
            let session = self
                .records("providerSessions")
                .iter()
                .find(|session| session["id"] == owner["providerSessionId"])
                .ok_or(Error::Unsupported(
                    "interrupt requires session capabilities",
                ))?;
            let caps: OrchestrationV2ProviderCapabilities =
                serde_json::from_value(session["capabilities"].clone())?;
            let command_id: CommandId = serde_json::from_value(self.command["commandId"].clone())?;
            let instance: ProviderInstanceId =
                serde_json::from_value(run["providerInstanceId"].clone())?;
            crate::ports::decisions::command_policy::CapabilityCheck {
                command_id: &command_id,
                thread_id: &self.thread_id,
                provider_instance_id: &instance,
                capabilities: &caps,
            }
            .ensure_interrupt()
            .map_err(|error| rejection(&self.command, error.to_string()))?;
        }
        self.signal(&run, Some(&turn), false, false)?;
        self.stop_remaining(&run)?;
        if live {
            self.interrupt_effect(&owner, &turn)?;
        } else {
            self.settle_interrupted_run(&run, &turn)?;
            self.settle_background(&owner["id"], run["ordinal"].as_u64().unwrap_or(0))?;
        }
        if background || self.command["holdQueue"] == true {
            self.interrupt_other_background(&owner)?;
        }
        Ok(())
    }

    fn interrupt_other_background(&mut self, owner: &Value) -> Result<()> {
        let mut latest: Vec<(Value, Option<u64>)> = Vec::new();
        for item in self.pending_background_items() {
            let Some(turn) = self
                .records("providerTurns")
                .iter()
                .find(|turn| turn["id"] == item["providerTurnId"])
                .cloned()
            else {
                continue;
            };
            if turn["providerThreadId"] == owner["id"] {
                continue;
            }
            let ordinal = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == item["runId"])
                .and_then(|run| run["ordinal"].as_u64());
            if let Some((previous, run_ordinal)) = latest
                .iter_mut()
                .find(|(previous, _)| previous["providerThreadId"] == turn["providerThreadId"])
            {
                if ordinal > *run_ordinal {
                    *previous = turn;
                    *run_ordinal = ordinal;
                }
            } else {
                latest.push((turn, ordinal));
            }
        }
        for (turn, _) in latest {
            if let Some(provider) = self
                .records("providerThreads")
                .iter()
                .find(|provider| provider["id"] == turn["providerThreadId"])
                .cloned()
                && !provider["providerSessionId"].is_null()
                && self.session_present(&provider["providerSessionId"])?
            {
                self.interrupt_effect(&provider, &turn)?;
            }
        }
        Ok(())
    }

    fn settle_interrupted_run(&mut self, run: &Value, turn: &Value) -> Result<()> {
        let Some(attempt) = self
            .records("attempts")
            .iter()
            .find(|attempt| attempt["id"] == turn["runAttemptId"])
            .cloned()
        else {
            return Ok(());
        };
        if run["status"] != "running"
            || run["activeAttemptId"] != attempt["id"]
            || attempt["status"] == "pending"
        {
            return Ok(());
        }
        let scope = Self::run_scope(run);
        for (records, kind, mut value) in [
            ("providerTurns", "provider-turn.updated", turn.clone()),
            ("attempts", "run-attempt.updated", attempt),
        ] {
            if value["status"] == "running" {
                value["status"] = json!("interrupted");
                value["completedAt"] = json!(self.context.now);
                self.write_record(records, kind, value, &scope)?;
            }
        }
        if let Some(mut owner) = self
            .records("providerThreads")
            .iter()
            .find(|owner| {
                owner["id"] == turn["providerThreadId"]
                    && owner["status"] == "active"
                    && owner["lastRunOrdinal"] == run["ordinal"]
            })
            .cloned()
        {
            owner["status"] = json!("idle");
            owner["updatedAt"] = json!(self.context.now);
            self.write_record("providerThreads", "provider-thread.updated", owner, &scope)?;
        }
        for mut message in self
            .records("messages")
            .to_vec()
            .into_iter()
            .filter(|message| message["runId"] == run["id"] && message["streaming"] == true)
        {
            message["streaming"] = json!(false);
            message["updatedAt"] = json!(self.context.now);
            self.write_record("messages", "message.updated", message, &scope)?;
        }
        let delegated: Vec<_> = self
            .records("subagents")
            .iter()
            .filter(|task| task["origin"] == "app_owned")
            .map(|task| task["id"].clone())
            .collect();
        for (records, kind) in [
            ("nodes", "node.updated"),
            ("subagents", "subagent.updated"),
            ("turnItems", "turn-item.updated"),
        ] {
            for mut value in self.records(records).to_vec().into_iter().filter(|value| {
                value["runId"] == run["id"]
                    && matches!(
                        value["status"].as_str(),
                        Some("pending" | "running" | "waiting")
                    )
                    && if records == "nodes" {
                        !delegated.contains(&value["id"])
                    } else if records == "subagents" {
                        value["origin"] != "app_owned"
                    } else {
                        matches!(
                            value["type"].as_str(),
                            Some("assistant_message" | "reasoning")
                        )
                    }
            }) {
                value["status"] = json!("interrupted");
                value["completedAt"] = json!(self.context.now);
                if records != "nodes" {
                    value["updatedAt"] = json!(self.context.now);
                }
                if value.get("streaming").is_some() {
                    value["streaming"] = json!(false);
                }
                self.write_record(records, kind, value, &scope)?;
            }
        }
        for mut request in
            self.records("runtimeRequests")
                .to_vec()
                .into_iter()
                .filter(|request| {
                    request["status"] == "pending"
                        && self.records("nodes").iter().any(|node| {
                            node["id"] == request["nodeId"] && node["runId"] == run["id"]
                        })
                })
                .collect::<Vec<_>>()
        {
            request["status"] = json!("cancelled");
            request["resolvedAt"] = json!(self.context.now);
            request["responseCapability"] =
                json!({"type":"not_resumable","reason":"The run was interrupted."});
            let mut scoped = scope.clone();
            scoped["nodeId"] = request["nodeId"].clone();
            self.write_record(
                "runtimeRequests",
                "runtime-request.updated",
                request,
                &scoped,
            )?;
        }
        self.signal(run, Some(turn), true, false)?;
        let mut ended = self
            .records("runs")
            .iter()
            .find(|candidate| candidate["id"] == run["id"])
            .cloned()
            .unwrap_or_else(|| run.clone());
        ended["status"] = json!("interrupted");
        ended["completedAt"] = json!(self.context.now);
        ended
            .as_object_mut()
            .ok_or(Error::Stale)?
            .shift_remove("delegatedCompletion");
        self.write_record("runs", "run.updated", ended, &scope)?;
        if let Some(root) = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == run["rootNodeId"])
            && !root["checkpointScopeId"].is_null()
        {
            self.decision.effects.push(PendingEffect {
                id: format!(
                    "effect:checkpoint.capture:{}",
                    run["id"].as_str().unwrap_or_default()
                ),
                command_id: CommandId(format!(
                    "command:effect:checkpoint.capture:{}",
                    run["id"].as_str().unwrap_or_default()
                )),
                thread_id: self.thread_id.clone(),
                request: EffectRequest::Capture {
                    run_id: serde_json::from_value(run["id"].clone())?,
                    scope_id: serde_json::from_value(root["checkpointScopeId"].clone())?,
                },
                available_at: None,
            });
        }
        Ok(())
    }

    fn pending_background_items(&self) -> Vec<Value> {
        self.records("turnItems")
            .iter()
            .filter(|item| {
                matches!(
                    item["type"].as_str(),
                    Some("command_execution" | "dynamic_tool" | "subagent")
                ) && matches!(
                    item["status"].as_str(),
                    Some("pending" | "running" | "waiting")
                ) && !(item["type"] == "subagent" && item["origin"] == "app_owned")
                    && !self.records("runs").iter().any(|run| {
                        run["id"] == item["runId"]
                            && matches!(
                                run["status"].as_str(),
                                Some("queued" | "preparing" | "starting" | "rolled_back")
                            )
                    })
            })
            .cloned()
            .collect()
    }

    fn cascade_native_children(&mut self, first: &Value, instance: &Value) -> Result<()> {
        if first.is_null() {
            return Ok(());
        }
        let mut pending = vec![(first.clone(), instance.clone())];
        let mut seen = Vec::new();
        while let Some((id, instance)) = pending.pop() {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            let Some(child) = self
                .inputs
                .and_then(|inputs| {
                    inputs
                        .related_projections
                        .iter()
                        .find(|child| stored_value(&child.thread.id).ok().as_ref() == Some(&id))
                })
                .cloned()
            else {
                continue;
            };
            self.end_runless_child(
                &child,
                &instance,
                &json!("interrupted"),
                &json!(self.context.now),
            )?;
            let saved_projection = self.projection.clone();
            let saved_thread = self.thread_id.clone();
            self.projection = stored_value(&child)?;
            self.thread_id = serde_json::from_value(id)?;
            let result = (|| -> Result<()> {
                for task in child
                    .subagents
                    .iter()
                    .map(stored_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_iter()
                    .filter(|task| task["origin"] == "provider_native" && task["runId"].is_null())
                {
                    self.end_native_task(
                        &task["id"],
                        &json!("interrupted"),
                        &json!(self.context.now),
                        None,
                    )?;
                    if !task["childThreadId"].is_null() {
                        pending.push((
                            task["childThreadId"].clone(),
                            task["providerInstanceId"].clone(),
                        ));
                    }
                }
                Ok(())
            })();
            self.projection = saved_projection;
            self.thread_id = saved_thread;
            result?;
        }
        Ok(())
    }

    fn settle_background(&mut self, stopped: &Value, through: u64) -> Result<()> {
        for mut item in self.pending_background_items() {
            if self.records("runs").iter().any(|run| {
                run["id"] == item["runId"] && run["ordinal"].as_u64().unwrap_or(0) > through
            }) {
                continue;
            }
            if !item["providerThreadId"].is_null()
                && item["providerThreadId"] != *stopped
                && let Some(owner) = self
                    .records("providerThreads")
                    .iter()
                    .find(|owner| owner["id"] == item["providerThreadId"])
                && self.session_present(&owner["providerSessionId"])?
            {
                continue;
            }
            let scope = item.clone();
            item["status"] = json!("interrupted");
            item["completedAt"] = json!(self.context.now);
            item["updatedAt"] = json!(self.context.now);
            self.write_record("turnItems", "turn-item.updated", item.clone(), &scope)?;
            if item["type"] == "subagent" {
                self.end_native_task(
                    &item["subagentId"],
                    &json!("interrupted"),
                    &json!(self.context.now),
                    None,
                )?;
                self.cascade_native_children(&item["childThreadId"], &item["providerInstanceId"])?;
            }
        }
        for mut owner in self
            .records("providerThreads")
            .to_vec()
            .into_iter()
            .filter(|owner| {
                owner["pendingBackgroundTasks"]
                    .as_array()
                    .is_some_and(|tasks| !tasks.is_empty())
            })
        {
            if owner["id"] == *stopped {
                if self.inputs.is_some_and(|inputs| {
                    inputs
                        .background_work
                        .iter()
                        .any(|(id, pending)| json!(id) == owner["id"] && *pending)
                }) {
                    continue;
                }
            } else if self.session_present(&owner["providerSessionId"])? {
                continue;
            }
            if owner["status"] == "active"
                && owner["lastRunOrdinal"]
                    .as_u64()
                    .is_some_and(|ordinal| ordinal <= through)
            {
                owner["status"] = json!("idle");
            }
            owner["pendingBackgroundTasks"] = json!([]);
            owner["updatedAt"] = json!(self.context.now);
            let scope = owner.clone();
            self.write_record("providerThreads", "provider-thread.updated", owner, &scope)?;
        }
        Ok(())
    }

    fn end_native_task(
        &mut self,
        id: &Value,
        status: &Value,
        completed: &Value,
        result: Option<&Value>,
    ) -> Result<()> {
        let Some(task) = self
            .records("subagents")
            .iter()
            .find(|task| task["id"] == *id && task["origin"] == "provider_native")
            .cloned()
        else {
            return Ok(());
        };
        let scope = json!({"runId":task["runId"],"nodeId":task["id"],"providerInstanceId":task["providerInstanceId"]});
        for (records, kind) in [
            ("subagents", "subagent.updated"),
            ("nodes", "node.updated"),
            ("turnItems", "turn-item.updated"),
        ] {
            let row = self
                .records(records)
                .iter()
                .find(|row| {
                    if records == "turnItems" {
                        row["type"] == "subagent" && row["subagentId"] == *id
                    } else {
                        row["id"] == *id
                    }
                })
                .cloned();
            let Some(mut row) = row.filter(|row| {
                matches!(
                    row["status"].as_str(),
                    Some("pending" | "running" | "waiting")
                )
            }) else {
                continue;
            };
            if self
                .decision
                .events
                .iter()
                .map(stored_value)
                .collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .any(|event| event["type"] == kind && event["payload"]["id"] == row["id"])
            {
                continue;
            }
            row["status"] = status.clone();
            row["completedAt"] = completed.clone();
            if records != "nodes" {
                row["updatedAt"] = json!(self.context.now);
                if let Some(result) = result {
                    row["result"] = result.clone();
                }
            }
            let mut scoped = scope.clone();
            if records == "subagents" {
                scoped["driver"] = task["driver"].clone();
            }
            self.write_record(records, kind, row, &scoped)?;
        }
        Ok(())
    }

    fn end_runless_child(
        &mut self,
        child: &OrchestrationV2ThreadProjection,
        instance: &Value,
        status: &Value,
        completed: &Value,
    ) -> Result<()> {
        let value = stored_value(child)?;
        let saved = self.thread_id.clone();
        self.thread_id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
            &child.thread.id,
        )?;
        let result = (|| -> Result<()> {
            for node in value["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|node| {
                    node["kind"] == "root_turn"
                        && node["runId"].is_null()
                        && matches!(
                            node["status"].as_str(),
                            Some("pending" | "running" | "waiting")
                        )
                })
            {
                let scope = json!({"nodeId":node["id"],"providerInstanceId":instance});
                let mut ended = node.clone();
                ended["status"] = status.clone();
                ended["completedAt"] = completed.clone();
                self.emit("node.updated", ended, Some(&scope))?;
                for item in value["turnItems"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|item| {
                        item["nodeId"] == node["id"]
                            && item["runId"].is_null()
                            && matches!(
                                item["status"].as_str(),
                                Some("pending" | "running" | "waiting")
                            )
                    })
                {
                    let mut ended = item.clone();
                    ended["status"] = if status == "completed"
                        && matches!(
                            item["type"].as_str(),
                            Some("command_execution" | "dynamic_tool")
                        ) {
                        json!("failed")
                    } else {
                        status.clone()
                    };
                    ended["completedAt"] = completed.clone();
                    ended["updatedAt"] = json!(self.context.now);
                    if matches!(
                        item["type"].as_str(),
                        Some("assistant_message" | "reasoning")
                    ) {
                        ended["streaming"] = json!(false);
                    }
                    self.emit("turn-item.updated", ended, Some(&scope))?;
                }
                for turn in value["providerTurns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|turn| {
                        turn["nodeId"] == node["id"]
                            && turn["runAttemptId"].is_null()
                            && matches!(
                                turn["status"].as_str(),
                                Some("pending" | "running" | "waiting")
                            )
                    })
                {
                    let mut ended = turn.clone();
                    ended["status"] = status.clone();
                    ended["completedAt"] = completed.clone();
                    self.emit("provider-turn.updated", ended, Some(&scope))?;
                }
            }
            Ok(())
        })();
        self.thread_id = saved;
        result
    }

    fn background_settle(&mut self) -> Result<()> {
        if let Some(stopped) = self.command.get("subagent").cloned() {
            let Some(task) = self
                .records("subagents")
                .iter()
                .find(|task| {
                    task["id"] == stopped["id"]
                        && task["origin"] == "provider_native"
                        && matches!(
                            task["status"].as_str(),
                            Some("pending" | "running" | "waiting")
                        )
                })
                .cloned()
            else {
                return Ok(());
            };
            if task["startedAt"]
                .as_str()
                .and_then(epoch_millis)
                .zip(stopped["stoppedAt"].as_str().and_then(epoch_millis))
                .is_some_and(|(started, stopped)| started > stopped)
            {
                return Ok(());
            }
            let outcome = &stopped["outcome"];
            self.end_native_task(
                &task["id"],
                &outcome["status"],
                &outcome["completedAt"],
                outcome.get("result"),
            )?;
            if !task["childThreadId"].is_null()
                && let Some(child) = self
                    .inputs
                    .and_then(|inputs| {
                        inputs.related_projections.iter().find(|child| {
                            stored_value(&child.thread.id).ok().as_ref()
                                == Some(&task["childThreadId"])
                        })
                    })
                    .cloned()
            {
                self.end_runless_child(
                    &child,
                    &task["providerInstanceId"],
                    &outcome["status"],
                    &outcome["completedAt"],
                )?;
            }
            return Ok(());
        }
        let Some(turn) = self
            .records("providerTurns")
            .iter()
            .find(|turn| {
                turn["id"] == self.command["providerTurnId"]
                    && turn["providerThreadId"] == self.command["providerThreadId"]
            })
            .cloned()
        else {
            return Ok(());
        };
        let Some(run) = self
            .records("attempts")
            .iter()
            .find(|attempt| attempt["id"] == turn["runAttemptId"])
            .and_then(|attempt| {
                self.records("runs")
                    .iter()
                    .find(|run| run["id"] == attempt["runId"])
            })
            .cloned()
        else {
            return Ok(());
        };
        if matches!(
            run["status"].as_str(),
            Some("preparing" | "starting" | "running")
        ) && run["activeAttemptId"] != turn["runAttemptId"]
        {
            return Ok(());
        }
        self.settle_interrupted_run(&run, &turn)?;
        if self.records("runs").iter().any(|candidate| {
            candidate["id"] != run["id"]
                && matches!(
                    candidate["status"].as_str(),
                    Some("preparing" | "starting" | "running")
                )
        }) {
            return Ok(());
        }
        self.settle_background(
            &self.command["providerThreadId"].clone(),
            run["ordinal"].as_u64().unwrap_or(0),
        )
    }

    fn stop_remaining(&mut self, run: &Value) -> Result<()> {
        let wake = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == run["userMessageId"])
            .map(|message| message["delegatedCompletion"].clone())
            .unwrap_or(Value::Null);
        if self.command["holdQueue"] == true {
            self.hold_stopped()?;
        } else {
            self.dispose_cohort(&run["id"], "stopped", None)?;
            if !wake.is_null() && wake["parentRunId"] != run["id"] {
                self.dispose_cohort(&wake["parentRunId"], "stopped", wake["taskIds"].as_array())?;
            }
        }
        Ok(())
    }

    fn hold_stopped(&mut self) -> Result<()> {
        let mut thread = self.thread()?;
        if !thread["limitRecovery"].is_null() && thread["limitRecovery"]["autoResume"] == true {
            thread["limitRecovery"]["autoResume"] = json!(false);
            thread["limitRecovery"]["requestId"] = self.command["commandId"].clone();
            thread["updatedAt"] = json!(self.context.now);
            self.update_thread("thread.metadata-updated", thread.clone())?;
        }
        for mut run in self
            .records("runs")
            .to_vec()
            .into_iter()
            .filter(|run| run["status"] == "queued" && run["queueHeld"] != true)
        {
            if self.records("messages").iter().any(|message| {
                message["id"] == run["userMessageId"]
                    && message["notification"]["delivery"] == "buffered"
            }) {
                self.cancel_queued_run(&run)?;
                continue;
            }
            run["queueHeld"] = json!(true);
            let scope = Self::run_scope(&run);
            self.write_record("runs", "run.updated", run, &scope)?;
        }
        if thread["pullRequests"]
            .as_array()
            .is_some_and(|links| links.iter().any(|link| link.get("watch").is_some()))
        {
            for link in thread["pullRequests"].as_array_mut().ok_or(Error::Stale)? {
                link.as_object_mut()
                    .ok_or(Error::Stale)?
                    .shift_remove("watch");
            }
            self.update_thread("thread.pull-request-synced", thread)?;
        }
        let mut ids: Vec<_> = self
            .records("runs")
            .iter()
            .filter(|run| run.get("delegatedCompletion").is_some())
            .map(|run| run["id"].clone())
            .collect();
        for task in self
            .records("subagents")
            .iter()
            .filter(|task| task["origin"] == "app_owned" && !task["runId"].is_null())
        {
            if !ids.contains(&task["runId"]) {
                ids.push(task["runId"].clone());
            }
        }
        for id in ids {
            self.dispose_cohort(&id, "stopped", None)?;
        }
        for mut item in self
            .records("turnItems")
            .to_vec()
            .into_iter()
            .filter(|item| item["type"] == "link_request" && item["linkStatus"] == "pending")
        {
            item["status"] = json!("cancelled");
            item["linkStatus"] = json!("cancelled");
            item["completedAt"] = json!(self.context.now);
            item["updatedAt"] = json!(self.context.now);
            let scope = item.clone();
            self.write_record("turnItems", "turn-item.updated", item, &scope)?;
        }
        Ok(())
    }

    fn cancel_idle_orphans(&mut self) -> Result<()> {
        let projection: OrchestrationV2ThreadProjection =
            serde_json::from_value(self.projection.clone())?;
        let children = self
            .inputs
            .map_or(&[][..], |inputs| inputs.related_projections.as_slice());
        let tasks = crate::ports::decisions::orchestrator::idle_orphan_tasks(
            &projection,
            children,
            pending_restart_observations(self.inputs),
            &self.context.now,
        )?;
        for task in tasks {
            let task = stored_value(task)?;
            let scope = json!({"nodeId":task["id"]});
            self.write_record("subagents", "subagent.updated", task.clone(), &scope)?;
            if let Some(mut node) = self
                .records("nodes")
                .iter()
                .find(|node| node["id"] == task["id"])
                .cloned()
            {
                node["status"] = task["status"].clone();
                node["completedAt"] = task["completedAt"].clone();
                self.write_record("nodes", "node.updated", node, &scope)?;
            }
            for mut item in self
                .records("turnItems")
                .to_vec()
                .into_iter()
                .filter(|item| item["type"] == "subagent" && item["subagentId"] == task["id"])
            {
                item["status"] = task["status"].clone();
                item["completedAt"] = task["completedAt"].clone();
                item["updatedAt"] = json!(self.context.now);
                self.write_record("turnItems", "turn-item.updated", item, &scope)?;
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if !self.projection["thread"]["deletedAt"].is_null() {
            return Ok(());
        }
        let target = self
            .records("runs")
            .iter()
            .rev()
            .find(|run| {
                matches!(
                    run["status"].as_str(),
                    Some("preparing" | "starting" | "running")
                )
            })
            .cloned()
            .or_else(|| {
                if !self.pending_background_items().is_empty()
                    || self.records("providerThreads").iter().any(|owner| {
                        owner["pendingBackgroundTasks"]
                            .as_array()
                            .is_some_and(|tasks| !tasks.is_empty())
                    })
                {
                    self.records("runs")
                        .iter()
                        .rev()
                        .find(|run| run["status"] != "queued")
                        .cloned()
                } else {
                    None
                }
            });
        let mut interrupted = false;
        if let Some(run) = &target {
            let saved_command = self.command.clone();
            let saved_projection = self.projection.clone();
            let saved_ids = self.id_positions.clone();
            let saved_decision = self.decision.clone();
            self.command["type"] = json!("run.interrupt");
            self.command["runId"] = run["id"].clone();
            self.command["holdQueue"] = json!(true);
            interrupted = self.interrupt().is_ok();
            self.command = saved_command;
            if !interrupted {
                self.projection = saved_projection;
                self.id_positions = saved_ids;
                self.decision = saved_decision;
            }
        }
        if !interrupted {
            let executed = self
                .records("runs")
                .iter()
                .rev()
                .find(|run| run["status"] != "queued")
                .cloned();
            let mut reached = target.into_iter().collect::<Vec<_>>();
            if let Some(run) = executed
                && !reached.iter().any(|candidate| candidate["id"] == run["id"])
                && !matches!(
                    run["status"].as_str(),
                    Some("preparing" | "starting" | "running")
                )
            {
                reached.push(run);
            }
            for run in reached {
                if !self.records("turnItems").iter().any(|item| {
                    item["type"] == "run_interrupt_request" && item["runId"] == run["id"]
                }) {
                    self.signal(&run, None, false, false)?;
                }
            }
            self.hold_stopped()?;
        }
        if self.command["stopDelegatedTasks"] != false
            && self
                .records("subagents")
                .iter()
                .any(|task| task["origin"] == "app_owned" && !task["childThreadId"].is_null())
        {
            self.effect(
                "delegated-tasks.stop",
                EffectRequest::StopDelegated {
                    reason: self.command["reason"].as_str().map(str::to_owned),
                },
            );
        }
        Ok(())
    }

    fn delivery_run(&self, delivery: &Value) -> Option<Value> {
        self.records("messages")
            .iter()
            .find(|message| message["id"] == delivery["messageId"])
            .and_then(|message| {
                self.records("runs")
                    .iter()
                    .find(|run| run["id"] == message["runId"])
            })
            .cloned()
    }

    fn dispose_cohort(
        &mut self,
        parent: &Value,
        disposition: &str,
        interrupted: Option<&Vec<Value>>,
    ) -> Result<()> {
        self.dispose_cohort_with_cancel(parent, disposition, interrupted, true)
    }

    fn dispose_cohort_with_cancel(
        &mut self,
        parent: &Value,
        disposition: &str,
        interrupted: Option<&Vec<Value>>,
        cancel_queued: bool,
    ) -> Result<()> {
        let Some(mut run) = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == *parent)
            .cloned()
        else {
            return Ok(());
        };
        let tasks: Vec<_> = self
            .records("subagents")
            .iter()
            .filter(|task| task["origin"] == "app_owned" && task["runId"] == *parent)
            .cloned()
            .collect();
        let cohort = run["delegatedCompletion"].clone();
        if cohort.is_null() && tasks.is_empty() {
            return Ok(());
        }
        let kept: Vec<_> = tasks
            .iter()
            .filter(|task| {
                interrupted.is_some_and(|ids| !ids.contains(&task["id"]))
                    && cohort["disposition"] == "open"
                    && !matches!(
                        task["completionDelivery"]["state"].as_str(),
                        Some("acknowledged" | "delivered" | "disposed")
                    )
            })
            .map(|task| task["id"].clone())
            .collect();
        if kept.is_empty() {
            run["delegatedCompletion"] = json!({"disposition":if cohort["disposition"]=="disposed" {"disposed"}else{disposition},"nextGeneration":cohort.get("nextGeneration").cloned().unwrap_or(json!(1)),"delivery":null});
        }
        let scope = Self::run_scope(&run);
        self.write_record("runs", "run.updated", run, &scope)?;
        for mut task in tasks.into_iter().filter(|task| {
            !kept.contains(&task["id"])
                && !matches!(
                    task["completionDelivery"]["state"].as_str(),
                    Some("acknowledged" | "delivered" | "disposed")
                )
        }) {
            task["completionDelivery"] = json!({"state":"disposed","observedByRunId":null});
            task["updatedAt"] = json!(self.context.now);
            let mut scope = task.clone();
            scope["nodeId"] = task["id"].clone();
            self.write_record("subagents", "subagent.updated", task, &scope)?;
        }
        if cancel_queued
            && kept.is_empty()
            && let Some(wake) = self.delivery_run(&cohort["delivery"])
            && wake["status"] == "queued"
        {
            self.cancel_queued_run(&wake)?;
        }
        Ok(())
    }

    fn completion_detail(&self, ids: &[Value], replacement: Option<&Value>) -> String {
        let mut remaining = 16_000;
        ids.iter()
            .map(|id| {
                let task = replacement.filter(|task| task["id"] == *id).or_else(|| {
                    self.records("subagents")
                        .iter()
                        .find(|task| task["id"] == *id)
                });
                let label = format!(
                    "Delegated task {}{}{}",
                    id.as_str().unwrap_or_default(),
                    task.and_then(|task| task["title"].as_str())
                        .map(|title| format!(" ({title})"))
                        .unwrap_or_default(),
                    task.map(|task| format!(" [{}]", task["status"].as_str().unwrap_or_default()))
                        .unwrap_or_else(|| " reached a terminal state".into())
                );
                if let Some(result) = task.and_then(|task| task["result"].as_str()) {
                    let detail = format!("{label}:\n{result}");
                    if detail.len() <= remaining {
                        remaining -= detail.len();
                        return detail;
                    }
                }
                format!(
                    "{label}. Use task_status with taskId {} to read the result.",
                    id.as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn plan_delivery(&mut self, mut task: Value) -> Result<Value> {
        if matches!(
            task["completionDelivery"]["state"].as_str(),
            Some("acknowledged" | "delivered" | "disposed")
        ) {
            return Ok(task);
        }
        let parent = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == task["runId"])
            .cloned();
        if parent.is_none()
            || !self.projection["thread"]["archivedAt"].is_null()
            || !self.projection["thread"]["deletedAt"].is_null()
            || parent.as_ref().is_some_and(|run| {
                !run["delegatedCompletion"].is_null()
                    && run["delegatedCompletion"]["disposition"] != "open"
            })
        {
            task["completionDelivery"] = json!({"state":"disposed","observedByRunId":null});
            return Ok(task);
        }
        let mut parent = parent.ok_or(Error::Stale)?;
        if task["completionWake"] != "always"
            && matches!(
                parent["status"].as_str(),
                Some("preparing" | "starting" | "running")
            )
        {
            return Ok(task);
        }
        let cohort = parent["delegatedCompletion"].clone();
        let delivery = cohort["delivery"].clone();
        if !delivery.is_null() {
            let wake = self.delivery_run(&delivery);
            if wake.as_ref().is_some_and(|run| run["status"] != "queued") {
                task["completionDelivery"] = json!({"state":"pending","observedByRunId":null});
                return Ok(task);
            }
            let mut ids = delivery["taskIds"].as_array().cloned().unwrap_or_default();
            if !ids.contains(&task["id"]) {
                ids.push(task["id"].clone());
            }
            parent["delegatedCompletion"]["delivery"]["taskIds"] = json!(ids);
            if let Some(mut message) = self
                .records("messages")
                .iter()
                .find(|message| message["id"] == delivery["messageId"])
                .cloned()
                .filter(|_| wake.is_some())
            {
                message["text"] = json!(self.completion_detail(&ids, Some(&task)));
                message["delegatedCompletion"] = json!({"parentRunId":parent["id"],"generation":delivery["generation"],"taskIds":ids});
                message["updatedAt"] = json!(self.context.now);
                let scope = message.clone();
                self.write_record("messages", "message.updated", message, &scope)?;
            }
        } else {
            let generation = cohort["nextGeneration"].as_u64().unwrap_or(1);
            let message = self.allocate(IdKind::Message)?;
            parent["delegatedCompletion"] = json!({"disposition":"open","nextGeneration":generation+1,"delivery":{"generation":generation,"messageId":message,"taskIds":[task["id"]]}});
        }
        task["completionDelivery"] = json!({"state":"claimed","observedByRunId":null});
        let scope = Self::run_scope(&parent);
        self.write_record("runs", "run.updated", parent, &scope)?;
        Ok(task)
    }

    fn write_delivery_task(&mut self, task: Value, scope: &Value) -> Result<Value> {
        let first = self.decision.events.len();
        let task = self.plan_delivery(task)?;
        self.write_record("subagents", "subagent.updated", task.clone(), scope)?;
        let event = self.decision.events.pop().ok_or(Error::Stale)?;
        self.decision.events.insert(first, event);
        Ok(task)
    }

    fn remote_complete(&mut self) -> Result<()> {
        let Some(mut task) = self
            .records("subagents")
            .iter()
            .find(|task| {
                task["id"] == self.command["taskId"]
                    && task["origin"] == "app_owned"
                    && task.get("remoteChild").is_some()
            })
            .cloned()
        else {
            return Err(rejection(
                &self.command,
                format!(
                    "Delegated task {} is not a remote task of thread {}.",
                    self.command["taskId"].as_str().unwrap_or_default(),
                    self.thread_id.0
                ),
            ));
        };
        if self.command["stoppedThere"] == "stopped"
            && self.command["stopCount"].as_u64().unwrap_or(0)
                != task["remoteChild"]["stopCount"].as_u64().unwrap_or(0)
        {
            return Ok(());
        }
        if !task["result"].is_null() {
            if self.command.get("stoppedThere").is_none() {
                return self.remote_stop();
            }
            let owes = matches!(task["status"].as_str(), Some("cancelled" | "interrupted"))
                || task["remoteChild"]["stoppedThere"] == false;
            if task["remoteChild"]["stoppedThere"] == true
                || owes && self.command["stoppedThere"] != "stopped"
            {
                return Ok(());
            }
            task["remoteChild"]["stoppedThere"] = json!(true);
            task["updatedAt"] = json!(self.context.now);
            let scope = json!({"runId":task["runId"],"nodeId":task["id"],"driver":task["driver"]});
            return self.write_record("subagents", "subagent.updated", task, &scope);
        }
        if self.command.get("stoppedThere").is_some() {
            task["remoteChild"]["stoppedThere"] = json!(true);
        }
        task["status"] = self.command["status"].clone();
        task["result"] = self.command["result"].clone();
        task["completedAt"] = json!(self.context.now);
        task["updatedAt"] = json!(self.context.now);
        let scope = json!({"runId":task["runId"],"nodeId":task["id"],"driver":task["driver"]});
        let task = self.write_delivery_task(task, &scope)?;
        if let Some(mut node) = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == task["id"])
            .cloned()
        {
            node["status"] = self.command["status"].clone();
            node["completedAt"] = json!(self.context.now);
            self.write_record("nodes", "node.updated", node, &scope)?;
        }
        if let Some(mut item) = self
            .records("turnItems")
            .iter()
            .find(|item| item["type"] == "subagent" && item["subagentId"] == task["id"])
            .cloned()
        {
            item["status"] = self.command["status"].clone();
            item["result"] = self.command["result"].clone();
            item["completedAt"] = json!(self.context.now);
            item["updatedAt"] = json!(self.context.now);
            self.write_record("turnItems", "turn-item.updated", item, &scope)?;
        }
        Ok(())
    }

    fn wake_policy(&mut self) -> Result<()> {
        let Some(mut task) = self
            .records("subagents")
            .iter()
            .find(|task| task["id"] == self.command["taskId"] && task["origin"] == "app_owned")
            .cloned()
        else {
            return Err(rejection(
                &self.command,
                format!(
                    "Delegated task {} is not an app-owned task of thread {}.",
                    self.command["taskId"].as_str().unwrap_or_default(),
                    self.thread_id.0
                ),
            ));
        };
        if task["completionWake"] == self.command["completionWake"] {
            return Err(rejection(
                &self.command,
                format!(
                    "Delegated task {} already wakes the parent with completionWake {}.",
                    task["id"].as_str().unwrap_or_default(),
                    self.command["completionWake"].as_str().unwrap_or_default()
                ),
            ));
        }
        task["completionWake"] = self.command["completionWake"].clone();
        task["updatedAt"] = json!(self.context.now);
        if task["completionWake"] == "always"
            && matches!(
                task["status"].as_str(),
                Some("completed" | "failed" | "cancelled" | "interrupted")
            )
            && self.records("runs").iter().any(|run| {
                run["id"] == task["runId"]
                    && matches!(
                        run["status"].as_str(),
                        Some("preparing" | "starting" | "running")
                    )
            })
        {
            task = self.plan_delivery(task)?;
        }
        let scope = json!({"runId":task["runId"],"nodeId":task["id"],"driver":task["driver"],"providerInstanceId":task["providerInstanceId"]});
        self.write_record("subagents", "subagent.updated", task, &scope)
    }

    fn notification_accept(&mut self) -> Result<()> {
        let message = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == self.command["messageId"])
            .cloned();
        if let Some(message) = message
            && !message["delegatedCompletion"].is_null()
        {
            let completion = &message["delegatedCompletion"];
            let Some(mut parent) = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == completion["parentRunId"])
                .cloned()
            else {
                return self.update_thread("thread.metadata-updated", self.thread()?);
            };
            if parent["delegatedCompletion"]["disposition"] == "open"
                && parent["delegatedCompletion"]["delivery"]["messageId"] == message["id"]
                && parent["delegatedCompletion"]["delivery"]["generation"]
                    == completion["generation"]
            {
                let parent_live = matches!(
                    parent["status"].as_str(),
                    Some("preparing" | "starting" | "running")
                );
                let pending: Vec<_> = if self.projection["thread"]["archivedAt"].is_null()
                    && self.projection["thread"]["deletedAt"].is_null()
                {
                    self.records("subagents")
                        .iter()
                        .filter(|task| {
                            task["origin"] == "app_owned"
                                && task["runId"] == parent["id"]
                                && task["completionDelivery"]["state"] == "pending"
                                && matches!(
                                    task["status"].as_str(),
                                    Some("completed" | "failed" | "cancelled" | "interrupted")
                                )
                                && (!parent_live || task["completionWake"] == "always")
                        })
                        .map(|task| task["id"].clone())
                        .collect()
                } else {
                    Vec::new()
                };
                let accepted = parent["delegatedCompletion"]["delivery"]["taskIds"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for mut task in self.records("subagents").to_vec() {
                    let state = if pending.contains(&task["id"]) {
                        "claimed"
                    } else if accepted.contains(&task["id"])
                        && task["completionDelivery"]["state"] == "claimed"
                    {
                        "delivered"
                    } else {
                        continue;
                    };
                    task["completionDelivery"] = json!({"state":state,"observedByRunId":null});
                    task["updatedAt"] = json!(self.context.now);
                    let scope = json!({"runId":parent["id"],"nodeId":task["id"],"driver":task["driver"],"providerInstanceId":task["providerInstanceId"]});
                    self.write_record("subagents", "subagent.updated", task, &scope)?;
                }
                if pending.is_empty() {
                    parent["delegatedCompletion"]["delivery"] = Value::Null;
                } else {
                    let generation = parent["delegatedCompletion"]["nextGeneration"]
                        .as_u64()
                        .ok_or(Error::Stale)?;
                    parent["delegatedCompletion"]["delivery"] = json!({"generation":generation,"messageId":self.allocate(IdKind::Message)?,"taskIds":pending});
                    parent["delegatedCompletion"]["nextGeneration"] = json!(generation + 1);
                }
                let scope = Self::run_scope(&parent);
                return self.write_record("runs", "run.updated", parent, &scope);
            }
        }
        self.update_thread("thread.metadata-updated", self.thread()?)
    }

    fn completion_resolution(&mut self) -> Result<()> {
        let Some(mut task) = self
            .records("subagents")
            .iter()
            .find(|task| task["id"] == self.command["taskId"] && task["origin"] == "app_owned")
            .cloned()
        else {
            return Err(rejection(
                &self.command,
                format!(
                    "Delegated task {} is not an app-owned task of thread {}.",
                    self.command["taskId"].as_str().unwrap_or_default(),
                    self.thread_id.0
                ),
            ));
        };
        let acknowledge = self.command["type"] == "delegated_task.completion-delivery.acknowledge";
        if acknowledge && self.command["observedByRunId"].is_null() {
            if !matches!(
                task["status"].as_str(),
                Some("completed" | "failed" | "cancelled" | "interrupted")
            ) || task["completionDelivery"]["state"] != "pending"
                || self.records("runs").iter().any(|run| {
                    run["id"] == task["runId"]
                        && matches!(
                            run["status"].as_str(),
                            Some("preparing" | "starting" | "running")
                        )
                })
            {
                return Ok(());
            }
            let planned = self.plan_delivery(task)?;
            if planned["completionDelivery"]["state"] == "pending" {
                return Ok(());
            }
            let scope = json!({"runId":planned["runId"],"nodeId":planned["id"],"driver":planned["driver"],"providerInstanceId":planned["providerInstanceId"]});
            return self.write_record("subagents", "subagent.updated", planned, &scope);
        }
        let state = if acknowledge {
            "acknowledged"
        } else {
            "disposed"
        };
        let scope = json!({"runId":task["runId"],"nodeId":task["id"],"driver":task["driver"],"providerInstanceId":task["providerInstanceId"]});
        if task["completionDelivery"]["state"] == state
            || acknowledge && task["completionDelivery"]["state"] == "disposed"
        {
            return self.write_record("subagents", "subagent.updated", task, &scope);
        }
        task["completionDelivery"] = json!({"state":state,"observedByRunId":if acknowledge {self.command["observedByRunId"].clone()}else{Value::Null}});
        task["updatedAt"] = json!(self.context.now);
        self.write_record("subagents", "subagent.updated", task.clone(), &scope)?;
        let Some(mut parent) = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == task["runId"])
            .cloned()
        else {
            return Ok(());
        };
        let delivery = parent["delegatedCompletion"]["delivery"].clone();
        let Some(ids) = delivery["taskIds"].as_array() else {
            return Ok(());
        };
        if !ids.contains(&task["id"]) {
            return Ok(());
        }
        let remaining: Vec<_> = ids
            .iter()
            .filter(|id| **id != task["id"])
            .cloned()
            .collect();
        let wake = self.delivery_run(&delivery);
        if remaining.is_empty() && wake.as_ref().is_none_or(|run| run["status"] == "queued") {
            parent["delegatedCompletion"]["delivery"] = Value::Null;
        } else {
            parent["delegatedCompletion"]["delivery"]["taskIds"] = json!(remaining);
        }
        let scope = Self::run_scope(&parent);
        self.write_record("runs", "run.updated", parent.clone(), &scope)?;
        if let Some(wake) = wake
            && wake["status"] == "queued"
        {
            if remaining.is_empty() {
                self.cancel_queued_run(&wake)?;
            } else {
                if let Some(mut message) = self
                    .records("messages")
                    .iter()
                    .find(|message| message["id"] == delivery["messageId"])
                    .cloned()
                {
                    message["text"] = json!(self.completion_detail(&remaining, None));
                    message["delegatedCompletion"] = json!({"parentRunId":parent["id"],"generation":delivery["generation"],"taskIds":remaining});
                    message["updatedAt"] = json!(self.context.now);
                    let scope = Self::run_scope(&wake);
                    self.write_record("messages", "message.updated", message, &scope)?;
                }
            }
        }
        Ok(())
    }

    fn update_thread(&mut self, kind: &str, thread: Value) -> Result<()> {
        self.emit(kind, thread.clone(), Some(&thread))?;
        self.projection["thread"] = thread;
        Ok(())
    }

    fn root_failure(&self, run: &Value) -> Option<&Value> {
        self.records("turnItems")
            .iter()
            .filter(|item| {
                item["type"] == "error"
                    && item["status"] == "failed"
                    && item["runId"] == run["id"]
                    && item["nodeId"] == run["rootNodeId"]
            })
            .max_by(|left, right| {
                left["updatedAt"]
                    .as_str()
                    .and_then(epoch_millis)
                    .cmp(&right["updatedAt"].as_str().and_then(epoch_millis))
                    .then_with(|| left["ordinal"].as_u64().cmp(&right["ordinal"].as_u64()))
                    .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
            })
            .map(|item| &item["failure"])
    }

    fn admit_continuation(&mut self, mode: &Value) -> Result<bool> {
        use crate::ports::runs_ingestion::restart_background_note::{
            continuation_failed_before_prompt, is_restart_continuation_source,
        };
        let thread = self.projection["thread"].clone();
        let typed = self.context.projection.as_ref().ok_or(Error::Stale)?;
        let stopped = |id: &Value| {
            self.records("turnItems")
                .iter()
                .any(|item| item["type"] == "run_interrupt_request" && item["runId"] == *id)
        };
        if let Some(id) = self.command.get("steerFollowUpOfRunId") {
            let source = self.records("runs").iter().find(|run| run["id"] == *id);
            if source.is_none()
                || mode["type"] != "queue_after_active"
                || !self.records("messages").iter().any(|message| {
                    message["id"] == self.command["messageId"] && message["runId"] == *id
                })
                || !thread["archivedAt"].is_null()
                || !thread["deletedAt"].is_null()
                || stopped(id)
            {
                self.update_thread("thread.metadata-updated", thread)?;
                return Ok(false);
            }
        }
        if let Some(id) = self.command.get("manualContinuationOfRunId") {
            let source = self.records("runs").iter().find(|run| run["id"] == *id);
            let latest = crate::ports::decisions::model_switch::latest_executed_run(&typed.runs)
                .map(stored_value)
                .transpose()?;
            if mode["type"] != "start_immediately"
                || source.is_none_or(|run| {
                    run["status"] != "interrupted"
                        && !(run["status"] == "failed"
                            && self
                                .root_failure(run)
                                .is_some_and(|failure| failure["class"] == "usage_limit"))
                })
                || latest.is_none_or(|run| run["id"] != *id)
                || !thread["archivedAt"].is_null()
                || !thread["deletedAt"].is_null()
                || self
                    .records("runtimeRequests")
                    .iter()
                    .any(|request| request["status"] == "pending")
            {
                return Err(rejection(
                    &self.command,
                    "This thread can no longer be resumed from that run.",
                ));
            }
        }
        if let Some(id) = self.command.get("restartContinuationOfRunId") {
            let source = typed
                .runs
                .iter()
                .find(|run| stored_value(&run.id).ok().as_ref() == Some(id));
            let stale = source.is_none_or(|source| {
                !(is_restart_continuation_source(source, &typed.provider_turns)
                    || continuation_failed_before_prompt(source, &typed.provider_turns))
                    || typed.runs.iter().any(|run| {
                        run.id != source.id
                            && run.status != OrchestrationV2RunStatus::Queued
                            && !(run.status == OrchestrationV2RunStatus::Cancelled
                                && run.started_at.is_none())
                            && crate::ports::decisions::model_switch::run_ran_after(run, source)
                    })
                    || thread["providerInstanceId"] != json!(source.provider_instance_id)
            }) || !thread["archivedAt"].is_null()
                || !thread["deletedAt"].is_null()
                || stopped(id);
            if stale {
                self.update_thread("thread.metadata-updated", thread)?;
                return Ok(false);
            }
        }
        if let Some(id) = self.command.get("usageLimitContinuationOfRunId") {
            let source = self.records("runs").iter().find(|run| run["id"] == *id);
            let failure = source.and_then(|run| self.root_failure(run));
            let recovery = thread["limitRecovery"].clone();
            let now = epoch_millis(&self.context.now.0).ok_or(Error::Stale)?;
            let snoozed = thread["snoozedUntil"]
                .as_str()
                .and_then(epoch_millis)
                .is_some_and(|until| until > now);
            let latest = crate::ports::decisions::model_switch::latest_executed_run(&typed.runs)
                .map(stored_value)
                .transpose()?;
            let stale = source.is_none()
                || latest.as_ref().is_none_or(|run| run["id"] != *id)
                || failure.is_none_or(|failure| failure["class"] != "usage_limit")
                || !self.usage_limit_holds_queue(&thread["providerInstanceId"], None)?
                || recovery["autoResume"] != true
                || recovery["requestId"] != self.command["usageLimitRecoveryRequestId"]
                || recovery["runId"] != *id
                || failure.is_none_or(|failure| recovery["resetAt"] != failure["resetAt"])
                || recovery["resetAt"]
                    .as_str()
                    .and_then(epoch_millis)
                    .is_none_or(|reset| reset > now)
                || !thread["archivedAt"].is_null()
                || !thread["deletedAt"].is_null()
                || thread["settledOverride"] == "settled"
                || source
                    .is_some_and(|run| run["providerInstanceId"] != thread["providerInstanceId"])
                || self
                    .records("runtimeRequests")
                    .iter()
                    .any(|request| request["status"] == "pending")
                || snoozed;
            if stale {
                let mut thread = thread;
                if recovery["autoResume"] == true
                    && recovery["requestId"] == self.command["usageLimitRecoveryRequestId"]
                    && recovery["runId"] == *id
                    && snoozed
                {
                    thread["limitRecovery"]["requestId"] = self.command["commandId"].clone();
                }
                self.update_thread("thread.metadata-updated", thread)?;
                return Ok(false);
            }
        }
        if self.command["idleCompaction"] == "beforeCacheExpires"
            && self.records("runs").iter().any(|run| {
                run["status"] == "queued"
                    || Self::active_run(run)
                    || run["delegatedCompletion"]["disposition"] == "open"
                        && !run["delegatedCompletion"]["delivery"].is_null()
                        && !self.records("messages").iter().any(|message| {
                            message["id"] == run["delegatedCompletion"]["delivery"]["messageId"]
                        })
            })
        {
            self.update_thread("thread.metadata-updated", thread)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn source_plan(&self) -> Result<Option<Value>> {
        let Some(reference) = self.command.get("sourcePlanRef") else {
            return Ok(None);
        };
        let projection = if reference["threadId"] == json!(self.thread_id) {
            Some(self.projection.clone())
        } else {
            self.inputs
                .and_then(|inputs| {
                    inputs.related_projections.iter().find(|projection| {
                        stored_value(&projection.thread.id).ok().as_ref()
                            == Some(&reference["threadId"])
                    })
                })
                .map(stored_value)
                .transpose()?
        };
        let plan = projection
            .as_ref()
            .and_then(|projection| projection["plans"].as_array())
            .and_then(|plans| {
                plans.iter().find(|plan| {
                    plan["id"] == reference["planId"] && plan["kind"] == "proposed_plan"
                })
            })
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Proposed plan {} does not exist on thread {}.",
                        reference["planId"].as_str().unwrap_or_default(),
                        reference["threadId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        if projection.as_ref().is_some_and(|projection| {
            projection["thread"]["projectId"] != self.projection["thread"]["projectId"]
        }) {
            return Err(rejection(
                &self.command,
                format!(
                    "Proposed plan {} belongs to a different project.",
                    reference["planId"].as_str().unwrap_or_default()
                ),
            ));
        }
        if plan["status"] != "active" {
            return Err(rejection(
                &self.command,
                format!(
                    "Proposed plan {} is not active.",
                    plan["id"].as_str().unwrap_or_default()
                ),
            ));
        }
        Ok(Some(plan))
    }

    fn complete_source_plan(&mut self, plan: Option<Value>) -> Result<()> {
        if let Some(mut plan) = plan {
            plan["status"] = json!("completed");
            let saved = self.thread_id.clone();
            self.thread_id = serde_json::from_value(plan["threadId"].clone())?;
            let scope = json!({"runId":plan["runId"],"nodeId":plan["nodeId"]});
            let result = self.emit("plan.updated", plan, Some(&scope));
            self.thread_id = saved;
            result?;
        }
        Ok(())
    }

    fn validate_steering_message(&self, message: &Value) -> Result<()> {
        let text = message["text"].as_str().unwrap_or_default();
        let count = message["attachments"].as_array().map_or(0, Vec::len);
        if crate::ports::decisions::command_policy::is_native_maintenance_command(text, count) {
            return Err(rejection(
                &self.command,
                if t3_contracts::wire::trim_js(text).eq_ignore_ascii_case("/compact") {
                    "Context compaction must run as a separate turn. Queue it or wait for the active turn to finish."
                } else {
                    "Signing out must run as a separate turn. Queue it or wait for the active turn to finish."
                },
            ));
        }
        if crate::ports::decisions::command_policy::goal_command_type(text, count).is_some() {
            return Err(rejection(
                &self.command,
                "Goal commands must run as a separate turn. Queue it or wait for the active turn to finish.",
            ));
        }
        Ok(())
    }

    fn validate_steering_capability(&self, instance: &Value) -> Result<()> {
        let capabilities = self
            .inputs
            .and_then(|inputs| {
                inputs
                    .provider_metadata
                    .iter()
                    .find(|(id, _)| json!(id) == *instance)
                    .map(|(_, metadata)| &metadata.capabilities)
            })
            .or(self.context.capabilities.as_ref())
            .ok_or(Error::Unsupported(
                "pre-start steering requires adapter capabilities",
            ))?;
        let command_id = serde_json::from_value(self.command["commandId"].clone())?;
        let instance = serde_json::from_value(instance.clone())?;
        crate::ports::decisions::command_policy::CapabilityCheck {
            command_id: &command_id,
            thread_id: &self.thread_id,
            provider_instance_id: &instance,
            capabilities,
        }
        .steering(false)
        .map_err(|error| rejection(&self.command, error.to_string()))?;
        Ok(())
    }

    fn native_goal(&self, message: &Value, instance: &Value) -> Result<Option<&'static str>> {
        let Some(goal) = crate::ports::decisions::command_policy::goal_command_type(
            message["text"].as_str().unwrap_or_default(),
            message["attachments"].as_array().map_or(0, Vec::len),
        ) else {
            return Ok(None);
        };
        let metadata = self
            .inputs
            .and_then(|inputs| {
                inputs
                    .provider_metadata
                    .iter()
                    .find(|(id, _)| json!(id) == *instance)
                    .map(|(_, metadata)| metadata)
            })
            .ok_or(Error::Unsupported(
                "goal validation requires selected adapter metadata",
            ))?;
        let driver = metadata.driver.0.as_str();
        let native = matches!(driver, "codex" | "claudeAgent")
            || matches!(driver, "opencode" | "antigravity")
                && metadata.capabilities.turns.supports_native_goals == Some(Some(true));
        if native && goal != "set" {
            if driver == "antigravity" {
                return Err(rejection(
                    &self.command,
                    "Antigravity accepts /goal followed by an objective. Interrupt the running turn, then ask Antigravity to stop the current goal.",
                ));
            }
            if driver == "claudeAgent" && matches!(goal, "pause" | "resume") {
                return Err(rejection(
                    &self.command,
                    "Claude goals cannot be paused or resumed. Send /goal clear to remove the goal.",
                ));
            }
        }
        Ok(native.then_some(goal))
    }

    fn stop_queued_command(
        &mut self,
        queued_id: &Value,
        target: Option<&Value>,
        reason: &str,
    ) -> Result<()> {
        let mut others: Vec<_> = self
            .records("runs")
            .iter()
            .filter(|run| {
                run["status"] == "queued"
                    && run["id"] != *queued_id
                    && !self.records("messages").iter().any(|message| {
                        message["id"] == run["userMessageId"]
                            && (!message["delegatedCompletion"].is_null()
                                || !message["notification"].is_null())
                    })
            })
            .cloned()
            .collect();
        others.sort_by_key(|run| {
            run["queuePosition"]
                .as_u64()
                .or_else(|| run["ordinal"].as_u64())
        });
        for (index, mut run) in others.into_iter().enumerate() {
            if run["queuePosition"] != json!(index + 2) {
                run["queuePosition"] = json!(index + 2);
                let scope = Self::run_scope(&run);
                self.write_record("runs", "run.updated", run, &scope)?;
            }
        }
        if let Some(target) = target {
            let command = self.command.clone();
            let projection = self.projection.clone();
            let decision = self.decision.clone();
            let ids = self.id_positions.clone();
            self.command = json!({"type":"run.interrupt","commandId":command["commandId"],"threadId":self.thread_id,"runId":target,"reason":reason});
            if self.interrupt().is_err() {
                self.projection = projection;
                self.decision = decision;
                self.id_positions = ids;
            }
            self.command = command;
        }
        Ok(())
    }

    fn message(&mut self) -> Result<()> {
        self.message_inner()?;
        self.record_subagent_followup()
    }

    fn record_subagent_followup(&mut self) -> Result<()> {
        if !self.command["scheduledTaskId"].is_null()
            || !self.command["restartContinuationOfRunId"].is_null()
        {
            return Ok(());
        }
        let starts = if !self.command["notification"].is_null() {
            self.command["notification"]["source"]["kind"] != "monitor"
        } else {
            self.command["createdBy"] == "user"
                || self.command["creationSource"] == "mcp"
                || !self.command["usageLimitContinuationOfRunId"].is_null()
        };
        if !starts {
            return Ok(());
        }
        let child = self.thread()?;
        let parent_id = child["lineage"]["parentThreadId"].clone();
        if child["lineage"]["relationshipToParent"] != "subagent"
            || parent_id.is_null()
            || self.command["notification"].is_null()
                && self.command["createdBy"] != "user"
                && self.command["usageLimitContinuationOfRunId"].is_null()
                && !self.command["senderThreadId"].is_null()
                && self.command["senderThreadId"] != parent_id
        {
            return Ok(());
        }
        let events = self
            .decision
            .events
            .iter()
            .map(stored_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let Some(message) = events
            .iter()
            .rev()
            .find(|event| {
                event["type"] == "message.updated"
                    && event["payload"]["id"] == self.command["messageId"]
            })
            .map(|event| &event["payload"])
        else {
            return Ok(());
        };
        if message["runId"].is_null()
            || !events.iter().any(|event| {
                event["type"] == "run.created" && event["payload"]["id"] == message["runId"]
                    || event["type"] == "run-attempt.created"
                        && event["payload"]["runId"] == message["runId"]
            })
        {
            return Ok(());
        }
        let Some(mut run) = events
            .iter()
            .rev()
            .find(|event| {
                matches!(event["type"].as_str(), Some("run.created" | "run.updated"))
                    && event["payload"]["id"] == message["runId"]
            })
            .map(|event| event["payload"].clone())
            .or_else(|| {
                self.records("runs")
                    .iter()
                    .find(|run| run["id"] == message["runId"])
                    .cloned()
            })
        else {
            return Ok(());
        };
        if !run["delegatedTaskId"].is_null() {
            return Ok(());
        }
        let parent = self
            .inputs
            .and_then(|inputs| {
                inputs.related_projections.iter().find(|projection| {
                    stored_value(&projection.thread.id).ok().as_ref() == Some(&parent_id)
                })
            })
            .ok_or(Error::Unsupported("follow-up requires its parent controls"))?;
        let parent_value = stored_value(parent)?;
        let Some(original) = parent_value["subagents"].as_array().and_then(|tasks| {
            tasks.iter().find(|task| {
                task["origin"] == "app_owned"
                    && task["childThreadId"] == child["id"]
                    && task["id"] == child["forkedFrom"]["nodeId"]
                    && !task["result"].is_null()
            })
        }) else {
            return Ok(());
        };
        let Some(owner) =
            crate::ports::decisions::command_policy::follow_up_owner_run(&parent.runs)
        else {
            return Ok(());
        };
        if owner.root_node_id.is_none() {
            return Ok(());
        }
        let owner = stored_value(owner)?;
        let metadata = self
            .inputs
            .and_then(|inputs| {
                inputs
                    .provider_metadata
                    .iter()
                    .find(|(id, _)| json!(id) == run["providerInstanceId"])
                    .map(|(_, metadata)| metadata)
            })
            .ok_or(Error::Unsupported(
                "follow-up requires selected adapter metadata",
            ))?;
        let driver = json!(metadata.driver);
        let now = message["createdAt"].clone();
        let command_id = self.command["commandId"].as_str().ok_or(Error::Stale)?;
        let task_id = crate::ports::decisions::orchestrator::derived_id(
            "node",
            &["delegated-task", command_id],
        );
        let item_id = crate::ports::decisions::orchestrator::derived_id(
            "turn-item",
            &["delegated-task", command_id],
        );
        let status = if run["status"] == "queued" {
            if run["queueHeld"] == true {
                "idle"
            } else {
                "pending"
            }
        } else {
            "running"
        };
        let mut task = original.clone();
        for field in ["progress", "cancelledRunId"] {
            task.as_object_mut()
                .ok_or(Error::Stale)?
                .shift_remove(field);
        }
        for (field, value) in [
            ("id", json!(task_id)),
            ("createdBy", self.command["createdBy"].clone()),
            ("driver", driver.clone()),
            ("runId", owner["id"].clone()),
            ("parentNodeId", owner["rootNodeId"].clone()),
            ("providerInstanceId", run["providerInstanceId"].clone()),
            ("providerThreadId", run["providerThreadId"].clone()),
            ("model", run["modelSelection"]["model"].clone()),
            ("modelSelection", run["modelSelection"].clone()),
            ("prompt", self.command["text"].clone()),
            ("title", child["title"].clone()),
            ("completionWake", json!("always")),
            (
                "completionDelivery",
                json!({"state":"pending","observedByRunId":null}),
            ),
            ("status", json!(status)),
            ("result", Value::Null),
            ("startedAt", now.clone()),
            ("completedAt", Value::Null),
            ("updatedAt", now.clone()),
        ] {
            task[field] = value;
        }
        let node = json!({"id":task_id,"threadId":parent_id,"runId":owner["id"],"parentNodeId":owner["rootNodeId"],"rootNodeId":owner["rootNodeId"],"kind":"subagent","status":status,"countsForRun":false,"providerThreadId":run["providerThreadId"],"providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":now,"completedAt":null});
        let ordinal = self
            .inputs
            .and_then(|inputs| {
                inputs
                    .next_item_ordinals
                    .iter()
                    .find(|(thread, run, _)| json!(thread) == parent_id && run.is_none())
                    .map(|(_, _, ordinal)| *ordinal)
            })
            .ok_or(Error::Unsupported("follow-up requires parent item ordinal"))?;
        let turn = self.provider_turn_for_run(&parent_value, &owner);
        let item = json!({"id":item_id,"threadId":parent_id,"runId":owner["id"],"nodeId":task_id,"providerThreadId":owner["providerThreadId"],"providerTurnId":turn.map_or(Value::Null,|turn|turn["id"].clone()),"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":status,"title":child["title"],"startedAt":now,"completedAt":null,"updatedAt":now,"type":"subagent","subagentId":task_id,"origin":"app_owned","driver":driver,"providerInstanceId":run["providerInstanceId"],"childThreadId":child["id"],"prompt":self.command["text"],"result":null});
        run["delegatedTaskId"] = json!(task_id);
        self.emit(
            "run.updated",
            run.clone(),
            Some(&json!({"runId":run["id"]})),
        )?;
        let saved = self.thread_id.clone();
        self.thread_id = serde_json::from_value(parent_id)?;
        let scope = json!({"runId":owner["id"],"nodeId":task_id});
        let result = (|| {
            self.emit("node.updated", node, Some(&scope))?;
            self.emit("subagent.updated", task, Some(&scope))?;
            self.emit("turn-item.updated", item, Some(&scope))
        })();
        self.thread_id = saved;
        result
    }

    fn message_inner(&mut self) -> Result<()> {
        use crate::ports::decisions::command_policy::{
            is_native_maintenance_command, resolve_message_dispatch_intent,
        };
        let mut thread = self.thread()?;
        if thread["lineage"]["relationshipToParent"] == "subagent"
            && thread["creationSource"] == "provider"
        {
            return Err(rejection(
                &self.command,
                "This subagent is run by its provider and cannot take messages. Message the parent thread instead.",
            ));
        }
        if let Some(id) = self.command.get("guestMessageId").cloned() {
            let sent = guest_share_decider::decide_guest_message_send(
                &serde_json::from_value(thread.clone())?,
                &serde_json::from_value(id)?,
                &self.context.now,
            )
            .map_err(|reason| rejection(&self.command, reason))?;
            thread = stored_value(&sent.thread)?;
            self.update_thread("thread.guest-share-updated", thread.clone())?;
            self.command["text"] = json!(sent.text);
            self.command["context"] = json!({"version":1,"records":[],"guest":sent.guest});
        } else if let Some(context) = self
            .command
            .get_mut("context")
            .and_then(Value::as_object_mut)
        {
            context.shift_remove("guest");
        }
        let projection = self.context.projection.as_ref().ok_or(Error::Stale)?;
        let requested = serde_json::from_value(self.command["dispatchMode"].clone())?;
        let resolved = resolve_message_dispatch_intent(
            projection,
            requested,
            self.command["deliveryIntent"].as_str(),
        );
        let mut mode = stored_value(resolved)?;
        if matches!(
            mode["type"].as_str(),
            Some("steer_active" | "restart_active")
        ) {
            let target = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == mode["targetRunId"]);
            let late = target.is_some_and(|run| {
                run["status"] == "completed"
                    || (mode["type"] == "restart_active" && !Self::active_run(run))
                    || (matches!(run["status"].as_str(), Some("running" | "waiting"))
                        && self
                            .provider_turn_for_run(&self.projection, run)
                            .is_some_and(|turn| turn["status"] == "completed"))
            });
            if late {
                mode = json!({"type":"start_immediately"});
            }
        }
        if self.command["freshContext"] == true
            && matches!(
                mode["type"].as_str(),
                Some("steer_active" | "restart_active")
            )
        {
            mode = json!({"type":"queue_after_active"});
        }
        let instance = self
            .command
            .get("modelSelection")
            .unwrap_or(&thread["modelSelection"])["instanceId"]
            .clone();
        let goal = self.native_goal(&self.command, &instance)?;
        let mut queued_stop = None;
        let mut steer_when_ready = None;
        if goal.is_some()
            && matches!(
                mode["type"].as_str(),
                Some("steer_active" | "restart_active" | "queue_after_active")
            )
        {
            let target = if mode["type"] == "queue_after_active" {
                if self.command["deliveryIntent"].is_null() {
                    None
                } else {
                    self.records("runs").iter().rev().find(|run| {
                        matches!(
                            run["status"].as_str(),
                            Some("preparing" | "starting" | "running")
                        )
                    })
                }
            } else {
                self.records("runs")
                    .iter()
                    .find(|run| run["id"] == mode["targetRunId"])
            };
            if let Some(target) = target {
                let status = self
                    .records("providerThreads")
                    .iter()
                    .find(|owner| owner["id"] == target["providerThreadId"])
                    .and_then(|owner| owner["goal"]["status"].as_str());
                let maintenance = self
                    .records("messages")
                    .iter()
                    .find(|message| message["id"] == target["userMessageId"])
                    .is_some_and(|message| {
                        is_native_maintenance_command(
                            message["text"].as_str().unwrap_or_default(),
                            message["attachments"].as_array().map_or(0, Vec::len),
                        )
                    });
                if matches!(
                    target["status"].as_str(),
                    Some("preparing" | "starting" | "running")
                ) && crate::ports::decisions::command_policy::goal_command_stops_active_run(
                    goal.unwrap_or_default(),
                    status,
                ) && !maintenance
                    && !self.records("turnItems").iter().any(|item| {
                        item["type"] == "run_interrupt_request" && item["runId"] == target["id"]
                    })
                {
                    queued_stop = Some((target["id"].clone(), "Stopped to apply /goal", false));
                }
                mode = json!({"type":"queue_after_active"});
            }
        }
        if mode["type"] == "restart_active"
            && let Some(target) = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == mode["targetRunId"] && run["status"] == "running")
        {
            let session = self
                .records("providerThreads")
                .iter()
                .find(|owner| owner["id"] == target["providerThreadId"])
                .and_then(|owner| {
                    self.records("providerSessions")
                        .iter()
                        .find(|session| session["id"] == owner["providerSessionId"])
                });
            if session.is_some_and(|session| {
                session["capabilities"]["turns"]["supportsInterrupt"] == true
                    && session["capabilities"]["turns"]["supportsQueuedMessages"] == true
                    && session["capabilities"]["turns"]["supportsSteeringByInterruptRestart"]
                        != true
            }) && !is_native_maintenance_command(
                self.command["text"].as_str().unwrap_or_default(),
                self.command["attachments"].as_array().map_or(0, Vec::len),
            ) && !self
                .records("messages")
                .iter()
                .find(|message| message["id"] == target["userMessageId"])
                .is_some_and(|message| {
                    is_native_maintenance_command(
                        message["text"].as_str().unwrap_or_default(),
                        message["attachments"].as_array().map_or(0, Vec::len),
                    )
                })
                && !self.records("turnItems").iter().any(|item| {
                    item["type"] == "run_interrupt_request" && item["runId"] == target["id"]
                })
            {
                queued_stop = Some((target["id"].clone(), "Stopped to send a new message", true));
                mode = json!({"type":"queue_after_active"});
            }
        }
        if mode["type"] == "steer_active"
            && let Some(target) = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == mode["targetRunId"])
            && (matches!(target["status"].as_str(), Some("preparing" | "starting"))
                || target["status"] == "running"
                    && !self.records("providerTurns").iter().any(|turn| {
                        turn["runAttemptId"] == target["activeAttemptId"]
                            && turn["status"] == "running"
                    }))
        {
            self.validate_steering_message(&self.command)?;
            self.validate_steering_capability(&target["providerInstanceId"])?;
            steer_when_ready = Some(target["id"].clone());
            mode = json!({"type":"queue_after_active"});
        }
        if !self.command["notification"].is_null()
            && (self.command["createdBy"] != "agent"
                || !matches!(
                    self.command["creationSource"].as_str(),
                    Some("server" | "provider")
                )
                || (mode["type"] != "queue_after_active"
                    && !(mode["type"] == "start_immediately"
                        && !self.command["restartContinuationOfRunId"].is_null())))
        {
            return Err(rejection(
                &self.command,
                "Notifications must be server- or provider-created queued messages.",
            ));
        }
        if !self.command["delegatedCompletion"].is_null() {
            if self.command["createdBy"] != "agent"
                || self.command["creationSource"] != "server"
                || mode["type"] != "queue_after_active"
            {
                return Err(rejection(
                    &self.command,
                    "Delegated completion delivery must be a server-created queued message.",
                ));
            }
            let ownership = self.command["delegatedCompletion"].clone();
            let parent = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == ownership["parentRunId"]);
            let delivery = parent
                .map(|run| run["delegatedCompletion"]["delivery"].clone())
                .unwrap_or(Value::Null);
            let typed = self.context.projection.as_ref().ok_or(Error::Stale)?;
            let id: MessageId = serde_json::from_value(self.command["messageId"].clone())?;
            if parent.is_none_or(|run|run["delegatedCompletion"]["disposition"]!="open") || delivery.is_null() || delivery["generation"]!=ownership["generation"] || delivery["messageId"]!=self.command["messageId"] || self.records("messages").iter().any(|message|message["id"]==self.command["messageId"]) && !crate::ports::runs_ingestion::notification_mailbox::is_undelivered_mailbox_steer(&typed.messages,&typed.runs,&typed.provider_turns,&id){return Err(rejection(&self.command,"Delegated completion delivery is no longer dispatchable."));}
            self.command["delegatedCompletion"] = json!({"parentRunId":ownership["parentRunId"],"generation":delivery["generation"],"taskIds":delivery["taskIds"]});
            self.command["text"] = json!(
                self.completion_detail(delivery["taskIds"].as_array().ok_or(Error::Stale)?, None)
            );
        }
        let command_notice = self.command["notification"]["source"]["kind"] == "command"
            && self.command["creationSource"] == "server";
        let may_steer = if !self.command["delegatedCompletion"].is_null() {
            self.command["delegatedCompletion"]["taskIds"]
                .as_array()
                .is_some_and(|ids| {
                    ids.iter().all(|id| {
                        self.records("subagents")
                            .iter()
                            .any(|task| task["id"] == *id && task["completionWake"] == "always")
                    })
                })
        } else {
            self.command["notification"]["source"]["kind"] == "delegated_task" || command_notice
        };
        if self.command["freshContext"] != true
            && may_steer
            && let Some(active) = self
                .records("runs")
                .iter()
                .find(|run| run["status"] == "running")
        {
            let owner = self
                .records("providerThreads")
                .iter()
                .find(|owner| owner["id"] == active["providerThreadId"]);
            let turn = self.records("providerTurns").iter().any(|turn| {
                turn["runAttemptId"] == active["activeAttemptId"] && turn["status"] == "running"
            });
            let maintenance = self
                .records("messages")
                .iter()
                .find(|message| message["id"] == active["userMessageId"])
                .is_some_and(|message| {
                    is_native_maintenance_command(
                        message["text"].as_str().unwrap_or_default(),
                        message["attachments"].as_array().map_or(0, Vec::len),
                    )
                });
            if let Some(owner) = owner
                && turn
                && (!command_notice || owner["goal"]["status"] == "active")
                && !maintenance
                && !owner["providerSessionId"].is_null()
                && self.session_present(&owner["providerSessionId"])?
                && self.records("providerSessions").iter().any(|session| {
                    session["id"] == owner["providerSessionId"]
                        && session["capabilities"]["turns"]["supportsActiveSteering"] == true
                        && session["capabilities"]["turns"]["activeSteeringInterruptsTools"] != true
                })
                && !self.records("turnItems").iter().any(|item| {
                    item["type"] == "run_interrupt_request" && item["runId"] == active["id"]
                })
            {
                mode = json!({"type":"steer_active","targetRunId":active["id"]});
            }
        }
        if !self.admit_continuation(&mode)? {
            return Ok(());
        }
        let maintenance = is_native_maintenance_command(
            self.command["text"].as_str().unwrap_or_default(),
            self.command["attachments"].as_array().map_or(0, Vec::len),
        );
        if thread["lastVisitedAt"].is_null() && !maintenance {
            thread["lastVisitedAt"] = json!(iso_from_epoch_millis(
                epoch_millis(&self.context.now.0).ok_or(Error::Stale)? - 1
            ));
            self.update_thread("thread.metadata-updated", thread.clone())?;
        }
        if !thread["settledOverride"].is_null() {
            if thread["settledOverride"] != "active" {
                thread["unsettledAt"] = json!(self.context.now);
            }
            thread["settledOverride"] = Value::Null;
            thread["settledAt"] = Value::Null;
            thread["updatedAt"] = json!(self.context.now);
            self.update_thread("thread.unsettled", thread.clone())?;
        }
        if !thread["snoozedUntil"].is_null()
            && self.command["notification"].is_null()
            && self.command["delegatedCompletion"].is_null()
        {
            thread["snoozedUntil"] = Value::Null;
            thread["snoozedAt"] = Value::Null;
            thread["updatedAt"] = json!(self.context.now);
            self.update_thread("thread.unsnoozed", thread.clone())?;
        }
        let source_plan = self.source_plan()?;
        let mut selection = self
            .command
            .get("modelSelection")
            .cloned()
            .unwrap_or_else(|| thread["modelSelection"].clone());
        if matches!(
            mode["type"].as_str(),
            Some("steer_active" | "restart_active")
        ) {
            if (!self.command["delegatedCompletion"].is_null()
                || !self.command["notification"].is_null())
                && let Some(target) = self
                    .records("runs")
                    .iter()
                    .find(|run| run["id"] == mode["targetRunId"])
            {
                selection = target["modelSelection"].clone();
            }
            self.steer_message(&mode, &selection)?;
            return self.complete_source_plan(source_plan);
        }
        let active = self
            .records("runs")
            .iter()
            .find(|run| Self::active_run(run))
            .cloned();
        let missing_worktree = if active.is_none() && thread["worktreePath"].is_null() {
            crate::ports::decisions::model_switch::latest_executed_run(&projection.runs)
                .and_then(|run| stored_value(run).ok())
                .filter(|run| {
                    matches!(run["status"].as_str(), Some("failed" | "interrupted"))
                        && run["workspacePreparation"]["type"] == "worktree"
                })
        } else {
            None
        };
        let behind = active.or(missing_worktree.clone());
        let queued = behind.is_some();
        let fresh = self.command["freshContext"] == true;
        if queued
            && self.command.get("modelSelection").is_none()
            && self.command["notification"].is_null()
            && let Some(tail) = queued_runs_in_delivery_order(projection).last()
        {
            selection = stored_value(&tail.model_selection)?;
        }
        let instance: ProviderInstanceId = serde_json::from_value(selection["instanceId"].clone())?;
        let selected = self
            .inputs
            .and_then(|inputs| {
                inputs
                    .provider_metadata
                    .iter()
                    .find(|(id, _)| id == &instance)
                    .map(|(_, metadata)| metadata)
            })
            .ok_or(Error::Unsupported(
                "message dispatch requires selected adapter metadata",
            ))?;
        let capabilities = &selected.capabilities;
        let driver = selected.driver.0.clone();
        if queued && !capabilities.turns.supports_queued_messages {
            return Err(rejection(
                &self.command,
                format!(
                    "{} cannot satisfy queued_messages for command {}: providerInstanceId does not support app-owned queued turns",
                    instance.0,
                    self.command["commandId"].as_str().unwrap_or_default()
                ),
            ));
        }
        let pending_context = self.records("contextTransfers").iter().any(|transfer| {
            transfer["status"] == "pending"
                && matches!(transfer["type"].as_str(), Some("fork" | "merge_back"))
        });
        if fresh && pending_context {
            return Err(rejection(
                &self.command,
                "A fresh session can't start while a fork or merge is still being applied.",
            ));
        }
        let legacy_context = self.projection["thread"]["historyOrigin"] == "v1_import"
            && !self
                .records("runs")
                .iter()
                .any(|run| run["status"] == "completed")
            && !self.records("turnItems").is_empty()
            && !self
                .records("providerThreads")
                .iter()
                .any(|provider| !provider["nativeThreadRef"].is_null());
        let mut existing = if fresh {
            None
        } else {
            self.records("providerThreads")
                .iter()
                .find(|provider| {
                    provider["id"] == thread["activeProviderThreadId"]
                        && provider["providerInstanceId"] == selection["instanceId"]
                })
                .or_else(|| {
                    self.records("providerThreads")
                        .iter()
                        .rev()
                        .find(|provider| {
                            provider["providerInstanceId"] == selection["instanceId"]
                                && provider["ownerNodeId"].is_null()
                        })
                })
                .cloned()
        };
        let changed_provider = self
            .records("providerThreads")
            .iter()
            .find(|provider| provider["id"] == thread["activeProviderThreadId"])
            .is_some_and(|provider| provider["providerInstanceId"] != selection["instanceId"]);
        let prepared_context = if !queued
            && !fresh
            && (pending_context
                || legacy_context
                || changed_provider
                || !thread["activeProviderThreadId"].is_null() && existing.is_none())
        {
            Some(prepared_message_context(self.inputs)?.clone())
        } else {
            None
        };
        if let Some(context) = &prepared_context {
            existing = Some(stored_value(&context.provider_thread)?);
        }
        let sessions_to_detach = if fresh && !queued {
            self.fresh_sessions()?
        } else {
            Vec::new()
        };
        let ordinal = self
            .records("runs")
            .iter()
            .filter_map(|run| run["ordinal"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        let run_id = crate::ports::decisions::orchestrator::derived_id(
            "run",
            &["thread", &self.thread_id.0, "ordinal", &ordinal.to_string()],
        );
        let attempt_id = crate::ports::decisions::orchestrator::derived_id(
            "run-attempt",
            &["run", &run_id, "attempt", "1"],
        );
        let node_id =
            crate::ports::decisions::orchestrator::derived_id("node", &["run", &run_id, "root"]);
        let provider_thread_id = existing
            .as_ref()
            .map(|provider| {
                provider["id"]
                    .as_str()
                    .ok_or(Error::Stale)
                    .map(str::to_owned)
            })
            .transpose()?
            .unwrap_or_else(|| {
                crate::ports::decisions::orchestrator::derived_id(
                    "provider-thread",
                    &[
                        "provider",
                        &driver,
                        "native-thread",
                        &format!("pending:{run_id}"),
                    ],
                )
            });
        let session_id = if queued {
            Value::Null
        } else if let Some(id) = existing
            .as_ref()
            .and_then(|provider| provider.get("providerSessionId"))
            .filter(|id| !id.is_null())
        {
            id.clone()
        } else if capabilities
            .sessions
            .supports_multiple_provider_threads_per_session
        {
            json!(crate::ports::decisions::orchestrator::derived_id(
                "provider-session",
                &[
                    "provider-instance",
                    selection["instanceId"].as_str().ok_or(Error::Stale)?,
                    "shared"
                ]
            ))
        } else {
            json!(self.allocate(IdKind::ProviderSession)?)
        };
        let mut provider = existing.clone().unwrap_or_else(|| json!({"id":provider_thread_id,"driver":driver,"providerInstanceId":selection["instanceId"],"providerSessionId":session_id,"appThreadId":self.thread_id,"ownerNodeId":null,"nativeThreadRef":null,"nativeConversationHeadRef":null,"status":"not_loaded","firstRunOrdinal":if queued {Value::Null} else {json!(ordinal)},"lastRunOrdinal":if queued {Value::Null} else {json!(ordinal)},"handoffIds":[],"forkedFrom":null,"createdAt":self.context.now,"updatedAt":self.context.now}));
        if !queued {
            provider["providerSessionId"] = session_id;
            if provider["firstRunOrdinal"].is_null() {
                provider["firstRunOrdinal"] = json!(ordinal);
            }
            provider["lastRunOrdinal"] = json!(ordinal);
            provider["updatedAt"] = json!(self.context.now);
        }
        let scope =
            json!({"runId":run_id,"nodeId":node_id,"providerInstanceId":selection["instanceId"]});
        let deferred = !queued && mode["type"] == "defer_start";
        let mut run = json!({"id":run_id,"threadId":self.thread_id,"ordinal":ordinal,"providerInstanceId":selection["instanceId"],"modelSelection":selection,"providerThreadId":provider_thread_id,"userMessageId":self.command["messageId"],"rootNodeId":node_id,"activeAttemptId":attempt_id,"status":if queued {"queued"} else if deferred {"preparing"} else {"starting"},"queuePosition":if queued {json!(self.records("runs").iter().filter(|run| run["status"] == "queued").map(|run| run["queuePosition"].as_u64().or_else(|| run["ordinal"].as_u64()).unwrap_or(0)).max().unwrap_or(0)+1)} else {Value::Null},"requestedAt":self.context.now,"startedAt":null,"completedAt":null,"checkpointId":null,"contextHandoffId":null});
        if let Some(handoff) = prepared_context
            .as_ref()
            .and_then(|context| context.handoff.as_ref())
        {
            run["contextHandoffId"] = stored_value(&handoff.id)?;
        }
        if !queued
            && let Some(at) = crate::ports::decisions::orchestrator::wake_work_started_at(
                projection,
                &self.command,
                "immediate",
            )?
        {
            run["workStartedAt"] = json!(at);
        }
        for field in [
            "restartContinuationOfRunId",
            "usageLimitContinuationOfRunId",
        ] {
            if let Some(value) = self.command.get(field) {
                run[field] = value.clone();
            }
        }
        if let Some(task) = self
            .records("runs")
            .iter()
            .find(|run| {
                if self.command["restartContinuationOfRunId"].is_null() {
                    self.command["senderThreadId"] == json!(self.thread_id) && Self::active_run(run)
                } else {
                    run["id"] == self.command["restartContinuationOfRunId"]
                }
            })
            .and_then(|run| run.get("delegatedTaskId"))
        {
            run["delegatedTaskId"] = task.clone();
        }
        let sent_from_client = self.command["createdBy"] == "user"
            && matches!(
                self.command["creationSource"].as_str(),
                Some("web" | "mobile")
            )
            && self.command["scheduledTaskId"].is_null();
        let setup_pending = behind
            .as_ref()
            .is_some_and(|run| run["status"] == "preparing")
            || missing_worktree.is_some();
        let held: Vec<_> = self
            .records("runs")
            .iter()
            .filter(|run| run["status"] == "queued" && run["queueHeld"] == true)
            .cloned()
            .collect();
        if sent_from_client && !setup_pending {
            for mut held in held.clone() {
                let scope = Self::run_scope(&held);
                held["queueHeld"] = json!(false);
                self.emit("run.updated", held, Some(&scope))?;
            }
        }
        if queued
            && (missing_worktree.is_some()
                || (!held.is_empty() && !(sent_from_client && !setup_pending)))
        {
            run["queueHeld"] = json!(true);
        }
        if let Some(id) = steer_when_ready {
            run["steerWhenReadyRunId"] = id;
        }
        if let Some((_, _, skips_hold)) = &queued_stop {
            run["queuePosition"] = json!(1);
            if *skips_hold && missing_worktree.is_none() {
                run.as_object_mut()
                    .ok_or(Error::Stale)?
                    .shift_remove("queueHeld");
            }
        }
        if fresh {
            run["freshContext"] = json!(true);
        }
        if deferred && let Some(strategy) = mode.get("workspaceStrategy") {
            run["workspacePreparation"] = strategy.clone();
        }
        if let Some(source_plan) = self.command.get("sourcePlanRef") {
            run["sourcePlanRef"] = source_plan.clone();
        }
        let mut node = json!({"id":node_id,"threadId":self.thread_id,"runId":run_id,"parentNodeId":null,"rootNodeId":node_id,"kind":"root_turn","status":"pending","countsForRun":true,"providerThreadId":provider_thread_id,"providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":null,"completedAt":null});
        let checkpoint_scope = if deferred
            || behind
                .as_ref()
                .is_some_and(|run| run["status"] == "preparing")
            || missing_worktree.is_some()
        {
            None
        } else {
            Some(self.root_scope(&run, &node, &provider)?)
        };
        if let Some(checkpoint) = &checkpoint_scope {
            node["checkpointScopeId"] = checkpoint["id"].clone();
        }
        if !queued
            && (thread["modelSelection"] != selection
                || thread["providerInstanceId"] != selection["instanceId"])
        {
            let event = if thread["providerInstanceId"] == selection["instanceId"] {
                "thread.model-selection-updated"
            } else {
                "thread.provider-switched"
            };
            thread["providerInstanceId"] = selection["instanceId"].clone();
            thread["modelSelection"] = selection.clone();
            thread["updatedAt"] = json!(self.context.now);
            self.update_thread(event, thread.clone())?;
        }
        if !maintenance
            && !self.command["titleSeed"].is_null()
            && self.records("messages").is_empty()
        {
            thread["title"] = self.command["titleSeed"].clone();
            thread["titleRegeneration"] =
                json!({"requestId":self.command["commandId"],"startedAt":self.context.now});
            thread["updatedAt"] = json!(self.context.now);
            self.update_thread("thread.metadata-updated", thread)?;
            self.effect(
                "thread-title.generate",
                EffectRequest::GenerateTitle {
                    kind: TitleKind::Initial {
                        message_id: serde_json::from_value(self.command["messageId"].clone())?,
                    },
                },
            );
        }
        if let Some(context) = &prepared_context {
            provider = self.emit_message_context(context, &run, provider)?;
        }
        if existing.is_none() || !queued {
            let driver_scope =
                json!({"driver":driver,"providerInstanceId":selection["instanceId"]});
            self.emit("provider-thread.updated", provider, Some(&driver_scope))?;
        }
        self.emit("run.created", run, Some(&scope))?;
        self.emit("run-attempt.created", json!({"id":attempt_id,"runId":run_id,"attemptOrdinal":1,"rootNodeId":node_id,"providerInstanceId":selection["instanceId"],"providerThreadId":provider_thread_id,"providerTurnId":null,"reason":"initial","status":"pending","startedAt":null,"completedAt":null}), Some(&scope))?;
        self.emit("node.updated", node, Some(&scope))?;
        if let Some(checkpoint) = checkpoint_scope {
            self.emit("checkpoint-scope.created", checkpoint, Some(&scope))?;
        }
        self.append_message(
            MessageTarget {
                run_id: &run_id,
                node_id: &node_id,
                provider_thread_id: &provider_thread_id,
                provider_turn_id: Value::Null,
                scope: &scope,
                model_selection: &selection,
                handoff: prepared_context
                    .as_ref()
                    .is_some_and(|context| context.handoff.is_some()),
            },
            if queued { "queued_turn" } else { "turn_start" },
            !queued,
        )?;
        if deferred {
            let id = crate::ports::decisions::orchestrator::derived_id(
                "turn-item",
                &[
                    "provider",
                    &driver,
                    "native-item",
                    &format!("workspace-preparation:{run_id}"),
                ],
            );
            self.emit("turn-item.updated", json!({"id":id,"threadId":self.thread_id,"runId":run_id,"nodeId":node_id,"providerThreadId":provider_thread_id,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal()+1,"status":"running","title":"Preparing workspace","startedAt":self.context.now,"completedAt":null,"updatedAt":self.context.now,"type":"command_execution","input":"Preparing workspace"}), Some(&scope))?;
        }
        if let Some((target, reason, _)) = &queued_stop {
            self.stop_queued_command(&json!(run_id), Some(target), reason)?;
        }
        self.complete_source_plan(source_plan)?;
        self.detach_fresh_sessions(sessions_to_detach)?;
        if !queued && !deferred {
            self.effect(
                &format!("provider-turn.start:{run_id}"),
                EffectRequest::Start {
                    run_id: RunId(run_id),
                },
            );
        }
        Ok(())
    }

    fn append_message(
        &mut self,
        target: MessageTarget<'_>,
        intent: &str,
        create_item: bool,
    ) -> Result<()> {
        let MessageTarget {
            run_id,
            node_id,
            provider_thread_id,
            provider_turn_id,
            scope,
            model_selection,
            handoff,
        } = target;
        let mut message = json!({"id":self.command["messageId"],"threadId":self.thread_id,"runId":run_id,"nodeId":node_id,"role":"user","text":self.command["text"],"attachments":self.command["attachments"],"streaming":false,"createdBy":self.command["createdBy"],"creationSource":self.command["creationSource"],"createdAt":self.context.now,"updatedAt":self.context.now});
        for field in [
            "scheduledTaskId",
            "senderThreadId",
            "linkedSender",
            "idleCompaction",
            "context",
            "notification",
            "delegatedCompletion",
        ] {
            if let Some(value) = self.command.get(field) {
                message[field] = value.clone();
            }
        }
        self.emit("message.updated", message.clone(), Some(scope))?;
        if create_item {
            let id = crate::ports::decisions::orchestrator::derived_id(
                "turn-item",
                &[
                    "message",
                    self.command["messageId"].as_str().ok_or(Error::Stale)?,
                ],
            );
            let mut item = json!({"id":id,"threadId":self.thread_id,"runId":run_id,"nodeId":node_id,"providerThreadId":provider_thread_id,"providerTurnId":provider_turn_id,"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal(),"status":"completed","title":null,"startedAt":self.context.now,"completedAt":self.context.now,"updatedAt":self.context.now,"type":"user_message","messageId":self.command["messageId"],"inputIntent":intent,"text":self.command["text"],"attachments":self.command["attachments"],"createdBy":self.command["createdBy"],"creationSource":self.command["creationSource"]});
            for field in [
                "scheduledTaskId",
                "senderThreadId",
                "linkedSender",
                "idleCompaction",
                "context",
                "freshContext",
            ] {
                if let Some(value) = self.command.get(field) {
                    item[field] = value.clone();
                }
            }
            if let Some(projection) = &self.context.projection
                && !handoff
                && let Some(switch) = crate::ports::decisions::model_switch::model_switch_since(
                    &projection.runs,
                    &projection.turn_items,
                    &serde_json::from_value(model_selection.clone())?,
                    None,
                )
            {
                item["modelSwitch"] = stored_value(switch)?;
            }
            self.emit("turn-item.updated", item, Some(scope))?;
        }
        Ok(())
    }

    fn steer_message(&mut self, mode: &Value, selection: &Value) -> Result<()> {
        use crate::ports::decisions::{
            command_policy::{CapabilityCheck, SteeringExecution, is_native_maintenance_command},
            model_switch::model_selections_equal,
        };
        use crate::provider::SelectionTransitionPlan;
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == mode["targetRunId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Target run {} was not found.",
                        mode["targetRunId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        self.validate_steering_message(&self.command)?;
        let attachment_count = self.command["attachments"].as_array().map_or(0, Vec::len);
        if let Some(message) = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == run["userMessageId"])
            && is_native_maintenance_command(
                message["text"].as_str().unwrap_or_default(),
                message["attachments"].as_array().map_or(0, Vec::len),
            )
        {
            return Err(rejection(
                &self.command,
                if t3_contracts::wire::trim_js(message["text"].as_str().unwrap_or_default())
                    .eq_ignore_ascii_case("/compact")
                {
                    "Wait for context compaction to finish before steering the thread."
                } else {
                    "Wait for sign-out to finish before steering the thread."
                },
            ));
        }
        if run["rootNodeId"].is_null() {
            return Err(rejection(
                &self.command,
                format!(
                    "Target run {} has no root node.",
                    run["id"].as_str().unwrap_or_default()
                ),
            ));
        }
        if run["status"] != "running" {
            return Err(rejection(
                &self.command,
                format!(
                    "Target run {} is {} and cannot be steered.",
                    run["id"].as_str().unwrap_or_default(),
                    run["status"].as_str().unwrap_or_default()
                ),
            ));
        }
        if self
            .records("turnItems")
            .iter()
            .any(|item| item["type"] == "run_interrupt_request" && item["runId"] == run["id"])
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Target run {} is stopping and cannot be steered.",
                    run["id"].as_str().unwrap_or_default()
                ),
            ));
        }
        let owner = self
            .records("providerThreads")
            .iter()
            .find(|owner| owner["id"] == run["providerThreadId"])
            .cloned()
            .filter(|owner| !owner["providerSessionId"].is_null())
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Provider thread {} has no active provider session for steering.",
                        run["providerThreadId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        let turn = self
            .records("providerTurns")
            .iter()
            .find(|turn| {
                turn["runAttemptId"] == run["activeAttemptId"] && turn["status"] == "running"
            })
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "No running provider turn found for active run {}.",
                        run["id"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        if !self.session_present(&owner["providerSessionId"])? {
            return Err(rejection(
                &self.command,
                format!(
                    "Provider session {} is not active.",
                    owner["providerSessionId"].as_str().unwrap_or_default()
                ),
            ));
        }
        let session = self
            .records("providerSessions")
            .iter()
            .find(|session| session["id"] == owner["providerSessionId"])
            .ok_or(Error::Unsupported(
                "steering requires live session capabilities",
            ))?;
        let caps: OrchestrationV2ProviderCapabilities =
            serde_json::from_value(session["capabilities"].clone())?;
        let picked = serde_json::from_value(selection.clone())?;
        let changed = !model_selections_equal(
            &serde_json::from_value(run["modelSelection"].clone())?,
            &picked,
        );
        let instance_changed = run["providerInstanceId"] != selection["instanceId"];
        let transition = if changed && !instance_changed {
            Some(
                self.inputs
                    .and_then(|inputs| inputs.selection_transition.as_ref())
                    .ok_or(Error::Unsupported(
                        "steering requires the adapter selection transition",
                    ))?,
            )
        } else {
            None
        };
        if let Some(SelectionTransitionPlan::Reject { reason }) = transition {
            return Err(rejection(&self.command, reason.clone()));
        }
        let force = mode["type"] == "restart_active"
            || changed
                && (instance_changed
                    || !matches!(transition, Some(SelectionTransitionPlan::ApplyOnNextTurn)))
            || attachment_count > 0
                && caps.turns.active_steering_accepts_attachments == Some(Some(false));
        let command_id: CommandId = serde_json::from_value(self.command["commandId"].clone())?;
        let instance: ProviderInstanceId =
            serde_json::from_value(run["providerInstanceId"].clone())?;
        let execution = CapabilityCheck {
            command_id: &command_id,
            thread_id: &self.thread_id,
            provider_instance_id: &instance,
            capabilities: &caps,
        }
        .steering(force)
        .map_err(|error| rejection(&self.command, error.to_string()))?;
        let carries =
            self.command["delegatedCompletion"].is_null() && self.command["notification"].is_null();
        let scope = Self::run_scope(&run);
        let intent = if self.command["type"] == "queued-message.promote-to-steer" {
            "promoted_queued_to_steer"
        } else {
            "steer"
        };
        if execution == SteeringExecution::ActiveSteering {
            if carries
                && !model_selections_equal(
                    &serde_json::from_value(self.projection["thread"]["modelSelection"].clone())?,
                    &picked,
                )
            {
                let mut thread = self.thread()?;
                let switched = thread["providerInstanceId"] != selection["instanceId"];
                thread["modelSelection"] = selection.clone();
                thread["providerInstanceId"] = selection["instanceId"].clone();
                thread["updatedAt"] = json!(self.context.now);
                self.update_thread(
                    if switched {
                        "thread.provider-switched"
                    } else {
                        "thread.model-selection-updated"
                    },
                    thread,
                )?;
            }
            self.append_message(
                MessageTarget {
                    run_id: run["id"].as_str().ok_or(Error::Stale)?,
                    node_id: run["rootNodeId"].as_str().ok_or(Error::Stale)?,
                    provider_thread_id: owner["id"].as_str().ok_or(Error::Stale)?,
                    provider_turn_id: turn["id"].clone(),
                    scope: &scope,
                    model_selection: selection,
                    handoff: false,
                },
                intent,
                true,
            )?;
            self.effect(
                &format!(
                    "provider-turn.steer:{}",
                    turn["id"].as_str().unwrap_or_default()
                ),
                EffectRequest::Steer {
                    provider_session_id: serde_json::from_value(
                        owner["providerSessionId"].clone(),
                    )?,
                    provider_thread_id: serde_json::from_value(owner["id"].clone())?,
                    provider_turn_id: serde_json::from_value(turn["id"].clone())?,
                    message_id: serde_json::from_value(self.command["messageId"].clone())?,
                    model_selection: carries.then_some(picked),
                    steer_follow_up_after_run_ordinal: Some(
                        self.records("runs")
                            .iter()
                            .filter_map(|run| run["ordinal"].as_u64())
                            .max()
                            .unwrap_or(0),
                    ),
                },
            );
            return Ok(());
        }
        let prepared_context = if instance_changed
            || matches!(transition, Some(SelectionTransitionPlan::CreateWithHandoff))
        {
            Some(prepared_message_context(self.inputs)?.clone())
        } else {
            None
        };
        let ordinal = self
            .records("attempts")
            .iter()
            .filter(|attempt| attempt["runId"] == run["id"])
            .filter_map(|attempt| attempt["attemptOrdinal"].as_u64())
            .max()
            .unwrap_or(0)
            + 1;
        let run_id = run["id"].as_str().ok_or(Error::Stale)?;
        let number = ordinal.to_string();
        let attempt_id = crate::ports::decisions::orchestrator::derived_id(
            "run-attempt",
            &["run", run_id, "attempt", &number],
        );
        let root_id = crate::ports::decisions::orchestrator::derived_id(
            "node",
            &["run", run_id, "attempt", &number, "root"],
        );
        let mut restarted_owner = prepared_context
            .as_ref()
            .map(|context| stored_value(&context.provider_thread))
            .transpose()?
            .unwrap_or_else(|| owner.clone());
        let mut session_transition = prepared_context
            .as_ref()
            .map(|_| crate::effects::SessionTransition::Detach);
        if matches!(transition, Some(SelectionTransitionPlan::RestartSession)) {
            let session_id = ProviderSessionId(self.allocate(IdKind::ProviderSession)?);
            restarted_owner["providerSessionId"] = json!(session_id);
            restarted_owner["status"] = json!("not_loaded");
            restarted_owner["updatedAt"] = json!(self.context.now);
            session_transition = Some(crate::effects::SessionTransition::Replace {
                replacement_provider_session_id: session_id,
            });
        }
        let mut next_run = run.clone();
        next_run["providerInstanceId"] = selection["instanceId"].clone();
        next_run["providerThreadId"] = restarted_owner["id"].clone();
        if let Some(handoff) = prepared_context
            .as_ref()
            .and_then(|context| context.handoff.as_ref())
        {
            next_run["contextHandoffId"] = stored_value(&handoff.id)?;
        }

        next_run["modelSelection"] = selection.clone();
        next_run["userMessageId"] = self.command["messageId"].clone();
        next_run["rootNodeId"] = json!(root_id);
        next_run["activeAttemptId"] = json!(attempt_id);
        next_run["status"] = json!("starting");
        let mut root = json!({"id":root_id,"threadId":self.thread_id,"runId":run["id"],"parentNodeId":null,"rootNodeId":root_id,"kind":"root_turn","status":"pending","countsForRun":true,"providerThreadId":owner["id"],"providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":null,"completedAt":null});
        root["providerThreadId"] = restarted_owner["id"].clone();
        let checkpoint = self.root_scope(&next_run, &root, &restarted_owner)?;
        root["checkpointScopeId"] = checkpoint["id"].clone();
        if let Some(mut current) = self
            .records("attempts")
            .iter()
            .find(|attempt| attempt["id"] == run["activeAttemptId"])
            .cloned()
        {
            current["status"] = json!("superseded");
            current["completedAt"] = json!(self.context.now);
            self.write_record("attempts", "run-attempt.updated", current, &scope)?;
        }
        if let Some(mut current) = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == run["rootNodeId"])
            .cloned()
        {
            current["status"] = json!("interrupted");
            current["completedAt"] = json!(self.context.now);
            self.write_record("nodes", "node.updated", current, &scope)?;
        }
        if changed {
            let mut thread = self.thread()?;
            thread["modelSelection"] = selection.clone();
            thread["updatedAt"] = json!(self.context.now);
            thread["providerInstanceId"] = selection["instanceId"].clone();
            self.update_thread(
                if instance_changed {
                    "thread.provider-switched"
                } else {
                    "thread.model-selection-updated"
                },
                thread,
            )?;
        }
        if let Some(context) = &prepared_context {
            restarted_owner = self.emit_message_context(context, &next_run, restarted_owner)?;
        }
        if session_transition.is_some() {
            self.write_record(
                "providerThreads",
                "provider-thread.updated",
                restarted_owner.clone(),
                &scope,
            )?;
        }
        let scope = Self::run_scope(&next_run);
        self.write_record("runs", "run.updated", next_run, &scope)?;
        self.write_record("attempts","run-attempt.created",json!({"id":attempt_id,"runId":run["id"],"attemptOrdinal":ordinal,"rootNodeId":root_id,"providerInstanceId":selection["instanceId"],"providerThreadId":restarted_owner["id"],"providerTurnId":null,"reason":"steering_restart","status":"pending","startedAt":null,"completedAt":null}),&scope)?;
        self.write_record("nodes", "node.updated", root, &scope)?;
        self.emit("checkpoint-scope.created", checkpoint, Some(&scope))?;
        self.append_message(
            MessageTarget {
                run_id,
                node_id: &root_id,
                provider_thread_id: restarted_owner["id"].as_str().ok_or(Error::Stale)?,
                provider_turn_id: Value::Null,
                scope: &scope,
                model_selection: selection,
                handoff: prepared_context
                    .as_ref()
                    .is_some_and(|context| context.handoff.is_some()),
            },
            intent,
            true,
        )?;
        self.effect(
            &format!(
                "provider-turn.restart:{}",
                turn["id"].as_str().unwrap_or_default()
            ),
            EffectRequest::Restart {
                provider_session_id: serde_json::from_value(owner["providerSessionId"].clone())?,
                provider_thread_id: serde_json::from_value(owner["id"].clone())?,
                provider_turn_id: serde_json::from_value(turn["id"].clone())?,
                interrupted_attempt_id: serde_json::from_value(run["activeAttemptId"].clone())?,
                run_id: serde_json::from_value(run["id"].clone())?,
                session_transition,
            },
        );
        Ok(())
    }

    fn reopen_checks(&mut self) -> Result<()> {
        let mut thread = self.thread()?;
        let Some(links) = thread["pullRequests"].as_array_mut() else {
            return Ok(());
        };
        let mut changed = false;
        for link in links {
            if let Some(watch) = link["watch"].as_object_mut() {
                changed |= watch.shift_remove("checksPending").is_some();
            }
        }
        if changed {
            self.emit("thread.pull-request-synced", thread, None)?;
        }
        Ok(())
    }

    fn remote_request(&mut self) -> Result<()> {
        let projection = self
            .context
            .projection
            .as_ref()
            .ok_or_else(|| Error::ThreadNotFound(self.thread_id.clone()))?;
        let follows_up = self.command["type"] == "delegated_task.remote.request"
            && self.records("subagents").iter().any(|task| {
                !task["result"].is_null()
                    && task["remoteChild"]["environmentId"]
                        == self.command["remoteChild"]["environmentId"]
                    && task["remoteChild"]["threadId"] == self.command["remoteChild"]["threadId"]
            });
        let owner = crate::ports::decisions::command_policy::follow_up_owner_run(&projection.runs)
            .map(stored_value)
            .transpose()?;
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["parentRunId"])
            .filter(|run| {
                !run["rootNodeId"].is_null()
                    && if follows_up {
                        owner.as_ref().is_some_and(|owner| owner["id"] == run["id"])
                    } else {
                        Self::active_run(run)
                    }
            })
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Parent run {} is not active.",
                        self.command["parentRunId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        if self
            .records("turnItems")
            .iter()
            .any(|item| item["type"] == "run_interrupt_request" && item["runId"] == run["id"])
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Parent run {} is stopping.",
                    self.command["parentRunId"].as_str().unwrap_or_default()
                ),
            ));
        }
        let command_id = self.command["commandId"].as_str().ok_or(Error::Stale)?;
        let node_id = crate::ports::decisions::orchestrator::derived_id(
            "node",
            &["delegated-task", command_id],
        );
        let item_id = crate::ports::decisions::orchestrator::derived_id(
            "turn-item",
            &["delegated-task", command_id],
        );
        let detail = self.command["title"]
            .as_str()
            .map(t3_contracts::wire::trim_js)
            .filter(|title| !title.is_empty())
            .or_else(|| {
                self.command["task"]
                    .as_str()
                    .map(t3_contracts::wire::trim_js)
                    .filter(|task| !task.is_empty())
            });
        let title = match detail {
            None => format!(
                "{} subagent {}",
                self.projection["thread"]["title"]
                    .as_str()
                    .unwrap_or_default(),
                self.records("subagents").len() + 1
            ),
            Some(detail) if detail.encode_utf16().count() <= 72 => detail.to_owned(),
            Some(detail) => {
                let mut length = 0;
                let title: String = detail
                    .chars()
                    .take_while(|character| {
                        length += character.len_utf16();
                        length <= 69
                    })
                    .collect();
                format!("{title}...")
            }
        };
        let scope = json!({"runId":run["id"],"nodeId":node_id,"driver":self.command["driver"],"providerInstanceId":self.command["modelSelection"]["instanceId"]});
        self.emit("node.updated", json!({"id":node_id,"threadId":self.thread_id,"runId":run["id"],"parentNodeId":self.command["parentNodeId"],"rootNodeId":run["rootNodeId"],"kind":"subagent","status":"running","countsForRun":false,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":self.context.now,"completedAt":null}), Some(&scope))?;
        let mut task = json!({"id":node_id,"threadId":self.thread_id,"runId":run["id"],"parentNodeId":self.command["parentNodeId"],"origin":"app_owned","createdBy":"agent","driver":self.command["driver"],"providerInstanceId":self.command["modelSelection"]["instanceId"],"providerThreadId":null,"childThreadId":null,"remoteChild":self.command["remoteChild"],"nativeTaskRef":null,"prompt":self.command["task"],"title":self.command["title"],"model":self.command["modelSelection"]["model"],"status":"running","result":null,"startedAt":self.context.now,"completedAt":null,"updatedAt":self.context.now});
        if let Some(policy) = self.command.get("completionWake") {
            task["completionWake"] = policy.clone();
        }
        if let Some(child) = self.command.get("childThreadId") {
            task["childThreadId"] = child.clone();
            task["createdBy"] = self.command["createdBy"].clone();
            task.as_object_mut()
                .ok_or(Error::Stale)?
                .shift_remove("remoteChild");
        }
        self.emit("subagent.updated", task, Some(&scope))?;
        let mut item = json!({"id":item_id,"threadId":self.thread_id,"runId":run["id"],"nodeId":node_id,"providerThreadId":run["providerThreadId"],"providerTurnId":self.provider_turn_for_run(&self.projection, &run).map_or(Value::Null, |turn| turn["id"].clone()),"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal(),"status":"running","title":self.command.get("title").filter(|title| !title.is_null()).cloned().unwrap_or(json!(title)),"startedAt":self.context.now,"completedAt":null,"updatedAt":self.context.now,"type":"subagent","subagentId":node_id,"origin":"app_owned","driver":self.command["driver"],"providerInstanceId":self.command["modelSelection"]["instanceId"],"childThreadId":null,"remoteChild":self.command["remoteChild"],"prompt":self.command["task"],"result":null});
        if let Some(child) = self.command.get("childThreadId") {
            item["childThreadId"] = child.clone();
            item.as_object_mut()
                .ok_or(Error::Stale)?
                .shift_remove("remoteChild");
        }
        self.emit("turn-item.updated", item, Some(&scope))
    }

    fn adopt_folder(&mut self) -> Result<()> {
        let mut thread = self.thread()?;
        let projection = self.context.projection.as_ref().ok_or(Error::Stale)?;
        if crate::ports::decisions::orchestrator::thread_has_unfinished_work(projection) {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} still has work in progress. Finish or stop it first.",
                    self.thread_id.0
                ),
            ));
        }
        let projects = &self
            .inputs
            .ok_or(Error::Unsupported("folder adoption requires project rows"))?
            .projects;
        let target = projects
            .iter()
            .find(|project| {
                json!(project.project_id) == self.command["projectId"]
                    && project.deleted_at.is_none()
            })
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Thread {} cannot move to missing project {}.",
                        self.thread_id.0,
                        self.command["projectId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        let current = projects
            .iter()
            .find(|project| json!(project.project_id) == thread["projectId"]);
        let cwd = thread["worktreePath"]
            .as_str()
            .or_else(|| current.map(|project| project.workspace_root.as_str()));
        if cwd != Some(target.workspace_root.as_str()) {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} does not work in {}.",
                    self.thread_id.0, target.workspace_root
                ),
            ));
        }
        thread["projectId"] = self.command["projectId"].clone();
        thread["worktreePath"] = Value::Null;
        thread["updatedAt"] = json!(self.context.now);
        self.emit("thread.metadata-updated", thread, None)
    }

    fn remote_stop(&mut self) -> Result<()> {
        let mut task = self
            .records("subagents")
            .iter()
            .find(|task| {
                task["id"] == self.command["taskId"]
                    && task["origin"] == "app_owned"
                    && !task["remoteChild"].is_null()
                    && !task["result"].is_null()
            })
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Delegated task {} is not a finished remote task of thread {}.",
                        self.command["taskId"].as_str().unwrap_or_default(),
                        self.thread_id.0
                    ),
                )
            })?;
        let scope = json!({"runId":task["runId"],"nodeId":task["id"],"driver":task["driver"]});
        task["remoteChild"]["stoppedThere"] = json!(false);
        task["remoteChild"]["stopCount"] =
            json!(task["remoteChild"]["stopCount"].as_u64().unwrap_or(0) + 1);
        task["updatedAt"] = json!(self.context.now);
        self.emit("subagent.updated", task, Some(&scope))
    }

    fn subagent_stop(&mut self) -> Result<()> {
        let task = self
            .records("subagents")
            .iter()
            .find(|task| task["id"] == self.command["subagentId"])
            .cloned();
        if let Some(task) = &task
            && task["origin"] == "app_owned"
            && !task["childThreadId"].is_null()
            && task["runId"] == self.command["runId"]
        {
            let child = self.inputs.and_then(|inputs| {
                inputs
                    .related_projections
                    .iter()
                    .find(|projection| json!(projection.thread.id) == task["childThreadId"])
            });
            let child = child.map(stored_value).transpose()?;
            let active = matches!(
                task["status"].as_str(),
                Some("pending" | "running" | "waiting")
            );
            if active
                && child.as_ref().is_some_and(|child| {
                    (child["thread"]["forkedFrom"]["type"] == "node"
                        && child["thread"]["forkedFrom"]["nodeId"] == task["id"])
                        || child["runs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|run| run["delegatedTaskId"] == task["id"])
                })
            {
                return Ok(());
            }
            let mut updated = task.clone();
            if active {
                updated["status"] = json!("cancelled");
                updated["completedAt"] = json!(self.context.now);
                updated["updatedAt"] = json!(self.context.now);
                updated["completionDelivery"] = json!({"state":"disposed","observedByRunId":null});
            }
            let scope = json!({"nodeId":task["id"]});
            self.emit("subagent.updated", updated.clone(), Some(&scope))?;
            if let Some(mut node) = self
                .records("nodes")
                .iter()
                .find(|node| node["id"] == task["id"])
                .cloned()
            {
                node["status"] = updated["status"].clone();
                node["completedAt"] = updated["completedAt"].clone();
                self.emit("node.updated", node, Some(&scope))?;
            }
            let items: Vec<_> = self
                .records("turnItems")
                .iter()
                .filter(|item| item["type"] == "subagent" && item["subagentId"] == task["id"])
                .cloned()
                .collect();
            for mut item in items {
                item["status"] = updated["status"].clone();
                item["completedAt"] = updated["completedAt"].clone();
                item["updatedAt"] = json!(self.context.now);
                self.emit("turn-item.updated", item, Some(&scope))?;
            }
            return Ok(());
        }
        let mut task = task
            .filter(|task| {
                task["runId"] == self.command["runId"]
                    && task["origin"] == "provider_native"
                    && task["driver"] == "claudeAgent"
                    && task["nativeTaskRef"]["strength"] == "strong"
                    && !task["nativeTaskRef"]["nativeId"].is_null()
            })
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    "The requested native subagent cannot be stopped on this run.",
                )
            })?;
        if !matches!(
            task["status"].as_str(),
            Some("pending" | "running" | "waiting")
        ) {
            return Ok(());
        }
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["runId"]);
        let provider_thread = run.and_then(|run| {
            self.records("providerThreads")
                .iter()
                .find(|thread| thread["id"] == run["providerThreadId"])
        });
        let turn = provider_thread
            .and_then(|thread| {
                self.records("providerTurns")
                    .iter()
                    .rev()
                    .find(|turn| turn["providerThreadId"] == thread["id"])
            })
            .cloned();
        let provider_thread = provider_thread
            .filter(|thread| {
                !thread["providerSessionId"].is_null()
                    && thread["driver"] == task["driver"]
                    && thread["providerInstanceId"] == task["providerInstanceId"]
            })
            .cloned()
            .filter(|_| turn.is_some())
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    "The requested native subagent has no owning provider session.",
                )
            })?;
        let turn = turn.ok_or(Error::Stale)?;
        let scope = json!({"runId":self.command["runId"],"nodeId":task["id"],"providerInstanceId":task["providerInstanceId"]});
        task["updatedAt"] = json!(self.context.now);
        self.emit("subagent.updated", task.clone(), Some(&scope))?;
        self.effect(
            &format!("subagent.stop:{}", task["id"].as_str().ok_or(Error::Stale)?),
            EffectRequest::Interrupt {
                provider_session_id: serde_json::from_value(
                    provider_thread["providerSessionId"].clone(),
                )?,
                provider_thread_id: serde_json::from_value(provider_thread["id"].clone())?,
                provider_turn_id: serde_json::from_value(turn["id"].clone())?,
                subagent: Some(crate::effects::InterruptSubagent {
                    id: serde_json::from_value(task["id"].clone())?,
                    native_task_id: task["nativeTaskRef"]["nativeId"]
                        .as_str()
                        .ok_or(Error::Stale)?
                        .to_owned(),
                }),
            },
        );
        Ok(())
    }

    fn merge_back(&mut self) -> Result<()> {
        let source = self
            .inputs
            .and_then(|inputs| {
                inputs.related_projections.iter().find(|projection| {
                    json!(projection.thread.id) == self.command["sourceThreadId"]
                })
            })
            .ok_or(Error::Unsupported(
                "merge-back decision requires the source projection",
            ))?;
        let source_value = stored_value(source)?;
        if source_value["thread"]["lineage"]["relationshipToParent"] != "fork"
            || source_value["thread"]["lineage"]["parentThreadId"] != self.command["targetThreadId"]
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} is not a fork of {}.",
                    self.command["sourceThreadId"].as_str().unwrap_or_default(),
                    self.thread_id.0
                ),
            ));
        }
        if self
            .records("runs")
            .iter()
            .any(|run| Self::active_run(run) || run["status"] == "queued")
        {
            return Err(rejection(
                &self.command,
                "Finish or stop the target thread's active work and clear its queue before merging back.",
            ));
        }
        if self.records("contextTransfers").iter().any(|transfer| {
            transfer["type"] == "merge_back"
                && transfer["status"] == "pending"
                && transfer["targetThreadId"] == self.command["targetThreadId"]
                && transfer["sourceThreadId"] != self.command["sourceThreadId"]
        }) {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} already has pending context from another side chat or fork. Send a message there first.",
                    self.thread_id.0
                ),
            ));
        }
        let point = &self.command["sourcePoint"];
        let run = run_for_source_point(source, point).ok_or_else(|| {
            rejection(
                &self.command,
                format!(
                    "No stable source run was found for merge-back source {}.",
                    point["type"].as_str().unwrap_or_default()
                ),
            )
        })?;
        if !matches!(
            run.status,
            OrchestrationV2RunStatus::Completed | OrchestrationV2RunStatus::Waiting
        ) {
            return Err(rejection(
                &self.command,
                format!(
                    "Merge-back source run {} is {}; only provider-finished runs are supported.",
                    stored_value(&run.id)?.as_str().ok_or(Error::Stale)?,
                    stored_value(&run.status)?.as_str().ok_or(Error::Stale)?
                ),
            ));
        }
        let fork_transfer = source_value["contextTransfers"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .find(|transfer| {
                transfer["type"] == "fork"
                    && transfer["sourceThreadId"] == self.command["targetThreadId"]
                    && transfer["targetThreadId"] == self.command["sourceThreadId"]
            })
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "No fork transfer exists between {} and {}.",
                        self.thread_id.0,
                        self.command["sourceThreadId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        let provider_thread = source
            .provider_threads
            .iter()
            .find(|thread| Some(&thread.id) == run.provider_thread_id.as_ref());
        let turn = self.provider_turn_for_run(&source_value, &stored_value(run)?);
        let mut canonical = json!({"threadId":source.thread.id,"runId":run.id});
        if let Some(checkpoint) = &run.checkpoint_id {
            canonical["checkpointId"] = json!(checkpoint);
        }
        if let Some(reference) =
            provider_thread.and_then(|thread| thread.native_thread_ref.as_ref())
        {
            canonical["providerThreadRef"] = json!(reference);
        }
        if let Some(reference) = turn
            .as_ref()
            .and_then(|turn| turn.get("nativeTurnRef"))
            .filter(|reference| !reference.is_null())
        {
            canonical["providerTurnRef"] = reference.clone();
        }
        let at = self
            .command
            .get("createdAt")
            .cloned()
            .unwrap_or(json!(self.context.now));
        let transfer_id = self.allocate(IdKind::ContextTransfer)?;
        let transfer = json!({"id":transfer_id,"type":"merge_back","sourceThreadId":self.command["sourceThreadId"],"targetThreadId":self.thread_id,"sourcePoint":canonical,"basePoint":fork_transfer["sourcePoint"],"sourceProviderInstanceId":run.provider_instance_id,"targetProviderInstanceId":self.projection["thread"]["modelSelection"]["instanceId"],"targetRunId":null,"status":"pending","resolution":null,"createdBy":self.command["createdBy"],"error":if provider_thread.is_none() { json!("Source merge-back run has no provider thread.") } else { Value::Null },"createdAt":at,"updatedAt":at,"consumedAt":null});
        let pending: Vec<_> = self
            .records("contextTransfers")
            .iter()
            .filter(|transfer| {
                transfer["type"] == "merge_back"
                    && transfer["status"] == "pending"
                    && transfer["sourceThreadId"] == self.command["sourceThreadId"]
                    && transfer["targetThreadId"] == self.command["targetThreadId"]
            })
            .cloned()
            .collect();
        let provider = json!(run.provider_instance_id);
        for mut pending in pending {
            pending["status"] = json!("superseded");
            pending["error"] = json!(format!("Superseded by merge-back transfer {transfer_id}."));
            pending["updatedAt"] = at.clone();
            self.emit_transfer("context-transfer.updated", pending, &provider, &at)?;
        }
        self.emit_transfer("context-transfer.created", transfer, &provider, &at)
    }

    fn emit_transfer(
        &mut self,
        kind: &str,
        payload: Value,
        provider: &Value,
        at: &Value,
    ) -> Result<()> {
        let id = self.allocate(IdKind::Event)?;
        self.decision.events.push(serde_json::from_value(json!({"id":id,"type":kind,"threadId":self.thread_id,"providerInstanceId":provider,"occurredAt":at,"payload":payload}))?);
        Ok(())
    }

    fn provider_turn_for_run(&self, projection: &Value, run: &Value) -> Option<Value> {
        if run["activeAttemptId"].is_null() {
            return None;
        }
        let turns = projection["providerTurns"].as_array()?;
        turns
            .iter()
            .filter(|turn| turn["runAttemptId"] == run["activeAttemptId"])
            .reduce(|latest, turn| {
                if turn["ordinal"].as_u64() > latest["ordinal"].as_u64() {
                    turn
                } else {
                    latest
                }
            })
            .or_else(|| {
                let attempt = projection["attempts"]
                    .as_array()?
                    .iter()
                    .find(|attempt| attempt["id"] == run["activeAttemptId"])?;
                turns
                    .iter()
                    .find(|turn| turn["id"] == attempt["providerTurnId"])
            })
            .cloned()
    }

    fn secret_record(&mut self) -> Result<()> {
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["runId"])
            .filter(|run| run["rootNodeId"] == self.command["nodeId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Node {} is not the root of run {}.",
                        self.command["nodeId"].as_str().unwrap_or_default(),
                        self.command["runId"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        let existing = self
            .records("turnItems")
            .iter()
            .find(|item| item["id"] == self.command["turnItemId"])
            .cloned();
        if let Some(existing) = &existing {
            let reason = if existing["type"] != "secret_request" {
                Some(format!(
                    "Turn item {} is not a secret request.",
                    self.command["turnItemId"].as_str().unwrap_or_default()
                ))
            } else if existing["runId"] != self.command["runId"] {
                Some(format!(
                    "Secret request {} belongs to another run.",
                    self.command["turnItemId"].as_str().unwrap_or_default()
                ))
            } else {
                None
            };
            if let Some(reason) = reason {
                return Err(rejection(&self.command, reason));
            }
        }
        let scope = Self::run_scope(&run);
        if let Some(existing) = &existing
            && (existing["secretStatus"] != "pending" || self.command["secretStatus"] == "pending")
        {
            return self.emit("turn-item.updated", existing.clone(), Some(&scope));
        }
        let pending = self.command["secretStatus"] == "pending";
        let mut item = json!({"id":self.command["turnItemId"],"threadId":self.thread_id,"runId":run["id"],"nodeId":self.command["nodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":self.provider_turn_for_run(&self.projection, &run).map_or(Value::Null, |turn| turn["id"].clone()),"nativeItemRef":null,"parentItemId":null,"ordinal":existing.as_ref().map_or_else(|| json!(self.next_item_ordinal()), |item| item["ordinal"].clone()),"status":if pending { "waiting" } else if self.command["secretStatus"] == "saved" { "completed" } else { "cancelled" },"title":self.command["label"],"startedAt":existing.as_ref().and_then(|item| item.get("startedAt")).filter(|time| !time.is_null()).cloned().unwrap_or(json!(self.context.now)),"completedAt":if pending { Value::Null } else {json!(self.context.now)},"updatedAt":self.context.now,"type":"secret_request","label":self.command["label"],"reason":self.command["reason"],"secretStatus":self.command["secretStatus"]});
        for field in ["url", "placeholder", "delivery"] {
            if let Some(value) = self.command.get(field) {
                item[field] = value.clone();
            }
        }
        if self.command["secretStatus"] == "saved"
            && !self.projection["thread"]["snoozedUntil"].is_null()
        {
            let mut thread = self.projection["thread"].clone();
            thread["snoozedUntil"] = Value::Null;
            thread["snoozedAt"] = Value::Null;
            thread["updatedAt"] = json!(self.context.now);
            self.emit("thread.unsnoozed", thread, None)?;
        }
        self.emit("turn-item.updated", item, Some(&scope))
    }

    fn created_thread_record(&mut self) -> Result<()> {
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["parentRunId"])
            .cloned();
        let node = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == self.command["parentNodeId"]);
        if run
            .as_ref()
            .is_none_or(|run| run["rootNodeId"] != self.command["parentNodeId"])
            || node.is_none_or(|node| node["runId"] != self.command["parentRunId"])
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Parent node {} is not the root of run {}.",
                    self.command["parentNodeId"].as_str().unwrap_or_default(),
                    self.command["parentRunId"].as_str().unwrap_or_default()
                ),
            ));
        }
        let run = run.ok_or(Error::Stale)?;
        let target = self
            .inputs
            .and_then(|inputs| {
                inputs.related_projections.iter().find(|projection| {
                    json!(projection.thread.id) == self.command["targetThreadId"]
                })
            })
            .ok_or(Error::Unsupported(
                "created thread record requires the target projection",
            ))?;
        let target = stored_value(target)?;
        if target["thread"]["projectId"] != self.projection["thread"]["projectId"] {
            return Err(rejection(
                &self.command,
                format!(
                    "Target thread {} belongs to another project.",
                    self.command["targetThreadId"].as_str().unwrap_or_default()
                ),
            ));
        }
        if !self.command["targetRunId"].is_null()
            && !target["runs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|run| run["id"] == self.command["targetRunId"])
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Target run {} does not belong to thread {}.",
                    self.command["targetRunId"].as_str().unwrap_or_default(),
                    self.command["targetThreadId"].as_str().unwrap_or_default()
                ),
            ));
        }
        let id = crate::ports::decisions::orchestrator::derived_id(
            "turn-item",
            &[
                "created-thread",
                self.command["commandId"].as_str().ok_or(Error::Stale)?,
            ],
        );
        let scope = Self::run_scope(&run);
        self.emit("turn-item.updated", json!({"id":id,"threadId":self.thread_id,"runId":run["id"],"nodeId":self.command["parentNodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":self.provider_turn_for_run(&self.projection, &run).map_or(Value::Null, |turn| turn["id"].clone()),"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal(),"status":"completed","title":target["thread"]["title"],"startedAt":self.context.now,"completedAt":self.context.now,"updatedAt":self.context.now,"type":"thread_created","targetThreadId":self.command["targetThreadId"],"targetRunId":self.command["targetRunId"],"targetProviderInstanceId":target["thread"]["modelSelection"]["instanceId"],"targetModel":target["thread"]["modelSelection"]["model"]}), Some(&scope))
    }

    fn fork(&mut self) -> Result<()> {
        use crate::ports::checkpoints_forks::thread_fork_service::{
            ForkPlanInput, is_forkable_source_run_status, plan,
        };
        let source = self
            .inputs
            .and_then(|inputs| {
                inputs.related_projections.iter().find(|projection| {
                    json!(projection.thread.id) == self.command["sourceThreadId"]
                })
            })
            .ok_or(Error::Unsupported(
                "fork decision requires the source projection",
            ))?;
        let source_value = stored_value(source)?;
        let side_chat = self.command["sideChat"] == true;
        if side_chat && !source_value["thread"]["deletedAt"].is_null() {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} was deleted.",
                    self.command["sourceThreadId"].as_str().unwrap_or_default()
                ),
            ));
        }
        if side_chat
            && source_value["thread"]["worktreePath"].is_null()
            && crate::ports::decisions::model_switch::latest_executed_run(&source.runs)
                .and_then(|run| stored_value(run).ok())
                .is_some_and(|run| {
                    run["workspacePreparation"]["type"] == "worktree"
                        && matches!(
                            run["status"].as_str(),
                            Some("preparing" | "failed" | "interrupted")
                        )
                })
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} has no worktree yet. Start the side chat once its worktree is set up.",
                    self.command["sourceThreadId"].as_str().unwrap_or_default()
                ),
            ));
        }
        if self.context.projection.is_some() {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} already exists. Use a new thread id.",
                    self.thread_id.0
                ),
            ));
        }
        let point = &self.command["sourcePoint"];
        let run = run_for_source_point(source, point);
        if run.is_none() && !(side_chat && point["type"] == "latest_stable") {
            return Err(rejection(
                &self.command,
                format!(
                    "No stable source run was found for fork source {}.",
                    point["type"].as_str().unwrap_or_default()
                ),
            ));
        }
        if let Some(run) = run
            && !is_forkable_source_run_status(&run.status)
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Fork source run {} is {}; in-progress and rolled-back runs cannot be forked.",
                    stored_value(&run.id)?.as_str().ok_or(Error::Stale)?,
                    stored_value(&run.status)?.as_str().ok_or(Error::Stale)?
                ),
            ));
        }
        let provider_thread = run.and_then(|run| {
            source
                .provider_threads
                .iter()
                .find(|thread| Some(&thread.id) == run.provider_thread_id.as_ref())
        });
        let turn =
            run.and_then(|run| self.provider_turn_for_run(&source_value, &stored_value(run).ok()?));
        let mut canonical = json!({"threadId":source.thread.id});
        if let Some(run) = run {
            canonical["runId"] = json!(run.id);
            if let Some(checkpoint) = &run.checkpoint_id {
                canonical["checkpointId"] = json!(checkpoint);
            }
            if let Some(reference) =
                provider_thread.and_then(|thread| thread.native_thread_ref.as_ref())
            {
                canonical["providerThreadRef"] = json!(reference);
            }
            if let Some(reference) = turn
                .as_ref()
                .and_then(|turn| turn.get("nativeTurnRef"))
                .filter(|reference| !reference.is_null())
            {
                canonical["providerTurnRef"] = reference.clone();
            }
        }
        let at = self
            .command
            .get("createdAt")
            .cloned()
            .unwrap_or(json!(self.context.now));
        let transfer_id = self.allocate(IdKind::ContextTransfer)?;
        let result = plan(ForkPlanInput {
            source_thread: &source.thread,
            source_run: run,
            source_provider_thread: provider_thread,
            source_token_usage: turn
                .and_then(|turn| serde_json::from_value(turn["tokenUsage"].clone()).ok()),
            canonical_source_point: serde_json::from_value(canonical)?,
            transfer_id: ContextTransferId(transfer_id),
            target_thread_id: self.thread_id.clone(),
            title: self.command["title"].as_str().map(str::to_owned),
            side_chat,
            created_by: serde_json::from_value(self.command["createdBy"].clone())?,
            creation_source: serde_json::from_value(self.command["creationSource"].clone())?,
            created_at: serde_json::from_value(at.clone())?,
        })
        .map_err(|error| match error {
            Error::Service { detail, .. } => rejection(&self.command, detail),
            other => other,
        })?;
        for (kind, payload, provider) in [
            (
                "thread.created",
                stored_value(&result.target_thread)?,
                json!(result.target_thread.provider_instance_id),
            ),
            (
                "context-transfer.created",
                stored_value(&result.transfer)?,
                json!(result.transfer.source_provider_instance_id),
            ),
        ] {
            let id = self.allocate(IdKind::Event)?;
            self.decision.events.push(serde_json::from_value(json!({"id":id,"type":kind,"threadId":self.thread_id,"providerInstanceId":provider,"occurredAt":at,"payload":payload}))?);
        }
        Ok(())
    }

    fn prepared_run(&mut self, kind: &str) -> Result<()> {
        let run = self
            .records("runs")
            .iter()
            .find(|run| run["id"] == self.command["runId"])
            .cloned();
        let attempt = run
            .as_ref()
            .and_then(|run| {
                self.records("attempts")
                    .iter()
                    .find(|attempt| attempt["id"] == run["activeAttemptId"])
            })
            .cloned();
        let node = run
            .as_ref()
            .and_then(|run| {
                self.records("nodes")
                    .iter()
                    .find(|node| node["id"] == run["rootNodeId"])
            })
            .cloned();
        let provider_thread = run
            .as_ref()
            .and_then(|run| {
                self.records("providerThreads")
                    .iter()
                    .find(|thread| thread["id"] == run["providerThreadId"])
            })
            .cloned();
        let item = self
            .records("turnItems")
            .iter()
            .find(|item| {
                item["runId"] == self.command["runId"]
                    && item["type"] == "command_execution"
                    && item["input"] == "Preparing workspace"
            })
            .cloned();
        let retry = kind == "prepared-run.retry";
        let missing = || {
            rejection(
                &self.command,
                format!(
                    "Run {} {}.",
                    self.command["runId"].as_str().unwrap_or_default(),
                    if retry {
                        "has no failed workspace preparation to retry"
                    } else {
                        "is not awaiting workspace preparation"
                    }
                ),
            )
        };
        let mut run = run
            .filter(|run| run["status"] == if retry { "failed" } else { "preparing" })
            .ok_or_else(missing)?;
        let mut attempt = attempt.ok_or_else(missing)?;
        let mut node = node.ok_or_else(missing)?;
        let provider_thread = provider_thread.ok_or_else(missing)?;
        let mut item = item.ok_or_else(missing)?;
        let scope = Self::run_scope(&run);
        if kind == "prepared-run.release" {
            let checkpoint_scope = self.root_scope(&run, &node, &provider_thread)?;
            node["checkpointScopeId"] = checkpoint_scope["id"].clone();
            self.emit("checkpoint-scope.created", checkpoint_scope, Some(&scope))?;
            self.emit("node.updated", node, Some(&scope))?;
            item["status"] = json!("completed");
            item["title"] = json!("Workspace ready");
            item["output"] = json!("Workspace preparation completed.");
            item["exitCode"] = json!(0);
            item["completedAt"] = json!(self.context.now);
            item["updatedAt"] = json!(self.context.now);
            self.emit("turn-item.updated", item, Some(&scope))?;
            run["status"] = json!("starting");
            self.emit("run.updated", run.clone(), Some(&scope))?;
            self.effect(
                &format!(
                    "provider-turn.start:{}",
                    run["id"].as_str().ok_or(Error::Stale)?
                ),
                EffectRequest::Start {
                    run_id: serde_json::from_value(run["id"].clone())?,
                },
            );
            return Ok(());
        }
        if kind == "prepared-run.progress" {
            item["title"] = json!(if self.command["phase"] == "worktree" {
                "Preparing worktree"
            } else {
                "Starting setup script"
            });
            item["updatedAt"] = json!(self.context.now);
            return self.emit("turn-item.updated", item, Some(&scope));
        }
        if retry {
            let mut failure = self
                .records("turnItems")
                .iter()
                .find(|item| {
                    item["runId"] == run["id"]
                        && item["type"] == "error"
                        && item["status"] == "failed"
                        && item["failure"]["code"] == "workspace_preparation_failed"
                })
                .cloned()
                .ok_or_else(missing)?;
            if run.get("workspacePreparation").is_none()
                || !self.projection["thread"]["archivedAt"].is_null()
                || !self.projection["thread"]["deletedAt"].is_null()
            {
                return Err(missing());
            }
            if self
                .records("runs")
                .iter()
                .any(|other| other["id"] != run["id"] && Self::active_run(other))
            {
                return Err(rejection(
                    &self.command,
                    "Another run is active on this thread.",
                ));
            }
            failure["status"] = json!("cancelled");
            failure["updatedAt"] = json!(self.context.now);
            self.emit("turn-item.updated", failure, Some(&scope))?;
            for field in ["output", "exitCode", "outputIndicatesFailure"] {
                item.as_object_mut()
                    .ok_or(Error::Stale)?
                    .shift_remove(field);
            }
            item["status"] = json!("running");
            item["title"] = json!("Preparing workspace");
            item["completedAt"] = Value::Null;
            item["updatedAt"] = json!(self.context.now);
            self.emit("turn-item.updated", item, Some(&scope))?;
            attempt["status"] = json!("pending");
            attempt["completedAt"] = Value::Null;
            self.emit("run-attempt.updated", attempt, Some(&scope))?;
            node["status"] = json!("pending");
            node["completedAt"] = Value::Null;
            self.emit("node.updated", node, Some(&scope))?;
            run["status"] = json!("preparing");
            run["completedAt"] = Value::Null;
            self.emit("run.updated", run.clone(), Some(&scope))?;
            let projection = self.context.projection.as_ref().ok_or(Error::Stale)?;
            if crate::ports::decisions::model_switch::latest_executed_run(&projection.runs)
                .is_some_and(|latest| stored_value(&latest.id).ok().as_ref() == Some(&run["id"]))
            {
                let held: Vec<_> = self
                    .records("runs")
                    .iter()
                    .filter(|run| run["status"] == "queued" && run["queueHeld"] == true)
                    .cloned()
                    .collect();
                for mut run in held {
                    let scope =
                        json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]});
                    run["queueHeld"] = json!(false);
                    self.emit("run.updated", run, Some(&scope))?;
                }
            }
            return Ok(());
        }
        attempt["status"] = json!("failed");
        attempt["completedAt"] = json!(self.context.now);
        self.emit("run-attempt.updated", attempt, Some(&scope))?;
        node["status"] = json!("failed");
        node["completedAt"] = json!(self.context.now);
        self.emit("node.updated", node.clone(), Some(&scope))?;
        item["status"] = json!("failed");
        item["title"] = json!("Workspace preparation failed");
        item["output"] = self.command["failure"]["message"].clone();
        item["exitCode"] = json!(1);
        item["completedAt"] = json!(self.context.now);
        item["updatedAt"] = json!(self.context.now);
        self.emit("turn-item.updated", item, Some(&scope))?;
        let id = crate::ports::decisions::orchestrator::derived_id(
            "turn-item",
            &[
                "provider",
                provider_thread["driver"].as_str().ok_or(Error::Stale)?,
                "native-item",
                &format!(
                    "workspace-preparation-failure:{}",
                    run["id"].as_str().ok_or(Error::Stale)?
                ),
            ],
        );
        let mut failure = self.command["failure"].clone();
        failure["code"] = json!("workspace_preparation_failed");
        self.emit("turn-item.updated", json!({"id":id,"threadId":self.thread_id,"runId":run["id"],"nodeId":node["id"],"providerThreadId":provider_thread["id"],"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal(),"status":"failed","title":"Workspace preparation failed","startedAt":self.context.now,"completedAt":self.context.now,"updatedAt":self.context.now,"type":"error","failure":failure}), Some(&scope))?;
        run["status"] = json!("failed");
        run["completedAt"] = json!(self.context.now);
        self.emit("run.updated", run, Some(&scope))
    }

    fn active_run(run: &Value) -> bool {
        matches!(
            run["status"].as_str(),
            Some("preparing" | "starting" | "running" | "waiting")
        )
    }

    fn root_scope(&self, run: &Value, node: &Value, provider_thread: &Value) -> Result<Value> {
        use crate::ports::checkpoints_forks::checkpoint_service::{
            PrepareRootScope, prepare_root_run_scope,
        };
        let cwd = self
            .inputs
            .and_then(|inputs| inputs.runtime_policy.as_ref())
            .and_then(|configured| configured.policy.cwd.clone())
            .or_else(|| {
                self.projection["thread"]["worktreePath"]
                    .as_str()
                    .map(std::path::PathBuf::from)
            })
            .or_else(|| {
                self.inputs
                    .and_then(|inputs| {
                        inputs.projects.iter().find(|project| {
                            json!(project.project_id) == self.projection["thread"]["projectId"]
                        })
                    })
                    .map(|project| std::path::PathBuf::from(&project.workspace_root))
            })
            .ok_or(Error::Unsupported(
                "root run scope requires a preloaded runtime policy or project workspace",
            ))?;
        stored_value(prepare_root_run_scope(PrepareRootScope {
            thread_id: self.thread_id.clone(),
            run_id: serde_json::from_value(run["id"].clone())?,
            root_node_id: serde_json::from_value(node["id"].clone())?,
            provider_thread_id: serde_json::from_value(provider_thread["id"].clone())?,
            cwd,
            created_at: self.context.now.clone(),
        }))
        .map_err(Error::from)
    }

    fn queue_resume(&mut self) -> Result<()> {
        if self.projection.is_null() {
            return Err(Error::ThreadNotFound(self.thread_id.clone()));
        }
        let thread = self.projection["thread"].clone();
        if !thread["archivedAt"].is_null() || !thread["deletedAt"].is_null() {
            return Err(rejection(
                &self.command,
                format!("Thread {} is not active.", self.thread_id.0),
            ));
        }
        let projection = self.context.projection.as_ref().ok_or(Error::Stale)?;
        let selected = if self.command["type"] == "queued-run.start" {
            let run = self
                .records("runs")
                .iter()
                .find(|run| run["id"] == self.command["runId"] && run["status"] == "queued");
            let message = run.and_then(|run| {
                self.records("messages")
                    .iter()
                    .find(|message| message["id"] == run["userMessageId"])
            });
            if run.is_none()
                || message.is_none_or(|message| {
                    !message["delegatedCompletion"].is_null() || !message["notification"].is_null()
                })
                || self.records("runs").iter().any(Self::active_run)
            {
                return Err(rejection(
                    &self.command,
                    "A queued user message can only be sent now while the thread is stopped.",
                ));
            }
            run.cloned()
        } else {
            None
        };
        let next = selected.or_else(|| {
            queued_runs_in_delivery_order(projection)
                .first()
                .and_then(|run| stored_value(run).ok())
        });
        let provider = next.as_ref().map_or_else(
            || thread["providerInstanceId"].clone(),
            |run| {
                let message = self
                    .records("messages")
                    .iter()
                    .find(|message| message["id"] == run["userMessageId"]);
                let follows = message.is_some_and(|message| {
                    !message["delegatedCompletion"].is_null()
                        || (message["notification"]["source"]["kind"] == "monitor"
                            && !(message["createdBy"] == "agent"
                                && message["creationSource"] == "provider"
                                && !message["notification"].is_null()
                                && message["delegatedCompletion"].is_null()))
                });
                if follows {
                    thread["modelSelection"]["instanceId"].clone()
                } else {
                    run["modelSelection"]["instanceId"].clone()
                }
            },
        );
        if self.usage_limit_holds_queue(&provider, None)? {
            return Err(rejection(
                &self.command,
                "Continue the limited thread before resuming its queue.",
            ));
        }
        let queued: Vec<_> = self
            .records("runs")
            .iter()
            .filter(|run| run["status"] == "queued")
            .cloned()
            .collect();
        if queued.is_empty() {
            return self.emit("thread.metadata-updated", thread, Some(&json!({})));
        }
        for mut run in queued {
            let scope = json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]});
            run["queueHeld"] = json!(false);
            self.emit("run.updated", run, Some(&scope))?;
        }
        Ok(())
    }

    fn usage_limit_holds_queue(&self, provider: &Value, now: Option<&IsoDateTime>) -> Result<bool> {
        let projection = self.context.projection.as_ref().ok_or(Error::Stale)?;
        let runs: Vec<_> = projection
            .runs
            .iter()
            .filter(|run| stored_value(&run.provider_instance_id).ok().as_ref() == Some(provider))
            .cloned()
            .collect();
        let Some(latest) = crate::ports::decisions::model_switch::latest_executed_run(&runs) else {
            return Ok(false);
        };
        if latest.status != OrchestrationV2RunStatus::Failed {
            return Ok(false);
        }
        let latest = stored_value(latest)?;
        let failure = self
            .records("turnItems")
            .iter()
            .filter(|item| {
                item["type"] == "error"
                    && item["status"] == "failed"
                    && item["runId"] == latest["id"]
                    && item["nodeId"] == latest["rootNodeId"]
            })
            .max_by(|left, right| {
                left["updatedAt"]
                    .as_str()
                    .and_then(epoch_millis)
                    .cmp(&right["updatedAt"].as_str().and_then(epoch_millis))
                    .then_with(|| left["ordinal"].as_u64().cmp(&right["ordinal"].as_u64()))
                    .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
            });
        let session = self
            .records("providerSessions")
            .iter()
            .filter(|session| &session["providerInstanceId"] == provider)
            .reduce(|latest, session| {
                if session["updatedAt"].as_str().and_then(epoch_millis)
                    > latest["updatedAt"].as_str().and_then(epoch_millis)
                {
                    session
                } else {
                    latest
                }
            });
        let limited = failure.is_some_and(|failure| {
            failure["failure"]["class"] == "usage_limit"
                && session.is_none_or(|session| {
                    session["lastError"].is_null()
                        || session["lastError"] == failure["failure"]["message"]
                })
        });
        if !limited {
            return Ok(false);
        }
        if crate::ports::decisions::model_switch::latest_executed_run(&projection.runs)
            .is_some_and(|run| stored_value(&run.id).ok().as_ref() == Some(&latest["id"]))
        {
            return Ok(true);
        }
        Ok(now.is_some_and(|now| {
            failure
                .and_then(|item| item["failure"]["resetAt"].as_str())
                .and_then(epoch_millis)
                .zip(epoch_millis(&now.0))
                .is_some_and(|(reset, now)| reset > now)
        }))
    }

    fn next_item_ordinal(&self) -> u64 {
        if let Some(ordinal) = self.inputs.and_then(|inputs| {
            inputs
                .next_item_ordinals
                .iter()
                .find(|(thread, run, _)| thread == &self.thread_id && run.is_none())
                .map(|(_, _, ordinal)| *ordinal)
        }) {
            let pending = self
                .decision
                .events
                .iter()
                .filter_map(|event| stored_value(event).ok())
                .filter(|event| event["type"] == "turn-item.updated")
                .filter_map(|event| event["payload"]["ordinal"].as_u64())
                .max()
                .map_or(0, |ordinal| ordinal + 1);
            return ordinal.max(pending);
        }
        self.records("turnItems")
            .iter()
            .map(|item| item["ordinal"].as_u64().unwrap_or(0))
            .max()
            .map_or(0, |ordinal| ordinal + 1)
    }

    fn request_context(&self) -> (Option<Value>, Option<Value>, Option<Value>, Option<Value>) {
        let request = self
            .records("runtimeRequests")
            .iter()
            .find(|request| request["id"] == self.command["requestId"])
            .cloned();
        let node = request
            .as_ref()
            .and_then(|request| {
                self.records("nodes")
                    .iter()
                    .find(|node| node["id"] == request["nodeId"])
            })
            .cloned();
        let session = request
            .as_ref()
            .filter(|request| request["responseCapability"]["type"] == "live")
            .and_then(|request| {
                self.records("providerSessions").iter().find(|session| {
                    session["id"] == request["responseCapability"]["providerSessionId"]
                })
            })
            .cloned();
        let item = self
            .records("turnItems")
            .iter()
            .find(|item| {
                item["requestId"] == self.command["requestId"]
                    && matches!(
                        item["type"].as_str(),
                        Some("approval_request" | "user_input_request")
                    )
            })
            .cloned();
        (request, node, session, item)
    }

    fn user_input_dismiss(&mut self) -> Result<()> {
        let (request, _, _, _) = self.request_context();
        let request = request
            .filter(|request| request["status"] == "pending")
            .ok_or_else(|| rejection(&self.command, "This question has already been answered."))?;
        if request["kind"] != "user_input" || request["responseCapability"]["type"] != "message" {
            return Err(rejection(
                &self.command,
                "This question needs an answer. Answer it or stop the turn.",
            ));
        }
        self.command["decision"] = json!("cancel");
        self.runtime_response()
    }

    fn runtime_response(&mut self) -> Result<()> {
        let (request, node, session, item) = self.request_context();
        let mut request = request.ok_or_else(|| {
            rejection(
                &self.command,
                format!(
                    "Runtime request {} was not found.",
                    self.command["requestId"].as_str().unwrap_or_default()
                ),
            )
        })?;
        if request["status"] != "pending" {
            return Err(rejection(
                &self.command,
                format!(
                    "Runtime request {} is {}.",
                    self.command["requestId"].as_str().unwrap_or_default(),
                    request["status"].as_str().unwrap_or_default()
                ),
            ));
        }
        let capability = request["responseCapability"].clone();
        if capability["type"] == "not_resumable" {
            return Err(rejection(
                &self.command,
                capability["reason"].as_str().unwrap_or_default(),
            ));
        }
        if let Some(feedback) = self.command.get("feedback")
            && (capability["type"] != "live"
                || capability["supportsFeedback"] != true
                || request["kind"] == "user_input"
                || self.command["decision"] != "decline"
                || feedback
                    .as_str()
                    .is_none_or(|text| t3_contracts::wire::trim_js(text).is_empty()))
        {
            return Err(rejection(
                &self.command,
                "Feedback requires a live approval that supports it and a decline decision.",
            ));
        }
        if capability["type"] == "live" && session.is_none() {
            return Err(rejection(
                &self.command,
                format!(
                    "Provider session {} was not found.",
                    capability["providerSessionId"].as_str().unwrap_or_default()
                ),
            ));
        }
        let mut answers = self.command.get("answers").cloned();
        if request["kind"] == "user_input"
            && let Some(answers) = &mut answers
        {
            let invalid = || {
                rejection(
                    &self.command,
                    "Answers must be strings, string arrays, or objects containing a string array named answers.",
                )
            };
            for answer in answers.as_object_mut().ok_or_else(invalid)?.values_mut() {
                if answer.is_object() {
                    *answer = answer.get("answers").cloned().ok_or_else(invalid)?;
                }
                if !answer.is_string()
                    && !answer
                        .as_array()
                        .is_some_and(|values| values.iter().all(Value::is_string))
                {
                    return Err(invalid());
                }
            }
        }
        let cancelled = matches!(
            self.command["decision"].as_str(),
            Some("decline" | "cancel")
        );
        let mut replies = Vec::new();
        if capability["type"] == "message" && !cancelled {
            let question = item
                .as_ref()
                .filter(|item| item["type"] == "user_input_request")
                .ok_or_else(|| {
                    rejection(
                        &self.command,
                        "The question for this request was not found.",
                    )
                })?;
            let enabled = self
                .inputs
                .and_then(|inputs| {
                    inputs
                        .provider_enabled
                        .iter()
                        .find(|(instance, _)| {
                            json!(instance) == self.projection["thread"]["providerInstanceId"]
                        })
                        .map(|(_, enabled)| *enabled)
                })
                .ok_or(Error::Unsupported(
                    "message question requires project provider enablement observation",
                ))?;
            if !enabled {
                return Err(rejection(
                    &self.command,
                    "The provider for this project is disabled.",
                ));
            }
            let valid = |answer: &Value| {
                answer
                    .as_str()
                    .is_some_and(|text| !t3_contracts::wire::trim_js(text).is_empty())
                    || answer.as_array().is_some_and(|values| {
                        !values.is_empty()
                            && values.iter().all(|value| {
                                value.as_str().is_some_and(|text| {
                                    !t3_contracts::wire::trim_js(text).is_empty()
                                })
                            })
                    })
            };
            let questions = question["questions"].as_array().ok_or(Error::Stale)?;
            if answers
                .as_ref()
                .and_then(Value::as_object)
                .is_some_and(|answers| {
                    answers.iter().any(|(id, answer)| {
                        !questions
                            .iter()
                            .any(|question| question["id"] == id.as_str())
                            && !valid(answer)
                    })
                })
            {
                return Err(rejection(
                    &self.command,
                    "Answer each question before sending.",
                ));
            }
            for question in questions {
                let answer = answers
                    .as_ref()
                    .and_then(|answers| answers.get(question["id"].as_str().unwrap_or_default()))
                    .unwrap_or(&Value::Null);
                if !valid(answer) {
                    if question["required"] == false {
                        continue;
                    }
                    return Err(rejection(
                        &self.command,
                        "Answer each question before sending.",
                    ));
                }
                let text = answer.as_str().map(str::to_owned).unwrap_or_else(|| {
                    answer
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                });
                replies.push(format!(
                    "{}\n{}",
                    question["question"].as_str().unwrap_or_default(),
                    t3_contracts::wire::trim_js(&text)
                ));
            }
            if replies.is_empty() {
                return Err(rejection(&self.command, "Enter an answer before sending."));
            }
        }
        let status = if cancelled { "cancelled" } else { "completed" };
        let mut scope = json!({"nodeId":request["nodeId"]});
        if let Some(node) = &node {
            scope["runId"] = node["runId"].clone();
        }
        if let Some(session) = &session {
            scope["driver"] = session["driver"].clone();
            scope["providerInstanceId"] = session["providerInstanceId"].clone();
        }
        if let Some(feedback) = self.command.get("feedback").cloned() {
            let message_id = format!(
                "approval-feedback:{}",
                self.command["requestId"].as_str().unwrap_or_default()
            );
            let run_id = node
                .as_ref()
                .map_or(Value::Null, |node| node["runId"].clone());
            let feedback_scope = json!({"nodeId": request["nodeId"], "runId":run_id});
            self.emit("message.updated", json!({"id":message_id,"threadId":self.thread_id,"runId":run_id,"nodeId":request["nodeId"],"role":"user","text":feedback,"attachments":[],"streaming":false,"createdBy":"user","creationSource":"server","createdAt":self.context.now,"updatedAt":self.context.now}), Some(&feedback_scope))?;
            let provider_thread_id = item
                .as_ref()
                .map(|item| &item["providerThreadId"])
                .or_else(|| node.as_ref().map(|node| &node["providerThreadId"]))
                .cloned()
                .unwrap_or(Value::Null);
            let item_id = crate::ports::decisions::orchestrator::derived_id(
                "turn-item",
                &["message", &message_id],
            );
            self.emit("turn-item.updated", json!({"id":item_id,"threadId":self.thread_id,"runId":run_id,"nodeId":request["nodeId"],"providerThreadId":provider_thread_id,"providerTurnId":request["providerTurnId"],"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal(),"status":"completed","title":null,"startedAt":self.context.now,"completedAt":self.context.now,"updatedAt":self.context.now,"type":"user_message","messageId":message_id,"inputIntent":"steer","text":feedback,"attachments":[],"createdBy":"user","creationSource":"server"}), Some(&feedback_scope))?;
        }
        request["status"] = json!("resolved");
        request["resolvedAt"] = json!(self.context.now);
        for name in ["decision", "feedback"] {
            if let Some(value) = self.command.get(name) {
                request[name] = value.clone();
            }
        }
        if let Some(answers) = &answers {
            request["answers"] = answers.clone();
        }
        self.write_record(
            "runtimeRequests",
            "runtime-request.updated",
            request.clone(),
            &scope,
        )?;
        if let Some(mut node) = node {
            node["status"] = json!(status);
            node["completedAt"] = json!(self.context.now);
            self.emit("node.updated", node, Some(&scope))?;
        }
        if let Some(mut item) = item {
            let mut item_scope = scope.clone();
            item_scope["runId"] = item["runId"].clone();
            item_scope["nodeId"] = item["nodeId"].clone();
            if item["type"] == "user_input_request"
                && let Some(answers) = &answers
            {
                let questions: serde_json::Map<String, Value> = item["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|question| {
                        question["id"]
                            .as_str()
                            .map(|id| (id.to_owned(), question["question"].clone()))
                    })
                    .collect();
                item["questionAnswer"] = json!({"requestId":self.command["requestId"],"answers":answers,"attachmentsByQuestionId":self.command.get("attachmentsByQuestionId").cloned().unwrap_or(json!({})),"questionTextById":questions});
            }
            item["status"] = json!(status);
            item["completedAt"] = json!(self.context.now);
            item["updatedAt"] = json!(self.context.now);
            self.emit("turn-item.updated", item, Some(&item_scope))?;
        }
        if capability["type"] == "message" && !cancelled {
            let mut mode = json!({"type":"queue_after_active"});
            if let Some(run) = self
                .records("runs")
                .iter()
                .find(|run| run["status"] == "running")
                && self.records("providerTurns").iter().any(|turn| {
                    turn["runAttemptId"] == run["activeAttemptId"] && turn["status"] == "running"
                })
                && let Some(owner) = self
                    .records("providerThreads")
                    .iter()
                    .find(|owner| owner["id"] == run["providerThreadId"])
                && self.session_present(&owner["providerSessionId"])?
                && self.records("providerSessions").iter().any(|session| {
                    session["id"] == owner["providerSessionId"]
                        && session["capabilities"]["turns"]["supportsActiveSteering"] == true
                })
            {
                mode = json!({"type":"steer_active","targetRunId":run["id"]});
            }
            let original = self.command.clone();
            self.command = json!({"type":"message.dispatch","commandId":original["commandId"],"threadId":self.thread_id,"messageId":format!("async-answer:{}",original["requestId"].as_str().unwrap_or_default()),"text":replies.join("\n\n"),"attachments":[],"createdBy":"user","creationSource":"server","dispatchMode":mode});
            let result = self.message();
            self.command = original;
            return result;
        }
        if capability["type"] != "live" {
            return Ok(());
        }
        if request["kind"] == "user_input" && !cancelled {
            let mut thread = self.thread()?;
            if !thread["snoozedUntil"].is_null() {
                thread["snoozedUntil"] = Value::Null;
                thread["snoozedAt"] = Value::Null;
                thread["updatedAt"] = json!(self.context.now);
                self.emit("thread.unsnoozed", thread, None)?;
            }
        }
        let effect = EffectRequest::Respond {
            provider_session_id: serde_json::from_value(capability["providerSessionId"].clone())?,
            request_id: serde_json::from_value(self.command["requestId"].clone())?,
            decision: self
                .command
                .get("decision")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?,
            feedback: self.command["feedback"].as_str().map(str::to_owned),
            answers: answers.map(serde_json::from_value).transpose()?,
        };
        self.effect(
            &format!(
                "runtime-request.respond:{}",
                self.command["requestId"].as_str().unwrap_or_default()
            ),
            effect,
        );
        Ok(())
    }

    fn rollback_failure(&mut self) -> Result<()> {
        let mut thread = self.projection["thread"].clone();
        if !thread["deletedAt"].is_null() {
            return Ok(());
        }
        if thread["rollbackRequestId"] != self.command["requestId"] {
            return Err(rejection(
                &self.command,
                format!(
                    "Rollback {} is no longer the thread's current rollback.",
                    self.command["requestId"].as_str().unwrap_or_default()
                ),
            ));
        }
        thread["rollbackFailure"] =
            json!({"requestId":self.command["requestId"],"message":self.command["message"]});
        thread["updatedAt"] = json!(self.context.now);
        self.emit("thread.metadata-updated", thread, None)
    }

    fn runtime_delivery_failure(&mut self) -> Result<()> {
        let thread = &self.projection["thread"];
        if !thread["deletedAt"].is_null() {
            return Ok(());
        }
        let (_, node, _, _) = self.request_context();
        let Some(node) = node else {
            return Ok(());
        };
        let scope = json!({"runId":node["runId"],"nodeId":node["id"],"providerInstanceId":thread["providerInstanceId"]});
        self.emit("turn-item.updated", json!({"id":format!("turn-item:runtime-request-delivery-failure:{}",self.command["requestId"].as_str().unwrap_or_default()),"threadId":self.thread_id,"runId":node["runId"],"nodeId":node["id"],"providerThreadId":node["providerThreadId"],"providerTurnId":node["providerTurnId"],"nativeItemRef":null,"parentItemId":null,"ordinal":self.next_item_ordinal(),"status":"failed","title":"Answer delivery not confirmed","startedAt":self.context.now,"completedAt":self.context.now,"updatedAt":self.context.now,"type":"error","failure":{"class":"transport_error","message":"T3 Code could not confirm that your answer reached the agent. If the run stays stuck, stop it and try again.","code":null,"retryable":null}}), Some(&scope))
    }

    fn interrupt_unresponsive(&mut self) -> Result<()> {
        let Some(turn) = self.records("providerTurns").iter().find(|turn| {
            turn["id"] == self.command["providerTurnId"]
                && turn["providerThreadId"] == self.command["providerThreadId"]
        }) else {
            return Ok(());
        };
        let Some(run) = self
            .records("runs")
            .iter()
            .find(|run| run["activeAttemptId"] == turn["runAttemptId"])
            .cloned()
        else {
            return Ok(());
        };
        if self
            .records("turnItems")
            .iter()
            .any(|item| item["runId"] == run["id"] && item["type"] == "run_interrupt_result")
        {
            return Ok(());
        }
        let Some(mut item) = self
            .records("turnItems")
            .iter()
            .find(|item| item["runId"] == run["id"] && item["type"] == "run_interrupt_request")
            .cloned()
        else {
            return Ok(());
        };
        if item.get("unresponsiveAt").is_some() {
            return Ok(());
        }
        let mut scope = Self::run_scope(&run);
        scope["nodeId"] = item["nodeId"].clone();
        item["unresponsiveAt"] = json!(self.context.now);
        item["updatedAt"] = json!(self.context.now);
        self.emit("turn-item.updated", item, Some(&scope))
    }

    fn allocate(&mut self, kind: IdKind) -> Result<String> {
        let cursor = self
            .id_positions
            .iter()
            .position(|(candidate, _)| *candidate == kind);
        let start = cursor.map_or(0, |cursor| self.id_positions[cursor].1);
        let (index, (_, id)) = self
            .context
            .ids
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, (candidate, _))| *candidate == kind)
            .ok_or(Error::Service {
                operation: "DecisionEngine.decide",
                detail: format!("No preallocated {kind:?} ID remains."),
            })?;
        if let Some(cursor) = cursor {
            self.id_positions[cursor].1 = index + 1;
        } else {
            self.id_positions.push((kind, index + 1));
        }
        Ok(id.clone())
    }

    fn emit(&mut self, kind: &str, payload: Value, scope: Option<&Value>) -> Result<()> {
        let mut event = json!({ "id": self.allocate(IdKind::Event)?, "threadId": self.thread_id, "type": kind, "occurredAt": self.context.now, "payload": payload });
        let source = scope.unwrap_or(&self.projection["thread"]);
        if let Some(provider) = source.get("providerInstanceId").filter(|id| !id.is_null()) {
            event["providerInstanceId"] = provider.clone();
        }
        if let Some(run) = source.get("runId").filter(|id| !id.is_null()) {
            event["runId"] = run.clone();
        }
        if let Some(node) = source.get("nodeId").filter(|id| !id.is_null()) {
            event["nodeId"] = node.clone();
        }
        if let Some(driver) = source.get("driver").filter(|id| !id.is_null()) {
            event["driver"] = driver.clone();
        }
        self.decision.events.push(serde_json::from_value(event)?);
        Ok(())
    }

    fn effect(&mut self, suffix: &str, request: EffectRequest) {
        let command_id = CommandId(
            self.command["commandId"]
                .as_str()
                .unwrap_or_default()
                .into(),
        );
        self.decision.effects.push(PendingEffect {
            id: format!("effect:{}:{suffix}", command_id.0),
            command_id,
            thread_id: self.thread_id.clone(),
            request,
            available_at: None,
        });
    }

    fn thread(&self) -> Result<Value> {
        if self.projection.is_null() {
            return Err(Error::ThreadNotFound(self.thread_id.clone()));
        }
        let thread = &self.projection["thread"];
        if !thread["deletedAt"].is_null() {
            return Err(rejection(
                &self.command,
                format!("Thread {} is deleted.", self.thread_id.0),
            ));
        }
        Ok(thread.clone())
    }

    fn records(&self, kind: &str) -> &[Value] {
        self.projection[kind]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn create(&mut self) -> Result<()> {
        if self.context.projection.is_some() {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} already exists. Use a new thread id.",
                    self.thread_id.0
                ),
            ));
        }
        let mut thread = json!({
            "createdBy": self.command["createdBy"], "creationSource": self.command["creationSource"],
            "id": self.thread_id, "projectId": self.command["projectId"], "title": self.command["title"],
            "providerInstanceId": self.command["modelSelection"]["instanceId"], "modelSelection": self.command["modelSelection"],
            "runtimeMode": self.command["runtimeMode"], "interactionMode": self.command["interactionMode"],
            "branch": self.command["branch"], "worktreePath": self.command["worktreePath"],
            "activeProviderThreadId": null, "pullRequests": [],
            "lineage": {"parentThreadId": null, "relationshipToParent": null, "rootThreadId": self.thread_id},
            "forkedFrom": null, "createdAt": self.context.now, "updatedAt": self.context.now,
            "archivedAt": null, "settledOverride": null, "settledAt": null, "snoozedUntil": null,
            "snoozedAt": null, "lastVisitedAt": null, "deletedAt": null
        });
        if let Some(origin) = self.command.get("linkOrigin") {
            thread["linkOrigin"] = origin.clone();
            if let Some(from) = self.command.get("delegatedFrom") {
                thread["delegatedFrom"] = from.clone();
            }
        }
        self.emit("thread.created", thread.clone(), Some(&thread))?;
        if let Some(import) = self.command.get("importedNativeThread").cloned() {
            let native = &import["ref"];
            let provider_thread_id = crate::ports::decisions::orchestrator::derived_id(
                "provider-thread",
                &[
                    "provider",
                    native["driver"].as_str().ok_or(Error::Stale)?,
                    "provider-instance",
                    self.command["modelSelection"]["instanceId"]
                        .as_str()
                        .ok_or(Error::Stale)?,
                    "native-thread",
                    native["nativeId"].as_str().ok_or(Error::Stale)?,
                ],
            );
            let provider = self.command["modelSelection"]["instanceId"].clone();
            let payload = json!({"id": provider_thread_id, "driver": native["driver"], "providerInstanceId": provider,
                "providerSessionId": null, "appThreadId": self.thread_id, "ownerNodeId": null,
                "nativeThreadRef": native, "nativeConversationHeadRef": null, "status": "not_loaded",
                "firstRunOrdinal": null, "lastRunOrdinal": null, "handoffIds": [], "forkedFrom": null,
                "contextUsage": null, "nativeMetadata": import.get("metadata").unwrap_or(&Value::Null),
                "createdAt": self.context.now, "updatedAt": self.context.now});
            let scope = json!({"driver": native["driver"], "providerInstanceId": provider});
            self.emit("provider-thread.updated", payload, Some(&scope))?;
        }
        Ok(())
    }

    fn guest_share(&mut self, command: &OrchestrationV2ServerCommand) -> Result<()> {
        self.thread()?;
        let thread = &self
            .context
            .projection
            .as_ref()
            .ok_or_else(|| Error::ThreadNotFound(self.thread_id.clone()))?
            .thread;
        use OrchestrationV2Command as Client;
        use OrchestrationV2InternalCommand as Internal;
        use OrchestrationV2ServerCommand as Server;
        let guest = match command {
            Server::Client(command) => match command.as_ref() {
                Client::ThreadGuestShareClose(_) => GuestShareCommand::Close,
                Client::ThreadGuestMessageDismiss(command) => {
                    GuestShareCommand::Dismiss(&command.guest_message_id)
                }
                _ => return Err(Error::Unsupported("not a guest command")),
            },
            Server::Internal(command) => match command.as_ref() {
                Internal::ThreadGuestShareOpen(command) => GuestShareCommand::Open(command),
                Internal::ThreadGuestShareRedeem(command) => GuestShareCommand::Redeem(command),
                Internal::ThreadGuestMessageSubmit(command) => GuestShareCommand::Submit(command),
                _ => return Err(Error::Unsupported("not a guest command")),
            },
        };
        if matches!(guest, GuestShareCommand::Close)
            && thread
                .guest_share
                .as_ref()
                .and_then(Option::as_ref)
                .is_none()
        {
            return Ok(());
        }
        let updated =
            guest_share_decider::decide_guest_share_command(thread, guest, &self.context.now)
                .map_err(|reason| rejection(&self.command, reason))?;
        let attachment_ids = guest_share_decider::released_guest_attachment_ids(
            thread.guest_share.as_ref().and_then(Option::as_ref),
            updated.guest_share.as_ref().and_then(Option::as_ref),
        );
        self.emit("thread.guest-share-updated", stored_value(updated)?, None)?;
        if !attachment_ids.is_empty() {
            self.effect(
                "guest-attachment.cleanup",
                EffectRequest::CleanupAttachments { attachment_ids },
            );
        }
        Ok(())
    }

    fn thread_mutation(&mut self, kind: &str) -> Result<()> {
        let mut thread = self.thread()?;
        let now = stored_value(&self.context.now)?;
        let id = &self.thread_id.0;
        if matches!(
            kind,
            "thread.unsettle"
                | "thread.snooze"
                | "thread.unsnooze"
                | "thread.auto-settle.set"
                | "thread.pin"
                | "thread.unpin"
                | "thread.pin.reorder"
                | "thread.active.reorder"
        ) && !thread["archivedAt"].is_null()
        {
            return Err(rejection(
                &self.command,
                format!("Thread {id} is archived."),
            ));
        }
        let event_type = match kind {
            "thread.unarchive" => {
                if thread["archivedAt"].is_null() {
                    return Err(rejection(
                        &self.command,
                        format!("Thread {id} is not archived."),
                    ));
                }
                thread["archivedAt"] = Value::Null;
                thread["updatedAt"] = now;
                "thread.unarchived"
            }
            "thread.unsettle" => {
                if thread["settledOverride"] != "active" {
                    thread["unsettledAt"] = now.clone();
                    thread["updatedAt"] = now;
                } else if thread.get("unsettledAt").is_none() {
                    thread["unsettledAt"] = Value::Null;
                }
                thread["settledOverride"] = json!("active");
                thread["settledAt"] = Value::Null;
                "thread.unsettled"
            }
            "thread.snooze" => {
                if epoch_millis(self.command["snoozedUntil"].as_str().unwrap_or_default())
                    .zip(epoch_millis(&self.context.now.0))
                    .is_none_or(|(wake, now)| wake <= now)
                {
                    return Err(rejection(
                        &self.command,
                        format!(
                            "Thread {id} snooze wake time {} is not in the future.",
                            self.command["snoozedUntil"].as_str().unwrap_or_default()
                        ),
                    ));
                }
                if self.records("runtimeRequests").iter().any(|request| {
                    request["status"] == "pending" && request["kind"] != "user_input"
                }) {
                    return Err(rejection(
                        &self.command,
                        format!("Thread {id} has a pending approval and cannot be snoozed."),
                    ));
                }
                if self.records("runs").iter().any(|run| {
                    run["status"] == "queued"
                        && !self.records("messages").iter().any(|message| {
                            message["id"] == run["userMessageId"]
                                && (message.get("notification").is_some()
                                    || message.get("delegatedCompletion").is_some())
                        })
                }) {
                    return Err(rejection(
                        &self.command,
                        format!("Thread {id} has a queued run and cannot be snoozed."),
                    ));
                }
                thread["snoozedUntil"] = self.command["snoozedUntil"].clone();
                thread["snoozedAt"] = now.clone();
                thread["updatedAt"] = now;
                if thread["limitRecovery"].is_object() {
                    thread["limitRecovery"]["snooze"] = json!(false);
                } else {
                    thread["limitRecovery"] = Value::Null;
                }
                "thread.snoozed"
            }
            "thread.unsnooze" => {
                if !thread["snoozedUntil"].is_null() {
                    thread["lastSnoozeWakeAt"] = now.clone();
                    thread["updatedAt"] = now;
                }
                thread["snoozedUntil"] = Value::Null;
                thread["snoozedAt"] = Value::Null;
                "thread.unsnoozed"
            }
            "thread.auto-settle.set" => {
                let enabled = self.command["enabled"] == true;
                if enabled != thread["autoSettleDisabledAt"].is_null() {
                    thread["updatedAt"] = now.clone();
                }
                thread["autoSettleDisabledAt"] = if enabled {
                    Value::Null
                } else if thread["autoSettleDisabledAt"].is_null() {
                    now
                } else {
                    thread["autoSettleDisabledAt"].clone()
                };
                "thread.auto-settle-set"
            }
            "thread.pin" => {
                let pinned = !thread["pinnedAt"].is_null();
                let promotes =
                    thread["settledOverride"] == "settled" || !thread["snoozedUntil"].is_null();
                if !pinned {
                    thread["pinnedAt"] = now.clone();
                    if let Some(key) = self.command.get("orderKey") {
                        thread["pinOrderKey"] = key.clone();
                    }
                }
                if thread["settledOverride"] == "settled" {
                    thread["settledOverride"] = json!("active");
                    thread["settledAt"] = Value::Null;
                    thread["unsettledAt"] = now.clone();
                }
                if !thread["snoozedUntil"].is_null() {
                    thread["lastSnoozeWakeAt"] = now.clone();
                }
                thread["snoozedUntil"] = Value::Null;
                thread["snoozedAt"] = Value::Null;
                if !pinned || promotes {
                    thread["updatedAt"] = now;
                }
                "thread.pinned"
            }
            "thread.unpin" => {
                if !thread["pinnedAt"].is_null() {
                    thread["updatedAt"] = now;
                }
                thread["pinnedAt"] = Value::Null;
                thread["pinOrderKey"] = Value::Null;
                "thread.unpinned"
            }
            "thread.pin.reorder" => {
                if thread["pinnedAt"].is_null() {
                    return Err(rejection(
                        &self.command,
                        format!("Thread {id} is not pinned and cannot be reordered."),
                    ));
                }
                if thread["pinOrderKey"] != self.command["orderKey"] {
                    thread["updatedAt"] = now;
                }
                thread["pinOrderKey"] = self.command["orderKey"].clone();
                "thread.pin-reordered"
            }
            "thread.active.reorder" => {
                if !thread["pinnedAt"].is_null() || thread["settledOverride"] == "settled" {
                    return Err(rejection(
                        &self.command,
                        format!("Thread {id} is not active and cannot be reordered."),
                    ));
                }
                thread["activeOrderKey"] = self.command["orderKey"].clone();
                "thread.active-reordered"
            }
            "thread.tag.set" => {
                let mut tags = thread["tags"].as_array().cloned().unwrap_or_default();
                if self.command["tagged"] == true {
                    if !tags.contains(&self.command["tag"]) {
                        tags.push(self.command["tag"].clone());
                    }
                } else {
                    tags.retain(|tag| tag != &self.command["tag"]);
                }
                thread["tags"] = json!(tags);
                "thread.tags-updated"
            }
            "thread.mark-unread" => {
                let shell = self.inputs.and_then(|inputs| {
                    inputs
                        .shell_completions
                        .iter()
                        .find(|(thread, _)| thread == &self.thread_id)
                });
                let latest = if let Some((_, completed)) = shell {
                    completed.as_ref().and_then(|date| epoch_millis(&date.0))
                } else {
                    self.records("runs")
                        .iter()
                        .max_by_key(|run| run["ordinal"].as_u64().unwrap_or_default())
                        .and_then(|run| run["completedAt"].as_str())
                        .and_then(epoch_millis)
                }
                .ok_or_else(|| {
                    rejection(
                        &self.command,
                        format!("Thread {id} has no completed run to mark unread."),
                    )
                })?;
                thread["lastVisitedAt"] = json!(iso_from_epoch_millis(latest - 1));
                if thread["snoozedUntil"]
                    .as_str()
                    .and_then(epoch_millis)
                    .zip(epoch_millis(&self.context.now.0))
                    .is_some_and(|(wake, now)| wake <= now)
                {
                    thread["lastSnoozeWakeAt"] = thread["snoozedUntil"].clone();
                    thread["snoozedUntil"] = Value::Null;
                    thread["snoozedAt"] = Value::Null;
                }
                "thread.marked-unread"
            }
            "thread.visit" => {
                let visited = self.command["visitedAt"]
                    .as_str()
                    .and_then(epoch_millis)
                    .ok_or_else(|| {
                        rejection(
                            &self.command,
                            format!(
                                "Thread {id} visit time {} is not a valid timestamp.",
                                self.command["visitedAt"].as_str().unwrap_or_default()
                            ),
                        )
                    })?;
                if thread["lastVisitedAt"]
                    .as_str()
                    .and_then(epoch_millis)
                    .is_none_or(|previous| visited > previous)
                {
                    thread["lastVisitedAt"] = self.command["visitedAt"].clone();
                }
                if self.command["opened"] == true {
                    let picked = crate::ports::decisions::orchestrator::picked_search_query(
                        self.command["searchQuery"].as_str().unwrap_or_default(),
                    )?;
                    thread["opens"] = crate::ports::decisions::orchestrator::record_thread_open(
                        &thread["opens"],
                        epoch_millis(&self.context.now.0).ok_or(Error::Stale)?,
                        &picked,
                    );
                }
                "thread.visited"
            }
            "thread.title.regeneration.complete" => {
                if thread["titleRegeneration"]["requestId"] == self.command["requestId"] {
                    if let Some(title) = self.command.get("title") {
                        thread["title"] = title.clone();
                    }
                    if let Some(outcome) = self.command.get("outcome") {
                        let mut outcome = outcome.clone();
                        outcome["requestId"] = self.command["requestId"].clone();
                        outcome["completedAt"] = now.clone();
                        thread["lastTitleRegeneration"] = outcome;
                    }
                    thread["titleRegeneration"] = Value::Null;
                    thread["updatedAt"] = now;
                }
                "thread.metadata-updated"
            }
            "thread.interaction-mode.set" => {
                thread["interactionMode"] = self.command["interactionMode"].clone();
                thread["updatedAt"] = now;
                "thread.interaction-mode-updated"
            }
            _ => return Err(Error::Unsupported("thread mutation")),
        };
        self.emit(event_type, thread, None)
    }

    fn provider_selection_plan(
        &self,
        target: &t3_contracts::model_selection::ModelSelection,
    ) -> Result<(bool, Vec<Value>)> {
        use crate::ports::sessions_recovery::provider_session_transition_policy::{
            ContinuationIdentity, ProviderSessionTransition, SessionTransitionState,
            decide_provider_session_transition,
        };
        let current: t3_contracts::model_selection::ModelSelection =
            serde_json::from_value(self.projection["thread"]["modelSelection"].clone())?;
        let metadata = |id: &ProviderInstanceId| {
            self.inputs.and_then(|inputs| {
                inputs
                    .provider_metadata
                    .iter()
                    .find(|(instance, _)| instance == id)
                    .map(|(_, metadata)| metadata)
            })
        };
        let mut sessions: Vec<_> = self
            .records("providerSessions")
            .iter()
            .filter(|session| session["providerInstanceId"] == json!(current.instance_id))
            .collect();
        sessions.sort_by(|a, b| {
            b["updatedAt"]
                .as_str()
                .and_then(epoch_millis)
                .cmp(&a["updatedAt"].as_str().and_then(epoch_millis))
        });
        let live = sessions
            .iter()
            .copied()
            .find(|session| !matches!(session["status"].as_str(), Some("stopped" | "error")));
        let native = self.records("providerThreads").iter().any(|provider| {
            provider["id"] == self.projection["thread"]["activeProviderThreadId"]
                && provider["providerInstanceId"] == json!(current.instance_id)
                && !provider["nativeThreadRef"].is_null()
        });
        let state = |metadata: &AdapterMetadata, selection, workspace| -> Result<_> {
            Ok(SessionTransitionState {
                driver: metadata.driver.clone(),
                continuation_identity: ContinuationIdentity {
                    driver: metadata.driver.clone(),
                    key: metadata.continuation_key.clone(),
                },
                model_selection: selection,
                runtime_mode: serde_json::from_value(
                    self.projection["thread"]["runtimeMode"].clone(),
                )?,
                interaction_mode: serde_json::from_value(
                    self.projection["thread"]["interactionMode"].clone(),
                )?,
                workspace,
            })
        };
        let current_cwd = live
            .and_then(|session| session["cwd"].as_str())
            .or_else(|| self.projection["thread"]["worktreePath"].as_str())
            .unwrap_or("<unresolved-workspace>")
            .to_owned();
        let target_cwd = self.projection["thread"]["worktreePath"]
            .as_str()
            .or_else(|| live.and_then(|session| session["cwd"].as_str()))
            .unwrap_or("<unresolved-workspace>")
            .to_owned();
        let current_state = metadata(&current.instance_id)
            .filter(|_| live.is_some() || native)
            .map(|metadata| state(metadata, current.clone(), current_cwd))
            .transpose()?;
        let target_metadata = metadata(&target.instance_id).ok_or(Error::Unsupported(
            "selection change requires target provider metadata",
        ))?;
        let transition = decide_provider_session_transition(
            current_state.as_ref(),
            &state(target_metadata, target.clone(), target_cwd)?,
            target_metadata.enabled,
            self.inputs
                .and_then(|inputs| inputs.selection_transition.as_ref()),
        );
        if let ProviderSessionTransition::Reject { reason } = transition {
            return Err(rejection(&self.command, reason));
        }
        let release = self
            .records("providerSessions")
            .iter()
            .filter(|session| !matches!(session["status"].as_str(), Some("stopped" | "error")))
            .filter(|session| match transition {
                ProviderSessionTransition::RestartAndResume => {
                    live.is_some_and(|live| live["id"] == session["id"])
                }
                ProviderSessionTransition::CreateWithHandoff => {
                    session["providerInstanceId"] != json!(target.instance_id)
                }
                _ => false,
            })
            .cloned()
            .collect();
        Ok((current.instance_id != target.instance_id, release))
    }

    fn selection_mutation(&mut self, kind: &str) -> Result<()> {
        let mut thread = self.thread()?;
        let now = json!(self.context.now);
        let mut detaching = Vec::new();
        let event_type = match kind {
            "thread.metadata.update" => {
                for (expected, current, reason) in [
                    ("expectedWorktreePath", "worktreePath", "worktree"),
                    ("expectedBranch", "branch", "branch"),
                ] {
                    if self
                        .command
                        .get(expected)
                        .is_some_and(|expected| expected != &thread[current])
                    {
                        return Err(rejection(
                            &self.command,
                            format!(
                                "Thread {} {reason} changed before the metadata update could be applied.",
                                self.thread_id.0
                            ),
                        ));
                    }
                }
                if self.command["expectedEmpty"] == true
                    && (!self.records("runs").is_empty() || !self.records("messages").is_empty())
                {
                    return Err(rejection(
                        &self.command,
                        format!("Thread {} is no longer empty.", self.thread_id.0),
                    ));
                }
                if self.command.get("linkedPullRequest").is_some() {
                    let mut request = self.command.clone();
                    request["type"] = json!("thread.pull-request.sync");
                    request["branchPullRequest"] = thread["branchPullRequest"].clone();
                    thread = crate::ports::decisions::orchestrator::pull_request_mutation(
                        thread,
                        &request,
                        &self.context.now,
                        prepared_pull_request_identities(self.inputs)?,
                    )?;
                }
                if let Some(requested) = self.command.get("limitRecovery").cloned() {
                    let previous = thread["limitRecovery"].clone();
                    let now_ms = epoch_millis(&self.context.now.0).ok_or(Error::Stale)?;
                    let recovery = if requested.is_null() {
                        Value::Null
                    } else {
                        let run = crate::ports::decisions::model_switch::latest_executed_run(
                            &self.context.projection.as_ref().ok_or(Error::Stale)?.runs,
                        )
                        .map(stored_value)
                        .transpose()?;
                        let failure = run.as_ref().and_then(|run| self.root_failure(run));
                        let reset = requested["resetAt"].as_str().and_then(epoch_millis);
                        if requested["snooze"] == true && reset.is_some_and(|reset| reset <= now_ms)
                        {
                            return Err(rejection(
                                &self.command,
                                "The reset time has passed. Retry the thread manually.",
                            ));
                        }
                        if reset.is_none()
                            || !thread["archivedAt"].is_null()
                            || thread["settledOverride"] == "settled"
                            || run.as_ref().is_none_or(|run| {
                                run["id"] != requested["runId"]
                                    || reset
                                        <= run["completedAt"]
                                            .as_str()
                                            .or_else(|| run["requestedAt"].as_str())
                                            .and_then(epoch_millis)
                            })
                            || failure.is_none_or(|failure| {
                                failure["class"] != "usage_limit"
                                    || failure["resetAt"] != requested["resetAt"]
                            })
                            || self
                                .records("runtimeRequests")
                                .iter()
                                .any(|request| request["status"] == "pending")
                        {
                            return Err(rejection(
                                &self.command,
                                "The provider limit changed before recovery could be configured.",
                            ));
                        }
                        let same = previous["runId"] == requested["runId"]
                            && previous["resetAt"] == requested["resetAt"];
                        let mut recovery = requested.clone();
                        recovery["autoResume"] = json!(
                            requested["autoResume"]
                                .as_bool()
                                .or_else(|| same
                                    .then(|| previous["autoResume"].as_bool())
                                    .flatten())
                                .unwrap_or(false)
                        );
                        recovery["snooze"] = json!(
                            requested["snooze"]
                                .as_bool()
                                .or_else(|| same.then(|| previous["snooze"].as_bool()).flatten())
                                .unwrap_or(false)
                        );
                        recovery["requestId"] = self.command["commandId"].clone();
                        recovery
                    };
                    if recovery["snooze"] == true
                        && recovery["resetAt"]
                            .as_str()
                            .and_then(epoch_millis)
                            .is_some_and(|reset| reset > now_ms)
                    {
                        thread["snoozedUntil"] = recovery["resetAt"].clone();
                        thread["snoozedAt"] = now.clone();
                    } else if previous["snooze"] == true
                        && thread["snoozedUntil"].as_str().and_then(epoch_millis)
                            == previous["resetAt"].as_str().and_then(epoch_millis)
                        && !thread["snoozedUntil"].is_null()
                    {
                        thread["snoozedUntil"] = Value::Null;
                        thread["snoozedAt"] = Value::Null;
                        thread["lastSnoozeWakeAt"] = now.clone();
                    }
                    thread["limitRecovery"] = recovery;
                }
                if self
                    .command
                    .get("worktreePath")
                    .is_some_and(|path| path != &thread["worktreePath"])
                {
                    detaching.extend(self.records("providerSessions").iter().cloned());
                }
                for field in ["title", "branch", "worktreePath"] {
                    if let Some(value) = self.command.get(field) {
                        thread[field] = value.clone();
                    }
                }
                if self.command["regenerateTitle"] == true {
                    thread["titleRegeneration"] =
                        json!({"requestId":self.command["commandId"],"startedAt":now});
                } else if self.command["regenerateTitle"] == false
                    || self.command.get("title").is_some()
                {
                    thread["titleRegeneration"] = Value::Null;
                }
                "thread.metadata-updated"
            }
            "thread.runtime-mode.set" => {
                detaching.extend(self.records("providerSessions").iter().filter(|session| session["capabilities"]["sessions"]["supportsRuntimeModeSwitchInSession"] != true).cloned());
                thread["runtimeMode"] = self.command["runtimeMode"].clone();
                "thread.runtime-mode-updated"
            }
            "thread.model-selection.set" | "provider.switch" => {
                let changed =
                    thread["providerInstanceId"] != self.command["modelSelection"]["instanceId"];
                if !self.records("providerSessions").is_empty()
                    || !self.records("providerThreads").is_empty()
                {
                    let (_, released) = self.provider_selection_plan(&serde_json::from_value(
                        self.command["modelSelection"].clone(),
                    )?)?;
                    detaching.extend(released);
                }
                if (changed || !detaching.is_empty())
                    && self.records("runs").iter().any(|run| {
                        matches!(
                            run["status"].as_str(),
                            Some("preparing" | "starting" | "running" | "waiting")
                        )
                    })
                {
                    return Err(rejection(
                        &self.command,
                        format!(
                            "Thread {} has an active turn. Wait for it to finish, then retry the provider selection change.",
                            self.thread_id.0
                        ),
                    ));
                }
                thread["providerInstanceId"] = self.command["modelSelection"]["instanceId"].clone();
                thread["modelSelection"] = self.command["modelSelection"].clone();
                if changed || kind == "provider.switch" {
                    "thread.provider-switched"
                } else {
                    "thread.model-selection-updated"
                }
            }
            _ => return Err(Error::Unsupported("selection mutation")),
        };
        detaching
            .retain(|session| !matches!(session["status"].as_str(), Some("stopped" | "error")));
        for session in &detaching {
            if self.session_present(&session["id"])?
                && self
                    .records("providerThreads")
                    .iter()
                    .filter(|provider| provider["providerSessionId"] == session["id"])
                    .any(|provider| {
                        self.inputs.is_some_and(|inputs| {
                            inputs
                                .background_work
                                .iter()
                                .any(|(id, pending)| json!(id) == provider["id"] && *pending)
                        })
                    })
            {
                return Err(rejection(
                    &self.command,
                    "The current session is still running background agents or commands, and this change would end them. Wait for them to finish, or press Stop, then try again.",
                ));
            }
        }
        thread["updatedAt"] = now;
        self.emit(event_type, thread.clone(), Some(&thread))?;
        if kind == "thread.model-selection.set"
            && detaching.is_empty()
            && let Some(run) = self
                .records("runs")
                .iter()
                .find(|run| run["status"] == "running")
            && let Some(provider) = self
                .records("providerThreads")
                .iter()
                .find(|provider| provider["id"] == run["providerThreadId"])
            && self.records("providerTurns").iter().any(|turn| {
                turn["runAttemptId"] == run["activeAttemptId"] && turn["status"] == "running"
            })
            && self.session_present(&provider["providerSessionId"])?
            && session_supports_selection(self.inputs, &provider["providerSessionId"])?
        {
            self.effect(
                "provider-session.apply-model-selection",
                EffectRequest::ApplySelection {
                    provider_session_id: serde_json::from_value(
                        provider["providerSessionId"].clone(),
                    )?,
                    provider_thread_id: serde_json::from_value(provider["id"].clone())?,
                    model_selection: serde_json::from_value(
                        self.command["modelSelection"].clone(),
                    )?,
                },
            );
        }
        let reason = if kind == "thread.metadata.update" {
            "Workspace changed."
        } else if kind == "thread.runtime-mode.set" {
            "Runtime mode changed."
        } else {
            "Provider or model selection changed."
        };
        for session in detaching {
            self.emit("provider-session.detached",json!({"providerSessionId":session["id"],"detachedAt":self.context.now,"reason":reason}),Some(&session))?;
            self.effect(
                &format!(
                    "provider-session.detach:{}",
                    session["id"].as_str().unwrap_or_default()
                ),
                EffectRequest::Detach {
                    provider_session_id: serde_json::from_value(session["id"].clone())?,
                    detail: Some(reason.into()),
                    revoke_mcp_credential: None,
                },
            );
        }
        if kind == "thread.metadata.update" && self.command["regenerateTitle"] == true {
            self.effect(
                "thread-title.generate",
                EffectRequest::GenerateTitle {
                    kind: TitleKind::Regenerate,
                },
            );
        }
        Ok(())
    }

    fn settlement(&mut self, kind: &str) -> Result<()> {
        let original = self.thread()?;
        let mut thread = original.clone();
        let now = json!(self.context.now);
        let archive = kind == "thread.archive";
        if kind == "thread.auto-settle"
            && (!thread["settledOverride"].is_null()
                || thread["updatedAt"]
                    .as_str()
                    .and_then(epoch_millis)
                    .zip(self.command["snapshotAt"].as_str().and_then(epoch_millis))
                    .is_some_and(|(updated, snapshot)| updated > snapshot))
        {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} changed before automatic settlement.",
                    self.thread_id.0
                ),
            ));
        }
        if !thread["archivedAt"].is_null() {
            return Err(rejection(
                &self.command,
                format!(
                    "Thread {} is {}archived.",
                    self.thread_id.0,
                    if archive { "already " } else { "" }
                ),
            ));
        }
        if archive {
            if self.records("subagents").iter().any(|task| {
                task["origin"] == "app_owned"
                    && !task["childThreadId"].is_null()
                    && matches!(
                        task["status"].as_str(),
                        Some("pending" | "running" | "waiting")
                    )
            }) {
                return Err(rejection(
                    &self.command,
                    format!(
                        "Thread {} has running delegated tasks. Wait for them or cancel them before archiving.",
                        self.thread_id.0
                    ),
                ));
            }
            let cohort_ids: Vec<_> =
                self.records("runs")
                    .iter()
                    .filter(|run| {
                        run["delegatedCompletion"].is_object()
                            || self.records("subagents").iter().any(|task| {
                                task["origin"] == "app_owned" && task["runId"] == run["id"]
                            })
                    })
                    .map(|run| run["id"].clone())
                    .collect();
            for id in cohort_ids {
                self.dispose_cohort_with_cancel(&id, "disposed", None, false)?;
            }
            thread["archivedAt"] = now.clone();
            thread["titleRegeneration"] = Value::Null;
            thread["guestShare"] = Value::Null;
            thread["updatedAt"] = now.clone();
        } else {
            if let Some(expected) = self.command.get("expectedLatestRunId")
                && self
                    .records("runs")
                    .iter()
                    .max_by_key(|run| run["ordinal"].as_u64())
                    .map(|run| &run["id"])
                    != Some(expected)
            {
                return Err(rejection(
                    &self.command,
                    "The thread received another run before settlement.",
                ));
            }
            let blocking_request = self.records("runtimeRequests").iter().any(|request| {
                request["status"] == "pending"
                    && (request["kind"] != "user_input"
                        || request["responseCapability"]["type"] != "message")
            });
            let running = self.records("runs").iter().any(|run| {
                matches!(
                    run["status"].as_str(),
                    Some("preparing" | "starting" | "running" | "waiting")
                )
            });
            let queued = self.records("runs").iter().any(|run| {
                run["status"] == "queued"
                    && !self.records("messages").iter().any(|message| {
                        message["id"] == run["userMessageId"]
                            && message["role"] == "user"
                            && (message.get("notification").is_some()
                                || message.get("delegatedCompletion").is_some())
                    })
            });
            if let Some(blocker) = if blocking_request {
                Some("is waiting on an approval or question. Answer it before settling.")
            } else if running {
                Some("is still running. Stop it before settling.")
            } else if queued {
                Some("has a queued message. Send it or remove it from the queue before settling.")
            } else {
                None
            } {
                return Err(rejection(
                    &self.command,
                    format!("Thread {} {blocker}", self.thread_id.0),
                ));
            }
            for run in self
                .records("runs")
                .to_vec()
                .into_iter()
                .filter(|run| run["status"] == "queued")
            {
                if let Some(message) = self
                    .records("messages")
                    .iter()
                    .find(|message| message["id"] == run["userMessageId"])
                    .cloned()
                    && let Some(completion) = message.get("delegatedCompletion")
                {
                    self.dispose_cohort_with_cancel(
                        &completion["parentRunId"],
                        "disposed",
                        None,
                        false,
                    )?;
                }
                self.cancel_queued_run(&run)?;
            }
            for mut request in
                self.records("runtimeRequests")
                    .to_vec()
                    .into_iter()
                    .filter(|request| {
                        request["status"] == "pending"
                            && request["kind"] == "user_input"
                            && request["responseCapability"]["type"] == "message"
                    })
            {
                request["status"] = json!("cancelled");
                request["resolvedAt"] = json!(self.context.now);
                request["responseCapability"] =
                    json!({"type":"not_resumable","reason":"The thread was settled."});
                let scope = json!({"nodeId":request["nodeId"]});
                self.write_record(
                    "runtimeRequests",
                    "runtime-request.updated",
                    request.clone(),
                    &scope,
                )?;
                if let Some(mut node) = self
                    .records("nodes")
                    .iter()
                    .find(|node| node["id"] == request["nodeId"])
                    .cloned()
                {
                    node["status"] = json!("cancelled");
                    node["completedAt"] = json!(self.context.now);
                    self.write_record("nodes", "node.updated", node, &scope)?;
                }
            }
            let already = thread["settledOverride"] == "settled"
                && !thread["settledAt"].is_null()
                && thread["pinnedAt"].is_null();
            if !already {
                thread["settledAt"] = self
                    .command
                    .get("settledAt")
                    .filter(|at| !at.is_null())
                    .cloned()
                    .unwrap_or_else(|| now.clone());
            }
            if !already || !thread["snoozedUntil"].is_null() || !thread["snoozedAt"].is_null() {
                thread["updatedAt"] = now.clone();
            }
            thread["settledOverride"] = json!("settled");
            for field in [
                "unsettledAt",
                "pinnedAt",
                "pinOrderKey",
                "activeOrderKey",
                "snoozedUntil",
                "snoozedAt",
            ] {
                thread[field] = Value::Null;
            }
        }
        if let Some(links) = thread.get_mut("pullRequests").and_then(Value::as_array_mut) {
            for link in links {
                if let Some(watch) = link
                    .as_object_mut()
                    .and_then(|link| link.shift_remove("watch"))
                {
                    link["endedWatch"] = watch;
                }
            }
        }
        self.emit(
            if archive {
                "thread.archived"
            } else {
                "thread.settled"
            },
            thread,
            None,
        )?;
        if archive {
            self.archive_work()?;
            let before = self.context.projection.as_ref().and_then(|projection| {
                projection
                    .thread
                    .guest_share
                    .as_ref()
                    .and_then(Option::as_ref)
            });
            let attachment_ids = guest_share_decider::released_guest_attachment_ids(before, None);
            if !attachment_ids.is_empty() {
                self.effect(
                    "guest-attachment.cleanup",
                    EffectRequest::CleanupAttachments { attachment_ids },
                );
            }
        }
        let reason = if archive {
            "Thread archived."
        } else {
            "Thread settled."
        };
        let sessions: Vec<_> = self
            .records("providerSessions")
            .iter()
            .filter(|session| !matches!(session["status"].as_str(), Some("stopped" | "error")))
            .cloned()
            .collect();
        for session in sessions {
            self.emit("provider-session.detached",json!({"providerSessionId":session["id"],"detachedAt":self.context.now,"reason":reason}),Some(&session))?;
            self.effect(
                &format!(
                    "provider-session.detach:{}",
                    session["id"].as_str().unwrap_or_default()
                ),
                EffectRequest::Detach {
                    provider_session_id: serde_json::from_value(session["id"].clone())?,
                    detail: Some(reason.into()),
                    revoke_mcp_credential: archive.then_some(true),
                },
            );
        }
        if archive {
            self.effect("terminal.cleanup", EffectRequest::CleanupTerminals);
        }
        Ok(())
    }

    fn archive_work(&mut self) -> Result<()> {
        let runs: Vec<_> = self
            .records("runs")
            .iter()
            .filter(|run| matches!(run["status"].as_str(), Some("queued" | "preparing")))
            .cloned()
            .collect();
        for run in &runs {
            let mut updated = run.clone();
            updated["status"] = json!("cancelled");
            updated["queuePosition"] = Value::Null;
            updated["completedAt"] = json!(self.context.now);
            self.emit(
                "run.updated",
                updated,
                Some(&json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]})),
            )?;
        }
        for run in runs.iter().filter(|run| run["status"] == "preparing") {
            if let Some(mut item) = self
                .records("turnItems")
                .iter()
                .find(|item| {
                    item["runId"] == run["id"]
                        && item["type"] == "command_execution"
                        && item["input"] == "Preparing workspace"
                        && item["status"] == "running"
                })
                .cloned()
            {
                item["status"] = json!("cancelled");
                item["title"] = json!("Workspace preparation cancelled");
                item["completedAt"] = json!(self.context.now);
                item["updatedAt"] = json!(self.context.now);
                self.emit("turn-item.updated",item.clone(),Some(&json!({"runId":run["id"],"nodeId":item["nodeId"],"providerInstanceId":run["providerInstanceId"]})))?;
            }
        }
        for (records, kind) in [
            ("attempts", "run-attempt.updated"),
            ("nodes", "node.updated"),
        ] {
            let entries: Vec<_> = self
                .records(records)
                .iter()
                .filter(|entry| {
                    runs.iter().any(|run| run["id"] == entry["runId"])
                        && matches!(
                            entry["status"].as_str(),
                            Some("pending" | "running" | "waiting")
                        )
                })
                .cloned()
                .collect();
            for mut entry in entries {
                let run = runs
                    .iter()
                    .find(|run| run["id"] == entry["runId"])
                    .ok_or(Error::Stale)?;
                let scope = json!({"runId":run["id"],"nodeId":if records == "attempts" {&entry["rootNodeId"]} else {&entry["id"]},"providerInstanceId":run["providerInstanceId"]});
                entry["status"] = json!("cancelled");
                entry["completedAt"] = json!(self.context.now);
                self.emit(kind, entry, Some(&scope))?;
            }
        }
        let links: Vec<_> = self
            .records("turnItems")
            .iter()
            .filter(|item| item["type"] == "link_request" && item["linkStatus"] == "pending")
            .cloned()
            .collect();
        for mut item in links {
            item["status"] = json!("cancelled");
            item["linkStatus"] = json!("cancelled");
            item["completedAt"] = json!(self.context.now);
            item["updatedAt"] = json!(self.context.now);
            let mut scope = item.clone();
            scope
                .as_object_mut()
                .ok_or(Error::Stale)?
                .shift_remove("providerInstanceId");
            self.emit("turn-item.updated", item, Some(&scope))?;
        }
        Ok(())
    }

    fn detach(&mut self) -> Result<()> {
        let session = self
            .records("providerSessions")
            .iter()
            .find(|session| session["id"] == self.command["providerSessionId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Provider session {} does not belong to thread {}.",
                        self.command["providerSessionId"]
                            .as_str()
                            .unwrap_or_default(),
                        self.thread_id.0
                    ),
                )
            })?;
        let mut payload =
            json!({"providerSessionId": session["id"], "detachedAt": self.context.now});
        if let Some(reason) = self.command.get("reason") {
            payload["reason"] = reason.clone();
        }
        self.emit("provider-session.detached", payload, Some(&session))?;
        let request = EffectRequest::Detach {
            provider_session_id: serde_json::from_value(session["id"].clone())?,
            detail: self.command["reason"].as_str().map(str::to_owned),
            revoke_mcp_credential: None,
        };
        self.effect(
            &format!(
                "provider-session.detach:{}",
                session["id"].as_str().unwrap_or_default()
            ),
            request,
        );
        Ok(())
    }

    fn run_scope(run: &Value) -> Value {
        let mut scope =
            json!({"runId": run["id"], "providerInstanceId": run["providerInstanceId"]});
        if !run["rootNodeId"].is_null() {
            scope["nodeId"] = run["rootNodeId"].clone();
        }
        scope
    }

    fn queued_run(&self) -> Result<Value> {
        self.records("runs")
            .iter()
            .find(|run| run["id"] == self.command["runId"] && run["status"] == "queued")
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Run {} is not queued.",
                        self.command["runId"].as_str().unwrap_or_default()
                    ),
                )
            })
    }

    fn queue_reorder(&mut self) -> Result<()> {
        let projection = self
            .context
            .projection
            .as_ref()
            .ok_or_else(|| Error::ThreadNotFound(self.thread_id.clone()))?;
        let ordered = queued_runs_in_delivery_order(projection);
        let moving = self.queued_run()?;
        let message = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == moving["userMessageId"]);
        if message.is_some_and(|message| {
            message.get("delegatedCompletion").is_some() || message.get("notification").is_some()
        }) {
            return Err(rejection(
                &self.command,
                "Automatic completion deliveries cannot be reordered.",
            ));
        }
        let ordered: Vec<Value> = ordered
            .into_iter()
            .map(stored_value)
            .collect::<std::result::Result<_, _>>()?;
        let automatic = |run: &Value| {
            self.records("messages").iter().any(|message| {
                message["id"] == run["userMessageId"]
                    && message.get("delegatedCompletion").is_some()
            })
        };
        let mut completions: Vec<_> = ordered
            .iter()
            .filter(|run| automatic(run))
            .cloned()
            .collect();
        if !self.command["beforeRunId"].is_null()
            && completions
                .iter()
                .any(|run| run["id"] == self.command["beforeRunId"])
        {
            return Err(rejection(
                &self.command,
                "Queued messages cannot be reordered ahead of automatic completion delivery.",
            ));
        }
        let mut visible: Vec<_> = ordered
            .iter()
            .filter(|run| !automatic(run) && run["id"] != moving["id"])
            .cloned()
            .collect();
        let before = if self.command["beforeRunId"].is_null() {
            visible.len()
        } else {
            visible
                .iter()
                .position(|run| run["id"] == self.command["beforeRunId"])
                .ok_or_else(|| {
                    rejection(
                        &self.command,
                        format!(
                            "Queue target {} is not queued.",
                            self.command["beforeRunId"].as_str().unwrap_or_default()
                        ),
                    )
                })?
        };
        visible.insert(before, moving);
        completions.extend(visible);
        for (index, mut run) in completions.into_iter().enumerate() {
            let position = json!(index + 1);
            if run["queuePosition"] == position {
                continue;
            }
            run["queuePosition"] = position;
            self.emit("run.updated", run.clone(), Some(&Self::run_scope(&run)))?;
        }
        Ok(())
    }

    fn queue_edit(&mut self) -> Result<()> {
        if t3_contracts::wire::trim_js(self.command["text"].as_str().unwrap_or_default()).is_empty()
        {
            return Err(rejection(
                &self.command,
                "Queued message text cannot be empty.",
            ));
        }
        let run = self.queued_run()?;
        let mut message = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == run["userMessageId"])
            .cloned()
            .ok_or_else(|| {
                rejection(
                    &self.command,
                    format!(
                        "Queued run {} has no user message.",
                        run["id"].as_str().unwrap_or_default()
                    ),
                )
            })?;
        if !message["delegatedCompletion"].is_null() || !message["notification"].is_null() {
            return Err(rejection(
                &self.command,
                "Automatic completion deliveries cannot be edited.",
            ));
        }
        if !message["context"]["guest"].is_null() {
            return Err(rejection(
                &self.command,
                "A guest's message cannot be edited. Cancel it instead.",
            ));
        }
        let mut goal_message = self.command.clone();
        if goal_message.get("attachments").is_none() {
            goal_message["attachments"] = message["attachments"].clone();
        }
        self.native_goal(&goal_message, &run["providerInstanceId"])?;
        let item = self
            .records("turnItems")
            .iter()
            .find(|item| item["type"] == "user_message" && item["messageId"] == message["id"])
            .cloned();
        let update = |value: &mut Value| {
            value["text"] = self.command["text"].clone();
            if let Some(attachments) = self.command.get("attachments") {
                value["attachments"] = attachments.clone();
            }
            if let Some(context) = self
                .command
                .get("context")
                .filter(|context| context.is_object())
            {
                value["context"] = context.clone();
            }
            if let Some(context) = value.get_mut("context").and_then(Value::as_object_mut) {
                context.shift_remove("guest");
            }
            value["updatedAt"] = json!(self.context.now);
        };
        update(&mut message);
        let item = item.map(|mut item| {
            update(&mut item);
            item
        });
        let scope = Self::run_scope(&run);
        self.emit("message.updated", message, Some(&scope))?;
        if let Some(item) = item {
            self.emit("turn-item.updated", item, Some(&scope))?;
        }
        Ok(())
    }

    fn queue_cancel(&mut self) -> Result<()> {
        let run = self.queued_run()?;
        let message = self
            .records("messages")
            .iter()
            .find(|message| message["id"] == run["userMessageId"])
            .cloned();
        if let Some(completion) = message
            .as_ref()
            .and_then(|message| message.get("delegatedCompletion"))
        {
            self.dispose_cohort_with_cancel(
                &completion["parentRunId"],
                "disposed",
                completion["taskIds"].as_array(),
                false,
            )?;
        }
        self.cancel_queued_run(&run)?;
        if let Some(guest) = message
            .as_ref()
            .and_then(|message| message["context"].get("guest"))
        {
            let guest = serde_json::from_value(guest.clone())?;
            let thread = &self
                .context
                .projection
                .as_ref()
                .ok_or_else(|| Error::ThreadNotFound(self.thread_id.clone()))?
                .thread;
            let updated = guest_share_decider::guest_share_after_cancelled_send(
                thread,
                &guest,
                &self.context.now,
            );
            if &updated != thread {
                self.emit("thread.guest-share-updated", stored_value(updated)?, None)?;
            }
        }
        Ok(())
    }

    fn cancel_queued_run(&mut self, run: &Value) -> Result<()> {
        let scope = Self::run_scope(run);
        let mut updated = run.clone();
        updated["status"] = json!("cancelled");
        updated["queuePosition"] = Value::Null;
        updated["completedAt"] = json!(self.context.now);
        self.emit("run.updated", updated, Some(&scope))?;
        if let Some(mut attempt) = self
            .records("attempts")
            .iter()
            .find(|attempt| attempt["id"] == run["activeAttemptId"])
            .cloned()
        {
            attempt["status"] = json!("cancelled");
            attempt["completedAt"] = json!(self.context.now);
            self.emit("run-attempt.updated", attempt, Some(&scope))?;
        }
        if let Some(mut node) = self
            .records("nodes")
            .iter()
            .find(|node| node["id"] == run["rootNodeId"])
            .cloned()
        {
            node["status"] = json!("cancelled");
            node["completedAt"] = json!(self.context.now);
            self.emit("node.updated", node, Some(&scope))?;
        }
        Ok(())
    }
}
