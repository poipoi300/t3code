//! Composed, destination-guarded operations shared by the sidebar and timeline.
pub mod actions;
pub mod approvals;
pub mod deletion;
pub mod header;
pub mod menu;
pub mod metadata;
pub mod plans;
pub mod projects;
pub mod recall;
pub mod rewind;
pub mod snooze;
pub mod undo;

use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::SessionEndpoint,
    types::{EnvironmentId, ThreadKey},
    workspace::project_ops::ProjectCommands,
};
use futures::{StreamExt, stream::LocalBoxStream};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Default)]
pub struct ThreadOpsState {
    approvals: RefCell<BTreeMap<ThreadKey, approvals::PendingApprovals>>,
    files: ProjectCommands,
    actions: actions::ActionState,
    deletion: deletion::DeletionState,
    projects: crate::workspace::project_ops::ProjectEntities,
    rewind: rewind::RewindStateStore,
    shells: RefCell<Rc<crate::thread_views::threads::shell::ThreadShellViews>>,
    undo: undo::UndoState,
    confirmation: RefCell<Option<actions::ConfirmationPort>>,
    unsupported: RefCell<BTreeMap<(EnvironmentId, &'static str), u64>>,
    answering: RefCell<BTreeSet<(ThreadKey, String)>>,
}
impl ThreadOpsState {
    pub(crate) fn forget_environment(&self, environment: &EnvironmentId) {
        self.approvals
            .borrow_mut()
            .retain(|key, _| &key.environment != environment);
        self.files.remove_environment(environment);
        self.actions.forget_environment(environment);
        self.undo.forget_environment(environment);
        self.rewind.forget_environment(environment);
        self.unsupported
            .borrow_mut()
            .retain(|(key, _), _| key != environment);
        self.answering
            .borrow_mut()
            .retain(|(key, _)| &key.environment != environment);
        *self.shells.borrow_mut() = Rc::default();
        self.actions.notify();
    }
}

#[derive(Clone)]
pub struct ThreadOperations<P: Platform + Clone + 'static> {
    pub(crate) runtime: ClientRuntime<P>,
    pub(crate) platform: P,
    pub(crate) state: Rc<ThreadOpsState>,
}
impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    fn session_generation(&self, environment: &EnvironmentId) -> Option<u64> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(environment)
            .map(|e| e.session.0)
    }
    fn unsupported(&self, environment: &EnvironmentId, method: &'static str) -> bool {
        self.state
            .unsupported
            .borrow()
            .get(&(environment.clone(), method))
            .is_some_and(|g| Some(*g) == self.session_generation(environment))
    }
    fn remember_unsupported(
        &self,
        environment: &EnvironmentId,
        method: &'static str,
        generation: Option<u64>,
        error: &ClientError,
    ) {
        if matches!(error, ClientError::Unsupported(_))
            && generation == self.session_generation(environment)
            && let Some(generation) = generation
        {
            self.state
                .unsupported
                .borrow_mut()
                .insert((environment.clone(), method), generation);
            self.state.actions.notify();
        }
    }
    pub fn new(runtime: ClientRuntime<P>, platform: P, state: Rc<ThreadOpsState>) -> Self {
        Self {
            runtime,
            platform,
            state,
        }
    }
    pub(crate) fn endpoint(
        &self,
        environment: &EnvironmentId,
    ) -> Result<SessionEndpoint, ClientError> {
        self.runtime
            .client(environment)
            .and_then(|c| c.session())
            .ok_or(ClientError::Disconnected)
    }
    pub fn changes(&self, key: &ThreadKey) -> LocalBoxStream<'static, ()> {
        futures::stream::select(
            self.runtime
                .store()
                .subscribe_environment(&key.environment)
                .map(|_| ()),
            self.state.actions.changes(),
        )
        .boxed_local()
    }
    pub(crate) fn refused(message: &str) -> ClientError {
        ClientError::InvalidPayload(message.into())
    }
    fn command_failure(
        &self,
        environment: &EnvironmentId,
        method: &'static str,
        generation: Option<u64>,
        error: crate::rpc::RpcCallError,
    ) -> ClientError {
        let error = error.into_client_error();
        self.remember_unsupported(environment, method, generation, &error);
        error
    }
}
