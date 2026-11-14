use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use t3_host::{Result, background::Clock, database_storage::*};
use tokio::sync::Notify;
struct Time;
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        1800000000000
    }
}
struct Host {
    restart: bool,
    space: DiskSpace,
    restarted: Notify,
}
#[async_trait]
impl DatabaseStorageHost for Host {
    fn restart_available(&self) -> bool {
        self.restart
    }
    async fn disk_space(&self, _: &Path) -> Result<DiskSpace> {
        Ok(self.space)
    }
    async fn restart(&self) -> Result<()> {
        self.restarted.notify_one();
        Ok(())
    }
    async fn prune_event_store(&self, _: &mut CompactionPacer) -> Result<()> {
        Ok(())
    }
}
fn host(restart: bool, free: u64) -> Arc<Host> {
    Arc::new(Host {
        restart,
        space: DiskSpace {
            database_free: free,
            temp_free: free,
            same_device: true,
        },
        restarted: Notify::new(),
    })
}
#[tokio::test]
async fn reclaims_at_most_1024_pages_without_losing_retained_rows() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("userdata/statev2.sqlite");
    let db = t3_persistence::Database::open_v2(&db_path, Default::default()).unwrap();
    db.writer(|c|{c.execute_batch("CREATE TABLE retained(value TEXT);INSERT INTO retained VALUES('keep');CREATE TABLE discarded(value BLOB);INSERT INTO discarded VALUES(zeroblob(6291456));DROP TABLE discarded;")?;Ok(())}).await.unwrap();
    let s = DatabaseStorage::new(
        db.clone(),
        temp.path().into(),
        db_path,
        "t3 doctor".into(),
        host(false, u64::MAX),
        Arc::new(Time),
    )
    .await;
    let before = s.current().await.unwrap();
    assert_eq!(s.reclaim().await.unwrap(), 1024);
    let after = s.current().await.unwrap();
    assert_ne!(before.reclaimable_bytes, after.reclaimable_bytes);
    assert!(s.reclaim().await.unwrap() > 0);
    assert_eq!(s.reclaim().await.unwrap(), 0);
    db.reader(|c| {
        assert_eq!(
            c.query_row("SELECT value FROM retained", [], |r| r.get::<_, String>(0))?,
            "keep"
        );
        Ok(())
    })
    .await
    .unwrap();
}
#[tokio::test(start_paused = true)]
async fn repair_refusals_write_no_marker_and_success_restarts_after_reply() {
    for mode in [0, 1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("userdata/statev2.sqlite");
        let db = t3_persistence::Database::open_v2(&db_path, Default::default()).unwrap();
        db.writer(move |c| {
            c.execute_batch(&format!("PRAGMA auto_vacuum={mode};VACUUM;"))?;
            Ok(())
        })
        .await
        .unwrap();
        for (restart, free) in [(false, u64::MAX), (true, 0), (true, u64::MAX)] {
            let h = host(restart, free);
            let s = Arc::new(
                DatabaseStorage::new(
                    db.clone(),
                    temp.path().into(),
                    db_path.clone(),
                    "t3 doctor".into(),
                    h.clone(),
                    Arc::new(Time),
                )
                .await,
            );
            let result = s.request_reclaim().await;
            let request = db_path.parent().unwrap().join("reclaim-requested");
            if mode == 0 && restart && free == u64::MAX {
                result.unwrap();
                assert_eq!(std::fs::read_to_string(&request).unwrap(), "1800000000000");
                tokio::time::advance(std::time::Duration::from_millis(500)).await;
                h.restarted.notified().await;
            } else {
                assert!(result.is_err());
                assert!(!request.exists());
            }
        }
    }
}
#[tokio::test]
async fn database_outside_doctor_target_is_hidden_and_cannot_request_repair() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("elsewhere.sqlite");
    let db = t3_persistence::Database::open_v2(&path, Default::default()).unwrap();
    let s = Arc::new(
        DatabaseStorage::new(
            db,
            temp.path().into(),
            path,
            "t3 doctor".into(),
            host(true, u64::MAX),
            Arc::new(Time),
        )
        .await,
    );
    assert!(s.current().await.is_none());
    assert!(s.request_reclaim().await.is_err());
    assert!(!temp.path().join("reclaim-requested").exists());
}
#[test]
fn vacuum_space_counts_temp_copy_and_wal_on_their_devices() {
    assert_eq!(
        space_shortfall(
            100,
            DiskSpace {
                database_free: 219,
                temp_free: 999,
                same_device: true
            }
        ),
        Some((222, 219))
    );
    assert_eq!(
        space_shortfall(
            100,
            DiskSpace {
                database_free: 111,
                temp_free: 110,
                same_device: false
            }
        ),
        Some((111, 110))
    );
    assert_eq!(
        space_shortfall(
            100,
            DiskSpace {
                database_free: 222,
                temp_free: 222,
                same_device: true
            }
        ),
        None
    );
}

#[tokio::test(start_paused = true)]
async fn exit_repair_consumes_once_retries_only_busy_and_retains_failure_for_settings() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Repair {
        calls: AtomicUsize,
        fail: bool,
    }
    #[async_trait]
    impl OfflineRepair for Repair {
        async fn compact(&self, _: &Path) -> std::result::Result<(), RepairFailure> {
            let count = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(RepairFailure::Failed("Database compaction failed.".into()))
            } else if count < 2 {
                Err(RepairFailure::InUse)
            } else {
                Ok(())
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("statev2.sqlite");
    let request = temp.path().join("reclaim-requested");
    let failure = temp.path().join("reclaim-failed");
    std::fs::write(&request, "requested").unwrap();
    std::fs::write(&failure, "old failure").unwrap();
    let repair = Arc::new(Repair {
        calls: AtomicUsize::new(0),
        fail: false,
    });
    run_requested_reclaim(&path, repair.clone()).await.unwrap();
    assert_eq!(repair.calls.load(Ordering::SeqCst), 3);
    assert!(!request.exists());
    assert!(!failure.exists());
    run_requested_reclaim(&path, repair.clone()).await.unwrap();
    assert_eq!(repair.calls.load(Ordering::SeqCst), 3);
    std::fs::write(&request, "requested").unwrap();
    let repair = Arc::new(Repair {
        calls: AtomicUsize::new(0),
        fail: true,
    });
    run_requested_reclaim(&path, repair.clone()).await.unwrap();
    assert_eq!(repair.calls.load(Ordering::SeqCst), 1);
    assert!(!request.exists());
    assert_eq!(
        std::fs::read_to_string(&failure).unwrap(),
        "Database compaction failed."
    );
}
