mod common;
use common::{Repository, TestResult};
use t3_git::{DiffFormat, DiffOptions, ExecuteOptions, GitError, OutputMode, checkpoint_ref};

#[tokio::test]
async fn capture_preserves_head_and_user_index_and_uses_exact_ref_identity() -> TestResult {
    let repo = Repository::new().await?;
    assert_eq!(
        checkpoint_ref("scope", 3),
        "refs/t3/orchestration-v2/checkpoints/NWYxNjFjOTE0OTg4MmUwZTEwMTI0YmM1ZGQ1YzExZjA/ordinal/3"
    );
    repo.write("file.txt", "base\n")?;
    repo.write("outside", "base\n")?;
    let head = repo.commit("initial").await?;
    repo.write("file.txt", "staged\n")?;
    repo.command(&["add", "file.txt"]).await?;
    repo.write("file.txt", "unstaged\n")?;
    repo.write("new file [x].txt", "new\n")?;
    repo.write(".gitignore", "ignored\n")?;
    repo.write("ignored", "secret\n")?;
    let index = repo.index()?;
    let reference = checkpoint_ref("capture", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(repo.show(&reference, "file.txt").await?, "unstaged\n");
    assert_eq!(repo.show(&reference, "new file [x].txt").await?, "new\n");
    assert!(
        !repo
            .command(&["ls-tree", "-r", "--name-only", &reference])
            .await?
            .lines()
            .any(|path| path == "ignored")
    );
    assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?.trim(), head);
    assert_eq!(
        repo.command(&["rev-parse", &format!("{reference}^")])
            .await?
            .trim(),
        head
    );
    assert_eq!(repo.index()?, index);
    assert_eq!(
        repo.command(&["show", "-s", "--format=%an <%ae>%n%s", &reference])
            .await?,
        format!("T3 Code <t3code@users.noreply.github.com>\nt3 checkpoint ref={reference}\n")
    );
    assert!(std::fs::read_dir(repo.root().join(".git"))?.all(|entry| {
        !entry
            .expect("directory entry")
            .file_name()
            .to_string_lossy()
            .starts_with("t3-checkpoint-index-")
    }));
    Ok(())
}

#[tokio::test]
async fn subdirectory_scope_preserves_outside_paths_and_index() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("scope/a", "base\n")?;
    repo.write("outside", "base\n")?;
    repo.commit("initial").await?;
    repo.write("outside", "outside staged\n")?;
    repo.command(&["add", "outside"]).await?;
    repo.write("scope/a", "inside staged\n")?;
    repo.command(&["add", "scope/a"]).await?;
    repo.write("scope/a", "inside working\n")?;
    repo.write("scope/new", "new\n")?;
    let index = repo.index()?;
    let scoped = repo.git.at(repo.root().join("scope"));
    let reference = checkpoint_ref("subdir", 0);
    assert!(scoped.is_repository().await?);
    scoped.capture_checkpoint(&reference).await?;
    assert_eq!(repo.show(&reference, "outside").await?, "base\n");
    assert_eq!(repo.show(&reference, "scope/a").await?, "inside working\n");
    assert_eq!(repo.show(&reference, "scope/new").await?, "new\n");
    assert_eq!(repo.index()?, index);
    repo.write("scope/a", "later\n")?;
    repo.write("scope/newer", "later\n")?;
    assert!(scoped.restore_checkpoint(&reference, false).await?);
    assert_eq!(
        std::fs::read_to_string(repo.root().join("scope/a"))?,
        "inside working\n"
    );
    assert!(!repo.root().join("scope/newer").exists());
    assert_eq!(
        repo.command(&["diff", "--cached", "--name-only"]).await?,
        "outside\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root().join("outside"))?,
        "outside staged\n"
    );
    Ok(())
}

