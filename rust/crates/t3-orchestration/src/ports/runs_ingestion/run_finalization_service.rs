use crate::{
    api::*,
    error::{Error, Result},
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct FinalizationTarget {
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub scope_id: CheckpointScopeId,
}

#[async_trait]
pub trait FinalizationCapture: Send + Sync {
    async fn capture(
        &self,
        target: FinalizationTarget,
        cancellation: CancellationToken,
    ) -> Result<()>;
}

#[async_trait]
impl FinalizationCapture
    for crate::ports::checkpoints_forks::checkpoint_capture_service::CheckpointCaptureService
{
    async fn capture(
        &self,
        target: FinalizationTarget,
        cancellation: CancellationToken,
    ) -> Result<()> {
        self.execute(
            target.thread_id,
            target.run_id,
            target.scope_id,
            cancellation,
        )
        .await
    }
}

#[async_trait]
pub trait FinalizationObserver: Send + Sync {
    async fn refresh(&self, cwd: String, thread_id: ThreadId, run_id: RunId) -> Result<()>;
    async fn refresh_after_turn(&self, _project_id: ProjectId) -> Result<()> {
        Ok(())
    }
}

pub struct LocalWorkspaceStatus {
    pub reference: Option<String>,
    pub is_default: bool,
}
// seam-shim: host adapters bind workspace entries, VCS, pull requests and project enrichment.
#[async_trait]
pub trait PostTurnBackend: Send + Sync {
    async fn refresh_entries(&self, cwd: String) -> Result<()>;
    async fn refresh_local_status(&self, cwd: String) -> Result<LocalWorkspaceStatus>;
    async fn refresh_pull_request_status(&self, cwd: String) -> Result<()>;
    async fn refresh_after_turn(&self, project_id: ProjectId) -> Result<()>;
    async fn project_workspace_root(&self, project_id: ProjectId) -> Result<Option<String>>;
    async fn has_repository_identity(&self, workspace_root: String) -> Result<bool>;
    async fn refresh_repository_identity(&self, workspace_root: String) -> Result<()>;
}
pub struct WorkspaceFinalizationObserver {
    pub backend: Arc<dyn PostTurnBackend>,
    pub projections: Arc<dyn ProjectionReader>,
    tasks: tokio_util::task::TaskTracker,
    cancellation: CancellationToken,
    keyed: Arc<std::sync::Mutex<std::collections::HashMap<String, (u64, CancellationToken)>>>,
    generation: std::sync::atomic::AtomicU64,
}
impl Drop for WorkspaceFinalizationObserver {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.tasks.close();
    }
}
impl WorkspaceFinalizationObserver {
    pub fn new(backend: Arc<dyn PostTurnBackend>, projections: Arc<dyn ProjectionReader>) -> Self {
        Self {
            backend,
            projections,
            tasks: tokio_util::task::TaskTracker::new(),
            cancellation: CancellationToken::new(),
            keyed: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            generation: std::sync::atomic::AtomicU64::new(0),
        }
    }
    async fn replace(
        &self,
        key: String,
        future: impl std::future::Future<Output = Result<()>> + Send + 'static,
    ) {
        let token = self.cancellation.child_token();
        let generation = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some((_, previous)) = self
            .keyed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(key.clone(), (generation, token.clone()))
        {
            previous.cancel();
        }
        let keyed = self.keyed.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        self.tasks.spawn(async move {
            let mut future = Box::pin(future);
            let mut started = Some(started);
            let polled = std::future::poll_fn(|cx| {
                let result = future.as_mut().poll(cx);
                if let Some(started) = started.take() {
                    let _ = started.send(());
                }
                result
            });
            tokio::select! { biased; _=token.cancelled()=>{}, _=polled=>{} }
            let mut keyed = keyed.lock().unwrap_or_else(|error| error.into_inner());
            if keyed
                .get(&key)
                .is_some_and(|(current, _)| *current == generation)
            {
                keyed.remove(&key);
            }
        });
        let _ = ready.await;
    }
    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}
#[async_trait]
impl FinalizationObserver for WorkspaceFinalizationObserver {
    async fn refresh_after_turn(&self, project_id: ProjectId) -> Result<()> {
        self.backend.refresh_after_turn(project_id).await
    }
    async fn refresh(&self, cwd: String, thread_id: ThreadId, run_id: RunId) -> Result<()> {
        let (_, local) = tokio::try_join!(
            self.backend.refresh_entries(cwd.clone()),
            self.backend.refresh_local_status(cwd.clone())
        )?;
        let Some(thread) = self.projections.shell(thread_id).await? else {
            return Ok(());
        };
        let backend = self.backend.clone();
        let project = thread.project_id.clone();
        self.replace(format!("identity:{}", project.0), async move {
            let Some(root) = backend.project_workspace_root(project).await? else {
                return Ok(());
            };
            if !backend.has_repository_identity(root.clone()).await? {
                backend.refresh_repository_identity(root).await?;
            }
            Ok(())
        })
        .await;
        let Some(reference) = local.reference.filter(|_| !local.is_default) else {
            return Ok(());
        };
        if thread.branch.as_ref().map(|branch| branch.as_str()) != Some(reference.as_str())
            || thread
                .active_run_id
                .as_ref()
                .is_some_and(|active| active != &run_id)
        {
            return Ok(());
        }
        let backend = self.backend.clone();
        self.replace(format!("pull-request:{cwd}"), async move {
            backend.refresh_pull_request_status(cwd).await
        })
        .await;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("run finalization {operation} for {target:?}: {cause}")]
pub struct RunFinalizationError {
    pub target: FinalizationTarget,
    pub operation: &'static str,
    #[source]
    pub cause: Box<Error>,
}

pub struct RunFinalizationService {
    pub capture: Arc<dyn FinalizationCapture>,
    pub projections: Arc<dyn ProjectionReader>,
    pub observer: Arc<dyn FinalizationObserver>,
}

impl RunFinalizationService {
    pub async fn finalize(
        &self,
        target: FinalizationTarget,
        cancellation: CancellationToken,
    ) -> std::result::Result<(), RunFinalizationError> {
        self.capture
            .capture(target.clone(), cancellation)
            .await
            .map_err(|cause| RunFinalizationError {
                target: target.clone(),
                operation: "capture-checkpoint",
                cause: Box::new(cause),
            })?;
        let records = self
            .projections
            .records(
                target.thread_id.clone(),
                vec![RecordKind::CheckpointScopes],
                RecordFilter::default(),
            )
            .await
            .map_err(|cause| RunFinalizationError {
                target: target.clone(),
                operation: "refresh-workspace",
                cause: Box::new(cause),
            })?;
        for set in records.records {
            if let ProjectionRecordSet::CheckpointScopes(scopes) = set
                && let Some(scope) = scopes.into_iter().find(|scope| {
                    scope.id
                        == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                            target.scope_id.clone(),
                        )
                })
            {
                // Display refresh failures cannot block the next turn once capture is durable.
                let _ = self
                    .observer
                    .refresh(scope.cwd.0, target.thread_id.clone(), target.run_id.clone())
                    .await;
                break;
            }
        }
        Ok(())
    }
}
