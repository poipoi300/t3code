use crate::{Error, FailureReason, PullRequestService, service::SummaryOptions};
use async_trait::async_trait;
use futures_util::StreamExt;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use t3_contracts::{
    pull_request::*,
    thread_pull_request::{ThreadPullRequestLink, ThreadPullRequestStack},
};
use t3_orchestration::{
    api::{PullRequestBackend, PullRequestWatchFingerprint},
    error::{Error as OrchestrationError, PullRequestHostError, Result as OrchestrationResult},
};
use tokio::sync::{Mutex, broadcast, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use t3_orchestration::ports::thread_project::thread_pull_request_service::{
    LinkedThreadsChange, LinkedThreadsChangeDetector, ThreadPullRequestService,
};

/// One detector per composed thread pull-request service, including its activity watermark.
pub fn linked_threads_detector() -> Arc<LinkedThreadsChangeDetector> {
    let detector = std::sync::Mutex::new(crate::linked::LinkedThreadsDetector::default());
    Arc::new(move |event| {
        match detector
            .lock()
            .expect("linked thread detector")
            .observe(event)
        {
            crate::linked::LinkedChange::Changed => LinkedThreadsChange::Changed,
            crate::linked::LinkedChange::Reordered => LinkedThreadsChange::Reordered,
            crate::linked::LinkedChange::None => LinkedThreadsChange::None,
        }
    })
}

/// The full thread port observes the same commits. Only install the fallback in its absence.
pub fn start_standalone_follower(
    composed: Option<&ThreadPullRequestService>,
    tasks: &TaskTracker,
    events: broadcast::Receiver<Arc<t3_contracts::application_event::ApplicationStoredEvent>>,
    stop: CancellationToken,
    notify: impl Fn() + Send + 'static,
) {
    if composed.is_none() {
        tasks.spawn(crate::linked::follow_changes(events, stop, notify));
    }
}

/// Checkout lookup must use the host's authoritative worktree/project and repository identity services.
#[async_trait]
pub trait CheckoutPullRequests: Send + Sync {
    async fn refresh(
        &self,
        cwd: PathBuf,
        links: Vec<ThreadPullRequestLink>,
    ) -> crate::Result<Vec<ThreadPullRequestLink>>;
    async fn discover(
        &self,
        cwd: PathBuf,
        branch: String,
    ) -> crate::Result<Vec<ThreadPullRequestLink>>;
}

struct Inner {
    service: PullRequestService,
    checkouts: Arc<dyn CheckoutPullRequests>,
    changes: Mutex<HashMap<String, watch::Sender<u64>>>,
    merges: Mutex<Option<broadcast::Sender<PullRequestRef>>>,
    cancel: CancellationToken,
    tasks: TaskTracker,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Clone)]
pub struct OrchestrationPullRequestBackend(Arc<Inner>);
impl OrchestrationPullRequestBackend {
    pub fn new(service: PullRequestService, checkouts: Arc<dyn CheckoutPullRequests>) -> Self {
        let cancel = CancellationToken::new();
        let tasks = TaskTracker::new();
        tasks.close();
        let merges = broadcast::channel(64).0;
        let mut events = service.subscribe_merges();
        let stop = cancel.clone();
        let output = merges.clone();
        tasks.spawn(async move{loop{tokio::select!{
            _ = stop.cancelled()=>break,
            event=events.recv()=>match event{Ok(event)=>{let _=output.send(event.reference);},Err(broadcast::error::RecvError::Lagged(_))=>{},Err(broadcast::error::RecvError::Closed)=>break}
        }}});
        Self(Arc::new(Inner {
            service,
            checkouts,
            changes: Mutex::new(HashMap::new()),
            merges: Mutex::new(Some(merges)),
            cancel,
            tasks,
        }))
    }
    fn admit(&self) -> OrchestrationResult<tokio_util::task::task_tracker::TaskTrackerToken> {
        let token = self.0.tasks.token();
        if self.0.cancel.is_cancelled() {
            return Err(OrchestrationError::Cancelled);
        }
        Ok(token)
    }
    async fn run<T>(
        &self,
        operation: impl std::future::Future<Output = crate::Result<T>>,
    ) -> OrchestrationResult<T> {
        let _activity = self.admit()?;
        tokio::select! {_ = self.0.cancel.cancelled()=>Err(OrchestrationError::Cancelled),result=operation=>result.map_err(map_error)}
    }
    async fn shared_feed(&self, key: &str) -> Option<watch::Receiver<u64>> {
        let mut feeds = self.0.changes.lock().await;
        feeds.retain(|_, feed| feed.receiver_count() > 0);
        feeds.get(key).map(watch::Sender::subscribe)
    }
    pub async fn shutdown(&self) {
        self.0.cancel.cancel();
        self.0.tasks.wait().await;
        self.0.changes.lock().await.clear();
        self.0.merges.lock().await.take();
    }
}

