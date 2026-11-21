//! The shared connection task owns authorization, RPC, synchronization and reconnects.
use super::{
    socket::{PlatformSession, PlatformSessionFactory},
    *,
};
use crate::{
    ClientError, ClientStore,
    authorization::{RemoteAuthorization, Sensitive, bootstrap_remote_session},
    environment::{
        EnvironmentRegistration, EnvironmentRegistry, derive_ws_base_url,
        fetch_remote_environment_descriptor, normalize_http_base_url,
    },
    persistence::EnvironmentCacheStore,
    platform::PlatformError,
    rpc::SessionEndpoint,
    state::{
        http::HttpSnapshotSource,
        sync::{ShellSync, ThreadSync},
    },
    types::{SyncStatus, ThreadId, ThreadKey},
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{AbortHandle, Abortable, LocalBoxFuture},
    stream::{FuturesUnordered, LocalBoxStream},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
};

pub enum ServerCredential {
    Bearer(Sensitive<String>),
    PairingToken(Sensitive<String>),
}
pub type ConnectionTask = LocalBoxFuture<'static, Result<(), ClientError>>;
/// Saves the direct addresses an environment announced over its active route and returns the
/// environment's routes when they changed (TypeScript's `learnRoutes` registry option). Called
/// with the environment, the active route's id and the announced endpoints.
pub type LearnRoutes = Rc<
    dyn Fn(
        EnvironmentId,
        String,
        Vec<ReportedEndpoint>,
    ) -> LocalBoxFuture<'static, Option<Vec<Route>>>,
>;

struct Owner {
    environment: EnvironmentId,
    store: ClientStore,
    events: mpsc::UnboundedSender<ConnectionEvent>,
    endpoint: RefCell<Option<SessionEndpoint>>,
    /// The current session's HTTP address: the paired address, the endpoint the relay named,
    /// or the local end of the SSH tunnel.
    http_base: RefCell<Option<String>>,
    active_route: RefCell<Option<String>>,
    settings_http: RefCell<Option<crate::permissions::EnvironmentHttpClient>>,
    store_session: Cell<Option<Generation>>,
    threads: Rc<super::threads::ThreadDemand>,
    abort: AbortHandle,
    close_requested: Cell<bool>,
    /// This connection's claim on the environment's connection state in the store.
    serial: u64,
}
impl Drop for Owner {
    fn drop(&mut self) {
        // Composition requests close and then drops its client handle. The driver
        // still has to send the close frame in that case.
        if !self.close_requested.get() {
            self.abort.abort();
        }
        if let Some(endpoint) = self.endpoint.get_mut().take() {
            endpoint.invalidate();
        }
        if let Some(session) = self.store_session.get() {
            self.store.disconnect(&self.environment, session);
        }
    }
}

#[derive(Clone)]
pub struct ConnectedClient {
    owner: Rc<Owner>,
}
impl ConnectedClient {
    pub fn environment(&self) -> &EnvironmentId {
        &self.owner.environment
    }
    /// Identifies this connection among every connection the environment has had.
    pub fn serial(&self) -> u64 {
        self.owner.serial
    }
    pub fn store(&self) -> &ClientStore {
        &self.owner.store
    }
    pub fn session(&self) -> Option<SessionEndpoint> {
        self.owner.endpoint.borrow().clone()
    }
    /// Where the current session's HTTP requests (signed uploads, assets) go, as TypeScript's
    /// prepared connection `httpBaseUrl`; `None` while there is no session.
    pub fn http_base_url(&self) -> Option<String> {
        self.owner.http_base.borrow().clone()
    }
    /// The route of the installed session, including relay and SSH routes whose
    /// prepared address differs from the saved route.
    pub fn active_route_id(&self) -> Option<String> {
        self.owner.active_route.borrow().clone()
    }
    /// The guarded HTTP client belongs to the same authorization generation as the
    /// installed socket. Settings mutations resolve it again at submission.
    pub(crate) fn access_http(&self) -> Option<crate::permissions::EnvironmentHttpClient> {
        (self
            .session()?
            .grants_scope(t3_contracts::auth::AuthEnvironmentScope::AccessWrite)
            == Some(true))
        .then(|| self.owner.settings_http.borrow().clone())
        .flatten()
    }
    pub fn state(&self) -> ConnectionState {
        self.store()
            .snapshot()
            .environments
            .get(self.environment())
            .and_then(|state| state.connection.as_deref().cloned())
            .unwrap_or_default()
    }
    pub fn reconnect(&self) -> Result<(), ClientError> {
        self.send(ConnectionEvent::Restart)
    }
    pub(crate) fn set_enabled(&self, enabled: bool) -> Result<(), ClientError> {
        self.send(ConnectionEvent::SetEnabled(enabled))
    }
    /// Follows `thread` (snapshot, then live events, into the store) while the lease is held and
    /// the connection has a session; every new session follows it again. Leases for one thread
    /// share one follow.
    pub fn open_thread(&self, thread: ThreadId) -> super::ThreadLease {
        self.owner.threads.lease(thread)
    }
    pub fn close(&self) -> Result<(), ClientError> {
        self.send(ConnectionEvent::Disconnect)?;
        self.owner.close_requested.set(true);
        Ok(())
    }
    fn send(&self, event: ConnectionEvent) -> Result<(), ClientError> {
        self.owner
            .events
            .unbounded_send(event)
            .map_err(|_| ClientError::Disconnected)
    }
    pub async fn ready(&self) -> Result<(), ClientError> {
        self.wait_ready(true).await
    }
    async fn wait_ready(&self, require_live: bool) -> Result<(), ClientError> {
        let mut changes = self.store().subscribe_environment(self.environment());
        loop {
            let state = self.state();
            if state.phase == Phase::Blocked {
                return Err(ClientError::InvalidPayload(state.last_failure.map_or_else(
                    || "Connection is blocked.".into(),
                    |error| error.detail,
                )));
            }
            if !state.desired {
                return Err(ClientError::Cancelled);
            }
            let available = self
                .store()
                .snapshot()
                .environments
                .get(self.environment())
                .is_some_and(|state| {
                    state.config.is_some()
                        && (!require_live
                            || (state.shell.snapshot.is_some()
                                && state.shell.status == SyncStatus::Live))
                });
            if state.phase == Phase::Connected && available && self.session().is_some() {
                return Ok(());
            }
            if changes.next().await.is_none() {
                return Err(ClientError::Disconnected);
            }
        }
    }
}

