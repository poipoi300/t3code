use crate::{
    history::neutralize,
    process::{Process, ProcessEvent},
    stream::{Queue, attached_size, bounded_event},
    *,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

pub(crate) type Key = (String, String);
pub type ProviderEnvironmentResolver =
    Arc<dyn Fn(&str, Option<&Environment>) -> Result<Environment, TerminalError> + Send + Sync>;
pub struct ManagerOptions {
    /// The caller passes the TypeScript `logsDir`, not the live T3 home.
    pub logs_dir: PathBuf,
    pub env: Environment,
    pub home: PathBuf,
    pub app_version: String,
    pub preferred_shell: Option<String>,
    /// Opt in to killing detached Windows descendants with a Job Object.
    pub kill_descendants: bool,
    pub history_line_limit: usize,
    pub history_byte_limit: usize,
    pub max_retained_inactive_sessions: usize,
    pub max_queued_output: usize,
    pub persist_debounce: Duration,
    pub subprocess_poll_interval: Duration,
    pub managed_binary_directories: Vec<String>,
    pub resolve_provider_environment: Option<ProviderEnvironmentResolver>,
}
impl ManagerOptions {
    /// Production/explicit-base layout from config.ts: <base>/userdata/logs/terminals.
    pub fn from_base_dir(base: impl Into<PathBuf>) -> Self {
        Self::from_state_dir(base.into().join("userdata"))
    }
    /// Use the server-resolved state directory for dev mode's <base>/dev layout.
    pub fn from_state_dir(state: impl Into<PathBuf>) -> Self {
        Self::new(state.into().join("logs").join("terminals"))
    }
    pub fn new(logs_dir: impl Into<PathBuf>) -> Self {
        Self {
            logs_dir: logs_dir.into(),
            env: std::env::vars_os()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
                .collect(),
            home: std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .unwrap_or_default(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            preferred_shell: None,
            kill_descendants: false,
            history_line_limit: 5000,
            history_byte_limit: 8 * 1024 * 1024,
            max_retained_inactive_sessions: 128,
            max_queued_output: 8 * 1024 * 1024,
            persist_debounce: Duration::from_millis(250),
            subprocess_poll_interval: Duration::from_secs(1),
            managed_binary_directories: vec![],
            resolve_provider_environment: None,
        }
    }
}
struct Viewer {
    streams: usize,
    grid: Option<(u64, u64)>,
}
struct Session {
    snapshot: TerminalSessionSnapshot,
    history: BoundedTerminalHistory,
    sanitizer: HistorySanitizer,
    process: Option<Process>,
    generation: u64,
    runtime_env: Option<Environment>,
    stop_on_settle: bool,
    has_subprocess: bool,
    dirty: bool,
    pending: String,
    persisted: bool,
    inputs: u64,
}
impl Session {
    fn snapshot(&self) -> TerminalSessionSnapshot {
        let mut s = self.snapshot.clone();
        s.history = self.history.value();
        s
    }
    fn summary(&self) -> TerminalSummary {
        let s = &self.snapshot;
        TerminalSummary {
            thread_id: s.thread_id.clone(),
            terminal_id: s.terminal_id.clone(),
            cwd: s.cwd.clone(),
            worktree_path: s.worktree_path.clone(),
            status: s.status,
            pid: s.pid,
            exit_code: s.exit_code,
            exit_signal: s.exit_signal,
            has_running_subprocess: self.has_subprocess,
            label: s.label.clone(),
            updated_at: s.updated_at.clone(),
        }
    }
    fn stamp(&mut self) -> EventIdentity {
        self.snapshot.updated_at = now();
        let seq = self
            .snapshot
            .sequence
            .get_or_insert(Some(0))
            .get_or_insert(0);
        *seq += 1;
        EventIdentity {
            thread_id: self.snapshot.thread_id.clone(),
            terminal_id: self.snapshot.terminal_id.clone(),
            sequence: Some(Some(*seq)),
        }
    }
}
struct Attached {
    key: Key,
    queue: Weak<Queue<TerminalAttachStreamEvent>>,
    max_history: Option<usize>,
}
#[derive(Default)]
pub(crate) struct Hub {
    summaries: BTreeMap<Key, TerminalSummary>,
    pub events: BTreeMap<u64, Weak<Queue<TerminalEvent>>>,
    pub metadata: BTreeMap<u64, Weak<Queue<TerminalMetadataStreamEvent>>>,
    next: u64,
    stopped: bool,
}
impl Hub {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    pub fn summaries(&self) -> Vec<TerminalSummary> {
        let mut out = self.summaries.values().cloned().collect::<Vec<_>>();
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then(a.thread_id.cmp(&b.thread_id))
                .then(a.terminal_id.cmp(&b.terminal_id))
        });
        out
    }
}
pub(crate) struct State {
    hub: Arc<Mutex<Hub>>,
    sessions: BTreeMap<Key, Session>,
    closed: BTreeSet<Key>,
    attached: BTreeMap<u64, Attached>,
    viewers: BTreeMap<Key, BTreeMap<String, Viewer>>,
    next: u64,
    stopped: bool,
    retiring: Vec<watch::Receiver<bool>>,
    quiet: BTreeMap<Key, (u64, u64, u8)>,
    quiet_ticks: u8,
}
impl State {
    fn new(hub: Arc<Mutex<Hub>>) -> Self {
        Self {
            hub,
            sessions: BTreeMap::new(),
            closed: BTreeSet::new(),
            attached: BTreeMap::new(),
            viewers: BTreeMap::new(),
            next: 0,
            stopped: false,
            retiring: Vec::new(),
            quiet: BTreeMap::new(),
            quiet_ticks: 0,
        }
    }
    fn grid(&self, key: &Key) -> Option<(u64, u64)> {
        self.viewers
            .get(key)?
            .values()
            .filter_map(|v| v.grid)
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1)))
    }
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    fn publish(&mut self, event: TerminalEvent) {
        let id = event.identity();
        let key = (id.thread_id, id.terminal_id);
        self.attached.retain(|_, listener| {
            if let Some(queue) = listener.queue.upgrade() {
                if listener.key == key {
                    let attached = bounded_event(event.attached(), listener.max_history);
                    let size = attached_size(&attached);
                    queue.push(attached, size);
                }
                true
            } else {
                false
            }
        });
        let mut hub = lock(&self.hub);
        if hub.stopped {
            return;
        }
        if let Some(session) = self.sessions.get(&key) {
            hub.summaries.insert(key.clone(), session.summary());
        } else {
            hub.summaries.remove(&key);
        }
        hub.publish(event);
    }
}
impl Hub {
    fn publish(&mut self, event: TerminalEvent) {
        let id = event.identity();
        let key = (id.thread_id, id.terminal_id);
        let event_size = crate::stream::event_size(&event);
        self.events.retain(|_, weak| {
            if let Some(q) = weak.upgrade() {
                q.push(event.clone(), event_size);
                true
            } else {
                false
            }
        });
        let metadata = match &event {
            TerminalEvent::TerminalOutputEvent(..) | TerminalEvent::TerminalClearedEvent(..) => {
                None
            }
            TerminalEvent::TerminalClosedEvent(..) => {
                Some(TerminalMetadataStreamEvent::TerminalMetadataRemoveEvent(
                    TerminalMetadataRemoveEvent {
                        r#type: TerminalMetadataRemoveEventType::Remove,
                        thread_id: key.0.clone(),
                        terminal_id: key.1.clone(),
                    },
                ))
            }
            _ => self.summaries.get(&key).map(|terminal| {
                TerminalMetadataStreamEvent::TerminalMetadataUpsertEvent(
                    TerminalMetadataUpsertEvent {
                        r#type: TerminalMetadataUpsertEventType::Upsert,
                        terminal: terminal.clone(),
                    },
                )
            }),
        };
        if let Some(event) = metadata {
            self.metadata.retain(|_, weak| {
                if let Some(q) = weak.upgrade() {
                    q.push(event.clone(), crate::stream::metadata_size(&event));
                    true
                } else {
                    false
                }
            });
        }
    }
}
pub(crate) struct Inner {
    pub state: Arc<Mutex<Hub>>,
    states: Mutex<BTreeMap<Key, Arc<Mutex<State>>>>,
    stopped: AtomicBool,
    wake: [Arc<tokio::sync::Notify>; 2],
    options: ManagerOptions,
    store: HistoryStore,
    pub(crate) runtime: tokio::runtime::Handle,
}
/// Cloneable transport-neutral service. Drop of the final handle closes owned children;
/// `shutdown` additionally waits for PTY readers and flushes persistence.
#[derive(Clone)]
pub struct TerminalManager {
    inner: Arc<Inner>,
    _lifetime: Arc<ManagerLifetime>,
}
// Background operations temporarily retain Inner; public handle lifetime owns shutdown.
struct ManagerLifetime(Arc<Inner>);
impl Drop for ManagerLifetime {
    fn drop(&mut self) {
        let inner = self.0.clone();
        let runtime = inner.runtime.clone();
        runtime.spawn_blocking(move || inner.stop_now());
    }
}
impl TerminalManager {
    pub fn new(options: ManagerOptions) -> Result<Self, TerminalError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| TerminalError::Worker(e.to_string()))?;
        if options.persist_debounce.is_zero() || options.subprocess_poll_interval.is_zero() {
            return Err(TerminalError::InvalidInput(
                "worker intervals must be positive".into(),
            ));
        }
        let inner = Arc::new(Inner {
            store: HistoryStore::new(
                &options.logs_dir,
                options.history_line_limit,
                options.history_byte_limit,
            ),
            options,
            state: Arc::new(Mutex::new(Hub::default())),
            states: Mutex::new(BTreeMap::new()),
            stopped: AtomicBool::new(false),
            wake: std::array::from_fn(|_| Arc::new(tokio::sync::Notify::new())),
            runtime,
        });
        for persistence in [true, false] {
            let weak = Arc::downgrade(&inner);
            let wake = inner.wake[usize::from(persistence)].clone();
            let base_delay = if persistence {
                inner.options.persist_debounce
            } else {
                inner.options.subprocess_poll_interval
            };
            inner.runtime.spawn(async move {
                let mut delay = base_delay;
                let mut failures = 0_u32;
                loop {
                    if let Some(inner) = weak.upgrade() {
                        if inner.stopped.load(Ordering::Acquire) {
                            break;
                        }
                        let idle = lock(&inner.state).summaries.is_empty();
                        drop(inner);
                        if idle {
                            wake.notified().await;
                        } else {
                            tokio::time::sleep(delay).await;
                        }
                    } else {
                        break;
                    }
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    if lock(&inner.state).stopped {
                        break;
                    }
                    let started = tokio::time::Instant::now();
                    let result = tokio::task::spawn_blocking(move || {
                        if persistence {
                            inner.flush_all()
                        } else {
                            inner.poll_activity(false)
                        }
                    })
                    .await;
                    if let Err(error) = &result {
                        tracing::error!(%error, "terminal worker panicked");
                    }
                    if let Ok(Err(error)) = &result {
                        tracing::warn!(%error,"terminal background worker failed");
                    }
                    if !persistence {
                        failures = if matches!(result, Ok(Ok(()))) {
                            0
                        } else {
                            failures.saturating_add(1).min(30)
                        };
                        delay = base_delay
                            .saturating_mul(2_u32.saturating_pow(failures))
                            .max(started.elapsed())
                            .min(Duration::from_secs(60));
                    }
                }
            });
        }
        let lifetime = Arc::new(ManagerLifetime(inner.clone()));
        Ok(Self {
            inner,
            _lifetime: lifetime,
        })
    }
    async fn run<T: Send + 'static>(
        &self,
        op: impl FnOnce(Arc<Inner>) -> Result<T, TerminalError> + Send + 'static,
    ) -> Result<T, TerminalError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || op(inner))
            .await
            .map_err(|e| TerminalError::Worker(e.to_string()))?
    }
    pub async fn open(
        &self,
        input: TerminalOpenInput,
    ) -> Result<TerminalSessionSnapshot, TerminalError> {
        self.run(move |inner| {
            let cell = inner.cell(&(input.thread_id.0.clone(), input.terminal_id.0.clone()));
            let mut state = lock(&cell);
            let result = inner.open_locked(&mut state, input);
            drop(state);
            inner.evict_inactive();
            result
        })
        .await
    }
    pub async fn attach(
        &self,
        input: TerminalAttachInput,
        viewer_id: Option<String>,
    ) -> Result<AttachStream, TerminalError> {
        self.run(move |inner| inner.attach(input, viewer_id, false))
            .await
    }
    pub async fn observe(
        &self,
        input: TerminalObserveInput,
    ) -> Result<AttachStream, TerminalError> {
        self.run(move |inner| {
            inner.attach(
                TerminalAttachInput {
                    thread_id: input.thread_id,
                    terminal_id: input.terminal_id,
                    cwd: None,
                    worktree_path: None,
                    cols: None,
                    rows: None,
                    env: None,
                    provider_instance_id: None,
                    stop_on_settle: None,
                    restart_if_not_running: None,
                    resume: None,
                    max_history_bytes: None,
                },
                None,
                true,
            )
        })
        .await
    }
    pub async fn get_shell(&self, input: TerminalObserveInput) -> Result<String, TerminalError> {
        self.run(move |inner| {
            let cell = inner.cell(&(input.thread_id.0.clone(), input.terminal_id.0.clone()));
            let state = lock(&cell);
            let s = require(&state, &(input.thread_id.0, input.terminal_id.0))?;
            s.process
                .as_ref()
                .map(|p| p.shell.clone())
                .ok_or_else(|| not_running(&s.snapshot))
        })
        .await
    }
    pub async fn snapshot(
        &self,
        input: TerminalObserveInput,
    ) -> Result<TerminalSessionSnapshot, TerminalError> {
        self.run(move |inner| {
            let key = (input.thread_id.0, input.terminal_id.0);
            let cell = inner.cell(&key);
            Ok(require(&lock(&cell), &key)?.snapshot())
        })
        .await
    }
    /// Validation and enqueue happen at invocation, before the returned future is polled.
    pub fn write(
        &self,
        input: TerminalWriteInput,
    ) -> impl std::future::Future<Output = Result<(), TerminalError>> + Send + use<> {
        let queued = (|| {
            validate_id(&input.thread_id.0, &input.terminal_id.0)?;
            if input.data.is_empty() || input.data.encode_utf16().count() > 65536 {
                return Err(TerminalError::InvalidInput(
                    "data length must be 1..65536".into(),
                ));
            }
            let cell = self
                .inner
                .cell(&(input.thread_id.0.clone(), input.terminal_id.0.clone()));
            let mut state = lock(&cell);
            let s = require_mut(&mut state, &(input.thread_id.0, input.terminal_id.0))?;
            if s.snapshot.status == TerminalSessionStatus::Exited {
                return Ok(None);
            }
            let process = s.process.as_ref().ok_or_else(|| not_running(&s.snapshot))?;
            let receive = process.writer().enqueue(&input.data)?;
            s.inputs += 1;
            Ok(Some(receive))
        })();
        async move {
            match queued? {
                Some(receive) => receive
                    .await
                    .map_err(|e| TerminalError::Worker(e.to_string()))?,
                None => Ok(()),
            }
        }
    }
    pub async fn resize(
        &self,
        input: TerminalResizeInput,
        viewer_id: Option<String>,
    ) -> Result<(), TerminalError> {
        self.run(move |inner| {
            validate_grid(Some(input.cols), Some(input.rows))?;
            let key = (input.thread_id.0, input.terminal_id.0);
            let cell = inner.cell(&key);
            let mut state = lock(&cell);
            let target = if let Some(v) = viewer_id.and_then(|id| {
                state
                    .viewers
                    .get_mut(&key)
                    .and_then(|viewers| viewers.get_mut(&id))
            }) {
                v.grid = Some((input.cols, input.rows));
                state.grid(&key)
            } else if state.grid(&key).is_some() {
                None
            } else {
                Some((input.cols, input.rows))
            };
            if let Some(grid) = target {
                resize_session(&mut state, &key, grid)?;
            }
            Ok(())
        })
        .await
    }
    pub async fn clear(&self, input: TerminalClearInput) -> Result<(), TerminalError> {
        self.run(move |inner| {
            let cell = inner.cell(&(input.thread_id.0.clone(), input.terminal_id.0.clone()));
            let mut state = lock(&cell);
            let s = require_mut(
                &mut state,
                &(input.thread_id.0.clone(), input.terminal_id.0.clone()),
            )?;
            s.history.clear();
            if let Some(process) = &s.process {
                process.clear_output();
            }
            s.sanitizer.clear();
            s.pending.clear();
            s.dirty = false;
            s.persisted = warn_persist(inner.store.persist(
                &input.thread_id.0,
                &input.terminal_id.0,
                "",
            ));
            s.dirty = !s.persisted;
            let id = s.stamp();
            state.publish(TerminalEvent::TerminalClearedEvent(TerminalClearedEvent {
                r#type: TerminalClearedEventType::Cleared,
                thread_id: id.thread_id,
                terminal_id: id.terminal_id,
                sequence: id.sequence,
            }));
            Ok(())
        })
        .await
    }
    pub async fn close(&self, input: TerminalCloseInput) -> Result<(), TerminalError> {
        self.run(move |inner| {
            for (key, cell) in inner.cells() {
                if key.0 != input.thread_id.0
                    || input
                        .terminal_id
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|id| id.0 != key.1)
                {
                    continue;
                }
                inner.close_locked(&mut lock(&cell), &key)?;
            }
            if input.delete_history.flatten() == Some(true) {
                warn_persist(
                    inner.store.delete(
                        &input.thread_id.0,
                        input
                            .terminal_id
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|id| id.0.as_str()),
                    ),
                );
            }
            Ok(())
        })
        .await
    }
    pub async fn close_idle(&self, input: TerminalCloseInput) -> Result<(), TerminalError> {
        self.run(move |inner| inner.close_idle(&input)).await
    }
    pub async fn close_on_settle(&self, input: TerminalThreadInput) -> Result<(), TerminalError> {
        let thread = input.thread_id.0;
        let copy = thread.clone();
        self.run(move |inner| {
            for (key, cell) in inner.cells() {
                if key.0 == copy {
                    let mut state = lock(&cell);
                    if state.sessions.get(&key).is_some_and(|s| s.stop_on_settle) {
                        inner.close_locked(&mut state, &key)?;
                    }
                }
            }
            Ok(())
        })
        .await?;
        self.close_idle(TerminalCloseInput {
            thread_id: t3_contracts::base_schemas::TrimmedNonEmptyString(thread),
            terminal_id: None,
            delete_history: None,
        })
        .await
    }
    pub fn subscribe(&self) -> EventStream {
        let queue = Queue::new(self.inner.options.max_queued_output);
        let mut state = lock(&self.inner.state);
        let registration = state.id();
        state.events.insert(registration, Arc::downgrade(&queue));
        if state.stopped {
            queue.close();
        }
        EventStream { queue }
    }
    pub fn subscribe_metadata(&self) -> MetadataStream {
        let queue = Queue::new(self.inner.options.max_queued_output);
        let mut state = lock(&self.inner.state);
        let registration = state.id();
        queue.push(
            TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(
                TerminalMetadataSnapshotEvent {
                    r#type: TerminalMetadataSnapshotEventType::Snapshot,
                    terminals: state.summaries(),
                },
            ),
            128,
        );
        state.metadata.insert(registration, Arc::downgrade(&queue));
        if state.stopped {
            queue.close();
        }
        MetadataStream {
            queue,
            manager: Arc::downgrade(&self.inner),
        }
    }
    pub async fn flush(&self) -> Result<(), TerminalError> {
        self.run(|inner| inner.flush_all()).await
    }
    pub async fn refresh_activity(&self) -> Result<(), TerminalError> {
        self.run(|inner| inner.poll_activity(true)).await
    }
    pub async fn shutdown(&self) -> Result<(), TerminalError> {
        let readers = self
            .run(|inner| {
                inner.stopped.store(true, Ordering::Release);
                {
                    let mut hub = lock(&inner.state);
                    hub.stopped = true;
                    close_hub(&hub);
                    hub.summaries.clear();
                }
                for wake in &inner.wake {
                    wake.notify_one();
                }
                let mut readers = Vec::new();
                let mut error = None;
                for (key, cell) in inner.cells() {
                    let mut state = lock(&cell);
                    state.stopped = true;
                    close_queues(&state);
                    if let Some(mut session) = state.sessions.remove(&key) {
                        if let Some(mut process) = session.process.take() {
                            process.stop();
                            readers.push(process.done.clone());
                        }
                        if let Err(e) = inner.flush_session(&key, &mut session) {
                            error = Some(e);
                        }
                    }
                    readers.append(&mut state.retiring);
                }
                Ok((readers, error))
            })
            .await?;
        if tokio::time::timeout(Duration::from_secs(2), async {
            for mut done in readers.0 {
                while !*done.borrow_and_update() {
                    if done.changed().await.is_err() {
                        break;
                    }
                }
            }
        })
        .await
        .is_err()
        {
            tracing::warn!("terminal shutdown drain reached its deadline");
        }
        if let Some(e) = readers.1 {
            return Err(e);
        }
        Ok(())
    }
}

