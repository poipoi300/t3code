use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrchestrationDispatchCommandErrorTag {
    #[serde(rename = "OrchestrationDispatchCommandError")]
    OrchestrationDispatchCommandError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapThreadDisposition {
    #[serde(rename = "deleted")]
    Deleted,
    #[serde(rename = "not-created")]
    NotCreated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationDispatchCommandError {
    #[serde(rename = "_tag")]
    pub _tag: OrchestrationDispatchCommandErrorTag,
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub bootstrap_thread_disposition: Option<Option<BootstrapThreadDisposition>>,
}
