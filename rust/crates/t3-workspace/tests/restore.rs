use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_workspace::restore::{RestoreOutcome, WorktreeRestore};

async fn source(root: &Path) -> t3_workspace::Result<(t3_git::Git, t3_git::RepositoryBinding)> {
    let git = t3_git::Git::new(root);
    let binding = git.bind_repository().await?.expect("fixture repository");
    Ok((git, binding))
}
async fn git(root: &Path, args: &[&str]) {
    let mut options = t3_process::ProcessOptions::new("git").args(args.iter().copied());
    options.cwd = Some(root.into());
    options.env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        Some(root.join("fixture-config").into_os_string()),
    );
    options
        .env
        .insert("GIT_CONFIG_NOSYSTEM".into(), Some("1".into()));
    let out = t3_process::run(options).await.expect("git");
    assert_eq!(out.code, Some(0));
}
fn fixture(root: &Path) -> WorktreeRestore {
    WorktreeRestore {
        leases: Arc::default(),
        managed_roots: vec![root.join("worktrees")],
        scratch_root: root.join("scratch"),
    }
}
fn canonical(root: &Path) -> PathBuf {
    dunce::canonicalize(root).expect("canonical")
}
#[tokio::test]
async fn missing_managed_checkout_and_leftover_folders_restore_once_under_the_shared_lease() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = canonical(temp.path());
    let repo = root.join("repo");
    fs::create_dir(&repo).expect("repo");
    git(&repo, &["init", "-b", "main"]).await;
    fs::write(repo.join("tracked.txt"), "restored").expect("file");
    git(&repo, &["add", "."]).await;
    git(
        &repo,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "fixture",
        ],
    )
    .await;
    let path = root.join("worktrees/repo/saved");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "saved",
            path.to_str().expect("path"),
        ],
    )
    .await;
    fs::remove_dir_all(&path).expect("remove fixture checkout");
    fs::create_dir_all(path.join("node_modules/cache")).expect("leftovers");
    fs::create_dir(path.join(".next")).expect("next");
    let restore = fixture(&root);
    let other = root.join("other");
    fs::create_dir(&other).expect("second project");
    git(&other, &["init", "-b", "main"]).await;
    let (_, other_binding) = source(&other).await.expect("second binding");
    let refused = restore
        .restore(Some(&repo), Some(&path), Some("saved"), async {
            Ok((t3_git::Git::new(&repo), other_binding))
        })
        .await;
    assert!(matches!(
        refused,
        Err(t3_workspace::Error::Git(
            t3_git::GitError::RepositoryChanged { .. }
        ))
    ));
    assert!(path.join("node_modules/cache").is_dir());
    let changed = restore
        .restore(Some(&repo), Some(&path), Some("saved"), async {
            fs::write(path.join("notes"), "keep").expect("concurrent file");
            source(&repo).await
        })
        .await
        .expect("recheck leftovers");
    assert_eq!(changed, RestoreOutcome::Unchanged);
    assert_eq!(
        fs::read_to_string(path.join("notes")).expect("notes"),
        "keep"
    );
    fs::remove_file(path.join("notes")).expect("reset fixture");
    let (a, b) = tokio::join!(
        restore.restore(Some(&repo), Some(&path), Some("saved"), source(&repo)),
        restore.restore(Some(&repo), Some(&path), Some("saved"), source(&repo))
    );
    let outcomes = [a.expect("restore a"), b.expect("restore b")];
    assert!(outcomes.contains(&RestoreOutcome::CheckoutCreated));
    assert!(outcomes.contains(&RestoreOutcome::Unchanged));
    assert_eq!(
        fs::read_to_string(path.join("tracked.txt")).expect("restored"),
        "restored"
    );
    assert!(path.join(".git").is_file());
    assert!(!path.join("node_modules").exists());
    fs::remove_dir_all(&path).expect("remove fixture checkout");
    assert_eq!(
        restore
            .restore(Some(&repo), Some(&path), Some("saved"), source(&repo))
            .await
            .expect("missing restore"),
        RestoreOutcome::CheckoutCreated
    );
    assert!(path.join("tracked.txt").is_file());
}
#[tokio::test]
async fn branchless_restore_only_recreates_a_direct_child_of_the_projects_verified_scratch_root() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = canonical(temp.path());
    let restore = fixture(&root);
    fs::create_dir(&restore.scratch_root).expect("scratch");
    let direct = restore.scratch_root.join("thread");
    assert_eq!(
        restore
            .restore(
                Some(&restore.scratch_root),
                Some(&direct),
                None,
                source(&restore.scratch_root)
            )
            .await
            .expect("scratch restore"),
        RestoreOutcome::ScratchCreated
    );
    assert!(direct.is_dir());
    for (project, path) in [
        (restore.scratch_root.clone(), root.join("outside")),
        (
            restore.scratch_root.clone(),
            restore.scratch_root.join("nested/thread"),
        ),
        (
            root.join("other"),
            restore.scratch_root.join("wrong-project"),
        ),
    ] {
        assert_eq!(
            restore
                .restore(Some(&project), Some(&path), None, source(&project))
                .await
                .expect("no-op"),
            RestoreOutcome::Unchanged
        );
        assert!(!path.exists());
    }
}
#[tokio::test]
async fn leaves_user_files_and_unmanaged_paths_alone_and_surfaces_missing_base() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = canonical(temp.path());
    let repo = root.join("repo");
    fs::create_dir(&repo).expect("repo");
    git(&repo, &["init"]).await;
    let restore = fixture(&root);
    let user = root.join("worktrees/repo/user");
    fs::create_dir_all(&user).expect("user");
    fs::write(user.join("notes"), "keep").expect("notes");
    assert_eq!(
        restore
            .restore(Some(&repo), Some(&user), Some("user"), source(&repo))
            .await
            .expect("user no-op"),
        RestoreOutcome::Unchanged
    );
    assert_eq!(
        fs::read_to_string(user.join("notes")).expect("notes"),
        "keep"
    );
    let outside = root.join("external/thread");
    assert_eq!(
        restore
            .restore(Some(&repo), Some(&outside), Some("branch"), source(&repo))
            .await
            .expect("unmanaged no-op"),
        RestoreOutcome::Unchanged
    );
    assert!(!outside.exists());
    assert!(matches!(
        restore
            .restore(
                Some(&repo),
                Some(&root.join("worktrees/repo/missing")),
                Some("missing"),
                source(&repo)
            )
            .await,
        Err(t3_workspace::Error::WorktreeRecreateNoBase { .. })
    ));
}

