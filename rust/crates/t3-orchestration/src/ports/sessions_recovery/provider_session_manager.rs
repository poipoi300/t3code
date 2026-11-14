use crate::{
    api::{
        Clock, CommandOutputSink, CommitGuard, CommitRequest, Decision, EventCommitter,
        EventIngestor, IdGenerator, IdKind, ProjectionReader, ProviderRegistry, RecordFilter,
        RecordKind, SessionDirectory,
    },
    error::{Error, Result},
    provider::*,
};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};
use t3_contracts::orchestration_v2::*;
use t3_contracts::{base_schemas::*, provider_instance::ProviderInstanceId};
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, mpsc, oneshot, watch},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

struct SessionEntry {
    admission: Arc<AsyncMutex<()>>,
    changed: Arc<Notify>,
    runtime: Arc<dyn SessionRuntime>,
    exposed: Arc<dyn SessionRuntime>,
    cancellation: CancellationToken,
    ingestion: Option<JoinHandle<Result<()>>>,
    native_key: Option<(String, String)>,
    instance_id: ProviderInstanceId,
    threads: BTreeSet<ThreadId>,
    credentials: BTreeMap<ThreadId, PreparedSessionCredential>,
    busy: BTreeMap<(ProviderThreadId, u64), ThreadId>,
    loaded: BTreeMap<ThreadId, LoadedThread>,
    idle_generation: u64,
    idle_deadline: Instant,
    pinned_since: Option<Instant>,
    subscribers: Vec<(
        mpsc::Sender<ProviderResult<ProviderEvent>>,
        CancellationToken,
    )>,
    retries: BTreeMap<ProviderTurnId, RetryWatch>,
}

struct RetryWatch {
    item: serde_json::Value,
    deadline: Option<Instant>,
    window: Duration,
    active: bool,
    stalled: bool,
}

struct LoadedThread {
    thread: OrchestrationV2ProviderThread,
    key: String,
    deadline: Option<Instant>,
}

#[derive(PartialEq)]
enum ChildArchive {
    None,
    AwaitingDelivery,
    Ready,
}

#[derive(Clone)]
pub struct SessionManagerOptions {
    pub idle_timeout: Duration,
    pub max_idle_pin: Duration,
    pub interrupt_timeout: Duration,
    pub unload_timeout: Duration,
    pub close_timeout: Duration,
    pub retry_stall_timeout: Duration,
}
impl Default for SessionManagerOptions {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30 * 60),
            max_idle_pin: Duration::from_secs(4 * 60 * 60),
            interrupt_timeout: Duration::from_secs(15),
            unload_timeout: Duration::from_secs(10),
            close_timeout: Duration::from_secs(30),
            retry_stall_timeout: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    IdleTimeout,
    RuntimeError,
    ManualShutdown,
    ServerShutdown,
}

pub struct PreparedCredential {
    pub id: String,
    pub issued: bool,
}

pub type PublishedCredential = Box<dyn Send + Sync>;

pub trait SessionCredentialReservation: Send + Sync {
    /// Owner is an opaque unique lease key; the returned guard releases only that owner on Drop.
    fn publish(self: Box<Self>, owner: ProviderSessionId) -> Result<PublishedCredential>;
}

pub struct PreparedSessionCredential {
    pub id: String,
    pub issued: bool,
    managed: bool,
    reservation: Option<Box<dyn SessionCredentialReservation>>,
    ownership: Option<PublishedCredential>,
}

impl PreparedSessionCredential {
    pub fn with_reservation(
        credential: PreparedCredential,
        reservation: Box<dyn SessionCredentialReservation>,
    ) -> Self {
        Self {
            id: credential.id,
            issued: credential.issued,
            managed: true,
            reservation: Some(reservation),
            ownership: None,
        }
    }

    pub fn unmanaged(credential: PreparedCredential) -> Self {
        Self {
            id: credential.id,
            issued: credential.issued,
            managed: false,
            reservation: None,
            ownership: None,
        }
    }

    fn publish(&mut self) -> Result<()> {
        if let Some(reservation) = self.reservation.take() {
            static NEXT_OWNER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let owner = NEXT_OWNER
                .try_update(
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                    |value| value.checked_add(1),
                )
                .map_err(|_| Error::Service {
                    operation: "SessionCredentials.publish",
                    detail: "credential owner ID space exhausted".into(),
                })?;
            self.ownership =
                Some(reservation.publish(ProviderSessionId(format!("credential-owner:{owner}")))?);
        }
        Ok(())
    }
}

#[async_trait]
pub trait SessionCredentials: Send + Sync {
    async fn prepare(
        &self,
        thread: ThreadId,
        instance: ProviderInstanceId,
    ) -> Result<PreparedCredential> {
        let _ = (thread, instance);
        Err(Error::Unsupported("SessionCredentials.prepare"))
    }
    /// The returned reservation must already protect validation and release races inside this await.
    async fn prepare_reserved(
        &self,
        thread: ThreadId,
        instance: ProviderInstanceId,
    ) -> Result<PreparedSessionCredential> {
        self.prepare(thread, instance)
            .await
            .map(PreparedSessionCredential::unmanaged)
    }
    async fn revoke(&self, thread: ThreadId, credential_id: String) -> Result<()> {
        let _ = (thread, credential_id);
        Err(Error::Unsupported("SessionCredentials.revoke"))
    }
}

pub struct SessionPersistence {
    pub reader: Arc<dyn ProjectionReader>,
    pub sink: Arc<dyn EventCommitter>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

#[derive(Default)]
pub struct SessionServices {
    pub persistence: Option<SessionPersistence>,
    pub credentials: Option<Arc<dyn SessionCredentials>>,
}

#[derive(Default)]
struct State {
    closed: bool,
    sessions: BTreeMap<ProviderSessionId, SessionEntry>,
    lanes: BTreeMap<ProviderSessionId, Weak<AsyncMutex<()>>>,
    writers: BTreeMap<(String, String), ProviderSessionId>,
    shutdown_result: Option<watch::Receiver<Option<Option<String>>>>,
    shutdown_task: Option<JoinHandle<()>>,
    suggestions: BTreeMap<ThreadId, watch::Sender<OrchestrationV2PromptSuggestion>>,
    suggestion_order: VecDeque<ThreadId>,
    thread_lanes: BTreeMap<(ProviderSessionId, ThreadId), Weak<AsyncMutex<()>>>,
    credential_reservations: BTreeMap<(ThreadId, String), usize>,
}

struct Inner {
    registry: Arc<dyn ProviderRegistry>,
    ingestor: Option<Arc<dyn EventIngestor>>,
    state: Mutex<State>,
    cancellation: CancellationToken,
    operations: TaskTracker,
    maintenance: TaskTracker,
    options: SessionManagerOptions,
    services: SessionServices,
    command_output: OnceLock<Arc<dyn CommandOutputSink>>,
}

#[derive(Clone)]
pub struct ProviderSessionManager {
    inner: Arc<Inner>,
}

struct CancelOnDrop(Option<CancellationToken>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(token) = &self.0 {
            token.cancel();
        }
    }
}

pub fn delegated_task_will_wake(projection: &OrchestrationV2ThreadProjection) -> bool {
    projection.thread.archived_at.is_none()
        && projection.thread.deleted_at.is_none()
        && projection.subagents.iter().any(|task| {
            task.origin == SubagentOrigin::AppOwned
                && matches!(
                    task.status,
                    OrchestrationV2TurnItemStatus::Pending
                        | OrchestrationV2TurnItemStatus::Running
                        | OrchestrationV2TurnItemStatus::Waiting
                )
                && projection.runs.iter().any(|run| {
                    task.run_id.as_ref() == Some(&run.id)
                        && run
                            .delegated_completion
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_none_or(|cohort| {
                                cohort.disposition == DelegatedCompletionCohortDisposition::Open
                            })
                })
                && task
                    .completion_delivery
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none_or(|delivery| {
                        !matches!(
                            delivery.state,
                            OrchestrationV2DelegatedCompletionTaskDeliveryState::Acknowledged
                                | OrchestrationV2DelegatedCompletionTaskDeliveryState::Delivered
                                | OrchestrationV2DelegatedCompletionTaskDeliveryState::Disposed
                        )
                    })
        })
}

impl ProviderSessionManager {
    pub fn new(
        registry: Arc<dyn ProviderRegistry>,
        ingestor: Option<Arc<dyn EventIngestor>>,
    ) -> Self {
        Self::with_options(registry, ingestor, SessionManagerOptions::default(), None)
    }

    pub fn with_options(
        registry: Arc<dyn ProviderRegistry>,
        ingestor: Option<Arc<dyn EventIngestor>>,
        mut options: SessionManagerOptions,
        services: Option<SessionServices>,
    ) -> Self {
        options.idle_timeout = options.idle_timeout.max(Duration::from_millis(1));
        Self {
            inner: Arc::new(Inner {
                registry,
                ingestor,
                state: Mutex::new(State::default()),
                cancellation: CancellationToken::new(),
                operations: TaskTracker::new(),
                maintenance: TaskTracker::new(),
                options,
                services: services.unwrap_or_default(),
                command_output: OnceLock::new(),
            }),
        }
    }

    /// Where live command chunks go (TS `startEventPump`); set once, before sessions open.
    pub fn set_command_output(&self, sink: Arc<dyn CommandOutputSink>) -> Result<()> {
        self.inner
            .command_output
            .set(sink)
            .map_err(|_| Error::Service {
                operation: "ProviderSessionManager.set_command_output",
                detail: "Command output is already set.".into(),
            })
    }

    async fn release_live_output(&self, settled: Option<(ThreadId, TurnItemId)>) -> Result<()> {
        match (settled, self.inner.command_output.get()) {
            (Some((thread_id, item_id)), Some(sink)) => sink.finish(thread_id, item_id).await,
            _ => Ok(()),
        }
    }

    pub async fn attach_with_policy(
        &self,
        instance_id: ProviderInstanceId,
        input: OpenSession,
        options: RuntimePolicyOptions,
    ) -> Result<Arc<dyn SessionRuntime>> {
        self.attach_configured(instance_id, input, Some(options))
            .await
    }

