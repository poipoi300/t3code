use crate::{Error, Result, blocking, io_error, leases::WorkspaceLeases, paths};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

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
    ) -> Result<()> {
        let Some(path) = worktree_path else {
            return Ok(());
        };
        let normalized = paths::absolute(path)?;
        if !path.is_absolute() || path != normalized {
            return Ok(());
        }
        let _lease = self.leases.acquire(&normalized).await?;
        let Some(project) = project_root else {
            return Ok(());
        };
        let Some(branch) = branch else {
            let (project, path, scratch) = (
                project.to_owned(),
                path.to_owned(),
                self.scratch_root.clone(),
            );
            // Scratch cleanup is recoverable only inside that project's configured scratch root.
            let _ = blocking(move || {
                if fs::symlink_metadata(&path).is_ok()
                    || path.parent() != Some(paths::absolute(&project)?.as_path())
                {
                    return Ok(());
                }
                let scratch = paths::absolute(&scratch)?;
                let real = paths::canonicalize(&scratch).unwrap_or_else(|_| scratch.clone());
                if path.parent() != Some(scratch.as_path()) && path.parent() != Some(real.as_path())
                {
                    return Ok(());
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
                Ok(())
            })
            .await;
            return Ok(());
        };
        let (target, roots) = (path.to_owned(), self.managed_roots.clone());
        let prepare = blocking(move || {
            let exists = match fs::symlink_metadata(&target) {
                Ok(_) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => return Ok(false),
            };
            let leftover = exists && is_leftover(&target);
            if exists && !leftover {
                return Ok(false);
            }
            let candidate = paths::nearest_real(&target)?.0;
            let managed = roots.iter().any(|root| {
                let Ok((root, _)) = paths::nearest_real(root) else {
                    return false;
                };
                root.parent().is_some()
                    && candidate.strip_prefix(&root).is_ok_and(|relative| {
                        relative.components().count() == 2
                            && relative
                                .components()
                                .all(|p| matches!(p, std::path::Component::Normal(_)))
                    })
            });
            if !managed {
                return Ok(false);
            }
            if leftover {
                fs::remove_dir_all(&target)
                    .map_err(|e| io_error("remove-worktree-leftovers", &target, e))?;
            }
            Ok(true)
        })
        .await;
        let result = async {
            if !prepare? {
                return Ok(());
            }
            let git = t3_git::Git::new(project);
            git.prune_worktrees().await?;
            if git
                .list_local_branch_names()
                .await?
                .iter()
                .any(|b| b == branch)
            {
                git.add_worktree(t3_git::WorktreeOptions::new(path, branch))
                    .await?;
                return Ok(());
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
            let mut options = t3_git::WorktreeOptions::new(path, reference);
            options.new_branch = Some(branch.into());
            git.add_worktree(options).await?;
            if let Some(base) =
                base_ref.and_then(|base| base.strip_prefix("refs/heads/").map(str::to_owned))
            {
                let _ = git
                    .write_config_value(&format!("branch.{branch}.t3-merge-base"), &base)
                    .await;
            }
            Ok(())
        }
        .await;
        result.map_err(|error| match error {
            Error::WorktreeRecreateNoBase { .. } => error,
            _ => Error::WorktreeRecreate {
                path: path.into(),
                branch: branch.into(),
                source: Box::new(error),
            },
        })
    }
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
