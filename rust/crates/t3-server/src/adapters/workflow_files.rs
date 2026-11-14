//! `orchestration.getWorkflowScript`'s file host: the runtime's `WorkflowFileBackend` over
//! `t3_workspace::contained_files` (open without following any link, identity from the opened
//! handle). Each call runs on a blocking thread.
//!
//! The root is TypeScript's `workflowFilesRoot(home)`: `<home>/.claude/projects`, where Claude
//! Code records workflow scripts. `getWorkflowScript` passes no `configDir`.
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_orchestration::{
    composition::WorkflowFiles,
    ports::runs_ingestion::workflow_file_read::{
        WorkflowFileBackend, WorkflowFileIdentity, WorkflowFileStat, WorkflowOpenFile,
    },
};
use t3_workspace::contained_files::{ContainedFile, ContainedFiles, FileIdentity};

/// `RuntimePorts::workflow_files` for the account home.
pub fn workflow_files(account_home: &Path) -> WorkflowFiles {
    let projects_root = account_home.join(".claude").join("projects");
    WorkflowFiles {
        backend: Arc::new(Backend(ContainedFiles::new(projects_root.clone()))),
        projects_root,
    }
}

struct Backend(ContainedFiles);

fn identity(identity: FileIdentity) -> WorkflowFileIdentity {
    WorkflowFileIdentity {
        device: identity.device,
        inode: identity.inode,
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(io::Error::other)?
}

#[async_trait::async_trait]
impl WorkflowFileBackend for Backend {
    async fn realpath(&self, path: &Path) -> io::Result<PathBuf> {
        let (files, path) = (self.0.clone(), path.to_owned());
        blocking(move || files.realpath(&path)).await
    }
    async fn open_nonblocking(&self, path: &Path) -> io::Result<Arc<dyn WorkflowOpenFile>> {
        let (files, path) = (self.0.clone(), path.to_owned());
        let file = blocking(move || files.open(&path)).await?;
        Ok(Arc::new(Open(Arc::new(file))))
    }
    async fn lstat_identity(&self, path: &Path) -> io::Result<WorkflowFileIdentity> {
        let (files, path) = (self.0.clone(), path.to_owned());
        blocking(move || files.lstat_identity(&path))
            .await
            .map(identity)
    }
}

struct Open(Arc<ContainedFile>);

#[async_trait::async_trait]
impl WorkflowOpenFile for Open {
    async fn stat(&self) -> io::Result<WorkflowFileStat> {
        let file = self.0.clone();
        let stat = blocking(move || file.stat()).await?;
        Ok(WorkflowFileStat {
            size: stat.size,
            identity: identity(stat.identity),
            regular: stat.regular,
        })
    }
    async fn read_at(&self, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let file = self.0.clone();
        blocking(move || file.read_at(offset, length)).await
    }
}
