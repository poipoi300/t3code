#![allow(clippy::unwrap_used)]
mod support;
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use support::*;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    project_clone::*,
    worktree_setup::*,
};
use t3_orchestration::testing::ManualClock;
use t3_process::CancellationToken;
use t3_project::{clones::*, setup::*, *};
use tokio::sync::watch;

struct GitCloneFixture {
    source: PathBuf,
    root: PathBuf,
    gate: watch::Sender<bool>,
    ready: watch::Sender<u64>,
    succeed_on_cancel: AtomicBool,
    fail: AtomicBool,
    created: Arc<AtomicBool>,
}
#[async_trait]
impl CloneRepository for GitCloneFixture {
    async fn prepare(&self, input: &ProjectCloneStartInput) -> Result<PreparedClone> {
        Ok(PreparedClone {
            destination_id: None,
            created_repository: Default::default(),
            destination_path: input.destination_path.0.clone(),
            remote_url: "https://example.invalid/team/repo.git".into(),
            clone_url: "https://secret@example.invalid/team/repo.git?token=hidden".into(),
            repository: None,
        })
    }
    async fn clone_repository(
        &self,
        _: &ProjectCloneStartInput,
        p: &PreparedClone,
        progress: Arc<dyn Fn(CloneProgress) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<()> {
        assert!(
            self.created.load(Ordering::Acquire),
            "project must be created before Git starts"
        );
        assert!(p.clone_url.contains("secret"));
        let git = t3_git::Git::new(&self.root)
            .with_environment(git_environment(&self.root))
            .with_cancellation(cancel.clone());
        git.execute(
            &[
                "clone",
                "--",
                self.source.to_str().unwrap(),
                &p.destination_path,
            ],
            t3_git::ExecuteOptions::default(),
        )
        .await?;
        progress(CloneProgress {
            stage: ProjectCloneStage::Receiving,
            percent: Some(40),
            detail: Some("1 MiB".into()),
        });
        self.ready.send_modify(|n| *n += 1);
        let mut gate = self.gate.subscribe();
        tokio::select! {
            _=cancel.cancelled()=>if self.succeed_on_cancel.load(Ordering::Acquire) {Ok(())} else {Err(Error::Cancelled)},
            _=async {while !*gate.borrow_and_update() {gate.changed().await.unwrap();}}=>{
                if self.fail.swap(false,Ordering::AcqRel) {Err(Error::Invalid("fixture clone failed".into()))}else{Ok(())}
            }
        }
    }
    async fn discard(&self, path: &str) -> Result<()> {
        let p = std::path::Path::new(path);
        if p.exists() {
            if !p.join(".git").is_dir() {
                return Err(Error::Invalid("refusing non-clone directory".into()));
            }
            <FixtureIo as t3_project::managed::ManagedFolderIo>::remove_directory(&FixtureIo, p)
                .await?;
        }
        Ok(())
    }
}
struct Hooks {
    created: Arc<AtomicBool>,
    done: watch::Sender<bool>,
}
#[async_trait]
impl CloneHooks for Hooks {
    async fn create_project(&self, _: &ProjectCloneStartInput, _: &PreparedClone) -> Result<()> {
        self.created.store(true, Ordering::Release);
        Ok(())
    }
    async fn on_cloned(&self, _: &ProjectId, _: &str) -> Result<()> {
        self.done.send_replace(true);
        Ok(())
    }
}
async fn harness() -> (
    tempfile::TempDir,
    Arc<GitCloneFixture>,
    Arc<Hooks>,
    ProjectCloneTracker,
    ProjectCloneStartInput,
) {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let git = t3_git::Git::new(&source).with_environment(git_environment(tmp.path()));
    git.execute(&["init"], t3_git::ExecuteOptions::default())
        .await
        .unwrap();
    std::fs::write(source.join("file"), "fixture checkout").unwrap();
    git.execute(&["add", "file"], t3_git::ExecuteOptions::default())
        .await
        .unwrap();
    git.execute(
        &["commit", "-m", "fixture"],
        t3_git::ExecuteOptions::default(),
    )
    .await
    .unwrap();
    let created = Arc::new(AtomicBool::new(false));
    let backend = Arc::new(GitCloneFixture {
        source,
        root: tmp.path().into(),
        gate: watch::channel(false).0,
        ready: watch::channel(0).0,
        succeed_on_cancel: AtomicBool::new(false),
        fail: AtomicBool::new(false),
        created: created.clone(),
    });
    let hooks = Arc::new(Hooks {
        created,
        done: watch::channel(false).0,
    });
    let tracker = ProjectCloneTracker::new(
        backend.clone(),
        Arc::new(ManualClock::new("2026-01-01T00:00:00.000Z")),
    );
    let input=serde_json::from_value(serde_json::json!({"projectId":"p","title":"repo","createdAt":"2026-01-01T00:00:00.000Z","remoteUrl":"https://example.invalid/team/repo.git","destinationPath":tmp.path().join("clone")})).unwrap();
    (tmp, backend, hooks, tracker, input)
}
async fn wait_phase(
    stream: &mut watch::Receiver<Vec<ProjectCloneSnapshot>>,
    phase: ProjectClonePhase,
) {
    loop {
        if stream
            .borrow_and_update()
            .first()
            .is_some_and(|s| s.phase == phase)
        {
            return;
        }
        stream.changed().await.unwrap();
    }
}
#[tokio::test]
async fn clone_creates_first_streams_redacted_progress_and_keeps_completed_checkout() {
    let (_tmp, backend, hooks, tracker, input) = harness().await;
    let id = input.project_id.clone();
    let mut ready = backend.ready.subscribe();
    let mut stream = tracker.subscribe_project_clones();
    assert!(stream.borrow().is_empty());
    tracker.start(input.clone(), hooks.clone()).await.unwrap();
    ready.changed().await.unwrap();
    let snapshot = tracker.get(&id).unwrap();
    assert_eq!(snapshot.percent, Some(40));
    assert_eq!(snapshot.stage, ProjectCloneStage::Receiving);
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(!serialized.contains("secret"));
    assert!(!serialized.contains("hidden"));
    assert!(
        tracker
            .reject_commands_during_clone(
                &serde_json::json!({"type":"thread.create","projectId":id})
            )
            .is_err()
    );
    backend.gate.send_replace(true);
    wait_phase(&mut stream, ProjectClonePhase::Done).await;
    assert!(!tracker.cancel(&id).await.unwrap());
    tracker.discard(&id).await.unwrap();
    assert!(tracker.get(&id).is_none());
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&input.destination_path.0).join("file"))
            .unwrap(),
        "fixture checkout"
    );
    tracker.shutdown().await;
}
#[tokio::test]
async fn cancel_reaps_work_and_retry_reuses_the_claim() {
    let (_tmp, backend, hooks, tracker, input) = harness().await;
    let id = input.project_id.clone();
    let mut ready = backend.ready.subscribe();
    tracker.start(input.clone(), hooks).await.unwrap();
    ready.changed().await.unwrap();
    assert!(tracker.cancel(&id).await.unwrap());
    assert_eq!(
        tracker.get(&id).unwrap().phase,
        ProjectClonePhase::Cancelled
    );
    assert!(!std::path::Path::new(&input.destination_path.0).exists());
    assert!(!tracker.cancel(&id).await.unwrap());
    assert!(tracker.retry(&id).await.unwrap());
    ready.changed().await.unwrap();
    assert!(tracker.reject_commands_during_clone(&serde_json::json!({"type":"thread.turn.start","bootstrap":{"createThread":{"projectId":id}}})).is_err());
    backend.gate.send_replace(true);
    let mut stream = tracker.subscribe_project_clones();
    wait_phase(&mut stream, ProjectClonePhase::Done).await;
    tracker.shutdown().await;
}
#[tokio::test]
async fn cancellation_landing_after_git_success_never_discards_checkout() {
    let (_tmp, backend, hooks, tracker, input) = harness().await;
    let id = input.project_id.clone();
    let mut ready = backend.ready.subscribe();
    backend.succeed_on_cancel.store(true, Ordering::Release);
    tracker.start(input.clone(), hooks).await.unwrap();
    ready.changed().await.unwrap();
    assert!(tracker.cancel(&id).await.unwrap());
    assert_eq!(tracker.get(&id).unwrap().phase, ProjectClonePhase::Done);
    assert!(
        std::path::Path::new(&input.destination_path.0)
            .join("file")
            .exists()
    );
    tracker.shutdown().await;
}
#[tokio::test]
async fn failed_clone_retains_error_and_prevents_duplicate_destination_claims() {
    let (_tmp, backend, hooks, tracker, input) = harness().await;
    let id = input.project_id.clone();
    let mut ready = backend.ready.subscribe();
    backend.fail.store(true, Ordering::Release);
    tracker.start(input.clone(), hooks.clone()).await.unwrap();
    ready.changed().await.unwrap();
    assert!(tracker.start(input.clone(), hooks).await.is_err());
    backend.gate.send_replace(true);
    let mut stream = tracker.subscribe_project_clones();
    wait_phase(&mut stream, ProjectClonePhase::Failed).await;
    assert_eq!(
        tracker.get(&id).unwrap().error.as_deref(),
        Some("fixture clone failed")
    );
    tracker.discard(&id).await.unwrap();
    assert!(tracker.get(&id).is_none());
    tracker.shutdown().await;
}
#[tokio::test]
async fn clone_subscription_keeps_start_order_across_progress_changes() {
    let (tmp, backend, hooks, tracker, mut input) = harness().await;
    input.project_id = ProjectId("z".into());
    tracker.start(input.clone(), hooks.clone()).await.unwrap();
    input.project_id = ProjectId("a".into());
    input.destination_path = tmp
        .path()
        .join("second-clone")
        .to_string_lossy()
        .into_owned()
        .into();
    tracker.start(input, hooks).await.unwrap();
    let stream = tracker.subscribe_project_clones();
    assert_eq!(
        stream
            .borrow()
            .iter()
            .map(|s| s.project_id.0.clone())
            .collect::<Vec<_>>(),
        vec!["z", "a"]
    );
    backend.gate.send_replace(true);
    tracker.shutdown().await;
}
#[tokio::test]
async fn staged_clone_cleanup_preserves_published_and_replacement_contents() {
    let (tmp, fixture, _hooks, tracker, mut input) = harness().await;
    let backend = ForgeCloneRepository {
        registry: Arc::new(t3_forge::registry::ForgeRegistry::default()),
        git: t3_git::Git::new(tmp.path()).with_environment(git_environment(tmp.path())),
        cwd: tmp.path().into(),
        home: tmp.path().into(),
    };
    let source = fixture.source.to_string_lossy().replace('\\', "/");
    let remote = if cfg!(windows) {
        format!("file:///{source}")
    } else {
        format!("file://{source}")
    };
    input.remote_url = Some(Some(remote.into()));
    let prepared = backend.prepare(&input).await.unwrap();
    let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = progress.clone();
    backend
        .clone_repository(
            &input,
            &prepared,
            Arc::new(move |_| {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&input.destination_path.0).join("file"))
            .unwrap(),
        "fixture checkout"
    );
    assert!(progress.load(std::sync::atomic::Ordering::SeqCst) > 0);
    // A completed checkout has already left private staging. Cleanup by its former
    // destination must preserve both that checkout and anything later placed there.
    let destination = std::path::Path::new(&input.destination_path.0);
    backend.discard(&input.destination_path.0).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(destination.join("file")).unwrap(),
        "fixture checkout"
    );
    std::fs::rename(destination, tmp.path().join("published")).unwrap();
    std::fs::create_dir(destination).unwrap();
    t3_git::Git::new(destination)
        .with_environment(git_environment(tmp.path()))
        .execute(&["init"], Default::default())
        .await
        .unwrap();
    let pack = destination.join(".git/objects/incomplete");
    std::fs::write(&pack, "partial pack").unwrap();
    let foreign = destination.join("foreign");
    std::fs::write(&foreign, "user data").unwrap();
    assert!(backend.prepare(&input).await.is_err());
    backend.discard(&input.destination_path.0).await.unwrap();
    assert_eq!(std::fs::read_to_string(pack).unwrap(), "partial pack");
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "user data");
    std::fs::rename(
        destination.join(".git"),
        tmp.path().join("replacement-metadata"),
    )
    .unwrap();
    assert!(backend.prepare(&input).await.is_err());
    backend.discard(&input.destination_path.0).await.unwrap();
    assert_eq!(std::fs::read_to_string(foreign).unwrap(), "user data");
    tracker.shutdown().await;
}
#[tokio::test]
async fn clone_marks_stall_and_expires_success_only_after_retention() {
    let (_tmp, backend, hooks, tracker, input) = harness().await;
    let id = input.project_id.clone();
    let mut ready = backend.ready.subscribe();
    let mut stream = tracker.subscribe_project_clones();
    let mut completed = hooks.done.subscribe();
    tracker.start(input, hooks).await.unwrap();
    tokio::select! {
        result=ready.changed()=>{result.unwrap();}
        _=wait_phase(&mut stream,ProjectClonePhase::Failed)=>panic!("fixture clone failed before readiness: {:?}",tracker.get(&id).unwrap().error),
    }
    tokio::time::pause();
    assert_eq!(tracker.get(&id).unwrap().stalled, Some(false));
    tokio::time::advance(Duration::from_secs(20)).await;
    loop {
        if stream
            .borrow_and_update()
            .first()
            .is_some_and(|s| s.stalled == Some(true))
        {
            break;
        }
        stream.changed().await.unwrap();
    }
    backend.gate.send_replace(true);
    completed.changed().await.unwrap();
    assert_eq!(tracker.get(&id).unwrap().stalled, Some(false));
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(tracker.get(&id).is_some());
    let removed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if stream.borrow_and_update().is_empty() {
                return;
            }
            stream.changed().await.unwrap();
        }
    })
    .await;
    assert!(
        removed.is_ok(),
        "success remained published beyond its retention deadline"
    );
    assert!(tracker.get(&id).is_none());
    tracker.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn setup_orders_stages_clamps_output_and_retains_for_thirty_seconds() {
    let clock = Arc::new(ManualClock::new("2026-01-01T00:00:00.000Z"));
    let tracker = WorktreeSetupTracker::new(clock.clone());
    let id = ThreadId("t".into());
    let mut stream = tracker.subscribe_worktree_setup(&id);
    assert!(stream.borrow().is_none());
    tracker
        .begin(
            id.clone(),
            None,
            None,
            &[
                WorktreeSetupStageId::Agent,
                WorktreeSetupStageId::Fetch,
                WorktreeSetupStageId::Checkout,
            ],
            None,
        )
        .unwrap();
    let first = tracker.get(&id).unwrap();
    assert_eq!(
        first.stages.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![
            WorktreeSetupStageId::Fetch,
            WorktreeSetupStageId::Checkout,
            WorktreeSetupStageId::Agent
        ]
    );
    tracker.stage_status(
        &id,
        WorktreeSetupStageId::Fetch,
        WorktreeSetupStageStatus::Running,
        Some(Some("x".repeat(300))),
    );
    for i in 0..6 {
        tracker.append_tail(
            &id,
            WorktreeSetupStageId::Fetch,
            &format!("{i}{}", "x".repeat(500)),
        );
    }
    let running = tracker.get(&id).unwrap();
    assert_eq!(running.stages[0].detail.as_ref().unwrap().len(), 202);
    assert_eq!(running.stages[0].tail.len(), 4);
    assert!(running.stages[0].tail[0].starts_with('2'));
    tracker.finish(&id, WorktreeSetupPhase::Cancelled, None);
    assert_eq!(
        tracker.get(&id).unwrap().stages[0].status,
        WorktreeSetupStageStatus::Skipped
    );
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(tracker.get(&id).is_some());
    let expired = tokio::time::timeout(Duration::from_secs(2), async {
        while stream.borrow_and_update().is_some() {
            stream.changed().await.unwrap();
        }
    })
    .await;
    assert!(
        expired.is_ok(),
        "setup retention started after finish returned"
    );
    assert!(tracker.get(&id).is_none());
    tracker.begin(id.clone(), None, None, &[], None).unwrap();
    stream.changed().await.unwrap();
    assert!(stream.borrow().is_some());
    tracker.shutdown().await;
}
#[tokio::test]
async fn setup_cancel_waits_for_finalization_and_uncancellable_keeps_running() {
    let tracker = WorktreeSetupTracker::new(Arc::new(ManualClock::new("2026-01-01T00:00:00.000Z")));
    let id = ThreadId("t".into());
    let token = CancellationToken::new();
    let (done, receiver) = watch::channel(false);
    tracker
        .begin(
            id.clone(),
            None,
            None,
            &[],
            Some(SetupCancellation {
                token: token.clone(),
                finished: receiver,
            }),
        )
        .unwrap();
    let task = tokio::spawn(async move {
        token.cancelled().await;
        done.send_replace(true);
    });
    assert!(tracker.cancel(&id).await);
    task.await.unwrap();
    tracker.mark_uncancellable(&id);
    assert!(!tracker.cancel(&id).await);
    tracker.shutdown().await;
}

