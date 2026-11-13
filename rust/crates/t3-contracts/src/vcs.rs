#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsDriverKind {
    #[serde(rename = "git")]
    Git,
    #[serde(rename = "jj")]
    Jj,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsDriverCapabilitiesIgnoreClassifier {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "git-compatible-fallback")]
    GitCompatibleFallback,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsDriverCapabilities {
    pub kind: VcsDriverKind,
    pub supports_worktrees: bool,
    pub supports_bookmarks: bool,
    pub supports_atomic_snapshot: bool,
    pub supports_push_default_remote: bool,
    pub ignore_classifier: VcsDriverCapabilitiesIgnoreClassifier,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessSpawnErrorTag {
    #[serde(rename = "VcsProcessSpawnError")]
    VcsProcessSpawnError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsProcessSpawnError {
    #[serde(rename = "_tag")]
    pub _tag: VcsProcessSpawnErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessExitErrorTag {
    #[serde(rename = "VcsProcessExitError")]
    VcsProcessExitError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessExitFailureKind {
    #[serde(rename = "authentication")]
    Authentication,
    #[serde(rename = "not-found")]
    NotFound,
    #[serde(rename = "rate-limited")]
    RateLimited,
    #[serde(rename = "command-failed")]
    CommandFailed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsProcessExitError {
    #[serde(rename = "_tag")]
    pub _tag: VcsProcessExitErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    pub exit_code: EncodedNumber,
    pub detail: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure_kind: Option<Option<VcsProcessExitFailureKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub retryable: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stderr_length: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stderr_truncated: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessTimeoutErrorTag {
    #[serde(rename = "VcsProcessTimeoutError")]
    VcsProcessTimeoutError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsProcessTimeoutError {
    #[serde(rename = "_tag")]
    pub _tag: VcsProcessTimeoutErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    pub timeout_ms: EncodedNumber,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessStdinWriteErrorTag {
    #[serde(rename = "VcsProcessStdinWriteError")]
    VcsProcessStdinWriteError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsProcessStdinWriteError {
    #[serde(rename = "_tag")]
    pub _tag: VcsProcessStdinWriteErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    pub stdin_bytes: crate::base_schemas::NonNegativeInt,
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessOutputReadErrorTag {
    #[serde(rename = "VcsProcessOutputReadError")]
    VcsProcessOutputReadError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessOutputReadErrorStream {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "stderr")]
    Stderr,
    #[serde(rename = "exitCode")]
    ExitCode,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsProcessOutputReadError {
    #[serde(rename = "_tag")]
    pub _tag: VcsProcessOutputReadErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    pub stream: VcsProcessOutputReadErrorStream,
    pub cause: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessOutputLimitErrorTag {
    #[serde(rename = "VcsProcessOutputLimitError")]
    VcsProcessOutputLimitError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessOutputLimitErrorStream {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "stderr")]
    Stderr,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsProcessOutputLimitError {
    #[serde(rename = "_tag")]
    pub _tag: VcsProcessOutputLimitErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
    pub stream: VcsProcessOutputLimitErrorStream,
    pub max_bytes: crate::base_schemas::NonNegativeInt,
    pub observed_bytes: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsProcessMissingExitCodeErrorTag {
    #[serde(rename = "VcsProcessMissingExitCodeError")]
    VcsProcessMissingExitCodeError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsProcessMissingExitCodeError {
    #[serde(rename = "_tag")]
    pub _tag: VcsProcessMissingExitCodeErrorTag,
    pub operation: String,
    pub command: String,
    pub cwd: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub argument_count: Option<Option<crate::base_schemas::NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsRepositoryDetectionErrorTag {
    #[serde(rename = "VcsRepositoryDetectionError")]
    VcsRepositoryDetectionError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsRepositoryDetectionError {
    #[serde(rename = "_tag")]
    pub _tag: VcsRepositoryDetectionErrorTag,
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
pub enum VcsUnsupportedOperationErrorTag {
    #[serde(rename = "VcsUnsupportedOperationError")]
    VcsUnsupportedOperationError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsUnsupportedOperationError {
    #[serde(rename = "_tag")]
    pub _tag: VcsUnsupportedOperationErrorTag,
    pub operation: String,
    pub kind: VcsDriverKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum VcsError {
    Spawn(VcsProcessSpawnError),
    Exit(VcsProcessExitError),
    Timeout(VcsProcessTimeoutError),
    StdinWrite(VcsProcessStdinWriteError),
    OutputRead(VcsProcessOutputReadError),
    OutputLimit(VcsProcessOutputLimitError),
    MissingExitCode(VcsProcessMissingExitCodeError),
    RepositoryDetection(VcsRepositoryDetectionError),
    UnsupportedOperation(VcsUnsupportedOperationError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VcsFreshnessSource {
    #[serde(rename = "live-local")]
    LiveLocal,
    #[serde(rename = "cached-local")]
    CachedLocal,
    #[serde(rename = "cached-remote")]
    CachedRemote,
    #[serde(rename = "explicit-remote")]
    ExplicitRemote,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "_tag")]
pub enum WireOption<T> {
    None,
    Some { value: T },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsFreshness {
    pub source: VcsFreshnessSource,
    pub observed_at: crate::wire::WireDateTime,
    pub expires_at: WireOption<crate::wire::WireDateTime>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsRemote {
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    pub push_url: WireOption<crate::base_schemas::TrimmedNonEmptyString>,
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsListRemotesResult {
    pub remotes: Vec<VcsRemote>,
    pub freshness: VcsFreshness,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsListWorkspaceFilesResult {
    pub paths: Vec<crate::base_schemas::TrimmedNonEmptyString>,
    pub truncated: bool,
    pub freshness: VcsFreshness,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcsRepositoryIdentity {
    pub kind: VcsDriverKind,
    pub root_path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub metadata_path: Option<crate::base_schemas::TrimmedNonEmptyString>,
    pub freshness: VcsFreshness,
}

// Effect's JSON codec preserves non-finite Schema.Number values as string markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NonFiniteNumber {
    #[serde(rename = "NaN")]
    NaN,
    #[serde(rename = "Infinity")]
    Infinity,
    #[serde(rename = "-Infinity")]
    NegativeInfinity,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum EncodedNumber {
    Finite(crate::wire::JsNumber),
    NonFinite(NonFiniteNumber),
}
