use super::{
    orchestrator_receipt_race_test::Harness,
    thread_management_service_test::{Recorder, run, sample},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use t3_contracts::{environment::RepositoryIdentity, pull_request::*, thread_pull_request::*};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::thread_project::thread_pull_request_service::*,
    projections::write::{ParsedPullRequestKey, PullRequestIdentityRegistry},
    stores::projects,
    testing::{SequenceIds, TEST_TIME},
};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

pub(super) struct Registry;
impl PullRequestIdentityRegistry for Registry {
    fn parse_change_request_url(&self, _: &str) -> Option<ParsedPullRequestKey> {
        None
    }
    fn canonical_repository_key(&self, key: &str) -> String {
        key.to_lowercase()
    }
    fn contains_repository(&self, _: &str) -> bool {
        true
    }
}
pub(super) fn identity() -> RepositoryIdentity {
    serde_json::from_value(json!({"canonicalKey":"github.com/owner/repo","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/owner/repo.git"},"displayName":"owner/repo","provider":"github","owner":"owner","name":"repo"})).unwrap()
}
pub(super) fn branch(state: &str) -> GitBranchPullRequest {
    GitBranchPullRequest { pull_request:serde_json::from_value(json!({"number":42,"title":"Review","url":"https://github.com/owner/repo/pull/42","baseRef":"main","headRef":"feature","state":state})) .unwrap(),repository_key:Some("github.com/owner/repo".into()),updated_at:None,closed_at:None,merged_at:None,head_sha:None }
}
pub(super) struct Workspace {
    pub(super) queries: Mutex<Vec<BranchPullRequestQuery>>,
    identity_refresh: Mutex<Vec<bool>>,
    pub(super) detected: Mutex<Option<GitBranchPullRequest>>,
    pub(super) fresh: Mutex<Option<Option<GitBranchPullRequest>>>,
    pub(super) refreshed_identity: Mutex<Option<Option<RepositoryIdentity>>>,
    checked_out: AtomicBool,
    fail: AtomicBool,
    pub(super) gate: Option<(mpsc::UnboundedSender<()>, Arc<Semaphore>)>,
}
impl Workspace {
    pub(super) fn new() -> Self {
        Self {
            queries: Mutex::new(Vec::new()),
            identity_refresh: Mutex::new(Vec::new()),
            detected: Mutex::new(Some(branch("open"))),
            fresh: Mutex::new(None),
            refreshed_identity: Mutex::new(None),
            checked_out: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            gate: None,
        }
    }
}
#[async_trait]
impl WorkspaceBackend for Workspace {
    async fn repository_identity_with_refresh(
        &self,
        _: PathBuf,
        refresh: bool,
    ) -> Result<Option<RepositoryIdentity>> {
        self.identity_refresh.lock().unwrap().push(refresh);
        if refresh && let Some(identity) = self.refreshed_identity.lock().unwrap().clone() {
            return Ok(identity);
        }
        Ok(Some(identity()))
    }
    async fn branch_pull_request(
        &self,
        input: BranchPullRequestQuery,
    ) -> Result<Option<GitBranchPullRequest>> {
        self.queries.lock().unwrap().push(input.clone());
        if !input.fresh_git_state
            && let Some((entered, gate)) = &self.gate
        {
            entered.send(()).unwrap();
            gate.acquire().await.unwrap().forget();
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::Unsupported("branch lookup failed"));
        }
        if input.fresh_git_state
            && let Some(result) = self.fresh.lock().unwrap().clone()
        {
            return Ok(result);
        }
        Ok(self.detected.lock().unwrap().clone())
    }
    async fn checked_out_pull_request(&self, _: PathBuf, _: String, _: String) -> Result<bool> {
        Ok(self.checked_out.load(Ordering::SeqCst))
    }
    async fn prepare(
        &self,
        _: ProjectId,
        _: ThreadId,
        _: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        _: CancellationToken,
    ) -> Result<t3_orchestration::api::Workspace> {
        Err(Error::Unsupported("prepare"))
    }
    async fn remove(
        &self,
        _: t3_orchestration::api::Workspace,
        _: CancellationToken,
    ) -> Result<()> {
        Err(Error::Unsupported("remove"))
    }
    async fn current_branch(&self, _: PathBuf) -> Result<Option<String>> {
        Err(Error::Unsupported("current branch"))
    }
}
struct PullRequests {
    summary: Mutex<Option<PullRequestSummary>>,
    notifications: AtomicUsize,
    notified: mpsc::UnboundedSender<()>,
}
#[async_trait]
impl PullRequestBackend for PullRequests {
    async fn summary(&self, _: PullRequestRef) -> Result<PullRequestSummary> {
        self.summary
            .lock()
            .unwrap()
            .clone()
            .ok_or(Error::Unsupported("summary"))
    }
    async fn notify_linked_threads_changed(&self) -> Result<()> {
        self.notifications.fetch_add(1, Ordering::SeqCst);
        self.notified.send(()).unwrap();
        Ok(())
    }
    async fn refresh(
        &self,
        _: PathBuf,
        _: Vec<ThreadPullRequestLink>,
    ) -> Result<Vec<ThreadPullRequestLink>> {
        Err(Error::Unsupported(
            "application refresh must not replace discovery",
        ))
    }
    async fn discover(&self, _: PathBuf, _: String) -> Result<Vec<ThreadPullRequestLink>> {
        Err(Error::Unsupported(
            "application discover must not replace fresh branch validation",
        ))
    }
}

