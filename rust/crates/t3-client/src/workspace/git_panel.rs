//! Composed workspace services used by the details panel on every client.
pub mod actions;
pub mod providers;
mod context;

use crate::{
    ClientError, composition::{ClientRuntime, Spawner}, platform::Platform,
    rpc::SessionEndpoint, services::resources::{Resource, ResourcePolicy},
    types::{EnvironmentId, Generation, ThreadKey},
};
use super::{project_ops::{actions::VcsActionManager, source_control::SourceControl, subscriptions::LiveVcs}, vcs::CheckoutKey};
use futures::{FutureExt, StreamExt, future::{AbortHandle, Abortable}, stream::LocalBoxStream};
use std::{cell::RefCell, collections::BTreeMap, rc::{Rc, Weak}};
use t3_contracts::{git::*, orchestration_v2::OrchestrationV2ThreadShell};

#[derive(Default)]
pub(crate) struct GitPanelState {
    source: SourceControl,
    actions: VcsActionManager,
    checkouts: RefCell<BTreeMap<CheckoutKey, Weak<Checkout>>>,
    diff: RefCell<crate::view_logic::shell::diff_state::DiffStore>,
}

#[derive(Clone)]
pub struct GitPanel<P: Platform + Clone + 'static> {
    pub(crate) runtime: ClientRuntime<P>,
    pub(crate) platform: P,
    pub(crate) spawn: Spawner,
    state: Rc<GitPanelState>,
}

pub(crate) struct Checkout {
    status: Resource<VcsStatusResult>,
    tasks: RefCell<Vec<AbortHandle>>,
}
impl Drop for Checkout {
    fn drop(&mut self) {
        for task in self.tasks.get_mut() { task.abort(); }
    }
}

impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub(crate) fn new(runtime: ClientRuntime<P>, platform: P, spawn: Spawner, state: Rc<GitPanelState>) -> Self {
        Self { runtime, platform, spawn, state }
    }
    pub fn session(&self, environment: &EnvironmentId) -> Result<SessionEndpoint, ClientError> {
        self.runtime.client(environment).and_then(|c| c.session()).ok_or(ClientError::Disconnected)
    }
    pub fn thread(&self, key: &ThreadKey) -> Option<Rc<OrchestrationV2ThreadShell>> {
        self.runtime.store().shared_shell(&key.environment)?.threads.iter().find(|t| t.id == key.thread).cloned()
    }
    fn task(&self, checkout: &Checkout, future: impl std::future::Future<Output = ()> + 'static) {
        let (abort, registration) = AbortHandle::new_pair();
        checkout.tasks.borrow_mut().push(abort);
        (self.spawn)(Abortable::new(future, registration).map(|_| ()).boxed_local());
    }
    fn checkout(&self, target: &CheckoutKey) -> Rc<Checkout> {
        if let Some(checkout) = self.state.checkouts.borrow().get(target).and_then(Weak::upgrade) { return checkout; }
        let live = Rc::new(LiveVcs::new(self.state.source.refs.clone()));
        let input = VcsStatusSubscriptionInput { cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(target.cwd.clone()), include_remote: None };
        let status = live.status(target.environment.clone(), &input).unwrap_or_else(|_| Resource::new(ResourcePolicy::default(), Generation::default()));
        let checkout = Rc::new(Checkout { status, tasks: RefCell::default() });
        self.state.checkouts.borrow_mut().insert(target.clone(), Rc::downgrade(&checkout));
        let service = self.clone();
        let target = target.clone();
        let follower = live.clone();
        let env = target.environment.clone();
        let platform = self.platform.clone();
        self.task(&checkout, async move { let _ = follower.follow_status(env, &platform, input).await; });
        let mut changes = self.runtime.store().subscribe();
        let mut runtime_changes = self.runtime.changes();
        let mut events = self.platform.events();
        self.task(&checkout, async move {
            let mut generation = None;
            loop {
                let current = service.runtime.store().snapshot().environments.get(&target.environment).map(|e| e.session);
                if current != generation {
                    generation = current;
                    let endpoint = service.session(&target.environment).ok();
                    let _ = live.set_session(target.environment.clone(), current.unwrap_or_default(), endpoint.clone().map(Rc::new));
                    if let Some(endpoint) = endpoint {
                        service.state.source.begin_session(&target.environment, current.unwrap_or_default());
                        let _ = service.state.source.refresh_status(target.environment.clone(), &endpoint, &service.platform, t3_contracts::rpc::WsVcsRefreshStatusPayload { cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(target.cwd.clone()), local_only: None }).await;
                    }
                }
                futures::select! {
                    change = changes.next().fuse() => { if change.is_none() { break; } },
                    change = runtime_changes.next().fuse() => { if change.is_none() { break; } },
                    event = events.next().fuse() => {
                        match event {
                            Some(crate::platform::PlatformEvent::Foreground) => {
                                if let Ok(endpoint) = service.session(&target.environment) {
                                    let _ = service.state.source.refresh_status(target.environment.clone(), &endpoint, &service.platform, t3_contracts::rpc::WsVcsRefreshStatusPayload { cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(target.cwd.clone()), local_only: None }).await;
                                }
                            },
                            Some(_) => (),
                            None => events = futures::stream::pending().boxed_local(),
                        }
                    },
                }
            }
        });
        checkout
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.runtime.store().subscribe().map(|_| ()).boxed_local()
    }
}
