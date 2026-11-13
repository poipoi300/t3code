mod support;
use async_trait::async_trait;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use support::*;
use t3_contracts::pull_request::*;
use t3_pull_request::{
    PullRequestService, Result,
    cache::{MemoryStore, PullRequestReadCache},
    context::{Binding, Context},
    host::Host,
};
use tokio::sync::{Notify, Semaphore};

struct RacingViewedHost {
    caps: PullRequestCapabilities,
    viewed: Mutex<bool>,
    first: AtomicBool,
    reads: AtomicUsize,
    started: Notify,
    release: Semaphore,
}
#[async_trait]
impl Host for RacingViewedHost {
    fn kind(&self) -> &str {
        "github"
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.caps
    }
    async fn files_viewed(&self, _: &Context) -> Result<PullRequestFilesViewedResult> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        let viewed = *self.viewed.lock().expect("viewed");
        if self.first.swap(false, Ordering::AcqRel) {
            self.started.notify_one();
            self.release.acquire().await.expect("release").forget();
        }
        Ok(PullRequestFilesViewedResult {
            files: if viewed {
                vec![PullRequestFileViewed {
                    path: "a.rs".into(),
                    state: PullRequestFileViewedState::Viewed,
                }]
            } else {
                vec![]
            },
            truncated: false,
        })
    }
    async fn set_files_viewed(
        &self,
        _: &Context,
        input: &PullRequestSetFilesViewedInput,
    ) -> Result<()> {
        *self.viewed.lock().expect("viewed") = input.files[0].viewed;
        Ok(())
    }
}
#[tokio::test]
async fn an_older_viewed_read_cannot_repopulate_the_cache_after_an_untick() {
    let mut caps = capabilities();
    caps.viewed_files = Some(Some(PullRequestViewedFilesStore::Host));
    let host = Arc::new(RacingViewedHost {
        caps,
        viewed: Mutex::new(true),
        first: AtomicBool::new(true),
        reads: AtomicUsize::new(0),
        started: Notify::new(),
        release: Semaphore::new(0),
    });
    let source = Arc::new(Source(Mutex::new(vec![Binding {
        project: project(),
        host: host.clone(),
    }])));
    let clock = Arc::new(TestClock::default());
    let service = PullRequestService::new(
        source,
        clock.clone(),
        Arc::new(PullRequestReadCache::new(
            Arc::new(MemoryStore::default()),
            clock,
        )),
    );
    let older = {
        let service = service.clone();
        tokio::spawn(async move { service.files_viewed(reference()).await })
    };
    host.started.notified().await;
    service
        .set_files_viewed(decode(input(
            serde_json::json!({"files":[{"path":"a.rs","viewed":false}]}),
        )))
        .await
        .expect("untick");
    host.release.add_permits(1);
    assert_eq!(older.await.expect("task").expect("old read").files.len(), 1);
    assert!(
        service
            .files_viewed(reference())
            .await
            .expect("after untick")
            .files
            .is_empty()
    );
    assert_eq!(host.reads.load(Ordering::Acquire), 2);
    let refreshes = service.subscribe_refreshes();
    *host.viewed.lock().expect("viewed") = true;
    service
        .invalidate(
            decode(serde_json::json!({"reference":reference(),"filesViewedOnly":true})),
            true,
        )
        .await
        .expect("reference invalidation");
    assert_eq!(
        service
            .files_viewed(reference())
            .await
            .expect("updated host mark")
            .files
            .len(),
        1
    );
    *host.viewed.lock().expect("viewed") = false;
    service
        .invalidate(decode(serde_json::json!({"filesViewedOnly":true})), true)
        .await
        .expect("all viewed invalidation");
    assert!(
        service
            .files_viewed(reference())
            .await
            .expect("global refresh")
            .files
            .is_empty()
    );
    assert_eq!(host.reads.load(Ordering::Acquire), 4);
    assert!(!refreshes.has_changed().expect("refresh state"));
    service.shutdown().await;
}
