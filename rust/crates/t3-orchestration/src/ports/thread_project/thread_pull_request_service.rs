use crate::{
    api::*,
    error::{Error, Result},
    projections::{
        read::PullRequestThread, store::ProjectionStore, write::PullRequestIdentityRegistry,
    },
    runtime::threads::{NoPullRequestRechecks, ThreadProjectWorker, ThreadServiceScope, date_ms},
    stores::projects,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    environment::RepositoryIdentity,
    orchestration_project::OrchestrationProjectShell,
    pull_request::{PullRequestRef, PullRequestState},
    thread_pull_request::ThreadLinkedPullRequest,
};
use tokio::sync::{Mutex, Semaphore, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub type RepositorySelector = dyn Fn(&RepositoryIdentity) -> Option<String> + Send + Sync;
pub type LinkedThreadsChangeDetector =
    dyn Fn(&OrchestrationV2DomainEvent) -> LinkedThreadsChange + Send + Sync;
pub type OpportunisticWork = dyn Fn() -> bool + Send + Sync;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkedThreadsChange {
    Changed,
    Reordered,
    None,
}

pub struct ThreadPullRequestServices {
    pub projections: Arc<ProjectionStore>,
    pub commands: Arc<dyn CommandDispatcher>,
    pub committer: Arc<dyn EventCommitter>,
    pub workspaces: Arc<dyn WorkspaceBackend>,
    pub pull_requests: Arc<dyn PullRequestBackend>,
    pub identities: Arc<dyn PullRequestIdentityRegistry>,
    pub repository_selector: Arc<RepositorySelector>,
    pub linked_threads_change: Arc<LinkedThreadsChangeDetector>,
    pub opportunistic_work: Arc<OpportunisticWork>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone, Debug)]
pub struct PullRequestRefresh {
    pub thread_id: Option<ThreadId>,
    pub refresh: bool,
    pub backfill: bool,
}

struct Context {
    services: ThreadPullRequestServices,
    backfill: Mutex<BTreeMap<ThreadId, u8>>,
    rechecks: Mutex<NoPullRequestRechecks>,
    tasks: TaskTracker,
    cancellation: CancellationToken,
}

pub struct ThreadPullRequestService {
    context: Arc<Context>,
    worker: ThreadProjectWorker<Option<ThreadId>, PullRequestRefresh>,
    scope: ThreadServiceScope,
    tasks: TaskTracker,
    started: Mutex<bool>,
    reordered: watch::Sender<u64>,
}

pub fn project_workspace_matches_snapshot(
    current: Option<&OrchestrationProjectShell>,
    expected: &str,
) -> bool {
    current.is_some_and(|project| project.workspace_root.0 == expected)
}

fn same_pull_request(
    left: Option<&ThreadLinkedPullRequest>,
    right: Option<&ThreadLinkedPullRequest>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.project_id == right.project_id
                && left.repository.0.eq_ignore_ascii_case(&right.repository.0)
                && left.number == right.number
                && left.url == right.url
        }
        _ => false,
    }
}

fn legacy_reference(link: &ThreadLinkedPullRequest) -> Result<PullRequestRef> {
    Ok(serde_json::from_value(
        json!({"projectId":link.project_id,"repository":link.repository,"number":link.number}),
    )?)
}

fn request_state(request: &GitBranchPullRequest) -> Result<PullRequestState> {
    Ok(serde_json::from_value(json!(request.pull_request.state))?)
}

pub(crate) async fn pull_request_cwd(worktree: Option<&str>, root: &str) -> Result<PathBuf> {
    if let Some(worktree) = worktree {
        let git = PathBuf::from(worktree).join(".git");
        let exists = tokio::task::spawn_blocking(move || git.try_exists())
            .await
            .map_err(|error| Error::Service {
                operation: "pull request worktree lookup",
                detail: error.to_string(),
            })?;
        // Unreadable .git stays on the worktree, matching the source's failed-exists handling.
        if exists.unwrap_or(true) {
            return Ok(worktree.into());
        }
    }
    Ok(root.into())
}

