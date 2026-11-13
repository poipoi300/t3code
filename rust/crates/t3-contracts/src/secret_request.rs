use crate::{
    base_schemas::{ThreadId, TurnItemId},
    secret_reference::SecretReference,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SecretRequestAnswer {
    Save { secret: String },
    Decline,
    Reference { reference: SecretReference },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequestAnswerInput {
    pub thread_id: ThreadId,
    pub turn_item_id: TurnItemId,
    pub answer: SecretRequestAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretRequestFailureReason {
    #[serde(rename = "load_failed")]
    LoadFailed,
    #[serde(rename = "not_found")]
    NotFound,
    #[serde(rename = "already_answered")]
    AlreadyAnswered,
    #[serde(rename = "agent_stopped")]
    AgentStopped,
    #[serde(rename = "store_failed")]
    StoreFailed,
    #[serde(rename = "reference_unreadable")]
    ReferenceUnreadable,
    #[serde(rename = "record_failed")]
    RecordFailed,
    #[serde(rename = "invalid_ref")]
    InvalidRef,
    #[serde(rename = "read_failed")]
    ReadFailed,
    #[serde(rename = "ref_unavailable")]
    RefUnavailable,
    #[serde(rename = "ref_expired")]
    RefExpired,
    #[serde(rename = "consume_failed")]
    ConsumeFailed,
    #[serde(rename = "not_a_file_request")]
    NotAFileRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretRequestErrorTag {
    #[serde(rename = "SecretRequestError")]
    SecretRequestError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequestError {
    #[serde(rename = "_tag")]
    pub _tag: SecretRequestErrorTag,
    pub reason: SecretRequestFailureReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub provider: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub cause: Option<serde_json::Value>,
}

impl std::fmt::Debug for SecretRequestAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Save { .. } => f.write_str("Save { secret: [REDACTED] }"),
            Self::Decline => f.write_str("Decline"),
            Self::Reference { reference } => f
                .debug_struct("Reference")
                .field("reference", reference)
                .finish(),
        }
    }
}
