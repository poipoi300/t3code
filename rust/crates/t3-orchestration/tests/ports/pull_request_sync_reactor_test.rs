#![allow(clippy::unwrap_used)]

use super::{
    orchestrator_receipt_race_test::Harness,
    thread_management_service_test::{Recorder, sample},
    thread_pull_request_service_test::Registry,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::{pull_request::*, thread_pull_request::*};
use t3_orchestration::{
    api::*,
    error::{Error, PullRequestHostError, Result},
    ports::thread_project::pull_request_sync_reactor::*,
    testing::{SequenceIds, TEST_TIME},
};
use tokio::sync::{Semaphore, mpsc};

struct Policy;
impl PullRequestSyncPolicy for Policy {
    fn sibling_url(&self, url: &str, number: u64) -> Option<String> {
        Some(format!("{}/{number}", url.rsplit_once('/')?.0))
    }
    fn avatar_identity(&self, url: &str) -> String {
        url.split("?token=").next().unwrap().into()
    }
    fn command(&self, input: &str, _: &str) -> CommandObservation {
        CommandObservation {
            pushes: input == "push" || input == "createpushclose",
            closes: input == "close" || input == "createpushclose",
            created: if input == "create" || input == "createpushclose" {
                vec![(
                    serde_json::from_value(
                        json!({"host":"github.com","repository":"owner/repo","number":42}),
                    )
                    .unwrap(),
                    "https://github.com/owner/repo/pull/42".into(),
                )]
            } else {
                vec![]
            },
        }
    }
}
struct Host {
    summaries: Mutex<BTreeMap<u64, PullRequestSummary>>,
    errors: Mutex<BTreeMap<String, PullRequestHostError>>,
    stack: Mutex<Option<ThreadPullRequestStack>>,
    stack_error: Mutex<Option<PullRequestHostError>>,
    batched: bool,
    calls: AtomicUsize,
    stack_calls: AtomicUsize,
    invalidations: AtomicUsize,
    gate: Option<(mpsc::UnboundedSender<()>, Arc<Semaphore>)>,
}
fn summary(number: u64) -> PullRequestSummary {
    let mut value = sample("PullRequestSummary");
    for (key, value2) in [
        ("provider", json!("github")),
        ("projectId", json!("project")),
        ("repository", json!("owner/repo")),
        ("number", json!(number)),
        ("title", json!("Review")),
        (
            "url",
            json!(format!("https://github.com/owner/repo/pull/{number}")),
        ),
        ("state", json!("open")),
        ("isDraft", json!(false)),
        ("headBranch", json!("feature")),
        ("baseBranch", json!("main")),
        ("updatedAt", json!("2026-08-01T00:00:00.000Z")),
        ("closedAt", Value::Null),
        ("mergedAt", Value::Null),
        ("stack", Value::Null),
        ("mergeability", json!("unknown")),
    ] {
        value[key] = value2;
    }
    serde_json::from_value(value).unwrap()
}
fn host() -> Host {
    Host {
        batched: false,
        summaries: Mutex::new([(42, summary(42)), (43, summary(43))].into_iter().collect()),
        errors: Mutex::new(BTreeMap::new()),
        stack: Mutex::new(None),
        stack_error: Mutex::new(None),
        calls: AtomicUsize::new(0),
        stack_calls: AtomicUsize::new(0),
        invalidations: AtomicUsize::new(0),
        gate: None,
    }
}
#[async_trait]
impl PullRequestBackend for Host {
    async fn summary(&self, input: PullRequestRef) -> Result<PullRequestSummary> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((entered, gate)) = &self.gate {
            entered.send(()).unwrap();
            gate.acquire().await.unwrap().forget();
        }
        if let Some(error) = self
            .errors
            .lock()
            .unwrap()
            .get(&input.project_id.0)
            .cloned()
        {
            return Err(Error::PullRequestHost(error));
        }
        Ok(self.summaries.lock().unwrap()[&input.number].clone())
    }
    async fn stack(
        &self,
        _: PullRequestRef,
        details: bool,
    ) -> Result<Option<ThreadPullRequestStack>> {
        assert!(!details);
        self.stack_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.stack_error.lock().unwrap().clone() {
            return Err(Error::PullRequestHost(error));
        }
        Ok(self.stack.lock().unwrap().clone())
    }
    async fn batches_reads(&self, _: PullRequestRef) -> Result<bool> {
        Ok(self.batched)
    }
    async fn invalidate(&self, _: PullRequestInvalidateInput, notify: bool) -> Result<()> {
        assert!(!notify);
        self.invalidations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn refresh(
        &self,
        _: PathBuf,
        _: Vec<ThreadPullRequestLink>,
    ) -> Result<Vec<ThreadPullRequestLink>> {
        panic!("strict summary port required")
    }
    async fn discover(&self, _: PathBuf, _: String) -> Result<Vec<ThreadPullRequestLink>> {
        panic!("strict summary port required")
    }
}
fn snapshot() -> Value {
    let value = json!(summary(42));
    let mut snapshot = json!({"syncedAt":TEST_TIME});
    for key in [
        "state",
        "title",
        "headBranch",
        "baseBranch",
        "isDraft",
        "updatedAt",
        "closedAt",
        "mergedAt",
        "author",
        "additions",
        "deletions",
        "changedFiles",
        "reviewDecision",
        "checksState",
        "mergeability",
    ] {
        if let Some(value) = value.get(key) {
            snapshot[key] = value.clone();
        }
    }
    snapshot
}
fn link(number: u64, snapshot: Value) -> Value {
    json!({"host":"github.com","repository":"owner/repo","number":number,"url":format!("https://github.com/owner/repo/pull/{number}"),"source":"manual","linkedAt":"2026-08-01T00:00:00.000Z","snapshot":snapshot,"stack":null})
}
async fn thread(
    harness: &Harness,
    id: &str,
    links: Value,
    settled: bool,
    project: &str,
) -> Result<()> {
    harness.create(id).await?;
    let mut value = json!(harness.reader.thread(ThreadId(id.into())).await?.unwrap());
    value["projectId"] = json!(project);
    value["pullRequests"] = links;
    if settled {
        value["settledOverride"] = json!("settled");
        value["settledAt"] = json!(TEST_TIME);
    }
    harness.sink.commit(CommitRequest{receipt:None,decision:Decision{events:vec![serde_json::from_value(json!({"id":format!("pr:{id}"),"threadId":id,"type":"thread.metadata-updated","occurredAt":TEST_TIME,"payload":value}))?],..Default::default()},guard:CommitGuard::None}).await?;
    Ok(())
}
fn reactor(harness: &Harness, host: Arc<Host>) -> (Arc<PullRequestSyncReactor>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder {
        commands: Mutex::new(Vec::new()),
        downstream: None,
    });
    (
        Arc::new(PullRequestSyncReactor::new(PullRequestSyncServices {
            projections: harness.reader.clone(),
            commands: recorder.clone(),
            committer: harness.sink.clone(),
            pull_requests: host,
            identities: Arc::new(Registry),
            policy: Arc::new(Policy),
            clock: harness.clock.clone(),
            ids: Arc::new(SequenceIds::new("sync")),
        })),
        recorder,
    )
}
fn request() -> ThreadPullRequestKey {
    serde_json::from_value(json!({"host":"github.com","repository":"owner/repo","number":42}))
        .unwrap()
}

