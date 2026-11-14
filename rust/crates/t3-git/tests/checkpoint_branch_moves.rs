mod common;
use common::{Repository, TestResult};
use std::collections::HashSet;
use t3_git::{DiffOptions, checkpoint_ref};

#[tokio::test]
async fn branch_switches_rebase_the_snapshot_but_reset_deletions_remain_authored() -> TestResult {
    for committed_before in [false, true] {
        let repo = Repository::new().await?;
        repo.write("README.md", "# test\n")?;
        repo.commit("initial").await?;
        repo.command(&["branch", "b"]).await?;
        repo.command(&["checkout", "-b", "a"]).await?;
        repo.write("a.txt", "a\n")?;
        repo.write("README.md", "# on a\n")?;
        repo.command(&["add", "."]).await?;
        if committed_before {
            repo.command(&["commit", "-m", "work on a"]).await?;
        }
        let first = checkpoint_ref("switch", 0);
        let second = checkpoint_ref("switch", 1);
        let third = checkpoint_ref("switch", 2);
        repo.git.capture_checkpoint(&first).await?;
        let message = repo.command(&["log", "-1", "--format=%B", &first]).await?;
        assert!(
            message
                .lines()
                .any(|line| line.starts_with("T3-Head-Moved-At: ")),
            "{message}"
        );
        if !committed_before {
            repo.command(&["commit", "-m", "work on a"]).await?;
        }
        repo.command(&["checkout", "b"]).await?;
        repo.write("README.md", "# on b\n")?;
        repo.write("b.txt", "b\n")?;
        let index = repo.index()?;
        repo.git
            .capture_checkpoint_with_previous(&second, Some(&first))
            .await?;
        assert_eq!(repo.index()?, index);
        let patch = repo
            .git
            .diff_checkpoints(DiffOptions::new(&first, &second))
            .await?;
        assert!(!patch.contains("a.txt"), "{patch}");
        assert!(patch.contains("-# test\n+# on b\n"), "{patch}");
        assert!(patch.contains("b/b.txt"));
        assert_eq!(repo.git.list_authored_paths(&first, &second).await?, None);
        let parents = repo.command(&["log", "-1", "--format=%P", &second]).await?;
        assert_eq!(parents.split_whitespace().count(), 2);
        let baseline = repo
            .command(&["log", "-1", "--format=%P", &format!("{second}^2")])
            .await?;
        assert_eq!(
            baseline.split_whitespace().nth(1),
            Some(repo.command(&["rev-parse", &first]).await?.trim())
        );
        assert_eq!(
            repo.command(&["log", "-1", "--format=%ct", &first]).await?,
            repo.command(&["log", "-1", "--format=%ct", &format!("{second}^2")])
                .await?
        );
        repo.commit("work on b").await?;
        repo.git
            .capture_checkpoint_with_previous(&second, Some(&first))
            .await?;
        repo.command(&["reset", "--hard", "HEAD~1"]).await?;
        repo.git
            .capture_checkpoint_with_previous(&third, Some(&second))
            .await?;
        let reset = repo
            .git
            .diff_checkpoints(DiffOptions::new(&second, &third))
            .await?;
        assert!(reset.contains("--- a/b.txt\n+++ /dev/null"), "{reset}");
        assert_eq!(
            repo.git.list_authored_paths(&second, &third).await?,
            Some(HashSet::from(["README.md".into(), "b.txt".into()]))
        );
    }
    Ok(())
}

