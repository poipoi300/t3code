use crate::api::RequestContext;
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use t3_contracts::{auth::AuthEnvironmentScope as Scope, relay_client::*};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, mpsc, watch},
};

pub const PINNED_VERSION: &str = "2026.10.4";
pub const MINIMUM_VERSION: &str = "2025.6.1";
#[derive(Clone)]
pub struct ReleaseAsset {
    pub url: String,
    pub sha256: String,
    pub archive: Archive,
}
#[derive(Debug, Clone, Copy)]
pub enum Archive {
    Binary,
    Tgz,
}
impl std::fmt::Debug for ReleaseAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReleaseAsset([REDACTED])")
    }
}
#[derive(Debug, Clone, thiserror::Error)]
#[error("relay client installation failed ({reason:?})")]
pub struct InstallError {
    pub reason: RelayClientInstallFailureReason,
}
impl InstallError {
    fn new(reason: RelayClientInstallFailureReason) -> Self {
        Self { reason }
    }
    pub fn wire(&self) -> RelayClientInstallFailedError {
        RelayClientInstallFailedError::RelayClientInstallFailedError {
            reason: self.reason,
            message: self.to_string(),
        }
    }
}
type InstallResult<T> = std::result::Result<T, InstallError>;
type Report = Arc<dyn Fn(RelayClientInstallProgressStage) + Send + Sync>;
#[async_trait]
pub trait BinaryDownload: Send + Sync {
    /// Cap bytes and clean partial writes on cancellation; never log download URLs.
    async fn fetch(&self, url: &str, destination: &Path) -> InstallResult<()>;
}
pub struct NativeDownload(reqwest::Client);
impl NativeDownload {
    pub fn new() -> InstallResult<Self> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 5 && attempt.url().scheme() == "https" {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map(Self)
            .map_err(|_| InstallError::new(RelayClientInstallFailureReason::DownloadFailed))
    }
}
#[async_trait]
impl BinaryDownload for NativeDownload {
    async fn fetch(&self, url: &str, destination: &Path) -> InstallResult<()> {
        let fail = || InstallError::new(RelayClientInstallFailureReason::DownloadFailed);
        let response = self
            .0
            .get(url)
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(|_| fail())?;
        if !response.status().is_success() {
            return Err(fail());
        }
        let mut response = response.bytes_stream();
        let mut file = tokio::fs::File::create(destination)
            .await
            .map_err(|_| fail())?;
        let mut total = 0_u64;
        while let Some(bytes) = response.next().await {
            let bytes = bytes.map_err(|_| fail())?;
            total = total.saturating_add(bytes.len() as u64);
            if total > 512 * 1024 * 1024 {
                return Err(fail());
            }
            file.write_all(&bytes).await.map_err(|_| fail())?;
        }
        file.sync_all().await.map_err(|_| fail())
    }
}
#[async_trait]
pub trait ExecutableProbe: Send + Sync {
    async fn version(&self, path: &Path, timeout: Duration) -> Option<String>;
    fn process_alive(&self, pid: u32) -> bool;
}
pub struct NativeProbe;
#[async_trait]
impl ExecutableProbe for NativeProbe {
    async fn version(&self, path: &Path, timeout: Duration) -> Option<String> {
        let mut options = t3_process::ProcessOptions::new(path).args(["version"]);
        options.timeout = Some(timeout);
        options.max_output_bytes = 8192;
        let output = t3_process::run(options).await.ok()?;
        parse_version(&output.stdout_text())
    }
    fn process_alive(&self, pid: u32) -> bool {
        t3_config::runtime::is_process_alive(i64::from(pid))
    }
}
pub struct RelayClientOptions {
    pub base_dir: PathBuf,
    pub platform: String,
    pub arch: String,
    pub executable_override: Option<PathBuf>,
    pub search_path: Vec<PathBuf>,
    pub release: Option<ReleaseAsset>,
    pub install_allowed: watch::Receiver<bool>,
}
pub struct RelayClient {
    options: RelayClientOptions,
    download: Arc<dyn BinaryDownload>,
    probe: Arc<dyn ExecutableProbe>,
    lock: Mutex<()>,
}
impl RelayClient {
    pub fn new(
        options: RelayClientOptions,
        download: Arc<dyn BinaryDownload>,
        probe: Arc<dyn ExecutableProbe>,
    ) -> Arc<Self> {
        Arc::new(Self {
            options,
            download,
            probe,
            lock: Mutex::new(()),
        })
    }
    fn executable_name(&self) -> &'static str {
        if self.options.platform == "win32" {
            "cloudflared.exe"
        } else {
            "cloudflared"
        }
    }
    fn root(&self) -> PathBuf {
        self.options.base_dir.join("tools").join("cloudflared")
    }
    pub fn pinned_path(&self) -> PathBuf {
        self.path_for(PINNED_VERSION)
    }
    fn path_for(&self, version: &str) -> PathBuf {
        self.root()
            .join(version)
            .join(format!("{}-{}", self.options.platform, self.options.arch))
            .join(self.executable_name())
    }
    async fn compatible(
        &self,
        path: PathBuf,
        source: RelayClientSource,
        exact: Option<&str>,
    ) -> Option<RelayClientStatus> {
        let metadata = tokio::fs::metadata(&path).await.ok()?;
        if !metadata.is_file() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return None;
            }
        }
        let version = self.probe.version(&path, Duration::from_secs(10)).await?;
        if version_tuple(&version)? < version_tuple(MINIMUM_VERSION)?
            || exact.is_some_and(|exact| exact != version)
        {
            return None;
        }
        Some(RelayClientStatus::Available {
            executable_path: path.to_string_lossy().into_owned(),
            source,
            version,
        })
    }
    async fn older(&self) -> Vec<String> {
        let mut result = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(self.root()).await else {
            return result;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let value = entry.file_name().to_string_lossy().into_owned();
            if version_tuple(&value)
                .is_some_and(|version| version < version_tuple(PINNED_VERSION).unwrap_or_default())
                && entry
                    .file_type()
                    .await
                    .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                result.push(value);
            }
        }
        result.sort_by_key(|version| std::cmp::Reverse(version_tuple(version)));
        result
    }
    pub async fn resolve(&self) -> RelayClientStatus {
        let missing = || RelayClientStatus::Missing {
            version: PINNED_VERSION.into(),
        };
        if let Some(path) = &self.options.executable_override {
            return self
                .compatible(path.clone(), RelayClientSource::Override, None)
                .await
                .unwrap_or_else(missing);
        }
        if let Some(status) = self
            .compatible(
                self.pinned_path(),
                RelayClientSource::Managed,
                Some(PINNED_VERSION),
            )
            .await
        {
            return status;
        }
        for version in self.older().await {
            if let Some(status) = self
                .compatible(
                    self.path_for(&version),
                    RelayClientSource::Managed,
                    Some(&version),
                )
                .await
            {
                return status;
            }
        }
        for directory in &self.options.search_path {
            if let Some(status) = self
                .compatible(
                    directory.join(self.executable_name()),
                    RelayClientSource::Path,
                    None,
                )
                .await
            {
                return status;
            }
        }
        if self.options.release.is_some() {
            missing()
        } else {
            RelayClientStatus::Unsupported {
                platform: self.options.platform.clone(),
                arch: self.options.arch.clone(),
                version: PINNED_VERSION.into(),
            }
        }
    }
    pub async fn status(&self, context: &RequestContext) -> crate::Result<RelayClientStatus> {
        context.require(Scope::RelayRead)?;
        Ok(self.resolve().await)
    }
    pub async fn install(&self, report: Report) -> InstallResult<RelayClientStatus> {
        use RelayClientInstallFailureReason as Failure;
        let _lock = self.lock.lock().await;
        if !*self.options.install_allowed.borrow() {
            return Err(InstallError::new(Failure::ManagedByPolicy));
        }
        report(RelayClientInstallProgressStage::Checking);
        if let Some(path) = &self.options.executable_override {
            return self
                .compatible(path.clone(), RelayClientSource::Override, None)
                .await
                .ok_or_else(|| InstallError::new(Failure::OverrideMissing));
        }
        if let Some(status) = self
            .compatible(
                self.pinned_path(),
                RelayClientSource::Managed,
                Some(PINNED_VERSION),
            )
            .await
        {
            return Ok(status);
        }
        let asset = self
            .options
            .release
            .as_ref()
            .ok_or_else(|| InstallError::new(Failure::UnsupportedPlatform))?;
        let url =
            url::Url::parse(&asset.url).map_err(|_| InstallError::new(Failure::DownloadFailed))?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(InstallError::new(Failure::DownloadFailed));
        }
        let destination = self.pinned_path();
        let parent = destination
            .parent()
            .ok_or_else(|| InstallError::new(Failure::WriteFailed))?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| InstallError::new(Failure::WriteFailed))?;
        report(RelayClientInstallProgressStage::WaitingForLock);
        let lock_path = destination.with_file_name(format!("{}.lock", self.executable_name()));
        let _guard = self.acquire_lock(&lock_path).await?;
        if let Some(status) = self
            .compatible(
                destination.clone(),
                RelayClientSource::Managed,
                Some(PINNED_VERSION),
            )
            .await
        {
            return Ok(status);
        }
        let temporary = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(parent)
            .map_err(|_| InstallError::new(Failure::WriteFailed))?;
        let archive = temporary
            .path()
            .join(if matches!(asset.archive, Archive::Tgz) {
                "cloudflared.tgz"
            } else {
                self.executable_name()
            });
        report(RelayClientInstallProgressStage::Downloading);
        self.download.fetch(&asset.url, &archive).await?;
        report(RelayClientInstallProgressStage::Verifying);
        let archive_for_hash = archive.clone();
        let hash = tokio::task::spawn_blocking(move || hash_file(&archive_for_hash))
            .await
            .map_err(|_| InstallError::new(Failure::ValidationFailed))??;
        if hash != asset.sha256.to_ascii_lowercase() {
            return Err(InstallError::new(Failure::InvalidChecksum));
        }
        report(RelayClientInstallProgressStage::Installing);
        let executable = temporary.path().join(self.executable_name());
        if matches!(asset.archive, Archive::Tgz) {
            // A checksum-pinned archive is extracted only inside this installation's directory.
            let mut options = t3_process::ProcessOptions::new("tar").args([
                "-xzf".as_ref(),
                archive.as_os_str(),
                "-C".as_ref(),
                temporary.path().as_os_str(),
            ]);
            options.timeout = Some(Duration::from_secs(120));
            options.max_output_bytes = 8192;
            t3_process::run(options)
                .await
                .map_err(|_| InstallError::new(Failure::WriteFailed))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
                .await
                .map_err(|_| InstallError::new(Failure::WriteFailed))?;
        }
        report(RelayClientInstallProgressStage::Validating);
        if self
            .probe
            .version(&executable, Duration::from_secs(120))
            .await
            .as_deref()
            != Some(PINNED_VERSION)
        {
            return Err(InstallError::new(Failure::ValidationFailed));
        }
        report(RelayClientInstallProgressStage::Activating);
        let mut installed = false;
        for _ in 0..40 {
            match tokio::fs::rename(&executable, &destination).await {
                Ok(()) => {
                    installed = true;
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
        if !installed {
            return Err(InstallError::new(Failure::WriteFailed));
        }
        Ok(RelayClientStatus::Available {
            executable_path: destination.to_string_lossy().into_owned(),
            source: RelayClientSource::Managed,
            version: PINNED_VERSION.into(),
        })
    }
    async fn acquire_lock(&self, path: &Path) -> InstallResult<InstallLock> {
        use RelayClientInstallFailureReason as Failure;
        let parent = path
            .parent()
            .ok_or_else(|| InstallError::new(Failure::WriteFailed))?;
        let candidate = tempfile::Builder::new()
            .prefix(".lock-")
            .tempdir_in(parent)
            .map_err(|_| InstallError::new(Failure::WriteFailed))?;
        let owner = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
        std::fs::write(candidate.path().join(&owner), b"")
            .map_err(|_| InstallError::new(Failure::WriteFailed))?;
        for _ in 0..100 {
            if tokio::fs::rename(candidate.path(), path).await.is_ok() {
                return Ok(InstallLock {
                    path: path.to_owned(),
                    owner,
                });
            }
            if let Ok(metadata) = tokio::fs::symlink_metadata(path).await {
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    if let Ok(mut entries) = tokio::fs::read_dir(path).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            let owner = entry.file_name().to_string_lossy().into_owned();
                            if let Some(pid) = owner
                                .split_once('-')
                                .and_then(|(pid, _)| pid.parse::<u32>().ok())
                                .filter(|pid| *pid > 0)
                                && !self.probe.process_alive(pid)
                            {
                                let _ = tokio::fs::remove_file(entry.path()).await;
                            }
                        }
                    }
                    let _ = tokio::fs::remove_dir(path).await;
                } else if metadata.is_file()
                    && metadata
                        .modified()
                        .ok()
                        .and_then(|at| at.elapsed().ok())
                        .is_some_and(|elapsed| elapsed > Duration::from_secs(1800))
                {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(InstallError::new(Failure::InstallLocked))
    }
    pub async fn prune_managed_versions(&self) -> InstallResult<()> {
        let root = self.root();
        let canonical_root = tokio::fs::canonicalize(&root)
            .await
            .map_err(|_| InstallError::new(RelayClientInstallFailureReason::WriteFailed))?;
        for version in self.older().await.into_iter().skip(1) {
            let path = root.join(&version);
            let canonical = tokio::fs::canonicalize(&path)
                .await
                .map_err(|_| InstallError::new(RelayClientInstallFailureReason::WriteFailed))?;
            if canonical.parent() == Some(canonical_root.as_path()) {
                let _ = tokio::fs::remove_dir_all(path).await;
            }
        }
        Ok(())
    }
    pub fn install_with_progress(
        self: &Arc<Self>,
        context: &RequestContext,
    ) -> crate::Result<InstallStream> {
        context.require(Scope::RelayWrite)?;
        let (sender, receiver) = mpsc::channel(16);
        let service = self.clone();
        let cancel = t3_process::CancellationToken::new();
        let cancelled = cancel.clone();
        let task = tokio::spawn(async move {
            let progress = sender.clone();
            let report = Arc::new(move |stage| {
                let _ = progress.try_send(Ok(RelayClientInstallProgressEvent::Progress { stage }));
            });
            let result = tokio::select! {_=cancelled.cancelled()=>return,result=service.install(report)=>result};
            let event = result
                .map(|status| RelayClientInstallProgressEvent::Complete { status })
                .map_err(|error| error.wire());
            let _ = sender.send(event).await;
        });
        Ok(InstallStream {
            receiver,
            cancel,
            task,
        })
    }
}
struct InstallLock {
    path: PathBuf,
    owner: String,
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.path.join(&self.owner));
        let _ = std::fs::remove_dir(&self.path);
    }
}
pub struct InstallStream {
    receiver: mpsc::Receiver<
        std::result::Result<RelayClientInstallProgressEvent, RelayClientInstallFailedError>,
    >,
    cancel: t3_process::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Stream for InstallStream {
    type Item = std::result::Result<RelayClientInstallProgressEvent, RelayClientInstallFailedError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}
impl InstallStream {
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        let _ = (&mut self.task).await;
    }
}
impl Drop for InstallStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
fn hash_file(path: &Path) -> InstallResult<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .map_err(|_| InstallError::new(RelayClientInstallFailureReason::ValidationFailed))?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file
            .read(&mut bytes)
            .map_err(|_| InstallError::new(RelayClientInstallFailureReason::ValidationFailed))?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
