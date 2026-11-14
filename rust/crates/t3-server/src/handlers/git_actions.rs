//! `git.runStackedAction` and `git.preparePullRequestThread` (`ws.ts`) over
//! `adapters/git_actions.rs`: each handler calls one adapter method and maps its refusal to the
//! method's contract failure. The RPC guard has already applied the method's scope.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "git-actions")]
    if let Some(adapter) = domains.get::<crate::adapters::git_actions::GitActionsAdapter>() {
        return live::register(registry, adapter);
    }
    let _ = domains;
    registry.unimplemented::<WsGitRunStackedActionRpc>()?;
    registry.unimplemented::<WsGitPreparePullRequestThreadRpc>()
}

#[cfg(feature = "git-actions")]
mod live {
    use super::*;
    use crate::{
        adapters::{forge::PullRequestRefusal, git_actions::GitActionsAdapter},
        registry::Failure,
    };
    use std::sync::Arc;

    fn refusal(refusal: PullRequestRefusal) -> Failure<WsSubscribeVcsBranchError> {
        Failure::Typed(match refusal {
            PullRequestRefusal::Authorization(e) => {
                WsSubscribeVcsBranchError::EnvironmentAuthorizationError(e)
            }
            PullRequestRefusal::Git(e) => WsSubscribeVcsBranchError::GitManagerServiceError(e),
        })
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        adapter: Arc<GitActionsAdapter>,
    ) -> crate::Result<()> {
        // Dropping the stream (socket close, interrupt, shutdown) cancels the action.
        let a = adapter.clone();
        registry.register_stream::<WsGitRunStackedActionRpc, _, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                let mut events = a
                    .run_stacked_action(&ctx.session_id, input)
                    .await
                    .map_err(refusal)?;
                Ok(async_stream::stream! {
                    use futures::StreamExt;
                    while let Some(event) = events.next().await {
                        yield event.map_err(|e| {
                            Failure::Typed(WsSubscribeVcsBranchError::GitManagerServiceError(e))
                        });
                    }
                })
            }
        })?;
        let a = adapter;
        registry.register::<WsGitPreparePullRequestThreadRpc, _, _>(move |ctx, input| {
            let a = a.clone();
            async move {
                a.prepare_pull_request_thread(&ctx.session_id, input)
                    .await
                    .map_err(refusal)
            }
        })
    }
}
