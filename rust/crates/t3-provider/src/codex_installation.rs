use crate::{
    catalog::ManifestSource,
    environment::{Environment, Platform, process_options},
    sensitive::{ServiceError, ServiceResult},
    transport::ProviderClock,
    versions::{compare_versions, resolve_compatibility},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{provider_setup::*, server::ServerProviderCompatibilityStatus};
use t3_process::CancellationToken;
use tokio::sync::{Mutex, OwnedRwLockReadGuard, RwLock, watch};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub version: String,
    pub target: String,
    pub url: String,
    pub sha256: String,
    pub archive_bytes: Option<u64>,
}
pub fn pinned_release(platform: &str, architecture: &str) -> Option<ReleaseAsset> {
    serde_json::from_str::<BTreeMap<String, ReleaseAsset>>(include_str!(
        "../data/codex-releases.json"
    ))
    .ok()?
    .remove(&format!("{platform}-{architecture}"))
}
#[derive(Debug, Clone)]
pub struct CodexExecutable {
    pub path: PathBuf,
    pub version: String,
    pub source: ProviderInstallSource,
    pub managed_directory: Option<PathBuf>,
}
pub struct ExecutableLease {
    pub executable: CodexExecutable,
    _lease: OwnedRwLockReadGuard<()>,
}

