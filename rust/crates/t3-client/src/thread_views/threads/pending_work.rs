use super::execution::{id_text, interruptible_status, work_active};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    thread_pull_request::ThreadPullRequestLinkSource,
};

pub fn background_work_holds_completion(tasks: &[OrchestrationV2PendingBackgroundTask]) -> bool {
    tasks.iter().any(|task| match task {
        OrchestrationV2PendingBackgroundTask::Subagent(_)
        | OrchestrationV2PendingBackgroundTask::BackgroundTask(_) => true,
        OrchestrationV2PendingBackgroundTask::Command(t)
        | OrchestrationV2PendingBackgroundTask::Monitor(t) => {
            t.awaited.as_ref().is_some_and(Option::is_some)
        }
    })
}

pub fn task_id(task: &OrchestrationV2PendingBackgroundTask) -> &str {
    match task {
        OrchestrationV2PendingBackgroundTask::Subagent(t) => &t.task_id.0,
        OrchestrationV2PendingBackgroundTask::Command(t)
        | OrchestrationV2PendingBackgroundTask::Monitor(t)
        | OrchestrationV2PendingBackgroundTask::BackgroundTask(t) => &t.task_id.0,
    }
}
pub fn pending_background_turn_items(
    p: &OrchestrationV2ThreadProjection,
) -> Vec<&OrchestrationV2TurnItem> {
    p.turn_items.iter().filter(|item| {
        matches!(item, OrchestrationV2TurnItem::CommandExecution(_) | OrchestrationV2TurnItem::DynamicTool(_) | OrchestrationV2TurnItem::Subagent(_)) && work_active(item_field!(item,status)) &&
        !matches!(item, OrchestrationV2TurnItem::DynamicTool(t) if t.input.get("persistent") == Some(&serde_json::Value::Bool(true))) &&
        !p.runs.iter().any(|run| run.status == OrchestrationV2RunStatus::RolledBack && item_field!(item,run_id).as_ref() == Some(&run.id))
    }).collect()
}

fn command(task_id: String, description: Option<String>) -> PendingBackgroundTaskCommand {
    PendingBackgroundTaskCommand {
        command: None,
        item_id: None,
        source_thread_id: None,
        task_id: TrimmedNonEmptyString(task_id),
        description: description.map(|d| Some(TrimmedNonEmptyString(d))),
        parent_task_id: None,
        native_item_id: None,
        awaited: None,
    }
}

fn watches(
    p: &OrchestrationV2ThreadProjection,
    tasks: &mut Vec<OrchestrationV2PendingBackgroundTask>,
) {
    for link in p
        .thread
        .pull_requests
        .as_ref()
        .and_then(Option::as_deref)
        .unwrap_or(&[])
    {
        if link.watch.is_none()
            || link.source == ThreadPullRequestLinkSource::StackDismissed
            || link
                .snapshot
                .as_ref()
                .is_some_and(|s| s.state == t3_contracts::pull_request::PullRequestState::Merged)
        {
            continue;
        }
        let id = format!(
            "pull-request-watch:{}/{}#{}",
            link.host.0.to_lowercase(),
            link.repository.0.to_lowercase(),
            link.number
        );
        let mut task = command(
            id.clone(),
            Some(format!("Watching pull request #{}", link.number)),
        );
        if link
            .watch
            .as_ref()
            .and_then(Option::as_ref)
            .is_none_or(|w| w.checks_pending != Some(Some(false)))
        {
            task.awaited = Some(Some(LiteralTrue));
        }
        let task = OrchestrationV2PendingBackgroundTask::Monitor(Box::new(task));
        if let Some(existing) = tasks.iter_mut().find(|t| task_id(t) == id) {
            *existing = task;
        } else {
            tasks.push(task);
        }
    }
}

