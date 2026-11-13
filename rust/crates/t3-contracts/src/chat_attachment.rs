use crate::base_schemas::{IsoDateTime, MessageId, NonNegativeInt, PositiveInt, ThreadId};
use serde::{Deserialize, Serialize};

pub type ChatAttachmentId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapShotAccessibilityBounds {
    pub x: NonNegativeInt,
    pub y: NonNegativeInt,
    pub width: PositiveInt,
    pub height: PositiveInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapShotCheckedState {
    #[serde(rename = "on")]
    On,
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "mixed")]
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapShotAccessibilityState {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub active: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub busy: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub checked: Option<Option<SnapShotCheckedState>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub editable: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub enabled: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub expanded: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub focused: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub selected: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub visible: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapShotAccessibilityNode {
    pub role: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub value: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub description: Option<String>,
    #[serde(with = "crate::wire::nullable")]
    pub bounds: Option<SnapShotAccessibilityBounds>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub state: Option<SnapShotAccessibilityState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub actions: Option<Vec<String>>,
    pub children: Vec<SnapShotAccessibilityNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapShotImageSize {
    pub width: PositiveInt,
    pub height: PositiveInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapturedImageCoordinateSpace {
    #[serde(rename = "captured-image")]
    CapturedImage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format")]
pub enum SnapShotAccessibility {
    #[serde(rename = "flat-text")]
    FlatText { text: String, truncated: bool },
    #[serde(rename = "element-tree")]
    ElementTree {
        #[serde(rename = "coordinateSpace")]
        coordinate_space: CapturedImageCoordinateSpace,
        #[serde(rename = "imageSize")]
        image_size: SnapShotImageSize,
        truncated: bool,
        root: SnapShotAccessibilityNode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapShotSourceKind {
    #[serde(rename = "snap-shot")]
    SnapShot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapShotSource {
    pub kind: SnapShotSourceKind,
    pub captured_at: IsoDateTime,
    pub app_name: String,
    pub window_title: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accessible_text: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accessibility: Option<Option<SnapShotAccessibility>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub app_identifier: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub app_icon_data_url: Option<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatImageAttachmentType {
    #[serde(rename = "image")]
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatFileAttachmentType {
    #[serde(rename = "file")]
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PastedTextAttachmentSourceTag {
    #[serde(rename = "pasted-text")]
    PastedText,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PastedTextAttachmentSource {
    #[serde(rename = "_tag")]
    pub _tag: PastedTextAttachmentSourceTag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatImageAttachment {
    pub r#type: ChatImageAttachmentType,
    pub id: ChatAttachmentId,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source: Option<Option<SnapShotSource>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatFileAttachment {
    pub r#type: ChatFileAttachmentType,
    pub id: ChatAttachmentId,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source: Option<Option<PastedTextAttachmentSource>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UnknownAttachmentType(pub String);

impl<'de> Deserialize<'de> for UnknownAttachmentType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value == "image" || value == "file" || crate::wire::trim_js(&value).is_empty() {
            return Err(serde::de::Error::custom(
                "expected an unknown attachment type",
            ));
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUnknownAttachment {
    pub r#type: UnknownAttachmentType,
    pub id: ChatAttachmentId,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadChatImageAttachment {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub id: Option<Option<ChatAttachmentId>>,
    pub r#type: ChatImageAttachmentType,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: NonNegativeInt,
    pub data_url: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub source: Option<Option<SnapShotSource>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatAttachment {
    Image(Box<ChatImageAttachment>),
    File(ChatFileAttachment),
    Unknown(ChatUnknownAttachment),
}

pub type UploadChatAttachment = UploadChatImageAttachment;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistChatAttachmentsInput {
    pub thread_id: ThreadId,
    pub message_id: MessageId,
    pub attachments: Vec<UploadChatAttachment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistChatAttachmentsResult {
    pub attachments: Vec<ChatAttachment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistChatAttachmentsErrorTag {
    #[serde(rename = "PersistChatAttachmentsError")]
    PersistChatAttachmentsError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistChatAttachmentsError {
    #[serde(rename = "_tag")]
    pub _tag: PersistChatAttachmentsErrorTag,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

pub const PROVIDER_SEND_TURN_MAX_INPUT_CHARS: usize = 120_000;
pub const PROVIDER_SEND_TURN_MAX_ATTACHMENTS: usize = 100;
pub const PROVIDER_SEND_TURN_MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
pub const PROVIDER_SEND_TURN_MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
pub const PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES: &[&str] =
    &["image/gif", "image/jpeg", "image/png", "image/webp"];
pub const SNAP_SHOT_ACCESSIBLE_TEXT_MAX_CHARS: usize = 32_000;
pub const SNAP_SHOT_ACCESSIBILITY_MAX_NODES: usize = 10_000;
pub const SNAP_SHOT_ACCESSIBILITY_MAX_SERIALIZED_CHARS: usize = 32_000;
