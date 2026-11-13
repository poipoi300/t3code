use crate::platform::PlatformError;
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("RPC disconnected")]
    Disconnected,
    #[error("authorization denied for {method}")]
    PermissionDenied { method: String },
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
    #[error(transparent)]
    Codec(#[from] serde_json::Error),
}
