//! The terminals of a thread, as the drawer, its panes and every other surface use them. Ports
//! of `packages/client-runtime/src/state/terminal.ts` (attach, observe, metadata, the commands
//! and their scheduling), `apps/web/src/state/terminalSessions.ts` (a shown terminal's combined
//! session, the known sessions), `apps/web/src/terminalUiStateStore.ts` (tabs and groups,
//! persisted, through `view_logic::shell::terminal_state`),
//! `components/terminal/useThreadTerminalActions.ts`, and the drawer's wiring in
//! `components/PersistentThreadTerminals.tsx`.
use super::{
    sessions::{EnvironmentStream, ServiceSession},
    terminal::{
        AttachGenerations, DEFAULT_MAX_TERMINAL_BUFFER_BYTES, TerminalAttachResume,
        TerminalBufferState, TerminalGrid, TerminalOutputState, TerminalStatus,
        apply_terminal_metadata,
    },
};
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::{Platform, StorageArea, StorageKey, StorageWrite},
    rpc::{RpcCallError, RpcEndpoint, RpcMethod, SessionEndpoint},
    types::{EnvironmentId, ThreadKey},
    view_logic::{
        session::terminal_drawer::{
            self as drawer, SessionMark, TerminalLabel, project_script_cwd,
            project_script_runtime_env, resolve_terminal_session_label, terminal_label,
        },
        shell::terminal_state::{STORAGE_KEY, TerminalState, TerminalStore},
    },
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{AbortHandle, LocalBoxFuture},
    lock::Mutex,
    stream::LocalBoxStream,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    rpc::{
        Empty, WsSubscribeTerminalMetadataRpc, WsTerminalAttachRpc, WsTerminalClearRpc,
        WsTerminalCloseRpc, WsTerminalObserveRpc, WsTerminalOpenRpc, WsTerminalResizeRpc,
        WsTerminalWriteRpc,
    },
    terminal::{
        TerminalAttachInput, TerminalAttachStreamEvent, TerminalCloseInput, TerminalOpenInput,
        TerminalResizeInput, TerminalSessionInput, TerminalSessionSnapshot, TerminalSummary,
        TerminalWriteInput,
    },
};

const RECORD_KIND: &str = "terminal-state";
const RECORD_VERSION: u32 = 1;

fn storage_key() -> StorageKey {
    StorageKey::new(RECORD_KIND, None, STORAGE_KEY)
}

fn text(value: &str) -> TrimmedNonEmptyString {
    TrimmedNonEmptyString(value.to_owned())
}

/// Coalescing change notices: each reader holds at most one pending notice.
#[derive(Default)]
struct Notifier(RefCell<Vec<mpsc::Sender<()>>>);
impl Notifier {
    fn subscribe(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::channel(0);
        self.0.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    fn notify(&self) {
        self.0
            .borrow_mut()
            .retain_mut(|sender| match sender.try_send(()) {
                Ok(()) => true,
                Err(error) => !error.is_disconnected(),
            });
    }
}

#[derive(Default)]
struct UiState {
    store: TerminalStore,
    loaded: bool,
    /// Changed here before the saved state was read: the saved state does not replace it.
    dirty: bool,
}

/// Shared by every [`TerminalSessions`] handle of one runtime.
#[derive(Default)]
pub struct TerminalSessionsState {
    ui: RefCell<UiState>,
    ui_changes: Notifier,
    writes: Mutex<()>,
    metadata: RefCell<BTreeMap<EnvironmentId, Weak<MetadataShared>>>,
    sessions: RefCell<BTreeMap<String, Weak<SessionShared>>>,
    generations: Rc<RefCell<AttachGenerations>>,
    lifecycle: RefCell<BTreeMap<ThreadKey, Rc<Mutex<()>>>>,
    resizes: Rc<RefCell<BTreeMap<(ThreadKey, String), Option<TerminalGrid>>>>,
}

/// Where a new terminal starts. Without one, the thread's project and worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLocation {
    pub cwd: String,
    pub worktree_path: Option<String>,
}

/// Where a shown terminal attaches: its launch directory and environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    New,
    Split,
    SplitVertical,
}

struct MetadataShared {
    /// Sorted by terminal id; `None` until the server's first snapshot, or without read access.
    summaries: RefCell<Option<Rc<[Rc<TerminalSummary>]>>>,
    changes: Notifier,
    driver: Cell<Option<AbortHandle>>,
}
impl Drop for MetadataShared {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
        }
    }
}

