#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCloneActionInput {
    pub project_id: crate::base_schemas::ProjectId,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCloneActionResult {
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectClonePhase {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectCloneStage {
    #[serde(rename = "connecting")]
    Connecting,
    #[serde(rename = "counting")]
    Counting,
    #[serde(rename = "receiving")]
    Receiving,
    #[serde(rename = "resolving")]
    Resolving,
    #[serde(rename = "checkout")]
    Checkout,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCloneSnapshot {
    pub project_id: crate::base_schemas::ProjectId,
    pub remote_url: crate::base_schemas::TrimmedNonEmptyString,
    pub destination_path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub repository: Option<crate::source_control::SourceControlRepositoryInfo>,
    pub phase: ProjectClonePhase,
    pub stage: ProjectCloneStage,
    #[serde(with = "crate::wire::nullable")]
    pub percent: Option<i64>,
    #[serde(with = "crate::wire::nullable")]
    pub detail: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub stalled: Option<bool>,
    #[serde(with = "crate::wire::nullable")]
    pub error: Option<String>,
    pub started_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub ended_at: Option<crate::base_schemas::IsoDateTime>,
    pub sequence: crate::base_schemas::NonNegativeInt,
}

pub type ProjectCloneListEvent = Vec<ProjectCloneSnapshot>;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCloneStartInput {
    pub project_id: crate::base_schemas::ProjectId,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub created_at: crate::base_schemas::IsoDateTime,
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
    pub repository: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub remote_url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub destination_path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub protocol: Option<Option<crate::source_control::SourceControlCloneProtocol>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCloneStartResult {
    pub project_id: crate::base_schemas::ProjectId,
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub remote_url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub repository: Option<crate::source_control::SourceControlRepositoryInfo>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCloneSubscribeInput {}

pub const PROJECT_CLONE_DETAIL_MAX_LENGTH: usize = 200;
pub const PROJECT_CLONE_ERROR_MAX_LENGTH: usize = 1000;