#[tokio::test]
async fn unborn_and_empty_checkpoints_restore_without_staging() -> TestResult {
    for initial_commit in [false, true] {
        let repo = Repository::new().await?;
        if initial_commit {
            repo.commit("empty").await?;
        }
        std::fs::create_dir(repo.root().join("scope"))?;
        let scoped = repo.git.at(repo.root().join("scope"));
        let reference = checkpoint_ref("empty", 0);
        scoped.capture_checkpoint(&reference).await?;
        repo.write("outside", "keep\n")?;
        repo.command(&["add", "outside"]).await?;
        repo.write("scope/added", "remove\n")?;
        repo.command(&["add", "scope/added"]).await?;
        repo.write("scope/.gitignore", "hidden\n")?;
        repo.write("scope/hidden", "revealed by deleting ignore file\n")?;
        assert!(scoped.restore_checkpoint(&reference, false).await?);
        assert!(repo.root().join("scope").is_dir());
        assert_eq!(std::fs::read_dir(repo.root().join("scope"))?.count(), 0);
        assert_eq!(
            repo.command(&["diff", "--cached", "--name-only"]).await?,
            "outside\n"
        );
        assert!(!scoped.restore_checkpoint("refs/t3/missing", false).await?);
    }
    let repo = Repository::new().await?;
    repo.write("a", "unborn\n")?;
    let reference = checkpoint_ref("unborn", 0);
    repo.git.capture_checkpoint(&reference).await?;
    repo.write("a", "later\n")?;
    assert!(repo.git.restore_checkpoint(&reference, false).await?);
    assert_eq!(repo.command(&["ls-files"]).await?, "");
    assert_eq!(std::fs::read_to_string(repo.root().join("a"))?, "unborn\n");
    Ok(())
}

#[tokio::test]
async fn ignored_subdirectory_includes_tracked_and_staged_additions_only() -> TestResult {
    for unborn in [false, true] {
        let repo = Repository::new().await?;
        repo.write(".gitignore", "scope/\n")?;
        repo.write("scope/tracked", "base\n")?;
        repo.command(&["add", "-f", "scope/tracked"]).await?;
        if !unborn {
            repo.commit("initial").await?;
        }
        repo.write("scope/tracked", "working\n")?;
        repo.write("scope/staged with spaces", "addition\n")?;
        repo.command(&["add", "-f", "scope/staged with spaces"])
            .await?;
        repo.write("scope/ignored", "omit\n")?;
        let index = repo.index()?;
        let reference = checkpoint_ref("ignored", 0);
        repo.git
            .at(repo.root().join("scope"))
            .capture_checkpoint(&reference)
            .await?;
        assert_eq!(repo.show(&reference, "scope/tracked").await?, "working\n");
        assert_eq!(
            repo.show(&reference, "scope/staged with spaces").await?,
            "addition\n"
        );
        assert!(
            !repo
                .command(&["ls-tree", "-r", "--name-only", &reference])
                .await?
                .contains("scope/ignored")
        );
        assert_eq!(repo.index()?, index);
    }
    let repo = Repository::new().await?;
    repo.write(".gitignore", "ignored/\n")?;
    repo.commit("initial").await?;
    repo.write("ignored/a", "omit\n")?;
    assert!(matches!(
        repo.git
            .at(repo.root().join("ignored"))
            .capture_checkpoint(&checkpoint_ref("ignored-empty", 0))
            .await,
        Err(GitError::Unsupported(_))
    ));
    Ok(())
}

