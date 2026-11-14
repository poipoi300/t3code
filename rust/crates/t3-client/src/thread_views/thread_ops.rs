//! Composed, destination-guarded operations shared by the sidebar and timeline.
pub mod approvals;
pub mod plans;

use crate::{ClientError, composition::ClientRuntime, platform::Platform, rpc::SessionEndpoint,
    types::{EnvironmentId, ThreadKey}, workspace::project_ops::ProjectCommands};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use futures::{StreamExt, stream::LocalBoxStream};

#[derive(Default)]
pub struct ThreadOpsState {
    approvals: RefCell<BTreeMap<ThreadKey, approvals::PendingApprovals>>,
    files: ProjectCommands,
}

#[derive(Clone)]
pub struct ThreadOperations<P: Platform + Clone + 'static> {
    pub(crate) runtime: ClientRuntime<P>,
    pub(crate) platform: P,
    pub(crate) state: Rc<ThreadOpsState>,
}
impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn new(runtime: ClientRuntime<P>, platform: P, state: Rc<ThreadOpsState>) -> Self {
        Self { runtime, platform, state }
    }
    pub(crate) fn endpoint(&self, environment: &EnvironmentId) -> Result<SessionEndpoint, ClientError> {
        self.runtime.client(environment).and_then(|c| c.session()).ok_or(ClientError::Disconnected)
    }
    pub fn changes(&self, key: &ThreadKey) -> LocalBoxStream<'static, ()> {
        self.runtime.store().subscribe_environment(&key.environment).map(|_| ()).boxed_local()
    }
    pub(crate) fn refused(message: &str) -> ClientError {
        ClientError::Rejected { message: message.into(), detail: None }
    }
}
