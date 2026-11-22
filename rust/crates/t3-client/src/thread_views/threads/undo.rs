use crate::types::ThreadKey;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
};
#[derive(Default)]
struct UndoObservation {
    pending: Option<u64>,
    waker: Option<std::task::Waker>,
}
#[derive(Default)]
struct UndoState {
    next: u64,
    revision: u64,
    actions: BTreeMap<(String, ThreadKey), u64>,
    observers: Vec<Weak<RefCell<UndoObservation>>>,
}
#[derive(Clone, Default)]
pub struct ThreadUndoRegistry(Rc<RefCell<UndoState>>);
pub struct UndoToken {
    owner: Weak<RefCell<UndoState>>,
    key: (String, ThreadKey),
    id: u64,
}
pub struct UndoSubscription {
    owner: Weak<RefCell<UndoState>>,
    slot: Rc<RefCell<UndoObservation>>,
}
impl Drop for UndoSubscription {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let slot = Rc::downgrade(&self.slot);
            owner
                .borrow_mut()
                .observers
                .retain(|observer| !observer.ptr_eq(&slot));
        }
    }
}
impl futures::Stream for UndoSubscription {
    type Item = u64;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u64>> {
        let mut slot = self.slot.borrow_mut();
        if let Some(revision) = slot.pending.take() {
            return std::task::Poll::Ready(Some(revision));
        }
        if self.owner.upgrade().is_none() {
            return std::task::Poll::Ready(None);
        }
        slot.waker = Some(cx.waker().clone());
        std::task::Poll::Pending
    }
}
fn change(owner: &Rc<RefCell<UndoState>>, update: impl FnOnce(&mut UndoState) -> bool) {
    let wakers = {
        let mut state = owner.borrow_mut();
        if !update(&mut state) {
            return;
        }
        state.revision += 1;
        let revision = state.revision;
        let mut wakers = vec![];
        state.observers.retain(|observer| {
            let Some(observer) = observer.upgrade() else {
                return false;
            };
            let mut slot = observer.borrow_mut();
            slot.pending = Some(revision);
            if let Some(waker) = slot.waker.take() {
                wakers.push(waker);
            }
            true
        });
        wakers
    };
    for waker in wakers {
        waker.wake();
    }
}
impl ThreadUndoRegistry {
    pub fn revision(&self) -> u64 {
        self.0.borrow().revision
    }
    pub fn begin(&self, kind: String, thread: ThreadKey) -> UndoToken {
        let key = (kind, thread);
        let mut id = 0;
        change(&self.0, |state| {
            state.next += 1;
            id = state.next;
            state.actions.insert(key.clone(), id);
            true
        });
        UndoToken {
            owner: Rc::downgrade(&self.0),
            key,
            id,
        }
    }
    pub fn invalidate(&self, kind: String, thread: ThreadKey) {
        change(&self.0, |state| {
            state.actions.remove(&(kind, thread));
            true
        });
    }
    pub fn invalidate_environment(&self, environment: &crate::types::EnvironmentId) {
        change(&self.0, |state| {
            let before = state.actions.len();
            state
                .actions
                .retain(|(_, key), _| &key.environment != environment);
            state.actions.len() != before
        });
    }
    pub fn subscribe(&self) -> UndoSubscription {
        let mut state = self.0.borrow_mut();
        let slot = Rc::new(RefCell::new(UndoObservation {
            pending: Some(state.revision),
            waker: None,
        }));
        state.observers.push(Rc::downgrade(&slot));
        UndoSubscription {
            owner: Rc::downgrade(&self.0),
            slot,
        }
    }
}
impl UndoToken {
    pub fn is_current(&self) -> bool {
        self.owner
            .upgrade()
            .is_some_and(|owner| owner.borrow().actions.get(&self.key) == Some(&self.id))
    }
    pub fn finish(&self) {
        if let Some(owner) = self.owner.upgrade() {
            change(&owner, |state| {
                if state.actions.get(&self.key) != Some(&self.id) {
                    return false;
                }
                state.actions.remove(&self.key);
                true
            });
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadUndoStep {
    Unsettle,
    Unsnooze,
    Pin { order_key: Option<String> },
    Snooze { snoozed_until: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoAction {
    Settle,
    Snooze,
    Unpin,
}
pub fn plan_thread_undo(
    action: UndoAction,
    pinned: bool,
    order_key: Option<String>,
    snoozed_until: Option<String>,
) -> Vec<ThreadUndoStep> {
    if action == UndoAction::Snooze {
        return vec![ThreadUndoStep::Unsnooze];
    }
    let mut steps = vec![];
    if action == UndoAction::Settle {
        steps.push(ThreadUndoStep::Unsettle);
    }
    if action == UndoAction::Unpin || pinned {
        steps.push(ThreadUndoStep::Pin { order_key });
    }
    if let Some(snoozed_until) = snoozed_until {
        steps.push(ThreadUndoStep::Snooze { snoozed_until });
    }
    steps
}
pub async fn execute_thread_undo<T, F: std::future::Future<Output = T>>(
    plan: &[ThreadUndoStep],
    mut execute: impl FnMut(ThreadUndoStep) -> F,
    succeeded: impl Fn(&T) -> bool,
    now: impl Fn() -> i64,
    parse: impl Fn(&str) -> Option<i64>,
) -> Option<T> {
    let (first, rest) = plan.split_first()?;
    let mut result = execute(first.clone()).await;
    for step in rest {
        if !succeeded(&result) {
            return Some(result);
        }
        if let ThreadUndoStep::Snooze { snoozed_until } = step
            && parse(snoozed_until).is_some_and(|at| at <= now())
        {
            continue;
        }
        result = execute(step.clone()).await;
    }
    Some(result)
}
