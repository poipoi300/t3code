use crate::base_schemas::TrimmedNonEmptyString;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpHandoffInput {
    #[serde(rename = "branch")]
    pub branch: TrimmedNonEmptyString,
    #[serde(
        rename = "baseRef",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_ref: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "startFromOrigin",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub start_from_origin: Option<Option<bool>>,
    #[serde(
        rename = "runSetupScript",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_setup_script: Option<Option<bool>>,
    #[serde(
        rename = "continuationPrompt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub continuation_prompt: Option<Option<TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpSetupScriptStartedStatus {
    #[serde(rename = "started")]
    Started,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpSetupScriptStarted {
    #[serde(rename = "status")]
    pub status: WorktreeMcpSetupScriptStartedStatus,
    #[serde(rename = "scriptName")]
    pub script_name: TrimmedNonEmptyString,
    #[serde(rename = "terminalId")]
    pub terminal_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpSetupScriptNoScriptStatus {
    #[serde(rename = "no-script")]
    NoScript,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpSetupScriptNoScript {
    #[serde(rename = "status")]
    pub status: WorktreeMcpSetupScriptNoScriptStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpSetupScriptSkippedStatus {
    #[serde(rename = "skipped")]
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpSetupScriptSkipped {
    #[serde(rename = "status")]
    pub status: WorktreeMcpSetupScriptSkippedStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpSetupScriptFailedStatus {
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpSetupScriptFailed {
    #[serde(rename = "status")]
    pub status: WorktreeMcpSetupScriptFailedStatus,
    #[serde(rename = "detail")]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorktreeMcpSetupScriptStatus {
    WorktreeMcpSetupScriptStarted(WorktreeMcpSetupScriptStarted),
    WorktreeMcpSetupScriptNoScript(WorktreeMcpSetupScriptNoScript),
    WorktreeMcpSetupScriptSkipped(WorktreeMcpSetupScriptSkipped),
    WorktreeMcpSetupScriptFailed(WorktreeMcpSetupScriptFailed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpContinuationScheduledStatus {
    #[serde(rename = "scheduled")]
    Scheduled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpContinuationScheduled {
    #[serde(rename = "status")]
    pub status: WorktreeMcpContinuationScheduledStatus,
    #[serde(rename = "delivery")]
    pub delivery: WorktreeMcpDelivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpContinuationSkippedStatus {
    #[serde(rename = "skipped")]
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpContinuationSkipped {
    #[serde(rename = "status")]
    pub status: WorktreeMcpContinuationSkippedStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpContinuationFailedStatus {
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpContinuationFailed {
    #[serde(rename = "status")]
    pub status: WorktreeMcpContinuationFailedStatus,
    #[serde(rename = "detail")]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorktreeMcpContinuationStatus {
    WorktreeMcpContinuationScheduled(WorktreeMcpContinuationScheduled),
    WorktreeMcpContinuationSkipped(WorktreeMcpContinuationSkipped),
    WorktreeMcpContinuationFailed(WorktreeMcpContinuationFailed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpDelivery {
    #[serde(rename = "started")]
    Started,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "steered")]
    Steered,
    #[serde(rename = "restarted")]
    Restarted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpHandoffResult {
    #[serde(rename = "worktreePath")]
    pub worktree_path: TrimmedNonEmptyString,
    #[serde(rename = "branch")]
    pub branch: TrimmedNonEmptyString,
    #[serde(rename = "baseRef")]
    pub base_ref: TrimmedNonEmptyString,
    #[serde(rename = "startedFromOrigin")]
    pub started_from_origin: bool,
    #[serde(rename = "setupScript")]
    pub setup_script: WorktreeMcpSetupScriptStatus,
    #[serde(rename = "continuation")]
    pub continuation: WorktreeMcpContinuationStatus,
    #[serde(rename = "note")]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpStatusResult {
    #[serde(rename = "attached")]
    pub attached: bool,
    #[serde(rename = "worktreePath", with = "crate::wire::nullable")]
    pub worktree_path: Option<TrimmedNonEmptyString>,
    #[serde(rename = "branch", with = "crate::wire::nullable")]
    pub branch: Option<TrimmedNonEmptyString>,
    #[serde(rename = "projectWorkspaceRoot")]
    pub project_workspace_root: TrimmedNonEmptyString,
    #[serde(rename = "defaultStartFromOrigin")]
    pub default_start_from_origin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpFailureCode {
    #[serde(rename = "capability_denied")]
    CapabilityDenied,
    #[serde(rename = "thread_not_found")]
    ThreadNotFound,
    #[serde(rename = "project_not_found")]
    ProjectNotFound,
    #[serde(rename = "already_in_worktree")]
    AlreadyInWorktree,
    #[serde(rename = "handoff_in_progress")]
    HandoffInProgress,
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "operation_failed")]
    OperationFailed,
    #[serde(rename = "thread_credential_required")]
    ThreadCredentialRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeMcpFailureTag {
    #[serde(rename = "WorktreeMcpFailure")]
    WorktreeMcpFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeMcpFailure {
    #[serde(rename = "_tag")]
    pub _tag: WorktreeMcpFailureTag,
    #[serde(rename = "code")]
    pub code: WorktreeMcpFailureCode,
    #[serde(rename = "message")]
    pub message: String,
}
