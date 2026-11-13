use super::super::vcs::{CheckoutKey, VcsCommandScheduler, VcsRefsCache};
use crate::{
    ClientError,
    platform::{Platform, Storage},
    rpc::{RpcCallError, RpcEndpoint},
    services::resources::{Resource, ResourcePolicy, ResourceRegistry},
    types::{EnvironmentId, Generation},
};
use std::{cell::RefCell, rc::Rc};
use t3_contracts::rpc::*;

pub fn check_destination(
    endpoint: &impl RpcEndpoint,
    environment: &EnvironmentId,
) -> Result<(), RpcCallError> {
    if endpoint.destination().as_ref() != Some(environment) {
        return Err(RpcCallError::NotSent(ClientError::EnvironmentMismatch));
    }
    Ok(())
}
#[derive(Clone, Default)]
pub struct SourceControl {
    pub scheduler: VcsCommandScheduler,
    pub refs: VcsRefsCache,
    pub queries: Rc<RefCell<ResourceRegistry>>,
    clones: VcsCommandScheduler,
}
pub(crate) struct RefsInvalidationGuard {
    pub refs: VcsRefsCache,
    pub environment: EnvironmentId,
    pub settled: bool,
}
impl Drop for RefsInvalidationGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.refs.invalidate(&self.environment, Some(false));
        }
    }
}
impl SourceControl {
    pub fn begin_session(&self, environment: &EnvironmentId, generation: Generation) {
        self.queries
            .borrow_mut()
            .replace_session(environment, generation);
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        self.queries.borrow_mut().remove_environment(environment);
        self.refs.remove_environment(environment);
    }
    pub fn query<M: t3_contracts::rpc::RpcMethod>(
        &self,
        environment: EnvironmentId,
        input: &M::Payload,
    ) -> Result<Resource<M::Success>, ClientError>
    where
        M::Success: 'static,
    {
        self.queries
            .borrow_mut()
            .query::<M>(environment, input, ResourcePolicy::default())
    }
    pub async fn refresh_query<M: t3_contracts::rpc::RpcMethod>(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: M::Payload,
    ) -> Result<(), ClientError>
    where
        M::Success: PartialEq + 'static,
    {
        check_destination(endpoint, &environment).map_err(RpcCallError::into_client_error)?;
        self.query::<M>(environment, &input)?
            .execute::<M, _, _>(endpoint, platform, input)
            .await
    }
    pub async fn command<M: t3_contracts::rpc::RpcMethod>(
        &self,
        environment: &EnvironmentId,
        endpoint: &impl RpcEndpoint,
        input: M::Payload,
    ) -> Result<M::Success, RpcCallError>
    where
        M::Success: 'static,
    {
        check_destination(endpoint, environment)?;
        endpoint.command::<M>(input).await
    }
    pub async fn clone_command<M: t3_contracts::rpc::RpcMethod>(
        &self,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        input: M::Payload,
    ) -> Result<M::Success, RpcCallError>
    where
        M::Success: 'static,
    {
        check_destination(endpoint, &environment)?;
        let _lease = self
            .clones
            .acquire(CheckoutKey {
                environment,
                cwd: String::new(),
            })
            .await;
        endpoint.command::<M>(input).await
    }
    pub async fn vcs_command<M: t3_contracts::rpc::RpcMethod>(
        &self,
        target: CheckoutKey,
        endpoint: &impl RpcEndpoint,
        storage: &impl Storage,
        input: M::Payload,
        serial: bool,
        invalidate: bool,
    ) -> Result<M::Success, RpcCallError>
    where
        M::Success: 'static,
    {
        check_destination(endpoint, &target.environment)?;
        let _lease = if serial {
            Some(self.scheduler.acquire(target.clone()).await)
        } else {
            None
        };
        let mut guard = RefsInvalidationGuard {
            refs: self.refs.clone(),
            environment: target.environment.clone(),
            settled: !invalidate,
        };
        let result = endpoint.command::<M>(input).await;
        if invalidate {
            self.refs
                .invalidate_cached(storage, &target.environment)
                .await;
            guard.settled = true;
        }
        result
    }
}

