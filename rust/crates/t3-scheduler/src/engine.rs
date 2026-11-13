use crate::{Clock, Result};
use async_trait::async_trait;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct DueHint {
    version: u64,
    hint: Option<(i64, i64)>,
}
impl DueHint {
    pub fn invalidate(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.hint = None;
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn is_idle(&self, now_ms: i64) -> bool {
        self.hint
            .is_some_and(|(due, checked)| due > now_ms && now_ms.saturating_sub(checked) < 600_000)
    }
    pub fn record(&mut self, version: u64, due_ms: i64, now_ms: i64) {
        if version == self.version {
            self.hint = Some((due_ms, now_ms));
        }
    }
}

#[async_trait]
pub trait SchedulerSource: Send + Sync + 'static {
    async fn run_due_work(&self) -> Result<()>;
}

/// Each registration owns one non-overlapping poller; a failing source cannot stop another.
pub struct Scheduler {
    clock: Arc<dyn Clock>,
    active: watch::Sender<bool>,
    cancel: CancellationToken,
}

impl Scheduler {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            active: watch::channel(false).0,
            cancel: CancellationToken::new(),
        }
    }
    /// Call only after startup recovery has completed.
    pub fn activate(&self) {
        self.active.send_replace(true);
    }
    pub fn register(&self, source: Arc<dyn SchedulerSource>) -> Registration {
        let cancel = self.cancel.child_token();
        let stop = cancel.clone();
        let mut active = self.active.subscribe();
        let clock = self.clock.clone();
        let running = Arc::new(AtomicBool::new(false));
        let current = Arc::new(Mutex::new(None::<JoinHandle<()>>));
        let child = current.clone();
        let task = tokio::spawn(async move {
            while !*active.borrow_and_update() {
                tokio::select! { _ = stop.cancelled() => return, result = active.changed() => if result.is_err() { return; } }
            }
            loop {
                if !running.swap(true, Ordering::AcqRel) {
                    let source = source.clone();
                    let running = running.clone();
                    let source_stop = stop.clone();
                    *child.lock().unwrap_or_else(|e| e.into_inner()) = Some(tokio::spawn(
                        async move {
                            struct Release(Arc<AtomicBool>);
                            impl Drop for Release {
                                fn drop(&mut self) {
                                    self.0.store(false, Ordering::Release);
                                }
                            }
                            let _release = Release(running);
                            tokio::select! { _=source_stop.cancelled()=>{},result=source.run_due_work()=>{if let Err(error)=result {tracing::warn!(%error,"Scheduler source failed");}} }
                        },
                    ));
                }
                tokio::select! { _ = stop.cancelled() => return, _ = clock.sleep(Duration::from_secs(5)) => {} }
            }
        });
        Registration {
            cancel,
            task,
            current,
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct Registration {
    cancel: CancellationToken,
    task: JoinHandle<()>,
    current: Arc<Mutex<Option<JoinHandle<()>>>>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
        if let Some(task) = self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
    }
}
