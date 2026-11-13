use crate::{
    ClientError,
    persistence::{Record, StoredVcsRefs, decode_record, decode_vcs_refs},
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
    types::EnvironmentId,
};
use futures::{FutureExt, future::LocalBoxFuture};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    task::{Poll, Waker},
};
pub use t3_contracts::git::VcsRef;
use t3_contracts::git::{VcsListRefsInput, VcsListRefsResult};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VcsRefTarget {
    pub environment: Option<EnvironmentId>,
    pub cwd: Option<String>,
    pub query: Option<String>,
}

pub const REFS_IDLE_TTL_MS: u64 = 30_000;
pub const STATUS_IDLE_TTL_MS: u64 = 10_000;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckoutKey {
    pub environment: EnvironmentId,
    pub cwd: String,
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<u64>,
    wakers: BTreeMap<u64, Waker>,
}
#[derive(Default)]
struct SchedulerInner {
    next: u64,
    queues: BTreeMap<CheckoutKey, Queue>,
}
#[derive(Clone, Default)]
pub struct VcsCommandScheduler {
    inner: Rc<RefCell<SchedulerInner>>,
}
pub struct CommandLease {
    scheduler: VcsCommandScheduler,
    key: CheckoutKey,
    id: u64,
}
impl Drop for CommandLease {
    fn drop(&mut self) {
        let wake = {
            let mut inner = self.scheduler.inner.borrow_mut();
            let Some(queue) = inner.queues.get_mut(&self.key) else {
                return;
            };
            let was_head = queue.waiting.front() == Some(&self.id);
            queue.waiting.retain(|id| *id != self.id);
            queue.wakers.remove(&self.id);
            let wake = if was_head {
                queue
                    .waiting
                    .front()
                    .and_then(|id| queue.wakers.get(id))
                    .cloned()
            } else {
                None
            };
            if queue.waiting.is_empty() {
                inner.queues.remove(&self.key);
            }
            wake
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}
impl VcsCommandScheduler {
    pub fn acquire(&self, key: CheckoutKey) -> LocalBoxFuture<'static, CommandLease> {
        let id = {
            let mut inner = self.inner.borrow_mut();
            inner.next += 1;
            let id = inner.next;
            inner
                .queues
                .entry(key.clone())
                .or_default()
                .waiting
                .push_back(id);
            id
        };
        let mut lease = Some(CommandLease {
            scheduler: self.clone(),
            key,
            id,
        });
        futures::future::poll_fn(move |cx| {
            let Some(held) = lease.as_ref() else {
                return Poll::Pending;
            };
            let ready = {
                let mut inner = held.scheduler.inner.borrow_mut();
                let Some(queue) = inner.queues.get_mut(&held.key) else {
                    return Poll::Pending;
                };
                if queue.waiting.front() == Some(&held.id) {
                    true
                } else {
                    queue.wakers.insert(held.id, cx.waker().clone());
                    false
                }
            };
            if ready {
                lease.take().map_or(Poll::Pending, Poll::Ready)
            } else {
                Poll::Pending
            }
        })
        .boxed_local()
    }
}

#[derive(Debug, Clone, Default)]
struct Revalidation {
    subscribers: usize,
    connected: bool,
    wake_at: Option<u64>,
}
#[derive(Debug, Default)]
pub struct VcsStatusRevalidation {
    targets: BTreeMap<CheckoutKey, Revalidation>,
    pub wake_debounce_ms: u64,
}
impl VcsStatusRevalidation {
    pub fn subscribe(&mut self, target: CheckoutKey, connected: Option<bool>) -> bool {
        let entry = self.targets.entry(target).or_default();
        entry.subscribers += 1;
        if entry.subscribers > 1 {
            return false;
        }
        entry.connected = connected.unwrap_or(false);
        entry.connected
    }
    pub fn unsubscribe(&mut self, target: &CheckoutKey) {
        if let Some(entry) = self.targets.get_mut(target) {
            entry.subscribers = entry.subscribers.saturating_sub(1);
            if entry.subscribers == 0 {
                self.targets.remove(target);
            }
        }
    }
    pub fn connection_changed(
        &mut self,
        environment: &EnvironmentId,
        connected: bool,
    ) -> Vec<CheckoutKey> {
        let mut refresh = Vec::new();
        for (key, entry) in &mut self.targets {
            if &key.environment != environment || entry.connected == connected {
                continue;
            }
            entry.connected = connected;
            if connected {
                refresh.push(key.clone());
            }
        }
        refresh
    }
    pub fn wake(&mut self, now: u64) {
        for entry in self.targets.values_mut() {
            entry.wake_at = Some(now.saturating_add(self.wake_debounce_ms));
        }
    }
    pub fn take_due(&mut self, now: u64) -> Vec<CheckoutKey> {
        self.targets
            .iter_mut()
            .filter_map(|(key, entry)| {
                if entry.wake_at.is_some_and(|due| now >= due) {
                    entry.wake_at = None;
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn next_due(&self) -> Option<u64> {
        self.targets.values().filter_map(|v| v.wake_at).min()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefsCacheState {
    pub revision: u64,
    pub persisted_cache_readable: bool,
}
impl Default for RefsCacheState {
    fn default() -> Self {
        Self {
            revision: 0,
            persisted_cache_readable: true,
        }
    }
}
#[derive(Clone, Default)]
pub struct VcsRefsCache {
    states: Rc<RefCell<BTreeMap<EnvironmentId, RefsCacheState>>>,
    server_revisions: Rc<RefCell<BTreeMap<CheckoutKey, u64>>>,
    persistence: VcsCommandScheduler,
    changes:
        Rc<RefCell<BTreeMap<EnvironmentId, crate::services::resources::Resource<RefsCacheState>>>>,
}
pub fn can_use_refs_cache(input: &VcsListRefsInput) -> bool {
    input.query.is_none()
        && input.cursor.is_none()
        && input.include_matching_remote_refs.is_none()
        && input.ref_kind.is_none()
        && input.limit == Some(Some(100))
}
impl VcsRefsCache {
    pub fn state(&self, environment: &EnvironmentId) -> RefsCacheState {
        self.states
            .borrow()
            .get(environment)
            .copied()
            .unwrap_or_default()
    }
    pub fn invalidate(&self, environment: &EnvironmentId, readable: Option<bool>) {
        let state = {
            let mut states = self.states.borrow_mut();
            let state = states.entry(environment.clone()).or_default();
            state.revision += 1;
            if let Some(readable) = readable {
                state.persisted_cache_readable = readable;
            }
            *state
        };
        let resource = self.changes.borrow().get(environment).cloned();
        if let Some(resource) = resource {
            let token = resource.begin();
            resource.finish(token, state, 0, false);
        }
    }
    pub fn observe(
        &self,
        environment: &EnvironmentId,
    ) -> crate::services::resources::ResourceSubscription<RefsCacheState> {
        use crate::services::resources::{Resource, ResourcePolicy};
        let resource = self
            .changes
            .borrow_mut()
            .entry(environment.clone())
            .or_insert_with(|| {
                Resource::new(
                    ResourcePolicy::default(),
                    crate::types::Generation::default(),
                )
            })
            .clone();
        resource.observe(0)
    }
    pub fn bypass_server_snapshot(&self, target: &CheckoutKey, expected_revision: u64) -> bool {
        expected_revision
            > self
                .server_revisions
                .borrow()
                .get(target)
                .copied()
                .unwrap_or(0)
    }
    fn lock_key(environment: &EnvironmentId) -> CheckoutKey {
        CheckoutKey {
            environment: environment.clone(),
            cwd: String::new(),
        }
    }
    fn storage_key(target: &CheckoutKey) -> StorageKey {
        StorageKey {
            namespace: "cache.vcs-refs".into(),
            environment: Some(target.environment.clone()),
            record: target.cwd.clone(),
        }
    }
    async fn clear(
        storage: &(impl Storage + ?Sized),
        environment: &EnvironmentId,
    ) -> Result<(), ClientError> {
        let keys = storage
            .list(StorageArea::Ordinary, "cache.vcs-refs".into())
            .await?;
        storage
            .write(
                StorageArea::Ordinary,
                keys.into_iter()
                    .filter(|key| key.environment.as_ref() == Some(environment))
                    .map(|key| StorageWrite { key, value: None })
                    .collect(),
            )
            .await?;
        Ok(())
    }
    pub async fn invalidate_cached(
        &self,
        storage: &(impl Storage + ?Sized),
        environment: &EnvironmentId,
    ) -> bool {
        let _lease = self.persistence.acquire(Self::lock_key(environment)).await;
        let readable = Self::clear(storage, environment).await.is_ok();
        self.invalidate(environment, Some(readable));
        readable
    }
    pub async fn load(
        &self,
        storage: &(impl Storage + ?Sized),
        target: &CheckoutKey,
        input: &VcsListRefsInput,
    ) -> Option<VcsListRefsResult> {
        if !can_use_refs_cache(input) {
            return None;
        }
        let expected = self.state(&target.environment);
        if !expected.persisted_cache_readable {
            return None;
        }
        let bytes = storage
            .get(StorageArea::Ordinary, Self::storage_key(target))
            .await
            .ok()??;
        if self.state(&target.environment) != expected {
            return None;
        }
        let record = decode_record::<StoredVcsRefs>(&bytes, &Self::storage_key(target), 1).ok()?;
        let bytes = serde_json::to_vec(&record.payload).ok()?;
        decode_vcs_refs(&bytes, &target.environment, &target.cwd)
            .ok()
            .map(|stored| stored.refs)
    }
    pub async fn commit_refresh(
        &self,
        storage: &(impl Storage + ?Sized),
        target: &CheckoutKey,
        refs: &VcsListRefsResult,
        expected_revision: u64,
        persist: bool,
    ) -> bool {
        let _lease = self
            .persistence
            .acquire(Self::lock_key(&target.environment))
            .await;
        let state = self.state(&target.environment);
        if state.revision != expected_revision {
            return false;
        }
        if persist {
            let readable = state.persisted_cache_readable
                || Self::clear(storage, &target.environment).await.is_ok();
            let stored = StoredVcsRefs {
                schema_version: 1,
                environment_id: target.environment.clone(),
                cwd: target.cwd.clone(),
                refs: refs.clone(),
            };
            let record = Record::new(
                "cache.vcs-refs",
                Some(target.environment.clone()),
                &target.cwd,
                1,
                stored,
            );
            if let Ok(bytes) = record.encode() {
                let _ = storage
                    .write(
                        StorageArea::Ordinary,
                        vec![StorageWrite {
                            key: Self::storage_key(target),
                            value: Some(bytes),
                        }],
                    )
                    .await;
            }
            if let Some(current) = self.states.borrow_mut().get_mut(&target.environment)
                && current.revision == expected_revision
            {
                current.persisted_cache_readable = readable;
            }
        }
        if self.state(&target.environment).revision != expected_revision {
            return false;
        }
        self.server_revisions
            .borrow_mut()
            .insert(target.clone(), expected_revision);
        true
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        self.invalidate(environment, Some(false));
        self.server_revisions
            .borrow_mut()
            .retain(|key, _| &key.environment != environment);
    }
}

pub fn refs_retry_delay_ms(attempt: u32) -> u64 {
    1_000_u64
        .saturating_mul(2_u64.saturating_pow(attempt))
        .min(30_000)
}

pub fn apply_status_event(
    current: Option<&t3_contracts::git::VcsStatusResult>,
    event: t3_contracts::git::VcsStatusStreamEvent,
) -> t3_contracts::git::VcsStatusResult {
    use t3_contracts::git::*;
    let local_of = |s: &VcsStatusResult| VcsStatusLocalResult {
        is_repo: s.is_repo,
        source_control_provider: s.source_control_provider.clone(),
        has_primary_remote: s.has_primary_remote,
        is_default_ref: s.is_default_ref,
        ref_name: s.ref_name.clone(),
        has_working_tree_changes: s.has_working_tree_changes,
        working_tree: VcsStatusLocalResultWorkingTree {
            root_path: s.working_tree.root_path.clone(),
            files: s
                .working_tree
                .files
                .iter()
                .map(|f| VcsStatusLocalResultWorkingTreeFiles {
                    path: f.path.clone(),
                    insertions: f.insertions,
                    deletions: f.deletions,
                })
                .collect(),
            insertions: s.working_tree.insertions,
            deletions: s.working_tree.deletions,
        },
        branch_changes: s.branch_changes.as_ref().map(|v| {
            v.as_ref().map(|b| VcsStatusLocalResultBranchChanges {
                base_ref: b.base_ref.clone(),
                insertions: b.insertions,
                deletions: b.deletions,
            })
        }),
    };
    let remote_of = |s: &VcsStatusResult| VcsStatusRemoteResult {
        has_upstream: s.has_upstream,
        ahead_count: s.ahead_count,
        behind_count: s.behind_count,
        ahead_of_default_count: s.ahead_of_default_count,
        pr: s.pr.clone(),
    };
    let (local, remote) = match event {
        VcsStatusStreamEvent::Snapshot(e) => (e.local, e.remote),
        VcsStatusStreamEvent::LocalUpdated(e) => (e.local, current.map(remote_of)),
        VcsStatusStreamEvent::RemoteUpdated(e) => (
            current.map(local_of).unwrap_or(VcsStatusLocalResult {
                is_repo: true,
                source_control_provider: None,
                has_primary_remote: false,
                is_default_ref: false,
                ref_name: None,
                has_working_tree_changes: false,
                working_tree: VcsStatusLocalResultWorkingTree {
                    root_path: None,
                    files: Vec::new(),
                    insertions: 0,
                    deletions: 0,
                },
                branch_changes: None,
            }),
            e.remote,
        ),
    };
    let remote = remote.unwrap_or(VcsStatusRemoteResult {
        has_upstream: false,
        ahead_count: 0,
        behind_count: 0,
        ahead_of_default_count: Some(Some(0)),
        pr: None,
    });
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
        branch_changes: local.branch_changes.map(|v| {
            v.map(|b| VcsStatusResultBranchChanges {
                base_ref: b.base_ref,
                insertions: b.insertions,
                deletions: b.deletions,
            })
        }),
        has_upstream: remote.has_upstream,
        ahead_count: remote.ahead_count,
        behind_count: remote.behind_count,
        ahead_of_default_count: remote.ahead_of_default_count,
        pr: remote.pr,
    }
}
