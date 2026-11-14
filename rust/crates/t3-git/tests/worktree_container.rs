mod common;
use common::{Repository, TestResult};
use std::path::PathBuf;
use t3_git::Git;

async fn container(
    repo: &Repository,
    name: &str,
    bare: bool,
) -> Result<Git, Box<dyn std::error::Error>> {
    let root = repo.temp.path().join(name);
    std::fs::create_dir(&root)?;
    repo.command(&[
        "clone",
        "--bare",
        &repo.root().to_string_lossy(),
        &root.join(".bare").to_string_lossy(),
    ])
    .await?;
    std::fs::write(root.join(".git"), "gitdir: ./.bare\n")?;
    let git = repo.git.at(root);
    git.execute(&["worktree", "add", "main", "main"], Default::default())
        .await?;
    git.execute(
        &["config", "core.bare", if bare { "true" } else { "false" }],
        Default::default(),
    )
    .await?;
    Ok(git)
}
async fn separate(repo: &Repository, name: &str) -> Result<Git, Box<dyn std::error::Error>> {
    let root = repo.temp.path().join(name);
    std::fs::create_dir(&root)?;
    let git = repo.git.at(root);
    git.execute(
        &["init", "-b", "main", "--separate-git-dir", ".metadata", "."],
        Default::default(),
    )
    .await?;
    Ok(git)
}

#[tokio::test]
async fn only_metadata_containers_with_nested_linked_worktrees_are_refused() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("README.md", "fixture")?;
    repo.commit("fixture").await?;
    assert!(!repo.git.is_worktree_container().await, "ordinary checkout");
    for bare in [false, true] {
        let git = container(
            &repo,
            if bare { "bare-container" } else { "container" },
            bare,
        )
        .await?;
        assert!(
            git.is_worktree_container().await,
            "container, core.bare={bare}"
        );
        assert!(
            !git.at(git.cwd().join("main")).is_worktree_container().await,
            "linked checkout"
        );
        let link = repo
            .temp
            .path()
            .join(if bare { "bare-alias" } else { "alias" });
        #[cfg(windows)]
        junction::create(git.cwd(), &link)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(git.cwd(), &link)?;
        assert!(
            repo.git.at(&link).is_worktree_container().await,
            "container alias"
        );
        #[cfg(windows)]
        std::fs::remove_dir(link)?;
        #[cfg(unix)]
        std::fs::remove_file(link)?;
    }
    let fresh = separate(&repo, "fresh").await?;
    fresh
        .execute(
            &[
                "worktree",
                "add",
                "--orphan",
                "-b",
                "feature",
                "../fresh-feature",
            ],
            Default::default(),
        )
        .await?;
    assert!(
        !fresh.is_worktree_container().await,
        "unborn repository with an orphan worktree"
    );
    let checkout = separate(&repo, "checkout").await?;
    std::fs::write(checkout.cwd().join("README.md"), "fixture")?;
    checkout
        .execute(&["add", "README.md"], Default::default())
        .await?;
    checkout
        .execute(&["commit", "-m", "fixture"], Default::default())
        .await?;
    checkout
        .execute(
            &["worktree", "add", "worktrees/feature", "-b", "feature"],
            Default::default(),
        )
        .await?;
    std::fs::remove_file(checkout.cwd().join("README.md"))?;
    assert!(
        !checkout.is_worktree_container().await,
        "index survives deletion of tracked files"
    );
    let outside = repo.temp.path().join("outside");
    repo.command(&[
        "clone",
        "--no-checkout",
        "--separate-git-dir",
        &outside.join(".metadata").to_string_lossy(),
        &repo.root().to_string_lossy(),
        &outside.to_string_lossy(),
    ])
    .await?;
    let outside = repo.git.at(outside);
    outside
        .execute(
            &["worktree", "add", "../outside-feature", "-b", "feature"],
            Default::default(),
        )
        .await?;
    assert!(
        !outside.is_worktree_container().await,
        "only outside worktrees"
    );
    assert!(
        !repo
            .git
            .at(repo.temp.path().join("missing"))
            .is_worktree_container()
            .await,
        "missing cwd"
    );
    let inconclusive = repo
        .git
        .at(repo.temp.path().join("container"))
        .with_program(PathBuf::from("missing-fixture-git"));
    assert!(
        !inconclusive.is_worktree_container().await,
        "failed probe is inconclusive"
    );
    Ok(())
}
