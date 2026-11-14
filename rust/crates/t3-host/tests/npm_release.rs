use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_host::npm_release::*;
struct Host {
    npm: bool,
    calls: AtomicUsize,
    status: u16,
    body: Vec<u8>,
}
#[async_trait]
impl NpmReleaseHost for Host {
    async fn npm_command(&self) -> Option<PathBuf> {
        self.npm.then(|| PathBuf::from("npm"))
    }
    async fn npm_view(&self, _: &std::path::Path, spec: &str) -> LookupResult<NpmOutput> {
        assert!(valid_spec(spec));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(NpmOutput {
            code: if self.status == 200 { 0 } else { 1 },
            stdout: self.body.clone(),
            incomplete: false,
        })
    }
    async fn public_registry(&self, spec: &str) -> LookupResult<(u16, Vec<u8>)> {
        assert!(!self.npm, "installed npm must never fall back");
        assert!(valid_spec(spec));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((self.status, self.body.clone()))
    }
}
#[tokio::test(start_paused = true)]
async fn validates_specs_metadata_cache_and_configured_registry_failures() {
    for body in [
        r#"{"version":"1.2.3","t3code":{"databaseMigration":91}}"#,
        r#"[{"version":"1.2.3","t3code":{"databaseMigration":91}}]"#,
    ] {
        let h = Arc::new(Host {
            npm: true,
            calls: AtomicUsize::new(0),
            status: 200,
            body: body.as_bytes().into(),
        });
        let s = NpmRelease::new(h.clone());
        for spec in [
            "--registry=secret",
            "../other",
            "x@latest",
            "*",
            "",
            "1.2.3+build",
        ] {
            assert!(s.get(spec).await.is_err());
        }
        assert_eq!(h.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            s.get("1.2.3").await.unwrap().unwrap().database_migration,
            Some(91)
        );
        s.get("1.2.3").await.unwrap();
        assert_eq!(h.calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        s.get("1.2.3").await.unwrap();
        assert_eq!(h.calls.load(Ordering::SeqCst), 2);
        assert!(s.get("9.9.9").await.is_err());
    }
    for npm in [true, false] {
        for status in [404, 401] {
            let h = Arc::new(Host {
                npm,
                calls: AtomicUsize::new(0),
                status,
                body: br#"{"error":{"code":"E404","detail":"sensitive"}}"#.to_vec(),
            });
            let s = NpmRelease::new(h);
            let result = s.get("latest").await;
            if status == 404 || npm {
                assert!(result.unwrap().is_none());
            } else {
                assert!(!format!("{:?}", result.unwrap_err()).contains("sensitive"));
            }
        }
    }
}
#[tokio::test]
async fn rejects_incomplete_unbounded_invalid_and_mismatched_metadata() {
    for body in [
        b"not json".to_vec(),
        b"[]".to_vec(),
        br#"{"version":"9.9.9"}"#.to_vec(),
        vec![b' '; 256 * 1024 + 1],
    ] {
        let s = NpmRelease::new(Arc::new(Host {
            npm: true,
            calls: AtomicUsize::new(0),
            status: 200,
            body,
        }));
        assert!(s.get("1.2.3").await.is_err());
    }
}

#[tokio::test(start_paused = true)]
async fn lookups_are_single_flight_bounded_to_two_and_stalled_hosts_time_out() {
    use tokio::sync::{Semaphore, mpsc};
    struct Gate {
        started: mpsc::UnboundedSender<()>,
        release: Semaphore,
        running: AtomicUsize,
        peak: AtomicUsize,
        public: bool,
    }
    #[async_trait]
    impl NpmReleaseHost for Gate {
        async fn npm_command(&self) -> Option<PathBuf> {
            (!self.public).then(|| "npm".into())
        }
        async fn npm_view(&self, _: &std::path::Path, _: &str) -> LookupResult<NpmOutput> {
            let count = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(count, Ordering::SeqCst);
            self.started.send(()).unwrap();
            let permit = self.release.acquire().await.unwrap();
            permit.forget();
            self.running.fetch_sub(1, Ordering::SeqCst);
            Ok(NpmOutput {
                code: 0,
                stdout: br#"{"version":"1.2.3"}"#.to_vec(),
                incomplete: false,
            })
        }
        async fn public_registry(&self, _: &str) -> LookupResult<(u16, Vec<u8>)> {
            self.started.send(()).unwrap();
            std::future::pending().await
        }
    }
    let (tx, mut rx) = mpsc::unbounded_channel();
    let h = Arc::new(Gate {
        started: tx,
        release: Semaphore::new(0),
        running: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        public: false,
    });
    let s = Arc::new(NpmRelease::new(h.clone()));
    let mut tasks = vec![];
    for spec in ["latest", "nightly", "1.2.3", "latest"] {
        let s = s.clone();
        tasks.push(tokio::spawn(async move { s.get(spec).await }));
    }
    rx.recv().await.unwrap();
    rx.recv().await.unwrap();
    h.release.add_permits(3);
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(h.peak.load(Ordering::SeqCst), 2);
    assert_eq!(rx.try_recv(), Ok(()));
    assert!(rx.try_recv().is_err());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = Arc::new(NpmRelease::new(Arc::new(Gate {
        started: tx,
        release: Semaphore::new(0),
        running: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        public: true,
    })));
    let task = {
        let s = s.clone();
        tokio::spawn(async move { s.get("latest").await })
    };
    rx.recv().await.unwrap();
    tokio::time::advance(std::time::Duration::from_secs(15)).await;
    assert!(task.await.unwrap().is_err());
}

#[tokio::test]
async fn version_digits_are_ascii_and_integer_migration_metadata_accepts_json_decimal_notation() {
    let s = NpmRelease::new(Arc::new(Host {
        npm: true,
        calls: AtomicUsize::new(0),
        status: 200,
        body: br#"{"version":"1.2.3","t3code":{"databaseMigration":91.0}}"#.to_vec(),
    }));
    assert!(!valid_spec("١.2.3"));
    assert_eq!(
        s.get("1.2.3").await.unwrap().unwrap().database_migration,
        Some(91)
    );
}
