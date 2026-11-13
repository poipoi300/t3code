use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
}

#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn get(&self, key: &str) -> std::io::Result<Option<String>>;
    async fn set(&self, key: &str, value: &str) -> std::io::Result<()>;
    async fn clear(&self) -> std::io::Result<()>;
}
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);
#[async_trait]
impl CacheStore for MemoryStore {
    async fn get(&self, key: &str) -> std::io::Result<Option<String>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }
    async fn set(&self, key: &str, value: &str) -> std::io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.into(), value.into());
        Ok(())
    }
    async fn clear(&self) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
        Ok(())
    }
}
pub struct FileStore {
    directory: std::path::PathBuf,
}
impl FileStore {
    pub async fn open(directory: impl Into<std::path::PathBuf>) -> std::io::Result<Self> {
        let directory = directory.into();
        tokio::fs::create_dir_all(&directory).await?;
        Ok(Self { directory })
    }
    fn path(&self, key: &str) -> std::path::PathBuf {
        self.directory
            .join(format!("pr-rust-v1-{:x}", Sha256::digest(key.as_bytes())))
    }
    pub async fn prune(&self, now_ms: u64) -> std::io::Result<()> {
        let mut entries = tokio::fs::read_dir(&self.directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("pr-rust-v1-")
                || name.len() != 75
                || !name[11..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            let metadata = entry.metadata().await?;
            if metadata.is_file()
                && metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .saturating_add(86_400_000)
                    < u128::from(now_ms)
            {
                match tokio::fs::remove_file(entry.path()).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }
}
#[async_trait]
impl CacheStore for FileStore {
    async fn get(&self, key: &str) -> std::io::Result<Option<String>> {
        match tokio::fs::read_to_string(self.path(key)).await {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    async fn set(&self, key: &str, value: &str) -> std::io::Result<()> {
        tokio::fs::write(self.path(key), value).await
    }
    async fn clear(&self) -> std::io::Result<()> {
        let mut entries = tokio::fs::read_dir(&self.directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("pr-rust-v1-")
                && name.len() == 75
                && name[11..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                tokio::fs::remove_file(entry.path()).await?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Revision {
    token: u64,
    expires_at: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    payload: String,
    expires_at: u64,
    revisions: Vec<u64>,
}

/// Scope read locks prevent a read started before invalidation from publishing under its new revision.
#[derive(Clone)]
pub struct PullRequestReadCache {
    store: Arc<dyn CacheStore>,
    clock: Arc<dyn Clock>,
    enabled: Arc<AtomicBool>,
    revisions: Arc<AsyncMutex<Option<BTreeMap<String, Revision>>>>,
    stripes: Vec<Arc<RwLock<()>>>,
    flights: Arc<Mutex<HashMap<String, std::sync::Weak<AsyncMutex<()>>>>>,
    writes: tokio_util::task::TaskTracker,
    reads: Arc<tokio::sync::Semaphore>,
}
impl PullRequestReadCache {
    pub fn new(store: Arc<dyn CacheStore>, clock: Arc<dyn Clock>) -> Self {
        let writes = tokio_util::task::TaskTracker::new();
        writes.close();
        Self {
            store,
            clock,
            enabled: Arc::new(AtomicBool::new(true)),
            revisions: Arc::new(AsyncMutex::new(None)),
            stripes: (0..64).map(|_| Arc::new(RwLock::new(()))).collect(),
            flights: Arc::new(Mutex::new(HashMap::new())),
            writes,
            reads: Arc::new(tokio::sync::Semaphore::new(512)),
        }
    }
    fn stripe(scope: &str) -> usize {
        usize::from(Sha256::digest(scope.as_bytes())[0]) % 64
    }
    async fn revisions(
        &self,
    ) -> std::io::Result<tokio::sync::MutexGuard<'_, Option<BTreeMap<String, Revision>>>> {
        let mut revisions = self.revisions.lock().await;
        if revisions.is_none() {
            let raw = self.store.get("revisions").await?;
            *revisions = Some(match raw {
                None => BTreeMap::new(),
                Some(raw) => match serde_json::from_str(&raw) {
                    Ok(v) => v,
                    Err(_) => {
                        self.store.clear().await?;
                        BTreeMap::new()
                    }
                },
            });
        }
        Ok(revisions)
    }
    pub async fn get<T, F>(&self, key: &str, scopes: &[String], lookup: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: Future<Output = Result<T>>,
    {
        if !self.enabled.load(Ordering::Acquire) {
            return lookup.await;
        }
        let _read = self
            .reads
            .acquire()
            .await
            .map_err(|_| crate::Error::invalid("cache", "Pull request cache is closed."))?;
        let mut stripes: Vec<_> = scopes.iter().map(|s| Self::stripe(s)).collect();
        stripes.sort_unstable();
        stripes.dedup();
        let mut guards = Vec::new();
        for stripe in stripes {
            guards.push(self.stripes[stripe].read().await);
        }
        let flight = {
            let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
            flights.retain(|_, v| v.strong_count() > 0);
            flights
                .get(key)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| {
                    let gate = Arc::new(AsyncMutex::new(()));
                    flights.insert(key.into(), Arc::downgrade(&gate));
                    gate
                })
        };
        let _flight = flight.lock().await;
        let now = self.clock.now_ms();
        let current = match self.revisions().await {
            Ok(revisions) => scopes
                .iter()
                .map(|s| {
                    revisions
                        .as_ref()
                        .and_then(|r| r.get(s))
                        .filter(|r| r.expires_at > now)
                        .map_or(0, |r| r.token)
                })
                .collect::<Vec<_>>(),
            Err(_) => return lookup.await,
        };
        if let Ok(Some(raw)) = self.store.get(key).await
            && let Ok(entry) = serde_json::from_str::<Entry>(&raw)
            && entry.expires_at > now
            && current
                .iter()
                .enumerate()
                .all(|(i, r)| *r == 0 || entry.revisions.get(i) == Some(r))
            && let Ok(value) = serde_json::from_str(&entry.payload)
        {
            return Ok(value);
        }
        let value = lookup.await?;
        if let Ok(payload) = serde_json::to_string(&value) {
            let entry = Entry {
                payload,
                expires_at: self.clock.now_ms().saturating_add(60_000),
                revisions: current,
            };
            if let Ok(raw) = serde_json::to_string(&entry) {
                let _ = self.store.set(key, &raw).await;
            }
        }
        Ok(value)
    }
    pub async fn invalidate(&self, scope: &str) {
        let cache = self.clone();
        let scope = scope.to_string();
        let _ = self
            .writes
            .spawn(async move {
                cache.invalidate_inner(&scope).await;
            })
            .await;
    }
    pub async fn drain(&self) {
        self.writes.wait().await;
    }
    async fn invalidate_inner(&self, scope: &str) {
        let _stripe = self.stripes[Self::stripe(scope)].write().await;
        let result = async {
            let now = self.clock.now_ms();
            let mut guard = self.revisions().await?;
            let revisions = guard.as_mut().expect("revision map initialized");
            let previous = revisions.get(scope).map_or(0, |r| r.token);
            revisions.retain(|_, r| r.expires_at > now);
            revisions.insert(
                scope.into(),
                Revision {
                    token: now.max(previous).saturating_add(1),
                    expires_at: now.saturating_add(60_000),
                },
            );
            self.store
                .set("revisions", &serde_json::to_string(revisions)?)
                .await
        }
        .await;
        if result.is_err() {
            self.enabled.store(false, Ordering::Release);
        }
    }
}