#[tokio::test]
async fn imported_recent_commits_are_excluded_but_commits_created_in_another_worktree_are_kept()
-> TestResult {
    let upstream = Repository::new().await?;
    upstream.write("base", "base\n")?;
    upstream.commit("base").await?;
    let local = Repository::new().await?;
    local
        .command(&[
            "remote",
            "add",
            "origin",
            upstream.root().to_str().expect("utf8"),
        ])
        .await?;
    local.command(&["fetch", "origin"]).await?;
    local.command(&["reset", "--hard", "origin/main"]).await?;
    let worktree = local.temp.path().join("turn");
    local
        .command(&[
            "worktree",
            "add",
            worktree.to_str().expect("utf8"),
            "-b",
            "turn",
        ])
        .await?;
    let turn = local.git.at(&worktree);
    let first = checkpoint_ref("fetched", 0);
    let second = checkpoint_ref("fetched", 1);
    turn.capture_checkpoint(&first).await?;
    upstream.command(&["checkout", "-b", "release"]).await?;
    upstream.write("first.txt", "first\n")?;
    upstream.commit("first").await?;
    turn.execute(&["fetch", "origin"], Default::default())
        .await?;
    upstream.write("second.txt", "second\n")?;
    upstream.commit("second").await?;
    local.command(&["checkout", "--detach"]).await?;
    local.write("own.txt", "own\n")?;
    local.commit("own").await?;
    local
        .command(&["push", "origin", "HEAD:refs/heads/pushed"])
        .await?;
    upstream.command(&["merge", "pushed", "--no-edit"]).await?;
    turn.execute(&["fetch", "origin"], Default::default())
        .await?;
    turn.execute(
        &["merge", "--ff-only", "origin/release"],
        Default::default(),
    )
    .await?;
    std::fs::write(worktree.join("local.txt"), "local\n")?;
    turn.execute(&["add", "."], Default::default()).await?;
    turn.execute(&["commit", "-m", "local"], Default::default())
        .await?;
    turn.execute(
        &["push", "origin", "HEAD:refs/heads/mirror"],
        Default::default(),
    )
    .await?;
    turn.capture_checkpoint(&second).await?;
    assert_eq!(
        turn.list_authored_paths(&first, &second).await?,
        Some(HashSet::from(["own.txt".into(), "local.txt".into()]))
    );
    let patch = turn
        .filtered_checkpoint_diff(&first, &second, false, t3_git::DIFF_MAX_BYTES)
        .await?;
    assert!(patch.contains("own.txt") && patch.contains("local.txt"));
    assert!(!patch.contains("first.txt") && !patch.contains("second.txt"));
    Ok(())
}

#[tokio::test]
async fn restore_overrides_manual_skip_bits_and_preserves_real_sparse_exclusions() -> TestResult {
    for sparse in [false, true] {
        for nested in [false, true] {
            let repo = Repository::new().await?;
            repo.write("scope/skipped", "base\n")?;
            repo.write("outside/excluded", "excluded\n")?;
            repo.commit("initial").await?;
            if sparse {
                repo.command(&["sparse-checkout", "set", "scope"]).await?;
            }
            repo.command(&["update-index", "--skip-worktree", "scope/skipped"])
                .await?;
            let git = if nested {
                repo.git.at(repo.root().join("scope"))
            } else {
                repo.git.clone()
            };
            let reference = checkpoint_ref("skip", 0);
            git.capture_checkpoint(&reference).await?;
            repo.write("scope/skipped", "after\n")?;
            assert!(git.restore_checkpoint(&reference, false).await?);
            assert_eq!(
                std::fs::read_to_string(repo.root().join("scope/skipped"))?,
                "base\n"
            );
            assert_eq!(repo.root().join("outside/excluded").exists(), !sparse);
            assert!(repo.command(&["diff", "--cached"]).await?.is_empty());
        }
    }
    Ok(())
}