#[tokio::test]
async fn setup_progress_and_failure_do_not_publish_remote_credentials() {
    let tracker = WorktreeSetupTracker::new(Arc::new(ManualClock::new("2026-01-01T00:00:00.000Z")));
    let id = ThreadId("private-setup".into());
    tracker
        .begin(id.clone(), None, None, &[WorktreeSetupStageId::Fetch], None)
        .unwrap();
    let url = "https://fixture-user:fixture-secret@example.test/team/repo.git?token=fixture-query#fixture-fragment";
    tracker.stage_status(
        &id,
        WorktreeSetupStageId::Fetch,
        WorktreeSetupStageStatus::Running,
        Some(Some(format!("Fetching {url}"))),
    );
    tracker.update(&id, |snapshot| {
        snapshot.setup_script = Some(WorktreeSetupScript {
            name: "Prepare".into(),
            command: format!("git fetch {url}").into(),
            terminal_id: "fixture-terminal".into(),
        });
    });
    tracker.append_tail(&id, WorktreeSetupStageId::Fetch, &format!("remote: {url}"));
    tracker.finish(
        &id,
        WorktreeSetupPhase::Failed,
        Some(&format!("could not fetch {url}")),
    );
    let snapshot = tracker.get(&id).unwrap();
    let safe_url = "https://example.test/team/repo.git";
    assert_eq!(
        snapshot.stages[0].detail,
        Some(format!("Fetching {safe_url}"))
    );
    assert_eq!(snapshot.stages[0].tail, vec![format!("remote: {safe_url}")]);
    assert_eq!(snapshot.error, Some(format!("could not fetch {safe_url}")));
    assert_eq!(
        snapshot.setup_script.as_ref().unwrap().command.0,
        format!("git fetch {safe_url}")
    );
    let snapshot = serde_json::to_string(&snapshot).unwrap();
    for secret in [
        "fixture-user",
        "fixture-secret",
        "fixture-query",
        "fixture-fragment",
    ] {
        assert!(
            !snapshot.contains(secret),
            "setup snapshot exposed {secret}"
        );
    }
    tracker.shutdown().await;
}
