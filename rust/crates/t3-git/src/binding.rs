use crate::{ExecuteOptions, Git, GitError};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepositoryBinding {
    pub top_level: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    Missing,
    Directory(PathBuf),
    File(Vec<u8>, PathBuf),
}

pub(crate) struct BoundRepository {
    pub identity: RepositoryBinding,
    scope: PathBuf,
    markers: BTreeMap<PathBuf, Marker>,
}

fn changed(path: &Path, detail: &'static str) -> GitError {
    GitError::RepositoryChanged {
        path: path.to_owned(),
        detail,
    }
}

fn marker(path: &Path) -> Result<Marker, GitError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(Marker::Directory(std::fs::canonicalize(path)?)),
        Ok(metadata) if metadata.len() <= 16_384 && std::fs::metadata(path)?.is_file() => {
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(16_385)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 16_384 {
                return Err(changed(path, "Repository pointer exceeds the size limit."));
            }
            Ok(Marker::File(bytes, std::fs::canonicalize(path)?))
        }
        Ok(_) => Err(changed(
            path,
            "Repository pointer is not a supported file or directory.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Marker::Missing),
        Err(error) => Err(error.into()),
    }
}

impl BoundRepository {
    fn snapshot(identity: RepositoryBinding, cwd: &Path) -> Result<Self, GitError> {
        let scope = crate::safety::resolve(cwd)?;
        if !scope.starts_with(&identity.top_level) {
            return Err(changed(cwd, "Scope is outside the recorded top-level."));
        }
        let mut markers = BTreeMap::new();
        for directory in scope.ancestors() {
            let path = directory.join(".git");
            markers.insert(path.clone(), marker(&path)?);
            if directory == identity.top_level {
                break;
            }
        }
        for name in ["commondir", "gitdir"] {
            let path = identity.git_dir.join(name);
            markers.insert(path.clone(), marker(&path)?);
        }
        Ok(Self {
            identity,
            scope,
            markers,
        })
    }

    fn check(&self, cwd: &Path) -> Result<(), GitError> {
        if crate::safety::resolve(cwd).map_err(|_| changed(cwd, "Scope cannot be resolved."))?
            != self.scope
        {
            return Err(changed(cwd, "Scope now resolves to a different directory."));
        }
        for path in [
            &self.identity.top_level,
            &self.identity.git_dir,
            &self.identity.common_dir,
        ] {
            if std::fs::canonicalize(path)
                .map_err(|_| changed(path, "Recorded directory is unavailable."))?
                != *path
            {
                return Err(changed(path, "Recorded directory now resolves elsewhere."));
            }
        }
        for (path, expected) in &self.markers {
            if marker(path).map_err(|_| changed(path, "Repository metadata cannot be checked."))?
                != *expected
            {
                return Err(changed(
                    path,
                    "Repository metadata was added, removed or redirected.",
                ));
            }
        }
        Ok(())
    }

    pub(crate) async fn validate(self: &Arc<Self>, cwd: &Path) -> Result<(), GitError> {
        let binding = self.clone();
        let cwd = cwd.to_owned();
        tokio::task::spawn_blocking(move || binding.check(&cwd))
            .await
            .map_err(|_| GitError::Unavailable("Repository identity check failed.".into()))?
    }
}

type Bindings = tokio::sync::Mutex<BTreeMap<PathBuf, Arc<BoundRepository>>>;
fn bindings() -> &'static Bindings {
    // Identity must not be evicted or forgotten when a transport recreates its Git handle.
    static BINDINGS: OnceLock<Bindings> = OnceLock::new();
    BINDINGS.get_or_init(Default::default)
}

impl Git {
    pub async fn bind_repository(&self) -> Result<Option<RepositoryBinding>, GitError> {
        Ok(self
            .repository_operation()
            .await?
            .bound_repository
            .as_ref()
            .map(|bound| bound.identity.clone()))
    }

    pub async fn with_repository_binding(
        mut self,
        identity: RepositoryBinding,
    ) -> Result<Self, GitError> {
        self.bound_repository = None;
        self.fsmonitor_disabled = true;
        self.bound_repository = self.resolve_repository_binding(Some(&identity)).await?;
        Ok(self)
    }

