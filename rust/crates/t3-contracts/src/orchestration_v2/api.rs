use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SubscribeCommandOutputInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "itemId")]
    pub item_id: crate::base_schemas::TurnItemId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandOutputFrameKind {
    #[serde(rename = "replace")]
    Replace,
    #[serde(rename = "append")]
    Append,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CommandOutputFrame {
    #[serde(rename = "kind")]
    pub kind: CommandOutputFrameKind,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(rename = "truncated")]
    pub truncated: bool,
    #[serde(rename = "running")]
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandOutputErrorTag {
    #[serde(rename = "OrchestrationV2CommandOutputError")]
    OrchestrationV2CommandOutputError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2CommandOutputError {
    #[serde(rename = "_tag")]
    pub _tag: CommandOutputErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SubscribePromptSuggestionInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2PromptSuggestionValue {
    #[serde(rename = "runId")]
    pub run_id: crate::base_schemas::RunId,
    #[serde(rename = "text")]
    pub text: String,
}

pub type OrchestrationV2PromptSuggestion = Option<OrchestrationV2PromptSuggestionValue>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadLaunchInputInitialMessage {
    #[serde(
        rename = "messageId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub message_id: Option<Option<crate::base_schemas::MessageId>>,
    #[serde(rename = "text")]
    pub text: String,
    #[serde(
        rename = "context",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub context: Option<Option<crate::composer_context::OrchestrationMessageContext>>,
    #[serde(rename = "attachments")]
    pub attachments: Vec<crate::chat_attachment::ChatAttachment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadLaunchInput {
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(
        rename = "creationSource",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub creation_source: Option<Option<OrchestrationV2CreationSource>>,
    #[serde(
        rename = "threadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(
        rename = "reuseExistingThread",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reuse_existing_thread: Option<Option<bool>>,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(rename = "title")]
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "generateTitle",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub generate_title: Option<Option<bool>>,
    #[serde(rename = "modelSelection")]
    pub model_selection: crate::model_selection::ModelSelection,
    #[serde(rename = "runtimeMode")]
    pub runtime_mode: crate::provider_policy::RuntimeMode,
    #[serde(rename = "interactionMode")]
    pub interaction_mode: crate::provider_policy::ProviderInteractionMode,
    #[serde(rename = "workspaceStrategy")]
    pub workspace_strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
    #[serde(
        rename = "initialMessage",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub initial_message: Option<Option<ThreadLaunchInputInitialMessage>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadLaunchResult {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "projection")]
    pub projection: OrchestrationV2ThreadProjection,
    #[serde(rename = "resumed")]
    pub resumed: bool,
    #[serde(
        rename = "sequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub sequence: Option<crate::base_schemas::NonNegativeInt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadLinkMetadata {
    #[serde(rename = "title")]
    pub title: String,
    #[serde(rename = "projectId", with = "crate::wire::nullable")]
    pub project_id: Option<crate::base_schemas::ProjectId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetWorktreeThreadIdsInput {
    #[serde(rename = "worktreePath")]
    pub worktree_path: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SearchThreadsInput {
    #[serde(rename = "query")]
    pub query: crate::base_schemas::TrimmedString,
    #[serde(
        rename = "limit",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub limit: Option<i64>,
    #[serde(
        rename = "olderSettledBefore",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub older_settled_before: Option<crate::base_schemas::IsoDateTime>,
    #[serde(
        rename = "projectIds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub project_ids: Option<Vec<crate::base_schemas::ProjectId>>,
    #[serde(
        rename = "environmentLabel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub environment_label: Option<String>,
    #[serde(
        rename = "titlesOnly",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub titles_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SearchThreadsResult {
    #[serde(rename = "matches")]
    pub matches: Vec<crate::thread_search::OrchestrationThreadSearchMatch>,
    #[serde(
        rename = "snapshotSequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub snapshot_sequence: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(
        rename = "threads",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub threads: Option<Vec<OrchestrationV2ThreadShell>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2OlderSettledCursor {
    #[serde(rename = "settledAt")]
    pub settled_at: crate::base_schemas::IsoDateTime,
    #[serde(
        rename = "threadId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub thread_id: Option<crate::base_schemas::ThreadId>,
}

pub type OrchestrationV2WorktreeThreadIds = OrchestrationV2GetThreadTitlesInput;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SubscribeThreadInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "afterSequence",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub after_sequence: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(
        rename = "requestCompletionMarker",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub request_completion_marker: Option<bool>,
    #[serde(
        rename = "acceptBoundedSnapshot",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub accept_bounded_snapshot: Option<bool>,
    #[serde(
        rename = "acceptDeferredWork",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub accept_deferred_work: Option<bool>,
    #[serde(
        rename = "acceptCompactTurnItems",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub accept_compact_turn_items: Option<bool>,
    #[serde(
        rename = "acceptDedupedBodies",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub accept_deduped_bodies: Option<bool>,
    #[serde(
        rename = "acceptCompactIds",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub accept_compact_ids: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ThreadLaunchErrorTag {
    #[serde(rename = "OrchestrationV2ThreadLaunchError")]
    OrchestrationV2ThreadLaunchError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadLaunchError {
    #[serde(rename = "_tag")]
    pub _tag: ThreadLaunchErrorTag,
    #[serde(rename = "commandId")]
    pub command_id: crate::base_schemas::CommandId,
    #[serde(rename = "projectId")]
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2StopWorkflowInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "subagentId")]
    pub subagent_id: crate::base_schemas::NodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StopWorkflowErrorTag {
    #[serde(rename = "OrchestrationV2StopWorkflowError")]
    OrchestrationV2StopWorkflowError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StopWorkflowErrorReason {
    #[serde(rename = "not-running")]
    NotRunning,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "unavailable")]
    Unavailable,
    #[serde(rename = "stop-failed")]
    StopFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2StopWorkflowError {
    #[serde(rename = "_tag")]
    pub _tag: StopWorkflowErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "subagentId")]
    pub subagent_id: crate::base_schemas::NodeId,
    #[serde(rename = "reason")]
    pub reason: StopWorkflowErrorReason,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ForceStopThreadInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub run_id: Option<crate::base_schemas::RunId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForceStopThreadResultOutcome {
    #[serde(rename = "killed")]
    Killed,
    #[serde(rename = "released")]
    Released,
    #[serde(rename = "releasing")]
    Releasing,
    #[serde(rename = "shared")]
    Shared,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ForceStopThreadResult {
    #[serde(rename = "outcome")]
    pub outcome: ForceStopThreadResultOutcome,
    #[serde(rename = "sharedWithThreadIds")]
    pub shared_with_thread_ids: Vec<crate::base_schemas::ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForceStopThreadErrorTag {
    #[serde(rename = "OrchestrationV2ForceStopThreadError")]
    OrchestrationV2ForceStopThreadError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForceStopThreadErrorReason {
    #[serde(rename = "superseded")]
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ForceStopThreadError {
    #[serde(rename = "_tag")]
    pub _tag: ForceStopThreadErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "reason",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<ForceStopThreadErrorReason>>,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetWorkflowScriptInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "scriptPath")]
    pub script_path: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetWorkflowScriptResult {
    #[serde(rename = "scriptPath")]
    pub script_path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "contents")]
    pub contents: String,
    #[serde(rename = "truncated")]
    pub truncated: bool,
}

pub type OrchestrationV2GetNativeChildWorkInput = OrchestrationV2SubscribePromptSuggestionInput;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeChildStepToolCallsItem {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "input")]
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2NativeChildStep {
    #[serde(rename = "index")]
    pub index: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(rename = "status", with = "crate::wire::nullable")]
    pub status: Option<String>,
    #[serde(rename = "text", with = "crate::wire::nullable")]
    pub text: Option<String>,
    #[serde(rename = "toolCalls")]
    pub tool_calls: Vec<NativeChildStepToolCallsItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2NativeChildWork {
    #[serde(rename = "conversationId")]
    pub conversation_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "role")]
    pub role: String,
    #[serde(rename = "prompt")]
    pub prompt: String,
    #[serde(rename = "steps", with = "crate::wire::nullable")]
    pub steps: Option<Vec<OrchestrationV2NativeChildStep>>,
    #[serde(rename = "truncated")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetNativeChildWorkResult {
    #[serde(rename = "children")]
    pub children: Vec<OrchestrationV2NativeChildWork>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetNativeChildWorkErrorTag {
    #[serde(rename = "OrchestrationV2GetNativeChildWorkError")]
    OrchestrationV2GetNativeChildWorkError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetNativeChildWorkError {
    #[serde(rename = "_tag")]
    pub _tag: GetNativeChildWorkErrorTag,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationWorkflowFileErrorTag {
    #[serde(rename = "OrchestrationWorkflowFileError")]
    OrchestrationWorkflowFileError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationWorkflowFileErrorReason {
    #[serde(rename = "invalid-path")]
    InvalidPath,
    #[serde(rename = "root-unavailable")]
    RootUnavailable,
    #[serde(rename = "not-found")]
    NotFound,
    #[serde(rename = "outside-root")]
    OutsideRoot,
    #[serde(rename = "wrong-extension")]
    WrongExtension,
    #[serde(rename = "not-regular-file")]
    NotRegularFile,
    #[serde(rename = "changed-during-read")]
    ChangedDuringRead,
    #[serde(rename = "read-failed")]
    ReadFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationWorkflowFileError {
    #[serde(rename = "_tag")]
    pub _tag: OrchestrationWorkflowFileErrorTag,
    #[serde(rename = "reason")]
    pub reason: OrchestrationWorkflowFileErrorReason,
    #[serde(rename = "path")]
    pub path: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetTurnItemInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(rename = "itemId")]
    pub item_id: crate::base_schemas::TurnItemId,
    #[serde(
        rename = "revision",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub revision: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2GetTurnItemResult {
    #[serde(rename = "item", with = "crate::wire::nullable")]
    pub item: Option<OrchestrationV2TurnItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationGetWorkflowScriptErrorTag {
    #[serde(rename = "OrchestrationGetWorkflowScriptError")]
    OrchestrationGetWorkflowScriptError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrchestrationGetWorkflowScriptErrorReason {
    #[serde(rename = "invalid-path")]
    InvalidPath,
    #[serde(rename = "root-unavailable")]
    RootUnavailable,
    #[serde(rename = "not-found")]
    NotFound,
    #[serde(rename = "outside-root")]
    OutsideRoot,
    #[serde(rename = "not-js")]
    NotJs,
    #[serde(rename = "not-regular-file")]
    NotRegularFile,
    #[serde(rename = "changed-during-read")]
    ChangedDuringRead,
    #[serde(rename = "read-failed")]
    ReadFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationGetWorkflowScriptError {
    #[serde(rename = "_tag")]
    pub _tag: OrchestrationGetWorkflowScriptErrorTag,
    #[serde(rename = "reason")]
    pub reason: OrchestrationGetWorkflowScriptErrorReason,
    #[serde(rename = "scriptPath")]
    pub script_path: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchThreadInputSkillsItem {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(
        rename = "displayName",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub display_name: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchThreadInputStart {
    #[serde(rename = "entryId")]
    pub entry_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "occurrence")]
    pub occurrence: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SearchThreadInput {
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "skills",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub skills: Option<Vec<SearchThreadInputSkillsItem>>,
    #[serde(rename = "query")]
    pub query: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "index",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub index: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(
        rename = "offset",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub offset: Option<i64>,
    #[serde(
        rename = "start",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub start: Option<SearchThreadInputStart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2ThreadFindMatch {
    #[serde(rename = "entryId")]
    pub entry_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::base_schemas::RunId>,
    #[serde(rename = "occurrence")]
    pub occurrence: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchThreadResultNavigationItem {
    #[serde(rename = "entryId")]
    pub entry_id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(rename = "runId", with = "crate::wire::nullable")]
    pub run_id: Option<crate::base_schemas::RunId>,
    #[serde(rename = "startIndex")]
    pub start_index: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "count")]
    pub count: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SearchThreadResult {
    #[serde(
        rename = "complete",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub complete: Option<bool>,
    #[serde(rename = "snapshotSequence")]
    pub snapshot_sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "totalMatches")]
    pub total_matches: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "activeIndex")]
    pub active_index: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "match", with = "crate::wire::nullable")]
    pub r#match: Option<OrchestrationV2ThreadFindMatch>,
    #[serde(
        rename = "navigation",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub navigation: Option<Vec<SearchThreadResultNavigationItem>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SearchThreadErrorTag {
    #[serde(rename = "OrchestrationV2SearchThreadError")]
    OrchestrationV2SearchThreadError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2SearchThreadError {
    #[serde(rename = "_tag")]
    pub _tag: SearchThreadErrorTag,
    #[serde(rename = "cause")]
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReplayEntryExpectOutbound {
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(rename = "frame")]
    pub frame: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReplayEntryEmitInbound {
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub label: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(rename = "frame")]
    pub frame: serde_json::Value,
    #[serde(
        rename = "afterMs",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub after_ms: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderReplayEntryRuntimeExitStatus {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReplayEntryRuntimeExit {
    #[serde(rename = "status")]
    pub status: ProviderReplayEntryRuntimeExitStatus,
    #[serde(
        rename = "error",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub error: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderReplayEntry {
    #[serde(rename = "expect_outbound")]
    ExpectOutbound(Box<ProviderReplayEntryExpectOutbound>),
    #[serde(rename = "emit_inbound")]
    EmitInbound(Box<ProviderReplayEntryEmitInbound>),
    #[serde(rename = "runtime_exit")]
    RuntimeExit(Box<ProviderReplayEntryRuntimeExit>),
}
impl crate::wire::TaggedUnion for ProviderReplayEntry {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] =
        &["expect_outbound", "emit_inbound", "runtime_exit"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderReplayNdjsonRecord {
    #[serde(rename = "transcript_start")]
    TranscriptStart(Box<ProviderReplayNdjsonRecordTranscriptStart>),
    #[serde(rename = "expect_outbound")]
    ExpectOutbound(Box<ProviderReplayEntryExpectOutbound>),
    #[serde(rename = "emit_inbound")]
    EmitInbound(Box<ProviderReplayEntryEmitInbound>),
    #[serde(rename = "runtime_exit")]
    RuntimeExit(Box<ProviderReplayEntryRuntimeExit>),
}
impl crate::wire::TaggedUnion for ProviderReplayNdjsonRecord {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "transcript_start",
        "expect_outbound",
        "emit_inbound",
        "runtime_exit",
    ];
}
