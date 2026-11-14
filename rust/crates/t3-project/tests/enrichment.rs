#![allow(clippy::unwrap_used)]
mod support;
use std::sync::Arc;
use support::{Fixture, git_environment};
use t3_git::{ExecuteOptions, Git};
use t3_project::{enrichment::*, identity::RepositoryIdentityResolver};

#[tokio::test]
async fn remembered_identity_survives_cache_eviction_by_a_remote_less_folder() {
    let f = Fixture::new();
    let root = f.root("repo");
    let empty = f.root("no-remote");
    let git = Git::new(&root).with_environment(git_environment(f.temp.path()));
    for args in [
        vec!["init"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/Fixture/Repo.git",
        ],
    ] {
        git.execute(&args, ExecuteOptions::default()).await.unwrap();
    }
    let enrichment = ProjectEnrichmentService::with_options(
        RepositoryIdentityResolver::default().with_environment(git_environment(f.temp.path())),
        Arc::new(t3_workspace::project::FaviconResolver::new(
            f.temp.path().into(),
        )),
        EnrichmentOptions {
            cache_capacity: 1,
            ..Default::default()
        },
    );
    let root = root.to_string_lossy();
    let identity = enrichment.read_repository_identity(&root).await.unwrap();
    assert!(
        enrichment
            .read_repository_identity(&empty.to_string_lossy())
            .await
            .is_none()
    );
    let available = enrichment.peek(&root).await;
    assert_eq!(
        available.repository_identity.unwrap().canonical_key,
        identity.canonical_key
    );
    assert!(!available.repository_identity_resolved);
    enrichment.invalidate([root.to_string()]).await;
    assert!(enrichment.peek(&root).await.repository_identity.is_none());
    enrichment.shutdown().await;
    f.shutdown().await;
}

use async_trait::async_trait;
use std::{collections::HashMap, sync::Mutex};
use t3_contracts::environment::RepositoryIdentity;
use t3_project::Result;
use tokio::sync::{Semaphore, mpsc};

struct GatedMetadata {
    native: NativeProjectMetadata,
    identity_gate: Arc<Semaphore>,
    favicon_gate: Arc<Semaphore>,
    counts: Mutex<HashMap<String, usize>>,
    started: mpsc::UnboundedSender<(bool, String)>,
}
#[async_trait]
impl ProjectMetadata for GatedMetadata {
    async fn repository_identity(&self, root: &str, _: bool) -> Result<Option<RepositoryIdentity>> {
        let first = {
            let mut counts = self.counts.lock().unwrap();
            let count = counts.entry(root.into()).or_default();
            *count += 1;
            *count == 1
        };
        let result = self.native.repository_identity(root, true).await;
        self.started.send((true, root.into())).unwrap();
        if first {
            self.identity_gate.acquire().await.unwrap().forget();
        }
        result
    }
    async fn favicon(&self, root: &str) -> Result<Option<String>> {
        self.started.send((false, root.into())).unwrap();
        self.favicon_gate.acquire().await.unwrap().forget();
        self.native.favicon(root).await
    }
    async fn invalidate_favicon(&self, root: &str) {
        self.native.invalidate_favicon(root).await;
    }
}
fn gated(
    f: &Fixture,
    options: EnrichmentOptions,
) -> (
    ProjectEnrichmentService,
    Arc<GatedMetadata>,
    mpsc::UnboundedReceiver<(bool, String)>,
) {
    let (started, events) = mpsc::unbounded_channel();
    let backend = Arc::new(GatedMetadata {
        native: NativeProjectMetadata::new(
            RepositoryIdentityResolver::default().with_environment(git_environment(f.temp.path())),
            Arc::new(t3_workspace::project::FaviconResolver::new(
                f.temp.path().into(),
            )),
        ),
        identity_gate: Arc::new(Semaphore::new(0)),
        favicon_gate: Arc::new(Semaphore::new(0)),
        counts: Default::default(),
        started,
    });
    (
        ProjectEnrichmentService::with_backend(backend.clone(), options),
        backend,
        events,
    )
}
async fn repository(f: &Fixture, name: &str) -> String {
    let root = f.root(name);
    let git = Git::new(&root).with_environment(git_environment(f.temp.path()));
    git.execute(&["init"], ExecuteOptions::default())
        .await
        .unwrap();
    git.execute(
        &[
            "remote",
            "add",
            "origin",
            &format!("https://github.com/Fixture/{name}.git"),
        ],
        ExecuteOptions::default(),
    )
    .await
    .unwrap();
    root.to_string_lossy().into_owned()
}
#[tokio::test]
async fn independent_lanes_bound_pending_dedupe_and_keep_foreground_reads_available() {
    let f = Fixture::new();
    let first = repository(&f, "first").await;
    let second = repository(&f, "second").await;
    let foreground = repository(&f, "foreground").await;
    let dropped = repository(&f, "dropped").await;
    let (service, backend, mut started) = gated(
        &f,
        EnrichmentOptions {
            cache_capacity: 1,
            max_pending: 1,
            concurrency: 1,
        },
    );
    let mut changes = service.subscribe_changes();
    service.request(&first).await;
    let mut begun = vec![started.recv().await.unwrap(), started.recv().await.unwrap()];
    begun.sort();
    assert_eq!(begun, vec![(false, first.clone()), (true, first.clone())]);
    for _ in 0..10 {
        service.request(&first).await;
    }
    service.request(&dropped).await;
    assert_eq!(backend.counts.lock().unwrap().len(), 1);
    backend.identity_gate.add_permits(1);
    assert_eq!(changes.recv().await.unwrap().workspace_root, first);
    service.request(&second).await;
    assert_eq!(started.recv().await.unwrap(), (true, second.clone()));
    backend.identity_gate.add_permits(1);
    let next = changes.recv().await.unwrap();
    assert_eq!(next.workspace_root, second);
    assert!(next.enrichment.repository_identity_resolved);
    assert!(next.enrichment.favicon_path.is_none());
    let task = tokio::spawn({
        let service = service.clone();
        let foreground = foreground.clone();
        async move { service.read_repository_identity(&foreground).await }
    });
    assert_eq!(started.recv().await.unwrap(), (true, foreground.clone()));
    backend.identity_gate.add_permits(1);
    assert_eq!(task.await.unwrap().unwrap().name.unwrap().0, "foreground");
    assert!(!backend.counts.lock().unwrap().contains_key(&dropped));
    service.shutdown().await;
    f.shutdown().await;
}
#[tokio::test]
async fn invalidated_inflight_lookup_cannot_overwrite_a_newer_identity_and_caller_drop_is_safe() {
    let f = Fixture::new();
    let root = repository(&f, "old").await;
    let (service, backend, mut started) = gated(&f, EnrichmentOptions::default());
    let old = tokio::spawn({
        let service = service.clone();
        let root = root.clone();
        async move { service.read_repository_identity(&root).await }
    });
    assert_eq!(started.recv().await.unwrap(), (true, root.clone()));
    service.invalidate([root.clone()]).await;
    Git::new(&root)
        .with_environment(git_environment(f.temp.path()))
        .execute(
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/Fixture/New.git",
            ],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .read_repository_identity(&root)
            .await
            .unwrap()
            .name
            .unwrap()
            .0,
        "New"
    );
    assert_eq!(started.recv().await.unwrap(), (true, root.clone()));
    backend.identity_gate.add_permits(1);
    assert_eq!(old.await.unwrap().unwrap().name.unwrap().0, "old");
    assert_eq!(
        service
            .peek(&root)
            .await
            .repository_identity
            .unwrap()
            .name
            .unwrap()
            .0,
        "New"
    );
    let root = repository(&f, "disconnected").await;
    let task = tokio::spawn({
        let service = service.clone();
        let root = root.clone();
        async move { service.read_repository_identity(&root).await }
    });
    assert_eq!(started.recv().await.unwrap(), (true, root.clone()));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    backend.identity_gate.add_permits(1);
    assert!(service.read_repository_identity(&root).await.is_some());
    assert_eq!(backend.counts.lock().unwrap()[&root], 1);
    service.shutdown().await;
    f.shutdown().await;
}

