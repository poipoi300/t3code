#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestAction {
    #[serde(rename = "merge")]
    Merge,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "draft")]
    Draft,
    #[serde(rename = "close")]
    Close,
    #[serde(rename = "reopen")]
    Reopen,
    #[serde(rename = "delete-branch")]
    DeleteBranch,
    #[serde(rename = "update-branch")]
    UpdateBranch,
    #[serde(rename = "enable-auto-merge")]
    EnableAutoMerge,
    #[serde(rename = "disable-auto-merge")]
    DisableAutoMerge,
    #[serde(rename = "revert")]
    Revert,
    #[serde(rename = "approve-workflows")]
    ApproveWorkflows,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStackHead {
    pub number: crate::base_schemas::PositiveInt,
    pub head_sha: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestMergeMethod {
    #[serde(rename = "merge")]
    Merge,
    #[serde(rename = "squash")]
    Squash,
    #[serde(rename = "rebase")]
    Rebase,
    #[serde(rename = "fast-forward")]
    FastForward,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestUpdateMethod {
    #[serde(rename = "merge")]
    Merge,
    #[serde(rename = "rebase")]
    Rebase,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "bool", into = "bool")]
pub struct PullRequestActionInputReplyWithSummary;
impl TryFrom<bool> for PullRequestActionInputReplyWithSummary {
    type Error = &'static str;
    fn try_from(value: bool) -> Result<Self, Self::Error> {
        if value {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<PullRequestActionInputReplyWithSummary> for bool {
    fn from(_: PullRequestActionInputReplyWithSummary) -> Self {
        true
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestActionInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stack_number: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_stack_heads: Option<Option<Vec<PullRequestStackHead>>>,
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub action: PullRequestAction,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub merge_method: Option<Option<PullRequestMergeMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub update_method: Option<Option<PullRequestUpdateMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reply_with_summary: Option<Option<PullRequestActionInputReplyWithSummary>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestActor {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_bot: Option<Option<bool>>,
    pub login: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub name: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestCommentKind {
    #[serde(rename = "issue-comment")]
    IssueComment,
    #[serde(rename = "review-comment")]
    ReviewComment,
    #[serde(rename = "review")]
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReactionContent {
    #[serde(rename = "thumbs-up")]
    ThumbsUp,
    #[serde(rename = "thumbs-down")]
    ThumbsDown,
    #[serde(rename = "laugh")]
    Laugh,
    #[serde(rename = "hooray")]
    Hooray,
    #[serde(rename = "confused")]
    Confused,
    #[serde(rename = "heart")]
    Heart,
    #[serde(rename = "rocket")]
    Rocket,
    #[serde(rename = "eyes")]
    Eyes,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReaction {
    pub content: PullRequestReactionContent,
    pub count: crate::base_schemas::PositiveInt,
    pub actors: Vec<crate::base_schemas::TrimmedNonEmptyString>,
    pub viewer_has_reacted: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestComment {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: PullRequestCommentKind,
    #[serde(with = "crate::wire::nullable")]
    pub author: Option<PullRequestActor>,
    pub body: String,
    pub created_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub edited_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(with = "crate::wire::nullable")]
    pub url: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub path: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub review_state: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reactions: Option<Option<Vec<PullRequestReaction>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub can_edit: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestDiffSide {
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestThreadComment {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub author: Option<PullRequestActor>,
    pub body: String,
    pub created_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub edited_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(with = "crate::wire::nullable")]
    pub url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub review_url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reactions: Option<Option<Vec<PullRequestReaction>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub can_edit: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewThread {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub line: Option<crate::base_schemas::PositiveInt>,
    pub side: PullRequestDiffSide,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub comments: Vec<PullRequestThreadComment>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub comment_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub next_comments_cursor: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub can_resolve: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestOverviewThread {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub path: (),
    pub line: (),
    pub side: PullRequestDiffSide,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub comments: Vec<PullRequestThreadComment>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub comment_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub next_comments_cursor: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub can_resolve: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCommit {
    pub oid: crate::base_schemas::TrimmedNonEmptyString,
    pub message_headline: String,
    pub committed_date: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub additions: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub deletions: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub authors: Option<Option<Vec<PullRequestActor>>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestActivity {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub author: Option<Option<PullRequestActor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reviewers: Option<Option<Vec<PullRequestActor>>>,
    pub comments: Vec<PullRequestComment>,
    pub comment_count: crate::base_schemas::NonNegativeInt,
    pub comments_truncated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub review_threads_truncated: Option<Option<bool>>,
    pub review_threads: Vec<PullRequestReviewThread>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub overview_threads: Option<Option<Vec<PullRequestOverviewThread>>>,
    pub commits: Vec<PullRequestCommit>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reactions: Option<Option<Vec<PullRequestReaction>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestBaseComparison {
    #[serde(rename = "up-to-date")]
    UpToDate,
    #[serde(rename = "behind")]
    Behind,
    #[serde(rename = "unknown")]
    Unknown,
}

fn pull_request_capabilities_update_methods_array<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Vec<PullRequestUpdateMethod>>>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct Items(
        #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
        Vec<PullRequestUpdateMethod>,
    );
    Ok(Some(
        <Option<Items> as serde::Deserialize>::deserialize(deserializer)?.map(|items| items.0),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestViewedFilesStore {
    #[serde(rename = "host")]
    Host,
    #[serde(rename = "environment")]
    Environment,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewVerdict {
    #[serde(rename = "comment")]
    Comment,
    #[serde(rename = "approve")]
    Approve,
    #[serde(rename = "request-changes")]
    RequestChanges,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewCapabilities {
    pub inline_comment: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub range_comment: Option<Option<bool>>,
    pub reply: bool,
    pub resolve: bool,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub verdicts: Vec<PullRequestReviewVerdict>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewerCapabilities {
    pub request: bool,
    pub list_candidates: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub search_candidates: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestEditCapabilities {
    pub change_request: bool,
    pub comment: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub maintainer_edits: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReferenceSigil {
    #[serde(rename = "#")]
    Hash,
    #[serde(rename = "!")]
    Bang,
}

fn pull_request_capabilities_reference_sigils_array<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Vec<PullRequestReferenceSigil>>>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct Items(
        #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
        Vec<PullRequestReferenceSigil>,
    );
    Ok(Some(
        <Option<Items> as serde::Deserialize>::deserialize(deserializer)?.map(|items| items.0),
    ))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCapabilities {
    pub diff: bool,
    pub comment: bool,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub actions: Vec<PullRequestAction>,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub merge_methods: Vec<PullRequestMergeMethod>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub delete_branch: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "pull_request_capabilities_update_methods_array"
    )]
    pub update_methods: Option<Option<Vec<PullRequestUpdateMethod>>>,
    pub search: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reactions: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewed_files: Option<Option<PullRequestViewedFilesStore>>,
    pub review: PullRequestReviewCapabilities,
    pub reviewers: PullRequestReviewerCapabilities,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub edit: Option<Option<PullRequestEditCapabilities>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stacks: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stack_actions: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub labels: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub mentions: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "pull_request_capabilities_reference_sigils_array"
    )]
    pub reference_sigils: Option<Option<Vec<PullRequestReferenceSigil>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checks_always_report: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLineCount {
    pub additions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

fn pull_request_change_breakdown_docs_default() -> PullRequestLineCount {
    PullRequestLineCount {
        additions: 0,
        deletions: 0,
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestChangeBreakdownCounts {
    pub code: PullRequestLineCount,
    pub tests: PullRequestLineCount,
    pub mocks: PullRequestLineCount,
    pub generated: PullRequestLineCount,
    #[serde(default = "pull_request_change_breakdown_docs_default")]
    pub docs: PullRequestLineCount,
}

pub type PullRequestChangeBreakdown = Option<PullRequestChangeBreakdownCounts>;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestCheckStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "action-required")]
    ActionRequired,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "failure")]
    Failure,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "neutral")]
    Neutral,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCheck {
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    pub status: PullRequestCheckStatus,
    #[serde(with = "crate::wire::nullable")]
    pub description: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub required: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub placeholder: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "merged")]
    Merged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestChecks {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub observed_at: Option<Option<crate::wire::JsNumber>>,
    pub state: PullRequestState,
    pub checks: Vec<PullRequestCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestChecksState {
    #[serde(rename = "passing")]
    Passing,
    #[serde(rename = "failing")]
    Failing,
    #[serde(rename = "pending")]
    Pending,
}

pub type CommentBody = String;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCommentInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub body: CommentBody,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestCommentUpdateInputKind {
    #[serde(rename = "issue-comment")]
    IssueComment,
    #[serde(rename = "review-comment")]
    ReviewComment,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCommentUpdateInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub comment_id: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: PullRequestCommentUpdateInputKind,
    pub body: CommentBody,
}

fn pull_request_viewer_permissions_update_methods_array<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Vec<PullRequestUpdateMethod>>>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct Items(
        #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
        Vec<PullRequestUpdateMethod>,
    );
    Ok(Some(
        <Option<Items> as serde::Deserialize>::deserialize(deserializer)?.map(|items| items.0),
    ))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestViewerPermissions {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stack_rebase: Option<Option<bool>>,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub actions: Vec<PullRequestAction>,
    pub comment: bool,
    pub resolve: bool,
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub verdicts: Vec<PullRequestReviewVerdict>,
    pub request_reviewers: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "pull_request_viewer_permissions_update_methods_array"
    )]
    pub update_methods: Option<Option<Vec<PullRequestUpdateMethod>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub labels: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub edit_change_request: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub maintainer_edits: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestMergeability {
    #[serde(rename = "mergeable")]
    Mergeable,
    #[serde(rename = "conflicting")]
    Conflicting,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLabel {
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub color: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub description: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMergeCapabilities {
    pub merge: bool,
    pub squash: bool,
    pub rebase: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    #[serde(rename = "fast-forward")]
    pub fast_forward: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMergeQueueRemoval {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub removed_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetail {
    pub provider: crate::source_control::SourceControlProviderKind,
    pub capabilities: PullRequestCapabilities,
    pub viewer_permissions: PullRequestViewerPermissions,
    pub project_id: crate::base_schemas::ProjectId,
    pub project_title: crate::base_schemas::TrimmedNonEmptyString,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub body: String,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub author: Option<PullRequestActor>,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub mergeability: PullRequestMergeability,
    pub additions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
    pub changed_files: crate::base_schemas::NonNegativeInt,
    pub head_branch: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub head_repository_name_with_owner: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub head_sha: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub base_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub created_at: crate::base_schemas::IsoDateTime,
    pub updated_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub observed_at: Option<Option<crate::wire::JsNumber>>,
    #[serde(with = "crate::wire::nullable")]
    pub merged_at: Option<crate::base_schemas::IsoDateTime>,
    #[serde(with = "crate::wire::nullable")]
    pub closed_at: Option<crate::base_schemas::IsoDateTime>,
    pub reviewers: Vec<PullRequestActor>,
    pub labels: Vec<PullRequestLabel>,
    pub checks: Vec<PullRequestCheck>,
    pub merge_capabilities: PullRequestMergeCapabilities,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewer: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_comparison: Option<Option<PullRequestBaseComparison>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub behind_by: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_merge_enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_merge_method: Option<Option<PullRequestMergeMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workflow_approvals_required: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub merge_queue_removal: Option<Option<PullRequestMergeQueueRemoval>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub maintainer_can_modify: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetailView {
    pub provider: crate::source_control::SourceControlProviderKind,
    pub capabilities: PullRequestCapabilities,
    pub viewer_permissions: PullRequestViewerPermissions,
    pub project_id: crate::base_schemas::ProjectId,
    pub project_title: crate::base_schemas::TrimmedNonEmptyString,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub body: String,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub author: Option<PullRequestActor>,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub mergeability: PullRequestMergeability,
    pub additions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
    pub changed_files: crate::base_schemas::NonNegativeInt,
    pub head_branch: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub head_repository_name_with_owner: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub head_sha: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub base_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub created_at: crate::base_schemas::IsoDateTime,
    pub updated_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub observed_at: Option<Option<crate::wire::JsNumber>>,
    #[serde(with = "crate::wire::nullable")]
    pub merged_at: Option<crate::base_schemas::IsoDateTime>,
    #[serde(with = "crate::wire::nullable")]
    pub closed_at: Option<crate::base_schemas::IsoDateTime>,
    pub reviewers: Vec<PullRequestActor>,
    pub labels: Vec<PullRequestLabel>,
    pub checks: Vec<PullRequestCheck>,
    pub merge_capabilities: PullRequestMergeCapabilities,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub viewer: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_comparison: Option<Option<PullRequestBaseComparison>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub behind_by: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_merge_enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_merge_method: Option<Option<PullRequestMergeMethod>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workflow_approvals_required: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub merge_queue_removal: Option<Option<PullRequestMergeQueueRemoval>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub maintainer_can_modify: Option<Option<bool>>,
    pub comments: Vec<PullRequestComment>,
    pub comment_count: crate::base_schemas::NonNegativeInt,
    pub comments_truncated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub review_threads_truncated: Option<Option<bool>>,
    pub review_threads: Vec<PullRequestReviewThread>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub overview_threads: Option<Option<Vec<PullRequestOverviewThread>>>,
    pub commits: Vec<PullRequestCommit>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reactions: Option<Option<Vec<PullRequestReaction>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestDiffFileContentsInputChangeType {
    #[serde(rename = "change")]
    Change,
    #[serde(rename = "rename-pure")]
    RenamePure,
    #[serde(rename = "rename-changed")]
    RenameChanged,
    #[serde(rename = "new")]
    New,
    #[serde(rename = "deleted")]
    Deleted,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDiffFileContentsInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub commit: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub change_type: PullRequestDiffFileContentsInputChangeType,
    pub old_path: crate::base_schemas::TrimmedNonEmptyString,
    pub new_path: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDiffFileContentsResult {
    pub old_contents: String,
    pub new_contents: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDiffInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cursor: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub commit: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestOmittedFileStat {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    pub additions: crate::vcs::EncodedNumber,
    pub deletions: crate::vcs::EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDiffResult {
    pub patch: String,
    pub truncated: bool,
    #[serde(with = "crate::wire::nullable")]
    pub next_cursor: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub omitted_file_stats: Option<Option<Vec<PullRequestOmittedFileStat>>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDiffStat {
    pub project_id: crate::base_schemas::ProjectId,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub additions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
}

pub type FilePath = String;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestFileViewedState {
    #[serde(rename = "unviewed")]
    Unviewed,
    #[serde(rename = "viewed")]
    Viewed,
    #[serde(rename = "dismissed")]
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFileViewed {
    pub path: FilePath,
    pub state: PullRequestFileViewedState,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFilesViewedResult {
    pub files: Vec<PullRequestFileViewed>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestFilterOptionKind {
    #[serde(rename = "labels")]
    Labels,
    #[serde(rename = "authors")]
    Authors,
    #[serde(rename = "references")]
    References,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReferenceCandidateKind {
    #[serde(rename = "issue")]
    Issue,
    #[serde(rename = "change-request")]
    ChangeRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReferenceCandidateState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "merged")]
    Merged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReferenceCandidate {
    pub reference: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: PullRequestReferenceCandidateKind,
    pub number: crate::base_schemas::PositiveInt,
    pub title: String,
    pub state: PullRequestReferenceCandidateState,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFilterOptions {
    pub labels: Vec<PullRequestLabel>,
    pub authors: Vec<PullRequestActor>,
    #[serde(default)]
    pub references: Vec<PullRequestReferenceCandidate>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestFilterOptionsInput {
    pub kind: PullRequestFilterOptionKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<crate::base_schemas::ProjectId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_ids: Option<Option<Vec<crate::base_schemas::ProjectId>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub query: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub sigil: Option<Option<PullRequestReferenceSigil>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRef {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestInvalidateInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reference: Option<Option<PullRequestRef>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub files_viewed_only: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestInvolvement {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "reviewing")]
    Reviewing,
    #[serde(rename = "authored")]
    Authored,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLabelCandidate {
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub color: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub description: Option<String>,
    pub is_applied: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLabelCandidateList {
    pub candidates: Vec<PullRequestLabelCandidate>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLabelChangeInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub labels: Vec<crate::base_schemas::TrimmedNonEmptyString>,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLinkedThreadsResultThreads {
    pub id: crate::base_schemas::ThreadId,
    pub project_id: crate::base_schemas::ProjectId,
    pub title: String,
    #[serde(with = "crate::wire::nullable")]
    pub archived_at: Option<crate::base_schemas::IsoDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub settled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestLinkedThreadsResult {
    pub threads: Vec<PullRequestLinkedThreadsResultThreads>,
}

pub type PullRequestListCursors = std::collections::BTreeMap<
    crate::base_schemas::TrimmedNonEmptyString,
    crate::base_schemas::TrimmedNonEmptyString,
>;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStackMembership {
    pub number: crate::base_schemas::PositiveInt,
    pub position: crate::base_schemas::PositiveInt,
    pub size: crate::base_schemas::PositiveInt,
    pub base: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewDecision {
    #[serde(rename = "approved")]
    Approved,
    #[serde(rename = "changes-requested")]
    ChangesRequested,
    #[serde(rename = "review-required")]
    ReviewRequired,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListEntry {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stack: Option<Option<PullRequestStackMembership>>,
    pub provider: crate::source_control::SourceControlProviderKind,
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub project_id: crate::base_schemas::ProjectId,
    pub project_title: crate::base_schemas::TrimmedNonEmptyString,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub author: Option<PullRequestActor>,
    pub head_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub base_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub mergeability: PullRequestMergeability,
    pub additions: crate::base_schemas::NonNegativeInt,
    pub deletions: crate::base_schemas::NonNegativeInt,
    pub created_at: crate::base_schemas::IsoDateTime,
    pub updated_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub observed_at: Option<Option<crate::wire::JsNumber>>,
    pub viewer_review_requested: bool,
    pub labels: Vec<PullRequestLabel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub review_decision: Option<Option<PullRequestReviewDecision>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checks_state: Option<Option<PullRequestChecksState>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestListFiltersDraft {
    #[serde(rename = "only")]
    Only,
    #[serde(rename = "hide")]
    Hide,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestListFiltersReview {
    #[serde(rename = "approved")]
    Approved,
    #[serde(rename = "changes-requested")]
    ChangesRequested,
    #[serde(rename = "review-required")]
    ReviewRequired,
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestListFiltersChecks {
    #[serde(rename = "passing")]
    Passing,
    #[serde(rename = "failing")]
    Failing,
}

pub type PullRequestQualifierValue = crate::base_schemas::TrimmedNonEmptyString;

pub type PullRequestQualifierValues = Vec<PullRequestQualifierValue>;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListFilters {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub draft: Option<Option<PullRequestListFiltersDraft>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub review: Option<Option<PullRequestListFiltersReview>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checks: Option<Option<PullRequestListFiltersChecks>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub labels: Option<Option<Vec<PullRequestQualifierValues>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub excluded_labels: Option<Option<PullRequestQualifierValues>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub author: Option<Option<PullRequestQualifierValue>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestListState {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "merged")]
    Merged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListInput {
    pub state: PullRequestListState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub involvement: Option<Option<PullRequestInvolvement>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub filters: Option<Option<PullRequestListFilters>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<crate::base_schemas::ProjectId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_ids: Option<Option<Vec<crate::base_schemas::ProjectId>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub limit: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cursors: Option<Option<PullRequestListCursors>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub query: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListProjectError {
    pub project_id: crate::base_schemas::ProjectId,
    pub project_title: crate::base_schemas::TrimmedNonEmptyString,
    pub message: crate::base_schemas::TrimmedNonEmptyString,
}

fn pull_request_provider_summary_actions_array<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Vec<PullRequestAction>>>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct Items(
        #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
        Vec<PullRequestAction>,
    );
    Ok(Some(
        <Option<Items> as serde::Deserialize>::deserialize(deserializer)?.map(|items| items.0),
    ))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestProviderSummary {
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: crate::source_control::SourceControlProviderKind,
    pub searches_on_host: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "pull_request_provider_summary_actions_array"
    )]
    pub actions: Option<Option<Vec<PullRequestAction>>>,
    pub project_count: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_ids: Option<Option<Vec<crate::base_schemas::ProjectId>>>,
    pub configured: bool,
    #[serde(with = "crate::wire::nullable")]
    pub detail: Option<crate::base_schemas::TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListResult {
    pub viewers: std::collections::BTreeMap<
        crate::base_schemas::TrimmedNonEmptyString,
        crate::base_schemas::TrimmedNonEmptyString,
    >,
    pub providers: Vec<PullRequestProviderSummary>,
    pub entries: Vec<PullRequestListEntry>,
    pub errors: Vec<PullRequestListProjectError>,
    pub truncated: bool,
    pub next_cursors: PullRequestListCursors,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListStatsInput {
    pub refs: Vec<PullRequestRef>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListStatsResult {
    pub stats: Vec<PullRequestDiffStat>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestOperationErrorTag {
    #[serde(rename = "PullRequestOperationError")]
    PullRequestOperationError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestOperationErrorReason {
    #[serde(rename = "not-found")]
    NotFound,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestOperationError {
    #[serde(rename = "_tag")]
    pub _tag: PullRequestOperationErrorTag,
    pub operation: String,
    pub detail: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reason: Option<Option<PullRequestOperationErrorReason>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub list_result: Option<Option<PullRequestListResult>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestPreview {
    pub project_id: crate::base_schemas::ProjectId,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub author: Option<PullRequestActor>,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub created_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub updated_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub observed_at: Option<Option<crate::wire::JsNumber>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReactionInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub subject_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub content: PullRequestReactionContent,
    pub reacted: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReportStateInput {
    pub reference: PullRequestRef,
    pub state: PullRequestState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_draft: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewPositionMember1Kind {
    #[serde(rename = "added")]
    Added,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewPositionMember1 {
    pub kind: PullRequestReviewPositionMember1Kind,
    pub new_line: crate::base_schemas::PositiveInt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewPositionMember2Kind {
    #[serde(rename = "deleted")]
    Deleted,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewPositionMember2 {
    pub kind: PullRequestReviewPositionMember2Kind,
    pub old_line: crate::base_schemas::PositiveInt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewPositionMember3Kind {
    #[serde(rename = "context")]
    Context,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewPositionMember3 {
    pub kind: PullRequestReviewPositionMember3Kind,
    pub old_line: crate::base_schemas::PositiveInt,
    pub new_line: crate::base_schemas::PositiveInt,
    pub side: PullRequestDiffSide,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PullRequestReviewPosition {
    Added(PullRequestReviewPositionMember1),
    Deleted(PullRequestReviewPositionMember2),
    Context(PullRequestReviewPositionMember3),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewCommentDraft {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub old_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub position: PullRequestReviewPosition,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub start_position: Option<Option<PullRequestReviewPosition>>,
    pub body: CommentBody,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewerKind {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "team")]
    Team,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewerCandidate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_bot: Option<Option<bool>>,
    pub login: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub name: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub avatar_url: Option<String>,
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: PullRequestReviewerKind,
    pub is_requested: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewerCandidateList {
    pub candidates: Vec<PullRequestReviewerCandidate>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewerCandidatesInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub query: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewerRequestInputReviewers {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: PullRequestReviewerKind,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewerRequestInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub reviewers: Vec<PullRequestReviewerRequestInputReviewers>,
    pub requested: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRoutingIdentityInput {
    pub host: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRoutingIdentityResult {
    pub account_id: crate::base_schemas::TrimmedNonEmptyString,
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub provider: crate::source_control::SourceControlProviderKind,
    pub viewer: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRoutingResult {
    pub account_id: crate::base_schemas::TrimmedNonEmptyString,
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub provider: crate::source_control::SourceControlProviderKind,
    pub viewer: crate::base_schemas::TrimmedNonEmptyString,
    pub project_title: crate::base_schemas::TrimmedNonEmptyString,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSetFilesViewedInputFiles {
    pub path: FilePath,
    pub viewed: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub revision: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSetFilesViewedInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub files: Vec<PullRequestSetFilesViewedInputFiles>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStackLayers {
    pub number: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_draft: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub head_sha: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub head_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub state: PullRequestState,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStack {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    pub base: crate::base_schemas::TrimmedNonEmptyString,
    pub layers: Vec<PullRequestStackLayers>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSubmitReviewInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub verdict: PullRequestReviewVerdict,
    pub body: String,
    pub comments: Vec<PullRequestReviewCommentDraft>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSummary {
    pub provider: crate::source_control::SourceControlProviderKind,
    pub project_id: crate::base_schemas::ProjectId,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    pub state: PullRequestState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_draft: Option<Option<bool>>,
    pub head_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub base_branch: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub closed_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub merged_at: Option<Option<String>>,
    pub updated_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub observed_at: Option<Option<crate::wire::JsNumber>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub author: Option<Option<PullRequestActor>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub additions: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub deletions: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub changed_files: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub review_decision: Option<Option<PullRequestReviewDecision>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub checks_state: Option<Option<PullRequestChecksState>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub mergeability: Option<Option<PullRequestMergeability>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub stack: Option<Option<PullRequestStackMembership>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub head_sha: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_merge_enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_merge_method: Option<Option<PullRequestMergeMethod>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestThreadCommentsInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub thread_id: crate::base_schemas::TrimmedNonEmptyString,
    pub cursor: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestThreadCommentsResult {
    pub comments: Vec<PullRequestThreadComment>,
    #[serde(with = "crate::wire::nullable")]
    pub next_cursor: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub hidden_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestThreadReplyInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub thread_id: crate::base_schemas::TrimmedNonEmptyString,
    pub body: CommentBody,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestThreadResolutionInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub thread_id: crate::base_schemas::TrimmedNonEmptyString,
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestUnavailableErrorTag {
    #[serde(rename = "PullRequestUnavailableError")]
    PullRequestUnavailableError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestUnavailableReason {
    #[serde(rename = "cli-missing")]
    CliMissing,
    #[serde(rename = "cli-unauthenticated")]
    CliUnauthenticated,
    #[serde(rename = "provider-unsupported")]
    ProviderUnsupported,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestUnavailableError {
    #[serde(rename = "_tag")]
    pub _tag: PullRequestUnavailableErrorTag,
    pub reason: PullRequestUnavailableReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider: Option<Option<crate::source_control::SourceControlProviderKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub list_result: Option<Option<PullRequestListResult>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub requirement: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestUpdateInput {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub host: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_account_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_stale: Option<Option<bool>>,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub body: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub maintainer_can_modify: Option<Option<bool>>,
}

pub const PULL_REQUEST_LIST_PROJECT_LIMIT: usize = 100;
