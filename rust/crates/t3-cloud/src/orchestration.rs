//! Native bindings to the authoritative orchestration reader and command intake.
use crate::{
    Error, Result,
    link_requests::{ActiveParent, LinkRequestStore},
    peer::CallLimits,
    remote::{RemoteParent, RemoteTask, RemoteTaskStore, TaskKey},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::{EnvironmentId, NodeId, ProjectId, ThreadId, TurnItemId},
    orchestration_v2::*,
};
use t3_orchestration::projections::store::ProjectionStore;
use t3_orchestration::{
    api::{EventCommitter, RecordFilter, RecordKind},
    ports::thread_project::thread_management_service::ThreadManagementService,
};
use tokio::sync::broadcast;

#[async_trait]
pub trait ProjectRepository: Send + Sync {
    async fn canonical_key(&self, project: &ProjectId) -> Result<Option<String>>;
}
pub struct OrchestrationStore {
    management: Arc<ThreadManagementService>,
    projections: Arc<ProjectionStore>,
    sink: Arc<dyn EventCommitter>,
    repository: Arc<dyn ProjectRepository>,
    events: broadcast::Sender<TaskKey>,
}
impl OrchestrationStore {
    pub fn new(
        management: Arc<ThreadManagementService>,
        projections: Arc<ProjectionStore>,
        repository: Arc<dyn ProjectRepository>,
        sink: Arc<dyn EventCommitter>,
    ) -> (Arc<Self>, EventForwarder) {
        let (events, _) = broadcast::channel(256);
        let mut source = sink.subscribe();
        let cancel = t3_process::CancellationToken::new();
        let cancelled = cancel.clone();
        let forwarded = events.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _=cancelled.cancelled()=>return Ok(()),
                    event=source.recv()=>match event {
                        Ok(t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored))=>if let OrchestrationV2DomainEvent::SubagentUpdated(event)=stored.event {if let t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id)=event.payload.id {let _=forwarded.send((event.thread_id,id));}},
                        Ok(_)=>{},Err(broadcast::error::RecvError::Closed)=>return Err(Error::Storage),
                        Err(broadcast::error::RecvError::Lagged(_))=>{let _=forwarded.send((ThreadId(String::new()),NodeId(String::new())));},
                    }
                }
            }
        });
        (
            Arc::new(Self {
                management,
                projections,
                sink,
                repository,
                events,
            }),
            EventForwarder { cancel, task },
        )
    }
    async fn records(
        &self,
        thread: &ThreadId,
        fields: Vec<RecordKind>,
        filter: RecordFilter,
    ) -> Result<Value> {
        let projection = self
            .management
            .records(thread.clone(), fields, filter)
            .await
            .map_err(|_| Error::Storage)?;
        if projection.thread.deleted_at.is_some() {
            return Err(Error::NotLinked);
        }
        crate::encode(&projection)
    }
    async fn parent_value(&self, thread: &ThreadId) -> Result<Value> {
        self.records(thread, vec![RecordKind::Runs], RecordFilter::default())
            .await
    }
    async fn task_value(&self, key: &TaskKey) -> Result<Option<(Value, Value)>> {
        let projection = self
            .records(
                &key.0,
                vec![RecordKind::Subagents],
                RecordFilter {
                    subagent_ids: Some(vec![key.1.clone()]),
                    ..Default::default()
                },
            )
            .await?;
        Ok(projection["subagents"]
            .as_array()
            .and_then(|tasks| tasks.iter().find(|task| task["id"] == key.1.0))
            .cloned()
            .map(|task| (projection["thread"].clone(), task)))
    }
    fn task_from(thread: &Value, task: &Value) -> Result<RemoteTask> {
        Ok(RemoteTask {
            key: (
                crate::decode(thread["id"].clone())?,
                crate::decode(task["id"].clone())?,
            ),
            remote: crate::decode(task["remoteChild"].clone())?,
            status: task["status"].as_str().ok_or(Error::Protocol)?.into(),
            result: task["result"].as_str().map(str::to_owned),
            model_selection: crate::decode(
                task.get("modelSelection")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or_else(
                        || json!({"instanceId":task["providerInstanceId"],"model":task["model"]}),
                    ),
            )?,
            limits: CallLimits {
                runtime_mode: crate::decode(thread["runtimeMode"].clone())?,
                interaction_mode: crate::decode(thread["interactionMode"].clone())?,
            },
        })
    }
    async fn dispatch(&self, command: OrchestrationV2InternalCommand) -> Result<()> {
        self.management
            .dispatch(OrchestrationV2ServerCommand::Internal(Box::new(command)))
            .await
            .map_err(|_| Error::Storage)
            .and_then(|receipt| {
                if receipt.status == t3_orchestration::stores::receipts::ReceiptStatus::Accepted {
                    Ok(())
                } else {
                    Err(Error::Superseded)
                }
            })
    }
}
fn active_parent(projection: &Value) -> Result<Option<ActiveParent>> {
    let Some(run) = projection["runs"].as_array().and_then(|runs| {
        runs.iter()
            .filter(|run| {
                matches!(
                    run["status"].as_str(),
                    Some("preparing" | "starting" | "running" | "waiting")
                )
            })
            .max_by_key(|run| run["ordinal"].as_u64().unwrap_or(0))
    }) else {
        return Ok(None);
    };
    if run["rootNodeId"].is_null() {
        return Ok(None);
    }
    Ok(Some(ActiveParent {
        run_id: crate::decode(run["id"].clone())?,
        node_id: crate::decode(run["rootNodeId"].clone())?,
        provider_instance_id: crate::decode(run["providerInstanceId"].clone())?,
    }))
}
#[async_trait]
impl LinkRequestStore for OrchestrationStore {
    async fn card(
        &self,
        thread: &ThreadId,
        id: &TurnItemId,
    ) -> Result<Option<InternalCommandLinkRequestRecord>> {
        let projection = self
            .records(
                thread,
                vec![RecordKind::TurnItems],
                RecordFilter {
                    turn_item_types: Some(vec!["link_request".into()]),
                    ..Default::default()
                },
            )
            .await?;
        let Some(item) = projection["turnItems"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == id.0))
        else {
            return Ok(None);
        };
        if item["runId"].is_null() || item["nodeId"].is_null() {
            return Ok(None);
        }
        let mut item = item.clone();
        item["commandId"] = json!("read-link-card");
        item["turnItemId"] = crate::encode(id)?;
        crate::decode(item).map(Some)
    }
    async fn active_parent(&self, thread: &ThreadId) -> Result<Option<ActiveParent>> {
        active_parent(&self.parent_value(thread).await?)
    }
    async fn record(&self, command: InternalCommandLinkRequestRecord) -> Result<()> {
        self.dispatch(OrchestrationV2InternalCommand::LinkRequestRecord(Box::new(
            command,
        )))
        .await
    }
}
#[async_trait]
impl RemoteTaskStore for OrchestrationStore {
    async fn parent(&self, thread: &ThreadId) -> Result<RemoteParent> {
        let value = self.parent_value(thread).await?;
        let t = &value["thread"];
        let project = crate::decode::<ProjectId>(t["projectId"].clone())?;
        Ok(RemoteParent {
            thread_id: thread.clone(),
            project_id: project.clone(),
            provider_instance_id: crate::decode(t["providerInstanceId"].clone())?,
            title: t["title"].as_str().ok_or(Error::Protocol)?.into(),
            model_selection: crate::decode(t["modelSelection"].clone())?,
            limits: CallLimits {
                runtime_mode: crate::decode(t["runtimeMode"].clone())?,
                interaction_mode: crate::decode(t["interactionMode"].clone())?,
            },
            active: active_parent(&value)?,
            repository_key: self.repository.canonical_key(&project).await?,
        })
    }
    async fn task(&self, key: &TaskKey) -> Result<Option<RemoteTask>> {
        self.task_value(key)
            .await?
            .filter(|(_, task)| {
                task.get("remoteChild")
                    .is_some_and(|remote| !remote.is_null())
            })
            .map(|(thread, task)| Self::task_from(&thread, &task))
            .transpose()
    }
    async fn outstanding(&self) -> Result<Vec<TaskKey>> {
        self.projections
            .open_remote_delegated_tasks()
            .await
            .map(|tasks| {
                tasks
                    .into_iter()
                    .map(|task| (task.parent_thread_id, task.task_id))
                    .collect()
            })
            .map_err(|_| Error::Storage)
    }
    fn subscribe(&self) -> broadcast::Receiver<TaskKey> {
        self.events.subscribe()
    }
    async fn request(&self, command: InternalCommandDelegatedTaskRemoteRequest) -> Result<NodeId> {
        let parent = command.parent_thread_id.clone();
        let remote = command.remote_child.clone();
        let command_id = command.command_id.clone();
        self.dispatch(OrchestrationV2InternalCommand::DelegatedTaskRemoteRequest(
            Box::new(command),
        ))
        .await?;
        for stored in self
            .sink
            .events_by_command(command_id)
            .await
            .map_err(|_| Error::Storage)?
        {
            if let OrchestrationV2DomainEvent::SubagentUpdated(event) = stored.event
                && let t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) = event.payload.id
            {
                return Ok(id);
            }
        }
        let projection = self
            .records(
                &parent,
                vec![RecordKind::Subagents],
                RecordFilter::default(),
            )
            .await?;
        projection["subagents"]
            .as_array()
            .and_then(|tasks| {
                tasks.iter().find(|task| {
                    task["remoteChild"]["threadId"] == remote.thread_id.0
                        && task["remoteChild"]["environmentId"] == remote.environment_id.0
                        && task["remoteChild"]["runId"]
                            == crate::encode(&remote.run_id).unwrap_or(Value::Null)
                })
            })
            .ok_or(Error::Storage)
            .and_then(|task| crate::decode(task["id"].clone()))
    }
    async fn complete(&self, command: InternalCommandDelegatedTaskRemoteComplete) -> Result<()> {
        self.dispatch(OrchestrationV2InternalCommand::DelegatedTaskRemoteComplete(
            Box::new(command),
        ))
        .await
    }
    async fn shared_result_run(&self, key: &TaskKey) -> Result<bool> {
        let Some((_, current)) = self.task_value(key).await? else {
            return Ok(false);
        };
        let projection = self
            .records(&key.0, vec![RecordKind::Subagents], RecordFilter::default())
            .await?;
        Ok(projection["subagents"].as_array().is_some_and(|tasks| {
            tasks.iter().any(|task| {
                task["id"] != key.1.0
                    && task["remoteChild"]["threadId"] == current["remoteChild"]["threadId"]
                    && task["remoteChild"]["environmentId"]
                        == current["remoteChild"]["environmentId"]
                    && (current["startedAt"].is_null()
                        || task["completedAt"].is_null()
                        || task["completedAt"].as_str() > current["startedAt"].as_str())
            })
        }))
    }
    async fn finished_owner(
        &self,
        parent: &ThreadId,
        environment: &EnvironmentId,
        thread: &ThreadId,
    ) -> Result<Option<RemoteTask>> {
        let projection = self
            .records(parent, vec![RecordKind::Subagents], RecordFilter::default())
            .await?;
        projection["subagents"]
            .as_array()
            .and_then(|tasks| {
                tasks.iter().find(|task| {
                    task["remoteChild"]["environmentId"] == environment.0
                        && task["remoteChild"]["threadId"] == thread.0
                        && !task["result"].is_null()
                })
            })
            .map(|task| Self::task_from(&projection["thread"], task))
            .transpose()
    }
}
pub struct EventForwarder {
    cancel: t3_process::CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
}
impl EventForwarder {
    pub async fn shutdown(mut self) -> Result<()> {
        self.cancel.cancel();
        (&mut self.task).await.map_err(|_| Error::Process)?
    }
}
impl Drop for EventForwarder {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
