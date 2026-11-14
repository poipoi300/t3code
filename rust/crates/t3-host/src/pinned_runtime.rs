use crate::{
    self_update::{RuntimeRequest, UpdateResult, update_error},
    service_launcher::is_exact_service_version,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Mutex;
static INSTALL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const ARCHIVE_LIMIT: usize = 512 * 1024 * 1024;
pub struct ReleaseResponse {
    pub status: u16,
    pub body: Vec<u8>,
}
#[async_trait]
pub trait RuntimeInstallHost: Send + Sync {
    /// No credentials, URLs or response bodies in errors. Bound each header/body stall to 60s.
    async fn fetch(&self, url: &str, max_bytes: usize) -> UpdateResult<ReleaseResponse>;
    /// Run via t3-process, reject nonzero, timed out, truncated and invalid UTF-8 output.
    async fn preflight(&self, options: t3_process::ProcessOptions) -> UpdateResult<Vec<u8>>;
}
pub struct NativeRuntimeInstallHost {
    client: reqwest::Client,
}
impl Default for NativeRuntimeInstallHost {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}
#[async_trait]
impl RuntimeInstallHost for NativeRuntimeInstallHost {
    async fn fetch(&self, url: &str, max_bytes: usize) -> UpdateResult<ReleaseResponse> {
        use futures_util::StreamExt;
        let response = tokio::time::timeout(Duration::from_secs(60), self.client.get(url).send())
            .await
            .map_err(|_| update_error("Release download timed out."))?
            .map_err(|_| update_error("Release download failed."))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Ok(ReleaseResponse {
                status,
                body: vec![],
            });
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(60), stream.next())
                .await
                .map_err(|_| update_error("Release download timed out."))?;
            let Some(chunk) = next else {
                break;
            };
            let chunk = chunk.map_err(|_| update_error("Release download failed."))?;
            if body.len().saturating_add(chunk.len()) > max_bytes {
                return Err(update_error("Release asset exceeds the size limit."));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(ReleaseResponse { status, body })
    }
    async fn preflight(&self, options: t3_process::ProcessOptions) -> UpdateResult<Vec<u8>> {
        let output = t3_process::run(options)
            .await
            .map_err(|_| update_error("Could not run the staged service preflight."))?;
        if output.code != Some(0)
            || output.timed_out
            || output.stdout_truncated
            || output.stdout_invalid_utf8
        {
            return Err(update_error(
                "The staged service preflight did not complete.",
            ));
        }
        Ok(output.stdout)
    }
}
pub struct PinnedRuntimeInstaller {
    base_dir: PathBuf,
    platform_key: String,
    host: Arc<dyn RuntimeInstallHost>,
}
impl PinnedRuntimeInstaller {
    pub fn new(base_dir: PathBuf, platform_key: String, host: Arc<dyn RuntimeInstallHost>) -> Self {
        Self {
            base_dir,
            platform_key,
            host,
        }
    }
    pub fn native(base_dir: PathBuf) -> Self {
        let os = if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        };
        Self::new(
            base_dir,
            format!("{os}-{arch}"),
            Arc::new(NativeRuntimeInstallHost::default()),
        )
    }
    pub async fn ensure(&self, request: RuntimeRequest) -> UpdateResult<PathBuf> {
        if !is_exact_service_version(&request.target_version)
            || !matches!(request.launcher_protocol, 3 | 4)
        {
            return Err(update_error("Invalid pinned runtime request."));
        }
        if ![
            "win32-x64",
            "win32-arm64",
            "linux-x64",
            "linux-arm64",
            "darwin-arm64",
        ]
        .contains(&self.platform_key.as_str())
        {
            return Err(update_error("No t3 release archive supports this machine."));
        }
        let _lock = INSTALL_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        let root = self.base_dir.join("runtime/versions");
        fs::create_dir_all(&root)
            .map_err(|_| update_error("Could not prepare the pinned runtime directory."))?;
        let destination = root.join(&request.target_version);
        let entry_name = if self.platform_key.starts_with("win32-") {
            "t3.exe"
        } else {
            "t3"
        };
        let entry = destination.join(entry_name);
        let sentinel = destination.join(".install-complete");
        let destination_exists = match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                let real_parent = dunce::canonicalize(&root)
                    .map_err(|_| update_error("Could not inspect the pinned runtime directory."))?;
                if metadata.file_type().is_symlink()
                    || dunce::canonicalize(&destination).ok()
                        != Some(real_parent.join(&request.target_version))
                {
                    return Err(update_error("Pinned runtime directory is a symbolic link."));
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => {
                return Err(update_error(
                    "Could not inspect the pinned runtime directory.",
                ));
            }
        };
        let cached = entry.is_file()
            && fs::read_to_string(&sentinel).is_ok_and(|s| s.trim() == request.target_version);
        if destination_exists && !cached {
            if !direct_directory(&root, &destination) {
                return Err(update_error(
                    "Pinned runtime directory changed during inspection.",
                ));
            }
            remove_runtime_directory(root.clone(), destination.clone()).await?;
        }
        let archive_name = format!(
            "t3-{}-{}.{}",
            request.target_version,
            self.platform_key,
            if self.platform_key.starts_with("win32-") {
                "zip"
            } else {
                "tar.gz"
            }
        );
        let checksum = if !cached || request.require_published_release {
            let raw = self
                .asset(&request, "SHA256SUMS", "checksums", 1024 * 1024)
                .await?;
            let checksums = String::from_utf8(raw)
                .map_err(|_| update_error("Release checksums are invalid."))?;
            let expected = checksums
                .lines()
                .filter_map(|l| {
                    let mut p = l.splitn(2, char::is_whitespace);
                    Some((p.next()?, p.next()?.trim_start().trim_start_matches('*')))
                })
                .find(|(_, name)| *name == archive_name)
                .map(|(hash, _)| hash.to_ascii_lowercase())
                .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
            Some(expected.ok_or_else(|| unpublished(&request))?)
        } else {
            None
        };
        if cached {
            self.validate(&entry, &request).await?;
            return Ok(entry);
        }
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&root)
            .map_err(|_| update_error("Could not prepare the pinned runtime directory."))?;
        let archive = self
            .asset(&request, &archive_name, "archive", ARCHIVE_LIMIT)
            .await?;
        if hex::encode(Sha256::digest(&archive)) != checksum.unwrap_or_default() {
            return Err(update_error(
                "Could not verify the t3 release archive checksum.",
            ));
        }
        let extract_path = staging.path().to_owned();
        let is_zip = self.platform_key.starts_with("win32-");
        tokio::time::timeout(
            Duration::from_secs(600),
            tokio::task::spawn_blocking(move || extract_archive(&archive, &extract_path, is_zip)),
        )
        .await
        .map_err(|_| update_error("Release extraction timed out."))?
        .map_err(|_| update_error("Release extraction stopped unexpectedly."))??;
        let staged_entry = staging.path().join(entry_name);
        self.validate(&staged_entry, &request).await?;
        fs::write(
            staging.path().join(".install-complete"),
            format!("{}\n", request.target_version),
        )
        .map_err(|_| update_error("Could not record the completed install."))?;
        if fs::rename(staging.path(), &destination).is_err() {
            if !entry.is_file()
                || !fs::read_to_string(&sentinel).is_ok_and(|s| s.trim() == request.target_version)
            {
                return Err(update_error("Could not publish the pinned runtime."));
            }
            self.validate(&entry, &request).await?;
        }
        Ok(entry)
    }
    async fn asset(
        &self,
        request: &RuntimeRequest,
        name: &str,
        step: &str,
        limit: usize,
    ) -> UpdateResult<Vec<u8>> {
        let source = request
            .release_base_url
            .clone()
            .or_else(|| std::env::var("T3CODE_RELEASE_BASE_URL").ok())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://github.com/pingdotgg/t3code/releases/download".into());
        let mut url = reqwest::Url::parse(source.trim())
            .map_err(|_| update_error("Invalid release source."))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(update_error("Invalid release source."));
        }
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| update_error("Invalid release source."))?;
            segments
                .pop_if_empty()
                .push(&format!("v{}", request.target_version))
                .push(name);
        }
        for attempt in 0..4 {
            let response = self.host.fetch(url.as_str(), limit).await?;
            if (200..300).contains(&response.status) {
                if response.body.len() > limit {
                    return Err(update_error("Release asset exceeds the size limit."));
                }
                return Ok(response.body);
            }
            if response.status == 404 && request.mirror_organization.is_some() {
                return Err(unpublished(request));
            }
            if attempt < 3 && (response.status >= 500 || matches!(response.status, 408 | 429)) {
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                continue;
            }
            return Err(update_error(format!(
                "Could not prepare t3@{} while downloading the t3 release {step} (HTTP {} from {}).",
                request.target_version,
                response.status,
                url.host_str().unwrap_or("release server")
            )));
        }
        Err(update_error("Release download failed."))
    }
    async fn validate(&self, entry: &Path, request: &RuntimeRequest) -> UpdateResult<()> {
        if !entry.is_file() {
            return Err(update_error("The staged service runtime is missing."));
        }
        let protocol = request.launcher_protocol.to_string();
        let mut options = t3_process::ProcessOptions::new(entry.as_os_str()).args([
            "__service-preflight",
            "--database-path",
            &request.database_path.to_string_lossy(),
            "--launcher-protocol",
            &protocol,
        ]);
        options.timeout = Some(Duration::from_secs(180));
        options.max_output_bytes = 1024 * 1024;
        let bytes = tokio::time::timeout(Duration::from_secs(180), self.host.preflight(options))
            .await
            .map_err(|_| update_error("The staged service preflight timed out."))??;
        if bytes.len() > 1024 * 1024 {
            return Err(update_error(
                "The staged service preflight output is too large.",
            ));
        }
        let result: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| update_error("Could not decode the staged service preflight."))?;
        if result["version"] != request.target_version {
            return Err(update_error(
                "The staged service preflight version does not match.",
            ));
        }
        if result["status"] == "blocked"
            && let Some(reason) = result["reason"].as_str()
        {
            return Err(update_error(reason));
        }
        if result["status"] != "ready"
            || result["launcherProtocol"].as_u64() != Some(request.launcher_protocol.into())
        {
            return Err(update_error(
                "The staged service preflight protocol does not match.",
            ));
        }
        Ok(())
    }
}
fn unpublished(request: &RuntimeRequest) -> ServerError {
    update_error(if let Some(organization) = &request.mirror_organization {
        format!(
            "{organization}'s update mirror has no t3@{} for this machine.",
            request.target_version
        )
    } else {
        format!(
            "The t3 release checksums do not list this machine's archive for t3@{}.",
            request.target_version
        )
    })
}
type ServerError = t3_contracts::server::ServerSelfUpdateError;
fn stripped_path(name: &Path) -> UpdateResult<Option<PathBuf>> {
    if name.is_absolute() {
        return Err(update_error("Release archive contains an unsafe path."));
    }
    let components = name.components().collect::<Vec<_>>();
    if components
        .iter()
        .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(update_error("Release archive contains an unsafe path."));
    }
    let relative = components
        .iter()
        .skip(1)
        .map(|c| c.as_os_str())
        .collect::<PathBuf>();
    if relative
        .components()
        .any(|c| c.as_os_str().to_string_lossy().contains([':', '\\']))
    {
        return Err(update_error("Release archive contains an unsafe path."));
    }
    Ok((!relative.as_os_str().is_empty()).then_some(relative))
}
fn extract_archive(bytes: &[u8], destination: &Path, is_zip: bool) -> UpdateResult<()> {
    let failure = || update_error("Could not extract the t3 release archive.");
    let mut total = 0u64;
    if is_zip {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| failure())?;
        if archive.len() > 100_000 {
            return Err(failure());
        }
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|_| failure())?;
            if entry.is_symlink() {
                return Err(update_error("Release archive contains a symbolic link."));
            }
            let name = entry.name().map_err(|_| failure())?.into_owned();
            let Some(relative) = stripped_path(Path::new(&name))? else {
                continue;
            };
            let path = destination.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(path).map_err(|_| failure())?;
                continue;
            }
            total = total.saturating_add(entry.size());
            if total > 1024 * 1024 * 1024 {
                return Err(update_error("Release extraction exceeds the size limit."));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|_| failure())?;
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|_| failure())?;
            let size = entry.size();
            std::io::copy(
                &mut entry.by_ref().take(size.saturating_add(1)),
                &mut output,
            )
            .map_err(|_| failure())?;
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        for (index, entry) in archive.entries().map_err(|_| failure())?.enumerate() {
            if index >= 100_000 {
                return Err(failure());
            }
            let mut entry = entry.map_err(|_| failure())?;
            let Some(relative) = stripped_path(&entry.path().map_err(|_| failure())?)? else {
                continue;
            };
            let path = destination.join(relative);
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                fs::create_dir_all(path).map_err(|_| failure())?;
                continue;
            }
            if !kind.is_file() {
                return Err(update_error(
                    "Release archive contains an unsupported entry.",
                ));
            }
            total = total.saturating_add(entry.size());
            if total > 1024 * 1024 * 1024 {
                return Err(update_error("Release extraction exceeds the size limit."));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|_| failure())?;
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| failure())?;
            std::io::copy(&mut entry, &mut output).map_err(|_| failure())?;
            output.flush().map_err(|_| failure())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    path,
                    fs::Permissions::from_mode(
                        entry.header().mode().map_err(|_| failure())? & 0o777,
                    ),
                )
                .map_err(|_| failure())?;
            }
        }
    }
    Ok(())
}

