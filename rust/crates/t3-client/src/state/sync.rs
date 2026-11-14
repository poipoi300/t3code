use super::{
    cache::CachePersistence,
    history::{WorkPage, merge_history},
    items,
    thread::ThreadState,
};
use crate::{
    ClientError, ClientStore,
    persistence::{
        EnvironmentCacheStore, SHELL_CACHE_SCHEMA_VERSION, ShellOrigin, StoredShellSnapshot,
        StoredThreadSnapshot, THREAD_CACHE_SCHEMA_VERSION,
    },
    platform::Platform,
    rpc::RpcEndpoint,
    types::{EnvironmentId, Generation, SyncAction, SyncStatus, ThreadId, ThreadKey},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use t3_contracts::{
    orchestration_v2::*,
    rpc::{
        WsOrchestrationV2GetTurnItemRpc, WsOrchestrationV2SubscribeShellRpc,
        WsOrchestrationV2SubscribeThreadRpc,
    },
    thread_pull_request::ThreadPullRequestLink,
};

pub type DeferredLinks = LocalBoxFuture<'static, BTreeMap<ThreadId, Vec<ThreadPullRequestLink>>>;
pub type ShellHttpLoad<'a> = (Option<String>, Option<LocalBoxFuture<'a, ()>>, bool);

struct ShellSubscription;
impl crate::rpc::RpcMethod for ShellSubscription {
    type Input = OrchestrationV2SubscribeShellInput;
    type Output = serde_json::Value;
    const NAME: &'static str =
        <WsOrchestrationV2SubscribeShellRpc as t3_contracts::rpc::RpcMethod>::TAG;
    fn valid_error(error: &serde_json::Value) -> bool {
        <WsOrchestrationV2SubscribeShellRpc as crate::rpc::RpcMethod>::valid_error(error)
    }
}

pub struct LoadedShell {
    pub snapshot: OrchestrationV2ShellSnapshot,
    pub project_identities: Option<String>,
    pub links: Option<DeferredLinks>,
}

pub enum ShellLoadResult {
    Present(LoadedShell),
    Resumable,
    Unavailable,
}

