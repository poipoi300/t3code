use crate::{
    antigravity_release::{self, Format, Release},
    catalog::ManifestSource,
    environment::{Environment, Platform, process_options},
    sensitive::{ServiceError, ServiceResult},
    transport::ProviderClock,
    versions::{compare_versions, resolve_compatibility},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    provider_setup::{ProviderInstallPhase, ProviderInstallSource, ProviderInstallState},
    server::ServerProviderCompatibilityStatus,
};
use t3_process::CancellationToken;
use tokio::sync::{Mutex, OwnedRwLockReadGuard, RwLock, watch};
const MAX_BYTES: u64 = 512 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableSource {
    Override,
    Managed,
    Path,
}
#[derive(Debug, Clone)]
pub struct AntigravityExecutable {
    pub executable: PathBuf,
    pub harness: Option<PathBuf>,
    pub source: ExecutableSource,
    pub version: Option<String>,
    pub managed_directory: Option<PathBuf>,
}
pub struct ExecutableLease {
    pub executable: AntigravityExecutable,
    _lease: Option<OwnedRwLockReadGuard<()>>,
}
#[async_trait]
pub trait AntigravityReleases: Send + Sync {
    async fn latest(&self, target: &str) -> ServiceResult<Option<Release>>;
    async fn download(
        &self,
        release: &Release,
        path: &Path,
        progress: watch::Sender<u64>,
    ) -> ServiceResult<()>;
}
pub struct ReqwestAntigravityReleases {
    client: reqwest::Client,
    platform: Platform,
    architecture: String,
    musl: bool,
}
impl ReqwestAntigravityReleases {
    pub fn new(platform: Platform, architecture: String, musl: bool) -> ServiceResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ServiceError::Network)?,
            platform,
            architecture,
            musl,
        })
    }
}
#[async_trait]
impl AntigravityReleases for ReqwestAntigravityReleases {
    async fn latest(&self, target: &str) -> ServiceResult<Option<Release>> {
        let value = self
            .client
            .get(format!(
                "{}/{target}.json",
                antigravity_release::MANIFEST_BASE
            ))
            .send()
            .await
            .map_err(|_| ServiceError::Network)?
            .error_for_status()
            .map_err(|_| ServiceError::Network)?
            .bytes()
            .await
            .map_err(|_| ServiceError::Network)?;
        if value.len() > 65536 {
            return Err(ServiceError::Response);
        }
        Ok(antigravity_release::parse(
            &serde_json::from_slice::<serde_json::Value>(&value)
                .map_err(|_| ServiceError::Response)?,
            self.platform,
            &self.architecture,
            self.musl,
        ))
    }
    async fn download(
        &self,
        release: &Release,
        path: &Path,
        progress: watch::Sender<u64>,
    ) -> ServiceResult<()> {
        use tokio::io::AsyncWriteExt;
        let mut response = self
            .client
            .get(&release.url)
            .send()
            .await
            .map_err(|_| ServiceError::Network)?
            .error_for_status()
            .map_err(|_| ServiceError::Network)?;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
            .map_err(|_| ServiceError::Storage)?;
        let mut bytes = 0u64;
        while let Some(chunk) = response.chunk().await.map_err(|_| ServiceError::Network)? {
            bytes = bytes
                .checked_add(chunk.len() as u64)
                .ok_or(ServiceError::Response)?;
            if bytes > MAX_BYTES
                || release
                    .archive_bytes
                    .is_some_and(|expected| bytes > expected)
            {
                return Err(ServiceError::Response);
            }
            file.write_all(&chunk)
                .await
                .map_err(|_| ServiceError::Storage)?;
            progress.send_replace(bytes);
        }
        file.flush().await.map_err(|_| ServiceError::Storage)?;
        Ok(())
    }
}
#[async_trait]
pub trait AntigravityValidator: Send + Sync {
    async fn validate(
        &self,
        executable: &AntigravityExecutable,
        version: &str,
    ) -> ServiceResult<()>;
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileRecord {
    name: String,
    bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    release_id: String,
    version: String,
    executable: FileRecord,
    #[serde(with = "t3_contracts::wire::nullable")]
    harness: Option<FileRecord>,
}
pub struct InstallationOptions {
    pub base: PathBuf,
    pub platform: Platform,
    pub architecture: String,
    pub musl: bool,
    pub environment: Environment,
    pub manifest: ManifestSource,
    pub server_version: String,
    pub pin: Option<Release>,
    pub update_checks: bool,
}
struct Running {
    operation: String,
    cancel: CancellationToken,
    finished: CancellationToken,
}
pub struct AntigravityInstallation {
    options: InstallationOptions,
    directory: PathBuf,
    releases: Arc<dyn AntigravityReleases>,
    validator: Arc<dyn AntigravityValidator>,
    clock: Arc<dyn ProviderClock>,
    state: watch::Sender<ProviderInstallState>,
    gate: Mutex<Option<Running>>,
    leases: Mutex<BTreeMap<PathBuf, Arc<RwLock<()>>>>,
    latest: Mutex<(Option<i64>, Option<Release>)>,
}
fn running(state: &ProviderInstallState) -> bool {
    matches!(
        state.phase,
        ProviderInstallPhase::Downloading
            | ProviderInstallPhase::Extracting
            | ProviderInstallPhase::Verifying
    )
}
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn executable(path: &Path, size: Option<u64>) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file()
        || metadata.len() == 0
        || size.is_some_and(|expected| expected != metadata.len())
    {
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
fn record<T: serde::de::DeserializeOwned>(path: &Path) -> ServiceResult<T> {
    let mut bytes = vec![];
    fs::File::open(path)
        .map_err(|_| ServiceError::Storage)?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| ServiceError::Storage)?;
    if bytes.len() > 8192 {
        return Err(ServiceError::Response);
    }
    serde_json::from_slice(&bytes).map_err(|_| ServiceError::Response)
}
fn atomic<T: Serialize>(path: &Path, value: &T) -> ServiceResult<()> {
    let parent = path.parent().ok_or(ServiceError::Storage)?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| ServiceError::Storage)?;
    serde_json::to_writer(&mut file, value).map_err(|_| ServiceError::Storage)?;
    file.write_all(b"\n").map_err(|_| ServiceError::Storage)?;
    file.persist(path).map_err(|_| ServiceError::Storage)?;
    Ok(())
}
fn safe_member(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && !matches!(name, "." | "..")
        && !name.chars().any(char::is_control)
}
impl AntigravityInstallation {
    pub fn new(
        options: InstallationOptions,
        releases: Arc<dyn AntigravityReleases>,
        validator: Arc<dyn AntigravityValidator>,
        clock: Arc<dyn ProviderClock>,
    ) -> Arc<Self> {
        let platform = match options.platform {
            Platform::Windows => "win32",
            Platform::Macos => "darwin",
            Platform::Linux => "linux",
        };
        let directory = options
            .base
            .join("tools")
            .join("antigravity-acp")
            .join(format!("{platform}-{}", options.architecture));
        let (state, _) = watch::channel(ProviderInstallState {
            driver: "antigravity".into(),
            operation_id: None,
            phase: ProviderInstallPhase::Idle,
            downloaded_bytes: 0,
            total_bytes: options
                .pin
                .as_ref()
                .and_then(|release| release.archive_bytes),
            version: options
                .pin
                .as_ref()
                .map(|release| release.version.clone().into()),
            installed_version: None,
            executable_path: None,
            source: None,
            can_remove: directory.exists(),
            message: None,
        });
        let installation = Arc::new(Self {
            options,
            directory,
            releases,
            validator,
            clock,
            state,
            gate: Mutex::new(None),
            leases: Mutex::new(BTreeMap::new()),
            latest: Mutex::new((None, None)),
        });
        if installation.directory.join("active.json").exists() {
            let restored = installation.root().and_then(|root| {
                let active: serde_json::Value = record(&root.join("active.json"))?;
                installation.completed(
                    &root,
                    active["releaseId"].as_str().ok_or(ServiceError::Response)?,
                )
            });
            match restored {
                Ok(executable) => installation.state.send_modify(|state| {
                    state.installed_version = executable.version.clone().map(Into::into);
                    if state.version.as_ref().is_none_or(|version| {
                        executable.version.as_deref().is_some_and(|installed| {
                            compare_versions(installed, &version.0).is_gt()
                        })
                    }) {
                        state.version = state.installed_version.clone();
                    }
                }),
                Err(_) => installation.state.send_modify(|state| {
                    state.phase = ProviderInstallPhase::Failed;
                    state.message = Some(
                        "The managed Antigravity runtime is incomplete. Remove it and reinstall."
                            .into(),
                    );
                }),
            }
        }
        installation
    }
    pub fn state(&self) -> ProviderInstallState {
        self.state.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<ProviderInstallState> {
        self.state.subscribe()
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    fn eligible(&self, version: &str) -> bool {
        let manifest = self.options.manifest.current();
        let compatibility = resolve_compatibility(
            manifest.compatibility.as_deref().unwrap_or_default(),
            "antigravity",
            Some(version),
            &self.options.server_version,
        )
        .or_else(|| {
            let bundled = crate::catalog::Manifest::bundled().ok()?;
            resolve_compatibility(
                bundled.compatibility.as_deref().unwrap_or_default(),
                "antigravity",
                Some(version),
                &self.options.server_version,
            )
        });
        !compatibility.is_some_and(|advisory| {
            matches!(
                advisory.status,
                ServerProviderCompatibilityStatus::Broken
                    | ServerProviderCompatibilityStatus::Unsupported
            )
        })
    }
    fn root(&self) -> ServiceResult<PathBuf> {
        fs::create_dir_all(&self.options.base).map_err(|_| ServiceError::Storage)?;
        let mut root = fs::canonicalize(&self.options.base).map_err(|_| ServiceError::Storage)?;
        let relative = self
            .directory
            .strip_prefix(&self.options.base)
            .map_err(|_| ServiceError::Storage)?;
        for component in relative.components() {
            let path = root.join(component);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(ServiceError::Storage);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&path).map_err(|_| ServiceError::Storage)?
                }
                Err(_) => return Err(ServiceError::Storage),
            }
            let canonical = fs::canonicalize(&path).map_err(|_| ServiceError::Storage)?;
            if canonical.parent() != Some(root.as_path()) {
                return Err(ServiceError::Storage);
            }
            root = canonical;
        }
        Ok(root)
    }
    fn completed(&self, root: &Path, id: &str) -> ServiceResult<AntigravityExecutable> {
        if !valid_id(id) {
            return Err(ServiceError::Response);
        }
        let versions = root.join("versions");
        if fs::symlink_metadata(&versions).is_ok_and(|metadata| metadata.file_type().is_symlink())
            || fs::canonicalize(&versions)
                .map_err(|_| ServiceError::Storage)?
                .parent()
                != Some(root)
        {
            return Err(ServiceError::Storage);
        }
        let directory = versions.join(id);
        if fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(ServiceError::Storage);
        }
        let value: Record = record(&directory.join(".install-complete.json"))?;
        let (exe, harness) = if self.options.platform == Platform::Windows {
            ("agy_acp_server.exe", "localharness_external.exe")
        } else {
            ("agy_acp_server.par", "localharness_external")
        };
        let cli = if self.options.platform == Platform::Windows {
            "agy.exe"
        } else {
            "antigravity"
        };
        if value.release_id != id
            || value.version.trim().is_empty()
            || value.executable.name != if value.harness.is_some() { exe } else { cli }
            || value
                .harness
                .as_ref()
                .is_some_and(|file| file.name != harness)
            || !executable(
                &directory.join(&value.executable.name),
                Some(value.executable.bytes),
            )
            || value
                .harness
                .as_ref()
                .is_some_and(|file| !executable(&directory.join(&file.name), Some(file.bytes)))
        {
            return Err(ServiceError::Maintenance);
        }
        Ok(AntigravityExecutable {
            executable: directory.join(value.executable.name),
            harness: value.harness.map(|file| directory.join(file.name)),
            source: ExecutableSource::Managed,
            version: Some(value.version),
            managed_directory: Some(directory),
        })
    }
    fn external(
        &self,
        candidate: &Path,
        source: ExecutableSource,
        root: &Path,
    ) -> ServiceResult<Option<AntigravityExecutable>> {
        if !executable(candidate, None) {
            return Ok(None);
        }
        let real = fs::canonicalize(candidate).map_err(|_| ServiceError::Storage)?;
        if let Some(directory) = real.parent()
            && directory.parent() == Some(root.join("versions").as_path())
            && let Some(id) = directory
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|id| valid_id(id))
        {
            let mut installed = self.completed(root, id)?;
            installed.source = source;
            return Ok(Some(installed));
        }
        let is_cli = [candidate, &real].into_iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    regex::Regex::new(r"(?i)^(agy|antigravity)(\.(exe|cmd|bat))?$")
                        .expect("fixed CLI name")
                        .is_match(name)
                })
        });
        if is_cli {
            return Ok(Some(AntigravityExecutable {
                executable: candidate.into(),
                harness: None,
                source,
                version: None,
                managed_directory: None,
            }));
        }
        let harness = real.with_file_name(if self.options.platform == Platform::Windows {
            "localharness_external.exe"
        } else {
            "localharness_external"
        });
        Ok(executable(&harness, None).then_some(AntigravityExecutable {
            executable: real,
            harness: Some(harness),
            source,
            version: None,
            managed_directory: None,
        }))
    }
    pub async fn resolve(
        &self,
        binary: Option<&str>,
        environment: &Environment,
    ) -> ServiceResult<AntigravityExecutable> {
        let root = self.root()?;
        if let Some(binary) = binary
            .map(t3_contracts::wire::trim_js)
            .filter(|binary| !binary.is_empty())
        {
            let options = process_options(binary, &[], environment);
            let command = t3_process::resolve_command(&options.program, &[], &options.env, false)
                .map_err(|_| ServiceError::Unavailable)?;
            return self
                .external(
                    Path::new(&command.program),
                    ExecutableSource::Override,
                    &root,
                )?
                .ok_or(ServiceError::Unavailable);
        }
        let pointer = root.join("active.json");
        if pointer.exists() {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Active {
                release_id: String,
            }
            let active: Active = record(&pointer)?;
            let installed = self.completed(&root, &active.release_id)?;
            if self.eligible(installed.version.as_deref().unwrap_or_default()) {
                return Ok(installed);
            }
        }
        let command = if self.options.platform == Platform::Windows {
            "agy_acp_server.exe"
        } else {
            "agy_acp_server.par"
        };
        let delimiter = if self.options.platform == Platform::Windows {
            ';'
        } else {
            ':'
        };
        let paths = environment
            .iter()
            .rfind(|(name, _)| {
                if self.options.platform == Platform::Windows {
                    name.eq_ignore_ascii_case("PATH")
                } else {
                    name.as_str() == "PATH"
                }
            })
            .map(|(_, value)| value.as_str())
            .unwrap_or_default();
        for directory in paths
            .split(delimiter)
            .map(|directory| directory.trim().trim_matches('"'))
            .filter(|directory| !directory.is_empty())
        {
            if let Some(executable) = self.external(
                &Path::new(directory).join(command),
                ExecutableSource::Path,
                &root,
            )? {
                return Ok(executable);
            }
        }
        Err(ServiceError::Unavailable)
    }
    pub async fn acquire(
        &self,
        binary: Option<&str>,
        environment: &Environment,
    ) -> ServiceResult<ExecutableLease> {
        let _gate = self.gate.lock().await;
        let executable = self.resolve(binary, environment).await?;
        let lease = if let Some(directory) = &executable.managed_directory {
            Some(
                self.leases
                    .lock()
                    .await
                    .entry(directory.clone())
                    .or_default()
                    .clone()
                    .read_owned()
                    .await,
            )
        } else {
            None
        };
        Ok(ExecutableLease {
            executable,
            _lease: lease,
        })
    }
    async fn refresh_release(&self, fresh: bool) {
        if self.options.pin.is_some() {
            return;
        }
        if !fresh && !self.options.update_checks {
            return;
        }
        let mut latest = self.latest.lock().await;
        let now = self.clock.now_ms();
        if !fresh && latest.0.is_some_and(|at| now >= at && now - at < 3600000) {
            return;
        }
        latest.0 = Some(now);
        let Some(target) = antigravity_release::target(
            self.options.platform,
            &self.options.architecture,
            self.options.musl,
        ) else {
            return;
        };
        if let Ok(Ok(Some(release))) =
            tokio::time::timeout(Duration::from_secs(20), self.releases.latest(&target)).await
        {
            latest.1 = Some(release).filter(|release| self.eligible(&release.version));
        }
    }
    pub async fn check_for_update(&self) {
        self.refresh_release(false).await;
        let latest = self.latest.lock().await.1.clone();
        if !running(&self.state())
            && let Some(release) = latest
        {
            self.state.send_modify(|state| {
                if state.installed_version.as_ref().is_none_or(|installed| {
                    !self.eligible(&installed.0)
                        || !compare_versions(&installed.0, &release.version).is_gt()
                }) {
                    state.version = Some(release.version.clone().into());
                    state.total_bytes = release.archive_bytes;
                }
            });
        }
    }
    pub async fn start(self: &Arc<Self>) -> ServiceResult<ProviderInstallState> {
        if running(&self.state()) {
            return Ok(self.state());
        }
        self.refresh_release(true).await;
        let mut gate = self.gate.lock().await;
        if running(&self.state()) {
            return Ok(self.state());
        }
        let release = self
            .latest
            .lock()
            .await
            .1
            .clone()
            .or_else(|| self.options.pin.clone())
            .filter(|release| self.eligible(&release.version));
        let installed = self
            .resolve(None, &self.options.environment)
            .await
            .ok()
            .filter(|installed| {
                installed.source == ExecutableSource::Managed && installed.harness.is_none()
            });
        let reuse = installed.filter(|installed| {
            release.as_ref().is_none_or(|release| {
                installed
                    .version
                    .as_deref()
                    .is_some_and(|version| compare_versions(version, &release.version).is_gt())
            })
        });
        if release.is_none() && reuse.is_none() {
            return Err(ServiceError::Unavailable);
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let finished = CancellationToken::new();
        *gate = Some(Running {
            operation: operation.clone(),
            cancel: cancel.clone(),
            finished: finished.clone(),
        });
        self.state.send_modify(|state| {
            state.operation_id = Some(operation.clone().into());
            state.phase = if reuse.is_some() {
                ProviderInstallPhase::Verifying
            } else {
                ProviderInstallPhase::Downloading
            };
            state.downloaded_bytes = 0;
            state.total_bytes = release.as_ref().and_then(|release| release.archive_bytes);
            state.version = reuse
                .as_ref()
                .and_then(|installed| installed.version.clone())
                .or_else(|| release.as_ref().map(|release| release.version.clone()))
                .map(Into::into);
            state.message = None;
        });
        let initial = self.state();
        let owner = self.clone();
        tokio::spawn(async move {
            let work = async {
                if let Some(installed) = reuse {
                    owner
                        .validator
                        .validate(&installed, installed.version.as_deref().unwrap_or_default())
                        .await?;
                    if !owner.eligible(installed.version.as_deref().unwrap_or_default()) {
                        return Err(ServiceError::Maintenance);
                    }
                    owner.publish(&installed);
                    Ok(())
                } else {
                    owner
                        .install(release.ok_or(ServiceError::Unavailable)?)
                        .await
                }
            };
            let result = tokio::select! {biased;_=cancel.cancelled()=>Err(ServiceError::Cancelled),result=work=>result};
            if let Err(error) = result {
                owner.state.send_modify(|state|{if state.operation_id.as_ref().is_some_and(|id|id.0==operation)&&state.phase!=ProviderInstallPhase::Succeeded{state.phase=if error==ServiceError::Cancelled{ProviderInstallPhase::Cancelled}else{ProviderInstallPhase::Failed};state.message=Some(if error==ServiceError::Cancelled{"Installation cancelled. The previous runtime is unchanged."}else{"Could not finish the Antigravity installation. Check its release, disk space and directory access."}.into());}});
            }
            finished.cancel();
        });
        Ok(initial)
    }
    fn publish(&self, executable: &AntigravityExecutable) {
        self.state.send_modify(|state| {
            state.phase = ProviderInstallPhase::Succeeded;
            state.installed_version = executable.version.clone().map(Into::into);
            state.executable_path = Some(Some(
                executable.executable.to_string_lossy().into_owned().into(),
            ));
            state.source = Some(Some(if executable.source == ExecutableSource::Managed {
                ProviderInstallSource::Managed
            } else {
                ProviderInstallSource::Local
            }));
            state.can_remove = true;
            state.message = None;
        });
    }
    async fn install(&self, release: Release) -> ServiceResult<()> {
        let id = release
            .id()
            .filter(|id| valid_id(id))
            .ok_or(ServiceError::Response)?
            .to_owned();
        if !self.eligible(&release.version)
            || !safe_member(&release.executable.name)
            || release
                .harness
                .as_ref()
                .is_some_and(|file| !safe_member(&file.name))
        {
            return Err(ServiceError::Maintenance);
        }
        let root = self.root()?;
        let versions = root.join("versions");
        match fs::symlink_metadata(&versions) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ServiceError::Storage);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&versions).map_err(|_| ServiceError::Storage)?
            }
            Err(_) => return Err(ServiceError::Storage),
        }
        let destination = versions.join(&id);
        if !destination.exists() {
            let required = release
                .archive_bytes
                .unwrap_or(MAX_BYTES)
                .saturating_add(release.executable.bytes.unwrap_or(MAX_BYTES))
                .saturating_add(
                    release
                        .harness
                        .as_ref()
                        .and_then(|file| file.bytes)
                        .unwrap_or(0),
                )
                .saturating_add(256 * 1024 * 1024);
            if fs4::available_space(&versions).is_ok_and(|available| available < required) {
                return Err(ServiceError::Storage);
            }
            if release.harness.is_some()
                && self.options.platform == Platform::Linux
                && matches!(self.options.architecture.as_str(), "arm64" | "aarch64")
            {
                let mut options =
                    process_options("getconf", &["PAGESIZE".into()], &self.options.environment);
                options.timeout = Some(Duration::from_secs(5));
                options.max_output_bytes = 128;
                if t3_process::run(options)
                    .await
                    .ok()
                    .and_then(|output| output.stdout_text().trim().parse::<u64>().ok())
                    .is_some_and(|size| size > 4096)
                {
                    return Err(ServiceError::Maintenance);
                }
            }
            let stage = Arc::new(
                tempfile::Builder::new()
                    .prefix(".install-")
                    .tempdir_in(&versions)
                    .map_err(|_| ServiceError::Storage)?,
            );
            let archive = stage.path().join("download");
            let (progress, mut receiver) = watch::channel(0);
            let download = tokio::time::timeout(
                Duration::from_secs(45 * 60),
                self.releases.download(&release, &archive, progress),
            );
            tokio::pin!(download);
            loop {
                tokio::select! {result=&mut download=>{result.map_err(|_|ServiceError::Network)??;break;},changed=receiver.changed()=>{if changed.is_ok(){let bytes=*receiver.borrow_and_update();self.state.send_modify(|state|state.downloaded_bytes=bytes);}}}
            }
            let asset = release.clone();
            let retained = stage.clone();
            tokio::task::spawn_blocking(move || {
                verify_download(&retained.path().join("download"), &asset)
            })
            .await
            .map_err(|_| ServiceError::Maintenance)??;
            self.state
                .send_modify(|state| state.phase = ProviderInstallPhase::Extracting);
            extract(
                stage.clone(),
                &release,
                self.options.platform,
                &self.options.environment,
            )
            .await?;
            if release.harness.is_some()
                && self.options.platform == Platform::Macos
                && matches!(self.options.architecture.as_str(), "x64" | "x86_64")
            {
                let runtime = stage.path().join("runtime");
                let mut args = vec![
                    "--force".into(),
                    "--sign".into(),
                    "-".into(),
                    runtime
                        .join(&release.executable.name)
                        .to_string_lossy()
                        .into_owned(),
                ];
                if let Some(harness) = &release.harness {
                    args.push(runtime.join(&harness.name).to_string_lossy().into_owned());
                }
                let mut options =
                    process_options("/usr/bin/codesign", &args, &self.options.environment);
                options.timeout = Some(Duration::from_secs(30));
                t3_process::run(options)
                    .await
                    .map_err(|_| ServiceError::Maintenance)?;
            }
            self.state
                .send_modify(|state| state.phase = ProviderInstallPhase::Verifying);
            let runtime = stage.path().join("runtime");
            let staged = AntigravityExecutable {
                executable: runtime.join(&release.executable.name),
                harness: release
                    .harness
                    .as_ref()
                    .map(|file| runtime.join(&file.name)),
                source: ExecutableSource::Managed,
                version: Some(release.version.clone()),
                managed_directory: Some(runtime.clone()),
            };
            tokio::time::timeout(
                Duration::from_secs(90),
                self.validator.validate(&staged, &release.version),
            )
            .await
            .map_err(|_| ServiceError::Maintenance)??;
            if !self.eligible(&release.version) {
                return Err(ServiceError::Maintenance);
            }
            let entry = |path: &Path, name: &str| -> ServiceResult<FileRecord> {
                let bytes = fs::metadata(path).map_err(|_| ServiceError::Storage)?.len();
                Ok(FileRecord {
                    name: name.into(),
                    bytes,
                })
            };
            let value = Record {
                release_id: id.clone(),
                version: release.version.clone(),
                executable: entry(&staged.executable, &release.executable.name)?,
                harness: staged
                    .harness
                    .as_ref()
                    .zip(release.harness.as_ref())
                    .map(|(path, file)| entry(path, &file.name))
                    .transpose()?,
            };
            atomic(&runtime.join(".install-complete.json"), &value)?;
            if fs::rename(&runtime, &destination).is_err() {
                let previous = self.completed(&root, &id)?;
                if previous.version.as_deref() != Some(&release.version) {
                    return Err(ServiceError::Maintenance);
                }
            }
        } else {
            let previous = self.completed(&root, &id)?;
            if previous.version.as_deref() != Some(&release.version) {
                return Err(ServiceError::Maintenance);
            }
            tokio::time::timeout(
                Duration::from_secs(90),
                self.validator.validate(&previous, &release.version),
            )
            .await
            .map_err(|_| ServiceError::Maintenance)??;
        }
        if !self.eligible(&release.version) {
            return Err(ServiceError::Maintenance);
        }
        atomic(
            &root.join("active.json"),
            &serde_json::json!({"releaseId":id}),
        )?;
        let executable = self.completed(&root, &id)?;
        self.publish(&executable);
        Ok(())
    }
    pub async fn cancel(&self, operation: &str) -> ServiceResult<ProviderInstallState> {
        let gate = self.gate.lock().await;
        let active = gate
            .as_ref()
            .filter(|active| active.operation == operation)
            .ok_or(ServiceError::Inactive)?;
        if running(&self.state()) {
            active.cancel.cancel();
            active.finished.cancelled().await;
        }
        Ok(self.state())
    }
    pub async fn remove(&self, protected: &[PathBuf]) -> ServiceResult<ProviderInstallState> {
        let gate = self.gate.lock().await;
        if running(&self.state())
            || gate
                .as_ref()
                .is_some_and(|active| !active.finished.is_cancelled())
        {
            return Err(ServiceError::Busy);
        }
        let leases = self.leases.lock().await;
        let guards = leases
            .values()
            .map(|lease| lease.try_write())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ServiceError::Busy)?;
        let root = self.root()?;
        for path in protected {
            let real = crate::environment::canonicalize_existing_parent(path)
                .map_err(|_| ServiceError::Storage)?;
            if real == root || real.starts_with(&root) {
                return Err(ServiceError::Busy);
            }
        }
        if fs::symlink_metadata(&root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(ServiceError::Storage);
        }
        fs::remove_dir_all(&root).map_err(|_| ServiceError::Storage)?;
        drop(guards);
        self.state.send_modify(|state| {
            state.phase = ProviderInstallPhase::Idle;
            state.operation_id = None;
            state.installed_version = None;
            state.can_remove = false;
            state.executable_path = None;
            state.source = None;
            state.downloaded_bytes = 0;
            state.message = None;
        });
        Ok(self.state())
    }
}
fn verify_download(path: &Path, release: &Release) -> ServiceResult<()> {
    let metadata = fs::metadata(path).map_err(|_| ServiceError::Storage)?;
    if metadata.len() == 0
        || metadata.len() > MAX_BYTES
        || release
            .archive_bytes
            .is_some_and(|bytes| bytes != metadata.len())
    {
        return Err(ServiceError::Maintenance);
    }
    let mut file = fs::File::open(path).map_err(|_| ServiceError::Storage)?;
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|_| ServiceError::Storage)?;
        if count == 0 {
            break;
        }
        sha256.update(&buffer[..count]);
        sha512.update(&buffer[..count]);
    }
    if let Some(expected) = &release.sha512 {
        if expected.to_lowercase() != format!("{:x}", sha512.finalize()) {
            return Err(ServiceError::Maintenance);
        }
    } else if release
        .sha256
        .as_ref()
        .is_none_or(|expected| expected.to_lowercase() != format!("{:x}", sha256.finalize()))
    {
        return Err(ServiceError::Maintenance);
    }
    Ok(())
}
async fn extract(
    stage: Arc<tempfile::TempDir>,
    release: &Release,
    platform: Platform,
    environment: &Environment,
) -> ServiceResult<()> {
    let root = stage.path().join("runtime");
    fs::create_dir(&root).map_err(|_| ServiceError::Storage)?;
    let archive = stage.path().join("download");
    match release.format {
        Format::Executable => {
            let stage = stage.clone();
            let name = release.executable.name.clone();
            tokio::task::spawn_blocking(move || {
                fs::copy(
                    stage.path().join("download"),
                    stage.path().join("runtime").join(name),
                )
                .map_err(|_| ServiceError::Storage)
            })
            .await
            .map_err(|_| ServiceError::Storage)??;
        }
        Format::Zip => {
            let retained = stage.clone();
            let release = release.clone();
            tokio::task::spawn_blocking(move || {
                let file = fs::File::open(retained.path().join("download"))
                    .map_err(|_| ServiceError::Storage)?;
                let mut zip = zip::ZipArchive::new(file).map_err(|_| ServiceError::Response)?;
                let expected = std::iter::once(&release.executable)
                    .chain(release.harness.iter())
                    .collect::<Vec<_>>();
                if zip.len() != expected.len() {
                    return Err(ServiceError::Maintenance);
                }
                let mut seen = std::collections::BTreeSet::new();
                for index in 0..zip.len() {
                    let entry = zip.by_index(index).map_err(|_| ServiceError::Response)?;
                    let member = expected
                        .iter()
                        .find(|member| member.name == entry.name())
                        .filter(|member| {
                            safe_member(&member.name) && seen.insert(member.name.clone())
                        })
                        .ok_or(ServiceError::Maintenance)?;
                    if entry.encrypted()
                        || entry.is_dir()
                        || entry
                            .unix_mode()
                            .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
                        || !matches!(
                            entry.compression(),
                            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
                        )
                        || member.bytes != Some(entry.size())
                        || entry.size() > MAX_BYTES
                    {
                        return Err(ServiceError::Maintenance);
                    }
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(retained.path().join("runtime").join(&member.name))
                        .map_err(|_| ServiceError::Storage)?;
                    let count = std::io::copy(
                        &mut entry.take(member.bytes.unwrap_or(MAX_BYTES).saturating_add(1)),
                        &mut output,
                    )
                    .map_err(|_| ServiceError::Storage)?;
                    if member.bytes != Some(count) {
                        return Err(ServiceError::Maintenance);
                    }
                }
                Ok(())
            })
            .await
            .map_err(|_| ServiceError::Maintenance)??;
        }
        Format::TarGz => {
            let tar = crate::environment::archive_tar(platform, environment);
            let run = async |args: Vec<String>| {
                let mut options = process_options(&tar, &args, environment);
                options.timeout = Some(Duration::from_secs(120));
                options.max_output_bytes = 65536;
                t3_process::run(options)
                    .await
                    .map_err(|_| ServiceError::Maintenance)
            };
            let list = run(vec!["-tzf".into(), archive.to_string_lossy().into_owned()]).await?;
            let kinds = run(vec!["-tvzf".into(), archive.to_string_lossy().into_owned()]).await?;
            if list.stdout_text().trim() != release.executable.name
                || kinds.stdout_text().trim().lines().count() != 1
                || !kinds.stdout_text().starts_with('-')
            {
                return Err(ServiceError::Maintenance);
            }
            let mut options = process_options(
                &tar,
                &[
                    "-xOzf".into(),
                    archive.to_string_lossy().into_owned(),
                    release.executable.name.clone(),
                ],
                environment,
            );
            options.timeout = None;
            let mut child = t3_process::spawn_non_blocking(&options)
                .await
                .map_err(|_| ServiceError::Maintenance)?;
            let stdout = child.stdout.take().ok_or(ServiceError::Maintenance)?;
            let stderr = child.stderr.take();
            let drain = stderr.map(|mut stderr| {
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
                })
            });
            let mut output = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(&release.executable.name))
                .await
                .map_err(|_| ServiceError::Storage)?;
            use tokio::io::AsyncReadExt;
            let bytes = tokio::io::copy(&mut stdout.take(MAX_BYTES + 1), &mut output)
                .await
                .map_err(|_| ServiceError::Storage)?;
            if let Some(drain) = drain {
                drain.abort();
            }
            if bytes == 0
                || bytes > MAX_BYTES
                || !child
                    .wait()
                    .await
                    .map_err(|_| ServiceError::Maintenance)?
                    .success()
            {
                return Err(ServiceError::Maintenance);
            }
        }
    }
    for member in std::iter::once(&release.executable).chain(release.harness.iter()) {
        let path = root.join(&member.name);
        if !fs::metadata(&path).is_ok_and(|metadata| {
            metadata.is_file()
                && metadata.len() > 0
                && metadata.len() <= MAX_BYTES
                && member.bytes.is_none_or(|bytes| bytes == metadata.len())
        }) {
            return Err(ServiceError::Maintenance);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .map_err(|_| ServiceError::Storage)?;
        }
    }
    Ok(())
}
#[async_trait]
impl crate::installations::ManagedInstallation for AntigravityInstallation {
    async fn start(self: Arc<Self>) -> ServiceResult<ProviderInstallState> {
        AntigravityInstallation::start(&self).await
    }
    async fn cancel(&self, operation: &str) -> ServiceResult<ProviderInstallState> {
        AntigravityInstallation::cancel(self, operation).await
    }
    async fn remove(&self, protected: &[PathBuf]) -> ServiceResult<ProviderInstallState> {
        AntigravityInstallation::remove(self, protected).await
    }
    fn subscribe(&self) -> watch::Receiver<ProviderInstallState> {
        AntigravityInstallation::subscribe(self)
    }
    async fn check_for_update(&self) {
        AntigravityInstallation::check_for_update(self).await;
    }
}
