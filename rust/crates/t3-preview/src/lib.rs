//! Native preview sessions, browser automation and HTML publication; not a wasm crate.

pub mod activity;
pub mod browser;
pub mod control;
pub mod desktop;
pub mod discovery;
pub mod driver;
pub mod host;
pub mod html;
pub mod installer;
pub mod manager;
pub mod mcp;
pub mod profiles;
pub mod stream;
pub mod urls;

pub use browser::PreviewService;
pub use driver::{BrowserDriver, PlaywrightDriver};
pub use html::HtmlService;
pub use manager::PreviewManager;

pub type Result<T> = std::result::Result<T, Error>;
pub type Admission =
    std::sync::Arc<dyn Fn() -> futures_util::future::BoxFuture<'static, Result<()>> + Send + Sync>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Mcp(#[from] t3_mcp::McpError),
    #[error("{}", invalid_url_message(.reason, *.input_length, .protocol.as_deref()))]
    InvalidUrl {
        reason: String,
        input_length: usize,
        protocol: Option<String>,
    },
    #[error("The preview tab does not exist.")]
    TabNotFound,
    #[error("Server preview tabs require server control.")]
    ControlRequired,
    #[error("Browser control changed: {0:?}.")]
    Control(t3_contracts::preview_automation::PreviewAutomationControlReason),
    #[error("{message}")]
    Operation {
        tag: String,
        message: String,
        detail: Option<serde_json::Value>,
    },
    #[error("Preview operation timed out: {0}.")]
    Timeout(String),
    #[error("The browser driver disconnected.")]
    Disconnected,
    #[error("{0}")]
    Io(String),
    #[error("Invalid preview input: {0}.")]
    InvalidInput(String),
    #[error("Could not stop local server: {0:?}.")]
    Stop(t3_contracts::preview::PreviewStopLocalServerReason),
}
fn invalid_url_message(reason: &str, length: usize, protocol: Option<&str>) -> String {
    let suffix = protocol
        .map(|protocol| format!(": {protocol}"))
        .unwrap_or_default();
    format!(
        "Invalid preview URL ({reason}{suffix}; input length {length}). Use an http(s) URL or a schemeless host such as localhost:5173. With preview_open, omit url when creating a new tab to open a blank tab."
    )
}

impl Error {
    pub fn operation(tag: &str, message: impl Into<String>) -> Self {
        Self::Operation {
            tag: tag.into(),
            message: message.into(),
            detail: None,
        }
    }
    pub fn tag(&self) -> &str {
        match self {
            Self::Mcp(_) => "OrchestratorMcpFailure",
            Self::InvalidUrl { .. } => "PreviewInvalidUrlError",
            Self::TabNotFound => "PreviewAutomationTabNotFoundError",
            Self::ControlRequired => "PreviewControlRequiredError",
            Self::Control(_) => "PreviewAutomationControlInterruptedError",
            Self::Operation { tag, .. } => tag,
            Self::Timeout(_) => "PreviewAutomationTimeoutError",
            Self::Disconnected => "PreviewAutomationClientDisconnectedError",
            Self::Stop(_) => "PreviewStopLocalServerError",
            Self::Io(_) => "PreviewAutomationExecutionError",
            Self::InvalidInput(_) => "invalid_request",
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::InvalidInput(value.to_string())
    }
}
impl From<t3_process::ProcessError> for Error {
    fn from(value: t3_process::ProcessError) -> Self {
        Self::Io(value.to_string())
    }
}
pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