#[tokio::test]
async fn manually_flagged_split_missing_and_corrupt_indices_do_not_hide_edits() -> TestResult {
    for mode in [
        "assume-unchanged",
        "skip-worktree",
        "split",
        "missing",
        "invalid",
    ] {
        let repo = Repository::new().await?;
        repo.write("scope/a", "before\n")?;
        repo.write("scope/deleted", "before\n")?;
        repo.write("outside", "before\n")?;
        repo.commit("initial").await?;
        match mode {
            "assume-unchanged" | "skip-worktree" => {
                repo.command(&[
                    "update-index",
                    &format!("--{mode}"),
                    "scope/a",
                    "scope/deleted",
                ])
                .await?;
                repo.command(&["config", "core.ignorestat", "true"]).await?;
            }
            "split" => {
                repo.command(&["config", "core.splitIndex", "true"]).await?;
                repo.command(&["config", "splitIndex.sharedIndexExpire", "now"])
                    .await?;
                repo.command(&["update-index", "--split-index"]).await?;
            }
            "missing" => std::fs::remove_file(repo.root().join(".git/index"))?,
            "invalid" => std::fs::write(repo.root().join(".git/index"), "not an index")?,
            _ => unreachable!(),
        }
        let before = repo.index().ok();
        repo.write("scope/a", "after!\n")?;
        std::fs::remove_file(repo.root().join("scope/deleted"))?;
        let reference = checkpoint_ref(mode, 0);
        repo.git
            .at(repo.root().join("scope"))
            .capture_checkpoint(&reference)
            .await?;
        assert_eq!(
            repo.show(&reference, "scope/a").await?,
            "after!\n",
            "{mode}"
        );
        assert!(
            !repo
                .command(&["ls-tree", "-r", "--name-only", &reference])
                .await?
                .contains("scope/deleted")
        );
        assert_eq!(repo.index().ok(), before, "{mode}");
        if mode == "split" {
            assert!(repo.command(&["ls-files"]).await?.contains("scope/a"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn racy_same_size_same_timestamp_edit_is_captured() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "before\n")?;
    repo.command(&["config", "core.trustctime", "false"])
        .await?;
    let time = filetime::FileTime::from_unix_time(1_700_000_000, 999_000_000);
    filetime::set_file_times(repo.root().join("a"), time, time)?;
    repo.commit("racy file").await?;
    filetime::set_file_times(repo.root().join(".git/index"), time, time)?;
    repo.write("a", "after!\n")?;
    filetime::set_file_times(repo.root().join("a"), time, time)?;
    let reference = checkpoint_ref("racy", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(repo.show(&reference, "a").await?, "after!\n");
    Ok(())
}

#[tokio::test]
async fn sparse_indices_preserve_out_of_cone_entries_and_clear_manual_flags() -> TestResult {
    for non_cone in [false, true] {
        let repo = Repository::new().await?;
        repo.write("scope/in/a", "before\n")?;
        repo.write("scope/in/deleted", "before\n")?;
        repo.write("scope/out/b", "keep out\n")?;
        repo.write("elsewhere/b", "keep\n")?;
        repo.commit("sparse").await?;
        repo.command(&[
            "sparse-checkout",
            "set",
            "--cone",
            "--sparse-index",
            "scope/in",
            "elsewhere",
        ])
        .await?;
        if non_cone {
            repo.command(&[
                "sparse-checkout",
                "set",
                "--no-cone",
                "/scope/in/",
                "/elsewhere/",
            ])
            .await?;
        }
        repo.command(&["update-index", "--skip-worktree", "scope/in/deleted"])
            .await?;
        repo.write("scope/in/a", "working\n")?;
        repo.write("scope/in/new", "new\n")?;
        std::fs::remove_file(repo.root().join("scope/in/deleted"))?;
        let index = repo.index()?;
        let reference = checkpoint_ref("sparse", 0);
        repo.git
            .at(repo.root().join("scope"))
            .capture_checkpoint(&reference)
            .await?;
        assert_eq!(repo.show(&reference, "scope/in/a").await?, "working\n");
        assert_eq!(repo.show(&reference, "scope/out/b").await?, "keep out\n");
        assert!(
            !repo
                .command(&["ls-tree", "-r", "--name-only", &reference])
                .await?
                .contains("scope/in/deleted")
        );
        assert_eq!(repo.index()?, index);
    }
    Ok(())
}

#[tokio::test]
async fn nested_unborn_repos_are_excluded_and_committed_repos_are_gitlinks() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    repo.commit("initial").await?;
    repo.command(&["init", "empty nested"]).await?;
    repo.command(&["init", "committed"]).await?;
    repo.write("committed/a", "nested\n")?;
    let nested = repo.git.at(repo.root().join("committed"));
    nested
        .execute(&["add", "."], ExecuteOptions::default())
        .await?;
    nested
        .execute(&["commit", "-m", "nested"], ExecuteOptions::default())
        .await?;
    repo.write("a", "working\n")?;
    let reference = checkpoint_ref("nested", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(repo.show(&reference, "a").await?, "working\n");
    assert!(
        repo.command(&["ls-tree", &reference, "--", "empty nested"])
            .await?
            .is_empty()
    );
    assert!(
        repo.command(&["ls-tree", &reference, "--", "committed"])
            .await?
            .starts_with("160000 commit ")
    );
    Ok(())
}

#[tokio::test]
async fn tracked_submodules_capture_gitlink_not_dirty_contents() -> TestResult {
    let repo = Repository::new().await?;
    let sub = Repository::new().await?;
    sub.write("a", "first\n")?;
    sub.commit("sub initial").await?;
    repo.commit("initial").await?;
    let source = sub.root().to_string_lossy().into_owned();
    repo.command(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        &source,
        "module",
    ])
    .await?;
    repo.commit("add submodule").await?;
    repo.write("module/a", "dirty\n")?;
    let reference = checkpoint_ref("submodules", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(
        repo.command(&["ls-tree", &reference, "module"]).await?,
        repo.command(&["ls-tree", "HEAD", "module"]).await?
    );
    assert_eq!(
        std::fs::read_to_string(repo.root().join("module/a"))?,
        "dirty\n"
    );
    Ok(())
}

#[tokio::test]
async fn capture_honors_filters_but_overrides_safecrlf_and_cleans_failed_scratch() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    repo.commit("initial").await?;
    repo.command(&["config", "core.autocrlf", "true"]).await?;
    repo.command(&["config", "core.safecrlf", "true"]).await?;
    repo.write("a", "working\n")?;
    let reference = checkpoint_ref("crlf", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(repo.show(&reference, "a").await?, "working\n");
    repo.write(".gitattributes", "a filter=reject\n")?;
    repo.command(&["config", "filter.reject.clean", "false"])
        .await?;
    repo.command(&["config", "filter.reject.required", "true"])
        .await?;
    assert!(
        repo.git
            .capture_checkpoint(&checkpoint_ref("filter-fails", 0))
            .await
            .is_err()
    );
    assert!(std::fs::read_dir(repo.root().join(".git"))?.all(|entry| {
        !entry
            .expect("directory entry")
            .file_name()
            .to_string_lossy()
            .starts_with("t3-checkpoint-index-")
    }));
    Ok(())
}

#[tokio::test]
async fn refs_listing_pruning_missing_refs_and_head_fallback() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    repo.commit("initial").await?;
    let first = checkpoint_ref("refs", 0);
    let second = checkpoint_ref("refs", 1);
    repo.git.capture_checkpoint(&first).await?;
    repo.write("a", "after\n")?;
    repo.git.capture_checkpoint(&second).await?;
    assert!(repo.git.has_checkpoint(&first).await?);
    assert_eq!(repo.git.list_checkpoint_refs().await?.len(), 2);
    let missing = "refs/t3/missing";
    assert!(!repo.git.has_checkpoint(missing).await?);
    assert!(
        repo.git
            .diff_checkpoints(DiffOptions::new(missing, &second))
            .await
            .is_err()
    );
    let mut options = DiffOptions::new(missing, &second);
    options.fallback_from_to_head = true;
    assert!(repo.git.diff_checkpoints(options).await?.contains("+after"));
    assert!(repo.git.restore_checkpoint(missing, true).await?);
    assert_eq!(std::fs::read_to_string(repo.root().join("a"))?, "base\n");
    let foreign = checkpoint_ref("another-home", 0);
    repo.git.capture_checkpoint(&foreign).await?;
    let removed = repo
        .git
        .prune_checkpoint_refs("refs", &[second.clone()].into_iter().collect())
        .await?;
    assert_eq!(removed, vec![first]);
    assert!(repo.git.has_checkpoint(&foreign).await?);
    assert!(repo.git.has_checkpoint(&second).await?);
    assert!(
        repo.git
            .capture_checkpoint("refs/heads/main")
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn diff_uses_repo_paths_literal_pathspecs_and_stable_patch_format() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("scope/a [x]", "one\n")?;
    repo.write("outside", "one\n")?;
    repo.commit("initial").await?;
    let first = checkpoint_ref("diff", 0);
    let second = checkpoint_ref("diff", 1);
    repo.git.capture_checkpoint(&first).await?;
    repo.write("scope/a [x]", "two\n")?;
    repo.write("outside", "two\n")?;
    repo.git.capture_checkpoint(&second).await?;
    repo.command(&["config", "diff.relative", "true"]).await?;
    repo.command(&["config", "diff.noprefix", "true"]).await?;
    repo.command(&["config", "diff.mnemonicPrefix", "true"])
        .await?;
    repo.command(&["config", "diff.suppressBlankEmpty", "true"])
        .await?;
    let paths = vec!["scope/a [x]".into()];
    let mut options = DiffOptions::new(&first, &second);
    options.file_paths = Some(&paths);
    let scoped = repo.git.at(repo.root().join("scope"));
    let patch = scoped.diff_checkpoints(options).await?;
    assert!(patch.contains("diff --git a/scope/a [x] b/scope/a [x]"));
    assert!(!patch.contains("outside"));
    let mut options = DiffOptions::new(&first, &second);
    options.format = DiffFormat::Numstat;
    assert!(
        scoped
            .diff_checkpoints(options)
            .await?
            .contains("1\t1\toutside\0")
    );
    let empty = Vec::new();
    let mut options = DiffOptions::new("missing", "missing");
    options.file_paths = Some(&empty);
    assert_eq!(repo.git.diff_checkpoints(options).await?, "");
    Ok(())
}

#[tokio::test]
async fn raw_diff_truncates_patch_but_numstat_rejects_overflow() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "before\n")?;
    repo.commit("initial").await?;
    let first = checkpoint_ref("budget", 0);
    let second = checkpoint_ref("budget", 1);
    repo.git.capture_checkpoint(&first).await?;
    repo.write("a", "é".repeat(1000))?;
    repo.git.capture_checkpoint(&second).await?;
    let mut options = DiffOptions::new(&first, &second);
    options.max_output_bytes = 128;
    assert_eq!(repo.git.diff_checkpoints(options).await?.len(), 128);
    let mut options = DiffOptions::new(&first, &second);
    options.max_output_bytes = 1;
    options.format = DiffFormat::Numstat;
    assert!(matches!(
        repo.git.diff_checkpoints(options).await,
        Err(GitError::Process(
            t3_process::ProcessError::OutputLimit { .. }
        ))
    ));
    let diff = repo
        .git
        .filtered_checkpoint_diff(&first, &second, false, 150)
        .await?;
    assert!(diff.len() <= 150);
    assert!(diff.ends_with("\n\n[truncated]"));
    assert!(!diff.contains('\u{fffd}'));
    // Error mode is also used when reading branch metadata.
    let error = repo
        .git
        .execute(
            &["status", "--porcelain=2", "--branch", "-z"],
            ExecuteOptions {
                max_output_bytes: 1,
                output_mode: OutputMode::Error,
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(
        error,
        Err(GitError::Process(
            t3_process::ProcessError::OutputLimit { .. }
        ))
    ));
    Ok(())
}

#[tokio::test]
async fn restore_preserves_ignored_files_and_nested_repositories_and_bounds_cleanup() -> TestResult
{
    let repo = Repository::new().await?;
    repo.write("checkpoint.txt", "baseline\n")?;
    repo.commit("initial").await?;
    let reference = checkpoint_ref("clean", 0);
    repo.git.capture_checkpoint(&reference).await?;
    std::fs::write(repo.root().join(".git/info/exclude"), "ignored.txt\n")?;
    repo.write("ignored.txt", "keep\n")?;
    repo.command(&["init", "nested"]).await?;
    repo.write("nested/a", "keep nested\n")?;
    repo.write("untracked/.gitignore", "hidden.txt\n")?;
    repo.write("untracked/hidden.txt", "remove\n")?;
    assert!(repo.git.restore_checkpoint(&reference, false).await?);
    assert_eq!(
        std::fs::read_to_string(repo.root().join("ignored.txt"))?,
        "keep\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root().join("nested/a"))?,
        "keep nested\n"
    );
    assert!(!repo.root().join("untracked").exists());
    let mut path = String::new();
    for _ in 0..4 {
        repo.write(&format!("{path}.gitignore"), "d/.gitignore\n")?;
        path.push_str("d/");
    }
    repo.write(&format!("{path}.gitignore"), "hidden.txt\n")?;
    repo.write(&format!("{path}hidden.txt"), "still hidden\n")?;
    repo.write("checkpoint.txt", "later\n")?;
    let restored = repo.git.restore_checkpoint(&reference, false).await;
    assert!(
        matches!(restored, Err(GitError::Unavailable(_))),
        "{restored:?}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root().join("checkpoint.txt"))?,
        "baseline\n"
    );
    assert!(
        repo.command(&["diff", "--cached", "--name-only"])
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_during_clean_filter_kills_tree_and_cleans_private_index() -> TestResult {
    use tokio::io::AsyncReadExt;
    let repo = Repository::new().await?;
    repo.write("a", "before\n")?;
    repo.commit("initial").await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    repo.write(".gitattributes", "a filter=hold\n")?;
    let executable = env!("CARGO_BIN_EXE_git-filter-fixture").replace('\\', "/");
    repo.command(&["config", "filter.hold.clean", &format!("\"{executable}\"")])
        .await?;
    repo.command(&["config", "filter.hold.required", "true"])
        .await?;
    repo.write("a", "working\n")?;
    let index = repo.index()?;
    let token = t3_git::CancellationToken::new();
    let git = repo
        .git
        .clone()
        .with_cancellation(token.clone())
        .with_environment(
            [(
                "T3_GIT_FILTER_ADDRESS".into(),
                Some(listener.local_addr()?.to_string().into()),
            )]
            .into_iter()
            .collect(),
        );
    let reference = checkpoint_ref("cancel", 0);
    let task = tokio::spawn(async move { git.capture_checkpoint(&reference).await });
    let (mut socket, _) = listener.accept().await?;
    let mut greeting = [0; 5];
    socket.read_exact(&mut greeting).await?;
    token.cancel();
    assert!(matches!(
        task.await?,
        Err(GitError::Process(t3_process::ProcessError::Cancelled))
    ));
    assert_eq!(socket.read(&mut greeting).await.unwrap_or(0), 0);
    assert_eq!(repo.index()?, index);
    assert!(std::fs::read_dir(repo.root().join(".git"))?.all(|entry| {
        !entry
            .expect("directory entry")
            .file_name()
            .to_string_lossy()
            .starts_with("t3-checkpoint-index-")
    }));
    Ok(())
}

#[tokio::test]
async fn aborting_capture_waits_for_the_owned_child_before_scratch_cleanup() -> TestResult {
    use tokio::io::AsyncReadExt;
    let repo = Repository::new().await?;
    repo.write("a", "before\n")?;
    repo.commit("initial").await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    repo.write(".gitattributes", "a filter=hold\n")?;
    let executable = env!("CARGO_BIN_EXE_git-filter-fixture").replace('\\', "/");
    repo.command(&["config", "filter.hold.clean", &format!("\"{executable}\"")])
        .await?;
    repo.command(&["config", "filter.hold.required", "true"])
        .await?;
    repo.write("a", "working\n")?;
    let before = repo.index()?;
    let git = repo.git.clone().with_environment(
        [(
            "T3_GIT_FILTER_ADDRESS".into(),
            Some(listener.local_addr()?.to_string().into()),
        )]
        .into_iter()
        .collect(),
    );
    let cleanup = git.clone();
    let task =
        tokio::spawn(async move { git.capture_checkpoint(&checkpoint_ref("abort", 0)).await });
    let (mut socket, _) = listener.accept().await?;
    let mut greeting = [0; 5];
    socket.read_exact(&mut greeting).await?;
    task.abort();
    assert!(task.await.expect_err("aborted").is_cancelled());
    cleanup.drain_operations().await;
    assert_eq!(socket.read(&mut greeting).await.unwrap_or(0), 0);
    assert_eq!(repo.index()?, before);
    assert!(std::fs::read_dir(repo.root().join(".git"))?.all(|entry| {
        !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with("t3-checkpoint-index-")
    }));
    Ok(())
}

#[tokio::test]
async fn full_diff_budget_and_numstat_support_patches_over_ten_megabytes() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("large", "before\n")?;
    repo.commit("initial").await?;
    let from = checkpoint_ref("large", 0);
    let to = checkpoint_ref("large", 1);
    repo.git.capture_checkpoint(&from).await?;
    repo.write("large", "x".repeat(t3_git::DIFF_MAX_BYTES + 1024))?;
    repo.git.capture_checkpoint(&to).await?;
    let mut options = DiffOptions::new(&from, &to);
    options.format = DiffFormat::Numstat;
    let files = t3_git::parse_numstat(&repo.git.diff_checkpoints(options).await?);
    assert_eq!(files.len(), 1);
    assert_eq!((files[0].additions, files[0].deletions), (1, 1));
    let raw = repo
        .git
        .diff_checkpoints(DiffOptions::new(&from, &to))
        .await?;
    assert_eq!(raw.len(), t3_git::DIFF_MAX_BYTES);
    assert!(!raw.ends_with(t3_git::TRUNCATED_MARKER));
    let query = repo
        .git
        .filtered_checkpoint_diff(&from, &to, true, t3_git::DIFF_MAX_BYTES)
        .await?;
    assert_eq!(query.len(), t3_git::DIFF_MAX_BYTES);
    assert!(query.ends_with(t3_git::TRUNCATED_MARKER));
    Ok(())
}

#[tokio::test]
async fn injected_git_config_is_preserved_and_bare_repository_protection_is_appended() -> TestResult
{
    let repo = Repository::new().await?;
    for count in ["1", "+1", "  1"] {
        let git = repo.git.clone().with_environment(
            [
                ("GIT_CONFIG_COUNT".into(), Some(count.into())),
                ("GIT_CONFIG_KEY_0".into(), Some("t3.test".into())),
                ("GIT_CONFIG_VALUE_0".into(), Some("preserved".into())),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            git.execute(&["config", "--get", "t3.test"], ExecuteOptions::default())
                .await?
                .stdout_text(),
            "preserved\n"
        );
        assert_eq!(
            git.execute(
                &["config", "--get", "safe.bareRepository"],
                ExecuteOptions::default()
            )
            .await?
            .stdout_text(),
            "explicit\n"
        );
    }
    for count in ["", "0", "+0", "-00"] {
        let git = repo.git.clone().with_environment(
            [("GIT_CONFIG_COUNT".into(), Some(count.into()))]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            git.execute(
                &["config", "--get", "safe.bareRepository"],
                ExecuteOptions::default()
            )
            .await?
            .stdout_text(),
            "explicit\n"
        );
    }
    let invalid = repo.git.clone().with_environment(
        [("GIT_CONFIG_COUNT".into(), Some("invalid".into()))]
            .into_iter()
            .collect(),
    );
    assert!(invalid.is_repository().await.is_err());
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn capture_and_restore_override_disabled_long_path_support() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    repo.commit("initial").await?;
    repo.command(&["config", "core.longpaths", "false"]).await?;
    let nested = format!("{}/{}/{}", "a".repeat(90), "b".repeat(90), "c".repeat(90));
    let file = format!("{nested}/file");
    repo.write(&file, "snapshot\n")?;
    let reference = checkpoint_ref("long-paths", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(
        repo.command(&[
            "-c",
            "core.longpaths=true",
            "show",
            &format!("{reference}:{file}")
        ])
        .await?,
        "snapshot\n"
    );
    repo.write(&file, "changed\n")?;
    repo.write(&format!("{nested}/new"), "remove\n")?;
    assert!(repo.git.restore_checkpoint(&reference, false).await?);
    assert_eq!(
        std::fs::read_to_string(repo.root().join(file))?,
        "snapshot\n"
    );
    assert!(!repo.root().join(nested).join("new").exists());
    Ok(())
}

#[tokio::test]
async fn noncone_sparse_checkout_refuses_fresh_index_rebuild() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("included/a", "before\n")?;
    repo.write("excluded/a", "keep\n")?;
    repo.commit("initial").await?;
    repo.command(&["sparse-checkout", "set", "--no-cone", "/included/"])
        .await?;
    std::fs::remove_file(repo.root().join(".git/index"))?;
    let reference = checkpoint_ref("no-cone-missing", 0);
    assert!(matches!(
        repo.git.capture_checkpoint(&reference).await,
        Err(GitError::Unsupported(_))
    ));
    assert!(!repo.git.has_checkpoint(&reference).await?);
    assert!(!repo.root().join(".git/index").exists());
    Ok(())
}

#[tokio::test]
async fn nested_recovery_is_bounded_by_commands_and_candidates_not_a_wall_clock_budget()
-> TestResult {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    repo.commit("base").await?;
    repo.command(&["init", "empty nested"]).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let git = repo
        .git
        .clone()
        .with_program(env!("CARGO_BIN_EXE_git-gate-fixture"))
        .with_environment(
            [(
                "T3_GIT_GATE_ADDRESS".into(),
                Some(listener.local_addr()?.to_string().into()),
            )]
            .into_iter()
            .collect(),
        );
    let reference = checkpoint_ref("delayed-nested", 0);
    let captured = reference.clone();
    let task = tokio::spawn(async move { git.capture_checkpoint(&captured).await });
    let (mut socket, _) = listener.accept().await?;
    let mut greeting = [0; 5];
    socket.read_exact(&mut greeting).await?;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    tokio::time::resume();
    socket.write_all(b"x").await?;
    task.await??;
    assert_eq!(repo.show(&reference, "a").await?, "base\n");
    assert!(
        repo.command(&["ls-tree", &reference, "--", "empty nested"])
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn checkpoint_ref_writes_and_prunes_never_follow_symbolic_refs() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "base\n")?;
    let head = repo.commit("base").await?;
    let reference = checkpoint_ref("sym-capture", 0);
    repo.command(&["symbolic-ref", &reference, "refs/heads/main"])
        .await?;
    repo.write("a", "working\n")?;
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(repo.command(&["rev-parse", "HEAD"]).await?.trim(), head);
    let foreign = checkpoint_ref("foreign-scope", 0);
    repo.git.capture_checkpoint(&foreign).await?;
    let alias = checkpoint_ref("sym-prune", 0);
    repo.command(&["symbolic-ref", &alias, &foreign]).await?;
    repo.git
        .prune_checkpoint_refs("sym-prune", &Default::default())
        .await?;
    assert!(repo.git.has_checkpoint(&foreign).await?);
    assert!(!repo.git.has_checkpoint(&alias).await?);
    let lock = repo.root().join(format!(".git/{foreign}.lock"));
    std::fs::write(&lock, "owned test lock")?;
    repo.git
        .delete_checkpoint_refs(&[&foreign, &reference])
        .await?;
    assert!(repo.git.has_checkpoint(&foreign).await?);
    assert!(
        !repo.git.has_checkpoint(&reference).await?,
        "a locked ref must not prevent pruning other refs"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn internal_file_symlinks_keep_git_symlink_semantics() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("target", "inside\n")?;
    repo.commit("base").await?;
    let link = repo.root().join("link");
    match std::os::windows::fs::symlink_file("target", &link) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(1314) => {
            eprintln!("symlink unavailable: {error}");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    }
    repo.command(&["config", "core.symlinks", "true"]).await?;
    let reference = checkpoint_ref("file-link", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert!(
        repo.command(&["ls-tree", &reference, "--", "link"])
            .await?
            .starts_with("120000 blob ")
    );
    assert!(repo.git.restore_checkpoint(&reference, false).await?);
    assert_eq!(std::fs::read_link(&link)?, std::path::Path::new("target"));
    assert_eq!(std::fs::read(repo.root().join("target"))?, b"inside\n");
    Ok(())
}

#[tokio::test]
async fn ref_pruning_uses_one_process_and_sparse_help_is_cached_per_program() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("included/a", "base\n")?;
    repo.write("excluded/b", "keep\n")?;
    repo.commit("base").await?;
    repo.command(&[
        "sparse-checkout",
        "set",
        "--cone",
        "--sparse-index",
        "included",
    ])
    .await?;
    let executable = repo.temp.path().join(if cfg!(windows) {
        "unique-git.exe"
    } else {
        "unique-git"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_git-gate-fixture"), &executable)?;
    let trace = repo.temp.path().join("trace.json");
    let git = repo.git.clone().with_program(executable).with_environment(
        [(
            "GIT_TRACE2_EVENT".into(),
            Some(trace.as_os_str().to_owned()),
        )]
        .into(),
    );
    let references = [
        checkpoint_ref("process-counts", 0),
        checkpoint_ref("process-counts", 1),
    ];
    for reference in &references {
        git.capture_checkpoint(reference).await?;
    }
    let starts = |text: &str| -> Vec<Vec<String>> {
        text.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["event"] == "start")
            .filter_map(|event| serde_json::from_value(event["argv"].clone()).ok())
            .collect()
    };
    let commands = starts(&std::fs::read_to_string(&trace)?);
    let probes = commands
        .iter()
        .filter(|args| args.windows(2).any(|args| args == ["add", "-h"]))
        .count();
    std::fs::write(&trace, "")?;
    git.delete_checkpoint_refs(&references).await?;
    assert_eq!(
        starts(&std::fs::read_to_string(&trace)?).len(),
        1,
        "ref deletion must use one Git process"
    );
    assert_eq!(
        probes, 1,
        "sparse capability must be probed once per program"
    );
    for reference in &references {
        assert!(!repo.git.has_checkpoint(reference).await?);
    }
    Ok(())
}
