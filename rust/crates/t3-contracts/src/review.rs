#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBranchCommitsInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_ref: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBranchCommitsResultCommits {
    pub sha: String,
    pub subject: String,
    pub committed_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBranchCommitsResult {
    pub commits: Vec<ReviewBranchCommitsResultCommits>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReviewDiffPreviewSourceKind {
    #[serde(rename = "working-tree")]
    WorkingTree,
    #[serde(rename = "branch-range")]
    BranchRange,
    #[serde(rename = "commit")]
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReviewDiffFileContentsInputChangeType {
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
pub struct ReviewDiffFileContentsInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub source_kind: ReviewDiffPreviewSourceKind,
    pub change_type: ReviewDiffFileContentsInputChangeType,
    #[serde(with = "crate::wire::nullable")]
    pub base_ref: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub head_ref: Option<crate::base_schemas::TrimmedNonEmptyString>,
    pub old_path: crate::base_schemas::TrimmedNonEmptyString,
    pub new_path: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffFileContentsResult {
    pub old_contents: String,
    pub new_contents: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffFileStat {
    pub path: String,
    #[serde(with = "crate::wire::nullable")]
    pub previous_path: Option<String>,
    pub additions: crate::vcs::EncodedNumber,
    pub deletions: crate::vcs::EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ReviewDiffPreviewError {
    Vcs(crate::vcs::VcsError),
    Git(crate::git::GitCommandError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReviewDiffPreviewInputSourceKind {
    #[serde(rename = "working-tree")]
    WorkingTree,
    #[serde(rename = "branch-range")]
    BranchRange,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReviewDiffPreviewInputFileSourceKind {
    #[serde(rename = "working-tree")]
    WorkingTree,
    #[serde(rename = "branch-range")]
    BranchRange,
    #[serde(rename = "commit")]
    Commit,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffPreviewInputFile {
    pub path: String,
    #[serde(with = "crate::wire::nullable")]
    pub previous_path: Option<String>,
    pub source_kind: ReviewDiffPreviewInputFileSourceKind,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffPreviewInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub base_ref: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub commit_sha: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub ignore_whitespace: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub source_kind: Option<ReviewDiffPreviewInputSourceKind>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub tracked_only: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub metadata_only: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub file: Option<ReviewDiffPreviewInputFile>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffPreviewSource {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: ReviewDiffPreviewSourceKind,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub commit_subject: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub base_ref: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(with = "crate::wire::nullable")]
    pub head_ref: Option<crate::base_schemas::TrimmedNonEmptyString>,
    pub diff: String,
    pub diff_hash: crate::base_schemas::TrimmedNonEmptyString,
    pub truncated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub patches_deferred: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub files: Option<Vec<ReviewDiffFileStat>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffPreviewResult {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub generated_at: crate::wire::WireDateTime,
    pub sources: Vec<ReviewDiffPreviewSource>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub incomplete: Option<bool>,
}
