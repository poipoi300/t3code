use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Default)]
pub struct KeyedLanes {
    lanes: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
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
            guards.push(self.lane(&key).lock_owned().await);
        }
        guards
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
