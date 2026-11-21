use crate::checkpoint_service_test::support::*;
use std::{path::Path, sync::Arc};
use t3_orchestration::{
    api::*, error::Result, ports::checkpoints_forks::checkpoint_restore_safety::*,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn restores_require_the_exact_worktree_and_reject_active_and_archived_overlap() -> Result<()>
{
    let parent = tempfile::tempdir()?;
    let cwd = parent.path().join("worktree");
    let nested = cwd.join("nested");
    let sibling = parent.path().join("worktree2");
    std::fs::create_dir_all(&nested)?;
    std::fs::create_dir(&sibling)?;
    for owner in [
        "nested",
        "ancestor",
        "provider",
        "sibling",
        "shared-provider",
        "scope",
        "archived",
        "deleted",
        "stopped-provider",
    ] {
        let h = Harness::new().await?;
        let current = thread("thread:test", Some(&cwd));
        h.seed("thread.created", &current).await?;
        let mut other = thread(
            "thread:other",
            Some(match owner {
                "nested" | "archived" | "deleted" => &nested,
                "ancestor" => parent.path(),
                _ => &sibling,
            }),
        );
        if owner == "archived" {
            other.archived_at = Some(time());
        }
        if owner == "deleted" {
            other.deleted_at = Some(time());
        }
        h.seed("thread.created", &other).await?;
        if owner.contains("provider") {
            let mut session = session();
            session.cwd = TrimmedNonEmptyString(nested.to_string_lossy().into_owned());
            session
                .capabilities
                .sessions
                .supports_multiple_provider_threads_per_session = owner == "shared-provider";
            session.status = if owner == "stopped-provider" {
                ProviderSessionStatus::Stopped
            } else {
                ProviderSessionStatus::Error
            };
            let event=t3_orchestration::ports::checkpoints_forks::checkpoint_capture_service::domain_event(h.ids.as_ref(),&ThreadId("thread:other".into()),&time(),"provider-session.attached",session,serde_json::json!({}))?;
            h.sink
                .commit(CommitRequest {
                    receipt: None,
                    guard: CommitGuard::None,
                    decision: Decision {
                        events: vec![event],
                        ..Default::default()
                    },
                })
                .await?;
        }
        if owner == "scope" {
            let mut other_scope = scope(&nested, 1);
            other_scope.thread_id = other.id.clone();
            h.seed("checkpoint-scope.created", other_scope).await?;
        }
        let result = is_checkpoint_restore_isolated(
            &current,
            &scope(&cwd, 1),
            h.reader.as_ref(),
            &h.db.stores,
        )
        .await?;
        assert_eq!(
            result,
            matches!(
                owner,
                "sibling" | "shared-provider" | "deleted" | "stopped-provider"
            ),
            "owner={owner}"
        );
    }
    let h = Harness::new().await?;
    assert!(
        !is_checkpoint_restore_isolated(
            &thread("thread:test", None),
            &scope(&cwd, 1),
            h.reader.as_ref(),
            &h.db.stores
        )
        .await?
    );
    assert!(
        !is_checkpoint_restore_isolated(
            &thread("thread:test", Some(&sibling)),
            &scope(&cwd, 1),
            h.reader.as_ref(),
            &h.db.stores
        )
        .await?
    );
    Ok(())
}

#[tokio::test]
async fn missing_scope_is_canonicalized_through_its_existing_ancestor() -> Result<()> {
    let parent = tempfile::tempdir()?;
    let missing = parent.path().join("gone").join("worktree");
    let h = Harness::new().await?;
    let current = thread("thread:test", Some(&missing));
    h.seed("thread.created", &current).await?;
    assert!(
        is_checkpoint_restore_isolated(
            &current,
            &scope(&missing, 1),
            h.reader.as_ref(),
            &h.db.stores
        )
        .await?
    );
    h.seed(
        "thread.created",
        thread("thread:other", Some(parent.path())),
    )
    .await?;
    assert!(
        !is_checkpoint_restore_isolated(
            &current,
            &scope(&missing, 1),
            h.reader.as_ref(),
            &h.db.stores
        )
        .await?
    );
    assert!(paths_overlap(
        &canonicalize_missing(&missing)?,
        &std::fs::canonicalize(parent.path())?
    ));
    let cwd = parent.path().join("existing-worktree");
    std::fs::create_dir(&cwd)?;
    let h = Harness::new().await?;
    let current = thread("thread:test", Some(&cwd));
    h.seed("thread.created", &current).await?;
    h.seed(
        "thread.created",
        thread("thread:other", Some(&cwd.join("not-created"))),
    )
    .await?;
    assert!(
        !is_checkpoint_restore_isolated(&current, &scope(&cwd, 1), h.reader.as_ref(), &h.db.stores)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn idle_question_only_side_chats_do_not_own_the_workspace_but_running_ones_do() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    for (running, used_tools) in [(false, false), (true, false), (false, true)] {
        let h = Harness::new().await?;
        let current = thread("thread:test", Some(temp.path()));
        h.seed("thread.created", &current).await?;
        let mut other = thread("thread:other", Some(temp.path()));
        other.lineage.side_chat = Some(Some(true));
        h.seed("thread.created", &other).await?;
        if running {
            let mut other_run = run(1, "running");
            other_run.thread_id = other.id;
            other_run.id =
                t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(RunId("run:other".into()));
            h.seed("run.created", other_run).await?;
        }
        if used_tools {
            h.seed("turn-item.updated", serde_json::json!({"id":"item:side-tool","threadId":"thread:other","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":time(),"type":"file_change","fileName":"touched.txt"})).await?;
        }
        assert_eq!(
            is_checkpoint_restore_isolated(
                &current,
                &scope(temp.path(), 1),
                h.reader.as_ref(),
                &h.db.stores
            )
            .await?,
            !running && !used_tools
        );
    }
    Ok(())
}

#[tokio::test]
async fn overlapping_restore_reservations_block_starts_and_release_on_cancellation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let reservations = Arc::new(WorkspaceReservations::default());
    let token = CancellationToken::new();
    let restore = reservations
        .reserve(temp.path(), ReservationKind::Restore, &token)
        .await?;
    let nested = temp.path().join("nested");
    let mut start = Box::pin(reservations.reserve(&nested, ReservationKind::TurnStart, &token));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(start.as_mut().poll(cx).is_pending()))
            .await
    );
    drop(restore);
    let start = start.await?;
    let sibling = temp.path().with_extension("sibling");
    let _other = reservations
        .reserve(&sibling, ReservationKind::Restore, &token)
        .await?;
    let cancelled = CancellationToken::new();
    let mut pending =
        Box::pin(reservations.reserve(temp.path(), ReservationKind::Restore, &cancelled));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(pending.as_mut().poll(cx).is_pending()))
            .await
    );
    cancelled.cancel();
    assert!(matches!(
        pending.await,
        Err(t3_orchestration::error::Error::Cancelled)
    ));
    drop(start);
    let _restored = reservations
        .reserve(temp.path(), ReservationKind::Restore, &token)
        .await?;
    Ok(())
}