#[tokio::test]
async fn sync_groups_reads_and_ignores_synced_time_and_known_github_mergeability() -> Result<()> {
    let harness = Harness::new(None, false);
    let host = Arc::new(host());
    let mut saved = snapshot();
    saved["mergeability"] = json!("mergeable");
    thread(
        &harness,
        "one",
        json!([link(42, saved.clone())]),
        false,
        "project",
    )
    .await?;
    saved["syncedAt"] = json!("2026-09-01T00:00:00.000Z");
    thread(&harness, "two", json!([link(42, saved)]), false, "project").await?;
    let (reactor, recorder) = reactor(&harness, host.clone());
    reactor.sweep().await?;
    reactor.sweep().await?;
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.stack_calls.load(Ordering::SeqCst), 0);
    assert!(recorder.commands.lock().unwrap().is_empty());
    reactor.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn sync_final_snapshots_and_settled_threads_refresh_only_on_request() -> Result<()> {
    for state in ["merged", "closed", "settled"] {
        let harness = Harness::new(None, false);
        let host = Arc::new(host());
        let mut saved = snapshot();
        if state != "settled" {
            saved["state"] = json!(state);
            saved["mergedAt"] = json!("2026-08-01T00:00:00.000Z");
            saved["closedAt"] = json!("2026-08-01T00:00:00.000Z");
        }
        thread(
            &harness,
            "thread",
            json!([link(42, saved)]),
            state == "settled",
            "project",
        )
        .await?;
        let (reactor, _) = reactor(&harness, host.clone());
        reactor.sweep().await?;
        assert_eq!(host.calls.load(Ordering::SeqCst), 0);
        reactor.request_sync(request()).await?;
        reactor.drain().await;
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.invalidations.load(Ordering::SeqCst), 1);
        reactor.shutdown().await;
    }
    Ok(())
}
#[tokio::test]
async fn sync_not_found_backs_off_but_explicit_refresh_and_other_project_still_read() -> Result<()>
{
    let harness = Harness::new(None, false);
    let host = Arc::new(host());
    host.errors.lock().unwrap().insert(
        "project".into(),
        PullRequestHostError::NotFound {
            detail: "missing".into(),
        },
    );
    thread(
        &harness,
        "one",
        json!([link(42, Value::Null)]),
        false,
        "project",
    )
    .await?;
    thread(
        &harness,
        "two",
        json!([link(42, Value::Null)]),
        false,
        "other",
    )
    .await?;
    let (reactor, recorder) = reactor(&harness, host.clone());
    reactor.sweep().await?;
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    assert_eq!(recorder.commands.lock().unwrap().len(), 1);
    reactor.sweep().await?;
    assert_eq!(host.calls.load(Ordering::SeqCst), 3);
    reactor.request_sync(request()).await?;
    reactor.drain().await;
    assert_eq!(host.calls.load(Ordering::SeqCst), 5);
    reactor.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn sync_rate_limit_pauses_only_its_project_host_and_preserves_request() -> Result<()> {
    let harness = Harness::new(None, false);
    let host = Arc::new(host());
    host.errors.lock().unwrap().insert(
        "project".into(),
        PullRequestHostError::RateLimited {
            retry_at: Some(serde_json::from_value(json!("2027-01-01T00:00:00.000Z"))?),
            detail: "later".into(),
        },
    );
    thread(
        &harness,
        "one",
        json!([link(42, Value::Null)]),
        false,
        "project",
    )
    .await?;
    thread(
        &harness,
        "two",
        json!([link(42, Value::Null)]),
        false,
        "other",
    )
    .await?;
    let (reactor, _) = reactor(&harness, host.clone());
    reactor.sweep().await?;
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    reactor.request_sync(request()).await?;
    reactor.drain().await;
    assert_eq!(host.calls.load(Ordering::SeqCst), 3);
    reactor.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn sync_stack_failure_retries_and_links_siblings_before_snapshot_without_resurrecting_tombstones()
-> Result<()> {
    let harness = Harness::new(None, false);
    let host = Arc::new(host());
    let mut summary = json!(summary(42));
    summary["stack"] = json!({"number":1,"position":1,"size":3,"base":"main"});
    host.summaries
        .lock()
        .unwrap()
        .insert(42, serde_json::from_value(summary)?);
    *host.stack_error.lock().unwrap() = Some(PullRequestHostError::Failed {
        transient: true,
        detail: "stack offline".into(),
    });
    let mut dismissed = link(43, Value::Null);
    dismissed["source"] = json!("stack-dismissed");
    thread(
        &harness,
        "thread",
        json!([link(42, Value::Null), dismissed]),
        false,
        "project",
    )
    .await?;
    let (reactor, recorder) = reactor(&harness, host.clone());
    reactor.sweep().await?;
    assert!(recorder.commands.lock().unwrap().is_empty());
    *host.stack_error.lock().unwrap() = None;
    *host.stack.lock().unwrap() = Some(serde_json::from_value(
        json!({"kind":"native","id":"stack","number":1,"url":"https://github.com/owner/repo/stack/1","base":"main","layers":[{"number":42,"headBranch":"feature","state":"open"},{"number":43,"headBranch":"dismissed","state":"open"},{"number":44,"headBranch":"new","state":"open"}]}),
    )?);
    reactor.sweep().await?;
    {
        let commands = recorder.commands.lock().unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0]["type"], "thread.pull-request.link");
        assert_eq!(commands[0]["number"], 44);
        assert_eq!(commands[1]["type"], "thread.pull-request-link.sync");
    }
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    assert_eq!(host.stack_calls.load(Ordering::SeqCst), 2);
    assert_eq!(host.invalidations.load(Ordering::SeqCst), 1);
    reactor.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn sync_requests_arriving_during_read_run_next_and_shutdown_cancels_active_reads()
-> Result<()> {
    let harness = Harness::new(None, false);
    thread(
        &harness,
        "thread",
        json!([link(42, snapshot())]),
        false,
        "project",
    )
    .await?;
    let (entered, mut calls) = mpsc::unbounded_channel();
    let gate = Arc::new(Semaphore::new(0));
    let mut backend = host();
    backend.gate = Some((entered, gate.clone()));
    let host = Arc::new(backend);
    let (reactor, _) = reactor(&harness, host.clone());
    reactor.request_sync(request()).await?;
    calls.recv().await.unwrap();
    reactor.request_sync(request()).await?;
    gate.add_permits(1);
    calls.recv().await.unwrap();
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    reactor.shutdown().await;
    assert!(matches!(
        reactor.request_sync(request()).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(reactor.sweep().await, Err(Error::Cancelled)));
    Ok(())
}
#[tokio::test]
async fn sync_command_observations_use_stable_receipts_reopen_checks_and_refresh_close_at_run_end()
-> Result<()> {
    let harness = Harness::new(None, false);
    let host = Arc::new(host());
    let mut watched = link(42, snapshot());
    watched["watch"] = json!({"startedAt":TEST_TIME,"headSha":null,"failedChecks":[],"passedChecks":[],"passed":false,"remarksThrough":TEST_TIME,"remarkIds":[],"conflicting":false,"wakes":0,"checksPending":false});
    thread(&harness, "thread", json!([watched]), false, "project").await?;
    let (reactor, recorder) = reactor(&harness, host.clone());
    for command in ["create", "push", "close"] {
        let event = serde_json::from_value(
            json!({"id":format!("event:{command}"),"threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":{"id":command,"threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":TEST_TIME,"completedAt":TEST_TIME,"updatedAt":TEST_TIME,"type":"command_execution","input":command,"output":""}}),
        )?;
        reactor.process_event(&event).await?;
        reactor.process_event(&event).await?;
    }
    reactor.drain().await;
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    {
        let commands = recorder.commands.lock().unwrap();
        let creates: Vec<_> = commands
            .iter()
            .filter(|cmd| cmd["source"] == "created")
            .collect();
        assert_eq!(creates.len(), 2);
        assert_eq!(creates[0]["commandId"], creates[1]["commandId"]);
        let pushes: Vec<_> = commands
            .iter()
            .filter(|cmd| cmd["type"] == "thread.pull-request-watch.checks-reopen")
            .collect();
        assert_eq!(pushes.len(), 2);
        assert_eq!(pushes[0]["commandId"], pushes[1]["commandId"]);
    }
    let mut run = super::thread_management_service_test::run("run", 1, "completed");
    run["status"] = json!("completed");
    let event = serde_json::from_value(
        json!({"id":"end","threadId":"thread","type":"run.updated","occurredAt":TEST_TIME,"payload":run}),
    )?;
    reactor.process_event(&event).await?;
    reactor.drain().await;
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    reactor.process_event(&event).await?;
    reactor.drain().await;
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    reactor.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn sync_host_read_pools_enforce_unbatched_and_batched_limits_and_join_on_shutdown()
-> Result<()> {
    for (batched, width) in [(false, 25), (true, 100)] {
        let harness = Harness::new(None, false);
        let links: Vec<_> = (1..=width + 1)
            .map(|number| link(number as u64, Value::Null))
            .collect();
        thread(&harness, "thread", json!(links), false, "project").await?;
        let (entered, mut calls) = mpsc::unbounded_channel();
        let gate = Arc::new(Semaphore::new(0));
        let mut backend = host();
        backend.batched = batched;
        backend.gate = Some((entered, gate.clone()));
        for number in 1..=width + 1 {
            backend
                .summaries
                .lock()
                .unwrap()
                .insert(number as u64, summary(number as u64));
        }
        let backend = Arc::new(backend);
        let (reactor, _) = reactor(&harness, backend.clone());
        let sweep = {
            let reactor = reactor.clone();
            tokio::spawn(async move { reactor.sweep().await })
        };
        for _ in 0..width {
            calls.recv().await.unwrap();
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), width);
        gate.add_permits(1);
        calls.recv().await.unwrap();
        assert_eq!(backend.calls.load(Ordering::SeqCst), width + 1);
        reactor.shutdown().await;
        assert!(matches!(sweep.await.unwrap(), Err(Error::Cancelled)));
    }
    Ok(())
}

#[tokio::test]
async fn sync_rejected_created_link_does_not_block_push_checks_or_close_refresh() -> Result<()> {
    struct RejectCreated(Arc<Recorder>);
    #[async_trait]
    impl CommandDispatcher for RejectCreated {
        async fn dispatch(
            &self,
            command: OrchestrationV2ServerCommand,
        ) -> Result<t3_orchestration::stores::receipts::CommandReceipt> {
            if json!(command)["source"] == "created" {
                return Err(Error::Rejected {
                    command_id: CommandId("rejected".into()),
                    reason: "thread changed".into(),
                });
            }
            self.0.dispatch(command).await
        }
    }
    let harness = Harness::new(None, false);
    let host = Arc::new(host());
    let mut watched = link(42, snapshot());
    watched["watch"] = json!({"startedAt":TEST_TIME,"headSha":null,"failedChecks":[],"passedChecks":[],"passed":false,"remarksThrough":TEST_TIME,"remarkIds":[],"conflicting":false,"wakes":0,"checksPending":false});
    thread(&harness, "thread", json!([watched]), false, "project").await?;
    let recorder = Arc::new(Recorder {
        commands: Mutex::new(Vec::new()),
        downstream: None,
    });
    let reactor = PullRequestSyncReactor::new(PullRequestSyncServices {
        projections: harness.reader.clone(),
        commands: Arc::new(RejectCreated(recorder.clone())),
        committer: harness.sink.clone(),
        pull_requests: host.clone(),
        identities: Arc::new(Registry),
        policy: Arc::new(Policy),
        clock: harness.clock.clone(),
        ids: Arc::new(SequenceIds::new("sync-reject")),
    });
    let event = serde_json::from_value(
        json!({"id":"event","threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":{"id":"command","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed","title":null,"startedAt":TEST_TIME,"completedAt":TEST_TIME,"updatedAt":TEST_TIME,"type":"command_execution","input":"createpushclose","output":""}}),
    )?;
    reactor.process_event(&event).await?;
    reactor.drain().await;
    assert!(
        recorder
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|cmd| cmd["type"] == "thread.pull-request-watch.checks-reopen")
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    reactor.shutdown().await;
    Ok(())
}
