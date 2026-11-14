use super::{
    checkpoint_restore_safety::expanded_id,
    checkpoint_service::{Capture, CheckpointService, encode_id_part},
};
use crate::{
    api::*,
    error::{Error, Result},
    stores::receipts::{AggregateId, CommandReceipt, ReceiptStatus},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct CheckpointCaptureService {
    pub checkpoints: Arc<CheckpointService>,
    pub projections: Arc<dyn ProjectionReader>,
    pub events: Arc<dyn EventCommitter>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
}

pub fn domain_event(
    ids: &dyn IdGenerator,
    thread_id: &ThreadId,
    now: &IsoDateTime,
    kind: &str,
    payload: impl Serialize,
    metadata: Value,
) -> Result<OrchestrationV2DomainEvent> {
    let mut value = json!({ "id": ids.next(IdKind::Event), "type": kind, "threadId": thread_id, "occurredAt": now, "payload": payload });
    if let Some(metadata) = metadata.as_object() {
        for (key, entry) in metadata {
            value[key] = entry.clone();
        }
    }
    Ok(serde_json::from_value(value)?)
}

impl CheckpointCaptureService {
    pub async fn execute(
        &self,
        thread_id: ThreadId,
        run_id: RunId,
        scope_id: CheckpointScopeId,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let context = self
            .projections
            .checkpoint_capture(thread_id.clone(), run_id.clone(), scope_id.clone())
            .await?;
        let stopped = context.run.as_ref().is_some_and(|run| {
            matches!(
                run.status,
                OrchestrationV2RunStatus::Interrupted | OrchestrationV2RunStatus::Cancelled
            )
        });
        if context.run.as_ref().is_some_and(|run| {
            (run.checkpoint_id.is_some()
                && (run.status == OrchestrationV2RunStatus::Completed || stopped))
                || run.status == OrchestrationV2RunStatus::RolledBack
        }) {
            return Ok(());
        }
        let (Some(mut run), Some(root), Some(scope), Some(provider)) = (
            context.run,
            context.root_node,
            context.scope,
            context.provider_thread,
        ) else {
            return Err(invalid_target());
        };
        if (run.status != OrchestrationV2RunStatus::Waiting && !stopped)
            || root.checkpoint_scope_id.as_ref() != Some(&scope.id)
        {
            return Err(invalid_target());
        }
        if expanded_id(&run.id)? != run_id || expanded_id(&scope.id)? != scope_id {
            return Err(invalid_target());
        }
        let now = self.clock.now();
        let baseline_ordinal = run.ordinal.saturating_sub(1);
        let make_input = |ordinal| -> Result<Capture> {
            Ok(Capture {
                scope: scope.clone(),
                run_id: None,
                node_id: expanded_id(&root.id)?,
                ordinal,
                app_run_ordinal: None,
                captured_at: run.requested_at.clone(),
                force_missing: context.side_chat,
            })
        };
        let mut baselines = Vec::new();
        if baseline_ordinal != 0 && !context.preserved_ordinals.contains(&0) {
            baselines.push(make_input(0)?);
        }
        if !context.preserved_ordinals.contains(&baseline_ordinal) {
            baselines.push(make_input(baseline_ordinal)?);
        }
        let (captured, checkpoint) = self
            .checkpoints
            .finalize_run(
                baselines,
                Capture {
                    scope: scope.clone(),
                    run_id: Some(run_id.clone()),
                    node_id: expanded_id(&root.id)?,
                    ordinal: run.ordinal,
                    app_run_ordinal: Some(run.ordinal),
                    captured_at: now.clone(),
                    force_missing: context.side_chat,
                },
                cancellation,
            )
            .await?;
        let provider_metadata = json!({ "driver": provider.driver, "providerInstanceId": run.provider_instance_id, "nodeId": root.id });
        let run_metadata = json!({ "runId": run.id, "nodeId": root.id, "driver": provider.driver, "providerInstanceId": run.provider_instance_id });
        let mut events = Vec::new();
        for baseline in captured {
            events.push(domain_event(
                self.ids.as_ref(),
                &thread_id,
                &now,
                "checkpoint.captured",
                baseline,
                provider_metadata.clone(),
            )?);
        }
        events.push(domain_event(
            self.ids.as_ref(),
            &thread_id,
            &now,
            "checkpoint.captured",
            &checkpoint,
            run_metadata.clone(),
        )?);
        let driver: String = serde_json::from_value(serde_json::to_value(&provider.driver)?)?;
        let item = json!({
            "id": format!("turn-item:provider:{}:native-item:{}", encode_id_part(&driver), encode_id_part(&format!("checkpoint:{}", expanded_id(&checkpoint.id)?.0))),
            "threadId": run.thread_id, "runId": run.id, "nodeId": root.id, "providerThreadId": provider.id,
            "providerTurnId": root.provider_turn_id, "nativeItemRef": null, "parentItemId": null,
            "ordinal": run.ordinal.checked_mul(100).and_then(|n| n.checked_add(99)).ok_or_else(invalid_target)?,
            "status": "completed", "title": null, "startedAt": now, "completedAt": now, "updatedAt": now,
            "type": "checkpoint", "checkpointId": checkpoint.id, "scopeId": checkpoint.scope_id, "files": checkpoint.files,
        });
        events.push(domain_event(
            self.ids.as_ref(),
            &thread_id,
            &now,
            "turn-item.updated",
            item,
            run_metadata,
        )?);
        run.delegated_completion = None;
        run.checkpoint_id = Some(checkpoint.id);
        if !stopped {
            run.status = OrchestrationV2RunStatus::Completed;
            run.completed_at = Some(now.clone());
        }
        let metadata = json!({ "runId": run.id, "nodeId": root.id, "providerInstanceId": run.provider_instance_id });
        events.push(domain_event(
            self.ids.as_ref(),
            &thread_id,
            &now,
            "run.updated",
            &run,
            metadata.clone(),
        )?);
        if !stopped {
            let mut root = root;
            root.status = ExecutionNodeStatus::Completed;
            root.completed_at = Some(now.clone());
            root.checkpoint_scope_id = Some(scope.id);
            events.push(domain_event(
                self.ids.as_ref(),
                &thread_id,
                &now,
                "node.updated",
                root,
                metadata,
            )?);
        }
        self.events
            .commit(CommitRequest {
                receipt: Some(CommandReceipt {
                    command_id: CommandId(format!(
                        "command:effect:checkpoint.capture:{}",
                        run_id.0
                    )),
                    aggregate: AggregateId::Thread(thread_id),
                    command_type: "checkpoint.capture".into(),
                    accepted_at: now.0,
                    result_sequence: 0,
                    status: ReceiptStatus::Accepted,
                    error: None,
                }),
                decision: Decision {
                    events,
                    ..Default::default()
                },
                guard: CommitGuard::None,
            })
            .await?;
        Ok(())
    }
}

fn invalid_target() -> Error {
    Error::Service {
        operation: "checkpoint.capture",
        detail: "The persisted checkpoint capture target is incomplete or no longer waiting."
            .into(),
    }
}
