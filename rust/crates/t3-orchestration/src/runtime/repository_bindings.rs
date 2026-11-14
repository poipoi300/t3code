//! The file the server keeps each scope's repository binding in (t3-git H-1).
use crate::{
    api::RepositoryBindingStore,
    error::{Error, Result},
    ports::checkpoints_forks::checkpoint_restore_safety::canonicalize_missing,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

/// The file format's version; a binding's content changes with t3-git's identity (re-review 4
/// F4), and the version changes with it.
const VERSION: u64 = 1;

/// `{"version": 1, "bindings": {folder: binding}}` under the server's state directory. Every read
/// goes to the file under a lock shared with other servers on the same state directory, and
/// every write re-reads it under an exclusive lock and replaces it atomically (re-review 4 L2).
/// A folder is saved under the spelling it was given and under its canonical path, and found
/// under either (L1); the given spelling comes first, so a folder swapped for a link still finds
/// the binding that refuses it.
pub struct FileRepositoryBindings {
    path: PathBuf,
}

impl FileRepositoryBindings {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

fn failure(detail: impl Into<String>) -> Error {
    Error::Service {
        operation: "repository-bindings",
        detail: detail.into(),
    }
}

fn keys(scope: &Path) -> Result<Vec<String>> {
    let text = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| failure("Folder paths must be valid Unicode."))
    };
    let given = text(scope)?;
    let canonical = text(&canonicalize_missing(scope)?)?;
    Ok(if canonical == given {
        vec![given]
    } else {
        vec![given, canonical]
    })
}

fn read(path: &Path) -> Result<BTreeMap<String, Value>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    let file: Value = serde_json::from_slice(&bytes)?;
    if file["version"] != json!(VERSION) {
        return Err(failure(format!(
            "The repository bindings file has version {}, not {VERSION}.",
            file["version"]
        )));
    }
    Ok(serde_json::from_value(file["bindings"].clone())?)
}

/// Runs `work` holding the bindings lock file, shared or exclusive.
fn locked<T>(file: &Path, exclusive: bool, work: impl FnOnce() -> Result<T>) -> Result<T> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(file.with_extension("lock"))?;
    if exclusive {
        lock.lock()?;
    } else {
        lock.lock_shared()?;
    }
    work()
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| failure(error.to_string()))?
}

#[async_trait]
impl RepositoryBindingStore for FileRepositoryBindings {
    async fn get(&self, scope: &Path) -> Result<Option<Value>> {
        let keys = keys(scope)?;
        let path = self.path.clone();
        blocking(move || {
            locked(&path, false, || {
                let entries = read(&path)?;
                Ok(keys.iter().find_map(|key| entries.get(key).cloned()))
            })
        })
        .await
    }

    async fn put(&self, scope: &Path, binding: Value) -> Result<()> {
        let keys = keys(scope)?;
        let path = self.path.clone();
        blocking(move || {
            locked(&path, true, || {
                let mut entries = read(&path)?;
                for key in keys {
                    entries.insert(key, binding.clone());
                }
                let temporary = path.with_extension("json.tmp");
                let mut file = std::fs::File::create(&temporary)?;
                file.write_all(&serde_json::to_vec_pretty(
                    &json!({"version": VERSION, "bindings": entries}),
                )?)?;
                file.sync_all()?;
                drop(file);
                std::fs::rename(&temporary, &path)?;
                Ok(())
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Re-review 4 L1: a binding saved under one spelling of a folder is found under another,
    /// also by a fresh store (after a restart).
    #[tokio::test]
    async fn a_binding_is_found_under_another_spelling_of_its_folder() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let folder = temp.path().join("Work");
        std::fs::create_dir(&folder)?;
        let path = temp.path().join("bindings.json");
        FileRepositoryBindings::new(path.clone())
            .put(&folder, json!({"saved": true}))
            .await?;
        let mut spellings = vec![folder.join(""), folder.join("."), folder.clone()];
        if cfg!(windows) {
            spellings.push(PathBuf::from(folder.to_string_lossy().to_lowercase()));
            spellings.push(std::fs::canonicalize(&folder)?);
        }
        let store = FileRepositoryBindings::new(path);
        for spelling in spellings {
            assert_eq!(
                store.get(&spelling).await?,
                Some(json!({"saved": true})),
                "{spelling:?}"
            );
        }
        Ok(())
    }

    /// Re-review 4 L2: two stores on one file (two servers on one state directory) keep each
    /// other's bindings, and a corrupt file fails closed.
    #[tokio::test]
    async fn stores_sharing_a_file_keep_each_others_bindings() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("bindings.json");
        let (a, b) = (temp.path().join("a"), temp.path().join("b"));
        std::fs::create_dir(&a)?;
        std::fs::create_dir(&b)?;
        let first = FileRepositoryBindings::new(path.clone());
        let second = FileRepositoryBindings::new(path.clone());
        assert_eq!(first.get(&a).await?, None);
        second.put(&b, json!("b")).await?;
        first.put(&a, json!("a")).await?;
        assert_eq!(second.get(&a).await?, Some(json!("a")));
        let fresh = FileRepositoryBindings::new(path.clone());
        assert_eq!(fresh.get(&a).await?, Some(json!("a")));
        assert_eq!(fresh.get(&b).await?, Some(json!("b")));
        std::fs::write(&path, "not json")?;
        assert!(FileRepositoryBindings::new(path).get(&a).await.is_err());
        Ok(())
    }
}
