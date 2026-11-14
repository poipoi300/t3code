use t3_contracts::{base_schemas::ThreadId, worktree_setup::*};
use t3_git::{SetupTask, WorktreeSetupTracker};

#[tokio::test(start_paused = true)]
async fn stages_clamp_output_slide_and_expire_without_removing_a_replacement() {
    let tracker = WorktreeSetupTracker::new();
    let id = ThreadId("thread".into());
    let mut stream = tracker.stream(&id);
    assert!(stream.recv().await.expect("initial").is_none());
    tracker.begin(
        id.clone(),
        Some("feature".into()),
        Some("main".into()),
        &[
            WorktreeSetupStageId::Agent,
            WorktreeSetupStageId::Checkout,
            WorktreeSetupStageId::Fetch,
        ],
        None,
    );
    let initial = stream.recv().await.expect("begin").expect("snapshot");
    assert_eq!(
        initial.stages.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![
            WorktreeSetupStageId::Fetch,
            WorktreeSetupStageId::Checkout,
            WorktreeSetupStageId::Agent
        ]
    );
    tracker.stage_status(
        &id,
        WorktreeSetupStageId::Checkout,
        WorktreeSetupStageStatus::Running,
        Some(Some("x".repeat(2000))),
    );
    for line in 0..6 {
        tracker.append_tail(&id, WorktreeSetupStageId::Checkout, &format!("line {line}"));
    }
    tracker.stage(&id, WorktreeSetupStageId::Checkout, |stage| {
        stage.percent = Some(42)
    });
    let latest = stream.recv().await.expect("latest").expect("snapshot");
    assert!(latest.sequence > initial.sequence);
    assert_eq!(
        latest.stages[1].tail,
        ["line 2", "line 3", "line 4", "line 5"]
    );
    assert_eq!(
        latest.stages[1]
            .detail
            .as_ref()
            .expect("detail")
            .encode_utf16()
            .count(),
        200
    );
    let failed = tracker
        .finish(&id, WorktreeSetupPhase::Failed, Some("boom"))
        .expect("finished");
    assert_eq!(failed.stages[1].status, WorktreeSetupStageStatus::Failed);
    assert_eq!(failed.stages[2].status, WorktreeSetupStageStatus::Pending);
    assert!(failed.stages[1].started_at.is_some() && failed.stages[1].ended_at.is_some());
    tracker.begin(id.clone(), None, None, &[WorktreeSetupStageId::Agent], None);
    assert!(tracker.get(&id).expect("new").sequence > failed.sequence);
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    assert_eq!(
        tracker.get(&id).expect("replacement survives").phase,
        WorktreeSetupPhase::Running
    );
    tracker.finish(&id, WorktreeSetupPhase::Done, None);
    stream.recv().await.expect("done");
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    assert!(stream.recv().await.expect("expired").is_none());
    assert!(tracker.get(&id).is_none());
    tracker.begin(id.clone(), None, None, &[], None);
    assert_eq!(
        stream
            .recv()
            .await
            .expect("restarted")
            .expect("snapshot")
            .sequence,
        0
    );
}

#[tokio::test]
async fn cancel_awaits_task_cleanup_and_uncancellable_setup_keeps_running() {
    let tracker = WorktreeSetupTracker::new();
    let id = ThreadId("thread".into());
    let (cleaned, mut cleanup) = tokio::sync::mpsc::channel(1);
    let (task, handle) = SetupTask::spawn(move |cancel| async move {
        cancel.cancelled().await;
        cleaned.send(()).await.expect("cleanup");
    });
    tracker.begin(id.clone(), None, None, &[], Some(task));
    assert!(tracker.cancel(&id).await);
    cleanup.recv().await.expect("cleanup completed");
    handle.await.expect("joined");
    tracker.finish(&id, WorktreeSetupPhase::Cancelled, None);
    assert!(!tracker.cancel(&id).await);
    let (task, handle) = SetupTask::spawn(|cancel| async move { cancel.cancelled().await });
    let cancel = task.cancellation().clone();
    tracker.begin(id.clone(), None, None, &[], Some(task));
    tracker.mark_uncancellable(&id);
    assert!(!tracker.cancel(&id).await);
    assert!(!cancel.is_cancelled());
    cancel.cancel();
    handle.await.expect("joined");
    assert!(!tracker.cancel(&ThreadId("missing".into())).await);
}
