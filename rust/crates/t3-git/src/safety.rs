use crate::{CancellationToken, ExecuteOptions, Git, GitError, OutputMode};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub const SHARED_WORKSPACE_RESTORE_MESSAGE: &str = "File restore requires an isolated worktree. This workspace may contain changes from another thread or a side chat. Rewind the conversation without restoring files instead, or delete side chats you no longer need.";

#[derive(Debug, Clone)]
pub struct RestoreSession {
    pub cwd: PathBuf,
    pub stopped: bool,
    pub supports_multiple_provider_threads: bool,
}
#[derive(Debug, Clone)]
pub struct RestoreParticipant {
    pub thread_id: String,
    pub deleted: bool,
    pub worktree_path: Option<PathBuf>,
    /// Required for a thread without a worktree; absence fails closed.
    pub project_root: Option<PathBuf>,
    pub scope_paths: Vec<PathBuf>,
    pub provider_sessions: Vec<RestoreSession>,
    pub side_chat: bool,
    pub active_run: bool,
    pub has_mutating_turn_items: bool,
}
#[derive(Debug, Clone)]
pub struct RestoreSafety {
    pub thread_id: String,
    pub worktree_path: Option<PathBuf>,
    pub scope_cwd: PathBuf,
    /// Include both active and archived threads and their current provider sessions.
    pub participants: Vec<RestoreParticipant>,
}
pub(crate) fn lexical_absolute(path: &Path) -> Result<PathBuf, std::io::Error> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component),
        }
    }
    Ok(normalized)
}
pub(crate) fn resolve(path: &Path) -> Result<PathBuf, std::io::Error> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for name in missing.into_iter().rev() {
                    resolved.push(name);
                }
                return lexical_absolute(&resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(
                    component @ (Component::Normal(_) | Component::ParentDir | Component::CurDir),
                ) = ancestor.components().next_back()
                else {
                    return Err(error);
                };
                missing.push(component.as_os_str().to_owned());
                let Some(parent) = ancestor.parent() else {
                    return Err(error);
                };
                ancestor = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

fn comparison_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    let path = resolve(path)?;
    #[cfg(windows)]
    let path = PathBuf::from(path.to_string_lossy().to_lowercase());
    Ok(path)
}

fn is_link(path: &Path, metadata: &std::fs::Metadata) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 == 0 {
            return Ok(false);
        }
        // std reads the reparse tag and decodes only SYMLINK and MOUNT_POINT.
        decoded_reparse_link(std::fs::read_link(path))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(metadata.file_type().is_symlink())
    }
}

