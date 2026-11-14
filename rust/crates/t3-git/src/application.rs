use crate::{
    CancellationToken, GitError, GitWorkflowService, SetupTask, VcsBranchSubscription,
    VcsStatusBroadcaster, VcsStatusSubscription, WorktreeSetupSubscription, WorktreeSetupTracker,
};
use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use t3_contracts::{git::*, worktree_setup::*};
use tokio::sync::{mpsc, watch};

#[async_trait]
pub trait GitActionEffects: Send + Sync {
    /// Link newly created PRs and refresh pushed PRs through orchestration/project services.
    async fn after_action(
        &self,
        input: &GitRunStackedActionInput,
        result: &GitRunStackedActionResult,
    );
}

#[derive(Clone)]
pub struct GitApplicationService {
    workflow: GitWorkflowService,
    broadcaster: VcsStatusBroadcaster,
    tracker: WorktreeSetupTracker,
    effects: Option<Arc<dyn GitActionEffects>>,
    factory: Option<Arc<dyn crate::GitWorkflowFactory>>,
}
impl GitApplicationService {
    pub fn new(
        workflow: GitWorkflowService,
        broadcaster: VcsStatusBroadcaster,
        tracker: WorktreeSetupTracker,
    ) -> Self {
        Self {
            workflow,
            broadcaster,
            tracker,
            effects: None,
            factory: None,
        }
    }
    pub fn with_action_effects(mut self, effects: Arc<dyn GitActionEffects>) -> Self {
        self.effects = Some(effects);
        self
    }
    pub fn with_workflow_factory(mut self, factory: Arc<dyn crate::GitWorkflowFactory>) -> Self {
        self.factory = Some(factory);
        self
    }
    async fn workflow(&self, cwd: &Path) -> Result<GitWorkflowService, GitError> {
        match &self.factory {
            Some(factory) => factory.for_checkout(cwd).await,
            None => Ok(self.workflow.at(cwd)),
        }
    }
    pub async fn subscribe_vcs_branch(
        &self,
        input: &VcsStatusInput,
    ) -> Result<VcsBranchSubscription, GitError> {
        self.broadcaster
            .stream_branch_status(Path::new(&input.cwd.0))
            .await
    }
    pub async fn refresh_branch(&self, input: &VcsStatusInput) -> Result<(), GitError> {
        self.broadcaster
            .refresh_branch_status(Path::new(&input.cwd.0))
            .await
            .map(|_| ())
    }
    pub async fn subscribe_vcs_status(
        &self,
        input: &VcsStatusSubscriptionInput,
    ) -> Result<VcsStatusSubscription, GitError> {
        self.broadcaster
            .stream_status(
                Path::new(&input.cwd.0),
                input.include_remote.flatten() != Some(false),
            )
            .await
    }
    pub async fn refresh_status(
        &self,
        input: &t3_contracts::rpc::WsVcsRefreshStatusPayload,
    ) -> Result<VcsStatusResult, GitError> {
        self.broadcaster
            .refresh_status(
                Path::new(&input.cwd.0),
                input.local_only.flatten() == Some(true),
            )
            .await
    }
    pub fn subscribe_worktree_setup(
        &self,
        input: &WorktreeSetupSubscribeInput,
    ) -> WorktreeSetupSubscription {
        self.tracker.stream(&input.thread_id)
    }
    pub async fn cancel_worktree_setup(
        &self,
        input: &WorktreeSetupCancelInput,
    ) -> WorktreeSetupCancelResult {
        WorktreeSetupCancelResult {
            cancelled: self.tracker.cancel(&input.thread_id).await,
        }
    }
    pub async fn create_worktree(
        &self,
        input: &VcsCreateWorktreeInput,
    ) -> Result<VcsCreateWorktreeResult, GitError> {
        let result = self
            .workflow(Path::new(&input.cwd.0))
            .await?
            .create_worktree(input)
            .await?;
        let _ = self
            .broadcaster
            .refresh_status(Path::new(&input.cwd.0), false)
            .await;
        Ok(result)
    }
    pub async fn resolve_pull_request(
        &self,
        input: &GitPullRequestRefInput,
    ) -> Result<GitResolvePullRequestResult, GitError> {
        self.workflow(Path::new(&input.cwd.0))
            .await?
            .resolve_pull_request(&input.reference.0)
            .await
    }
    pub async fn prepare_pull_request_thread(
        &self,
        input: &GitPreparePullRequestThreadInput,
    ) -> Result<GitPreparePullRequestThreadResult, GitError> {
        let result = self
            .workflow(Path::new(&input.cwd.0))
            .await?
            .prepare_pull_request_thread(input)
            .await?;
        let _ = self
            .broadcaster
            .refresh_status(Path::new(&input.cwd.0), false)
            .await;
        Ok(result)
    }
    pub fn run_stacked_action(&self, input: GitRunStackedActionInput) -> GitActionSubscription {
        let (sender, receiver) = mpsc::unbounded_channel();
        let service = self.clone();
        let (task, handle) = SetupTask::spawn(move |cancellation| async move {
            let workflow = tokio::select! { _ = cancellation.cancelled() => return, result = service.workflow(Path::new(&input.cwd.0)) => result };
            let workflow = match workflow {
                Ok(workflow) => workflow.with_cancellation(cancellation.clone()),
                Err(error) => {
                    let _ = sender.send(Err(error));
                    return;
                }
            };
            let progress = sender.clone();
            let result = workflow
                .run_stacked_action(
                    &input,
                    None,
                    Some(Arc::new(move |event| {
                        let _ = progress.send(Ok(event));
                    })),
                )
                .await;
            if !cancellation.is_cancelled() {
                if let Ok(result) = &result
                    && let Some(effects) = &service.effects
                {
                    tokio::select! { _ = cancellation.cancelled() => {}, _ = effects.after_action(&input, result) => {} }
                }
                tokio::select! { _ = cancellation.cancelled() => {}, _ = service.broadcaster.refresh_status(Path::new(&input.cwd.0), false) => {} }
                if let Err(error) = result {
                    let _ = sender.send(Err(error));
                }
            }
            workflow.git().drain_operations().await;
        });
        let (done, finished) = watch::channel(false);
        tokio::spawn(async move {
            let _ = handle.await;
            done.send_replace(true);
        });
        GitActionSubscription {
            receiver,
            cancellation: task.cancellation().clone(),
            finished,
        }
    }
}
pub struct GitActionSubscription {
    receiver: mpsc::UnboundedReceiver<Result<GitActionProgressEvent, GitError>>,
    cancellation: CancellationToken,
    finished: watch::Receiver<bool>,
}
impl GitActionSubscription {
    pub async fn recv(&mut self) -> Option<Result<GitActionProgressEvent, GitError>> {
        self.receiver.recv().await
    }
    pub async fn close(self) {
        self.cancellation.cancel();
        let _ = self.finished.clone().wait_for(|done| *done).await;
    }
}
impl Drop for GitActionSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
