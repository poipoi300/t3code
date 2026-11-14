use super::super::branch::{self, EnvMode, PreviousWorktree, WorktreeThread};
use super::*;
use crate::rpc::RpcCallError;
use futures::{
    channel::mpsc,
    future::LocalBoxFuture,
    stream::{self, SelectAll},
};
use serde_json::json;
use t3_contracts::{base_schemas::TrimmedNonEmptyString, rpc::*};

#[derive(Clone, Debug, PartialEq)]
pub struct BranchItem {
    pub reference: VcsRef,
    pub disabled_reason: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchSelectorState {
    pub key: Option<ThreadKey>,
    pub project: Option<super::super::ProjectKey>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub current_git_branch: Option<String>,
    pub checkout_root: Option<String>,
    pub refs: Vec<VcsRef>,
    pub filtered: Vec<BranchItem>,
    pub query: String,
    pub next_cursor: Option<u64>,
    pub total_count: u64,
    pub loading: bool,
    pub loading_next: bool,
    pub pending: bool,
    pub error: Option<String>,
    pub unsupported: bool,
    pub can_write: bool,
    pub can_operate: bool,
    pub can_change: bool,
    pub env_mode: EnvMode,
    pub env_locked: bool,
    pub start_from_origin: bool,
    pub selecting_worktree_base: bool,
    pub previous_worktree: Option<PreviousWorktree>,
    pub create_name: Option<String>,
    pub project_cloning: bool,
    /// The checkout's `isRepo`; `None` until its status arrives.
    pub is_repo: Option<bool>,
    pub offers_git: bool,
}
struct Core {
    view: Resource<BranchSelectorState>,
    optimistic: RefCell<Option<Option<String>>>,
    trigger: mpsc::UnboundedSender<()>,
    clones: Resource<Vec<t3_contracts::project_clone::ProjectCloneSnapshot>>,
    checkout_root: RefCell<Option<String>>,
    fetch: RefCell<Option<AbortHandle>>,
    fetch_generation: std::cell::Cell<u64>,
}
struct Owner {
    core: Rc<Core>,
    abort: AbortHandle,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.abort.abort();
        if let Some(fetch) = self.core.fetch.borrow_mut().take() {
            fetch.abort();
        }
    }
}
#[derive(Clone)]
pub struct BranchSelectorLease<P: Platform + Clone + 'static> {
    service: GitPanel<P>,
    key: ThreadKey,
    owner: Rc<Owner>,
}
fn publish(core: &Core, state: BranchSelectorState) {
    let token = core.view.begin();
    core.view.finish(token, state, 0, false);
}

struct PendingSelection {
    core: Rc<Core>,
    previous: Option<String>,
}
impl PendingSelection {
    fn begin(core: Rc<Core>, branch: String) -> Self {
        let mut state = core
            .view
            .snapshot()
            .value
            .as_deref()
            .cloned()
            .unwrap_or_default();
        let previous = state.branch.clone();
        *core.optimistic.borrow_mut() = Some(Some(branch.clone()));
        state.branch = Some(branch);
        state.pending = true;
        state.error = None;
        publish(&core, state);
        Self { core, previous }
    }
}
impl Drop for PendingSelection {
    fn drop(&mut self) {
        *self.core.optimistic.borrow_mut() = None;
        let mut state = self
            .core
            .view
            .snapshot()
            .value
            .as_deref()
            .cloned()
            .unwrap_or_default();
        state.pending = false;
        state.branch = self.previous.clone();
        publish(&self.core, state);
    }
}

impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub fn branch_selector(&self, key: &ThreadKey) -> BranchSelectorLease<P> {
        let (trigger, receiver) = mpsc::unbounded();
        let clones = self.runtime.query::<WsSubscribeProjectClonesRpc>(
            &key.environment,
            t3_contracts::project_clone::ProjectCloneSubscribeInput {},
            ResourcePolicy::default(),
        );
        let core = Rc::new(Core {
            view: Resource::new(ResourcePolicy::default(), Generation::default()),
            optimistic: RefCell::new(None),
            trigger,
            clones: clones.resource().as_ref().clone(),
            checkout_root: RefCell::new(None),
            fetch: RefCell::new(None),
            fetch_generation: std::cell::Cell::new(0),
        });
        publish(&core, BranchSelectorState::default());
        let (abort, registration) = AbortHandle::new_pair();
        let lease = BranchSelectorLease {
            service: self.clone(),
            key: key.clone(),
            owner: Rc::new(Owner {
                core: core.clone(),
                abort,
            }),
        };
        lease.recompute();
        let service = self.clone();
        let key = key.clone();
        (self.spawn)(
            Abortable::new(
                async move {
                    let mut changes = SelectAll::<LocalBoxStream<'static, ()>>::new();
                    changes.push(
                        service
                            .runtime
                            .store()
                            .subscribe_environment(&key.environment)
                            .map(|_| ())
                            .boxed_local(),
                    );
                    changes.push(service.runtime.composer_drafts().changes());
                    changes.push(service.state.context_changes.subscribe());
                    changes.push(receiver.boxed_local());
                    changes.push(
                        service
                            .state
                            .source
                            .refs
                            .observe(&key.environment)
                            .map(|_| ())
                            .boxed_local(),
                    );
                    changes.push(
                        clones
                            .resource()
                            .observe(service.platform.monotonic_ms())
                            .map(|_| ())
                            .boxed_local(),
                    );
                    let mut checkout = None;
                    let mut project_checkout: Option<Rc<Checkout>> = None;
                    let mut identity = None;
                    while changes.next().await.is_some() {
                        let current = service.workspace_context(&key, None);
                        let Some(context) = current else {
                            publish(
                                &core,
                                BranchSelectorState {
                                    key: Some(key.clone()),
                                    ..Default::default()
                                },
                            );
                            continue;
                        };
                        let cwd = context
                            .worktree_path
                            .as_deref()
                            .unwrap_or(&context.project.workspace_root.0)
                            .to_owned();
                        let next_checkout = service.checkout(&CheckoutKey {
                            environment: key.environment.clone(),
                            cwd: cwd.clone(),
                        });
                        if checkout
                            .as_ref()
                            .is_none_or(|c| !Rc::ptr_eq(c, &next_checkout))
                        {
                            changes.push(
                                next_checkout
                                    .status
                                    .observe(service.platform.monotonic_ms())
                                    .map(|_| ())
                                    .boxed_local(),
                            );
                            checkout = Some(next_checkout);
                        }
                        let next_project = service.checkout(&CheckoutKey {
                            environment: key.environment.clone(),
                            cwd: context.project.workspace_root.0.clone(),
                        });
                        if project_checkout
                            .as_ref()
                            .is_none_or(|c| !Rc::ptr_eq(c, &next_project))
                        {
                            changes.push(
                                next_project
                                    .status
                                    .observe(service.platform.monotonic_ms())
                                    .map(|_| ())
                                    .boxed_local(),
                            );
                            project_checkout = Some(next_project);
                        }
                        *core.checkout_root.borrow_mut() = project_checkout
                            .as_ref()
                            .and_then(|c| c.status.snapshot().value)
                            .and_then(|s| s.working_tree.root_path.clone().flatten());
                        let clone_done = clones.resource().snapshot().value.is_some_and(|c| {
                            c.iter().any(|c| {
                                c.project_id == context.project.id
                                    && c.phase
                                        == t3_contracts::project_clone::ProjectClonePhase::Done
                            })
                        });
                        let query = core
                            .view
                            .snapshot()
                            .value
                            .as_ref()
                            .map(|s| s.query.clone())
                            .unwrap_or_default();
                        let generation = service
                            .runtime
                            .store()
                            .snapshot()
                            .environments
                            .get(&key.environment)
                            .map(|e| e.session);
                        let next = (
                            cwd,
                            query,
                            generation,
                            service.state.source.refs.state(&key.environment).revision,
                            clone_done,
                        );
                        if identity.as_ref() != Some(&next) {
                            identity = Some(next);
                            let _ = fetch_refs(&service, &key, &core, false, true).await;
                        }
                        recompute(&service, &key, &core, checkout.as_deref());
                    }
                },
                registration,
            )
            .map(|_| ())
            .boxed_local(),
        );
        lease
    }
}

