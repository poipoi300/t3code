//! Native terminal service (tokio + portable-pty); not built for wasm32-unknown-unknown.
mod environment;
mod history;
mod manager;
mod process;
mod sanitizer;
pub mod sequence;
mod stream;
mod types;

pub use environment::{Platform, ShellCandidate, shell_candidates, spawn_environment};
pub use history::{
    BoundedTerminalHistory, HistorySanitizer, HistoryStore, retained_terminal_history,
};
pub use manager::{ManagerOptions, ProviderEnvironmentResolver, TerminalManager};
pub use stream::{AttachStream, EventStream, MetadataStream, OutputWindow};
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error(
        "Terminal folder does not exist: {cwd}. Restore the folder at this path before retrying."
    )]
    CwdNotFound { cwd: String },
    #[error("Terminal cwd is not a directory: {cwd}")]
    CwdNotDirectory { cwd: String },
    #[error("Failed to access terminal cwd: {cwd}: {source}")]
    CwdStat { cwd: String, source: std::io::Error },
    #[error("Unknown terminal thread: {thread_id}, terminal: {terminal_id}")]
    SessionLookup {
        thread_id: String,
        terminal_id: String,
    },
    #[error("Terminal is not running for thread: {thread_id}, terminal: {terminal_id}")]
    NotRunning {
        thread_id: String,
        terminal_id: String,
    },
    #[error("Provider instance is not available: {0}")]
    ProviderInstanceNotFound(String),
    #[error("Could not prepare the terminal environment for provider instance: {0}: {1}")]
    ProviderEnvironment(String, String),
    #[error(
        "Failed to {operation} terminal history for thread: {thread_id}, terminal: {terminal_id}: {source}"
    )]
    History {
        operation: &'static str,
        thread_id: String,
        terminal_id: String,
        source: std::io::Error,
    },
    #[error("PTY {operation} failed: {message}")]
    Pty {
        operation: &'static str,
        message: String,
        pid: Option<u32>,
        cols: Option<u16>,
        rows: Option<u16>,
        missing: bool,
    },
    #[error("Invalid terminal input: {0}")]
    InvalidInput(String),
    #[error("Terminal manager has stopped")]
    Stopped,
    #[error("Terminal worker failed: {0}")]
    Worker(String),
}

pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl TerminalError {
    fn context(mut self, process_id: u32, grid: Option<(u16, u16)>) -> Self {
        if let Self::Pty {
            pid, cols, rows, ..
        } = &mut self
        {
            *pid = Some(process_id);
            if let Some((c, r)) = grid {
                *cols = Some(c);
                *rows = Some(r);
            }
        }
        self
    }
}
