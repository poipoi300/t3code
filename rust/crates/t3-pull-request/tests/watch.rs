mod support;
use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use support::*;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    thread_pull_request::*,
};
use t3_pull_request::{Error, FailureReason, Result, watch::*};

struct RecordingOrchestration {
    links: Mutex<Vec<ThreadPullRequestLink>>,
    commands: Mutex<Vec<WatchCommand>>,
    subagent: AtomicBool,
    deleted: AtomicBool,
    reads: AtomicUsize,
}
impl RecordingOrchestration {
    fn new() -> Self {
        Self {
            links: Default::default(),
            commands: Default::default(),
            subagent: AtomicBool::new(false),
            deleted: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
        }
    }
}
fn watch() -> ThreadPullRequestWatch {
    decode(
        json!({"startedAt":"2026-01-01T00:00:00.000Z","headSha":null,"failedChecks":[],"passed":false,"passedChecks":[],"remarksThrough":"2026-01-01T00:00:00.000Z","remarkIds":["already-seen"],"conflicting":false,"wakes":2}),
    )
}
fn link() -> ThreadPullRequestLink {
    decode(
        json!({"host":"github.com","repository":"owner/repo","number":42,"url":"https://github.com/owner/repo/pull/42","source":"agent","linkedAt":"2026-01-01T00:00:00.000Z","snapshot":null,"stack":null}),
    )
}
#[async_trait]
impl WatchOrchestration for RecordingOrchestration {
    async fn thread(&self, id: &ThreadId) -> Result<Option<WatchThread>> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        Ok(Some(WatchThread {
            id: id.clone(),
            project_id: ProjectId("p".into()),
            deleted: self.deleted.load(Ordering::Acquire),
            subagent: self.subagent.load(Ordering::Acquire),
            links: self.links.lock().expect("links").clone(),
        }))
    }
    async fn resolve_target(&self, _: Target, _: &ProjectId) -> Result<ResolvedTarget> {
        Ok(ResolvedTarget {
            key: decode(json!({"host":"github.com","repository":"owner/repo","number":42})),
            url: "https://github.com/owner/repo/pull/42".into(),
        })
    }
    async fn dispatch_watch(&self, command: WatchCommand) -> Result<()> {
        let mut links = self.links.lock().expect("links");
        if links.is_empty() {
            links.push(link());
        }
        links[0].watch = if command.watching {
            Some(Some(
                links[0].ended_watch.clone().flatten().unwrap_or_else(watch),
            ))
        } else {
            links[0].ended_watch = links[0].watch.clone();
            None
        };
        self.commands.lock().expect("commands").push(command);
        Ok(())
    }
}
#[tokio::test]
async fn registration_preflights_uncached_and_observes_actual_committed_state() {
    let (service, host, _, _) = setup();
    service
        .summary(reference(), Default::default())
        .await
        .expect("seed");
    let orchestration = Arc::new(RecordingOrchestration::new());
    let watch = PullRequestWatchService::new(orchestration.clone(), service);
    let result = watch
        .set_watching(ThreadId("t".into()), Target::default(), true)
        .await
        .expect("watch");
    assert!(result.watching);
    assert!(!result.was_watching);
    assert_eq!(host.reads.load(Ordering::Acquire), 2);
    assert_eq!(orchestration.reads.load(Ordering::Acquire), 2);
    assert_eq!(orchestration.commands.lock().expect("commands").len(), 1);
}

#[tokio::test]
async fn transient_preflight_errors_allow_watch_but_unavailability_and_merged_refuse() {
    for (reason, allowed) in [
        (FailureReason::Failed, true),
        (FailureReason::RateLimited, true),
        (FailureReason::NotFound, true),
        (FailureReason::Unauthenticated, false),
        (FailureReason::MissingTool, false),
        (FailureReason::Unsupported, false),
    ] {
        let (service, host, _, _) = setup();
        host.summaries.lock().expect("queue").push_back(Err(Error {
            reason,
            ..Error::invalid("summary", "Host error.")
        }));
        let orchestration = Arc::new(RecordingOrchestration::new());
        let watch = PullRequestWatchService::new(orchestration.clone(), service);
        assert_eq!(
            watch
                .set_watching(ThreadId("t".into()), Target::default(), true)
                .await
                .is_ok(),
            allowed
        );
        assert_eq!(
            orchestration.commands.lock().expect("commands").len(),
            usize::from(allowed)
        );
    }
    let (service, host, _, _) = setup();
    host.summaries
        .lock()
        .expect("queue")
        .push_back(Ok(summary("merged")));
    let watch = PullRequestWatchService::new(Arc::new(RecordingOrchestration::new()), service);
    assert!(
        watch
            .set_watching(ThreadId("t".into()), Target::default(), true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn subagents_cannot_start_watches_but_can_stop_existing_watches_and_resume_preserves_baseline()
 {
    let (service, host, _, _) = setup();
    let orchestration = Arc::new(RecordingOrchestration::new());
    orchestration.subagent.store(true, Ordering::Release);
    let watch = PullRequestWatchService::new(orchestration.clone(), service);
    assert!(
        watch
            .set_watching(ThreadId("t".into()), Target::default(), true)
            .await
            .is_err()
    );
    watch
        .set_watching(ThreadId("t".into()), Target::default(), false)
        .await
        .expect("stop");
    assert_eq!(host.reads.load(Ordering::Acquire), 0);
    orchestration.subagent.store(false, Ordering::Release);
    watch
        .set_watching(ThreadId("t".into()), Target::default(), true)
        .await
        .expect("start");
    watch
        .set_watching(ThreadId("t".into()), Target::default(), false)
        .await
        .expect("stop");
    let resumed = watch
        .set_watching(ThreadId("t".into()), Target::default(), true)
        .await
        .expect("resume");
    assert!(resumed.watching);
    let links = orchestration.links.lock().expect("links");
    assert_eq!(
        links[0]
            .watch
            .as_ref()
            .and_then(Option::as_ref)
            .expect("watch")
            .remark_ids[0]
            .as_str(),
        "already-seen"
    );
}

#[tokio::test]
async fn deleted_threads_are_rejected_before_host_reads() {
    let (service, host, _, _) = setup();
    let orchestration = Arc::new(RecordingOrchestration::new());
    orchestration.deleted.store(true, Ordering::Release);
    let watch = PullRequestWatchService::new(orchestration.clone(), service);
    assert_eq!(
        watch
            .set_watching(ThreadId("t".into()), Target::default(), false)
            .await
            .err()
            .expect("not found")
            .reason,
        FailureReason::NotFound
    );
    assert_eq!(host.reads.load(Ordering::Acquire), 0);
    assert!(orchestration.commands.lock().expect("commands").is_empty());
}
