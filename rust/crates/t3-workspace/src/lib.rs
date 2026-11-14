//! Native workspace, project filesystem, and signed asset services. Not a wasm32 dependency.

pub mod assets;
pub mod attachments;
pub mod contained_files;
pub mod entries;
pub mod filesystem;
pub mod ignore_patterns;
pub mod image;
pub mod leases;
pub mod native_icons;
pub mod paths;
pub mod project;
pub mod restore;
mod scoped;
pub mod scripts;
pub mod search;
mod symlinks;

use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("workspace root does not exist: {0}")]
    RootMissing(PathBuf),
    #[error("workspace root is not a directory: {0}")]
    RootNotDirectory(PathBuf),
    #[error("path must be relative to the workspace: {0}")]
    OutsideRoot(String),
    #[error("path resolves outside the workspace or into protected metadata: {0}")]
    PathEscape(PathBuf),
    #[error("not a registered workspace: {0}")]
    UnknownWorkspace(PathBuf),
    #[error("not a regular file: {0}")]
    NotFile(PathBuf),
    #[error("binary file cannot be previewed as text: {0}")]
    Binary(PathBuf),
    #[error("{operation} failed at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "Could not recreate branch '{branch}' at {path}: the project workspace has no commit to start it from."
    )]
    WorktreeRecreateNoBase { path: PathBuf, branch: String },
    #[error("Could not recreate the worktree for branch '{branch}' at {path}.")]
    WorktreeRecreate {
        path: PathBuf,
        branch: String,
        #[source]
        source: Box<Error>,
    },
    #[error(transparent)]
    Git(#[from] t3_git::GitError),
    #[error("search failed: {0}")]
    Search(String),
    #[error("content index scan did not finish within 15 seconds")]
    ScanTimeout,
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("asset not found or not authorized")]
    AssetDenied,
    #[error("attachment upload failed ({status}): {detail}")]
    Upload { status: u16, detail: String },
    #[error(transparent)]
    Process(#[from] t3_process::ProcessError),
    #[error(transparent)]
    Terminal(#[from] t3_terminal::TerminalError),
    #[error("blocking filesystem task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn io_error(
    operation: &'static str,
    path: impl Into<PathBuf>,
    source: io::Error,
) -> Error {
    Error::Io {
        operation,
        path: path.into(),
        source,
    }
}
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}
pub(crate) fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match icu_collator::Collator::try_new(Default::default(), Default::default()) {
        Ok(collator) => collator.compare(a, b),
        Err(_) => a.cmp(b),
    }
}
