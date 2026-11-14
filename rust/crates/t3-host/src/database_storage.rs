use crate::{Error, Result, background::Clock};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::settings::{
    DatabaseReclaimError, DatabaseReclaimErrorTag, DatabaseStorageStatus, WireNumber,
};
use t3_persistence::Database;
use tokio::sync::{Mutex, watch};
#[derive(Debug, Clone, Copy)]
pub struct DiskSpace {
    pub database_free: u64,
    pub temp_free: u64,
    pub same_device: bool,
}
/// Available bytes (not total free blocks), for the database directory and SQLite's temp
/// directory. On Unix use the first writable/searchable SQLITE_TMPDIR, TMPDIR,
/// /var/tmp, /usr/tmp, /tmp, otherwise cwd; Windows uses the OS temp directory.
#[async_trait]
pub trait DatabaseStorageHost: Send + Sync {
    fn restart_available(&self) -> bool;
    async fn disk_space(&self, database_path: &Path) -> Result<DiskSpace>;
    /// Restart the server through its supervisor, after the reply has flushed.
    async fn restart(&self) -> Result<()>;
    /// Use ProjectionMaintenanceV2; call between_pages after each committed prune page.
    async fn prune_event_store(&self, pacer: &mut CompactionPacer) -> Result<()>;
}
pub struct CompactionPacer {
    since: tokio::time::Instant,
}
impl Default for CompactionPacer {
    fn default() -> Self {
        Self {
            since: tokio::time::Instant::now(),
        }
    }
}
impl CompactionPacer {
    pub async fn between_pages(&mut self) {
        let worked = self.since.elapsed();
        if worked >= Duration::from_millis(25) {
            tokio::time::sleep(worked.saturating_mul(9)).await;
            self.since = tokio::time::Instant::now();
        }
    }
}
pub fn space_shortfall(live: u64, space: DiskSpace) -> Option<(u64, u64)> {
    let copy = ((live as f64) * 1.1).ceil() as u64;
    let needed = if space.same_device {
        copy.saturating_mul(2)
    } else {
        copy
    };
    if space.database_free < needed {
        Some((needed, space.database_free))
    } else if !space.same_device && space.temp_free < copy {
        Some((copy, space.temp_free))
    } else {
        None
    }
}
pub struct DatabaseStorage {
    database: Database,
    path: PathBuf,
    doctor_target: bool,
    compact_command: String,
    host: Arc<dyn DatabaseStorageHost>,
    clock: Arc<dyn Clock>,
    status: watch::Sender<Option<DatabaseStorageStatus>>,
    request_gate: Mutex<()>,
}
pub struct StorageWorker {
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for StorageWorker {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl DatabaseStorage {
    pub async fn new(
        database: Database,
        base_dir: PathBuf,
        path: PathBuf,
        compact_command: String,
        host: Arc<dyn DatabaseStorageHost>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let expected = path
            .file_name()
            .map(|name| base_dir.join("userdata").join(name));
        let normalized = |p: &Path| {
            std::path::absolute(p).ok().map(|p| {
                if cfg!(windows) {
                    p.to_string_lossy().to_lowercase()
                } else {
                    p.to_string_lossy().into_owned()
                }
            })
        };
        let doctor_target = expected.is_some_and(|p| normalized(&p) == normalized(&path));
        let (status, _) = watch::channel(None);
        let service = Self {
            database,
            path,
            doctor_target,
            compact_command,
            host,
            clock,
            status,
            request_gate: Mutex::new(()),
        };
        let _ = service.refresh().await;
        service
    }
    pub fn status(&self) -> watch::Receiver<Option<DatabaseStorageStatus>> {
        self.status.subscribe()
    }
    pub async fn current(&self) -> Option<DatabaseStorageStatus> {
        self.status.borrow().clone()
    }
    async fn pages(&self) -> Result<(u64, u64, u64, i64)> {
        self.database
            .reader(|c| {
                let query = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0));
                Ok((
                    query("PRAGMA page_count")?.max(0) as u64,
                    query("PRAGMA freelist_count")?.max(0) as u64,
                    query("PRAGMA page_size")?.max(0) as u64,
                    query("PRAGMA auto_vacuum")?,
                ))
            })
            .await
            .map_err(|_| Error::Service)
    }
    pub async fn refresh(&self) -> Result<()> {
        if !self.doctor_target {
            return Ok(());
        }
        let (pages, free, size, mode) = self.pages().await?;
        let status = DatabaseStorageStatus {
            database_bytes: WireNumber::Finite(pages.saturating_mul(size).into()),
            reclaimable_bytes: WireNumber::Finite(free.saturating_mul(size).into()),
            incremental_vacuum_enabled: mode == 2,
            compact_command: self.compact_command.clone(),
            can_reclaim: Some(self.host.restart_available()),
            reclaim_failure: std::fs::read_to_string(self.sibling("reclaim-failed"))
                .ok()
                .filter(|s| !s.is_empty()),
        };
        self.status.send_if_modified(|latest| {
            if latest.as_ref() == Some(&status) {
                false
            } else {
                *latest = Some(status);
                true
            }
        });
        Ok(())
    }
    pub async fn reclaim(&self) -> Result<u64> {
        let mut count = 0;
        if self.pages().await?.3 == 2 {
            for _ in 0..1024 {
                let reclaimed = self
                    .database
                    .writer(|c| {
                        let free: i64 = c.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
                        if free == 0 {
                            return Ok(false);
                        }
                        c.execute_batch("PRAGMA incremental_vacuum(1)")?;
                        Ok(true)
                    })
                    .await
                    .map_err(|_| Error::Service)?;
                if !reclaimed {
                    break;
                }
                count += 1;
            }
        }
        self.refresh().await?;
        Ok(count)
    }
    fn sibling(&self, name: &str) -> PathBuf {
        self.path.with_file_name(name)
    }
    pub async fn compact_history(&self) -> Result<()> {
        self.host
            .prune_event_store(&mut CompactionPacer::default())
            .await?;
        std::fs::write(
            self.sibling("compacted-at"),
            self.clock.now_ms().to_string(),
        )
        .map_err(|_| Error::Io)
    }
    pub fn start(self: &Arc<Self>) -> StorageWorker {
        let service = self.clone();
        let maintenance = tokio::spawn(async move {
            loop {
                let _ = service.reclaim().await;
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
        let service = self.clone();
        let history = tokio::spawn(async move {
            let interval = 6 * 60 * 60 * 1000i64;
            let last = std::fs::read_to_string(service.sibling("compacted-at"))
                .ok()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let delay = last
                .saturating_add(interval)
                .saturating_sub(service.clock.now_ms())
                .clamp(60_000, interval);
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
            loop {
                let _ = service.compact_history().await;
                tokio::time::sleep(Duration::from_millis(interval as u64)).await;
            }
        });
        StorageWorker {
            tasks: vec![maintenance, history],
        }
    }
    pub async fn request_reclaim(
        self: &Arc<Self>,
    ) -> std::result::Result<(), DatabaseReclaimError> {
        let error = |reason: String| DatabaseReclaimError {
            _tag: DatabaseReclaimErrorTag::DatabaseReclaimError,
            reason,
        };
        let _gate = self.request_gate.lock().await;
        if !self.host.restart_available() || !self.doctor_target {
            return Err(error(format!(
                "Stop T3 Code on this machine, then run: {}",
                self.compact_command
            )));
        }
        let (pages, free, size, mode) = self
            .pages()
            .await
            .map_err(|_| error("Could not prepare the repair. Try again.".into()))?;
        if mode != 0 {
            return Err(error(
                "This database already shrinks on its own. Nothing to reclaim.".into(),
            ));
        }
        let space = self
            .host
            .disk_space(&self.path)
            .await
            .map_err(|_| error("Could not prepare the repair. Try again.".into()))?;
        if let Some((needed, free)) =
            space_shortfall(pages.saturating_sub(free).saturating_mul(size), space)
        {
            return Err(error(format!(
                "Reclaiming needs {:.1} GiB of free disk space on this machine. {:.1} GiB is free.",
                needed as f64 / 1073741824.0,
                free as f64 / 1073741824.0
            )));
        }
        let service = self.clone();
        tokio::spawn(async move {
            match std::fs::remove_file(service.sibling("reclaim-failed")) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Error::Io),
            }
            service.refresh().await?;
            std::fs::write(
                service.sibling("reclaim-requested"),
                service.clock.now_ms().to_string(),
            )
            .map_err(|_| Error::Io)?;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = service.host.restart().await;
            });
            Ok(())
        })
        .await
        .map_err(|_| error("Could not prepare the repair. Try again.".into()))?
        .map_err(|_| error("Could not prepare the repair. Try again.".into()))
    }
}