/// Authorizes and reads config before returning. Poll the returned local task for shell sync, updates
/// and reconnects; dropping it releases its sockets, timers and subscriptions.
pub async fn connect<P: Platform + Clone + 'static>(
    platform: P,
    origin: &str,
    credential: ServerCredential,
) -> Result<(ConnectedClient, ConnectionTask), ClientError> {
    connect_into(platform, ClientStore::default(), origin, credential).await
}

/// [`connect`] into an existing store, so several environments share the app's one store.
pub async fn connect_into<P: Platform + Clone + 'static>(
    platform: P,
    store: ClientStore,
    origin: &str,
    credential: ServerCredential,
) -> Result<(ConnectedClient, ConnectionTask), ClientError> {
    let http_base = normalize_http_base_url(origin)?;
    let descriptor = fetch_remote_environment_descriptor(&platform, &http_base, None).await?;
    if let Some(error) = orchestration_protocol_compatibility_error(&descriptor) {
        return Err(ClientError::Unsupported(if error.server_update_required {
            "server update required"
        } else {
            "orchestration protocol"
        }));
    }
    let bearer = match credential {
        ServerCredential::Bearer(token) => token,
        ServerCredential::PairingToken(token) => Sensitive(
            bootstrap_remote_session(&platform, &http_base, token.expose(), None, None, None)
                .await
                .map_err(auth_failure)?
                .into_inner()
                .access_token
                .0,
        ),
    };
    if bearer.expose().trim().is_empty() {
        return Err(ClientError::InvalidPayload("credential is empty".into()));
    }
    let shared: Rc<dyn Platform> = Rc::new(platform.clone());
    let authorization = RemoteAuthorization::direct(shared.clone());
    let mut registry = EnvironmentRegistry::load(
        &platform,
        NetworkStatus::Unknown,
        Time::from_platform(&platform),
    )
    .await?;
    registry.bind_authorization(authorization.clone());
    let environment = descriptor.environment_id.clone();
    let route_id = format!(
        "bearer:{}:{}",
        environment.0,
        http_base.trim_end_matches('/')
    );
    let ws_base = derive_ws_base_url(&http_base)?;
    let route = Route {
        id: route_id.clone(),
        environment: environment.clone(),
        mode: ConnectionMode::Remote,
        http_base: http_base.clone(),
        ws_base: ws_base.clone(),
    };
    let change = registry
        .register_prepared(
            &platform,
            PreparedRegistration {
                registration: EnvironmentRegistration {
                    id: environment.clone(),
                    label: descriptor.label.0.clone(),
                    routes: vec![route],
                    enabled: true,
                },
                profile: ConnectionProfile::Bearer {
                    connection_id: route_id,
                    environment_id: environment.clone(),
                    label: descriptor.label.0,
                    http_base_url: http_base,
                    ws_base_url: ws_base,
                    learned: None,
                    authorization: None,
                    network: None,
                },
                credential: ConnectionCredential::Bearer { token: bearer },
            },
            false,
            true,
            NetworkStatus::Unknown,
            Time::from_platform(&platform),
        )
        .await?;
    let mut actions: VecDeque<_> = change
        .actions
        .into_iter()
        .filter(|(id, _)| id == &environment)
        .map(|(_, action)| action)
        .collect();
    actions.extend(
        registry
            .get_mut(&environment)
            .ok_or(ClientError::EnvironmentMismatch)?
            .handle(
                ConnectionEvent::Connect,
                Time::from_platform(&platform),
                0.0,
            ),
    );
    let (client, mut task) = start(
        platform,
        store,
        registry,
        environment,
        authorization,
        actions,
        None,
    );
    let initialized = {
        match futures::future::select(client.wait_ready(false).boxed_local(), task.as_mut()).await {
            futures::future::Either::Left((result, _)) => result,
            futures::future::Either::Right((result, _)) => {
                result.and(Err(ClientError::Disconnected))
            }
        }
    };
    initialized?;
    Ok((client, task))
}