fn recompute<P: Platform + Clone + 'static>(
    service: &GitPanel<P>,
    key: &ThreadKey,
    core: &Core,
    checkout: Option<&Checkout>,
) {
    let mut state = core
        .view
        .snapshot()
        .value
        .as_deref()
        .cloned()
        .unwrap_or_default();
    let Some(c) = service.workspace_context(key, None) else {
        return;
    };
    let cwd = c
        .worktree_path
        .as_deref()
        .unwrap_or(&c.project.workspace_root.0);
    let endpoint = service.session(&key.environment).ok();
    state.can_write = endpoint.as_ref().is_some_and(|e| {
        e.available::<WsVcsSwitchRefRpc>(&VcsSwitchRefInput {
            cwd: TrimmedNonEmptyString(cwd.into()),
            ref_name: TrimmedNonEmptyString("availability".into()),
        })
    });
    let command = serde_json::from_value(json!({"type":"thread.metadata.update","commandId":"availability","threadId":key.thread,"branch":null})).ok();
    state.can_operate = command.is_some_and(|command| {
        endpoint
            .as_ref()
            .is_some_and(|e| e.available::<WsOrchestrationV2DispatchCommandRpc>(&command))
    });
    state.can_change = state.can_write && (c.thread.is_none() || state.can_operate);
    state.env_mode = c.env_mode;
    state.env_locked = c
        .thread
        .as_ref()
        .is_some_and(|t| t.active_provider_thread_id.is_some() || t.worktree_path.is_some());
    state.start_from_origin = c.start_from_origin;
    state.key = Some(key.clone());
    state.project = Some(super::super::ProjectKey {
        environment: key.environment.clone(),
        project: c.project.id.clone(),
    });
    state.worktree_path = c.worktree_path.clone();
    state.project_cloning = core.clones.snapshot().value.is_some_and(|clones| {
        clones.iter().any(|clone| {
            clone.project_id == c.project.id
                && clone.phase != t3_contracts::project_clone::ProjectClonePhase::Done
        })
    });
    state.selecting_worktree_base =
        c.env_mode == EnvMode::Worktree && !state.env_locked && c.worktree_path.is_none();
    let status = checkout.and_then(|c| c.status.snapshot().value);
    state.is_repo = status.as_ref().map(|s| s.is_repo);
    state.offers_git = service.offers_git(key);
    state.current_git_branch = status
        .as_ref()
        .and_then(|s| s.ref_name.as_ref().map(|b| b.0.clone()))
        .or_else(|| {
            state
                .refs
                .iter()
                .find(|r| r.current)
                .map(|r| r.name.0.clone())
        });
    state.checkout_root = status
        .as_ref()
        .and_then(|s| s.working_tree.root_path.clone().flatten());
    state.branch = core.optimistic.borrow().clone().unwrap_or_else(|| {
        branch::branch_toolbar_value(
            c.env_mode,
            c.worktree_path.as_deref(),
            c.branch.as_deref(),
            state.current_git_branch.as_deref(),
        )
        .map(str::to_owned)
    });
    state.previous_worktree = if c.thread.is_none() {
        service
            .runtime
            .store()
            .shared_shell(&key.environment)
            .and_then(|s| {
                let rows = s
                    .threads
                    .iter()
                    .filter(|t| t.project_id == c.project.id)
                    .map(|t| WorktreeThread {
                        branch: t.branch.as_ref().map(|b| b.0.as_str()),
                        path: t.worktree_path.as_ref().map(|p| p.0.as_str()),
                        updated_at: &t.updated_at.0,
                        archived: false,
                    })
                    .collect::<Vec<_>>();
                branch::previous_worktree_seed(&rows, c.worktree_path.as_deref())
            })
    } else {
        None
    };
    state.filtered = branch::dedupe_remote_refs_with_local_matches(&state.refs)
        .into_iter()
        .filter(|r| {
            branch::include_branch_picker_item(
                &r.name.0,
                &state.query.trim().to_lowercase(),
                None,
                None,
            )
        })
        .map(|r| {
            let target = branch::branch_selection_target(
                &c.project.workspace_root.0,
                core.checkout_root.borrow().as_deref(),
                c.worktree_path.as_deref(),
                r,
            );
            let blocked = branch::worktree_change_blocked(
                c.thread.as_ref().is_some_and(|t| t.active_run_id.is_some()),
                c.worktree_path.as_deref(),
                target.next_worktree_path.as_deref(),
            );
            BranchItem {
                reference: r.clone(),
                disabled_reason: if blocked {
                    Some("Stop the current turn to switch worktrees.".into())
                } else if !state.can_change {
                    Some("This connection cannot change the thread's branch.".into())
                } else {
                    None
                },
            }
        })
        .collect();
    let name = branch::sanitize_new_ref_name(&state.query);
    state.create_name = (state.can_change
        && !state.selecting_worktree_base
        && !name.is_empty()
        && !state.refs.iter().any(|r| r.name.0 == name))
    .then_some(name);
    publish(core, state);
}

