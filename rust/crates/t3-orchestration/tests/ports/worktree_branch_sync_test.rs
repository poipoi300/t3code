use crate::checkpoint_service_test::support::*;
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::checkpoints_forks::worktree_branch_sync::WorktreeBranchSync,
    stores::receipts::{AggregateId, CommandReceipt, ReceiptStatus},
};
use tokio_util::sync::CancellationToken;

struct Workspaces(Option<String>);
#[async_trait]
impl WorkspaceBackend for Workspaces {
    async fn prepare(
        &self,
        _project: ProjectId,
        _thread: ThreadId,
        _strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
        _cancellation: CancellationToken,
    ) -> Result<Workspace> {
        Err(Error::Unsupported("unused prepare"))
    }
    async fn remove(&self, _workspace: Workspace, _cancellation: CancellationToken) -> Result<()> {
        Err(Error::Unsupported("unused remove"))
    }
    async fn current_branch(&self, _cwd: PathBuf) -> Result<Option<String>> {
        Ok(self.0.clone())
    }
}
#[derive(Default)]
struct Commands {
    received: Mutex<Vec<serde_json::Value>>,
    conflict: bool,
    failure: std::sync::atomic::AtomicBool,
    gate: Option<Arc<Gate>>,
}
#[async_trait]
impl CommandDispatcher for Commands {
    async fn dispatch(&self, command: OrchestrationV2ServerCommand) -> Result<CommandReceipt> {
        let value = serde_json::to_value(command)?;
        self.received.lock().expect("commands").push(value.clone());
        if let Some(gate) = &self.gate {
            gate.entered.add_permits(1);
            gate.release
                .acquire()
                .await
                .expect("dispatch release")
                .forget();
        }
        if self
            .failure
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(Error::Unsupported("first branch update failed"));
        }
        let command_id = CommandId(value["commandId"].as_str().expect("command").into());
        if self.conflict && value["threadId"] == "generated" {
            return Err(Error::Rejected {
                command_id,
                reason: "Thread moved".into(),
            });
        }
        Ok(CommandReceipt {
            command_id,
            aggregate: AggregateId::Thread(ThreadId(
                value["threadId"].as_str().expect("thread").into(),
            )),
            command_type: "thread.metadata.update".into(),
            accepted_at: time().0,
            result_sequence: 1,
            status: ReceiptStatus::Accepted,
            error: None,
        })
    }
}

#[tokio::test]
async fn branch_subscription_shutdown_awaits_an_inflight_dispatch() -> Result<()> {
    use t3_orchestration::ports::checkpoints_forks::worktree_branch_sync::{
        BranchChange, CheckpointSubscriptions,
    };
    let temp = tempfile::tempdir()?;
    let cwd = std::fs::canonicalize(temp.path())?;
    let h = Harness::new().await?;
    h.seed("thread.created", thread("shutdown", Some(&cwd)))
        .await?;
    let gate = Arc::new(Gate::default());
    let commands = Arc::new(Commands {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let sync = WorktreeBranchSync {
        projections: h.reader.clone(),
        workspaces: Arc::new(Workspaces(Some("changed".into()))),
        commands,
        ids: h.ids.clone(),
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    sender
        .send(BranchChange {
            cwd,
            ref_name: Some("changed".into()),
        })
        .await
        .expect("event");
    let subscriptions = CheckpointSubscriptions::start(Arc::new(sync), receiver, None);
    gate.entered.acquire().await.expect("entered").forget();
    let running = subscriptions.shutdown();
    tokio::pin!(running);
    std::future::poll_fn(|cx| {
        assert!(running.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    gate.release.add_permits(1);
    running.await;
    Ok(())
}

#[tokio::test]
async fn branch_subscription_continues_after_failure_and_joins_on_shutdown() -> Result<()> {
    use t3_orchestration::ports::checkpoints_forks::worktree_branch_sync::BranchChange;
    let temp = tempfile::tempdir()?;
    let cwd = std::fs::canonicalize(temp.path())?;
    let h = Harness::new().await?;
    h.seed("thread.created", thread("subscription", Some(&cwd)))
        .await?;
    let commands = Arc::new(Commands {
        failure: std::sync::atomic::AtomicBool::new(true),
        ..Default::default()
    });
    let sync = WorktreeBranchSync {
        projections: h.reader.clone(),
        workspaces: Arc::new(Workspaces(Some("changed".into()))),
        commands: commands.clone(),
        ids: h.ids.clone(),
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let token = CancellationToken::new();
    let task = tokio::spawn(async move { sync.run(receiver, token).await });
    for _ in 0..2 {
        sender
            .send(BranchChange {
                cwd: cwd.clone(),
                ref_name: Some("changed".into()),
            })
            .await
            .expect("branch event");
    }
    drop(sender);
    task.await.expect("subscription task");
    assert_eq!(commands.received.lock().expect("commands").len(), 2);
    Ok(())
}

#[tokio::test]
async fn reconciles_shared_worktree_branches_with_guarded_updates_and_continues_after_conflicts()
-> Result<()> {
    let parent = tempfile::tempdir()?;
    let cwd = parent.path().join("worktree");
    std::fs::create_dir(&cwd)?;
    std::fs::create_dir(cwd.join("alias-parent"))?;
    let alias = cwd.join("alias-parent").join("..");
    let other = parent.path().join("other");
    std::fs::create_dir(&other)?;
    for conflict in [false, true] {
        let h = Harness::new().await?;
        for (id, branch, path, archived) in [
            ("generated", "temp", Some(cwd.as_path()), false),
            ("shared", "temp", Some(alias.as_path()), false),
            ("current", "union", Some(cwd.as_path()), false),
            ("root", "main", None, false),
            ("other", "feature", Some(other.as_path()), false),
            ("archived", "temp", Some(cwd.as_path()), true),
        ] {
            let mut thread = thread(id, path);
            thread.branch = Some(TrimmedNonEmptyString(branch.into()));
            thread.archived_at = archived.then(time);
            h.seed("thread.created", thread).await?;
        }
        let commands = Arc::new(Commands {
            conflict,
            ..Default::default()
        });
        let sync = WorktreeBranchSync {
            projections: h.reader.clone(),
            workspaces: Arc::new(Workspaces(Some("union".into()))),
            commands: commands.clone(),
            ids: h.ids.clone(),
        };
        let canonical = std::fs::canonicalize(&cwd)?;
        sync.reconcile(canonical.clone(), None).await?;
        sync.reconcile(canonical.clone(), Some("stale-status".into()))
            .await?;
        assert!(commands.received.lock().expect("commands").is_empty());
        sync.reconcile(canonical, Some("union".into())).await?;
        let values = commands.received.lock().expect("commands");
        assert_eq!(values.len(), 3);
        let mut ids = values
            .iter()
            .map(|v| v["threadId"].as_str().expect("thread"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, vec!["archived", "generated", "shared"]);
        for value in values.iter() {
            assert_eq!(value["type"], "thread.metadata.update");
            assert_eq!(value["branch"], "union");
            assert_eq!(value["expectedBranch"], "temp");
            assert!(!value["expectedWorktreePath"].is_null());
        }
    }
    Ok(())
}
