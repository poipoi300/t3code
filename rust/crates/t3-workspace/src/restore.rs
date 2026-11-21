use crate::{Error, Result, blocking, io_error, leases::WorkspaceLeases, paths};
use cap_fs_ext::DirExt;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreOutcome {
    Unchanged,
    ScratchCreated,
    CheckoutCreated,
}

pub struct WorktreeRestore {
    pub leases: Arc<WorkspaceLeases>,
    pub managed_roots: Vec<PathBuf>,
    pub scratch_root: PathBuf,
}
impl WorktreeRestore {
    pub async fn restore(
        &self,
        project_root: Option<&Path>,
        worktree_path: Option<&Path>,
        branch: Option<&str>,
        // Resolve the project binding only when a managed checkout needs recreation.
        source: impl Future<Output = Result<(t3_git::Git, t3_git::RepositoryBinding)>>,
    ) -> Result<RestoreOutcome> {
        use RestoreOutcome::{CheckoutCreated, ScratchCreated, Unchanged};
        let Some(path) = worktree_path else {
            return Ok(Unchanged);
        };
        let normalized = paths::absolute(path)?;
        if !path.is_absolute() || path != normalized {
            return Ok(Unchanged);
        }
        let _lease = self.leases.acquire(&normalized).await?;
        let Some(project) = project_root else {
            return Ok(Unchanged);
        };
        let Some(branch) = branch else {
            let (project, path, scratch) = (
                project.to_owned(),
                path.to_owned(),
                self.scratch_root.clone(),
            );
            // Scratch cleanup is recoverable only inside that project's configured scratch root.
            return Ok(blocking(move || {
                if fs::symlink_metadata(&path).is_ok()
                    || path.parent() != Some(paths::absolute(&project)?.as_path())
                {
                    return Ok(Unchanged);
                }
                let scratch = paths::absolute(&scratch)?;
                let real = paths::canonicalize(&scratch).unwrap_or_else(|_| scratch.clone());
                if path.parent() != Some(scratch.as_path()) && path.parent() != Some(real.as_path())
                {
                    return Ok(Unchanged);
                }
                fs::create_dir_all(&project)
                    .map_err(|e| io_error("recreate-scratch-root", &project, e))?;
                let scope = crate::scoped::ScopedDir::open(&project)?;
                let name = path
                    .file_name()
                    .ok_or_else(|| Error::PathEscape(path.clone()))?;
                scope
                    .dir
                    .create_dir(name)
                    .map_err(|e| io_error("recreate-scratch-folder", &path, e))?;
                Ok(ScratchCreated)
            })
            .await
            .unwrap_or(Unchanged));
        };
        let (target, roots) = (path.to_owned(), self.managed_roots.clone());
        let target_state = blocking(move || {
            let exists = match fs::symlink_metadata(&target) {
                Ok(_) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => return Ok(None),
            };
            let leftover = exists && is_leftover(&target);
            if exists && !leftover {
                return Ok(None);
            }
            let candidate = paths::nearest_real(&target)?.0;
            let Some((parent, name)) = target.parent().zip(target.file_name()) else {
                return Ok(None);
            };
            let destination = paths::nearest_real(parent)?.0.join(name);
            let managed = roots.iter().any(|root| {
                let Ok((root, _)) = paths::nearest_real(root) else {
                    return false;
                };
                root.parent().is_some()
                    && [&candidate, &destination].into_iter().all(|p| {
                        p.strip_prefix(&root)
                            .is_ok_and(|relative| relative.components().count() == 2)
                    })
            });
            if !managed {
                return Ok(None);
            }
            Ok(Some((candidate, destination, leftover)))
        })
        .await;
        let recreate_error = |error| match error {
            Error::WorktreeRecreateNoBase { .. } => error,
            _ => Error::WorktreeRecreate {
                path: path.into(),
                branch: branch.into(),
                source: Box::new(error),
            },
        };
        let Some((original, destination, leftover)) = target_state.map_err(recreate_error)? else {
            return Ok(Unchanged);
        };
        // A refused project binding leaves even disposable leftovers intact.
        let (git, binding) = source.await?;
        if git.cwd() != project {
            return Err(Error::Invalid(
                "worktree recovery source differs from the project".into(),
            ));
        }
        let git = git.with_repository_binding(binding).await?;
        let result = async {
            verify_target(path, &original)?;
            if leftover {
                let (named, real) = (path.to_owned(), original.clone());
                let removed = blocking(move || remove_leftovers(&named, &real)).await?;
                if !removed {
                    return Ok(Unchanged);
                }
            }
            git.prune_worktrees().await?;
            if git
                .list_local_branch_names()
                .await?
                .iter()
                .any(|b| b == branch)
            {
                verify_target(path, &destination)?;
                git.add_worktree(t3_git::WorktreeOptions::new(&destination, branch))
                    .await?;
                return Ok(CheckoutCreated);
            }
            let remote = git.primary_remote_name().await.ok().flatten();
            let mut reference = None;
            if let Some(remote) = &remote {
                let pushed = format!("refs/remotes/{remote}/{branch}");
                if git.has_commit(&pushed).await? {
                    reference = Some(pushed);
                }
            }
            let mut base_ref = None;
            if reference.is_none() {
                if let Some(current) = git.current_branch().await? {
                    let current = format!("refs/heads/{current}");
                    if git.has_commit(&current).await? {
                        base_ref = Some(current.clone());
                        reference = Some(current);
                    }
                }
                if reference.is_none()
                    && let Some(remote) = &remote
                {
                    let head = format!("refs/remotes/{remote}/HEAD");
                    if git.has_commit(&head).await? {
                        reference =
                            Some(git.resolve_remote_tracking_commit("HEAD", remote).await?.0);
                        base_ref = Some(head);
                    }
                }
                if reference.is_none() && git.has_commit("HEAD").await? {
                    reference = Some("HEAD".into());
                }
            }
            let reference = reference.ok_or_else(|| Error::WorktreeRecreateNoBase {
                path: path.into(),
                branch: branch.into(),
            })?;
            verify_target(path, &destination)?;
            let mut options = t3_git::WorktreeOptions::new(&destination, reference);
            options.new_branch = Some(branch.into());
            git.add_worktree(options).await?;
            if let Some(base) =
                base_ref.and_then(|base| base.strip_prefix("refs/heads/").map(str::to_owned))
            {
                let _ = git
                    .write_config_value(&format!("branch.{branch}.t3-merge-base"), &base)
                    .await;
            }
            Ok(CheckoutCreated)
        }
        .await;
        result.map_err(recreate_error)
    }
}
fn verify_target(named: &Path, expected: &Path) -> Result<()> {
    if paths::nearest_real(named)?.0 != expected || paths::nearest_real(expected)?.0 != expected {
        return Err(Error::PathEscape(named.into()));
    }
    Ok(())
}

