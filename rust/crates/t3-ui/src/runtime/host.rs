//! The one place that chooses the host. Swapping in the real client composition root (and
//! `t3_platform::platform()` under it) changes the body of `backend` and nothing else.

use std::rc::Rc;

use t3_client::ClientStore;

use super::backend::Backend;
use super::demo::DemoBackend;

pub fn backend(store: ClientStore) -> Rc<dyn Backend> {
    Rc::new(DemoBackend::new(store))
}
