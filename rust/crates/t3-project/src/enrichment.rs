use crate::{Result, identity::RepositoryIdentityResolver};
use async_trait::async_trait;
use futures_util::FutureExt;
use std::{
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
    sync::Arc,
    time::Duration,
};
use t3_contracts::environment::RepositoryIdentity;
use t3_workspace::project::FaviconResolver;
use tokio::sync::{Mutex, Semaphore, broadcast, watch};
use tokio::time::Instant;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, Clone, Default)]
pub struct ProjectEnrichment {
    pub repository_identity: Option<RepositoryIdentity>,
    pub favicon_path: Option<String>,
    pub repository_identity_resolved: bool,
}
#[derive(Debug, Clone)]
pub struct EnrichmentChange {
    pub workspace_root: String,
    pub enrichment: ProjectEnrichment,
}
#[derive(Clone, Copy)]
pub struct EnrichmentOptions {
    pub cache_capacity: usize,
    pub max_pending: usize,
    pub concurrency: usize,
}
impl Default for EnrichmentOptions {
    fn default() -> Self {
        Self {
            cache_capacity: 512,
            max_pending: 512,
            concurrency: 4,
        }
    }
}
#[async_trait]
pub trait ProjectMetadata: Send + Sync {
    async fn repository_identity(
        &self,
        root: &str,
        refresh: bool,
    ) -> Result<Option<RepositoryIdentity>>;
    async fn favicon(&self, root: &str) -> Result<Option<String>>;
    async fn invalidate_favicon(&self, root: &str);
}
pub struct NativeProjectMetadata {
    identity: RepositoryIdentityResolver,
    favicon: Arc<FaviconResolver>,
}
impl NativeProjectMetadata {
    pub fn new(identity: RepositoryIdentityResolver, favicon: Arc<FaviconResolver>) -> Self {
        Self { identity, favicon }
    }
}
#[async_trait]
impl ProjectMetadata for NativeProjectMetadata {
    async fn repository_identity(
        &self,
        root: &str,
        refresh: bool,
    ) -> Result<Option<RepositoryIdentity>> {
        Ok(self.identity.resolve(root, refresh).await)
    }
    async fn favicon(&self, root: &str) -> Result<Option<String>> {
        Ok(self
            .favicon
            .resolve(root, None)
            .await?
            .map(|p| p.to_string_lossy().into()))
    }
    async fn invalidate_favicon(&self, root: &str) {
        self.favicon.invalidate(std::path::Path::new(root)).await;
    }
}
#[derive(Clone)]
struct Outcome<T> {
    value: Option<T>,
    success: bool,
}
impl<T> Outcome<T> {
    fn failed() -> Self {
        Self {
            value: None,
            success: false,
        }
    }
    fn from_result(result: Result<Option<T>>) -> Self {
        match result {
            Ok(value) => Self {
                value,
                success: true,
            },
            Err(_) => Self::failed(),
        }
    }
}
struct Cached<T> {
    outcome: Outcome<T>,
    until: Instant,
    touched: u64,
}
struct Lookup<T> {
    serial: u64,
    result: watch::Sender<Option<Outcome<T>>>,
}
struct Field<T> {
    cached: HashMap<String, Cached<T>>,
    lookups: HashMap<String, Lookup<T>>,
    pending: HashSet<String>,
}
impl<T> Default for Field<T> {
    fn default() -> Self {
        Self {
            cached: HashMap::new(),
            lookups: HashMap::new(),
            pending: HashSet::new(),
        }
    }
}
impl<T: Clone> Field<T> {
    fn available(&mut self, root: &str, touched: u64) -> Option<Outcome<T>> {
        let entry = self.cached.get_mut(root)?;
        if entry.until <= Instant::now() {
            return None;
        }
        entry.touched = touched;
        Some(entry.outcome.clone())
    }
    fn invalidate(&mut self, root: &str) {
        self.cached.remove(root);
        self.lookups.remove(root);
    }
    fn cache(
        &mut self,
        root: &str,
        outcome: Outcome<T>,
        touched: u64,
        capacity: usize,
        ttl: Duration,
    ) {
        if !self.cached.contains_key(root)
            && self.cached.len() >= capacity
            && let Some(oldest) = self
                .cached
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(k, _)| k.clone())
        {
            self.cached.remove(&oldest);
        }
        let ttl = if outcome.success {
            ttl
        } else {
            Duration::from_secs(5)
        };
        self.cached.insert(
            root.into(),
            Cached {
                outcome,
                until: Instant::now() + ttl,
                touched,
            },
        );
    }
}
#[derive(Default)]
struct State {
    identities: Field<RepositoryIdentity>,
    favicons: Field<String>,
    remembered: HashMap<String, (RepositoryIdentity, u64)>,
    serial: u64,
    closed: bool,
}
impl State {
    fn tick(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }
}
fn identities(state: &mut State) -> &mut Field<RepositoryIdentity> {
    &mut state.identities
}
fn favicons(state: &mut State) -> &mut Field<String> {
    &mut state.favicons
}
fn remember_identity(
    state: &mut State,
    root: &str,
    outcome: &Outcome<RepositoryIdentity>,
    capacity: usize,
) {
    if !outcome.success {
        return;
    }
    state.remembered.remove(root);
    if let Some(identity) = &outcome.value {
        if state.remembered.len() >= capacity
            && let Some(oldest) = state
                .remembered
                .iter()
                .min_by_key(|(_, (_, n))| *n)
                .map(|(k, _)| k.clone())
        {
            state.remembered.remove(&oldest);
        }
        let serial = state.tick();
        state
            .remembered
            .insert(root.into(), (identity.clone(), serial));
    }
}
struct Inner {
    options: EnrichmentOptions,
    state: Mutex<State>,
    backend: Arc<dyn ProjectMetadata>,
    identity_lane: Arc<Semaphore>,
    favicon_lane: Arc<Semaphore>,
    changes: broadcast::Sender<EnrichmentChange>,
    tasks: TaskTracker,
    stop: CancellationToken,
}
#[derive(Clone)]
pub struct ProjectEnrichmentService(Arc<Inner>);
impl ProjectEnrichmentService {
    pub fn new(identity: RepositoryIdentityResolver, favicon: Arc<FaviconResolver>) -> Self {
        Self::with_options(identity, favicon, EnrichmentOptions::default())
    }
    pub fn with_options(
        identity: RepositoryIdentityResolver,
        favicon: Arc<FaviconResolver>,
        options: EnrichmentOptions,
    ) -> Self {
        Self::with_backend(
            Arc::new(NativeProjectMetadata::new(identity, favicon)),
            options,
        )
    }
    pub fn with_backend(backend: Arc<dyn ProjectMetadata>, options: EnrichmentOptions) -> Self {
        let options = EnrichmentOptions {
            cache_capacity: options.cache_capacity.max(1),
            max_pending: options.max_pending.max(1),
            concurrency: options.concurrency.max(1),
        };
        Self(Arc::new(Inner {
            options,
            state: Default::default(),
            backend,
            identity_lane: Arc::new(Semaphore::new(options.concurrency)),
            favicon_lane: Arc::new(Semaphore::new(options.concurrency)),
            changes: broadcast::channel(256).0,
            tasks: TaskTracker::new(),
            stop: CancellationToken::new(),
        }))
    }
    pub fn subscribe_changes(&self) -> broadcast::Receiver<EnrichmentChange> {
        self.0.changes.subscribe()
    }
    pub async fn peek(&self, root: &str) -> ProjectEnrichment {
        let mut state = self.0.state.lock().await;
        let tick = state.tick();
        let identity = state.identities.available(root, tick);
        let favicon = state.favicons.available(root, tick);
        let resolved = identity.as_ref().is_some_and(|v| v.success);
        ProjectEnrichment {
            repository_identity: if resolved {
                identity.and_then(|v| v.value)
            } else {
                state.remembered.get(root).map(|(v, _)| v.clone())
            },
            favicon_path: favicon.filter(|v| v.success).and_then(|v| v.value),
            repository_identity_resolved: resolved,
        }
    }
    pub async fn get_available(&self, root: &str) -> ProjectEnrichment {
        let value = self.peek(root).await;
        self.request(root).await;
        value
    }
    pub async fn request(&self, root: &str) {
        for identity in [true, false] {
            let mut state = self.0.state.lock().await;
            if state.closed {
                return;
            }
            let tick = state.tick();
            let (fresh, pending) = if identity {
                (
                    state.identities.available(root, tick).is_some(),
                    &mut state.identities.pending,
                )
            } else {
                (
                    state.favicons.available(root, tick).is_some(),
                    &mut state.favicons.pending,
                )
            };
            if fresh || pending.len() >= self.0.options.max_pending || !pending.insert(root.into())
            {
                continue;
            }
            let this = self.clone();
            let root = root.to_owned();
            self.0.tasks.spawn(async move {
                let lane = if identity { &this.0.identity_lane } else { &this.0.favicon_lane };
                let change = tokio::select! {
                    biased;
                    _ = this.0.stop.cancelled() => None,
                    change = async {
                        let Ok(_permit) = lane.acquire().await else { return None; };
                        if identity {
                            let result = this.identity(&root).await;
                            let favicon = this.peek(&root).await.favicon_path;
                            Some(EnrichmentChange {
                                workspace_root: root.clone(),
                                enrichment: ProjectEnrichment { repository_identity: result.value, favicon_path: favicon, repository_identity_resolved: result.success },
                            })
                        } else {
                            this.favicon(&root).await;
                            None
                        }
                    } => change,
                };
                let mut state = this.0.state.lock().await;
                if identity { state.identities.pending.remove(&root); } else { state.favicons.pending.remove(&root); }
                drop(state);
                if let Some(change) = change {
                    let _ = this.0.changes.send(change);
                }
            });
        }
    }
    async fn lookup<T, F, Fut>(
        &self,
        root: &str,
        field: fn(&mut State) -> &mut Field<T>,
        ttl: Duration,
        remember: fn(&mut State, &str, &Outcome<T>, usize),
        resolve: F,
    ) -> Outcome<T>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Option<T>>> + Send,
    {
        let mut state = self.0.state.lock().await;
        if state.closed {
            return Outcome::failed();
        }
        let serial = state.tick();
        if let Some(value) = field(&mut state).available(root, serial) {
            return value;
        }
        let mut receiver = if let Some(lookup) = field(&mut state).lookups.get(root) {
            lookup.result.subscribe()
        } else {
            let (sender, receiver) = watch::channel(None);
            field(&mut state).lookups.insert(
                root.into(),
                Lookup {
                    serial,
                    result: sender.clone(),
                },
            );
            let this = self.clone();
            let root = root.to_owned();
            self.0.tasks.spawn(async move {
                let outcome = tokio::select! {
                    biased;
                    _ = this.0.stop.cancelled() => Outcome::failed(),
                    value = AssertUnwindSafe(resolve()).catch_unwind() => match value {
                        Ok(value) => Outcome::from_result(value), Err(_) => Outcome::failed(),
                    },
                };
                let mut state = this.0.state.lock().await;
                if !state.closed
                    && field(&mut state)
                        .lookups
                        .get(&root)
                        .is_some_and(|v| v.serial == serial)
                {
                    field(&mut state).lookups.remove(&root);
                    let tick = state.tick();
                    field(&mut state).cache(
                        &root,
                        outcome.clone(),
                        tick,
                        this.0.options.cache_capacity,
                        ttl,
                    );
                    remember(&mut state, &root, &outcome, this.0.options.cache_capacity);
                }
                sender.send_replace(Some(outcome));
            });
            receiver
        };
        drop(state);
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result;
            }
            if receiver.changed().await.is_err() {
                return Outcome::failed();
            }
        }
    }
    async fn identity(&self, root: &str) -> Outcome<RepositoryIdentity> {
        let backend = self.0.backend.clone();
        let owned = root.to_owned();
        self.lookup(
            root,
            identities,
            Duration::from_secs(60),
            remember_identity,
            move || async move { backend.repository_identity(&owned, false).await },
        )
        .await
    }
    async fn favicon(&self, root: &str) -> Outcome<String> {
        let backend = self.0.backend.clone();
        let owned = root.to_owned();
        self.lookup(
            root,
            favicons,
            Duration::from_secs(900),
            |_, _, _, _| {},
            move || async move { backend.favicon(&owned).await },
        )
        .await
    }
    pub async fn read_repository_identity(&self, root: &str) -> Option<RepositoryIdentity> {
        self.identity(root).await.value
    }
    pub async fn invalidate(&self, roots: impl IntoIterator<Item = String>) {
        let roots: HashSet<_> = roots.into_iter().collect();
        for root in roots {
            let mut state = self.0.state.lock().await;
            state.identities.invalidate(&root);
            state.favicons.invalidate(&root);
            state.remembered.remove(&root);
            drop(state);
            self.0.backend.invalidate_favicon(&root).await;
        }
    }
    pub async fn refresh_repository_identity(&self, root: &str) {
        let task = {
            let state = self.0.state.lock().await;
            if state.closed {
                return;
            }
            let this = self.clone();
            let root = root.to_owned();
            self.0.tasks.spawn(async move {
                let preflight = tokio::select! {
                    biased;
                    _ = this.0.stop.cancelled() => return,
                    value = AssertUnwindSafe(this.0.backend.repository_identity(&root, true)).catch_unwind() => value,
                };
                if !matches!(preflight, Ok(Ok(Some(_)))) { return; }
                this.0.state.lock().await.identities.invalidate(&root);
                let identity = this.identity(&root).await;
                let favicon = this.peek(&root).await.favicon_path;
                let _ = this.0.changes.send(EnrichmentChange {
                    workspace_root: root,
                    enrichment: ProjectEnrichment { repository_identity: identity.value, favicon_path: favicon, repository_identity_resolved: identity.success },
                });
            })
        };
        let _ = task.await;
    }
    pub async fn shutdown(&self) {
        self.0.state.lock().await.closed = true;
        self.0.stop.cancel();
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }
}