async fn fetch_refs<P: Platform + Clone + 'static>(
    service: &GitPanel<P>,
    key: &ThreadKey,
    core: &Core,
    next: bool,
    refresh: bool,
) -> Result<(), ClientError> {
    let context = service
        .workspace_context(key, None)
        .ok_or(ClientError::Cancelled)?;
    let cwd = context
        .worktree_path
        .as_deref()
        .unwrap_or(&context.project.workspace_root.0);
    let mut state = core
        .view
        .snapshot()
        .value
        .as_deref()
        .cloned()
        .unwrap_or_default();
    let query = state.query.clone();
    let cursor = if next { state.next_cursor } else { None };
    if next && cursor.is_none() {
        return Ok(());
    }
    state.loading = !next;
    state.loading_next = next;
    state.error = None;
    publish(core, state);
    let query_name = branch::sanitize_new_ref_name(&query);
    let input = VcsListRefsInput {
        cwd: TrimmedNonEmptyString(cwd.into()),
        query: (!query_name.is_empty()).then_some(Some(TrimmedNonEmptyString(query_name))),
        cursor: cursor.map(Some),
        include_matching_remote_refs: None,
        ref_kind: None,
        refresh: refresh.then_some(Some(true)),
        limit: Some(Some(100)),
    };
    if let Some(previous) = core.fetch.borrow_mut().take() {
        previous.abort();
    }
    let generation = core.fetch_generation.get().saturating_add(1);
    core.fetch_generation.set(generation);
    let (abort, registration) = AbortHandle::new_pair();
    *core.fetch.borrow_mut() = Some(abort);
    let request = async {
        match service.session(&key.environment) {
            Ok(endpoint) => service
                .state
                .source
                .read_refs(&key.environment, &endpoint, input)
                .await
                .map_err(RpcCallError::into_client_error),
            Err(error) => Err(error),
        }
    };
    let result = Abortable::new(request, registration)
        .await
        .unwrap_or(Err(ClientError::Cancelled));
    if core.fetch_generation.get() != generation {
        return Err(ClientError::Cancelled);
    }
    core.fetch.borrow_mut().take();
    let mut state = core
        .view
        .snapshot()
        .value
        .as_deref()
        .cloned()
        .unwrap_or_default();
    if state.query != query {
        return Err(ClientError::Cancelled);
    }
    state.loading = false;
    state.loading_next = false;
    match result {
        Ok(result) => {
            state.refs = if next {
                let mut refs = state.refs;
                for item in result.refs {
                    if !refs.iter().any(|r| r.name == item.name) {
                        refs.push(item);
                    }
                }
                refs
            } else {
                result.refs
            };
            state.next_cursor = result.next_cursor;
            state.total_count = result.total_count;
            state.unsupported = false;
            publish(core, state);
            let cloning = core.clones.snapshot().value.is_some_and(|clones| {
                clones.iter().any(|clone| {
                    clone.project_id == context.project.id
                        && clone.phase != t3_contracts::project_clone::ProjectClonePhase::Done
                })
            });
            if !cloning
                && context.env_mode == EnvMode::Worktree
                && context.worktree_path.is_none()
                && context.branch.is_none()
                && context
                    .thread
                    .as_ref()
                    .is_none_or(|t| t.active_provider_thread_id.is_none())
            {
                let candidate =
                    core.view.snapshot().value.and_then(|s| {
                        branch::default_worktree_base_branch(&s.refs).map(str::to_owned)
                    });
                if let Some(candidate) = candidate {
                    let _ = service
                        .set_branch_context(key, Some(candidate), None, true)
                        .await;
                }
            }
            Ok(())
        }
        Err(error) => {
            state.unsupported = matches!(error, ClientError::Unsupported(_));
            state.error = Some(error.to_string());
            publish(core, state);
            Err(error)
        }
    }
}

