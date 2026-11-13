use crate::{
    base_schemas::ThreadId,
    wire::{JsInteger, JsNumber, WireDateTime},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestrationThreadSearchSource {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationSearchThreadsInput {
    pub query: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub limit: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationThreadSearchMatch {
    pub thread_id: ThreadId,
    pub project_id: crate::base_schemas::ProjectId,
    pub source: OrchestrationThreadSearchSource,
    pub snippet: String,
    #[serde(with = "crate::wire::nullable")]
    pub message_created_at: Option<WireDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub quality: Option<JsNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub hits: Option<JsInteger>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub message_id: Option<crate::base_schemas::MessageId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationSearchThreadsResult {
    pub matches: Vec<OrchestrationThreadSearchMatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestrationSearchThreadsErrorTag {
    #[serde(rename = "OrchestrationSearchThreadsError")]
    OrchestrationSearchThreadsError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationSearchThreadsError {
    #[serde(rename = "_tag")]
    pub _tag: OrchestrationSearchThreadsErrorTag,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

pub const THREAD_SEARCH_QUERY_MAX_LENGTH: usize = 200;
