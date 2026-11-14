use super::*;
use crate::{rpc::{RpcEndpoint, RpcCallError}, workspace::{git_actions, project_ops::actions::VcsActionOperation}};
use futures::{future::LocalBoxFuture, stream::{self, SelectAll}};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, rpc::*, source_control::*};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitActionsScope { pub thread: ThreadKey, pub cwd: String, pub draft_id: Option<String> }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitActionsWatch { pub show_actions: bool, pub show_changes: bool }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitActionRequest {
    pub action: GitStackedAction, pub commit_message: Option<String>, pub feature_branch: bool, pub file_paths: Option<Vec<String>>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitActionsSnapshot {
    pub status: Option<Rc<VcsStatusResult>>, pub status_error: Option<String>,
    pub branch_preview: Option<git_actions::ChangesTotals>, pub action: Rc<crate::workspace::project_ops::actions::VcsActionState>,
    pub can_write: bool, pub can_operate: bool, pub is_local_draft: bool, pub has_server_thread: bool,
    pub latest_run_is_null: bool, pub selecting_worktree_base: bool, pub project_cloning: bool,
    pub thread_branch: Option<String>, pub discovery: Option<Rc<SourceControlDiscoveryResult>>,
    pub offered_providers: Option<BTreeMap<String, bool>>, pub publish_providers: Vec<providers::PublishProvider>,
    pub presentation: Option<providers::SourceControlPresentation>, pub announce_status_changes: bool,
}

impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub fn watch_actions(&self, scope: &GitActionsScope, watch: GitActionsWatch) -> LocalBoxStream<'static, GitActionsSnapshot> {
        let target = CheckoutKey { environment: scope.thread.environment.clone(), cwd: scope.cwd.clone() };
        let checkout = self.checkout(&target);
        let discovery = watch.show_actions.then(|| self.runtime.query::<WsServerDiscoverSourceControlRpc>(&target.environment, Empty {}, ResourcePolicy::default()));
        let mut changes = SelectAll::<LocalBoxStream<'static, ()>>::new();
        changes.push(checkout.status.observe(self.platform.monotonic_ms()).map(|_| ()).boxed_local());
        changes.push(self.state.actions.observe(&target).map(|_| ()).boxed_local());
        changes.push(self.changes());
        if let Some(query) = discovery.as_ref() { changes.push(query.resource().observe(self.platform.monotonic_ms()).map(|_| ()).boxed_local()); }
        let service = self.clone();
        let scope = scope.clone();
        stream::unfold((true, changes, checkout, discovery, service, scope, target), |(first, mut changes, checkout, discovery, service, scope, target)| async move {
            if !first { changes.next().await?; }
            let status = checkout.status.snapshot();
            let thread = service.thread(&scope.thread);
            let endpoint = service.session(&target.environment).ok();
            let pull = VcsPullInput { cwd: TrimmedNonEmptyString(scope.cwd.clone()) };
            let can_write = endpoint.as_ref().is_some_and(|e| e.available::<WsVcsPullRpc>(&pull));
            let metadata = serde_json::from_value::<t3_contracts::orchestration_v2::OrchestrationV2Command>(serde_json::json!({"type":"thread.metadata.update","commandId":"availability","threadId":scope.thread.thread,"branch":null}));
            let can_operate = metadata.ok().is_some_and(|m| endpoint.as_ref().is_some_and(|e| e.available::<WsOrchestrationV2DispatchCommandRpc>(&m)));
            let value = GitActionsSnapshot {
                status: status.value.clone(), status_error: status.error,
                action: service.state.actions.state(Some(&target)), can_write, can_operate,
                is_local_draft: scope.draft_id.is_some() && thread.is_none(), has_server_thread: thread.is_some(),
                latest_run_is_null: thread.as_ref().is_some_and(|t| t.latest_run_id.is_none()),
                thread_branch: thread.as_ref().and_then(|t| t.branch.as_ref().map(|b| b.0.clone())),
                discovery: discovery.as_ref().and_then(|q| q.resource().snapshot().value),
                publish_providers: providers::publish_providers(),
                presentation: Some(providers::presentation(status.value.as_ref().and_then(|s| s.source_control_provider.as_ref().and_then(Option::as_ref)).map(|p| p.kind.0.as_str()))),
                ..GitActionsSnapshot::default()
            };
            Some((value, (false, changes, checkout, discovery, service, scope, target)))
        }).boxed_local()
    }
    pub fn refresh_actions(&self, scope: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone(); let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            service.state.source.refresh_status(scope.thread.environment, &endpoint, &service.platform, WsVcsRefreshStatusPayload { cwd: TrimmedNonEmptyString(scope.cwd), local_only: None }).await.map_err(RpcCallError::into_client_error)?;
            Ok(())
        }.boxed_local()
    }
    pub fn run_action(&self, scope: &GitActionsScope, request: GitActionRequest) -> LocalBoxFuture<'static, Result<GitRunStackedActionResult, ClientError>> {
        let service = self.clone(); let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let thread = service.thread(&scope.thread);
            let input = GitRunStackedActionInput {
                action_id: TrimmedNonEmptyString(crate::operations::allocate_command_id(&service.platform, None)?.0),
                cwd: TrimmedNonEmptyString(scope.cwd.clone()), action: request.action,
                commit_message: request.commit_message.map(|m| Some(TrimmedNonEmptyString(m))),
                feature_branch: request.feature_branch.then_some(Some(true)),
                file_paths: request.file_paths.map(Some),
                thread_id: thread.as_ref().map(|t| Some(t.id.clone())),
                project_id: if thread.is_none() { return Err(ClientError::Unsupported("draft context is not loaded")); } else { None },
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
    pub fn sync_branch(&self, scope: &GitActionsScope, branch: Option<String>, expected: Option<String>, _manual: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone(); let scope = scope.clone();
        async move {
            if service.thread(&scope.thread).is_none() { return Err(ClientError::Unsupported("draft context is not loaded")); }
            let command = serde_json::from_value(serde_json::json!({"type":"thread.metadata.update","commandId":crate::operations::allocate_command_id(&service.platform, None)?,"threadId":scope.thread.thread,"branch":branch,"expectedBranch":expected}))?;
            service.session(&scope.thread.environment)?.call::<WsOrchestrationV2DispatchCommandRpc>(command).await?;
            Ok(())
        }.boxed_local()
    }
    pub fn pull(&self, scope: &GitActionsScope) -> LocalBoxFuture<'static, Result<VcsPullResult, ClientError>> {
        let service = self.clone(); let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let target = CheckoutKey { environment: scope.thread.environment.clone(), cwd: scope.cwd.clone() };
            service.state.actions.track(&target, VcsActionOperation::Pull, "Pulling latest changes", &service.platform,
                service.state.source.pull(scope.thread.environment, &endpoint, &service.platform, VcsPullInput { cwd: TrimmedNonEmptyString(scope.cwd) })).await.map_err(RpcCallError::into_client_error)
        }.boxed_local()
    }
    pub fn initialize(&self, scope: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone(); let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let target = CheckoutKey { environment: scope.thread.environment.clone(), cwd: scope.cwd.clone() };
            service.state.actions.track(&target, VcsActionOperation::Init, "Initializing Git", &service.platform,
                service.state.source.init(scope.thread.environment, &endpoint, &service.platform, VcsInitInput { cwd: TrimmedNonEmptyString(scope.cwd), kind: None })).await.map_err(RpcCallError::into_client_error)?;
            Ok(())
        }.boxed_local()
    }
    pub fn publish(&self, scope: &GitActionsScope, input: SourceControlPublishRepositoryInput) -> LocalBoxFuture<'static, Result<SourceControlPublishRepositoryResult, ClientError>> {
        let service = self.clone(); let scope = scope.clone();
        async move {
            if input.cwd.0 != scope.cwd { return Err(ClientError::EnvironmentMismatch); }
            let endpoint = service.session(&scope.thread.environment)?;
            let target = CheckoutKey { environment: scope.thread.environment.clone(), cwd: scope.cwd };
            service.state.actions.track(&target, VcsActionOperation::PublishRepository, "Publishing repository", &service.platform,
                service.state.source.publish_repository(scope.thread.environment, &endpoint, &service.platform, input)).await.map_err(RpcCallError::into_client_error)
        }.boxed_local()
    }
    pub fn open_file(&self, scope: &GitActionsScope, root: String, path: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone(); let scope = scope.clone();
        async move {
            let endpoint = service.session(&scope.thread.environment)?;
            let editors = service.runtime.store().snapshot().environments.get(&scope.thread.environment).and_then(|e| e.config.clone()).map(|c| c.available_editors.0.clone()).unwrap_or_default();
            crate::workspace::project_ops::editor::open_in_preferred_editor(Some(&scope.thread.environment), &editors, Some(&root), &path, &endpoint, &service.platform, |_| true).await.map_err(|e| ClientError::Rejected { message: e.to_string(), detail: None })?;
            Ok(())
        }.boxed_local()
    }
}