pub fn map_error(error: Error) -> OrchestrationError {
    let detail = error.detail;
    let host = match error.reason {
        FailureReason::RateLimited => PullRequestHostError::RateLimited {
            retry_at: error
                .retry_at
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|date| {
                    t3_contracts::wire::WireDateTime(
                        date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    )
                }),
            detail,
        },
        FailureReason::NotFound => PullRequestHostError::NotFound { detail },
        FailureReason::Failed => PullRequestHostError::Failed {
            transient: error.transient,
            detail,
        },
        FailureReason::MissingTool
        | FailureReason::Unauthenticated
        | FailureReason::Forbidden
        | FailureReason::Unsupported => PullRequestHostError::Permanent { detail },
    };
    OrchestrationError::PullRequestHost(host)
}

#[async_trait]
impl PullRequestBackend for OrchestrationPullRequestBackend {
    async fn detail(&self, input: PullRequestRef) -> OrchestrationResult<PullRequestDetail> {
        self.run(self.0.service.detail(input)).await
    }
    async fn activity(&self, input: PullRequestRef) -> OrchestrationResult<PullRequestActivity> {
        self.run(self.0.service.activity(input)).await
    }
    async fn thread_comments(
        &self,
        input: PullRequestThreadCommentsInput,
    ) -> OrchestrationResult<PullRequestThreadCommentsResult> {
        self.run(self.0.service.thread_comments(input)).await
    }
    async fn watch_fingerprint(
        &self,
        input: PullRequestRef,
    ) -> OrchestrationResult<Option<PullRequestWatchFingerprint>> {
        Ok(self
            .run(self.0.service.watch_fingerprint(input))
            .await?
            .map(|value| PullRequestWatchFingerprint {
                status: value.status,
                remarks: value.remarks,
            }))
    }
    async fn summary(&self, input: PullRequestRef) -> OrchestrationResult<PullRequestSummary> {
        self.run(self.0.service.summary(
            input,
            SummaryOptions {
                bypass_cache: false,
                recover_transient_failure: Some(false),
            },
        ))
        .await
    }
    async fn stack(
        &self,
        input: PullRequestRef,
        include_details: bool,
    ) -> OrchestrationResult<Option<ThreadPullRequestStack>> {
        let _activity = self.admit()?;
        self.run(self.0.service.stack(input, include_details))
            .await?
            .map(|value| {
                let mut raw = serde_json::to_value(value)?;
                raw["kind"] = serde_json::json!("native");
                Ok(serde_json::from_value(raw)?)
            })
            .transpose()
    }
    async fn batches_reads(&self, input: PullRequestRef) -> OrchestrationResult<bool> {
        self.run(async { Ok(self.0.service.batches_reads(&input).await) })
            .await
    }
    async fn invalidate(
        &self,
        input: PullRequestInvalidateInput,
        notify_readers: bool,
    ) -> OrchestrationResult<()> {
        self.run(self.0.service.invalidate(input, notify_readers))
            .await
    }
    async fn changes(&self, input: PullRequestRef) -> OrchestrationResult<watch::Receiver<u64>> {
        let _activity = self.admit()?;
        let context = self.run(self.0.service.context(&input)).await?;
        let key = context.cache_key();
        if let Some(receiver) = self.shared_feed(&key).await {
            return Ok(receiver);
        }
        // Opened without the registry lock: one slow host must not hold up unrelated feeds.
        let mut stream = self.run(self.0.service.changes(&input)).await?;
        let mut feeds = self.0.changes.lock().await;
        feeds.retain(|_, feed| feed.receiver_count() > 0);
        if let Some(feed) = feeds.get(&key) {
            // Another subscriber opened this feed meanwhile; this stream is dropped unused.
            return Ok(feed.subscribe());
        }
        let (output, receiver) = watch::channel(0u64);
        feeds.insert(key, output.clone());
        drop(feeds);
        let cancel = self.0.cancel.clone();
        self.0.tasks.spawn(async move{loop{tokio::select!{
            _ = cancel.cancelled()=>break,
            _ = output.closed()=>break,
            change=stream.next()=>match change{Some(())=>output.send_modify(|value|*value=value.saturating_add(1)),None=>break}
        }}});
        Ok(receiver)
    }
    async fn subscribe_merges(&self) -> OrchestrationResult<broadcast::Receiver<PullRequestRef>> {
        let _activity = self.admit()?;
        Ok(self
            .0
            .merges
            .lock()
            .await
            .as_ref()
            .ok_or(OrchestrationError::Cancelled)?
            .subscribe())
    }
    async fn notify_linked_threads_changed(&self) -> OrchestrationResult<()> {
        let _activity = self.admit()?;
        self.0.service.notify_linked_threads_changed();
        Ok(())
    }
    async fn refresh(
        &self,
        cwd: PathBuf,
        links: Vec<ThreadPullRequestLink>,
    ) -> OrchestrationResult<Vec<ThreadPullRequestLink>> {
        self.run(self.0.checkouts.refresh(cwd, links)).await
    }
    async fn discover(
        &self,
        cwd: PathBuf,
        branch: String,
    ) -> OrchestrationResult<Vec<ThreadPullRequestLink>> {
        self.run(self.0.checkouts.discover(cwd, branch)).await
    }
}
