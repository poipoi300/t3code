#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestRedeemRequest {
    pub credential: crate::base_schemas::TrimmedNonEmptyString,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuestShareCreateErrorTag {
    #[serde(rename = "GuestShareCreateError")]
    GuestShareCreateError,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuestShareCreateErrorReason {
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "connect_unavailable")]
    ConnectUnavailable,
    #[serde(rename = "thread_unavailable")]
    ThreadUnavailable,
    #[serde(rename = "read_failed")]
    ReadFailed,
    #[serde(rename = "changed")]
    Changed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestShareCreateError {
    #[serde(rename = "_tag")]
    pub _tag: GuestShareCreateErrorTag,
    pub reason: GuestShareCreateErrorReason,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestShareCreateInput {
    pub thread_id: crate::base_schemas::ThreadId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThreadGuestMessageStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "sent")]
    Sent,
    #[serde(rename = "dismissed")]
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGuestMessage {
    pub id: crate::base_schemas::GuestMessageId,
    pub text: String,
    pub status: ThreadGuestMessageStatus,
    pub submitted_at: crate::base_schemas::IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub attachments: Option<Option<Vec<crate::chat_attachment::ChatAttachment>>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGuestShare {
    pub id: crate::base_schemas::GuestShareId,
    pub host: crate::base_schemas::TrimmedNonEmptyString,
    pub created_at: crate::base_schemas::IsoDateTime,
    pub expires_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub guest_name: Option<crate::base_schemas::TrimmedNonEmptyString>,
    pub messages: Vec<ThreadGuestMessage>,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestShareCreateResult {
    pub url: crate::base_schemas::TrimmedNonEmptyString,
    pub share: ThreadGuestShare,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestSubmitRequest {
    pub share_id: crate::base_schemas::GuestShareId,
    pub text: crate::base_schemas::TrimmedString,
}

pub const GUEST_SHARE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
pub const GUEST_MESSAGE_MAX_LENGTH: usize = 4_000;
pub const GUEST_NAME_MAX_LENGTH: usize = 40;
pub const GUEST_PENDING_MESSAGE_LIMIT: usize = 3;
pub const GUEST_RESOLVED_MESSAGE_LIMIT: usize = 20;
pub const GUEST_MESSAGE_MAX_ATTACHMENTS: usize = 4;
pub const GUEST_MESSAGE_MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
pub const GUEST_PENDING_ATTACHMENT_BYTES_LIMIT: u64 = 20 * 1024 * 1024;
pub const GUEST_ROUTE_PREFIX: &str = "/api/guest";
pub const GUEST_SHARE_PARAM: &str = "share";
pub const GUEST_RENDER_ROUTE: &str = "/api/guest/render";
pub const GUEST_ITEM_ROUTE: &str = "/api/guest/item";
pub const GUEST_HISTORY_ROUTE: &str = "/api/guest/history";
pub const GUEST_COMMAND_OUTPUT_ROUTE: &str = "/api/guest/command-output";
pub const GUEST_ATTACHMENT_ROUTE: &str = "/api/guest/attachment";
pub const GUEST_MEDIA_ROUTE: &str = "/api/guest/media";

crate::redacted_debug!(GuestRedeemRequest { name } redact { credential });
crate::redacted_debug!(GuestShareCreateResult { share } redact { url });
