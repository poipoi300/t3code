use super::provider_event_ingestor_test::{Harness, fixture};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::runs_ingestion::run_finalization_service::*,
};
use tokio_util::sync::CancellationToken;

pub struct Capture {
    pub calls: Arc<Mutex<Vec<&'static str>>>,
    pub fail: bool,
}
#[async_trait]
impl FinalizationCapture for Capture {
    async fn capture(
        &self,
        _target: FinalizationTarget,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        self.calls.lock().expect("test value").push("capture");
        if self.fail {
            Err(Error::Service {
                operation: "capture",
                detail: "checkpoint storage unavailable".into(),
            })
        } else {
            Ok(())
        }
    }
}
pub struct Refresh(pub Arc<Mutex<Vec<&'static str>>>);
#[async_trait]
impl FinalizationObserver for Refresh {
    async fn refresh(&self, cwd: String, _thread_id: ThreadId, _run_id: RunId) -> Result<()> {
        assert_eq!(cwd, "/repo");
        self.0.lock().expect("test value").push("refresh");
        Err(Error::Service {
            operation: "refresh",
            detail: "Git command timed out".into(),
        })
    }
}
pub async fn service(
    h: &Harness,
    fail: bool,
) -> Result<(RunFinalizationService, Arc<Mutex<Vec<&'static str>>>)> {
    let mut scope = fixture("OrchestrationV2CheckpointScope");
    scope["id"] = "scope:test".into();
    scope["threadId"] = "thread:test".into();
    scope["cwd"] = "/repo".into();
    h.write("checkpoint-scope.created", scope).await?;
    let calls = Arc::new(Mutex::new(Vec::new()));
    Ok((
        RunFinalizationService {
            capture: Arc::new(Capture {
                calls: calls.clone(),
                fail,
            }),
            projections: h.reader.clone(),
            observer: Arc::new(Refresh(calls.clone())),
        },
        calls,
    ))
}
pub fn target() -> FinalizationTarget {
    FinalizationTarget {
        thread_id: ThreadId("thread:test".into()),
        run_id: RunId("run:test".into()),
        scope_id: CheckpointScopeId("scope:test".into()),
    }
}

#[tokio::test]
async fn a_postcapture_refresh_failure_succeeds_but_a_capture_failure_remains_retryable()
-> Result<()> {
    for fail in [false, true] {
        let h = Harness::new().await?;
        let (service, calls) = service(&h, fail).await?;
        let result = service.finalize(target(), CancellationToken::new()).await;
        if fail {
            let error = result.unwrap_err();
            assert_eq!(error.operation, "capture-checkpoint");
            assert!(error.to_string().contains("checkpoint storage unavailable"));
            assert_eq!(*calls.lock().unwrap(), vec!["capture"]);
        } else {
            result.unwrap();
            assert_eq!(*calls.lock().unwrap(), vec!["capture", "refresh"]);
        }
    }
    Ok(())
}

struct PostBackend {
    reference: Option<String>,
    is_default: bool,
    missing_identity: bool,
    block_identity: bool,
    calls: std::sync::atomic::AtomicUsize,
    identity_calls: std::sync::atomic::AtomicUsize,
    dropped: tokio::sync::mpsc::UnboundedSender<&'static str>,
}
struct Dropped(
    tokio::sync::mpsc::UnboundedSender<&'static str>,
    &'static str,
);
impl Drop for Dropped {
    fn drop(&mut self) {
        let _ = self.0.send(self.1);
    }
}
#[async_trait]
impl PostTurnBackend for PostBackend {
    async fn refresh_entries(&self, _cwd: String) -> Result<()> {
        Ok(())
    }
    async fn refresh_local_status(&self, _cwd: String) -> Result<LocalWorkspaceStatus> {
        Ok(LocalWorkspaceStatus {
            reference: self.reference.clone(),
            is_default: self.is_default,
        })
    }
    async fn refresh_pull_request_status(&self, _cwd: String) -> Result<()> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            let _drop = Dropped(self.dropped.clone(), "pr");
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    async fn refresh_after_turn(&self, _project_id: ProjectId) -> Result<()> {
        Ok(())
    }
    async fn project_workspace_root(&self, _project_id: ProjectId) -> Result<Option<String>> {
        Ok(Some("/repo".into()))
    }
    async fn has_repository_identity(&self, _workspace_root: String) -> Result<bool> {
        Ok(!self.missing_identity)
    }
    async fn refresh_repository_identity(&self, _workspace_root: String) -> Result<()> {
        self.identity_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.block_identity {
            let _drop = Dropped(self.dropped.clone(), "identity");
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}
async fn observer(
    checked_out: Option<&str>,
    branch: Option<&str>,
    newer_run: bool,
    missing_identity: bool,
    block_identity: bool,
) -> Result<(
    WorkspaceFinalizationObserver,
    Arc<PostBackend>,
    tokio::sync::mpsc::UnboundedReceiver<&'static str>,
)> {
    let h = Harness::new().await?;
    let mut thread = fixture("OrchestrationV2AppThread");
    thread["id"] = "thread:test".into();
    thread["projectId"] = "project:test".into();
    thread["branch"] = serde_json::json!(branch);
    h.write("thread.created", thread).await?;
    if newer_run {
        let mut run = fixture("OrchestrationV2Run");
        run["threadId"] = "thread:test".into();
        run["id"] = "newer".into();
        run["status"] = "running".into();
        h.write("run.updated", run).await?;
    }
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let backend = Arc::new(PostBackend {
        reference: checked_out.map(str::to_owned),
        is_default: checked_out == Some("main"),
        missing_identity,
        block_identity,
        calls: std::sync::atomic::AtomicUsize::new(0),
        identity_calls: std::sync::atomic::AtomicUsize::new(0),
        dropped: sender,
    });
    Ok((
        WorkspaceFinalizationObserver::new(backend.clone(), h.reader.clone()),
        backend,
        receiver,
    ))
}
#[tokio::test]
async fn post_turn_forge_refresh_requires_the_matching_nondefault_branch_and_no_newer_active_run()
-> Result<()> {
    for (checked_out, branch, newer, expected) in [
        (Some("feature"), Some("feature"), false, 1),
        (Some("main"), Some("main"), false, 0),
        (Some("other"), Some("feature"), false, 0),
        (None, Some("feature"), false, 0),
        (Some("feature"), Some("feature"), true, 0),
    ] {
        let (observer, backend, _) = observer(checked_out, branch, newer, false, false).await?;
        observer
            .refresh(
                "/repo".into(),
                ThreadId("thread:test".into()),
                RunId("completed".into()),
            )
            .await?;
        assert_eq!(
            backend.calls.load(std::sync::atomic::Ordering::SeqCst),
            expected
        );
        observer.shutdown().await;
    }
    Ok(())
}
#[tokio::test]
async fn a_new_turn_replaces_a_slow_forge_lookup_and_shutdown_joins_it() -> Result<()> {
    let (observer, backend, mut dropped) =
        observer(Some("feature"), Some("feature"), false, false, false).await?;
    observer
        .refresh(
            "/repo".into(),
            ThreadId("thread:test".into()),
            RunId("completed".into()),
        )
        .await?;
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    observer
        .refresh(
            "/repo".into(),
            ThreadId("thread:test".into()),
            RunId("completed".into()),
        )
        .await?;
    assert_eq!(dropped.recv().await, Some("pr"));
    assert_eq!(backend.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    observer.shutdown().await;
    Ok(())
}
#[tokio::test]
async fn missing_repository_identity_refresh_runs_in_background_and_existing_identity_is_preserved()
-> Result<()> {
    for missing in [false, true] {
        let (observer, backend, mut dropped) =
            observer(Some("main"), Some("main"), false, missing, true).await?;
        observer
            .refresh(
                "/repo".into(),
                ThreadId("thread:test".into()),
                RunId("completed".into()),
            )
            .await?;
        assert_eq!(
            backend
                .identity_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(missing)
        );
        observer.shutdown().await;
        if missing {
            assert_eq!(dropped.recv().await, Some("identity"));
        }
    }
    Ok(())
}
