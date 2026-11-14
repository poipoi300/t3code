use crate::{ExecuteOptions, Git};
use std::{io::Read, path::Path, time::Duration};

const MAX_GITFILE_BYTES: u64 = 64 * 1024;

fn strictly_inside(parent: &Path, child: &Path) -> bool {
    #[cfg(windows)]
    let (parent, child) = (
        std::path::PathBuf::from(parent.to_string_lossy().to_lowercase()),
        std::path::PathBuf::from(child.to_string_lossy().to_lowercase()),
    );
    child != parent && child.starts_with(parent)
}

async fn gitfile_target(root: &Path) -> Option<String> {
    let path = root.join(".git");
    let metadata = tokio::fs::metadata(&path).await.ok()?;
    if !metadata.is_file() || metadata.len() > MAX_GITFILE_BYTES {
        return None;
    }
    let read = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(path).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut contents = String::new();
        file.take(MAX_GITFILE_BYTES + 1)
            .read_to_string(&mut contents)
            .ok()?;
        if contents.len() as u64 > MAX_GITFILE_BYTES {
            return None;
        }
        let first = contents.split('\n').next()?;
        let target = first.strip_prefix("gitdir:")?.trim();
        (!target.is_empty()).then(|| target.to_owned())
    });
    tokio::time::timeout(Duration::from_secs(30), read)
        .await
        .ok()?
        .ok()?
}

impl Git {
    /// Conservative metadata-only container probe; inconclusive checks accept the folder.
    pub async fn is_worktree_container(&self) -> bool {
        self.probe_worktree_container().await.unwrap_or(false)
    }
    async fn probe_worktree_container(&self) -> Option<bool> {
        let target = gitfile_target(self.cwd()).await?;
        let root = tokio::fs::canonicalize(self.cwd()).await.ok()?;
        let git_dir = tokio::fs::canonicalize(self.cwd().join(target))
            .await
            .ok()?;
        if !strictly_inside(&root, &git_dir) {
            return Some(false);
        }
        let bare = self
            .container_probe_git(&["rev-parse", "--is-bare-repository"])
            .await?;
        if bare.trim() != "true" {
            self.container_probe_git(&["rev-parse", "--quiet", "--verify", "HEAD^{commit}"])
                .await?;
            if tokio::fs::try_exists(git_dir.join("index"))
                .await
                .unwrap_or(true)
            {
                return Some(false);
            }
        }
        let worktrees = self
            .container_probe_git(&["worktree", "list", "--porcelain", "-z"])
            .await?;
        for worktree in worktrees
            .split('\0')
            .filter_map(|field| field.strip_prefix("worktree "))
            .skip(1)
        {
            if let Ok(worktree) = tokio::fs::canonicalize(worktree).await
                && strictly_inside(&root, &worktree)
            {
                return Some(true);
            }
        }
        Some(false)
    }
    async fn container_probe_git(&self, args: &[&str]) -> Option<String> {
        let output = self
            .execute(
                args,
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await
            .ok()?;
        (output.code == Some(0)
            && !output.timed_out
            && !output.stdout_truncated
            && !output.stdout_invalid_utf8)
            .then(|| output.stdout_text())
    }
}
