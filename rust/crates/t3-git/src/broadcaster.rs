use crate::{CancellationToken, GitError, GitWorkflowService};
use async_trait::async_trait;
use notify::{RecursiveMode, Watcher};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_contracts::git::*;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};

pub const DEFAULT_VCS_FETCH_INTERVAL: Duration = Duration::from_secs(30);

#[async_trait]
pub trait VcsStatusPolicy: Send + Sync {
    fn automatic_fetch_interval(&self) -> Duration {
        DEFAULT_VCS_FETCH_INTERVAL
    }
    async fn should_run(&self, _cwd: &Path) -> bool {
        true
    }
    async fn auto_pull_enabled(&self, _cwd: &Path) -> bool {
        false
    }
}
struct DefaultPolicy;
#[async_trait]
impl VcsStatusPolicy for DefaultPolicy {}
pub struct DisabledVcsPolling;
#[async_trait]
impl VcsStatusPolicy for DisabledVcsPolling {
    fn automatic_fetch_interval(&self) -> Duration {
        Duration::ZERO
    }
}

#[derive(Clone)]
struct RemoteRead {
    branch: Option<String>,
    value: Option<VcsStatusRemoteResult>,
}
#[derive(Default)]
struct Cached {
    local: Option<VcsStatusLocalResult>,
    remote: Option<RemoteRead>,
    branch: Option<VcsBranchStatus>,
    revision: u64,
    refresh: Option<(u64, VcsStatusLocalResult)>,
    remote_revision: u64,
}
struct Worker {
    cancellation: CancellationToken,
    finished: watch::Receiver<bool>,
    ready: watch::Receiver<bool>,
}
impl Worker {
    fn cancel(&self) {
        self.cancellation.cancel();
    }
    async fn wait(&self) {
        let _ = self.finished.clone().wait_for(|done| *done).await;
    }
}
struct Completion(watch::Sender<bool>);
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
#[derive(Default)]
struct Listeners {
    next: u64,
    subscribers: usize,
    branches: BTreeMap<u64, mpsc::UnboundedSender<(u64, VcsBranchStatus)>>,
    statuses: BTreeMap<u64, mpsc::UnboundedSender<VcsStatusStreamEvent>>,
    remote_demand: BTreeMap<u64, PathBuf>,
    head: Option<Arc<Worker>>,
    poller: Option<Arc<Worker>>,
    retiring: Vec<Arc<Worker>>,
}
struct Checkout {
    workflow: GitWorkflowService,
    policy: Arc<dyn VcsStatusPolicy>,
    cached: AsyncMutex<Cached>,
    local_lock: AsyncMutex<()>,
    remote_lock: AsyncMutex<()>,
    requests: AtomicU64,
    detailed: AtomicUsize,
    listeners: Mutex<Listeners>,
}
impl Checkout {
    fn listeners(&self) -> std::sync::MutexGuard<'_, Listeners> {
        self.listeners.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn publish(&self, event: VcsStatusStreamEvent) {
        self.listeners()
            .statuses
            .retain(|_, sender| sender.send(event.clone()).is_ok());
    }
    fn publish_branch(&self, cached: &mut Cached, branch: VcsBranchStatus) {
        if cached.branch.as_ref() == Some(&branch) {
            return;
        }
        cached.revision += 1;
        cached.branch = Some(branch.clone());
        self.listeners()
            .branches
            .retain(|_, sender| sender.send((cached.revision, branch.clone())).is_ok());
    }
    async fn update_local(
        &self,
        local: VcsStatusLocalResult,
        publish: bool,
    ) -> VcsStatusLocalResult {
        let mut cached = self.cached.lock().await;
        let branch = local.ref_name.as_ref().map(|name| name.0.clone());
        if cached
            .remote
            .as_ref()
            .is_some_and(|remote| remote.branch != branch)
        {
            cached.remote = None;
            if publish {
                self.publish(remote_event(None));
            }
        }
        self.publish_branch(
            &mut cached,
            VcsBranchStatus {
                ref_name: branch,
                is_repo: Some(Some(local.is_repo)),
            },
        );
        let changed = cached.local.as_ref() != Some(&local);
        cached.local = Some(local.clone());
        if publish && changed {
            self.publish(local_event(local.clone()));
        }
        local
    }
    async fn local_unlocked(
        &self,
        workflow: &GitWorkflowService,
    ) -> Result<VcsStatusLocalResult, GitError> {
        let cached = self.cached.lock().await.local.clone();
        if let Some(local) = cached {
            if workflow.current_branch().await?
                == local.ref_name.as_ref().map(|name| name.0.clone())
            {
                return Ok(local);
            }
            workflow.invalidate_local_status().await;
            return Ok(self
                .update_local(workflow.local_status().await?, true)
                .await);
        }
        Ok(self
            .update_local(workflow.local_status().await?, false)
            .await)
    }
    async fn local(&self, workflow: &GitWorkflowService) -> Result<VcsStatusLocalResult, GitError> {
        let _lock = self.local_lock.lock().await;
        self.local_unlocked(workflow).await
    }
    async fn refresh_local(
        &self,
        workflow: &GitWorkflowService,
    ) -> Result<VcsStatusLocalResult, GitError> {
        let requested = self.requests.fetch_add(1, Ordering::AcqRel) + 1;
        let _lock = self.local_lock.lock().await;
        if let Some((started, local)) = &self.cached.lock().await.refresh
            && *started >= requested
        {
            return Ok(local.clone());
        }
        let started = self.requests.load(Ordering::Acquire);
        workflow.invalidate_local_status().await;
        let local = workflow.local_status().await?;
        self.cached.lock().await.refresh = Some((started, local.clone()));
        Ok(self.update_local(local, true).await)
    }
    async fn branch(
        &self,
        workflow: &GitWorkflowService,
    ) -> Result<(u64, VcsBranchStatus), GitError> {
        let _lock = self.local_lock.lock().await;
        if self.detailed.load(Ordering::Acquire) > 0 {
            self.local_unlocked(workflow).await?;
        } else {
            let ref_name = workflow.current_branch().await?;
            let is_repo = ref_name.is_some() || workflow.repository_root().await?.is_some();
            workflow.invalidate_local_status().await;
            let mut cached = self.cached.lock().await;
            cached.local = None;
            cached.refresh = None;
            drop(cached);
            if self.detailed.load(Ordering::Acquire) > 0 {
                self.update_local(workflow.local_status().await?, true)
                    .await;
            } else {
                self.publish_branch(
                    &mut *self.cached.lock().await,
                    VcsBranchStatus {
                        ref_name,
                        is_repo: Some(Some(is_repo)),
                    },
                );
            }
        }
        let cached = self.cached.lock().await;
        cached
            .branch
            .clone()
            .map(|branch| (cached.revision, branch))
            .ok_or_else(|| GitError::InvalidOutput("branch read did not produce a value".into()))
    }
    async fn read_remote(
        &self,
        workflow: &GitWorkflowService,
        fetch: bool,
    ) -> Result<Option<RemoteRead>, GitError> {
        let branch = workflow.current_branch().await?;
        let revision = self.cached.lock().await.remote_revision;
        workflow.invalidate_remote_status().await;
        let mut value = workflow.remote_status(fetch).await?;
        if self.cached.lock().await.remote_revision != revision
            && let Some(value) = &mut value
            && value.has_upstream
        {
            let details = workflow.git().status_details_remote(false).await?;
            if details.branch == branch {
                value.ahead_count = details.ahead_count;
                value.behind_count = details.behind_count;
            }
        }
        let current = workflow.current_branch().await?;
        let local_branch = self
            .cached
            .lock()
            .await
            .local
            .as_ref()
            .and_then(|local| local.ref_name.as_ref().map(|name| name.0.clone()));
        if branch != current || local_branch != current {
            self.refresh_local(workflow).await?;
        }
        Ok((branch == current).then_some(RemoteRead { branch, value }))
    }
    async fn update_remote(
        &self,
        remote: Option<RemoteRead>,
        publish: bool,
    ) -> Option<VcsStatusRemoteResult> {
        let mut cached = self.cached.lock().await;
        let local_branch = cached
            .local
            .as_ref()
            .and_then(|local| local.ref_name.as_ref().map(|name| name.0.clone()));
        let remote = remote.filter(|remote| remote.branch == local_branch);
        let changed = match (&cached.remote, &remote) {
            (Some(left), Some(right)) => left.value != right.value,
            (None, None) => false,
            _ => true,
        };
        let value = remote.as_ref().and_then(|remote| remote.value.clone());
        cached.remote = remote;
        cached.remote_revision += 1;
        if changed && publish {
            self.publish(remote_event(value.clone()));
        }
        value
    }
    async fn remote(
        &self,
        workflow: &GitWorkflowService,
        fetch: bool,
        invalidate: bool,
        publish: bool,
    ) -> Result<Option<VcsStatusRemoteResult>, GitError> {
        let _lock = self.remote_lock.lock().await;
        if invalidate {
            workflow.invalidate_status().await;
        }
        let mut read = self.read_remote(workflow, fetch).await?;
        let should_pull = read
            .as_ref()
            .and_then(|r| r.value.as_ref())
            .is_some_and(|r| r.has_upstream && r.ahead_count == 0 && r.behind_count > 0);
        if fetch && should_pull {
            let demand: Vec<_> = self.listeners().remote_demand.values().cloned().collect();
            let demand = if demand.is_empty() {
                vec![workflow.git().cwd().to_owned()]
            } else {
                demand
            };
            let mut enabled = false;
            for cwd in demand {
                enabled |= self.policy.auto_pull_enabled(&cwd).await;
            }
            if enabled {
                workflow.invalidate_local_status().await;
                let local = workflow.local_status().await?;
                if local.is_repo
                    && local.is_default_ref
                    && !local.has_working_tree_changes
                    && local.ref_name.as_ref().map(|name| &name.0)
                        == read.as_ref().and_then(|r| r.branch.as_ref())
                {
                    // A failed pull can still move refs, so reconcile both outcomes.
                    let _ = workflow.git().pull_current_branch().await;
                    workflow.invalidate_status().await;
                    self.refresh_local(workflow).await?;
                    read = self.read_remote(workflow, false).await?;
                }
            }
        }
        self.refresh_local(workflow).await?;
        Ok(self.update_remote(read, publish).await)
    }
    async fn refresh_checked_out(&self, workflow: &GitWorkflowService) -> Result<(), GitError> {
        if self.detailed.load(Ordering::Acquire) == 0 {
            self.branch(workflow).await?;
        } else if self.cached.lock().await.remote.is_some() {
            self.remote(workflow, false, false, true).await?;
        } else {
            self.refresh_local(workflow).await?;
        }
        Ok(())
    }
    fn spawn_worker(self: &Arc<Self>, head: bool, immediate: bool) -> Arc<Worker> {
        let cancellation = CancellationToken::new();
        let token = cancellation.clone();
        let (finished_tx, finished) = watch::channel(false);
        let (ready_tx, ready) = watch::channel(false);
        let completion = Completion(finished_tx);
        let checkout = self.clone();
        tokio::spawn(async move {
            let _completion = completion;
            let workflow = checkout.workflow.clone().with_cancellation(token.clone());
            if head {
                checkout.watch_head(&workflow, &token, ready_tx).await;
            } else {
                ready_tx.send_replace(true);
                checkout.poll_remote(&workflow, &token, immediate).await;
            }
            workflow.git().drain_operations().await;
        });
        Arc::new(Worker {
            cancellation,
            finished,
            ready,
        })
    }
    async fn watch_head(
        &self,
        workflow: &GitWorkflowService,
        token: &CancellationToken,
        ready: watch::Sender<bool>,
    ) {
        let path = workflow.git().head_path().await;
        let (sender, mut receiver) = mpsc::channel(1);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event
                    && !matches!(event.kind, notify::EventKind::Access(_))
                    && (matches!(event.kind, notify::EventKind::Remove(_))
                        || event
                            .paths
                            .iter()
                            .any(|path| path.file_name().is_some_and(|name| name == "HEAD")))
                {
                    let _ = sender.try_send(event);
                }
            })
            .ok();
        let directory = path.as_ref().and_then(|path| path.parent());
        let watching = match (&mut watcher, directory) {
            (Some(watcher), Some(directory)) => watcher
                .watch(directory, RecursiveMode::NonRecursive)
                .is_ok(),
            _ => false,
        };
        ready.send_replace(true);
        if !watching {
            return;
        }
        let mut failures = 0u32;
        loop {
            let event =
                tokio::select! { _ = token.cancelled() => break, event = receiver.recv() => event };
            let Some(event) = event else {
                break;
            };
            if directory.is_some_and(|dir| !dir.exists()) {
                break;
            }
            if !event
                .paths
                .iter()
                .any(|event_path| event_path.file_name().is_some_and(|name| name == "HEAD"))
            {
                continue;
            }
            loop {
                if token.is_cancelled() {
                    return;
                }
                match self.refresh_checked_out(workflow).await {
                    Ok(()) => {
                        failures = 0;
                        break;
                    }
                    Err(_) if token.is_cancelled() => return,
                    Err(_) => {
                        let delay = Duration::from_secs(
                            1u64.checked_shl(failures.min(5)).unwrap_or(30).min(30),
                        );
                        failures = failures.saturating_add(1);
                        tokio::select! { _ = token.cancelled() => return, _ = tokio::time::sleep(delay) => {} }
                    }
                }
            }
        }
    }
    async fn poll_remote(
        &self,
        workflow: &GitWorkflowService,
        token: &CancellationToken,
        mut initial: bool,
    ) {
        let mut failures = 0u32;
        let mut delay = if initial {
            Duration::ZERO
        } else {
            active_interval(self.policy.automatic_fetch_interval())
        };
        loop {
            tokio::select! { _ = token.cancelled() => return, _ = tokio::time::sleep(delay) => {} }
            let interval = self.policy.automatic_fetch_interval();
            let active = active_interval(interval);
            let demand: Vec<_> = self.listeners().remote_demand.values().cloned().collect();
            let mut enabled = initial;
            if !interval.is_zero() {
                for cwd in demand {
                    enabled |= self.policy.should_run(&cwd).await;
                }
            }
            if interval.is_zero() && !initial || !enabled {
                delay = active;
                continue;
            }
            match self
                .remote(workflow, !interval.is_zero(), false, true)
                .await
            {
                Ok(_) => {
                    initial = false;
                    failures = 0;
                    delay = active;
                }
                Err(_) if token.is_cancelled() => return,
                Err(_) => {
                    failures = failures.saturating_add(1);
                    delay = refresh_failure_delay(failures, active);
                }
            }
        }
    }
    async fn retain(self: &Arc<Self>, detailed: bool, remote: bool, raw: &Path) -> Lease {
        let (id, head) = {
            let mut listeners = self.listeners();
            listeners.next += 1;
            let id = listeners.next;
            listeners.subscribers += 1;
            if detailed {
                self.detailed.fetch_add(1, Ordering::AcqRel);
            }
            if remote {
                listeners.remote_demand.insert(id, raw.to_owned());
            }
            if listeners
                .head
                .as_ref()
                .is_none_or(|worker| *worker.finished.borrow())
            {
                if let Some(old) = listeners.head.take() {
                    listeners.retiring.push(old);
                }
                listeners.head = Some(self.spawn_worker(true, false));
            }
            (id, listeners.head.clone())
        };
        let lease = Lease {
            checkout: self.clone(),
            id,
            detailed,
            remote,
        };
        if let Some(head) = head {
            let _ = head.ready.clone().wait_for(|ready| *ready).await;
        }
        lease
    }
    fn start_polling(self: &Arc<Self>, immediate: bool) {
        let mut listeners = self.listeners();
        listeners
            .retiring
            .retain(|worker| !*worker.finished.borrow());
        if listeners.poller.is_none() && !listeners.remote_demand.is_empty() {
            listeners.poller = Some(self.spawn_worker(false, immediate));
        }
    }
    async fn drain_retired(&self) {
        let retired = std::mem::take(&mut self.listeners().retiring);
        for worker in retired {
            worker.wait().await;
        }
    }
}
fn active_interval(interval: Duration) -> Duration {
    if interval.is_zero() {
        DEFAULT_VCS_FETCH_INTERVAL
    } else {
        interval
    }
}
pub fn refresh_failure_delay(failures: u32, interval: Duration) -> Duration {
    interval.max(Duration::from_secs(
        30u64
            .saturating_mul(
                1u64.checked_shl(failures.saturating_sub(1))
                    .unwrap_or(u64::MAX),
            )
            .min(900),
    ))
}
fn local_event(local: VcsStatusLocalResult) -> VcsStatusStreamEvent {
    VcsStatusStreamEvent::LocalUpdated(VcsStatusStreamEventMember2 {
        _tag: VcsStatusStreamEventMember2Tag::LocalUpdated,
        local,
    })
}
fn remote_event(remote: Option<VcsStatusRemoteResult>) -> VcsStatusStreamEvent {
    VcsStatusStreamEvent::RemoteUpdated(VcsStatusStreamEventMember3 {
        _tag: VcsStatusStreamEventMember3Tag::RemoteUpdated,
        remote,
    })
}
pub(crate) fn merge_status(
    local: VcsStatusLocalResult,
    remote: Option<VcsStatusRemoteResult>,
) -> VcsStatusResult {
    VcsStatusResult {
        is_repo: local.is_repo,
        source_control_provider: local.source_control_provider,
        has_primary_remote: local.has_primary_remote,
        is_default_ref: local.is_default_ref,
        ref_name: local.ref_name,
        has_working_tree_changes: local.has_working_tree_changes,
        working_tree: VcsStatusResultWorkingTree {
            root_path: local.working_tree.root_path,
            files: local
                .working_tree
                .files
                .into_iter()
                .map(|f| VcsStatusResultWorkingTreeFiles {
                    path: f.path,
                    insertions: f.insertions,
                    deletions: f.deletions,
                })
                .collect(),
            insertions: local.working_tree.insertions,
            deletions: local.working_tree.deletions,
        },
        branch_changes: local.branch_changes.flatten().map(|b| {
            Some(VcsStatusResultBranchChanges {
                base_ref: b.base_ref,
                insertions: b.insertions,
                deletions: b.deletions,
            })
        }),
        has_upstream: remote.as_ref().is_some_and(|r| r.has_upstream),
        ahead_count: remote.as_ref().map_or(0, |r| r.ahead_count),
        behind_count: remote.as_ref().map_or(0, |r| r.behind_count),
        ahead_of_default_count: remote.as_ref().and_then(|r| r.ahead_of_default_count),
        pr: remote.and_then(|r| r.pr),
    }
}
struct Lease {
    checkout: Arc<Checkout>,
    id: u64,
    detailed: bool,
    remote: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut listeners = self.checkout.listeners();
        listeners.branches.remove(&self.id);
        listeners.statuses.remove(&self.id);
        listeners.subscribers = listeners.subscribers.saturating_sub(1);
        if self.detailed {
            self.checkout.detailed.fetch_sub(1, Ordering::AcqRel);
        }
        if self.remote {
            listeners.remote_demand.remove(&self.id);
        }
        if listeners.remote_demand.is_empty()
            && let Some(worker) = listeners.poller.take()
        {
            worker.cancel();
            listeners.retiring.push(worker);
        }
        if listeners.subscribers == 0
            && let Some(worker) = listeners.head.take()
        {
            worker.cancel();
            listeners.retiring.push(worker);
        }
    }
}
pub struct VcsBranchSubscription {
    initial: Option<VcsBranchStatus>,
    revision: u64,
    receiver: mpsc::UnboundedReceiver<(u64, VcsBranchStatus)>,
    lease: Lease,
}
impl VcsBranchSubscription {
    pub async fn recv(&mut self) -> Option<VcsBranchStatus> {
        if let Some(initial) = self.initial.take() {
            return Some(initial);
        }
        while let Some((revision, branch)) = self.receiver.recv().await {
            if revision > self.revision {
                self.revision = revision;
                return Some(branch);
            }
        }
        None
    }
    pub async fn close(self) {
        let checkout = self.lease.checkout.clone();
        drop(self);
        checkout.drain_retired().await;
    }
}
pub struct VcsStatusSubscription {
    initial: Option<VcsStatusStreamEvent>,
    receiver: mpsc::UnboundedReceiver<VcsStatusStreamEvent>,
    lease: Lease,
}
impl VcsStatusSubscription {
    pub async fn recv(&mut self) -> Option<VcsStatusStreamEvent> {
        if let Some(initial) = self.initial.take() {
            Some(initial)
        } else {
            self.receiver.recv().await
        }
    }
    pub async fn close(self) {
        let checkout = self.lease.checkout.clone();
        drop(self);
        checkout.drain_retired().await;
    }
}
struct Inner {
    workflow: GitWorkflowService,
    policy: Arc<dyn VcsStatusPolicy>,
    checkouts: Mutex<BTreeMap<PathBuf, Arc<Checkout>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        for checkout in self
            .checkouts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            let listeners = checkout.listeners();
            for worker in listeners
                .head
                .iter()
                .chain(listeners.poller.iter())
                .chain(listeners.retiring.iter())
            {
                worker.cancel();
            }
        }
    }
}
#[derive(Clone)]
pub struct VcsStatusBroadcaster {
    inner: Arc<Inner>,
}
impl VcsStatusBroadcaster {
    pub fn new(workflow: GitWorkflowService) -> Self {
        Self {
            inner: Arc::new(Inner {
                workflow,
                policy: Arc::new(DefaultPolicy),
                checkouts: Mutex::new(BTreeMap::new()),
            }),
        }
    }
    pub fn with_policy(self, policy: Arc<dyn VcsStatusPolicy>) -> Self {
        Self {
            inner: Arc::new(Inner {
                workflow: self.inner.workflow.clone(),
                policy,
                checkouts: Mutex::new(BTreeMap::new()),
            }),
        }
    }
    fn checkout(&self, cwd: &Path) -> Arc<Checkout> {
        let path = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
        self.inner
            .checkouts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(path.clone())
            .or_insert_with(|| {
                Arc::new(Checkout {
                    workflow: self.inner.workflow.at(path),
                    policy: self.inner.policy.clone(),
                    cached: AsyncMutex::new(Cached::default()),
                    local_lock: AsyncMutex::new(()),
                    remote_lock: AsyncMutex::new(()),
                    requests: AtomicU64::new(0),
                    detailed: AtomicUsize::new(0),
                    listeners: Mutex::new(Listeners::default()),
                })
            })
            .clone()
    }
    pub async fn stream_branch_status(
        &self,
        cwd: &Path,
    ) -> Result<VcsBranchSubscription, GitError> {
        let checkout = self.checkout(cwd);
        let lease = checkout.retain(false, false, cwd).await;
        let (sender, receiver) = mpsc::unbounded_channel();
        checkout.listeners().branches.insert(lease.id, sender);
        let (revision, initial) = checkout.branch(&checkout.workflow).await?;
        Ok(VcsBranchSubscription {
            initial: Some(initial),
            revision,
            receiver,
            lease,
        })
    }
    pub async fn refresh_branch_status(&self, cwd: &Path) -> Result<VcsBranchStatus, GitError> {
        let checkout = self.checkout(cwd);
        checkout
            .branch(&checkout.workflow)
            .await
            .map(|(_, branch)| branch)
    }
    pub async fn stream_status(
        &self,
        cwd: &Path,
        include_remote: bool,
    ) -> Result<VcsStatusSubscription, GitError> {
        let checkout = self.checkout(cwd);
        let lease = checkout.retain(true, include_remote, cwd).await;
        let (sender, receiver) = mpsc::unbounded_channel();
        checkout.listeners().statuses.insert(lease.id, sender);
        let local = checkout.local(&checkout.workflow).await?;
        let cached = checkout.cached.lock().await;
        let remote = cached.remote.as_ref().and_then(|r| r.value.clone());
        let immediate = cached.remote.is_none();
        drop(cached);
        if include_remote {
            checkout.start_polling(immediate);
        }
        Ok(VcsStatusSubscription {
            initial: Some(VcsStatusStreamEvent::Snapshot(
                VcsStatusStreamEventMember1 {
                    _tag: VcsStatusStreamEventMember1Tag::Snapshot,
                    local,
                    remote,
                },
            )),
            receiver,
            lease,
        })
    }
    pub async fn refresh_local_status(&self, cwd: &Path) -> Result<VcsStatusLocalResult, GitError> {
        let checkout = self.checkout(cwd);
        let local = checkout.refresh_local(&checkout.workflow).await?;
        // Divergence comes from Git and must not queue behind a forge lookup.
        let details = checkout.workflow.git().status_details_remote(false).await?;
        let mut cached = checkout.cached.lock().await;
        if let Some(remote) = &mut cached.remote
            && remote.branch == details.branch
            && let Some(value) = &mut remote.value
            && value.has_upstream
        {
            let changed = value.ahead_count != details.ahead_count
                || value.behind_count != details.behind_count;
            value.ahead_count = details.ahead_count;
            value.behind_count = details.behind_count;
            if changed {
                checkout.publish(remote_event(Some(value.clone())));
            }
            cached.remote_revision += 1;
        }
        Ok(local)
    }
    pub async fn refresh_status(
        &self,
        cwd: &Path,
        local_only: bool,
    ) -> Result<VcsStatusResult, GitError> {
        let checkout = self.checkout(cwd);
        checkout.refresh_local(&checkout.workflow).await?;
        if !local_only {
            checkout
                .remote(
                    &checkout.workflow,
                    !checkout.policy.automatic_fetch_interval().is_zero(),
                    true,
                    true,
                )
                .await?;
        }
        let cached = checkout.cached.lock().await;
        let local = cached.local.clone().ok_or_else(|| {
            GitError::InvalidOutput("status read did not produce local status".into())
        })?;
        Ok(merge_status(
            local,
            cached.remote.as_ref().and_then(|r| r.value.clone()),
        ))
    }
    pub async fn get_status(&self, cwd: &Path) -> Result<VcsStatusResult, GitError> {
        let checkout = self.checkout(cwd);
        checkout.local(&checkout.workflow).await?;
        let _lock = checkout.remote_lock.lock().await;
        if checkout.cached.lock().await.remote.is_none() {
            let read = checkout
                .read_remote(
                    &checkout.workflow,
                    !checkout.policy.automatic_fetch_interval().is_zero(),
                )
                .await?;
            checkout.update_remote(read, false).await;
        }
        let cached = checkout.cached.lock().await;
        Ok(merge_status(
            cached
                .local
                .clone()
                .ok_or_else(|| GitError::InvalidOutput("missing local status".into()))?,
            cached.remote.as_ref().and_then(|r| r.value.clone()),
        ))
    }
    pub async fn refresh_pull_request_status(
        &self,
        cwd: &Path,
    ) -> Result<Option<VcsStatusRemoteResult>, GitError> {
        let checkout = self.checkout(cwd);
        if checkout
            .cached
            .lock()
            .await
            .remote
            .as_ref()
            .is_none_or(|r| r.value.is_none())
        {
            return Ok(None);
        }
        checkout
            .remote(&checkout.workflow, false, false, true)
            .await
    }
    pub fn active_workers(&self) -> (usize, usize) {
        self.inner
            .checkouts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .fold((0, 0), |(heads, pollers), checkout| {
                let listeners = checkout.listeners();
                (
                    heads
                        + usize::from(
                            listeners
                                .head
                                .as_ref()
                                .is_some_and(|w| !*w.finished.borrow()),
                        ),
                    pollers
                        + usize::from(
                            listeners
                                .poller
                                .as_ref()
                                .is_some_and(|w| !*w.finished.borrow()),
                        ),
                )
            })
    }
    pub async fn shutdown(&self) {
        let checkouts: Vec<_> = self
            .inner
            .checkouts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for checkout in checkouts {
            {
                let mut listeners = checkout.listeners();
                listeners.branches.clear();
                listeners.statuses.clear();
                listeners.remote_demand.clear();
                if let Some(worker) = listeners.head.take() {
                    worker.cancel();
                    listeners.retiring.push(worker);
                }
                if let Some(worker) = listeners.poller.take() {
                    worker.cancel();
                    listeners.retiring.push(worker);
                }
            }
            checkout.drain_retired().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn polling_failure_backoff_respects_configuration_and_caps_at_fifteen_minutes() {
        assert_eq!(
            refresh_failure_delay(1, Duration::from_secs(10)),
            Duration::from_secs(30)
        );
        assert_eq!(
            refresh_failure_delay(2, Duration::from_secs(10)),
            Duration::from_secs(60)
        );
        assert_eq!(
            refresh_failure_delay(u32::MAX, Duration::ZERO),
            Duration::from_secs(900)
        );
        assert_eq!(
            refresh_failure_delay(1, Duration::from_secs(1800)),
            Duration::from_secs(1800)
        );
    }
}
