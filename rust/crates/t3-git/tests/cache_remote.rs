mod common;
use common::{Repository, TestResult};
use t3_git::LocalStatusOptions;

#[tokio::test]
async fn local_status_omits_divergence_and_remote_refresh_preserves_fetch_head() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let remote = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &remote.to_string_lossy()])
        .await?;
    repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
        .await?;
    repo.command(&["push", "-u", "origin", "main"]).await?;
    repo.command(&["fetch", "origin"]).await?;
    let fetch_head = std::fs::read(repo.root().join(".git/FETCH_HEAD"))?;
    repo.command(&["commit", "--allow-empty", "-m", "ahead"])
        .await?;
    let local = repo.git.cached_local_status().await?;
    assert!(local.has_upstream);
    assert_eq!(local.ahead_count, 0);
    let remote_status = repo.git.status_details_remote(false).await?;
    assert_eq!(remote_status.ahead_count, 1);
    let refreshed = repo.git.status_details_remote(true).await?;
    assert_eq!(refreshed.ahead_count, 1);
    assert_eq!(
        std::fs::read(repo.root().join(".git/FETCH_HEAD"))?,
        fetch_head
    );
    Ok(())
}

#[tokio::test]
async fn status_static_config_reads_support_worktrees_backed_by_bare_repositories() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let bare = repo.temp.path().join("bare.git");
    let tree = repo.temp.path().join("linked");
    let bare_text = bare.to_string_lossy();
    repo.command(&["init", "--bare", &bare_text]).await?;
    repo.command(&[
        "--git-dir",
        &bare_text,
        "fetch",
        &repo.root().to_string_lossy(),
        "main:refs/heads/release",
    ])
    .await?;
    repo.command(&[
        "--git-dir",
        &bare_text,
        "remote",
        "add",
        "origin",
        &repo.root().to_string_lossy(),
    ])
    .await?;
    repo.command(&[
        "--git-dir",
        &bare_text,
        "update-ref",
        "refs/remotes/origin/release",
        "refs/heads/release",
    ])
    .await?;
    repo.command(&[
        "--git-dir",
        &bare_text,
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/release",
    ])
    .await?;
    repo.command(&[
        "--git-dir",
        &bare_text,
        "worktree",
        "add",
        &tree.to_string_lossy(),
        "release",
    ])
    .await?;
    let status = repo
        .git
        .at(&tree)
        .status_details_local(LocalStatusOptions::default())
        .await?;
    assert!(status.has_origin_remote);
    assert!(status.is_default_branch);
    assert_eq!(status.branch.as_deref(), Some("release"));
    Ok(())
}

#[tokio::test]
async fn review_marks_untracked_manifests_incomplete_above_the_file_cap() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("tracked", "before\n")?;
    repo.commit("initial").await?;
    repo.write("tracked", "after\n")?;
    std::fs::create_dir(repo.root().join("untracked"))?;
    for index in 0..5001 {
        std::fs::write(repo.root().join(format!("untracked/{index}")), "x\n")?;
    }
    let input = serde_json::from_value(
        serde_json::json!({"cwd":repo.root(),"sourceKind":"working-tree","metadataOnly":true}),
    )?;
    let result = repo.git.review_diff_preview(&input).await?;
    let source = &result.sources[0];
    assert!(source.truncated);
    assert!(source.files.is_none());
    assert!(source.patches_deferred.is_none());
    assert!(source.diff.contains("+after"));
    Ok(())
}

#[tokio::test]
async fn worktree_base_fetch_does_not_return_a_deleted_branches_stale_tracking_commit() -> TestResult
{
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let remote = repo.temp.path().join("remote.git");
    let remote_text = remote.to_string_lossy();
    repo.command(&["init", "--bare", &remote_text]).await?;
    repo.command(&["remote", "add", "my-org/upstream", &remote_text])
        .await?;
    repo.command(&["push", "-u", "my-org/upstream", "main:release"])
        .await?;
    let target = repo
        .git
        .resolve_worktree_base_remote("main")
        .await?
        .expect("target");
    assert_eq!(target, ("my-org/upstream".into(), "release".into()));
    let fetched = repo
        .git
        .fetch_worktree_base_commit("main", None)
        .await?
        .expect("fetched");
    assert_eq!(fetched.1, "my-org/upstream/release");
    repo.command(&[
        "--git-dir",
        &remote_text,
        "update-ref",
        "-d",
        "refs/heads/release",
    ])
    .await?;
    assert!(
        repo.git
            .fetch_worktree_base_commit("main", None)
            .await?
            .is_none()
    );
    assert!(
        repo.command(&["rev-parse", "refs/remotes/my-org/upstream/release"])
            .await?
            .trim()
            .len()
            >= 40
    );
    Ok(())
}

#[tokio::test]
async fn public_config_mutation_invalidates_cached_status_and_remote_metadata() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("base").await?;
    assert!(!repo.git.cached_local_status().await?.has_origin_remote);
    repo.git
        .write_config_value("remote.origin.url", ".")
        .await?;
    assert!(repo.git.cached_local_status().await?.has_origin_remote);
    Ok(())
}

#[tokio::test]
async fn fetching_one_commit_invalidates_opportunistically_updated_tracking_refs() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("base").await?;
    let remote = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &remote.to_string_lossy()])
        .await?;
    repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
        .await?;
    repo.command(&["push", "origin", "main"]).await?;
    let input = serde_json::from_value(
        serde_json::json!({"cwd": repo.root(), "includeMatchingRemoteRefs":true}),
    )?;
    let before = repo.git.list_branches(&input).await?;
    assert!(!before.refs.iter().any(|r| r.name.0 == "origin/new"));
    repo.command(&[
        "--git-dir",
        &remote.to_string_lossy(),
        "update-ref",
        "refs/heads/new",
        "refs/heads/main",
    ])
    .await?;
    repo.git.fetch_ref_commit("refs/heads/new").await?;
    assert!(
        !repo
            .command(&[
                "for-each-ref",
                "--format=%(refname)",
                "refs/remotes/origin/new"
            ])
            .await?
            .is_empty()
    );
    assert!(
        repo.git
            .list_branches(&input)
            .await?
            .refs
            .iter()
            .any(|r| r.name.0 == "origin/new")
    );
    Ok(())
}
