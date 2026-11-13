use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use t3_contracts::git::GitPullRequestReference;
pub use t3_contracts::environment::RepositoryIdentity;
pub use t3_contracts::pull_request::*;
pub use t3_contracts::source_control::{
    ChangeRequest, ChangeRequestState, SourceControlProviderInfo, SourceControlProviderKind,
    SourceControlRepositoryCloneUrls, SourceControlRepositoryVisibility,
};
pub use t3_contracts::vcs::{VcsRemote, VcsRepositoryIdentity};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestWatchFingerprint {
    pub status: String,
    pub remarks: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<PullRequestStackMembership>,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: Option<PullRequestActor>,
    pub head_branch: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub head_repository_name_with_owner: Option<Option<String>>,
    pub base_branch: String,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub mergeability: PullRequestMergeability,
    pub additions: u64,
    pub deletions: u64,
    pub created_at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub closed_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub merged_at: Option<Option<String>>,
    pub updated_at: String,
    pub review_request_logins: Vec<String>,
    pub labels: Vec<PullRequestLabel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub review_decision: Option<Option<PullRequestReviewDecision>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub checks_state: Option<Option<PullRequestChecksState>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestSummary {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub head_branch: String,
    pub base_branch: String,
    pub state: PullRequestState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_draft: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub closed_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub merged_at: Option<Option<String>>,
    pub updated_at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub author: Option<Option<PullRequestActor>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub review_decision: Option<Option<PullRequestReviewDecision>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub checks_state: Option<Option<PullRequestChecksState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mergeability: Option<PullRequestMergeability>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub stack: Option<Option<PullRequestStackMembership>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_merge_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_merge_method: Option<PullRequestMergeMethod>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestStackLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_draft: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    pub number: u64,
    pub head_branch: String,
    pub state: PullRequestState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestStack {
    pub id: String,
    pub number: u64,
    pub url: String,
    pub base: String,
    pub layers: Vec<ProviderChangeRequestStackLayer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListCursor {
    pub updated_before: String,
    pub delivered: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestPage {
    pub items: Vec<ProviderChangeRequest>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_advance: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_after: Option<String>,
    pub continues: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBatchedChangeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<PullRequestStackMembership>,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: Option<PullRequestActor>,
    pub head_branch: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub head_repository_name_with_owner: Option<Option<String>>,
    pub base_branch: String,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub mergeability: PullRequestMergeability,
    pub additions: u64,
    pub deletions: u64,
    pub created_at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub closed_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub merged_at: Option<Option<String>>,
    pub updated_at: String,
    pub review_request_logins: Vec<String>,
    pub labels: Vec<PullRequestLabel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub review_decision: Option<Option<PullRequestReviewDecision>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub checks_state: Option<Option<PullRequestChecksState>>,
    pub repository: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBatchedChangeRequestPage {
    pub items: Vec<ProviderBatchedChangeRequest>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestStat {
    pub repository: String,
    pub number: u64,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<PullRequestStackMembership>,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: Option<PullRequestActor>,
    pub head_branch: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub head_repository_name_with_owner: Option<Option<String>>,
    pub base_branch: String,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub mergeability: PullRequestMergeability,
    pub additions: u64,
    pub deletions: u64,
    pub created_at: String,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub closed_at: Option<String>,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub merged_at: Option<String>,
    pub updated_at: String,
    pub review_request_logins: Vec<String>,
    pub labels: Vec<PullRequestLabel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub review_decision: Option<Option<PullRequestReviewDecision>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub checks_state: Option<Option<PullRequestChecksState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintainer_can_modify: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub head_sha: Option<Option<String>>,
    pub body: String,
    pub changed_files: u64,
    pub reviewers: Vec<PullRequestActor>,
    pub checks: Vec<PullRequestCheck>,
    pub merge_capabilities: PullRequestMergeCapabilities,
    pub viewer_permissions: PullRequestViewerPermissions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_comparison: Option<PullRequestBaseComparison>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind_by: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_merge_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_merge_method: Option<PullRequestMergeMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_approvals_required: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_queue_removal: Option<PullRequestMergeQueueRemoval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChangeRequestActivity {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub author: Option<Option<PullRequestActor>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewers: Option<Vec<PullRequestActor>>,
    pub comments: Vec<PullRequestComment>,
    pub comment_count: u64,
    pub comments_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_threads_truncated: Option<bool>,
    pub review_threads: Vec<PullRequestReviewThread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview_threads: Option<Vec<PullRequestOverviewThread>>,
    pub commits: Vec<PullRequestCommit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reactions: Option<Vec<PullRequestReaction>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiffSlice {
    pub patch: String,
    pub truncated: bool,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted_file_stats: Option<Vec<PullRequestOmittedFileStat>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiffFileContents {
    pub old_contents: String,
    pub new_contents: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiffFilePaths {
    pub files: Vec<DiffFilePath>,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFilesViewed {
    pub files: Vec<PullRequestFileViewed>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFileStats {
    pub files: Vec<FileStat>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFileRevisions {
    pub revisions: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRepositoryRef {
    pub cwd: String,
    pub repository: String,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFilePath {
    pub old_path: String,
    pub new_path: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderFilterOptionPage<T> {
    pub items: Vec<T>,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub state: PullRequestState,
    pub checks: Vec<PullRequestCheck>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreview {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub created_at: String,
    #[serde(with = "t3_contracts::wire::nullable")]
    pub author: Option<PullRequestActor>,
}
