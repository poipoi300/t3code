crate::string_id!(SourceControlProviderKind);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChangeRequestState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "merged")]
    Merged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "_tag")]
pub enum WireOption<T> {
    None,
    Some { value: T },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRequest {
    pub provider: SourceControlProviderKind,
    pub number: crate::base_schemas::PositiveInt,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub url: String,
    pub base_ref_name: crate::base_schemas::TrimmedNonEmptyString,
    pub head_ref_name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub head_sha: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub state: ChangeRequestState,
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
    pub closed_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub merged_at: Option<Option<String>>,
    pub updated_at: WireOption<crate::wire::WireDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_cross_repository: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub head_repository_name_with_owner: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub head_repository_owner_login: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlAppAccessStatus {
    #[serde(rename = "covered")]
    Covered,
    #[serde(rename = "not-covered")]
    NotCovered,
    #[serde(rename = "signed-out")]
    SignedOut,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlAppAccess {
    pub status: SourceControlAppAccessStatus,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlAppAccessInput {
    pub provider: SourceControlProviderKind,
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlCloneProtocol {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "ssh")]
    Ssh,
    #[serde(rename = "https")]
    Https,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlCloneRepositoryInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider: Option<Option<SourceControlProviderKind>>,
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
    pub protocol: Option<Option<SourceControlCloneProtocol>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlRepositoryInfo {
    pub provider: SourceControlProviderKind,
    pub name_with_owner: crate::base_schemas::TrimmedNonEmptyString,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    pub ssh_url: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlCloneRepositoryResult {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub remote_url: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub repository: Option<SourceControlRepositoryInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlDiscoveryStatus {
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "missing")]
    Missing,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsDiscoveryItem {
    pub kind: crate::vcs::VcsDriverKind,
    pub implemented: bool,
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub executable: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub status: SourceControlDiscoveryStatus,
    pub version: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    pub install_hint: crate::base_schemas::TrimmedNonEmptyString,
    pub detail: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlProviderAuthStatus {
    #[serde(rename = "authenticated")]
    Authenticated,
    #[serde(rename = "unauthenticated")]
    Unauthenticated,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlProviderAuthAccounts {
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub account: crate::base_schemas::TrimmedNonEmptyString,
    pub active: bool,
    pub authenticated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub error: Option<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub environment_variable: Option<crate::base_schemas::TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlProviderAuth {
    pub status: SourceControlProviderAuthStatus,
    pub account: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    pub host: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    pub detail: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub accounts: Option<Vec<SourceControlProviderAuthAccounts>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlProviderDiscoveryItem {
    pub kind: SourceControlProviderKind,
    pub label: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub executable: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub status: SourceControlDiscoveryStatus,
    pub version: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    pub install_hint: crate::base_schemas::TrimmedNonEmptyString,
    pub detail: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    pub auth: SourceControlProviderAuth,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub app_sign_in: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlDiscoveryResult {
    pub version_control_systems: Vec<VcsDiscoveryItem>,
    pub source_control_providers: Vec<SourceControlProviderDiscoveryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlProviderErrorTag {
    #[serde(rename = "SourceControlProviderError")]
    SourceControlProviderError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlProviderError {
    #[serde(rename = "_tag")]
    pub _tag: SourceControlProviderErrorTag,
    pub provider: SourceControlProviderKind,
    pub operation: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub command: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub repository: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reference: Option<Option<String>>,
    pub detail: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlProviderInfo {
    pub kind: SourceControlProviderKind,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlRepositoryVisibility {
    #[serde(rename = "private")]
    Private,
    #[serde(rename = "public")]
    Public,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlPublishRepositoryInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub provider: SourceControlProviderKind,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    pub visibility: SourceControlRepositoryVisibility,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub remote_name: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub protocol: Option<Option<SourceControlCloneProtocol>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlPublishStatus {
    #[serde(rename = "pushed")]
    Pushed,
    #[serde(rename = "remote_added")]
    RemoteAdded,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlPublishRepositoryResult {
    pub repository: SourceControlRepositoryInfo,
    pub remote_name: crate::base_schemas::TrimmedNonEmptyString,
    pub remote_url: crate::base_schemas::TrimmedNonEmptyString,
    pub branch: crate::base_schemas::TrimmedNonEmptyString,
    pub status: SourceControlPublishStatus,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlRepositoryCloneUrls {
    pub name_with_owner: crate::base_schemas::TrimmedNonEmptyString,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    pub ssh_url: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlRepositoryErrorTag {
    #[serde(rename = "SourceControlRepositoryError")]
    SourceControlRepositoryError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlRepositoryError {
    #[serde(rename = "_tag")]
    pub _tag: SourceControlRepositoryErrorTag,
    pub provider: SourceControlProviderKind,
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
#[serde(rename_all = "camelCase")]
pub struct SourceControlRepositoryLookupInput {
    pub provider: SourceControlProviderKind,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlSignInErrorTag {
    #[serde(rename = "SourceControlSignInError")]
    SourceControlSignInError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceControlSignInFailure {
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "not-started")]
    NotStarted,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "denied")]
    Denied,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlSignInError {
    #[serde(rename = "_tag")]
    pub _tag: SourceControlSignInErrorTag,
    pub provider: SourceControlProviderKind,
    pub host: String,
    pub reason: SourceControlSignInFailure,
    pub detail: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlSignInInput {
    pub provider: SourceControlProviderKind,
    pub host: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlSignInPrompt {
    pub user_code: crate::base_schemas::TrimmedNonEmptyString,
    pub verification_uri: crate::base_schemas::TrimmedNonEmptyString,
    pub expires_at: crate::vcs::EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlSignInResult {
    pub account: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlStarredInput {
    pub provider: SourceControlProviderKind,
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub repository: crate::base_schemas::TrimmedNonEmptyString,
}
