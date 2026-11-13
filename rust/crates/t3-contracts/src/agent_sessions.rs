use crate::base_schemas::TrimmedNonEmptyString;
use crate::base_schemas::{IsoDateTime, ProjectId};
use crate::provider_instance::ProviderInstanceId;
use crate::speech::SpeechNumber;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentSessionSource {
    #[serde(rename = "claudeAgent")]
    ClaudeAgent,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "pi")]
    Pi,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentSessionInode {
    String(String),
    SpeechNumber(SpeechNumber),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionImportSource {
    pub provider: AgentSessionSource,
    pub provider_instance_id: ProviderInstanceId,
    pub provider_session_id: TrimmedNonEmptyString,
    pub file_path: TrimmedNonEmptyString,
    pub size: u64,
    #[serde(with = "crate::wire::nullable")]
    pub mtime_ms: Option<SpeechNumber>,
    pub device: SpeechNumber,
    #[serde(with = "crate::wire::nullable")]
    pub inode: Option<AgentSessionInode>,
    #[serde(with = "crate::wire::nullable")]
    pub birthtime_ms: Option<SpeechNumber>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionScanInput {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionProjectGit {
    #[serde(with = "crate::wire::nullable")]
    pub remote_key: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub repository: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionProjectCandidate {
    pub path: TrimmedNonEmptyString,
    pub title: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_id: Option<Option<ProjectId>>,
    pub sources: Vec<AgentSessionSource>,
    pub thread_count: u64,
    #[serde(with = "crate::wire::nullable")]
    pub last_active_at: Option<IsoDateTime>,
    pub already_imported: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub git: Option<Option<AgentSessionProjectGit>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionScanResult {
    pub candidates: Vec<AgentSessionProjectCandidate>,
    pub scanned_at: IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub truncated: Option<Option<bool>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionImportInput {
    pub project_id: ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expected_workspace_root: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentSessionImportProjectNotFoundErrorTag {
    #[serde(rename = "AgentSessionImportProjectNotFoundError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionImportProjectNotFoundError {
    #[serde(rename = "_tag")]
    pub _tag: AgentSessionImportProjectNotFoundErrorTag,
    pub project_id: ProjectId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentSessionImportProjectChangedErrorTag {
    #[serde(rename = "AgentSessionImportProjectChangedError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionImportProjectChangedError {
    #[serde(rename = "_tag")]
    pub _tag: AgentSessionImportProjectChangedErrorTag,
    pub project_id: ProjectId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionImportResult {
    pub imported_count: u64,
    pub skipped_count: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentSessionScanOperation {
    #[serde(rename = "read-settings")]
    ReadSettings,
    #[serde(rename = "read-projects")]
    ReadProjects,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentSessionScanErrorTag {
    #[serde(rename = "AgentSessionScanError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionScanError {
    #[serde(rename = "_tag")]
    pub _tag: AgentSessionScanErrorTag,
    pub operation: AgentSessionScanOperation,
    pub cause: Value,
}
