use crate::{
    CancellationToken, GitError, GitWorkflowService, SetupTask, VcsBranchSubscription,
    VcsStatusBroadcaster, VcsStatusSubscription, WorktreeSetupSubscription, WorktreeSetupTracker,
};
use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use t3_contracts::{git::*, worktree_setup::*};
use tokio::sync::watch;

#[derive(Default)]
struct ProgressBuffer {
    events: std::sync::Mutex<std::collections::VecDeque<Result<GitActionProgressEvent, GitError>>>,
    ready: tokio::sync::Notify,
}
impl ProgressBuffer {
    fn send(&self, event: Result<GitActionProgressEvent, GitError>) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if events.len() == 128 {
            // Keep lifecycle/terminal events when chatty hooks outrun the subscriber.
            if let Some(index) = events
                .iter()
                .position(|event| matches!(event, Ok(GitActionProgressEvent::HookOutput(_))))
            {
                events.remove(index);
            } else if matches!(&event, Ok(GitActionProgressEvent::HookOutput(_))) {
                return;
            } else {
                events.pop_front();
            }
        }
        events.push_back(event);
        self.ready.notify_one();
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    #[tokio::test]
    async fn slow_consumers_keep_bounded_output_and_terminal_events() {
        let queue = Arc::new(ProgressBuffer::default());
        queue.send(Ok(serde_json::from_value(serde_json::json!({"actionId":"test","cwd":"repo","action":"commit","kind":"action_started","phases":["commit"]})).unwrap()));
        for index in 0..4096 {
            queue.send(Ok(serde_json::from_value(serde_json::json!({"actionId":"test","cwd":"repo","action":"commit","kind":"hook_output","hookName":null,"stream":"stdout","text":format!("line {index}")})).unwrap()));
        }
        queue.send(Err(GitError::Unavailable("terminal".into())));
        {
            let events = queue.events.lock().unwrap();
            assert!(events.len() <= 128, "buffered {} events", events.len());
            assert!(matches!(
                events.front(),
                Some(Ok(GitActionProgressEvent::Started(_)))
            ));
            assert!(events.back().unwrap().is_err());
        }
        let (_done, finished) = watch::channel(true);
        let mut stream = GitActionSubscription {
            receiver: queue,
            cancellation: CancellationToken::new(),
            finished,
        };
        let mut terminal = false;
        let mut count = 0;
        while let Some(event) = stream.recv().await {
            count += 1;
            terminal = event.is_err();
        }
        assert_eq!(count, 128);
        assert!(terminal);
        stream.close().await;
    }
}

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
        let receiver = Arc::new(ProgressBuffer::default());
        let sender = receiver.clone();
        let service = self.clone();
        let (task, handle) = SetupTask::spawn(move |cancellation| async move {
            let workflow = tokio::select! { _ = cancellation.cancelled() => return, result = service.workflow(Path::new(&input.cwd.0)) => result };
            let workflow = match workflow {
                Ok(workflow) => workflow.with_cancellation(cancellation.clone()),
                Err(error) => {
                    sender.send(Err(error));
                    return;
                }
            };
            let progress = sender.clone();
            let result = workflow
                .run_stacked_action(
                    &input,
                    None,
                    Some(Arc::new(move |event| {
                        progress.send(Ok(event));
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
                    sender.send(Err(error));
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
    receiver: Arc<ProgressBuffer>,
    cancellation: CancellationToken,
    finished: watch::Receiver<bool>,
}
impl GitActionSubscription {
    pub async fn recv(&mut self) -> Option<Result<GitActionProgressEvent, GitError>> {
        loop {
            let ready = self.receiver.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            let finished = *self.finished.borrow_and_update();
            if let Some(event) = self
                .receiver
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop_front()
            {
                return Some(event);
            }
            if finished {
                return None;
            }
            tokio::select! {
                _ = ready => {},
                _ = self.finished.changed() => {},
            }
        }
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