/// The environment's known terminal sessions while held (`subscribeTerminalMetadata`).
#[derive(Clone)]
pub struct TerminalMetadataLease {
    shared: Rc<MetadataShared>,
}
impl TerminalMetadataLease {
    /// The thread's sessions in terminal-id order, `None` while unknown.
    pub fn sessions(&self, thread: &str) -> Option<Vec<Rc<TerminalSummary>>> {
        self.shared.summaries.borrow().as_ref().map(|all| {
            all.iter()
                .filter(|summary| summary.thread_id == thread)
                .cloned()
                .collect()
        })
    }
    pub fn summary(&self, thread: &str, terminal: &str) -> Option<Rc<TerminalSummary>> {
        self.shared
            .summaries
            .borrow()
            .as_ref()?
            .iter()
            .find(|summary| summary.thread_id == thread && summary.terminal_id == terminal)
            .cloned()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.shared.changes.subscribe()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// Operating the shell: `terminal.attach`.
    Attached,
    /// Reading it without operate permission: `terminal.observe`.
    Observing,
    /// Neither grant, or no session yet.
    Idle,
}

struct SessionShared {
    buffer: RefCell<TerminalBufferState>,
    failure: RefCell<Option<String>>,
    mode: Cell<SessionMode>,
    changes: Notifier,
    metadata: TerminalMetadataLease,
    thread: String,
    terminal: String,
    driver: Cell<Option<AbortHandle>>,
}
impl Drop for SessionShared {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
        }
    }
}

/// What a shown terminal's session is, without its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSessionView {
    pub status: TerminalStatus,
    pub version: u64,
    pub error: Option<String>,
    pub grid: Option<TerminalGrid>,
    pub generation: u64,
    pub lifecycle_version: u64,
    pub has_running_subprocess: bool,
    pub mode: SessionMode,
    pub mark: SessionMark,
}

/// One shown terminal's session while held: attached to operate it, or observed read-only,
/// combined with its metadata summary. Readers of one launch share one subscription; a
/// reconnect resubscribes with `resume` and keeps the shown output until the new snapshot.
#[derive(Clone)]
pub struct TerminalSessionLease {
    shared: Rc<SessionShared>,
}
impl TerminalSessionLease {
    pub fn view(&self) -> TerminalSessionView {
        let buffer = self.shared.buffer.borrow();
        let summary = self
            .shared
            .metadata
            .summary(&self.shared.thread, &self.shared.terminal);
        let failure = self.shared.failure.borrow().clone();
        let status = if failure.is_some() {
            TerminalStatus::Error
        } else if buffer.version > 0 {
            buffer.status
        } else {
            summary
                .as_ref()
                .map_or(buffer.status, |summary| summary.status.into())
        };
        let error = failure.or_else(|| buffer.error.clone());
        TerminalSessionView {
            status,
            version: buffer.version,
            grid: buffer.grid,
            generation: buffer.output.generation,
            lifecycle_version: buffer.lifecycle_version,
            has_running_subprocess: summary.is_some_and(|summary| summary.has_running_subprocess),
            mode: self.shared.mode.get(),
            mark: SessionMark::of(buffer.version, &buffer.output, error.as_deref()),
            error,
        }
    }
    /// Reads the retained output in place: a caller reads only what it has not seen.
    pub fn with_output<R>(&self, read: impl FnOnce(&TerminalOutputState) -> R) -> R {
        read(&self.shared.buffer.borrow().output)
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select(self.shared.changes.subscribe(), self.shared.metadata.changes())
            .boxed_local()
    }
}

/// What the drawer of one thread shows, composed from its tabs, the server's sessions and the
/// thread's project.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalDrawerView {
    pub key: ThreadKey,
    pub ui: TerminalState,
    pub can_operate: bool,
    /// The thread's project root; without one the drawer is not shown.
    pub project_root: Option<String>,
    pub cwd: Option<String>,
    pub worktree_path: Option<String>,
    pub runtime_env: BTreeMap<String, String>,
    /// The server's sessions of this thread, `None` while unknown.
    pub server_terminal_ids: Option<Vec<String>>,
    /// Persisted order while operating (known-session order changes on focus); the server's
    /// sessions for a read-only viewer.
    pub terminal_ids: Vec<String>,
    labels: BTreeMap<String, TerminalLabel>,
    launches: BTreeMap<String, TerminalLaunch>,
}
impl TerminalDrawerView {
    pub fn label(&self, terminal: &str) -> TerminalLabel {
        self.labels
            .get(terminal)
            .cloned()
            .unwrap_or_else(|| terminal_label(terminal))
    }
    /// Where `terminal` attaches: its session's own location, else the drawer's.
    pub fn launch(&self, terminal: &str) -> Option<TerminalLaunch> {
        self.launches.get(terminal).cloned().or_else(|| {
            Some(TerminalLaunch {
                cwd: self.cwd.clone()?,
                worktree_path: self.worktree_path.clone(),
                env: self.runtime_env.clone(),
            })
        })
    }
}

