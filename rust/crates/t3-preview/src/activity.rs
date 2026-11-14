use crate::{lock, manager::Events};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use t3_contracts::base_schemas::ThreadId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnChange {
    pub thread_id: ThreadId,
    pub active: bool,
}
#[derive(Clone, Default)]
pub struct PreviewAgentActivity {
    counts: Arc<Mutex<BTreeMap<String, usize>>>,
    changes: Events<TurnChange>,
}
pub struct ActiveTurn {
    activity: PreviewAgentActivity,
    thread: String,
}
impl PreviewAgentActivity {
    pub fn begin(&self, thread: ThreadId) -> ActiveTurn {
        let mut counts = lock(&self.counts);
        let count = counts.entry(thread.0.as_str().into()).or_default();
        *count += 1;
        if *count == 1 {
            self.changes.publish(TurnChange {
                thread_id: thread.clone(),
                active: true,
            });
        }
        ActiveTurn {
            activity: self.clone(),
            thread: thread.0,
        }
    }
    pub fn active(&self, thread: &str) -> bool {
        lock(&self.counts).contains_key(thread)
    }
    pub fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<TurnChange> {
        self.changes.subscribe()
    }
}
impl Drop for ActiveTurn {
    fn drop(&mut self) {
        let mut counts = lock(&self.activity.counts);
        if let Some(count) = counts.get_mut(&self.thread) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.thread);
                self.activity.changes.publish(TurnChange {
                    thread_id: self.thread.as_str().into(),
                    active: false,
                });
            }
        }
    }
}
