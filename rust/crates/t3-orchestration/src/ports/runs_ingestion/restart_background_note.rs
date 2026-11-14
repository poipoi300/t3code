use super::provider_event_ingestor::epoch_millis;
use crate::{api::*, error::Result};
use serde_json::{Value, json};
use std::collections::HashSet;
type Work = OrchestrationV2RestartCancelledBackgroundWork;
fn compact(value: &str, max: usize) -> Option<String> {
    let text = super::context_handoff_service::collapse_js_whitespace(value);
    if text.is_empty() {
        return None;
    }
    if text.encode_utf16().count() <= max {
        return Some(text);
    }
    let mut used = 0;
    let text = text
        .chars()
        .take_while(|ch| {
            used += ch.len_utf16();
            used < max
        })
        .collect::<String>();
    Some(format!("{text}…"))
}
fn label_with_id(name: Option<&str>, fallback: &str, suffix: &str) -> String {
    let budget = 160usize.saturating_sub(suffix.encode_utf16().count());
    if budget < 2 {
        return compact(&format!("{fallback}{suffix}"), 160).unwrap_or_else(|| fallback.into());
    }
    format!(
        "{}{suffix}",
        name.and_then(|name| compact(name, budget))
            .or_else(|| compact(fallback, budget))
            .unwrap_or_default()
    )
}
pub fn cancelled_turn_item_work(
    item: &OrchestrationV2TurnItem,
    resume_id: Option<&str>,
) -> Result<Option<Work>> {
    let value = t3_contracts::stored::to_value(item)?;
    let text = |key| value[key].as_str();
    let (kind, label) = match text("type") {
        Some("subagent") => {
            let native = if value["origin"] == "provider_native" {
                resume_id.or(value["nativeItemRef"]["nativeId"].as_str())
            } else {
                None
            };
            let title = text("title")
                .filter(|title| compact(title, 160).is_some())
                .or(text("prompt"));
            (
                "subagent",
                label_with_id(
                    title,
                    "subagent",
                    &native.map(|id| format!(" (id {id})")).unwrap_or_default(),
                ),
            )
        }
        Some("command_execution") => (
            if value["monitor"] == true {
                "monitor"
            } else {
                "shell"
            },
            text("input")
                .and_then(|text| compact(text, 160))
                .or_else(|| text("title").and_then(|text| compact(text, 160)))
                .unwrap_or_else(|| "background command".into()),
        ),
        Some("dynamic_tool") => (
            if value["input"]["persistent"] == true {
                "monitor"
            } else {
                "task"
            },
            text("title")
                .and_then(|text| compact(text, 160))
                .or_else(|| text("toolName").and_then(|text| compact(text, 160)))
                .unwrap_or_else(|| "background tool".into()),
        ),
        _ => return Ok(None),
    };
    let mut work = json!({"kind":kind,"label":label,"id":value["id"]});
    if value["type"] == "command_execution" && value["completionWake"] == "always" {
        work["awaited"] = true.into();
    }
    Ok(Some(serde_json::from_value(work)?))
}
pub fn awaiting_provider_thread_id(
    item: &OrchestrationV2TurnItem,
    runs: &[OrchestrationV2Run],
) -> Result<Option<ProviderThreadId>> {
    let item = t3_contracts::stored::to_value(item)?;
    let run = runs
        .iter()
        .find(|run| t3_contracts::stored::to_value(&run.id).ok().as_ref() == Some(&item["runId"]));
    let provider = run
        .and_then(|run| run.provider_thread_id.as_ref())
        .map(t3_contracts::stored::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    let id = if item["type"] == "subagent" && item["origin"] == "provider_native" {
        if provider.is_null() {
            &item["providerThreadId"]
        } else {
            &provider
        }
    } else if item["providerThreadId"].is_null() {
        &provider
    } else {
        &item["providerThreadId"]
    };
    Ok(serde_json::from_value(id.clone())?)
}
pub fn cancelled_roster_task_work(task: &OrchestrationV2PendingBackgroundTask) -> Result<Work> {
    let task = t3_contracts::stored::to_value(task)?;
    if let Some(parent) = task["parentTaskId"].as_str() {
        return Ok(serde_json::from_value(
            json!({"kind":"subagent","label":label_with_id(task["description"].as_str(),"background task",&format!(" (subagent {parent})")),"id":parent}),
        )?);
    }
    let kind = match task["kind"].as_str() {
        Some("command") => "shell",
        Some("subagent") => "subagent",
        Some("monitor") => "monitor",
        _ => "task",
    };
    let id = task["taskId"].as_str().unwrap_or_default();
    let description = task["description"]
        .as_str()
        .and_then(|text| compact(text, 160));
    let label = compact(
        &description
            .map(|text| format!("{text} (id {id})"))
            .unwrap_or_else(|| id.into()),
        160,
    )
    .unwrap_or_else(|| "background task".into());
    let mut work = json!({"kind":kind,"label":label,"id":id});
    if task["awaited"] == true {
        work["awaited"] = true.into();
    }
    Ok(serde_json::from_value(work)?)
}
pub fn restart_cancelled_background_work_note(work: &[Work]) -> String {
    let mut lines=vec!["Note: the T3 server restarted, and this background work was cancelled before it finished. It will not report back on its own, so resume or restart what you still need and wait for its result:".into()];
    for entry in work.iter().take(10) {
        let kind = match entry.kind {
            RestartCancelledBackgroundWorkKind::Shell => "shell",
            RestartCancelledBackgroundWorkKind::Monitor => "monitor",
            RestartCancelledBackgroundWorkKind::Subagent => "subagent",
            RestartCancelledBackgroundWorkKind::Task => "task",
        };
        lines.push(format!("- {kind}: {}", entry.label.as_str()));
    }
    if work.len() > 10 {
        lines.push(format!("- and {} more", work.len() - 10));
    }
    lines.join("\n")
}
pub fn is_awaited_work(work: &Work) -> bool {
    work.awaited.as_ref().and_then(Option::as_ref).is_some()
        || !matches!(
            work.kind,
            RestartCancelledBackgroundWorkKind::Shell | RestartCancelledBackgroundWorkKind::Monitor
        )
}
fn lost(run: &OrchestrationV2Run) -> &[Work] {
    run.restart_cancelled_background_work
        .as_ref()
        .and_then(Option::as_deref)
        .unwrap_or_default()
}
pub fn is_restart_note_source(
    run: &OrchestrationV2Run,
    turns: &[OrchestrationV2ProviderTurn],
) -> bool {
    matches!(
        run.status,
        OrchestrationV2RunStatus::Completed
            | OrchestrationV2RunStatus::Waiting
            | OrchestrationV2RunStatus::Cancelled
    ) && !lost(run).is_empty()
        && !turns.iter().any(|turn| {
            turn.run_attempt_id == run.active_attempt_id
                && turn.status == ProviderTurnStatus::Cancelled
        })
}
pub fn run_outcome(run: &OrchestrationV2Run) -> OrchestrationV2RunStatus {
    if matches!(
        run.status,
        OrchestrationV2RunStatus::Completed | OrchestrationV2RunStatus::Cancelled
    ) && lost(run).iter().any(is_awaited_work)
    {
        OrchestrationV2RunStatus::Interrupted
    } else {
        run.status.clone()
    }
}
pub fn never_started(run: &OrchestrationV2Run) -> bool {
    run.started_at.is_none()
        && run
            .restart_continuation_of_run_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_none()
}
pub fn cancelled_before_start(run: &OrchestrationV2Run) -> bool {
    run.status == OrchestrationV2RunStatus::Cancelled && never_started(run)
}
pub fn delegated_task_runs(
    runs: &[OrchestrationV2Run],
    messages: &[OrchestrationV2ConversationMessage],
    task_id: &NodeId,
) -> Result<Vec<OrchestrationV2Run>> {
    use super::provider_event_ingestor::expanded_id;
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;
    let mut owned = runs
        .iter()
        .filter(|run| {
            run.delegated_task_id.as_ref().and_then(Option::as_ref)
                == Some(&IdOrIndex::Id(task_id.clone()))
        })
        .map(|run| expanded_id(&run.id))
        .collect::<Result<HashSet<_>>>()?;
    let untagged = runs
        .iter()
        .filter(|run| run.delegated_task_id.is_none())
        .map(|run| expanded_id(&run.id))
        .collect::<Result<HashSet<_>>>()?;
    if owned.is_empty() {
        return Ok(Vec::new());
    }
    loop {
        let before = owned.len();
        for message in messages {
            let Some(run) = message.run_id.as_ref().map(expanded_id).transpose()? else {
                continue;
            };
            let Some(completion) = message
                .delegated_completion
                .as_ref()
                .and_then(Option::as_ref)
            else {
                continue;
            };
            if untagged.contains(&run)
                && !owned.contains(&run)
                && owned.contains(&completion.parent_run_id)
            {
                owned.insert(run);
            }
        }
        if before == owned.len() {
            break;
        }
    }
    runs.iter()
        .filter_map(|run| match expanded_id(&run.id) {
            Ok(id) if owned.contains(&id) => Some(Ok(run.clone())),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub fn delegated_task_awaits_work(
    runs: &[OrchestrationV2Run],
    subagents: &[OrchestrationV2Subagent],
    provider_threads: &[OrchestrationV2ProviderThread],
) -> bool {
    subagents.iter().any(|task| {
        runs.iter().any(|run| task.run_id.as_ref() == Some(&run.id))
            && (matches!(
                task.status,
                OrchestrationV2TurnItemStatus::Pending
                    | OrchestrationV2TurnItemStatus::Running
                    | OrchestrationV2TurnItemStatus::Waiting
            ) || task.status != OrchestrationV2TurnItemStatus::Idle
                && task
                    .completion_delivery
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|delivery| {
                        matches!(
                            delivery.state,
                            OrchestrationV2DelegatedCompletionTaskDeliveryState::Pending
                                | OrchestrationV2DelegatedCompletionTaskDeliveryState::Claimed
                        )
                    }))
    }) || provider_threads.iter().any(|thread| {
        thread
            .pending_background_tasks
            .as_ref()
            .and_then(Option::as_deref)
            .unwrap_or_default()
            .iter()
            .any(|task| match task {
                OrchestrationV2PendingBackgroundTask::Subagent(_)
                | OrchestrationV2PendingBackgroundTask::BackgroundTask(_) => true,
                OrchestrationV2PendingBackgroundTask::Command(task)
                | OrchestrationV2PendingBackgroundTask::Monitor(task) => {
                    task.awaited.as_ref().and_then(Option::as_ref).is_some()
                }
            })
    })
}

pub fn delegated_task_result_run_from(runs: &[OrchestrationV2Run]) -> Option<&OrchestrationV2Run> {
    select_task_result(runs.iter())
}

pub fn delegated_task_result_run<'a>(
    runs: &'a [OrchestrationV2Run],
    task_id: &NodeId,
) -> Option<&'a OrchestrationV2Run> {
    select_task_result(runs.iter().filter(|run| {
        run.delegated_task_id.as_ref().and_then(Option::as_ref)
            == Some(&t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                task_id.clone(),
            ))
    }))
}

