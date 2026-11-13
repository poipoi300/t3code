use crate::{
    api::*,
    error::{Error, Result},
    ports::checkpoints_forks::checkpoint_service::CheckpointMetadata,
};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub struct NoRepositoryCheckpoints;

#[async_trait]
impl CheckpointBackend for NoRepositoryCheckpoints {
    async fn capture(
        &self,
        _input: CaptureCheckpoint,
        _cancellation: CancellationToken,
    ) -> Result<CheckpointRef> {
        Err(Error::Unsupported("capture outside a repository"))
    }
    async fn lookup(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _ordinal: u64,
    ) -> Result<Option<CheckpointRef>> {
        Ok(None)
    }
    async fn diff(
        &self,
        _cwd: PathBuf,
        _from: CheckpointRef,
        _to: CheckpointRef,
    ) -> Result<CheckpointDiff> {
        Err(Error::Unsupported("diff outside a repository"))
    }
    async fn restore(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _ordinal: u64,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(Error::Unsupported("restore outside a repository"))
    }
    async fn prune(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _preserve: Vec<u64>,
    ) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl CheckpointMetadata for NoRepositoryCheckpoints {
    fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
        CheckpointRef(format!("refs/t3/test/{}/{ordinal}", scope.0))
    }
    async fn is_repository(&self, _cwd: PathBuf) -> Result<bool> {
        Ok(false)
    }
    async fn delete_refs(
        &self,
        _scope: OrchestrationV2CheckpointScope,
        _checkpoints: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        Ok(())
    }
}