#[test]
fn scope_boundaries_are_component_aware() {
    assert!(!paths_overlap(
        Path::new("/repo/tree"),
        Path::new("/repo/tree2")
    ));
    assert!(paths_overlap(
        Path::new("/repo/tree"),
        Path::new("/repo/tree/nested")
    ));
}

#[test]
fn managed_recovery_requires_an_absolute_normalized_two_segment_path() -> Result<()> {
    let root = tempfile::tempdir()?;
    let roots = vec![root.path().to_path_buf()];
    assert!(is_managed_worktree_path(
        &root.path().join("repo").join("branch"),
        &roots
    )?);
    assert!(!is_managed_worktree_path(root.path(), &roots)?);
    assert!(!is_managed_worktree_path(
        &root.path().join("repo"),
        &roots
    )?);
    assert!(!is_managed_worktree_path(
        &root.path().join("repo").join("branch").join("nested"),
        &roots
    )?);
    assert!(!is_managed_worktree_path(
        &root.path().join("repo").join("..").join("branch"),
        &roots
    )?);
    assert!(!is_managed_worktree_path(
        Path::new("relative/repo/branch"),
        &roots
    )?);
    let filesystem_root = root.path().ancestors().last().expect("root").to_path_buf();
    assert!(!is_managed_worktree_path(
        &filesystem_root.join("repo").join("branch"),
        &[filesystem_root]
    )?);
    Ok(())
}

