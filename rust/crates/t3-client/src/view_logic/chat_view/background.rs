use super::rewind::id_key;
use crate::view_logic::JsTrim;
use crate::{ClientError, state::items};
use serde_json::json;
use std::collections::BTreeSet;
use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex, wire::WireDateTime};

fn native_ref(item: &OrchestrationV2TurnItem) -> Option<&OrchestrationV2ProviderRef> {
    match item {
        OrchestrationV2TurnItem::Notification(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::UserMessage(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::AssistantMessage(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::Reasoning(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::ProposedPlan(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::TodoList(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::UserInputRequest(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::FileChange(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::CommandExecution(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::FileSearch(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::WebSearch(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::ApprovalRequest(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::Checkpoint(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::RunInterruptRequest(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::RunInterruptResult(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::SystemNotice(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::Error(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::Compaction(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::Handoff(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::Fork(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::ThreadCreated(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::SecretRequest(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::LinkRequest(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::Subagent(v) => v.native_item_ref.as_ref(),
        OrchestrationV2TurnItem::DynamicTool(v) => v.native_item_ref.as_ref(),
    }
}

fn task_id(task: &OrchestrationV2PendingBackgroundTask) -> &str {
    match task {
        OrchestrationV2PendingBackgroundTask::Subagent(v) => &v.task_id.0,
        OrchestrationV2PendingBackgroundTask::Command(v)
        | OrchestrationV2PendingBackgroundTask::Monitor(v)
        | OrchestrationV2PendingBackgroundTask::BackgroundTask(v) => &v.task_id.0,
    }
}

fn latest_usage_limited<'a>(
    p: &'a OrchestrationV2ThreadProjection,
    epoch: &impl Fn(&WireDateTime) -> i64,
) -> Option<&'a OrchestrationV2Run> {
    let mut latest: Option<&OrchestrationV2Run> = None;
    let end = |run: &OrchestrationV2Run| run.completed_at.as_ref().map_or(i64::MAX, epoch);
    for run in &p.runs {
        if run.status == OrchestrationV2RunStatus::Queued
            || (run.status == OrchestrationV2RunStatus::Cancelled && run.started_at.is_none())
        {
            continue;
        }
        if latest.is_none_or(|old| (end(run), run.ordinal) > (end(old), old.ordinal)) {
            latest = Some(run);
        }
    }
    let run = latest.filter(|run| run.status == OrchestrationV2RunStatus::Failed)?;
    let mut latest_error: Option<&TurnItemError> = None;
    for item in &p.turn_items {
        let OrchestrationV2TurnItem::Error(item) = item else {
            continue;
        };
        if item.status != OrchestrationV2TurnItemStatus::Failed
            || item.run_id.as_ref() != Some(&run.id)
            || item.node_id != run.root_node_id
        {
            continue;
        }
        if latest_error.is_none_or(|old| {
            (epoch(&item.updated_at), item.ordinal, id_key(&item.id))
                > (epoch(&old.updated_at), old.ordinal, id_key(&old.id))
        }) {
            latest_error = Some(item);
        }
    }
    let failure = &latest_error?.failure;
    let session_error = p
        .provider_sessions
        .iter()
        .rfind(|s| s.provider_instance_id == p.thread.provider_instance_id)
        .and_then(|s| s.last_error.as_deref());
    if session_error.is_some_and(|error| error != failure.message.0)
        || failure.class != OrchestrationV2ProviderFailureClass::UsageLimit
    {
        return None;
    }
    p.runs
        .iter()
        .any(|r| r.ordinal > run.ordinal)
        .then_some(run)
}

pub fn derive_thread_pending_background_tasks(
    projection: Option<&OrchestrationV2ThreadProjection>,
    epoch: impl Fn(&WireDateTime) -> i64,
) -> Result<Vec<OrchestrationV2PendingBackgroundTask>, ClientError> {
    let Some(p) = projection else {
        return Ok(Vec::new());
    };
    let live: BTreeSet<_> = p
        .runs
        .iter()
        .filter(|run| {
            matches!(
                run.status,
                OrchestrationV2RunStatus::Queued
                    | OrchestrationV2RunStatus::Preparing
                    | OrchestrationV2RunStatus::Starting
                    | OrchestrationV2RunStatus::Running
            )
        })
        .map(|r| id_key(&r.id))
        .collect();
    let foreground: BTreeSet<_> = p
        .turn_items
        .iter()
        .filter(|item| {
            items::run_id(item)
                .as_ref()
                .is_some_and(|id| live.contains(&id_key(id)))
        })
        .filter_map(|item| {
            native_ref(item)?
                .native_id
                .as_ref()
                .map(|id| id.0.as_str())
                .filter(|id| !id.is_empty())
        })
        .collect();
    let settled: Vec<_> = p
        .runs
        .iter()
        .filter(|run| !live.contains(&id_key(&run.id)))
        .collect();
    let mut latest = latest_usage_limited(p, &epoch);
    if latest.is_none() {
        for run in &settled {
            if run.status == OrchestrationV2RunStatus::Queued
                && run.queue_held.flatten() == Some(true)
            {
                continue;
            }
            let outlived = run.status == OrchestrationV2RunStatus::Cancelled
                && run.started_at.is_none()
                && run.completed_at.as_ref().is_some_and(|end| {
                    settled.iter().any(|earlier| {
                        earlier.ordinal < run.ordinal
                            && earlier.status != OrchestrationV2RunStatus::Queued
                            && earlier
                                .completed_at
                                .as_ref()
                                .is_none_or(|other| epoch(other) >= epoch(end))
                    })
                });
            if !outlived && latest.is_none_or(|old| run.ordinal > old.ordinal) {
                latest = Some(run);
            }
        }
    }
    let mut tasks = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(latest) = latest {
        if !matches!(
            latest.status,
            OrchestrationV2RunStatus::Cancelled
                | OrchestrationV2RunStatus::Completed
                | OrchestrationV2RunStatus::Failed
                | OrchestrationV2RunStatus::Interrupted
                | OrchestrationV2RunStatus::Waiting
        ) {
            return Ok(tasks);
        }
        for thread in &p.provider_threads {
            if p.thread
                .active_provider_thread_id
                .as_ref()
                .is_some_and(|active| active != &thread.id)
            {
                continue;
            }
            for task in thread
                .pending_background_tasks
                .as_ref()
                .and_then(Option::as_ref)
                .into_iter()
                .flatten()
            {
                if foreground.contains(task_id(task))
                    || task_id(task).is_empty()
                    || !seen.insert(task_id(task).to_owned())
                {
                    continue;
                }
                let mut task = task.clone();
                match &mut task {
                    OrchestrationV2PendingBackgroundTask::Subagent(task) => {
                        task.provider_instance_id = match &thread.provider_instance_id {
                            IdOrIndex::Id(id) => Some(Some(id.clone())),
                            IdOrIndex::Index(_) => {
                                return Err(ClientError::InvalidPayload(
                                    "background task owner still has a compact id".into(),
                                ));
                            }
                        };
                        task.description = task
                            .description
                            .take()
                            .flatten()
                            .and_then(|d| {
                                (!d.0.js_trim().is_empty()).then(|| {
                                    t3_contracts::base_schemas::TrimmedNonEmptyString(
                                        d.0.js_trim().to_owned(),
                                    )
                                })
                            })
                            .map(Some);
                    }
                    OrchestrationV2PendingBackgroundTask::Command(task)
                    | OrchestrationV2PendingBackgroundTask::Monitor(task)
                    | OrchestrationV2PendingBackgroundTask::BackgroundTask(task) => {
                        task.description = task
                            .description
                            .take()
                            .flatten()
                            .and_then(|d| {
                                (!d.0.js_trim().is_empty()).then(|| {
                                    t3_contracts::base_schemas::TrimmedNonEmptyString(
                                        d.0.js_trim().to_owned(),
                                    )
                                })
                            })
                            .map(Some)
                    }
                }
                tasks.push(task);
            }
        }
        let rolled_back: BTreeSet<_> = p
            .runs
            .iter()
            .filter(|r| r.status == OrchestrationV2RunStatus::RolledBack)
            .map(|r| id_key(&r.id))
            .collect();
        for item in &p.turn_items {
            if !items::active(item)
                || items::run_id(item).as_ref().is_some_and(|id| {
                    live.contains(&id_key(id)) || rolled_back.contains(&id_key(id))
                })
            {
                continue;
            }
            let (kind, description, child) = match item {
                OrchestrationV2TurnItem::CommandExecution(v) => (
                    if v.monitor.flatten() == Some(true) {
                        "monitor"
                    } else {
                        "command"
                    },
                    v.title
                        .as_deref()
                        .filter(|t| !t.js_trim().is_empty())
                        .unwrap_or(&v.input)
                        .js_trim(),
                    None,
                ),
                OrchestrationV2TurnItem::DynamicTool(v)
                    if v.input.get("persistent") != Some(&serde_json::Value::Bool(true)) =>
                {
                    (
                        "background_task",
                        v.title
                            .as_deref()
                            .filter(|t| !t.js_trim().is_empty())
                            .or(v.tool_name.as_ref().map(|n| n.0.as_str()))
                            .unwrap_or("")
                            .js_trim(),
                        None,
                    )
                }
                OrchestrationV2TurnItem::Subagent(v) => (
                    "subagent",
                    v.title
                        .as_deref()
                        .filter(|t| !t.js_trim().is_empty())
                        .unwrap_or(&v.prompt)
                        .js_trim(),
                    v.child_thread_id.as_ref(),
                ),
                _ => continue,
            };
            let id = native_ref(item)
                .and_then(|r| r.native_id.as_ref())
                .map(|id| id.0.as_str())
                .filter(|id| !id.is_empty())
                .or_else(|| match items::id(item) {
                    IdOrIndex::Id(id) => Some(id.0.as_str()),
                    IdOrIndex::Index(_) => None,
                })
                .ok_or_else(|| {
                    ClientError::InvalidPayload("background item still has a compact id".into())
                })?;
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let mut task = json!({"taskId": id, "kind": kind});
            if !description.is_empty() {
                task["description"] = description.into();
            }
            if let Some(IdOrIndex::Id(child)) = child {
                task["childThreadId"] = json!(child);
            }
            tasks.push(serde_json::from_value(task)?);
        }
    }
    for link in p
        .thread
        .pull_requests
        .as_ref()
        .and_then(Option::as_ref)
        .into_iter()
        .flatten()
    {
        use t3_contracts::{
            pull_request::PullRequestState, thread_pull_request::ThreadPullRequestLinkSource,
        };
        let Some(watch) = link.watch.as_ref().and_then(Option::as_ref) else {
            continue;
        };
        if link.source == ThreadPullRequestLinkSource::StackDismissed
            || link
                .snapshot
                .as_ref()
                .is_some_and(|s| s.state == PullRequestState::Merged)
        {
            continue;
        }
        let id = format!(
            "pull-request-watch:{}/{}#{}",
            link.host.0.to_lowercase(),
            link.repository.0.to_lowercase(),
            link.number
        );
        let mut task = json!({"taskId": id, "description": format!("Watching pull request #{}", link.number), "kind": "monitor"});
        if watch.checks_pending.flatten() != Some(false) {
            task["awaited"] = true.into();
        }
        let task = serde_json::from_value(task)?;
        if let Some(index) = tasks.iter().position(|old| task_id(old) == id) {
            tasks[index] = task;
        } else {
            tasks.push(task);
        }
    }
    Ok(tasks)
}