impl ThreadPullRequestService {
    pub fn new(services: ThreadPullRequestServices) -> Self {
        let scope = ThreadServiceScope::default();
        let tasks = TaskTracker::new();
        let context = Arc::new(Context {
            services,
            backfill: Mutex::new(BTreeMap::new()),
            rechecks: Mutex::new(NoPullRequestRechecks::default()),
            tasks: tasks.clone(),
            cancellation: scope.cancellation(),
        });
        let work = context.clone();
        let worker = ThreadProjectWorker::new(
            1,
            |current: &mut PullRequestRefresh, next| {
                current.refresh |= next.refresh;
                current.backfill |= next.backfill;
            },
            move |request, _| {
                let context = work.clone();
                async move {
                    if let Err(error) = context.synchronize(request).await
                        && !matches!(error, Error::Cancelled)
                    {
                        eprintln!("thread pull request refresh failed: {error}");
                    }
                }
            },
        );
        Self {
            context,
            worker,
            scope,
            tasks,
            started: Mutex::new(false),
            reordered: watch::channel(0).0,
        }
    }

    pub async fn refresh(&self, request: PullRequestRefresh) -> Result<()> {
        self.scope.run(self.context.synchronize(request)).await
    }

    pub async fn enqueue(&self, request: PullRequestRefresh) -> Result<()> {
        self.scope
            .run(self.worker.enqueue(request.thread_id.clone(), request))
            .await
    }

    pub async fn drain(&self) {
        self.worker.drain().await;
    }

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        self.scope.run(async {
            let mut started = self.started.lock().await;
            if *started { return Ok(()); }
            let mut events = self.context.services.committer.subscribe();
            let mut reordered = self.reordered.subscribe();
            self.enqueue(PullRequestRefresh {thread_id:None,refresh:false,backfill:true}).await?;
            *started = true;
            let service = self.clone();
            self.tasks.spawn(async move {
                let cancellation = service.scope.cancellation();
                let mut interval = tokio::time::interval_at(tokio::time::Instant::now()+Duration::from_secs(60),Duration::from_secs(60));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    let result = tokio::select! { biased;
                        _ = cancellation.cancelled() => break,
                        _ = interval.tick() => {
                            if !(service.context.services.opportunistic_work)() { continue; }
                            service.enqueue(PullRequestRefresh {thread_id:None,refresh:false,backfill:false}).await
                        },
                        event = events.recv() => match event {
                            Ok(t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored)) => service.process_event(&stored.event).await,
                            Ok(_) => Ok(()),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => service.enqueue(PullRequestRefresh {thread_id:None,refresh:true,backfill:true}).await,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    };
                    if let Err(error) = result && !matches!(error,Error::Cancelled) { eprintln!("thread pull request event failed: {error}"); }
                }
            });
            let service = self.clone();
            self.tasks.spawn(async move {
                let cancellation = service.scope.cancellation();
                loop {
                    tokio::select! { biased; _ = cancellation.cancelled() => break, changed = reordered.changed() => {if changed.is_err(){break;} reordered.borrow_and_update();} }
                    if let Err(error) = service.scope.run(service.context.services.pull_requests.notify_linked_threads_changed()).await && !matches!(error,Error::Cancelled) { eprintln!("linked thread reorder notification failed: {error}"); }
                    tokio::select! { biased; _ = cancellation.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(10)) => {} }
                }
            });
            Ok(())
        }).await
    }

    pub async fn process_event(&self, event: &OrchestrationV2DomainEvent) -> Result<()> {
        self.scope
            .run(async {
                match (self.context.services.linked_threads_change)(event) {
                    LinkedThreadsChange::Changed => {
                        self.context
                            .services
                            .pull_requests
                            .notify_linked_threads_changed()
                            .await?
                    }
                    LinkedThreadsChange::Reordered => self
                        .reordered
                        .send_modify(|counter| *counter = counter.wrapping_add(1)),
                    LinkedThreadsChange::None => {}
                }
                let value = t3_contracts::stored::to_value(event)?;
                let refresh = match value["type"].as_str() {
                    Some("thread.created" | "thread.unarchived" | "thread.metadata-updated") => {
                        false
                    }
                    Some("thread.unsettled" | "checkpoint.captured") => true,
                    Some("run.updated")
                        if matches!(
                            value["payload"]["status"].as_str(),
                            Some("completed" | "failed" | "cancelled" | "interrupted")
                        ) =>
                    {
                        true
                    }
                    _ => return Ok(()),
                };
                self.enqueue(PullRequestRefresh {
                    thread_id: Some(serde_json::from_value(value["threadId"].clone())?),
                    refresh,
                    backfill: false,
                })
                .await
            })
            .await
    }

    pub async fn shutdown(&self) {
        self.scope.shutdown().await;
        self.worker.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
    }
}

