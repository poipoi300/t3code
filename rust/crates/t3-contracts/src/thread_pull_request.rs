#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLinkedPullRequest {
    pub project_id: crate::base_schemas::ProjectId,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestKey {
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThreadPullRequestLinkSource {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "created")]
    Created,
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "stack")]
    Stack,
    #[serde(rename = "stack-dismissed")]
    StackDismissed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestSnapshot {
    pub state: crate::pull_request::PullRequestState,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub head_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub base_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub is_draft: bool,
    #[serde(with = "crate::wire::nullable")]
    pub updated_at: Option<crate::base_schemas::IsoDateTime>,
    pub synced_at: crate::base_schemas::IsoDateTime,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub author: Option<Option<crate::pull_request::PullRequestActor>>,
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
    pub review_decision: Option<Option<crate::pull_request::PullRequestReviewDecision>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub checks_state: Option<Option<crate::pull_request::PullRequestChecksState>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub mergeability: Option<Option<crate::pull_request::PullRequestMergeability>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThreadPullRequestStackKind {
    #[serde(rename = "native")]
    Native,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestStackLayer {
    pub number: crate::base_schemas::PositiveInt,
    pub head_branch: crate::base_schemas::TrimmedNonEmptyString,
    pub state: crate::pull_request::PullRequestState,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestStack {
    pub kind: ThreadPullRequestStackKind,
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    pub base: crate::base_schemas::TrimmedNonEmptyString,
    pub layers: Vec<ThreadPullRequestStackLayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThreadPullRequestWatchAwaitingBaseline {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "checks")]
    Checks,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestWatch {
    pub started_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub head_sha: Option<crate::base_schemas::TrimmedNonEmptyString>,
    pub failed_checks: Vec<crate::base_schemas::TrimmedNonEmptyString>,
    pub passed: bool,
    #[serde(default)]
    pub passed_checks: Vec<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub owed_checks: Option<Option<Vec<crate::base_schemas::TrimmedNonEmptyString>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub awaiting_approval: Option<Option<Vec<crate::base_schemas::TrimmedNonEmptyString>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub approval_announced: Option<Option<bool>>,
    pub remarks_through: crate::base_schemas::IsoDateTime,
    pub remark_ids: Vec<crate::base_schemas::TrimmedNonEmptyString>,
    pub conflicting: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub merge_queue_removal_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub wakes: crate::base_schemas::NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub awaiting_baseline: Option<Option<ThreadPullRequestWatchAwaitingBaseline>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub checks_awaited_since: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checks_pending: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestLink {
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub number: crate::base_schemas::PositiveInt,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub repository_web_url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub source: ThreadPullRequestLinkSource,
    pub linked_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub snapshot: Option<ThreadPullRequestSnapshot>,
    #[serde(with = "crate::wire::nullable")]
    pub stack: Option<ThreadPullRequestStack>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub watch: Option<Option<ThreadPullRequestWatch>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ended_watch: Option<Option<ThreadPullRequestWatch>>,
}
