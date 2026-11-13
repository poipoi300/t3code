use crate::{api::*, error::Result};
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::application_event::ApplicationStoredEvent;
use tokio_util::sync::CancellationToken;

pub struct DelegatedTaskQuestionReactor {
    pub projections: Arc<dyn ProjectionReader>,
    pub dispatcher: Arc<dyn CommandDispatcher>,
}
fn terminal(status: &Value) -> bool {
    matches!(
        status.as_str(),
        Some("completed" | "failed" | "cancelled" | "interrupted")
    )
}
fn label(text: &str) -> Option<String> {
    let first = t3_contracts::wire::trim_js(t3_contracts::wire::trim_js(text).split('\n').next()?);
    if first.is_empty() {
        return None;
    }
    if first.encode_utf16().count() <= 80 {
        return Some(first.into());
    }
    let mut used = 0;
    let text = first
        .chars()
        .take_while(|ch| {
            used += ch.len_utf16();
            used <= 79
        })
        .collect::<String>();
    Some(format!("{text}…"))
}
impl DelegatedTaskQuestionReactor {
    async fn cancel_queued(&self, parent_id: ThreadId, requests: Vec<Value>) -> Result<()> {
        let parent = self
            .projections
            .records(
                parent_id.clone(),
                vec![RecordKind::Runs],
                RecordFilter::default(),
            )
            .await?;
        for set in parent.records {
            if let ProjectionRecordSet::Runs(runs) = set {
                for request in &requests {
                    let key = format!("{}:{}", parent_id.0, request.as_str().unwrap_or_default());
                    let message = format!("message:delegated-task-question:{key}");
                    for run in &runs {
                        let run = t3_contracts::stored::to_value(run)?;
                        if run["status"] == "queued" && run["userMessageId"] == message {
                            let _=self.dispatcher.dispatch(serde_json::from_value(json!({"type":"queued-run.cancel","commandId":format!("server:delegated-task-question-cancel:{key}"),"threadId":parent_id,"runId":run["id"]}))?).await;
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    async fn wake(&self, task: &Value, request: &Value) -> Result<()> {
        if task["childThreadId"].is_null() {
            return Ok(());
        }
        let key = format!(
            "{}:{}",
            task["threadId"].as_str().unwrap_or_default(),
            request["id"].as_str().unwrap_or_default()
        );
        let id = task["id"].as_str().unwrap_or_default();
        let child = task["childThreadId"].as_str().unwrap_or_default();
        let request_id = request["id"].as_str().unwrap_or_default();
        let text = if request["responseCapability"]["type"] == "message" {
            format!("Delegated task {id} asked a question and is still working.")
        } else {
            format!(
                "Delegated task {id} is blocked on a question and cannot continue until it is answered."
            )
        };
        let text = format!(
            "{text} Read it with t3_pending_request action read, threadId {child}, and requestId {request_id}. Answer it with action respond when the answer follows from the task or the user's instructions; otherwise ask the user, then answer it. If it is no longer pending, it was answered or dismissed."
        );
        let name = label(
            task["title"]
                .as_str()
                .filter(|title| !t3_contracts::wire::trim_js(title).is_empty())
                .unwrap_or(task["prompt"].as_str().unwrap_or_default()),
        )
        .map(|name| format!(" \"{name}\""))
        .unwrap_or_default();
        self.dispatcher.dispatch(serde_json::from_value(json!({"type":"message.dispatch","commandId":format!("server:delegated-task-question:{key}"),"threadId":task["threadId"],"messageId":format!("message:delegated-task-question:{key}"),"text":text,"notification":{"source":{"kind":"delegated_task","taskIds":[task["id"]],"childThreadId":task["childThreadId"]},"outcome":"updated","summary":format!("Delegated task{name} has a question")},"attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"server"}))?).await?;
        Ok(())
    }
    pub async fn handle(&self, event: &OrchestrationV2DomainEvent) -> Result<()> {
        let event = t3_contracts::stored::to_value(event)?;
        if event["type"] == "subagent.updated" {
            let task = &event["payload"];
            if task["origin"] != "app_owned"
                || task["childThreadId"].is_null()
                || !terminal(&task["status"])
            {
                return Ok(());
            }
            let child = self
                .projections
                .records(
                    serde_json::from_value(task["childThreadId"].clone())?,
                    vec![
                        RecordKind::RuntimeRequests,
                        RecordKind::Runs,
                        RecordKind::Nodes,
                    ],
                    RecordFilter::default(),
                )
                .await?;
            let mut requests = vec![];
            let mut runs = vec![];
            let mut nodes = vec![];
            for set in child.records {
                match set {
                    ProjectionRecordSet::RuntimeRequests(value) => requests = value,
                    ProjectionRecordSet::Runs(value) => runs = value,
                    ProjectionRecordSet::Nodes(value) => nodes = value,
                    _ => {}
                }
            }
            let thread = t3_contracts::stored::to_value(&child.thread)?;
            let runs = runs
                .iter()
                .map(t3_contracts::stored::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let run_ids = runs
                .iter()
                .filter(|run| {
                    run["delegatedTaskId"] == task["id"]
                        || (run.get("delegatedTaskId").is_none()
                            && thread["forkedFrom"]["type"] == "node"
                            && thread["forkedFrom"]["nodeId"] == task["id"])
                })
                .map(|run| &run["id"])
                .collect::<Vec<_>>();
            let nodes = nodes
                .iter()
                .map(t3_contracts::stored::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let node_ids = nodes
                .iter()
                .filter(|node| !node["runId"].is_null() && run_ids.contains(&&node["runId"]))
                .map(|node| &node["id"])
                .collect::<Vec<_>>();
            let requests = requests
                .iter()
                .map(t3_contracts::stored::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            return self
                .cancel_queued(
                    serde_json::from_value(task["threadId"].clone())?,
                    requests
                        .iter()
                        .filter(|request| {
                            request["kind"] == "user_input"
                                && node_ids.contains(&&request["nodeId"])
                        })
                        .map(|request| request["id"].clone())
                        .collect(),
                )
                .await;
        }
        if event["type"] == "run.updated" {
            if !matches!(
                event["payload"]["status"].as_str(),
                Some("completed" | "failed" | "waiting")
            ) {
                return Ok(());
            }
            let parent = self
                .projections
                .records(
                    serde_json::from_value(event["threadId"].clone())?,
                    vec![RecordKind::Subagents],
                    RecordFilter::default(),
                )
                .await?;
            if parent.thread.archived_at.is_some() || parent.thread.deleted_at.is_some() {
                return Ok(());
            }
            for set in parent.records {
                if let ProjectionRecordSet::Subagents(tasks) = set {
                    for task in tasks {
                        let task = t3_contracts::stored::to_value(&task)?;
                        if task["origin"] != "app_owned"
                            || task["runId"] != event["payload"]["id"]
                            || task["completionWake"] != "settled_only"
                            || task["completionDelivery"]["state"] == "disposed"
                            || task["childThreadId"].is_null()
                            || terminal(&task["status"])
                        {
                            continue;
                        }
                        let child = self
                            .projections
                            .records(
                                serde_json::from_value(task["childThreadId"].clone())?,
                                vec![RecordKind::RuntimeRequests],
                                RecordFilter::default(),
                            )
                            .await?;
                        for set in child.records {
                            if let ProjectionRecordSet::RuntimeRequests(requests) = set {
                                for request in requests {
                                    let request = t3_contracts::stored::to_value(&request)?;
                                    if request["kind"] == "user_input"
                                        && request["status"] == "pending"
                                        && request["responseCapability"]["type"] == "live"
                                    {
                                        let _ = self.wake(&task, &request).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            return Ok(());
        }
        if event["type"] != "runtime-request.updated" || event["payload"]["kind"] != "user_input" {
            return Ok(());
        }
        let thread_id: ThreadId = serde_json::from_value(event["threadId"].clone())?;
        let Some(child) = self.projections.thread(thread_id.clone()).await? else {
            return Ok(());
        };
        let child = t3_contracts::stored::to_value(&child)?;
        if child["lineage"]["parentThreadId"].is_null() {
            return Ok(());
        }
        let parent_id: ThreadId =
            serde_json::from_value(child["lineage"]["parentThreadId"].clone())?;
        let requests = self
            .projections
            .records(
                thread_id.clone(),
                vec![RecordKind::RuntimeRequests],
                RecordFilter::default(),
            )
            .await?;
        let request = requests.records.into_iter().find_map(|set| match set {
            ProjectionRecordSet::RuntimeRequests(requests) => {
                requests.into_iter().find_map(|request| {
                    let value = t3_contracts::stored::to_value(&request).ok()?;
                    (value["id"] == event["payload"]["id"]).then_some(value)
                })
            }
            _ => None,
        });
        let parent = self
            .projections
            .records(
                parent_id.clone(),
                vec![RecordKind::Subagents, RecordKind::Runs],
                RecordFilter::default(),
            )
            .await?;
        let mut runs = vec![];
        let mut tasks = vec![];
        for set in parent.records {
            match set {
                ProjectionRecordSet::Runs(value) => runs = value,
                ProjectionRecordSet::Subagents(value) => tasks = value,
                _ => {}
            }
        }
        let runs = t3_contracts::stored::to_value(&runs)?;
        let key = format!(
            "{}:{}",
            parent_id.0,
            event["payload"]["id"].as_str().unwrap_or_default()
        );
        let message = format!("message:delegated-task-question:{key}");
        if event["payload"]["status"] != "pending"
            || request
                .as_ref()
                .is_none_or(|request| request["status"] != "pending")
        {
            if let Some(queued) = runs
                .as_array()
                .expect("a serialized typed vector is an array")
                .iter()
                .find(|run| run["status"] == "queued" && run["userMessageId"] == message)
            {
                self.dispatcher.dispatch(serde_json::from_value(json!({"type":"queued-run.cancel","commandId":format!("server:delegated-task-question-cancel:{key}"),"threadId":parent_id,"runId":queued["id"]}))?).await?;
            }
            return Ok(());
        }
        if parent.thread.archived_at.is_some() || parent.thread.deleted_at.is_some() {
            return Ok(());
        }
        let child_runs = self
            .projections
            .records(thread_id, vec![RecordKind::Runs], RecordFilter::default())
            .await?;
        let mut candidates = vec![];
        for set in child_runs.records {
            if let ProjectionRecordSet::Runs(value) = set {
                candidates = value;
            }
        }
        let candidates = t3_contracts::stored::to_value(&candidates)?;
        let candidates = candidates
            .as_array()
            .expect("a serialized typed vector is an array");
        let run = candidates
            .iter()
            .find(|run| run["id"] == event["runId"])
            .or_else(|| candidates.iter().max_by_key(|run| run["ordinal"].as_u64()));
        let task_id = run
            .and_then(|run| run.get("delegatedTaskId"))
            .filter(|id| !id.is_null())
            .or_else(|| {
                (child["forkedFrom"]["type"] == "node").then_some(&child["forkedFrom"]["nodeId"])
            });
        let Some(task_id) = task_id else {
            return Ok(());
        };
        for task in tasks {
            let task = t3_contracts::stored::to_value(&task)?;
            if task["id"] != *task_id || task["origin"] != "app_owned" {
                continue;
            }
            if task["completionWake"] == "settled_only"
                && event["payload"]["responseCapability"]["type"] == "live"
                && runs
                    .as_array()
                    .expect("a serialized typed vector is an array")
                    .iter()
                    .any(|run| {
                        run["id"] == task["runId"]
                            && matches!(
                                run["status"].as_str(),
                                Some("preparing" | "starting" | "running")
                            )
                    })
            {
                return Ok(());
            }
            return self
                .wake(
                    &task,
                    request.as_ref().expect("pending request checked above"),
                )
                .await;
        }
        Ok(())
    }
    pub fn start(
        self: Arc<Self>,
        committer: &dyn EventCommitter,
        cancellation: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let mut events = committer.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! { biased; _=cancellation.cancelled()=>break,event=events.recv()=>match event { Ok(ApplicationStoredEvent::Orchestration(event))=>{let _=self.handle(&event.event).await;},Ok(_)=>{},Err(_)=>break } }
            }
        })
    }
}
