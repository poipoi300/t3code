use crate::{Error, Result, clamp_text};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::{
    base_schemas::{ThreadId, TrimmedNonEmptyString},
    worktree_setup::*,
};
use t3_orchestration::api::Clock;
use tokio::sync::watch;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

const ORDER: [WorktreeSetupStageId; 5] = [
    WorktreeSetupStageId::Fetch,
    WorktreeSetupStageId::Checkout,
    WorktreeSetupStageId::Submodules,
    WorktreeSetupStageId::SetupScript,
    WorktreeSetupStageId::Agent,
];
#[derive(Clone)]
pub struct SetupCancellation {
    pub token: CancellationToken,
    pub finished: watch::Receiver<bool>,
}

struct Entry {
    sender: watch::Sender<Option<WorktreeSetupSnapshot>>,
    cancel: Option<SetupCancellation>,
    retention: CancellationToken,
}
struct Inner {
    entries: Mutex<HashMap<String, Entry>>,
    clock: Arc<dyn Clock>,
    tasks: TaskTracker,
    stop: CancellationToken,
}
#[derive(Clone)]
pub struct WorktreeSetupTracker(Arc<Inner>);
impl WorktreeSetupTracker {
    fn prune_idle(entries: &mut HashMap<String, Entry>) {
        entries.retain(|_, entry| {
            entry.sender.borrow().is_some() || entry.sender.receiver_count() > 0
        });
    }
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self(Arc::new(Inner {
            entries: Default::default(),
            clock,
            tasks: TaskTracker::new(),
            stop: CancellationToken::new(),
        }))
    }
    pub fn begin(
        &self,
        thread_id: ThreadId,
        branch: Option<TrimmedNonEmptyString>,
        base_ref: Option<TrimmedNonEmptyString>,
        stages: &[WorktreeSetupStageId],
        cancel: Option<SetupCancellation>,
    ) -> Result<()> {
        if self.0.stop.is_cancelled() {
            return Err(Error::Unavailable("WorktreeSetupTracker is shut down"));
        }
        let mut entries = self.0.entries.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune_idle(&mut entries);
        let e = entries.entry(thread_id.0.clone()).or_insert_with(|| Entry {
            sender: watch::channel(None).0,
            cancel: None,
            retention: CancellationToken::new(),
        });
        e.retention.cancel();
        e.retention = CancellationToken::new();
        e.cancel = cancel;
        let sequence = e
            .sender
            .borrow()
            .as_ref()
            .map(|s| s.sequence + 1)
            .unwrap_or(0);
        e.sender.send_replace(Some(WorktreeSetupSnapshot {
            thread_id,
            phase: WorktreeSetupPhase::Running,
            started_at: self.0.clock.now(),
            ended_at: None,
            branch,
            base_ref,
            worktree_path: None,
            setup_script: None,
            stages: ORDER
                .into_iter()
                .filter(|id| stages.contains(id))
                .map(|id| WorktreeSetupStage {
                    id,
                    status: WorktreeSetupStageStatus::Pending,
                    started_at: None,
                    ended_at: None,
                    percent: None,
                    detail: None,
                    tail: vec![],
                })
                .collect(),
            error: None,
            sequence,
        }));
        Ok(())
    }
    pub fn update(&self, id: &ThreadId, f: impl FnOnce(&mut WorktreeSetupSnapshot)) {
        let entries = self.0.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.get(&id.0) {
            e.sender.send_modify(|s| {
                if let Some(s) = s {
                    let next = s.sequence + 1;
                    f(s);
                    s.sequence = next;
                }
            });
        }
    }
    pub fn stage(
        &self,
        id: &ThreadId,
        stage: WorktreeSetupStageId,
        f: impl FnOnce(&mut WorktreeSetupStage),
    ) {
        self.update(id, |s| {
            if let Some(stage) = s.stages.iter_mut().find(|e| e.id == stage) {
                f(stage);
                stage.detail = stage.detail.as_ref().map(|s| clamp_text(s, 200));
            }
        });
    }
    pub fn stage_status(
        &self,
        id: &ThreadId,
        stage: WorktreeSetupStageId,
        status: WorktreeSetupStageStatus,
        detail: Option<Option<String>>,
    ) {
        let now = self.0.clock.now();
        self.stage(id, stage, |s| {
            s.status = status;
            if s.started_at.is_none() && status != WorktreeSetupStageStatus::Pending {
                s.started_at = Some(now.clone());
            }
            s.ended_at = if matches!(
                status,
                WorktreeSetupStageStatus::Running | WorktreeSetupStageStatus::Pending
            ) {
                None
            } else {
                Some(s.ended_at.clone().unwrap_or(now))
            };
            if let Some(detail) = detail {
                s.detail = detail;
            }
        });
    }
    pub fn append_tail(&self, id: &ThreadId, stage: WorktreeSetupStageId, line: &str) {
        self.stage(id, stage, |s| {
            s.tail.push(clamp_text(line, 400));
            if s.tail.len() > 4 {
                s.tail.remove(0);
            }
        });
    }
    pub fn finish(
        &self,
        id: &ThreadId,
        phase: WorktreeSetupPhase,
        error: Option<&str>,
    ) -> Option<WorktreeSetupSnapshot> {
        if phase == WorktreeSetupPhase::Running {
            return None;
        }
        let now = self.0.clock.now();
        self.update(id, |s| {
            s.phase = phase;
            s.ended_at = Some(now.clone());
            s.error = error.map(|s| clamp_text(s, 1000));
            for e in &mut s.stages {
                if e.status == WorktreeSetupStageStatus::Running {
                    e.ended_at = Some(now.clone());
                    e.status = match phase {
                        WorktreeSetupPhase::Done => WorktreeSetupStageStatus::Done,
                        WorktreeSetupPhase::Cancelled => WorktreeSetupStageStatus::Skipped,
                        _ => WorktreeSetupStageStatus::Failed,
                    };
                }
            }
        });
        let (snapshot, retention) = {
            let mut entries = self.0.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = entries.get_mut(&id.0)?;
            e.cancel = None;
            e.retention.cancel();
            e.retention = CancellationToken::new();
            (e.sender.borrow().clone(), e.retention.clone())
        };
        let this = self.clone();
        let id = id.clone();
        let expires = tokio::time::Instant::now() + Duration::from_secs(30);
        self.0.tasks.spawn(async move {
            tokio::select! {
                _ = this.0.stop.cancelled() => {},
                _ = retention.cancelled() => {},
                _ = tokio::time::sleep_until(expires) => {
                    let mut entries = this.0.entries.lock().unwrap_or_else(|e| e.into_inner());
                    if !retention.is_cancelled() {
                        if let Some(e) = entries.get(&id.0) {
                            e.sender.send_replace(None);
                        }
                        if entries.get(&id.0).is_some_and(|e| e.sender.receiver_count() == 0) {
                            entries.remove(&id.0);
                        }
                    }
                }
            }
        });
        snapshot
    }
    pub fn mark_uncancellable(&self, id: &ThreadId) {
        if let Some(e) = self
            .0
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&id.0)
        {
            e.cancel = None;
        }
    }
    pub async fn cancel(&self, id: &ThreadId) -> bool {
        let cancel = {
            let entries = self.0.entries.lock().unwrap_or_else(|e| e.into_inner());
            entries
                .get(&id.0)
                .filter(|e| {
                    e.sender
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| s.phase == WorktreeSetupPhase::Running)
                })
                .and_then(|e| e.cancel.clone())
        };
        let Some(mut cancel) = cancel else {
            return false;
        };
        cancel.token.cancel();
        while !*cancel.finished.borrow_and_update() {
            if cancel.finished.changed().await.is_err() {
                break;
            }
        }
        true
    }
    pub fn get(&self, id: &ThreadId) -> Option<WorktreeSetupSnapshot> {
        let mut entries = self.0.entries.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune_idle(&mut entries);
        entries.get(&id.0).and_then(|e| e.sender.borrow().clone())
    }
    pub fn subscribe_worktree_setup(
        &self,
        id: &ThreadId,
    ) -> watch::Receiver<Option<WorktreeSetupSnapshot>> {
        let mut entries = self.0.entries.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune_idle(&mut entries);
        entries
            .entry(id.0.clone())
            .or_insert_with(|| Entry {
                sender: watch::channel(None).0,
                cancel: None,
                retention: CancellationToken::new(),
            })
            .sender
            .subscribe()
    }
    pub async fn shutdown(&self) {
        self.0.stop.cancel();
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_idle_subscriptions_are_reclaimed() {
        let tracker = WorktreeSetupTracker::new(Arc::new(
            t3_orchestration::testing::ManualClock::new("2026-01-01T00:00:00.000Z"),
        ));
        let first = tracker.subscribe_worktree_setup(&ThreadId("first".into()));
        drop(first);
        let _second = tracker.subscribe_worktree_setup(&ThreadId("second".into()));
        assert_eq!(tracker.0.entries.lock().expect("entries").len(), 1);
    }
}
