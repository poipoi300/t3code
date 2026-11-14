//! Which threads a connected client follows.
//!
//! Screens hold [`ThreadLease`]s; the connection runtime follows every leased thread while it has
//! a session, and only then. The runtime reads the leases when a session context starts and when
//! they change, and stops every follow when the context is released (disconnect, sign-out,
//! environment removal, replacement), so a follow never outlives its session and a released
//! lease is never followed again.
use crate::types::ThreadId;
use futures::channel::mpsc;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
};

/// The open leases, shared by the client handle and its runtime.
pub(crate) struct ThreadDemand {
    open: RefCell<BTreeMap<ThreadId, usize>>,
    changed: mpsc::UnboundedSender<()>,
}

impl ThreadDemand {
    pub(crate) fn new() -> (Rc<Self>, mpsc::UnboundedReceiver<()>) {
        let (changed, receiver) = mpsc::unbounded();
        let demand = Rc::new(Self {
            open: RefCell::new(BTreeMap::new()),
            changed,
        });
        (demand, receiver)
    }

    pub(crate) fn lease(self: &Rc<Self>, thread: ThreadId) -> ThreadLease {
        let first = {
            let mut open = self.open.borrow_mut();
            let count = open.entry(thread.clone()).or_default();
            *count += 1;
            *count == 1
        };
        if first {
            let _ = self.changed.unbounded_send(());
        }
        ThreadLease {
            demand: Rc::downgrade(self),
            thread,
        }
    }

    pub(crate) fn open(&self) -> BTreeSet<ThreadId> {
        self.open.borrow().keys().cloned().collect()
    }

    fn release(&self, thread: &ThreadId) {
        let last = {
            let mut open = self.open.borrow_mut();
            match open.get_mut(thread) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => {
                    open.remove(thread);
                    true
                }
                None => false,
            }
        };
        if last {
            let _ = self.changed.unbounded_send(());
        }
    }
}

/// Keeps a thread followed while held. Dropping the last lease for a thread ends its follow; the
/// thread's projection stays in the store, readable and no longer updated.
#[must_use = "the thread is followed only while its lease is held"]
pub struct ThreadLease {
    demand: Weak<ThreadDemand>,
    thread: ThreadId,
}

impl ThreadLease {
    pub fn thread(&self) -> &ThreadId {
        &self.thread
    }
}

impl std::fmt::Debug for ThreadLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ThreadLease").field(&self.thread).finish()
    }
}

impl Drop for ThreadLease {
    fn drop(&mut self) {
        if let Some(demand) = self.demand.upgrade() {
            demand.release(&self.thread);
        }
    }
}

/// The runtime's record of the follows it started in the current session context, by job slot.
#[derive(Default)]
pub(crate) struct ThreadFollows {
    next: u64,
    started: BTreeMap<ThreadId, u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FollowPlan {
    pub start: Vec<(ThreadId, u64)>,
    pub stop: Vec<u64>,
}

impl ThreadFollows {
    /// Starts every open thread not yet started in this context and stops every started thread
    /// no longer open. A follow that ended by itself (a deleted thread, a closed stream) stays
    /// recorded, so it is not restarted in a loop; the next session context starts it again.
    pub(crate) fn reconcile(&mut self, open: &BTreeSet<ThreadId>) -> FollowPlan {
        let mut plan = FollowPlan::default();
        self.started.retain(|thread, slot| {
            let keep = open.contains(thread);
            if !keep {
                plan.stop.push(*slot);
            }
            keep
        });
        for thread in open {
            if !self.started.contains_key(thread) {
                self.next += 1;
                self.started.insert(thread.clone(), self.next);
                plan.start.push((thread.clone(), self.next));
            }
        }
        plan
    }

    /// The session context ended: every follow stops and none is remembered.
    pub(crate) fn clear(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.started).into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{FutureExt, StreamExt};

    fn thread(id: &str) -> ThreadId {
        ThreadId(id.into())
    }

    #[test]
    fn leases_count_per_thread_and_notify_only_on_first_open_and_last_close() {
        let (demand, mut changed) = ThreadDemand::new();
        let first = demand.lease(thread("a"));
        let second = demand.lease(thread("a"));
        assert_eq!(demand.open(), BTreeSet::from([thread("a")]));
        assert!(changed.next().now_or_never().flatten().is_some());
        assert!(changed.next().now_or_never().is_none());
        drop(first);
        assert_eq!(demand.open(), BTreeSet::from([thread("a")]));
        assert!(changed.next().now_or_never().is_none());
        drop(second);
        assert!(demand.open().is_empty());
        assert!(changed.next().now_or_never().flatten().is_some());
    }

    #[test]
    fn a_lease_outliving_its_client_releases_nothing() {
        let (demand, _changed) = ThreadDemand::new();
        let lease = demand.lease(thread("a"));
        drop(demand);
        drop(lease);
    }

    #[test]
    fn follows_start_once_per_context_stop_when_released_and_clear_with_the_context() {
        let mut follows = ThreadFollows::default();
        let plan = follows.reconcile(&BTreeSet::from([thread("a"), thread("b")]));
        assert_eq!(plan.start, vec![(thread("a"), 1), (thread("b"), 2)]);
        assert!(plan.stop.is_empty());
        // Already started (or ended by itself): not started again in this context.
        assert_eq!(
            follows.reconcile(&BTreeSet::from([thread("a"), thread("b")])),
            FollowPlan::default()
        );
        let plan = follows.reconcile(&BTreeSet::from([thread("b")]));
        assert_eq!(
            plan,
            FollowPlan {
                start: vec![],
                stop: vec![1]
            }
        );
        // A new context: everything stops, then the open threads start under new slots.
        assert_eq!(follows.clear(), vec![2]);
        let plan = follows.reconcile(&BTreeSet::from([thread("b")]));
        assert_eq!(plan.start, vec![(thread("b"), 3)]);
    }
}
