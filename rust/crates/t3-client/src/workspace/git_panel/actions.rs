use super::*;
use crate::{
    rpc::{RpcCallError, RpcEndpoint},
    workspace::{git_actions, project_ops::actions::VcsActionOperation},
};
use futures::{
    future::LocalBoxFuture,
    stream::{self, SelectAll},
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, review::*, rpc::*, source_control::*};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitActionsScope {
    pub thread: ThreadKey,
    pub cwd: String,
    pub draft_id: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitActionsWatch {
    pub show_actions: bool,
    pub show_changes: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitActionRequest {
    pub action: GitStackedAction,
    pub commit_message: Option<String>,
    pub feature_branch: bool,
    pub file_paths: Option<Vec<String>>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitActionsSnapshot {
    pub status: Option<Rc<VcsStatusResult>>,
    pub status_error: Option<String>,
    pub branch_preview: Option<git_actions::ChangesTotals>,
    pub action: Rc<crate::workspace::project_ops::actions::VcsActionState>,
    pub can_write: bool,
    pub can_operate: bool,
    pub is_local_draft: bool,
    pub has_server_thread: bool,
    pub latest_run_is_null: bool,
    pub selecting_worktree_base: bool,
    pub project_cloning: bool,
    pub thread_branch: Option<String>,
    pub discovery: Option<Rc<SourceControlDiscoveryResult>>,
    pub offered_providers: Option<BTreeMap<String, bool>>,
    pub publish_providers: Vec<providers::PublishProvider>,
    pub presentation: Option<providers::SourceControlPresentation>,
    pub announce_status_changes: bool,
}

struct Observation<P: Platform + Clone + 'static> {
    first: bool,
    changes: SelectAll<LocalBoxStream<'static, ()>>,
    checkout: Rc<Checkout>,
    discovery: Option<crate::composition::QueryLease<SourceControlDiscoveryResult>>,
    clones: crate::composition::QueryLease<Vec<t3_contracts::project_clone::ProjectCloneSnapshot>>,
    preview: Option<crate::composition::QueryLease<ReviewDiffPreviewResult>>,
    preview_changes:
        Option<crate::services::resources::ResourceSubscription<ReviewDiffPreviewResult>>,
    revision: String,
    service: GitPanel<P>,
    scope: GitActionsScope,
    target: CheckoutKey,
    watch: GitActionsWatch,
}

fn preview_totals(preview: &ReviewDiffPreviewResult) -> Option<git_actions::ChangesTotals> {
    let source = preview
        .sources
        .iter()
        .find(|s| s.kind == ReviewDiffPreviewSourceKind::BranchRange)?;
    let count = |n: &t3_contracts::vcs::EncodedNumber| {
        serde_json::to_value(n)
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as u64
    };
    Some(git_actions::ChangesTotals {
        insertions: source
            .files
            .as_ref()
            .into_iter()
            .flatten()
            .map(|f| count(&f.additions))
            .sum(),
        deletions: source
            .files
            .as_ref()
            .into_iter()
            .flatten()
            .map(|f| count(&f.deletions))
            .sum(),
        base_ref: source.base_ref.as_ref().map(|b| b.0.clone()),
    })
}

impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub fn watch_actions(
        &self,
        scope: &GitActionsScope,
        watch: GitActionsWatch,
    ) -> LocalBoxStream<'static, GitActionsSnapshot> {
        let target = CheckoutKey {
            environment: scope.thread.environment.clone(),
            cwd: scope.cwd.clone(),
        };
        let checkout = self.checkout(&target);
        let discovery = watch.show_actions.then(|| {
            self.runtime.query::<WsServerDiscoverSourceControlRpc>(
                &target.environment,
                Empty {},
                ResourcePolicy::default(),
            )
        });
        let mut changes = SelectAll::<LocalBoxStream<'static, ()>>::new();
        changes.push(
            checkout
                .status
                .observe(self.platform.monotonic_ms())
                .map(|_| ())
                .boxed_local(),
        );
        changes.push(
            self.state
                .actions
                .observe(&target)
                .map(|_| ())
                .boxed_local(),
        );
        changes.push(self.changes());
        changes.push(self.runtime.composer_drafts().changes());
        changes.push(self.runtime.composer_preferences().changes());
        if let Some(query) = discovery.as_ref() {
            changes.push(
                query
                    .resource()
                    .observe(self.platform.monotonic_ms())
                    .map(|_| ())
                    .boxed_local(),
            );
        }
        let clones = self.runtime.query::<WsSubscribeProjectClonesRpc>(
            &target.environment,
            t3_contracts::project_clone::ProjectCloneSubscribeInput {},
            ResourcePolicy::default(),
        );
        changes.push(
            clones
                .resource()
                .observe(self.platform.monotonic_ms())
                .map(|_| ())
                .boxed_local(),
        );
        let state = Observation {
            first: true,
            changes,
            checkout,
            discovery,
            clones,
            preview: None,
            preview_changes: None,
            revision: String::new(),
            service: self.clone(),
            scope: scope.clone(),
            target,
            watch,
        };
        stream::unfold(state, |mut o| async move {
            if !o.first {
                if let Some(preview) = &mut o.preview_changes {
                    futures::select! { _ = o.changes.next().fuse() => (), _ = preview.next().fuse() => () }
                } else { o.changes.next().await?; }
            }
            o.first = false;
            let service = &o.service; let scope = &o.scope; let target = &o.target;
            let status = o.checkout.status.snapshot();
            let thread = service.thread(&scope.thread);
            let context = service.workspace_context(&scope.thread, scope.draft_id.as_deref());
            let previous_sync = service.state.synced_branches.borrow().get(&scope.thread).cloned();
            if let Some((_, expected)) = previous_sync
                && context.as_ref().and_then(|c|c.branch.as_ref()) != expected.as_ref() {
                service.state.synced_branches.borrow_mut().remove(&scope.thread);
            }
            let project_cloning = context.as_ref().is_some_and(|c| o.clones.resource().snapshot().value.is_some_and(|clones| clones.iter().any(|clone| clone.project_id == c.project.id && clone.phase != t3_contracts::project_clone::ProjectClonePhase::Done)));
            let selecting_worktree_base = context.as_ref().is_some_and(|c| c.env_mode == super::super::branch::EnvMode::Worktree && c.worktree_path.is_none() && c.thread.as_ref().is_none_or(|t|t.active_provider_thread_id.is_none()));
            let endpoint = service.session(&target.environment).ok();
            let pull = VcsPullInput { cwd: TrimmedNonEmptyString(scope.cwd.clone()) };
            let can_write = endpoint.as_ref().is_some_and(|e| e.available::<WsVcsPullRpc>(&pull));
            let metadata = serde_json::from_value::<t3_contracts::orchestration_v2::OrchestrationV2Command>(serde_json::json!({"type":"thread.metadata.update","commandId":"availability","threadId":scope.thread.thread,"branch":null}));
            let can_operate = metadata.ok().is_some_and(|m| endpoint.as_ref().is_some_and(|e| e.available::<WsOrchestrationV2DispatchCommandRpc>(&m)));
            if o.watch.show_changes && let Some(status) = status.value.as_ref().filter(|s| s.is_repo) {
                let base = service.state.diff.borrow().branch_base.get(&scope.thread).cloned().flatten();
                let ignore = service.state.diff.borrow().ignore_whitespace.unwrap_or(service.runtime.composer_preferences().snapshot().diff_ignore_whitespace);
                let input = ReviewDiffPreviewInput { cwd: TrimmedNonEmptyString(scope.cwd.clone()), base_ref: base.map(|b| Some(TrimmedNonEmptyString(b))), commit_sha: None, ignore_whitespace: Some(ignore), source_kind: Some(ReviewDiffPreviewInputSourceKind::BranchRange), tracked_only: None, metadata_only: Some(true), file: None };
                if endpoint.as_ref().is_some_and(|e| e.available::<WsReviewGetDiffPreviewRpc>(&input)) {
                    let mutation = service.runtime.store().shared_thread(&scope.thread).and_then(|p| {
                        let checkpoint = p.checkpoints.iter().rev().find(|c| c.app_run_ordinal.is_some() && c.run_id.is_some());
                        crate::thread_views::threads::detail::derive_workspace_mutation_id(&p.visible_turn_items, checkpoint.map(|c|c.captured_at.0.as_str())).ok().flatten()
                    });
                    let revision = serde_json::to_string(&(mutation, status.ref_name.clone(), &status.working_tree, &status.branch_changes, &input)).unwrap_or_default();
                    if revision != o.revision {
                        let query = service.runtime.query::<WsReviewGetDiffPreviewRpc>(&target.environment, input, ResourcePolicy::default());
                        if !o.revision.is_empty() { query.resource().invalidate(); }
                        o.preview_changes = Some(query.resource().observe(service.platform.monotonic_ms()));
                        o.preview = Some(query); o.revision = revision;
                    }
                } else { o.preview = None; o.preview_changes = None; }
            }
            let action = service.state.actions.state(Some(target));
            if let Some(context) = &context && context.thread.is_none() && !selecting_worktree_base && !project_cloning && !action.is_running
                && let Some(branch) = git_actions::live_thread_branch_update(context.branch.as_deref(), status.value.as_deref()) {
                let _ = service.sync_branch(scope, branch, context.branch.clone(), false).await;
            }
            let value = GitActionsSnapshot {
                status: status.value.clone(), status_error: status.error,
                branch_preview: o.preview.as_ref().and_then(|p| p.resource().snapshot().value).and_then(|p| preview_totals(&p)),
                action, can_write, can_operate, project_cloning, selecting_worktree_base,
                is_local_draft: context.as_ref().is_some_and(|c|c.draft_id.is_some()) && thread.is_none(), has_server_thread: thread.is_some(),
                latest_run_is_null: thread.as_ref().is_some_and(|t| t.latest_run_id.is_none()),
                thread_branch: context.as_ref().and_then(|c| c.branch.clone()),
                discovery: o.discovery.as_ref().and_then(|q| q.resource().snapshot().value),
                offered_providers: service.runtime.store().snapshot().environments.get(&target.environment).and_then(|e| e.config.as_ref()).map(|c| c.settings.offered_source_control_providers.clone()),
                publish_providers: providers::publish_providers(),
                presentation: Some(providers::presentation(status.value.as_ref().and_then(|s| s.source_control_provider.as_ref().and_then(Option::as_ref)).map(|p| p.kind.0.as_str()))),
                announce_status_changes: true,
            };
            Some((value, o))
        }).boxed_local()
    }
    pub fn refresh_actions(
        &self,
        scope: &GitActionsScope,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            service
                .state
                .source
                .refresh_status(
                    scope.thread.environment,
                    &endpoint,
                    &service.platform,
                    WsVcsRefreshStatusPayload {
                        cwd: TrimmedNonEmptyString(scope.cwd),
                        local_only: None,
                    },
                )
                .await
                .map_err(RpcCallError::into_client_error)?;
            Ok(())
        }
        .boxed_local()
    }
    pub fn run_action(
        &self,
        scope: &GitActionsScope,
        request: GitActionRequest,
    ) -> LocalBoxFuture<'static, Result<GitRunStackedActionResult, ClientError>> {
        let service = self.clone();
        let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let thread = service.thread(&scope.thread);
            if thread.is_none() && service.workspace_context(&scope.thread, scope.draft_id.as_deref()).is_none() { return Err(ClientError::Cancelled); }
            if request.feature_branch && thread.is_some() {
                let metadata = serde_json::from_value(serde_json::json!({"type":"thread.metadata.update","commandId":"availability","threadId":scope.thread.thread,"branch":null}))?;
                if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&metadata) { return Err(ClientError::PermissionDenied { method: "orchestration.dispatchCommand".into() }); }
            }
            let input = GitRunStackedActionInput {
                action_id: TrimmedNonEmptyString(crate::operations::allocate_command_id(&service.platform, None)?.0),
                cwd: TrimmedNonEmptyString(scope.cwd.clone()), action: request.action,
                commit_message: request.commit_message.map(|m| Some(TrimmedNonEmptyString(m))),
                feature_branch: request.feature_branch.then_some(Some(true)),
                file_paths: request.file_paths.map(Some),
                thread_id: thread.as_ref().map(|t| Some(t.id.clone())),
                project_id: if thread.is_none() { service.workspace_context(&scope.thread, scope.draft_id.as_deref()).map(|c| Some(c.project.id.clone())) } else { None },
            };
            let target = CheckoutKey { environment: scope.thread.environment.clone(), cwd: scope.cwd.clone() };
            let result = service.state.actions.run_stacked_action(&service.state.source, &target, &endpoint, &service.platform, input, |_| Ok(())).await.map_err(|error| match error {
                crate::workspace::project_ops::actions::VcsActionError::Client(e) => e,
                e => ClientError::Rejected { message: e.to_string(), detail: None },
            })?;
            if let Some(branch) = git_actions::thread_branch_update(&result) {
                service.sync_branch(&scope, Some(branch), thread.and_then(|t| t.branch.as_ref().map(|b| b.0.clone())), true).await?;
            }
            Ok(result)
        }.boxed_local()
    }
    pub fn sync_branch(
        &self,
        scope: &GitActionsScope,
        branch: Option<String>,
        expected: Option<String>,
        _manual: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        let scope = scope.clone();
        async move {
            if service.thread(&scope.thread).is_none() {
                let context = service.workspace_context(&scope.thread, scope.draft_id.as_deref()).ok_or(ClientError::Cancelled)?;
                if context.branch == branch { return Ok(()); }
                return service.update_draft_context(&scope.thread, scope.draft_id.as_deref(), serde_json::json!({"branch":branch,"worktreePath":context.worktree_path,"environmentSelection":if _manual || context.manual || context.branch.is_some() {"manual"} else {"auto"}})).await;
            }
            if service.thread(&scope.thread).is_some_and(|t| t.branch.as_ref().map(|b| &b.0) == branch.as_ref()) { return Ok(()); }
            if service.state.synced_branches.borrow().get(&scope.thread) == Some(&(branch.clone(), expected.clone())) { return Ok(()); }
            let command = serde_json::from_value(serde_json::json!({"type":"thread.metadata.update","commandId":crate::operations::allocate_command_id(&service.platform, None)?,"threadId":scope.thread.thread,"branch":branch,"expectedBranch":expected}))?;
            service.session(&scope.thread.environment)?.call::<WsOrchestrationV2DispatchCommandRpc>(command).await?;
            service.state.synced_branches.borrow_mut().insert(scope.thread, (branch, expected));
            Ok(())
        }.boxed_local()
    }
    pub fn pull(
        &self,
        scope: &GitActionsScope,
    ) -> LocalBoxFuture<'static, Result<VcsPullResult, ClientError>> {
        let service = self.clone();
        let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let target = CheckoutKey {
                environment: scope.thread.environment.clone(),
                cwd: scope.cwd.clone(),
            };
            service
                .state
                .actions
                .track(
                    &target,
                    VcsActionOperation::Pull,
                    "Pulling latest changes",
                    &service.platform,
                    service.state.source.pull(
                        scope.thread.environment,
                        &endpoint,
                        &service.platform,
                        VcsPullInput {
                            cwd: TrimmedNonEmptyString(scope.cwd),
                        },
                    ),
                )
                .await
                .map_err(RpcCallError::into_client_error)
        }
        .boxed_local()
    }
    pub fn initialize(
        &self,
        scope: &GitActionsScope,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let target = CheckoutKey {
                environment: scope.thread.environment.clone(),
                cwd: scope.cwd.clone(),
            };
            service
                .state
                .actions
                .track(
                    &target,
                    VcsActionOperation::Init,
                    "Initializing Git",
                    &service.platform,
                    service.state.source.init(
                        scope.thread.environment,
                        &endpoint,
                        &service.platform,
                        VcsInitInput {
                            cwd: TrimmedNonEmptyString(scope.cwd),
                            kind: None,
                        },
                    ),
                )
                .await
                .map_err(RpcCallError::into_client_error)?;
            Ok(())
        }
        .boxed_local()
    }
    pub fn publish(
        &self,
        scope: &GitActionsScope,
        input: SourceControlPublishRepositoryInput,
    ) -> LocalBoxFuture<'static, Result<SourceControlPublishRepositoryResult, ClientError>> {
        let service = self.clone();
        let scope = scope.clone();
        async move {
            if input.cwd.0 != scope.cwd {
                return Err(ClientError::EnvironmentMismatch);
            }
            let endpoint = service.session(&scope.thread.environment)?;
            let target = CheckoutKey {
                environment: scope.thread.environment.clone(),
                cwd: scope.cwd,
            };
            service
                .state
                .actions
                .track(
                    &target,
                    VcsActionOperation::PublishRepository,
                    "Publishing repository",
                    &service.platform,
                    service.state.source.publish_repository(
                        scope.thread.environment,
                        &endpoint,
                        &service.platform,
                        input,
                    ),
                )
                .await
                .map_err(RpcCallError::into_client_error)
        }
        .boxed_local()
    }
    pub fn open_file(
        &self,
        scope: &GitActionsScope,
        root: String,
        path: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let editors = service
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&scope.thread.environment)
                .and_then(|e| e.config.clone())
                .map(|c| c.available_editors.0.clone())
                .unwrap_or_default();
            let absolute = crate::services::filesystem::workspace_file_path(&root, &path);
            crate::workspace::project_ops::editor::open_in_preferred_editor(
                Some(&scope.thread.environment),
                &editors,
                Some(&root),
                &absolute,
                &endpoint,
                &service.platform,
                |_| true,
            )
            .await
            .map_err(|e| match e {
                crate::workspace::project_ops::editor::PreferredEditorError::Client(e) => e,
                crate::workspace::project_ops::editor::PreferredEditorError::Rpc(e) => {
                    e.into_client_error()
                }
                e => ClientError::Rejected {
                    message: e.to_string(),
                    detail: None,
                },
            })?;
            Ok(())
        }
        .boxed_local()
    }
}
