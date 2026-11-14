use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason as Reason;
use t3_orchestration::ports::runs_ingestion::{
    workflow_agent_transcript::*, workflow_file_read::*,
};

pub fn root() -> PathBuf {
    std::env::temp_dir()
        .join("orch-c-workflow-fixture")
        .join("projects")
}
pub struct MemoryFile {
    pub bytes: Mutex<Vec<u8>>,
    pub identity: WorkflowFileIdentity,
    pub regular: bool,
}
#[async_trait]
impl WorkflowOpenFile for MemoryFile {
    async fn stat(&self) -> io::Result<WorkflowFileStat> {
        Ok(WorkflowFileStat {
            size: self.bytes.lock().expect("test value").len() as u64,
            identity: self.identity.clone(),
            regular: self.regular,
        })
    }
    async fn read_at(&self, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let bytes = self.bytes.lock().expect("test value");
        Ok(bytes
            .iter()
            .skip(offset as usize)
            .take(length)
            .copied()
            .collect())
    }
}
#[derive(Default)]
pub struct MemoryFiles {
    pub files: Mutex<HashMap<PathBuf, Arc<MemoryFile>>>,
    pub aliases: Mutex<HashMap<PathBuf, PathBuf>>,
    pub swapped: Mutex<Option<WorkflowFileIdentity>>,
}
impl MemoryFiles {
    pub fn put(&self, path: PathBuf, bytes: Vec<u8>, inode: u64, regular: bool) -> Arc<MemoryFile> {
        let file = Arc::new(MemoryFile {
            bytes: Mutex::new(bytes),
            identity: WorkflowFileIdentity { device: 1, inode },
            regular,
        });
        self.files
            .lock()
            .expect("test value")
            .insert(path, file.clone());
        file
    }
}
#[async_trait]
impl WorkflowFileBackend for MemoryFiles {
    async fn realpath(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(self
            .aliases
            .lock()
            .expect("test value")
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_owned()))
    }
    async fn open_nonblocking(&self, path: &Path) -> io::Result<Arc<dyn WorkflowOpenFile>> {
        self.files
            .lock()
            .expect("test value")
            .get(path)
            .cloned()
            .map(|file| file as Arc<dyn WorkflowOpenFile>)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    async fn lstat_identity(&self, path: &Path) -> io::Result<WorkflowFileIdentity> {
        if let Some(identity) = self.swapped.lock().expect("test value").clone() {
            return Ok(identity);
        }
        Ok(self
            .files
            .lock()
            .expect("test value")
            .get(path)
            .expect("test value")
            .identity
            .clone())
    }
}
#[tokio::test]
async fn transcript_defers_partial_lines_and_resets_after_replacement_or_shrinkage() {
    let backend = MemoryFiles::default();
    let directory = root().join("test");
    let path = directory.join("agent-worker.jsonl");
    let first = json!({"type":"assistant","uuid":"a","message":{}}).to_string() + "\n";
    let second = json!({"type":"user","uuid":"b","message":{}}).to_string() + "\n";
    let file = backend.put(
        path.clone(),
        format!("{first}{}", &second[..10]).into_bytes(),
        1,
        true,
    );
    let read = read_workflow_agent_transcript(
        &backend,
        &root(),
        &directory,
        "worker",
        &WorkflowCursor::default(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(read.lines.len(), 1);
    assert_eq!(read.cursor.offset, first.len() as u64);
    assert!(!read.truncated);
    file.bytes
        .lock()
        .unwrap()
        .extend_from_slice(&second.as_bytes()[10..]);
    let appended =
        read_workflow_agent_transcript(&backend, &root(), &directory, "worker", &read.cursor, None)
            .await
            .unwrap();
    assert_eq!(appended.lines[0]["uuid"], "b");
    backend.put(path.clone(), first.as_bytes().to_vec(), 2, true);
    let replaced = read_workflow_agent_transcript(
        &backend,
        &root(),
        &directory,
        "worker",
        &appended.cursor,
        None,
    )
    .await
    .unwrap();
    assert_eq!(replaced.lines[0]["uuid"], "a");
    assert_ne!(replaced.cursor.identity, appended.cursor.identity);
    backend.put(path, second.as_bytes().to_vec(), 2, true);
    let shrunk = read_workflow_agent_transcript(
        &backend,
        &root(),
        &directory,
        "worker",
        &WorkflowCursor {
            offset: 999,
            identity: replaced.cursor.identity,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(shrunk.lines[0]["uuid"], "b");
}
#[test]
fn transcript_filters_bookkeeping_and_requires_stable_ids() {
    let lines = [
        json!({"type":"attachment","uuid":"attachment"}),
        json!({"type":"user","isMeta":true,"uuid":"meta"}),
        json!({"type":"assistant","uuid":" "}),
        json!({"type":"assistant","uuid":"valid","isApiErrorMessage":true}),
    ];
    let contents = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n{invalid}\n";
    let parsed = parse_workflow_agent_transcript_lines(&contents);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["uuid"], "valid");
}
#[tokio::test]
async fn over_cap_lines_advance_cursor_without_publishing_partial_json_and_leaf_swaps_fail() {
    let backend = MemoryFiles::default();
    let path = root().join("large.jsonl");
    backend.put(path.clone(), b"oversized line".to_vec(), 1, true);
    let read = read_contained_workflow_file(
        &backend,
        &root(),
        WorkflowReadInput {
            path: &path,
            extension: ".jsonl",
            byte_cap: 5,
            tail: false,
            from_offset: Some(&WorkflowCursor::default()),
        },
    )
    .await
    .unwrap();
    assert_eq!(read.end_offset, 5);
    assert_eq!(read.contents, "");
    assert!(read.truncated);
    *backend.swapped.lock().unwrap() = Some(WorkflowFileIdentity {
        device: 1,
        inode: 2,
    });
    let result = read_contained_workflow_file(
        &backend,
        &root(),
        WorkflowReadInput {
            path: &path,
            extension: ".jsonl",
            byte_cap: 5,
            tail: false,
            from_offset: None,
        },
    )
    .await;
    assert_eq!(result.err().unwrap().0.reason, Reason::ChangedDuringRead);
}