impl Context {
    async fn fail_backfill(&self, threads: &[PullRequestThread]) {
        let mut pending = self.backfill.lock().await;
        for thread in threads {
            if let Some(left) = pending.get_mut(&thread.id) {
                *left -= 1;
                if *left == 0 {
                    pending.remove(&thread.id);
                }
            }
        }
    }

    async fn finish_backfill(&self, threads: &[PullRequestThread]) {
        let mut pending = self.backfill.lock().await;
        for thread in threads {
            pending.remove(&thread.id);
        }
    }

    fn matches_project(
        &self,
        request: &GitBranchPullRequest,
        identity: Option<&RepositoryIdentity>,
    ) -> bool {
        request
            .repository_key
            .as_ref()
            .zip(identity)
            .is_some_and(|(key, identity)| {
                self.services.identities.canonical_repository_key(key)
                    == self
                        .services
                        .identities
                        .canonical_repository_key(&identity.canonical_key.0)
            })
    }

    async fn synchronize(self: &Arc<Self>, request: PullRequestRefresh) -> Result<()> {
        let pending = !self.backfill.lock().await.is_empty();
        let unsettled_only = request.thread_id.is_none() && !(request.backfill || pending);
        let (snapshot, projects) = tokio::try_join!(
            self.services
                .projections
                .pull_request_snapshot(request.thread_id.clone(), unsettled_only),
            self.services
                .projections
                .database
                .read(|db| projects::list_shells(db, None))
        )?;
        let now = self.services.clock.now();
        let now_ms = date_ms(&now.0).ok_or_else(|| Error::Decode {
            entity: "discovery clock",
            detail: now.0.clone(),
        })?;
        let projects: Arc<BTreeMap<_, _>> = Arc::new(
            projects
                .into_iter()
                .map(|project| (project.id.clone(), project))
                .collect(),
        );
        let threads: Vec<_> = snapshot
            .threads
            .into_iter()
            .filter(|thread| {
                request.thread_id.is_some()
                    || json!(thread.lineage)["relationshipToParent"] != "subagent"
            })
            .collect();
        let mut pending = self.backfill.lock().await;
        if request.backfill {
            for thread in &threads {
                if (json!(thread.settled_override) == "settled" || thread.settled_at.is_some())
                    && thread
                        .branch_pull_request
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_none()
                {
                    pending.insert(thread.id.clone(), 5);
                }
            }
        }
        let visible: BTreeSet<_> = threads
            .iter()
            .filter(|thread| thread.branch.is_some())
            .map(|thread| thread.id.clone())
            .collect();
        if let Some(id) = &request.thread_id {
            if !visible.contains(id) {
                pending.remove(id);
            }
        } else {
            pending.retain(|id, _| visible.contains(id));
        }
        let mut groups: BTreeMap<String, Vec<PullRequestThread>> = BTreeMap::new();
        for thread in threads {
            if thread.archived_at.is_some()
                || ((json!(thread.settled_override) == "settled" || thread.settled_at.is_some())
                    && request.thread_id.is_none()
                    && !pending.contains_key(&thread.id))
                || (thread.branch.is_none()
                    && thread
                        .branch_pull_request
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_none())
            {
                continue;
            }
            groups
                .entry(json!([thread.project_id, thread.worktree_path, thread.branch]).to_string())
                .or_default()
                .push(thread);
        }
        let scheduled: Vec<_> = if request.thread_id.is_some() {
            Vec::new()
        } else {
            groups
                .iter()
                .filter(|(_, group)| {
                    group[0].branch.is_some()
                        && group.iter().all(|thread| {
                            thread
                                .branch_pull_request
                                .as_ref()
                                .and_then(Option::as_ref)
                                .is_none()
                                && !pending.contains_key(&thread.id)
                        })
                })
                .map(|(key, group)| {
                    (
                        key.clone(),
                        group
                            .iter()
                            .filter_map(|thread| date_ms(&thread.updated_at.0))
                            .max()
                            .unwrap_or_default(),
                    )
                })
                .collect()
        };
        drop(pending);
        let scheduled_keys: BTreeSet<_> = scheduled.iter().map(|(key, _)| key.clone()).collect();
        let selected = self.rechecks.lock().await.select(now_ms, scheduled);
        let retained: Vec<_> = groups.keys().cloned().collect();
        let cancellation = self.cancellation.child_token();
        let _cancel = cancellation.clone().drop_guard();
        let slots = Arc::new(Semaphore::new(32));
        let mut jobs = Vec::new();
        for (key, group) in groups {
            if scheduled_keys.contains(&key) && !selected.contains(&key) {
                continue;
            }
            let permit = tokio::select! { biased; _ = cancellation.cancelled() => return Err(Error::Cancelled), permit = slots.clone().acquire_owned() => permit.map_err(|_|Error::Cancelled)? };
            let context = self.clone();
            let projects = projects.clone();
            let cancellation = cancellation.clone();
            let request = request.clone();
            let sequence = snapshot.snapshot_sequence;
            jobs.push(self.tasks.spawn(async move {
                let _permit = permit;
                tokio::select! { biased;
                    _ = cancellation.cancelled() => Err(Error::Cancelled),
                    result = context.synchronize_group(&request,&group,&projects,sequence,&key,now_ms) => {
                        if let Err(error) = &result && !matches!(error,Error::Cancelled) { context.fail_backfill(&group).await; }
                        result
                    }
                }
            }));
        }
        for job in jobs {
            match job.await {
                Ok(Ok(())) => {}
                Ok(Err(Error::Cancelled)) => return Err(Error::Cancelled),
                Ok(Err(error)) => eprintln!("thread branch pull request lookup failed: {error}"),
                Err(error) => eprintln!("thread pull request lookup task failed: {error}"),
            }
        }
        if request.thread_id.is_none() {
            self.rechecks.lock().await.retain(retained);
        }
        Ok(())
    }

