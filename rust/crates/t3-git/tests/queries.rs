mod common;
use common::{Repository, TestResult};
use t3_git::{
    CheckpointContext, CheckpointRecord, CheckpointRun, CheckpointScope, DiffFormat, DiffOptions,
    checkpoint_ref, parse_numstat, truncate_patch,
};

#[test]
fn parses_numstat_renames_binary_and_nontrivial_names() {
    let files = parse_numstat("3\t2\tregular\0-\t-\tbinary\09\t4\t\0old name\0new\tname\0bad\0");
    assert_eq!(files.len(), 3);
    let renamed = files
        .iter()
        .find(|file| file.previous_path.is_some())
        .expect("rename");
    assert_eq!(renamed.path, "new\tname");
    assert_eq!(renamed.previous_path.as_deref(), Some("old name"));
    assert_eq!((renamed.additions, renamed.deletions), (9, 4));
    assert_eq!(files[0].path, "binary");
    assert_eq!(files[0].additions, 0);
    assert_eq!(truncate_patch("exact", 5), "exact");
    let patch = truncate_patch(&"é".repeat(30), 30);
    assert!(patch.len() <= 30);
    assert!(patch.ends_with("\n\n[truncated]"));
    assert!(!patch.contains('\u{fffd}'));
}

#[tokio::test]
async fn rename_detection_and_whitespace_follow_git_config() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("old name", "line one\nline two\nline three\n")?;
    repo.commit("initial").await?;
    let first = checkpoint_ref("rename", 0);
    let second = checkpoint_ref("rename", 1);
    repo.git.capture_checkpoint(&first).await?;
    std::fs::rename(repo.root().join("old name"), repo.root().join("new name"))?;
    repo.git.capture_checkpoint(&second).await?;
    let mut options = DiffOptions::new(&first, &second);
    options.format = DiffFormat::Numstat;
    let files = parse_numstat(&repo.git.diff_checkpoints(options.clone()).await?);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].previous_path.as_deref(), Some("old name"));
    repo.command(&["config", "diff.renames", "false"]).await?;
    assert_eq!(
        parse_numstat(&repo.git.diff_checkpoints(options).await?).len(),
        2
    );
    let third = checkpoint_ref("rename", 2);
    repo.write("new name", "  line one\n line two\nline three\n")?;
    repo.git.capture_checkpoint(&third).await?;
    assert_eq!(
        repo.git
            .diff_checkpoints(DiffOptions::new(&second, &third))
            .await?,
        ""
    );
    let mut options = DiffOptions::new(&second, &third);
    options.ignore_whitespace = false;
    assert!(!repo.git.diff_checkpoints(options).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn imported_head_changes_are_filtered_and_authored_changes_survive() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("authored", "base\n")?;
    repo.write("imported", "base\n")?;
    repo.commit("initial").await?;
    let first = checkpoint_ref("authored", 0);
    let second = checkpoint_ref("authored", 1);
    repo.write("authored", "working before pull\n")?;
    repo.git.capture_checkpoint(&first).await?;
    // An old author timestamp excludes the upstream commit from work authored since capture.
    repo.write("imported", "upstream\n")?;
    repo.command(&["add", "imported"]).await?;
    let mut options = t3_git::ExecuteOptions::default();
    options.env.insert(
        "GIT_COMMITTER_DATE".into(),
        Some("2000-01-01T00:00:00Z".into()),
    );
    options.env.insert(
        "GIT_AUTHOR_DATE".into(),
        Some("2000-01-01T00:00:00Z".into()),
    );
    repo.git
        .execute(&["commit", "-m", "upstream import"], options)
        .await?;
    repo.write("authored", "working after pull\n")?;
    repo.git.capture_checkpoint(&second).await?;
    let authored = repo
        .git
        .list_authored_paths(&first, &second)
        .await?
        .expect("HEAD moved");
    assert!(authored.contains("authored"));
    assert!(!authored.contains("imported"));
    let raw = repo
        .git
        .diff_checkpoints(DiffOptions::new(&first, &second))
        .await?;
    assert!(raw.contains("+upstream"));
    let filtered = repo
        .git
        .filtered_checkpoint_diff(&first, &second, true, t3_git::DIFF_MAX_BYTES)
        .await?;
    assert!(filtered.contains("+working after pull"));
    assert!(!filtered.contains("imported"));
    Ok(())
}

