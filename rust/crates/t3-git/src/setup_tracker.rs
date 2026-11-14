use crate::CancellationToken;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::{
    base_schemas::{IsoDateTime, ThreadId, TrimmedNonEmptyString},
    worktree_setup::*,
};
use tokio::{sync::watch, task::JoinHandle};

#[derive(Clone)]
pub struct SetupTask {
    cancellation: CancellationToken,
    finished: watch::Receiver<bool>,
}
impl SetupTask {
    pub fn spawn<T, F, Fut>(operation: F) -> (Self, JoinHandle<T>)
    where
        T: Send + 'static,
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let cancellation = CancellationToken::new();
        let (sender, finished) = watch::channel(false);
        let token = cancellation.clone();
        let completion = Completion(sender);
        let handle = tokio::spawn(async move {
            let _completion = completion;
            operation(token).await
        });
        (
            Self {
                cancellation,
                finished,
            },
            handle,
        )
    }
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
    async fn cancel(&self) {
        self.cancellation.cancel();
        let mut finished = self.finished.clone();
        let _ = finished.wait_for(|done| *done).await;
    }
}
struct Completion(watch::Sender<bool>);
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

struct Tracked {
    sender: watch::Sender<Option<WorktreeSetupSnapshot>>,
    task: Option<SetupTask>,
    retention: Option<JoinHandle<()>>,
}
impl Drop for Tracked {
    fn drop(&mut self) {
        if let Some(retention) = &self.retention {
            retention.abort();
        }
        if let Some(task) = &self.task {
            task.cancellation.cancel();
        }
    }
}
#[derive(Clone, Default)]
pub struct WorktreeSetupTracker {
    setups: Arc<Mutex<BTreeMap<String, Tracked>>>,
}

