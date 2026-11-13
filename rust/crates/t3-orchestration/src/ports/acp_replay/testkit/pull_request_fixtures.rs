use serde::{Deserialize, Serialize};
use serde_json::json;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    model_selection::ModelSelection,
    orchestration_v2::{
        AppThreadSettledOverride, OrchestrationV2AppThreadLineage, OrchestrationV2ThreadShell,
    },
    provider_policy::{ProviderInteractionMode, RuntimeMode},
    thread_pull_request::{ThreadLinkedPullRequest, ThreadPullRequestLink},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestTestThread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub title: String,
    pub model_selection: ModelSelection,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub pull_requests: Vec<ThreadPullRequestLink>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::nullable_optional"
    )]
    pub linked_pull_request: Option<Option<ThreadLinkedPullRequest>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::nullable_optional"
    )]
    pub branch_pull_request: Option<Option<ThreadLinkedPullRequest>>,
    pub latest_user_message_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<OrchestrationV2AppThreadLineage>,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
    pub settled_override: Option<AppThreadSettledOverride>,
    pub settled_at: Option<String>,
}
pub fn v2_pull_request_thread(
    thread: PullRequestTestThread,
) -> crate::error::Result<OrchestrationV2ThreadShell> {
    use crate::ports::decisions::orchestrator::{epoch_millis, iso_from_epoch_millis};
    let mut value = serde_json::to_value(&thread)?;
    value["providerInstanceId"] = json!(thread.model_selection.instance_id);
    for (key, default) in [
        ("createdBy", json!("user")),
        ("creationSource", json!("web")),
        ("activeProviderThreadId", json!(null)),
        ("forkedFrom", json!(null)),
        ("latestRunId", json!(null)),
        ("activeRunId", json!(null)),
        ("status", json!("idle")),
        ("pendingRuntimeRequest", json!(null)),
        ("latestVisibleMessage", json!(null)),
        ("hasActionableProposedPlan", json!(false)),
        ("itemCount", json!(0)),
        ("visibleItemCount", json!(0)),
        ("deletedAt", json!(null)),
    ] {
        value[key] = default;
    }
    if thread.lineage.is_none() {
        value["lineage"] =
            json!({"rootThreadId":thread.id,"parentThreadId":null,"relationshipToParent":null});
    }
    for key in [
        "latestUserMessageAt",
        "createdAt",
        "updatedAt",
        "archivedAt",
        "settledAt",
    ] {
        if let Some(date) = value[key].as_str() {
            if date.is_empty() && !matches!(key, "createdAt" | "updatedAt") {
                value[key] = json!(null);
                continue;
            }
            let millis = epoch_millis(date).ok_or_else(|| crate::error::Error::Service {
                operation: "pull request fixture date",
                detail: format!("invalid {key}: {date}"),
            })?;
            value[key] = json!(iso_from_epoch_millis(millis));
        }
    }
    Ok(serde_json::from_value(value)?)
}
