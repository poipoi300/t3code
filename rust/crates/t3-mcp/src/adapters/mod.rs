#[cfg(feature = "workspace")]
pub mod attachments;
pub mod integrated;
#[cfg(feature = "orchestration")]
pub mod orchestration;
#[cfg(feature = "scheduler")]
pub mod scheduler;
#[cfg(feature = "git")]
pub mod worktree;
pub use integrated::ServiceAdapters;

pub fn not_available(tool: &str, method: &str) -> crate::McpError {
    crate::McpError::NotAvailable {
        tool: tool.into(),
        method: method.into(),
    }
}

#[cfg(any(
    feature = "orchestration",
    feature = "workspace",
    feature = "scheduler"
))]
pub(crate) fn encoded<T: serde::Serialize>(value: T) -> crate::Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|_| crate::McpError::Internal)
}
#[cfg(any(
    feature = "orchestration",
    feature = "workspace",
    feature = "scheduler"
))]
pub(crate) fn decoded<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> crate::Result<T> {
    serde_json::from_value(value).map_err(|error| crate::McpError::InvalidInput(error.to_string()))
}
