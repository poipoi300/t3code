use crate::{Error, FailureReason, PullRequestService, Result, service::SummaryOptions};
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    pull_request::{PullRequestRef, PullRequestState},
    thread_pull_request::{
        ThreadPullRequestKey, ThreadPullRequestLink, ThreadPullRequestLinkSource,
    },
};

#[derive(Debug, Clone, Default)]
pub struct Target {
    pub url: Option<String>,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub number: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub key: ThreadPullRequestKey,
    pub url: String,
}
pub struct WatchThread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub deleted: bool,
    pub subagent: bool,
    pub links: Vec<ThreadPullRequestLink>,
}
#[derive(Debug, Clone)]
pub struct WatchCommand {
    pub thread_id: ThreadId,
    pub target: ResolvedTarget,
    pub watching: bool,
}
#[async_trait]
pub trait WatchOrchestration: Send + Sync {
    async fn thread(&self, id: &ThreadId) -> Result<Option<WatchThread>>;
    async fn resolve_target(
        &self,
        target: Target,
        project_id: &ProjectId,
    ) -> Result<ResolvedTarget>;
    /// Dispatch must persist the watch and recheck thread/run admission before committing.
    async fn dispatch_watch(&self, command: WatchCommand) -> Result<()>;
}
pub struct WatchResult {
    pub target: ResolvedTarget,
    pub watching: bool,
    pub was_watching: bool,
}
pub struct PullRequestWatchService {
    orchestration: Arc<dyn WatchOrchestration>,
    pull_requests: PullRequestService,
}
fn linked<'a>(
    thread: &'a WatchThread,
    target: &ResolvedTarget,
) -> Option<&'a ThreadPullRequestLink> {
    thread.links.iter().find(|l| {
        l.source != ThreadPullRequestLinkSource::StackDismissed
            && l.number == target.key.number
            && l.host
                .as_str()
                .eq_ignore_ascii_case(target.key.host.as_str())
            && l.repository
                .as_str()
                .eq_ignore_ascii_case(target.key.repository.as_str())
    })
}
impl PullRequestWatchService {
    pub fn new(
        orchestration: Arc<dyn WatchOrchestration>,
        pull_requests: PullRequestService,
    ) -> Self {
        Self {
            orchestration,
            pull_requests,
        }
    }
    async fn thread(&self, id: &ThreadId) -> Result<WatchThread> {
        self.orchestration
            .thread(id)
            .await?
            .filter(|t| !t.deleted)
            .ok_or_else(|| Error {
                reason: FailureReason::NotFound,
                ..Error::invalid("watch", "Thread was not found.")
            })
    }
    pub async fn set_watching(
        &self,
        thread_id: ThreadId,
        target: Target,
        watching: bool,
    ) -> Result<WatchResult> {
        let thread = self.thread(&thread_id).await?;
        let target = self
            .orchestration
            .resolve_target(target, &thread.project_id)
            .await?;
        if watching && thread.subagent {
            return Err(Error::invalid(
                "watch",
                "This thread is a subagent, so it cannot watch pull requests. Its parent thread owns the pull request: finish your task and report back instead.",
            ));
        }
        let before = linked(&thread, &target);
        let was_watching = before
            .and_then(|l| l.watch.as_ref().and_then(Option::as_ref))
            .is_some();
        if watching
            && before
                .and_then(|l| l.snapshot.as_ref())
                .is_some_and(|s| s.state == PullRequestState::Merged)
        {
            return Err(Error::invalid(
                "watch",
                "The pull request is merged, so there is nothing to watch.",
            ));
        }
        if watching && !was_watching {
            let reference: PullRequestRef = crate::convert(
                serde_json::json!({"projectId":thread.project_id,"host":target.key.host,"repository":target.key.repository,"number":target.key.number,"allowStale":false}),
            )?;
            match self
                .pull_requests
                .summary(
                    reference,
                    SummaryOptions {
                        bypass_cache: true,
                        recover_transient_failure: Some(false),
                    },
                )
                .await
            {
                Ok(summary) if summary.state == PullRequestState::Merged => {
                    return Err(Error::invalid(
                        "watch",
                        "The pull request is merged, so there is nothing to watch.",
                    ));
                }
                Err(e)
                    if matches!(
                        e.reason,
                        FailureReason::MissingTool
                            | FailureReason::Unauthenticated
                            | FailureReason::Unsupported
                    ) =>
                {
                    return Err(e);
                }
                _ => {}
            }
        }
        self.orchestration
            .dispatch_watch(WatchCommand {
                thread_id: thread_id.clone(),
                target: target.clone(),
                watching,
            })
            .await?;
        let after = self.thread(&thread_id).await?;
        let watching = linked(&after, &target)
            .and_then(|l| l.watch.as_ref().and_then(Option::as_ref))
            .is_some();
        Ok(WatchResult {
            target,
            watching,
            was_watching,
        })
    }
}
