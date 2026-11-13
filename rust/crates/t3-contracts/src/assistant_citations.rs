use crate::base_schemas::{EnvironmentId, MessageId, NonNegativeInt, ThreadId};
use serde::{Deserialize, Serialize};

pub const ASSISTANT_CITATION_MAX_TEXT_LENGTH: usize = 8_000;
pub const ASSISTANT_CITATION_MAX_COMMENT_LENGTH: usize = 8_000;
pub const ASSISTANT_CITATION_CONTEXT_LENGTH: usize = 32;
pub const ASSISTANT_CITATION_MAX_ID_LENGTH: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantCitation {
    pub version: u8,
    pub environment_id: EnvironmentId,
    pub thread_id: ThreadId,
    pub message_id: MessageId,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub comment: Option<Option<String>>,
    pub start: NonNegativeInt,
    pub end: NonNegativeInt,
    pub prefix: String,
    pub suffix: String,
}