fn remove_leftovers(named: &Path, expected: &Path) -> Result<bool> {
    let parent = named
        .parent()
        .ok_or_else(|| Error::PathEscape(named.into()))?;
    let parent = crate::scoped::ScopedDir::open(parent)?;
    let name = named
        .file_name()
        .ok_or_else(|| Error::PathEscape(named.into()))?;
    verify_target(named, expected)?;
    // Unlink a directory link itself. Its target's contents must stay untouched.
    let kind = parent
        .dir
        .symlink_metadata(name)
        .map_err(|e| io_error("stat-leftovers", named, e))?;
    if kind.is_symlink() {
        if !is_leftover(named) {
            return Ok(false);
        }
        parent
            .dir
            .remove_file_or_symlink(name)
            .map_err(|e| io_error("unlink-leftovers", named, e))?;
        return Ok(true);
    }
    let mut dirs = parent.walk(Path::new(name), false)?;
    let dir = dirs.pop().ok_or_else(|| Error::PathEscape(named.into()))?;
    if parent.root.join(name) != expected {
        return Err(Error::PathEscape(named.into()));
    }
    let entries = dir
        .entries()
        .map_err(|e| io_error("read-leftovers", named, e))?;
    let mut disposable = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_error("read-leftovers", named, e))?;
        let file = entry.file_name();
        if !matches!(file.to_str(), Some("node_modules" | ".next"))
            || !fs::metadata(named.join(&file)).is_ok_and(|m| m.is_dir())
        {
            return Ok(false);
        }
        disposable.push(file);
    }
    for file in disposable {
        let metadata = dir
            .symlink_metadata(&file)
            .map_err(|e| io_error("stat-leftover", named, e))?;
        if metadata.is_symlink() {
            dir.remove_file_or_symlink(file)
        } else {
            dir.remove_dir_all(file)
        }
        .map_err(|e| io_error("remove-worktree-leftovers", named, e))?;
    }
    drop(dir);
    // An unexpected file added during cleanup prevents removal of the folder.
    parent
        .dir
        .remove_dir(name)
        .map_err(|e| io_error("remove-worktree-leftovers", named, e))?;
    Ok(true)
}

fn is_leftover(path: &Path) -> bool {
    if fs::symlink_metadata(path.join(".git")).is_ok() {
        return false;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.into_iter().all(|entry| {
        entry.is_ok_and(|entry| {
            matches!(entry.file_name().to_str(), Some("node_modules" | ".next"))
                && fs::metadata(entry.path()).is_ok_and(|info| info.is_dir())
        })
    })
}