pub struct WorktreeSetupSubscription {
    receiver: watch::Receiver<Option<WorktreeSetupSnapshot>>,
    initial: bool,
    setups: std::sync::Weak<Mutex<BTreeMap<String, Tracked>>>,
    thread_id: String,
}
impl Drop for WorktreeSetupSubscription {
    fn drop(&mut self) {
        if let Some(setups) = self.setups.upgrade() {
            let mut setups = setups.lock().unwrap_or_else(|e| e.into_inner());
            if setups.get(&self.thread_id).is_some_and(|tracked| {
                tracked.sender.receiver_count() == 1 && tracked.sender.borrow().is_none()
            }) {
                setups.remove(&self.thread_id);
            }
        }
    }
}
impl WorktreeSetupSubscription {
    pub async fn recv(&mut self) -> Option<Option<WorktreeSetupSnapshot>> {
        if !std::mem::take(&mut self.initial) && self.receiver.changed().await.is_err() {
            return None;
        }
        Some(self.receiver.borrow_and_update().clone())
    }
}
fn now() -> IsoDateTime {
    t3_contracts::wire::WireDateTime(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
}
fn clamp(text: &str, limit: usize) -> String {
    if text.encode_utf16().count() <= limit {
        return text.into();
    }
    let mut units = 0;
    let mut result = String::new();
    for c in text.chars() {
        if units + c.len_utf16() > limit - 1 {
            break;
        }
        result.push(c);
        units += c.len_utf16();
    }
    result.push('…');
    result
}
impl WorktreeSetupTracker {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn begin(
        &self,
        thread_id: ThreadId,
        branch: Option<String>,
        base_ref: Option<String>,
        stages: &[WorktreeSetupStageId],
        task: Option<SetupTask>,
    ) {
        let mut setups = self.setups.lock().unwrap_or_else(|e| e.into_inner());
        let tracked = setups
            .entry(thread_id.0.clone())
            .or_insert_with(|| Tracked {
                sender: watch::channel(None).0,
                task: None,
                retention: None,
            });
        if let Some(retention) = tracked.retention.take() {
            retention.abort();
        }
        let sequence = tracked
            .sender
            .borrow()
            .as_ref()
            .map_or(0, |s| s.sequence + 1);
        let ordered = [
            WorktreeSetupStageId::Fetch,
            WorktreeSetupStageId::Checkout,
            WorktreeSetupStageId::Submodules,
            WorktreeSetupStageId::SetupScript,
            WorktreeSetupStageId::Agent,
        ];
        let snapshot = WorktreeSetupSnapshot {
            thread_id,
            phase: WorktreeSetupPhase::Running,
            started_at: now(),
            ended_at: None,
            branch: branch.map(TrimmedNonEmptyString),
            base_ref: base_ref.map(TrimmedNonEmptyString),
            worktree_path: None,
            setup_script: None,
            error: None,
            sequence,
            stages: ordered
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
        };
        tracked.task = task;
        tracked.sender.send_replace(Some(snapshot));
    }
    pub fn update(&self, thread_id: &ThreadId, mutate: impl FnOnce(&mut WorktreeSetupSnapshot)) {
        let setups = self.setups.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tracked) = setups.get(&thread_id.0) else {
            return;
        };
        tracked.sender.send_modify(|snapshot| {
            if let Some(snapshot) = snapshot {
                let sequence = snapshot.sequence;
                mutate(snapshot);
                snapshot.sequence = sequence + 1;
            }
        });
    }
    pub fn stage(
        &self,
        thread_id: &ThreadId,
        id: WorktreeSetupStageId,
        mutate: impl FnOnce(&mut WorktreeSetupStage),
    ) {
        self.update(thread_id, |snapshot| {
            if let Some(stage) = snapshot.stages.iter_mut().find(|stage| stage.id == id) {
                mutate(stage);
                stage.detail = stage.detail.as_ref().map(|detail| clamp(detail, 200));
            }
        });
    }
    pub fn stage_status(
        &self,
        thread_id: &ThreadId,
        id: WorktreeSetupStageId,
        status: WorktreeSetupStageStatus,
        detail: Option<Option<String>>,
    ) {
        self.stage(thread_id, id, |stage| {
            let at = now();
            if stage.started_at.is_none() && status != WorktreeSetupStageStatus::Pending {
                stage.started_at = Some(at.clone());
            }
            stage.ended_at = if matches!(
                status,
                WorktreeSetupStageStatus::Pending | WorktreeSetupStageStatus::Running
            ) {
                None
            } else {
                stage.ended_at.clone().or(Some(at))
            };
            stage.status = status;
            if let Some(detail) = detail {
                stage.detail = detail;
            }
        });
    }
    pub fn append_tail(&self, thread_id: &ThreadId, id: WorktreeSetupStageId, line: &str) {
        self.stage(thread_id, id, |stage| {
            stage.tail.push(clamp(line, 400));
            if stage.tail.len() > 4 {
                stage.tail.drain(..stage.tail.len() - 4);
            }
        });
    }
    pub fn finish(
        &self,
        thread_id: &ThreadId,
        phase: WorktreeSetupPhase,
        error: Option<&str>,
    ) -> Option<WorktreeSetupSnapshot> {
        let mut setups = self.setups.lock().unwrap_or_else(|e| e.into_inner());
        let tracked = setups.get_mut(&thread_id.0)?;
        let mut snapshot = tracked.sender.borrow().clone()?;
        snapshot.phase = phase;
        snapshot.ended_at = Some(now());
        snapshot.error = error.map(|e| clamp(e, 1000));
        snapshot.sequence += 1;
        for stage in &mut snapshot.stages {
            if stage.status == WorktreeSetupStageStatus::Running {
                stage.status = match phase {
                    WorktreeSetupPhase::Done => WorktreeSetupStageStatus::Done,
                    WorktreeSetupPhase::Cancelled => WorktreeSetupStageStatus::Skipped,
                    _ => WorktreeSetupStageStatus::Failed,
                };
                stage.ended_at = snapshot.ended_at.clone();
            }
        }
        tracked.task = None;
        tracked.sender.send_replace(Some(snapshot.clone()));
        if let Some(retention) = tracked.retention.take() {
            retention.abort();
        }
        let weak = Arc::downgrade(&self.setups);
        let id = thread_id.0.clone();
        let sequence = snapshot.sequence;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        tracked.retention = Some(tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            if let Some(setups) = weak.upgrade() {
                let mut setups = setups.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(tracked) = setups.get_mut(&id)
                    && tracked.sender.borrow().as_ref().is_some_and(|s| {
                        s.sequence == sequence && s.phase != WorktreeSetupPhase::Running
                    })
                {
                    tracked.retention.take();
                    tracked.sender.send_replace(None);
                    // Keep the sender only while subscribers need to observe the next setup.
                    if tracked.sender.receiver_count() == 0 {
                        setups.remove(&id);
                    }
                }
            }
        }));
        Some(snapshot)
    }
    pub fn get(&self, id: &ThreadId) -> Option<WorktreeSetupSnapshot> {
        self.setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id.0)
            .and_then(|t| t.sender.borrow().clone())
    }
    pub fn mark_uncancellable(&self, id: &ThreadId) {
        if let Some(tracked) = self
            .setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&id.0)
        {
            tracked.task = None;
        }
    }
    pub async fn cancel(&self, id: &ThreadId) -> bool {
        let task = self
            .setups
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id.0)
            .and_then(|t| {
                t.sender
                    .borrow()
                    .as_ref()
                    .filter(|s| s.phase == WorktreeSetupPhase::Running)
                    .and(t.task.clone())
            });
        if let Some(task) = task {
            task.cancel().await;
            true
        } else {
            false
        }
    }
    pub fn stream(&self, id: &ThreadId) -> WorktreeSetupSubscription {
        let mut setups = self.setups.lock().unwrap_or_else(|e| e.into_inner());
        let tracked = setups.entry(id.0.clone()).or_insert_with(|| Tracked {
            sender: watch::channel(None).0,
            task: None,
            retention: None,
        });
        WorktreeSetupSubscription {
            receiver: tracked.sender.subscribe(),
            initial: true,
            setups: Arc::downgrade(&self.setups),
            thread_id: id.0.clone(),
        }
    }
}
