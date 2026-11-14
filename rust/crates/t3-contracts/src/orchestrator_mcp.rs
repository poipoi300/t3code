//! Agent-facing orchestration MCP wire contracts.

use crate::{
    auth::{AuthMcpClientAccess, EncodedNumber},
    base_schemas::{
        ContextTransferId, EnvironmentId, IsoDateTime, MessageId, NodeId, NonNegativeInt,
        PositiveInt, ProjectId, RunId, RuntimeRequestId, ScheduledTaskId, SecretRef, ThreadId,
        TrimmedNonEmptyString, TurnItemId,
    },
    model::{ProviderOptionSelection, ProviderOptionSelectionValue},
    orchestration_v2::{
        OrchestrationV2Actor, OrchestrationV2CreationSource, OrchestrationV2RunStatus,
        OrchestrationV2TurnItemStatus,
    },
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::{ProviderInteractionMode, ProviderRequestKind, RuntimeMode},
    pull_request::PullRequestState,
    scheduled_task::{ScheduledTaskRunStatus, ScheduledTaskSchedule, ScheduledTaskUpsertSchedule},
    thread_pull_request::{ThreadLinkedPullRequest, ThreadPullRequestLinkSource},
    thread_title::{LastThreadTitleRegeneration, ThreadTitleRegeneration},
    wire::JsNumber,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct OrchestratorMcpClientRequestId(pub TrimmedNonEmptyString);

impl<'de> Deserialize<'de> for OrchestratorMcpClientRequestId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = TrimmedNonEmptyString::deserialize(deserializer)?;
        if value.0.encode_utf16().count() > 256 {
            return Err(serde::de::Error::custom(
                "clientRequestId exceeds 256 UTF-16 code units",
            ));
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct OrchestratorMcpClampedPositiveInt<const MAXIMUM: u64>(pub u64);

impl<'de, const MAXIMUM: u64> Deserialize<'de> for OrchestratorMcpClampedPositiveInt<MAXIMUM> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        if value == 0 {
            return Err(serde::de::Error::custom("expected a positive integer"));
        }
        Ok(Self(value.min(MAXIMUM)))
    }
}