impl Inner {
    fn cell(&self, key: &Key) -> Arc<Mutex<State>> {
        lock(&self.states)
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(State::new(self.state.clone()))))
            .clone()
    }
    fn cells(&self) -> Vec<(Key, Arc<Mutex<State>>)> {
        lock(&self.states)
            .iter()
            .map(|(key, cell)| (key.clone(), cell.clone()))
            .collect()
    }
    fn session_cells(&self) -> Vec<(Key, Arc<Mutex<State>>)> {
        let keys = lock(&self.state)
            .summaries
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let cells = lock(&self.states);
        keys.into_iter()
            .filter_map(|key| cells.get(&key).map(|cell| (key, cell.clone())))
            .collect()
    }
    fn resolve_env(&self, input: &mut TerminalOpenInput) -> Result<(), TerminalError> {
        if let Some(id) = input.provider_instance_id.as_ref().and_then(Option::as_ref) {
            input.env = Some(Some(self
                .options
                .resolve_provider_environment
                .as_ref()
                .ok_or_else(|| TerminalError::ProviderInstanceNotFound(id.0.clone()))?(
                &id.0,
                input.env.as_ref().and_then(Option::as_ref),
            )?));
        }
        let mut env = input.env.take().flatten();
        normalize_env(&mut env);
        input.env = env.map(Some);
        Ok(())
    }
    fn open_locked(
        self: &Arc<Self>,
        state: &mut State,
        mut input: TerminalOpenInput,
    ) -> Result<TerminalSessionSnapshot, TerminalError> {
        if state.stopped || self.stopped.load(Ordering::Acquire) {
            return Err(TerminalError::Stopped);
        }
        validate_open(&input)?;
        self.resolve_env(&mut input)?;
        validate_cwd(&input.cwd.0)?;
        let key = (input.thread_id.0.clone(), input.terminal_id.0.clone());
        if !state.sessions.contains_key(&key) {
            let inherited = self.store.read(&key.0, &key.1)?;
            let inherited = neutralize(
                &inherited,
                self.options.history_line_limit,
                self.options
                    .history_byte_limit
                    .min(DEFAULT_MAX_TERMINAL_BUFFER_BYTES),
            );
            let mut history = BoundedTerminalHistory::new(
                self.options.history_line_limit,
                self.options.history_byte_limit,
            );
            history.append(&inherited);
            state.closed.remove(&key);
            let generation = state.id();
            state.sessions.insert(
                key.clone(),
                Session {
                    snapshot: TerminalSessionSnapshot {
                        thread_id: key.0.clone(),
                        terminal_id: key.1.clone(),
                        cwd: input.cwd.0.clone(),
                        worktree_path: input.worktree_path.clone().flatten(),
                        status: TerminalSessionStatus::Starting,
                        pid: None,
                        history: String::new(),
                        exit_code: None,
                        exit_signal: None,
                        label: terminal_label(&key.1),
                        updated_at: now(),
                        sequence: Some(Some(0)),
                        cols: Some(Some(input.cols.flatten().unwrap_or(120))),
                        rows: Some(Some(input.rows.flatten().unwrap_or(30))),
                    },
                    history,
                    sanitizer: HistorySanitizer::default(),
                    process: None,
                    generation,
                    runtime_env: input.env.clone().flatten(),
                    stop_on_settle: input.stop_on_settle.flatten().unwrap_or(false),
                    has_subprocess: false,
                    dirty: false,
                    pending: String::new(),
                    persisted: false,
                    inputs: 0,
                },
            );
        } else {
            let s = require_mut(state, &key)?;
            let worktree = input
                .worktree_path
                .clone()
                .unwrap_or_else(|| s.snapshot.worktree_path.clone());
            let changed = s.snapshot.cwd != input.cwd.0
                || s.runtime_env != input.env.clone().flatten()
                || s.snapshot.worktree_path != worktree;
            if let Some(stop) = input.stop_on_settle.flatten() {
                s.stop_on_settle = stop;
            }
            if changed
                || matches!(
                    s.snapshot.status,
                    TerminalSessionStatus::Exited | TerminalSessionStatus::Error
                )
            {
                if let Some(mut p) = s.process.take() {
                    p.stop();
                    state.retiring.push(p.done.clone());
                }
                let s = require_mut(state, &key)?;
                s.history.clear();
                s.sanitizer.clear();
                s.pending.clear();
                s.dirty = false;
                s.runtime_env = input.env.clone().flatten();
                s.snapshot.cwd = input.cwd.0.clone();
                s.snapshot.worktree_path = worktree;
                s.snapshot.pid = None;
                s.snapshot.status = TerminalSessionStatus::Exited;
                s.persisted = warn_persist(self.store.persist(&key.0, &key.1, ""));
                s.dirty = !s.persisted;
            }
        }
        let grid = state.grid(&key);
        let s = require_mut(state, &key)?;
        let target = grid.unwrap_or((
            input
                .cols
                .flatten()
                .unwrap_or(s.snapshot.cols.flatten().unwrap_or(120)),
            input
                .rows
                .flatten()
                .unwrap_or(s.snapshot.rows.flatten().unwrap_or(30)),
        ));
        if s.process.is_some() {
            resize_session(state, &key, target)?;
            return Ok(require(state, &key)?.snapshot());
        }
        let env = spawn_environment(
            &self.options.env,
            s.runtime_env.as_ref(),
            Platform::current(),
            &self.options.home,
            &self.options.app_version,
            &self.options.managed_binary_directories,
        );
        let candidates = shell_candidates(
            Platform::current(),
            &self.options.env,
            self.options.preferred_shell.as_deref(),
        );
        let mut failure = String::new();
        let mut spawned = None;
        for shell in candidates {
            match Process::spawn(
                &shell,
                PathBuf::from(&input.cwd.0).as_path(),
                &env,
                target.0 as u16,
                target.1 as u16,
                self.options.kill_descendants,
            ) {
                Ok(p) => {
                    spawned = Some(p);
                    break;
                }
                Err(e) => {
                    let retry = matches!(&e, TerminalError::Pty { missing: true, .. });
                    failure = e.to_string();
                    if !retry {
                        break;
                    }
                }
            }
        }
        let generation = state.id();
        let s = require_mut(state, &key)?;
        s.generation = generation;
        s.snapshot.cols = Some(Some(target.0));
        s.snapshot.rows = Some(Some(target.1));
        s.snapshot.exit_code = None;
        s.snapshot.exit_signal = None;
        s.has_subprocess = false;
        s.snapshot.label = terminal_label(&key.1);
        if let Some((process, mut receive)) = spawned {
            s.snapshot.pid = Some(u64::from(process.pid));
            s.snapshot.status = TerminalSessionStatus::Running;
            s.process = Some(process);
            let id = s.stamp();
            let snapshot = s.snapshot();
            state.publish(TerminalEvent::TerminalStartedEvent(TerminalStartedEvent {
                r#type: TerminalStartedEventType::Started,
                thread_id: id.thread_id,
                terminal_id: id.terminal_id,
                sequence: id.sequence,
                snapshot: snapshot.clone(),
            }));
            let weak = Arc::downgrade(self);
            let event_key = key.clone();
            self.runtime.spawn(async move {
                while let Some(event) = receive.recv().await {
                    let Some(inner) = weak.upgrade() else {
                        continue;
                    };
                    let key = event_key.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        inner.process_event(key, generation, event)
                    })
                    .await;
                }
            });
            for wake in &self.wake {
                wake.notify_one();
            }
            Ok(snapshot)
        } else {
            s.snapshot.pid = None;
            s.snapshot.status = TerminalSessionStatus::Error;
            let id = s.stamp();
            let snapshot = s.snapshot();
            state.publish(TerminalEvent::TerminalErrorEvent(TerminalErrorEvent {
                r#type: TerminalErrorEventType::Error,
                thread_id: id.thread_id,
                terminal_id: id.terminal_id,
                sequence: id.sequence,
                message: failure,
            }));
            for wake in &self.wake {
                wake.notify_one();
            }
            Ok(snapshot)
        }
    }
    fn attach(
        self: &Arc<Self>,
        input: TerminalAttachInput,
        viewer: Option<String>,
        observe: bool,
    ) -> Result<AttachStream, TerminalError> {
        validate_id(&input.thread_id.0, &input.terminal_id.0)?;
        validate_grid(input.cols.flatten(), input.rows.flatten())?;
        let key = (input.thread_id.0.clone(), input.terminal_id.0.clone());
        let cell = self.cell(&key);
        let mut state = lock(&cell);
        if state.stopped || self.stopped.load(Ordering::Acquire) {
            return Err(TerminalError::Stopped);
        }
        let registration = state.id();
        let viewer = if observe {
            None
        } else {
            Some(viewer.unwrap_or_else(|| format!("stream:{registration}")))
        };
        let missing = !state.sessions.contains_key(&key);
        let closed = input.resume.flatten() == Some(true) && state.closed.contains(&key);
        if observe && missing {
            return Err(lookup(&key));
        }
        let restart = !observe
            && !missing
            && input.resume.flatten() != Some(true)
            && input.restart_if_not_running.flatten() == Some(true)
            && state
                .sessions
                .get(&key)
                .is_some_and(|s| s.process.is_none())
            && input.cwd.as_ref().is_some_and(Option::is_some);
        if !observe
            && ((missing && !closed) || restart)
            && input.cwd.as_ref().is_none_or(Option::is_none)
        {
            return Err(lookup(&key));
        }
        if let Some(viewer) = &viewer {
            let grid = input.cols.flatten().zip(input.rows.flatten());
            let v = state
                .viewers
                .entry(key.clone())
                .or_default()
                .entry(viewer.clone())
                .or_insert(Viewer { streams: 0, grid });
            v.streams += 1;
            if v.grid.is_none() {
                v.grid = grid;
            }
        }
        if !observe && ((missing && !closed) || restart) {
            let cwd = input.cwd.clone().flatten().ok_or_else(|| lookup(&key))?;
            let opened = self.open_locked(
                &mut state,
                TerminalOpenInput {
                    thread_id: t3_contracts::base_schemas::TrimmedNonEmptyString(key.0.clone()),
                    terminal_id: t3_contracts::base_schemas::TrimmedNonEmptyString(key.1.clone()),
                    cwd,
                    worktree_path: input.worktree_path.clone(),
                    cols: input.cols,
                    rows: input.rows,
                    env: input.env.clone(),
                    provider_instance_id: input.provider_instance_id.clone(),
                    stop_on_settle: input.stop_on_settle,
                },
            );
            if let Err(error) = opened {
                drop(state);
                self.detach(registration, &key, viewer.as_deref());
                return Err(error);
            }
        }
        if let Some(viewer) = &viewer {
            let target = state.grid(&key);
            if let Some(grid) = target
                && let Err(error) = resize_session(&mut state, &key, grid)
            {
                drop(state);
                self.detach(registration, &key, Some(viewer.as_str()));
                return Err(error);
            }
        }
        drop(state);
        self.evict_inactive();
        let mut state = lock(&cell);
        let queue = Queue::new(self.options.max_queued_output);
        let event = snapshot_event(
            &state,
            &key,
            input
                .max_history_bytes
                .flatten()
                .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        );
        let size = attached_size(&event);
        queue.push(event, size);
        state.attached.insert(
            registration,
            Attached {
                key: key.clone(),
                queue: Arc::downgrade(&queue),
                max_history: input
                    .max_history_bytes
                    .flatten()
                    .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
            },
        );
        Ok(AttachStream {
            queue,
            manager: Arc::downgrade(self),
            key,
            registration,
            viewer,
            max_history: input
                .max_history_bytes
                .flatten()
                .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        })
    }
    pub fn detach(self: &Arc<Self>, registration: u64, key: &Key, viewer: Option<&str>) {
        let cell = self.cell(key);
        let mut state = lock(&cell);
        state.attached.remove(&registration);
        if let Some(viewers) = state.viewers.get_mut(key) {
            if let Some(viewer) = viewer
                && let Some(v) = viewers.get_mut(viewer)
            {
                v.streams -= 1;
                if v.streams == 0 {
                    viewers.remove(viewer);
                }
            }
            if viewers.is_empty() {
                state.viewers.remove(key);
            }
            if state.grid(key).is_some() {
                let weak = Arc::downgrade(self);
                let key = key.clone();
                self.runtime.spawn_blocking(move || {
                    if let Some(inner) = weak.upgrade() {
                        let cell = inner.cell(&key);
                        let mut state = lock(&cell);
                        if let Some(grid) = state.grid(&key) {
                            let _ = resize_session(&mut state, &key, grid);
                        }
                    }
                });
            }
        }
    }
    pub fn resync(&self, key: &Key, queue: &Queue<TerminalAttachStreamEvent>, max: Option<usize>) {
        let cell = self.cell(key);
        let state = lock(&cell);
        let event = snapshot_event(&state, key, max);
        let size = attached_size(&event);
        queue.reset(event, size);
    }
    fn process_event(&self, key: Key, generation: u64, event: ProcessEvent) {
        let cell = self.cell(&key);
        let mut state = lock(&cell);
        if state.stopped {
            return;
        }
        let Some(s) = state.sessions.get_mut(&key) else {
            return;
        };
        if s.generation != generation || s.snapshot.status != TerminalSessionStatus::Running {
            return;
        }
        let inactive = !matches!(&event, ProcessEvent::Output { .. });
        let event = match event {
            ProcessEvent::Output { data, epoch } => {
                if s.process.as_ref().is_none_or(|p| p.output_epoch() != epoch) {
                    return;
                }
                let visible = s.sanitizer.push(&data);
                if !visible.is_empty() {
                    s.history.append(&visible);
                    s.pending.push_str(&visible);
                    s.dirty = true;
                    if s.pending.len() > self.options.history_byte_limit {
                        s.pending = s.history.value();
                        if self.store.persist(&key.0, &key.1, &s.pending).is_ok() {
                            s.pending.clear();
                            s.dirty = false;
                            s.persisted = true;
                        } else {
                            s.persisted = false;
                        }
                    }
                }
                let id = s.stamp();
                TerminalEvent::TerminalOutputEvent(TerminalOutputEvent {
                    r#type: TerminalOutputEventType::Output,
                    thread_id: id.thread_id,
                    terminal_id: id.terminal_id,
                    sequence: id.sequence,
                    data,
                })
            }
            ProcessEvent::Exit { code, signal } => {
                s.snapshot.status = TerminalSessionStatus::Exited;
                s.snapshot.pid = None;
                s.snapshot.exit_code = code;
                s.snapshot.exit_signal = signal.map(i64::from);
                s.has_subprocess = false;
                s.snapshot.label = terminal_label(&key.1);
                if let Some(p) = s.process.take() {
                    state.retiring.push(p.done.clone());
                }
                let Some(s) = state.sessions.get_mut(&key) else {
                    return;
                };
                let _ = self.flush_session(&key, s);
                let id = s.stamp();
                TerminalEvent::TerminalExitedEvent(TerminalExitedEvent {
                    r#type: TerminalExitedEventType::Exited,
                    thread_id: id.thread_id,
                    terminal_id: id.terminal_id,
                    sequence: id.sequence,
                    exit_code: code,
                    exit_signal: signal.map(i64::from),
                })
            }
            ProcessEvent::Error(message) => {
                s.snapshot.status = TerminalSessionStatus::Error;
                s.snapshot.pid = None;
                s.has_subprocess = false;
                if let Some(mut process) = s.process.take() {
                    process.stop();
                    state.retiring.push(process.done.clone());
                }
                let Some(s) = state.sessions.get_mut(&key) else {
                    return;
                };
                let id = s.stamp();
                TerminalEvent::TerminalErrorEvent(TerminalErrorEvent {
                    r#type: TerminalErrorEventType::Error,
                    thread_id: id.thread_id,
                    terminal_id: id.terminal_id,
                    sequence: id.sequence,
                    message,
                })
            }
        };
        state.publish(event);
        drop(state);
        if inactive {
            self.evict_inactive();
        }
    }
    fn close_locked(&self, state: &mut State, key: &Key) -> Result<(), TerminalError> {
        state.closed.insert(key.clone());
        state.quiet.remove(key);
        if let Some(mut s) = state.sessions.remove(key) {
            if let Some(mut p) = s.process.take() {
                p.stop();
                state.retiring.push(p.done.clone());
            }
            warn_persist(self.flush_session(key, &mut s));
            warn_persist(self.store.persist(&key.0, &key.1, &s.history.value()));
            let id = s.stamp();
            state.publish(TerminalEvent::TerminalClosedEvent(TerminalClosedEvent {
                r#type: TerminalClosedEventType::Closed,
                thread_id: id.thread_id,
                terminal_id: id.terminal_id,
                sequence: id.sequence,
            }));
        }
        Ok(())
    }
    fn flush_session(&self, key: &Key, s: &mut Session) -> Result<(), TerminalError> {
        if s.dirty {
            let result = if s.persisted {
                self.store
                    .append(&key.0, &key.1, &s.pending, || s.history.value())
            } else {
                self.store.persist(&key.0, &key.1, &s.history.value())
            };
            if let Err(error) = result {
                s.persisted = false;
                return Err(error);
            }
            s.persisted = true;
            s.pending.clear();
            s.dirty = false;
        }
        Ok(())
    }
    fn flush_all(&self) -> Result<(), TerminalError> {
        let mut error = None;
        for (key, cell) in self.session_cells() {
            let mut state = lock(&cell);
            if let Some(s) = state.sessions.get_mut(&key)
                && let Err(e) = self.flush_session(&key, s)
            {
                error = Some(e);
            }
            state.retiring.retain(|r| !*r.borrow());
        }
        error.map_or(Ok(()), Err)
    }
    fn evict_inactive(&self) {
        let mut inactive = {
            let hub = lock(&self.state);
            hub.summaries
                .iter()
                .filter(|(_, s)| s.status != TerminalSessionStatus::Running)
                .map(|(key, s)| (s.updated_at.clone(), key.clone()))
                .collect::<Vec<_>>()
        };
        inactive.sort();
        let n = inactive
            .len()
            .saturating_sub(self.options.max_retained_inactive_sessions);
        for (_, key) in inactive.into_iter().take(n) {
            let cell = self.cell(&key);
            let mut state = lock(&cell);
            if state
                .sessions
                .get(&key)
                .is_some_and(|s| s.snapshot.status == TerminalSessionStatus::Running)
            {
                continue;
            }
            if let Some(mut session) = state.sessions.remove(&key) {
                warn_persist(self.flush_session(&key, &mut session));
            }
            state.closed.insert(key.clone());
            lock(&self.state).summaries.remove(&key);
        }
    }
    fn poll_activity(&self, force: bool) -> Result<(), TerminalError> {
        let marks = self.activity_marks();
        if marks.is_empty() {
            return Ok(());
        }
        if !force {
            let mut idle = true;
            for (key, generation, mark, _) in &marks {
                let cell = self.cell(key);
                let mut state = lock(&cell);
                if state
                    .quiet
                    .get(key)
                    .is_some_and(|q| q.0 == *generation && q.1 == *mark && q.2 == 2)
                    && state.quiet_ticks < 29
                {
                    state.quiet_ticks += 1;
                } else {
                    state.quiet_ticks = 0;
                    idle = false;
                }
            }
            if idle {
                return Ok(());
            }
        }
        let table = process_table()?;
        for (key, generation, mark, pid) in marks {
            let cell = self.cell(&key);
            let mut state = lock(&cell);
            let (busy, label) = inspect(&table, pid);
            let previous = state.quiet.remove(&key);
            let Some(s) = state.sessions.get_mut(&key) else {
                continue;
            };
            if s.generation != generation
                || s.snapshot.status != TerminalSessionStatus::Running
                || s.snapshot.sequence.flatten().unwrap_or(0) + s.inputs != mark
            {
                continue;
            }
            let label = label.unwrap_or_else(|| terminal_label(&key.1));
            let mut next_mark = mark;
            if s.has_subprocess != busy || s.snapshot.label != label {
                s.has_subprocess = busy;
                s.snapshot.label = label.clone();
                let id = s.stamp();
                next_mark += 1;
                state.publish(TerminalEvent::TerminalActivityEvent(
                    TerminalActivityEvent {
                        r#type: TerminalActivityEventType::Activity,
                        thread_id: id.thread_id,
                        terminal_id: id.terminal_id,
                        sequence: id.sequence,
                        has_running_subprocess: busy,
                        label,
                        cols: None,
                        rows: None,
                    },
                ));
            }
            if !busy {
                let count = if previous.is_some_and(|p| p.0 == generation && p.1 == mark) {
                    2
                } else {
                    1
                };
                state.quiet.insert(key, (generation, next_mark, count));
            }
        }
        Ok(())
    }
    fn activity_marks(&self) -> Vec<(Key, u64, u64, u32)> {
        self.session_cells()
            .into_iter()
            .flat_map(|(_key, cell)| {
                let state = lock(&cell);
                state
                    .sessions
                    .iter()
                    .filter_map(|(key, s)| {
                        s.snapshot
                            .pid
                            .and_then(|pid| u32::try_from(pid).ok())
                            .map(|pid| {
                                (
                                    key.clone(),
                                    s.generation,
                                    s.snapshot.sequence.flatten().unwrap_or(0) + s.inputs,
                                    pid,
                                )
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    fn close_idle(&self, input: &TerminalCloseInput) -> Result<(), TerminalError> {
        let marks = self
            .activity_marks()
            .into_iter()
            .filter(|(k, _, _, _)| {
                k.0 == input.thread_id.0
                    && input
                        .terminal_id
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_none_or(|id| id.0 == k.1)
            })
            .collect::<Vec<_>>();
        if marks.is_empty() {
            return Ok(());
        }
        let table = match process_table() {
            Ok(table) => table,
            Err(error) => {
                tracing::warn!(%error,"idle terminal inspection failed");
                return Ok(());
            }
        };
        for (key, generation, mark, pid) in marks {
            let cell = self.cell(&key);
            let mut state = lock(&cell);
            if inspect(&table, pid).0 {
                continue;
            }
            if state.sessions.get(&key).is_some_and(|s| {
                s.generation == generation
                    && s.snapshot.status == TerminalSessionStatus::Running
                    && s.snapshot.sequence.flatten().unwrap_or(0) + s.inputs == mark
            }) {
                self.close_locked(&mut state, &key)?;
            }
        }
        Ok(())
    }
}
impl Inner {
    fn stop_now(&self) {
        self.stopped.store(true, Ordering::Release);
        {
            let mut state = lock(&self.state);
            state.stopped = true;
            close_hub(&state);
        }
        for wake in &self.wake {
            wake.notify_one();
        }
        for (key, cell) in self.cells() {
            let mut state = lock(&cell);
            state.stopped = true;
            close_queues(&state);
            if let Some(s) = state.sessions.get_mut(&key) {
                if let Some(mut process) = s.process.take() {
                    process.stop();
                }
                warn_persist(self.store.persist(&key.0, &key.1, &s.history.value()));
            }
        }
    }
}

fn close_queues(state: &State) {
    for q in state.attached.values().filter_map(|a| a.queue.upgrade()) {
        q.close();
    }
}
fn close_hub(state: &Hub) {
    for q in state.events.values().filter_map(Weak::upgrade) {
        q.close();
    }
    for q in state.metadata.values().filter_map(Weak::upgrade) {
        q.close();
    }
}
fn resize_session(state: &mut State, key: &Key, grid: (u64, u64)) -> Result<(), TerminalError> {
    let Some(s) = state.sessions.get_mut(key) else {
        return Ok(());
    };
    if s.snapshot.status != TerminalSessionStatus::Running
        || (s.snapshot.cols == Some(Some(grid.0)) && s.snapshot.rows == Some(Some(grid.1)))
    {
        return Ok(());
    }
    if let Some(p) = &s.process {
        p.resize(grid.0 as u16, grid.1 as u16)?;
    }
    s.snapshot.cols = Some(Some(grid.0));
    s.snapshot.rows = Some(Some(grid.1));
    let id = s.stamp();
    let label = s.snapshot.label.clone();
    let busy = s.has_subprocess;
    state.publish(TerminalEvent::TerminalActivityEvent(
        TerminalActivityEvent {
            r#type: TerminalActivityEventType::Activity,
            thread_id: id.thread_id,
            terminal_id: id.terminal_id,
            sequence: id.sequence,
            has_running_subprocess: busy,
            label,
            cols: Some(Some(grid.0)),
            rows: Some(Some(grid.1)),
        },
    ));
    Ok(())
}
fn snapshot_event(state: &State, key: &Key, max: Option<usize>) -> TerminalAttachStreamEvent {
    bounded_event(
        match state.sessions.get(key) {
            Some(s) => TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(
                TerminalAttachSnapshotEvent {
                    r#type: TerminalAttachSnapshotEventType::Snapshot,
                    snapshot: s.snapshot(),
                },
            ),
            None => TerminalAttachStreamEvent::TerminalClosedEvent(TerminalClosedEvent {
                r#type: TerminalClosedEventType::Closed,
                thread_id: key.0.clone(),
                terminal_id: key.1.clone(),
                sequence: None,
            }),
        },
        max,
    )
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn warn_persist(result: Result<(), TerminalError>) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error,"failed to persist terminal history");
            false
        }
    }
}
fn terminal_label(id: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^term(?:inal)?-(\d+)(?:-[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12})?$",
        )
        .expect("constant expression")
    });
    let label = RE
        .captures(id)
        .map(|c| format!("Terminal {}", &c[1]))
        .unwrap_or_else(|| id.into());
    label.chars().take(128).collect()
}
fn lookup(key: &Key) -> TerminalError {
    TerminalError::SessionLookup {
        thread_id: key.0.clone(),
        terminal_id: key.1.clone(),
    }
}
fn require<'a>(state: &'a State, key: &Key) -> Result<&'a Session, TerminalError> {
    state.sessions.get(key).ok_or_else(|| lookup(key))
}
fn require_mut<'a>(state: &'a mut State, key: &Key) -> Result<&'a mut Session, TerminalError> {
    state.sessions.get_mut(key).ok_or_else(|| lookup(key))
}
fn not_running(s: &TerminalSessionSnapshot) -> TerminalError {
    TerminalError::NotRunning {
        thread_id: s.thread_id.clone(),
        terminal_id: s.terminal_id.clone(),
    }
}
fn validate_cwd(cwd: &str) -> Result<(), TerminalError> {
    match std::fs::metadata(cwd) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(TerminalError::CwdNotDirectory { cwd: cwd.into() }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(TerminalError::CwdNotFound { cwd: cwd.into() })
        }
        Err(source) => Err(TerminalError::CwdStat {
            cwd: cwd.into(),
            source,
        }),
    }
}
fn validate_id(thread: &str, terminal: &str) -> Result<(), TerminalError> {
    if thread.trim().is_empty()
        || thread.trim() != thread
        || thread.contains('\0')
        || terminal.trim().is_empty()
        || terminal.trim() != terminal
        || terminal.encode_utf16().count() > 128
        || terminal.contains('\0')
    {
        return Err(TerminalError::InvalidInput(
            "invalid threadId or terminalId".into(),
        ));
    }
    Ok(())
}
fn validate_grid(cols: Option<u64>, rows: Option<u64>) -> Result<(), TerminalError> {
    if cols.is_some_and(|c| !(1..=1000).contains(&c))
        || rows.is_some_and(|r| !(1..=500).contains(&r))
    {
        return Err(TerminalError::InvalidInput(
            "grid outside 1..1000 columns / 1..500 rows".into(),
        ));
    }
    Ok(())
}
fn validate_open(input: &TerminalOpenInput) -> Result<(), TerminalError> {
    validate_id(&input.thread_id.0, &input.terminal_id.0)?;
    validate_grid(input.cols.flatten(), input.rows.flatten())?;
    if input.cwd.0.trim().is_empty()
        || input.cwd.0.trim() != input.cwd.0
        || input.cwd.0.contains('\0')
    {
        return Err(TerminalError::InvalidInput("invalid cwd".into()));
    }
    if let Some(env) = input.env.as_ref().and_then(Option::as_ref)
        && (env.len() > 128
            || env.iter().any(|(k, v)| {
                k.len() > 128
                    || k.is_empty()
                    || !k.chars().enumerate().all(|(i, c)| {
                        c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                    })
                    || v.encode_utf16().count() > 8192
                    || v.contains('\0')
            }))
    {
        return Err(TerminalError::InvalidInput("invalid env".into()));
    }
    Ok(())
}
fn normalize_env(env: &mut Option<Environment>) {
    if env.as_ref().is_some_and(Environment::is_empty) {
        *env = None;
    }
}
fn process_table() -> Result<sysinfo::System, TerminalError> {
    let mut system = sysinfo::System::new().map_err(|e| TerminalError::Worker(e.to_string()))?;
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    Ok(system)
}
fn inspect(table: &sysinfo::System, pid: u32) -> (bool, Option<String>) {
    let root = sysinfo::Pid::from_u32(pid);
    let Some(shell) = table.process(root) else {
        return (true, None);
    };
    let mut children = table
        .processes()
        .values()
        .filter(|p| p.parent() == Some(root))
        .collect::<Vec<_>>();
    children.sort_by_key(|p| p.pid());
    let child = children.into_iter().find(|p| {
        p.name() != shell.name()
            || table
                .processes()
                .values()
                .any(|c| c.parent() == Some(p.pid()))
    });
    match child {
        Some(p) => {
            let name = p.name().to_string_lossy();
            let name = name
                .trim()
                .trim_matches(['[', ']', '(', ')'])
                .rsplit(if cfg!(windows) {
                    &['/', '\\'][..]
                } else {
                    &['/'][..]
                })
                .next()
                .unwrap_or("")
                .trim();
            let name = if cfg!(windows) && name.to_lowercase().ends_with(".exe") {
                &name[..name.len() - 4]
            } else {
                name
            };
            (true, Some(name.chars().take(128).collect()))
        }
        None => (false, None),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use t3_contracts::base_schemas::TrimmedNonEmptyString as Text;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_discards_output_already_queued_by_the_reader() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let mut options = ManagerOptions::new(temp.path().join("logs"));
        options.preferred_shell = Some("C:\\Windows\\System32\\cmd.exe".into());
        let manager = TerminalManager::new(options).expect("manager");
        let key = ("t".to_owned(), "term-1".to_owned());
        manager
            .open(TerminalOpenInput {
                thread_id: Text(key.0.clone()),
                terminal_id: Text(key.1.clone()),
                cwd: Text(temp.path().to_string_lossy().into_owned()),
                worktree_path: None,
                cols: None,
                rows: None,
                env: None,
                provider_instance_id: None,
                stop_on_settle: None,
            })
            .await
            .expect("open real PTY");
        let (generation, epoch) = {
            let cell = manager.inner.cell(&key);
            let state = lock(&cell);
            let session = state.sessions.get(&key).expect("session");
            (
                session.generation,
                session.process.as_ref().expect("PTY").output_epoch(),
            )
        };
        let input = TerminalSessionInput {
            thread_id: Text(key.0.clone()),
            terminal_id: Text(key.1.clone()),
        };
        manager.clear(input.clone()).await.expect("clear");
        manager.inner.process_event(
            key.clone(),
            generation,
            ProcessEvent::Output {
                data: "old-reader-event".into(),
                epoch,
            },
        );
        assert!(
            !manager
                .snapshot(input.clone())
                .await
                .expect("snapshot")
                .history
                .contains("old-reader-event")
        );
        manager.inner.process_event(
            key,
            generation,
            ProcessEvent::Output {
                data: "new-reader-event".into(),
                epoch: epoch + 1,
            },
        );
        assert!(
            manager
                .snapshot(input)
                .await
                .expect("snapshot")
                .history
                .contains("new-reader-event")
        );
        manager.shutdown().await.expect("shutdown");
    }
}

