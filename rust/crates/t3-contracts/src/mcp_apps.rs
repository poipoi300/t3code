use crate::base_schemas::TrimmedNonEmptyString;
use crate::base_schemas::{ThreadId, TurnItemId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppCallToolInput {
    pub thread_id: ThreadId,
    pub item_id: TurnItemId,
    pub name: TrimmedNonEmptyString,
    pub arguments: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppCallToolResult {
    pub content: Vec<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub structured_content: Option<Option<Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_error: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    #[serde(rename = "_meta")]
    pub _meta: Option<Option<Value>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppToolInfoInput {
    pub thread_id: ThreadId,
    pub item_id: TurnItemId,
    pub name: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppToolInfo {
    pub callable: bool,
    pub read_only: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub tool: Option<Option<Value>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppReadResourceInput {
    pub thread_id: ThreadId,
    pub item_id: TurnItemId,
    pub uri: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppReadResourceResult {
    pub contents: Vec<Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppUpdateModelContextInput {
    pub thread_id: ThreadId,
    pub item_id: TurnItemId,
    pub conversation_thread_id: ThreadId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub content: Option<Option<Vec<Value>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub structured_content: Option<Option<BTreeMap<String, Value>>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum McpAppRequestErrorReason {
    #[serde(rename = "not-an-app")]
    NotAnApp,
    #[serde(rename = "provider-unsupported")]
    ProviderUnsupported,
    #[serde(rename = "session-stopped")]
    SessionStopped,
    #[serde(rename = "tool-not-callable")]
    ToolNotCallable,
    #[serde(rename = "unsupported-content")]
    UnsupportedContent,
    #[serde(rename = "request-failed")]
    RequestFailed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum McpAppRequestErrorTag {
    #[serde(rename = "McpAppRequestError")]
    Error,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAppRequestError {
    #[serde(rename = "_tag")]
    pub _tag: McpAppRequestErrorTag,
    pub thread_id: ThreadId,
    pub reason: McpAppRequestErrorReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<Value>>,
}
