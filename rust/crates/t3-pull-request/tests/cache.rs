mod support;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::TestClock;
use t3_pull_request::{
    Error,
    cache::{CacheStore, FileStore, MemoryStore, PullRequestReadCache},
};
use tokio::sync::oneshot;

#[tokio::test]
async fn persistent_scope_invalidation_survives_service_restart_and_ttl_expires() {
    let dir = tempfile::tempdir().expect("temp");
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(FileStore::open(dir.path()).await.expect("store"));
    let cache = PullRequestReadCache::new(store.clone(), clock.clone());
    let scopes = ["project:p".into(), "pr".into()];
    assert_eq!(
        cache
            .get("summary", &scopes, async { Ok("old".to_string()) })
            .await
            .expect("first"),
        "old"
    );
    cache.invalidate("pr").await;
    let cache = PullRequestReadCache::new(store.clone(), clock.clone());
    assert_eq!(
        cache
            .get("summary", &scopes, async { Ok("new".to_string()) })
            .await
            .expect("after restart"),
        "new"
    );
    clock.0.store(60_001, Ordering::Release);
    assert_eq!(
        cache
            .get("summary", &scopes, async { Ok("expired".to_string()) })
            .await
            .expect("expired"),
        "expired"
    );
}

#[tokio::test]
async fn invalidation_waits_for_inflight_scoped_read_before_restart() {
    let store = Arc::new(MemoryStore::default());
    let clock = Arc::new(TestClock::default());
    let cache = Arc::new(PullRequestReadCache::new(store.clone(), clock.clone()));
    let (started, entered) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let reader = {
        let cache = cache.clone();
        tokio::spawn(async move {
            cache
                .get("summary", &["pr".into()], async move {
                    started.send(()).expect("started");
                    released.await.expect("release");
                    Ok("old".to_string())
                })
                .await
        })
    };
    entered.await.expect("entered");
    let invalidate = cache.invalidate("pr");
    tokio::pin!(invalidate);
    assert!(futures_util::poll!(&mut invalidate).is_pending());
    release.send(()).expect("release");
    assert_eq!(reader.await.expect("task").expect("read"), "old");
    invalidate.await;
    let restarted = PullRequestReadCache::new(store, clock);
    assert_eq!(
        restarted
            .get("summary", &["pr".into()], async { Ok("new".to_string()) })
            .await
            .expect("restarted"),
        "new"
    );
}

#[tokio::test]
async fn failed_reads_are_not_persisted_and_corrupt_revisions_clear_entries() {
    let store = Arc::new(MemoryStore::default());
    let clock = Arc::new(TestClock::default());
    let cache = PullRequestReadCache::new(store.clone(), clock.clone());
    let calls = AtomicUsize::new(0);
    for _ in 0..2 {
        let result: Result<String, _> = cache
            .get("failure", &[], async {
                calls.fetch_add(1, Ordering::AcqRel);
                Err(Error::invalid("summary", "failed"))
            })
            .await;
        assert!(result.is_err());
    }
    assert_eq!(calls.load(Ordering::Acquire), 2);
    cache
        .get("summary", &["pr".into()], async { Ok("old".to_string()) })
        .await
        .expect("seed");
    store.set("revisions", "not json").await.expect("corrupt");
    let cache = PullRequestReadCache::new(store, clock);
    assert_eq!(
        cache
            .get("summary", &["pr".into()], async { Ok("fresh".to_string()) })
            .await
            .expect("recover"),
        "fresh"
    );
}

struct FailingStore;
#[async_trait::async_trait]
impl CacheStore for FailingStore {
    async fn get(&self, _: &str) -> std::io::Result<Option<String>> {
        Ok(None)
    }
    async fn set(&self, _: &str, _: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("failed"))
    }
    async fn clear(&self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn failed_invalidation_disables_cache_and_retains_lookup_error() {
    let cache = PullRequestReadCache::new(Arc::new(FailingStore), Arc::new(TestClock::default()));
    cache.invalidate("pr").await;
    let error = cache
        .get::<String, _>("key", &[], async {
            Err(Error::invalid("summary", "Host error."))
        })
        .await
        .expect_err("host error");
    assert_eq!(error.detail, "Host error.");
}

#[tokio::test]
async fn file_store_never_uses_untrusted_key_as_a_path_and_clear_preserves_neighbors() {
    let dir = tempfile::tempdir().expect("temp");
    let store = FileStore::open(dir.path()).await.expect("store");
    tokio::fs::write(dir.path().join("keep.txt"), "keep")
        .await
        .expect("neighbor");
    store.set("../../escape", "payload").await.expect("write");
    assert_eq!(
        store.get("../../escape").await.expect("read").as_deref(),
        Some("payload")
    );
    store.clear().await.expect("clear");
    assert!(dir.path().join("keep.txt").is_file());
    assert!(store.get("../../escape").await.expect("gone").is_none());
}

struct BlockingStore {
    inner: MemoryStore,
    started: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
#[async_trait::async_trait]
impl CacheStore for BlockingStore {
    async fn get(&self, key: &str) -> std::io::Result<Option<String>> {
        self.inner.get(key).await
    }
    async fn set(&self, key: &str, value: &str) -> std::io::Result<()> {
        if key == "revisions" {
            self.started.notify_one();
            self.release.acquire().await.expect("release").forget();
        }
        self.inner.set(key, value).await
    }
    async fn clear(&self) -> std::io::Result<()> {
        self.inner.clear().await
    }
}
#[tokio::test]
async fn canceling_invalidation_caller_does_not_cancel_its_persisted_revision() {
    let store = Arc::new(BlockingStore {
        inner: Default::default(),
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    let clock = Arc::new(TestClock::default());
    let cache = Arc::new(PullRequestReadCache::new(store.clone(), clock.clone()));
    cache
        .get("key", &["pr".into()], async { Ok("old".to_string()) })
        .await
        .expect("seed");
    let invalidation = {
        let cache = cache.clone();
        tokio::spawn(async move {
            cache.invalidate("pr").await;
        })
    };
    store.started.notified().await;
    invalidation.abort();
    assert!(invalidation.await.expect_err("aborted").is_cancelled());
    store.release.add_permits(1);
    cache.drain().await;
    let restarted = PullRequestReadCache::new(store, clock);
    assert_eq!(
        restarted
            .get("key", &["pr".into()], async { Ok("new".to_string()) })
            .await
            .expect("refreshed"),
        "new"
    );
}

#[tokio::test]
async fn identical_concurrent_successful_reads_share_one_lookup() {
    let cache = Arc::new(PullRequestReadCache::new(
        Arc::new(MemoryStore::default()),
        Arc::new(TestClock::default()),
    ));
    let (started, entered) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let first = {
        let cache = cache.clone();
        tokio::spawn(async move {
            cache
                .get("key", &[], async move {
                    started.send(()).expect("started");
                    released.await.expect("release");
                    Ok("first".to_string())
                })
                .await
        })
    };
    entered.await.expect("entered");
    let second = cache.get("key", &[], async { Ok("second".to_string()) });
    tokio::pin!(second);
    assert!(futures_util::poll!(&mut second).is_pending());
    release.send(()).expect("release");
    assert_eq!(first.await.expect("task").expect("first"), "first");
    assert_eq!(second.await.expect("second"), "first");
}
