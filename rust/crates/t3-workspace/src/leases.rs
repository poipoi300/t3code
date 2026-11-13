use crate::{Result, paths};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard};

#[derive(Default)]
pub struct WorkspaceLeases {
    locks: Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>,
}
impl WorkspaceLeases {
    pub async fn acquire(&self, cwd: &Path) -> Result<OwnedMutexGuard<()>> {
        let cwd = paths::absolute(cwd)?;
        let lock = {
            let mut locks = self
                .locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, v| v.strong_count() > 0);
            let lock = locks
                .get(&cwd)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(AsyncMutex::new(())));
            locks.insert(cwd, Arc::downgrade(&lock));
            lock
        };
        Ok(lock.lock_owned().await)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReservationKind {
    Restore,
    TurnStart,
}
#[derive(Default)]
struct Reservations {
    held: Mutex<HashMap<uuid::Uuid, (PathBuf, ReservationKind)>>,
    released: Notify,
}
#[derive(Clone, Default)]
pub struct WorkspaceReservations(Arc<Reservations>);
pub struct Reservation {
    owner: Arc<Reservations>,
    id: uuid::Uuid,
}
impl WorkspaceReservations {
    pub async fn acquire(&self, cwd: &Path, kind: ReservationKind) -> Result<Reservation> {
        let cwd = paths::nearest_real(&paths::absolute(cwd)?).map(|(p, _)| p)?;
        loop {
            let notified = self.0.released.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut held = self
                    .0
                    .held
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !held.values().any(|(p, k)| {
                    (kind == ReservationKind::Restore || *k == ReservationKind::Restore)
                        && (paths::within(p, &cwd) || paths::within(&cwd, p))
                }) {
                    let id = uuid::Uuid::new_v4();
                    held.insert(id, (cwd, kind));
                    return Ok(Reservation {
                        owner: self.0.clone(),
                        id,
                    });
                }
            }
            notified.await;
        }
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.owner
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
        self.owner.released.notify_waiters();
    }
}
