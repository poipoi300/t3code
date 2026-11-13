use crate::base_schemas::TrimmedNonEmptyString;
use crate::base_schemas::{ThreadId, TurnItemId};
use crate::provider_runtime::ToolActivityNativeAppReference;
use crate::speech::SpeechNumber;
use serde::{Deserialize, Serialize};
pub const SOURCE_CONTROL_MEDIA_ASSET: &str = "github-media";
pub const ATTACHMENT_UPLOAD_URL_TTL_MS: u64 = 600_000;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeAppIconReference {
    Native(ToolActivityNativeAppReference),
    Path(AppPathReference),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppPathTag {
    #[serde(rename = "app-path")]
    AppPath,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppPathReference {
    #[serde(rename = "_tag")]
    pub tag: AppPathTag,
    pub path: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetDisposition {
    Inline,
    Attachment,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DownloadDisposition {
    #[serde(rename = "attachment")]
    Attachment,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetRenderIntent {
    #[serde(rename = "mcp-app")]
    McpApp,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "_tag",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum AssetResource {
    WorkspaceFile {
        thread_id: ThreadId,
        path: TrimmedNonEmptyString,
    },
    MediaFile {
        thread_id: ThreadId,
        path: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        disposition: Option<DownloadDisposition>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        host_only: Option<crate::speech::True>,
    },
    DraftWorkspaceFile {
        cwd: TrimmedNonEmptyString,
        path: TrimmedNonEmptyString,
    },
    Attachment {
        attachment_id: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        file_name: Option<TrimmedNonEmptyString>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        mime_type: Option<TrimmedNonEmptyString>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        disposition: Option<AssetDisposition>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        render_intent: Option<AssetRenderIntent>,
    },
    ToolOutputImage {
        thread_id: ThreadId,
        item_id: TurnItemId,
        index: u64,
    },
    ProjectFavicon {
        cwd: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        path: Option<Option<crate::project::ProjectFaviconPath>>,
    },
    NativeAppIcon {
        app: NativeAppIconReference,
    },
    #[serde(rename = "github-media")]
    SourceControlMedia {
        cwd: TrimmedNonEmptyString,
        url: TrimmedNonEmptyString,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::optional"
        )]
        repository: Option<TrimmedNonEmptyString>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetCreateUrlInput {
    pub resource: AssetResource,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetImageDimensions {
    pub width: u64,
    pub height: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetCreateUrlResult {
    pub relative_url: TrimmedNonEmptyString,
    pub expires_at: SpeechNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source_path: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub image_dimensions: Option<Option<AssetImageDimensions>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub file_revision: Option<Option<TrimmedNonEmptyString>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImageAttachmentType {
    #[serde(rename = "image")]
    Image,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FileAttachmentType {
    #[serde(rename = "file")]
    File,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UploadImageMimeType {
    #[serde(rename = "image/gif")]
    Gif,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/webp")]
    Webp,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentCreateUploadUrlInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub r#type: Option<ImageAttachmentType>,
    pub name: TrimmedNonEmptyString,
    pub mime_type: UploadImageMimeType,
    pub size_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAttachmentCreateUploadUrlInput {
    pub r#type: FileAttachmentType,
    pub name: TrimmedNonEmptyString,
    pub mime_type: TrimmedNonEmptyString,
    pub size_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttachmentCreateUploadUrlInput {
    Image(ImageAttachmentCreateUploadUrlInput),
    File(FileAttachmentCreateUploadUrlInput),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentCreateUploadUrlResult {
    pub attachment_id: TrimmedNonEmptyString,
    pub relative_url: TrimmedNonEmptyString,
    pub expires_at: SpeechNumber,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDeleteInput {
    pub attachment_id: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AttachmentUploadSigningKeyErrorTag {
    #[serde(rename = "AttachmentUploadSigningKeyError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentUploadSigningKeyError {
    #[serde(rename = "_tag")]
    pub tag: AttachmentUploadSigningKeyErrorTag,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetWorkspaceContextNotFoundErrorTag {
    #[serde(rename = "AssetWorkspaceContextNotFoundError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetWorkspaceContextNotFoundError {
    #[serde(rename = "_tag")]
    pub tag: AssetWorkspaceContextNotFoundErrorTag,
    pub resource: AssetResource,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetWorkspaceContextResolutionErrorTag {
    #[serde(rename = "AssetWorkspaceContextResolutionError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetWorkspaceContextResolutionError {
    #[serde(rename = "_tag")]
    pub tag: AssetWorkspaceContextResolutionErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetWorkspaceRootNormalizationErrorTag {
    #[serde(rename = "AssetWorkspaceRootNormalizationError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetWorkspaceRootNormalizationError {
    #[serde(rename = "_tag")]
    pub tag: AssetWorkspaceRootNormalizationErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetWorkspacePathValidationErrorTag {
    #[serde(rename = "AssetWorkspacePathValidationError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetWorkspacePathValidationError {
    #[serde(rename = "_tag")]
    pub tag: AssetWorkspacePathValidationErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetPreviewTypeValidationErrorTag {
    #[serde(rename = "AssetPreviewTypeValidationError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPreviewTypeValidationError {
    #[serde(rename = "_tag")]
    pub tag: AssetPreviewTypeValidationErrorTag,
    pub resource: AssetResource,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetWorkspaceAssetInspectionErrorTag {
    #[serde(rename = "AssetWorkspaceAssetInspectionError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetWorkspaceAssetInspectionError {
    #[serde(rename = "_tag")]
    pub tag: AssetWorkspaceAssetInspectionErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetWorkspaceAssetNotFoundErrorTag {
    #[serde(rename = "AssetWorkspaceAssetNotFoundError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetWorkspaceAssetNotFoundError {
    #[serde(rename = "_tag")]
    pub tag: AssetWorkspaceAssetNotFoundErrorTag,
    pub resource: AssetResource,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub checked_path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetWorkspaceResolutionErrorTag {
    #[serde(rename = "AssetWorkspaceResolutionError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetWorkspaceResolutionError {
    #[serde(rename = "_tag")]
    pub tag: AssetWorkspaceResolutionErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetAttachmentNotFoundErrorTag {
    #[serde(rename = "AssetAttachmentNotFoundError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetAttachmentNotFoundError {
    #[serde(rename = "_tag")]
    pub tag: AssetAttachmentNotFoundErrorTag,
    pub resource: AssetResource,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetProjectFaviconResolutionErrorTag {
    #[serde(rename = "AssetProjectFaviconResolutionError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetProjectFaviconResolutionError {
    #[serde(rename = "_tag")]
    pub tag: AssetProjectFaviconResolutionErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetProjectFaviconInspectionErrorTag {
    #[serde(rename = "AssetProjectFaviconInspectionError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetProjectFaviconInspectionError {
    #[serde(rename = "_tag")]
    pub tag: AssetProjectFaviconInspectionErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetProjectFaviconNotFoundErrorTag {
    #[serde(rename = "AssetProjectFaviconNotFoundError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetProjectFaviconNotFoundError {
    #[serde(rename = "_tag")]
    pub tag: AssetProjectFaviconNotFoundErrorTag,
    pub resource: AssetResource,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetSigningKeyLoadErrorTag {
    #[serde(rename = "AssetSigningKeyLoadError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetSigningKeyLoadError {
    #[serde(rename = "_tag")]
    pub tag: AssetSigningKeyLoadErrorTag,
    pub resource: AssetResource,
    pub cause: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssetSourceControlMediaUrlValidationErrorTag {
    #[serde(rename = "AssetGitHubMediaUrlValidationError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetSourceControlMediaUrlValidationError {
    #[serde(rename = "_tag")]
    pub tag: AssetSourceControlMediaUrlValidationErrorTag,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssetAccessError {
    WorkspaceContextNotFoundError(AssetWorkspaceContextNotFoundError),
    WorkspaceContextResolutionError(AssetWorkspaceContextResolutionError),
    WorkspaceRootNormalizationError(AssetWorkspaceRootNormalizationError),
    WorkspacePathValidationError(AssetWorkspacePathValidationError),
    PreviewTypeValidationError(AssetPreviewTypeValidationError),
    WorkspaceAssetInspectionError(AssetWorkspaceAssetInspectionError),
    WorkspaceAssetNotFoundError(AssetWorkspaceAssetNotFoundError),
    WorkspaceResolutionError(AssetWorkspaceResolutionError),
    AttachmentNotFoundError(AssetAttachmentNotFoundError),
    ProjectFaviconResolutionError(AssetProjectFaviconResolutionError),
    ProjectFaviconInspectionError(AssetProjectFaviconInspectionError),
    ProjectFaviconNotFoundError(AssetProjectFaviconNotFoundError),
    SigningKeyLoadError(AssetSigningKeyLoadError),
    SourceControlMediaUrlValidationError(AssetSourceControlMediaUrlValidationError),
}
