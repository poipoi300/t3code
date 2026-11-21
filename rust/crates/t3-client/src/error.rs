use crate::platform::PlatformError;
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("RPC disconnected")]
    Disconnected,
    #[error("authorization denied for {method}")]
    PermissionDenied { method: String },
    #[error("{0}")]
    ServerPermissionDenied(String),
    #[error("unexpected environment identity")]
    EnvironmentMismatch,
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    #[error("script mismatch: {0}")]
    ScriptMismatch(String),
    #[error("cancelled")]
    Cancelled,
    #[error("unsupported client operation: {0}")]
    Unsupported(&'static str),
    /// The server refused the request with a typed error: its `message`, and `detail` when the
    /// server gave one.
    #[error("{}", rejection_text(message, detail.as_deref()))]
    Rejected {
        message: String,
        detail: Option<String>,
    },
    #[error(transparent)]
    Codec(#[from] serde_json::Error),
}

/// `"{message}: {detail}"`, or the message alone.
pub fn rejection_text(message: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => format!("{message}: {detail}"),
        None => message.to_owned(),
    }
}