#[tokio::test]
async fn startup_excludes_restore_and_cleanup_until_its_handle_is_returned() -> Result<()> {
    use t3_orchestration::ports::checkpoints_forks::checkpoint_service::WorkspaceLocks;
    let root = tempfile::tempdir()?;
    let reservations = Arc::new(WorkspaceReservations::default());
    let leases = Arc::new(WorkspaceLocks::default());
    let coordination = WorkspaceCoordination::new(reservations.clone(), leases);
    let cancellation = CancellationToken::new();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (finish, finishing) = tokio::sync::oneshot::channel();
    let cwd = root.path().to_path_buf();
    let task = {
        let coordination = coordination.clone();
        let cancellation = cancellation.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            coordination
                .start(Some(&cwd), Some(&cwd), &cancellation, async {
                    entered.send(()).expect("entered");
                    finishing.await.expect("finish");
                    Ok("execution handle")
                })
                .await
        })
    };
    started.await.expect("started");
    let backend = Arc::new(t3_orchestration::testing::FakeWorkspaceBackend::default());
    backend.remove.push(Ok(()));
    let workspaces = CoordinatedWorkspaces::new(backend.clone(), coordination.clone());
    let mut cleanup = Box::pin(workspaces.remove(
        Workspace {
            cwd: cwd.clone(),
            branch: None,
            worktree_path: Some(cwd.clone()),
        },
        cancellation.clone(),
    ));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(cleanup.as_mut().poll(cx).is_pending()))
            .await
    );
    let nested = cwd.join("nested");
    let mut restore =
        Box::pin(reservations.reserve(&nested, ReservationKind::Restore, &cancellation));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(restore.as_mut().poll(cx).is_pending()))
            .await
    );
    finish.send(()).expect("release");
    assert_eq!(task.await.expect("startup")?, "execution handle");
    cleanup.await?;
    let (removed, _) = backend.remove.next_call().await.expect("removal");
    assert_eq!(removed.worktree_path.as_deref(), Some(cwd.as_path()));
    drop(restore.await?);
    let cleanup_lease = coordination.cleanup(&cwd, &cancellation, async { Ok(()) });
    cleanup_lease.await?;
    Ok(())
}

