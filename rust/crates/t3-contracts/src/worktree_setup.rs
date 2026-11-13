use crate::base_schemas::{IsoDateTime, ThreadId, TrimmedNonEmptyString};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeSetupStageId {
    #[serde(rename = "fetch")]
    Fetch,
    #[serde(rename = "checkout")]
    Checkout,
    #[serde(rename = "submodules")]
    Submodules,
    #[serde(rename = "setup-script")]
    SetupScript,
    #[serde(rename = "agent")]
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeSetupStageStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeSetupPhase {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetupStage {
    #[serde(rename = "id")]
    pub id: WorktreeSetupStageId,
    #[serde(rename = "status")]
    pub status: WorktreeSetupStageStatus,
    #[serde(rename = "startedAt", with = "crate::wire::nullable")]
    pub started_at: Option<IsoDateTime>,
    #[serde(rename = "endedAt", with = "crate::wire::nullable")]
    pub ended_at: Option<IsoDateTime>,
    #[serde(rename = "percent", with = "crate::wire::nullable")]
    pub percent: Option<i64>,
    #[serde(rename = "detail", with = "crate::wire::nullable")]
    pub detail: Option<String>,
    #[serde(rename = "tail")]
    pub tail: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetupScript {
    #[serde(rename = "name")]
    pub name: TrimmedNonEmptyString,
    #[serde(rename = "command")]
    pub command: TrimmedNonEmptyString,
    #[serde(rename = "terminalId")]
    pub terminal_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetupSnapshot {
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
    #[serde(rename = "phase")]
    pub phase: WorktreeSetupPhase,
    #[serde(rename = "startedAt")]
    pub started_at: IsoDateTime,
    #[serde(rename = "endedAt", with = "crate::wire::nullable")]
    pub ended_at: Option<IsoDateTime>,
    #[serde(rename = "branch", with = "crate::wire::nullable")]
    pub branch: Option<TrimmedNonEmptyString>,
    #[serde(rename = "baseRef", with = "crate::wire::nullable")]
    pub base_ref: Option<TrimmedNonEmptyString>,
    #[serde(rename = "worktreePath", with = "crate::wire::nullable")]
    pub worktree_path: Option<TrimmedNonEmptyString>,
    #[serde(rename = "setupScript", with = "crate::wire::nullable")]
    pub setup_script: Option<WorktreeSetupScript>,
    #[serde(rename = "stages")]
    pub stages: Vec<WorktreeSetupStage>,
    #[serde(rename = "error", with = "crate::wire::nullable")]
    pub error: Option<String>,
    #[serde(rename = "sequence")]
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetupSubscribeInput {
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
}

pub type WorktreeSetupStreamEvent = Option<WorktreeSetupSnapshot>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetupCancelInput {
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetupCancelResult {
    #[serde(rename = "cancelled")]
    pub cancelled: bool,
}
