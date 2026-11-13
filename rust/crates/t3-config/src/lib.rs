//! Native server configuration and compatible T3 home files. Not a wasm crate.

pub mod config;
mod defaults;
pub mod environment;
mod events;
mod files;
pub mod home;
pub mod keybindings;
pub mod policy;
pub mod runtime;
pub mod secrets;
pub mod settings;
mod settings_migrations;
mod watch;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{operation} failed at {path}: {source}")]
    Io {
        operation: &'static str,
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("invalid {context}: {detail}")]
    Invalid { context: String, detail: String },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Watch(#[from] notify::Error),
    #[error(transparent)]
    Persistence(#[from] t3_persistence::Error),
    #[error("blocking configuration task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(context: impl Into<String>, detail: impl Into<String>) -> Error {
    Error::Invalid {
        context: context.into(),
        detail: detail.into(),
    }
}