#[async_trait]
pub trait ReleaseTransport: Send + Sync {
    async fn latest(&self, target: &str) -> ServiceResult<Option<ReleaseAsset>>;
    async fn download(
        &self,
        asset: &ReleaseAsset,
        destination: &Path,
        progress: watch::Sender<u64>,
    ) -> ServiceResult<()>;
}
pub struct ReqwestReleases {
    client: reqwest::Client,
}
impl ReqwestReleases {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .build()
                .map_err(|_| ServiceError::Network)?,
        })
    }
}
#[async_trait]
impl ReleaseTransport for ReqwestReleases {
    async fn latest(&self, target: &str) -> ServiceResult<Option<ReleaseAsset>> {
        let latest: Value = self
            .client
            .get("https://registry.npmjs.org/@openai/codex/latest")
            .send()
            .await
            .map_err(|_| ServiceError::Network)?
            .error_for_status()
            .map_err(|_| ServiceError::Network)?
            .json()
            .await
            .map_err(|_| ServiceError::Response)?;
        let version = latest["version"]
            .as_str()
            .filter(|v| valid_version(v))
            .ok_or(ServiceError::Response)?;
        let root = format!("https://github.com/openai/codex/releases/download/rust-v{version}");
        let mut response = self
            .client
            .get(format!("{root}/codex-package_SHA256SUMS"))
            .send()
            .await
            .map_err(|_| ServiceError::Network)?
            .error_for_status()
            .map_err(|_| ServiceError::Network)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ServiceError::Network)? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(ServiceError::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(bytes).map_err(|_| ServiceError::Response)?;
        let file = format!("codex-package-{target}.tar.gz");
        let checksum = text.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            let digest = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            (name == file && valid_digest(digest)).then_some(digest)
        });
        Ok(checksum.map(|sha256| ReleaseAsset {
            version: version.into(),
            target: target.into(),
            url: format!("{root}/{file}"),
            sha256: sha256.to_owned(),
            archive_bytes: None,
        }))
    }
    async fn download(
        &self,
        asset: &ReleaseAsset,
        destination: &Path,
        progress: watch::Sender<u64>,
    ) -> ServiceResult<()> {
        let mut response = self
            .client
            .get(&asset.url)
            .send()
            .await
            .map_err(|_| ServiceError::Network)?
            .error_for_status()
            .map_err(|_| ServiceError::Network)?;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .map_err(|_| ServiceError::Storage)?;
        let mut bytes = 0u64;
        while let Some(chunk) = response.chunk().await.map_err(|_| ServiceError::Network)? {
            bytes = bytes
                .checked_add(chunk.len() as u64)
                .ok_or(ServiceError::Response)?;
            if bytes > asset.archive_bytes.unwrap_or(1024 * 1024 * 1024) {
                return Err(ServiceError::Response);
            }
            file.write_all(&chunk).map_err(|_| ServiceError::Storage)?;
            progress.send_replace(bytes);
        }
        Ok(())
    }
}
#[async_trait]
pub trait CodexExecutableValidator: Send + Sync {
    async fn validate(
        &self,
        executable: &Path,
        version: &str,
        environment: &Environment,
    ) -> ServiceResult<()>;
}
pub struct ProcessCodexValidator;
#[async_trait]
impl CodexExecutableValidator for ProcessCodexValidator {
    async fn validate(
        &self,
        executable: &Path,
        version: &str,
        environment: &Environment,
    ) -> ServiceResult<()> {
        let mut options =
            process_options(executable.as_os_str(), &["--version".into()], environment);
        options.timeout = Some(Duration::from_secs(90));
        options.max_output_bytes = 8192;
        let output = t3_process::run(options)
            .await
            .map_err(|_| ServiceError::Maintenance)?;
        if output.stdout_text().trim() == format!("codex-cli {version}") {
            Ok(())
        } else {
            Err(ServiceError::Maintenance)
        }
    }
}
#[async_trait]
pub trait LocalCodex: Send + Sync {
    async fn resolve(&self) -> ServiceResult<Option<CodexExecutable>>;
}
pub struct NoLocalCodex;
#[async_trait]
impl LocalCodex for NoLocalCodex {
    async fn resolve(&self) -> ServiceResult<Option<CodexExecutable>> {
        Ok(None)
    }
}
pub struct ProcessLocalCodex {
    pub environment: Environment,
    pub managed_directory: PathBuf,
}
#[async_trait]
impl LocalCodex for ProcessLocalCodex {
    async fn resolve(&self) -> ServiceResult<Option<CodexExecutable>> {
        let mut options = process_options("codex", &[], &self.environment);
        let resolved = match t3_process::resolve_command(&options.program, &[], &options.env, false)
        {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let path = PathBuf::from(&resolved.program);
        let real = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(_) => return Ok(None),
        };
        if fs::canonicalize(&self.managed_directory)
            .ok()
            .is_some_and(|root| real.starts_with(root))
        {
            return Ok(None);
        }
        let basename = real
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let launcher = [
            "volta-shim",
            "volta-shim.exe",
            "mise",
            "mise.exe",
            "aqua-proxy",
            "aqua-proxy.exe",
        ]
        .contains(&basename.as_str());
        let is_script = if cfg!(windows) {
            real.extension()
                .is_some_and(|s| !s.eq_ignore_ascii_case("exe"))
                || ["bunx", "shim"].iter().any(|s| {
                    fs::metadata(real.with_extension(s)).is_ok_and(|m| m.is_file() && m.len() > 0)
                })
        } else {
            let mut header = [0; 2];
            fs::File::open(&real)
                .and_then(|mut f| f.read_exact(&mut header))
                .is_ok()
                && header == *b"#!"
        };
        if !launcher
            && !is_script
            && !executable_file(&real.with_file_name(if cfg!(windows) {
                "codex-code-mode-host.exe"
            } else {
                "codex-code-mode-host"
            }))
        {
            return Ok(None);
        }
        options.program = resolved.program;
        options.args = vec!["--version".into()];
        options.timeout = Some(Duration::from_secs(5));
        options.max_output_bytes = 8192;
        let output = match t3_process::run(options.clone()).await {
            Ok(output) => output,
            Err(_) => return Ok(None),
        };
        let text = output.stdout_text();
        let Some(version) = text
            .trim()
            .strip_prefix("codex-cli ")
            .filter(|v| valid_version(v))
        else {
            return Ok(None);
        };
        options.args = vec!["app-server".into(), "--help".into()];
        if t3_process::run(options).await.is_err() {
            return Ok(None);
        }
        Ok(Some(CodexExecutable {
            path,
            version: version.into(),
            source: ProviderInstallSource::Local,
            managed_directory: None,
        }))
    }
}

