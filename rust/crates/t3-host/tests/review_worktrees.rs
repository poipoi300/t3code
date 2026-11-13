use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_host::{Result, background::Clock, review::*, worktrees::*};
struct Time;
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        0
    }
}
struct Known(PathBuf);
#[async_trait]
impl ReviewWorkspaces for Known {
    async fn contains(&self, path: &Path) -> Result<bool> {
        Ok(path.starts_with(&self.0))
    }
}
#[tokio::test]
async fn review_rejects_outside_and_alias_escape_before_git_but_allows_current_known_workspace() {
    let configured = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let known = tempfile::tempdir().unwrap();
    let service = ReviewService::new(
        configured.path().into(),
        Arc::new(Known(dunce::canonicalize(known.path()).unwrap())),
        Arc::new(Time),
    );
    let input = |cwd: &Path| {
        serde_json::from_value(serde_json::json!({"cwd":cwd.to_string_lossy()})).unwrap()
    };
    assert!(
        service
            .get_branch_commits(&input(outside.path()))
            .await
            .is_err()
    );
    assert!(
        service
            .get_branch_commits(&input(known.path()))
            .await
            .unwrap()
            .commits
            .is_empty()
    );
    #[cfg(windows)]
    junction::create(outside.path(), configured.path().join("escape")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), configured.path().join("escape")).unwrap();
    assert!(
        service
            .get_branch_commits(&input(&configured.path().join("escape")))
            .await
            .is_err()
    );
}
#[test]
fn managed_worktree_recreation_requires_exact_two_segments_and_real_nonroot_location() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("managed");
    std::fs::create_dir(&root).unwrap();
    assert!(
        is_managed_worktree_path(&root.join("repo/branch"), std::slice::from_ref(&root)).unwrap()
    );
    for path in [
        root.clone(),
        root.join("branch"),
        root.join("repo/nested/branch"),
        root.join("../escape/branch"),
    ] {
        assert!(
            !is_managed_worktree_path(&path, std::slice::from_ref(&root)).unwrap(),
            "{}",
            path.display()
        );
    }
    assert!(
        !is_managed_worktree_path(
            Path::new("relative/repo/branch"),
            std::slice::from_ref(&root)
        )
        .unwrap()
    );
    let outside = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    junction::create(outside.path(), root.join("alias")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.join("alias")).unwrap();
    assert!(!is_managed_worktree_path(&root.join("alias/branch"), &[root]).unwrap());
}
