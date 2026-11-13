use crate::{Git, GitError};
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
fn lexical_absolute(path: &Path) -> Result<PathBuf, std::io::Error> {
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

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(crate) fn checkpoint_exclusions(cwd: &Path) -> Result<Vec<String>, GitError> {
    Ok(scan_checkpoint(cwd, false)?.0)
}

pub(crate) fn checkpoint_files(cwd: &Path) -> Result<Vec<String>, GitError> {
    Ok(scan_checkpoint(cwd, true)?.1)
}

fn scan_checkpoint(
    cwd: &Path,
    collect_files: bool,
) -> Result<(Vec<String>, Vec<String>), GitError> {
    let absolute = std::path::absolute(cwd)?;
    for ancestor in absolute.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(GitError::UnsafeRestore(
                    "Checkpoint scope traverses a symlink or reparse point.".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let root = absolute
        .ancestors()
        .find(|path| path.join(".git").exists())
        .ok_or_else(|| {
            GitError::Unavailable("Checkpoint workspace has no repository root.".into())
        })?;
    let lexical_root = root.to_owned();
    let root = std::fs::canonicalize(root)?;
    let scope = resolve(&absolute)?;
    if !scope.starts_with(&root) {
        return Err(GitError::UnsafeRestore(
            "Checkpoint scope resolves outside the worktree.".into(),
        ));
    }
    // Never traverse directory links, including internal links that can contain cycles or other links.
    let mut directories = vec![absolute.clone()];
    let mut exclusions = Vec::new();
    let mut files = Vec::new();
    let mut repositories = Vec::new();
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
    while let Some(directory) = directories.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_name() == ".git" {
                if collect_files && directory != absolute {
                    repositories.push(format!("{}/", relative(&directory)?));
                }
                continue;
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if is_link(&metadata) {
                let internal_file = metadata.file_type().is_symlink()
                    && std::fs::canonicalize(&path)
                        .is_ok_and(|target| target.starts_with(&root) && target.is_file());
                if internal_file {
                    if collect_files {
                        files.push(relative(&path)?);
                    }
                    continue;
                }
                exclusions.push(relative(&path)?);
            } else if metadata.is_dir() {
                directories.push(path);
            } else if collect_files {
                files.push(relative(&path)?);
            }
        }
    }
    if collect_files {
        files.retain(|path| {
            !repositories
                .iter()
                .any(|repository| path.starts_with(repository))
        });
        repositories.retain(|path| {
            !exclusions
                .iter()
                .any(|excluded| path.starts_with(&format!("{excluded}/")))
        });
        files.extend(repositories);
    }
    Ok((exclusions, files))
}

pub(crate) fn ensure_checkpoint_contained(cwd: &Path) -> Result<(), GitError> {
    if !checkpoint_exclusions(cwd)?.is_empty() {
        return Err(GitError::UnsafeRestore("Checkpoint workspace contains a symlink or reparse point; remove it before restoring files.".into()));
    }
    Ok(())
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
