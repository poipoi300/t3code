use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

pub struct KeyedLanes {
    lanes: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    /// Keys someone is blocked on, counted; `waiters` changes with every entry and exit.
    waiting: Mutex<HashMap<String, usize>>,
    waiters: tokio::sync::watch::Sender<u64>,
}

impl Default for KeyedLanes {
    fn default() -> Self {
        Self {
            lanes: Mutex::default(),
            waiting: Mutex::default(),
            waiters: tokio::sync::watch::channel(0).0,
        }
    }
}

/// Marks one blocked `acquire` of `key` while it waits.
struct Waiting<'a> {
    owner: &'a KeyedLanes,
    key: String,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut waiting = self
            .owner
            .waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(count) = waiting.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                waiting.remove(&self.key);
            }
        }
        drop(waiting);
        self.owner
            .waiters
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

impl KeyedLanes {
    pub fn lane(&self, key: &str) -> Arc<AsyncMutex<()>> {
        let mut lanes = self.lanes.lock().unwrap_or_else(|error| error.into_inner());
        lanes.retain(|_, lane| lane.strong_count() > 0);
        if let Some(lane) = lanes.get(key).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(AsyncMutex::new(()));
        lanes.insert(key.to_owned(), Arc::downgrade(&lane));
        lane
    }

    pub async fn acquire(
        &self,
        keys: impl IntoIterator<Item = String>,
    ) -> Vec<OwnedMutexGuard<()>> {
        let mut keys: Vec<_> = keys.into_iter().collect();
        keys.sort();
        keys.dedup();
        let mut guards = Vec::with_capacity(keys.len());
        for key in keys {
            let lane = self.lane(&key);
            // A free lane is taken at once; tokio hands a released lane to its first waiter,
            // so this never overtakes one.
            let guard = match lane.clone().try_lock_owned() {
                Ok(guard) => guard,
                Err(_) => {
                    *self
                        .waiting
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .entry(key.clone())
                        .or_default() += 1;
                    let _waiting = Waiting { owner: self, key };
                    self.waiters
                        .send_modify(|generation| *generation = generation.wrapping_add(1));
                    lane.lock_owned().await
                }
            };
            guards.push(guard);
        }
        guards
    }

    /// Resolves once an `acquire` is blocked on `key`: a test's proof that a command waits for
    /// the lane rather than not having reached it yet.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn waiting_on(&self, key: &str) {
        let mut changed = self.waiters.subscribe();
        loop {
            if self
                .waiting
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains_key(key)
            {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    pub fn prune(&self) {
        self.lanes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|_, lane| lane.strong_count() > 0);
    }

    pub fn live_lanes(&self) -> usize {
        self.lanes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|lane| lane.strong_count() > 0)
            .count()
    }
}
