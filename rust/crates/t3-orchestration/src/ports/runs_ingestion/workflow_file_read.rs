use async_trait::async_trait;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::orchestration_v2::{
    OrchestrationWorkflowFileError, OrchestrationWorkflowFileErrorReason as Reason,
    OrchestrationWorkflowFileErrorTag,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowFileIdentity {
    pub device: u64,
    pub inode: u64,
}
#[derive(Debug, Clone, Default)]
pub struct WorkflowCursor {
    pub offset: u64,
    pub identity: Option<WorkflowFileIdentity>,
}
pub struct WorkflowFileStat {
    pub size: u64,
    pub identity: WorkflowFileIdentity,
    pub regular: bool,
}
#[async_trait]
pub trait WorkflowOpenFile: Send + Sync {
    async fn stat(&self) -> io::Result<WorkflowFileStat>;
    async fn read_at(&self, offset: u64, length: usize) -> io::Result<Vec<u8>>;
}
// seam-shim: t3-workspace host must open nonblocking and return descriptor-bound identity.
#[async_trait]
pub trait WorkflowFileBackend: Send + Sync {
    async fn realpath(&self, path: &Path) -> io::Result<PathBuf>;
    async fn open_nonblocking(&self, path: &Path) -> io::Result<Arc<dyn WorkflowOpenFile>>;
    async fn lstat_identity(&self, path: &Path) -> io::Result<WorkflowFileIdentity>;
}
#[derive(Debug)]
pub struct WorkflowReadError(pub OrchestrationWorkflowFileError);
impl std::fmt::Display for WorkflowReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Workflow file {:?}: {}", self.0.reason, self.0.path)
    }
}
impl std::error::Error for WorkflowReadError {}
impl WorkflowReadError {
    pub fn new(reason: Reason, path: &Path, cause: Option<io::Error>) -> Self {
        Self(OrchestrationWorkflowFileError {
            _tag: OrchestrationWorkflowFileErrorTag::OrchestrationWorkflowFileError,
            reason,
            path: path.to_string_lossy().into_owned(),
            cause: cause.map(|error| Some(serde_json::Value::String(error.to_string()))),
        })
    }
}
pub struct WorkflowReadInput<'a> {
    pub path: &'a Path,
    pub extension: &'a str,
    pub byte_cap: usize,
    pub tail: bool,
    pub from_offset: Option<&'a WorkflowCursor>,
}
pub struct WorkflowRead {
    pub path: PathBuf,
    pub contents: String,
    pub truncated: bool,
    pub start_offset: u64,
    pub end_offset: u64,
    pub identity: WorkflowFileIdentity,
}
fn extension_matches(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| format!(".{value}") == extension)
}
pub async fn read_contained_workflow_file(
    backend: &dyn WorkflowFileBackend,
    projects_root: &Path,
    input: WorkflowReadInput<'_>,
) -> std::result::Result<WorkflowRead, WorkflowReadError> {
    if !input.path.is_absolute() || !extension_matches(input.path, input.extension) {
        return Err(WorkflowReadError::new(
            Reason::InvalidPath,
            input.path,
            None,
        ));
    }
    let root = backend.realpath(projects_root).await.map_err(|cause| {
        WorkflowReadError::new(Reason::RootUnavailable, input.path, Some(cause))
    })?;
    let resolved = backend
        .realpath(input.path)
        .await
        .map_err(|cause| WorkflowReadError::new(Reason::NotFound, input.path, Some(cause)))?;
    if !resolved.starts_with(&root) {
        return Err(WorkflowReadError::new(Reason::OutsideRoot, &resolved, None));
    }
    if !extension_matches(&resolved, input.extension) {
        return Err(WorkflowReadError::new(
            Reason::WrongExtension,
            &resolved,
            None,
        ));
    }
    let failed = |cause| WorkflowReadError::new(Reason::ReadFailed, &resolved, Some(cause));
    let handle = backend.open_nonblocking(&resolved).await.map_err(failed)?;
    let stat = handle.stat().await.map_err(failed)?;
    if !stat.regular {
        return Err(WorkflowReadError::new(
            Reason::NotRegularFile,
            &resolved,
            None,
        ));
    }
    if backend.lstat_identity(&resolved).await.map_err(failed)? != stat.identity {
        return Err(WorkflowReadError::new(
            Reason::ChangedDuringRead,
            &resolved,
            None,
        ));
    }
    let (start, buffer, truncated, end) = if let Some(cursor) = input.from_offset {
        let start =
            if cursor.identity.as_ref() == Some(&stat.identity) && cursor.offset <= stat.size {
                cursor.offset
            } else {
                0
            };
        let length = (stat.size - start).min(input.byte_cap as u64) as usize;
        let bytes = handle.read_at(start, length).await.map_err(failed)?;
        let line_end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let consumed = if line_end == 0 && bytes.len() == input.byte_cap {
            bytes.len()
        } else {
            line_end
        };
        (
            start,
            bytes[..line_end].to_vec(),
            stat.size - start > input.byte_cap as u64,
            start + consumed as u64,
        )
    } else {
        let length = stat
            .size
            .min(input.byte_cap.saturating_add(usize::from(input.tail)) as u64)
            as usize;
        let start = if input.tail {
            stat.size - length as u64
        } else {
            0
        };
        let bytes = handle.read_at(start, length).await.map_err(failed)?;
        let end = start + bytes.len() as u64;
        (start, bytes, stat.size > length as u64, end)
    };
    Ok(WorkflowRead {
        path: resolved,
        contents: String::from_utf8_lossy(&buffer).into_owned(),
        truncated,
        start_offset: start,
        end_offset: end,
        identity: stat.identity,
    })
}