pub type OrchestratorMcpEnvironmentTarget = Option<EnvironmentId>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestratorMcpTargetOptions {
    Selections(Vec<ProviderOptionSelection>),
    Record(std::collections::BTreeMap<String, ProviderOptionSelectionValue>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpTarget {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<ProviderInstanceId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver_kind: Option<Option<ProviderDriverKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub model: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub options: Option<Option<OrchestratorMcpTargetOptions>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<ProjectId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpInherit {
    #[serde(rename = "inherit")]
    Inherit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestratorMcpRuntimeMode {
    Inherit(OrchestratorMcpInherit),
    Mode(RuntimeMode),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestratorMcpInteractionMode {
    Inherit(OrchestratorMcpInherit),
    Mode(ProviderInteractionMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpTaskRole {
    #[serde(rename = "implementation")]
    Implementation,
    #[serde(rename = "research")]
    Research,
    #[serde(rename = "review")]
    Review,
    #[serde(rename = "design")]
    Design,
    #[serde(rename = "test")]
    Test,
    #[serde(rename = "general")]
    General,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpDelegatedTaskStatus {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpTerminalDelegatedTaskStatus {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpIsolation {
    #[serde(rename = "shared")]
    Shared,
    #[serde(rename = "worktree")]
    Worktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpDelegateMode {
    #[serde(rename = "async")]
    Async,
    #[serde(rename = "wait")]
    Wait,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpDelegateTaskInput {
    pub task: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub target: Option<Option<OrchestratorMcpTarget>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub role: Option<Option<OrchestratorMcpTaskRole>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub isolation: Option<Option<OrchestratorMcpIsolation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub mode: Option<Option<OrchestratorMcpDelegateMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub runtime_mode: Option<Option<OrchestratorMcpRuntimeMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub interaction_mode: Option<Option<OrchestratorMcpInteractionMode>>,
}

pub const ORCHESTRATOR_MCP_RUNNING_TASK_NEXT: &str = "Completion wakes this thread. Do not wait on it or poll it. Continue other work, and end your turn when none is left.";

pub type OrchestratorMcpRunningTaskNext = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpRemoteChild {
    pub environment_id: EnvironmentId,
    pub thread_id: ThreadId,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpPendingApproval {
    pub request_id: RuntimeRequestId,
    pub kind: ProviderRequestKind,
    pub run_id: RunId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpWorkState {
    #[serde(rename = "working")]
    Working,
    #[serde(rename = "waiting_for_children")]
    WaitingForChildren,
    #[serde(rename = "result_available")]
    ResultAvailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpDelegateTaskResult {
    pub task_id: NodeId,
    #[serde(with = "crate::wire::nullable")]
    pub child_thread_id: Option<ThreadId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub remote_child: Option<Option<OrchestratorMcpRemoteChild>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link: Option<Option<String>>,
    #[serde(with = "crate::wire::nullable")]
    pub child_run_id: Option<RunId>,
    pub child_node_id: NodeId,
    pub status: OrchestratorMcpDelegatedTaskStatus,
    pub work_state: OrchestratorMcpWorkState,
    pub has_pending_child_runs: bool,
    #[serde(with = "crate::wire::nullable")]
    pub latest_terminal_run_id: Option<RunId>,
    #[serde(with = "crate::wire::nullable")]
    pub latest_terminal_status: Option<OrchestratorMcpTerminalDelegatedTaskStatus>,
    #[serde(with = "crate::wire::nullable")]
    pub latest_terminal_summary: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub latest_terminal_result_context_transfer_id: Option<ContextTransferId>,
    pub provider_instance_id: ProviderInstanceId,
    #[serde(with = "crate::wire::nullable")]
    pub model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub executed_model: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub options: Option<Option<Vec<ProviderOptionSelection>>>,
    #[serde(with = "crate::wire::nullable")]
    pub summary: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub result_context_transfer_id: Option<ContextTransferId>,
    pub has_pending_question: bool,
    #[serde(with = "crate::wire::nullable")]
    pub pending_approval: Option<OrchestratorMcpPendingApproval>,
    pub wait_timed_out: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub next: Option<Option<OrchestratorMcpRunningTaskNext>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpTaskStatusInput {
    pub task_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpTaskCancelInput {
    pub task_id: NodeId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpTaskCancelStatus {
    #[serde(rename = "cancel_requested")]
    CancelRequested,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpTaskCancelResult {
    pub task_id: NodeId,
    pub status: OrchestratorMcpTaskCancelStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpCreateThreadRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub target: Option<Option<OrchestratorMcpTarget>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub runtime_mode: Option<Option<OrchestratorMcpRuntimeMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub interaction_mode: Option<Option<OrchestratorMcpInteractionMode>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpCreateThreadsInput {
    pub threads: Vec<OrchestratorMcpCreateThreadRequest>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpCreatedThreadStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "preparing")]
    Preparing,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "rolled_back")]
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpCreatedThread {
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link: Option<Option<String>>,
    #[serde(with = "crate::wire::nullable")]
    pub run_id: Option<RunId>,
    pub status: OrchestratorMcpCreatedThreadStatus,
    pub title: String,
    pub created_by: OrchestrationV2Actor,
    pub creation_source: OrchestrationV2CreationSource,
    pub provider_instance_id: ProviderInstanceId,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpCreateThreadsResult {
    pub threads: Vec<OrchestratorMcpCreatedThread>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpIdle {
    #[serde(rename = "idle")]
    Idle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestratorMcpThreadStatus {
    Idle(OrchestratorMcpIdle),
    Run(OrchestrationV2RunStatus),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadListInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<ProjectId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub all_projects: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub statuses: Option<Option<Vec<OrchestratorMcpThreadStatus>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title_contains: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub settled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub snoozed: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub include_subagents: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cursor: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub limit: Option<Option<PositiveInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadPullRequest {
    pub host: TrimmedNonEmptyString,
    pub repository: TrimmedNonEmptyString,
    pub number: PositiveInt,
    pub url: TrimmedNonEmptyString,
    pub source: ThreadPullRequestLinkSource,
    #[serde(with = "crate::wire::nullable")]
    pub state: Option<PullRequestState>,
    pub watching: bool,
    #[serde(with = "crate::wire::nullable")]
    pub watch_started_at: Option<IsoDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpBackgroundTaskKind {
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "monitor")]
    Monitor,
    #[serde(rename = "background_task")]
    BackgroundTask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "bool", into = "bool")]
pub struct OrchestratorMcpAwaited;
impl TryFrom<bool> for OrchestratorMcpAwaited {
    type Error = &'static str;
    fn try_from(value: bool) -> Result<Self, Self::Error> {
        if value {
            Ok(Self)
        } else {
            Err("expected true")
        }
    }
}
impl From<OrchestratorMcpAwaited> for bool {
    fn from(_: OrchestratorMcpAwaited) -> Self {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadBackgroundTask {
    pub task_id: TrimmedNonEmptyString,
    pub kind: OrchestratorMcpBackgroundTaskKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub awaited: Option<Option<OrchestratorMcpAwaited>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub child_thread_id: Option<Option<ThreadId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpRelationshipToParent {
    #[serde(rename = "fork")]
    Fork,
    #[serde(rename = "subagent")]
    Subagent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadListItem {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub title: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link: Option<Option<String>>,
    pub created_by: OrchestrationV2Actor,
    pub creation_source: OrchestrationV2CreationSource,
    pub status: OrchestratorMcpThreadStatus,
    #[serde(with = "crate::wire::nullable")]
    pub latest_run_id: Option<RunId>,
    pub provider_instance_id: ProviderInstanceId,
    pub model: String,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    #[serde(with = "crate::wire::nullable")]
    pub linked_pull_request: Option<ThreadLinkedPullRequest>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pull_requests: Option<Option<Vec<OrchestratorMcpThreadPullRequest>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pull_request_count: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub open_pull_request_count: Option<Option<NonNegativeInt>>,
    pub settled: bool,
    #[serde(with = "crate::wire::nullable")]
    pub settled_at: Option<IsoDateTime>,
    pub snoozed: bool,
    #[serde(with = "crate::wire::nullable")]
    pub snoozed_until: Option<IsoDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub blocked_on_thread_wait: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pending_background_tasks: Option<Option<Vec<OrchestratorMcpThreadBackgroundTask>>>,
    #[serde(with = "crate::wire::nullable")]
    pub parent_thread_id: Option<ThreadId>,
    #[serde(with = "crate::wire::nullable")]
    pub relationship_to_parent: Option<OrchestratorMcpRelationshipToParent>,
    pub item_count: NonNegativeInt,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadListResult {
    #[serde(with = "crate::wire::nullable")]
    pub project_id: Option<ProjectId>,
    #[serde(with = "crate::wire::nullable")]
    pub current_thread_id: Option<ThreadId>,
    pub threads: Vec<OrchestratorMcpThreadListItem>,
    #[serde(with = "crate::wire::nullable")]
    pub next_cursor: Option<NonNegativeInt>,
    pub total: NonNegativeInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpThreadReadView {
    #[serde(rename = "messages")]
    Messages,
    #[serde(rename = "activity")]
    Activity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadReadInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub item_id: Option<Option<TurnItemId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub text_offset: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub view: Option<Option<OrchestratorMcpThreadReadView>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub after_position: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub before_position: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub limit: Option<Option<OrchestratorMcpClampedPositiveInt<100>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_limit: Option<Option<OrchestratorMcpClampedPositiveInt<50>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub max_chars_per_item: Option<Option<OrchestratorMcpClampedPositiveInt<20000>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadDetail {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub title: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link: Option<Option<String>>,
    pub created_by: OrchestrationV2Actor,
    pub creation_source: OrchestrationV2CreationSource,
    pub status: OrchestratorMcpThreadStatus,
    #[serde(with = "crate::wire::nullable")]
    pub latest_run_id: Option<RunId>,
    pub provider_instance_id: ProviderInstanceId,
    pub model: String,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    #[serde(with = "crate::wire::nullable")]
    pub linked_pull_request: Option<ThreadLinkedPullRequest>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pull_requests: Option<Option<Vec<OrchestratorMcpThreadPullRequest>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pull_request_count: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub open_pull_request_count: Option<Option<NonNegativeInt>>,
    pub settled: bool,
    #[serde(with = "crate::wire::nullable")]
    pub settled_at: Option<IsoDateTime>,
    pub snoozed: bool,
    #[serde(with = "crate::wire::nullable")]
    pub snoozed_until: Option<IsoDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub blocked_on_thread_wait: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pending_background_tasks: Option<Option<Vec<OrchestratorMcpThreadBackgroundTask>>>,
    #[serde(with = "crate::wire::nullable")]
    pub parent_thread_id: Option<ThreadId>,
    #[serde(with = "crate::wire::nullable")]
    pub relationship_to_parent: Option<OrchestratorMcpRelationshipToParent>,
    pub item_count: NonNegativeInt,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub active_run_id: Option<RunId>,
    #[serde(with = "crate::wire::nullable")]
    pub title_regeneration: Option<ThreadTitleRegeneration>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub last_title_regeneration: Option<Option<LastThreadTitleRegeneration>>,
    #[serde(with = "crate::wire::nullable")]
    pub branch: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub worktree_path: Option<String>,
    pub run_count: NonNegativeInt,
    pub pending_request_count: NonNegativeInt,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadRun {
    pub run_id: RunId,
    pub ordinal: PositiveInt,
    pub status: OrchestrationV2RunStatus,
    pub provider_instance_id: ProviderInstanceId,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub executed_model: Option<Option<String>>,
    pub requested_at: IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub started_at: Option<IsoDateTime>,
    #[serde(with = "crate::wire::nullable")]
    pub completed_at: Option<IsoDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpTimelineVisibility {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "inherited")]
    Inherited,
    #[serde(rename = "synthetic")]
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadTimelineItem {
    pub position: NonNegativeInt,
    pub visibility: OrchestratorMcpTimelineVisibility,
    pub source_thread_id: ThreadId,
    pub item_id: TurnItemId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub subagent_id: Option<Option<NodeId>>,
    #[serde(with = "crate::wire::nullable")]
    pub run_id: Option<RunId>,
    #[serde(with = "crate::wire::nullable")]
    pub message_id: Option<MessageId>,
    #[serde(with = "crate::wire::nullable")]
    pub created_by: Option<OrchestrationV2Actor>,
    #[serde(with = "crate::wire::nullable")]
    pub creation_source: Option<OrchestrationV2CreationSource>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sender_thread_id: Option<Option<ThreadId>>,
    pub r#type: String,
    pub status: OrchestrationV2TurnItemStatus,
    #[serde(with = "crate::wire::nullable")]
    pub title: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub text: Option<String>,
    pub text_truncated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub next_text_offset: Option<Option<NonNegativeInt>>,
    pub updated_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadReadResult {
    pub thread: OrchestratorMcpThreadDetail,
    pub recent_runs: Vec<OrchestratorMcpThreadRun>,
    pub items: Vec<OrchestratorMcpThreadTimelineItem>,
    #[serde(with = "crate::wire::nullable")]
    pub next_position: Option<NonNegativeInt>,
    pub has_more: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub earlier_position: Option<Option<NonNegativeInt>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpThreadSendMode {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "queue")]
    Queue,
    #[serde(rename = "steer")]
    Steer,
    #[serde(rename = "restart")]
    Restart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadSendInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    pub thread_id: ThreadId,
    pub message: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub mode: Option<Option<OrchestratorMcpThreadSendMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_context: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpThreadDelivery {
    #[serde(rename = "started")]
    Started,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "steered")]
    Steered,
    #[serde(rename = "restarted")]
    Restarted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadSendResult {
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub link: Option<Option<String>>,
    pub message_id: MessageId,
    pub run_id: RunId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub task_id: Option<Option<NodeId>>,
    pub status: OrchestrationV2RunStatus,
    pub delivery: OrchestratorMcpThreadDelivery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadWaitInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<RunId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub until_delegated_work_settled: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpResultRun {
    pub run_id: RunId,
    pub status: OrchestrationV2RunStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadWaitResult {
    pub thread_id: ThreadId,
    #[serde(with = "crate::wire::nullable")]
    pub run_id: Option<RunId>,
    pub status: OrchestratorMcpThreadStatus,
    pub timed_out: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub next: Option<Option<OrchestratorMcpRunningTaskNext>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub result_run: Option<Option<OrchestratorMcpResultRun>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadInterruptInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<RunId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stop: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpThreadInterruptStatus {
    #[serde(rename = "interrupt_requested")]
    InterruptRequested,
    #[serde(rename = "no_active_run")]
    NoActiveRun,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "rolled_back")]
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpThreadInterruptResult {
    pub thread_id: ThreadId,
    #[serde(with = "crate::wire::nullable")]
    pub run_id: Option<RunId>,
    pub status: OrchestratorMcpThreadInterruptStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpCapabilitiesInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub search: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpModelOption {
    pub id: String,
    pub values: Vec<ProviderOptionSelectionValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub r#default: Option<Option<ProviderOptionSelectionValue>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpCapabilityModel {
    pub id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub options: Option<Option<Vec<OrchestratorMcpModelOption>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpProviderCapability {
    pub provider_instance_id: ProviderInstanceId,
    pub driver_kind: ProviderDriverKind,
    #[serde(with = "crate::wire::nullable")]
    pub display_name: Option<String>,
    pub models: Vec<OrchestratorMcpCapabilityModel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub omitted_models: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub runtime_modes: Option<Option<Vec<RuntimeMode>>>,
    pub can_run_child_task: bool,
    pub can_run_cross_provider_child_task: bool,
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpEnvironmentLinkStatus {
    #[serde(rename = "reachable")]
    Reachable,
    #[serde(rename = "unreachable")]
    Unreachable,
    #[serde(rename = "expired")]
    Expired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpEnvironmentLink {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub status: OrchestratorMcpEnvironmentLinkStatus,
    pub access: AuthMcpClientAccess,
    pub expires_at: IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpEnvironmentLinksResult {
    pub environment_id: EnvironmentId,
    pub links: Vec<OrchestratorMcpEnvironmentLink>,
    pub not_listed: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpFeatures {
    pub app_owned_subagents: bool,
    pub async_polling: bool,
    pub cancellation: bool,
    pub batch_thread_creation: bool,
    pub thread_management: bool,
    pub incremental_thread_read: bool,
    pub scheduled_tasks: bool,
    pub max_batch_threads: EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpCapabilitiesResult {
    #[serde(with = "crate::wire::nullable")]
    pub parent_thread_id: Option<ThreadId>,
    #[serde(with = "crate::wire::nullable")]
    pub inherited_provider_instance_id: Option<ProviderInstanceId>,
    #[serde(with = "crate::wire::nullable")]
    pub inherited_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub inherited_options: Option<Option<Vec<ProviderOptionSelection>>>,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub providers: Vec<OrchestratorMcpProviderCapability>,
    pub features: OrchestratorMcpFeatures,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OrchestratorMcpJsonSchedule(String);
impl TryFrom<String> for OrchestratorMcpJsonSchedule {
    type Error = serde_json::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        serde_json::from_str::<ScheduledTaskUpsertSchedule>(&value)?;
        Ok(Self(value))
    }
}
impl From<OrchestratorMcpJsonSchedule> for String {
    fn from(value: OrchestratorMcpJsonSchedule) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestratorMcpSchedule {
    Object(ScheduledTaskUpsertSchedule),
    Json(OrchestratorMcpJsonSchedule),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpScheduleTaskInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<ProjectId>>,
    pub prompt: TrimmedNonEmptyString,
    pub schedule: OrchestratorMcpSchedule,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bind_to_current_thread: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_session: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpSaveScheduledTaskInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<ProjectId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub schedule: Option<Option<OrchestratorMcpSchedule>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bind_to_current_thread: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_session: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scheduled_task_id: Option<Option<ScheduledTaskId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpWebhookSignature {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "set")]
    Set,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpScheduledTask {
    pub scheduled_task_id: ScheduledTaskId,
    pub title: String,
    pub prompt: String,
    pub enabled: bool,
    pub project_id: ProjectId,
    #[serde(with = "crate::wire::nullable")]
    pub bound_thread_id: Option<ThreadId>,
    pub fresh_session: bool,
    pub schedule: ScheduledTaskSchedule,
    #[serde(with = "crate::wire::nullable")]
    pub next_run_at: Option<IsoDateTime>,
    pub last_run_status: ScheduledTaskRunStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub webhook_url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub webhook_signature: Option<Option<OrchestratorMcpWebhookSignature>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub blocked_by_policy: Option<Option<String>>,
}

pub type OrchestratorMcpScheduleTaskResult = OrchestratorMcpScheduledTask;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpListScheduledTasksInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<ProjectId>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpListScheduledTasksResult {
    pub tasks: Vec<OrchestratorMcpScheduledTask>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpUpdateScheduledTaskInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    pub scheduled_task_id: ScheduledTaskId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub schedule: Option<Option<OrchestratorMcpSchedule>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bind_to_current_thread: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub fresh_session: Option<Option<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpSecretDelivery {
    #[serde(rename = "secretRef")]
    SecretRef,
    #[serde(rename = "file")]
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpRequestSecretInput {
    pub label: TrimmedNonEmptyString,
    pub reason: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub placeholder: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delivery: Option<Option<OrchestratorMcpSecretDelivery>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub value: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout_ms: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpSecretSavedStatus {
    #[serde(rename = "saved")]
    Saved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpSecretUnansweredStatus {
    #[serde(rename = "declined")]
    Declined,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "timed_out")]
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpSecretRefSaved {
    pub status: OrchestratorMcpSecretSavedStatus,
    pub secret_ref: SecretRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpSecretFileSaved {
    pub status: OrchestratorMcpSecretSavedStatus,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpSecretUnanswered {
    pub status: OrchestratorMcpSecretUnansweredStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub note: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestratorMcpRequestSecretResult {
    SecretRef(OrchestratorMcpSecretRefSaved),
    File(OrchestratorMcpSecretFileSaved),
    Unanswered(OrchestratorMcpSecretUnanswered),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpEnvironmentLinkInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hint: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub requested_access: Option<Option<AuthMcpClientAccess>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub client_request_id: Option<Option<OrchestratorMcpClientRequestId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpLinkPendingStatus {
    #[serde(rename = "pending")]
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpLinkLinkedStatus {
    #[serde(rename = "linked")]
    Linked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpLinkUnansweredStatus {
    #[serde(rename = "declined")]
    Declined,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpLinkFailedStatus {
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpLinkPending {
    pub status: OrchestratorMcpLinkPendingStatus,
    pub turn_item_id: TurnItemId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpLinkLinked {
    pub status: OrchestratorMcpLinkLinkedStatus,
    pub environment_id: EnvironmentId,
    pub label: String,
    pub access: AuthMcpClientAccess,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpLinkUnanswered {
    pub status: OrchestratorMcpLinkUnansweredStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpLinkFailed {
    pub status: OrchestratorMcpLinkFailedStatus,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrchestratorMcpEnvironmentLinkResult {
    Pending(OrchestratorMcpLinkPending),
    Linked(OrchestratorMcpLinkLinked),
    Unanswered(OrchestratorMcpLinkUnanswered),
    Failed(OrchestratorMcpLinkFailed),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpEnvironmentUnlinkInput {
    pub environment_id: EnvironmentId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpEnvironmentUnlinkResult {
    pub environment_id: EnvironmentId,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpDeleteScheduledTaskInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<EnvironmentId>>,
    pub scheduled_task_id: ScheduledTaskId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpDeleteScheduledTaskResult {
    pub scheduled_task_id: ScheduledTaskId,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpFailureTag {
    #[serde(rename = "OrchestratorMcpFailure")]
    OrchestratorMcpFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestratorMcpFailureCode {
    #[serde(rename = "capability_denied")]
    CapabilityDenied,
    #[serde(rename = "parent_not_active")]
    ParentNotActive,
    #[serde(rename = "provider_unavailable")]
    ProviderUnavailable,
    #[serde(rename = "model_unavailable")]
    ModelUnavailable,
    #[serde(rename = "runtime_mode_escalation_denied")]
    RuntimeModeEscalationDenied,
    #[serde(rename = "interaction_mode_escalation_denied")]
    InteractionModeEscalationDenied,
    #[serde(rename = "task_not_found")]
    TaskNotFound,
    #[serde(rename = "task_not_cancellable")]
    TaskNotCancellable,
    #[serde(rename = "thread_not_found")]
    ThreadNotFound,
    #[serde(rename = "run_not_found")]
    RunNotFound,
    #[serde(rename = "thread_not_sendable")]
    ThreadNotSendable,
    #[serde(rename = "thread_not_interruptible")]
    ThreadNotInterruptible,
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "orchestration_error")]
    OrchestrationError,
    #[serde(rename = "thread_credential_required")]
    ThreadCredentialRequired,
    #[serde(rename = "target_required")]
    TargetRequired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorMcpFailure {
    #[serde(rename = "_tag")]
    pub tag: OrchestratorMcpFailureTag,
    pub code: OrchestratorMcpFailureCode,
    pub message: String,
}