pub enum ThreadLoadResult {
    Present(Box<OrchestrationV2ThreadDetailSnapshot>),
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Default)]
pub struct ShellSnapshotOptions {
    pub omit_idle_subagents: bool,
    pub omit_older_settled: bool,
    pub resume_from: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct HistoryQuery {
    pub thread: ThreadId,
    pub cursor: String,
    pub through_entry: Option<String>,
    pub view: Option<HistoryView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryView {
    Conversation,
    Activity,
    Work,
}

/// Bound to one destination and its authenticated HTTP session by the connection owner.
pub trait SnapshotSource {
    fn shell(
        &self,
        options: ShellSnapshotOptions,
    ) -> LocalBoxFuture<'_, Result<ShellLoadResult, ClientError>>;
    fn thread(&self, thread: ThreadId)
    -> LocalBoxFuture<'_, Result<ThreadLoadResult, ClientError>>;
    fn history(
        &self,
        query: HistoryQuery,
    ) -> LocalBoxFuture<'_, Result<OrchestrationV2ThreadHistoryPage, ClientError>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryLoadResult {
    Loaded,
    Noop,
    Busy,
}

#[derive(Clone)]
struct Ownership {
    store: ClientStore,
    scope: crate::store::Scope,
    environment: EnvironmentId,
    session: Generation,
    owner: u64,
}
impl Ownership {
    fn active(&self) -> bool {
        self.store
            .session_is_current(&self.environment, self.session)
            && self.store.owns_sync(&self.scope, self.owner)
    }
    async fn run<F: std::future::Future>(&self, operation: F) -> Option<F::Output> {
        let mut changes = self.store.subscribe_environment(&self.environment);
        futures::pin_mut!(operation);
        loop {
            if !self.active() {
                return None;
            }
            match futures::future::select(operation.as_mut(), changes.next()).await {
                futures::future::Either::Left((result, _)) => {
                    return self.active().then_some(result);
                }
                futures::future::Either::Right((None, _)) => return None,
                futures::future::Either::Right((Some(_), _)) => {}
            }
        }
    }
}

struct HistoryLoadingGuard {
    store: ClientStore,
    key: ThreadKey,
    session: Generation,
    generation: u64,
    cursor: Rc<RefCell<String>>,
    work: bool,
    owner: u64,
}

impl Drop for HistoryLoadingGuard {
    fn drop(&mut self) {
        if self.work
            || !self
                .store
                .owns_sync(&crate::store::Scope::Thread(self.key.clone()), self.owner)
        {
            return;
        }
        self.store.update_thread(&self.key, self.session, |state| {
            if state.generation != self.generation
                || state.history.cursor.as_deref() != Some(self.cursor.borrow().as_str())
                || !state.history.loading
            {
                return false;
            }
            state.history.loading = false;
            true
        });
    }
}

pub struct ShellSync<P, R, L> {
    pub store: ClientStore,
    pub environment: EnvironmentId,
    pub session: Generation,
    pub cache: Rc<EnvironmentCacheStore<P>>,
    pub rpc: Rc<R>,
    pub source: Rc<L>,
    pub expected_origin: Option<ShellOrigin>,
    origin: RefCell<Option<ShellOrigin>>,
    cache_queue: CachePersistence<StoredShellSnapshot>,
    pending_links: Cell<Option<u64>>,
    load_generation: Cell<u64>,
    cache_loaded: Cell<bool>,
    owner: u64,
    last_persisted: RefCell<Option<StoredShellSnapshot>>,
}

impl<P: Platform, R: RpcEndpoint, L: SnapshotSource> ShellSync<P, R, L> {
    pub fn new(
        store: ClientStore,
        environment: EnvironmentId,
        session: Generation,
        cache: Rc<EnvironmentCacheStore<P>>,
        rpc: Rc<R>,
        source: Rc<L>,
        expected_origin: Option<ShellOrigin>,
    ) -> Self {
        let owner = store.claim_sync(crate::store::Scope::Shell(environment.clone()));
        Self {
            store,
            environment,
            session,
            cache,
            rpc,
            source,
            expected_origin,
            origin: RefCell::new(None),
            cache_queue: CachePersistence::default(),
            pending_links: Cell::new(None),
            load_generation: Cell::new(0),
            cache_loaded: Cell::new(false),
            owner,
            last_persisted: RefCell::new(None),
        }
    }

    fn active(&self) -> bool {
        self.store
            .session_is_current(&self.environment, self.session)
            && self.store.owns_sync(
                &crate::store::Scope::Shell(self.environment.clone()),
                self.owner,
            )
    }
    fn ownership(&self) -> Ownership {
        Ownership {
            store: self.store.clone(),
            scope: crate::store::Scope::Shell(self.environment.clone()),
            environment: self.environment.clone(),
            session: self.session,
            owner: self.owner,
        }
    }

    fn current(&self) -> Option<Rc<super::shell::ShellState>> {
        if !self.active() {
            return None;
        }
        self.store
            .snapshot()
            .environments
            .get(&self.environment)
            .map(|state| state.shell.clone())
    }

    fn remember(&self) {
        if self.pending_links.get().is_some() {
            return;
        }
        if let Some(current) = self.current()
            && let Some(snapshot) = &current.snapshot
        {
            self.cache_queue.queue(StoredShellSnapshot {
                schema_version: SHELL_CACHE_SCHEMA_VERSION,
                environment_id: self.environment.clone(),
                snapshot: snapshot.as_ref().clone(),
                origin: self.origin.borrow().clone(),
            });
        }
    }

    async fn persist(&self, stored: StoredShellSnapshot) -> Result<(), ClientError> {
        if !self.active() {
            return Ok(());
        }
        let unchanged = self
            .last_persisted
            .borrow()
            .as_ref()
            .is_some_and(|previous| {
                previous.origin == stored.origin
                    && previous.snapshot.schema_version == stored.snapshot.schema_version
                    && stored.snapshot.snapshot_sequence >= previous.snapshot.snapshot_sequence
                    && previous.snapshot.projects == stored.snapshot.projects
                    && previous.snapshot.threads == stored.snapshot.threads
                    && previous.snapshot.archived_threads == stored.snapshot.archived_threads
            });
        if !unchanged {
            let Some(result) = self.ownership().run(self.cache.save_shell(&stored)).await else {
                return Ok(());
            };
            result.map_err(|error| ClientError::InvalidPayload(error.to_string()))?;
        }
        *self.last_persisted.borrow_mut() = Some(stored);
        Ok(())
    }

    pub async fn flush(&self) -> Result<(), ClientError> {
        self.remember();
        self.cache_queue.flush(|stored| self.persist(stored)).await
    }

    pub async fn persist_changes(&self) -> Result<(), ClientError> {
        self.ownership()
            .run(
                self.cache_queue
                    .run(self.cache.platform(), |stored| self.persist(stored)),
            )
            .await
            .unwrap_or(Ok(()))
    }

    pub async fn load_cache(&self) -> Result<(), ClientError> {
        self.cache_loaded.set(true);
        let before = self.current();
        let Some(cached) = self
            .ownership()
            .run(self.cache.load_shell(&self.environment))
            .await
        else {
            return Ok(());
        };
        let cached = cached.map_err(|error| ClientError::InvalidPayload(error.to_string()))?;
        if !self.active() {
            return Ok(());
        }
        let unchanged = match (&before, self.current()) {
            (Some(before), Some(after)) => Rc::ptr_eq(before, &after),
            _ => false,
        };
        if !unchanged {
            return Ok(());
        }
        if let Some(cached) = cached {
            let installed = self
                .store
                .update_shell(&self.environment, self.session, |state| {
                    if state.snapshot.is_some() {
                        return false;
                    }
                    let generation = state.generation;
                    state.install(cached.snapshot);
                    state.generation = generation;
                    state.status = SyncStatus::Cached;
                    true
                });
            if installed == SyncAction::Applied {
                *self.origin.borrow_mut() = cached.origin;
            }
        }
        Ok(())
    }

    pub fn apply(&self, items: Vec<OrchestrationV2ShellStreamItem>) -> SyncAction {
        if !self.active() {
            return SyncAction::StaleSession;
        }
        let authoritative = items.iter().any(|item| matches!(item, OrchestrationV2ShellStreamItem::Snapshot(snapshot) if snapshot.resolved_repository_identity_roots.is_none()));
        let invalidated = items
            .iter()
            .any(|item| matches!(item, OrchestrationV2ShellStreamItem::SnapshotRequired(_)));
        let action = self
            .store
            .apply_shell(&self.environment, self.session, items);
        if action == SyncAction::StaleSession {
            return action;
        }
        if authoritative {
            self.load_generation.set(self.load_generation.get() + 1);
            self.pending_links.set(None);
            *self.origin.borrow_mut() = self.expected_origin.clone().map(|mut origin| {
                origin.snapshot_at = self.cache.platform().now_epoch_ms();
                origin
            });
        }
        if invalidated || action == SyncAction::ReloadSnapshot {
            *self.origin.borrow_mut() = None;
        }
        if action == SyncAction::Applied {
            self.remember();
        }
        action
    }

    pub fn resume_sequence(&self) -> Option<u64> {
        let current = self.current()?;
        let expected = self.expected_origin.as_ref()?;
        let origin = self.origin.borrow();
        if origin
            .as_ref()?
            .resumable(expected, self.cache.platform().now_epoch_ms())
        {
            current.resume_sequence()
        } else {
            None
        }
    }

    pub async fn load_http(&self) -> Result<ShellHttpLoad<'_>, ClientError> {
        let load = self.load_generation.get() + 1;
        self.load_generation.set(load);
        let options = ShellSnapshotOptions {
            omit_idle_subagents: self
                .expected_origin
                .as_ref()
                .is_some_and(|origin| origin.omit_idle_subagents),
            omit_older_settled: self
                .expected_origin
                .as_ref()
                .is_some_and(|origin| origin.omit_older_settled),
            resume_from: self.resume_sequence(),
        };
        let Some(answer) = self.ownership().run(self.source.shell(options)).await else {
            return Ok((None, None, false));
        };
        let answer = answer.unwrap_or(ShellLoadResult::Unavailable);
        if !self.active() || self.load_generation.get() != load {
            return Ok((None, None, false));
        }
        match answer {
            ShellLoadResult::Present(loaded) => {
                let has_links = loaded.links.is_some();
                if has_links {
                    self.pending_links.set(Some(load));
                }
                let action = self.store.apply_shell(
                    &self.environment,
                    self.session,
                    vec![OrchestrationV2ShellStreamItem::Snapshot(Box::new(
                        ShellStreamItemSnapshot {
                            snapshot: loaded.snapshot,
                            resolved_repository_identity_roots: None,
                        },
                    ))],
                );
                if action == SyncAction::StaleSession {
                    return Ok((None, None, false));
                }
                *self.origin.borrow_mut() = self.expected_origin.clone().map(|mut origin| {
                    origin.snapshot_at = self.cache.platform().now_epoch_ms();
                    origin
                });
                let arrived = self.store.shared_shell(&self.environment);
                let generation = self.current().map(|state| state.generation);
                self.remember();
                let fill = loaded.links.map(|mut links| {
                    async move {
                        let mut changes = self.store.subscribe_environment(&self.environment);
                        let links = loop {
                            if self.pending_links.get() != Some(load)
                                || self.current().map(|state| state.generation) != generation
                            {
                                return;
                            }
                            match futures::future::select(links.as_mut(), changes.next()).await {
                                futures::future::Either::Left((links, _)) => break links,
                                futures::future::Either::Right((Some(_), _)) => {}
                                futures::future::Either::Right((None, _)) => return,
                            }
                        };
                        let current_rows = self.store.shared_shell(&self.environment);
                        self.store
                            .update_shell(&self.environment, self.session, |state| {
                                let (Some(snapshot), Some(arrived), Some(current_rows)) =
                                    (&mut state.snapshot, &arrived, &current_rows)
                                else {
                                    return false;
                                };
                                let mut changed = false;
                                for row in &mut Rc::make_mut(snapshot).threads {
                                    let prior =
                                        arrived.threads.iter().find(|prior| prior.id == row.id);
                                    let current = current_rows
                                        .threads
                                        .iter()
                                        .find(|current| current.id == row.id);
                                    if !prior
                                        .zip(current)
                                        .is_some_and(|(prior, current)| Rc::ptr_eq(prior, current))
                                    {
                                        continue;
                                    }
                                    let next = links.get(&row.id).cloned().map(Some);
                                    if row.pull_requests != next {
                                        row.pull_requests = next;
                                        changed = true;
                                    }
                                }
                                changed
                            });
                        self.pending_links.set(None);
                        self.remember();
                    }
                    .boxed_local()
                });
                Ok((loaded.project_identities, fill, true))
            }
            ShellLoadResult::Resumable => Ok((None, None, false)),
            ShellLoadResult::Unavailable => Ok((None, None, false)),
        }
    }

    pub async fn load_http_with_grace(&self) -> Result<ShellHttpLoad<'_>, ClientError> {
        if self.resume_sequence().is_none() {
            return self.load_http().await;
        }
        match futures::future::select(
            self.load_http().boxed_local(),
            self.cache.platform().sleep(3_000),
        )
        .await
        {
            futures::future::Either::Left((result, _)) => result,
            futures::future::Either::Right(_) => Ok((None, None, false)),
        }
    }

