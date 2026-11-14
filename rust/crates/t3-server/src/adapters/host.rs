//! The only contact surface with `t3-host`; compiled with the `host` feature.
//!
//! Background policy leases (`background/BackgroundPolicy.ts`): clients report their activity
//! per socket, and the leases of a socket are dropped when it closes. The npm release lookup,
//! triage preparation and database storage services are constructed once here.
use std::sync::Arc;
use t3_contracts::base_schemas::{AuthSessionId, RpcClientId};
use t3_host::{
    background::{BackgroundPolicy, Clock, HostPowerMonitor, SystemClock},
    npm_release::{NativeNpmReleaseHost, NpmRelease},
    triage::{TriageMachine, TriagePaths, TriageService},
};

pub struct HostAdapter {
    pub background: BackgroundPolicy,
    pub npm: NpmRelease,
    pub triage: TriageService,
    #[cfg(feature = "orchestration")]
    pub storage: storage::Storage,
}

impl HostAdapter {
    /// Host power starts unknown and stale: no native power source is assembled, so host
    /// constraints never pause work (TypeScript treats a stale snapshot the same way).
    #[cfg(feature = "local-services")]
    pub async fn start(context: &crate::services::DomainContext<'_>) -> crate::Result<Arc<Self>> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let power = HostPowerMonitor::unknown(clock.now_ms())
            .map_err(|e| crate::Error::Service(e.to_string()))?;
        let paths = &context.local.config.paths;
        let triage = TriageService::new(
            TriagePaths {
                base_dir: paths.base_dir.clone(),
                state_dir: paths.state_dir.clone(),
                db_path: paths.database(),
                settings_path: paths.settings(),
                logs_dir: paths.logs(),
                server_trace_path: paths.server_trace(),
                provider_event_log_path: paths.provider_events(),
                terminal_logs_dir: paths.terminal_logs(),
                provider_status_cache_dir: paths.caches(),
                secrets_dir: paths.secrets(),
            },
            TriageMachine {
                os: std::env::consts::OS.into(),
                runtime_version: "native".into(),
                server_version: env!("CARGO_PKG_VERSION").into(),
                pid: std::process::id(),
            },
            clock.clone(),
        );
        Ok(Arc::new(Self {
            background: BackgroundPolicy::new(clock.clone(), Arc::new(power)),
            npm: NpmRelease::new(Arc::new(NativeNpmReleaseHost::default())),
            triage,
            #[cfg(feature = "orchestration")]
            storage: storage::Storage::start(context, clock).await,
        }))
    }

    /// The RPC client id of a socket: one per connection, as Effect's RPC client id is.
    pub fn rpc_client(socket: u64) -> Option<RpcClientId> {
        serde_json::from_value(serde_json::json!(socket)).ok()
    }

    /// Drops every lease the socket reported; called when it closes.
    pub async fn socket_closed(&self, session: &AuthSessionId, socket: u64) {
        if let Some(client) = Self::rpc_client(socket) {
            self.background.remove_rpc_client(session, &client).await;
        }
    }
}

impl crate::services::DomainRuntime for HostAdapter {
    fn name(&self) -> &'static str {
        "host"
    }
    /// Stops the storage workers and joins triage's source clones while SQLite is open.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            #[cfg(feature = "orchestration")]
            self.storage.stop();
            self.triage.drain().await;
            Ok(())
        })
    }
}