#[tokio::test]
async fn cancelled_start_waiting_for_cleanup_never_runs_startup_or_leaks_reservation() -> Result<()>
{
    use t3_orchestration::ports::checkpoints_forks::checkpoint_service::WorkspaceLocks;
    let root = tempfile::tempdir()?;
    let reservations = Arc::new(WorkspaceReservations::default());
    let leases = Arc::new(WorkspaceLocks::default());
    let coordination = WorkspaceCoordination::new(reservations.clone(), leases.clone());
    let held = leases.acquire(root.path()).await?;
    assert_eq!(
        coordination
            .start(Some(root.path()), None, &CancellationToken::new(), async {
                Ok("root startup")
            })
            .await?,
        "root startup"
    );
    let cancellation = CancellationToken::new();
    let mut start =
        Box::pin(
            coordination.start(Some(root.path()), Some(root.path()), &cancellation, async {
                Err::<(), _>(t3_orchestration::error::Error::Unsupported(
                    "cancelled startup ran",
                ))
            }),
        );
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(start.as_mut().poll(cx).is_pending()))
            .await
    );
    cancellation.cancel();
    assert!(matches!(
        start.await,
        Err(t3_orchestration::error::Error::Cancelled)
    ));
    drop(held);
    let _restore = reservations
        .reserve(
            root.path(),
            ReservationKind::Restore,
            &CancellationToken::new(),
        )
        .await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn recovery_keeps_the_named_workspace_leased_after_unlinking_a_leftover() -> Result<()> {
    use t3_orchestration::ports::checkpoints_forks::checkpoint_service::WorkspaceLocks;
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("leftover");
    let named = temp.path().join("checkout");
    std::fs::create_dir(&target)?;
    std::os::unix::fs::symlink(&target, &named)?;
    let locks = WorkspaceLocks::default();
    let recovery = locks.acquire_for_recovery(&named).await?;
    use t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::{
        ReservationKind, WorkspaceReservations,
    };
    let reservations = std::sync::Arc::new(WorkspaceReservations::default());
    let token = tokio_util::sync::CancellationToken::new();
    let start = reservations
        .reserve(&named, ReservationKind::TurnStart, &token)
        .await?;
    std::fs::remove_file(&named)?;
    let normal = locks.acquire(&named);
    tokio::pin!(normal);
    tokio::select! { biased; _ = &mut normal => panic!("new lease entered during recovery"), () = std::future::ready(()) => {} }
    let rollback = reservations.reserve(&named, ReservationKind::Restore, &token);
    tokio::pin!(rollback);
    tokio::select! { biased; _ = &mut rollback => panic!("rollback entered during startup"), () = std::future::ready(()) => {} }
    drop(start);
    let _reserved = rollback.await?;
    drop(recovery);
    let _acquired = normal.await?;
    Ok(())
}

#[tokio::test]
async fn reserving_a_missing_checkout_covers_case_variants_of_its_unresolved_name() -> Result<()> {
    let parent = tempfile::tempdir()?;
    let reservations = WorkspaceReservations::global();
    let cancel = CancellationToken::new();
    let restore = reservations
        .reserve(
            &parent.path().join("Future"),
            ReservationKind::Restore,
            &cancel,
        )
        .await?;
    let alternate = parent.path().join("future");
    let launch = reservations.reserve(&alternate, ReservationKind::TurnStart, &cancel);
    tokio::pin!(launch);
    tokio::select! {
        biased;
        result = &mut launch => panic!("an alternate spelling claimed the missing checkout: {}", result.is_ok()),
        _ = std::future::ready(()) => {}
    }
    drop(restore);
    launch.await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_waiting_claim_rechecks_its_alias_after_it_wakes() -> Result<()> {
    let parent = tempfile::tempdir()?;
    let a = parent.path().join("a");
    let b = parent.path().join("b");
    let link = parent.path().join("link");
    std::fs::create_dir(&a)?;
    std::fs::create_dir(&b)?;
    std::os::unix::fs::symlink(&a, &link)?;
    let reservations = WorkspaceReservations::global();
    let cancel = CancellationToken::new();
    let first = reservations
        .reserve(&a, ReservationKind::Restore, &cancel)
        .await?;
    let second = reservations
        .reserve(&b, ReservationKind::Restore, &cancel)
        .await?;
    let waiter = reservations.reserve(&link, ReservationKind::TurnStart, &cancel);
    tokio::pin!(waiter);
    tokio::select! { biased; _ = &mut waiter => panic!("first restore bypassed"), _ = std::future::ready(()) => {} }
    std::fs::remove_file(&link)?;
    std::os::unix::fs::symlink(&b, &link)?;
    drop(first);
    tokio::select! { biased; _ = &mut waiter => panic!("second restore bypassed"), _ = std::future::ready(()) => {} }
    drop(second);
    waiter.await?;
    Ok(())
}