/// Supervises an environment saved in the catalogue, with its saved route and credential,
/// without waiting for a connection: like TypeScript's registry, the supervisor retries with
/// backoff and publishes its state into the store. A disabled environment is not connected.
///
/// `authorization` is shared by every environment of one client, as TypeScript's
/// `RemoteEnvironmentAuthorization`; a relay route needs one built with a signer and relay.
///
/// `learn`, when given, saves the direct addresses the environment announces in its config.
pub async fn supervise_saved<P: Platform + Clone + 'static>(
    platform: P,
    store: ClientStore,
    authorization: RemoteAuthorization,
    environment: &EnvironmentId,
    learn: Option<LearnRoutes>,
) -> Result<(ConnectedClient, ConnectionTask), ClientError> {
    let mut registry = EnvironmentRegistry::load(
        &platform,
        NetworkStatus::Unknown,
        Time::from_platform(&platform),
    )
    .await?;
    registry.bind_authorization(authorization.clone());
    let handle = registry
        .get_mut(environment)
        .ok_or(ClientError::EnvironmentMismatch)?;
    let actions: VecDeque<_> = if handle.registration().enabled {
        handle
            .handle(
                ConnectionEvent::Connect,
                Time::from_platform(&platform),
                0.0,
            )
            .into()
    } else {
        VecDeque::new()
    };
    // A relay route mints for the signed-in account and this device's key, so the attempt's
    // identity carries both, as TypeScript's supervisor captures them.
    if !actions.is_empty() && authorization.dpop_signer().is_some() {
        registry
            .authorize_attempt(environment, authorization.clone())
            .await?;
    }
    Ok(start(
        platform,
        store,
        registry,
        environment.clone(),
        authorization,
        actions,
        learn,
    ))
}

fn start<P: Platform + Clone + 'static>(
    platform: P,
    store: ClientStore,
    registry: EnvironmentRegistry<PlatformSession>,
    environment: EnvironmentId,
    authorization: RemoteAuthorization,
    actions: VecDeque<ConnectionAction>,
    learn: Option<LearnRoutes>,
) -> (ConnectedClient, ConnectionTask) {
    let shared: Rc<dyn Platform> = Rc::new(platform.clone());
    let (events, receiver) = mpsc::unbounded();
    let (learned, learned_routes) = mpsc::unbounded();
    let (frames, frame_receiver) = mpsc::channel(1);
    let (abort, registration) = AbortHandle::new_pair();
    let (threads, thread_changes) = super::threads::ThreadDemand::new();
    let serial = store.claim_sync(crate::store::Scope::Environment(environment.clone()));
    let owner = Rc::new(Owner {
        environment: environment.clone(),
        store: store.clone(),
        events: events.clone(),
        endpoint: RefCell::new(None),
        http_base: RefCell::new(None),
        active_route: RefCell::new(None),
        settings_http: RefCell::new(None),
        store_session: Cell::new(None),
        threads: threads.clone(),
        abort,
        close_requested: Cell::new(false),
        serial,
    });
    let runtime = Runtime {
        host: platform.events(),
        platform,
        registry,
        environment,
        store,
        owner: Rc::downgrade(&owner),
        serial,
        resolver: ConnectionResolver::new(shared.clone(), authorization.clone()),
        factory: PlatformSessionFactory::new(shared, authorization).with_frames(frames),
        events,
        receiver,
        frames: frame_receiver,
        pending: actions,
        jobs: FuturesUnordered::new(),
        aborts: BTreeMap::new(),
        context: None,
        host_closed: false,
        threads,
        thread_changes,
        follows: Default::default(),
        learn,
        learned,
        learned_routes,
    };
    runtime.publish();
    let task: ConnectionTask = async move {
        Abortable::new(runtime.run(), registration)
            .await
            .unwrap_or(Ok(()))
    }
    .boxed_local();
    (ConnectedClient { owner }, task)
}

/// The direct addresses a server announces in its config, as `ReportedEndpoint`s.
fn reported_endpoints(
    config: &t3_contracts::server::ServerConfig,
) -> Option<Vec<ReportedEndpoint>> {
    let endpoints = config.direct_endpoints.as_ref()?;
    Some(
        endpoints
            .0
            .iter()
            .map(|endpoint| ReportedEndpoint {
                http_base_url: endpoint.http_base_url.0.clone(),
                kind: serde_json::to_value(endpoint.kind)
                    .ok()
                    .and_then(|kind| kind.as_str().map(str::to_owned)),
            })
            .collect(),
    )
}

