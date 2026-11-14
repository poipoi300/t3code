use crate::{Error, Result};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct ServerSecretStore {
    directory: Arc<PathBuf>,
}
impl ServerSecretStore {
    pub async fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        let operation = directory.clone();
        blocking(move || {
            std::fs::create_dir_all(&operation)
                .map_err(|e| secret_error("secure", &operation.display().to_string(), e))?;
            secure(&operation, 0o700)
                .map_err(|e| secret_error("secure", &operation.display().to_string(), e))
        })
        .await?;
        Ok(Self {
            directory: Arc::new(directory),
        })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.join(format!("{name}.bin"))
    }
    pub async fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path(name);
        let name = name.to_owned();
        blocking(move || match std::fs::File::open(&path) {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|e| secret_error("read", &name, e))?;
                Ok(Some(bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(secret_error("read", &name, e)),
        })
        .await
    }
    pub async fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        self.publish(name, value, false).await
    }
    pub async fn create(&self, name: &str, value: &[u8]) -> Result<()> {
        self.publish(name, value, true).await
    }
    async fn publish(&self, name: &str, value: &[u8], exclusive: bool) -> Result<()> {
        let path = self.path(name);
        let name = name.to_owned();
        let bytes = value.to_vec();
        blocking(move || {
            let temp = path.with_extension(format!("bin.{}.tmp", crate::utils::random_uuid()?));
            let result = (|| -> std::io::Result<()> {
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temp)?;
                file.write_all(&bytes)?;
                if exclusive {
                    file.sync_all()?;
                }
                drop(file);
                secure(&temp, 0o600)?;
                if exclusive {
                    publish_exclusive(&temp, &path)?;
                } else {
                    std::fs::rename(&temp, &path)?;
                    secure(&path, 0o600)?;
                }
                Ok(())
            })();
            let _ = std::fs::remove_file(temp);
            result.map_err(|e| secret_error("persist", &name, e))
        })
        .await
    }
    pub async fn get_or_create_random(&self, name: &str, count: usize) -> Result<Vec<u8>> {
        if let Some(bytes) = self.get(name).await? {
            return Ok(bytes);
        }
        let mut bytes = vec![0; count];
        getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
        match self.create(name, &bytes).await {
            Ok(()) => Ok(bytes),
            Err(e) if e.is_already_exists() => self.get(name).await?.ok_or_else(|| {
                secret_error(
                    "read after concurrent creation",
                    name,
                    std::io::Error::from(std::io::ErrorKind::NotFound),
                )
            }),
            Err(e) => Err(e),
        }
    }
    pub async fn remove(&self, name: &str) -> Result<()> {
        let path = self.path(name);
        let name = name.to_owned();
        blocking(move || match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(secret_error("remove", &name, e)),
        })
        .await
    }
    pub async fn prune_replay_markers(&self, now_ms: i64) -> Result<PruneResult> {
        let directory = self.directory.clone();
        blocking(move || {
            let entries = std::fs::read_dir(&*directory)
                .map_err(|e| secret_error("read", "replay directory", e))?;
            let mut result = PruneResult::default();
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        result.failed += 1;
                        continue;
                    }
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".bin")
                    || !REPLAY_PREFIXES
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
                {
                    continue;
                }
                result.matched += 1;
                let remove = (|| -> std::io::Result<bool> {
                    let modified = entry.metadata()?.modified()?;
                    let ms = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or_else(|e| -(e.duration().as_millis() as i64));
                    if ms >= now_ms - 86_400_000 {
                        return Ok(false);
                    }
                    std::fs::remove_file(entry.path())?;
                    Ok(true)
                })();
                match remove {
                    Ok(true) => result.removed += 1,
                    Ok(false) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => result.failed += 1,
                }
            }
            Ok(result)
        })
        .await
    }
}
#[cfg(windows)]
fn publish_exclusive(temp: &Path, path: &Path) -> std::io::Result<()> {
    // Windows hard-link publication can prevent replacement after a read in the same process.
    tempfile::TempPath::try_from_path(temp.to_owned())?
        .persist_noclobber(path)
        .map_err(|error| error.error)
}
#[cfg(not(windows))]
fn publish_exclusive(temp: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::hard_link(temp, path)
}
pub const REPLAY_PREFIXES: &[&str] = &[
    "dpop-proof-",
    "cloud-mint-jti-",
    "cloud-mint-nonce-",
    "cloud-health-jti-",
    "cloud-health-nonce-",
];
#[derive(Debug, Default)]
pub struct PruneResult {
    pub matched: usize,
    pub removed: usize,
    pub failed: usize,
}
fn secret_error(operation: &'static str, name: &str, source: std::io::Error) -> Error {
    Error::Secret {
        operation,
        name: name.to_owned(),
        source,
    }
}
fn secure(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| secret_error("execute", "blocking operation", std::io::Error::other(e)))?
}
