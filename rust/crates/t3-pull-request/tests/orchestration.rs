#![cfg(feature = "orchestration")]
mod support;
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::*;
use t3_contracts::{pull_request::*, thread_pull_request::ThreadPullRequestLink};
use t3_orchestration::{
    api::PullRequestBackend,
    error::{Error as OrchestrationError, PullRequestHostError},
};
use t3_pull_request::{
    Error, FailureReason, PullRequestService, Result,
    cache::{MemoryStore, PullRequestReadCache},
    context::{Binding, Context, Project},
    host::{Changes, Host},
    orchestration::{CheckoutPullRequests, OrchestrationPullRequestBackend, map_error},
};
use tokio::sync::broadcast;

struct Checkouts;
#[async_trait]
impl CheckoutPullRequests for Checkouts {
    async fn refresh(
        &self,
        _: PathBuf,
        links: Vec<ThreadPullRequestLink>,
    ) -> Result<Vec<ThreadPullRequestLink>> {
        Ok(links)
    }
    async fn discover(&self, _: PathBuf, _: String) -> Result<Vec<ThreadPullRequestLink>> {
        Err(Error::unsupported("discover"))
    }
}
struct FeedHost {
    fake: Arc<FakeHost>,
    events: broadcast::Sender<()>,
    subscriptions: AtomicUsize,
}
#[async_trait]
impl Host for FeedHost {
    fn kind(&self) -> &str {
        "github"
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.fake.caps
    }
    fn batches_reads(&self) -> bool {
        true
    }
    async fn viewer(&self, project: &Project) -> Result<String> {
        self.fake.viewer(project).await
    }
    async fn summary(&self, ctx: &Context) -> Result<PullRequestSummary> {
        self.fake.summary(ctx).await
    }
    async fn permissions(
        &self,
        ctx: &Context,
        update: bool,
    ) -> Result<PullRequestViewerPermissions> {
        self.fake.permissions(ctx, update).await
    }
    async fn run_action(
        &self,
        ctx: &Context,
        input: &PullRequestActionInput,
    ) -> Result<Option<PullRequestSummary>> {
        self.fake.run_action(ctx, input).await
    }
    async fn stack(&self, _: &Context, include_details: bool) -> Result<Option<PullRequestStack>> {
        Ok(Some(decode(
            json!({"id":"stack","number":1,"url":"https://github.com/owner/repo/pull/1","base":"main","layers":[{"number":42,"headBranch":"topic","state":"open","title":if include_details{Some("Title")}else{None}}]}),
        )))
    }
    async fn changes(&self, _: &Context) -> Result<Changes> {
        self.subscriptions.fetch_add(1, Ordering::AcqRel);
        let receiver = self.events.subscribe();
        Ok(Box::pin(stream::unfold(
            receiver,
            |mut receiver| async move {
                loop {
                    match receiver.recv().await {
                        Ok(()) => return Some(((), receiver)),
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        )))
    }
}
fn backend() -> (
    OrchestrationPullRequestBackend,
    PullRequestService,
    Arc<FeedHost>,
    Arc<TestClock>,
) {
    let host = Arc::new(FeedHost {
        fake: Arc::new(FakeHost::default()),
        events: broadcast::channel(8).0,
        subscriptions: AtomicUsize::new(0),
    });
    let source = Arc::new(Source(Mutex::new(vec![Binding {
        project: project(),
        host: host.clone(),
    }])));
    let clock = Arc::new(TestClock::default());
    let service = PullRequestService::new(
        source,
        clock.clone(),
        Arc::new(PullRequestReadCache::new(
            Arc::new(MemoryStore::default()),
            clock.clone(),
        )),
    );
    (
        OrchestrationPullRequestBackend::new(service.clone(), Arc::new(Checkouts)),
        service,
        host,
        clock,
    )
}

#[test]
fn adapter_errors_preserve_host_policy_and_convert_retry_milliseconds() {
    for (reason, kind) in [
        (FailureReason::Failed, "failed"),
        (FailureReason::RateLimited, "limited"),
        (FailureReason::NotFound, "missing"),
        (FailureReason::Forbidden, "permanent"),
        (FailureReason::Unauthenticated, "permanent"),
        (FailureReason::MissingTool, "permanent"),
        (FailureReason::Unsupported, "permanent"),
    ] {
        let error = map_error(Error {
            reason,
            transient: true,
            retry_at: Some(60_000),
            ..Error::invalid("summary", "Host failed.")
        });
        let OrchestrationError::PullRequestHost(error) = error else {
            panic!("host error lost")
        };
        match (kind, error) {
            (
                "failed",
                PullRequestHostError::Failed {
                    transient: true, ..
                },
            )
            | ("missing", PullRequestHostError::NotFound { .. })
            | ("permanent", PullRequestHostError::Permanent { .. }) => {}
            (
                "limited",
                PullRequestHostError::RateLimited {
                    retry_at: Some(at), ..
                },
            ) => assert_eq!(at.0, "1970-01-01T00:01:00.000Z"),
            (_, error) => panic!("wrong policy: {error:?}"),
        }
    }
    assert!(matches!(
        map_error(Error::invalid("decode", "Invalid data.")),
        OrchestrationError::PullRequestHost(PullRequestHostError::Failed {
            transient: false,
            ..
        })
    ));
}

#[tokio::test]
async fn strict_orchestration_summary_returns_transient_failure_and_stack_uses_persisted_contract()
{
    let (backend, service, host, clock) = backend();
    service
        .summary(reference(), Default::default())
        .await
        .expect("seed");
    clock.0.store(60_001, Ordering::Release);
    host.fake
        .summaries
        .lock()
        .expect("queue")
        .push_back(Err(Error {
            transient: true,
            ..Error::invalid("summary", "Temporary host failure.")
        }));
    let mut input = reference();
    input.allow_stale = Some(Some(true));
    assert!(matches!(
        backend.summary(input).await.expect_err("strict error"),
        OrchestrationError::PullRequestHost(PullRequestHostError::Failed {
            transient: true,
            ..
        })
    ));
    assert_eq!(host.fake.reads.load(Ordering::Acquire), 2);
    let stack = backend
        .stack(reference(), false)
        .await
        .expect("stack")
        .expect("native stack");
    assert_eq!(stack.layers[0].number, 42);
    assert!(
        backend
            .batches_reads(reference())
            .await
            .expect("batch flag")
    );
    backend.shutdown().await;
    service.shutdown().await;
}

#[tokio::test]
async fn change_feeds_share_one_host_subscription_and_close_on_shutdown() {
    let (backend, service, host, _) = backend();
    let mut first = backend.changes(reference()).await.expect("first");
    let mut second = backend.changes(reference()).await.expect("second");
    assert_eq!(host.subscriptions.load(Ordering::Acquire), 1);
    host.events.send(()).expect("change");
    first.changed().await.expect("first change");
    second.changed().await.expect("second change");
    assert_eq!(*first.borrow(), 1);
    assert_eq!(*second.borrow(), 1);
    backend.shutdown().await;
    assert!(matches!(
        backend.changes(reference()).await,
        Err(OrchestrationError::Cancelled)
    ));
    assert_eq!(host.events.receiver_count(), 0);
    service.shutdown().await;
}

#[tokio::test]
async fn adapter_forwards_confirmed_merge_refs_and_link_refresh_notifications() {
    let (backend, service, host, _) = backend();
    let mut merges = backend.subscribe_merges().await.expect("merge feed");
    let mut linked = service.subscribe_linked_thread_refreshes();
    backend
        .notify_linked_threads_changed()
        .await
        .expect("notify");
    linked.changed().await.expect("link refresh");
    *host.fake.action.lock().expect("action") = Some(summary("merged"));
    service
        .run_action(decode(input(json!({"action":"merge"}))))
        .await
        .expect("merge");
    assert_eq!(merges.recv().await.expect("merged ref").number, 42);
    backend.shutdown().await;
    service.shutdown().await;
}

#[tokio::test]
async fn shutdown_cancels_admitted_checkout_work_and_closes_merge_receivers() {
    struct PendingCheckouts {
        started: tokio::sync::Notify,
    }
    #[async_trait]
    impl CheckoutPullRequests for PendingCheckouts {
        async fn refresh(
            &self,
            _: PathBuf,
            _: Vec<ThreadPullRequestLink>,
        ) -> Result<Vec<ThreadPullRequestLink>> {
            self.started.notify_one();
            std::future::pending().await
        }
        async fn discover(&self, _: PathBuf, _: String) -> Result<Vec<ThreadPullRequestLink>> {
            self.started.notify_one();
            std::future::pending().await
        }
    }
    let (service, _, _, _) = setup();
    let checkouts = Arc::new(PendingCheckouts {
        started: tokio::sync::Notify::new(),
    });
    let backend = OrchestrationPullRequestBackend::new(service.clone(), checkouts.clone());
    let mut merges = backend.subscribe_merges().await.expect("merges");
    let work = {
        let backend = backend.clone();
        tokio::spawn(async move {
            backend
                .discover(PathBuf::from("C:/authorized-worktree"), "topic".into())
                .await
        })
    };
    checkouts.started.notified().await;
    backend.shutdown().await;
    assert!(matches!(
        work.await.expect("task"),
        Err(OrchestrationError::Cancelled)
    ));
    assert!(matches!(
        merges.recv().await,
        Err(broadcast::error::RecvError::Closed)
    ));
    service.shutdown().await;
}

/// Opens #1's change feed only when released; every other feed opens at once.
struct GatedHost {
    fake: Arc<FakeHost>,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl Host for GatedHost {
    fn kind(&self) -> &str {
        "github"
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.fake.caps
    }
    async fn changes(&self, ctx: &Context) -> Result<Changes> {
        if ctx.reference.number == 1 {
            if let Some(entered) = self.entered.lock().expect("entered").take() {
                let _ = entered.send(());
            }
            self.release.notified().await;
        }
        Ok(Box::pin(stream::pending()))
    }
}

#[tokio::test]
async fn a_slow_change_feed_does_not_block_feeds_for_other_pull_requests() {
    use futures_util::FutureExt;
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = Arc::new(GatedHost {
        fake: Arc::new(FakeHost::default()),
        entered: Mutex::new(Some(entered)),
        release: release.clone(),
    });
    let source = Arc::new(Source(Mutex::new(vec![Binding {
        project: project(),
        host,
    }])));
    let clock = Arc::new(TestClock::default());
    let service = PullRequestService::new(
        source,
        clock.clone(),
        Arc::new(PullRequestReadCache::new(
            Arc::new(MemoryStore::default()),
            clock,
        )),
    );
    let backend = OrchestrationPullRequestBackend::new(service, Arc::new(Checkouts));
    let numbered = |number: u64| {
        let mut input = reference();
        input.number = number;
        input
    };
    let slow = tokio::spawn({
        let backend = backend.clone();
        let input = numbered(1);
        async move { backend.changes(input).await.map(|_| ()) }
    });
    entered_rx.await.expect("the slow feed is opening");
    let other = backend.changes(numbered(2)).now_or_never();
    assert!(
        matches!(other, Some(Ok(_))),
        "a feed for another pull request waited on the slow one"
    );
    release.notify_one();
    slow.await.expect("joined").expect("slow feed opened");
    backend.shutdown().await;
}
