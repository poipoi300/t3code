//! Composed workspace services used by the details panel on every client.
pub mod actions;
pub mod branches;
mod context;
pub mod details;
pub mod editors;
pub mod providers;
pub mod scripts;
pub use context::WorkspaceContext;

use super::{
    project_ops::{
        actions::VcsActionManager, source_control::SourceControl, subscriptions::LiveVcs,
    },
    vcs::CheckoutKey,
};
use crate::{
    ClientError,
    composition::{ClientRuntime, Spawner},
    platform::Platform,
    rpc::SessionEndpoint,
    services::resources::{Resource, ResourcePolicy},
    types::{EnvironmentId, Generation, ThreadKey},
};
use futures::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable},
    stream::LocalBoxStream,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::{git::*, orchestration_v2::OrchestrationV2ThreadShell};

type BranchSynchronization = (Option<String>, Option<String>);

#[derive(Default)]
pub(crate) struct GitPanelState {
    source: SourceControl,
    actions: VcsActionManager,
    checkouts: RefCell<BTreeMap<CheckoutKey, Weak<Checkout>>>,
    diff: RefCell<crate::view_logic::shell::diff_state::DiffStore>,
    synced_branches: RefCell<BTreeMap<ThreadKey, BranchSynchronization>>,
    editor_changes: PanelChanges,
    scripts: scripts::ScriptsState,
    context_overrides: RefCell<BTreeMap<ThreadKey, (super::branch::EnvMode, bool)>>,
    context_changes: PanelChanges,
    diff_panel: super::diff_panel::DiffPanelState,
}

/// The diff panel shares this panel's diff selection, as both read diffPanelStore.ts.
impl GitPanelState {
    pub(crate) fn diff_store(&self) -> &RefCell<crate::view_logic::shell::diff_state::DiffStore> {
        &self.diff
    }
    pub(crate) fn diff_panel(&self) -> &super::diff_panel::DiffPanelState {
        &self.diff_panel
    }
    pub(crate) fn diff_changed(&self) {
        self.context_changes.notify();
    }
    pub(crate) fn diff_changes(&self) -> LocalBoxStream<'static, ()> {
        self.context_changes.subscribe()
    }
}

struct PanelChanges(Resource<u64>);
impl Default for PanelChanges {
    fn default() -> Self {
        Self(Resource::new(
            ResourcePolicy::default(),
            Generation::default(),
        ))
    }
}
impl PanelChanges {
    fn subscribe(&self) -> LocalBoxStream<'static, ()> {
        self.0.observe(0).map(|_| ()).boxed_local()
    }
    fn notify(&self) {
        let revision = self.0.snapshot().value.map_or(0, |v| *v) + 1;
        let token = self.0.begin();
        self.0.finish(token, revision, 0, false);
    }
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
        for task in self.tasks.get_mut() {
            task.abort();
        }
    }
}

impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub(crate) fn new(
        runtime: ClientRuntime<P>,
        platform: P,
        spawn: Spawner,
        state: Rc<GitPanelState>,
    ) -> Self {
        Self {
            runtime,
            platform,
            spawn,
            state,
        }
    }
    pub(crate) fn session(
        &self,
        environment: &EnvironmentId,
    ) -> Result<SessionEndpoint, ClientError> {
        self.runtime
            .client(environment)
            .and_then(|c| c.session())
            .ok_or(ClientError::Disconnected)
    }
    pub fn thread(&self, key: &ThreadKey) -> Option<Rc<OrchestrationV2ThreadShell>> {
        self.runtime
            .store()
            .shared_shell(&key.environment)?
            .threads
            .iter()
            .find(|t| t.id == key.thread)
            .cloned()
    }
    pub fn set_diff_context(
        &self,
        key: &ThreadKey,
        base_ref: Option<&str>,
        ignore_whitespace: Option<bool>,
    ) {
        let mut diff = self.state.diff.borrow_mut();
        diff.select_base(key.clone(), base_ref);
        diff.ignore_whitespace = ignore_whitespace;
        drop(diff);
        self.state.context_changes.notify();
    }
    fn task(&self, checkout: &Checkout, future: impl std::future::Future<Output = ()> + 'static) {
        let (abort, registration) = AbortHandle::new_pair();
        checkout.tasks.borrow_mut().push(abort);
        (self.spawn)(
            Abortable::new(future, registration)
                .map(|_| ())
                .boxed_local(),
        );
    }
    fn checkout(&self, target: &CheckoutKey) -> Rc<Checkout> {
        if let Some(checkout) = self
            .state
            .checkouts
            .borrow()
            .get(target)
            .and_then(Weak::upgrade)
        {
            return checkout;
        }
        self.state
            .checkouts
            .borrow_mut()
            .retain(|_, checkout| checkout.strong_count() > 0);
        let live = Rc::new(LiveVcs::new(self.state.source.refs.clone()));
        let input = VcsStatusSubscriptionInput {
            cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(target.cwd.clone()),
            include_remote: None,
        };
        let status = live
            .status(target.environment.clone(), &input)
            .unwrap_or_else(|_| Resource::new(ResourcePolicy::default(), Generation::default()));
        let checkout = Rc::new(Checkout {
            status,
            tasks: RefCell::default(),
        });
        self.state
            .checkouts
            .borrow_mut()
            .insert(target.clone(), Rc::downgrade(&checkout));
        let service = self.clone();
        let target = target.clone();
        let follower = live.clone();
        let env = target.environment.clone();
        let platform = self.platform.clone();
        self.task(&checkout, async move {
            let _ = follower.follow_status(env, &platform, input).await;
        });
        let mut changes = self.runtime.store().subscribe();
        let mut runtime_changes = self.runtime.changes();
        let mut events = self.platform.events();
        self.task(&checkout, async move {
            let mut generation = None;
            let mut revalidation = super::vcs::VcsStatusRevalidation::default();
            revalidation.wake_debounce_ms = 250;
            revalidation.subscribe(target.clone(), None);
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
                let due = revalidation.next_due();
                let wake = async {
                    if let Some(due) = due {
                        service.platform.sleep(due.saturating_sub(service.platform.monotonic_ms())).await;
                    } else {
                        futures::future::pending::<()>().await;
                    }
                }.boxed_local();
                futures::pin_mut!(wake);
                futures::select! {
                    _ = wake.fuse() => {
                        for _ in revalidation.take_due(service.platform.monotonic_ms()) {
                            if let Ok(endpoint) = service.session(&target.environment) {
                                let _ = service.state.source.refresh_status(target.environment.clone(), &endpoint, &service.platform, t3_contracts::rpc::WsVcsRefreshStatusPayload { cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(target.cwd.clone()), local_only: None }).await;
                            }
                        }
                    },
                    change = changes.next().fuse() => { if change.is_none() { break; } },
                    change = runtime_changes.next().fuse() => { if change.is_none() { break; } },
                    event = events.next().fuse() => {
                        match event {
                            Some(crate::platform::PlatformEvent::Foreground) => {
                                revalidation.wake(service.platform.monotonic_ms());
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
        futures::stream::select(
            self.runtime.store().subscribe().map(|_| ()),
            self.state.context_changes.subscribe(),
        )
        .boxed_local()
    }
}
