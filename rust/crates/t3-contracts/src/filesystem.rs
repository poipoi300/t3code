use crate::base_schemas::TrimmedNonEmptyString;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemGetMetadataInput {
    pub paths: Vec<TrimmedNonEmptyString>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilesystemEntryKind {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "directory")]
    Directory,
    #[serde(rename = "other")]
    Other,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemEntryMetadata {
    pub kind: FilesystemEntryKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub byte_length: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub mime_type: Option<TrimmedNonEmptyString>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemGetMetadataResult {
    pub entries: Vec<Option<FilesystemEntryMetadata>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemBrowseInput {
    pub partial_path: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemBrowseEntry {
    pub name: TrimmedNonEmptyString,
    pub full_path: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemBrowseResult {
    pub parent_path: TrimmedNonEmptyString,
    pub entries: Vec<FilesystemBrowseEntry>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilesystemBrowseFailure {
    #[serde(rename = "windows_path_unsupported")]
    WindowsPathUnsupported,
    #[serde(rename = "current_project_required")]
    CurrentProjectRequired,
    #[serde(rename = "read_directory_failed")]
    ReadDirectoryFailed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilesystemBrowseErrorTag {
    #[serde(rename = "FilesystemBrowseError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemBrowseError {
    #[serde(rename = "_tag")]
    pub _tag: FilesystemBrowseErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub partial_path: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<FilesystemBrowseFailure>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub parent_path: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub platform: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub path_missing: Option<Option<bool>>,
    pub message: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<Value>>,
}