#[tokio::test]
async fn turn_queries_use_stopped_predecessor_completed_destinations_and_scope_cwd() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "baseline\n")?;
    repo.commit("initial").await?;
    let baseline = checkpoint_ref("scope", 0);
    repo.git.capture_checkpoint(&baseline).await?;
    let first = checkpoint_ref("scope", 1);
    repo.write("a", "first\n")?;
    repo.git.capture_checkpoint(&first).await?;
    let stopped = checkpoint_ref("scope", 2);
    repo.write("a", "stopped\n")?;
    repo.git.capture_checkpoint(&stopped).await?;
    let last = checkpoint_ref("scope", 3);
    repo.write("a", "completed\n")?;
    repo.git.capture_checkpoint(&last).await?;
    let context = CheckpointContext {
        thread_id: "thread".into(),
        runs: vec![
            CheckpointRun {
                id: "run1".into(),
                completed: true,
            },
            CheckpointRun {
                id: "run2".into(),
                completed: true,
            },
        ],
        scopes: vec![CheckpointScope {
            id: "scope".into(),
            cwd: repo.root().into(),
            root_run: true,
        }],
        checkpoints: vec![
            CheckpointRecord {
                reference: first,
                scope_id: "scope".into(),
                run_id: Some("run1".into()),
                ready: true,
                ordinal_within_scope: 1,
                app_run_ordinal: Some(1),
            },
            CheckpointRecord {
                reference: stopped,
                scope_id: "scope".into(),
                run_id: None,
                ready: true,
                ordinal_within_scope: 2,
                app_run_ordinal: None,
            },
            CheckpointRecord {
                reference: last,
                scope_id: "scope".into(),
                run_id: Some("run2".into()),
                ready: true,
                ordinal_within_scope: 3,
                app_run_ordinal: Some(2),
            },
        ],
    };
    let diff = repo.git.get_turn_diff(&context, 1, 2, None).await?;
    assert!(diff.diff.contains("-stopped"));
    assert!(!diff.diff.contains("-first"));
    let full = repo.git.get_full_thread_diff(&context, 2, None).await?;
    assert!(full.diff.contains("-baseline"));
    assert!(repo.git.get_turn_diff(&context, 0, 3, None).await.is_err());
    assert!(
        repo.git
            .get_turn_diff(&context, 9, 9, None)
            .await?
            .diff
            .is_empty()
    );
    assert!(
        repo.git
            .get_full_thread_diff(&context, 0, None)
            .await?
            .diff
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn filtered_path_batches_share_the_byte_budget_and_preserve_utf8_at_exact_fit() -> TestResult
{
    let repo = Repository::new().await?;
    repo.write("base", "base\n")?;
    repo.commit("base").await?;
    let from = checkpoint_ref("batch-budget", 0);
    let to = checkpoint_ref("batch-budget", 1);
    repo.git.capture_checkpoint(&from).await?;
    repo.write("upstream.txt", "upstream import\n")?;
    repo.command(&["add", "upstream.txt"]).await?;
    let mut options = t3_git::ExecuteOptions::default();
    for key in ["GIT_COMMITTER_DATE", "GIT_AUTHOR_DATE"] {
        options
            .env
            .insert(key.into(), Some("2000-01-01T00:00:00Z".into()));
    }
    repo.git
        .execute(&["commit", "-m", "old import"], options)
        .await?;
    for n in 0..1600 {
        repo.write(
            &format!("authored/file-{n:04}-long-path-for-batching.txt"),
            format!("{n}:{}\n", "é".repeat(6000)),
        )?;
    }
    repo.git.capture_checkpoint(&to).await?;
    let full = repo
        .git
        .filtered_checkpoint_diff(&from, &to, false, 30_000_000)
        .await?;
    assert!(full.len() > t3_git::DIFF_MAX_BYTES);
    assert!(!full.contains("upstream.txt"));
    assert!(full.contains("file-1599"));
    let truncated = repo
        .git
        .filtered_checkpoint_diff(&from, &to, false, t3_git::DIFF_MAX_BYTES)
        .await?;
    assert_eq!(truncated, truncate_patch(&full, t3_git::DIFF_MAX_BYTES));
    assert!(!truncated.contains('\u{fffd}'));
    assert!(truncated.ends_with(t3_git::TRUNCATED_MARKER));
    let exact = repo
        .git
        .filtered_checkpoint_diff(&from, &to, false, full.len())
        .await?;
    assert_eq!(exact, full);
    assert!(!exact.ends_with(t3_git::TRUNCATED_MARKER));
    Ok(())
}