/// `DatabaseStorage` (`persistence/DatabaseStorage.ts`) over the server's one database.
#[cfg(feature = "orchestration")]
pub mod storage {
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };
    use t3_host::{
        Error as HostError,
        background::Clock,
        database_storage::{
            CompactionPacer, DatabaseStorage, DatabaseStorageHost, DiskSpace, StorageWorker,
        },
    };
    use t3_orchestration::ports::runs_ingestion::projection_maintenance::{
        ProjectionMaintenance, PrunePageObserver,
    };

    pub struct Storage {
        pub service: Arc<DatabaseStorage>,
        worker: Mutex<Option<StorageWorker>>,
    }
    impl Storage {
        #[cfg(feature = "local-services")]
        pub async fn start(
            context: &crate::services::DomainContext<'_>,
            clock: Arc<dyn Clock>,
        ) -> Self {
            let paths = &context.local.config.paths;
            let host = Arc::new(Host {
                maintenance: ProjectionMaintenance {
                    database: t3_orchestration::stores::StoreDatabase(context.database.clone()),
                    clock: Arc::new(crate::adapters::orchestration::SystemClock),
                },
            });
            let service = Arc::new(
                DatabaseStorage::new(
                    context.database.clone(),
                    paths.base_dir.clone(),
                    paths.database(),
                    doctor_command(&paths.base_dir),
                    host,
                    clock,
                )
                .await,
            );
            let worker = service.start();
            Self {
                service,
                worker: Mutex::new(Some(worker)),
            }
        }
        pub fn stop(&self) {
            drop(self.worker.lock().unwrap_or_else(|e| e.into_inner()).take());
        }
    }

    /// The `doctor` command users run with the server stopped. TypeScript's
    /// `resolveHostCliCommand` also resolves an install launcher; this is its plain form, used
    /// when `t3` on PATH is this install.
    fn doctor_command(base_dir: &Path) -> String {
        let base = base_dir.to_string_lossy();
        let quoted = if cfg!(windows) {
            format!("'{}'", base.replace('\'', "''"))
        } else {
            format!("'{}'", base.replace('\'', r"'\''"))
        };
        format!("t3 doctor --base-dir {quoted}")
    }

    struct Host {
        maintenance: ProjectionMaintenance,
    }

    struct Pacer<'a>(tokio::sync::Mutex<&'a mut CompactionPacer>);
    #[async_trait::async_trait]
    impl PrunePageObserver for Pacer<'_> {
        async fn between_pages(&self) -> t3_orchestration::error::Result<()> {
            self.0.lock().await.between_pages().await;
            Ok(())
        }
    }

    /// The filesystem holding `path`: the disk whose mount point is its longest prefix.
    fn disk_of(disks: &sysinfo::Disks, path: &Path) -> Option<(PathBuf, u64)> {
        let path = std::path::absolute(path).ok()?;
        disks
            .list()
            .iter()
            .filter(|disk| path.starts_with(disk.mount_point()))
            .max_by_key(|disk| disk.mount_point().as_os_str().len())
            .map(|disk| (disk.mount_point().to_owned(), disk.available_space()))
    }

    #[async_trait::async_trait]
    impl DatabaseStorageHost for Host {
        /// No supervisor can restart this server with every database handle closed, and no
        /// offline repair is assembled: reclaiming answers with the manual doctor command.
        fn restart_available(&self) -> bool {
            false
        }
        async fn disk_space(&self, database_path: &Path) -> t3_host::Result<DiskSpace> {
            let database = database_path.parent().unwrap_or(database_path).to_owned();
            tokio::task::spawn_blocking(move || {
                let disks = sysinfo::Disks::new_with_refreshed_list();
                let (database_mount, database_free) =
                    disk_of(&disks, &database).ok_or(HostError::Io)?;
                let (temp_mount, temp_free) =
                    disk_of(&disks, &std::env::temp_dir()).ok_or(HostError::Io)?;
                Ok(DiskSpace {
                    database_free,
                    temp_free,
                    same_device: database_mount == temp_mount,
                })
            })
            .await
            .map_err(|_| HostError::Service)?
        }
        async fn restart(&self) -> t3_host::Result<()> {
            Err(HostError::Service)
        }
        async fn prune_event_store(&self, pacer: &mut CompactionPacer) -> t3_host::Result<()> {
            let observer = Pacer(tokio::sync::Mutex::new(pacer));
            self.maintenance
                .prune_event_store(Some(&observer))
                .await
                .map(|_| ())
                .map_err(|_| HostError::Service)
        }
    }
}
