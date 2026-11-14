//! The file the server keeps each scope's repository binding in (t3-git H-1).
use crate::{
    api::RepositoryBindingStore,
    error::{Error, Result},
};
use async_trait::async_trait;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// One JSON object `{scope folder: binding}` under the server's state directory, replaced
/// atomically on every write.
pub struct FileRepositoryBindings {
    path: PathBuf,
    entries: tokio::sync::Mutex<Option<BTreeMap<String, Value>>>,
}

impl FileRepositoryBindings {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            entries: tokio::sync::Mutex::new(None),
        }
    }

    fn load(&self) -> Result<BTreeMap<String, Value>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error.into()),
        }
    }
}

fn key(scope: &Path) -> Result<String> {
    scope.to_str().map(str::to_owned).ok_or(Error::Service {
        operation: "repository-bindings",
        detail: "Scope paths must be valid Unicode.".into(),
    })
}

#[async_trait]
impl RepositoryBindingStore for FileRepositoryBindings {
    async fn get(&self, scope: &Path) -> Result<Option<Value>> {
        let mut entries = self.entries.lock().await;
        if entries.is_none() {
            *entries = Some(self.load()?);
        }
        let key = key(scope)?;
        Ok(entries
            .as_ref()
            .and_then(|entries| entries.get(&key).cloned()))
    }

    async fn put(&self, scope: &Path, binding: Value) -> Result<()> {
        let mut entries = self.entries.lock().await;
        let mut next = match entries.take() {
            Some(entries) => entries,
            None => self.load()?,
        };
        next.insert(key(scope)?, binding);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&next)?)?;
        std::fs::rename(&temporary, &self.path)?;
        *entries = Some(next);
        Ok(())
    }
}