#[tokio::test]
async fn recreates_deleted_branches_from_pushed_refs_before_the_current_project_branch() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = canonical(temp.path());
    let repo = root.join("repo");
    fs::create_dir(&repo).expect("repo");
    git(&repo, &["init", "-b", "main"]).await;
    fs::write(repo.join("tracked.txt"), "remote").expect("file");
    git(&repo, &["add", "."]).await;
    git(
        &repo,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "remote fixture",
        ],
    )
    .await;
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://fixture.invalid/repo.git",
        ],
    )
    .await;
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/published", "HEAD"],
    )
    .await;
    fs::write(repo.join("tracked.txt"), "current").expect("file");
    git(&repo, &["add", "."]).await;
    git(
        &repo,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "current fixture",
        ],
    )
    .await;
    let restore = fixture(&root);
    let published = root.join("worktrees/repo/published");
    restore
        .restore(
            Some(&repo),
            Some(&published),
            Some("published"),
            source(&repo),
        )
        .await
        .expect("published restore");
    assert_eq!(
        fs::read_to_string(published.join("tracked.txt")).expect("file"),
        "remote"
    );
    let fresh = root.join("worktrees/repo/fresh");
    restore
        .restore(Some(&repo), Some(&fresh), Some("fresh"), source(&repo))
        .await
        .expect("fresh restore");
    assert_eq!(
        fs::read_to_string(fresh.join("tracked.txt")).expect("file"),
        "current"
    );
    assert_eq!(
        t3_git::Git::new(&repo)
            .read_branch_base("fresh")
            .await
            .expect("base")
            .as_deref(),
        Some("main")
    );
}