    pub async fn follow(&self) -> Result<(), ClientError> {
        if !self.cache_loaded.get() {
            let _ = self.load_cache().await;
        }
        // Per session and never reset by a value, as TypeScript's `streamFailureRetries`: a
        // stream can emit before every chunk failure.
        let mut failures = 0_u32;
        loop {
            if !self.active() {
                return Ok(());
            }
            self.store
                .update_shell(&self.environment, self.session, |state| {
                    state.begin_session(self.session);
                    true
                });
            let held = self.resume_sequence().is_some();
            let (known, mut fill, loaded) = self.load_http_with_grace().await?;
            if !self.active() {
                return Ok(());
            }
            let resuming = held && !loaded && self.resume_sequence().is_some();
            let sequence = if resuming {
                self.resume_sequence()
            } else if loaded {
                self.current().and_then(|state| state.resume_sequence())
            } else {
                None
            };
            let mut stream = self
                .rpc
                .subscribe::<ShellSubscription>(OrchestrationV2SubscribeShellInput {
                    after_sequence: sequence,
                    http_snapshot_fallback: resuming.then_some(LiteralTrue),
                    known_project_identities: known,
                    request_completion_marker: Some(true),
                    omit_idle_subagents: self
                        .expected_origin
                        .as_ref()
                        .is_some_and(|origin| origin.omit_idle_subagents)
                        .then_some(true),
                    omit_older_settled: self
                        .expected_origin
                        .as_ref()
                        .is_some_and(|origin| origin.omit_older_settled)
                        .then_some(true),
                })
                .ready_chunks(1024);
            let mut reload = false;
            let mut retry_stream = false;
            loop {
                let item = if let Some(pending) = fill.as_mut() {
                    let Some(next) = self
                        .ownership()
                        .run(futures::future::select(stream.next(), pending))
                        .await
                    else {
                        return Ok(());
                    };
                    match next {
                        futures::future::Either::Left((item, _)) => item,
                        futures::future::Either::Right(((), _)) => {
                            fill = None;
                            continue;
                        }
                    }
                } else {
                    self.ownership().run(stream.next()).await.flatten()
                };
                let Some(item) = item else {
                    break;
                };
                if !self.active() {
                    return Ok(());
                }
                let mut events = Vec::new();
                let mut failure = None;
                for item in item {
                    match item.and_then(super::decoding::shell_stream) {
                        Ok(Some(item)) => events.push(item),
                        Ok(None) => {}
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                if !events.is_empty() && self.apply(events) == SyncAction::ReloadSnapshot {
                    reload = true;
                    break;
                }
                if let Some(error) = failure {
                    self.store
                        .update_shell(&self.environment, self.session, |state| {
                            state.disconnect();
                            state.error = Some("Could not synchronize environment data.".into());
                            true
                        });
                    if matches!(
                        error,
                        ClientError::PermissionDenied { .. }
                            | ClientError::ScriptMismatch(_)
                            | ClientError::Unsupported(_)
                    ) {
                        return Err(error);
                    }
                    retry_stream = true;
                    break;
                }
                self.cache.platform().yield_to_host().await;
            }
            if reload {
                continue;
            }
            if retry_stream {
                if self
                    .ownership()
                    .run(self.cache.platform().sleep(retry_delay(failures)))
                    .await
                    .is_none()
                {
                    return Ok(());
                }
                failures = failures.saturating_add(1);
                continue;
            }
            if let Some(fill) = fill {
                self.ownership().run(fill).await;
            }
            return Ok(());
        }
    }
}

pub struct ThreadSync<P, R, L> {
    pub store: ClientStore,
    pub key: ThreadKey,
    pub session: Generation,
    pub cache: Rc<EnvironmentCacheStore<P>>,
    pub rpc: Rc<R>,
    pub source: Rc<L>,
    pub accepts_bounded: bool,
    cache_queue: CachePersistence<Option<OrchestrationV2ThreadDetailSnapshot>>,
    queued_snapshot: RefCell<Option<OrchestrationV2ThreadDetailSnapshot>>,
    socket_tried: Cell<bool>,
    socket_pagination: Cell<bool>,
    http_misses: Cell<u32>,
    cache_loaded: Cell<bool>,
    owner: u64,
}

impl<P: Platform, R: RpcEndpoint, L: SnapshotSource> ThreadSync<P, R, L> {
    pub fn new(
        store: ClientStore,
        key: ThreadKey,
        session: Generation,
        cache: Rc<EnvironmentCacheStore<P>>,
        rpc: Rc<R>,
        source: Rc<L>,
        accepts_bounded: bool,
    ) -> Self {
        let owner = store.claim_sync(crate::store::Scope::Thread(key.clone()));
        Self {
            store,
            key,
            session,
            cache,
            rpc,
            source,
            accepts_bounded,
            cache_queue: CachePersistence::default(),
            queued_snapshot: RefCell::new(None),
            socket_tried: Cell::new(false),
            socket_pagination: Cell::new(false),
            http_misses: Cell::new(0),
            cache_loaded: Cell::new(false),
            owner,
        }
    }

    fn active(&self) -> bool {
        self.store
            .session_is_current(&self.key.environment, self.session)
            && self
                .store
                .owns_sync(&crate::store::Scope::Thread(self.key.clone()), self.owner)
    }

    pub fn configure(&self, config: &t3_contracts::server::ServerConfig) {
        self.socket_pagination
            .set(config.thread_snapshot_pagination == Some(true));
    }

    pub fn set_socket_snapshot_pagination(&self, supported: bool) {
        self.socket_pagination.set(supported);
    }
    fn ownership(&self) -> Ownership {
        Ownership {
            store: self.store.clone(),
            scope: crate::store::Scope::Thread(self.key.clone()),
            environment: self.key.environment.clone(),
            session: self.session,
            owner: self.owner,
        }
    }

    fn current(&self) -> Option<Rc<ThreadState>> {
        if !self.active() {
            return None;
        }
        self.store
            .snapshot()
            .environments
            .get(&self.key.environment)?
            .threads
            .get(&self.key.thread)
            .cloned()
    }

    fn queue_snapshot(&self, snapshot: OrchestrationV2ThreadDetailSnapshot) {
        *self.queued_snapshot.borrow_mut() = Some(snapshot.clone());
        self.cache_queue.queue(Some(snapshot));
    }

    fn remember(&self) {
        let Some(current) = self.current() else {
            return;
        };
        if current.status == SyncStatus::Deleted {
            *self.queued_snapshot.borrow_mut() = None;
            self.cache_queue.queue(None);
        } else if let Some(snapshot) = current.cache_snapshot(self.accepts_bounded) {
            self.queue_snapshot(snapshot);
        }
    }

    async fn persist(
        &self,
        value: Option<OrchestrationV2ThreadDetailSnapshot>,
    ) -> Result<(), ClientError> {
        if !self.active() {
            return Ok(());
        }
        let operation = async {
            if let Some(snapshot) = value {
                self.cache
                    .save_thread(&StoredThreadSnapshot {
                        schema_version: THREAD_CACHE_SCHEMA_VERSION,
                        environment_id: self.key.environment.clone(),
                        thread_id: self.key.thread.clone(),
                        snapshot,
                    })
                    .await
            } else {
                self.cache
                    .remove_thread(&self.key.environment, &self.key.thread)
                    .await
            }
        };
        let Some(result) = self.ownership().run(operation).await else {
            return Ok(());
        };
        result.map_err(|error| ClientError::InvalidPayload(error.to_string()))
    }

    pub async fn flush(&self) -> Result<(), ClientError> {
        self.remember();
        self.cache_queue.flush(|value| self.persist(value)).await
    }

    pub async fn persist_changes(&self) -> Result<(), ClientError> {
        self.ownership()
            .run(
                self.cache_queue
                    .run(self.cache.platform(), |value| self.persist(value)),
            )
            .await
            .unwrap_or(Ok(()))
    }

    pub async fn load_cache(&self) -> Result<(), ClientError> {
        self.cache_loaded.set(true);
        if let Some(retained) = self.current() {
            if retained.status == SyncStatus::Deleted {
                return Ok(());
            }
            if retained.projection.is_some() && !retained.missing_pending_question_card() {
                return Ok(());
            }
            if retained.missing_pending_question_card() {
                self.store.update_thread(&self.key, self.session, |state| {
                    state.projection = None;
                    state.sequence = 0;
                    state.history = Default::default();
                    true
                });
                return Ok(());
            }
        }
        let before = self.current();
        let Some(cached) = self
            .ownership()
            .run(
                self.cache
                    .load_thread(&self.key.environment, &self.key.thread),
            )
            .await
        else {
            return Ok(());
        };
        let cached = cached.map_err(|error| ClientError::InvalidPayload(error.to_string()))?;
        if !self.active() {
            return Ok(());
        }
        let unchanged = match (&before, self.current()) {
            (None, None) => true,
            (Some(before), Some(after)) => Rc::ptr_eq(before, &after),
            _ => false,
        };
        if !unchanged {
            return Ok(());
        }
        if let Some(cached) = cached {
            let mut state = ThreadState::new(self.key.thread.clone());
            state.begin_session(self.session);
            state.install(cached.snapshot.clone())?;
            if state.missing_pending_question_card() {
                return Ok(());
            }
            state.status = SyncStatus::Cached;
            self.store
                .update_thread(&self.key, self.session, |current| {
                    if current.projection.is_some() || current.status == SyncStatus::Deleted {
                        return false;
                    }
                    *current = state;
                    true
                });
            *self.queued_snapshot.borrow_mut() = Some(cached.snapshot);
        }
        Ok(())
    }

    pub fn apply(
        &self,
        items: Vec<OrchestrationV2ThreadStreamItem>,
    ) -> Result<SyncAction, ClientError> {
        if !self.active() {
            return Ok(SyncAction::StaleSession);
        }
        let action = self
            .store
            .apply_thread(self.key.clone(), self.session, items)?;
        if action == SyncAction::Applied {
            self.remember();
        }
        Ok(action)
    }

    pub async fn follow(&self) -> Result<(), ClientError> {
        if !self.cache_loaded.get() {
            let _ = self.load_cache().await;
        }
        let mut resuming_live = self
            .current()
            .is_some_and(|state| state.status == SyncStatus::Live);
        let mut failures = 0;
        loop {
            if !self.active() {
                return Ok(());
            }
            let mut current = self.current();
            if current
                .as_ref()
                .is_some_and(|state| state.status == SyncStatus::Deleted)
                && self.http_misses.get() == 0
            {
                return Ok(());
            }
            if !resuming_live {
                self.store.update_thread(&self.key, self.session, |state| {
                    state.begin_session(self.session);
                    true
                });
            }
            resuming_live = false;
            current = self.current();
            let socket_only = current
                .as_ref()
                .is_none_or(|state| state.projection.is_none())
                && self.accepts_bounded
                && self.socket_pagination.get()
                && !self.socket_tried.replace(true);
            if current
                .as_ref()
                .is_none_or(|state| state.projection.is_none())
                && !socket_only
            {
                let generation = current.as_ref().map(|state| state.generation);
                let Some(loaded) = self
                    .ownership()
                    .run(self.source.thread(self.key.thread.clone()))
                    .await
                else {
                    return Ok(());
                };
                let loaded = loaded.unwrap_or(ThreadLoadResult::Unavailable);
                if !self.active() {
                    return Ok(());
                }
                if self.current().as_ref().map(|state| state.generation) != generation {
                    continue;
                }
                match loaded {
                    ThreadLoadResult::Present(snapshot) => {
                        self.http_misses.set(0);
                        let raw = serde_json::to_value(snapshot)?;
                        let mut raw = raw;
                        raw["kind"] = serde_json::Value::String("snapshot".into());
                        self.apply(vec![serde_json::from_value(raw)?])?;
                    }
                    ThreadLoadResult::Missing => {
                        self.store.update_thread(&self.key, self.session, |state| {
                            state.projection = None;
                            state.status = SyncStatus::Deleted;
                            state.history = Default::default();
                            state.generation += 1;
                            true
                        });
                        self.cache_queue.queue(None);
                        self.cache_queue.flush(|value| self.persist(value)).await?;
                        let attempts = self.http_misses.get();
                        self.http_misses.set(attempts.saturating_add(1));
                        if self
                            .ownership()
                            .run(self.cache.platform().sleep(retry_delay(attempts)))
                            .await
                            .is_none()
                        {
                            return Ok(());
                        }
                        continue;
                    }
                    ThreadLoadResult::Unavailable => {}
                }
                current = self.current();
            }
            let sequence = current.as_ref().and_then(|state| state.resume_sequence());
            let mut stream = self
                .rpc
                .subscribe::<WsOrchestrationV2SubscribeThreadRpc>(
                    OrchestrationV2SubscribeThreadInput {
                        thread_id: self.key.thread.clone(),
                        after_sequence: sequence,
                        request_completion_marker: Some(true),
                        accept_bounded_snapshot: self.accepts_bounded.then_some(true),
                        accept_deferred_work: self.accepts_bounded.then_some(true),
                        accept_compact_turn_items: self.accepts_bounded.then_some(true),
                        accept_compact_ids: self.accepts_bounded.then_some(true),
                        accept_deduped_bodies: Some(true),
                    },
                )
                .ready_chunks(1024);
            let mut retry_http = false;
            let mut retry_stream = false;
            while let Some(item) = self.ownership().run(stream.next()).await.flatten() {
                if !self.active() {
                    return Ok(());
                }
                let mut events = Vec::new();
                let mut failure = None;
                for item in item {
                    match item {
                        Ok(item) => events.push(item),
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                if !events.is_empty() {
                    match self.apply(events) {
                        Ok(SyncAction::ReloadSnapshot) => {
                            retry_stream = true;
                            break;
                        }
                        Err(error) => {
                            self.store.update_thread(&self.key, self.session, |state| {
                                state.delivery_gap();
                                state.error = Some(error.to_string());
                                true
                            });
                            retry_stream = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(error) = failure {
                    if socket_only
                        && self
                            .current()
                            .is_none_or(|state| state.projection.is_none())
                        && !matches!(error, ClientError::PermissionDenied { .. })
                    {
                        retry_http = true;
                        break;
                    }
                    self.store.update_thread(&self.key, self.session, |state| {
                        state.disconnect();
                        state.error = Some(error.to_string());
                        true
                    });
                    if matches!(
                        error,
                        ClientError::PermissionDenied { .. }
                            | ClientError::ScriptMismatch(_)
                            | ClientError::Unsupported(_)
                    ) {
                        return Err(error);
                    }
                    retry_stream = true;
                    break;
                }
                self.cache.platform().yield_to_host().await;
            }
            if retry_http {
                continue;
            }
            if retry_stream {
                if self
                    .ownership()
                    .run(self.cache.platform().sleep(retry_delay(failures)))
                    .await
                    .is_none()
                {
                    return Ok(());
                }
                failures = failures.saturating_add(1);
                continue;
            }
            return Ok(());
        }
    }

    pub async fn load_earlier(
        &self,
        through_entry: Option<String>,
        work_anchor: Option<String>,
    ) -> Result<HistoryLoadResult, ClientError> {
        let Some(initial) = self.current() else {
            return Ok(HistoryLoadResult::Noop);
        };
        if initial.status == SyncStatus::Deleted || initial.projection.is_none() {
            return Ok(HistoryLoadResult::Noop);
        }
        let work_cursor = |state: &ThreadState| {
            work_anchor.as_ref().and_then(|id| state.projection.as_ref()?.visible_turn_items.iter().find(|row| matches!(&row.source_item_id, t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(source) if source.0 == *id))).and_then(|row| items::deferred(&row.item).as_ref().map(|deferred| deferred.cursor.0.clone()))
        };
        let cursor = if work_anchor.is_some() {
            work_cursor(&initial)
        } else {
            initial
                .history
                .cursor
                .clone()
                .filter(|_| initial.history.has_more)
        };
        let Some(mut cursor) = cursor else {
            return Ok(HistoryLoadResult::Noop);
        };
        if work_anchor.is_none() && initial.history.loading {
            return Ok(HistoryLoadResult::Busy);
        }
        let active_cursor = Rc::new(RefCell::new(cursor.clone()));
        let _loading = HistoryLoadingGuard {
            store: self.store.clone(),
            key: self.key.clone(),
            session: self.session,
            generation: initial.generation,
            cursor: active_cursor.clone(),
            work: work_anchor.is_some(),
            owner: self.owner,
        };
        self.store.update_thread(&self.key, self.session, |state| {
            if work_anchor.is_none() {
                state.history.loading = true;
                state.history.error = None;
            }
            true
        });
        loop {
            let views = if work_anchor.is_some() {
                vec![Some(HistoryView::Work)]
            } else if through_entry.is_some() {
                vec![Some(HistoryView::Conversation), None]
            } else {
                vec![None]
            };
            let mut next = None;
            for view in views {
                let Some(page) = self
                    .ownership()
                    .run(self.source.history(HistoryQuery {
                        thread: self.key.thread.clone(),
                        cursor: cursor.clone(),
                        through_entry: work_anchor.clone().or_else(|| through_entry.clone()),
                        view,
                    }))
                    .await
                else {
                    return Ok(HistoryLoadResult::Noop);
                };
                let Some(current) = self.current() else {
                    return Ok(HistoryLoadResult::Noop);
                };
                let active_cursor = if work_anchor.is_some() {
                    work_cursor(&current)
                } else {
                    current.history.cursor.clone()
                };
                if current.generation != initial.generation
                    || active_cursor.as_deref() != Some(cursor.as_str())
                {
                    return if work_anchor.is_some() && active_cursor.is_some() {
                        Err(ClientError::InvalidPayload(
                            "Work history changed. Retry loading it.".into(),
                        ))
                    } else {
                        Ok(HistoryLoadResult::Noop)
                    };
                }
                let page = match page {
                    Ok(page) => page,
                    Err(error) => {
                        self.store.update_thread(&self.key, self.session, |state| {
                            if work_anchor.is_none() {
                                state.history.loading = false;
                                state.history.error = Some(error.to_string());
                            }
                            true
                        });
                        return Err(error);
                    }
                };
                let complete = view != Some(HistoryView::Conversation);
                let keep_paging = complete
                    && if work_anchor.is_some() {
                        page.next_cursor.is_some()
                    } else {
                        through_entry.is_none()
                            && page.has_more_history
                            && !page.items.iter().any(|row| starts_turn(&row.item))
                    };
                if keep_paging
                    && page
                        .next_cursor
                        .as_ref()
                        .is_none_or(|next| next.0 == cursor)
                {
                    let message = if work_anchor.is_some() {
                        "Work history did not advance."
                    } else {
                        "History did not advance."
                    };
                    if work_anchor.is_none() {
                        self.store.update_thread(&self.key, self.session, |state| {
                            state.history.loading = false;
                            state.history.error = Some(message.into());
                            true
                        });
                    }
                    return Err(ClientError::InvalidPayload(message.into()));
                }
                next = keep_paging
                    .then(|| page.next_cursor.as_ref().map(|cursor| cursor.0.clone()))
                    .flatten();
                self.store.update_thread(&self.key, self.session, |state| {
                    let Some(projection) = &mut state.projection else {
                        return false;
                    };
                    let work = work_anchor.as_ref().map(|id| WorkPage {
                        anchor: t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                            t3_contracts::base_schemas::TurnItemId(id.clone()),
                        ),
                        next_cursor: page.next_cursor.clone(),
                    });
                    merge_history(Rc::make_mut(projection), page.items.clone(), work);
                    if work_anchor.is_none() && complete {
                        state.history.apply_page(&page);
                        state.history.loading = keep_paging;
                    } else {
                        state.history.expanded = true;
                    }
                    true
                });
            }
            let Some(next) = next else {
                return Ok(HistoryLoadResult::Loaded);
            };
            cursor = next;
            *active_cursor.borrow_mut() = cursor.clone();
        }
    }

    pub async fn reconcile_pending_work(&self) -> Result<(), ClientError> {
        let Some(initial) = self.current() else {
            return Ok(());
        };
        let Some(projection) = initial.projection.as_ref() else {
            return Ok(());
        };
        let queried = items::pending_background(projection)
            .into_iter()
            .map(|item| items::id(item).clone())
            .collect::<Vec<_>>();
        let lookup = futures::stream::iter(
            queried
                .iter()
                .filter_map(|id| match id {
                    t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) => Some(id),
                    _ => None,
                })
                .map(|item| async move {
                    let reply = self.rpc.call::<WsOrchestrationV2GetTurnItemRpc>(
                        OrchestrationV2GetTurnItemInput {
                            thread_id: self.key.thread.clone(),
                            item_id: item.clone(),
                            revision: None,
                        },
                    );
                    match futures::future::select(reply, self.cache.platform().sleep(10_000)).await
                    {
                        futures::future::Either::Left((reply, _)) => reply,
                        futures::future::Either::Right(_) => Err(ClientError::Disconnected),
                    }
                }),
        )
        .buffered(4)
        .collect::<Vec<_>>();
        let Some(replies) = self.ownership().run(lookup).await else {
            return Ok(());
        };
        let Some(current) = self.current() else {
            return Ok(());
        };
        if current.generation != initial.generation {
            return Ok(());
        }
        let mut terminal = replies
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|reply| reply.item)
            .filter(|item| !items::active(item))
            .collect::<Vec<_>>();
        self.store.update_thread(&self.key, self.session, |state| {
            state
                .projection
                .as_mut()
                .is_some_and(|projection| items::settle(Rc::make_mut(projection), &terminal))
        });
        let Some(current) = self.current() else {
            return Ok(());
        };
        if current.cache_snapshot(self.accepts_bounded).is_some() {
            self.remember();
            return Ok(());
        }
        if let Some(projection) = &current.projection {
            for item in &projection.turn_items {
                if queried.contains(items::id(item)) && !items::active(item) {
                    terminal.retain(|reply| items::id(reply) != items::id(item));
                    terminal.push(item.clone());
                }
            }
        }
        if terminal.is_empty() {
            return Ok(());
        }
        let queued = self.queued_snapshot.borrow().clone();
        let saved = if queued.is_some() {
            queued
        } else {
            self.cache
                .load_thread(&self.key.environment, &self.key.thread)
                .await
                .ok()
                .flatten()
                .map(|stored| stored.snapshot)
        };
        if !self.active()
            || self
                .current()
                .is_none_or(|state| state.generation != initial.generation)
        {
            return Ok(());
        }
        if let Some(projection) = self.current().and_then(|state| state.projection.clone()) {
            for item in &projection.turn_items {
                if queried.contains(items::id(item)) && !items::active(item) {
                    terminal.retain(|reply| items::id(reply) != items::id(item));
                    terminal.push(item.clone());
                }
            }
        }
        if let Some(mut saved) = saved
            && items::settle(&mut saved.projection, &terminal)
        {
            self.queue_snapshot(saved);
        }
        Ok(())
    }
}

pub fn retry_delay(attempt: u32) -> u64 {
    250_u64
        .saturating_mul(1_u64.checked_shl(attempt.min(7)).unwrap_or(128))
        .min(30_000)
}

impl<P: Platform, R: RpcEndpoint, L: SnapshotSource> super::history::HistoryHandler
    for ThreadSync<P, R, L>
{
    fn load_earlier(
        &self,
        through_entry: Option<String>,
        work_anchor: Option<String>,
    ) -> LocalBoxFuture<'_, Result<HistoryLoadResult, ClientError>> {
        ThreadSync::load_earlier(self, through_entry, work_anchor).boxed_local()
    }
    fn reconcile_pending_work(&self) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        ThreadSync::reconcile_pending_work(self).boxed_local()
    }
}

fn starts_turn(item: &OrchestrationV2TurnItem) -> bool {
    matches!(item, OrchestrationV2TurnItem::Notification(_))
        || matches!(item, OrchestrationV2TurnItem::UserMessage(item) if !matches!(item.input_intent, OrchestrationV2UserMessageInputIntent::Steer | OrchestrationV2UserMessageInputIntent::PromotedQueuedToSteer))
}