    async fn attach_configured(
        &self,
        instance_id: ProviderInstanceId,
        input: OpenSession,
        options: Option<RuntimePolicyOptions>,
    ) -> Result<Arc<dyn SessionRuntime>> {
        let (send, receive) = oneshot::channel();
        let cancellation = self.inner.cancellation.child_token();
        let mut cancel_on_drop = CancelOnDrop(Some(cancellation.clone()));
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(Error::Cancelled);
            }
            let lane = Self::lane(&mut state, &input.provider_session_id);
            let manager = self.clone();
            self.inner.operations.spawn(async move {
                // Serialize opening and awaited native cleanup for this session ID.
                let _lane = lane.lock().await;
                let id = input.provider_session_id.clone();
                let created = !manager.is_live(&id);
                let result = manager
                    .open(instance_id, input, options, cancellation)
                    .await;
                if let Err(Ok(_)) = send.send(result)
                    && created
                {
                    let _ = manager.close(&id).await;
                }
            });
        }
        let result = receive.await.map_err(|e| Error::Service {
            operation: "ProviderSessionManager.attach",
            detail: e.to_string(),
        })?;
        if result.is_ok() {
            cancel_on_drop.0 = None;
        }
        result
    }
    fn lane(state: &mut State, session: &ProviderSessionId) -> Arc<AsyncMutex<()>> {
        state.lanes.retain(|_, lane| lane.strong_count() > 0);
        if let Some(lane) = state.lanes.get(session).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(AsyncMutex::new(()));
        state.lanes.insert(session.clone(), Arc::downgrade(&lane));
        lane
    }

    pub fn is_live(&self, session: &ProviderSessionId) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .contains_key(session)
    }

    async fn open(
        &self,
        instance: ProviderInstanceId,
        input: OpenSession,
        options: Option<RuntimePolicyOptions>,
        cancellation: CancellationToken,
    ) -> Result<Arc<dyn SessionRuntime>> {
        let session_id = input.provider_session_id.clone();
        if let Some(cwd) = &input.runtime_policy.cwd {
            match std::fs::metadata(cwd) {
                Ok(metadata) if metadata.is_dir() => {}
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {}
                _ => {
                    return Err(Error::Service {
                        operation: "ProviderSessionManager.open",
                        detail: format!("Workspace directory is missing: {}", cwd.display()),
                    });
                }
            }
        }
        {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(Error::Cancelled);
            }
            if let Some(entry) = state.sessions.get(&session_id) {
                if entry.cancellation.is_cancelled() {
                    return Err(ProviderError::SessionStopped.into());
                }
                if entry.instance_id != instance
                    || (!entry.threads.contains(&input.thread_id)
                        && !entry
                            .runtime
                            .session()
                            .capabilities
                            .sessions
                            .supports_multiple_provider_threads_per_session)
                {
                    return Err(Error::Service {
                        operation: "ProviderSessionManager.open",
                        detail: "session ID belongs to another thread or provider instance".into(),
                    });
                }
            }
        }
        if let Some(runtime) = self.attach_existing(&instance, &input).await? {
            return Ok(runtime);
        }
        if input.model_selection.instance_id != instance {
            return Err(Error::Service {
                operation: "ProviderSessionManager.open",
                detail: "model selection belongs to another provider instance".into(),
            });
        }
        let adapter = self.inner.registry.adapter(instance.clone()).await?;
        let native_key = input
            .initial_native_thread_id
            .as_ref()
            .map(|id| (adapter.driver().0, id.clone()));
        if let Some(key) = &native_key {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.writers.contains_key(key) {
                return Err(ProviderError::ThreadInUse(ProviderThreadId(key.1.clone())).into());
            }
            state.writers.insert(key.clone(), session_id.clone());
        }
        let mut credential = match self
            .prepare_credential(input.thread_id.clone(), instance.clone())
            .await
        {
            Ok(credential) => credential,
            Err(error) => {
                self.release_writer(native_key.as_ref());
                return Err(error);
            }
        };
        let mut reservation = self.reserve_credential(input.thread_id.clone(), credential.as_ref());
        let opened = match options {
            Some(options) => {
                adapter
                    .open_session_with_policy(input.clone(), options, cancellation.clone())
                    .await
            }
            None => {
                adapter
                    .open_session(input.clone(), cancellation.clone())
                    .await
            }
        };
        let runtime = match opened {
            Ok(runtime) => runtime,
            Err(error) => {
                self.release_writer(native_key.as_ref());
                reservation.take();
                self.discard_prepared(input.thread_id.clone(), credential)
                    .await?;
                return Err(error.into());
            }
        };
        if cancellation.is_cancelled() {
            let result = self.close_native(&runtime).await;
            if let Err(error) = result {
                self.retain_failed_open(
                    &input,
                    instance,
                    runtime,
                    cancellation,
                    native_key,
                    credential,
                );
                return Err(error.into());
            }
            self.release_writer(native_key.as_ref());
            reservation.take();
            self.discard_prepared(input.thread_id.clone(), credential)
                .await?;
            return Err(Error::Cancelled);
        }
        let subscription = {
            match runtime.subscribe().await {
                Ok(subscription) => Some(subscription),
                Err(error) => {
                    cancellation.cancel();
                    let closed = self.close_native(&runtime).await;
                    if let Err(error) = closed {
                        self.retain_failed_open(
                            &input,
                            instance,
                            runtime,
                            cancellation,
                            native_key,
                            credential,
                        );
                        return Err(error.into());
                    }
                    self.release_writer(native_key.as_ref());
                    reservation.take();
                    self.discard_prepared(input.thread_id.clone(), credential)
                        .await?;
                    return Err(error.into());
                }
            }
        };
        if let Some(prepared) = &mut credential
            && let Err(error) = prepared.publish()
        {
            cancellation.cancel();
            if self.close_native(&runtime).await.is_err() {
                self.retain_failed_open(
                    &input,
                    instance,
                    runtime,
                    cancellation,
                    native_key,
                    credential,
                );
            } else {
                self.release_writer(native_key.as_ref());
                reservation.take();
                self.discard_prepared(input.thread_id.clone(), credential)
                    .await?;
            }
            return Err(error);
        }
        let exposed: Arc<dyn SessionRuntime> = Arc::new(ManagedRuntime {
            manager: Arc::downgrade(&self.inner),
            runtime: runtime.clone(),
            id: session_id.clone(),
        });
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .insert(
                session_id.clone(),
                SessionEntry {
                    admission: Arc::new(AsyncMutex::new(())),
                    changed: Arc::new(Notify::new()),
                    runtime: runtime.clone(),
                    exposed: exposed.clone(),
                    cancellation: cancellation.clone(),
                    ingestion: None,
                    native_key: native_key.clone(),
                    instance_id: instance.clone(),
                    threads: BTreeSet::from([input.thread_id.clone()]),
                    credentials: credential
                        .map(|credential| BTreeMap::from([(input.thread_id.clone(), credential)]))
                        .unwrap_or_default(),
                    busy: BTreeMap::new(),
                    loaded: BTreeMap::new(),
                    idle_generation: 0,
                    idle_deadline: Instant::now() + self.inner.options.idle_timeout,
                    pinned_since: None,
                    subscribers: Vec::new(),
                    retries: BTreeMap::new(),
                },
            );
        reservation.take();
        if let Err(error) = self
            .write_session(
                std::slice::from_ref(&input.thread_id),
                runtime.session(),
                "provider-session.attached",
            )
            .await
        {
            let _ = self
                .close_reason(
                    &session_id,
                    ReleaseReason::RuntimeError,
                    Some("Failed to persist the provider-session attachment.".into()),
                )
                .await;
            return Err(error);
        }
        let ingestion = subscription.map(|mut subscription| {
            let cancellation = cancellation.clone();
            let session_id = session_id.clone();
            let instance = instance.clone();
            let manager = self.clone();
            let runtime = runtime.clone();
            tokio::spawn(async move {
                let result: Result<()> = async {
                    let mut stopped_by_provider = false;
                    loop {
                        let event = tokio::select! {
                            biased;
                            _ = cancellation.cancelled() => { subscription.close(); return Ok(()); },
                            event = subscription.events.recv() => event,
                        };
                        let Some(event) = event else {
                            return if stopped_by_provider || cancellation.is_cancelled() { Ok(()) } else {
                                Err(ProviderError::Operation { operation: ProviderOperation::Stream, detail: "Provider event stream ended unexpectedly.".into() }.into())
                            };
                        };
                        let mut event = event?;
                        // Live output bypasses persistence and subscribers so floods cannot delay domain events.
                        if let ProviderEventKind::CommandOutput { thread_id, item_id, chunk } = event.kind {
                            if let Some(sink) = manager.inner.command_output.get() { sink.append(thread_id, item_id, chunk).await?; }
                            continue;
                        }
                        let settled_output = settled_command(&event)?;
                        if matches!(&event.kind, ProviderEventKind::Session(session) if session.status == ProviderSessionStatus::Stopped) { stopped_by_provider = true; }
                        manager.observe(&session_id, &runtime, &mut event).await?;
                        if !matches!(event.kind, ProviderEventKind::PromptSuggestion { .. }) {
                            if let Some(ingestor) = &manager.inner.ingestor
                                && let Some(target) = session_artifact_thread(&event)? {
                                let requires_attachment = matches!(&event.kind, ProviderEventKind::Thread { .. })
                                    || matches!(&event.kind, ProviderEventKind::TurnItem { item, .. } if matches!(item.as_ref(), OrchestrationV2TurnItem::CommandExecution(_)));
                                let current = {
                                    let state = manager.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.sessions.get(&session_id).filter(|entry| Arc::ptr_eq(&entry.runtime, &runtime)).is_some_and(|entry| !requires_attachment || entry.threads.contains(&target))
                                };
                                if current {
                                    ingestor.ingest(session_id.clone(), instance.clone(), target, event).await?;
                                    manager.release_live_output(settled_output).await?;
                                }
                                continue;
                            }
                            manager.publish(&session_id, &runtime, event).await;
                            manager.release_live_output(settled_output).await?;
                        }
                    }
                }.await;
                if !cancellation.is_cancelled() {
                    let detail = result.as_ref().err().map(|error| crate::ports::runs_ingestion::provider_turn_start_service::provider_failure_message(error).chars().take(500).collect::<String>());
                    if let Some(detail) = &detail { manager.publish_failure(&session_id, &runtime, detail).await; }
                    manager.release_stopped_stream(session_id, runtime, detail);
                }
                result
            })
        });
        if let Some(entry) = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .get_mut(&session_id)
        {
            entry.ingestion = ingestion;
        }
        self.start_maintenance(session_id.clone(), runtime);
        Ok(exposed)
    }

    fn retain_failed_open(
        &self,
        input: &OpenSession,
        instance_id: ProviderInstanceId,
        runtime: Arc<dyn SessionRuntime>,
        cancellation: CancellationToken,
        native_key: Option<(String, String)>,
        credential: Option<PreparedSessionCredential>,
    ) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .insert(
                input.provider_session_id.clone(),
                SessionEntry {
                    admission: Arc::new(AsyncMutex::new(())),
                    changed: Arc::new(Notify::new()),
                    exposed: Arc::new(ManagedRuntime {
                        manager: Arc::downgrade(&self.inner),
                        runtime: runtime.clone(),
                        id: input.provider_session_id.clone(),
                    }),
                    runtime,
                    cancellation,
                    ingestion: None,
                    native_key,
                    instance_id,
                    threads: BTreeSet::from([input.thread_id.clone()]),
                    credentials: credential
                        .map(|credential| BTreeMap::from([(input.thread_id.clone(), credential)]))
                        .unwrap_or_default(),
                    busy: BTreeMap::new(),
                    loaded: BTreeMap::new(),
                    idle_generation: 0,
                    idle_deadline: Instant::now() + self.inner.options.idle_timeout,
                    pinned_since: None,
                    subscribers: Vec::new(),
                    retries: BTreeMap::new(),
                },
            );
    }

    fn release_stopped_stream(
        &self,
        session_id: ProviderSessionId,
        runtime: Arc<dyn SessionRuntime>,
        detail: Option<String>,
    ) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return;
        }
        let lane = Self::lane(&mut state, &session_id);
        let manager = self.clone();
        self.inner.operations.spawn(async move {
            let _lane = lane.lock().await;
            let current = manager
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sessions
                .get(&session_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.runtime, &runtime));
            if current {
                let reason = if detail.is_some() {
                    ReleaseReason::RuntimeError
                } else {
                    ReleaseReason::ManualShutdown
                };
                let _ = manager.close_reason(&session_id, reason, detail).await;
            }
        });
    }

    fn release_writer(&self, key: Option<&(String, String)>) {
        if let Some(key) = key {
            self.inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .writers
                .remove(key);
        }
    }

    async fn close(&self, session: &ProviderSessionId) -> Result<()> {
        self.close_reason(session, ReleaseReason::ManualShutdown, None)
            .await
    }

    async fn close_reason(
        &self,
        session: &ProviderSessionId,
        reason: ReleaseReason,
        detail: Option<String>,
    ) -> Result<()> {
        self.close_reason_with_detach(session, reason, detail, None)
            .await
    }

    async fn close_reason_with_detach(
        &self,
        session: &ProviderSessionId,
        reason: ReleaseReason,
        detail: Option<String>,
        detached_thread: Option<ThreadId>,
    ) -> Result<()> {
        let entry = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .remove(session);
        let Some(mut entry) = entry else {
            return Ok(());
        };
        entry.changed.notify_one();
        let released_at = self
            .inner
            .services
            .persistence
            .as_ref()
            .map(|p| p.clock.now());
        let roster = self.release_roster(&entry, reason).await;
        if reason == ReleaseReason::ServerShutdown {
            for (_, cancellation) in &entry.subscribers {
                cancellation.cancel();
            }
        }
        entry.cancellation.cancel();
        let closed = self.close_native(&entry.runtime).await;
        let ingested = if let Some(ingestion) = entry.ingestion.take() {
            ingestion
                .await
                .map_err(|e| Error::Service {
                    operation: "ProviderSessionManager.ingestion",
                    detail: e.to_string(),
                })
                .and_then(|result| result)
        } else {
            Ok(())
        };
        if let Some(thread) = detached_thread {
            for (sender, cancellation) in std::mem::take(&mut entry.subscribers) {
                // MARKED SHIM: frozen ProviderError lacks EventStream's detached_thread_id.
                let error = ProviderError::Protocol {
                    reason: None,
                    detail: detail
                        .clone()
                        .unwrap_or_else(|| "Provider session released: manual_shutdown.".into()),
                    payload: Some(serde_json::json!({
                        "_tag": "ProviderAdapterEventStreamError",
                        "driver": entry.runtime.session().driver,
                        "providerSessionId": session,
                        "detachedThreadId": thread,
                    })),
                };
                tokio::select! {
                    _ = cancellation.cancelled() => {},
                    _ = self.inner.cancellation.cancelled() => {},
                    _ = sender.send(Err(error)) => {},
                }
            }
        }
        if let Err(error) = closed {
            self.inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sessions
                .insert(session.clone(), entry);
            return Err(error.into());
        }
        self.release_writer(entry.native_key.as_ref());
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .writers
            .retain(|_, owner| owner != session);
        let records = Arc::new(ReleaseRecords {
            session: session.clone(),
            runtime: entry.runtime.clone(),
            threads: entry.threads.clone(),
            reason,
            detail,
            released_at,
            roster,
        });
        let recorded = self.write_release_records(&records).await;
        if recorded.is_err() {
            let manager = self.clone();
            let records = records.clone();
            self.inner.maintenance.spawn(async move {
                let mut delay = Duration::from_secs(1);
                loop {
                    tokio::select! { _ = manager.inner.cancellation.cancelled() => break, _ = tokio::time::sleep(delay) => {} }
                    if manager.write_release_records(&records).await.is_ok() { break; }
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            });
        }
        let mut failure = recorded.err().or(ingested.err());
        for (thread, credential) in entry.credentials {
            if let Err(error) = self.dispose_credential(thread, credential, true).await {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn close_native(&self, runtime: &Arc<dyn SessionRuntime>) -> ProviderResult<()> {
        tokio::time::timeout(self.inner.options.close_timeout, runtime.close())
            .await
            .map_err(|_| ProviderError::Operation {
                operation: ProviderOperation::Close,
                detail: format!(
                    "Provider session cleanup exceeded {} ms.",
                    self.inner.options.close_timeout.as_millis()
                ),
            })?
    }

    async fn prepare_credential(
        &self,
        thread: ThreadId,
        instance: ProviderInstanceId,
    ) -> Result<Option<PreparedSessionCredential>> {
        match &self.inner.services.credentials {
            Some(service) => tokio::select! {
                biased;
                _ = self.inner.cancellation.cancelled() => Err(Error::Cancelled),
                credential = service.prepare_reserved(thread, instance) => credential.map(Some),
            },
            None => Ok(None),
        }
    }

    fn reserve_credential(
        &self,
        thread: ThreadId,
        credential: Option<&PreparedSessionCredential>,
    ) -> Option<CredentialReservation> {
        credential.map(|credential| {
            let key = (thread, credential.id.clone());
            *self
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .credential_reservations
                .entry(key.clone())
                .or_default() += 1;
            CredentialReservation {
                manager: Arc::downgrade(&self.inner),
                key,
            }
        })
    }

    fn thread_lane(&self, session: &ProviderSessionId, thread: &ThreadId) -> Arc<AsyncMutex<()>> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.thread_lanes.retain(|_, lane| lane.strong_count() > 0);
        let key = (session.clone(), thread.clone());
        if let Some(lane) = state.thread_lanes.get(&key).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(AsyncMutex::new(()));
        state.thread_lanes.insert(key, Arc::downgrade(&lane));
        lane
    }

    fn admission(&self, session: &ProviderSessionId) -> ProviderResult<Arc<AsyncMutex<()>>> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .get(session)
            .map(|entry| entry.admission.clone())
            .ok_or(ProviderError::SessionStopped)
    }

    async fn revoke_unheld(&self, thread: ThreadId, credential: String) -> Result<()> {
        let held = {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .credential_reservations
                .contains_key(&(thread.clone(), credential.clone()))
                || state.sessions.values().any(|entry| {
                    entry
                        .credentials
                        .get(&thread)
                        .is_some_and(|held| held.id == credential)
                })
        };
        if !held && let Some(service) = &self.inner.services.credentials {
            service.revoke(thread, credential).await?;
        }
        Ok(())
    }

    async fn dispose_credential(
        &self,
        thread: ThreadId,
        credential: PreparedSessionCredential,
        revoke_legacy: bool,
    ) -> Result<()> {
        let revoke = !credential.managed && revoke_legacy;
        let id = credential.id.clone();
        drop(credential);
        if revoke {
            self.revoke_unheld(thread, id).await?;
        }
        Ok(())
    }

    async fn discard_prepared(
        &self,
        thread: ThreadId,
        credential: Option<PreparedSessionCredential>,
    ) -> Result<()> {
        if let Some(credential) = credential {
            let issued = credential.issued;
            self.dispose_credential(thread, credential, issued).await?;
        }
        Ok(())
    }

    async fn attach_existing(
        &self,
        instance: &ProviderInstanceId,
        input: &OpenSession,
    ) -> Result<Option<Arc<dyn SessionRuntime>>> {
        let lane = self.thread_lane(&input.provider_session_id, &input.thread_id);
        let _lane = lane.lock().await;
        let existing = {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.sessions.get(&input.provider_session_id).map(|entry| {
                (
                    entry.runtime.clone(),
                    entry.exposed.clone(),
                    entry.threads.contains(&input.thread_id),
                )
            })
        };
        let Some((runtime, exposed, attached)) = existing else {
            return Ok(None);
        };
        if !attached {
            let mut credential = self
                .prepare_credential(input.thread_id.clone(), instance.clone())
                .await?;
            let reservation = self.reserve_credential(input.thread_id.clone(), credential.as_ref());
            if let Some(credential) = &mut credential {
                credential.publish()?;
            }
            if let Err(error) = self
                .write_session(
                    std::slice::from_ref(&input.thread_id),
                    runtime.session(),
                    "provider-session.attached",
                )
                .await
            {
                drop(reservation);
                self.discard_prepared(input.thread_id.clone(), credential)
                    .await?;
                return Err(error);
            }
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let entry = state
                .sessions
                .get_mut(&input.provider_session_id)
                .ok_or(ProviderError::SessionStopped)?;
            entry.threads.insert(input.thread_id.clone());
            if let Some(credential) = credential {
                entry
                    .credentials
                    .insert(input.thread_id.clone(), credential);
            }
        }
        self.touch(&input.provider_session_id, false);
        Ok(Some(exposed))
    }

    fn touch(&self, id: &ProviderSessionId, provider_activity: bool) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = state.sessions.get_mut(id) {
            entry.idle_generation = entry.idle_generation.wrapping_add(1);
            entry.idle_deadline = Instant::now() + self.inner.options.idle_timeout;
            if provider_activity {
                entry.pinned_since = None;
            }
            entry.changed.notify_one();
        }
    }

    fn mark_busy(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
        input: &TurnInput,
    ) -> ProviderResult<()> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state
            .sessions
            .get_mut(id)
            .filter(|entry| {
                Arc::ptr_eq(&entry.runtime, runtime) && !entry.cancellation.is_cancelled()
            })
            .ok_or(ProviderError::SessionStopped)?;
        entry.busy.insert(
            (expanded(&input.provider_thread.id)?, input.run_ordinal),
            input.thread_id.clone(),
        );
        entry.idle_generation = entry.idle_generation.wrapping_add(1);
        entry.pinned_since = None;
        if let Some(loaded) = entry.loaded.get_mut(&input.thread_id) {
            loaded.deadline = None;
        }
        entry.changed.notify_one();
        Ok(())
    }

    fn mark_idle(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
        thread: &ProviderThreadId,
        ordinal: u64,
    ) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state
            .sessions
            .get_mut(id)
            .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
        else {
            return;
        };
        entry.busy.remove(&(thread.clone(), ordinal));
        entry.idle_generation = entry.idle_generation.wrapping_add(1);
        entry.idle_deadline = Instant::now() + self.inner.options.idle_timeout;
        entry.changed.notify_one();
        let can_unload = runtime
            .session()
            .capabilities
            .sessions
            .supports_multiple_provider_threads_per_session
            && runtime
                .extensions()
                .is_some_and(|extension| extension.capabilities().unload);
        for loaded in entry
            .loaded
            .values_mut()
            .filter(|loaded| can_unload && loaded.thread.native_thread_ref.is_some())
        {
            if expanded(&loaded.thread.id).ok().as_ref() == Some(thread)
                && !entry.busy.keys().any(|(id, _)| id == thread)
            {
                loaded.deadline = Some(entry.idle_deadline);
            }
        }
    }

    pub async fn release_unless_busy(
        &self,
        session: ProviderSessionId,
        own_threads: &[ProviderThreadId],
        through_ordinal: u64,
    ) -> Result<bool> {
        self.release_checked(crate::api::ReleaseSession {
            provider_session_id: session,
            own_provider_thread_ids: own_threads.to_vec(),
            through_run_ordinal: through_ordinal,
            reason: "manual_shutdown".into(),
            detail: None,
            keep_if: None,
        })
        .await
    }

    async fn release_checked(&self, input: crate::api::ReleaseSession) -> Result<bool> {
        let session = input.provider_session_id;
        let reason = match input.reason.as_str() {
            "manual_shutdown" => ReleaseReason::ManualShutdown,
            "server_shutdown" => ReleaseReason::ServerShutdown,
            "runtime_error" => ReleaseReason::RuntimeError,
            "idle_timeout" => ReleaseReason::IdleTimeout,
            _ => {
                return Err(Error::Decode {
                    entity: "session release reason",
                    detail: input.reason,
                });
            }
        };
        let lane = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            Self::lane(&mut state, &session)
        };
        let _lane = lane.lock().await;
        let admission = match self.admission(&session) {
            Ok(admission) => admission,
            Err(ProviderError::SessionStopped) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        // Retention reads and turn admission share this gate; no SQLite guard crosses the await.
        let _admission = admission.lock().await;
        let keep = if let Some(check) = input.keep_if {
            check.keep().await?
        } else {
            false
        };
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state.sessions.get_mut(&session) else {
                return Ok(false);
            };
            entry.busy.retain(|(id, ordinal), _| {
                !input.own_provider_thread_ids.contains(id) || *ordinal > input.through_run_ordinal
            });
            if keep || !entry.busy.is_empty() {
                entry.idle_deadline = Instant::now() + self.inner.options.idle_timeout;
                return Ok(false);
            }
            // Removal and turn admission share the state lock.
            entry.cancellation.cancel();
        }
        self.close_reason(&session, reason, input.detail).await?;
        Ok(true)
    }

    pub async fn other_working_threads(
        &self,
        session: ProviderSessionId,
        thread: ThreadId,
    ) -> Result<Vec<ThreadId>> {
        let attached = {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state.sessions.get(&session) else {
                return Ok(Vec::new());
            };
            if !entry
                .runtime
                .session()
                .capabilities
                .sessions
                .supports_multiple_provider_threads_per_session
            {
                return Ok(Vec::new());
            }
            entry
                .threads
                .iter()
                .filter(|id| *id != &thread)
                .map(|id| (id.clone(), entry.busy.values().any(|owner| owner == id)))
                .collect::<Vec<_>>()
        };
        let mut working = Vec::new();
        for (id, busy) in attached {
            let Some(persistence) = &self.inner.services.persistence else {
                if busy {
                    working.push(id);
                }
                continue;
            };
            let running = persistence
                .reader
                .records(id.clone(), vec![RecordKind::Runs], RecordFilter::default())
                .await;
            let shell = persistence.reader.shell(id.clone()).await;
            let active = match (running, shell) {
                (Ok(records), Ok(shell)) => busy || records.records.iter().any(|set| matches!(set, crate::api::ProjectionRecordSet::Runs(runs) if runs.iter().any(|run| matches!(run.status, OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting | OrchestrationV2RunStatus::Running | OrchestrationV2RunStatus::Waiting)))) || shell.is_some_and(|s| s.pending_background_tasks.flatten().is_some_and(|tasks| !tasks.is_empty())),
                _ => true,
            };
            if active {
                working.push(id);
            }
        }
        Ok(working)
    }

    pub async fn detach_thread(
        &self,
        session: ProviderSessionId,
        thread: ThreadId,
        revoke: bool,
    ) -> Result<()> {
        self.detach_thread_with_detail(session, thread, revoke, None)
            .await
    }

    async fn detach_thread_with_detail(
        &self,
        session: ProviderSessionId,
        thread: ThreadId,
        revoke: bool,
        detail: Option<String>,
    ) -> Result<()> {
        let lane = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            Self::lane(&mut state, &session)
        };
        let _lane = lane.lock().await;
        let thread_lane = self.thread_lane(&session, &thread);
        let _thread_lane = thread_lane.lock().await;
        let mut provider_threads = Vec::new();
        if let Some(persistence) = &self.inner.services.persistence {
            let records = persistence
                .reader
                .records(
                    thread.clone(),
                    vec![RecordKind::ProviderThreads],
                    RecordFilter::default(),
                )
                .await?;
            for records in records.records {
                if let crate::api::ProjectionRecordSet::ProviderThreads(threads) = records {
                    provider_threads.extend(threads);
                }
            }
        }
        let detached = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state.sessions.get_mut(&session) else {
                return Ok(());
            };
            if !entry.threads.remove(&thread) {
                return Ok(());
            }
            entry.busy.retain(|_, owner| owner != &thread);
            let loaded = entry.loaded.remove(&thread);
            let credential = if revoke {
                entry.credentials.remove(&thread)
            } else {
                None
            };
            (
                entry.runtime.clone(),
                entry.threads.is_empty()
                    && !entry
                        .runtime
                        .session()
                        .capabilities
                        .sessions
                        .supports_multiple_provider_threads_per_session,
                loaded,
                credential,
            )
        };
        if let Some(credential) = detached.3 {
            self.dispose_credential(thread.clone(), credential, true)
                .await?;
        }
        if detached.1 {
            // Release cleanup includes the thread removed by this detach.
            if let Some(entry) = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sessions
                .get_mut(&session)
            {
                entry.threads.insert(thread.clone());
            }
            return self
                .close_reason_with_detach(
                    &session,
                    ReleaseReason::ManualShutdown,
                    detail,
                    Some(thread),
                )
                .await;
        }
        if let Some(loaded) = detached.2
            && !provider_threads
                .iter()
                .any(|provider| provider.id == loaded.thread.id)
        {
            provider_threads.push(loaded.thread);
        }
        if let Some(extension) = detached
            .0
            .extensions()
            .filter(|ext| ext.capabilities().unload)
        {
            for provider in provider_threads.into_iter().filter(|provider| {
                provider.native_thread_ref.is_some()
                    && provider.provider_session_id.as_ref() == Some(&detached.0.session().id)
            }) {
                let owner = provider
                    .app_thread_id
                    .as_ref()
                    .map(expanded)
                    .transpose()?
                    .unwrap_or_else(|| thread.clone());
                let owner_lane = self.thread_lane(&session, &owner);
                let _owner_lane = if owner == thread {
                    None
                } else {
                    Some(owner_lane.lock().await)
                };
                let attached = {
                    let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    state
                        .sessions
                        .get(&session)
                        .filter(|entry| Arc::ptr_eq(&entry.runtime, &detached.0))
                        .is_none_or(|entry| entry.threads.contains(&owner))
                };
                if attached {
                    continue;
                }
                let _ = tokio::time::timeout(
                    self.inner.options.unload_timeout,
                    extension.unload(provider, false),
                )
                .await;
            }
        }
        self.touch(&session, false);
        Ok(())
    }

    pub async fn prompt_suggestion(
        &self,
        thread: ThreadId,
    ) -> Result<OrchestrationV2PromptSuggestion> {
        let suggestion = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .suggestions
            .get(&thread)
            .and_then(|sender| sender.borrow().clone());
        if suggestion.is_some()
            && let Some(persistence) = &self.inner.services.persistence
            && !persistence
                .reader
                .thread(thread)
                .await?
                .is_some_and(|thread| thread.deleted_at.is_none())
        {
            return Ok(None);
        }
        Ok(suggestion)
    }

    pub async fn subscribe_prompt_suggestion(
        &self,
        thread: ThreadId,
    ) -> Result<watch::Receiver<OrchestrationV2PromptSuggestion>> {
        let Some(persistence) = &self.inner.services.persistence else {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            return Ok(state
                .suggestions
                .entry(thread)
                .or_insert_with(|| watch::channel(None).0)
                .subscribe());
        };
        let mut events = persistence.sink.subscribe();
        let mut raw = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(Error::Cancelled);
            }
            state
                .suggestions
                .entry(thread.clone())
                .or_insert_with(|| watch::channel(None).0)
                .subscribe()
        };
        let (send, receive) = watch::channel(self.prompt_suggestion(thread.clone()).await?);
        let manager = self.clone();
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(Error::Cancelled);
        }
        self.inner.maintenance.spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = manager.inner.cancellation.cancelled() => break,
                    _ = send.closed() => return,
                    changed = raw.changed() => { if changed.is_err() { break; } },
                    event = events.recv() => match event {
                        Ok(t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored)) => {
                            if !matches!(stored.event, OrchestrationV2DomainEvent::ThreadDeleted(event) if event.thread_id == thread) { continue; }
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => continue,
                    },
                }
                let next = manager.prompt_suggestion(thread.clone()).await.unwrap_or(None);
                send.send_if_modified(|current| { if *current == next { false } else { *current = next; true } });
            }
            send.send_replace(None);
        });
        Ok(receive)
    }

    async fn publish(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
        event: ProviderEvent,
    ) {
        let (subscribers, released) = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state
                .sessions
                .get_mut(id)
                .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
            else {
                return;
            };
            entry
                .subscribers
                .retain(|(send, token)| !send.is_closed() && !token.is_cancelled());
            (entry.subscribers.clone(), entry.cancellation.clone())
        };
        for (sender, cancellation) in subscribers {
            tokio::select! { _ = cancellation.cancelled() => {}, _ = released.cancelled() => {}, _ = self.inner.cancellation.cancelled() => {}, _ = sender.send(Ok(event.clone())) => {} }
        }
    }

    async fn publish_failure(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
        detail: &str,
    ) {
        let subscribers = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(entry) = state
                .sessions
                .get(id)
                .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
            else {
                return;
            };
            entry.subscribers.clone()
        };
        for (sender, cancellation) in subscribers {
            let error = ProviderError::Operation {
                operation: ProviderOperation::Stream,
                detail: detail.into(),
            };
            tokio::select! { _ = cancellation.cancelled() => {}, _ = self.inner.cancellation.cancelled() => {}, _ = sender.send(Err(error)) => {} }
        }
    }

    async fn observe(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
        event: &mut ProviderEvent,
    ) -> Result<()> {
        if let ProviderEventKind::PromptSuggestion {
            thread_id,
            run_id,
            text,
        } = &event.kind
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .suggestions
                .entry(thread_id.clone())
                .or_insert_with(|| watch::channel(None).0)
                .send_replace(Some(OrchestrationV2PromptSuggestionValue {
                    run_id: run_id.clone(),
                    text: text.clone(),
                }));
            state.suggestion_order.retain(|thread| thread != thread_id);
            state.suggestion_order.push_back(thread_id.clone());
            while state.suggestion_order.len() > 256 {
                if let Some(old) = state.suggestion_order.pop_front() {
                    state.suggestions.remove(&old);
                }
            }
            return Ok(());
        }
        self.observe_retry(id, runtime, event)?;
        match &event.kind {
            ProviderEventKind::Thread {
                provider_thread, ..
            } if provider_thread.status == ProviderThreadStatus::Error => {
                if let Some(thread) = provider_thread
                    .app_thread_id
                    .as_ref()
                    .and_then(|id| expanded(id).ok())
                {
                    let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(entry) = state
                        .sessions
                        .get_mut(id)
                        .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
                    {
                        entry.loaded.remove(&thread);
                    }
                }
                self.touch(id, true);
            }
            ProviderEventKind::Terminal {
                provider_thread_id,
                run_ordinal,
                ..
            } => self.mark_idle(id, runtime, provider_thread_id, *run_ordinal),
            ProviderEventKind::Session(session) => {
                let threads = {
                    let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    state
                        .sessions
                        .get(id)
                        .map(|entry| entry.threads.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                };
                self.write_session(&threads, session, "provider-session.updated")
                    .await?;
                if session.status != runtime.session().status {
                    self.touch(id, true);
                }
            }
            _ => self.touch(id, true),
        }
        Ok(())
    }

    fn start_maintenance(&self, id: ProviderSessionId, runtime: Arc<dyn SessionRuntime>) {
        let changed = {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state
                .sessions
                .get(&id)
                .filter(|entry| Arc::ptr_eq(&entry.runtime, &runtime))
            else {
                return;
            };
            entry.changed.clone()
        };
        let manager = self.clone();
        self.inner.maintenance.spawn(async move {
            loop {
                let notified = changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let deadline = {
                    let state = manager.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(entry) = state.sessions.get(&id).filter(|entry| Arc::ptr_eq(&entry.runtime, &runtime)) else { return };
                    entry.loaded.values().filter_map(|loaded| loaded.deadline).chain(entry.retries.values().filter_map(|retry| retry.deadline)).chain(entry.busy.is_empty().then_some(entry.idle_deadline)).min()
                };
                let deadline = deadline.unwrap_or_else(|| Instant::now() + manager.inner.options.idle_timeout);
                tokio::select! { _ = manager.inner.cancellation.cancelled() => return, _ = &mut notified => continue, _ = tokio::time::sleep_until(deadline) => {} }
                if let Err(error) = manager.expire(&id, &runtime).await { let _ = error; }
            }
        });
    }

    async fn expire(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
    ) -> Result<()> {
        self.expire_retries(id, runtime).await?;
        let candidate = {
            let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state
                .sessions
                .get(id)
                .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
            else {
                return Ok(());
            };
            (
                entry.idle_generation,
                entry.busy.is_empty() && Instant::now() >= entry.idle_deadline,
                entry
                    .loaded
                    .iter()
                    .filter(|(_, loaded)| {
                        loaded
                            .deadline
                            .is_some_and(|deadline| Instant::now() >= deadline)
                    })
                    .map(|(id, loaded)| (id.clone(), loaded.thread.clone(), loaded.key.clone()))
                    .collect::<Vec<_>>(),
            )
        };
        let mut awaiting_delivery = false;
        for (thread, provider_thread, key) in candidate.2 {
            let lane = self.thread_lane(id, &thread);
            let _lane = lane.lock().await;
            let Some(extension) = runtime.extensions().filter(|ext| ext.capabilities().unload)
            else {
                continue;
            };
            let pending = extension
                .pending_background_work(Some(provider_thread.clone()), false)
                .await
                .unwrap_or(false);
            let archive = self
                .delegated_child_archive(&thread)
                .await
                .unwrap_or(ChildArchive::None);
            awaiting_delivery |= archive == ChildArchive::AwaitingDelivery;
            let unload = {
                let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                let Some(entry) = state
                    .sessions
                    .get_mut(id)
                    .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
                else {
                    continue;
                };
                let busy = entry.busy.values().any(|owner| owner == &thread);
                if let Some(loaded) = entry.loaded.get_mut(&thread).filter(|loaded| {
                    loaded.key == key
                        && loaded
                            .deadline
                            .is_some_and(|deadline| Instant::now() >= deadline)
                }) {
                    if pending || busy || archive == ChildArchive::AwaitingDelivery {
                        loaded.deadline = Some(Instant::now() + self.inner.options.idle_timeout);
                        false
                    } else {
                        entry.loaded.remove(&thread);
                        true
                    }
                } else {
                    false
                }
            };
            if unload {
                let archived = archive == ChildArchive::Ready;
                let result = tokio::time::timeout(
                    self.inner.options.unload_timeout,
                    extension.unload(provider_thread.clone(), archived),
                )
                .await;
                if archived && !matches!(result, Ok(Ok(()))) {
                    let _ = tokio::time::timeout(
                        self.inner.options.unload_timeout,
                        extension.unload(provider_thread, false),
                    )
                    .await;
                }
            }
        }
        if !candidate.1 {
            return Ok(());
        }
        let pending = awaiting_delivery
            || if let Some(extension) = runtime.extensions() {
                extension
                    .pending_background_work(None, false)
                    .await
                    .unwrap_or(false)
            } else {
                false
            };
        let release = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state.sessions.get_mut(id).filter(|entry| {
                Arc::ptr_eq(&entry.runtime, runtime)
                    && entry.idle_generation == candidate.0
                    && entry.busy.is_empty()
            }) else {
                return Ok(());
            };
            let pinned = *entry.pinned_since.get_or_insert_with(Instant::now);
            if pending && Instant::now().duration_since(pinned) < self.inner.options.max_idle_pin {
                entry.idle_deadline = Instant::now() + self.inner.options.idle_timeout;
                false
            } else {
                entry.cancellation.cancel();
                true
            }
        };
        if release {
            self.close_reason(id, ReleaseReason::IdleTimeout, None)
                .await?;
        }
        Ok(())
    }

    async fn delegated_child_archive(&self, thread: &ThreadId) -> Result<ChildArchive> {
        let Some(persistence) = &self.inner.services.persistence else {
            return Ok(ChildArchive::None);
        };
        let records = persistence
            .reader
            .query_records(crate::api::ProjectionRecordQuery {
                thread_id: thread.clone(),
                fields: vec![RecordKind::Runs],
                filter: RecordFilter::default(),
                messages: Default::default(),
                run_statuses: Some(vec![]),
                include_latest_run: true,
            })
            .await?;
        let raw = t3_contracts::stored::to_value(&records.thread)?;
        let Some(parent) = raw["lineage"]["parentThreadId"].as_str() else {
            return Ok(ChildArchive::None);
        };
        let latest = records
            .records
            .into_iter()
            .find_map(|records| match records {
                crate::api::ProjectionRecordSet::Runs(runs) => {
                    runs.into_iter().max_by_key(|run| run.ordinal)
                }
                _ => None,
            })
            .map(|run| t3_contracts::stored::to_value(&run))
            .transpose()?;
        let task = latest
            .as_ref()
            .and_then(|run| run["delegatedTaskId"].as_str())
            .or_else(|| {
                (raw["forkedFrom"]["type"] == "node")
                    .then(|| raw["forkedFrom"]["nodeId"].as_str())
                    .flatten()
            });
        let Some(task) = task else {
            return Ok(ChildArchive::None);
        };
        let parent_records = persistence
            .reader
            .records(
                ThreadId(parent.into()),
                vec![RecordKind::Subagents],
                RecordFilter {
                    subagent_ids: Some(vec![NodeId(task.into())]),
                    ..Default::default()
                },
            )
            .await?;
        for records in parent_records.records {
            if let crate::api::ProjectionRecordSet::Subagents(tasks) = records {
                for task in tasks {
                    let task = t3_contracts::stored::to_value(&task)?;
                    if task["origin"] != "app_owned"
                        || task["childThreadId"] != thread.0
                        || task["status"] != "completed"
                    {
                        continue;
                    }
                    return Ok(match task["completionDelivery"]["state"].as_str() {
                        Some("pending" | "claimed") => ChildArchive::AwaitingDelivery,
                        Some("delivered" | "acknowledged") => ChildArchive::Ready,
                        _ => ChildArchive::None,
                    });
                }
            }
        }
        Ok(ChildArchive::None)
    }

    fn observe_retry(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
        event: &mut ProviderEvent,
    ) -> Result<()> {
        use serde_json::json;
        let thread = event_thread(event);
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state
            .sessions
            .get_mut(id)
            .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
        else {
            return Ok(());
        };
        if matches!(
            event.kind,
            ProviderEventKind::Message(_)
                | ProviderEventKind::TurnItem { .. }
                | ProviderEventKind::Node(_)
                | ProviderEventKind::Plan(_)
        ) && let Some(thread) = thread
        {
            for retry in entry.retries.values_mut() {
                if retry.item["threadId"] == json!(thread) {
                    retry.active = true;
                }
            }
        }
        match &mut event.kind {
            ProviderEventKind::TurnItem { item, .. } => {
                let value = t3_contracts::stored::to_value(&item)?;
                if value["type"] != "error"
                    || value.get("retry").is_none()
                    || value["providerTurnId"].is_null()
                    || value["providerThreadId"].is_null()
                {
                    return Ok(());
                }
                let turn: ProviderTurnId = serde_json::from_value(value["providerTurnId"].clone())?;
                if value["status"] == "running" {
                    let window = self.inner.options.retry_stall_timeout.saturating_add(
                        Duration::from_millis(value["retry"]["retryDelayMs"].as_u64().unwrap_or(0)),
                    );
                    entry.retries.insert(
                        turn,
                        RetryWatch {
                            item: value,
                            deadline: Some(Instant::now() + window),
                            window,
                            active: false,
                            stalled: false,
                        },
                    );
                } else if let Some(retry) = entry.retries.get_mut(&turn) {
                    if retry.stalled {
                        retry.deadline = None;
                    } else {
                        entry.retries.remove(&turn);
                    }
                }
            }
            ProviderEventKind::Turn { provider_turn, .. } => {
                let turn = expanded(&provider_turn.id)?;
                if provider_turn.status != ProviderTurnStatus::Running
                    && let Some(retry) = entry.retries.get_mut(&turn)
                {
                    retry.deadline = None;
                    if retry.stalled
                        && matches!(
                            provider_turn.status,
                            ProviderTurnStatus::Interrupted | ProviderTurnStatus::Cancelled
                        )
                    {
                        provider_turn.status = ProviderTurnStatus::Failed;
                    }
                    if !retry.stalled {
                        entry.retries.remove(&turn);
                    }
                }
            }
            ProviderEventKind::Terminal {
                provider_turn_id,
                outcome,
                ..
            } => {
                if let Some(retry) = entry
                    .retries
                    .remove(provider_turn_id)
                    .filter(|retry| retry.stalled)
                    && matches!(
                        outcome,
                        TerminalOutcome::Interrupted { .. } | TerminalOutcome::Cancelled { .. }
                    )
                {
                    let mut failure = retry.item["failure"].clone();
                    failure["retryable"] = json!(false);
                    *outcome = TerminalOutcome::Failed {
                        failure: Box::new(serde_json::from_value(failure)?),
                        failure_item_ordinal: retry.item["ordinal"].as_u64().unwrap_or(0),
                        retry: Some(serde_json::from_value(retry.item["retry"].clone())?),
                        retry_started_at: serde_json::from_value(retry.item["startedAt"].clone())?,
                        child_turns_continue: false,
                    };
                }
            }
            _ => {}
        }
        entry.changed.notify_one();
        Ok(())
    }

    async fn expire_retries(
        &self,
        id: &ProviderSessionId,
        runtime: &Arc<dyn SessionRuntime>,
    ) -> Result<()> {
        let due = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = state
                .sessions
                .get_mut(id)
                .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
            else {
                return Ok(());
            };
            let mut due = Vec::new();
            for (turn, retry) in &mut entry.retries {
                if !retry
                    .deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    continue;
                }
                if retry.active {
                    retry.active = false;
                    retry.deadline = Some(Instant::now() + retry.window);
                } else {
                    retry.deadline = None;
                    due.push((turn.clone(), retry.item.clone()));
                }
            }
            due
        };
        for (turn, item) in due {
            let provider_thread_id: ProviderThreadId =
                serde_json::from_value(item["providerThreadId"].clone())?;
            let loaded = {
                let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                state.sessions.get(id).and_then(|entry| {
                    entry
                        .loaded
                        .values()
                        .find(|loaded| {
                            expanded(&loaded.thread.id).ok().as_ref() == Some(&provider_thread_id)
                        })
                        .map(|loaded| loaded.thread.clone())
                })
            };
            let provider_thread = match loaded {
                Some(thread) => Some(thread),
                None => {
                    if let Some(persistence) = &self.inner.services.persistence {
                        persistence
                            .reader
                            .control(crate::api::ControlQuery::ProviderThread(provider_thread_id))
                            .await?
                            .and_then(|context| context.provider_thread)
                    } else {
                        None
                    }
                }
            };
            let Some(provider_thread) = provider_thread else {
                continue;
            };
            {
                let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                let Some(retry) = state
                    .sessions
                    .get_mut(id)
                    .filter(|entry| Arc::ptr_eq(&entry.runtime, runtime))
                    .and_then(|entry| entry.retries.get_mut(&turn))
                    .filter(|retry| retry.item == item)
                else {
                    continue;
                };
                retry.stalled = true;
            }
            let result = tokio::time::timeout(
                self.inner.options.interrupt_timeout,
                runtime.interrupt_turn(InterruptInput {
                    provider_thread,
                    provider_turn_id: turn.clone(),
                    request_runtime_restart: false,
                }),
            )
            .await;
            if matches!(result, Ok(Err(error)) if !matches!(error, ProviderError::InterruptTimeout { .. }))
                && let Some(retry) = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .sessions
                    .get_mut(id)
                    .and_then(|entry| entry.retries.get_mut(&turn))
                    .filter(|retry| retry.item == item)
            {
                retry.stalled = false;
            }
        }
        Ok(())
    }

    pub async fn close_instance(&self, instance: &ProviderInstanceId) -> Result<()> {
        let sessions = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .iter()
            .filter(|(_, entry)| &entry.instance_id == instance)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut failure = None;
        for id in sessions {
            if let Err(error) = self.detach(id, false).await {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn write_session(
        &self,
        threads: &[ThreadId],
        session: &OrchestrationV2ProviderSession,
        kind: &str,
    ) -> Result<()> {
        let Some(persistence) = &self.inner.services.persistence else {
            return Ok(());
        };
        let events = threads
            .iter()
            .map(|thread| {
                release_event(
                    persistence,
                    thread,
                    kind,
                    t3_contracts::stored::to_value(&session)?,
                    &session.driver,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        persistence
            .sink
            .commit(CommitRequest {
                receipt: None,
                guard: CommitGuard::None,
                decision: Decision {
                    events,
                    ..Default::default()
                },
            })
            .await?;
        Ok(())
    }

    async fn release_roster(
        &self,
        entry: &SessionEntry,
        reason: ReleaseReason,
    ) -> Vec<(ThreadId, OrchestrationV2ProviderThread)> {
        if reason == ReleaseReason::ServerShutdown
            || !entry.runtime.background_tasks_end_with_process()
        {
            return Vec::new();
        }
        let Some(persistence) = &self.inner.services.persistence else {
            return Vec::new();
        };
        let mut roster = Vec::new();
        for thread in &entry.threads {
            if let Ok(records) = persistence
                .reader
                .records(
                    thread.clone(),
                    vec![RecordKind::ProviderThreads],
                    RecordFilter::default(),
                )
                .await
            {
                for records in records.records {
                    if let crate::api::ProjectionRecordSet::ProviderThreads(threads) = records {
                        roster.extend(
                            threads
                                .into_iter()
                                .filter(|provider_thread| {
                                    provider_thread.provider_session_id.as_ref()
                                        == Some(&entry.runtime.session().id)
                                })
                                .map(|provider_thread| (thread.clone(), provider_thread)),
                        );
                    }
                }
            }
        }
        roster
    }

    async fn write_release_records(&self, records: &ReleaseRecords) -> Result<()> {
        use serde_json::{Value, json};
        let Some(persistence) = &self.inner.services.persistence else {
            return Ok(());
        };
        let mut failure = None;
        let replaced = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .contains_key(&records.session);
        if !replaced {
            let mut session = t3_contracts::stored::to_value(&records.runtime.session())?;
            session["status"] = json!(if records.reason == ReleaseReason::RuntimeError {
                "error"
            } else {
                "stopped"
            });
            session["updatedAt"] = json!(persistence.clock.now());
            session["lastError"] = if records.reason == ReleaseReason::RuntimeError {
                json!(
                    records
                        .detail
                        .as_deref()
                        .unwrap_or("Provider runtime failed.")
                )
            } else {
                Value::Null
            };
            if session.get("extensionStatuses").is_some() {
                session["extensionStatuses"] = json!([]);
            }
            if let Err(error) = self
                .write_session(
                    &records.threads.iter().cloned().collect::<Vec<_>>(),
                    &serde_json::from_value(session)?,
                    "provider-session.updated",
                )
                .await
            {
                failure.get_or_insert(error);
            }
        }
        for thread in &records.threads {
            if let Err(error) = self.release_requests(thread, records).await {
                failure.get_or_insert(error);
            }
        }
        for (thread, released) in &records.roster {
            if let Err(error) = self
                .release_background(thread, released, records.released_at.as_ref())
                .await
            {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn release_requests(&self, thread: &ThreadId, released: &ReleaseRecords) -> Result<()> {
        use serde_json::json;
        let Some(persistence) = &self.inner.services.persistence else {
            return Ok(());
        };
        let records = persistence
            .reader
            .records(
                thread.clone(),
                vec![
                    RecordKind::RuntimeRequests,
                    RecordKind::Nodes,
                    RecordKind::TurnItems,
                ],
                RecordFilter {
                    runtime_request_statuses: Some(vec![RuntimeRequestStatus::Pending]),
                    ..Default::default()
                },
            )
            .await?;
        let mut requests = Vec::new();
        let mut nodes = Vec::new();
        let mut items = Vec::new();
        for set in records.records {
            match set {
                crate::api::ProjectionRecordSet::RuntimeRequests(values) => requests = values,
                crate::api::ProjectionRecordSet::Nodes(values) => nodes = values,
                crate::api::ProjectionRecordSet::TurnItems(values) => items = values,
                _ => {}
            }
        }
        let now = persistence.clock.now();
        let mut events = Vec::new();
        for request in requests {
            let mut request = t3_contracts::stored::to_value(&request)?;
            if request["status"] != "pending"
                || request["responseCapability"]["type"] != "live"
                || request["responseCapability"]["providerSessionId"] != json!(released.session)
                || released
                    .released_at
                    .as_ref()
                    .is_some_and(|cutoff| !at_or_before(&request["createdAt"], cutoff))
                || (released.reason == ReleaseReason::ServerShutdown
                    && request["kind"] == "user_input")
            {
                continue;
            }
            let failed = released.reason == ReleaseReason::RuntimeError;
            request["status"] = json!(if matches!(
                released.reason,
                ReleaseReason::ManualShutdown | ReleaseReason::ServerShutdown
            ) {
                "cancelled"
            } else {
                "expired"
            });
            request["responseCapability"] = json!({"type":"not_resumable","reason":if failed {"Provider session failed before this runtime request was resolved."} else {"Provider session was closed before this runtime request was resolved."}});
            request["resolvedAt"] = json!(now);
            events.push(release_event(
                persistence,
                thread,
                "runtime-request.updated",
                request.clone(),
                &released.runtime.session().driver,
            )?);
            for node in &nodes {
                let mut node = t3_contracts::stored::to_value(&node)?;
                if node["id"] != request["nodeId"] {
                    continue;
                }
                node["status"] = json!(if failed { "failed" } else { "cancelled" });
                node["completedAt"] = json!(now);
                events.push(release_event(
                    persistence,
                    thread,
                    "node.updated",
                    node,
                    &released.runtime.session().driver,
                )?);
            }
            for item in &items {
                let mut item = t3_contracts::stored::to_value(&item)?;
                if !matches!(
                    item["type"].as_str(),
                    Some("approval_request" | "user_input_request")
                ) || item["requestId"] != request["id"]
                {
                    continue;
                }
                item["status"] = json!(if failed { "failed" } else { "cancelled" });
                item["completedAt"] = json!(now);
                item["updatedAt"] = json!(now);
                events.push(release_event(
                    persistence,
                    thread,
                    "turn-item.updated",
                    item,
                    &released.runtime.session().driver,
                )?);
            }
        }
        if !events.is_empty() {
            persistence
                .sink
                .commit(CommitRequest {
                    receipt: None,
                    guard: CommitGuard::None,
                    decision: Decision {
                        events,
                        ..Default::default()
                    },
                })
                .await?;
        }
        Ok(())
    }

    async fn release_background(
        &self,
        thread: &ThreadId,
        released: &OrchestrationV2ProviderThread,
        cutoff: Option<&IsoDateTime>,
    ) -> Result<()> {
        use serde_json::json;
        let Some(persistence) = &self.inner.services.persistence else {
            return Ok(());
        };
        let original = t3_contracts::stored::to_value(&released)?;
        let task_ids = original["pendingBackgroundTasks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|task| task["taskId"].clone())
            .collect::<Vec<_>>();
        for _ in 0..3 {
            let records = persistence
                .reader
                .records(
                    thread.clone(),
                    vec![RecordKind::ProviderThreads, RecordKind::TurnItems],
                    RecordFilter {
                        turn_item_types: Some(vec![
                            "command_execution".into(),
                            "dynamic_tool".into(),
                        ]),
                        turn_item_statuses: Some(vec![
                            OrchestrationV2TurnItemStatus::Pending,
                            OrchestrationV2TurnItemStatus::Running,
                            OrchestrationV2TurnItemStatus::Waiting,
                        ]),
                        ..Default::default()
                    },
                )
                .await?;
            let mut conflict = false;
            for set in records.records {
                match set {
                    crate::api::ProjectionRecordSet::ProviderThreads(threads) => {
                        for expected in threads {
                            if expected.id != released.id {
                                continue;
                            }
                            let mut updated = t3_contracts::stored::to_value(&expected)?;
                            let remaining = updated["pendingBackgroundTasks"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter(|task| !task_ids.contains(&task["taskId"]))
                                .cloned()
                                .collect::<Vec<_>>();
                            if remaining.len()
                                == updated["pendingBackgroundTasks"]
                                    .as_array()
                                    .map_or(0, Vec::len)
                            {
                                continue;
                            }
                            updated["pendingBackgroundTasks"] = json!(remaining);
                            updated["updatedAt"] = json!(persistence.clock.now());
                            let result = persistence
                                .sink
                                .commit(CommitRequest {
                                    receipt: None,
                                    guard: CommitGuard::ProviderThreadUnchanged(Box::new(expected)),
                                    decision: Decision {
                                        events: vec![release_event(
                                            persistence,
                                            thread,
                                            "provider-thread.updated",
                                            updated,
                                            &released.driver,
                                        )?],
                                        ..Default::default()
                                    },
                                })
                                .await?;
                            conflict |= !result.committed;
                        }
                    }
                    crate::api::ProjectionRecordSet::TurnItems(items) => {
                        for expected in items {
                            let mut updated = t3_contracts::stored::to_value(&expected)?;
                            if !matches!(
                                updated["type"].as_str(),
                                Some("command_execution" | "dynamic_tool")
                            ) || !matches!(
                                updated["status"].as_str(),
                                Some("pending" | "running" | "waiting")
                            ) || updated["providerThreadId"] != original["id"]
                            {
                                continue;
                            }
                            let started = if updated["startedAt"].is_null() {
                                &updated["updatedAt"]
                            } else {
                                &updated["startedAt"]
                            };
                            if cutoff.is_some_and(|cutoff| !at_or_before(started, cutoff)) {
                                continue;
                            }
                            updated["status"] = json!("interrupted");
                            updated["completedAt"] = json!(persistence.clock.now());
                            updated["updatedAt"] = json!(persistence.clock.now());
                            let result = persistence
                                .sink
                                .commit(CommitRequest {
                                    receipt: None,
                                    guard: CommitGuard::TurnItemUnchanged(Box::new(expected)),
                                    decision: Decision {
                                        events: vec![release_event(
                                            persistence,
                                            thread,
                                            "turn-item.updated",
                                            updated,
                                            &released.driver,
                                        )?],
                                        ..Default::default()
                                    },
                                })
                                .await?;
                            conflict |= !result.committed;
                        }
                    }
                    _ => {}
                }
            }
            if !conflict {
                break;
            }
        }
        Ok(())
    }
}

struct ReleaseRecords {
    session: ProviderSessionId,
    runtime: Arc<dyn SessionRuntime>,
    threads: BTreeSet<ThreadId>,
    reason: ReleaseReason,
    detail: Option<String>,
    released_at: Option<IsoDateTime>,
    roster: Vec<(ThreadId, OrchestrationV2ProviderThread)>,
}

struct CredentialReservation {
    manager: Weak<Inner>,
    key: (ThreadId, String),
}
impl Drop for CredentialReservation {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            let mut state = manager.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(count) = state.credential_reservations.get_mut(&self.key) {
                *count -= 1;
                if *count == 0 {
                    state.credential_reservations.remove(&self.key);
                }
            }
        }
    }
}

struct BusyGuard {
    manager: Weak<Inner>,
    runtime: Arc<dyn SessionRuntime>,
    id: ProviderSessionId,
    thread: ProviderThreadId,
    ordinal: u64,
    armed: bool,
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(inner) = self.manager.upgrade()
        {
            ProviderSessionManager { inner }.mark_idle(
                &self.id,
                &self.runtime,
                &self.thread,
                self.ordinal,
            );
        }
    }
}

fn at_or_before(value: &serde_json::Value, cutoff: &IsoDateTime) -> bool {
    value
        .as_str()
        .and_then(super::usage_limit_recovery_worker::epoch_millis)
        .zip(super::usage_limit_recovery_worker::epoch_millis(&cutoff.0))
        .is_some_and(|(value, cutoff)| value <= cutoff)
}

fn release_event(
    persistence: &SessionPersistence,
    thread: &ThreadId,
    kind: &str,
    payload: serde_json::Value,
    driver: &t3_contracts::provider_instance::ProviderDriverKind,
) -> Result<OrchestrationV2DomainEvent> {
    let mut event = serde_json::json!({"id":persistence.ids.next(IdKind::Event),"threadId":thread,"type":kind,"occurredAt":persistence.clock.now(),"driver":driver,"payload":payload});
    for key in ["runId", "nodeId", "driver", "providerInstanceId"] {
        if let Some(value) = payload.get(key).filter(|value| !value.is_null()) {
            event[key] = value.clone();
        }
    }
    if kind == "node.updated" {
        event["nodeId"] = payload["id"].clone();
    }
    Ok(serde_json::from_value(event)?)
}

fn expanded<T: Clone>(
    id: &t3_contracts::orchestration_v2_id_table::IdOrIndex<T>,
) -> ProviderResult<T> {
    match id {
        t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) => Ok(id.clone()),
        _ => Err(ProviderError::Operation {
            operation: ProviderOperation::Ensure,
            detail: "session manager requires expanded IDs".into(),
        }),
    }
}

/// A command item that stopped working: its live tail is released after the item is handed on.
fn settled_command(event: &ProviderEvent) -> ProviderResult<Option<(ThreadId, TurnItemId)>> {
    let ProviderEventKind::TurnItem { item, .. } = &event.kind else {
        return Ok(None);
    };
    let OrchestrationV2TurnItem::CommandExecution(item) = item.as_ref() else {
        return Ok(None);
    };
    if matches!(
        item.status,
        OrchestrationV2TurnItemStatus::Pending
            | OrchestrationV2TurnItemStatus::Running
            | OrchestrationV2TurnItemStatus::Waiting
    ) {
        return Ok(None);
    }
    Ok(Some((expanded(&item.thread_id)?, expanded(&item.id)?)))
}

fn event_thread(event: &ProviderEvent) -> Option<ThreadId> {
    match &event.kind {
        ProviderEventKind::Thread {
            provider_thread, ..
        } => provider_thread
            .app_thread_id
            .as_ref()
            .and_then(|id| expanded(id).ok()),
        ProviderEventKind::Message(message) => expanded(&message.thread_id).ok(),
        ProviderEventKind::Node(node) => expanded(&node.thread_id).ok(),
        ProviderEventKind::TurnItem { item, .. } => t3_contracts::stored::to_value(&item)
            .ok()
            .and_then(|item| item["threadId"].as_str().map(|id| ThreadId(id.into()))),
        ProviderEventKind::RuntimeRequest { thread_id, .. }
        | ProviderEventKind::Turn { thread_id, .. } => thread_id.clone(),
        _ => None,
    }
}

fn session_artifact_thread(event: &ProviderEvent) -> Result<Option<ThreadId>> {
    let id = match &event.kind {
        ProviderEventKind::Thread {
            provider_thread,
            session_scoped: Some(true),
            ..
        } => provider_thread.app_thread_id.as_ref(),
        ProviderEventKind::RuntimeRequest { request, thread_id }
            if request.provider_turn_id.is_none() =>
        {
            return Ok(thread_id.clone());
        }
        ProviderEventKind::Node(node)
            if node.run_id.is_none() && node.runtime_request_id.is_some() =>
        {
            Some(&node.thread_id)
        }
        ProviderEventKind::TurnItem {
            item,
            session_scoped,
        } => match item.as_ref() {
            OrchestrationV2TurnItem::CommandExecution(item) if *session_scoped == Some(true) => {
                Some(&item.thread_id)
            }
            OrchestrationV2TurnItem::ApprovalRequest(item) if item.run_id.is_none() => {
                Some(&item.thread_id)
            }
            OrchestrationV2TurnItem::UserInputRequest(item) if item.run_id.is_none() => {
                Some(&item.thread_id)
            }
            _ => None,
        },
        _ => None,
    };
    id.map(expanded).transpose().map_err(Into::into)
}

struct ManagedRuntime {
    manager: Weak<Inner>,
    runtime: Arc<dyn SessionRuntime>,
    id: ProviderSessionId,
}
impl ManagedRuntime {
    fn manager(&self) -> ProviderResult<ProviderSessionManager> {
        self.manager
            .upgrade()
            .map(|inner| ProviderSessionManager { inner })
            .ok_or(ProviderError::SessionStopped)
    }
    fn touch(&self) {
        if let Ok(manager) = self.manager() {
            manager.touch(&self.id, false);
        }
    }
    fn extension(&self) -> ProviderResult<&dyn SessionExtensions> {
        self.runtime
            .extensions()
            .ok_or(ProviderError::Unsupported("session extensions"))
    }
    async fn ensure_attached(&self, thread: &ThreadId) -> ProviderResult<()> {
        let manager = self.manager()?;
        let attached = {
            let state = manager
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let entry = state
                .sessions
                .get(&self.id)
                .filter(|entry| {
                    Arc::ptr_eq(&entry.runtime, &self.runtime) && !entry.cancellation.is_cancelled()
                })
                .ok_or(ProviderError::SessionStopped)?;
            if !entry.threads.contains(thread)
                && !self
                    .runtime
                    .session()
                    .capabilities
                    .sessions
                    .supports_multiple_provider_threads_per_session
            {
                return Err(ProviderError::Unsupported(
                    "multiple app threads per session",
                ));
            }
            entry.threads.contains(thread)
        };
        if attached {
            return Ok(());
        }
        let mut credential = manager
            .prepare_credential(
                thread.clone(),
                expanded(&self.runtime.session().provider_instance_id)?,
            )
            .await
            .map_err(|error| ProviderError::Operation {
                operation: ProviderOperation::Ensure,
                detail: error.to_string(),
            })?;
        let reservation = manager.reserve_credential(thread.clone(), credential.as_ref());
        if let Some(credential) = &mut credential {
            credential
                .publish()
                .map_err(|error| ProviderError::Operation {
                    operation: ProviderOperation::Ensure,
                    detail: error.to_string(),
                })?;
        }
        if let Err(error) = manager
            .write_session(
                std::slice::from_ref(thread),
                self.runtime.session(),
                "provider-session.attached",
            )
            .await
        {
            drop(reservation);
            let _ = manager.discard_prepared(thread.clone(), credential).await;
            return Err(ProviderError::Operation {
                operation: ProviderOperation::Ensure,
                detail: error.to_string(),
            });
        }
        let mut state = manager
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = state
            .sessions
            .get_mut(&self.id)
            .filter(|entry| Arc::ptr_eq(&entry.runtime, &self.runtime))
            .ok_or(ProviderError::SessionStopped)?;
        entry.threads.insert(thread.clone());
        if let Some(credential) = credential {
            entry.credentials.insert(thread.clone(), credential);
        }
        Ok(())
    }
    fn load_key(
        &self,
        thread: &OrchestrationV2ProviderThread,
        selection: Option<&t3_contracts::model_selection::ModelSelection>,
        policy: Option<&RuntimePolicy>,
        options: Option<&RuntimePolicyOptions>,
    ) -> ProviderResult<String> {
        let selection = selection.map(|selection| {
            self.runtime.extensions().map_or_else(
                || selection.clone(),
                |extension| extension.normalize_thread_load_selection(selection.clone()),
            )
        });
        let thread =
            t3_contracts::stored::to_value(&thread).map_err(|error| ProviderError::Operation {
                operation: ProviderOperation::Resume,
                detail: error.to_string(),
            })?;
        Ok(serde_json::json!({"providerThread":thread.get("nativeThreadRef").filter(|value| !value.is_null()).unwrap_or(&thread["id"]),"modelSelection":selection,"reasoningEffort":options.and_then(|options| options.reasoning_effort.as_deref()),"policy":policy.map(|policy| serde_json::json!({"runtimeMode":policy.runtime_mode,"interactionMode":policy.interaction_mode,"cwd":policy.cwd,"approvalPolicy":policy.approval_policy,"sandboxPolicy":policy.sandbox_policy,"maxRuntimeMode":policy.max_runtime_mode}))}).to_string())
    }
    fn loaded(
        &self,
        thread: ThreadId,
        provider_thread: OrchestrationV2ProviderThread,
        key: String,
    ) -> ProviderResult<()> {
        let manager = self.manager()?;
        let mut state = manager
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = state
            .sessions
            .get_mut(&self.id)
            .filter(|entry| Arc::ptr_eq(&entry.runtime, &self.runtime))
            .ok_or(ProviderError::SessionStopped)?;
        entry.loaded.insert(
            thread,
            LoadedThread {
                thread: provider_thread,
                key,
                deadline: None,
            },
        );
        Ok(())
    }
}

impl ManagedRuntime {
    async fn ensure(
        &self,
        input: EnsureThread,
        options: Option<RuntimePolicyOptions>,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let lane = self.manager()?.thread_lane(&self.id, &input.thread_id);
        let _lane = lane.lock().await;
        self.ensure_attached(&input.thread_id).await?;
        self.touch();
        let thread = match &options {
            Some(options) => {
                self.runtime
                    .ensure_thread_with_policy(input.clone(), options.clone())
                    .await?
            }
            None => self.runtime.ensure_thread(input.clone()).await?,
        };
        self.loaded(
            input.thread_id,
            thread.clone(),
            self.load_key(
                &thread,
                Some(&input.model_selection),
                Some(&input.runtime_policy),
                options.as_ref(),
            )?,
        )?;
        Ok(thread)
    }
    async fn resume(
        &self,
        input: ResumeThread,
        options: Option<RuntimePolicyOptions>,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.touch();
        let Some(thread_id) = input.thread_id.clone().or_else(|| {
            input
                .provider_thread
                .app_thread_id
                .as_ref()
                .and_then(|id| expanded(id).ok())
        }) else {
            return match options {
                Some(options) => self.runtime.resume_thread_with_policy(input, options).await,
                None => self.runtime.resume_thread(input).await,
            };
        };
        let lane = self.manager()?.thread_lane(&self.id, &thread_id);
        let _lane = lane.lock().await;
        self.ensure_attached(&thread_id).await?;
        let key = self.load_key(
            &input.provider_thread,
            input.model_selection.as_ref(),
            input.runtime_policy.as_ref(),
            options.as_ref(),
        )?;
        let manager = self.manager()?;
        {
            let mut state = manager
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = state
                .sessions
                .get_mut(&self.id)
                .filter(|entry| Arc::ptr_eq(&entry.runtime, &self.runtime))
            {
                if let Some(loaded) = entry.loaded.get_mut(&thread_id) {
                    loaded.deadline = None;
                    if loaded.key == key {
                        return Ok(input.provider_thread);
                    }
                }
            } else {
                return Err(ProviderError::SessionStopped);
            }
        }
        let thread = match options {
            Some(options) => {
                self.runtime
                    .resume_thread_with_policy(input, options)
                    .await?
            }
            None => self.runtime.resume_thread(input).await?,
        };
        self.loaded(thread_id, thread.clone(), key)?;
        Ok(thread)
    }
    async fn start(
        &self,
        input: TurnInput,
        options: Option<RuntimePolicyOptions>,
    ) -> ProviderResult<()> {
        let lane = self.manager()?.thread_lane(&self.id, &input.thread_id);
        let _lane = lane.lock().await;
        self.ensure_attached(&input.thread_id).await?;
        let manager = self.manager()?;
        let admission = manager.admission(&self.id)?;
        {
            let _admission = admission.lock().await;
            manager.mark_busy(&self.id, &self.runtime, &input)?;
        }
        let mut busy = BusyGuard {
            manager: Arc::downgrade(&manager.inner),
            runtime: self.runtime.clone(),
            id: self.id.clone(),
            thread: expanded(&input.provider_thread.id)?,
            ordinal: input.run_ordinal,
            armed: true,
        };
        let result = match options {
            Some(options) => self.runtime.start_turn_with_policy(input, options).await,
            None => self.runtime.start_turn(input).await,
        };
        if result.is_ok() {
            busy.armed = false;
        }
        result
    }
    async fn fork_managed(
        &self,
        input: ForkInput,
        options: Option<RuntimePolicyOptions>,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let lane = self
            .manager()?
            .thread_lane(&self.id, &input.target_thread_id);
        let _lane = lane.lock().await;
        self.ensure_attached(&input.target_thread_id).await?;
        let thread = match &options {
            Some(options) => {
                self.runtime
                    .fork_with_policy(input.clone(), options.clone())
                    .await?
            }
            None => self.runtime.fork(input.clone()).await?,
        };
        self.loaded(
            input.target_thread_id,
            thread.clone(),
            self.load_key(
                &thread,
                input.model_selection.as_ref(),
                input.runtime_policy.as_ref(),
                options.as_ref(),
            )?,
        )?;
        Ok(thread)
    }
}

#[async_trait]
impl SessionRuntime for ManagedRuntime {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        self.runtime.session()
    }
    fn can_resume_after_restart(&self) -> bool {
        self.runtime.can_resume_after_restart()
    }
    fn background_tasks_end_with_process(&self) -> bool {
        self.runtime.background_tasks_end_with_process()
    }
    fn leaves_no_background_work(&self) -> bool {
        self.runtime.leaves_no_background_work()
    }
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        self.runtime
            .extensions()
            .map(|_| self as &dyn SessionExtensions)
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        let manager = self.manager()?;
        let mut state = manager
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = state
            .sessions
            .get_mut(&self.id)
            .filter(|entry| Arc::ptr_eq(&entry.runtime, &self.runtime))
            .ok_or(ProviderError::SessionStopped)?;
        let (send, events) = mpsc::channel(64);
        // Subscribers drain terminal signals after the producer has stopped.
        let cancellation = CancellationToken::new();
        entry.subscribers.push((send, cancellation.clone()));
        Ok(EventSubscription {
            events,
            cancellation,
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.manager()?
            .detach(self.id.clone(), false)
            .await
            .map_err(|error| ProviderError::Operation {
                operation: ProviderOperation::Close,
                detail: error.to_string(),
            })
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.ensure(input, None).await
    }
    async fn ensure_thread_with_policy(
        &self,
        input: EnsureThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.ensure(input, Some(options)).await
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.resume(input, None).await
    }
    async fn resume_thread_with_policy(
        &self,
        input: ResumeThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.resume(input, Some(options)).await
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        self.start(input, None).await
    }
    async fn start_turn_with_policy(
        &self,
        input: TurnInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<()> {
        self.start(input, Some(options)).await
    }
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
        self.touch();
        self.runtime.steer_turn(input).await
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        self.touch();
        let timeout = self.manager()?.inner.options.interrupt_timeout;
        let turn_id = input.provider_turn_id.clone();
        tokio::time::timeout(timeout, self.runtime.interrupt_turn(input))
            .await
            .unwrap_or_else(|_| {
                Err(ProviderError::InterruptTimeout {
                    turn_id,
                    timeout_ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
                })
            })
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        self.touch();
        self.runtime.respond(input).await
    }
    async fn snapshot(
        &self,
        thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        self.runtime.snapshot(thread).await
    }
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        self.runtime.rollback(input).await
    }
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.fork_managed(input, None).await
    }
    async fn fork_with_policy(
        &self,
        input: ForkInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.fork_managed(input, Some(options)).await
    }
}

#[async_trait]
impl SessionExtensions for ManagedRuntime {
    fn capabilities(&self) -> ExtensionCapabilities {
        self.runtime.extensions().map_or_else(
            ExtensionCapabilities::default,
            SessionExtensions::capabilities,
        )
    }
    async fn pending_background_work(
        &self,
        thread: Option<OrchestrationV2ProviderThread>,
        include_subagents: bool,
    ) -> ProviderResult<bool> {
        self.extension()?
            .pending_background_work(thread, include_subagents)
            .await
    }
    async fn model_context_window(
        &self,
        selection: t3_contracts::model_selection::ModelSelection,
        cwd: Option<std::path::PathBuf>,
    ) -> ProviderResult<Option<u64>> {
        self.extension()?.model_context_window(selection, cwd).await
    }
    fn can_reuse_context_usage(
        &self,
        previous: &t3_contracts::model_selection::ModelSelection,
        next: &t3_contracts::model_selection::ModelSelection,
    ) -> bool {
        self.runtime
            .extensions()
            .is_some_and(|extension| extension.can_reuse_context_usage(previous, next))
    }
    fn normalize_thread_load_selection(
        &self,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> t3_contracts::model_selection::ModelSelection {
        self.runtime.extensions().map_or_else(
            || selection.clone(),
            |extension| extension.normalize_thread_load_selection(selection.clone()),
        )
    }
    async fn inject_history(
        &self,
        thread: OrchestrationV2ProviderThread,
        messages: Vec<OrchestrationV2HistoricalMessage>,
        context: String,
    ) -> ProviderResult<bool> {
        self.extension()?
            .inject_history(thread, messages, context)
            .await
    }
    async fn compact(&self, input: TurnInput) -> ProviderResult<()> {
        self.touch();
        self.extension()?.compact(input).await
    }
    async fn apply_selection(
        &self,
        thread: OrchestrationV2ProviderThread,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> ProviderResult<()> {
        self.touch();
        self.extension()?.apply_selection(thread, selection).await
    }
    async fn stop_task(
        &self,
        thread: OrchestrationV2ProviderThread,
        task_id: String,
    ) -> ProviderResult<()> {
        self.extension()?.stop_task(thread, task_id).await
    }
    async fn await_tasks(
        &self,
        thread: OrchestrationV2ProviderThread,
        task_ids: Vec<String>,
        awaited: bool,
    ) -> ProviderResult<Vec<String>> {
        self.extension()?
            .await_tasks(thread, task_ids, awaited)
            .await
    }
    async fn stop_subagent(
        &self,
        thread: OrchestrationV2ProviderThread,
        native_task_id: String,
        requested_at: Option<IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>> {
        self.extension()?
            .stop_subagent(thread, native_task_id, requested_at)
            .await
    }
    async fn unload(
        &self,
        thread: OrchestrationV2ProviderThread,
        archive: bool,
    ) -> ProviderResult<()> {
        let manager = self.manager()?;
        {
            let mut state = manager
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = state
                .sessions
                .get_mut(&self.id)
                .filter(|entry| Arc::ptr_eq(&entry.runtime, &self.runtime))
            {
                entry
                    .loaded
                    .retain(|_, loaded| loaded.thread.id != thread.id);
            }
        }
        self.extension()?.unload(thread, archive).await
    }
    async fn upload_feedback(
        &self,
        thread: OrchestrationV2ProviderThread,
        reason: Option<String>,
    ) -> ProviderResult<String> {
        self.extension()?.upload_feedback(thread, reason).await
    }
    async fn list_tools(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
    ) -> ProviderResult<Vec<McpTool>> {
        self.extension()?.list_tools(thread, server).await
    }
    async fn call_tool(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
        tool: String,
        arguments: serde_json::Value,
    ) -> ProviderResult<McpResult> {
        self.extension()?
            .call_tool(thread, server, tool, arguments)
            .await
    }
    async fn read_resource(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
        uri: String,
    ) -> ProviderResult<Vec<serde_json::Value>> {
        self.extension()?.read_resource(thread, server, uri).await
    }
}

impl crate::ports::runs_ingestion::run_execution_service::ProviderStreamContext
    for ProviderSessionManager
{
    fn detached_thread_id(&self, error: &ProviderError) -> Option<ThreadId> {
        let ProviderError::Protocol {
            payload: Some(payload),
            ..
        } = error
        else {
            return None;
        };
        // MARKED SHIM: only the manager's tagged stream error proves intentional detach.
        if payload["_tag"] != "ProviderAdapterEventStreamError" {
            return None;
        }
        serde_json::from_value(payload.get("detachedThreadId")?.clone()).ok()
    }
}

#[async_trait]
impl SessionDirectory for ProviderSessionManager {
    async fn attach_with_policy(
        &self,
        instance_id: ProviderInstanceId,
        input: OpenSession,
        options: RuntimePolicyOptions,
    ) -> Result<Arc<dyn SessionRuntime>> {
        ProviderSessionManager::attach_with_policy(self, instance_id, input, options).await
    }
    async fn detach_thread(&self, input: crate::api::DetachSession) -> Result<()> {
        ProviderSessionManager::detach_thread_with_detail(
            self,
            input.provider_session_id,
            input.thread_id,
            input.revoke_credential,
            input.detail,
        )
        .await
    }
    async fn release_unless_busy(&self, input: crate::api::ReleaseSession) -> Result<bool> {
        self.release_checked(input).await
    }
    async fn other_working_threads(
        &self,
        session: ProviderSessionId,
        thread: ThreadId,
    ) -> Result<Vec<ThreadId>> {
        ProviderSessionManager::other_working_threads(self, session, thread).await
    }
    async fn get(&self, session_id: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>> {
        self.touch(&session_id, false);
        Ok(self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .get(&session_id)
            .filter(|entry| !entry.cancellation.is_cancelled())
            .map(|entry| entry.exposed.clone()))
    }
    async fn attach(
        &self,
        instance_id: ProviderInstanceId,
        input: OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>> {
        self.attach_configured(instance_id, input, None).await
    }
    async fn detach(&self, session_id: ProviderSessionId, revoke_credential: bool) -> Result<()> {
        if revoke_credential && self.inner.services.credentials.is_none() {
            return Err(Error::Unsupported(
                "session credential revocation requires a credential service",
            ));
        }
        let (send, receive) = oneshot::channel();
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(Error::Cancelled);
            }
            let lane = Self::lane(&mut state, &session_id);
            let manager = self.clone();
            self.inner.operations.spawn(async move {
                let _lane = lane.lock().await;
                let _ = send.send(manager.close(&session_id).await);
            });
        }
        receive.await.map_err(|e| Error::Service {
            operation: "ProviderSessionManager.detach",
            detail: e.to_string(),
        })?
    }
    async fn shutdown(&self) -> Result<()> {
        let mut completion = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(completion) = &state.shutdown_result {
                completion.clone()
            } else {
                state.closed = true;
                self.inner.operations.close();
                self.inner.maintenance.close();
                self.inner.cancellation.cancel();
                let (send, receive) = watch::channel(None);
                state.shutdown_result = Some(receive.clone());
                let manager = self.clone();
                state.shutdown_task = Some(tokio::spawn(async move {
                    manager.inner.operations.wait().await;
                    manager.inner.maintenance.wait().await;
                    let sessions = manager
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .sessions
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>();
                    let mut closes = tokio::task::JoinSet::new();
                    for id in sessions {
                        let manager = manager.clone();
                        closes.spawn(async move {
                            manager
                                .close_reason(&id, ReleaseReason::ServerShutdown, None)
                                .await
                        });
                    }
                    let mut failure = None;
                    while let Some(result) = closes.join_next().await {
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                failure.get_or_insert_with(|| error.to_string());
                            }
                            Err(error) => {
                                failure.get_or_insert_with(|| error.to_string());
                            }
                        }
                    }
                    send.send_replace(Some(failure));
                }));
                receive
            }
        };
        loop {
            if let Some(result) = completion.borrow_and_update().clone() {
                return result.map_or(Ok(()), |detail| {
                    Err(Error::Service {
                        operation: "ProviderSessionManager.shutdown",
                        detail,
                    })
                });
            }
            completion.changed().await.map_err(|e| Error::Service {
                operation: "ProviderSessionManager.shutdown",
                detail: e.to_string(),
            })?;
        }
    }
}