#[async_trait]
pub trait RuntimeReferences: Send + Sync {
    /// Resolved executable paths (Windows command lines); unavailable, incomplete or
    /// empty process enumeration MUST fail so no runtime is selected for removal.
    async fn running_executables(&self) -> crate::Result<String>;
}
pub struct NativeRuntimeReferences;
#[async_trait]
impl RuntimeReferences for NativeRuntimeReferences {
    async fn running_executables(&self) -> crate::Result<String> {
        if cfg!(target_os = "linux") {
            let mut paths = Vec::new();
            for entry in fs::read_dir("/proc")
                .map_err(|_| crate::Error::Io)?
                .flatten()
            {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .bytes()
                    .all(|c| c.is_ascii_digit())
                    && let Ok(path) = fs::read_link(entry.path().join("exe"))
                {
                    paths.push(path.to_string_lossy().into_owned());
                }
            }
            if paths.is_empty() {
                return Err(crate::Error::Service);
            }
            return Ok(paths.join("\n"));
        }
        let mut options = if cfg!(windows) {
            t3_process::ProcessOptions::new("powershell.exe").args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_Process | Select-Object -ExpandProperty CommandLine",
            ])
        } else {
            t3_process::ProcessOptions::new("lsof").args(["-nP", "-w", "-d", "txt", "-F", "n"])
        };
        options.timeout = Some(Duration::from_secs(10));
        options.max_output_bytes = 16 * 1024 * 1024;
        let output = t3_process::run(options)
            .await
            .map_err(|_| crate::Error::Service)?;
        if output.code != Some(0)
            || output.stdout_truncated
            || output.stdout_invalid_utf8
            || output.stdout.is_empty()
        {
            return Err(crate::Error::Service);
        }
        Ok(output.stdout_text())
    }
}
pub struct LauncherSearch {
    pub windows: bool,
    pub home: PathBuf,
    pub environment: std::collections::BTreeMap<String, String>,
}
impl LauncherSearch {
    pub fn native(home: PathBuf) -> Self {
        Self {
            windows: cfg!(windows),
            home,
            environment: std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect(),
        }
    }
}
fn direct_directory(root: &Path, path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        && path.parent() == Some(root)
        && dunce::canonicalize(path).ok()
            == dunce::canonicalize(root)
                .ok()
                .and_then(|root| Some(root.join(path.file_name()?)))
}
fn launcher_targets(base: &Path, search: &LauncherSearch) -> Vec<String> {
    use std::collections::BTreeSet;
    let records = base.join("runtime/launchers");
    let recorded = fs::read_to_string(&records).unwrap_or_default();
    let mut launchers = recorded
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .collect::<BTreeSet<_>>();
    let bin = search
        .environment
        .get("T3CODE_INSTALL_BIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| search.home.join(".local/bin"));
    let name = if search.windows { "t3.cmd" } else { "t3" };
    launchers.insert(bin.join(name));
    if let Some(path) = search
        .environment
        .get("PATH")
        .or_else(|| search.environment.get("Path"))
    {
        for dir in path.split(if search.windows { ';' } else { ':' }) {
            let dir = Path::new(dir);
            if dir.is_absolute() {
                launchers.insert(dir.join(name));
            }
        }
    }
    let root = base.join("runtime/versions");
    let real_root = dunce::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let mut targets = Vec::new();
    let mut unrecorded = Vec::new();
    for launcher in launchers {
        let target = if search.windows {
            fs::read_to_string(&launcher).ok().and_then(|s| {
                s.lines().find_map(|l| {
                    l.strip_prefix('"')
                        .and_then(|s| s.split_once('"'))
                        .map(|(path, _)| PathBuf::from(path))
                })
            })
        } else {
            dunce::canonicalize(&launcher).ok()
        };
        let Some(target) = target else {
            continue;
        };
        if !search.windows
            && launcher
                .parent()
                .and_then(|p| p.parent())
                .is_some_and(|p| p == root || p == real_root)
            && dunce::canonicalize(&launcher).ok().as_ref() == Some(&target)
            && fs::read_link(&launcher).is_err()
        {
            continue;
        }
        targets.push(target.to_string_lossy().into_owned());
        if (target.starts_with(&root) || target.starts_with(&real_root))
            && !recorded.lines().any(|l| Path::new(l) == launcher)
            && !launcher.to_string_lossy().contains(['\r', '\n'])
        {
            unrecorded.push(launcher);
        }
    }
    if !unrecorded.is_empty()
        && let Ok(mut file) = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(records)
    {
        let _ = writeln!(file);
        for path in unrecorded {
            let _ = writeln!(file, "{}", path.display());
        }
    }
    targets
}
pub async fn prune_pinned_runtimes(
    base: &Path,
    keep: &[String],
    references: &[String],
    now_ms: i64,
    processes: &dyn RuntimeReferences,
    search: &LauncherSearch,
) -> UpdateResult<Vec<String>> {
    use std::collections::BTreeSet;
    let lock = INSTALL_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let root = base.join("runtime/versions");
    let Ok(entries) = fs::read_dir(&root) else {
        return Ok(vec![]);
    };
    let mut pruned = BTreeSet::new();
    let mut completed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if !direct_directory(&root, &path) {
            continue;
        }
        if let Some(version) = name.strip_prefix(".pruned-")
            && is_exact_service_version(version)
        {
            pruned.insert(path);
            continue;
        }
        if !is_exact_service_version(&name) {
            continue;
        }
        let sentinel = path.join(".install-complete");
        let old = fs::metadata(&sentinel)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|t| {
                (t.as_millis() as i128) < i128::from(now_ms.saturating_sub(3_600_000))
            });
        if old && fs::read_to_string(sentinel).is_ok_and(|s| s.trim() == name) {
            completed.push(name);
        }
    }
    let mut keep = keep.iter().cloned().collect::<BTreeSet<_>>();
    let can_select = match fs::read_to_string(base.join("runtime/service-state.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
        Ok(raw) => {
            if let Ok(state) = crate::service_launcher::parse_service_state(&raw) {
                keep.insert(state.active_version);
                if let Some(update) = state.update
                    && let Ok(value) = t3_contracts::stored::to_value(&update)
                {
                    for key in ["fromVersion", "targetVersion"] {
                        if let Some(version) = value[key].as_str() {
                            keep.insert(version.into());
                        }
                    }
                }
                true
            } else {
                false
            }
        }
    };
    if can_select
        && !completed.is_empty()
        && let Ok(executables) =
            tokio::time::timeout(Duration::from_secs(10), processes.running_executables()).await
        && let Ok(executables) = executables
        && !executables.trim().is_empty()
    {
        let fold = |s: String| if search.windows { s.to_lowercase() } else { s };
        let refs = std::iter::once(executables)
            .chain(launcher_targets(base, search))
            .chain(references.iter().cloned())
            .map(fold)
            .collect::<Vec<_>>();
        let real_root = dunce::canonicalize(&root).unwrap_or_else(|_| root.clone());
        for version in completed {
            if keep.contains(&version) {
                continue;
            }
            let source = root.join(&version);
            let prefixes = [source.clone(), real_root.join(&version)]
                .map(|p| fold(format!("{}{}", p.display(), std::path::MAIN_SEPARATOR)));
            if refs.iter().any(|r| prefixes.iter().any(|p| r.contains(p))) {
                continue;
            }
            let target = root.join(format!(".pruned-{version}"));
            if target.exists()
                && (!direct_directory(&root, &target)
                    || remove_runtime_directory(root.clone(), target.clone())
                        .await
                        .is_err())
            {
                continue;
            }
            if direct_directory(&root, &source) && fs::rename(source, &target).is_ok() {
                pruned.insert(target);
            }
        }
    }
    drop(lock);
    let mut removed = Vec::new();
    for target in pruned {
        if direct_directory(&root, &target)
            && remove_runtime_directory(root.clone(), target.clone())
                .await
                .is_ok()
            && let Some(version) = target
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(".pruned-"))
        {
            removed.push(version.into());
        }
    }
    Ok(removed)
}
impl PinnedRuntimeInstaller {
    pub async fn prune_on_start(
        &self,
        startup: RuntimeStartup<'_>,
        now_ms: i64,
        processes: &dyn RuntimeReferences,
        search: &LauncherSearch,
    ) -> UpdateResult<Vec<String>> {
        let RuntimeStartup {
            current_version,
            managed,
            executable,
            unit_program,
        } = startup;
        let entry = self
            .base_dir
            .join("runtime/versions")
            .join(current_version)
            .join(if self.platform_key.starts_with("win32-") {
                "t3.exe"
            } else {
                "t3"
            });
        if !managed
            && entry != executable
            && dunce::canonicalize(&entry).ok().as_deref() != Some(executable)
        {
            return Ok(vec![]);
        }
        let references = unit_program
            .into_iter()
            .flat_map(|p| {
                [
                    p.to_owned(),
                    dunce::canonicalize(p).unwrap_or_else(|_| p.to_owned()),
                ]
            })
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        prune_pinned_runtimes(
            &self.base_dir,
            &[current_version.into()],
            &references,
            now_ms,
            processes,
            search,
        )
        .await
    }
}

pub struct RuntimeStartup<'a> {
    pub current_version: &'a str,
    pub managed: bool,
    pub executable: &'a Path,
    pub unit_program: Option<&'a Path>,
}

async fn remove_runtime_directory(root: PathBuf, path: PathBuf) -> UpdateResult<()> {
    tokio::task::spawn_blocking(move || {
        if !direct_directory(&root, &path) {
            return Err(update_error("Runtime directory changed during inspection."));
        }
        fs::remove_dir_all(path).map_err(|_| update_error("Could not remove the unused runtime."))
    })
    .await
    .map_err(|_| update_error("Runtime removal stopped unexpectedly."))?
}
