use crate::identity::RepositoryIdentityResolver;
use std::{collections::HashMap, sync::Arc, time::Duration};
use t3_contracts::environment::RepositoryIdentity;
use t3_workspace::project::FaviconResolver;
use tokio::sync::{Mutex, Semaphore, broadcast};
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
#[derive(Default)]
struct Entry {
    identity: Option<RepositoryIdentity>,
    favicon: Option<String>,
    identity_until: Option<Instant>,
    favicon_until: Option<Instant>,
    identity_pending: bool,
    favicon_pending: bool,
    generation: u64,
    touched: u64,
    identity_gate: Arc<Mutex<()>>,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    version: u64,
    closed: bool,
}
struct Inner {
    cache: Mutex<Cache>,
    identity: RepositoryIdentityResolver,
    favicon: Arc<FaviconResolver>,
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
        Self(Arc::new(Inner {
            cache: Default::default(),
            identity,
            favicon,
            identity_lane: Arc::new(Semaphore::new(4)),
            favicon_lane: Arc::new(Semaphore::new(4)),
            changes: broadcast::channel(256).0,
            tasks: TaskTracker::new(),
            stop: CancellationToken::new(),
        }))
    }
    pub fn subscribe_changes(&self) -> broadcast::Receiver<EnrichmentChange> {
        self.0.changes.subscribe()
    }
    pub async fn peek(&self, root: &str) -> ProjectEnrichment {
        let cache = self.0.cache.lock().await;
        cache
            .entries
            .get(root)
            .map(|e| ProjectEnrichment {
                repository_identity: e.identity.clone(),
                repository_identity_resolved: e.identity_until.is_some_and(|t| t > Instant::now()),
                favicon_path: e
                    .favicon_until
                    .filter(|t| *t > Instant::now())
                    .and(e.favicon.clone()),
            })
            .unwrap_or_default()
    }
    pub async fn get_available(&self, root: &str) -> ProjectEnrichment {
        let value = self.peek(root).await;
        self.request(root).await;
        value
    }
    pub async fn request(&self, root: &str) {
        for identity in [true, false] {
            let mut cache = self.0.cache.lock().await;
            if cache.closed {
                return;
            }
            if !cache.entries.contains_key(root) && cache.entries.len() >= 512 {
                let old = cache
                    .entries
                    .iter()
                    .filter(|(_, e)| !e.identity_pending && !e.favicon_pending)
                    .min_by_key(|(_, e)| e.touched)
                    .map(|(k, _)| k.clone());
                let Some(old) = old else { continue };
                cache.entries.remove(&old);
            }
            cache.version += 1;
            let version = cache.version;
            let e = cache.entries.entry(root.into()).or_insert_with(|| Entry {
                generation: version,
                ..Default::default()
            });
            let (until, pending) = if identity {
                (&e.identity_until, &mut e.identity_pending)
            } else {
                (&e.favicon_until, &mut e.favicon_pending)
            };
            if *pending || until.is_some_and(|t| t > Instant::now()) {
                continue;
            }
            *pending = true;
            e.touched = version;
            let generation = e.generation;
            let root = root.to_owned();
            let this = self.clone();
            self.0.tasks.spawn(async move {
                let permits = if identity {
                    this.0.identity_lane.clone()
                } else {
                    this.0.favicon_lane.clone()
                };
                tokio::select! { _=this.0.stop.cancelled()=>{}, result=async {
                    let Ok(_permit)=permits.acquire_owned().await else{return};
                    this.resolve_field(&root,identity,false,generation).await;
                }=>{result} }
            });
        }
    }
    async fn resolve_field(&self, root: &str, identity: bool, refresh: bool, generation: u64) {
        let gate = {
            self.0
                .cache
                .lock()
                .await
                .entries
                .get(root)
                .map(|e| e.identity_gate.clone())
        };
        let Some(gate) = gate else { return };
        let _guard = if identity {
            Some(gate.lock().await)
        } else {
            None
        };
        {
            let cache = self.0.cache.lock().await;
            let Some(e) = cache.entries.get(root) else {
                return;
            };
            if e.generation != generation
                || identity && !refresh && e.identity_until.is_some_and(|t| t > Instant::now())
            {
                return;
            }
        }
        let (repo, favicon, success) = if identity {
            (self.0.identity.resolve(root, refresh).await, None, true)
        } else {
            match self.0.favicon.resolve(root, None).await {
                Ok(p) => (None, p.map(|p| p.to_string_lossy().into()), true),
                Err(_) => (None, None, false),
            }
        };
        let mut cache = self.0.cache.lock().await;
        let Some(e) = cache.entries.get_mut(root) else {
            return;
        };
        if e.generation != generation {
            return;
        }
        if identity {
            e.identity = repo;
            e.identity_until = Some(Instant::now() + Duration::from_secs(60));
            e.identity_pending = false;
        } else {
            e.favicon = favicon;
            e.favicon_until =
                Some(Instant::now() + Duration::from_secs(if success { 900 } else { 5 }));
            e.favicon_pending = false;
        }
        if identity {
            let _ = self.0.changes.send(EnrichmentChange {
                workspace_root: root.into(),
                enrichment: ProjectEnrichment {
                    repository_identity: e.identity.clone(),
                    favicon_path: e.favicon.clone(),
                    repository_identity_resolved: true,
                },
            });
        }
    }
    pub async fn read_repository_identity(&self, root: &str) -> Option<RepositoryIdentity> {
        if self.peek(root).await.repository_identity_resolved {
            return self.peek(root).await.repository_identity;
        }
        self.request(root).await;
        let generation = self
            .0
            .cache
            .lock()
            .await
            .entries
            .get(root)
            .map(|e| e.generation);
        if let Some(generation) = generation {
            self.resolve_field(root, true, false, generation).await;
        }
        self.peek(root).await.repository_identity
    }
    pub async fn invalidate(&self, roots: impl IntoIterator<Item = String>) {
        for root in roots {
            self.0.cache.lock().await.entries.remove(&root);
            self.0.favicon.invalidate(std::path::Path::new(&root)).await;
        }
    }
    pub async fn refresh_repository_identity(&self, root: &str) {
        if self.0.identity.resolve(root, true).await.is_none() {
            return;
        }
        self.request(root).await;
        let generation = self
            .0
            .cache
            .lock()
            .await
            .entries
            .get(root)
            .map(|e| e.generation);
        if let Some(generation) = generation {
            self.resolve_field(root, true, true, generation).await;
        }
    }
    pub async fn shutdown(&self) {
        self.0.cache.lock().await.closed = true;
        self.0.stop.cancel();
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }
}
