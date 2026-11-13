use crate::{
    api::*,
    error::{Error, Result},
};
use serde_json::json;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub struct WorktreeBranchSync {
    pub projections: Arc<dyn ProjectionReader>,
    pub workspaces: Arc<dyn WorkspaceBackend>,
    pub commands: Arc<dyn CommandDispatcher>,
    pub ids: Arc<dyn IdGenerator>,
}

pub struct BranchChange {
    pub cwd: PathBuf,
    pub ref_name: Option<String>,
}

impl WorktreeBranchSync {
    pub async fn run(
        &self,
        mut changes: tokio::sync::mpsc::Receiver<BranchChange>,
        cancellation: tokio_util::sync::CancellationToken,
    ) {
        loop {
            let change = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return,
                change = changes.recv() => match change { Some(change) => change, None => return },
            };
            let reconcile = self.reconcile(change.cwd, change.ref_name);
            tokio::pin!(reconcile);
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {let _=reconcile.await;return},
                result = &mut reconcile => result,
            };
            if let Err(error) = result {
                eprintln!("Worktree branch reconciliation failed: {error}");
            }
        }
    }

    pub async fn reconcile(&self, cwd: PathBuf, ref_name: Option<String>) -> Result<()> {
        let Some(ref_name) = ref_name else {
            return Ok(());
        };
        let snapshot = self.projections.shell_snapshot(None, None).await?;
        let mut paths = HashMap::new();
        let mut matching = HashMap::new();
        for thread in snapshot
            .threads
            .into_iter()
            .chain(snapshot.archived_threads)
        {
            if let Some(worktree) = thread.worktree_path {
                let matches = *paths.entry(worktree.0.clone()).or_insert_with(|| {
                    let path = PathBuf::from(&worktree.0);
                    path == cwd || std::fs::canonicalize(path).is_ok_and(|path| path == cwd)
                });
                if matches {
                    matching.insert(thread.id, worktree);
                }
            }
        }
        if matching.is_empty() {
            return Ok(());
        }
        let snapshot = self
            .projections
            .thread_shells(ShellQuery {
                thread_ids: Some(matching.keys().cloned().collect()),
                ..Default::default()
            })
            .await?;
        let sharing: Vec<_> = snapshot
            .threads
            .into_iter()
            .chain(snapshot.archived_threads)
            .filter(|thread| {
                thread.worktree_path.as_ref() == matching.get(&thread.id)
                    && thread.branch.as_ref().map(|b| b.0.as_str()) != Some(ref_name.as_str())
            })
            .collect();
        if sharing.is_empty()
            || self.workspaces.current_branch(cwd).await?.as_deref() != Some(&ref_name)
        {
            return Ok(());
        }
        for thread in sharing {
            let command = serde_json::from_value(
                json!({ "type": "thread.metadata.update", "commandId": self.ids.next(IdKind::Command), "threadId": thread.id, "branch": ref_name, "expectedBranch": thread.branch, "expectedWorktreePath": thread.worktree_path }),
            )?;
            match self.commands.dispatch(command).await {
                Ok(_) | Err(Error::Rejected { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// Owns subscriptions; the host stops effect admission before shutting this down.
pub struct CheckpointSubscriptions {
    cancellation: tokio_util::sync::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
    launches: Option<super::thread_launch_service::ThreadLaunchService>,
}

impl CheckpointSubscriptions {
    pub fn start(
        branches: Arc<WorktreeBranchSync>,
        changes: tokio::sync::mpsc::Receiver<BranchChange>,
        launches: Option<super::thread_launch_service::ThreadLaunchService>,
    ) -> Self {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let tasks = tokio_util::task::TaskTracker::new();
        let token = cancellation.child_token();
        tasks.spawn(async move {
            branches.run(changes, token).await;
        });
        Self {
            cancellation,
            tasks,
            launches,
        }
    }

    pub async fn shutdown(&self) {
        if let Some(launches) = &self.launches {
            launches.shutdown().await;
        }
        self.cancellation.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}
