pub type TrimmedNonEmptyStringSchema = crate::base_schemas::TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitStackedAction {
    #[serde(rename = "commit")]
    Commit,
    #[serde(rename = "push")]
    Push,
    #[serde(rename = "create_pr")]
    CreatePr,
    #[serde(rename = "commit_push")]
    CommitPush,
    #[serde(rename = "commit_push_pr")]
    CommitPushPr,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionStartedEventKind {
    #[serde(rename = "action_started")]
    ActionStarted,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionProgressPhase {
    #[serde(rename = "branch")]
    Branch,
    #[serde(rename = "commit")]
    Commit,
    #[serde(rename = "push")]
    Push,
    #[serde(rename = "pr")]
    Pr,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionStartedEvent {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    pub kind: GitActionStartedEventKind,
    pub phases: Vec<GitActionProgressPhase>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionPhaseStartedEventKind {
    #[serde(rename = "phase_started")]
    PhaseStarted,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionPhaseStartedEvent {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    pub kind: GitActionPhaseStartedEventKind,
    pub phase: GitActionProgressPhase,
    pub label: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionHookStartedEventKind {
    #[serde(rename = "hook_started")]
    HookStarted,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionHookStartedEvent {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    pub kind: GitActionHookStartedEventKind,
    pub hook_name: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionHookOutputEventKind {
    #[serde(rename = "hook_output")]
    HookOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionProgressStream {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "stderr")]
    Stderr,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionHookOutputEvent {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    pub kind: GitActionHookOutputEventKind,
    #[serde(with = "crate::wire::nullable")]
    pub hook_name: Option<TrimmedNonEmptyStringSchema>,
    pub stream: GitActionProgressStream,
    pub text: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionHookFinishedEventKind {
    #[serde(rename = "hook_finished")]
    HookFinished,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionHookFinishedEvent {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    pub kind: GitActionHookFinishedEventKind,
    pub hook_name: TrimmedNonEmptyStringSchema,
    #[serde(with = "crate::wire::nullable")]
    pub exit_code: Option<i64>,
    #[serde(with = "crate::wire::nullable")]
    pub duration_ms: Option<crate::base_schemas::NonNegativeInt>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionFinishedEventKind {
    #[serde(rename = "action_finished")]
    ActionFinished,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitBranchStepStatus {
    #[serde(rename = "created")]
    Created,
    #[serde(rename = "skipped_not_requested")]
    SkippedNotRequested,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionResultBranch {
    pub status: GitBranchStepStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub name: Option<Option<TrimmedNonEmptyStringSchema>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitCommitStepStatus {
    #[serde(rename = "created")]
    Created,
    #[serde(rename = "skipped_no_changes")]
    SkippedNoChanges,
    #[serde(rename = "skipped_not_requested")]
    SkippedNotRequested,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionResultCommit {
    pub status: GitCommitStepStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub commit_sha: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub subject: Option<Option<TrimmedNonEmptyStringSchema>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitPushStepStatus {
    #[serde(rename = "pushed")]
    Pushed,
    #[serde(rename = "skipped_not_requested")]
    SkippedNotRequested,
    #[serde(rename = "skipped_up_to_date")]
    SkippedUpToDate,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionResultPush {
    pub status: GitPushStepStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub branch: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub upstream_branch: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub set_upstream: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitPrStepStatus {
    #[serde(rename = "created")]
    Created,
    #[serde(rename = "opened_existing")]
    OpenedExisting,
    #[serde(rename = "skipped_not_requested")]
    SkippedNotRequested,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionResultPr {
    pub status: GitPrStepStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub number: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_branch: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub head_branch: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<TrimmedNonEmptyStringSchema>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitRunStackedActionToastCtaMember1Kind {
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionToastCtaMember1 {
    pub kind: GitRunStackedActionToastCtaMember1Kind,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitRunStackedActionToastCtaMember2Kind {
    #[serde(rename = "open_pr")]
    OpenPr,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionToastCtaMember2 {
    pub kind: GitRunStackedActionToastCtaMember2Kind,
    pub label: TrimmedNonEmptyStringSchema,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitRunStackedActionToastCtaMember3Kind {
    #[serde(rename = "run_action")]
    RunAction,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionToastRunAction {
    pub kind: GitStackedAction,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionToastCtaMember3 {
    pub kind: GitRunStackedActionToastCtaMember3Kind,
    pub label: TrimmedNonEmptyStringSchema,
    pub action: GitRunStackedActionToastRunAction,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum GitRunStackedActionToastCta {
    None(GitRunStackedActionToastCtaMember1),
    OpenPr(GitRunStackedActionToastCtaMember2),
    RunAction(GitRunStackedActionToastCtaMember3),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionToast {
    pub title: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<TrimmedNonEmptyStringSchema>>,
    pub cta: GitRunStackedActionToastCta,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionResult {
    pub action: GitStackedAction,
    pub branch: GitRunStackedActionResultBranch,
    pub commit: GitRunStackedActionResultCommit,
    pub push: GitRunStackedActionResultPush,
    pub pr: GitRunStackedActionResultPr,
    pub toast: GitRunStackedActionToast,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionFinishedEvent {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    pub kind: GitActionFinishedEventKind,
    pub result: GitRunStackedActionResult,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionFailedEventKind {
    #[serde(rename = "action_failed")]
    ActionFailed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActionFailedEvent {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    pub kind: GitActionFailedEventKind,
    #[serde(with = "crate::wire::nullable")]
    pub phase: Option<GitActionProgressPhase>,
    pub message: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum GitActionProgressEvent {
    Started(GitActionStartedEvent),
    PhaseStarted(GitActionPhaseStartedEvent),
    HookStarted(GitActionHookStartedEvent),
    HookOutput(GitActionHookOutputEvent),
    HookFinished(GitActionHookFinishedEvent),
    Finished(Box<GitActionFinishedEvent>),
    Failed(GitActionFailedEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitActionProgressKind {
    #[serde(rename = "action_started")]
    ActionStarted,
    #[serde(rename = "phase_started")]
    PhaseStarted,
    #[serde(rename = "hook_started")]
    HookStarted,
    #[serde(rename = "hook_output")]
    HookOutput,
    #[serde(rename = "hook_finished")]
    HookFinished,
    #[serde(rename = "action_finished")]
    ActionFinished,
    #[serde(rename = "action_failed")]
    ActionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitCommandErrorTag {
    #[serde(rename = "GitCommandError")]
    GitCommandError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitCommandFailureReason {
    #[serde(rename = "authentication_failed")]
    AuthenticationFailed,
    #[serde(rename = "branch_already_exists")]
    BranchAlreadyExists,
    #[serde(rename = "branch_checked_out_in_worktree")]
    BranchCheckedOutInWorktree,
    #[serde(rename = "config_locked")]
    ConfigLocked,
    #[serde(rename = "host_key_unverified")]
    HostKeyUnverified,
    #[serde(rename = "not_a_repository")]
    NotARepository,
    #[serde(rename = "path_already_exists")]
    PathAlreadyExists,
    #[serde(rename = "push_rejected_by_remote")]
    PushRejectedByRemote,
    #[serde(rename = "push_rejected_non_fast_forward")]
    PushRejectedNonFastForward,
    #[serde(rename = "ref_name_conflict")]
    RefNameConflict,
    #[serde(rename = "ref_not_found")]
    RefNotFound,
    #[serde(rename = "remote_unreachable")]
    RemoteUnreachable,
    #[serde(rename = "tag_would_be_clobbered")]
    TagWouldBeClobbered,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommandError {
    #[serde(rename = "_tag")]
    pub _tag: GitCommandErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::vcs::EncodedNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub exit_code: Option<Option<crate::vcs::EncodedNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stdout_length: Option<Option<crate::vcs::EncodedNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stderr_length: Option<Option<crate::vcs::EncodedNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output_length: Option<Option<crate::vcs::EncodedNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::lenient"
    )]
    pub reason: Option<GitCommandFailureReason>,
    pub detail: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitManagerErrorTag {
    #[serde(rename = "GitManagerError")]
    GitManagerError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitManagerError {
    #[serde(rename = "_tag")]
    pub _tag: GitManagerErrorTag,
    pub operation: String,
    pub cwd: String,
    pub detail: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitPullRequestMaterializationErrorTag {
    #[serde(rename = "GitPullRequestMaterializationError")]
    GitPullRequestMaterializationError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPullRequestMaterializationError {
    #[serde(rename = "_tag")]
    pub _tag: GitPullRequestMaterializationErrorTag,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub pull_request_number: crate::base_schemas::PositiveInt,
    #[serde(with = "crate::wire::nullable")]
    pub head_repository: Option<TrimmedNonEmptyStringSchema>,
    pub head_branch: TrimmedNonEmptyStringSchema,
    pub local_branch: TrimmedNonEmptyStringSchema,
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TextGenerationErrorTag {
    #[serde(rename = "TextGenerationError")]
    TextGenerationError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGenerationError {
    #[serde(rename = "_tag")]
    pub _tag: TextGenerationErrorTag,
    pub operation: String,
    pub detail: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum GitManagerServiceError {
    Manager(GitManagerError),
    PullRequestMaterialization(GitPullRequestMaterializationError),
    Command(GitCommandError),
    SourceControlProvider(crate::source_control::SourceControlProviderError),
    TextGeneration(TextGenerationError),
}

pub type GitPullRequestReference = TrimmedNonEmptyStringSchema;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitPreparePullRequestThreadMode {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "worktree")]
    Worktree,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPreparePullRequestThreadInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    pub reference: GitPullRequestReference,
    pub mode: GitPreparePullRequestThreadMode,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub thread_id: Option<Option<crate::base_schemas::ThreadId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitPullRequestState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "merged")]
    Merged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitResolvedPullRequest {
    pub number: crate::base_schemas::PositiveInt,
    pub title: TrimmedNonEmptyStringSchema,
    pub url: String,
    pub base_branch: TrimmedNonEmptyStringSchema,
    pub head_branch: TrimmedNonEmptyStringSchema,
    pub state: GitPullRequestState,
}

fn git_prepare_pull_request_thread_result_is_on_pull_request_head_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPreparePullRequestThreadResult {
    pub pull_request: GitResolvedPullRequest,
    pub branch: TrimmedNonEmptyStringSchema,
    #[serde(with = "crate::wire::nullable")]
    pub worktree_path: Option<TrimmedNonEmptyStringSchema>,
    #[serde(default = "git_prepare_pull_request_thread_result_is_on_pull_request_head_default")]
    pub is_on_pull_request_head: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPullRequestRefInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    pub reference: GitPullRequestReference,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitResolvePullRequestResult {
    pub pull_request: GitResolvedPullRequest,
}

pub type GitFilePath = String;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRunStackedActionInput {
    pub action_id: TrimmedNonEmptyStringSchema,
    pub cwd: TrimmedNonEmptyStringSchema,
    pub action: GitStackedAction,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub commit_message: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub feature_branch: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub file_paths: Option<Option<Vec<GitFilePath>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub thread_id: Option<Option<crate::base_schemas::ThreadId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<crate::base_schemas::ProjectId>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsBranchStatus {
    #[serde(with = "crate::wire::nullable")]
    pub ref_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_repo: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsCreateRefInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    pub ref_name: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub switch_ref: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsCreateRefResult {
    pub ref_name: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsCreateWorktreeInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    pub ref_name: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub new_ref_name: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_ref_name: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(with = "crate::wire::nullable")]
    pub path: Option<TrimmedNonEmptyStringSchema>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsWorktree {
    pub path: TrimmedNonEmptyStringSchema,
    pub ref_name: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsCreateWorktreeResult {
    pub worktree: VcsWorktree,
}

/// `vcs.trustRepository` (Rust only): the folder is named by the project or thread that owns it,
/// never by a path; exactly one of the two.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsTrustRepositoryInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::base_schemas::ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<crate::base_schemas::ThreadId>,
}

/// The repository now trusted for the folder, for the user to see what they trusted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsTrustRepositoryResult {
    /// The repository's top-level folder.
    pub top_level: String,
    /// A linked worktree of the project's own repository.
    pub worktree_of_project: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsInitInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub kind: Option<Option<crate::vcs::VcsDriverKind>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsListRefsInputRefKind {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "remote")]
    Remote,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsListRefsInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub query: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cursor: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub include_matching_remote_refs: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ref_kind: Option<Option<VcsListRefsInputRefKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub refresh: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub limit: Option<Option<crate::base_schemas::PositiveInt>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsRef {
    pub name: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_remote: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub remote_name: Option<Option<TrimmedNonEmptyStringSchema>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub upstream_ref: Option<Option<TrimmedNonEmptyStringSchema>>,
    pub current: bool,
    pub is_default: bool,
    #[serde(with = "crate::wire::nullable")]
    pub worktree_path: Option<TrimmedNonEmptyStringSchema>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsListRefsResult {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_at_repository_root: Option<Option<bool>>,
    pub refs: Vec<VcsRef>,
    pub is_repo: bool,
    pub has_primary_remote: bool,
    #[serde(with = "crate::wire::nullable")]
    pub next_cursor: Option<crate::base_schemas::NonNegativeInt>,
    pub total_count: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsPullInput {
    pub cwd: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsPullResultStatus {
    #[serde(rename = "pulled")]
    Pulled,
    #[serde(rename = "skipped_up_to_date")]
    SkippedUpToDate,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsPullResult {
    pub status: VcsPullResultStatus,
    pub ref_name: TrimmedNonEmptyStringSchema,
    #[serde(with = "crate::wire::nullable")]
    pub upstream_ref: Option<TrimmedNonEmptyStringSchema>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsRemoveWorktreeInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    pub path: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub force: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusInput {
    pub cwd: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusLocalResultWorkingTreeFiles {
    pub path: GitFilePath,
    pub insertions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusLocalResultWorkingTree {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub root_path: Option<Option<GitFilePath>>,
    pub files: Vec<VcsStatusLocalResultWorkingTreeFiles>,
    pub insertions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusLocalResultBranchChanges {
    #[serde(with = "crate::wire::nullable")]
    pub base_ref: Option<TrimmedNonEmptyStringSchema>,
    pub insertions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusLocalResult {
    pub is_repo: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_control_provider: Option<Option<crate::source_control::SourceControlProviderInfo>>,
    pub has_primary_remote: bool,
    pub is_default_ref: bool,
    #[serde(with = "crate::wire::nullable")]
    pub ref_name: Option<TrimmedNonEmptyStringSchema>,
    pub has_working_tree_changes: bool,
    pub working_tree: VcsStatusLocalResultWorkingTree,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub branch_changes: Option<Option<VcsStatusLocalResultBranchChanges>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsStatusChangeRequestState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "merged")]
    Merged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusChangeRequest {
    pub number: crate::base_schemas::PositiveInt,
    pub title: TrimmedNonEmptyStringSchema,
    pub url: String,
    pub base_ref: TrimmedNonEmptyStringSchema,
    pub head_ref: TrimmedNonEmptyStringSchema,
    pub state: VcsStatusChangeRequestState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_draft: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub updated_at: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusRemoteResult {
    pub has_upstream: bool,
    pub ahead_count: crate::base_schemas::NonNegativeInt,
    pub behind_count: crate::base_schemas::NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ahead_of_default_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(with = "crate::wire::nullable")]
    pub pr: Option<VcsStatusChangeRequest>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusResultWorkingTreeFiles {
    pub path: GitFilePath,
    pub insertions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusResultWorkingTree {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub root_path: Option<Option<GitFilePath>>,
    pub files: Vec<VcsStatusResultWorkingTreeFiles>,
    pub insertions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusResultBranchChanges {
    #[serde(with = "crate::wire::nullable")]
    pub base_ref: Option<TrimmedNonEmptyStringSchema>,
    pub insertions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusResult {
    pub is_repo: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_control_provider: Option<Option<crate::source_control::SourceControlProviderInfo>>,
    pub has_primary_remote: bool,
    pub is_default_ref: bool,
    #[serde(with = "crate::wire::nullable")]
    pub ref_name: Option<TrimmedNonEmptyStringSchema>,
    pub has_working_tree_changes: bool,
    pub working_tree: VcsStatusResultWorkingTree,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub branch_changes: Option<Option<VcsStatusResultBranchChanges>>,
    pub has_upstream: bool,
    pub ahead_count: crate::base_schemas::NonNegativeInt,
    pub behind_count: crate::base_schemas::NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ahead_of_default_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(with = "crate::wire::nullable")]
    pub pr: Option<VcsStatusChangeRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsStatusStreamEventMember1Tag {
    #[serde(rename = "snapshot")]
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusStreamEventMember1 {
    #[serde(rename = "_tag")]
    pub _tag: VcsStatusStreamEventMember1Tag,
    pub local: VcsStatusLocalResult,
    #[serde(with = "crate::wire::nullable")]
    pub remote: Option<VcsStatusRemoteResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsStatusStreamEventMember2Tag {
    #[serde(rename = "localUpdated")]
    LocalUpdated,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusStreamEventMember2 {
    #[serde(rename = "_tag")]
    pub _tag: VcsStatusStreamEventMember2Tag,
    pub local: VcsStatusLocalResult,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsStatusStreamEventMember3Tag {
    #[serde(rename = "remoteUpdated")]
    RemoteUpdated,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusStreamEventMember3 {
    #[serde(rename = "_tag")]
    pub _tag: VcsStatusStreamEventMember3Tag,
    #[serde(with = "crate::wire::nullable")]
    pub remote: Option<VcsStatusRemoteResult>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum VcsStatusStreamEvent {
    Snapshot(VcsStatusStreamEventMember1),
    LocalUpdated(VcsStatusStreamEventMember2),
    RemoteUpdated(VcsStatusStreamEventMember3),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsStatusSubscriptionInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub include_remote: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsSwitchRefInput {
    pub cwd: TrimmedNonEmptyStringSchema,
    pub ref_name: TrimmedNonEmptyStringSchema,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsSwitchRefResult {
    #[serde(with = "crate::wire::nullable")]
    pub ref_name: Option<TrimmedNonEmptyStringSchema>,
}
