//! Source-control RPCs: the VCS and review methods over the Git adapter, and the ten
//! source-control methods plus `git.resolvePullRequest` over the forge registry
//! (`adapters/forge.rs`). Pull-request application workflows are served elsewhere.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub const REGISTERED: &[&str] = &[
    WsVcsPullRpc::TAG,
    WsVcsListRefsRpc::TAG,
    WsVcsRemoveWorktreeRpc::TAG,
    WsVcsCreateRefRpc::TAG,
    WsVcsSwitchRefRpc::TAG,
    WsVcsInitRpc::TAG,
    WsReviewGetBranchCommitsRpc::TAG,
    WsReviewGetDiffPreviewRpc::TAG,
    WsReviewGetDiffFileContentsRpc::TAG,
];

fn register_vcs(registry: &mut HandlerRegistry) -> crate::Result<()> {
    #[cfg(feature = "git")]
    {
        registry.register::<WsVcsPullRpc, _, _>(|ctx, input| async move {
            let _admitted = crate::writers::admit(&ctx.services, "Git pull", &input.cwd.0).await;
            adapter(&ctx, WsVcsPullRpc::TAG)?
                .pull(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsListRefsRpc, _, _>(|ctx, input| async move {
            adapter(&ctx, WsVcsListRefsRpc::TAG)?
                .list_refs(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsCreateRefRpc, _, _>(|ctx, input| async move {
            let _admitted = crate::writers::admit(&ctx.services, "Git branch creation", &input.cwd.0).await;
            adapter(&ctx, WsVcsCreateRefRpc::TAG)?
                .create_ref(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsSwitchRefRpc, _, _>(|ctx, input| async move {
            let _admitted = crate::writers::admit(&ctx.services, "Git checkout", &input.cwd.0).await;
            adapter(&ctx, WsVcsSwitchRefRpc::TAG)?
                .switch_ref(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsRemoveWorktreeRpc, _, _>(|ctx, input| async move {
            let _admitted = crate::writers::admit(&ctx.services, "Git worktree removal", &input.cwd.0).await;
            let _removed = crate::writers::admit(&ctx.services, "Git worktree removal", &input.path.0).await;
            adapter(&ctx, WsVcsRemoveWorktreeRpc::TAG)?
                .remove_worktree(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsInitRpc, _, _>(|ctx, input| async move {
            let _admitted = crate::writers::admit(&ctx.services, "Git init", &input.cwd.0).await;
            adapter(&ctx, WsVcsInitRpc::TAG)?
                .init(&ctx.session_id, input)
                .await
                .map_err(init_error)
        })?;
        registry.register::<WsReviewGetBranchCommitsRpc, _, _>(|ctx, input| async move {
            adapter(&ctx, WsReviewGetBranchCommitsRpc::TAG)?
                .branch_commits(&ctx.session_id, input)
                .await
                .map_err(review_error)
        })?;
        registry.register::<WsReviewGetDiffPreviewRpc, _, _>(|ctx, input| async move {
            adapter(&ctx, WsReviewGetDiffPreviewRpc::TAG)?
                .diff_preview(&ctx.session_id, input)
                .await
                .map_err(review_error)
        })?;
        registry.register::<WsReviewGetDiffFileContentsRpc, _, _>(|ctx, input| async move {
            adapter(&ctx, WsReviewGetDiffFileContentsRpc::TAG)?
                .diff_file_contents(&ctx.session_id, input)
                .await
                .map_err(review_error)
        })?;
    }
    #[cfg(not(feature = "git"))]
    {
        registry.unimplemented::<WsVcsPullRpc>()?;
        registry.unimplemented::<WsVcsListRefsRpc>()?;
        registry.unimplemented::<WsVcsRemoveWorktreeRpc>()?;
        registry.unimplemented::<WsVcsCreateRefRpc>()?;
        registry.unimplemented::<WsVcsSwitchRefRpc>()?;
        registry.unimplemented::<WsVcsInitRpc>()?;
        registry.unimplemented::<WsReviewGetBranchCommitsRpc>()?;
        registry.unimplemented::<WsReviewGetDiffPreviewRpc>()?;
        registry.unimplemented::<WsReviewGetDiffFileContentsRpc>()?;
    }
    Ok(())
}

#[cfg(feature = "git")]
use crate::{
    adapters::git::{Error, GitAdapter, command_error},
    registry::{Context, Failure, ServerDefect},
};

#[cfg(feature = "git")]
fn adapter<E>(ctx: &Context, method: &str) -> Result<std::sync::Arc<GitAdapter>, Failure<E>> {
    ctx.services
        .domain::<GitAdapter>()
        .filter(|adapter| adapter.is_available())
        .ok_or_else(|| {
            Failure::Defect(ServerDefect::NotImplemented {
                method: method.into(),
            })
        })
}

#[cfg(feature = "git")]
fn vcs_error(error: Error) -> Failure<WsVcsPullError> {
    Failure::Typed(match error {
        Error::Authorization(e) => WsVcsPullError::EnvironmentAuthorizationError(e),
        Error::Git(e) => WsVcsPullError::GitCommandError(e),
        Error::Vcs(_) => WsVcsPullError::GitCommandError(Box::new(command_error("vcs"))),
    })
}

#[cfg(feature = "git")]
fn review_error(error: Error) -> Failure<WsReviewGetBranchCommitsError> {
    use t3_contracts::review::ReviewDiffPreviewError;
    Failure::Typed(match error {
        Error::Authorization(e) => WsReviewGetBranchCommitsError::EnvironmentAuthorizationError(e),
        Error::Git(e) => WsReviewGetBranchCommitsError::ReviewDiffPreviewError(Box::new(
            ReviewDiffPreviewError::Git(*e),
        )),
        Error::Vcs(e) => WsReviewGetBranchCommitsError::ReviewDiffPreviewError(Box::new(
            ReviewDiffPreviewError::Vcs(*e),
        )),
    })
}

#[cfg(feature = "git")]
fn init_error(error: Error) -> Failure<WsVcsInitError> {
    use t3_contracts::vcs::*;
    Failure::Typed(match error {
        Error::Authorization(e) => WsVcsInitError::EnvironmentAuthorizationError(e),
        Error::Vcs(e) => WsVcsInitError::VcsError(e),
        Error::Git(_) => WsVcsInitError::VcsError(Box::new(VcsError::RepositoryDetection(
            VcsRepositoryDetectionError {
                _tag: VcsRepositoryDetectionErrorTag::VcsRepositoryDetectionError,
                operation: "vcs.init".into(),
                cwd: String::new(),
                detail: "Git could not initialize the repository.".into(),
                cause: None,
            },
        ))),
    })
}

/// Every method in this module: the VCS group, then the forge group (served only with the
/// forge registry; otherwise unsupported).
pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    register_vcs(registry)?;
    #[cfg(feature = "forge")]
    if let Some(forge) = domains.get::<crate::adapters::forge::ForgeAdapter>() {
        return forge::register(registry, forge);
    }
    let _ = domains;
    registry.unimplemented::<WsSourceControlLookupRepositoryRpc>()?;
    registry.unimplemented::<WsSourceControlCloneRepositoryRpc>()?;
    registry.unimplemented::<WsSourceControlPublishRepositoryRpc>()?;
    registry.unimplemented::<WsSourceControlSignInStartRpc>()?;
    registry.unimplemented::<WsSourceControlSignInWaitRpc>()?;
    registry.unimplemented::<WsSourceControlSignInCancelRpc>()?;
    registry.unimplemented::<WsSourceControlAppAccessRpc>()?;
    registry.unimplemented::<WsSourceControlStarredRpc>()?;
    registry.unimplemented::<WsSourceControlStarRpc>()?;
    registry.unimplemented::<WsServerDiscoverSourceControlRpc>()?;
    registry.unimplemented::<WsGitResolvePullRequestRpc>()
}

/// `ws.ts`'s source-control block: each method calls one forge adapter method. The RPC guard
/// has already applied the method's scope.
#[cfg(feature = "forge")]
mod forge {
    use super::*;
    use crate::{
        adapters::forge::{ForgeAdapter, PullRequestRefusal, Refusal},
        registry::{Failure, ServerDefect},
    };
    use std::sync::Arc;
    use t3_contracts::source_control::SourceControlSignInError;

    fn repository(refusal: Refusal) -> Failure<WsSourceControlLookupRepositoryError> {
        Failure::Typed(match refusal {
            Refusal::Authorization(e) => {
                WsSourceControlLookupRepositoryError::EnvironmentAuthorizationError(e)
            }
            Refusal::Repository(e) => {
                WsSourceControlLookupRepositoryError::SourceControlRepositoryError(e)
            }
        })
    }

    fn sign_in(error: Box<SourceControlSignInError>) -> Failure<WsSourceControlSignInStartError> {
        Failure::Typed(WsSourceControlSignInStartError::SourceControlSignInError(
            error,
        ))
    }

    pub fn register(registry: &mut HandlerRegistry, forge: Arc<ForgeAdapter>) -> crate::Result<()> {
        macro_rules! method {
            ($rpc:ty, |$f:ident, $ctx:ident, $input:ident| $call:expr) => {{
                let forge = forge.clone();
                registry.register::<$rpc, _, _>(move |$ctx, $input| {
                    let $f = forge.clone();
                    async move { $call }
                })?;
            }};
        }
        method!(WsSourceControlLookupRepositoryRpc, |f, ctx, input| f
            .lookup_repository(&ctx.session_id, input)
            .await
            .map_err(repository));
        method!(WsSourceControlCloneRepositoryRpc, |f, _ctx, input| f
            .clone_repository(input)
            .await
            .map_err(repository));
        method!(WsSourceControlPublishRepositoryRpc, |f, ctx, input| f
            .publish_repository(&ctx.session_id, input)
            .await
            .map_err(repository));
        method!(WsSourceControlSignInStartRpc, |f, _ctx, input| f
            .sign_in_start(input)
            .await
            .map_err(sign_in));
        method!(WsSourceControlSignInWaitRpc, |f, _ctx, input| f
            .sign_in_wait(input)
            .await
            .map_err(sign_in));
        method!(WsSourceControlSignInCancelRpc, |f, _ctx, input| f
            .sign_in_cancel(input)
            .await
            .map_err(sign_in));
        method!(WsSourceControlAppAccessRpc, |f, _ctx, input| Ok(f
            .app_access(input)
            .await));
        method!(WsSourceControlStarredRpc, |f, _ctx, input| Ok(f
            .starred(input)
            .await));
        method!(WsSourceControlStarRpc, |f, _ctx, input| Ok(f
            .star(input)
            .await));
        method!(WsServerDiscoverSourceControlRpc, |f, _ctx, _input| f
            .discover()
            .await
            .ok_or_else(|| Failure::Defect(ServerDefect::Internal {
                message: "Source control discovery failed.".into(),
            })));
        method!(WsGitResolvePullRequestRpc, |f, ctx, input| f
            .resolve_pull_request(&ctx.session_id, input)
            .await
            .map_err(|refusal| Failure::Typed(match refusal {
                PullRequestRefusal::Authorization(e) => {
                    WsSubscribeVcsBranchError::EnvironmentAuthorizationError(e)
                }
                PullRequestRefusal::Git(e) => WsSubscribeVcsBranchError::GitManagerServiceError(e),
            })));
        Ok(())
    }
}