fn select_task_result<'a>(
    runs: impl Iterator<Item = &'a OrchestrationV2Run>,
) -> Option<&'a OrchestrationV2Run> {
    let mut tasks = runs.collect::<Vec<_>>();
    tasks.sort_by_key(|run| std::cmp::Reverse(run.ordinal));
    let awaited = tasks
        .iter()
        .copied()
        .filter(|run| {
            !cancelled_before_start(run)
                && !(run.status == OrchestrationV2RunStatus::Queued
                    && run.queue_held == Some(Some(true)))
        })
        .collect::<Vec<_>>();
    awaited
        .iter()
        .copied()
        .find(|run| {
            matches!(
                run.status,
                OrchestrationV2RunStatus::Preparing
                    | OrchestrationV2RunStatus::Starting
                    | OrchestrationV2RunStatus::Running
                    | OrchestrationV2RunStatus::Waiting
            )
        })
        .or_else(|| {
            awaited
                .iter()
                .copied()
                .find(|run| run.status == OrchestrationV2RunStatus::Queued)
        })
        .or_else(|| awaited.first().copied())
        .or_else(|| tasks.first().copied())
}

pub fn continuation_failed_before_prompt(
    run: &OrchestrationV2Run,
    turns: &[OrchestrationV2ProviderTurn],
) -> bool {
    run.status == OrchestrationV2RunStatus::Failed
        && run
            .restart_continuation_of_run_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some()
        && !turns
            .iter()
            .any(|turn| turn.run_attempt_id == run.active_attempt_id)
}
pub fn continuation_start_retry_delay_ms(
    run: &OrchestrationV2Run,
    runs: &[OrchestrationV2Run],
    turns: &[OrchestrationV2ProviderTurn],
    items: &[OrchestrationV2TurnItem],
) -> Result<Option<u64>> {
    if !continuation_failed_before_prompt(run, turns) {
        return Ok(None);
    }
    let value = t3_contracts::stored::to_value(run)?;
    let mut failures = vec![];
    for item in items {
        let item = t3_contracts::stored::to_value(item)?;
        if item["type"] == "error"
            && item["status"] == "failed"
            && item["runId"] == value["id"]
            && item["nodeId"] == value["rootNodeId"]
        {
            failures.push(item);
        }
    }
    failures.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["updatedAt"].as_str().and_then(epoch_millis),
                v["ordinal"].as_u64(),
                v["id"].as_str().unwrap_or_default().to_owned(),
            )
        };
        key(a).cmp(&key(b))
    });
    if failures.last().is_some_and(|item| {
        matches!(
            item["failure"]["class"].as_str(),
            Some("usage_limit" | "permission_error" | "validation_error")
        )
    }) {
        return Ok(None);
    }
    let mut count: usize = 0;
    let mut current = Some(run);
    let mut seen = Vec::new();
    while let Some(run) = current.filter(|run| continuation_failed_before_prompt(run, turns)) {
        if seen.contains(&run.id) {
            return Ok(None);
        }
        seen.push(run.id.clone());
        count += 1;
        current = run
            .restart_continuation_of_run_id
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|id| runs.iter().find(|run| &run.id == id));
    }
    Ok([60_000, 180_000].get(count.saturating_sub(1)).copied())
}
pub fn is_restart_continuation_source(
    run: &OrchestrationV2Run,
    turns: &[OrchestrationV2ProviderTurn],
) -> bool {
    if is_restart_note_source(run, turns) {
        lost(run).iter().any(is_awaited_work)
    } else {
        run.status == OrchestrationV2RunStatus::Cancelled
    }
}
pub fn merge_restart_cancelled_background_work(current: &[Work], added: &[Work]) -> Vec<Work> {
    let identity = |work: &Work| {
        work.id
            .as_ref()
            .and_then(Option::as_ref)
            .map(|id| id.as_str().to_owned())
            .unwrap_or_else(|| format!("{:?}\0{}", work.kind, work.label.as_str()))
    };
    let mut seen = current.iter().map(identity).collect::<HashSet<_>>();
    let mut merged = current.to_vec();
    for work in added {
        if seen.insert(identity(work)) {
            merged.push(work.clone());
        }
    }
    merged
}
pub struct ContinuationNote {
    pub work: Vec<Work>,
    pub settled: bool,
}
pub fn restart_continuation_note(
    source: &OrchestrationV2Run,
    runs: &[OrchestrationV2Run],
    turns: &[OrchestrationV2ProviderTurn],
    attempts: &[OrchestrationV2RunAttempt],
) -> ContinuationNote {
    let complete = turns
        .iter()
        .filter(|turn| turn.status == ProviderTurnStatus::Completed)
        .filter_map(|turn| turn.run_attempt_id.clone())
        .collect::<Vec<_>>();
    let completed_runs = attempts
        .iter()
        .filter(|attempt| complete.contains(&attempt.id))
        .map(|attempt| attempt.run_id.clone())
        .collect::<Vec<_>>();
    let mut current = source;
    let mut work = lost(current).to_vec();
    let mut visited = vec![current.id.clone()];
    while !completed_runs.contains(&current.id)
        && !current
            .active_attempt_id
            .as_ref()
            .is_some_and(|id| complete.contains(id))
    {
        let previous = current
            .restart_continuation_of_run_id
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|id| runs.iter().find(|run| &run.id == id));
        let Some(previous) = previous.filter(|previous| !visited.contains(&previous.id)) else {
            break;
        };
        visited.push(previous.id.clone());
        current = previous;
        work = merge_restart_cancelled_background_work(lost(current), &work);
    }
    ContinuationNote {
        work,
        settled: is_restart_note_source(current, turns),
    }
}
pub fn is_restart_note_continuation(
    run: &OrchestrationV2Run,
    runs: &[OrchestrationV2Run],
    turns: &[OrchestrationV2ProviderTurn],
    attempts: &[OrchestrationV2RunAttempt],
) -> bool {
    run.restart_continuation_of_run_id
        .as_ref()
        .and_then(Option::as_ref)
        .and_then(|id| runs.iter().find(|run| &run.id == id))
        .is_some_and(|run| {
            !restart_continuation_note(run, runs, turns, attempts)
                .work
                .is_empty()
        })
}
pub fn run_ran_after(run: &OrchestrationV2Run, other: &OrchestrationV2Run) -> bool {
    let end = |run: &OrchestrationV2Run| {
        run.completed_at
            .as_ref()
            .and_then(|date| epoch_millis(&date.0))
            .unwrap_or(i64::MAX)
    };
    if end(run) == end(other) {
        run.ordinal > other.ordinal
    } else {
        end(run) > end(other)
    }
}
pub fn pending_restart_cancelled_background_work(
    run: &OrchestrationV2Run,
    runs: &[OrchestrationV2Run],
    turns: &[OrchestrationV2ProviderTurn],
    attempts: &[OrchestrationV2RunAttempt],
    compactions: &HashSet<MessageId>,
) -> Vec<Work> {
    if run.provider_thread_id.is_none()
        || run
            .restart_continuation_of_run_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some()
        || compactions.contains(
            &super::provider_event_ingestor::expanded_id(&run.user_message_id)
                .unwrap_or(MessageId(String::new())),
        )
    {
        return vec![];
    }
    let delivered = turns
        .iter()
        .filter(|turn| {
            Some(&turn.provider_thread_id) == run.provider_thread_id.as_ref()
                && turn.status == ProviderTurnStatus::Completed
        })
        .filter_map(|turn| turn.run_attempt_id.clone())
        .collect::<Vec<_>>();
    if attempts.iter().any(|attempt| {
        attempt.run_id == run.id
            && Some(&attempt.id) != run.active_attempt_id.as_ref()
            && delivered.contains(&attempt.id)
    }) {
        return vec![];
    }
    let same = runs
        .iter()
        .filter(|candidate| candidate.provider_thread_id == run.provider_thread_id)
        .collect::<Vec<_>>();
    let prompted = same
        .iter()
        .filter(|candidate| {
            candidate.id != run.id
                && candidate.active_attempt_id.is_some()
                && candidate.status != OrchestrationV2RunStatus::RolledBack
                && (candidate
                    .active_attempt_id
                    .as_ref()
                    .is_some_and(|id| delivered.contains(id))
                    || attempts.iter().any(|attempt| {
                        attempt.run_id == candidate.id && delivered.contains(&attempt.id)
                    }))
                && !compactions.iter().any(|id| {
                    candidate.user_message_id
                        == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id.clone())
                })
                && (candidate
                    .restart_continuation_of_run_id
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none()
                    || is_restart_note_continuation(candidate, runs, turns, attempts))
        })
        .collect::<Vec<_>>();
    same.iter()
        .filter(|source| {
            run_ran_after(run, source)
                && !lost(source).is_empty()
                && !prompted.iter().any(|later| run_ran_after(later, source))
        })
        .fold(vec![], |work, source| {
            merge_restart_cancelled_background_work(&work, lost(source))
        })
}
