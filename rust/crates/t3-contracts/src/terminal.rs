use crate::{
    auth::EncodedNumber, base_schemas::TrimmedNonEmptyString, provider_instance::ProviderInstanceId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub type TerminalCols = u64;

pub type TerminalRows = u64;

pub type TerminalEnvironment = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalThreadInput {
    #[serde(rename = "threadId")]
    pub thread_id: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSessionInput {
    #[serde(rename = "threadId")]
    pub thread_id: TrimmedNonEmptyString,
    #[serde(rename = "terminalId")]
    pub terminal_id: TrimmedNonEmptyString,
}

pub type TerminalObserveInput = TerminalSessionInput;

pub type TerminalClearInput = TerminalSessionInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOpenInput {
    #[serde(rename = "threadId")]
    pub thread_id: TrimmedNonEmptyString,
    #[serde(rename = "terminalId")]
    pub terminal_id: TrimmedNonEmptyString,
    #[serde(rename = "cwd")]
    pub cwd: TrimmedNonEmptyString,
    #[serde(
        rename = "worktreePath",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub worktree_path: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "cols",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cols: Option<Option<TerminalCols>>,
    #[serde(
        rename = "rows",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub rows: Option<Option<TerminalRows>>,
    #[serde(
        rename = "env",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub env: Option<Option<TerminalEnvironment>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<ProviderInstanceId>>,
    #[serde(
        rename = "stopOnSettle",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stop_on_settle: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAttachInput {
    #[serde(rename = "threadId")]
    pub thread_id: TrimmedNonEmptyString,
    #[serde(rename = "terminalId")]
    pub terminal_id: TrimmedNonEmptyString,
    #[serde(
        rename = "cwd",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "worktreePath",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub worktree_path: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "cols",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cols: Option<Option<TerminalCols>>,
    #[serde(
        rename = "rows",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub rows: Option<Option<TerminalRows>>,
    #[serde(
        rename = "env",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub env: Option<Option<TerminalEnvironment>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<ProviderInstanceId>>,
    #[serde(
        rename = "stopOnSettle",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stop_on_settle: Option<Option<bool>>,
    #[serde(
        rename = "restartIfNotRunning",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub restart_if_not_running: Option<Option<bool>>,
    #[serde(
        rename = "resume",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub resume: Option<Option<bool>>,
    #[serde(
        rename = "maxHistoryBytes",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub max_history_bytes: Option<Option<u64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWriteInput {
    #[serde(rename = "threadId")]
    pub thread_id: TrimmedNonEmptyString,
    #[serde(rename = "terminalId")]
    pub terminal_id: TrimmedNonEmptyString,
    #[serde(rename = "data")]
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResizeInput {
    #[serde(rename = "threadId")]
    pub thread_id: TrimmedNonEmptyString,
    #[serde(rename = "terminalId")]
    pub terminal_id: TrimmedNonEmptyString,
    #[serde(rename = "cols")]
    pub cols: TerminalCols,
    #[serde(rename = "rows")]
    pub rows: TerminalRows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCloseInput {
    #[serde(rename = "threadId")]
    pub thread_id: TrimmedNonEmptyString,
    #[serde(
        rename = "terminalId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub terminal_id: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        rename = "deleteHistory",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delete_history: Option<Option<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalSessionStatus {
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "exited")]
    Exited,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSessionSnapshot {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(rename = "cwd")]
    pub cwd: String,
    #[serde(rename = "worktreePath", with = "crate::wire::nullable")]
    pub worktree_path: Option<TrimmedNonEmptyString>,
    #[serde(rename = "status")]
    pub status: TerminalSessionStatus,
    #[serde(rename = "pid", with = "crate::wire::nullable")]
    pub pid: Option<u64>,
    #[serde(rename = "history")]
    pub history: String,
    #[serde(rename = "exitCode", with = "crate::wire::nullable")]
    pub exit_code: Option<i64>,
    #[serde(rename = "exitSignal", with = "crate::wire::nullable")]
    pub exit_signal: Option<i64>,
    #[serde(rename = "label")]
    pub label: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(
        rename = "cols",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cols: Option<Option<TerminalCols>>,
    #[serde(
        rename = "rows",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub rows: Option<Option<TerminalRows>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSummary {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(rename = "cwd")]
    pub cwd: String,
    #[serde(rename = "worktreePath", with = "crate::wire::nullable")]
    pub worktree_path: Option<TrimmedNonEmptyString>,
    #[serde(rename = "status")]
    pub status: TerminalSessionStatus,
    #[serde(rename = "pid", with = "crate::wire::nullable")]
    pub pid: Option<u64>,
    #[serde(rename = "exitCode", with = "crate::wire::nullable")]
    pub exit_code: Option<i64>,
    #[serde(rename = "exitSignal", with = "crate::wire::nullable")]
    pub exit_signal: Option<i64>,
    #[serde(rename = "label")]
    pub label: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "hasRunningSubprocess")]
    pub has_running_subprocess: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalMetadataSnapshotEventType {
    #[serde(rename = "snapshot")]
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMetadataSnapshotEvent {
    #[serde(rename = "type")]
    pub r#type: TerminalMetadataSnapshotEventType,
    #[serde(rename = "terminals")]
    pub terminals: Vec<TerminalSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalMetadataUpsertEventType {
    #[serde(rename = "upsert")]
    Upsert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMetadataUpsertEvent {
    #[serde(rename = "type")]
    pub r#type: TerminalMetadataUpsertEventType,
    #[serde(rename = "terminal")]
    pub terminal: TerminalSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalMetadataRemoveEventType {
    #[serde(rename = "remove")]
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMetadataRemoveEvent {
    #[serde(rename = "type")]
    pub r#type: TerminalMetadataRemoveEventType,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TerminalMetadataStreamEvent {
    TerminalMetadataSnapshotEvent(TerminalMetadataSnapshotEvent),
    TerminalMetadataUpsertEvent(TerminalMetadataUpsertEvent),
    TerminalMetadataRemoveEvent(TerminalMetadataRemoveEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalStartedEventType {
    #[serde(rename = "started")]
    Started,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStartedEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalStartedEventType,
    #[serde(rename = "snapshot")]
    pub snapshot: TerminalSessionSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalOutputEventType {
    #[serde(rename = "output")]
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOutputEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalOutputEventType,
    #[serde(rename = "data")]
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalExitedEventType {
    #[serde(rename = "exited")]
    Exited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExitedEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalExitedEventType,
    #[serde(rename = "exitCode", with = "crate::wire::nullable")]
    pub exit_code: Option<i64>,
    #[serde(rename = "exitSignal", with = "crate::wire::nullable")]
    pub exit_signal: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalClosedEventType {
    #[serde(rename = "closed")]
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalClosedEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalClosedEventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalErrorEventType {
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalErrorEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalErrorEventType,
    #[serde(rename = "message")]
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalClearedEventType {
    #[serde(rename = "cleared")]
    Cleared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalClearedEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalClearedEventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalRestartedEventType {
    #[serde(rename = "restarted")]
    Restarted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRestartedEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalRestartedEventType,
    #[serde(rename = "snapshot")]
    pub snapshot: TerminalSessionSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalActivityEventType {
    #[serde(rename = "activity")]
    Activity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalActivityEvent {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sequence: Option<Option<u64>>,
    #[serde(rename = "type")]
    pub r#type: TerminalActivityEventType,
    #[serde(rename = "hasRunningSubprocess")]
    pub has_running_subprocess: bool,
    #[serde(rename = "label")]
    pub label: String,
    #[serde(
        rename = "cols",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cols: Option<Option<TerminalCols>>,
    #[serde(
        rename = "rows",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub rows: Option<Option<TerminalRows>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TerminalEvent {
    TerminalStartedEvent(TerminalStartedEvent),
    TerminalOutputEvent(TerminalOutputEvent),
    TerminalExitedEvent(TerminalExitedEvent),
    TerminalClosedEvent(TerminalClosedEvent),
    TerminalErrorEvent(TerminalErrorEvent),
    TerminalClearedEvent(TerminalClearedEvent),
    TerminalRestartedEvent(TerminalRestartedEvent),
    TerminalActivityEvent(TerminalActivityEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalAttachSnapshotEventType {
    #[serde(rename = "snapshot")]
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAttachSnapshotEvent {
    #[serde(rename = "type")]
    pub r#type: TerminalAttachSnapshotEventType,
    #[serde(rename = "snapshot")]
    pub snapshot: TerminalSessionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TerminalAttachStreamEvent {
    TerminalAttachSnapshotEvent(TerminalAttachSnapshotEvent),
    TerminalOutputEvent(TerminalOutputEvent),
    TerminalExitedEvent(TerminalExitedEvent),
    TerminalClosedEvent(TerminalClosedEvent),
    TerminalErrorEvent(TerminalErrorEvent),
    TerminalClearedEvent(TerminalClearedEvent),
    TerminalRestartedEvent(TerminalRestartedEvent),
    TerminalActivityEvent(TerminalActivityEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalCwdNotFoundErrorTag {
    #[serde(rename = "TerminalCwdNotFoundError")]
    TerminalCwdNotFoundError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCwdNotFoundError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalCwdNotFoundErrorTag,
    #[serde(rename = "cwd")]
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalCwdNotDirectoryErrorTag {
    #[serde(rename = "TerminalCwdNotDirectoryError")]
    TerminalCwdNotDirectoryError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCwdNotDirectoryError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalCwdNotDirectoryErrorTag,
    #[serde(rename = "cwd")]
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalCwdStatErrorTag {
    #[serde(rename = "TerminalCwdStatError")]
    TerminalCwdStatError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCwdStatError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalCwdStatErrorTag,
    #[serde(rename = "cwd")]
    pub cwd: String,
    #[serde(rename = "cause")]
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TerminalCwdError {
    TerminalCwdNotFoundError(TerminalCwdNotFoundError),
    TerminalCwdNotDirectoryError(TerminalCwdNotDirectoryError),
    TerminalCwdStatError(TerminalCwdStatError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalHistoryOperation {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "truncate")]
    Truncate,
    #[serde(rename = "migrate")]
    Migrate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalHistoryErrorTag {
    #[serde(rename = "TerminalHistoryError")]
    TerminalHistoryError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalHistoryError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalHistoryErrorTag,
    #[serde(rename = "operation")]
    pub operation: TerminalHistoryOperation,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalSessionLookupErrorTag {
    #[serde(rename = "TerminalSessionLookupError")]
    TerminalSessionLookupError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSessionLookupError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalSessionLookupErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalProviderInstanceNotFoundErrorTag {
    #[serde(rename = "TerminalProviderInstanceNotFoundError")]
    TerminalProviderInstanceNotFoundError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProviderInstanceNotFoundError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalProviderInstanceNotFoundErrorTag,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id: ProviderInstanceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalProviderEnvironmentErrorTag {
    #[serde(rename = "TerminalProviderEnvironmentError")]
    TerminalProviderEnvironmentError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProviderEnvironmentError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalProviderEnvironmentErrorTag,
    #[serde(rename = "providerInstanceId")]
    pub provider_instance_id: ProviderInstanceId,
    #[serde(rename = "cause")]
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalNotRunningErrorTag {
    #[serde(rename = "TerminalNotRunningError")]
    TerminalNotRunningError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalNotRunningError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalNotRunningErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalWriteErrorTag {
    #[serde(rename = "TerminalWriteError")]
    TerminalWriteError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWriteError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalWriteErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(rename = "terminalPid")]
    pub terminal_pid: EncodedNumber,
    #[serde(rename = "cause")]
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalResizeErrorTag {
    #[serde(rename = "TerminalResizeError")]
    TerminalResizeError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResizeError {
    #[serde(rename = "_tag")]
    pub _tag: TerminalResizeErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
    #[serde(rename = "terminalPid")]
    pub terminal_pid: EncodedNumber,
    #[serde(rename = "cols")]
    pub cols: TerminalCols,
    #[serde(rename = "rows")]
    pub rows: TerminalRows,
    #[serde(rename = "cause")]
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TerminalError {
    TerminalCwdError(TerminalCwdError),
    TerminalHistoryError(TerminalHistoryError),
    TerminalSessionLookupError(TerminalSessionLookupError),
    TerminalProviderInstanceNotFoundError(TerminalProviderInstanceNotFoundError),
    TerminalProviderEnvironmentError(TerminalProviderEnvironmentError),
    TerminalNotRunningError(TerminalNotRunningError),
    TerminalWriteError(TerminalWriteError),
    TerminalResizeError(TerminalResizeError),
}

pub const DEFAULT_TERMINAL_ID: &str = "term-1";
pub const DEFAULT_MAX_TERMINAL_BUFFER_BYTES: usize = 512 * 1024;
