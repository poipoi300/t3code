//! Native project lifecycle services; does not target wasm32-unknown-unknown.

pub mod clones;
pub mod enrichment;
pub mod identity;
#[cfg(feature = "session-inventory")]
pub mod import_store;
#[cfg(feature = "session-inventory")]
pub mod inventory;
pub mod managed;
mod projection;
pub mod promotion;
mod redaction;
pub mod scripts;
pub mod service;
pub mod session_json;
pub mod sessions;
pub mod setup;
pub mod transcript;

pub use service::{ProjectService, ProjectServices};

pub(crate) fn clamp_text(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.into();
    }
    let mut used = 0;
    let mut out = String::new();
    for c in text.chars() {
        if used + c.len_utf16() >= max {
            break;
        }
        used += c.len_utf16();
        out.push(c);
    }
    out.push('…');
    out
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Project {0} was not found.")]
    NotFound(String),
    #[error("Workspace {workspace_root} already belongs to project {conflicting_project_id}.")]
    Conflict {
        workspace_root: String,
        conflicting_project_id: String,
    },
    #[error("Project {0} is not empty.")]
    NotEmpty(String),
    #[error("{0}")]
    Invalid(String),
    #[error("Required backend is unavailable: {0}")]
    Unavailable(&'static str),
    #[error(transparent)]
    Orchestration(#[from] t3_orchestration::error::Error),
    #[error(transparent)]
    Workspace(#[from] t3_workspace::Error),
    #[error(transparent)]
    Git(#[from] t3_git::GitError),
    #[error(transparent)]
    Config(#[from] t3_config::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Project operation was cancelled")]
    Cancelled,
    #[error("{operation}: {detail}")]
    Operation {
        operation: &'static str,
        detail: String,
    },
    #[error("Project task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}