pub fn derive_pending_background_work(
    p: &OrchestrationV2ThreadProjection,
    latest: Option<&OrchestrationV2Run>,
) -> Vec<OrchestrationV2PendingBackgroundTask> {
    if p.runs.iter().any(|r| interruptible_status(&r.status)) {
        return vec![];
    }
    let mut tasks = vec![];
    let Some(latest) = latest else {
        watches(p, &mut tasks);
        return tasks;
    };
    if !matches!(
        latest.status,
        OrchestrationV2RunStatus::Cancelled
            | OrchestrationV2RunStatus::Completed
            | OrchestrationV2RunStatus::Failed
            | OrchestrationV2RunStatus::Interrupted
            | OrchestrationV2RunStatus::Waiting
    ) {
        return tasks;
    }
    for provider in p.provider_threads.iter().filter(|t| {
        p.thread
            .active_provider_thread_id
            .as_ref()
            .is_none_or(|id| id == &t.id)
    }) {
        for task in provider
            .pending_background_tasks
            .as_ref()
            .and_then(Option::as_deref)
            .unwrap_or(&[])
        {
            if task_id(task).is_empty() || tasks.iter().any(|t| task_id(t) == task_id(task)) {
                continue;
            }
            let mut task = task.clone();
            let description = match &mut task {
                OrchestrationV2PendingBackgroundTask::Subagent(t) => {
                    t.provider_instance_id = match &provider.provider_instance_id {
                        IdOrIndex::Id(id) => Some(Some(id.clone())),
                        IdOrIndex::Index(_) => None,
                    };
                    &mut t.description
                }
                OrchestrationV2PendingBackgroundTask::Command(t)
                | OrchestrationV2PendingBackgroundTask::Monitor(t)
                | OrchestrationV2PendingBackgroundTask::BackgroundTask(t) => &mut t.description,
            };
            *description = description
                .as_ref()
                .and_then(Option::as_ref)
                .map(|d| d.0.trim())
                .filter(|d| !d.is_empty())
                .map(|d| Some(TrimmedNonEmptyString(d.into())));
            tasks.push(task);
        }
    }
    for item in pending_background_turn_items(p) {
        let native = item_field!(item, native_item_ref)
            .as_ref()
            .and_then(|r| r.native_id.as_ref())
            .map(|s| s.0.as_str())
            .filter(|s| !s.is_empty());
        let id = native
            .map(str::to_owned)
            .unwrap_or_else(|| id_text(item_field!(item, id)));
        if tasks.iter().any(|t| task_id(t) == id) {
            continue;
        }
        let description = item_field!(item, title)
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| match item {
                OrchestrationV2TurnItem::CommandExecution(t) => Some(t.input.trim()),
                OrchestrationV2TurnItem::DynamicTool(t) => t.tool_name.as_ref().map(|n| n.0.trim()),
                OrchestrationV2TurnItem::Subagent(t) => Some(t.prompt.trim()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let base = command(id, description);
        tasks.push(match item {
            OrchestrationV2TurnItem::Subagent(t) => OrchestrationV2PendingBackgroundTask::Subagent(
                Box::new(PendingBackgroundTaskSubagent {
                    command: base.command,
                    item_id: base.item_id,
                    source_thread_id: base.source_thread_id,
                    task_id: base.task_id,
                    description: base.description,
                    parent_task_id: base.parent_task_id,
                    native_item_id: base.native_item_id,
                    awaited: base.awaited,
                    provider_instance_id: None,
                    child_thread_id: match &t.child_thread_id {
                        Some(IdOrIndex::Id(id)) => Some(Some(id.clone())),
                        _ => None,
                    },
                }),
            ),
            OrchestrationV2TurnItem::CommandExecution(t) if t.monitor == Some(Some(true)) => {
                OrchestrationV2PendingBackgroundTask::Monitor(Box::new(base))
            }
            OrchestrationV2TurnItem::CommandExecution(_) => {
                OrchestrationV2PendingBackgroundTask::Command(Box::new(base))
            }
            _ => OrchestrationV2PendingBackgroundTask::BackgroundTask(Box::new(base)),
        });
    }
    watches(p, &mut tasks);
    tasks
}
