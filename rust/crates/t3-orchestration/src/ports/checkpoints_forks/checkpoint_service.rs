use super::checkpoint_restore_safety::{canonicalize_missing, expanded_id};
use crate::{
    api::*,
    error::{Error, Result},
};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct WorkspaceLocks(Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>);

impl WorkspaceLocks {
    pub fn workspace_leases() -> Arc<Self> {
        static LEASES: std::sync::OnceLock<Arc<WorkspaceLocks>> = std::sync::OnceLock::new();
        LEASES.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub async fn acquire(&self, cwd: &Path) -> Result<OwnedMutexGuard<()>> {
        let key = canonicalize_missing(cwd)?;
        let lock = {
            let mut locks = self.0.lock().unwrap_or_else(|e| e.into_inner());
            locks.retain(|_, value| value.strong_count() > 0);
            if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                locks.insert(key, Arc::downgrade(&lock));
                lock
            }
        };
        Ok(lock.lock_owned().await)
    }
}

pub fn encode_id_part(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

pub fn root_scope_id(thread: &ThreadId) -> CheckpointScopeId {
    CheckpointScopeId(format!(
        "checkpoint-scope:thread:{}:name:root",
        encode_id_part(&thread.0)
    ))
}

pub fn checkpoint_id(scope: &CheckpointScopeId, ordinal: u64) -> CheckpointId {
    CheckpointId(format!(
        "checkpoint:scope:{}:name:{ordinal}",
        encode_id_part(&scope.0)
    ))
}

pub struct PrepareRootScope {
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub root_node_id: NodeId,
    pub provider_thread_id: ProviderThreadId,
    pub cwd: PathBuf,
    pub created_at: IsoDateTime,
}

pub fn prepare_root_run_scope(input: PrepareRootScope) -> OrchestrationV2CheckpointScope {
    OrchestrationV2CheckpointScope {
        id: IdOrIndex::Id(root_scope_id(&input.thread_id)),
        thread_id: IdOrIndex::Id(input.thread_id),
        run_id: Some(IdOrIndex::Id(input.run_id)),
        node_id: IdOrIndex::Id(input.root_node_id),
        parent_scope_id: None,
        provider_thread_id: Some(IdOrIndex::Id(input.provider_thread_id)),
        kind: CheckpointScopeKind::RootRun,
        ordinal_within_parent: 0,
        advances_app_run_count: true,
        cwd: TrimmedNonEmptyString(input.cwd.to_string_lossy().into_owned()),
        created_at: input.created_at,
    }
}

pub struct Capture {
    pub scope: OrchestrationV2CheckpointScope,
    pub run_id: Option<RunId>,
    pub node_id: NodeId,
    pub ordinal: u64,
    pub app_run_ordinal: Option<u64>,
    pub captured_at: IsoDateTime,
    pub force_missing: bool,
}

// This fills low-level operations absent from the frozen CheckpointBackend seam.
#[async_trait]
pub trait CheckpointMetadata: Send + Sync {
    async fn query_diff(
        &self,
        _cwd: PathBuf,
        _from: CheckpointRef,
        _to: CheckpointRef,
        _ignore_whitespace: bool,
    ) -> Result<String> {
        Err(Error::Unsupported("CheckpointMetadata.query_diff"))
    }
    fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef;
    async fn is_repository(&self, cwd: PathBuf) -> Result<bool>;
    async fn delete_refs(
        &self,
        scope: OrchestrationV2CheckpointScope,
        checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()>;
}

pub struct CheckpointService {
    backend: Arc<dyn CheckpointBackend>,
    metadata: Arc<dyn CheckpointMetadata>,
    pub locks: Arc<WorkspaceLocks>,
}

impl CheckpointService {
    pub fn validate_owned_checkpoint(
        &self,
        scope: &OrchestrationV2CheckpointScope,
        checkpoint: &OrchestrationV2Checkpoint,
    ) -> Result<()> {
        if checkpoint.scope_id != scope.id
            || checkpoint.thread_id != scope.thread_id
            || checkpoint.r#ref
                != self
                    .metadata
                    .reference(&expanded_id(&scope.id)?, checkpoint.ordinal_within_scope)
        {
            return Err(Error::Service {
                operation: "checkpoint.scope-ownership",
                detail: "Checkpoint is not owned by this scope".into(),
            });
        }
        Ok(())
    }

    /// Snapshot locks must be distinct from workspace leases, which rollback already holds during restore.
    pub fn new(
        backend: Arc<dyn CheckpointBackend>,
        metadata: Arc<dyn CheckpointMetadata>,
        locks: Arc<WorkspaceLocks>,
    ) -> Self {
        Self {
            backend,
            metadata,
            locks,
        }
    }

    pub async fn capture_baseline(
        &self,
        scope: OrchestrationV2CheckpointScope,
        ordinal: u64,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let _lock = self.locks.acquire(Path::new(&scope.cwd.0)).await?;
        if !self
            .metadata
            .is_repository(PathBuf::from(&scope.cwd.0))
            .await?
        {
            return Ok(());
        }
        if self.backend.lookup(scope.clone(), ordinal).await?.is_some() {
            return Ok(());
        }
        let child = cancellation.child_token();
        let capture = self.backend.capture(
            CaptureCheckpoint {
                scope,
                ordinal,
                message: String::new(),
            },
            child.clone(),
        );
        tokio::pin!(capture);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => { child.cancel(); let _ = capture.await; Err(Error::Cancelled) },
            result = &mut capture => match result { Ok(_) | Err(Error::Unsupported(_)) => Ok(()), Err(error) => Err(error) },
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                child.cancel();
                let _ = capture.await;
                Err(Error::Service { operation: "checkpoint.baseline", detail: "Baseline capture exceeded its five-second snapshot budget".into() })
            }
        }
    }

    fn record(
        &self,
        input: &Capture,
        status: CheckpointStatus,
        files: Vec<OrchestrationV2CheckpointFileSummary>,
    ) -> Result<OrchestrationV2Checkpoint> {
        let scope_id = expanded_id(&input.scope.id)?;
        Ok(OrchestrationV2Checkpoint {
            id: IdOrIndex::Id(checkpoint_id(&scope_id, input.ordinal)),
            thread_id: input.scope.thread_id.clone(),
            scope_id: input.scope.id.clone(),
            run_id: input.run_id.clone().map(IdOrIndex::Id),
            node_id: IdOrIndex::Id(input.node_id.clone()),
            parent_checkpoint_id: (input.ordinal > 0)
                .then(|| IdOrIndex::Id(checkpoint_id(&scope_id, input.ordinal - 1))),
            ordinal_within_scope: input.ordinal,
            app_run_ordinal: input.app_run_ordinal,
            r#ref: self.metadata.reference(&scope_id, input.ordinal),
            status,
            files,
            files_total: None,
            captured_at: input.captured_at.clone(),
        })
    }

    pub async fn materialize_baseline(
        &self,
        mut input: Capture,
    ) -> Result<OrchestrationV2Checkpoint> {
        let _lock = self.locks.acquire(Path::new(&input.scope.cwd.0)).await?;
        input.run_id = None;
        input.app_run_ordinal = None;
        let status = if input.force_missing {
            CheckpointStatus::Missing
        } else {
            match self
                .metadata
                .is_repository(PathBuf::from(&input.scope.cwd.0))
                .await
            {
                Ok(false) => CheckpointStatus::Missing,
                Err(_) => CheckpointStatus::Error,
                Ok(true) => match self
                    .backend
                    .lookup(input.scope.clone(), input.ordinal)
                    .await
                {
                    Ok(Some(_)) => CheckpointStatus::Ready,
                    Ok(None) => CheckpointStatus::Missing,
                    Err(_) => CheckpointStatus::Error,
                },
            }
        };
        let mut checkpoint = self.record(&input, status, vec![])?;
        checkpoint.parent_checkpoint_id = None;
        Ok(checkpoint)
    }

    pub async fn capture(
        &self,
        input: Capture,
        cancellation: CancellationToken,
    ) -> Result<OrchestrationV2Checkpoint> {
        let _lock = self.locks.acquire(Path::new(&input.scope.cwd.0)).await?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let repository = if input.force_missing {
            Ok(false)
        } else {
            self.metadata
                .is_repository(PathBuf::from(&input.scope.cwd.0))
                .await
        };
        let status = match repository {
            Ok(false) => CheckpointStatus::Missing,
            Err(_) => CheckpointStatus::Error,
            Ok(true) => match self
                .backend
                .capture(
                    CaptureCheckpoint {
                        scope: input.scope.clone(),
                        ordinal: input.ordinal,
                        message: String::new(),
                    },
                    cancellation.clone(),
                )
                .await
            {
                Ok(_) => CheckpointStatus::Ready,
                Err(Error::Unsupported(_)) => CheckpointStatus::Missing,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => CheckpointStatus::Error,
            },
        };
        let mut files = Vec::new();
        if status == CheckpointStatus::Ready {
            let previous = input.ordinal.saturating_sub(1);
            if let Ok(Some(from)) = self.backend.lookup(input.scope.clone(), previous).await {
                let to = self
                    .metadata
                    .reference(&expanded_id(&input.scope.id)?, input.ordinal);
                if let Ok(diff) = self
                    .backend
                    .diff(PathBuf::from(&input.scope.cwd.0), from, to)
                    .await
                {
                    files = diff.files;
                }
            }
        }
        self.record(&input, status, files)
    }

    pub async fn restore(
        &self,
        scope: OrchestrationV2CheckpointScope,
        checkpoint: &OrchestrationV2Checkpoint,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let _lock = self.locks.acquire(Path::new(&scope.cwd.0)).await?;
        let reference = self
            .metadata
            .reference(&expanded_id(&scope.id)?, checkpoint.ordinal_within_scope);
        if checkpoint.status != CheckpointStatus::Ready
            || checkpoint.scope_id != scope.id
            || checkpoint.thread_id != scope.thread_id
            || checkpoint.r#ref != reference
        {
            return Err(Error::Service {
                operation: "checkpoint.restore",
                detail: "Checkpoint is not a ready snapshot owned by this scope".into(),
            });
        }
        self.backend
            .restore(scope, checkpoint.ordinal_within_scope, cancellation)
            .await
    }

    pub async fn delete_stale_refs(
        &self,
        scope: OrchestrationV2CheckpointScope,
        checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        let _lock = self.locks.acquire(Path::new(&scope.cwd.0)).await?;
        for checkpoint in &checkpoints {
            if checkpoint.scope_id != scope.id
                || checkpoint.thread_id != scope.thread_id
                || checkpoint.r#ref
                    != self
                        .metadata
                        .reference(&expanded_id(&scope.id)?, checkpoint.ordinal_within_scope)
            {
                return Err(Error::Service {
                    operation: "checkpoint.delete-stale-refs",
                    detail: "Checkpoint is owned by another scope".into(),
                });
            }
        }
        self.metadata.delete_refs(scope, checkpoints).await
    }
}