impl<P: Platform + Clone + 'static> BranchSelectorLease<P> {
    pub fn snapshot(&self) -> Rc<BranchSelectorState> {
        self.owner.core.view.snapshot().value.unwrap_or_default()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, Rc<BranchSelectorState>> {
        let owner = self.owner.clone();
        let changes = owner
            .core
            .view
            .observe(self.service.platform.monotonic_ms());
        stream::unfold((owner, changes), |(owner, mut changes)| async move {
            changes.next().await?;
            let value = owner.core.view.snapshot().value.unwrap_or_default();
            Some((value, (owner, changes)))
        })
        .boxed_local()
    }
    fn recompute(&self) {
        let checkout = self.service.workspace_context(&self.key, None).map(|c| {
            self.service.checkout(&CheckoutKey {
                environment: self.key.environment.clone(),
                cwd: c
                    .worktree_path
                    .unwrap_or_else(|| c.project.workspace_root.0.clone()),
            })
        });
        recompute(
            &self.service,
            &self.key,
            &self.owner.core,
            checkout.as_deref(),
        );
    }
    pub fn set_query(&self, query: String) {
        if let Some(fetch) = self.owner.core.fetch.borrow_mut().take() {
            fetch.abort();
        }
        let mut state = (*self.snapshot()).clone();
        state.query = query;
        state.refs.clear();
        state.next_cursor = None;
        state.loading = false;
        state.loading_next = false;
        publish(&self.owner.core, state);
        let _ = self.owner.core.trigger.unbounded_send(());
        self.recompute();
    }
    pub fn refresh(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let lease = self.clone();
        async move {
            if let Some(c) = lease.service.workspace_context(&lease.key, None)
                && let Ok(endpoint) = lease.service.session(&lease.key.environment)
            {
                let cwd = c
                    .worktree_path
                    .unwrap_or_else(|| c.project.workspace_root.0.clone());
                let _ = lease
                    .service
                    .state
                    .source
                    .refresh_local_status_for_picker(
                        lease.key.environment.clone(),
                        &endpoint,
                        &lease.service.platform,
                        WsVcsRefreshStatusPayload {
                            cwd: TrimmedNonEmptyString(cwd),
                            local_only: Some(Some(true)),
                        },
                    )
                    .await;
            }
            let result =
                fetch_refs(&lease.service, &lease.key, &lease.owner.core, false, true).await;
            lease.recompute();
            result
        }
        .boxed_local()
    }
    pub fn load_next(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let lease = self.clone();
        async move {
            let result =
                fetch_refs(&lease.service, &lease.key, &lease.owner.core, true, false).await;
            lease.recompute();
            result
        }
        .boxed_local()
    }
    fn allowed(&self) -> Result<(), ClientError> {
        self.recompute();
        if self.snapshot().pending {
            return Err(ClientError::Cancelled);
        }
        if self.snapshot().can_change {
            Ok(())
        } else {
            Err(ClientError::PermissionDenied {
                method: "vcs.switchRef".into(),
            })
        }
    }
    pub fn select_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.retain_selection(self.select_ref_inner(name))
    }
    fn retain_selection(
        &self,
        selection: LocalBoxFuture<'static, Result<(), ClientError>>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let spawn = self.service.spawn.clone();
        async move {
            let (send, receive) = futures::channel::oneshot::channel();
            spawn(
                async move {
                    let _ = send.send(selection.await);
                }
                .boxed_local(),
            );
            receive.await.unwrap_or(Err(ClientError::Cancelled))
        }
        .boxed_local()
    }
    fn select_ref_inner(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let lease = self.clone();
        async move {
            lease.allowed()?;
            let context = lease
                .service
                .workspace_context(&lease.key, None)
                .ok_or(ClientError::Cancelled)?;
            let reference = lease
                .snapshot()
                .refs
                .iter()
                .find(|r| r.name.0 == name)
                .cloned()
                .ok_or(ClientError::Cancelled)?;
            if lease.snapshot().selecting_worktree_base {
                return lease
                    .service
                    .set_branch_context(&lease.key, Some(name), None, false)
                    .await;
            }
            let target = branch::branch_selection_target(
                &context.project.workspace_root.0,
                lease.owner.core.checkout_root.borrow().as_deref(),
                context.worktree_path.as_deref(),
                &reference,
            );
            if branch::worktree_change_blocked(
                context
                    .thread
                    .as_ref()
                    .is_some_and(|t| t.active_run_id.is_some()),
                context.worktree_path.as_deref(),
                target.next_worktree_path.as_deref(),
            ) {
                return Err(ClientError::Rejected {
                    message: "Stop the current turn to switch worktrees.".into(),
                    detail: None,
                });
            }
            if target.reuse_existing_worktree && !target.stays_in_active_checkout {
                return lease
                    .service
                    .set_branch_context(&lease.key, Some(name), target.next_worktree_path, false)
                    .await;
            }
            let selected = if reference.is_remote == Some(Some(true)) {
                branch::local_branch_from_remote(&name).to_owned()
            } else {
                name.clone()
            };
            let pending = PendingSelection::begin(lease.owner.core.clone(), selected.clone());
            lease.recompute();
            let result = async {
                let endpoint = lease.service.session(&lease.key.environment)?;
                let result = lease
                    .service
                    .state
                    .source
                    .switch_ref(
                        lease.key.environment.clone(),
                        &endpoint,
                        &lease.service.platform,
                        VcsSwitchRefInput {
                            cwd: TrimmedNonEmptyString(target.checkout_cwd),
                            ref_name: TrimmedNonEmptyString(name),
                        },
                    )
                    .await
                    .map_err(RpcCallError::into_client_error)?;
                let branch = if reference.is_remote == Some(Some(true)) {
                    result.ref_name.map(|n| n.0).unwrap_or(selected)
                } else {
                    selected
                };
                lease
                    .service
                    .set_branch_context(&lease.key, Some(branch), target.next_worktree_path, false)
                    .await
            }
            .await;
            drop(pending);
            let mut state = (*lease.snapshot()).clone();
            state.pending = false;
            state.error = result.as_ref().err().map(ToString::to_string);
            publish(&lease.owner.core, state);
            lease.recompute();
            result
        }
        .boxed_local()
    }
    pub fn create_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.retain_selection(self.create_ref_inner(name))
    }
    fn create_ref_inner(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let lease = self.clone();
        async move {
            lease.allowed()?;
            let name = branch::sanitize_new_ref_name(&name);
            if name.is_empty() {
                return Err(ClientError::Cancelled);
            }
            let c = lease
                .service
                .workspace_context(&lease.key, None)
                .ok_or(ClientError::Cancelled)?;
            let endpoint = lease.service.session(&lease.key.environment)?;
            let pending = PendingSelection::begin(lease.owner.core.clone(), name.clone());
            let result = lease
                .service
                .state
                .source
                .create_ref(
                    lease.key.environment.clone(),
                    &endpoint,
                    &lease.service.platform,
                    VcsCreateRefInput {
                        cwd: TrimmedNonEmptyString(
                            c.worktree_path
                                .clone()
                                .unwrap_or_else(|| c.project.workspace_root.0.clone()),
                        ),
                        ref_name: TrimmedNonEmptyString(name),
                        switch_ref: Some(Some(true)),
                    },
                )
                .await
                .map_err(RpcCallError::into_client_error)?;
            let result = lease
                .service
                .set_branch_context(&lease.key, Some(result.ref_name.0), c.worktree_path, false)
                .await;
            drop(pending);
            lease.recompute();
            result
        }
        .boxed_local()
    }
    pub fn set_env_mode(&self, mode: EnvMode) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let lease = self.clone();
        async move {
            if lease.snapshot().env_locked {
                return Err(ClientError::Cancelled);
            }
            let context = lease.service.workspace_context(&lease.key, None).ok_or(ClientError::Cancelled)?;
            if context.thread.is_some() {
                lease.service.state.context_overrides.borrow_mut().insert(lease.key.clone(), (mode, context.start_from_origin));
                lease.service.state.context_changes.notify();
                return Ok(());
            }
            let start = super::details::environment_config(lease.service.runtime.store(), &lease.key.environment).is_some_and(|c|mode == EnvMode::Worktree && c.settings.project_settings_overrides.get(&context.project.id).and_then(|p|p.new_worktrees_start_from_origin).unwrap_or(c.settings.new_worktrees_start_from_origin));
            let mut patch = json!({"envMode":if mode==EnvMode::Worktree{"worktree"}else{"local"},"startFromOrigin":start});
            if mode == EnvMode::Worktree && context.worktree_path.is_some() { patch["worktreePath"] = serde_json::Value::Null; }
            lease
                .service
                .update_draft_context(
                    &lease.key,
                    None,
                    patch,
                )
                .await
        }
        .boxed_local()
    }
    pub fn set_start_from_origin(
        &self,
        value: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let lease = self.clone();
        async move {
            let context = lease
                .service
                .workspace_context(&lease.key, None)
                .ok_or(ClientError::Cancelled)?;
            if context.thread.is_some() {
                if lease.snapshot().env_locked {
                    return Err(ClientError::Cancelled);
                }
                lease
                    .service
                    .state
                    .context_overrides
                    .borrow_mut()
                    .insert(lease.key.clone(), (context.env_mode, value));
                lease.service.state.context_changes.notify();
                return Ok(());
            }
            lease
                .service
                .update_draft_context(&lease.key, None, json!({"startFromOrigin":value}))
                .await
        }
        .boxed_local()
    }
    pub fn use_previous_worktree(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let lease = self.clone();
        async move{if lease.snapshot().env_locked{return Err(ClientError::Cancelled);}let previous=lease.snapshot().previous_worktree.clone().ok_or(ClientError::Cancelled)?;lease.service.update_draft_context(&lease.key,None,json!({"branch":previous.branch,"worktreePath":previous.path,"envMode":"worktree"})).await}.boxed_local()
    }
}