    async fn synchronize_group(
        &self,
        request: &PullRequestRefresh,
        group: &[PullRequestThread],
        projects: &BTreeMap<ProjectId, OrchestrationProjectShell>,
        sequence: u64,
        group_key: &str,
        now_ms: i64,
    ) -> Result<()> {
        let first = &group[0];
        let Some(project) = projects.get(&first.project_id) else {
            self.finish_backfill(group).await;
            return Ok(());
        };
        let identity = self
            .services
            .workspaces
            .repository_identity_with_refresh(
                project.workspace_root.0.clone().into(),
                request.refresh,
            )
            .await?;
        let repository = identity
            .as_ref()
            .and_then(|identity| (self.services.repository_selector)(identity));
        if first.branch.is_some() && repository.is_none() {
            self.finish_backfill(group).await;
            return Ok(());
        }
        let cwd = pull_request_cwd(
            first.worktree_path.as_ref().map(|path| path.0.as_str()),
            &project.workspace_root.0,
        )
        .await?;
        let detected = match &first.branch {
            Some(branch) => {
                self.services
                    .workspaces
                    .branch_pull_request(BranchPullRequestQuery {
                        cwd: cwd.clone(),
                        branch: branch.0.clone(),
                        refresh: request.refresh,
                        confirm_terminal: false,
                        fresh_git_state: false,
                    })
                    .await?
            }
            None => None,
        };
        if first.branch.is_some() {
            self.rechecks
                .lock()
                .await
                .record(group_key.into(), now_ms, detected.is_some());
        }
        if detected
            .as_ref()
            .is_some_and(|detected| !self.matches_project(detected, identity.as_ref()))
        {
            self.finish_backfill(group).await;
            return Ok(());
        }
        let reference: Option<ThreadLinkedPullRequest> = detected.as_ref().zip(repository).map(|(detected,repository)| serde_json::from_value(json!({"projectId":project.id,"repository":repository,"number":detected.pull_request.number,"url":detected.pull_request.url}))).transpose()?;
        let historical: BTreeSet<_> = group
            .iter()
            .filter(|thread| {
                detected.as_ref().is_some_and(|detected| {
                    let terminal_at = match request_state(detected) {
                        Ok(PullRequestState::Merged) => detected.merged_at.as_ref(),
                        Ok(PullRequestState::Closed) => detected.closed_at.as_ref(),
                        _ => None,
                    };
                    terminal_at
                        .and_then(|at| date_ms(&at.0))
                        .zip(date_ms(&thread.created_at.0))
                        .is_some_and(|(at, created)| at < created)
                        && !same_pull_request(
                            thread.branch_pull_request.as_ref().and_then(Option::as_ref),
                            reference.as_ref(),
                        )
                })
            })
            .map(|thread| thread.id.clone())
            .collect();
        let drops_historical =
            if let (Some(detected), Some(branch)) = (detected.as_ref(), first.branch.as_ref()) {
                !historical.is_empty()
                    && !self
                        .services
                        .workspaces
                        .checked_out_pull_request(
                            cwd.clone(),
                            branch.0.clone(),
                            detected.pull_request.url.clone(),
                        )
                        .await?
            } else {
                false
            };
        let mut updates = Vec::new();
        for thread in group {
            let plan: Result<_> = async {
                let previous = thread.branch_pull_request.as_ref().and_then(Option::as_ref);
                let linked = thread.linked_pull_request.as_ref().and_then(Option::as_ref);
                let mut branch = if drops_historical && historical.contains(&thread.id) {
                    None
                } else {
                    reference.clone()
                };
                if branch.is_none()
                    && thread.branch.is_some()
                    && thread.worktree_path.is_none()
                    && let Some(previous) = previous
                {
                    let summary = self
                        .services
                        .pull_requests
                        .summary(legacy_reference(previous)?)
                        .await?;
                    if thread
                        .branch
                        .as_ref()
                        .is_some_and(|branch| branch.0 == summary.head_branch.0)
                        && matches!(
                            summary.state,
                            PullRequestState::Merged | PullRequestState::Closed
                        )
                    {
                        branch = Some(previous.clone());
                    }
                }
                let mut replacement = None;
                if thread.pull_requests.is_empty()
                    && let Some(linked) = linked
                    && detected.as_ref().is_some_and(|detected| {
                        request_state(detected).is_ok_and(|state| state == PullRequestState::Open)
                    })
                    && reference.is_some()
                    && !same_pull_request(Some(linked), reference.as_ref())
                {
                    let summary = self
                        .services
                        .pull_requests
                        .summary(legacy_reference(linked)?)
                        .await?;
                    if matches!(
                        summary.state,
                        PullRequestState::Merged | PullRequestState::Closed
                    ) {
                        replacement = reference.clone();
                    }
                }
                Ok((branch, replacement))
            }
            .await;
            match plan {
                Ok((branch, replacement)) => {
                    if same_pull_request(
                        thread.branch_pull_request.as_ref().and_then(Option::as_ref),
                        branch.as_ref(),
                    ) && replacement.is_none()
                    {
                        self.backfill.lock().await.remove(&thread.id);
                    } else {
                        updates.push((thread, branch, replacement));
                    }
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    self.fail_backfill(std::slice::from_ref(thread)).await;
                    eprintln!("thread pull request discovery failed: {error}");
                }
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        if let (Some(detected), Some(branch)) = (detected.as_ref(), first.branch.as_ref()) {
            let current = self
                .services
                .workspaces
                .branch_pull_request(BranchPullRequestQuery {
                    cwd,
                    branch: branch.0.clone(),
                    refresh: false,
                    confirm_terminal: true,
                    fresh_git_state: true,
                })
                .await?;
            let identity = self
                .services
                .workspaces
                .repository_identity_with_refresh(project.workspace_root.0.clone().into(), true)
                .await?;
            if current.as_ref().is_none_or(|current| {
                current.pull_request.number != detected.pull_request.number
                    || current.pull_request.url != detected.pull_request.url
                    || current.pull_request.state != detected.pull_request.state
                    || current.repository_key != detected.repository_key
                    || !self.matches_project(current, identity.as_ref())
            }) {
                for (thread, _, _) in updates {
                    self.fail_backfill(std::slice::from_ref(thread)).await;
                }
                return Ok(());
            }
        }
        for (thread, branch, replacement) in updates {
            let project_id = project.id.clone();
            let current = self
                .services
                .projections
                .database
                .read(move |db| projects::get(db, &project_id, false))
                .await?;
            if current
                .as_ref()
                .is_none_or(|current| current.workspace_root != project.workspace_root.0)
            {
                self.fail_backfill(std::slice::from_ref(thread)).await;
                continue;
            }
            let mut command = json!({"type":"thread.pull-request.sync","commandId":format!("server:thread-pull-request:{}:{}",thread.id.0,self.services.ids.next(IdKind::Command)),"threadId":thread.id,"projectId":project.id,"snapshotSequence":sequence,"expected":{"workspaceRoot":project.workspace_root,"branch":thread.branch,"worktreePath":thread.worktree_path,"linkedPullRequest":thread.linked_pull_request.clone().flatten(),"branchPullRequest":thread.branch_pull_request.clone().flatten()},"branchPullRequest":branch});
            if let Some(replacement) = replacement {
                command["linkedPullRequest"] = json!(replacement);
            }
            match self
                .services
                .commands
                .dispatch(serde_json::from_value(command)?)
                .await
            {
                Ok(_) => {
                    self.backfill.lock().await.remove(&thread.id);
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    self.fail_backfill(std::slice::from_ref(thread)).await;
                    eprintln!("thread pull request update failed: {error}");
                }
            }
        }
        Ok(())
    }
}
