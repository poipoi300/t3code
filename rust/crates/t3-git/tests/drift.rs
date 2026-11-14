mod common;
use common::{Repository, TestResult};
use serde_json::json;

#[tokio::test]
async fn cached_refs_follow_external_checkouts_and_detached_head() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&["branch", "other"]).await?;
    let input = serde_json::from_value(json!({"cwd":repo.root()}))?;
    repo.git.list_branches(&input).await?;
    for branch in ["other", "main", "--detach"] {
        repo.command(&["switch", branch]).await?;
        let refs = repo.git.list_branches(&input).await?.refs;
        let current: Vec<_> = refs
            .iter()
            .filter(|item| item.current)
            .map(|item| item.name.0.as_str())
            .collect();
        assert_eq!(
            current,
            if branch == "--detach" {
                vec![]
            } else {
                vec![branch]
            }
        );
        assert!(
            refs.iter()
                .filter(|item| item.worktree_path.is_some())
                .all(|item| item.current)
        );
    }
    Ok(())
}

#[tokio::test]
async fn deleted_remote_default_and_head_alias_are_not_offered() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&["remote", "add", "origin", "."]).await?;
    repo.command(&["update-ref", "refs/remotes/origin/main", "HEAD"])
        .await?;
    repo.command(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ])
    .await?;
    assert_eq!(
        repo.git.default_branch_name("origin").await?.as_deref(),
        Some("main")
    );
    repo.command(&["update-ref", "-d", "refs/remotes/origin/main"])
        .await?;
    assert_eq!(repo.git.default_branch_name("origin").await?, None);
    repo.command(&["update-ref", "--no-deref", "-d", "refs/remotes/origin/HEAD"])
        .await?;
    repo.command(&["update-ref", "refs/remotes/origin/HEAD", "HEAD"])
        .await?;
    let input =
        serde_json::from_value(json!({"cwd":repo.root(),"includeMatchingRemoteRefs":true}))?;
    let refs = repo.git.list_branches(&input).await?.refs;
    assert!(
        refs.iter()
            .all(|item| !item.is_default && item.name.0 != "origin/HEAD")
    );
    Ok(())
}

#[tokio::test]
async fn oversized_untracked_review_keeps_small_files_and_omits_totals() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("tracked", "before\n")?;
    repo.commit("initial").await?;
    repo.write("tracked", "after\n")?;
    repo.write("a-large", "large untracked content\n")?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(repo.root().join("a-large"))?
        .set_len(256 * 1024 * 1024 + 1)?;
    repo.write("z-small", "small untracked content\n")?;
    let input = serde_json::from_value(
        json!({"cwd":repo.root(),"sourceKind":"working-tree","metadataOnly":true}),
    )?;
    let source = repo
        .git
        .review_diff_preview(&input)
        .await?
        .sources
        .remove(0);
    assert!(source.truncated);
    assert!(source.files.is_none());
    assert!(source.patches_deferred.is_none());
    assert!(source.diff.contains("+after"));
    assert!(source.diff.contains("+small untracked content"));
    assert!(!source.diff.contains("a-large"));
    Ok(())
}

#[tokio::test]
async fn oversized_worktree_copy_of_staged_deletion_does_not_spend_review_budget() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("removed", "original\n")?;
    repo.commit("initial").await?;
    repo.command(&["rm", "--cached", "removed"]).await?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(repo.root().join("removed"))?
        .set_len(256 * 1024 * 1024 + 1)?;
    repo.write("small", "small\n")?;
    let local = repo.git.cached_local_status().await?;
    let totals = local.branch_changes.expect("complete totals");
    assert_eq!(totals.insertions, 1);
    assert_eq!(totals.deletions, 1);
    let input = serde_json::from_value(
        json!({"cwd":repo.root(),"sourceKind":"working-tree","metadataOnly":true}),
    )?;
    let source = repo
        .git
        .review_diff_preview(&input)
        .await?
        .sources
        .remove(0);
    assert!(!source.truncated);
    assert_eq!(source.files.expect("complete").len(), 2);
    Ok(())
}

#[tokio::test]
async fn clone_invalidation_discards_missing_and_unborn_ref_caches() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.command(&["branch", "staging"]).await?;
    for unborn in [false, true] {
        let target = repo
            .temp
            .path()
            .join(if unborn { "unborn" } else { "missing" });
        std::fs::create_dir(&target)?;
        let git = repo.git.at(&target);
        if unborn {
            git.execute(&["init"], Default::default()).await?;
        }
        let input = serde_json::from_value(json!({"cwd":target}))?;
        assert!(git.list_branches(&input).await?.refs.is_empty());
        if unborn {
            std::fs::remove_dir_all(target.join(".git"))?;
        }
        repo.command(&[
            "clone",
            &repo.root().to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .await?;
        git.invalidate_caches().await;
        assert!(
            git.list_branches(&input)
                .await?
                .refs
                .iter()
                .any(|item| item.name.0 == "origin/staging")
        );
    }
    Ok(())
}
