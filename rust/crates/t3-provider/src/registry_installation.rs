use crate::{
    acp_service::{AcpError, AcpResult},
    environment::{Environment, Platform, process_options},
    registry::{self, Agent, BinaryTarget},
    sensitive::Sensitive,
    transport::ProviderClock,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use t3_contracts::acp_registry::AcpRegistryOperationErrorReason as Reason;
use tokio::io::AsyncWriteExt;
const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 10_000;
fn archive_entry_count(listing: &str) -> usize {
    listing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}
fn validate_tar_metadata(listing: &str, count: usize) -> AcpResult<()> {
    let mut total = 0u64;
    let mut entries = 0usize;
    for line in listing.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if !matches!(line.as_bytes().first(), Some(b'-' | b'd')) {
            return Err(archive_error());
        }
        // GNU prints owner/group together; BSD prints link count, owner and group.
        let size_index = if fields.get(1).is_some_and(|owner| owner.contains('/')) {
            2
        } else if fields
            .get(1)
            .is_some_and(|links| links.parse::<u64>().is_ok())
        {
            4
        } else {
            return Err(archive_error());
        };
        let size = fields
            .get(size_index)
            .and_then(|size| size.parse::<u64>().ok())
            .ok_or_else(archive_error)?;
        total = total
            .checked_add(size)
            .filter(|size| *size <= MAX_EXPANDED_BYTES)
            .ok_or_else(archive_error)?;
        entries += 1;
    }
    if entries != count {
        return Err(archive_error());
    }
    Ok(())
}
fn archive_cause<E: 'static>(cause: E) -> AcpError {
    crate::diagnostics::failure(
        "RegistryBinaryInstaller.archive_metadata",
        crate::sensitive::ServiceError::Maintenance,
        cause,
    );
    archive_error()
}
fn validate_zip_metadata(path: &Path, count: usize) -> AcpResult<()> {
    let file = fs::File::open(path).map_err(archive_cause)?;
    let mut zip = zip::ZipArchive::new(file).map_err(archive_cause)?;
    if zip.len() != count {
        return Err(archive_error());
    }
    let mut total = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(archive_cause)?;
        if entry.encrypted()
            || !registry::validate_archive_entries(entry.name())
            || entry
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000))
        {
            return Err(archive_error());
        }
        total = total
            .checked_add(entry.size())
            .filter(|size| *size <= MAX_EXPANDED_BYTES)
            .ok_or_else(archive_error)?;
    }
    Ok(())
}
fn install_error() -> AcpError {
    AcpError::new(
        Reason::InstallFailed,
        "Could not install the ACP Registry agent.",
    )
}
fn archive_error() -> AcpError {
    AcpError::new(
        Reason::ArchiveInvalid,
        "The ACP Registry archive contains an unsafe or missing command.",
    )
}

