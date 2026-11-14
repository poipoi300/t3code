use crate::{
    ClientError,
    state::{shell::ShellState, thread::ThreadState},
    types::*,
};
use futures::Stream;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    pin::Pin,
    rc::{Rc, Weak},
    task::{Context, Poll, Waker},
};
use t3_contracts::orchestration_v2::{
    OrchestrationV2ShellStreamItem, OrchestrationV2ThreadStreamItem,
};
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvironmentState {
    pub shell: Rc<ShellState>,
    pub threads: BTreeMap<ThreadId, Rc<ThreadState>>,
    pub session: Generation,
    pub config: Option<Rc<t3_contracts::server::ServerConfig>>,
    pub connection: Option<Rc<crate::connection::ConnectionState>>,
}
#[derive(Debug, Clone, Default)]
pub struct StoreSnapshot {
    pub revision: u64,
    pub environments: BTreeMap<EnvironmentId, Rc<EnvironmentState>>,
}
/// See [`ClientStore::pinned`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pinned {
    pub environments: Vec<EnvironmentId>,
    pub threads: Vec<ThreadKey>,
}
impl Pinned {
    pub fn is_empty(&self) -> bool {
        self.environments.is_empty() && self.threads.is_empty()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    All,
    Shell(EnvironmentId),
    Thread(ThreadKey),
    Environment(EnvironmentId),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    pub revision: u64,
    pub scopes: BTreeSet<Scope>,
}
#[derive(Default)]
struct Slot {
    pending: Option<ChangeSet>,
    waker: Option<Waker>,
    /// Only changes to this environment's scopes reach the subscriber.
    environment: Option<EnvironmentId>,
}

impl Scope {
    fn concerns(&self, environment: &EnvironmentId) -> bool {
        match self {
            Scope::All => true,
            Scope::Shell(id) | Scope::Environment(id) => id == environment,
            Scope::Thread(key) => &key.environment == environment,
        }
    }
}
#[derive(Default)]
struct Inner {
    snapshot: StoreSnapshot,
    subscribers: Vec<Weak<RefCell<Slot>>>,
    shell_views: BTreeMap<EnvironmentId, Rc<crate::state::sharing::SharedShellSnapshot>>,
    /// Rows the shell deltas touched since each cached shell view was built; `None` once
    /// anything else changed the snapshot, so the next view compares every row.
    shell_changes: BTreeMap<EnvironmentId, Option<crate::state::sharing::ShellRowChanges>>,
    thread_views: BTreeMap<ThreadKey, Rc<crate::state::sharing::SharedThreadProjection>>,
    sync_owners: BTreeMap<Scope, u64>,
    next_owner: u64,
    replacements: BTreeMap<ThreadKey, crate::state::sharing::StreamReplacement>,
}
#[derive(Clone, Default)]
pub struct ClientStore {
    inner: Rc<RefCell<Inner>>,
}
pub struct Subscription {
    slot: Rc<RefCell<Slot>>,
    owner: Weak<RefCell<Inner>>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let weak = Rc::downgrade(&self.slot);
            owner
                .borrow_mut()
                .subscribers
                .retain(|slot| !slot.ptr_eq(&weak));
        }
    }
}
impl Stream for Subscription {
    type Item = ChangeSet;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut slot = self.slot.borrow_mut();
        if let Some(change) = slot.pending.take() {
            Poll::Ready(Some(change))
        } else if self.owner.upgrade().is_none() {
            Poll::Ready(None)
        } else {
            slot.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}
impl ClientStore {
    pub fn set_connection(
        &self,
        environment: EnvironmentId,
        connection: crate::connection::ConnectionState,
    ) {
        {
            let mut inner = self.inner.borrow_mut();
            let state = Rc::make_mut(
                inner
                    .snapshot
                    .environments
                    .entry(environment.clone())
                    .or_default(),
            );
            if state.connection.as_deref() == Some(&connection) {
                return;
            }
            state.connection = Some(Rc::new(connection));
        }
        self.publish(BTreeSet::from([Scope::Environment(environment)]));
    }
    pub fn set_config(
        &self,
        environment: &EnvironmentId,
        session: Generation,
        config: t3_contracts::server::ServerConfig,
    ) -> bool {
        {
            let mut inner = self.inner.borrow_mut();
            let Some(state) = inner.snapshot.environments.get_mut(environment) else {
                return false;
            };
            let state = Rc::make_mut(state);
            if state.session != session || &config.environment.environment_id != environment {
                return false;
            }
            if state.config.as_deref() == Some(&config) {
                return true;
            }
            state.config = Some(Rc::new(config));
        }
        self.publish(BTreeSet::from([Scope::Environment(environment.clone())]));
        true
    }
    /// Sets the environment's connection state if `owner` still holds its connection claim
    /// (`claim_sync(Scope::Environment(..))`): a replaced or removed connection cannot overwrite
    /// its successor's state or bring a removed environment back.
    pub fn publish_connection(
        &self,
        environment: &EnvironmentId,
        owner: u64,
        connection: crate::connection::ConnectionState,
    ) -> bool {
        if !self.owns_sync(&Scope::Environment(environment.clone()), owner) {
            return false;
        }
        self.set_connection(environment.clone(), connection);
        true
    }
    pub fn claim_sync(&self, scope: Scope) -> u64 {
        let owner = {
            let mut inner = self.inner.borrow_mut();
            inner.next_owner += 1;
            let owner = inner.next_owner;
            inner.sync_owners.insert(scope.clone(), owner);
            owner
        };
        self.publish(BTreeSet::from([scope]));
        owner
    }

