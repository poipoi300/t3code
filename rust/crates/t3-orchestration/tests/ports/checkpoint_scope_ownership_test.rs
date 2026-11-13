use crate::checkpoint_service_test::support::*;
use std::sync::Arc;
use t3_orchestration::{
    api::*, error::Result, ports::checkpoints_forks::checkpoint_restore_safety::expanded_id,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn second_root_run_replaces_scope_ownership_without_losing_the_thread_baseline() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let prepared = temp.path().join("prepared");
    std::fs::create_dir(&prepared)?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let service = backend.service();
    let first = h.seed_run(temp.path(), 1, "completed").await?;
    service
        .capture_baseline(first.clone(), 0, CancellationToken::new())
        .await?;
    let baseline = service
        .materialize_baseline(capture_input(first.clone(), 0))
        .await?;
    h.seed("checkpoint.captured", &baseline).await?;
    let checkpoint = service
        .capture(capture_input(first.clone(), 1), CancellationToken::new())
        .await?;
    h.seed("checkpoint.captured", checkpoint).await?;
    let second = h.seed_run(&prepared, 2, "completed").await?;
    assert_eq!(first.id, second.id);
    let checkpoint = service
        .capture(capture_input(second.clone(), 2), CancellationToken::new())
        .await?;
    h.seed("checkpoint.captured", checkpoint).await?;
    let records = h
        .reader
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::Checkpoints, RecordKind::CheckpointScopes],
            RecordFilter::default(),
        )
        .await?;
    let ProjectionRecordSet::Checkpoints(checkpoints) = &records.records[0] else {
        panic!("checkpoints")
    };
    assert_eq!(
        checkpoints
            .iter()
            .map(|c| c.app_run_ordinal)
            .collect::<Vec<_>>(),
        vec![None, Some(1), Some(2)]
    );
    assert_eq!(checkpoints[0].id, baseline.id);
    let ProjectionRecordSet::CheckpointScopes(scopes) = &records.records[1] else {
        panic!("scopes")
    };
    assert_eq!(scopes.len(), 1);
    assert_eq!(
        scopes[0].run_id.as_ref().map(expanded_id).transpose()?,
        Some(RunId("run:2".into()))
    );
    assert_eq!(scopes[0].cwd.0, prepared.to_string_lossy());
    Ok(())
}
#[cfg(feature = "git-backend")]
#[tokio::test]
async fn full_thread_diff_uses_baseline_and_latest_scope_after_second_run_moves_worktree()
-> Result<()> {
    use std::{collections::BTreeMap, ffi::OsString};
    use t3_git::{CheckpointContext, CheckpointRecord, CheckpointRun, CheckpointScope, Git};
    use t3_orchestration::{
        ports::checkpoints_forks::checkpoint_service::{CheckpointService, WorkspaceLocks},
        runtime::checkpoints::GitCheckpointBackend,
    };
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    std::fs::create_dir(&root)?;
    let mut environment = BTreeMap::<OsString, Option<OsString>>::new();
    for key in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] {
        environment.insert(key.into(), Some(temp.path().as_os_str().to_owned()));
    }
    for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        environment.insert(
            key.into(),
            Some(temp.path().join("no-config").into_os_string()),
        );
    }
    for (key, value) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_COUNT", "0"),
        ("GIT_AUTHOR_NAME", "Test"),
        ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
        ("GIT_COMMITTER_NAME", "Test"),
        ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
    ] {
        environment.insert(key.into(), Some(value.into()));
    }
    for key in ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG", "GIT_TEMPLATE_DIR"] {
        environment.insert(key.into(), None);
    }
    let git = Git::new(&root).with_environment(environment.clone());
    let git_error = |error: t3_git::GitError| t3_orchestration::error::Error::Service {
        operation: "scope-test.git",
        detail: error.to_string(),
    };
    git.execute(&["init", "--initial-branch=main"], Default::default())
        .await
        .map_err(git_error)?;
    git.execute(&["config", "core.autocrlf", "false"], Default::default())
        .await
        .map_err(git_error)?;
    git.execute(
        &["commit", "--allow-empty", "-m", "initial"],
        Default::default(),
    )
    .await
    .map_err(git_error)?;
    let backend = Arc::new(GitCheckpointBackend::new(move |cwd| {
        Git::new(cwd).with_environment(environment.clone())
    }));
    let checkpoints = CheckpointService::new(
        backend.clone(),
        backend,
        Arc::new(WorkspaceLocks::default()),
    );
    let h = Harness::new().await?;
    let first = h.seed_run(&root, 1, "completed").await?;
    std::fs::write(root.join("file.txt"), "baseline\n")?;
    checkpoints
        .capture_baseline(first.clone(), 0, CancellationToken::new())
        .await?;
    let baseline = checkpoints
        .materialize_baseline(capture_input(first.clone(), 0))
        .await?;
    h.seed("checkpoint.captured", baseline).await?;
    std::fs::write(root.join("file.txt"), "first run\n")?;
    h.seed(
        "checkpoint.captured",
        checkpoints
            .capture(capture_input(first, 1), CancellationToken::new())
            .await?,
    )
    .await?;
    let prepared = temp.path().join("prepared");
    let mut worktree = t3_git::WorktreeOptions::new(prepared.clone(), "main");
    worktree.new_branch = Some("feature".into());
    git.add_worktree(worktree).await.map_err(git_error)?;
    std::fs::write(prepared.join("file.txt"), "second run\n")?;
    let second = h.seed_run(&prepared, 2, "completed").await?;
    h.seed(
        "checkpoint.captured",
        checkpoints
            .capture(capture_input(second, 2), CancellationToken::new())
            .await?,
    )
    .await?;
    let records = h
        .reader
        .records(
            ThreadId("thread:test".into()),
            vec![
                RecordKind::Runs,
                RecordKind::Checkpoints,
                RecordKind::CheckpointScopes,
            ],
            RecordFilter::default(),
        )
        .await?;
    let ProjectionRecordSet::Runs(runs) = &records.records[0] else {
        panic!("runs")
    };
    let ProjectionRecordSet::Checkpoints(cps) = &records.records[1] else {
        panic!("checkpoints")
    };
    let ProjectionRecordSet::CheckpointScopes(scopes) = &records.records[2] else {
        panic!("scopes")
    };
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].cwd.0, prepared.to_string_lossy());
    let context = CheckpointContext {
        thread_id: "thread:test".into(),
        runs: runs
            .iter()
            .map(|r| {
                Ok(CheckpointRun {
                    id: expanded_id(&r.id)?.0,
                    completed: r.status == OrchestrationV2RunStatus::Completed,
                })
            })
            .collect::<Result<_>>()?,
        scopes: scopes
            .iter()
            .map(|s| {
                Ok(CheckpointScope {
                    id: expanded_id(&s.id)?.0,
                    cwd: s.cwd.0.clone().into(),
                    root_run: s.kind == CheckpointScopeKind::RootRun,
                })
            })
            .collect::<Result<_>>()?,
        checkpoints: cps
            .iter()
            .map(|cp| {
                Ok(CheckpointRecord {
                    reference: cp.r#ref.0.clone(),
                    scope_id: expanded_id(&cp.scope_id)?.0,
                    run_id: cp
                        .run_id
                        .as_ref()
                        .map(expanded_id)
                        .transpose()?
                        .map(|id| id.0),
                    ready: cp.status == CheckpointStatus::Ready,
                    ordinal_within_scope: cp.ordinal_within_scope,
                    app_run_ordinal: cp.app_run_ordinal,
                })
            })
            .collect::<Result<_>>()?,
    };
    let full = git
        .get_full_thread_diff(&context, 2, Some(false))
        .await
        .map_err(git_error)?;
    assert_eq!(full.from_turn_count, 0);
    assert_eq!(full.to_turn_count, 2);
    assert!(full.diff.contains("-baseline"));
    assert!(full.diff.contains("+second run"));
    assert!(!full.diff.contains("+first run"));
    Ok(())
}