struct ActiveInstall {
    id: String,
    cancel: CancellationToken,
    done: watch::Receiver<bool>,
}
pub struct CodexInstallation {
    directory: PathBuf,
    platform: Platform,
    environment: Environment,
    pin: Option<ReleaseAsset>,
    manifest: ManifestSource,
    server_version: String,
    transport: Arc<dyn ReleaseTransport>,
    validator: Arc<dyn CodexExecutableValidator>,
    local: Arc<dyn LocalCodex>,
    clock: Arc<dyn ProviderClock>,
    state: watch::Sender<ProviderInstallState>,
    gate: Mutex<Option<ActiveInstall>>,
    leases: Arc<RwLock<()>>,
    upstream: Mutex<(Option<i64>, Option<ReleaseAsset>)>,
}
pub struct InstallationOptions {
    pub base_directory: PathBuf,
    pub platform: Platform,
    pub environment: Environment,
    pub pin: Option<ReleaseAsset>,
    pub manifest: ManifestSource,
    pub server_version: String,
}
impl CodexInstallation {
    pub fn new(
        options: InstallationOptions,
        transport: Arc<dyn ReleaseTransport>,
        validator: Arc<dyn CodexExecutableValidator>,
        local: Arc<dyn LocalCodex>,
        clock: Arc<dyn ProviderClock>,
    ) -> Arc<Self> {
        let (state, _) = watch::channel(ProviderInstallState {
            driver: "codex".into(),
            operation_id: None,
            phase: ProviderInstallPhase::Idle,
            downloaded_bytes: 0,
            total_bytes: options.pin.as_ref().and_then(|p| p.archive_bytes),
            version: options.pin.as_ref().map(|p| p.version.clone().into()),
            installed_version: None,
            executable_path: Some(None),
            source: Some(None),
            can_remove: false,
            message: None,
        });
        Arc::new(Self {
            directory: options.base_directory.join("tools").join("codex"),
            platform: options.platform,
            environment: options.environment,
            pin: options.pin,
            manifest: options.manifest,
            server_version: options.server_version,
            transport,
            validator,
            local,
            clock,
            state,
            gate: Mutex::new(None),
            leases: Arc::new(RwLock::new(())),
            upstream: Mutex::new((None, None)),
        })
    }
    pub fn state(&self) -> ProviderInstallState {
        self.state.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<ProviderInstallState> {
        self.state.subscribe()
    }
    fn supported(&self, version: &str) -> bool {
        resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "codex",
            Some(version),
            &self.server_version,
        )
        .is_some_and(|a| a.status == ServerProviderCompatibilityStatus::Supported)
    }
    fn root(&self) -> ServiceResult<PathBuf> {
        let base = self
            .directory
            .parent()
            .and_then(Path::parent)
            .ok_or(ServiceError::Storage)?;
        fs::create_dir_all(base).map_err(|_| ServiceError::Storage)?;
        let mut root = fs::canonicalize(base).map_err(|_| ServiceError::Storage)?;
        for component in ["tools", "codex"] {
            let path = root.join(component);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(ServiceError::Storage);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&path).map_err(|_| ServiceError::Storage)?;
                }
                Err(_) => return Err(ServiceError::Storage),
            }
            let resolved = fs::canonicalize(&path).map_err(|_| ServiceError::Storage)?;
            if resolved.parent() != Some(root.as_path()) {
                return Err(ServiceError::Storage);
            }
            root = resolved;
        }
        Ok(root)
    }
    fn package_directory(
        &self,
        directory: &Path,
        version: &str,
        target: &str,
    ) -> ServiceResult<CodexExecutable> {
        let manifest = record(&directory.join("codex-package.json"))?;
        let name = if self.platform == Platform::Windows {
            "codex.exe"
        } else {
            "codex"
        };
        if manifest["layoutVersion"] != 1
            || manifest["version"] != version
            || manifest["target"] != target
            || manifest["entrypoint"] != format!("bin/{name}")
        {
            return Err(ServiceError::Maintenance);
        }
        for name in [
            format!("bin/{name}"),
            format!(
                "bin/codex-code-mode-host{}",
                if self.platform == Platform::Windows {
                    ".exe"
                } else {
                    ""
                }
            ),
            format!(
                "codex-path/rg{}",
                if self.platform == Platform::Windows {
                    ".exe"
                } else {
                    ""
                }
            ),
        ] {
            if !executable_file(&directory.join(name)) {
                return Err(ServiceError::Maintenance);
            }
        }
        Ok(CodexExecutable {
            path: directory.join("bin").join(name),
            version: version.into(),
            source: ProviderInstallSource::Managed,
            managed_directory: Some(directory.to_path_buf()),
        })
    }
    fn completed(&self, version: &str) -> ServiceResult<CodexExecutable> {
        if !valid_version(version) {
            return Err(ServiceError::Maintenance);
        }
        let root = self.root()?;
        let directory = root.join(version);
        let real = fs::canonicalize(&directory).map_err(|_| ServiceError::Maintenance)?;
        if real.parent() != Some(root.as_path())
            || fs::symlink_metadata(&directory).is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(ServiceError::Maintenance);
        }
        let receipt = record(&directory.join(".install-complete.json"))?;
        if receipt["version"] != version {
            return Err(ServiceError::Maintenance);
        }
        self.package_directory(
            &directory,
            version,
            receipt["target"]
                .as_str()
                .ok_or(ServiceError::Maintenance)?,
        )
    }
    pub async fn resolve(&self) -> ServiceResult<CodexExecutable> {
        if let Some(local) = self.local.resolve().await?
            && self.supported(&local.version)
        {
            return Ok(local);
        }
        let active = record(&self.directory.join("active.json"))?;
        let executable = self.completed(
            active["version"]
                .as_str()
                .ok_or(ServiceError::Maintenance)?,
        )?;
        if !self.supported(&executable.version) {
            return Err(ServiceError::Maintenance);
        }
        Ok(executable)
    }
    pub async fn acquire(&self) -> ServiceResult<ExecutableLease> {
        let lease = self.leases.clone().read_owned().await;
        let executable = self.resolve().await?;
        Ok(ExecutableLease {
            executable,
            _lease: lease,
        })
    }
    pub async fn initialize(self: &Arc<Self>) -> ServiceResult<()> {
        if self.directory.exists() {
            let root = self.root()?;
            for entry in fs::read_dir(&root).map_err(|_| ServiceError::Storage)? {
                let entry = entry.map_err(|_| ServiceError::Storage)?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if (name.starts_with(".install-") || name.starts_with(".active-"))
                    && entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > Duration::from_secs(86400))
                {
                    remove_inside(&root, &entry.path())?;
                }
            }
            self.state.send_modify(|s| {
                s.can_remove = fs::read_dir(&root).map_or(true, |mut r| r.next().is_some())
            });
        }
        match self.resolve().await {
            Ok(executable) => {
                let update = executable.source == ProviderInstallSource::Managed
                    && self
                        .pin
                        .as_ref()
                        .is_some_and(|p| compare_versions(&executable.version, &p.version).is_lt());
                self.publish_executable(&executable);
                if update {
                    self.start().await?;
                }
                Ok(())
            }
            Err(_) if !self.directory.join("active.json").exists() => Ok(()),
            Err(error) => {
                self.state.send_modify(|s| {
                    s.phase = ProviderInstallPhase::Failed;
                    s.message = Some(
                        "The managed Codex runtime is incomplete. Remove it and reinstall.".into(),
                    );
                });
                Err(error)
            }
        }
    }
    fn publish_executable(&self, executable: &CodexExecutable) {
        self.state.send_modify(|s| {
            s.phase = ProviderInstallPhase::Succeeded;
            s.installed_version = Some(executable.version.clone().into());
            s.executable_path = Some(Some(executable.path.to_string_lossy().into_owned().into()));
            s.source = Some(Some(executable.source.clone()));
            s.message = None;
        });
    }
    pub async fn check_for_update(&self) {
        let Ok(mut upstream) = self.upstream.try_lock() else {
            return;
        };
        let now = self.clock.now_ms();
        if upstream.0.is_none_or(|at| now - at >= 3_600_000) {
            upstream.0 = Some(now);
            if let Some(pin) = self.pin.as_ref()
                && let Ok(Ok(release)) = tokio::time::timeout(
                    Duration::from_secs(20),
                    self.transport.latest(&pin.target),
                )
                .await
            {
                upstream.1 = release.filter(|r| self.supported(&r.version));
            }
        }
        let current = self.state();
        if !running(&current) {
            let offer = self.offer(
                current.installed_version.as_ref().map(|v| v.0.as_str()),
                upstream.1.as_ref(),
            );
            self.state.send_modify(|s| {
                s.version = offer
                    .as_ref()
                    .map(|a| a.version.clone().into())
                    .or_else(|| s.installed_version.clone());
                s.total_bytes = offer.and_then(|a| a.archive_bytes);
            });
        }
    }
    fn offer(
        &self,
        installed: Option<&str>,
        upstream: Option<&ReleaseAsset>,
    ) -> Option<ReleaseAsset> {
        let installed = installed.filter(|v| self.supported(v));
        [
            self.pin.as_ref(),
            upstream.filter(|r| self.supported(&r.version)),
        ]
        .into_iter()
        .flatten()
        .filter(|r| installed.is_none_or(|i| compare_versions(&r.version, i).is_gt()))
        .max_by(|a, b| compare_versions(&a.version, &b.version))
        .cloned()
    }
    pub async fn start(self: &Arc<Self>) -> ServiceResult<ProviderInstallState> {
        let mut gate = self.gate.lock().await;
        if running(&self.state()) {
            return Ok(self.state());
        }
        if let Some(local) = self
            .local
            .resolve()
            .await?
            .filter(|v| self.supported(&v.version))
        {
            self.publish_executable(&local);
            return Ok(self.state());
        }
        let current = self.state();
        let upstream = self.upstream.lock().await.1.clone();
        let release = self.offer(
            current.installed_version.as_ref().map(|v| v.0.as_str()),
            upstream.as_ref(),
        );
        if release.is_none() && current.installed_version.is_none() {
            return Err(ServiceError::Unsupported);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let (done, receiver) = watch::channel(false);
        self.state.send_modify(|s| {
            s.operation_id = Some(id.clone().into());
            s.phase = if release.is_some() {
                ProviderInstallPhase::Downloading
            } else {
                ProviderInstallPhase::Verifying
            };
            s.downloaded_bytes = 0;
            s.version = release
                .as_ref()
                .map(|r| r.version.clone().into())
                .or_else(|| s.installed_version.clone());
            s.total_bytes = release.as_ref().and_then(|r| r.archive_bytes);
            s.message = Some(
                if release.is_some() {
                    "Downloading Codex."
                } else {
                    "Checking Codex."
                }
                .into(),
            );
        });
        *gate = Some(ActiveInstall {
            id: id.clone(),
            cancel: cancel.clone(),
            done: receiver,
        });
        let this = self.clone();
        tokio::spawn(async move {
            let work = async {
                if let Some(release) = release {
                    this.install(release).await
                } else {
                    let executable = this.resolve().await?;
                    this.validator
                        .validate(&executable.path, &executable.version, &this.environment)
                        .await?;
                    this.publish_executable(&executable);
                    Ok(())
                }
            };
            let result = tokio::select! {_=cancel.cancelled()=>Err(ServiceError::Cancelled),result=work=>result};
            if let Err(error) = result {
                this.state.send_modify(|s|{if s.operation_id.as_ref().is_some_and(|v|v.0==id) && s.phase!=ProviderInstallPhase::Succeeded{s.phase=if error==ServiceError::Cancelled{ProviderInstallPhase::Cancelled}else{ProviderInstallPhase::Failed};s.message=Some(if error==ServiceError::Cancelled{"Installation cancelled. The previous runtime is unchanged."}else{"Could not finish the Codex installation. Check disk space and directory access."}.into());}});
            }
            done.send_replace(true);
        });
        Ok(self.state())
    }
    pub async fn cancel(&self, id: &str) -> ServiceResult<ProviderInstallState> {
        let gate = self.gate.lock().await;
        let active = gate
            .as_ref()
            .filter(|a| a.id == id)
            .ok_or(ServiceError::Inactive)?;
        if running(&self.state()) {
            active.cancel.cancel();
            let mut done = active.done.clone();
            let _ = done.wait_for(|v| *v).await;
        }
        Ok(self.state())
    }
    pub async fn wait(&self) {
        let done = self.gate.lock().await.as_ref().map(|a| a.done.clone());
        if let Some(mut done) = done {
            let _ = done.wait_for(|v| *v).await;
        }
    }
    async fn install(&self, release: ReleaseAsset) -> ServiceResult<()> {
        if !valid_version(&release.version)
            || !valid_digest(&release.sha256)
            || !crate::registry::https_url(&release.url)
        {
            return Err(ServiceError::Maintenance);
        }
        self.eligible(&release)?;
        let root = self.root()?;
        let destination = root.join(&release.version);
        self.state.send_modify(|s| s.can_remove = true);
        if destination.exists() {
            let executable = self.completed(&release.version)?;
            let receipt = record(&destination.join(".install-complete.json"))?;
            if receipt["sha256"] != release.sha256 || receipt["target"] != release.target {
                return Err(ServiceError::Maintenance);
            }
            self.validator
                .validate(&executable.path, &release.version, &self.environment)
                .await?;
            self.activate(&root, &release)?;
            return Ok(());
        }
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&root)
            .map_err(|_| ServiceError::Storage)?;
        let archive = staging.path().join("download.tar.gz");
        let runtime = staging.path().join("runtime");
        fs::create_dir(&runtime).map_err(|_| ServiceError::Storage)?;
        let (progress, mut receiver) = watch::channel(0);
        let download = self.transport.download(&release, &archive, progress);
        tokio::pin!(download);
        let bounded = async {
            loop {
                tokio::select! {result=&mut download=>break result,changed=receiver.changed()=>{if changed.is_ok(){let bytes=*receiver.borrow_and_update();self.state.send_modify(|s|s.downloaded_bytes=bytes);}}}
            }
        };
        tokio::time::timeout(Duration::from_secs(45 * 60), bounded)
            .await
            .map_err(|_| ServiceError::Network)??;
        let mut file = fs::File::open(&archive).map_err(|_| ServiceError::Storage)?;
        let size = file.metadata().map_err(|_| ServiceError::Storage)?.len();
        if size > release.archive_bytes.unwrap_or(1024 * 1024 * 1024)
            || release
                .archive_bytes
                .is_some_and(|expected| expected != size)
        {
            return Err(ServiceError::Maintenance);
        }
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|_| ServiceError::Storage)?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        if format!("{:x}", digest.finalize()) != release.sha256 {
            return Err(ServiceError::Maintenance);
        }
        self.state.send_modify(|s| {
            s.phase = ProviderInstallPhase::Extracting;
            s.downloaded_bytes = size;
            s.message = Some("Extracting Codex.".into());
        });
        let tar = if self.platform == Platform::Windows {
            PathBuf::from(
                self.environment
                    .get("SYSTEMROOT")
                    .or_else(|| self.environment.get("SystemRoot"))
                    .or_else(|| self.environment.get("windir"))
                    .map(String::as_str)
                    .unwrap_or("C:\\Windows"),
            )
            .join("System32")
            .join("tar.exe")
            .to_string_lossy()
            .into_owned()
        } else {
            "tar".into()
        };
        let names = self
            .tar(&tar, &["-tzf", &archive.to_string_lossy()])
            .await?;
        let kinds = self
            .tar(&tar, &["-tvzf", &archive.to_string_lossy()])
            .await?;
        validate_archive(&names, &kinds)?;
        self.tar(
            &tar,
            &[
                "-xzf",
                &archive.to_string_lossy(),
                "-C",
                &runtime.to_string_lossy(),
            ],
        )
        .await?;
        self.state.send_modify(|s| {
            s.phase = ProviderInstallPhase::Verifying;
            s.message = Some("Checking Codex.".into());
        });
        let executable = self.package_directory(&runtime, &release.version, &release.target)?;
        self.eligible(&release)?;
        self.validator
            .validate(&executable.path, &release.version, &self.environment)
            .await?;
        fs::write(
            runtime.join(".install-complete.json"),
            json!({"version":release.version,"target":release.target,"sha256":release.sha256})
                .to_string(),
        )
        .map_err(|_| ServiceError::Storage)?;
        for attempt in 0..=10 {
            match fs::rename(&runtime, &destination) {
                Ok(()) => break,
                Err(_) if self.platform == Platform::Windows && attempt < 10 => {
                    tokio::time::sleep(Duration::from_millis(100)).await
                }
                Err(_) => return Err(ServiceError::Storage),
            }
        }
        self.activate(&root, &release)
    }
    fn eligible(&self, release: &ReleaseAsset) -> ServiceResult<()> {
        if self
            .pin
            .as_ref()
            .is_some_and(|p| p.version == release.version)
            || self.supported(&release.version)
        {
            Ok(())
        } else {
            Err(ServiceError::Maintenance)
        }
    }
    fn activate(&self, root: &Path, release: &ReleaseAsset) -> ServiceResult<()> {
        self.eligible(release)?;
        let executable = self.completed(&release.version)?;
        let mut pointer =
            tempfile::NamedTempFile::new_in(root).map_err(|_| ServiceError::Storage)?;
        write!(pointer, "{}", json!({"version":release.version}))
            .map_err(|_| ServiceError::Storage)?;
        pointer
            .persist(root.join("active.json"))
            .map_err(|_| ServiceError::Storage)?;
        self.publish_executable(&executable);
        Ok(())
    }
    async fn tar(&self, command: &str, args: &[&str]) -> ServiceResult<String> {
        let mut options = process_options(
            command,
            &args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            &self.environment,
        );
        options.timeout = Some(Duration::from_secs(300));
        options.max_output_bytes = 8 * 1024 * 1024;
        let output = t3_process::run(options)
            .await
            .map_err(|_| ServiceError::Maintenance)?;
        Ok(output.stdout_text())
    }
    pub async fn remove(&self, protected: &[PathBuf]) -> ServiceResult<()> {
        let _gate = self.gate.lock().await;
        if running(&self.state()) {
            return Err(ServiceError::Busy);
        }
        let _leases = self.leases.try_write().map_err(|_| ServiceError::Busy)?;
        if self.directory.exists() {
            let root = self.root()?;
            for path in protected {
                let path = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                if path.starts_with(&root) {
                    return Err(ServiceError::Busy);
                }
            }
            let parent = root.parent().ok_or(ServiceError::Storage)?;
            remove_inside(parent, &root)?;
        }
        self.state.send_modify(|s| {
            s.phase = ProviderInstallPhase::Idle;
            s.operation_id = None;
            s.downloaded_bytes = 0;
            s.installed_version = None;
            s.executable_path = Some(None);
            s.source = Some(None);
            s.can_remove = false;
            s.message = None;
            s.version = self.pin.as_ref().map(|p| p.version.clone().into());
            s.total_bytes = self.pin.as_ref().and_then(|p| p.archive_bytes);
        });
        if let Some(local) = self
            .local
            .resolve()
            .await?
            .filter(|v| self.supported(&v.version))
        {
            self.publish_executable(&local);
        }
        Ok(())
    }
}
fn valid_version(value: &str) -> bool {
    crate::versions::parse_version(value).is_some()
        && !value.contains(['/', '\\', ':', '+'])
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn record(path: &Path) -> ServiceResult<Value> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ServiceError::Storage)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 8192 {
        return Err(ServiceError::Maintenance);
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| ServiceError::Storage)?)
        .map_err(|_| ServiceError::Maintenance)
}
fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}
pub fn validate_archive(names: &str, kinds: &str) -> ServiceResult<()> {
    let names: Vec<_> = names.trim().lines().collect();
    let kinds: Vec<_> = kinds.trim().lines().collect();
    if names.is_empty()
        || names.len() > 10000
        || names.len() != kinds.len()
        || kinds.iter().any(|line| !line.starts_with(['-', 'd']))
        || names.iter().any(|name| {
            name.is_empty()
                || name.contains('\\')
                || name.starts_with('/')
                || name
                    .trim_end_matches('/')
                    .split('/')
                    .any(|part| matches!(part, "" | "." | "..") || part.contains(':'))
        })
    {
        Err(ServiceError::Maintenance)
    } else {
        Ok(())
    }
}
fn running(state: &ProviderInstallState) -> bool {
    matches!(
        state.phase,
        ProviderInstallPhase::Downloading
            | ProviderInstallPhase::Extracting
            | ProviderInstallPhase::Verifying
    )
}
fn remove_inside(root: &Path, path: &Path) -> ServiceResult<()> {
    let target = fs::canonicalize(path).map_err(|_| ServiceError::Storage)?;
    let root = fs::canonicalize(root).map_err(|_| ServiceError::Storage)?;
    if target == root
        || !target.starts_with(&root)
        || fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(ServiceError::Storage);
    }
    fs::remove_dir_all(path).map_err(|_| ServiceError::Storage)
}
