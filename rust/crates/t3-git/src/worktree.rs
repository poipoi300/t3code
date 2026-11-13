#[cfg(windows)]
use crate::env;
use crate::{ExecuteOptions, Git, GitError, OutputMode, checkpoint::validate_revision};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorktreeSubmodules {
    #[default]
    None,
    Recursive,
}
#[derive(Debug, Clone)]
pub struct WorktreeOptions {
    pub reference: String,
    pub new_branch: Option<String>,
    pub path: Option<PathBuf>,
    /// Absolute parent for deterministic worktree names when `path` is absent.
    pub directory: Option<PathBuf>,
    pub submodules: WorktreeSubmodules,
}
impl WorktreeOptions {
    pub fn new(path: impl Into<PathBuf>, reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            new_branch: None,
            path: Some(path.into()),
            directory: None,
            submodules: WorktreeSubmodules::None,
        }
    }
}
impl Git {
    pub async fn list_worktrees(&self) -> Result<Vec<Worktree>, GitError> {
        let options = ExecuteOptions {
            allow_nonzero: true,
            max_output_bytes: 16 * 1024 * 1024,
            output_mode: OutputMode::Error,
            ..Default::default()
        };
        let output = self
            .execute(&["worktree", "list", "--porcelain", "-z"], options)
            .await?;
        let text = if output.code == Some(0) {
            output.stdout_text()
        } else {
            self.text(&["worktree", "list", "--porcelain"])
                .await?
                .replace('\n', "\0")
        };
        let mut worktrees = Vec::new();
        let mut current: Option<Worktree> = None;
        for field in text.split('\0') {
            if let Some(path) = field.strip_prefix("worktree ") {
                if let Some(previous) = current.take() {
                    worktrees.push(previous);
                }
                current = Some(Worktree {
                    path: path.into(),
                    ..Default::default()
                });
            } else if field.is_empty() {
                if let Some(previous) = current.take() {
                    worktrees.push(previous);
                }
            } else if let Some(current) = &mut current {
                if let Some(head) = field.strip_prefix("HEAD ") {
                    current.head = Some(head.into());
                } else if let Some(branch) = field.strip_prefix("branch ") {
                    current.branch = Some(branch.into());
                } else if field == "bare" {
                    current.bare = true;
                } else if field == "detached" {
                    current.detached = true;
                } else if field == "locked" || field.starts_with("locked ") {
                    current.locked = Some(field.strip_prefix("locked ").unwrap_or("").into());
                } else if field == "prunable" || field.starts_with("prunable ") {
                    current.prunable = Some(field.strip_prefix("prunable ").unwrap_or("").into());
                }
            }
        }
        if let Some(current) = current {
            worktrees.push(current);
        }
        Ok(worktrees)
    }
    pub async fn add_worktree(&self, options: WorktreeOptions) -> Result<PathBuf, GitError> {
        let result = self.add_worktree_inner(options).await;
        self.invalidate_caches().await;
        result
    }
    async fn add_worktree_inner(&self, options: WorktreeOptions) -> Result<PathBuf, GitError> {
        if options.reference != "-" {
            validate_revision(&options.reference)?;
        }
        if let Some(branch) = &options.new_branch {
            validate_revision(branch)?;
        }
        let target_branch = options.new_branch.as_ref().unwrap_or(&options.reference);
        let path = if let Some(path) = &options.path {
            absolute_from(&self.cwd, path)?
        } else {
            let parent = options.directory.as_ref().ok_or_else(|| {
                GitError::InvalidArgument(
                    "a worktree path or absolute directory is required".into(),
                )
            })?;
            if !parent.is_absolute() || parent.parent().is_none() {
                return Err(GitError::InvalidArgument(
                    "worktree directory must be absolute and not a drive root".into(),
                ));
            }
            let repository = self
                .detect_repository()
                .await?
                .ok_or_else(|| GitError::Unavailable("not a Git repository".into()))?;
            let identity = format!("{}\0{target_branch}", repository.common_dir.display());
            let digest = Sha256::digest(identity.as_bytes());
            let hash: String = digest[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let mut prefix = String::new();
            for character in target_branch.replace('/', "-").chars() {
                if prefix.len() + character.len_utf8() > 23 {
                    break;
                }
                prefix.push(character);
            }
            let cwd = std::path::absolute(&self.cwd)?;
            let name = cwd
                .file_name()
                .ok_or_else(|| GitError::InvalidArgument("repository has no basename".into()))?;
            let base = parent.join(name).join(format!("{prefix}-{hash}"));
            let mut path = base.clone();
            let mut suffix = 2;
            while path.try_exists()? {
                let mut value = base.as_os_str().to_owned();
                value.push(format!("-{suffix}"));
                path = value.into();
                suffix += 1;
            }
            path
        };
        if let Some(branch) = &options.new_branch {
            let start = if options.reference == "-" {
                "@{-1}"
            } else {
                &options.reference
            };
            self.text(&["branch", "--no-track", "--", branch, start])
                .await?;
            let start_ref = self
                .execute(
                    &["rev-parse", "--symbolic-full-name", start],
                    ExecuteOptions {
                        allow_nonzero: true,
                        ..Default::default()
                    },
                )
                .await?;
            let start_ref = start_ref.stdout_text();
            if start_ref.trim().starts_with("refs/remotes/") {
                let upstream = format!("--set-upstream-to={}", start_ref.trim());
                let _ = self.text(&["branch", &upstream, "--", branch]).await;
            }
        }
        let checkout_workers = self
            .execute(
                &["config", "--get", "checkout.workers"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        let workers = checkout_workers.stdout_text();
        let workers = if workers.trim().is_empty() {
            "0"
        } else {
            workers.trim()
        };
        let config = format!("checkout.workers={workers}");
        let args: Vec<std::ffi::OsString> = vec![
            "-c".into(),
            config.into(),
            "worktree".into(),
            "add".into(),
            "--".into(),
            path.as_os_str().to_owned(),
            target_branch.into(),
        ];
        let result = self
            .execute(
                &args,
                ExecuteOptions {
                    timeout: None,
                    append_truncation_marker: true,
                    ..Default::default()
                },
            )
            .await;
        if let Err(error) = result {
            if let Some(branch) = &options.new_branch {
                let _ = self.text(&["branch", "-D", "--", branch]).await;
            }
            return Err(error);
        }
        if options.submodules == WorktreeSubmodules::Recursive
            && path.join(".gitmodules").try_exists()?
        {
            self.at(&path)
                .execute(
                    &["submodule", "update", "--init", "--recursive"],
                    ExecuteOptions {
                        timeout: None,
                        ..Default::default()
                    },
                )
                .await?;
        }
        Ok(path)
    }
    pub async fn prune_worktrees(&self) -> Result<(), GitError> {
        let result = self
            .execute(
                &["worktree", "prune"],
                ExecuteOptions {
                    timeout: Some(Duration::from_secs(15)),
                    ..Default::default()
                },
            )
            .await;
        self.invalidate_caches().await;
        result.map(|_| ())
    }
    pub async fn remove_worktree(&self, path: &Path, force: bool) -> Result<(), GitError> {
        let result = self.remove_worktree_inner(path, force).await;
        self.invalidate_caches().await;
        result
    }
    async fn remove_worktree_inner(&self, path: &Path, force: bool) -> Result<(), GitError> {
        let target = absolute_from(&self.cwd, path)?;
        #[cfg(windows)]
        if target.join(".git").is_file() {
            return self.remove_windows_worktree(&target, force).await;
        }
        self.git_remove_worktree(&target, force, false).await
    }
    async fn git_remove_worktree(
        &self,
        path: &Path,
        force: bool,
        double_force: bool,
    ) -> Result<(), GitError> {
        let mut args: Vec<std::ffi::OsString> = vec![
            "-c".into(),
            "status.showUntrackedFiles=normal".into(),
            "worktree".into(),
            "remove".into(),
        ];
        if force {
            args.push("--force".into());
        }
        if double_force {
            args.push("--force".into());
        }
        args.extend(["--".into(), path.as_os_str().to_owned()]);
        let result = self.execute(&args, removal_options()).await?;
        if result.code == Some(0) {
            return Ok(());
        }
        if !path.try_exists()? && already_gone(&result.stderr_text()) {
            self.prune_worktrees().await?;
            return Ok(());
        }
        Err(self.vcs_exit("GitVcsDriver.removeWorktree", args.len(), &result))
    }
    #[cfg(windows)]
    async fn remove_windows_worktree(&self, target: &Path, force: bool) -> Result<(), GitError> {
        let git_file = target.join(".git");
        let not_worktree =
            || GitError::InvalidArgument(format!("'{}' is not a working tree", target.display()));
        let admin = self
            .at(target)
            .text(&["rev-parse", "--absolute-git-dir"])
            .await
            .map_err(|_| not_worktree())?;
        let admin = PathBuf::from(admin.trim());
        let repository = self.detect_repository().await?.ok_or_else(not_worktree)?;
        let expected_parent = std::fs::canonicalize(repository.common_dir.join("worktrees"))?;
        let actual_parent = std::fs::canonicalize(admin.parent().ok_or_else(not_worktree)?)?;
        let back = std::fs::read_to_string(admin.join("gitdir"))?;
        if actual_parent != expected_parent
            || std::fs::canonicalize(admin.join(back.trim()))? != std::fs::canonicalize(&git_file)?
        {
            return Err(not_worktree());
        }
        if admin.join("locked").try_exists()? {
            return Err(GitError::Unavailable(
                "cannot remove a locked working tree; unlock first".into(),
            ));
        }
        if !force {
            if admin.join("modules").try_exists()? {
                return Err(GitError::Unavailable(
                    "working trees containing submodules cannot be moved or removed".into(),
                ));
            }
            let bound_env = env(&[
                ("GIT_DIR", &git_file.to_string_lossy()),
                ("GIT_WORK_TREE", &target.to_string_lossy()),
            ]);
            let index = self
                .at(target)
                .execute(
                    &["ls-files", "--stage", "-z"],
                    ExecuteOptions {
                        env: bound_env.clone(),
                        max_output_bytes: 64 * 1024 * 1024,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?;
            for entry in index
                .stdout_text()
                .split('\0')
                .filter(|entry| entry.starts_with("160000 "))
            {
                if let Some((_, path)) = entry.split_once('\t')
                    && target.join(path).join(".git").try_exists()?
                {
                    return Err(GitError::Unavailable(
                        "working trees containing submodules cannot be moved or removed".into(),
                    ));
                }
            }
            let status = self
                .at(target)
                .execute(
                    &[
                        "-c",
                        "status.showUntrackedFiles=normal",
                        "status",
                        "--porcelain",
                        "--ignore-submodules=none",
                    ],
                    ExecuteOptions {
                        env: bound_env,
                        max_output_bytes: 64 * 1024 * 1024,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?;
            if !status.stdout.is_empty() {
                return Err(GitError::Unavailable(
                    "worktree contains modified or untracked files; use force to delete it".into(),
                ));
            }
        }
        let contents = std::fs::read(&git_file)?;
        if let Err(error) = std::fs::remove_dir_all(target) {
            let _ = std::fs::write(git_file, contents);
            return Err(error.into());
        }
        self.git_remove_worktree(target, true, true).await
    }
}
fn absolute_from(cwd: &Path, path: &Path) -> Result<PathBuf, std::io::Error> {
    std::path::absolute(if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    })
}

fn removal_options() -> ExecuteOptions {
    ExecuteOptions {
        allow_nonzero: true,
        timeout: Some(Duration::from_secs(300)),
        ..Default::default()
    }
}
fn already_gone(stderr: &str) -> bool {
    stderr.contains("is not a working tree") || stderr.contains("cannot remove working tree")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removal_has_five_minutes_and_recognizes_both_missing_tree_diagnostics() {
        for stderr in [
            "fatal: path is not a working tree",
            "fatal: cannot remove working tree: No such file or directory",
        ] {
            assert!(already_gone(stderr), "{stderr}");
        }
        assert!(!already_gone(
            "fatal: worktree contains modified or untracked files"
        ));
        assert_eq!(removal_options().timeout, Some(Duration::from_secs(300)));
    }
}