    pub fn owns_sync(&self, scope: &Scope, owner: u64) -> bool {
        self.inner.borrow().sync_owners.get(scope) == Some(&owner)
    }
    pub fn session_is_current(&self, environment: &EnvironmentId, session: Generation) -> bool {
        self.inner
            .borrow()
            .snapshot
            .environments
            .get(environment)
            .is_some_and(|state| state.session == session)
    }

    pub fn update_thread<F>(&self, key: &ThreadKey, session: Generation, update: F) -> SyncAction
    where
        F: FnOnce(&mut ThreadState) -> bool,
    {
        let (revision, mut next) = {
            let inner = self.inner.borrow();
            let Some(environment) = inner.snapshot.environments.get(&key.environment) else {
                return SyncAction::StaleSession;
            };
            if environment.session != session {
                return SyncAction::StaleSession;
            }
            let thread = environment
                .threads
                .get(&key.thread)
                .cloned()
                .unwrap_or_else(|| {
                    let mut state = ThreadState::new(key.thread.clone());
                    state.begin_session(session);
                    Rc::new(state)
                });
            (inner.snapshot.revision, thread)
        };
        if update(Rc::make_mut(&mut next)) {
            {
                let mut inner = self.inner.borrow_mut();
                if inner.snapshot.revision != revision {
                    return SyncAction::StaleSession;
                }
                let Some(environment) = inner.snapshot.environments.get_mut(&key.environment)
                else {
                    return SyncAction::StaleSession;
                };
                Rc::make_mut(environment)
                    .threads
                    .insert(key.thread.clone(), next);
                inner.replacements.remove(key);
            }
            self.publish(BTreeSet::from([Scope::Thread(key.clone())]));
            SyncAction::Applied
        } else {
            SyncAction::None
        }
    }

