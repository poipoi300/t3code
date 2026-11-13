use crate::{
    auth::AuthMcpClientAccess,
    base_schemas::{EnvironmentId, ThreadId, TrimmedNonEmptyString, TurnItemId},
    wire::WireDateTime,
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkStatus {
    #[serde(rename = "reachable")]
    Reachable,
    #[serde(rename = "unreachable")]
    Unreachable,
    #[serde(rename = "expired")]
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLink {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "label")]
    pub label: String,
    #[serde(rename = "urls")]
    pub urls: Vec<TrimmedNonEmptyString>,
    #[serde(rename = "access")]
    pub access: AuthMcpClientAccess,
    #[serde(rename = "linkedAt")]
    pub linked_at: WireDateTime,
    #[serde(rename = "expiresAt")]
    pub expires_at: WireDateTime,
    #[serde(rename = "lastReachedAt", with = "crate::wire::nullable")]
    pub last_reached_at: Option<WireDateTime>,
    #[serde(rename = "lastError", with = "crate::wire::nullable")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkSummary {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
    #[serde(rename = "label")]
    pub label: String,
    #[serde(rename = "urls")]
    pub urls: Vec<TrimmedNonEmptyString>,
    #[serde(rename = "access")]
    pub access: AuthMcpClientAccess,
    #[serde(rename = "linkedAt")]
    pub linked_at: WireDateTime,
    #[serde(rename = "expiresAt")]
    pub expires_at: WireDateTime,
    #[serde(rename = "lastReachedAt", with = "crate::wire::nullable")]
    pub last_reached_at: Option<WireDateTime>,
    #[serde(rename = "lastError", with = "crate::wire::nullable")]
    pub last_error: Option<String>,
    #[serde(rename = "status")]
    pub status: PeerLinkStatus,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkCreateInput {
    #[serde(rename = "url")]
    pub url: TrimmedNonEmptyString,
    #[serde(
        rename = "alternateUrls",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub alternate_urls: Option<Vec<TrimmedNonEmptyString>>,
    #[serde(rename = "pairingCode")]
    pub pairing_code: TrimmedNonEmptyString,
    #[serde(rename = "access")]
    pub access: AuthMcpClientAccess,
    #[serde(
        rename = "expectedEnvironmentId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub expected_environment_id: Option<EnvironmentId>,
    #[serde(
        rename = "expectedLabel",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub expected_label: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkRemoveInput {
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkRemoveResult {
    #[serde(rename = "removed")]
    pub removed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkErrorReason {
    #[serde(rename = "unreachable")]
    Unreachable,
    #[serde(rename = "not_a_t3_environment")]
    NotAT3Environment,
    #[serde(rename = "incompatible")]
    Incompatible,
    #[serde(rename = "self")]
    SelfLink,
    #[serde(rename = "pairing_rejected")]
    PairingRejected,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "unknown_link")]
    UnknownLink,
    #[serde(rename = "storage")]
    Storage,
    #[serde(rename = "disabled")]
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkErrorTag {
    #[serde(rename = "PeerLinkError")]
    PeerLinkError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkError {
    #[serde(rename = "_tag")]
    pub _tag: PeerLinkErrorTag,
    #[serde(rename = "reason")]
    pub reason: PeerLinkErrorReason,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cause: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkAnswerLinkType {
    #[serde(rename = "link")]
    Link,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkAnswerLink {
    #[serde(rename = "type")]
    pub r#type: PeerLinkAnswerLinkType,
    #[serde(
        rename = "environmentId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub environment_id: Option<EnvironmentId>,
    #[serde(
        rename = "label",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub label: Option<TrimmedNonEmptyString>,
    #[serde(rename = "urls")]
    pub urls: Vec<TrimmedNonEmptyString>,
    #[serde(rename = "access")]
    pub access: AuthMcpClientAccess,
    #[serde(rename = "pairingCode")]
    pub pairing_code: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkAnswerUseExistingType {
    #[serde(rename = "use-existing")]
    UseExisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkAnswerUseExisting {
    #[serde(rename = "type")]
    pub r#type: PeerLinkAnswerUseExistingType,
    #[serde(rename = "environmentId")]
    pub environment_id: EnvironmentId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkAnswerDeclineType {
    #[serde(rename = "decline")]
    Decline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkAnswerDecline {
    #[serde(rename = "type")]
    pub r#type: PeerLinkAnswerDeclineType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PeerLinkAnswer {
    PeerLinkAnswerLink(PeerLinkAnswerLink),
    PeerLinkAnswerUseExisting(PeerLinkAnswerUseExisting),
    PeerLinkAnswerDecline(PeerLinkAnswerDecline),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkRequestAnswerInput {
    #[serde(rename = "threadId")]
    pub thread_id: ThreadId,
    #[serde(rename = "turnItemId")]
    pub turn_item_id: TurnItemId,
    #[serde(rename = "answer")]
    pub answer: PeerLinkAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkRequestFailureReason {
    #[serde(rename = "load_failed")]
    LoadFailed,
    #[serde(rename = "not_found")]
    NotFound,
    #[serde(rename = "already_answered")]
    AlreadyAnswered,
    #[serde(rename = "record_failed")]
    RecordFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerLinkRequestErrorTag {
    #[serde(rename = "PeerLinkRequestError")]
    PeerLinkRequestError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLinkRequestError {
    #[serde(rename = "_tag")]
    pub _tag: PeerLinkRequestErrorTag,
    #[serde(rename = "reason")]
    pub reason: PeerLinkRequestFailureReason,
    #[serde(
        rename = "cause",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cause: Option<serde_json::Value>,
}

crate::redacted_debug!(PeerLinkCreateInput { url, alternate_urls, access, expected_environment_id, expected_label } redact { pairing_code });
crate::redacted_debug!(PeerLinkAnswerLink { r#type, environment_id, label, urls, access } redact { pairing_code });