fn auth_failure(error: crate::authorization::AuthorizationError) -> ClientError {
    ClientError::InvalidPayload(error.to_string())
}
pub(crate) fn session_failure(error: &ClientError) -> ConnectionFailure {
    let kind = match error {
        ClientError::PermissionDenied { .. } | ClientError::Platform(PlatformError::Denied(_)) => {
            FailureKind::Permission
        }
        ClientError::EnvironmentMismatch
        | ClientError::InvalidPayload(_)
        | ClientError::Codec(_) => FailureKind::Configuration,
        ClientError::Unsupported(_) => FailureKind::Unsupported,
        _ => FailureKind::Transport,
    };
    ConnectionFailure::new(kind, error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Establish(u64),
    Socket(u64),
    Shell(u64),
    Config(u64),
    Cache(u64),
    Probe(u64),
    Host(u64),
    Routes(u64),
    Thread(u64),
}
enum Outcome {
    Established(Result<Box<EnvironmentConnectionLease<PlatformSession>>, ConnectionFailure>),
    Ended(Result<(), ClientError>),
    Probe(Result<(), ConnectionFailure>),
    Host(bool),
    Routes(Vec<(String, RoutePreflight)>),
}
type Job = LocalBoxFuture<'static, (Key, Option<Outcome>)>;
type Sync<P> = ShellSync<P, PlatformSession, HttpSnapshotSource<P>>;
struct Context<P> {
    attempt: AttemptId,
    generation: Generation,
    store_session: Generation,
    route_id: String,
    prepared: crate::authorization::AuthorizedEnvironment,
    session: PlatformSession,
    sync: Rc<Sync<P>>,
}
struct Runtime<P: Platform + Clone + 'static> {
    platform: P,
    registry: EnvironmentRegistry<PlatformSession>,
    environment: EnvironmentId,
    store: ClientStore,
    owner: Weak<Owner>,
    serial: u64,
    resolver: ConnectionResolver,
    factory: PlatformSessionFactory,
    events: mpsc::UnboundedSender<ConnectionEvent>,
    receiver: mpsc::UnboundedReceiver<ConnectionEvent>,
    frames: mpsc::Receiver<ConnectionEvent>,
    host: LocalBoxStream<'static, PlatformEvent>,
    host_closed: bool,
    pending: VecDeque<ConnectionAction>,
    jobs: FuturesUnordered<Job>,
    aborts: BTreeMap<Key, AbortHandle>,
    context: Option<Context<P>>,
    threads: Rc<super::threads::ThreadDemand>,
    thread_changes: mpsc::UnboundedReceiver<()>,
    follows: super::threads::ThreadFollows,
    learn: Option<LearnRoutes>,
    /// Routes the config job learned, by the session generation that announced them.
    learned: mpsc::UnboundedSender<(u64, Vec<Route>)>,
    learned_routes: mpsc::UnboundedReceiver<(u64, Vec<Route>)>,
}
impl<P: Platform + Clone + 'static> Drop for Runtime<P> {
    fn drop(&mut self) {
        self.release();
    }
}
impl<P: Platform + Clone + 'static> Runtime<P> {
    fn publish(&self) {
        if let Some(handle) = self.registry.get(&self.environment) {
            self.store.publish_connection(
                &self.environment,
                self.serial,
                handle.supervisor().state().clone(),
            );
        }
    }
    fn random(&self) -> Result<f64, ClientError> {
        let bytes: [u8; 8] = self.platform.random_bytes(8)?.try_into().map_err(|_| {
            ClientError::InvalidPayload("host returned invalid random bytes".into())
        })?;
        Ok((u64::from_le_bytes(bytes) >> 11) as f64 / (1_u64 << 53) as f64)
    }
    fn feed(&mut self, event: ConnectionEvent) -> Result<(), ClientError> {
        let random = if matches!(
            event,
            ConnectionEvent::Failed { .. }
                | ConnectionEvent::Lost { .. }
                | ConnectionEvent::ProbeResult { result: Err(_), .. }
                | ConnectionEvent::Tick
        ) {
            self.random()?
        } else {
            0.0
        };
        let handle = self
            .registry
            .get_mut(&self.environment)
            .ok_or(ClientError::EnvironmentMismatch)?;
        self.pending
            .extend(handle.handle(event, Time::from_platform(&self.platform), random));
        self.publish();
        Ok(())
    }
    fn schedule(&mut self, key: Key, future: LocalBoxFuture<'static, Outcome>) {
        self.cancel(key);
        let (abort, registration) = AbortHandle::new_pair();
        self.aborts.insert(key, abort);
        self.jobs.push(
            async move { (key, Abortable::new(future, registration).await.ok()) }.boxed_local(),
        );
    }
    fn cancel(&mut self, key: Key) {
        if let Some(abort) = self.aborts.remove(&key) {
            abort.abort();
        }
    }
    fn release(&mut self) {
        for slot in self.follows.clear() {
            self.cancel(Key::Thread(slot));
        }
        if let Some(context) = self.context.take() {
            context.session.invalidate();
            self.store
                .disconnect(&self.environment, context.store_session);
            for key in [
                Key::Socket(context.generation.0),
                Key::Shell(context.generation.0),
                Key::Config(context.generation.0),
                Key::Cache(context.generation.0),
            ] {
                self.cancel(key);
            }
        }
        if let Some(owner) = self.owner.upgrade() {
            owner.endpoint.borrow_mut().take();
            owner.http_base.borrow_mut().take();
            owner.active_route.borrow_mut().take();
            owner.settings_http.borrow_mut().take();
            owner.store_session.set(None);
        }
    }
    fn driver(
        &self,
        identity: crate::environment::SessionIdentity,
        progress: Rc<dyn Fn(AttemptStage)>,
    ) -> ResolvedConnectionDriver<PlatformSessionFactory> {
        ResolvedConnectionDriver::new(
            self.resolver.clone(),
            self.factory.clone(),
            identity,
            progress,
        )
    }
    fn sync(
        &self,
        store_session: Generation,
        session: &PlatformSession,
        prepared: &crate::authorization::AuthorizedEnvironment,
    ) -> Rc<Sync<P>> {
        Rc::new(ShellSync::new(
            self.store.clone(),
            self.environment.clone(),
            store_session,
            Rc::new(EnvironmentCacheStore::new(self.platform.clone())),
            Rc::new(session.clone()),
            self.snapshot_source(prepared),
            None,
        ))
    }
    fn snapshot_source(
        &self,
        prepared: &crate::authorization::AuthorizedEnvironment,
    ) -> Rc<HttpSnapshotSource<P>> {
        Rc::new(HttpSnapshotSource {
            platform: Rc::new(self.platform.clone()),
            client: crate::permissions::EnvironmentHttpClient::new(
                Rc::new(self.platform.clone()),
                prepared.clone(),
                None,
                None,
                false,
            ),
        })
    }
    /// Follows the leased threads in the current session context; without one, nothing runs and
    /// the leases are read again when the next context starts.
    fn follow_threads(&mut self) {
        let Some(context) = &self.context else {
            return;
        };
        let plan = self.follows.reconcile(&self.threads.open());
        let jobs: Vec<_> = plan
            .start
            .into_iter()
            .map(|(thread, slot)| {
                let sync = Rc::new(ThreadSync::new(
                    self.store.clone(),
                    ThreadKey {
                        environment: self.environment.clone(),
                        thread,
                    },
                    context.store_session,
                    Rc::new(EnvironmentCacheStore::new(self.platform.clone())),
                    Rc::new(context.session.clone()),
                    self.snapshot_source(&context.prepared),
                    true,
                ));
                if let Some(config) = context.session.config() {
                    sync.configure(&config);
                }
                let job = async move {
                    let (result, _) =
                        futures::future::join(sync.follow(), sync.persist_changes()).await;
                    Outcome::Ended(result)
                }
                .boxed_local();
                (slot, job)
            })
            .collect();
        for slot in plan.stop {
            self.cancel(Key::Thread(slot));
        }
        for (slot, job) in jobs {
            self.schedule(Key::Thread(slot), job);
        }
    }
    fn follow_shell(&mut self) {
        let Some(context) = &self.context else {
            return;
        };
        let generation = context.generation.0;
        let sync = context.sync.clone();
        self.schedule(
            Key::Shell(generation),
            async move { Outcome::Ended(sync.follow().await) }.boxed_local(),
        );
        let sync = self.context.as_ref().map(|context| context.sync.clone());
        if let Some(sync) = sync {
            self.schedule(
                Key::Cache(generation),
                async move { Outcome::Ended(sync.persist_changes().await) }.boxed_local(),
            );
        }
    }
    fn start_context(
        &mut self,
        attempt: AttemptId,
        lease: EnvironmentConnectionLease<PlatformSession>,
    ) -> Result<(), ClientError> {
        let handle = self
            .registry
            .get(&self.environment)
            .ok_or(ClientError::EnvironmentMismatch)?;
        if handle.supervisor().active_attempt() != Some(attempt)
            || handle.pending_identity().as_ref() != Some(&lease.prepared.session)
        {
            return Ok(());
        }
        self.release();
        let generation = lease.prepared.session.generation;
        let store_session = self.store.begin_session(self.environment.clone());
        let config = lease.session.config().ok_or(ClientError::Disconnected)?;
        if !self
            .store
            .set_config(&self.environment, store_session, config.as_ref().clone())
        {
            return Err(ClientError::EnvironmentMismatch);
        }
        let sync = self.sync(store_session, &lease.session, &lease.prepared);
        self.context = Some(Context {
            attempt,
            generation,
            store_session,
            route_id: lease.route_id,
            prepared: lease.prepared,
            session: lease.session.clone(),
            sync,
        });
        if let Some(owner) = self.owner.upgrade() {
            owner.store_session.set(Some(store_session));
        }
        let socket = lease.session.clone();
        self.schedule(
            Key::Socket(generation.0),
            async move {
                socket.run().await;
                Outcome::Ended(Err(ClientError::Disconnected))
            }
            .boxed_local(),
        );
        let store = self.store.clone();
        let environment = self.environment.clone();
        let endpoint = lease.session.endpoint();
        let platform = self.platform.clone();
        let learn = self.learn.clone().zip(
            self.context
                .as_ref()
                .map(|context| (context.route_id.clone(), self.learned.clone())),
        );
        self.schedule(
            Key::Config(generation.0),
            async move {
                let input = match serde_json::from_value(serde_json::json!({
                    "environmentThemes": true, "usageLimitSources": true,
                    "usageLimitsCommand": true, "workspaceSnapshotsOnDemand": true,
                })) {
                    Ok(input) => input,
                    Err(error) => return Outcome::Ended(Err(error.into())),
                };
                // A failed request on a healthy socket resubscribes on it (WP5's policy); the
                // socket job ending is what replaces the session.
                let mut stream = endpoint
                    .subscribe_recovering::<t3_contracts::rpc::WsSubscribeServerConfigRpc, _>(
                        &platform,
                        input,
                        Default::default(),
                    );
                let mut current = None;
                let mut announced = None;
                while let Some(event) = stream.next().await {
                    let event = match event {
                        Ok(event) => event,
                        Err(error) => return Outcome::Ended(Err(error)),
                    };
                    match crate::services::config::apply_server_config_projection(
                        current.as_ref(),
                        event,
                    ) {
                        Ok(Some(next)) => {
                            if !store.set_config(
                                &environment,
                                store_session,
                                next.config.as_ref().clone(),
                            ) {
                                return Outcome::Ended(Err(ClientError::Cancelled));
                            }
                            let reported = reported_endpoints(&next.config);
                            if let Some((learn, (route_id, learned))) = &learn
                                && let Some(reported) = reported
                                && announced.as_ref() != Some(&reported)
                            {
                                announced = Some(reported.clone());
                                if let Some(routes) =
                                    learn(environment.clone(), route_id.clone(), reported).await
                                {
                                    let _ = learned.unbounded_send((generation.0, routes));
                                }
                            }
                            current = Some(next);
                        }
                        Ok(None) => {}
                        Err(error) => return Outcome::Ended(Err(error)),
                    }
                }
                Outcome::Ended(Err(ClientError::Disconnected))
            }
            .boxed_local(),
        );
        let context = self.context.as_ref().ok_or(ClientError::Disconnected)?;
        let handle = self
            .registry
            .get_mut(&self.environment)
            .ok_or(ClientError::EnvironmentMismatch)?;
        self.pending.extend(handle.install_authorized(
            context.attempt,
            &context.route_id,
            context.prepared.clone(),
            context.session.clone(),
            Time::from_platform(&self.platform),
        )?);
        if let Some(config) = context.session.config() {
            handle.set_config(context.generation, config.as_ref().clone());
        }
        if let Some(owner) = self.owner.upgrade() {
            owner.endpoint.replace(Some(context.session.endpoint()));
            owner.active_route.replace(Some(context.route_id.clone()));
            owner
                .settings_http
                .replace(Some(crate::permissions::EnvironmentHttpClient::new(
                    Rc::new(self.platform.clone()),
                    context.prepared.clone(),
                    self.resolver.remote.dpop_signer(),
                    Some(self.resolver.remote.clone()),
                    handle
                        .registration()
                        .routes
                        .iter()
                        .any(|r| r.id == context.route_id && r.mode == ConnectionMode::Relay),
                )));
            owner
                .http_base
                .replace(Some(context.prepared.http_base_url.clone()));
        }
        self.feed(ConnectionEvent::ConfigReady { generation })?;
        self.follow_shell();
        self.follow_threads();
        Ok(())
    }
    fn action(&mut self, action: ConnectionAction) -> Result<(), ClientError> {
        match action {
            ConnectionAction::Establish {
                attempt, routes, ..
            } => {
                let identity = self
                    .registry
                    .get(&self.environment)
                    .and_then(|handle| handle.pending_identity())
                    .ok_or(ClientError::Cancelled)?;
                let sender = self.events.clone();
                let driver = self.driver(
                    identity,
                    Rc::new(move |stage| {
                        let _ = sender.unbounded_send(ConnectionEvent::Progress { attempt, stage });
                    }),
                );
                self.schedule(
                    Key::Establish(attempt.0),
                    async move { Outcome::Established(driver.connect(routes).await.map(Box::new)) }
                        .boxed_local(),
                );
            }
            ConnectionAction::CancelAttempt { attempt } => {
                self.cancel(Key::Establish(attempt.0));
                if self
                    .context
                    .as_ref()
                    .is_some_and(|context| context.attempt == attempt)
                    && !self
                        .registry
                        .get(&self.environment)
                        .is_some_and(|handle| handle.supervisor().state().phase == Phase::Connected)
                {
                    self.release();
                }
            }
            ConnectionAction::Release { generation } => {
                if self
                    .context
                    .as_ref()
                    .is_some_and(|context| context.generation == generation)
                {
                    self.release();
                }
            }
            ConnectionAction::Probe { generation, probe } => {
                if let Some(context) = &self.context
                    && context.generation == generation
                {
                    let endpoint = context.session.endpoint();
                    self.schedule(
                        Key::Probe(probe.0),
                        async move {
                            Outcome::Probe(
                                endpoint
                                    .probe()
                                    .await
                                    .map_err(|error| session_failure(&error)),
                            )
                        }
                        .boxed_local(),
                    );
                }
            }
            ConnectionAction::CancelProbe { probe } => self.cancel(Key::Probe(probe.0)),
            ConnectionAction::CancelHostCheck { check } => self.cancel(Key::Host(check.0)),
            ConnectionAction::CancelRouteCheck { check } => self.cancel(Key::Routes(check.0)),
            ConnectionAction::CheckHost { generation, check } => {
                if let Some(context) = &self.context
                    && context.generation == generation
                {
                    let prepared = context.prepared.clone();
                    let driver = self.driver(prepared.session.clone(), Rc::new(|_| {}));
                    self.schedule(
                        Key::Host(check.0),
                        async move {
                            Outcome::Host(
                                driver.check_host(&prepared).await == RouteCheck::Answered,
                            )
                        }
                        .boxed_local(),
                    );
                }
            }
            ConnectionAction::CheckRoutes {
                generation,
                check,
                routes,
            } => {
                if let Some(context) = &self.context
                    && context.generation == generation
                {
                    let all = self
                        .registry
                        .get(&self.environment)
                        .map(|handle| handle.registration().routes.clone())
                        .unwrap_or_default();
                    let driver = self.driver(context.prepared.session.clone(), Rc::new(|_| {}));
                    self.schedule(
                        Key::Routes(check.0),
                        async move {
                            Outcome::Routes(
                                futures::future::join_all(routes.iter().map(|route| async {
                                    (route.id.clone(), driver.preflight(&all, route, false).await)
                                }))
                                .await,
                            )
                        }
                        .boxed_local(),
                    );
                }
            }
            ConnectionAction::Resubscribe { generation } => {
                if let Some(context) = &self.context
                    && context.generation == generation
                {
                    let sync =
                        self.sync(context.store_session, &context.session, &context.prepared);
                    if let Some(context) = &mut self.context {
                        context.sync = sync;
                    }
                    self.follow_shell();
                    for slot in self.follows.clear() {
                        self.cancel(Key::Thread(slot));
                    }
                    self.follow_threads();
                }
            }
        }
        Ok(())
    }
    fn complete(&mut self, key: Key, outcome: Outcome) -> Result<(), ClientError> {
        match (key, outcome) {
            (Key::Establish(id), Outcome::Established(Ok(lease))) => {
                self.start_context(AttemptId(id), *lease)?
            }
            (Key::Establish(id), Outcome::Established(Err(failure))) => {
                self.feed(ConnectionEvent::Failed {
                    attempt: AttemptId(id),
                    failure,
                })?
            }
            (
                Key::Socket(generation)
                | Key::Shell(generation)
                | Key::Config(generation)
                | Key::Cache(generation),
                Outcome::Ended(result),
            ) => {
                if let Some(context) = &self.context
                    && context.generation.0 == generation
                {
                    let failure =
                        session_failure(&result.err().unwrap_or(ClientError::Disconnected));
                    let event = if self.registry.get(&self.environment).is_some_and(|handle| {
                        handle.supervisor().accepts_session(context.generation)
                    }) {
                        ConnectionEvent::Lost {
                            generation: context.generation,
                            failure,
                        }
                    } else {
                        ConnectionEvent::Failed {
                            attempt: context.attempt,
                            failure,
                        }
                    };
                    self.feed(event)?;
                }
            }
            (Key::Probe(id), Outcome::Probe(result)) => {
                self.feed(ConnectionEvent::ProbeResult {
                    probe: ProbeId(id),
                    result,
                })?
            }
            (Key::Host(id), Outcome::Host(answered)) => self.feed(ConnectionEvent::HostResult {
                check: HostCheckId(id),
                answered,
            })?,
            (Key::Routes(id), Outcome::Routes(results)) => {
                self.feed(ConnectionEvent::RoutesChecked {
                    check: RouteCheckId(id),
                    results,
                })?
            }
            _ => {}
        }
        self.publish();
        Ok(())
    }
    fn finish_job(&mut self, key: Key, outcome: Option<Outcome>) -> Result<(), ClientError> {
        if let Some(outcome) = outcome {
            self.aborts.remove(&key);
            self.complete(key, outcome)?;
        }
        Ok(())
    }
    async fn run(mut self) -> Result<(), ClientError> {
        loop {
            let disconnecting = self
                .registry
                .get(&self.environment)
                .is_none_or(|handle| !handle.supervisor().state().desired);
            // Release clears the context and invalidates all of its RPC handles. Keep
            // just the session long enough to send the intentional close.
            let closing = if disconnecting {
                self.context.as_ref().map(|context| context.session.clone())
            } else {
                None
            };
            while let Some(action) = self.pending.pop_front() {
                self.action(action)?;
            }
            if disconnecting {
                self.release();
                if let Some(session) = closing {
                    session.close().await;
                }
                return Ok(());
            }
            enum Input {
                Event(Option<ConnectionEvent>),
                Host(Option<PlatformEvent>),
                Job(Option<(Key, Option<Outcome>)>),
                Threads,
                Timer,
                Learned(Option<(u64, Vec<Route>)>),
            }
            let deadline = self
                .registry
                .get(&self.environment)
                .and_then(|handle| handle.supervisor().next_deadline());
            let input = {
                let timer = match deadline {
                    Some(deadline) => self
                        .platform
                        .sleep(deadline.saturating_sub(self.platform.monotonic_ms())),
                    None => futures::future::pending().boxed_local(),
                }
                .fuse();
                let host = if self.host_closed {
                    futures::future::pending().boxed_local()
                } else {
                    self.host.next().boxed_local()
                }
                .fuse();
                let job = if self.jobs.is_empty() {
                    futures::future::pending().boxed_local()
                } else {
                    self.jobs.next().boxed_local()
                }
                .fuse();
                let event = self.receiver.next().fuse();
                let frame = self.frames.next().fuse();
                // The client keeps the sender alive, so this ends only after the client dropped.
                let threads = self.thread_changes.next().fuse();
                let learned = self.learned_routes.next().fuse();
                futures::pin_mut!(timer, host, job, event, frame, threads, learned);
                futures::select_biased! { event = event => Input::Event(event), job = job => Input::Job(job), frame = frame => Input::Event(frame), _ = threads => Input::Threads, learned = learned => Input::Learned(learned), host = host => Input::Host(host), _ = timer => Input::Timer }
            };
            match input {
                Input::Event(Some(event)) => self.feed(event)?,
                Input::Event(None) => self.feed(ConnectionEvent::Disconnect)?,
                Input::Timer => self.feed(ConnectionEvent::Tick)?,
                Input::Learned(None) => {}
                Input::Learned(Some((generation, routes))) => {
                    if self
                        .context
                        .as_ref()
                        .is_some_and(|context| context.generation.0 == generation)
                    {
                        let change = self.registry.set_routes(
                            &self.environment,
                            routes,
                            Time::from_platform(&self.platform),
                        )?;
                        self.pending
                            .extend(change.actions.into_iter().map(|(_, action)| action));
                        self.publish();
                    }
                }
                Input::Host(None) => self.host_closed = true,
                Input::Host(Some(event)) => {
                    let random = self.random()?;
                    if let Some(handle) = self.registry.get_mut(&self.environment) {
                        self.pending.extend(handle.platform_event(
                            event,
                            Time::from_platform(&self.platform),
                            random,
                        ));
                    }
                    self.publish();
                }
                Input::Job(Some((key, outcome))) => {
                    self.finish_job(key, outcome)?;
                }
                Input::Job(None) => {}
                Input::Threads => self.follow_threads(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakePlatform;

    #[test]
    fn resubscribed_work_stays_cancellable_when_the_previous_job_finishes() {
        let platform = FakePlatform::default();
        let shared: Rc<dyn Platform> = Rc::new(platform.clone());
        let authorization = RemoteAuthorization::direct(shared.clone());
        let (events, receiver) = mpsc::unbounded();
        let (_, frames) = mpsc::channel(1);
        let (learned, learned_routes) = mpsc::unbounded();
        let mut runtime = Runtime {
            host: platform.events(),
            platform,
            registry: EnvironmentRegistry::default(),
            environment: EnvironmentId("env".into()),
            store: ClientStore::default(),
            owner: Weak::new(),
            serial: 0,
            resolver: ConnectionResolver::new(shared.clone(), authorization.clone()),
            factory: PlatformSessionFactory::new(shared, authorization),
            events,
            receiver,
            frames,
            pending: VecDeque::new(),
            jobs: FuturesUnordered::new(),
            aborts: BTreeMap::new(),
            context: None,
            host_closed: false,
            threads: super::super::threads::ThreadDemand::new().0,
            thread_changes: mpsc::unbounded().1,
            follows: Default::default(),
            learn: None,
            learned,
            learned_routes,
        };
        let key = Key::Shell(1);
        runtime.schedule(key, futures::future::pending().boxed_local());
        runtime.schedule(key, futures::future::pending().boxed_local());
        let Some((retired, outcome)) = runtime.jobs.next().now_or_never().flatten() else {
            panic!("replaced work must finish when aborted");
        };
        assert!(outcome.is_none());
        assert!(runtime.finish_job(retired, outcome).is_ok());
        runtime.cancel(key);
        assert!(matches!(
            runtime.jobs.next().now_or_never(),
            Some(Some((_, None)))
        ));
    }
}