#[cfg(all(test, windows))]
mod regression_tests {
    use super::*;
    use t3_contracts::base_schemas::TrimmedNonEmptyString as Text;
    async fn fixture(limit: usize) -> (tempfile::TempDir, TerminalManager, Key, u64, u64) {
        let temp = tempfile::tempdir().expect("temp");
        let mut options = ManagerOptions::new(temp.path().join("logs"));
        options.preferred_shell = Some("C:\\Windows\\System32\\cmd.exe".into());
        options.history_byte_limit = limit;
        options.persist_debounce = Duration::from_secs(3600);
        let manager = TerminalManager::new(options).expect("manager");
        let key = ("a".to_owned(), "term-1".to_owned());
        manager
            .open(TerminalOpenInput {
                thread_id: Text(key.0.clone()),
                terminal_id: Text(key.1.clone()),
                cwd: Text(temp.path().to_string_lossy().into_owned()),
                worktree_path: None,
                cols: None,
                rows: None,
                env: None,
                provider_instance_id: None,
                stop_on_settle: None,
            })
            .await
            .expect("open");
        let (generation, epoch) = {
            let cell = manager.inner.cell(&key);
            let state = lock(&cell);
            let s = state.sessions.get(&key).expect("session");
            (
                s.generation,
                s.process.as_ref().expect("process").output_epoch(),
            )
        };
        manager
            .clear(TerminalSessionInput {
                thread_id: Text(key.0.clone()),
                terminal_id: Text(key.1.clone()),
            })
            .await
            .expect("clear");
        (temp, manager, key, generation, epoch + 1)
    }
    #[tokio::test]
    async fn reader_error_stops_running_session() {
        let (_temp, manager, key, generation, _) = fixture(1024).await;
        manager.inner.process_event(
            key.clone(),
            generation,
            ProcessEvent::Error("read failed".into()),
        );
        assert_eq!(
            manager
                .snapshot(TerminalSessionInput {
                    thread_id: Text(key.0),
                    terminal_id: Text(key.1)
                })
                .await
                .expect("snapshot")
                .status,
            TerminalSessionStatus::Error
        );
        manager.shutdown().await.expect("shutdown");
    }
    #[tokio::test]
    async fn failed_overflow_rewrite_invalidates_append_cursor() {
        let (_temp, manager, key, generation, epoch) = fixture(16).await;
        let path = manager.inner.store.path(&key.0, &key.1);
        std::fs::remove_file(&path).expect("remove");
        std::fs::create_dir(&path).expect("block persist");
        manager.inner.process_event(
            key.clone(),
            generation,
            ProcessEvent::Output {
                data: "012345678901234567890123456789".into(),
                epoch,
            },
        );
        std::fs::remove_dir(&path).expect("unblock");
        std::fs::write(&path, "previous").expect("stale history");
        manager.inner.process_event(
            key.clone(),
            generation,
            ProcessEvent::Output {
                data: "".into(),
                epoch,
            },
        );
        manager.flush().await.expect("flush");
        let expected = manager
            .snapshot(TerminalSessionInput {
                thread_id: Text(key.0.clone()),
                terminal_id: Text(key.1.clone()),
            })
            .await
            .expect("snapshot")
            .history;
        assert_eq!(std::fs::read_to_string(path).expect("persisted"), expected);
        manager.shutdown().await.expect("shutdown");
    }
    #[tokio::test]
    async fn flush_continues_after_another_session_fails() {
        let (temp, manager, key, generation, epoch) = fixture(1024).await;
        manager
            .open(TerminalOpenInput {
                thread_id: Text("b".into()),
                terminal_id: Text("term-1".into()),
                cwd: Text(temp.path().to_string_lossy().into_owned()),
                worktree_path: None,
                cols: None,
                rows: None,
                env: None,
                provider_instance_id: None,
                stop_on_settle: None,
            })
            .await
            .expect("second");
        manager.inner.process_event(
            key.clone(),
            generation,
            ProcessEvent::Output {
                data: "first".into(),
                epoch,
            },
        );
        let other = ("b".to_owned(), "term-1".to_owned());
        let (generation, epoch) = {
            let cell = manager.inner.cell(&other);
            let state = lock(&cell);
            let s = state.sessions.get(&other).expect("session");
            (
                s.generation,
                s.process.as_ref().expect("process").output_epoch(),
            )
        };
        manager.inner.process_event(
            other.clone(),
            generation,
            ProcessEvent::Output {
                data: "second-marker".into(),
                epoch,
            },
        );
        let path = manager.inner.store.path(&key.0, &key.1);
        std::fs::remove_file(&path).expect("remove");
        std::fs::create_dir(&path).expect("block");
        assert!(manager.flush().await.is_err());
        assert!(
            std::fs::read_to_string(manager.inner.store.path(&other.0, &other.1))
                .expect("other session flushed")
                .contains("second-marker")
        );
        std::fs::remove_dir(path).expect("unblock");
        manager.shutdown().await.expect("shutdown");
    }
}
