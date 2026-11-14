//! The VCS status, branch and worktree-setup RPCs (`ws.ts`'s `subscribeVcsBranch` …
//! `vcs.createWorktree` block) over the environment's one `GitAdapter`. Each handler calls one
//! adapter method and maps its error to the method's contract failure.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

#[cfg(all(feature = "git", feature = "local-services"))]
mod live {
    use super::*;
    use crate::{
        adapters::git::{Error, GitAdapter, command_error},
        registry::Failure,
    };
    use std::sync::Arc;
    use t3_contracts::{auth::EnvironmentAuthorizationError, git::GitManagerServiceError};

    fn status_error(method: &str) -> impl Fn(Error) -> Failure<WsSubscribeVcsBranchError> + '_ {
        move |error| {
            Failure::Typed(match error {
                Error::Authorization(e) => {
                    WsSubscribeVcsBranchError::EnvironmentAuthorizationError(e)
                }
                Error::Git(e) => WsSubscribeVcsBranchError::GitManagerServiceError(Box::new(
                    GitManagerServiceError::Command(*e),
                )),
                Error::Vcs(_) => WsSubscribeVcsBranchError::GitManagerServiceError(Box::new(
                    GitManagerServiceError::Command(command_error(method)),
                )),
            })
        }
    }

    fn worktree_error(error: Error) -> Failure<WsVcsPullError> {
        Failure::Typed(match error {
            Error::Authorization(e) => WsVcsPullError::EnvironmentAuthorizationError(e),
            Error::Git(e) => WsVcsPullError::GitCommandError(e),
            Error::Vcs(_) => WsVcsPullError::GitCommandError(Box::new(command_error(
                WsVcsCreateWorktreeRpc::TAG,
            ))),
        })
    }

    fn setup_error(error: Error) -> Failure<EnvironmentAuthorizationError> {
        match error {
            Error::Authorization(e) => Failure::Typed(*e),
            other => Failure::Defect(crate::registry::ServerDefect::Internal {
                message: other.to_string(),
            }),
        }
    }

    pub fn register(registry: &mut HandlerRegistry, adapter: Arc<GitAdapter>) -> crate::Result<()> {
        // Dropping a stream (socket close, unsubscribe) releases its broadcaster lease; at
        // shutdown the broadcaster ends every subscription.
        let a = adapter.clone();
        registry.register_stream::<WsSubscribeVcsBranchRpc, _, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                let mut branches = a
                    .subscribe_vcs_branch(&ctx.session_id, ctx.call.socket_id(), input)
                    .await
                    .map_err(status_error(WsSubscribeVcsBranchRpc::TAG))?;
                Ok(async_stream::stream! {
                    while let Some(branch) = branches.recv().await {
                        yield branch.map_err(status_error(WsSubscribeVcsBranchRpc::TAG));
                    }
                })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsVcsRefreshBranchRpc, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                a.refresh_branch(&ctx.session_id, input)
                    .await
                    .map_err(status_error(WsVcsRefreshBranchRpc::TAG))
            }
        })?;
        let a = adapter.clone();
        registry.register_stream::<WsSubscribeVcsStatusRpc, _, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                let mut statuses = a
                    .subscribe_vcs_status(&ctx.session_id, ctx.call.socket_id(), input)
                    .await
                    .map_err(status_error(WsSubscribeVcsStatusRpc::TAG))?;
                Ok(async_stream::stream! {
                    while let Some(status) = statuses.recv().await {
                        yield status.map_err(status_error(WsSubscribeVcsStatusRpc::TAG));
                    }
                })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsVcsRefreshStatusRpc, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                a.refresh_status(&ctx.session_id, input)
                    .await
                    .map_err(status_error(WsVcsRefreshStatusRpc::TAG))
            }
        })?;
        let a = adapter.clone();
        registry.register_stream::<WsSubscribeWorktreeSetupRpc, _, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                // An outer `None` ends the stream; an inner `None` is a null event.
                let mut setups = a.subscribe_worktree_setup(&input).map_err(setup_error)?;
                Ok(async_stream::stream! {
                    while let Some(setup) = setups.recv().await {
                        yield Ok(setup);
                    }
                })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsWorktreeSetupCancelRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move { a.cancel_worktree_setup(&input).await.map_err(setup_error) }
        })?;
        let a = adapter;
        registry.register::<WsVcsCreateWorktreeRpc, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                a.create_worktree(&ctx.session_id, input)
                    .await
                    .map_err(worktree_error)
            }
        })?;
        Ok(())
    }
}

/// Served when the Git adapter has its workspace authority (and so its VCS services).
pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(all(feature = "git", feature = "local-services"))]
    if let Some(adapter) = domains.get::<crate::adapters::git::GitAdapter>()
        && adapter.vcs().is_some()
    {
        return live::register(registry, adapter);
    }
    let _ = domains;
    registry.unimplemented::<WsSubscribeVcsBranchRpc>()?;
    registry.unimplemented::<WsVcsRefreshBranchRpc>()?;
    registry.unimplemented::<WsSubscribeVcsStatusRpc>()?;
    registry.unimplemented::<WsVcsRefreshStatusRpc>()?;
    registry.unimplemented::<WsSubscribeWorktreeSetupRpc>()?;
    registry.unimplemented::<WsWorktreeSetupCancelRpc>()?;
    registry.unimplemented::<WsVcsCreateWorktreeRpc>()
}