macro_rules! queries {
    ($($name:ident: $method:ident),* $(,)?) => { impl SourceControl { $(
        pub fn $name(&self, environment: EnvironmentId, input: &<$method as t3_contracts::rpc::RpcMethod>::Payload) -> Result<Resource<<$method as t3_contracts::rpc::RpcMethod>::Success>, ClientError> {
            self.query::<$method>(environment, input)
        }
    )* } };
}
queries! {
    discovery_query: WsServerDiscoverSourceControlRpc,
    app_access_query: WsSourceControlAppAccessRpc,
    repository_query: WsSourceControlLookupRepositoryRpc,
    pull_request_resolution: WsGitResolvePullRequestRpc,
}

macro_rules! plain_commands {
    ($($name:ident: $method:ident),* $(,)?) => { impl SourceControl { $(
        pub async fn $name(&self, environment: &EnvironmentId, endpoint: &impl RpcEndpoint, input: <$method as t3_contracts::rpc::RpcMethod>::Payload) -> Result<<$method as t3_contracts::rpc::RpcMethod>::Success, RpcCallError> {
            self.command::<$method>(environment, endpoint, input).await
        }
    )* } };
}
plain_commands! {
    discovery: WsServerDiscoverSourceControlRpc,
    app_access: WsSourceControlAppAccessRpc,
    check_app_access: WsSourceControlAppAccessRpc,
    check_starred: WsSourceControlStarredRpc,
    star: WsSourceControlStarRpc,
    repository: WsSourceControlLookupRepositoryRpc,
    start_sign_in: WsSourceControlSignInStartRpc,
    wait_for_sign_in: WsSourceControlSignInWaitRpc,
    cancel_sign_in: WsSourceControlSignInCancelRpc,
    resolve_pull_request: WsGitResolvePullRequestRpc,
    read_refs: WsVcsListRefsRpc,
    cancel_worktree_setup: WsWorktreeSetupCancelRpc,
}
macro_rules! clone_commands {
    ($($name:ident: $method:ident),* $(,)?) => { impl SourceControl { $(
        pub async fn $name(&self, environment: EnvironmentId, endpoint: &impl RpcEndpoint, input: <$method as t3_contracts::rpc::RpcMethod>::Payload) -> Result<<$method as t3_contracts::rpc::RpcMethod>::Success, RpcCallError> {
            self.clone_command::<$method>(environment, endpoint, input).await
        }
    )* } };
}
clone_commands! { start_project_clone: WsProjectCloneStartRpc, cancel_project_clone: WsProjectCloneCancelRpc, retry_project_clone: WsProjectCloneRetryRpc }
macro_rules! vcs_commands {
    ($($name:ident: $method:ident, $serial:literal, $invalidate:literal);* $(;)?) => { impl SourceControl { $(
        pub async fn $name(&self, environment: EnvironmentId, endpoint: &impl RpcEndpoint, storage: &impl Storage, input: <$method as t3_contracts::rpc::RpcMethod>::Payload) -> Result<<$method as t3_contracts::rpc::RpcMethod>::Success, RpcCallError> {
            let target = CheckoutKey { environment, cwd: input.cwd.0.clone() };
            self.vcs_command::<$method>(target, endpoint, storage, input, $serial, $invalidate).await
        }
    )* } };
}
vcs_commands! {
    publish_repository: WsSourceControlPublishRepositoryRpc, true, true;
    prepare_pull_request_thread: WsGitPreparePullRequestThreadRpc, true, true;
    refresh_branch: WsVcsRefreshBranchRpc, true, false;
    pull: WsVcsPullRpc, true, true;
    refresh_status: WsVcsRefreshStatusRpc, true, true;
    refresh_local_status_for_picker: WsVcsRefreshStatusRpc, false, true;
    create_worktree: WsVcsCreateWorktreeRpc, true, true;
    remove_worktree: WsVcsRemoveWorktreeRpc, true, true;
    create_ref: WsVcsCreateRefRpc, true, true;
    switch_ref: WsVcsSwitchRefRpc, true, true;
    init: WsVcsInitRpc, true, true;
}