#[async_trait]
pub trait RegistryCommands: Send + Sync {
    fn resolve(&self, command: &str, environment: &Environment) -> Option<PathBuf>;
    async fn run(
        &self,
        command: &Path,
        args: &[String],
        environment: &Environment,
        timeout: Duration,
        max_bytes: usize,
    ) -> AcpResult<Sensitive<String>>;
}
pub struct ProcessRegistryCommands {
    pub cwd: PathBuf,
}
#[async_trait]
impl RegistryCommands for ProcessRegistryCommands {
    fn resolve(&self, command: &str, environment: &Environment) -> Option<PathBuf> {
        let options = process_options(command, &[], environment);
        t3_process::resolve_command(&options.program, &[], &options.env, false)
            .ok()
            .map(|command| command.program.into())
    }
    async fn run(
        &self,
        command: &Path,
        args: &[String],
        environment: &Environment,
        timeout: Duration,
        max_bytes: usize,
    ) -> AcpResult<Sensitive<String>> {
        let mut options = process_options(command, args, environment);
        options.cwd = Some(self.cwd.clone());
        options.timeout = Some(timeout);
        options.max_output_bytes = max_bytes;
        options.check_exit = false;
        let result = t3_process::run(options)
            .await
            .map_err(|_| install_error())?;
        if result.code != Some(0) || result.stdout_truncated {
            return Err(install_error());
        }
        Ok(Sensitive::new(result.stdout_text()))
    }
}
#[async_trait]
pub trait RegistryDownloads: Send + Sync {
    async fn download(&self, url: &str, path: &Path) -> AcpResult<()>;
}
pub struct ReqwestRegistryDownloads {
    client: reqwest::Client,
}
impl ReqwestRegistryDownloads {
    pub fn new() -> AcpResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .build()
                .map_err(|_| install_error())?,
        })
    }
}
#[async_trait]
impl RegistryDownloads for ReqwestRegistryDownloads {
    async fn download(&self, url: &str, path: &Path) -> AcpResult<()> {
        let failure = || {
            AcpError::new(
                Reason::DownloadFailed,
                "Could not download the ACP Registry archive.",
            )
        };
        if !registry::https_url(url) {
            return Err(failure());
        }
        let mut response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(1200))
            .send()
            .await
            .map_err(|_| failure())?
            .error_for_status()
            .map_err(|_| failure())?;
        if !registry::https_url(response.url().as_str()) {
            return Err(failure());
        }
        let mut file = tokio::fs::File::create(path).await.map_err(|_| failure())?;
        let mut size = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
            size = size.saturating_add(chunk.len());
            if size > 1024 * 1024 * 1024 {
                return Err(failure());
            }
            file.write_all(&chunk).await.map_err(|_| failure())?;
        }
        file.flush().await.map_err(|_| failure())
    }
}
struct InstallLock {
    path: PathBuf,
    identity: file_id::FileId,
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| !metadata.file_type().is_symlink())
            && file_id::get_file_id(&self.path).ok() == Some(self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
async fn acquire_lock(path: &Path, clock: &dyn ProviderClock) -> AcpResult<InstallLock> {
    for _ in 0..300 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => {
                drop(file);
                return Ok(InstallLock {
                    path: path.into(),
                    identity: file_id::get_file_id(path).map_err(|_| install_error())?,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(metadata) = fs::symlink_metadata(path) {
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(install_error());
                    }
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map(|time| time.as_millis() as i64);
                    if modified.is_some_and(|time| clock.now_ms().saturating_sub(time) > 300_000) {
                        fs::remove_file(path).map_err(|_| install_error())?;
                        continue;
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(_) => return Err(install_error()),
        }
    }
    Err(AcpError::new(
        Reason::InstallFailed,
        "Timed out waiting for the ACP Registry install lock.",
    ))
}
pub fn directory_below(base: &Path, components: &[&str]) -> AcpResult<PathBuf> {
    fs::create_dir_all(base).map_err(|_| install_error())?;
    let mut current = fs::canonicalize(base).map_err(|_| install_error())?;
    for component in components {
        if component.is_empty()
            || matches!(*component, "." | "..")
            || component.contains(['/', '\\', '\0'])
        {
            return Err(install_error());
        }
        let next = current.join(component);
        match fs::symlink_metadata(&next) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(install_error());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&next).map_err(|_| install_error())?
            }
            Err(_) => return Err(install_error()),
        }
        let resolved = fs::canonicalize(next).map_err(|_| install_error())?;
        if resolved.parent() != Some(current.as_path()) {
            return Err(install_error());
        }
        current = resolved;
    }
    Ok(current)
}
pub fn executable_in_root(
    root: &Path,
    executable: &Path,
    make_executable: bool,
) -> AcpResult<PathBuf> {
    let root = fs::canonicalize(root).map_err(|_| archive_error())?;
    let executable = fs::canonicalize(executable).map_err(|_| archive_error())?;
    if executable == root || !executable.starts_with(&root) || !executable.is_file() {
        return Err(archive_error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&executable)
            .map_err(|_| archive_error())?
            .permissions();
        if make_executable {
            permissions.set_mode(permissions.mode() | 0o111);
            fs::set_permissions(&executable, permissions).map_err(|_| archive_error())?;
        } else if permissions.mode() & 0o111 == 0 {
            return Err(archive_error());
        }
    }
    #[cfg(not(unix))]
    let _ = make_executable;
    Ok(executable)
}
fn check_install_root(parent: &Path, root: &Path) -> AcpResult<()> {
    match fs::symlink_metadata(root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || fs::canonicalize(root)
                    .map_err(|_| install_error())?
                    .parent()
                    != Some(parent)
            {
                return Err(install_error());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(install_error()),
    }
    Ok(())
}
pub struct RegistryBinaryInstaller {
    pub tools: PathBuf,
    pub platform: Platform,
    pub target: String,
    pub environment: Environment,
    pub commands: Arc<dyn RegistryCommands>,
    pub downloads: Arc<dyn RegistryDownloads>,
    pub clock: Arc<dyn ProviderClock>,
}
impl RegistryBinaryInstaller {
    pub async fn install(&self, agent: &Agent, target: &BinaryTarget) -> AcpResult<PathBuf> {
        agent.validate().map_err(|_| archive_error())?;
        let segments = registry::normalize_command_path(&target.cmd).ok_or_else(archive_error)?;
        let matched = agent
            .distribution
            .binary
            .as_ref()
            .and_then(|targets| {
                targets
                    .iter()
                    .find(|(name, candidate)| {
                        (*name == &self.target
                            || self.target == "windows-aarch64" && *name == "windows-x86_64")
                            && candidate.archive == target.archive
                            && candidate.cmd == target.cmd
                    })
                    .map(|(name, _)| name.as_str())
            })
            .ok_or_else(archive_error)?;
        let version = registry::install_root(&self.tools, agent, matched)
            .map_err(|_| archive_error())?
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(archive_error)?
            .to_owned();
        let parent = directory_below(&self.tools, &[&agent.id, &version])?;
        let root = parent.join(matched);
        let executable = segments
            .iter()
            .fold(root.clone(), |path, segment| path.join(segment));
        check_install_root(&parent, &root)?;
        if executable.exists() {
            return executable_in_root(&root, &executable, false);
        }
        let _lock = acquire_lock(&parent.join(format!("{matched}.lock")), &*self.clock).await?;
        check_install_root(&parent, &root)?;
        if executable.exists() {
            return executable_in_root(&root, &executable, false);
        }
        let staging = tempfile::Builder::new()
            .prefix(".acp-registry-")
            .tempdir_in(&parent)
            .map_err(|_| install_error())?;
        let archive = staging.path().join("archive");
        let extracted = staging.path().join("extracted");
        fs::create_dir(&extracted).map_err(|_| install_error())?;
        self.downloads.download(&target.archive, &archive).await?;
        if let Some(expected) = target.sha256.as_ref() {
            let archive = archive.clone();
            let actual = tokio::task::spawn_blocking(move || {
                let mut file = fs::File::open(archive).map_err(|_| install_error())?;
                let mut hash = Sha256::new();
                let mut bytes = [0u8; 64 * 1024];
                loop {
                    let count = file.read(&mut bytes).map_err(|_| install_error())?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&bytes[..count]);
                }
                Ok::<_, AcpError>(format!("{:x}", hash.finalize()))
            })
            .await
            .map_err(|_| install_error())??;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(AcpError::new(
                    Reason::ChecksumMismatch,
                    "The ACP Registry archive checksum did not match.",
                ));
            }
        }
        let url = url::Url::parse(&target.archive).map_err(|_| archive_error())?;
        let path = url.path().to_ascii_lowercase();
        let archive_path = archive.to_string_lossy().into_owned();
        let extracted_path = extracted.to_string_lossy().into_owned();
        let kind = if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
            "gz"
        } else if path.ends_with(".tar.bz2") || path.ends_with(".tbz2") {
            "bz2"
        } else if path.ends_with(".zip") {
            "zip"
        } else {
            "raw"
        };
        if kind == "raw" {
            let command = segments
                .iter()
                .fold(extracted.clone(), |path, segment| path.join(segment));
            fs::create_dir_all(command.parent().ok_or_else(archive_error)?)
                .map_err(|_| install_error())?;
            fs::rename(&archive, command).map_err(|_| install_error())?;
        } else {
            let tar = crate::environment::archive_tar(self.platform, &self.environment);
            let tar = tar.to_str().ok_or_else(archive_error)?;
            let (tool, args) = if kind == "zip" && self.platform != Platform::Windows {
                ("unzip", vec!["-Z1".into(), archive_path.clone()])
            } else {
                (
                    tar,
                    vec![
                        match kind {
                            "gz" => "-tzf",
                            "bz2" => "-tjf",
                            _ => "-tf",
                        }
                        .into(),
                        archive_path.clone(),
                    ],
                )
            };
            let listing = self
                .commands
                .run(
                    Path::new(tool),
                    &args,
                    &self.environment,
                    Duration::from_secs(1200),
                    64 * 1024 * 1024,
                )
                .await?;
            let count = archive_entry_count(listing.expose());
            if count == 0
                || count > MAX_ARCHIVE_ENTRIES
                || !registry::validate_archive_entries(listing.expose())
            {
                return Err(archive_error());
            }
            if kind == "zip" {
                validate_zip_metadata(&archive, count)?;
            } else {
                let metadata = self
                    .commands
                    .run(
                        Path::new(tar),
                        &[
                            if kind == "gz" { "-tvzf" } else { "-tvjf" }.into(),
                            archive_path.clone(),
                            "--numeric-owner".into(),
                        ],
                        &self.environment,
                        Duration::from_secs(1200),
                        64 * 1024 * 1024,
                    )
                    .await?;
                validate_tar_metadata(metadata.expose(), count)?;
            }
            let (tool, args) = if kind == "zip" && self.platform != Platform::Windows {
                (
                    "unzip",
                    vec!["-q".into(), archive_path, "-d".into(), extracted_path],
                )
            } else {
                (
                    tar,
                    vec![
                        match kind {
                            "gz" => "-xzf",
                            "bz2" => "-xjf",
                            _ => "-xf",
                        }
                        .into(),
                        archive_path,
                        "-C".into(),
                        extracted_path,
                    ],
                )
            };
            self.commands
                .run(
                    Path::new(tool),
                    &args,
                    &self.environment,
                    Duration::from_secs(1200),
                    1024 * 1024,
                )
                .await?;
        }
        let staged = segments
            .iter()
            .fold(extracted.clone(), |path, segment| path.join(segment));
        executable_in_root(&extracted, &staged, true)?;
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(install_error()),
            Ok(_) => {
                let canonical = fs::canonicalize(&root).map_err(|_| install_error())?;
                if canonical.parent() != Some(parent.as_path()) {
                    return Err(install_error());
                }
                fs::remove_dir_all(&root).map_err(|_| install_error())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(install_error()),
        }
        fs::rename(extracted, &root).map_err(|_| install_error())?;
        executable_in_root(&root, &executable, false)
    }
}
