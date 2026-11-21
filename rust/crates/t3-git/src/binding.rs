use crate::{ExecuteOptions, Git, GitError};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

/// The saved shape of [`RepositoryBinding`]. Version 2 added the file ids; a record of any other
/// version is refused (re-review 4 F4), never rebound.
pub const REPOSITORY_BINDING_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepositoryBinding {
    /// Absent (0) in records saved before file ids were recorded.
    #[serde(default)]
    pub version: u32,
    pub top_level: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<RepositoryIds>,
}

/// The file ids of the `.git` entry at the top-level, the Git directory and the common
/// directory, so a `.git` replaced at the same path no longer matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepositoryIds {
    pub git_entry: FileId,
    pub git_dir: FileId,
    pub common_dir: FileId,
}

/// A file's volume and file id (Windows: volume serial number and file index, from a handle
/// opened on the entry itself; POSIX: device and inode). A replacement at the same path changes
/// it; an in-place edit keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileId {
    pub device: u64,
    pub inode: u64,
}

impl FileId {
    pub fn directory(path: &Path) -> std::io::Result<Self> {
        if !std::fs::symlink_metadata(path)?.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Expected a directory entry",
            ));
        }
        file_id(path)
    }
}

fn file_id(path: &Path) -> std::io::Result<FileId> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(FileId {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use cap_fs_ext::MetadataExt;
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        };
        // The entry itself (not a link's target), attribute access only, any sharing.
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(7)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        // Built from the handle, so the volume serial number and file index are present.
        let metadata = cap_std::fs::Metadata::from_file(&file)?;
        Ok(FileId {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    Missing,
    Directory(PathBuf, FileId),
    File(Vec<u8>, PathBuf, FileId),
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
        Ok(metadata) if metadata.is_dir() => Ok(Marker::Directory(
            std::fs::canonicalize(path)?,
            file_id(path)?,
        )),
        Ok(metadata) if metadata.len() <= 16_384 && std::fs::metadata(path)?.is_file() => {
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(16_385)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 16_384 {
                return Err(changed(path, "Repository pointer exceeds the size limit."));
            }
            Ok(Marker::File(
                bytes,
                std::fs::canonicalize(path)?,
                file_id(path)?,
            ))
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
        if let Some(ids) = &self.identity.ids {
            for (path, expected) in [
                (&self.identity.git_dir, ids.git_dir),
                (&self.identity.common_dir, ids.common_dir),
            ] {
                if file_id(path).map_err(|_| changed(path, "Recorded directory is unavailable."))?
                    != expected
                {
                    return Err(changed(
                        path,
                        "Recorded Git directory was replaced at the same path.",
                    ));
                }
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

fn forget_scope(
    bindings: &mut BTreeMap<PathBuf, Arc<BoundRepository>>,
    key: &Path,
    resolved: &Path,
) {
    let forgotten: Vec<_> = [key, resolved]
        .into_iter()
        .filter_map(|path| bindings.remove(path))
        .map(|bound| bound.identity.clone())
        .collect();
    // Subdirectory bindings never unpin the whole parent repository.
    bindings.retain(|_, bound| {
        !forgotten
            .iter()
            .any(|id| *id == bound.identity && id.top_level == resolved)
    });
}

/// Read a bounded, ordinary Git pointer relative to its containing directory.
pub(crate) fn pointer(path: &Path, prefix: &str) -> Result<PathBuf, GitError> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(changed(path, "Expected a regular repository pointer."));
    }
    let Marker::File(bytes, actual, _) = marker(path)? else {
        return Err(changed(path, "Repository pointer is missing."));
    };
    let value = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.trim_end_matches(['\r', '\n']).strip_prefix(prefix))
        .filter(|s| !s.is_empty() && !s.contains(['\r', '\n', '\0']))
        .ok_or_else(|| changed(path, "Invalid repository pointer."))?;
    Ok(std::fs::canonicalize(
        actual
            .parent()
            .ok_or_else(|| changed(path, "Missing parent."))?
            .join(value),
    )?)
}

impl Git {
    /// Pin maintenance commands without invoking Git's repository discovery on an untrusted checkout.
    pub async fn cleanup_checkout(
        &self,
        source: Option<&RepositoryBinding>,
        recorded: Option<&RepositoryBinding>,
    ) -> Result<Self, GitError> {
        let root = std::fs::canonicalize(self.cwd())?;
        let marker_path = root.join(".git");
        let (admin, common) = match source {
            Some(source) => (
                pointer(&marker_path, "gitdir: ")?,
                source.common_dir.clone(),
            ),
            None if recorded.is_some() && !std::fs::symlink_metadata(&marker_path)?.is_dir() => {
                let saved = recorded.ok_or_else(|| changed(&root, "Missing project binding."))?;
                (saved.git_dir.clone(), saved.common_dir.clone())
            }
            None => (marker_path.clone(), marker_path.clone()),
        };
        if source.is_some() && admin.parent() != Some(common.join("worktrees").as_path()) {
            return Err(changed(&root, "Unregistered worktree directory."));
        }
        let ids = RepositoryIds {
            git_entry: file_id(&marker_path)?,
            git_dir: FileId::directory(&admin)?,
            common_dir: FileId::directory(&common)?,
        };
        let identity = RepositoryBinding {
            version: REPOSITORY_BINDING_VERSION,
            top_level: root.clone(),
            git_dir: admin.clone(),
            common_dir: common.clone(),
            ids: Some(ids),
        };
        if recorded.is_some_and(|saved| saved != &identity)
            || source
                .is_some_and(|saved| saved.ids.map(|ids| ids.common_dir) != Some(ids.common_dir))
        {
            return Err(changed(
                &root,
                "The checkout differs from its recorded repository.",
            ));
        }
        let bound = BoundRepository::snapshot(identity, &root)?;
        if admin != marker_path || source.is_some() {
            if admin.parent() != Some(common.join("worktrees").as_path())
                || pointer(&marker_path, "gitdir: ")? != admin
                || pointer(&admin.join("commondir"), "")? != common
                || pointer(&admin.join("gitdir"), "")? != marker_path
            {
                return Err(changed(
                    &root,
                    "The worktree registration does not match its checkout.",
                ));
            }
        } else if !matches!(marker(&admin.join("commondir"))?, Marker::Missing) {
            return Err(changed(
                &root,
                "The project Git directory redirects its common directory.",
            ));
        }
        let mut git = self.clone();
        git.bound_repository = Some(Arc::new(bound));
        git.fsmonitor_disabled = true;
        git.cleanup_mode = true;
        git.checkpoint_mode = true;
        let top = git.discover_path("--show-toplevel").await?;
        if std::fs::canonicalize(top)? != root {
            return Err(changed(&root, "The checkout top-level changed."));
        }
        Ok(git)
    }

    /// A module store must be beneath the verified parent's own administrative directory.
    /// Populated modules also have a gitfile and a core.worktree backlink.
    pub async fn cleanup_submodule(
        &self,
        checkout: Option<&Path>,
        store: Option<&Path>,
    ) -> Result<Self, GitError> {
        let parent = self
            .recorded_repository()
            .flatten()
            .filter(|_| self.cleanup_mode)
            .ok_or_else(|| changed(self.cwd(), "Missing cleanup repository."))?;
        let root = match checkout {
            Some(path) => std::fs::canonicalize(path)?,
            None => parent.top_level.clone(),
        };
        if !root.starts_with(&parent.top_level) || checkout.is_some() && root == parent.top_level {
            return Err(changed(
                &root,
                "Submodule checkout lies outside its parent.",
            ));
        }
        let entry = root.join(".git");
        let directory = match store {
            Some(directory) => std::fs::canonicalize(directory)?,
            None => pointer(&entry, "gitdir: ")?,
        };
        let modules = parent.git_dir.join("modules");
        if !directory.starts_with(&modules)
            || directory == modules
            || !matches!(marker(&directory.join("commondir"))?, Marker::Missing)
        {
            return Err(changed(&directory, "Unregistered submodule store."));
        }
        let dir_id = FileId::directory(&directory)?;
        let identity = RepositoryBinding {
            version: REPOSITORY_BINDING_VERSION,
            top_level: root.clone(),
            git_dir: directory.clone(),
            common_dir: directory.clone(),
            ids: Some(RepositoryIds {
                git_entry: file_id(&entry)?,
                git_dir: dir_id,
                common_dir: dir_id,
            }),
        };
        let mut git = self.at(&root);
        git.bound_repository = Some(Arc::new(BoundRepository::snapshot(identity, &root)?));
        if checkout.is_some() {
            if pointer(&entry, "gitdir: ")? != directory {
                return Err(changed(&root, "Submodule registration changed."));
            }
            let backlink = git
                .execute(
                    &["config", "--local", "--get", "core.worktree"],
                    Default::default(),
                )
                .await?
                .stdout_text();
            if std::fs::canonicalize(directory.join(backlink.trim_end_matches(['\n', '\r'])))?
                != root
            {
                return Err(changed(&root, "Submodule checkout backlink changed."));
            }
        }
        let top = git.discover_path("--show-toplevel").await?;
        if std::fs::canonicalize(top)? != root {
            return Err(changed(&root, "Submodule top-level changed."));
        }
        Ok(git)
    }

    pub async fn bind_repository(&self) -> Result<Option<RepositoryBinding>, GitError> {
        Ok(self
            .repository_operation()
            .await?
            .bound_repository
            .as_ref()
            .map(|bound| bound.identity.clone()))
    }

    /// Trusts the repository now at this scope: forgets the in-process identity recorded for the
    /// scope (and for the same repository at other scopes), then binds what is there. Only for
    /// the user's explicit trust or a repository the server itself just created in the folder
    /// (a clone, a recreated worktree); everything else keeps refusing a changed repository.
    pub async fn rebind_repository(&self) -> Result<Option<RepositoryBinding>, GitError> {
        let key = crate::safety::lexical_absolute(self.cwd())?;
        let resolved = crate::safety::resolve(self.cwd())?;
        {
            let mut bindings = bindings().lock().await;
            forget_scope(&mut bindings, &key, &resolved);
        }
        let mut fresh = self.clone();
        fresh.bound_repository = None;
        fresh.bound_no_repository = false;
        fresh.bind_repository().await
    }

    /// Only replace the cached identity after discovery proves that the linked
    /// checkout belongs to the expected source repository. A refusal changes no cache entry.
    pub async fn bind_recreated_worktree(
        &self,
        source: &RepositoryBinding,
    ) -> Result<RepositoryBinding, GitError> {
        self.resolve_binding(None, Some(source), None)
            .await?
            .map(|binding| binding.identity.clone())
            .ok_or_else(|| changed(self.cwd(), "The recreated checkout is missing."))
    }

    /// Install an identity only after discovery verifies the standalone clone and its directory.
    pub async fn bind_cloned_repository(
        &self,
        destination: FileId,
        git_directory: FileId,
    ) -> Result<RepositoryBinding, GitError> {
        self.resolve_binding(None, None, Some((destination, git_directory)))
            .await?
            .map(|binding| binding.identity.clone())
            .ok_or_else(|| changed(self.cwd(), "The cloned repository is missing."))
    }

    pub async fn with_repository_binding(
        mut self,
        identity: RepositoryBinding,
    ) -> Result<Self, GitError> {
        if identity.version != REPOSITORY_BINDING_VERSION || identity.ids.is_none() {
            return Err(changed(
                self.cwd(),
                "The saved binding was recorded by an earlier version and cannot prove this is the same repository; trust the repository again to continue.",
            ));
        }
        self.bound_repository = None;
        self.bound_no_repository = false;
        self.fsmonitor_disabled = true;
        self.bound_repository = self.resolve_repository_binding(Some(&identity)).await?;
        Ok(self)
    }

    /// The explicit pin carried by this handle. The outer option is absent for discovery.
    pub fn recorded_repository(&self) -> Option<Option<&RepositoryBinding>> {
        if self.bound_no_repository {
            Some(None)
        } else {
            self.bound_repository
                .as_ref()
                .map(|binding| Some(&binding.identity))
        }
    }

    pub async fn with_no_repository_binding(mut self) -> Result<Self, GitError> {
        self.bound_repository = None;
        self.bound_no_repository = true;
        self.fsmonitor_disabled = true;
        self.resolve_repository_binding(None).await?;
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
        if self.cleanup_mode {
            return self.repository_reader().await;
        }
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
        self.resolve_binding(expected, None, None).await
    }

    async fn discover_path(&self, field: &str) -> Result<String, GitError> {
        let output = self
            .execute_unbound(
                &["rev-parse", "--path-format=absolute", field],
                ExecuteOptions {
                    max_output_bytes: 65_536,
                    ..Default::default()
                },
            )
            .await?;
        let path = output.stdout_text();
        let path = if cfg!(windows) {
            path.strip_suffix("\r\n")
        } else {
            None
        }
        .or_else(|| path.strip_suffix('\n'))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| GitError::InvalidOutput("repository identity".into()))?;
        Ok(path.to_owned())
    }

    async fn resolve_binding(
        &self,
        expected: Option<&RepositoryBinding>,
        recreated_from: Option<&RepositoryBinding>,
        cloned: Option<(FileId, FileId)>,
    ) -> Result<Option<Arc<BoundRepository>>, GitError> {
        let key = crate::safety::lexical_absolute(self.cwd())?;
        let resolved = crate::safety::resolve(self.cwd())?;
        let previous = {
            let bindings = bindings().lock().await;
            bindings
                .get(&key)
                .or_else(|| bindings.get(&resolved))
                .cloned()
        };
        let replacing = recreated_from.is_some() || cloned.is_some();
        let cached = previous.as_ref().filter(|_| !replacing);
        if let Some(cached) = cached {
            cached.validate(self.cwd()).await?;
        }
        let mut discovery = self.without_fsmonitor();
        discovery.bound_repository = None;
        discovery.bound_no_repository = false;
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
        if self.bound_no_repository {
            return Err(changed(
                self.cwd(),
                "A Git repository was created in this folder after T3 Code first saw it without one.",
            ));
        }
        let text = output.stdout_text();
        let lines: Vec<_> = text.lines().collect();
        let separate;
        let (git_dir, common_dir, top_level) = match lines.as_slice() {
            ["true", git_dir, common_dir, top_level] => (*git_dir, *common_dir, *top_level),
            _ => {
                // Git emits literal newlines in path names. Separate the records at the
                // process boundary when line splitting cannot identify all three paths.
                separate = [
                    discovery.discover_path("--absolute-git-dir").await?,
                    discovery.discover_path("--git-common-dir").await?,
                    discovery.discover_path("--show-toplevel").await?,
                ];
                (&*separate[0], &*separate[1], &*separate[2])
            }
        };
        let top_level = std::fs::canonicalize(top_level)?;
        let git_dir = std::fs::canonicalize(git_dir)?;
        let common_dir = std::fs::canonicalize(common_dir)?;
        let ids = RepositoryIds {
            git_entry: file_id(&top_level.join(".git"))?,
            git_dir: file_id(&git_dir)?,
            common_dir: file_id(&common_dir)?,
        };
        let identity = RepositoryBinding {
            version: REPOSITORY_BINDING_VERSION,
            top_level,
            git_dir,
            common_dir,
            ids: Some(ids),
        };
        if let Some((destination, git_directory)) = cloned
            && (crate::core::git_path(&resolved) != crate::core::git_path(&key)
                || FileId::directory(&resolved)? != destination
                || identity.top_level != resolved
                || identity.git_dir != resolved.join(".git")
                || identity.common_dir != identity.git_dir
                || ids.git_entry != git_directory
                || ids.git_dir != git_directory)
        {
            return Err(changed(
                self.cwd(),
                "The clone no longer has the verified repository identity.",
            ));
        }
        if let Some(source) = recreated_from
            && (source.version != REPOSITORY_BINDING_VERSION
                || source.ids.is_none()
                || identity.top_level != resolved
                || identity.git_dir == identity.common_dir
                || identity.common_dir != source.common_dir
                || identity.ids.as_ref().map(|ids| ids.common_dir)
                    != source.ids.as_ref().map(|ids| ids.common_dir))
        {
            return Err(changed(
                self.cwd(),
                "The recreated checkout does not belong to the recorded project repository.",
            ));
        }
        if expected.is_some_and(|expected| *expected != identity)
            || cached.is_some_and(|cached| cached.identity != identity)
        {
            return Err(changed(
                self.cwd(),
                "Discovered Git directory, common directory or top-level differs from the recorded repository.",
            ));
        }
        if let Some(cached) = cached {
            return Ok(Some(cached.clone()));
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
        if let Some((destination, _)) = cloned
            && FileId::directory(&resolved)? != destination
        {
            return Err(changed(
                self.cwd(),
                "The clone destination changed while verifying the repository.",
            ));
        }
        if replacing {
            let current = bindings.get(&key).or_else(|| bindings.get(&resolved));
            if current.map(|b| &b.identity) != previous.as_ref().map(|b| &b.identity) {
                return Err(changed(
                    self.cwd(),
                    "The repository binding changed while verifying a replacement checkout.",
                ));
            }
            forget_scope(&mut bindings, &key, &resolved);
        }
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
