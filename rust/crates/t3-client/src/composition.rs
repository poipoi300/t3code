//! The composition root: the one object a UI holds.
//!
//! [`ClientRuntime`] owns the saved environments' connections in the app's one [`ClientStore`],
//! the threads screens have open, and the per-environment message delivery. Screens read the
//! store and call these methods; they never touch connections, sessions or credentials. The
//! crate owns no executor, so the UI supplies a spawner for the futures it must keep running.
use crate::{
    ClientError, ClientStore,
    connection::{
        ConnectedClient, ConnectionFailure, ConnectionState, ConnectionStores, ConnectionTask,
        PairingInput, Phase, ServerCredential, ThreadLease, connect_into, resolve_pairing_target,
        runtime::session_failure,
    },
    environment::EnvironmentRegistry,
    platform::{NetworkStatus, Platform},
    rpc::SessionEndpoint,
    store::Scope,
    thread_views::composer::send::SendService,
    types::{EnvironmentId, Generation, ThreadKey},
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{AbortHandle, Abortable, LocalBoxFuture},
    stream::LocalBoxStream,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

/// Runs a future on the UI's executor until it finishes or is dropped by its owner.
pub type Spawner = Rc<dyn Fn(LocalBoxFuture<'static, ()>)>;

/// One saved environment as the connection layer sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentEntry {
    pub id: EnvironmentId,
    pub label: String,
    pub state: ConnectionState,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// The environment has no live session; the message was not accepted.
    #[error("The environment is not connected.")]
    Disconnected,
    /// The client's validation or the server refused it, with the client's English reason.
    #[error("{0}")]
    Rejected(String),
}

struct Environment<P: Platform + 'static> {
    id: EnvironmentId,
    label: String,
    client: Option<ConnectedClient>,
    send: Option<SendService<P, SessionEndpoint>>,
    /// Why the last connection attempt failed before a client existed.
    failure: Option<ConnectionFailure>,
    /// Ends this environment's delivery loop and session binding.
    tasks: Vec<AbortHandle>,
    /// Resolves when the connection driver has finished.
    finished: Option<futures::channel::oneshot::Receiver<()>>,
}

struct Inner<P: Platform + Clone + 'static> {
    platform: P,
    store: ClientStore,
    source: OrchestrationV2CreationSource,
    spawn: Spawner,
    environments: RefCell<Vec<Environment<P>>>,
    /// Open threads: how many opens each has, and its lease once its environment is connected.
    threads: RefCell<BTreeMap<ThreadKey, (usize, Option<ThreadLease>)>>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
}

impl<P: Platform + Clone + 'static> Drop for Inner<P> {
    fn drop(&mut self) {
        self.threads.get_mut().clear();
        for environment in self.environments.get_mut().drain(..) {
            drop(shut_down(environment));
        }
    }
}

/// Stops everything the environment runs; returns the signal of its driver finishing.
fn shut_down<P: Platform + Clone + 'static>(
    environment: Environment<P>,
) -> Option<futures::channel::oneshot::Receiver<()>> {
    for task in environment.tasks {
        task.abort();
    }
    if let Some(send) = environment.send {
        send.dispose();
    }
    if let Some(client) = environment.client {
        let _ = client.close();
    }
    environment.finished
}