fn version_tuple(value: &str) -> Option<(u32, u32, u32)> {
    let mut values = value.split('.');
    let result = (
        values.next()?.parse().ok()?,
        values.next()?.parse().ok()?,
        values.next()?.parse().ok()?,
    );
    values.next().is_none().then_some(result)
}
pub fn parse_version(value: &str) -> Option<String> {
    value
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .find_map(|part| {
            version_tuple(part)
                .filter(|(year, _, _)| *year >= 1000)
                .map(|(year, month, patch)| format!("{year}.{month}.{patch}"))
        })
}
pub fn pinned_asset(platform: &str, arch: &str) -> Option<ReleaseAsset> {
    let (name, sha256, archive) = match (platform, arch) {
        ("darwin", "arm64") => (
            "cloudflared-darwin-arm64.tgz",
            "68b6bc4080ac1330234db2a9002d07f98b000537a1bd531a687b660d4160fa7f",
            Archive::Tgz,
        ),
        ("darwin", "x64") => (
            "cloudflared-darwin-amd64.tgz",
            "f11fbef4207d715892331917dafd437f032cf4b7137ed64dea3008338c1c613d",
            Archive::Tgz,
        ),
        ("linux", "arm64") => (
            "cloudflared-linux-arm64",
            "2b7203b4fb0ba28a0c1e12f8855ce2c25f76d266a8d5edafac5540b6812acff6",
            Archive::Binary,
        ),
        ("linux", "x64") => (
            "cloudflared-linux-amd64",
            "ae5e8add7884b0531d66bbca4570d1de491ff6da51e4ebe7eb160cdf241d9e42",
            Archive::Binary,
        ),
        ("win32", "x64" | "arm64") => (
            "cloudflared-windows-amd64.exe",
            "411b16d40c8d872243efeb52c550053d00af0a8a97fbb32e730fb8aba429eaa8",
            Archive::Binary,
        ),
        _ => return None,
    };
    Some(ReleaseAsset {
        url: format!(
            "https://github.com/cloudflare/cloudflared/releases/download/{PINNED_VERSION}/{name}"
        ),
        sha256: sha256.into(),
        archive,
    })
}
