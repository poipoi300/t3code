use super::resources::{RequestToken, Resource, ResourcePolicy};
use crate::{
    ClientError,
    platform::Platform,
    rpc::RpcEndpoint,
    types::{EnvironmentId, Generation, ThreadKey},
};
use futures::{FutureExt, StreamExt};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
};
use t3_contracts::{
    auth::{AuthEnvironmentScope, AuthSessionState},
    filesystem::*,
    rpc::WsFilesystemGetMetadataRpc as FilesystemGetMetadata,
};

const METADATA_STALE_MS: u64 = 60_000;
const CACHE_LIMIT: usize = 1024;
pub const METADATA_BATCH_LIMIT: usize = 64;
pub fn windows_path(path: &str) -> bool {
    path.starts_with("\\\\")
        || (path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && path.as_bytes().get(1) == Some(&b':')
            && (path.len() == 2
                || path
                    .as_bytes()
                    .get(2)
                    .is_some_and(|character| matches!(character, b'/' | b'\\'))))
}
pub fn normalize_path(path: &str) -> String {
    let path = path.trim();
    let root = matches!(path, "/" | "\\") || (path.len() == 3 && windows_path(path));
    let trimmed = if root {
        path
    } else if path.starts_with('/') {
        path.trim_end_matches('/')
    } else {
        path.trim_end_matches(['/', '\\'])
    };
    let mut trimmed = if trimmed.is_empty() {
        path.to_string()
    } else {
        trimmed.to_string()
    };
    if trimmed.len() == 2 && trimmed.ends_with(':') && path.len() > 2 {
        trimmed.push('\\');
    }
    if windows_path(&trimmed) {
        trimmed.replace('/', "\\").to_lowercase()
    } else {
        trimmed
    }
}
pub fn workspace_file_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/')
        || windows_path(path)
        || path.starts_with("~/")
        || path.starts_with("~\\")
    {
        return path.into();
    }
    let base = cwd.trim_end_matches(['/', '\\']);
    if windows_path(cwd) {
        format!("{base}\\{}", path.replace('/', "\\"))
    } else {
        format!("{base}/{path}")
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadAccess {
    pub allowed: bool,
    pub pending: bool,
    pub error: Option<String>,
}
pub fn filesystem_read_access(
    catalog_ready: bool,
    phase: Option<&str>,
    connection_error: Option<&str>,
    session: Option<&AuthSessionState>,
    session_error: Option<&str>,
) -> ReadAccess {
    if let Some(error) = session_error {
        return ReadAccess {
            allowed: false,
            pending: false,
            error: Some(error.into()),
        };
    }
    if let Some(session) = session {
        return ReadAccess {
            allowed: crate::permissions::session_grants_scope(
                session,
                AuthEnvironmentScope::FilesystemRead,
            ),
            pending: false,
            error: None,
        };
    }
    let pending =
        !catalog_ready || matches!(phase, Some("connected" | "connecting" | "reconnecting"));
    ReadAccess {
        allowed: false,
        pending,
        error: (!pending).then(|| {
            connection_error
                .unwrap_or("This environment is not connected.")
                .into()
        }),
    }
}
#[derive(Clone)]
struct CachedMetadata {
    value: Option<FilesystemEntryMetadata>,
    expires: u64,
    order: u64,
}
struct RetainedThread {
    readers: usize,
    entries: BTreeMap<String, CachedMetadata>,
}
struct MetadataEntry {
    state: Resource<Option<FilesystemEntryMetadata>>,
    version: u64,
    fetching: bool,
}
#[derive(Default)]
struct MetadataEnvironment {
    session: Generation,
    suspended: bool,
    invalidation: u64,
    order: u64,
    cache: BTreeMap<String, CachedMetadata>,
    entries: BTreeMap<String, MetadataEntry>,
    pending: BTreeSet<String>,
}
#[derive(Default)]
struct MetadataInner {
    environments: BTreeMap<EnvironmentId, MetadataEnvironment>,
    threads: BTreeMap<ThreadKey, RetainedThread>,
}
#[derive(Clone)]
pub struct FileMetadataService {
    inner: Rc<RefCell<MetadataInner>>,
    changed: Resource<()>,
}
impl Default for FileMetadataService {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            changed: Resource::new(Default::default(), Generation::default()),
        }
    }
}
pub struct RetainedMetadataThread {
    key: ThreadKey,
    owner: Weak<RefCell<MetadataInner>>,
}
impl Drop for RetainedMetadataThread {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut inner = owner.borrow_mut();
            if let Some(thread) = inner.threads.get_mut(&self.key) {
                thread.readers -= 1;
                if thread.readers == 0 {
                    inner.threads.remove(&self.key);
                }
            }
        }
    }
}
pub struct MetadataBatch {
    pub environment: EnvironmentId,
    pub session: Generation,
    pub paths: Vec<String>,
    tokens: Vec<(u64, RequestToken)>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataSeed {
    pub session: Generation,
    invalidation: u64,
}
impl FileMetadataService {
    pub fn begin_session(&self, environment: EnvironmentId, session: Generation) {
        let mut inner = self.inner.borrow_mut();
        let env = inner.environments.entry(environment.clone()).or_default();
        env.suspended = false;
        if env.session == session {
            return;
        }
        env.session = session;
        env.cache.clear();
        env.pending.clear();
        for (path, entry) in &mut env.entries {
            entry.state.replace_session(session);
            entry.fetching = false;
            if entry.state.has_readers() {
                env.pending.insert(path.clone());
            }
        }
        for (key, thread) in &mut inner.threads {
            if key.environment == environment {
                thread.entries.clear();
            }
        }
    }
    pub fn retain_thread(&self, key: ThreadKey) -> RetainedMetadataThread {
        self.inner
            .borrow_mut()
            .threads
            .entry(key.clone())
            .or_insert_with(|| RetainedThread {
                readers: 0,
                entries: BTreeMap::new(),
            })
            .readers += 1;
        RetainedMetadataThread {
            key,
            owner: Rc::downgrade(&self.inner),
        }
    }
    pub fn enqueue(
        &self,
        environment: EnvironmentId,
        path: &str,
        thread: Option<&ThreadKey>,
        now: u64,
    ) -> Resource<Option<FilesystemEntryMetadata>> {
        let path = normalize_path(path);
        let mut inner = self.inner.borrow_mut();
        let retained = thread
            .filter(|key| key.environment == environment)
            .and_then(|key| inner.threads.get(key))
            .and_then(|thread| thread.entries.get(&path))
            .filter(|entry| entry.value.is_some() || entry.expires > now)
            .cloned();
        let env = inner.environments.entry(environment.clone()).or_default();
        let known = retained.or_else(|| {
            env.cache
                .get(&path)
                .filter(|entry| entry.expires > now)
                .cloned()
        });
        let entry = env
            .entries
            .entry(path.clone())
            .or_insert_with(|| MetadataEntry {
                state: Resource::new(
                    ResourcePolicy {
                        stale_ms: METADATA_STALE_MS,
                        idle_ttl_ms: METADATA_STALE_MS,
                        ..Default::default()
                    },
                    env.session,
                ),
                version: 0,
                fetching: false,
            });
        let resource = entry.state.clone();
        if let Some(known) = known {
            let token = resource.begin();
            resource.finish(token, known.value.clone(), now, false);
            if let Some(thread) = thread
                .filter(|key| key.environment == environment)
                .and_then(|key| inner.threads.get_mut(key))
            {
                thread.entries.insert(path, known);
            }
        } else {
            if !entry.fetching {
                env.pending.insert(path.clone());
            }
            if let Some(thread) = thread
                .filter(|key| key.environment == environment)
                .and_then(|key| inner.threads.get_mut(key))
            {
                thread.entries.entry(path).or_insert(CachedMetadata {
                    value: None,
                    expires: 0,
                    order: 0,
                });
            }
        }
        drop(inner);
        self.changed.invalidate();
        resource
    }
    pub fn next_batch(&self, environment: &EnvironmentId) -> Option<MetadataBatch> {
        let mut inner = self.inner.borrow_mut();
        let env = inner.environments.get_mut(environment)?;
        if env.suspended {
            return None;
        }
        let paths: Vec<_> = env
            .pending
            .iter()
            .take(METADATA_BATCH_LIMIT)
            .cloned()
            .collect();
        if paths.is_empty() {
            return None;
        }
        let tokens = paths
            .iter()
            .filter_map(|path| {
                env.pending.remove(path);
                let entry = env.entries.get_mut(path)?;
                entry.fetching = true;
                Some((entry.version, entry.state.begin()))
            })
            .collect();
        Some(MetadataBatch {
            environment: environment.clone(),
            session: env.session,
            paths,
            tokens,
        })
    }
    pub fn complete(
        &self,
        batch: MetadataBatch,
        entries: Vec<Option<FilesystemEntryMetadata>>,
        now: u64,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner
            .environments
            .get(&batch.environment)
            .is_none_or(|env| env.session != batch.session || env.suspended)
        {
            return false;
        }
        for (index, path) in batch.paths.iter().enumerate() {
            let env = inner
                .environments
                .get_mut(&batch.environment)
                .expect("validated environment");
            let Some(entry) = env.entries.get_mut(path) else {
                continue;
            };
            entry.fetching = false;
            let (version, token) = batch.tokens[index];
            if version != entry.version {
                env.pending.insert(path.clone());
                continue;
            }
            let value = entries.get(index).cloned().flatten();
            entry.state.finish(token, value.clone(), now, false);
            Self::remember_inner(&mut inner, &batch.environment, path, value, now, false);
        }
        true
    }
    pub fn fail(&self, batch: MetadataBatch, error: &ClientError, now: u64) {
        let mut inner = self.inner.borrow_mut();
        let Some(env) = inner.environments.get_mut(&batch.environment) else {
            return;
        };
        if env.session != batch.session || env.suspended {
            return;
        }
        for (index, path) in batch.paths.iter().enumerate() {
            if let Some(entry) = env.entries.get_mut(path) {
                entry.fetching = false;
                entry.state.fail(batch.tokens[index].1, error, now);
            }
        }
    }
    pub async fn fetch_pending(
        &self,
        environment: &EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
    ) -> Result<(), ClientError> {
        while let Some(batch) = self.next_batch(environment) {
            let input: FilesystemGetMetadataInput =
                serde_json::from_value(serde_json::json!({ "paths": batch.paths }))?;
            let mut guard = MetadataBatchGuard {
                service: self.clone(),
                batch: Some(batch),
            };
            let result = endpoint.call::<FilesystemGetMetadata>(input).await;
            let Some(batch) = guard.batch.take() else {
                return Err(ClientError::Cancelled);
            };
            match result {
                Ok(result) => {
                    if !self.complete(batch, result.entries, platform.monotonic_ms()) {
                        return Err(ClientError::Cancelled);
                    }
                }
                Err(error) => {
                    self.fail(batch, &error, platform.monotonic_ms());
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    fn remember_inner(
        inner: &mut MetadataInner,
        environment: &EnvironmentId,
        path: &str,
        value: Option<FilesystemEntryMetadata>,
        now: u64,
        partial: bool,
    ) {
        let env = inner.environments.entry(environment.clone()).or_default();
        env.order += 1;
        let cached = CachedMetadata {
            value: value.clone(),
            expires: now.saturating_add(METADATA_STALE_MS),
            order: env.order,
        };
        env.cache.insert(path.into(), cached.clone());
        if env.cache.len() > CACHE_LIMIT
            && let Some(oldest) = env
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.order)
                .map(|(path, _)| path.clone())
        {
            env.cache.remove(&oldest);
        }
        for (key, thread) in &mut inner.threads {
            if &key.environment != environment {
                continue;
            }
            if let Some(previous) = thread.entries.get_mut(path) {
                let mut cached = cached.clone();
                if partial
                    && let (Some(value), Some(previous)) = (&mut cached.value, &previous.value)
                    && value.kind == previous.kind
                {
                    value.byte_length = value.byte_length.or(previous.byte_length);
                    value.mime_type = value
                        .mime_type
                        .clone()
                        .or_else(|| previous.mime_type.clone());
                }
                *previous = cached;
            }
        }
    }
    pub fn seed_token(&self, environment: &EnvironmentId) -> Option<MetadataSeed> {
        let inner = self.inner.borrow();
        let env = inner.environments.get(environment)?;
        if env.suspended {
            return None;
        }
        Some(MetadataSeed {
            session: env.session,
            invalidation: env.invalidation,
        })
    }
    pub fn remember_file(
        &self,
        environment: &EnvironmentId,
        token: MetadataSeed,
        cwd: &str,
        relative_path: &str,
        byte_length: u64,
        now: u64,
    ) -> bool {
        if self.seed_token(environment) != Some(token) {
            return false;
        }
        let path = normalize_path(&workspace_file_path(cwd, relative_path));
        let mut inner = self.inner.borrow_mut();
        let previous = inner
            .environments
            .get(environment)
            .and_then(|env| env.cache.get(&path))
            .filter(|entry| entry.expires > now)
            .and_then(|entry| entry.value.as_ref());
        let value = FilesystemEntryMetadata {
            kind: FilesystemEntryKind::File,
            byte_length: Some(byte_length),
            mime_type: previous.and_then(|entry| entry.mime_type.clone()),
        };
        Self::remember_inner(&mut inner, environment, &path, Some(value), now, true);
        true
    }
    pub fn remember_entries(
        &self,
        environment: &EnvironmentId,
        token: MetadataSeed,
        cwd: &str,
        entries: impl IntoIterator<Item = (String, FilesystemEntryKind)>,
        now: u64,
    ) -> bool {
        if self.seed_token(environment) != Some(token) {
            return false;
        }
        let mut inner = self.inner.borrow_mut();
        for (path, kind) in entries {
            let path = normalize_path(&workspace_file_path(cwd, &path));
            if inner
                .environments
                .get(environment)
                .and_then(|env| env.cache.get(&path))
                .is_some_and(|entry| {
                    entry.expires > now
                        && entry.value.as_ref().is_some_and(|value| value.kind == kind)
                })
            {
                continue;
            }
            Self::remember_inner(
                &mut inner,
                environment,
                &path,
                Some(FilesystemEntryMetadata {
                    kind,
                    byte_length: None,
                    mime_type: None,
                }),
                now,
                true,
            );
        }
        true
    }
    pub fn invalidate(&self, environment: &EnvironmentId, path: &str) {
        let path = normalize_path(path);
        let mut inner = self.inner.borrow_mut();
        if let Some(env) = inner.environments.get_mut(environment) {
            env.invalidation += 1;
            env.cache.remove(&path);
            if let Some(entry) = env.entries.get_mut(&path) {
                entry.version += 1;
                entry.state.invalidate();
                if entry.state.has_readers() && !entry.fetching {
                    env.pending.insert(path.clone());
                }
            }
        }
        for (key, thread) in &mut inner.threads {
            if &key.environment == environment {
                thread.entries.remove(&path);
            }
        }
        drop(inner);
        self.changed.invalidate();
    }
    pub fn disconnect(&self, environment: &EnvironmentId) {
        let mut inner = self.inner.borrow_mut();
        if let Some(env) = inner.environments.get_mut(environment) {
            env.suspended = true;
            env.pending.clear();
            for entry in env.entries.values_mut() {
                entry.state.cancel();
                entry.fetching = false;
            }
        }
    }
    pub fn collect(&self, now: u64) {
        let mut inner = self.inner.borrow_mut();
        let retained: BTreeSet<_> = inner
            .threads
            .iter()
            .flat_map(|(key, thread)| {
                thread
                    .entries
                    .iter()
                    .filter(|(_, entry)| entry.value.is_some())
                    .map(|(path, _)| (key.environment.clone(), path.clone()))
            })
            .collect();
        for (environment, env) in &mut inner.environments {
            env.cache.retain(|_, entry| entry.expires > now);
            env.entries.retain(|path, entry| {
                if entry.state.expired(now) && !entry.fetching {
                    entry.state.cancel();
                    env.pending.remove(path);
                    return false;
                }
                let state = entry.state.snapshot();
                if !env.suspended
                    && !entry.fetching
                    && state.error.is_none()
                    && state.value.as_deref() == Some(&None)
                    && entry.state.needs_refresh(now)
                    && !retained.contains(&(environment.clone(), path.clone()))
                {
                    env.pending.insert(path.clone());
                }
                true
            });
        }
    }
    pub fn observe(
        &self,
        environment: EnvironmentId,
        path: &str,
        thread: Option<&ThreadKey>,
        now: u64,
    ) -> MetadataReader {
        let resource = self.enqueue(environment, path, thread, now);
        let observer = resource.observe(now);
        self.changed.invalidate();
        MetadataReader {
            resource,
            observer,
            changed: self.changed.clone(),
        }
    }
    pub async fn drive<R: RpcEndpoint>(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
        mut sessions: futures::stream::LocalBoxStream<
            'static,
            Option<super::sessions::ServiceSession<R>>,
        >,
    ) -> Result<(), ClientError> {
        let mut changes = self.changed.observe(platform.monotonic_ms());
        let mut current: Option<super::sessions::ServiceSession<R>> = None;
        let mut work: Option<futures::future::LocalBoxFuture<'_, Result<(), ClientError>>> = None;
        loop {
            self.collect(platform.monotonic_ms());
            let (pending, deadline) = {
                let inner = self.inner.borrow();
                inner
                    .environments
                    .get(&environment)
                    .map_or((false, None), |env| {
                        let deadline = env
                            .cache
                            .iter()
                            .filter(|(path, cached)| {
                                cached.value.is_none()
                                    && env
                                        .entries
                                        .get(*path)
                                        .is_some_and(|entry| entry.state.has_readers())
                            })
                            .map(|(_, cached)| cached.expires)
                            .min();
                        (!env.pending.is_empty(), deadline)
                    })
            };
            if work.is_none()
                && pending
                && let Some(session) = &current
            {
                let endpoint = session.endpoint.clone();
                let environment = environment.clone();
                work = Some(
                    async move {
                        self.fetch_pending(&environment, endpoint.as_ref(), platform)
                            .await
                    }
                    .boxed_local(),
                );
            }
            let response = async {
                match work.as_mut() {
                    Some(work) => work.await,
                    None => futures::future::pending().await,
                }
            }
            .fuse();
            let timer = async {
                if current.is_some()
                    && let Some(deadline) = deadline
                {
                    platform
                        .sleep(deadline.saturating_sub(platform.monotonic_ms()).max(1))
                        .await;
                } else {
                    futures::future::pending::<()>().await;
                }
            }
            .fuse();
            enum Event<R> {
                Session(Option<Option<super::sessions::ServiceSession<R>>>),
                Changed,
                Finished,
                Timer,
            }
            let event = {
                let session = sessions.next().fuse();
                let changed = changes.next().fuse();
                futures::pin_mut!(response, timer, session, changed);
                futures::select_biased! { session = session => Event::Session(session), _ = response => Event::Finished, _ = changed => Event::Changed, _ = timer => Event::Timer }
            };
            match event {
                Event::Session(None) => return Ok(()),
                Event::Session(Some(session)) => {
                    if let (Some(previous), Some(next)) = (&current, &session)
                        && previous.same(next)
                    {
                        continue;
                    }
                    work = None;
                    self.disconnect(&environment);
                    if let Some(session) = &session {
                        if session.identity.environment != environment
                            || session
                                .endpoint
                                .destination()
                                .is_some_and(|id| id != environment)
                        {
                            return Err(ClientError::EnvironmentMismatch);
                        }
                        self.begin_session(environment.clone(), session.identity.generation);
                    }
                    current = session;
                }
                Event::Finished => work = None,
                Event::Changed | Event::Timer => {}
            }
            changes.tick(platform.monotonic_ms());
        }
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        let mut inner = self.inner.borrow_mut();
        if let Some(env) = inner.environments.remove(environment) {
            for entry in env.entries.values() {
                entry.state.close();
            }
        }
        inner
            .threads
            .retain(|key, _| &key.environment != environment);
    }
}
pub struct MetadataReader {
    pub resource: Resource<Option<FilesystemEntryMetadata>>,
    observer: super::resources::ResourceSubscription<Option<FilesystemEntryMetadata>>,
    changed: Resource<()>,
}
impl MetadataReader {
    pub fn tick(&mut self, now: u64) {
        self.observer.tick(now);
    }
}
impl futures::Stream for MetadataReader {
    type Item = u64;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u64>> {
        std::pin::Pin::new(&mut self.observer).poll_next(cx)
    }
}
impl Drop for MetadataReader {
    fn drop(&mut self) {
        self.changed.invalidate();
    }
}
struct MetadataBatchGuard {
    service: FileMetadataService,
    batch: Option<MetadataBatch>,
}
impl Drop for MetadataBatchGuard {
    fn drop(&mut self) {
        let Some(batch) = self.batch.take() else {
            return;
        };
        let mut inner = self.service.inner.borrow_mut();
        let Some(env) = inner.environments.get_mut(&batch.environment) else {
            return;
        };
        if env.session != batch.session {
            return;
        }
        for path in batch.paths {
            if let Some(entry) = env.entries.get_mut(&path) {
                entry.fetching = false;
                entry.state.cancel();
                env.pending.insert(path);
            }
        }
    }
}

pub fn filter_browse_entries<'a>(
    entries: &'a [FilesystemBrowseEntry],
    query: &str,
) -> (
    Vec<&'a FilesystemBrowseEntry>,
    Option<&'a FilesystemBrowseEntry>,
) {
    let lower = query.to_lowercase();
    let visible: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry.name.0.as_str().to_lowercase().starts_with(&lower)
                && (query.starts_with('.') || !entry.name.0.as_str().starts_with('.'))
        })
        .collect();
    let exact = visible
        .iter()
        .copied()
        .find(|entry| !query.is_empty() && entry.name.0.as_str() == query);
    (visible, exact)
}
pub fn browse_tab_completion(
    entries: &[FilesystemBrowseEntry],
    filter: &str,
) -> Option<(String, bool)> {
    let first = entries.first()?;
    if entries.len() == 1 {
        return Some((first.name.0.as_str().into(), true));
    }
    let mut prefix: Vec<_> = first.name.0.as_str().chars().collect();
    for entry in &entries[1..] {
        let common = prefix
            .iter()
            .zip(entry.name.0.as_str().chars())
            .take_while(|(left, right)| {
                left.to_lowercase().to_string() == right.to_lowercase().to_string()
            })
            .count();
        prefix.truncate(common);
    }
    let name: String = prefix.iter().collect();
    let lower = name.to_lowercase();
    (lower.encode_utf16().count() > filter.to_lowercase().encode_utf16().count()
        && entries
            .iter()
            .all(|entry| entry.name.0.as_str().to_lowercase().starts_with(&lower)))
    .then_some((name, false))
}
#[derive(Default)]
pub struct BrowseNavigation {
    generation: u64,
}
impl BrowseNavigation {
    pub fn invalidate(&mut self) {
        self.generation += 1;
    }
    pub fn begin(&mut self) -> u64 {
        self.invalidate();
        self.generation
    }
    pub fn can_commit(&self, generation: u64) -> bool {
        self.generation == generation
    }
}
pub fn can_preload_browse_path(phase: Option<&str>) -> bool {
    phase == Some("connected")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemBrowsePath {
    pub browsing: bool,
    pub directory_path: String,
    pub filter_query: String,
    pub parent_path: Option<String>,
    pub can_browse_up: bool,
}
pub fn filesystem_browse_path(query: &str, platform: &str, enabled: bool) -> FilesystemBrowsePath {
    let browsing = enabled
        && (query.starts_with('/')
            || query.starts_with("~/")
            || ["./", "../", ".\\", "..\\"]
                .iter()
                .any(|prefix| query.starts_with(prefix))
            || platform.to_ascii_lowercase().starts_with("win") && windows_path(query));
    if !browsing {
        return FilesystemBrowsePath {
            browsing: false,
            directory_path: String::new(),
            filter_query: String::new(),
            parent_path: None,
            can_browse_up: false,
        };
    }
    let unix = query.starts_with('/');
    let separator = |character| character == '/' || (!unix && character == '\\');
    let trailing = query.ends_with(separator);
    let directory_path = if trailing {
        query.to_string()
    } else {
        query
            .rfind(separator)
            .map_or_else(|| query.into(), |index| query[..index + 1].into())
    };
    let filter_query = if trailing {
        String::new()
    } else {
        query
            .rfind(separator)
            .map_or(query, |index| &query[index + 1..])
            .into()
    };
    let normalized = normalize_path(&directory_path);
    let windows = windows_path(&directory_path);
    let sep = if windows || directory_path.contains('\\') && !unix {
        '\\'
    } else {
        '/'
    };
    let parent_path = if windows {
        let original = directory_path.trim_end_matches(['/', '\\']);
        let root_segments = if directory_path.starts_with("\\\\") {
            2
        } else {
            1
        };
        let segments: Vec<_> = original
            .split(['/', '\\'])
            .filter(|segment| !segment.is_empty())
            .collect();
        (segments.len() > root_segments).then(|| {
            let parent = segments[..segments.len() - 1].join("\\");
            format!(
                "{}{parent}\\",
                if directory_path.starts_with("\\\\") {
                    "\\\\"
                } else {
                    ""
                }
            )
        })
    } else if normalized == "/" {
        None
    } else {
        directory_path
            .trim_end_matches(separator)
            .rfind(separator)
            .map(|index| directory_path[..index + 1].replace('/', &sep.to_string()))
    };
    let can_browse_up = directory_path.ends_with(separator) && parent_path.is_some();
    FilesystemBrowsePath {
        browsing,
        directory_path,
        filter_query,
        parent_path,
        can_browse_up,
    }
}
