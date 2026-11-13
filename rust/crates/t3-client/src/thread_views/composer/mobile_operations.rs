//! Draft-scoped asynchronous work is cancelled when its owner discards or leaves the draft.
use futures::{
    Future,
    future::{AbortHandle, AbortRegistration, Abortable, Aborted},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
};

#[derive(Default)]
pub struct DraftOperations {
    pending: Rc<RefCell<BTreeMap<String, Vec<Rc<AbortHandle>>>>>,
}
pub struct DraftOperation {
    key: String,
    handle: Rc<AbortHandle>,
    registration: Option<AbortRegistration>,
    pending: Weak<RefCell<BTreeMap<String, Vec<Rc<AbortHandle>>>>>,
}
impl DraftOperations {
    pub fn start(&self, key: &str) -> DraftOperation {
        let (handle, registration) = AbortHandle::new_pair();
        let handle = Rc::new(handle);
        self.pending
            .borrow_mut()
            .entry(key.into())
            .or_default()
            .push(handle.clone());
        DraftOperation {
            key: key.into(),
            handle,
            registration: Some(registration),
            pending: Rc::downgrade(&self.pending),
        }
    }
    pub fn abort(&self, key: &str) {
        if let Some(pending) = self.pending.borrow_mut().remove(key) {
            for handle in pending {
                handle.abort();
            }
        }
    }
    pub fn abort_where(&self, matches: impl Fn(&str) -> bool) {
        let keys = self
            .pending
            .borrow()
            .keys()
            .filter(|key| matches(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.abort(&key);
        }
    }
}
impl DraftOperation {
    pub fn is_aborted(&self) -> bool {
        self.handle.is_aborted()
    }
    pub async fn run<T>(mut self, work: impl Future<Output = T>) -> Result<T, Aborted> {
        let Some(registration) = self.registration.take() else {
            return Err(Aborted);
        };
        Abortable::new(work, registration).await
    }
}
impl Drop for DraftOperation {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.upgrade() {
            let mut pending = pending.borrow_mut();
            if let Some(handles) = pending.get_mut(&self.key) {
                handles.retain(|handle| !Rc::ptr_eq(handle, &self.handle));
                if handles.is_empty() {
                    pending.remove(&self.key);
                }
            }
        }
    }
}
