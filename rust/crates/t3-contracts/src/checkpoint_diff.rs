use crate::base_schemas::{NonNegativeInt, ThreadId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCountRange {
    pub from_turn_count: NonNegativeInt,
    pub to_turn_count: NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTurnDiff {
    pub from_turn_count: NonNegativeInt,
    pub to_turn_count: NonNegativeInt,
    pub thread_id: ThreadId,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationGetTurnDiffInput {
    pub from_turn_count: NonNegativeInt,
    pub to_turn_count: NonNegativeInt,
    pub thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ignore_whitespace: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationGetFullThreadDiffInput {
    pub thread_id: ThreadId,
    pub to_turn_count: NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ignore_whitespace: Option<bool>,
}

pub type OrchestrationGetTurnDiffResult = ThreadTurnDiff;
pub type OrchestrationGetFullThreadDiffResult = ThreadTurnDiff;
pub const CHECKPOINT_DIFF_TRUNCATED_MARKER: &str = "\n\n[truncated]";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestrationGetTurnDiffErrorTag {
    #[serde(rename = "OrchestrationGetTurnDiffError")]
    OrchestrationGetTurnDiffError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationGetTurnDiffError {
    #[serde(rename = "_tag")]
    pub _tag: OrchestrationGetTurnDiffErrorTag,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestrationGetFullThreadDiffErrorTag {
    #[serde(rename = "OrchestrationGetFullThreadDiffError")]
    OrchestrationGetFullThreadDiffError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationGetFullThreadDiffError {
    #[serde(rename = "_tag")]
    pub _tag: OrchestrationGetFullThreadDiffErrorTag,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}
