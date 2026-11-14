use crate::{
    environment::Platform,
    sensitive::{ServiceError, ServiceResult},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct ObservedProcess {
    pub pid: u32,
    pub pgid: u32,
    pub start_time: String,
    pub command: String,
    pub zombie: bool,
}
impl std::fmt::Debug for ObservedProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObservedProcess([REDACTED])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupSignal {
    Terminate,
    Kill,
}
#[async_trait]
pub trait LedgerProcesses: Send + Sync {
    async fn observe(&self, pid: u32) -> ServiceResult<Option<ObservedProcess>>;
    async fn group(&self, pgid: u32) -> ServiceResult<Vec<ObservedProcess>>;
    async fn exists(&self, pgid: u32) -> ServiceResult<bool>;
    async fn signal(&self, pgid: u32, signal: GroupSignal) -> ServiceResult<()>;
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    pid: u32,
    start_time: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    version: u8,
    pgid: u32,
    pid: u32,
    start_time: String,
    command: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    args: Option<Vec<String>>,
    port: u16,
    state_dir: PathBuf,
    owner: Identity,
}
pub struct LedgerLease(PathBuf);
impl LedgerLease {
    pub async fn forget(self) -> ServiceResult<()> {
        match tokio::fs::remove_file(self.0).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ServiceError::Storage),
        }
    }
}
pub struct OpenCodeLedger {
    state: PathBuf,
    processes: Arc<dyn LedgerProcesses>,
    owner: Option<Identity>,
    recordable: bool,
}
fn runs_serve(observed: &ObservedProcess, args: &[String]) -> bool {
    observed.command.ends_with(&format!(" {}", args.join(" ")))
}
impl OpenCodeLedger {
    pub async fn open(
        state: PathBuf,
        platform: Platform,
        owner_pid: u32,
        processes: Arc<dyn LedgerProcesses>,
    ) -> ServiceResult<Arc<Self>> {
        let recordable = matches!(platform, Platform::Linux | Platform::Macos);
        let owner = if recordable {
            processes.observe(owner_pid).await?.map(|process| Identity {
                pid: owner_pid,
                start_time: process.start_time,
            })
        } else {
            None
        };
        Ok(Arc::new(Self {
            state,
            processes,
            owner,
            recordable,
        }))
    }
    pub async fn track(
        &self,
        pid: u32,
        port: u16,
        args: Vec<String>,
    ) -> ServiceResult<Option<LedgerLease>> {
        if !self.recordable || pid == 0 {
            return Ok(None);
        }
        let Some(owner) = self.owner.clone() else {
            return Ok(None);
        };
        let leader = self.processes.observe(pid).await?;
        let members = match leader.filter(|row| !row.zombie && row.pgid == pid) {
            Some(leader) => vec![leader],
            None => self
                .processes
                .group(pid)
                .await?
                .into_iter()
                .filter(|row| !row.zombie && row.pgid == pid)
                .collect(),
        };
        let Some(member) = members
            .iter()
            .find(|row| runs_serve(row, &args))
            .or_else(|| members.first())
        else {
            return Ok(None);
        };
        let entry = Entry {
            version: 1,
            pgid: pid,
            pid: member.pid,
            start_time: member.start_time.clone(),
            command: member.command.clone(),
            args: Some(args),
            port,
            state_dir: self.state.clone(),
            owner,
        };
        let directory = self.state.join("opencode-servers");
        let path = directory.join(format!("{pid}.json"));
        let bytes = serde_json::to_vec(&entry).map_err(|_| ServiceError::Storage)?;
        let target = path.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&directory).map_err(|_| ServiceError::Storage)?;
            let mut file =
                tempfile::NamedTempFile::new_in(&directory).map_err(|_| ServiceError::Storage)?;
            std::io::Write::write_all(&mut file, &bytes).map_err(|_| ServiceError::Storage)?;
            file.persist(target).map_err(|_| ServiceError::Storage)?;
            Ok::<_, ServiceError>(())
        })
        .await
        .map_err(|_| ServiceError::Storage)??;
        Ok(Some(LedgerLease(path)))
    }
    async fn stop_orphan(&self, entry: &Entry) -> ServiceResult<()> {
        let Some(row) = self.processes.observe(entry.pid).await? else {
            return Ok(());
        };
        if row.zombie
            || row.start_time != entry.start_time
            || row.pgid != entry.pgid
            || (row.command != entry.command
                && !entry
                    .args
                    .as_ref()
                    .is_some_and(|args| runs_serve(&row, args)))
        {
            return Ok(());
        }
        self.processes
            .signal(entry.pgid, GroupSignal::Terminate)
            .await?;
        for _ in 0..40 {
            if !self.processes.exists(entry.pgid).await? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if self.processes.exists(entry.pgid).await? {
            self.processes.signal(entry.pgid, GroupSignal::Kill).await?;
        }
        Ok(())
    }
    pub async fn reap_orphans(&self) -> ServiceResult<()> {
        if !self.recordable {
            return Ok(());
        }
        let directory = self.state.join("opencode-servers");
        let mut files = match tokio::fs::read_dir(&directory).await {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(ServiceError::Storage),
        };
        while let Some(file) = files
            .next_entry()
            .await
            .map_err(|_| ServiceError::Storage)?
        {
            let name = file.file_name();
            let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let metadata = file.metadata().await.map_err(|_| ServiceError::Storage)?;
            if !file
                .file_type()
                .await
                .map_err(|_| ServiceError::Storage)?
                .is_file()
                || metadata.len() > 65536
            {
                continue;
            }
            let bytes = tokio::fs::read(file.path())
                .await
                .map_err(|_| ServiceError::Storage)?;
            if let Ok(entry) = serde_json::from_slice::<Entry>(&bytes)
                && entry.version == 1
                && entry.pgid > 0
                && entry.pid > 0
                && entry.state_dir == self.state
            {
                let owner = self.processes.observe(entry.owner.pid).await?;
                if owner.is_some_and(|row| !row.zombie && row.start_time == entry.owner.start_time)
                {
                    continue;
                }
                self.stop_orphan(&entry).await?;
            }
            tokio::fs::remove_file(file.path())
                .await
                .map_err(|_| ServiceError::Storage)?;
        }
        Ok(())
    }
}