/// One thread's drawer while held: its view, and the reconciliation of its tabs with the
/// server's sessions (stopped with the lease).
pub struct TerminalDrawerLease {
    view: Rc<dyn Fn() -> TerminalDrawerView>,
    changes: Rc<dyn Fn() -> LocalBoxStream<'static, ()>>,
    reconcile: AbortHandle,
    _metadata: TerminalMetadataLease,
}
impl Drop for TerminalDrawerLease {
    fn drop(&mut self) {
        self.reconcile.abort();
    }
}
impl TerminalDrawerLease {
    pub fn view(&self) -> TerminalDrawerView {
        (self.view)()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        (self.changes)()
    }
}

#[derive(Clone)]
pub struct TerminalSessions<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    state: Rc<TerminalSessionsState>,
}

impl<P: Platform + Clone + 'static> TerminalSessions<P> {
    pub(crate) fn new(
        runtime: ClientRuntime<P>,
        platform: P,
        state: Rc<TerminalSessionsState>,
    ) -> Self {
        Self {
            runtime,
            platform,
            state,
        }
    }

    fn endpoint(&self, environment: &EnvironmentId) -> Result<SessionEndpoint, ClientError> {
        self.runtime
            .client(environment)
            .and_then(|client| client.session())
            .ok_or(ClientError::Disconnected)
    }

    /// The environment's current operate grant (`terminal:operate`), read at each use.
    pub fn can_operate(&self, environment: &EnvironmentId) -> bool {
        self.endpoint(environment).is_ok_and(|endpoint| {
            endpoint.available::<WsTerminalWriteRpc>(&TerminalWriteInput {
                thread_id: text("availability"),
                terminal_id: text("availability"),
                data: String::new(),
            })
        })
    }

    /// The environment's current read grant (`terminal:read`, implied by operate).
    pub fn can_read(&self, environment: &EnvironmentId) -> bool {
        self.endpoint(environment)
            .is_ok_and(|endpoint| endpoint.available::<WsSubscribeTerminalMetadataRpc>(&Empty {}))
    }

    // ---- Tabs and groups (terminalUiStateStore) ----

    /// Reads the saved tabs once; a change made before it finishes wins over the saved state.
    pub fn load(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            if service.state.ui.borrow().loaded {
                return Ok(());
            }
            let bytes = service
                .platform
                .get(StorageArea::Ordinary, storage_key())
                .await?;
            let store = match bytes {
                None => TerminalStore::default(),
                Some(bytes) => {
                    let raw: Value = serde_json::from_slice(&bytes)?;
                    let payload = if raw.get("formatVersion").is_some() {
                        crate::persistence::decode_record::<Value>(
                            &bytes,
                            &storage_key(),
                            RECORD_VERSION,
                        )
                        .map_err(|error| ClientError::InvalidPayload(error.to_string()))?
                        .payload
                    } else {
                        raw
                    };
                    TerminalStore::decode(&payload.to_string())?
                }
            };
            let changed = {
                let mut ui = service.state.ui.borrow_mut();
                let replace = !ui.loaded && !ui.dirty;
                ui.loaded = true;
                if replace {
                    ui.store = store;
                }
                replace
            };
            if changed {
                service.state.ui_changes.notify();
            }
            Ok(())
        }
        .boxed_local()
    }

    pub fn ui_state(&self, key: &ThreadKey) -> TerminalState {
        self.state
            .ui
            .borrow()
            .store
            .by_thread
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// Threads whose drawer is open.
    pub fn open_threads(&self) -> Vec<ThreadKey> {
        self.state
            .ui
            .borrow()
            .store
            .by_thread
            .iter()
            .filter(|(_, state)| state.terminal_open)
            .map(|(key, _)| key.clone())
            .collect()
    }

    pub fn ui_changes(&self) -> LocalBoxStream<'static, ()> {
        self.state.ui_changes.subscribe()
    }

    fn update_ui(&self, update: impl FnOnce(&mut TerminalStore)) {
        let changed = {
            let mut ui = self.state.ui.borrow_mut();
            let before = (ui.store.by_thread.clone(), ui.store.suppressed.clone());
            update(&mut ui.store);
            let changed = before.0 != ui.store.by_thread || before.1 != ui.store.suppressed;
            ui.dirty |= changed;
            changed
        };
        if !changed {
            return;
        }
        self.state.ui_changes.notify();
        self.persist();
    }

    /// Writes the tabs as they are when the write starts; writes run one at a time, so the
    /// last state is the one kept.
    fn persist(&self) {
        let service = self.clone();
        let _ = self.runtime.spawn(
            async move {
                let _guard = service.state.writes.lock().await;
                let Ok(encoded) = service.state.ui.borrow().store.encode() else {
                    return;
                };
                let Ok(payload) = serde_json::from_str::<Value>(&encoded) else {
                    return;
                };
                let Ok(bytes) = crate::persistence::Record::new(
                    RECORD_KIND,
                    None,
                    STORAGE_KEY,
                    RECORD_VERSION,
                    payload,
                )
                .encode() else {
                    return;
                };
                let _ = service
                    .platform
                    .write(
                        StorageArea::Ordinary,
                        vec![StorageWrite {
                            key: storage_key(),
                            value: Some(bytes),
                        }],
                    )
                    .await;
            }
            .boxed_local(),
        );
    }

    pub fn set_open(&self, key: &ThreadKey, open: bool) {
        self.update_ui(|store| store.set_open(key, open));
    }
    pub fn toggle(&self, key: &ThreadKey) {
        let open = self.ui_state(key).terminal_open;
        self.set_open(key, !open);
    }
    pub fn set_height(&self, key: &ThreadKey, height: f64) {
        self.update_ui(|store| store.update(key, |state| state.set_height(height)));
    }
    pub fn place(&self, key: &ThreadKey, terminal: &str, placement: Placement) {
        let (split, vertical) = match placement {
            Placement::New => (false, false),
            Placement::Split => (true, false),
            Placement::SplitVertical => (true, true),
        };
        self.update_ui(|store| store.upsert(key, terminal, split, vertical));
    }
    pub fn ensure(&self, key: &ThreadKey, terminal: &str, open: bool, active: bool) {
        self.update_ui(|store| store.ensure(key, terminal, open, active));
    }
    pub fn activate(&self, key: &ThreadKey, terminal: &str) {
        self.update_ui(|store| store.update(key, |state| state.activate(terminal)));
    }
    /// Removes the tab and keeps stale metadata from bringing it back.
    pub fn close_tab(&self, key: &ThreadKey, terminal: &str) {
        self.update_ui(|store| store.close(key, terminal));
    }
    pub fn reconcile(&self, key: &ThreadKey, server_ids: &[String]) {
        self.update_ui(|store| store.reconcile(key, server_ids));
    }
    pub fn clear_ui(&self, key: &ThreadKey) {
        self.update_ui(|store| {
            store.by_thread.remove(key);
            store.suppressed.remove(key);
        });
    }
    pub fn retain_ui(&self, keys: &std::collections::BTreeSet<ThreadKey>) {
        self.update_ui(|store| store.retain(keys));
    }

    // ---- Server state (subscriptions) ----

    pub fn metadata(&self, environment: &EnvironmentId) -> TerminalMetadataLease {
        if let Some(shared) = self
            .state
            .metadata
            .borrow()
            .get(environment)
            .and_then(Weak::upgrade)
        {
            return TerminalMetadataLease { shared };
        }
        let shared = Rc::new(MetadataShared {
            summaries: RefCell::new(None),
            changes: Notifier::default(),
            driver: Cell::new(None),
        });
        let weak = Rc::downgrade(&shared);
        let connections = self
            .runtime
            .query_connections(environment)
            .map(|connection| connection.session)
            .boxed_local();
        let platform = self.platform.clone();
        let environment_id = environment.clone();
        let driver = self.runtime.spawn(
            async move {
                type Method = WsSubscribeTerminalMetadataRpc;
                let unreadable = weak.clone();
                let frames = EnvironmentStream::recovering(
                    environment_id,
                    connections,
                    &platform,
                    <Method as RpcMethod>::NAME,
                    move |session: &ServiceSession<SessionEndpoint>| {
                        if session.endpoint.available::<Method>(&Empty {}) {
                            return session.endpoint.subscribe::<Method>(Empty {});
                        }
                        if let Some(shared) = unreadable.upgrade()
                            && shared.summaries.replace(None).is_some()
                        {
                            shared.changes.notify();
                        }
                        futures::stream::pending().boxed_local()
                    },
                );
                futures::pin_mut!(frames);
                let mut raw: Vec<TerminalSummary> = Vec::new();
                while let Some(frame) = frames.next().await {
                    let Some(shared) = weak.upgrade() else {
                        return;
                    };
                    // A failed stream keeps the last known sessions, as the web's atom does.
                    let Ok(event) = frame else { continue };
                    if shared.summaries.borrow().is_none() {
                        raw.clear();
                    }
                    apply_terminal_metadata(&mut raw, event);
                    let mut sorted: Vec<Rc<TerminalSummary>> =
                        raw.iter().cloned().map(Rc::new).collect();
                    sorted.sort_by(|left, right| {
                        drawer::compare_terminal_ids(&left.terminal_id, &right.terminal_id)
                    });
                    shared.summaries.replace(Some(sorted.into()));
                    shared.changes.notify();
                }
            }
            .boxed_local(),
        );
        shared.driver.set(Some(driver));
        let mut all = self.state.metadata.borrow_mut();
        all.retain(|_, lease| lease.strong_count() > 0);
        all.insert(environment.clone(), Rc::downgrade(&shared));
        TerminalMetadataLease { shared }
    }

    /// The attach input of a shown terminal, as the drawer's viewport builds it.
    pub fn attach_input(key: &ThreadKey, terminal: &str, launch: &TerminalLaunch) -> TerminalAttachInput {
        TerminalAttachInput {
            thread_id: text(&key.thread.0),
            terminal_id: text(terminal),
            cwd: Some(Some(text(&launch.cwd))),
            worktree_path: Some(launch.worktree_path.as_deref().map(text)),
            cols: None,
            rows: None,
            env: Some(Some(launch.env.clone())),
            provider_instance_id: None,
            stop_on_settle: None,
            restart_if_not_running: None,
            resume: None,
            max_history_bytes: None,
        }
    }

    /// One shown terminal's session. The mode follows each session's grants: attached with
    /// operate, observed with read only, idle (empty) with neither.
    pub fn session(&self, environment: &EnvironmentId, input: TerminalAttachInput) -> TerminalSessionLease {
        let identity = format!(
            "{}\u{0}{}",
            environment.0,
            serde_json::to_string(&input).unwrap_or_default()
        );
        if let Some(shared) = self
            .state
            .sessions
            .borrow()
            .get(&identity)
            .and_then(Weak::upgrade)
        {
            return TerminalSessionLease { shared };
        }
        let generations = self.state.generations.clone();
        let shared = Rc::new(SessionShared {
            buffer: RefCell::new(generations.borrow_mut().seed()),
            failure: RefCell::new(None),
            mode: Cell::new(SessionMode::Idle),
            changes: Notifier::default(),
            metadata: self.metadata(environment),
            thread: input.thread_id.0.clone(),
            terminal: input.terminal_id.0.clone(),
            driver: Cell::new(None),
        });
        let weak = Rc::downgrade(&shared);
        let connections = self
            .runtime
            .query_connections(environment)
            .map(|connection| connection.session)
            .boxed_local();
        let platform = self.platform.clone();
        let environment_id = environment.clone();
        let driver = self.runtime.spawn(
            async move {
                let resume = Rc::new(RefCell::new(TerminalAttachResume::default()));
                let observe = TerminalSessionInput {
                    thread_id: input.thread_id.clone(),
                    terminal_id: input.terminal_id.clone(),
                };
                let moded = weak.clone();
                let seeds = generations.clone();
                let frames = EnvironmentStream::recovering(
                    environment_id,
                    connections,
                    &platform,
                    <WsTerminalAttachRpc as RpcMethod>::NAME,
                    move |session: &ServiceSession<SessionEndpoint>| {
                        let endpoint = &session.endpoint;
                        let mode = if endpoint.available::<WsTerminalAttachRpc>(&input) {
                            SessionMode::Attached
                        } else if endpoint.available::<WsTerminalObserveRpc>(&observe) {
                            SessionMode::Observing
                        } else {
                            SessionMode::Idle
                        };
                        if let Some(shared) = moded.upgrade() {
                            let before = shared.mode.replace(mode);
                            if mode == SessionMode::Idle && before != SessionMode::Idle {
                                shared.buffer.replace(seeds.borrow_mut().seed());
                                shared.failure.replace(None);
                            }
                            if before != mode {
                                shared.changes.notify();
                            }
                        }
                        match mode {
                            SessionMode::Attached => {
                                let replied = resume.clone();
                                endpoint
                                    .subscribe::<WsTerminalAttachRpc>(
                                        resume.borrow().input(input.clone()),
                                    )
                                    .inspect(move |frame| {
                                        if frame.is_ok() {
                                            replied.borrow_mut().received();
                                        }
                                    })
                                    .boxed_local()
                            }
                            SessionMode::Observing => {
                                endpoint.subscribe::<WsTerminalObserveRpc>(observe.clone())
                            }
                            SessionMode::Idle => futures::stream::pending().boxed_local(),
                        }
                    },
                );
                futures::pin_mut!(frames);
                while let Some(frame) = frames.next().await {
                    let Some(shared) = weak.upgrade() else {
                        return;
                    };
                    match frame {
                        Ok(event) => {
                            shared.failure.replace(None);
                            apply(&shared, &event, &generations);
                        }
                        Err(error) => {
                            shared.failure.replace(Some(error.to_string()));
                        }
                    }
                    shared.changes.notify();
                }
            }
            .boxed_local(),
        );
        shared.driver.set(Some(driver));
        let mut sessions = self.state.sessions.borrow_mut();
        sessions.retain(|_, lease| lease.strong_count() > 0);
        sessions.insert(identity, Rc::downgrade(&shared));
        TerminalSessionLease { shared }
    }

    // ---- Commands ----

    fn guarded<M>(&self, environment: &EnvironmentId, input: M::Input) -> LocalBoxFuture<'static, Result<M::Output, ClientError>>
    where
        M: RpcMethod + 'static,
        M::Input: 'static,
        M::Output: 'static,
    {
        let endpoint = self.endpoint(environment);
        async move {
            endpoint?
                .command::<M>(input)
                .await
                .map_err(RpcCallError::into_client_error)
        }
        .boxed_local()
    }

    /// `terminal.open`, `terminal.clear` and `terminal.close` of one thread run one at a time.
    fn lifecycle<M>(&self, key: &ThreadKey, input: M::Input) -> LocalBoxFuture<'static, Result<M::Output, ClientError>>
    where
        M: RpcMethod + 'static,
        M::Input: 'static,
        M::Output: 'static,
    {
        let lane = {
            let mut lanes = self.state.lifecycle.borrow_mut();
            lanes.retain(|_, lane| Rc::strong_count(lane) > 1);
            lanes.entry(key.clone()).or_default().clone()
        };
        let service = self.clone();
        let environment = key.environment.clone();
        async move {
            let _turn = lane.lock().await;
            service.guarded::<M>(&environment, input).await
        }
        .boxed_local()
    }

    pub fn open(&self, key: &ThreadKey, input: TerminalOpenInput) -> LocalBoxFuture<'static, Result<TerminalSessionSnapshot, ClientError>> {
        self.lifecycle::<WsTerminalOpenRpc>(key, input)
    }

    pub fn write(&self, key: &ThreadKey, terminal: &str, data: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.guarded::<WsTerminalWriteRpc>(
            &key.environment,
            TerminalWriteInput {
                thread_id: text(&key.thread.0),
                terminal_id: text(terminal),
                data,
            },
        )
    }

    pub fn clear(&self, key: &ThreadKey, terminal: &str) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.lifecycle::<WsTerminalClearRpc>(
            key,
            TerminalSessionInput {
                thread_id: text(&key.thread.0),
                terminal_id: text(terminal),
            },
        )
    }

    pub fn close(&self, key: &ThreadKey, terminal: &str, delete_history: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.lifecycle::<WsTerminalCloseRpc>(
            key,
            TerminalCloseInput {
                thread_id: text(&key.thread.0),
                terminal_id: Some(Some(text(terminal))),
                delete_history: delete_history.then_some(Some(true)),
            },
        )
    }

    /// Resizes a terminal. While one resize of it is in flight, only the latest next grid is
    /// sent after it.
    pub fn resize(&self, key: &ThreadKey, terminal: &str, grid: TerminalGrid) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let slot = (key.clone(), terminal.to_owned());
        {
            let mut slots = self.state.resizes.borrow_mut();
            if let Some(pending) = slots.get_mut(&slot) {
                *pending = Some(grid);
                return futures::future::ready(Ok(())).boxed_local();
            }
            slots.insert(slot.clone(), None);
        }
        struct Release(Rc<RefCell<BTreeMap<(ThreadKey, String), Option<TerminalGrid>>>>, (ThreadKey, String));
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.borrow_mut().remove(&self.1);
            }
        }
        let release = Release(self.state.resizes.clone(), slot);
        let service = self.clone();
        async move {
            let mut grid = grid;
            loop {
                let result = service
                    .guarded::<WsTerminalResizeRpc>(
                        &release.1.0.environment,
                        TerminalResizeInput {
                            thread_id: text(&release.1.0.thread.0),
                            terminal_id: text(&release.1.1),
                            cols: grid.cols,
                            rows: grid.rows,
                        },
                    )
                    .await;
                let next = release
                    .0
                    .borrow_mut()
                    .get_mut(&release.1)
                    .and_then(Option::take);
                match next {
                    Some(newer) => grid = newer,
                    None => return result,
                }
            }
        }
        .boxed_local()
    }

    /// Ends a session and deletes its history. A failed close falls back to typing `exit`.
    pub fn close_session(&self, key: &ThreadKey, terminal: &str) -> LocalBoxFuture<'static, ()> {
        if !self.can_operate(&key.environment) {
            return futures::future::ready(()).boxed_local();
        }
        let service = self.clone();
        let (key, terminal) = (key.clone(), terminal.to_owned());
        async move {
            let closed = service.close(&key, &terminal, true).await;
            if let Err(error) = closed
                && !matches!(error, ClientError::Cancelled)
                && service.can_operate(&key.environment)
            {
                let _ = service.write(&key, &terminal, "exit\n".into()).await;
            }
        }
        .boxed_local()
    }

    // ---- The drawer (PersistentThreadTerminalDrawer, useThreadTerminalActions) ----

    fn project(&self, key: &ThreadKey) -> Option<(String, Option<String>)> {
        let context = self.runtime.git_panel().workspace_context(key, None)?;
        Some((context.project.workspace_root.0.clone(), context.worktree_path))
    }

    pub fn default_launch_location(&self, key: &ThreadKey) -> Option<LaunchLocation> {
        let (root, worktree) = self.project(key)?;
        Some(LaunchLocation {
            cwd: project_script_cwd(&root, worktree.as_deref()),
            worktree_path: worktree,
        })
    }

    /// A free terminal id for the thread, against its known sessions, its tabs and `panel`.
    pub fn allocate_terminal_id(&self, key: &ThreadKey, panel: &[String]) -> String {
        let known = self
            .state
            .metadata
            .borrow()
            .get(&key.environment)
            .and_then(Weak::upgrade)
            .and_then(|shared| TerminalMetadataLease { shared }.sessions(&key.thread.0))
            .map(|sessions| {
                sessions
                    .iter()
                    .map(|summary| summary.terminal_id.clone())
                    .collect::<Vec<_>>()
            });
        let drawer_ids = self.ui_state(key).terminal_ids;
        let platform = self.platform.clone();
        drawer::allocate_terminal_id(
            known.as_deref(),
            &drawer_ids,
            panel,
            self.can_read(&key.environment),
            move || {
                crate::operations::allocate_command_id(&platform, None)
                    .map(|id| id.0)
                    .unwrap_or_default()
            },
        )
    }

    /// Places a new terminal and opens its shell. `place` runs before the open resolves, so
    /// the destination mounts first. `None` without operate permission or a project.
    pub fn launch(
        &self,
        key: &ThreadKey,
        place: impl FnOnce(&str),
        location: Option<LaunchLocation>,
        extra_env: Option<&BTreeMap<String, String>>,
    ) -> Option<(String, LocalBoxFuture<'static, Result<TerminalSessionSnapshot, ClientError>>)> {
        if !self.can_operate(&key.environment) {
            return None;
        }
        let (root, _) = self.project(key)?;
        let location = location.or_else(|| self.default_launch_location(key))?;
        let terminal = self.allocate_terminal_id(key, &[]);
        place(&terminal);
        let opened = self.open(
            key,
            TerminalOpenInput {
                thread_id: text(&key.thread.0),
                terminal_id: text(&terminal),
                cwd: text(&location.cwd),
                worktree_path: location.worktree_path.as_deref().map(|path| Some(text(path))),
                cols: None,
                rows: None,
                env: Some(Some(project_script_runtime_env(
                    &root,
                    location.worktree_path.as_deref(),
                    extra_env,
                ))),
                provider_instance_id: None,
                stop_on_settle: None,
            },
        );
        Some((terminal, opened))
    }

    /// A drawer action: a new tab, or a split of the active group.
    pub fn launch_drawer_terminal(
        &self,
        key: &ThreadKey,
        placement: Placement,
        location: Option<LaunchLocation>,
    ) -> Option<(String, LocalBoxFuture<'static, Result<TerminalSessionSnapshot, ClientError>>)> {
        let service = self.clone();
        let target = key.clone();
        self.launch(key, move |terminal| service.place(&target, terminal, placement), location, None)
    }

    /// Closes a drawer tab: its session ends and its history goes.
    pub fn close_drawer_terminal(&self, key: &ThreadKey, terminal: &str) -> Option<LocalBoxFuture<'static, ()>> {
        if !self.can_operate(&key.environment) {
            return None;
        }
        let closing = self.close_session(key, terminal);
        self.close_tab(key, terminal);
        Some(closing)
    }

    fn drawer_view(&self, key: &ThreadKey, metadata: &TerminalMetadataLease, launch: Option<&LaunchLocation>) -> TerminalDrawerView {
        let ui = self.ui_state(key);
        let can_operate = self.can_operate(&key.environment);
        let project = self.project(key);
        let sessions = metadata.sessions(&key.thread.0);
        let mut labels = BTreeMap::new();
        let mut launches = BTreeMap::new();
        for summary in sessions.iter().flatten() {
            labels.insert(
                summary.terminal_id.clone(),
                resolve_terminal_session_label(&summary.terminal_id, Some(&summary.label)),
            );
            if let Some((root, _)) = &project {
                let worktree = match launch {
                    Some(launch) => launch.worktree_path.clone(),
                    None => summary.worktree_path.as_ref().map(|path| path.0.clone()),
                };
                launches.insert(
                    summary.terminal_id.clone(),
                    TerminalLaunch {
                        cwd: launch.map_or_else(|| summary.cwd.clone(), |launch| launch.cwd.clone()),
                        env: project_script_runtime_env(root, worktree.as_deref(), None),
                        worktree_path: worktree,
                    },
                );
            }
        }
        let server_terminal_ids: Option<Vec<String>> = sessions.as_ref().map(|sessions| {
            sessions
                .iter()
                .map(|summary| summary.terminal_id.clone())
                .collect()
        });
        let (project_root, cwd, worktree_path, runtime_env) = match &project {
            None => (None, None, None, BTreeMap::new()),
            Some((root, thread_worktree)) => {
                let worktree = match launch {
                    Some(launch) => launch.worktree_path.clone(),
                    None => thread_worktree.clone(),
                };
                let cwd = launch.map_or_else(
                    || project_script_cwd(root, worktree.as_deref()),
                    |launch| launch.cwd.clone(),
                );
                let env = project_script_runtime_env(root, worktree.as_deref(), None);
                (Some(root.clone()), Some(cwd), worktree, env)
            }
        };
        TerminalDrawerView {
            key: key.clone(),
            terminal_ids: if can_operate {
                ui.terminal_ids.clone()
            } else {
                server_terminal_ids.clone().unwrap_or_default()
            },
            ui,
            can_operate,
            project_root,
            cwd,
            worktree_path,
            runtime_env,
            server_terminal_ids,
            labels,
            launches,
        }
    }

    /// Adopts the server's sessions as the thread's tabs when the two diverge, except while
    /// the server still lags tabs opened here.
    fn reconcile_drawer(&self, key: &ThreadKey, metadata: &TerminalMetadataLease) {
        let server: Vec<String> = metadata
            .sessions(&key.thread.0)
            .unwrap_or_default()
            .iter()
            .map(|summary| summary.terminal_id.clone())
            .collect();
        let client = self.ui_state(key).terminal_ids;
        if drawer::should_reconcile_terminal_ids(&server, &client) {
            self.reconcile(key, &server);
        }
    }

    /// One thread's drawer. `launch` overrides where new terminals start (a script run's).
    pub fn drawer(&self, key: &ThreadKey, launch: Option<LaunchLocation>) -> TerminalDrawerLease {
        let metadata = self.metadata(&key.environment);
        let service = self.clone();
        let reconcile = {
            let service = self.clone();
            let key = key.clone();
            let metadata = metadata.clone();
            let changes = futures::stream::select(service.ui_changes(), metadata.changes());
            self.runtime.spawn(
                async move {
                    service.reconcile_drawer(&key, &metadata);
                    futures::pin_mut!(changes);
                    while changes.next().await.is_some() {
                        service.reconcile_drawer(&key, &metadata);
                    }
                }
                .boxed_local(),
            )
        };
        let view = {
            let key = key.clone();
            let metadata = metadata.clone();
            Rc::new(move || service.drawer_view(&key, &metadata, launch.as_ref()))
        };
        let changes = {
            let service = self.clone();
            let metadata = metadata.clone();
            let environment = key.environment.clone();
            Rc::new(move || {
                let store = service
                    .runtime
                    .store()
                    .subscribe_environment(&environment)
                    .map(|_| ());
                futures::stream::select(
                    futures::stream::select(service.ui_changes(), metadata.changes()),
                    store,
                )
                .boxed_local()
            })
        };
        TerminalDrawerLease {
            view,
            changes,
            reconcile,
            _metadata: metadata,
        }
    }
}

fn apply(shared: &SessionShared, event: &TerminalAttachStreamEvent, generations: &RefCell<AttachGenerations>) {
    shared.buffer.borrow_mut().apply(
        event,
        DEFAULT_MAX_TERMINAL_BUFFER_BYTES,
        &mut generations.borrow_mut(),
    );
}
