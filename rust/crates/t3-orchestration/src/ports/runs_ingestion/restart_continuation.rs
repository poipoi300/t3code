use super::{
    provider_event_ingestor::{epoch_millis, expanded_id},
    restart_background_note::*,
};
use crate::{
    api::*, error::Result,
    ports::thread_project::thread_management_service::ThreadManagementService,
};
use serde_json::json;
use std::sync::Arc;
pub const CONTINUE_PROMPT: &str = "Continue where you left off. Your last turn was cut short, so a command or other action it started may have finished or may still be running. Before you repeat one that changes state, confirm it did not take effect and is not still running; if you cannot tell, say so instead of repeating it.";
fn blocked(
    projection: &OrchestrationV2ThreadProjection,
    source: &OrchestrationV2Run,
    now: &IsoDateTime,
    settled: bool,
) -> Result<bool> {
    let thread = t3_contracts::stored::to_value(&projection.thread)?;
    if !thread["archivedAt"].is_null()
        || !thread["deletedAt"].is_null()
        || thread["settledOverride"] == "settled"
        || thread["snoozedUntil"]
            .as_str()
            .and_then(epoch_millis)
            .zip(epoch_millis(&now.0))
            .is_some_and(|(until, now)| until > now)
    {
        return Ok(true);
    }
    let turn = projection
        .provider_turns
        .iter()
        .filter(|turn| turn.run_attempt_id == source.active_attempt_id)
        .max_by_key(|turn| turn.ordinal);
    if !settled
        && turn.is_some_and(|turn| {
            matches!(
                turn.status,
                ProviderTurnStatus::Completed
                    | ProviderTurnStatus::Failed
                    | ProviderTurnStatus::Interrupted
            )
        })
    {
        return Ok(true);
    }
    for request in &projection.runtime_requests {
        let request = t3_contracts::stored::to_value(request)?;
        if !matches!(
            request["kind"].as_str(),
            Some("dynamic_tool_call" | "auth_refresh")
        ) && request["responseCapability"]["type"] != "message"
            && (request["status"] == "pending"
                || (matches!(request["status"].as_str(), Some("expired" | "cancelled"))
                    && request["responseCapability"]["type"] == "not_resumable"
                    && turn.is_some_and(|turn| {
                        t3_contracts::stored::to_value(&turn.id).ok().as_ref()
                            == Some(&request["providerTurnId"])
                    })))
        {
            return Ok(true);
        }
    }
    let source = t3_contracts::stored::to_value(source)?;
    for item in &projection.turn_items {
        let item = t3_contracts::stored::to_value(item)?;
        if item["type"] == "secret_request"
            && item["runId"] == source["id"]
            && (item["secretStatus"] == "pending"
                || (item["secretStatus"] == "cancelled"
                    && !item["completedAt"].is_null()
                    && !source["completedAt"].is_null()
                    && item["completedAt"].as_str().and_then(epoch_millis)
                        == source["completedAt"].as_str().and_then(epoch_millis)))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn open_awaited_work(
    projection: &OrchestrationV2ThreadProjection,
    provider: &OrchestrationV2ProviderThread,
) -> Result<bool> {
    for item in &projection.turn_items {
        let value = t3_contracts::stored::to_value(item)?;
        if !matches!(
            value["status"].as_str(),
            Some("pending" | "running" | "waiting")
        ) || (value["type"] == "subagent"
            && value["origin"] == "app_owned"
            && !value["childThreadId"].is_null())
        {
            continue;
        }
        if cancelled_turn_item_work(item, None)?
            .as_ref()
            .is_some_and(is_awaited_work)
            && awaiting_provider_thread_id(item, &projection.runs)?.as_ref()
                == Some(&expanded_id(&provider.id)?)
        {
            return Ok(true);
        }
    }
    if provider.owner_node_id.is_none() {
        for task in provider
            .pending_background_tasks
            .as_ref()
            .and_then(Option::as_ref)
            .into_iter()
            .flatten()
        {
            if is_awaited_work(&cancelled_roster_task_work(task)?) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
pub fn restart_continuation_run<'a>(
    projection: &'a OrchestrationV2ThreadProjection,
    now: &IsoDateTime,
) -> Result<Option<&'a OrchestrationV2Run>> {
    let mut latest = None;
    for run in &projection.runs {
        if run.status != OrchestrationV2RunStatus::Queued
            && !never_started(run)
            && latest.is_none_or(|latest| run_ran_after(run, latest))
        {
            latest = Some(run);
        }
    }
    let Some(run) = latest else {
        return Ok(None);
    };
    let prepared = run.status == OrchestrationV2RunStatus::Starting
        && run
            .restart_continuation_of_run_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some();
    let settled = matches!(
        run.status,
        OrchestrationV2RunStatus::Completed | OrchestrationV2RunStatus::Waiting
    );
    if (run.status != OrchestrationV2RunStatus::Running && !prepared && !settled)
        || blocked(projection, run, now, settled)?
        || projection.thread.provider_instance_id != run.provider_instance_id
    {
        return Ok(None);
    }
    let live = !prepared && !settled;
    let Some(provider) = projection
        .provider_threads
        .iter()
        .find(|provider| Some(&provider.id) == run.provider_thread_id.as_ref())
    else {
        return Ok(None);
    };
    let native = provider.native_thread_ref.as_ref();
    if provider.app_thread_id.as_ref() != Some(&projection.thread.id)
        || provider.owner_node_id.is_some()
        || provider.provider_instance_id != run.provider_instance_id
        || native.is_none_or(|native| {
            native.native_id.is_none()
                || native.strength != OrchestrationV2NativeRefStrength::Strong
                || native.driver != provider.driver
        })
        || (live && provider.status != ProviderThreadStatus::Active)
        || matches!(
            provider.status,
            ProviderThreadStatus::Closed | ProviderThreadStatus::Archived
        )
    {
        return Ok(None);
    }
    let Some(session) = projection
        .provider_sessions
        .iter()
        .find(|session| Some(&session.id) == provider.provider_session_id.as_ref())
    else {
        return Ok(None);
    };
    if session.provider_instance_id != run.provider_instance_id
        || session.driver != provider.driver
        || (!prepared
            && matches!(
                session.status,
                ProviderSessionStatus::Stopped | ProviderSessionStatus::Error
            ))
    {
        return Ok(None);
    }
    if settled {
        return Ok(open_awaited_work(projection, provider)?.then_some(run));
    }
    if live
        && !projection.provider_turns.iter().any(|turn| {
            turn.provider_thread_id == provider.id
                && turn.run_attempt_id == run.active_attempt_id
                && turn.status == ProviderTurnStatus::Running
        })
    {
        return Ok(None);
    }
    Ok(Some(run))
}
// seam-shim: classify the two typed pre-dispatch declines without swallowing ambiguous failures.
pub trait RestartDispatchPolicy: Send + Sync {
    fn declined(&self, error: &crate::error::Error) -> bool;
}
pub struct RestartContinuationService {
    pub threads: ThreadManagementService,
    pub settings: Arc<dyn SettingsService>,
    pub operations: Arc<dyn ThreadOperations>,
    pub clock: Arc<dyn Clock>,
    pub dispatch_policy: Option<Arc<dyn RestartDispatchPolicy>>,
}
impl RestartContinuationService {
    pub async fn continue_restarted_run(
        &self,
        thread_id: ThreadId,
        source_run_id: RunId,
    ) -> Result<()> {
        self.prepare_and_dispatch(thread_id.clone(), source_run_id.clone())
            .await?;
        self.operations
            .recover_delegated_task(thread_id, Some(source_run_id), None)
            .await
    }
    async fn prepare_and_dispatch(&self, thread_id: ThreadId, source_run_id: RunId) -> Result<()> {
        let settings = match self.settings.get().await {
            Ok(settings) => t3_contracts::stored::to_value(&settings)?,
            Err(_) => return Ok(()),
        };
        let message_id = MessageId(format!("message:restart-continuation:{}", source_run_id.0));
        let projection = self
            .threads
            .query_records(ProjectionRecordQuery {
                thread_id: thread_id.clone(),
                fields: vec![
                    RecordKind::Messages,
                    RecordKind::Runs,
                    RecordKind::ProviderTurns,
                    RecordKind::Attempts,
                    RecordKind::RuntimeRequests,
                    RecordKind::TurnItems,
                ],
                filter: RecordFilter {
                    turn_item_types: Some(vec!["secret_request".into()]),
                    ..Default::default()
                },
                messages: MessageFilter {
                    ids: Some(vec![message_id.clone()]),
                    ..Default::default()
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await?;
        let project = expanded_id(&projection.thread.project_id)?;
        if !settings["projectSettingsOverrides"][&project.0]["continueThreadsAfterServerUpdate"]
            .as_bool()
            .unwrap_or(
                settings["continueThreadsAfterServerUpdate"]
                    .as_bool()
                    .unwrap_or(false),
            )
            || projection.messages.iter().any(|message| {
                message.id
                    == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(message_id.clone())
            })
        {
            return Ok(());
        }
        let Some(source) = projection.runs.iter().find(|run| {
            run.id == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(source_run_id.clone())
        }) else {
            return Ok(());
        };
        if !(is_restart_continuation_source(source, &projection.provider_turns)
            || continuation_failed_before_prompt(source, &projection.provider_turns))
            || blocked(
                &projection,
                source,
                &self.clock.now(),
                is_restart_note_source(source, &projection.provider_turns),
            )?
            || projection.runs.iter().any(|run| {
                run.id != source.id
                    && run.status != OrchestrationV2RunStatus::Queued
                    && !cancelled_before_start(run)
                    && run_ran_after(run, source)
            })
            || projection.thread.provider_instance_id != source.provider_instance_id
        {
            return Ok(());
        }
        let window = epoch_millis(
            &source
                .completed_at
                .as_ref()
                .unwrap_or(&source.requested_at)
                .0,
        )
        .unwrap_or(0)
            - 300_000;
        let mut count = 0;
        let mut current = source;
        while current
            .restart_continuation_of_run_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some()
            && count < 3
        {
            if epoch_millis(&current.requested_at.0).unwrap_or(0) < window {
                break;
            }
            count += 1;
            let Some(previous) = current
                .restart_continuation_of_run_id
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|id| projection.runs.iter().find(|run| &run.id == id))
            else {
                break;
            };
            current = previous;
        }
        if count >= 3 {
            return Ok(());
        }
        let source_records = self
            .threads
            .query_records(ProjectionRecordQuery {
                thread_id: thread_id.clone(),
                fields: vec![RecordKind::Messages, RecordKind::TurnItems],
                filter: RecordFilter {
                    run_ids: Some(vec![source_run_id.clone()]),
                    turn_item_types: Some(vec!["run_interrupt_request".into()]),
                    ..Default::default()
                },
                messages: MessageFilter {
                    ids: Some(vec![expanded_id(&source.user_message_id)?]),
                    ..Default::default()
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await?;
        if !source_records.turn_items.is_empty() {
            return Ok(());
        }
        let thread = t3_contracts::stored::to_value(&projection.thread)?;
        let source_value = t3_contracts::stored::to_value(source)?;
        let task = source_value.get("delegatedTaskId").or_else(|| {
            (thread["forkedFrom"]["type"] == "node").then_some(&thread["forkedFrom"]["nodeId"])
        });
        if thread["lineage"]["relationshipToParent"] == "subagent"
            && !thread["lineage"]["parentThreadId"].is_null()
            && let Some(task) = task
        {
            let parent: ThreadId =
                serde_json::from_value(thread["lineage"]["parentThreadId"].clone())?;
            let records = self
                .threads
                .records(
                    parent.clone(),
                    vec![RecordKind::Subagents],
                    RecordFilter {
                        subagent_ids: Some(vec![serde_json::from_value(task.clone())?]),
                        ..Default::default()
                    },
                )
                .await?;
            let task = records
                .subagents
                .iter()
                .map(t3_contracts::stored::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .find(|candidate| {
                    candidate["id"] == *task
                        && candidate["result"].is_null()
                        && matches!(
                            candidate["status"].as_str(),
                            Some("pending" | "running" | "waiting")
                        )
                });
            if let Some(task) = task.filter(|task| !task["runId"].is_null())
                && !self
                    .threads
                    .records(
                        parent,
                        vec![RecordKind::TurnItems],
                        RecordFilter {
                            run_ids: Some(vec![serde_json::from_value(task["runId"].clone())?]),
                            turn_item_types: Some(vec!["run_interrupt_request".into()]),
                            ..Default::default()
                        },
                    )
                    .await?
                    .turn_items
                    .is_empty()
            {
                return Ok(());
            }
        }
        if source_records.messages.iter().any(|message| {
            message.id == source.user_message_id
                && message.attachments.is_empty()
                && matches!(
                    t3_contracts::wire::trim_js(&message.text)
                        .to_ascii_lowercase()
                        .as_str(),
                    "/compact" | "/logout"
                )
        }) {
            return Ok(());
        }
        let note = restart_continuation_note(
            source,
            &projection.runs,
            &projection.provider_turns,
            &projection.attempts,
        );
        let text = if note.work.is_empty() {
            CONTINUE_PROMPT.into()
        } else {
            let text = restart_cancelled_background_work_note(&note.work);
            if note.settled {
                text
            } else {
                format!("{text}\n\n{CONTINUE_PROMPT}")
            }
        };
        let command = serde_json::from_value(
            json!({"type":"message.dispatch","commandId":format!("command:restart-continuation:{}",source_run_id.0),"threadId":thread_id,"messageId":message_id,"text":text,"notification":{"source":{"kind":"system"},"outcome":"updated","summary":if note.settled{"T3 Code restarted and stopped background work"}else{"T3 Code restarted and resumed this turn"},"detail":text},"attachments":[],"modelSelection":source.model_selection,"dispatchMode":{"type":"start_immediately"},"createdBy":"agent","creationSource":"server","restartContinuationOfRunId":source_run_id}),
        )?;
        match self.threads.dispatch(command).await {
            Ok(_) => Ok(()),
            Err(error)
                if self
                    .dispatch_policy
                    .as_ref()
                    .is_some_and(|policy| policy.declined(&error)) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}
