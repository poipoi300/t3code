use crate::{Result, files, home::HomeLayout};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedPolicy {
    pub source: String,
    pub organization: String,
    pub fingerprint: String,
}

/// Local disk type from `serverRuntimeState.ts`; to move to t3-contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    #[serde(deserialize_with = "version_one")]
    pub version: u8,
    pub pid: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_network_only: Option<bool>,
    pub port: i64,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_url: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_managed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub managed_policy: Option<Option<LoadedPolicy>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
}
impl RuntimeState {
    pub fn for_config(
        config: &crate::config::ResolvedConfig,
        port: u16,
        started_at: String,
        server_version: String,
        service_managed: bool,
        managed_policy: Option<Option<LoadedPolicy>>,
    ) -> Self {
        Self {
            version: 1,
            pid: i64::from(std::process::id()),
            owner_id: None,
            host: config.host.clone().filter(|s| !s.is_empty()),
            private_network_only: config.private_network_only.then_some(true),
            port: i64::from(port),
            origin: runtime_origin(config.host.as_deref(), i64::from(port)),
            dev_url: config.dev_url.as_ref().map(ToString::to_string),
            started_at,
            service_managed: service_managed.then_some(true),
            server_version: Some(server_version),
            managed_policy,
        }
    }
}
fn version_one<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u8, D::Error> {
    let v = u8::deserialize(d)?;
    if v == 1 {
        Ok(v)
    } else {
        Err(serde::de::Error::custom("expected version 1"))
    }
}
pub fn runtime_origin(host: Option<&str>, port: i64) -> String {
    let host = host
        .filter(|s| !matches!(*s, "" | "0.0.0.0" | "::" | "[::]"))
        .unwrap_or("127.0.0.1");
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    format!("http://{host}:{port}")
}
pub fn read_runtime_state(path: &Path) -> Option<RuntimeState> {
    match files::read(path) {
        Ok(Some(raw)) if !raw.trim().is_empty() => match serde_json::from_str(raw.trim()) {
            Ok(state) => Some(state),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "Failed to decode server runtime state");
                None
            }
        },
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(%error, "Failed to read server runtime state");
            None
        }
    }
}
#[cfg(unix)]
pub fn is_process_alive(pid: i64) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    matches!(
        rustix::process::test_kill_process(pid),
        Ok(()) | Err(rustix::io::Errno::PERM)
    )
}
#[cfg(not(unix))]
pub fn is_process_alive(pid: i64) -> bool {
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    if pid == std::process::id() {
        return true;
    }
    let mut system = sysinfo::System::new();
    system.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        true,
    );
    system.process(sysinfo::Pid::from_u32(pid)).is_some()
}
pub fn read_live_runtime_state(path: &Path) -> Option<RuntimeState> {
    read_runtime_state(path).filter(|s| is_process_alive(s.pid))
}
pub async fn persist_runtime_state(path: &Path, state: &RuntimeState) -> Result<()> {
    files::write_json(path, state, false).await
}
pub fn clear_runtime_state(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, path = %path.display(), "Failed to clear server runtime state");
    }
}

/// Holds the persistence crate's SQLite transaction for the whole server lifetime.
pub struct ServerOwner {
    _lock: t3_persistence::OwnershipLock,
    path: PathBuf,
    pub owner_id: String,
    monitor: tokio::task::JoinHandle<()>,
    health: tokio::sync::watch::Receiver<OwnershipHealth>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipHealth {
    Healthy,
    StatWarning(String),
    Lost,
}
impl ServerOwner {
    pub async fn acquire(layout: &HomeLayout) -> Result<Self> {
        let state_dir = layout.state_dir.clone();
        let path = layout.runtime_state();
        let legacy_path = path.clone();
        let lock_path = layout.ownership_database();
        let acquisition_path = lock_path.clone();
        let (lock, identity, replaced) = tokio::task::spawn_blocking(move || -> Result<_> {
            fs::create_dir_all(&state_dir)
                .map_err(|e| files::io("prepare ownership", &state_dir, e))?;
            if !acquisition_path.exists() {
                drop(
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&acquisition_path)
                        .map_err(|e| files::io("prepare lock", &acquisition_path, e))?,
                );
            }
            let before = file_id::get_file_id(&acquisition_path)
                .map_err(|e| files::io("stat ownership", &acquisition_path, e))?;
            let lock = t3_persistence::OwnershipLock::acquire(&state_dir)?;
            let identity = file_id::get_file_id(&acquisition_path)
                .map_err(|e| files::io("stat ownership", &acquisition_path, e))?;
            if let Some(state) = read_live_runtime_state(&legacy_path)
                && state.owner_id.is_none()
                && legacy_owner_is_live(&state)
            {
                return Err(crate::invalid(
                    "server ownership",
                    format!(
                        "A pre-lock server is already running (pid {}, {})",
                        state.pid, state.origin
                    ),
                ));
            }
            let replaced = before != identity;
            Ok((lock, identity, replaced))
        })
        .await??;
        let (health_sender, health) = tokio::sync::watch::channel(OwnershipHealth::Healthy);
        let monitor = tokio::spawn(async move {
            let mut lost = replaced;
            let mut warned = false;
            while !lost {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                // Unix get_file_id uses stat only: closing another descriptor would release SQLite locks.
                match file_id::get_file_id(&lock_path) {
                    Ok(current) => {
                        lost = current != identity;
                        warned = false;
                        if !lost {
                            health_sender.send_replace(OwnershipHealth::Healthy);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => lost = true,
                    Err(error) => {
                        if !warned {
                            tracing::warn!(%error,"Cannot check server ownership lock");
                            health_sender
                                .send_replace(OwnershipHealth::StatWarning(error.to_string()));
                            warned = true;
                        }
                    }
                }
            }
            tracing::error!(path = %lock_path.display(),"Server ownership lock was deleted or replaced. Another server may now share this T3 home.");
            health_sender.send_replace(OwnershipHealth::Lost);
        });
        Ok(Self {
            _lock: lock,
            path,
            owner_id: uuid::Uuid::new_v4().to_string(),
            monitor,
            health,
        })
    }
    pub fn ownership_health(&self) -> tokio::sync::watch::Receiver<OwnershipHealth> {
        self.health.clone()
    }
    pub async fn publish(&self, mut state: RuntimeState) -> Result<RuntimeState> {
        state.owner_id = Some(self.owner_id.clone());
        persist_runtime_state(&self.path, &state).await?;
        Ok(state)
    }
}
fn legacy_owner_is_live(state: &RuntimeState) -> bool {
    if !is_process_alive(state.pid) {
        return false;
    }
    let Ok(recorded) = chrono::DateTime::parse_from_rfc3339(&state.started_at) else {
        return true;
    };
    let Ok(pid) = u32::try_from(state.pid) else {
        return false;
    };
    let mut system = sysinfo::System::new();
    system.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        true,
    );
    let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) else {
        return true;
    };
    let started = process.start_time();
    started == 0 || i128::from(started) * 1000 <= i128::from(recorded.timestamp_millis()) + 1000
}
impl Drop for ServerOwner {
    fn drop(&mut self) {
        self.monitor.abort();
        if read_runtime_state(&self.path)
            .is_some_and(|s| s.owner_id.as_deref() == Some(&self.owner_id))
        {
            clear_runtime_state(&self.path);
        }
    }
}
