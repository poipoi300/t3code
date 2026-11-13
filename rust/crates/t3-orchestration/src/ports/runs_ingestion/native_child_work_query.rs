use crate::{
    api::*,
    error::{Error, Result},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{
    orchestration_v2::OrchestrationV2NativeChildWork, provider_instance::ProviderInstanceId,
};

pub struct ChildLaunch {
    pub native_task_id: String,
    pub title: Option<String>,
    pub prompt: String,
}
pub struct ChildWorkInput {
    pub instance_id: ProviderInstanceId,
    pub root_session_id: String,
    pub launch: ChildLaunch,
    pub siblings: Vec<ChildLaunch>,
}
// seam-shim: resolve the instance profile and call t3-provider's Antigravity child-work reader.
#[async_trait]
pub trait NativeChildWorkBackend: Send + Sync {
    async fn read(&self, input: ChildWorkInput) -> Result<Vec<OrchestrationV2NativeChildWork>>;
}
pub struct NativeChildWorkQuery {
    pub projections: Arc<dyn ProjectionReader>,
    pub backend: Arc<dyn NativeChildWorkBackend>,
}
pub(crate) fn records_value(records: ProjectionRecords) -> Result<Value> {
    let mut value = json!({"thread":t3_contracts::stored::to_value(&records.thread)?,"runs":[],"subagents":[],"providerThreads":[],"providerTurns":[],"messages":[],"turnItems":[],"runtimeRequests":[],"contextTransfers":[],"providerSessions":[],"attempts":[],"nodes":[]});
    for set in records.records {
        let (key, rows) = match set {
            ProjectionRecordSet::Runs(value) => ("runs", t3_contracts::stored::to_value(&value)?),
            ProjectionRecordSet::Subagents(value) => {
                ("subagents", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::ProviderThreads(value) => {
                ("providerThreads", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::ProviderTurns(value) => {
                ("providerTurns", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::Messages(value) => {
                ("messages", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::TurnItems(value) => {
                ("turnItems", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::RuntimeRequests(value) => {
                ("runtimeRequests", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::ContextTransfers(value) => {
                ("contextTransfers", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::ProviderSessions(value) => {
                ("providerSessions", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::Attempts(value) => {
                ("attempts", t3_contracts::stored::to_value(&value)?)
            }
            ProjectionRecordSet::Nodes(value) => ("nodes", t3_contracts::stored::to_value(&value)?),
            _ => continue,
        };
        value[key] = rows;
    }
    Ok(value)
}
impl NativeChildWorkQuery {
    pub async fn get(&self, thread_id: ThreadId) -> Result<Vec<OrchestrationV2NativeChildWork>> {
        let thread = self
            .projections
            .thread(thread_id.clone())
            .await?
            .ok_or(Error::ThreadNotFound(thread_id))?;
        let thread = t3_contracts::stored::to_value(&thread)?;
        if thread["lineage"]["relationshipToParent"] != "subagent"
            || thread["creationSource"] != "provider"
            || thread["lineage"]["parentThreadId"].is_null()
            || thread["forkedFrom"]["type"] != "node"
        {
            return Ok(vec![]);
        }
        let parent_id: ThreadId =
            serde_json::from_value(thread["lineage"]["parentThreadId"].clone())?;
        let parent = records_value(
            self.projections
                .records(
                    parent_id.clone(),
                    vec![RecordKind::Subagents],
                    RecordFilter::default(),
                )
                .await?,
        )?;
        let tasks = parent["subagents"]
            .as_array()
            .expect("a serialized typed vector is an array");
        let Some(task) = tasks.iter().find(|task| {
            task["id"] == thread["forkedFrom"]["nodeId"] && task["childThreadId"] == thread["id"]
        }) else {
            return Ok(vec![]);
        };
        let native = task["nativeTaskRef"]["nativeId"]
            .as_str()
            .filter(|id| !id.is_empty());
        if task["driver"] != "antigravity"
            || task["origin"] != "provider_native"
            || task["runId"].is_null()
            || native.is_none()
        {
            return Ok(vec![]);
        }
        let siblings = tasks
            .iter()
            .filter(|other| {
                other["id"] != task["id"]
                    && other["driver"] == "antigravity"
                    && other["origin"] == "provider_native"
                    && other["providerInstanceId"] == task["providerInstanceId"]
            })
            .collect::<Vec<_>>();
        let run_ids = std::iter::once(&task["runId"])
            .chain(
                siblings
                    .iter()
                    .filter_map(|other| other.get("runId").filter(|id| !id.is_null())),
            )
            .cloned()
            .map(serde_json::from_value)
            .collect::<std::result::Result<Vec<RunId>, _>>()?;
        let parent = records_value(
            self.projections
                .records(
                    parent_id,
                    vec![RecordKind::Runs, RecordKind::ProviderThreads],
                    RecordFilter {
                        run_ids: Some(run_ids),
                        ..Default::default()
                    },
                )
                .await?,
        )?;
        let provider = |id: &Value| {
            let run = parent["runs"]
                .as_array()?
                .iter()
                .find(|run| run["id"] == *id)?;
            parent["providerThreads"]
                .as_array()?
                .iter()
                .find(|provider| provider["id"] == run["providerThreadId"])
        };
        let Some(provider_thread) = provider(&task["runId"]) else {
            return Ok(vec![]);
        };
        let Some(root) = provider_thread["nativeThreadRef"]["nativeId"]
            .as_str()
            .filter(|id| !id.is_empty())
        else {
            return Ok(vec![]);
        };
        if provider_thread["providerInstanceId"] != task["providerInstanceId"] {
            return Ok(vec![]);
        }
        let launch = |task: &Value| ChildLaunch {
            native_task_id: task["nativeTaskRef"]["nativeId"]
                .as_str()
                .unwrap_or_default()
                .into(),
            title: task["title"].as_str().map(str::to_owned),
            prompt: task["prompt"].as_str().unwrap_or_default().into(),
        };
        let siblings = siblings
            .into_iter()
            .filter(|other| {
                other["nativeTaskRef"]["nativeId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
                    && provider(&other["runId"])
                        .is_some_and(|provider| provider["nativeThreadRef"]["nativeId"] == root)
            })
            .map(launch)
            .collect();
        self.backend
            .read(ChildWorkInput {
                instance_id: serde_json::from_value(task["providerInstanceId"].clone())?,
                root_session_id: root.into(),
                launch: launch(task),
                siblings,
            })
            .await
    }
}
