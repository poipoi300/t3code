use crate::{
    Error, Result,
    driver::{BrowserCommand, BrowserDriver, BrowserEvent, PlaywrightDriver, PlaywrightOptions},
    lock,
    manager::Events,
};
use async_trait::async_trait;
use futures_util::{
    FutureExt, StreamExt,
    future::{BoxFuture, Shared},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

pub const CHROME_VERSION: &str = "157.0.8103.78";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserRelease {
    pub version: String,
    pub platform: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}
pub fn pinned_release(platform: &str, arch: &str) -> Option<BrowserRelease> {
    let (platform, bytes, sha256) = match (platform, arch) {
        ("linux", "x86_64" | "x64") => (
            "linux64",
            127965683,
            "f90329720fb61e5a76da9b6b509b6ad12a484c9c5df0359403e935177c6d153a",
        ),
        ("linux", "aarch64" | "arm64") => (
            "linux-arm64",
            128047138,
            "912fbf877e53de34d7b3cca5104f3b95607d41bd35412ccc6cd2a599ff0c0dbf",
        ),
        ("macos" | "darwin", "aarch64" | "arm64") => (
            "mac-arm64",
            104822985,
            "17bf66d53c1b1104dcb641c5b331cd4ac5d200c778f5feed0934a0b92deaf8d6",
        ),
        ("macos" | "darwin", "x86_64" | "x64") => (
            "mac-x64",
            110132547,
            "f1a6a9561eaf1f409849538de663695d36d72fbf1ebca4d42e47c7b5f2a157f3",
        ),
        ("windows" | "win32", "x86" | "ia32") => (
            "win32",
            120403126,
            "d9829c59dcc79d9e6e920f8ab841f2a9a055dcf54229d756ac0b42dfec303910",
        ),
        ("windows" | "win32", "x86_64" | "x64" | "aarch64" | "arm64") => (
            "win64",
            126047001,
            "2138c1583d25d6f3abec5269943b2f430207d87a539895aa9e90cc4697ec0e1f",
        ),
        _ => return None,
    };
    Some(BrowserRelease {
        version: CHROME_VERSION.into(),
        platform: platform.into(),
        url: format!(
            "https://storage.googleapis.com/chrome-for-testing-public/{CHROME_VERSION}/{platform}/chrome-headless-shell-{platform}.zip"
        ),
        bytes,
        sha256: sha256.into(),
    })
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub unpacking: bool,
}
type Installing = Shared<BoxFuture<'static, Result<PathBuf>>>;
#[derive(Default)]
struct State {
    installing: Option<Installing>,
    failed: Option<Error>,
    progress: InstallProgress,
}
#[derive(Clone)]
pub struct PreviewBrowser {
    root: PathBuf,
    release: Option<BrowserRelease>,
    wait: Duration,
    state: Arc<Mutex<State>>,
    client: reqwest::Client,
}
impl PreviewBrowser {
    pub fn new(base_dir: PathBuf) -> Result<Self> {
        Self::with_release(
            base_dir,
            pinned_release(std::env::consts::OS, std::env::consts::ARCH),
            Duration::from_secs(45),
        )
    }
    pub fn with_release(
        base_dir: PathBuf,
        release: Option<BrowserRelease>,
        wait: Duration,
    ) -> Result<Self> {
        if let Some(release) = &release {
            for segment in [&release.platform, &release.version] {
                if segment.is_empty()
                    || segment == "."
                    || segment == ".."
                    || segment
                        .bytes()
                        .any(|b| !b.is_ascii_alphanumeric() && !b".-_".contains(&b))
                {
                    return Err(install_error("Unsafe release path"));
                }
            }
        }
        let root = base_dir.join("tools/chrome-headless-shell").join(
            release
                .as_ref()
                .map(|r| r.platform.as_str())
                .unwrap_or("unsupported"),
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15 * 60))
            .build()
            .map_err(|e| install_error(e.to_string()))?;
        Ok(Self {
            root,
            release,
            wait,
            state: Arc::default(),
            client,
        })
    }
    fn executable_name(&self) -> &'static str {
        if cfg!(windows) {
            "chrome-headless-shell.exe"
        } else {
            "chrome-headless-shell"
        }
    }
    pub fn installed(&self) -> Option<PathBuf> {
        let release = self.release.as_ref()?;
        let path = self
            .root
            .join(&release.version)
            .join(self.executable_name());
        let metadata = std::fs::metadata(&path).ok()?;
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
        Some(path)
    }
    pub fn progress(&self) -> InstallProgress {
        lock(&self.state).progress.clone()
    }
    pub async fn executable(&self) -> Result<PathBuf> {
        let release = self.release.clone().ok_or_else(|| {
            Error::operation(
                "PreviewBrowserUnsupportedError",
                format!(
                    "Chrome Headless Shell is unavailable on {}-{}",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            )
        })?;
        let installing = {
            let mut state = lock(&self.state);
            if let Some(job) = &state.installing {
                job.clone()
            } else {
                if let Some(error) = state.failed.take() {
                    return Err(error);
                }
                if let Some(path) = self.installed() {
                    return Ok(path);
                }
                state.progress = InstallProgress {
                    total_bytes: release.bytes,
                    ..InstallProgress::default()
                };
                let installer = self.clone();
                let handle = tokio::spawn(async move {
                    let result = installer.install(release).await;
                    let mut state = lock(&installer.state);
                    state.installing = None;
                    if let Err(error) = &result {
                        state.failed = Some(error.clone());
                    }
                    result
                });
                let job = async move { handle.await.map_err(|e| install_error(e.to_string()))? }
                    .boxed()
                    .shared();
                state.installing = Some(job.clone());
                job
            }
        };
        match tokio::time::timeout(self.wait, installing).await {
            Ok(result) => {
                if result.is_err() {
                    lock(&self.state).failed = None;
                }
                result
            }
            Err(_) => {
                let progress = self.progress();
                Err(Error::Operation {
                    tag: "PreviewBrowserInstallingError".into(),
                    message: format!(
                        "T3 is installing its headless browser ({} of {} bytes downloaded). Try again in a minute.",
                        progress.downloaded_bytes, progress.total_bytes
                    ),
                    detail: Some(serde_json::to_value(progress)?),
                })
            }
        }
    }
    async fn install(&self, release: BrowserRelease) -> Result<PathBuf> {
        tokio::fs::create_dir_all(&self.root).await?;
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&self.root)?;
        let archive = staging.path().join("download.zip");
        let response = self
            .client
            .get(&release.url)
            .send()
            .await
            .map_err(|e| install_error(e.to_string()))?
            .error_for_status()
            .map_err(|e| install_error(e.to_string()))?;
        if let Some(length) = response.content_length()
            && length != release.bytes
        {
            return Err(install_error(
                "The download size did not match the pinned release.",
            ));
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive)
            .await?;
        use tokio::io::AsyncWriteExt;
        let mut stream = response.bytes_stream();
        let mut size = 0u64;
        let mut hash = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| install_error(e.to_string()))?;
            size = size.saturating_add(chunk.len() as u64);
            if size > release.bytes {
                return Err(install_error(
                    "The download was larger than the pinned release.",
                ));
            }
            hash.update(&chunk);
            file.write_all(&chunk).await?;
            lock(&self.state).progress.downloaded_bytes = size;
        }
        file.flush().await?;
        drop(file);
        if size != release.bytes || format!("{:x}", hash.finalize()) != release.sha256 {
            return Err(install_error(
                "The download failed its size or SHA-256 check. Nothing was installed.",
            ));
        }
        lock(&self.state).progress.unpacking = true;
        let destination = staging.path().join("browser");
        let unpacked = destination.clone();
        let platform = release.platform.clone();
        let executable = self.executable_name().to_owned();
        tokio::task::spawn_blocking(move || {
            extract_archive(&archive, &unpacked, &platform, &executable)
        })
        .await
        .map_err(|e| install_error(e.to_string()))??;
        let published = self.root.join(&release.version);
        match tokio::fs::remove_dir_all(&published).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        tokio::fs::rename(destination, &published).await?;
        let root = self.root.clone();
        let current = release.version.clone();
        tokio::task::spawn_blocking(move || prune_versions(&root, &current))
            .await
            .map_err(|e| install_error(e.to_string()))??;
        Ok(published.join(self.executable_name()))
    }
}
fn install_error(message: impl Into<String>) -> Error {
    Error::operation(
        "PreviewBrowserInstallError",
        format!(
            "T3 could not install its headless browser: {} Try again.",
            message.into()
        ),
    )
}
pub fn extract_archive(
    archive: &Path,
    destination: &Path,
    platform: &str,
    executable: &str,
) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| install_error(e.to_string()))?;
    std::fs::create_dir(destination)?;
    let root = format!("chrome-headless-shell-{platform}/");
    let mut found = false;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| install_error(e.to_string()))?;
        let name = entry.name().to_owned();
        let relative = name
            .strip_prefix(&root)
            .ok_or_else(|| install_error("Archive entry is outside the browser directory."))?;
        let path = Path::new(relative);
        let mode = entry.unix_mode().unwrap_or(0);
        let kind = mode & 0o170000;
        if name.contains('\\')
            || relative.contains(':')
            || relative.split('/').any(|part| part == "..")
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
            || path.is_absolute()
            || kind != 0 && kind != if entry.is_dir() { 0o040000 } else { 0o100000 }
            || entry.encrypted()
            || !matches!(
                entry.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            )
        {
            return Err(install_error("Archive contains an unsafe entry."));
        }
        let target = destination.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        std::io::copy(&mut entry, &mut output).map_err(|e| install_error(e.to_string()))?;
        output.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if mode & 0o777 != 0 {
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o777))?;
            }
        }
        if relative == executable {
            found = true;
        }
    }
    if !found {
        return Err(install_error(
            "Archive does not contain chrome-headless-shell.",
        ));
    }
    Ok(())
}
fn version_key(name: &str) -> Vec<u64> {
    name.split('.').map(|s| s.parse().unwrap_or(0)).collect()
}
fn prune_versions(root: &Path, current: &str) -> Result<()> {
    let entries = std::fs::read_dir(root)?
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();
    let previous = entries
        .iter()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name != current && !name.starts_with('.'))
        .max_by_key(|name| version_key(name));
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == current || previous.as_ref() == Some(&name) {
            continue;
        }
        if name.starts_with(".install-")
            && entry.metadata()?.modified()?.elapsed().unwrap_or_default()
                < Duration::from_secs(3600)
        {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(entry.path()) {
            tracing::warn!(%error,%name,"Could not remove an old preview browser");
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostDiagnosis {
    Root,
    Sandbox {
        setup_command: String,
    },
    Libraries {
        setup_command: String,
        libraries: Vec<String>,
    },
}
impl HostDiagnosis {
    pub fn into_error(self) -> Error {
        match self {
            Self::Root => Error::operation(
                "PreviewBrowserRootError",
                "T3 server is running as root; Chrome's sandbox cannot start. Run T3 as a nonroot user. Otherwise set T3CODE_SERVER_BROWSER_SANDBOX=0 in the server environment and restart; this disables Chrome's sandbox. t3 browser setup cannot fix root sandboxing.",
            ),
            Self::Sandbox { setup_command } => Error::Operation {
                tag: "PreviewBrowserSandboxError".into(),
                message: format!(
                    "This host blocks Chrome's sandbox. Run `{setup_command}` on the host once, then try again."
                ),
                detail: Some(serde_json::json!({"setupCommand":setup_command})),
            },
            Self::Libraries {
                setup_command,
                libraries,
            } => Error::Operation {
                tag: "PreviewBrowserLibrariesError".into(),
                message: format!(
                    "This host is missing libraries Chrome needs ({}). Run `{setup_command}` on the host to install them.",
                    libraries.join(", ")
                ),
                detail: Some(
                    serde_json::json!({"setupCommand":setup_command,"libraries":libraries}),
                ),
            },
        }
    }
}
pub async fn diagnose_launch_failure(
    executable: &Path,
    output: &str,
    setup_command: &str,
) -> Option<HostDiagnosis> {
    if output.contains("No usable sandbox") || output.to_lowercase().contains("sandboxing failed") {
        #[cfg(unix)]
        if nix::unistd::getuid().is_root() {
            return Some(HostDiagnosis::Root);
        }
        return Some(HostDiagnosis::Sandbox {
            setup_command: setup_command.into(),
        });
    }
    if cfg!(target_os = "linux") {
        let mut options = t3_process::ProcessOptions::new(executable.as_os_str());
        options.args = vec!["--version".into()];
        options.timeout = Some(Duration::from_secs(5));
        options.check_exit = false;
        let output = t3_process::run(options).await.ok()?;
        let text = output.stdout_text() + &output.stderr_text();
        let loader = regex::Regex::new(r"error while loading shared libraries: ([^:\s]+)")
            .expect("loader regex")
            .captures(&text)?
            .get(1)?
            .as_str()
            .to_owned();
        let mut options = t3_process::ProcessOptions::new("ldd").args([executable.as_os_str()]);
        options.timeout = Some(Duration::from_secs(5));
        options.check_exit = false;
        let text = t3_process::run(options)
            .await
            .map(|o| o.stdout_text())
            .unwrap_or_default();
        let mut libraries: Vec<_> = text
            .lines()
            .filter_map(|l| l.trim().strip_suffix(" => not found").map(str::to_owned))
            .collect();
        if libraries.is_empty() {
            libraries.push(loader);
        }
        return Some(HostDiagnosis::Libraries {
            setup_command: setup_command.into(),
            libraries,
        });
    }
    None
}

/// No worker owns a context, so profile cleanup only touches its encoded directory.
async fn clear_inactive_profile(root: &Path, profile_id: &str) -> Result<()> {
    crate::profiles::validate_profile_id(profile_id)?;
    if profile_id != "incognito" {
        match tokio::fs::remove_dir_all(crate::urls::profile_directory(root, profile_id)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) struct UnavailableBrowserDriver {
    pub profiles: PathBuf,
}
#[async_trait]
impl BrowserDriver for UnavailableBrowserDriver {
    async fn call(&self, command: BrowserCommand, _: Duration) -> Result<Value> {
        match command {
            BrowserCommand::ClearProfile { profile_id } => {
                clear_inactive_profile(&self.profiles, &profile_id).await?
            }
            BrowserCommand::Shutdown
            | BrowserCommand::Close { .. }
            | BrowserCommand::DiscardPopup { .. } => {}
            _ => {
                return Err(Error::operation(
                    "PreviewAutomationExecutionError",
                    "Server browser automation requires Node.js and playwright-core.",
                ));
            }
        }
        Ok(Value::Null)
    }
    fn subscribe(&self) -> mpsc::UnboundedReceiver<BrowserEvent> {
        mpsc::unbounded_channel().1
    }
}

pub struct ManagedBrowserDriver {
    node_environment: t3_process::Environment,
    generation: std::sync::atomic::AtomicU64,
    installer: PreviewBrowser,
    options: PlaywrightOptions,
    driver: tokio::sync::Mutex<Option<Arc<PlaywrightDriver>>>,
    events: Events<BrowserEvent>,
    setup_command: String,
}
impl ManagedBrowserDriver {
    pub fn new(installer: PreviewBrowser, options: PlaywrightOptions) -> Self {
        let base = installer
            .root
            .ancestors()
            .nth(3)
            .unwrap_or(&installer.root)
            .to_string_lossy();
        let quoted = if cfg!(windows) {
            format!("'{}'", base.replace('\'', "''"))
        } else {
            format!("'{}'", base.replace('\'', "'\\''"))
        };
        let setup_command = format!("t3 browser setup --base-dir {quoted}");
        Self {
            installer,
            node_environment: t3_process::Environment::new(),
            generation: std::sync::atomic::AtomicU64::new(0),
            options,
            driver: tokio::sync::Mutex::new(None),
            events: Events::default(),
            setup_command,
        }
    }
    pub fn with_node_environment(mut self, environment: t3_process::Environment) -> Self {
        self.node_environment = environment;
        self
    }
    pub fn with_setup_command(mut self, command: String) -> Self {
        self.setup_command = command;
        self
    }
}
impl ManagedBrowserDriver {
    async fn worker(&self) -> Result<Arc<PlaywrightDriver>> {
        let mut driver = self.driver.lock().await;
        if driver.as_ref().is_some_and(|driver| !driver.is_connected()) {
            *driver = None;
        }
        if driver.is_none() {
            let mut options = self.options.clone();
            if let Some(executable) = self.installer.installed() {
                options.executable = executable;
            }
            let child = Arc::new(
                PlaywrightDriver::start_with_environment(options, self.node_environment.clone())
                    .await?,
            );
            self.generation
                .store(child.generation(), std::sync::atomic::Ordering::Release);
            let mut events = child.subscribe();
            let outgoing = self.events.clone();
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    outgoing.publish(event);
                }
            });
            *driver = Some(child);
        }
        Ok(driver.as_ref().expect("driver initialized").clone())
    }
    async fn call_inner(&self, command: BrowserCommand, timeout: Duration) -> Result<Value> {
        if matches!(command, BrowserCommand::HtmlMeasure { .. })
            && self.installer.installed().is_none()
        {
            return Ok(Value::Null);
        }
        {
            let mut driver = self.driver.lock().await;
            if driver.as_ref().is_some_and(|driver| !driver.is_connected()) {
                *driver = None;
            }
            if driver.is_none() {
                match &command {
                    BrowserCommand::ClearProfile { profile_id } => {
                        clear_inactive_profile(&self.options.profiles_dir, profile_id).await?;
                        return Ok(Value::Null);
                    }
                    BrowserCommand::Shutdown
                    | BrowserCommand::Close { .. }
                    | BrowserCommand::DiscardPopup { .. } => return Ok(Value::Null),
                    _ => {}
                }
            }
        }
        self.dispatch_on(self.worker().await?, command, timeout)
            .await
    }
    async fn dispatch_on(
        &self,
        driver: Arc<PlaywrightDriver>,
        command: BrowserCommand,
        timeout: Duration,
    ) -> Result<Value> {
        let needs_executable = matches!(
            &command,
            BrowserCommand::Open {
                desktop_endpoint: None,
                popup_id: None,
                ..
            } | BrowserCommand::HtmlCapture { .. }
                | BrowserCommand::HtmlMeasure { .. }
        );
        if needs_executable {
            let path = self.installer.executable().await?;
            driver
                .call(BrowserCommand::Executable { path }, Duration::from_secs(2))
                .await?;
        }
        let launches = matches!(
            command,
            BrowserCommand::Open { .. }
                | BrowserCommand::HtmlCapture { .. }
                | BrowserCommand::HtmlMeasure { .. }
        );
        let result = driver.call(command, timeout).await;
        if launches
            && let Err(error) = &result
            && let Some(executable) = self.installer.installed()
            && let Some(diagnosis) =
                diagnose_launch_failure(&executable, &error.to_string(), &self.setup_command).await
        {
            return Err(diagnosis.into_error());
        }
        result
    }
}
#[async_trait]
impl BrowserDriver for ManagedBrowserDriver {
    fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }
    async fn prepare(&self) -> Result<u64> {
        Ok(self.worker().await?.generation())
    }
    async fn call_on(
        &self,
        generation: u64,
        command: BrowserCommand,
        timeout: Duration,
    ) -> Result<Value> {
        let driver = self
            .driver
            .lock()
            .await
            .as_ref()
            .filter(|driver| driver.generation() == generation && driver.is_connected())
            .cloned()
            .ok_or(Error::Disconnected)?;
        self.dispatch_on(driver, command, timeout).await
    }
    async fn call(&self, command: BrowserCommand, timeout: Duration) -> Result<Value> {
        // Installation has its own shared wait budget; the driver bounds the browser command.
        self.call_inner(command, timeout).await
    }
    fn subscribe(&self) -> mpsc::UnboundedReceiver<BrowserEvent> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    #[tokio::test]
    async fn queued_command_cannot_switch_to_the_replacement_worker() {
        let root = tempfile::tempdir().expect("tempdir");
        let options = PlaywrightOptions {
            node: "node".into(),
            cwd: root.path().into(),
            executable: root.path().join("never-run-chrome"),
            profiles_dir: root.path().join("profiles"),
            downloads_dir: root.path().join("downloads"),
            attachments_dir: root.path().join("attachments"),
            playwright_module: None,
            sandbox: true,
        };
        let managed = ManagedBrowserDriver::new(
            PreviewBrowser::with_release(root.path().into(), None, Duration::from_secs(1))
                .expect("installer"),
            options.clone(),
        );
        let original = managed.prepare().await.expect("original worker");
        let mut state = managed.driver.lock().await;
        let mut command = Box::pin(managed.call_on(
            original,
            BrowserCommand::Executable {
                path: root.path().join("stale-command"),
            },
            Duration::from_secs(5),
        ));
        assert!(futures_util::poll!(command.as_mut()).is_pending());
        let replacement = Arc::new(
            PlaywrightDriver::start(options)
                .await
                .expect("replacement worker"),
        );
        managed.generation.store(
            replacement.generation(),
            std::sync::atomic::Ordering::Release,
        );
        let previous = state.replace(replacement);
        drop(previous);
        drop(state);
        assert!(matches!(command.await, Err(Error::Disconnected)));
        managed
            .call(BrowserCommand::Shutdown, Duration::from_secs(5))
            .await
            .expect("shutdown");
    }
}