pub(super) async fn project(harness: &Harness, root: &str) -> Result<()> {
    let root = root.to_owned();
    harness.database.stores.transaction(move|db|projects::upsert(db,&serde_json::from_value(json!({"projectId":"project","title":"Project","workspaceRoot":root,"hasCustomTitle":false,"defaultModelSelection":null,"defaultThreadEnvMode":null,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"deletedAt":null}))?)).await
}
async fn thread(harness: &Harness, id: &str, changes: Value) -> Result<()> {
    harness.create(id).await?;
    let mut thread = json!(harness.reader.thread(ThreadId(id.into())).await?.unwrap());
    thread["branch"] = json!("feature");
    thread
        .as_object_mut()
        .unwrap()
        .extend(changes.as_object().unwrap().clone());
    harness.sink.commit(CommitRequest {receipt:None,decision:Decision {events:vec![serde_json::from_value(json!({"id":format!("metadata:{id}"),"threadId":id,"type":"thread.metadata-updated","occurredAt":TEST_TIME,"payload":thread}))?],..Default::default()},guard:CommitGuard::None}).await?;
    Ok(())
}
fn service(
    harness: &Harness,
    workspace: Arc<Workspace>,
    downstream: bool,
    change: LinkedThreadsChange,
) -> (
    Arc<ThreadPullRequestService>,
    Arc<Recorder>,
    Arc<PullRequests>,
    mpsc::UnboundedReceiver<()>,
) {
    let commands = Arc::new(Recorder {
        commands: Mutex::new(Vec::new()),
        downstream: downstream
            .then(|| Arc::new(harness.dispatcher.clone()) as Arc<dyn CommandDispatcher>),
    });
    let (notified, notifications) = mpsc::unbounded_channel();
    let pull_requests = Arc::new(PullRequests {
        summary: Mutex::new(None),
        notifications: AtomicUsize::new(0),
        notified,
    });
    (
        Arc::new(ThreadPullRequestService::new(ThreadPullRequestServices {
            projections: harness.reader.clone(),
            commands: commands.clone(),
            committer: harness.sink.clone(),
            workspaces: workspace,
            pull_requests: pull_requests.clone(),
            identities: Arc::new(Registry),
            repository_selector: Arc::new(|_| Some("owner/repo".into())),
            linked_threads_change: Arc::new(move |_| change),
            opportunistic_work: Arc::new(|| false),
            ids: Arc::new(SequenceIds::new("discovery")),
        })),
        commands,
        pull_requests,
        notifications,
    )
}
fn refresh(thread: Option<&str>, backfill: bool) -> PullRequestRefresh {
    PullRequestRefresh {
        thread_id: thread.map(|id| ThreadId(id.into())),
        refresh: false,
        backfill,
    }
}
fn reference() -> Value {
    json!({"projectId":"project","repository":"owner/repo","number":42,"url":"https://github.com/owner/repo/pull/42"})
}

