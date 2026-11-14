use crate::sensitive::{ServiceError, ServiceResult};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use t3_process::CancellationToken;

pub struct CredentialLease {
    path: Option<PathBuf>,
    identity: Option<file_id::FileId>,
    cancel: CancellationToken,
    compromised: CancellationToken,
}
impl CredentialLease {
    pub fn memory() -> Self {
        Self {
            path: None,
            identity: None,
            cancel: CancellationToken::new(),
            compromised: CancellationToken::new(),
        }
    }
    pub fn check(&self) -> ServiceResult<()> {
        if self.compromised.is_cancelled()
            || self
                .path
                .as_ref()
                .is_some_and(|p| !owns_directory(p, self.identity))
        {
            Err(ServiceError::Storage)
        } else {
            Ok(())
        }
    }
    pub async fn run<T>(
        &self,
        operation: impl std::future::Future<Output = ServiceResult<T>>,
    ) -> ServiceResult<T> {
        self.check()?;
        tokio::select! {
            biased;
            _ = self.compromised.cancelled() => Err(ServiceError::Storage),
            result = operation => {
                self.check()?;
                result
            }
        }
    }
    pub async fn acquire(directory: &Path, key: &str) -> ServiceResult<Self> {
        if key.is_empty() || key.contains(['/', '\\', '\0']) {
            return Err(ServiceError::Storage);
        }
        fs::create_dir_all(directory).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Storage,
        ))?;
        let path = directory.join(format!("{key}.bin.lock"));
        for attempt in 0..=80 {
            match fs::create_dir(&path) {
                Ok(()) => {
                    let identity =
                        file_id::get_file_id(&path).map_err(crate::diagnostics::mapped(
                            concat!(module_path!(), "::error"),
                            ServiceError::Storage,
                        ))?;
                    let mut lease = Self::memory();
                    lease.path = Some(path.clone());
                    lease.identity = Some(identity);
                    let file = directory_handle(&path)?;
                    file.set_modified(SystemTime::now())
                        .map_err(crate::diagnostics::mapped(
                            concat!(module_path!(), "::error"),
                            ServiceError::Storage,
                        ))?;
                    let mut modified = fs::metadata(&path).and_then(|m| m.modified()).map_err(
                        crate::diagnostics::mapped(
                            concat!(module_path!(), "::error"),
                            ServiceError::Storage,
                        ),
                    )?;
                    let cancel = lease.cancel.clone();
                    let compromised = lease.compromised.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(10))=>{}}
                            let valid = owns_directory(&path, Some(identity))
                                && fs::metadata(&path).and_then(|m| m.modified()).ok()
                                    == Some(modified);
                            if !valid || file.set_modified(SystemTime::now()).is_err() {
                                compromised.cancel();
                                break;
                            }
                            let Ok(next) = fs::metadata(&path).and_then(|m| m.modified()) else {
                                compromised.cancel();
                                break;
                            };
                            modified = next;
                        }
                    });
                    return Ok(lease);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata =
                        fs::symlink_metadata(&path).map_err(crate::diagnostics::mapped(
                            concat!(module_path!(), "::error"),
                            ServiceError::Storage,
                        ))?;
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(ServiceError::Storage);
                    }
                    if metadata
                        .modified()
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > Duration::from_secs(120))
                    {
                        let _ = fs::remove_dir(&path);
                        continue;
                    }
                }
                Err(_) => return Err(ServiceError::Storage),
            }
            if attempt < 80 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        Err(ServiceError::Storage)
    }
}
fn owns_directory(path: &Path, identity: Option<file_id::FileId>) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        && file_id::get_file_id(path).ok() == identity
}
fn directory_handle(path: &Path) -> ServiceResult<fs::File> {
    let mut options = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.access_mode(0x100).custom_flags(0x02000000);
    }
    #[cfg(not(windows))]
    {
        options.read(true);
    }
    options.open(path).map_err(crate::diagnostics::mapped(
        concat!(module_path!(), "::error"),
        ServiceError::Storage,
    ))
}
impl Drop for CredentialLease {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(path) = self.path.as_ref()
            && owns_directory(path, self.identity)
        {
            let _ = fs::remove_dir(path);
        }
    }
}