#[cfg(windows)]
fn decoded_reparse_link(result: std::io::Result<PathBuf>) -> std::io::Result<bool> {
    match result {
        Ok(_) => Ok(true),
        Err(error)
            if error.raw_os_error().is_none()
                && error.to_string() == "Unsupported reparse point type" =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

#[cfg(all(test, windows))]
mod reparse_tests {
    use super::*;
    #[test]
    fn only_supported_link_tags_are_links_and_io_failures_fail_closed() {
        assert!(decoded_reparse_link(Ok(PathBuf::from("target"))).unwrap());
        assert!(
            !decoded_reparse_link(Err(std::io::Error::other("Unsupported reparse point type")))
                .unwrap()
        );
        assert!(decoded_reparse_link(Err(std::io::Error::from_raw_os_error(5))).is_err());
    }
}

pub(crate) struct CheckpointScan {
    pub exclusions: Vec<String>,
    pub files: Vec<String>,
    pub pruned: Vec<String>,
    pruned_directories: Vec<String>,
    checked: Vec<PathBuf>,
    scope: PathBuf,
}
fn path_key(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/").to_lowercase()
    } else {
        path.to_owned()
    }
}
fn contains(parent: &str, path: &str) -> bool {
    path == parent || path.starts_with(&format!("{parent}/"))
}
impl CheckpointScan {
    pub fn excluded_pathspecs(&self) -> Vec<String> {
        self.pruned_directories
            .iter()
            .map(|path| {
                format!(
                    ":(top,exclude,literal{}){path}",
                    if cfg!(windows) { ",icase" } else { "" }
                )
            })
            .collect()
    }
    pub async fn recheck(&self, cancellation: &CancellationToken) -> Result<(), GitError> {
        if !self.exclusions.is_empty() {
            return Err(GitError::UnsafeRestore(
                "Checkpoint workspace contains a reachable link outside the scope.".into(),
            ));
        }
        let paths = self.checked.clone();
        let scope = self.scope.clone();
        let cancellation = cancellation.clone();
        tokio::task::spawn_blocking(move || {
            for path in paths {
                if cancellation.is_cancelled() {
                    return Err(t3_process::ProcessError::Cancelled.into());
                }
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if is_link(&path, &metadata)? => {
                        let safe_file = metadata.file_type().is_symlink()
                            && std::fs::canonicalize(&path)
                                .is_ok_and(|target| target.starts_with(&scope) && target.is_file());
                        if !safe_file {
                            return Err(GitError::UnsafeRestore(
                                "Checkpoint path became a link outside the scope.".into(),
                            ));
                        }
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| GitError::InvalidOutput("checkpoint containment task failed".into()))?
    }
}
impl Git {
    pub(crate) async fn checkpoint_scan(
        &self,
        tree: Option<&str>,
        tracked: &[String],
        environment: &t3_process::Environment,
    ) -> Result<CheckpointScan, GitError> {
        let mut args = vec![
            "ls-files".to_owned(),
            "--cached".into(),
            "--others".into(),
            "--ignored".into(),
            "--exclude-standard".into(),
            "--directory".into(),
            "--full-name".into(),
            "-t".into(),
            "-z".into(),
        ];
        if let Some(tree) = tree {
            args.push(format!("--with-tree={tree}"));
        }
        args.extend(["--".into(), ".".into()]);
        let output = self
            .execute(
                &args,
                ExecuteOptions {
                    capture: true,
                    env: environment.clone(),
                    max_output_bytes: 16 * 1024 * 1024,
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?;
        let mut ignored = BTreeSet::new();
        let mut tracked_ignored = BTreeSet::new();
        let mut directories = Vec::new();
        for entry in output
            .stdout_text()
            .split('\0')
            .filter(|entry| !entry.is_empty())
        {
            let path = entry
                .get(2..)
                .ok_or_else(|| GitError::InvalidOutput("ignored checkpoint path".into()))?
                .trim_end_matches('/');
            if entry.starts_with("? ") {
                if entry.ends_with('/') {
                    directories.push(path.to_owned());
                } else {
                    ignored.insert(path_key(path));
                }
            } else {
                tracked_ignored.insert(path_key(path));
            }
        }
        if !directories.is_empty() {
            // ls-files --directory also reports unignored parents of ignored entries.
            let root = self
                .cwd()
                .ancestors()
                .find(|path| path.join(".git").exists())
                .ok_or_else(|| {
                    GitError::Unavailable("Checkpoint workspace has no repository root.".into())
                })?;
            let filtered = self
                .at(root)
                .execute(
                    &["check-ignore", "--no-index", "-z", "--stdin"],
                    ExecuteOptions {
                        capture: true,
                        env: environment.clone(),
                        stdin: Some(format!("{}\0", directories.join("\0")).into_bytes()),
                        allow_nonzero: true,
                        max_output_bytes: 16 * 1024 * 1024,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?;
            if !matches!(filtered.code, Some(0 | 1)) {
                return Err(self.vcs_exit("GitVcsDriver.filterIgnoredDirectories", 4, &filtered));
            }
            ignored.extend(
                filtered
                    .stdout_text()
                    .split('\0')
                    .filter(|path| !path.is_empty())
                    .map(path_key),
            );
        }
        let cwd = self.cwd().to_owned();
        let tracked = tracked.to_vec();
        let token = self.cancellation.clone();
        tokio::task::spawn_blocking(move || {
            scan_checkpoint(&cwd, &ignored, &tracked_ignored, &tracked, &token)
        })
        .await
        .map_err(|_| GitError::InvalidOutput("checkpoint scan task failed".into()))?
    }
}
fn scan_checkpoint(
    cwd: &Path,
    ignored: &BTreeSet<String>,
    tracked_ignored: &BTreeSet<String>,
    tracked: &[String],
    cancellation: &CancellationToken,
) -> Result<CheckpointScan, GitError> {
    let absolute = std::path::absolute(cwd)?;
    let mut checked = BTreeSet::new();
    for ancestor in absolute.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(ancestor, &metadata)? => {
                return Err(GitError::UnsafeRestore(
                    "Checkpoint scope traverses a link.".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        checked.insert(ancestor.to_owned());
    }
    let lexical_root = absolute
        .ancestors()
        .find(|path| path.join(".git").exists())
        .ok_or_else(|| {
            GitError::Unavailable("Checkpoint workspace has no repository root.".into())
        })?
        .to_owned();
    let root = std::fs::canonicalize(&lexical_root)?;
    let scope = resolve(&absolute)?;
    if !scope.starts_with(&root) {
        return Err(GitError::UnsafeRestore(
            "Checkpoint scope resolves outside the worktree.".into(),
        ));
    }
    let relative = |path: &Path| -> Result<String, GitError> {
        let path = path
            .strip_prefix(&lexical_root)
            .map_err(|_| {
                GitError::UnsafeRestore("Checkpoint path is outside the worktree.".into())
            })?
            .to_str()
            .ok_or_else(|| GitError::Unsupported("Checkpoint path is not UTF-8.".into()))?;
        Ok(if cfg!(windows) {
            path.replace('\\', "/")
        } else {
            path.to_owned()
        })
    };
    let mut result = CheckpointScan {
        exclusions: vec![],
        files: vec![],
        pruned: vec![],
        pruned_directories: vec![],
        checked: vec![],
        scope,
    };
    let mut directories = vec![absolute.clone()];
    let mut repositories = Vec::new();
    while let Some(directory) = directories.pop() {
        if cancellation.is_cancelled() {
            return Err(t3_process::ProcessError::Cancelled.into());
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            if cancellation.is_cancelled() {
                return Err(t3_process::ProcessError::Cancelled.into());
            }
            let entry = entry?;
            if entry.file_name() == ".git" {
                if directory != absolute {
                    repositories.push(format!("{}/", relative(&directory)?));
                }
                continue;
            }
            let path = entry.path();
            let name = relative(&path)?;
            let key = path_key(&name);
            if ignored.contains(&key)
                && !tracked_ignored.contains(&key)
                && !tracked_ignored
                    .range(format!("{key}/")..)
                    .next()
                    .is_some_and(|tracked| contains(&key, tracked))
            {
                let metadata = std::fs::symlink_metadata(&path)?;
                if metadata.is_dir()
                    || is_link(&path, &metadata)?
                        && std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir())
                {
                    result.pruned_directories.push(name.clone());
                }
                result.pruned.push(name);
                continue;
            }
            checked.insert(path.clone());
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if is_link(&path, &metadata)? {
                let safe_file = metadata.file_type().is_symlink()
                    && std::fs::canonicalize(&path)
                        .is_ok_and(|target| target.starts_with(&result.scope) && target.is_file());
                if safe_file {
                    result.files.push(name);
                } else {
                    result.exclusions.push(name);
                }
            } else if metadata.is_dir() {
                directories.push(path);
            } else {
                result.files.push(name);
            }
        }
    }
    result.files.retain(|path| {
        !repositories
            .iter()
            .any(|repository| path.starts_with(repository))
    });
    repositories.retain(|path| {
        !result
            .exclusions
            .iter()
            .any(|excluded| path.starts_with(&format!("{excluded}/")))
    });
    result.files.extend(repositories);
    for name in tracked {
        let path = lexical_root.join(name);
        for ancestor in path
            .ancestors()
            .take_while(|path| path.starts_with(&absolute))
        {
            checked.insert(ancestor.to_owned());
        }
    }
    result.checked = checked.into_iter().collect();
    Ok(result)
}

pub fn is_restore_isolated(safety: &RestoreSafety) -> Result<bool, GitError> {
    let Some(worktree) = &safety.worktree_path else {
        return Ok(false);
    };
    let cwd = comparison_path(&safety.scope_cwd)?;
    if cwd != comparison_path(worktree)? {
        return Ok(false);
    }
    for other in &safety.participants {
        if other.thread_id == safety.thread_id || other.deleted {
            continue;
        }
        if other.side_chat
            && !other.active_run
            && !other.has_mutating_turn_items
            && other.worktree_path == safety.worktree_path
        {
            continue;
        }
        let mut paths: Vec<_> = other
            .worktree_path
            .iter()
            .chain(other.scope_paths.iter())
            .collect();
        paths.extend(
            other
                .provider_sessions
                .iter()
                .filter(|session| !session.stopped && !session.supports_multiple_provider_threads)
                .map(|session| &session.cwd),
        );
        if other.worktree_path.is_none() {
            let Some(root) = &other.project_root else {
                return Ok(false);
            };
            paths.push(root);
        }
        for path in paths {
            let path = comparison_path(path)?;
            if cwd.starts_with(&path) || path.starts_with(&cwd) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
impl Git {
    pub async fn restore_checkpoint_isolated(
        &self,
        reference: &str,
        fallback_to_head: bool,
        safety: &RestoreSafety,
    ) -> Result<bool, GitError> {
        if comparison_path(self.cwd())? != comparison_path(&safety.scope_cwd)?
            || !is_restore_isolated(safety)?
        {
            return Err(GitError::UnsafeRestore(
                SHARED_WORKSPACE_RESTORE_MESSAGE.into(),
            ));
        }
        self.restore_checkpoint(reference, fallback_to_head).await
    }
}

#[cfg(all(test, unix))]
mod path_tests {
    use super::*;
    #[test]
    fn realpath_resolves_symlinks_before_parent_components_and_missing_suffixes()
    -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::fs::create_dir_all(root.join("deep/target"))?;
        std::os::unix::fs::symlink(root.join("deep/target"), root.join("link"))?;
        assert_eq!(
            resolve(&root.join("link/.."))?,
            std::fs::canonicalize(root.join("deep"))?
        );
        assert_eq!(
            resolve(&root.join("link/../missing"))?,
            std::fs::canonicalize(root.join("deep"))?.join("missing")
        );
        Ok(())
    }
}