#[tokio::test]
async fn discovery_groups_branches_confirms_fresh_identity_and_dispatches_guarded_associations()
-> Result<()> {
    let harness = Harness::new(None, false);
    project(&harness, "C:/repo").await?;
    thread(&harness, "one", json!({})).await?;
    thread(&harness, "two", json!({})).await?;
    let workspace = Arc::new(Workspace::new());
    let (service, commands, _, _) = service(
        &harness,
        workspace.clone(),
        false,
        LinkedThreadsChange::None,
    );
    service.refresh(refresh(None, false)).await?;
    assert_eq!(workspace.queries.lock().unwrap().len(), 2);
    assert_eq!(
        *workspace.identity_refresh.lock().unwrap(),
        vec![false, true]
    );
    let commands = commands.commands.lock().unwrap().clone();
    assert_eq!(commands.len(), 2);
    for command in commands {
        assert_eq!(command["expected"]["workspaceRoot"], "C:/repo");
        assert_eq!(command["expected"]["branch"], "feature");
        assert_eq!(command["branchPullRequest"], reference());
        assert!(command["snapshotSequence"].as_u64().unwrap() > 0);
    }
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn discovery_rejects_changed_branch_identity_and_project_root_after_host_io() -> Result<()> {
    for changed in ["branch", "identity", "project"] {
        let harness = Harness::new(None, false);
        project(&harness, "C:/repo").await?;
        thread(&harness, "thread", json!({})).await?;
        let mut workspace = Workspace::new();
        let (entered, mut reads) = mpsc::unbounded_channel();
        let gate = Arc::new(Semaphore::new(0));
        workspace.gate = Some((entered, gate.clone()));
        let workspace = Arc::new(workspace);
        if changed == "branch" {
            let mut fresh = branch("open");
            fresh.pull_request.number = 43;
            *workspace.fresh.lock().unwrap() = Some(Some(fresh));
        }
        if changed == "identity" {
            let mut other = identity();
            other.canonical_key.0 = "github.com/elsewhere/repo".into();
            *workspace.refreshed_identity.lock().unwrap() = Some(Some(other));
        }
        let (service, commands, _, _) =
            service(&harness, workspace, false, LinkedThreadsChange::None);
        let lookup = tokio::spawn({
            let service = service.clone();
            async move { service.refresh(refresh(None, false)).await }
        });
        reads.recv().await.unwrap();
        if changed == "project" {
            project(&harness, "C:/changed").await?;
        }
        gate.add_permits(1);
        lookup.await.unwrap()?;
        assert!(commands.commands.lock().unwrap().is_empty(), "{changed}");
        service.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn discovery_rejects_old_closed_prs_unless_checked_out_or_already_saved() -> Result<()> {
    for checked_out in [false, true] {
        let harness = Harness::new(None, false);
        project(&harness, "C:/repo").await?;
        thread(&harness, "thread", json!({})).await?;
        let workspace = Arc::new(Workspace::new());
        let mut old = branch("merged");
        old.merged_at = Some(t3_contracts::wire::WireDateTime(
            "2020-01-01T00:00:00.000Z".into(),
        ));
        *workspace.detected.lock().unwrap() = Some(old);
        workspace.checked_out.store(checked_out, Ordering::SeqCst);
        let (service, commands, _, _) = service(
            &harness,
            workspace.clone(),
            false,
            LinkedThreadsChange::None,
        );
        service.refresh(refresh(None, false)).await?;
        assert_eq!(
            commands.commands.lock().unwrap().len(),
            usize::from(checked_out)
        );
        if checked_out {
            assert!(workspace.queries.lock().unwrap()[1].fresh_git_state);
        }
        service.shutdown().await;
    }
    let harness = Harness::new(None, false);
    project(&harness, "C:/repo").await?;
    thread(&harness, "thread", json!({"branchPullRequest":reference()})).await?;
    let workspace = Arc::new(Workspace::new());
    let mut old = branch("merged");
    old.merged_at = Some(t3_contracts::wire::WireDateTime(
        "2020-01-01T00:00:00.000Z".into(),
    ));
    *workspace.detected.lock().unwrap() = Some(old);
    let (service, commands, _, _) = service(
        &harness,
        workspace.clone(),
        false,
        LinkedThreadsChange::None,
    );
    service.refresh(refresh(None, false)).await?;
    assert!(commands.commands.lock().unwrap().is_empty());
    assert_eq!(workspace.queries.lock().unwrap().len(), 1);
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn discovery_backfills_settled_threads_only_five_times_after_lookup_failures() -> Result<()> {
    let harness = Harness::new(None, false);
    project(&harness, "C:/repo").await?;
    thread(
        &harness,
        "thread",
        json!({"settledOverride":"settled","settledAt":TEST_TIME}),
    )
    .await?;
    let workspace = Arc::new(Workspace::new());
    workspace.fail.store(true, Ordering::SeqCst);
    let (service, commands, _, _) = service(
        &harness,
        workspace.clone(),
        false,
        LinkedThreadsChange::None,
    );
    service.refresh(refresh(None, true)).await?;
    for _ in 0..5 {
        service.refresh(refresh(None, false)).await?;
    }
    assert_eq!(workspace.queries.lock().unwrap().len(), 5);
    assert!(commands.commands.lock().unwrap().is_empty());
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn discovery_preserves_terminal_shared_checkout_associations_and_refreshes_children_only_on_demand()
-> Result<()> {
    let harness = Harness::new(None, false);
    project(&harness, "C:/repo").await?;
    thread(&harness,"thread",json!({"branchPullRequest":reference(),"lineage":{"rootThreadId":"parent","parentThreadId":"parent","relationshipToParent":"subagent"}})).await?;
    let workspace = Arc::new(Workspace::new());
    *workspace.detected.lock().unwrap() = None;
    let (service, commands, pull_requests, _) = service(
        &harness,
        workspace.clone(),
        false,
        LinkedThreadsChange::None,
    );
    let mut summary = sample("PullRequestSummary");
    summary["headBranch"] = json!("feature");
    summary["state"] = json!("merged");
    *pull_requests.summary.lock().unwrap() = Some(serde_json::from_value(summary)?);
    service.refresh(refresh(None, false)).await?;
    assert!(workspace.queries.lock().unwrap().is_empty());
    service.refresh(refresh(Some("thread"), false)).await?;
    assert_eq!(workspace.queries.lock().unwrap().len(), 1);
    assert!(commands.commands.lock().unwrap().is_empty());
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn discovery_shutdown_cancels_and_joins_pending_lookup_and_refuses_late_admission()
-> Result<()> {
    let harness = Harness::new(None, false);
    project(&harness, "C:/repo").await?;
    thread(&harness, "thread", json!({})).await?;
    let mut workspace = Workspace::new();
    let (entered, mut reads) = mpsc::unbounded_channel();
    workspace.gate = Some((entered, Arc::new(Semaphore::new(0))));
    let (service, commands, _, _) = service(
        &harness,
        Arc::new(workspace),
        false,
        LinkedThreadsChange::None,
    );
    let lookup = tokio::spawn({
        let service = service.clone();
        async move { service.refresh(refresh(None, false)).await }
    });
    reads.recv().await.unwrap();
    service.shutdown().await;
    assert!(matches!(lookup.await.unwrap(), Err(Error::Cancelled)));
    assert!(commands.commands.lock().unwrap().is_empty());
    assert!(matches!(
        service.refresh(refresh(None, false)).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(service.start().await, Err(Error::Cancelled)));
    service.shutdown().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn discovery_throttles_reordered_reader_notifications_and_parks_opportunistic_sweeps()
-> Result<()> {
    let harness = Harness::new(None, false);
    project(&harness, "C:/repo").await?;
    let workspace = Arc::new(Workspace::new());
    let (service, _, pull_requests, mut notified) = service(
        &harness,
        workspace.clone(),
        false,
        LinkedThreadsChange::Reordered,
    );
    service.start().await?;
    service.drain().await;
    let event = serde_json::from_value(
        json!({"id":"ended","threadId":"thread","type":"run.updated","occurredAt":TEST_TIME,"payload":run("completed",1,"completed")}),
    )?;
    service.process_event(&event).await?;
    notified.recv().await.unwrap();
    for _ in 0..5 {
        service.process_event(&event).await?;
    }
    assert!(notified.try_recv().is_err());
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    notified.recv().await.unwrap();
    assert_eq!(pull_requests.notifications.load(Ordering::SeqCst), 2);
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    service.drain().await;
    assert!(workspace.queries.lock().unwrap().is_empty());
    service.shutdown().await;
    Ok(())
}