struct CheckedMetadata {
    native: NativeProjectMetadata,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl ProjectMetadata for CheckedMetadata {
    async fn repository_identity(
        &self,
        root: &str,
        refresh: bool,
    ) -> Result<Option<RepositoryIdentity>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.native.favicon(root).await?;
        self.native.repository_identity(root, refresh).await
    }
    async fn favicon(&self, root: &str) -> Result<Option<String>> {
        self.native.favicon(root).await
    }
    async fn invalidate_favicon(&self, root: &str) {
        self.native.invalidate_favicon(root).await;
    }
}
#[tokio::test]
async fn successful_null_is_resolved_but_cold_and_failed_lookups_are_not() {
    let f = Fixture::new();
    let empty = f.root("empty").to_string_lossy().into_owned();
    let missing = f.temp.path().join("missing").to_string_lossy().into_owned();
    let backend = Arc::new(CheckedMetadata {
        native: NativeProjectMetadata::new(
            RepositoryIdentityResolver::default().with_environment(git_environment(f.temp.path())),
            Arc::new(t3_workspace::project::FaviconResolver::new(
                f.temp.path().into(),
            )),
        ),
        calls: Default::default(),
    });
    let service =
        ProjectEnrichmentService::with_backend(backend.clone(), EnrichmentOptions::default());
    assert!(!service.peek(&empty).await.repository_identity_resolved);
    assert!(service.read_repository_identity(&empty).await.is_none());
    assert!(service.peek(&empty).await.repository_identity_resolved);
    assert!(service.read_repository_identity(&missing).await.is_none());
    assert!(!service.peek(&missing).await.repository_identity_resolved);
    assert!(service.read_repository_identity(&missing).await.is_none());
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    service.shutdown().await;
    f.shutdown().await;
}
#[tokio::test]
async fn expired_identity_remains_available_until_invalidated() {
    let f = Fixture::new();
    let root = repository(&f, "expiry").await;
    let service = ProjectEnrichmentService::new(
        RepositoryIdentityResolver::default().with_environment(git_environment(f.temp.path())),
        Arc::new(t3_workspace::project::FaviconResolver::new(
            f.temp.path().into(),
        )),
    );
    let original = service.read_repository_identity(&root).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    let expired = service.peek(&root).await;
    assert!(!expired.repository_identity_resolved);
    assert_eq!(
        expired.repository_identity.unwrap().canonical_key,
        original.canonical_key
    );
    service.invalidate([root.clone()]).await;
    assert!(service.peek(&root).await.repository_identity.is_none());
    service.shutdown().await;
    f.shutdown().await;
}
