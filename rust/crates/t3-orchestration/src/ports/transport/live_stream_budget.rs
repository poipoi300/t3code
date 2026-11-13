use crate::error::{Error, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

pub const LIVE_STREAM_MAX_ITEMS: usize = 1_000;
pub const LIVE_STREAM_MAX_SERIALIZED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct LiveStreamLimits {
    pub max_items: usize,
    pub max_serialized_bytes: usize,
}
impl Default for LiveStreamLimits {
    fn default() -> Self {
        Self {
            max_items: LIVE_STREAM_MAX_ITEMS,
            max_serialized_bytes: LIVE_STREAM_MAX_SERIALIZED_BYTES,
        }
    }
}

/// One immutable projection and size can be shared by every subscriber.
#[derive(Debug)]
pub struct SharedLiveItem<T> {
    pub value: T,
    pub sequence: u64,
    pub serialized_bytes: usize,
}
impl<T: Serialize> SharedLiveItem<T> {
    pub fn new(value: T, sequence: u64) -> Result<Arc<Self>> {
        let serialized_bytes = serde_json::to_vec(&value)?.len();
        Ok(Arc::new(Self {
            value,
            sequence,
            serialized_bytes,
        }))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LiveStreamUsage {
    pub retained_items: usize,
    pub retained_serialized_bytes: usize,
}
struct BufferState<T> {
    pending: BTreeMap<String, Arc<SharedLiveItem<T>>>,
    deferred: std::collections::BTreeSet<String>,
    in_flight: Vec<Arc<SharedLiveItem<T>>>,
    usage: LiveStreamUsage,
    failed: bool,
    closed: bool,
    serial: u64,
}
pub struct LiveBuffer<T> {
    state: Mutex<BufferState<T>>,
    changed: Notify,
    failure: Notify,
    limits: LiveStreamLimits,
}
impl<T> LiveBuffer<T> {
    pub fn new(limits: LiveStreamLimits) -> Self {
        Self {
            state: Mutex::new(BufferState {
                pending: BTreeMap::new(),
                deferred: std::collections::BTreeSet::new(),
                in_flight: Vec::new(),
                usage: LiveStreamUsage::default(),
                failed: false,
                closed: false,
                serial: 0,
            }),
            changed: Notify::new(),
            failure: Notify::new(),
            limits,
        }
    }

    pub fn usage(&self) -> LiveStreamUsage {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).usage
    }

    /// Only queued full states may replace one another. Delivered states remain charged.
    pub fn push(&self, item: Arc<SharedLiveItem<T>>, key: Option<String>) -> Result<()> {
        self.push_inner(item, key, false)
    }
    pub fn push_deferred(&self, item: Arc<SharedLiveItem<T>>, key: String) -> Result<()> {
        self.push_inner(item, Some(key), true)
    }
    fn push_inner(
        &self,
        item: Arc<SharedLiveItem<T>>,
        key: Option<String>,
        deferred: bool,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.failed {
            return Err(overflow());
        }
        if state.closed {
            return Err(Error::Cancelled);
        }
        let key = key.unwrap_or_else(|| {
            state.serial += 1;
            format!("fifo:{}", state.serial)
        });
        let old = state.pending.get(&key);
        if old.is_some_and(|old| old.sequence >= item.sequence) {
            return Ok(());
        }
        let items = state.usage.retained_items + usize::from(old.is_none());
        let bytes = state.usage.retained_serialized_bytes - old.map_or(0, |v| v.serialized_bytes)
            + item.serialized_bytes;
        if items > self.limits.max_items || bytes > self.limits.max_serialized_bytes {
            state.failed = true;
            state.pending.clear();
            state.deferred.clear();
            state.usage = LiveStreamUsage {
                retained_items: state.in_flight.len(),
                retained_serialized_bytes: state.in_flight.iter().map(|v| v.serialized_bytes).sum(),
            };
            drop(state);
            self.changed.notify_waiters();
            self.failure.notify_waiters();
            return Err(overflow());
        }
        if deferred {
            state.deferred.insert(key.clone());
        }
        state.pending.insert(key, item);
        state.usage = LiveStreamUsage {
            retained_items: items,
            retained_serialized_bytes: bytes,
        };
        drop(state);
        if !deferred {
            self.changed.notify_one();
        }
        Ok(())
    }

    pub fn flush_pending(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .deferred
            .clear();
        self.changed.notify_one();
    }

    /// Calling again acknowledges the previous batch, matching RPC pull/ACK delivery.
    /// A subscription has exactly one downstream reader.
    pub async fn pull(&self) -> Result<Option<Vec<Arc<SharedLiveItem<T>>>>> {
        self.acknowledge();
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.failed {
                    return Err(overflow());
                }
                if state.pending.len() > state.deferred.len() {
                    let ready: Vec<_> = state
                        .pending
                        .keys()
                        .filter(|key| !state.deferred.contains(*key))
                        .cloned()
                        .collect();
                    let mut batch: Vec<_> = ready
                        .into_iter()
                        .filter_map(|key| state.pending.remove(&key))
                        .collect();
                    batch.sort_by_key(|v| v.sequence);
                    state.in_flight = batch.clone();
                    return Ok(Some(batch));
                }
                if state.closed {
                    return Ok(None);
                }
            }
            changed.await;
        }
    }

    pub fn acknowledge(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let delivered = std::mem::take(&mut state.in_flight);
        state.usage.retained_items -= delivered.len();
        state.usage.retained_serialized_bytes -=
            delivered.iter().map(|v| v.serialized_bytes).sum::<usize>();
    }

    pub fn deliver_external(&self, item: Arc<SharedLiveItem<T>>) -> Result<()> {
        self.acknowledge();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.failed {
            return Err(overflow());
        }
        let items = state.usage.retained_items + 1;
        let bytes = state.usage.retained_serialized_bytes + item.serialized_bytes;
        if items > self.limits.max_items || bytes > self.limits.max_serialized_bytes {
            drop(state);
            self.fail();
            return Err(overflow());
        }
        state.usage = LiveStreamUsage {
            retained_items: items,
            retained_serialized_bytes: bytes,
        };
        state.in_flight.push(item);
        Ok(())
    }

    pub async fn failed(&self) {
        loop {
            let changed = self.failure.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.lock().unwrap_or_else(|e| e.into_inner()).failed {
                return;
            }
            changed.await;
        }
    }

    pub fn fail(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.failed = true;
        state.pending.clear();
        state.deferred.clear();
        state.usage.retained_items = state.in_flight.len();
        state.usage.retained_serialized_bytes =
            state.in_flight.iter().map(|v| v.serialized_bytes).sum();
        drop(state);
        self.changed.notify_waiters();
        self.failure.notify_waiters();
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.deferred.clear();
        drop(state);
        self.changed.notify_waiters();
    }
}
fn overflow() -> Error {
    Error::Service {
        operation: "LiveStreamBudget",
        detail: "The live event buffer is full. Resume from the last received sequence.".into(),
    }
}