    pub(crate) async fn repository_reader(&self) -> Result<Self, GitError> {
        let mut reader = self.without_fsmonitor();
        if let Some(binding) = &reader.bound_repository {
            binding.validate(reader.cwd()).await?;
        } else {
            reader.bound_repository = reader.resolve_repository_binding(None).await?;
        }
        Ok(reader)
    }

    pub(crate) async fn repository_operation(&self) -> Result<Self, GitError> {
        let expected = self
            .bound_repository
            .as_ref()
            .map(|bound| bound.identity.clone());
        let mut reader = self.without_fsmonitor();
        reader.bound_repository = None;
        reader.bound_repository = reader.resolve_repository_binding(expected.as_ref()).await?;
        Ok(reader)
    }

    pub(crate) async fn checkpoint_reader(&self) -> Result<Self, GitError> {
        let mut reader = if self.checkpoint_mode {
            self.repository_reader().await?
        } else {
            self.repository_operation().await?
        };
        reader.checkpoint_mode = true;
        Ok(reader)
    }

    async fn resolve_repository_binding(
        &self,
        expected: Option<&RepositoryBinding>,
    ) -> Result<Option<Arc<BoundRepository>>, GitError> {
        let key = crate::safety::lexical_absolute(self.cwd())?;
        let resolved = crate::safety::resolve(self.cwd())?;
        let cached = {
            let bindings = bindings().lock().await;
            bindings
                .get(&key)
                .or_else(|| bindings.get(&resolved))
                .cloned()
        };
        if let Some(cached) = &cached {
            cached.validate(self.cwd()).await?;
        }
        let mut discovery = self.without_fsmonitor();
        discovery.bound_repository = None;
        discovery.git_dir = None;
        let output = match discovery
            .execute_unbound(
                &[
                    "rev-parse",
                    "--is-inside-work-tree",
                    "--path-format=absolute",
                    "--absolute-git-dir",
                    "--git-common-dir",
                    "--show-toplevel",
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    max_output_bytes: 65_536,
                    output_mode: crate::OutputMode::Error,
                    ..Default::default()
                },
            )
            .await
        {
            Err(GitError::Process(crate::ProcessError::NotFound { .. }))
                if expected.is_none() && cached.is_none() =>
            {
                return Ok(None);
            }
            result => result?,
        };
        if output.code != Some(0)
            || !output.stdout_text().starts_with("true\n")
                && !output.stdout_text().starts_with("true\r\n")
        {
            if expected.is_some() || cached.is_some() {
                return Err(changed(
                    self.cwd(),
                    "Discovery no longer finds the recorded repository.",
                ));
            }
            return Ok(None);
        }
        let text = output.stdout_text();
        let lines: Vec<_> = text.lines().collect();
        let ["true", git_dir, common_dir, top_level] = lines.as_slice() else {
            return Err(GitError::InvalidOutput("repository identity".into()));
        };
        let identity = RepositoryBinding {
            top_level: std::fs::canonicalize(top_level)?,
            git_dir: std::fs::canonicalize(git_dir)?,
            common_dir: std::fs::canonicalize(common_dir)?,
        };
        if expected.is_some_and(|expected| *expected != identity)
            || cached
                .as_ref()
                .is_some_and(|cached| cached.identity != identity)
        {
            return Err(changed(
                self.cwd(),
                "Discovered Git directory, common directory or top-level differs from the recorded repository.",
            ));
        }
        if let Some(cached) = cached {
            return Ok(Some(cached));
        }
        let cwd = self.cwd().to_owned();
        let root_identity = identity.clone();
        let (binding, root) = tokio::task::spawn_blocking(move || {
            Ok::<_, GitError>((
                Arc::new(BoundRepository::snapshot(identity, &cwd)?),
                Arc::new(BoundRepository::snapshot(
                    root_identity.clone(),
                    &root_identity.top_level,
                )?),
            ))
        })
        .await
        .map_err(|_| GitError::Unavailable("Repository binding failed.".into()))??;
        binding.validate(self.cwd()).await?;
        let mut bindings = bindings().lock().await;
        let binding = bindings.entry(key).or_insert(binding).clone();
        if binding.identity != root.identity {
            return Err(changed(
                self.cwd(),
                "Concurrent binding selected a different repository.",
            ));
        }
        bindings.entry(resolved).or_insert_with(|| binding.clone());
        bindings
            .entry(root.identity.top_level.clone())
            .or_insert(root);
        Ok(Some(binding))
    }
}