#[tokio::test]
async fn legacy_checkpoints_and_unrelated_second_parents_keep_literal_baseline() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("f", "a\n")?;
    repo.commit("a").await?;
    let first = checkpoint_ref("legacy", 0);
    let second = checkpoint_ref("legacy", 1);
    let tree = repo.command(&["write-tree"]).await?;
    let legacy = repo
        .command(&[
            "commit-tree",
            tree.trim(),
            "-p",
            "HEAD",
            "-m",
            "legacy TypeScript checkpoint without trailer",
        ])
        .await?;
    repo.command(&["update-ref", &first, legacy.trim()]).await?;
    repo.command(&["checkout", "-b", "other"]).await?;
    repo.write("f", "b\n")?;
    repo.commit("b").await?;
    repo.git
        .capture_checkpoint_with_previous(&second, Some(&first))
        .await?;
    assert_eq!(
        repo.command(&["log", "-1", "--format=%P", &second])
            .await?
            .split_whitespace()
            .count(),
        1
    );
    let patch = repo
        .git
        .diff_checkpoints(DiffOptions::new(&first, &second))
        .await?;
    assert!(patch.contains("-a\n+b\n"));
    // TypeScript's second parent identifies its source checkpoint as its own second parent.
    let third = checkpoint_ref("legacy", 2);
    let start = repo
        .command(&[
            "commit-tree",
            tree.trim(),
            "-p",
            "HEAD",
            "-p",
            legacy.trim(),
            "-m",
            "t3 checkpoint turn start ref=typescript",
        ])
        .await?;
    let to_tree = repo.command(&["write-tree"]).await?;
    let to = repo
        .command(&[
            "commit-tree",
            to_tree.trim(),
            "-p",
            "HEAD",
            "-p",
            start.trim(),
            "-m",
            "t3 checkpoint ref=typescript",
        ])
        .await?;
    repo.command(&["update-ref", &third, to.trim()]).await?;
    assert!(
        repo.git
            .diff_checkpoints(DiffOptions::new(&first, &third))
            .await?
            .contains("-a\n+b\n")
    );
    // An aggregate range not beginning at the synthetic parent's source must ignore that parent.
    let patch = repo
        .git
        .diff_checkpoints(DiffOptions::new(&second, &third))
        .await?;
    assert!(patch.is_empty(), "{patch}");
    Ok(())
}

#[tokio::test]
async fn a_branch_switch_after_write_tree_belongs_to_the_next_checkpoint() -> TestResult {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let repo = Repository::new().await?;
    repo.write("f.txt", "A\n")?;
    let a = repo.commit("a").await?;
    repo.command(&["checkout", "-b", "b"]).await?;
    repo.write("f.txt", "B\n")?;
    repo.commit("b").await?;
    repo.command(&["checkout", "main"]).await?;
    let first = checkpoint_ref("race", 0);
    let second = checkpoint_ref("race", 1);
    let third = checkpoint_ref("race", 2);
    repo.git.capture_checkpoint(&first).await?;
    let real_git = if cfg!(windows) {
        std::process::Command::new("where.exe")
            .arg("git.exe")
            .output()?
    } else {
        std::process::Command::new("which").arg("git").output()?
    };
    let real_git = String::from_utf8(real_git.stdout)?
        .lines()
        .next()
        .ok_or("Git missing")?
        .to_owned();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let git = repo
        .git
        .clone()
        .with_program(env!("CARGO_BIN_EXE_git-filter-fixture"))
        .with_environment(
            [
                (
                    "T3_GIT_COMMAND_ADDRESS".into(),
                    Some(listener.local_addr()?.to_string().into()),
                ),
                ("T3_REAL_GIT".into(), Some(real_git.into())),
            ]
            .into_iter()
            .collect(),
        );
    let from = first.clone();
    let to = second.clone();
    let mut capture =
        tokio::spawn(async move { git.capture_checkpoint_with_previous(&to, Some(&from)).await });
    let (mut socket, _) = tokio::select! {connection=listener.accept()=>connection?,result=&mut capture=>return Err(format!("capture finished before snapshot barrier: {result:?}").into())};
    let mut ready = [0; 5];
    socket.read_exact(&mut ready).await?;
    assert_eq!(&ready, b"ready");
    repo.command(&["checkout", "b"]).await?;
    socket.write_all(b"x").await?;
    capture.await??;
    assert_eq!(
        repo.command(&["rev-parse", &format!("{second}^1")])
            .await?
            .trim(),
        a
    );
    assert_eq!(repo.show(&second, "f.txt").await?, "A\n");
    repo.git
        .capture_checkpoint_with_previous(&third, Some(&second))
        .await?;
    for (from, to) in [(&first, &second), (&second, &third)] {
        assert_eq!(
            repo.git
                .diff_checkpoints(DiffOptions::new(from, to))
                .await?,
            ""
        );
        assert_eq!(repo.git.list_authored_paths(from, to).await?, None);
    }
    Ok(())
}