    pub fn update_shell<F>(
        &self,
        environment: &EnvironmentId,
        session: Generation,
        update: F,
    ) -> SyncAction
    where
        F: FnOnce(&mut ShellState) -> bool,
    {
        let (revision, mut next) = {
            let inner = self.inner.borrow();
            let Some(state) = inner.snapshot.environments.get(environment) else {
                return SyncAction::StaleSession;
            };
            if state.session != session {
                return SyncAction::StaleSession;
            }
            (inner.snapshot.revision, state.shell.clone())
        };
        if update(Rc::make_mut(&mut next)) {
            {
                let mut inner = self.inner.borrow_mut();
                if inner.snapshot.revision != revision {
                    return SyncAction::StaleSession;
                }
                let Some(state) = inner.snapshot.environments.get_mut(environment) else {
                    return SyncAction::StaleSession;
                };
                Rc::make_mut(state).shell = next;
                inner.shell_changes.insert(environment.clone(), None);
            }
            self.publish(BTreeSet::from([Scope::Shell(environment.clone())]));
            SyncAction::Applied
        } else {
            SyncAction::None
        }
    }
    pub fn shared_shell(
        &self,
        environment: &EnvironmentId,
    ) -> Option<Rc<crate::state::sharing::SharedShellSnapshot>> {
        let mut inner = self.inner.borrow_mut();
        let source = inner
            .snapshot
            .environments
            .get(environment)?
            .shell
            .snapshot
            .clone()?;
        if let Some(previous) = inner.shell_views.get(environment)
            && previous.is_view_of(&source)
        {
            return Some(previous.clone());
        }
        let changes = inner.shell_changes.remove(environment).flatten();
        let next = Rc::new(crate::state::sharing::SharedShellSnapshot::update_changed(
            source,
            inner.shell_views.get(environment).map(Rc::as_ref),
            changes.as_ref(),
        ));
        inner.shell_views.insert(environment.clone(), next.clone());
        inner
            .shell_changes
            .insert(environment.clone(), Some(Default::default()));
        Some(next)
    }
    pub fn shared_thread(
        &self,
        key: &ThreadKey,
    ) -> Option<Rc<crate::state::sharing::SharedThreadProjection>> {
        let mut inner = self.inner.borrow_mut();
        let source = inner
            .snapshot
            .environments
            .get(&key.environment)?
            .threads
            .get(&key.thread)?
            .projection
            .clone()?;
        if let Some(previous) = inner.thread_views.get(key)
            && previous.is_view_of(&source)
        {
            return Some(previous.clone());
        }
        let hint = inner.replacements.remove(key);
        let next = Rc::new(match (inner.thread_views.get(key), hint) {
            (Some(previous), Some(hint)) => previous.replace(source, hint),
            (previous, _) => crate::state::sharing::SharedThreadProjection::update(
                source,
                previous.map(Rc::as_ref),
            ),
        });
        inner.thread_views.insert(key.clone(), next.clone());
        Some(next)
    }
    /// What is held outside the store right now: environments and threads whose state a
    /// `StoreSnapshot` (or an `Rc` taken from one) still references. Anything listed here is
    /// copied whole by the next change to it. For tests: after a UI frame settles this should be
    /// empty; a screen that keeps a snapshot shows up here.
    pub fn pinned(&self) -> Pinned {
        let inner = self.inner.borrow();
        let mut pinned = Pinned::default();
        for (id, environment) in &inner.snapshot.environments {
            if Rc::strong_count(environment) > 1 {
                pinned.environments.push(id.clone());
            }
            for (thread, state) in &environment.threads {
                let projection = state
                    .projection
                    .as_ref()
                    .is_some_and(|projection| Rc::strong_count(projection) > 1);
                if Rc::strong_count(state) > 1 || projection {
                    pinned.threads.push(ThreadKey {
                        environment: id.clone(),
                        thread: thread.clone(),
                    });
                }
            }
        }
        pinned
    }
    pub fn snapshot(&self) -> StoreSnapshot {
        self.inner.borrow().snapshot.clone()
    }
    pub fn subscribe(&self) -> Subscription {
        self.subscribe_to(None)
    }
    /// Like [`Self::subscribe`], woken only by changes to `environment`'s scopes (and
    /// `Scope::All`). Sync work for one environment waits on this, so streaming into one
    /// environment does not wake every other environment's in-flight requests.
    pub fn subscribe_environment(&self, environment: &EnvironmentId) -> Subscription {
        self.subscribe_to(Some(environment.clone()))
    }
    fn subscribe_to(&self, environment: Option<EnvironmentId>) -> Subscription {
        let mut inner = self.inner.borrow_mut();
        let slot = Rc::new(RefCell::new(Slot {
            pending: Some(ChangeSet {
                revision: inner.snapshot.revision,
                scopes: BTreeSet::from([Scope::All]),
            }),
            waker: None,
            environment,
        }));
        inner.subscribers.push(Rc::downgrade(&slot));
        Subscription {
            slot,
            owner: Rc::downgrade(&self.inner),
        }
    }
    fn publish(&self, scopes: BTreeSet<Scope>) {
        let mut wake = Vec::new();
        {
            let mut inner = self.inner.borrow_mut();
            inner.snapshot.revision += 1;
            let revision = inner.snapshot.revision;
            let Inner {
                snapshot,
                shell_views,
                thread_views,
                ..
            } = &mut *inner;
            shell_views.retain(|id, _| {
                snapshot
                    .environments
                    .get(id)
                    .is_some_and(|env| env.shell.snapshot.is_some())
            });
            thread_views.retain(|key, _| {
                snapshot
                    .environments
                    .get(&key.environment)
                    .and_then(|env| env.threads.get(&key.thread))
                    .is_some_and(|thread| thread.projection.is_some())
            });
            inner.subscribers.retain(|weak| {
                let Some(slot) = weak.upgrade() else {
                    return false;
                };
                let mut slot = slot.borrow_mut();
                if let Some(environment) = &slot.environment
                    && !scopes.iter().any(|scope| scope.concerns(environment))
                {
                    return true;
                }
                if let Some(change) = &mut slot.pending {
                    change.revision = revision;
                    change.scopes.extend(scopes.clone());
                } else {
                    slot.pending = Some(ChangeSet {
                        revision,
                        scopes: scopes.clone(),
                    });
                }
                if let Some(waker) = slot.waker.take() {
                    wake.push(waker);
                }
                true
            });
        }
        for waker in wake {
            waker.wake();
        }
    }
    /// Forgets an environment that was removed: its rows, views and sync ownership. Every sync
    /// still running for it stops at its next check, since no session of it is current.
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.snapshot.environments.remove(environment).is_none() {
                return;
            }
            inner.shell_views.remove(environment);
            inner.shell_changes.remove(environment);
            inner
                .thread_views
                .retain(|key, _| &key.environment != environment);
            inner
                .replacements
                .retain(|key, _| &key.environment != environment);
            inner.sync_owners.retain(|scope, _| match scope {
                Scope::Shell(id) | Scope::Environment(id) => id != environment,
                Scope::Thread(key) => &key.environment != environment,
                Scope::All => true,
            });
        }
        self.publish(BTreeSet::from([Scope::Environment(environment.clone())]));
    }
    pub fn begin_session(&self, environment: EnvironmentId) -> Generation {
        let generation = {
            let mut inner = self.inner.borrow_mut();
            let env = Rc::make_mut(
                inner
                    .snapshot
                    .environments
                    .entry(environment.clone())
                    .or_default(),
            );
            env.session.0 += 1;
            Rc::make_mut(&mut env.shell).begin_session(env.session);
            for thread in env.threads.values_mut() {
                Rc::make_mut(thread).begin_session(env.session);
            }
            env.session
        };
        self.publish(BTreeSet::from([Scope::Environment(environment)]));
        generation
    }
    pub fn disconnect(&self, environment: &EnvironmentId, session: Generation) -> SyncAction {
        {
            let mut inner = self.inner.borrow_mut();
            let Some(env) = inner.snapshot.environments.get_mut(environment) else {
                return SyncAction::StaleSession;
            };
            let env = Rc::make_mut(env);
            if env.session != session {
                return SyncAction::StaleSession;
            }
            env.session.0 += 1;
            let shell = Rc::make_mut(&mut env.shell);
            shell.session = env.session;
            shell.disconnect();
            for thread in env.threads.values_mut() {
                let thread = Rc::make_mut(thread);
                thread.session = env.session;
                thread.disconnect();
            }
        }
        self.publish(BTreeSet::from([Scope::Environment(environment.clone())]));
        SyncAction::Applied
    }
    pub fn begin_history(&self, key: &ThreadKey) -> Option<crate::state::thread::HistoryRequest> {
        let request = {
            let mut inner = self.inner.borrow_mut();
            let env = Rc::make_mut(inner.snapshot.environments.get_mut(&key.environment)?);
            Rc::make_mut(env.threads.get_mut(&key.thread)?).history_request()
        };
        if request.is_some() {
            self.publish(BTreeSet::from([Scope::Thread(key.clone())]));
        }
        request
    }
    pub fn apply_history(
        &self,
        key: &ThreadKey,
        request: &crate::state::thread::HistoryRequest,
        page: t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage,
    ) -> SyncAction {
        let result = {
            let mut inner = self.inner.borrow_mut();
            let Some(env) = inner.snapshot.environments.get_mut(&key.environment) else {
                return SyncAction::StaleSession;
            };
            let env = Rc::make_mut(env);
            let Some(thread) = env.threads.get_mut(&key.thread) else {
                return SyncAction::StaleSession;
            };
            Rc::make_mut(thread).apply_history(request, page)
        };
        if result == SyncAction::Applied {
            self.inner.borrow_mut().replacements.remove(key);
            self.publish(BTreeSet::from([Scope::Thread(key.clone())]));
        }
        result
    }
    pub fn apply_shell(
        &self,
        environment: &EnvironmentId,
        session: Generation,
        items: Vec<OrchestrationV2ShellStreamItem>,
    ) -> SyncAction {
        let result = {
            let mut inner = self.inner.borrow_mut();
            let inner = &mut *inner;
            let Some(env) = inner.snapshot.environments.get_mut(environment) else {
                return SyncAction::StaleSession;
            };
            let shell = Rc::make_mut(&mut Rc::make_mut(env).shell);
            let mut result = SyncAction::None;
            let mut changes = inner.shell_changes.remove(environment).flatten();
            for item in items {
                let known = changes
                    .as_mut()
                    .is_some_and(|changes| changes.record(&item));
                let next = shell.apply(session, item);
                if !known && next == SyncAction::Applied {
                    changes = None;
                }
                if matches!(next, SyncAction::Applied | SyncAction::ReloadSnapshot)
                    || result == SyncAction::None
                {
                    result = next;
                }
            }
            if inner.shell_views.contains_key(environment) {
                inner.shell_changes.insert(environment.clone(), changes);
            }
            result
        };
        if matches!(result, SyncAction::Applied | SyncAction::ReloadSnapshot) {
            self.publish(BTreeSet::from([Scope::Shell(environment.clone())]));
        }
        result
    }
    pub fn apply_thread(
        &self,
        key: ThreadKey,
        session: Generation,
        items: Vec<OrchestrationV2ThreadStreamItem>,
    ) -> Result<SyncAction, ClientError> {
        let replacement = {
            let inner = self.inner.borrow();
            inner.thread_views.get(&key).and_then(|view| {
                let current = inner
                    .snapshot
                    .environments
                    .get(&key.environment)?
                    .threads
                    .get(&key.thread)?
                    .projection
                    .as_ref()?;
                if items.len() == 1 && view.is_view_of(current) {
                    view.replacement(&items[0])
                } else {
                    None
                }
            })
        };
        let result = (|| {
            let mut inner = self.inner.borrow_mut();
            let Some(env) = inner.snapshot.environments.get_mut(&key.environment) else {
                return Ok(SyncAction::StaleSession);
            };
            let env = Rc::make_mut(env);
            if env.session != session {
                return Ok(SyncAction::StaleSession);
            }
            let thread = env.threads.entry(key.thread.clone()).or_insert_with(|| {
                let mut t = ThreadState::new(key.thread.clone());
                t.begin_session(session);
                Rc::new(t)
            });
            let thread = Rc::make_mut(thread);
            let mut result = SyncAction::None;
            for item in items {
                let next = thread.apply(session, item)?;
                if matches!(next, SyncAction::Applied | SyncAction::ReloadSnapshot)
                    || result == SyncAction::None
                {
                    result = next;
                }
            }
            Ok(result)
        })();
        {
            let mut inner = self.inner.borrow_mut();
            inner.replacements.remove(&key);
            if matches!(result, Ok(SyncAction::Applied))
                && let Some(replacement) = replacement
            {
                inner.replacements.insert(key.clone(), replacement);
            }
        }
        if !matches!(
            result,
            Ok(SyncAction::None | SyncAction::Duplicate | SyncAction::StaleSession)
        ) {
            self.publish(BTreeSet::from([Scope::Thread(key)]));
        }
        result
    }
}