#[derive(Debug)]
pub enum RepairFailure {
    InUse,
    Failed(String),
}
#[async_trait]
pub trait OfflineRepair: Send + Sync {
    /// Hold exclusive database ownership; prune events/receipts, VACUUM with
    /// auto_vacuum=INCREMENTAL, rebuild message search, then checkpoint(TRUNCATE).
    /// Use the persistence owner's connection, after all server DB handles close.
    async fn compact(&self, database: &Path) -> std::result::Result<(), RepairFailure>;
}
pub async fn run_requested_reclaim(database: &Path, repair: Arc<dyn OfflineRepair>) -> Result<()> {
    let request = database.with_file_name("reclaim-requested");
    let failure = database.with_file_name("reclaim-failed");
    match std::fs::remove_file(request) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(Error::Io),
    }
    let mut attempt = 0;
    loop {
        match repair.compact(database).await {
            Ok(()) => {
                match std::fs::remove_file(failure) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(Error::Io),
                }
                return Ok(());
            }
            Err(RepairFailure::InUse) if attempt < 10 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => {
                let reason=match error{RepairFailure::InUse=>"Another program kept the database open. Close other T3 Code apps and t3 commands on this machine, then try again.".to_owned(),RepairFailure::Failed(reason)=>reason};
                std::fs::write(failure, reason).map_err(|_| Error::Io)?;
                return Ok(());
            }
        }
    }
}