#[derive(Clone)]
pub struct ClientRuntime<P: Platform + Clone + 'static> {
    inner: Rc<Inner<P>>,
}

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    /// `source` names this client in the threads it creates (web, mobile).
    pub fn new(
        platform: P,
        store: ClientStore,
        source: OrchestrationV2CreationSource,
        spawn: Spawner,
    ) -> Self {
        Self {
            inner: Rc::new(Inner {
                platform,
                store,
                source,
                spawn,
                environments: RefCell::new(Vec::new()),
                threads: RefCell::new(BTreeMap::new()),
                listeners: RefCell::new(Vec::new()),
            }),
        }
    }

    pub fn store(&self) -> &ClientStore {
        &self.inner.store
    }

    /// Reconnects every environment saved by an earlier run, with its saved credential, in
    /// catalogue order. An environment that cannot connect stays listed with its failure.
    pub fn restore(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        async move {
            let platform: Rc<dyn Platform> = Rc::new(this.inner.platform.clone());
            let registry: EnvironmentRegistry<SessionEndpoint> = EnvironmentRegistry::load(
                platform.as_ref(),
                NetworkStatus::Unknown,
                crate::connection::Time::from_platform(&this.inner.platform),
            )
            .await?;
            let saved: Vec<_> = registry
                .entries()
                .filter_map(|registration| {
                    let route = registration.routes.first()?;
                    Some((
                        registration.id.clone(),
                        registration.label.clone(),
                        route.id.clone(),
                        route.http_base.clone(),
                    ))
                })
                .collect();
            let stores = ConnectionStores::new(platform);
            for (id, label, route, origin) in saved {
                this.list(id.clone(), label);
                let credential = match stores.credential(&id, &route).await {
                    Ok(Some(credential)) => credential,
                    Ok(None) => {
                        this.fail(&id, crate::connection::credential_missing_error(&route));
                        continue;
                    }
                    Err(failure) => {
                        this.fail(&id, failure);
                        continue;
                    }
                };
                let result = connect_into(
                    this.inner.platform.clone(),
                    this.inner.store.clone(),
                    &origin,
                    ServerCredential::Bearer(credential.token().clone()),
                )
                .await;
                match result {
                    Ok((client, driver)) => this.attach(client, driver, &origin),
                    Err(error) => this.fail(&id, session_failure(&error)),
                }
            }
            Ok(())
        }
        .boxed_local()
    }

    /// Pairs with the server the user described and saves it. Validation and its messages are
    /// the client's (`connection::resolve_pairing_target`).
    pub fn pair(
        &self,
        input: PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>> {
        let this = self.clone();
        async move {
            let target = resolve_pairing_target(&input)?;
            let (client, driver) = connect_into(
                this.inner.platform.clone(),
                this.inner.store.clone(),
                &target.http_base,
                ServerCredential::PairingToken(target.credential),
            )
            .await
            .map_err(|error| session_failure(&error))?;
            let id = client.environment().clone();
            this.attach(client, driver, &target.http_base);
            Ok(id)
        }
        .boxed_local()
    }

    /// Disconnects `environment`, ends its follows and delivery, removes its saved
    /// registration and credentials, and drops its data from the store.
    pub fn remove(
        &self,
        environment: &EnvironmentId,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        let environment = environment.clone();
        async move {
            this.inner
                .threads
                .borrow_mut()
                .retain(|key, _| key.environment != environment);
            let removed = {
                let mut environments = this.inner.environments.borrow_mut();
                environments
                    .iter()
                    .position(|entry| entry.id == environment)
                    .map(|index| environments.remove(index))
            };
            // The driver publishes its last connection state while it stops; clear the store after.
            if let Some(finished) = removed.and_then(shut_down) {
                let _ = finished.await;
            }
            this.inner.store.remove_environment(&environment);
            this.notify();
            let platform = this.inner.platform.clone();
            let mut registry: EnvironmentRegistry<SessionEndpoint> = EnvironmentRegistry::load(
                &platform,
                NetworkStatus::Unknown,
                crate::connection::Time::from_platform(&platform),
            )
            .await?;
            if registry.get(&environment).is_some() {
                registry.remove(
                    &environment,
                    crate::connection::Time::from_platform(&platform),
                )?;
                registry.save(&platform).await?;
            }
            Ok(())
        }
        .boxed_local()
    }

    /// The saved environments, in catalogue order.
    pub fn environments(&self) -> Vec<EnvironmentEntry> {
        self.inner
            .environments
            .borrow()
            .iter()
            .map(|entry| EnvironmentEntry {
                id: entry.id.clone(),
                label: entry.label.clone(),
                state: match (&entry.client, &entry.failure) {
                    (Some(client), _) => client.state(),
                    (None, failure) => ConnectionState {
                        phase: Phase::Blocked,
                        last_failure: failure.clone(),
                        ..ConnectionState::default()
                    },
                },
            })
            .collect()
    }

    /// Fires whenever [`Self::environments`] may answer differently: the list changed or an
    /// environment's connection state changed. Coalesces; read `environments` on each item.
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.inner.listeners.borrow_mut().push(sender);
        let connections = self
            .inner
            .store
            .subscribe()
            .filter_map(|change| async move {
                change
                    .scopes
                    .iter()
                    .any(|scope| matches!(scope, Scope::All | Scope::Environment(_)))
                    .then_some(())
            });
        futures::stream::select(receiver, connections).boxed_local()
    }

    /// Follows a thread while a screen shows it: snapshot, then live events, on every session of
    /// its environment. Opens are counted; [`Self::close_thread`] undoes one.
    pub fn open_thread(&self, key: &ThreadKey) {
        let lease = {
            let mut threads = self.inner.threads.borrow_mut();
            let entry = threads.entry(key.clone()).or_insert((0, None));
            entry.0 += 1;
            if entry.0 > 1 {
                return;
            }
            self.client(&key.environment)
                .map(|client| client.open_thread(key.thread.clone()))
        };
        if let Some(lease) = lease
            && let Some(entry) = self.inner.threads.borrow_mut().get_mut(key)
        {
            entry.1 = Some(lease);
        }
    }

    /// Stops following a thread no screen shows. Its projection stays readable in the store.
    pub fn close_thread(&self, key: &ThreadKey) {
        let released = {
            let mut threads = self.inner.threads.borrow_mut();
            match threads.get_mut(key) {
                Some(entry) if entry.0 > 1 => {
                    entry.0 -= 1;
                    None
                }
                Some(_) => threads.remove(key),
                None => None,
            }
        };
        drop(released);
    }

    /// Sends a user message as a new turn on `key`, through the durable outbox. Accepted means
    /// written to the outbox; delivery and its outcome follow in the store and `SendService`.
    pub fn send_message(
        &self,
        key: &ThreadKey,
        text: String,
    ) -> LocalBoxFuture<'static, Result<(), SendError>> {
        let key = key.clone();
        let service = self
            .inner
            .environments
            .borrow()
            .iter()
            .find(|entry| entry.id == key.environment)
            .filter(|entry| {
                entry
                    .client
                    .as_ref()
                    .is_some_and(|client| client.session().is_some())
            })
            .and_then(|entry| entry.send.clone());
        async move {
            if text.trim().is_empty() {
                return Err(SendError::Rejected("Enter a message.".into()));
            }
            let service = service.ok_or(SendError::Disconnected)?;
            match service.send_message(&key, text).await {
                Ok(_) => Ok(()),
                Err(ClientError::Disconnected) => Err(SendError::Disconnected),
                Err(error) => Err(SendError::Rejected(error.to_string())),
            }
        }
        .boxed_local()
    }

    /// The message delivery for `environment`, for pending messages and their states.
    pub fn messages(&self, environment: &EnvironmentId) -> Option<SendService<P, SessionEndpoint>> {
        self.inner
            .environments
            .borrow()
            .iter()
            .find(|entry| &entry.id == environment)
            .and_then(|entry| entry.send.clone())
    }

    /// The connected client for `environment`, for services that need its guarded session.
    pub fn client(&self, environment: &EnvironmentId) -> Option<ConnectedClient> {
        self.inner
            .environments
            .borrow()
            .iter()
            .find(|entry| &entry.id == environment)
            .and_then(|entry| entry.client.clone())
    }

    fn notify(&self) {
        self.inner
            .listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(()).is_ok());
    }

    fn list(&self, id: EnvironmentId, label: String) {
        let mut environments = self.inner.environments.borrow_mut();
        if !environments.iter().any(|entry| entry.id == id) {
            environments.push(Environment {
                id,
                label,
                client: None,
                send: None,
                failure: None,
                tasks: Vec::new(),
                finished: None,
            });
        }
        drop(environments);
        self.notify();
    }

    fn fail(&self, id: &EnvironmentId, failure: ConnectionFailure) {
        if let Some(entry) = self
            .inner
            .environments
            .borrow_mut()
            .iter_mut()
            .find(|entry| &entry.id == id)
        {
            entry.failure = Some(failure);
        }
        self.notify();
    }

    fn spawn(&self, future: LocalBoxFuture<'static, ()>) -> AbortHandle {
        let (abort, registration) = AbortHandle::new_pair();
        (self.inner.spawn)(
            Abortable::new(future, registration)
                .map(|_| ())
                .boxed_local(),
        );
        abort
    }

    /// Installs a connected environment: its driver, its message delivery and the binding of
    /// delivery to each new session, and the leases of threads already open on it.
    fn attach(&self, client: ConnectedClient, driver: ConnectionTask, origin: &str) {
        let id = client.environment().clone();
        let label = self
            .inner
            .store
            .snapshot()
            .environments
            .get(&id)
            .and_then(|state| state.config.as_ref())
            .map_or_else(
                || origin.to_owned(),
                |config| config.environment.label.0.clone(),
            );
        let (done, finished) = futures::channel::oneshot::channel();
        (self.inner.spawn)(
            async move {
                let _ = driver.await;
                let _ = done.send(());
            }
            .boxed_local(),
        );
        let send = SendService::new(
            id.clone(),
            Rc::new(self.inner.platform.clone()),
            self.inner.store.clone(),
            self.inner.source.clone(),
        );
        let delivery = send.run();
        let tasks = vec![
            self.spawn(
                async move {
                    let _ = delivery.await;
                }
                .boxed_local(),
            ),
            self.spawn(bind_sessions(
                self.inner.store.clone(),
                client.clone(),
                send.clone(),
                origin.to_owned(),
            )),
        ];
        let replaced = {
            let mut environments = self.inner.environments.borrow_mut();
            let entry = Environment {
                id: id.clone(),
                label,
                client: Some(client.clone()),
                send: Some(send),
                failure: None,
                tasks,
                finished: Some(finished),
            };
            match environments.iter_mut().find(|entry| entry.id == id) {
                Some(existing) => Some(std::mem::replace(existing, entry)),
                None => {
                    environments.push(entry);
                    None
                }
            }
        };
        if let Some(replaced) = replaced {
            drop(shut_down(replaced));
        }
        for (key, (_, lease)) in self.inner.threads.borrow_mut().iter_mut() {
            if key.environment == id {
                *lease = Some(client.open_thread(key.thread.clone()));
            }
        }
        self.notify();
    }
}

/// Binds message delivery to each new session of `client`, and unbinds it while there is none.
fn bind_sessions<P: Platform + 'static>(
    store: ClientStore,
    client: ConnectedClient,
    send: SendService<P, SessionEndpoint>,
    origin: String,
) -> LocalBoxFuture<'static, ()> {
    // Holds the client until the composition root aborts this task (removal, replacement, drop).
    async move {
        let mut changes = store.subscribe_environment(client.environment());
        let mut bound: Option<Generation> = None;
        while changes.next().await.is_some() {
            let environment = client.environment().clone();
            let current = store
                .snapshot()
                .environments
                .get(&environment)
                .and_then(|state| Some((state.session, state.config.clone()?, client.session()?)));
            match current {
                Some((generation, config, endpoint)) if bound != Some(generation) => {
                    if send
                        .connect_with_upload_base(generation, Rc::new(endpoint), config, &origin)
                        .is_ok()
                    {
                        bound = Some(generation);
                    }
                }
                None if bound.is_some() => {
                    send.disconnect();
                    bound = None;
                }
                _ => {}
            }
        }
    }
    .boxed_local()
}
