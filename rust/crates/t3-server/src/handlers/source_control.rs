//! Source-control RPCs; application PR/cache/watch workflows remain unsupported.
use crate::registry::HandlerRegistry;
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

pub fn register(registry: &mut HandlerRegistry) -> crate::Result<()> {
    #[cfg(feature = "git")]
    {
        registry.register::<WsVcsPullRpc, _, _>(|ctx, input| async move {
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
            adapter(&ctx, WsVcsCreateRefRpc::TAG)?
                .create_ref(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsSwitchRefRpc, _, _>(|ctx, input| async move {
            adapter(&ctx, WsVcsSwitchRefRpc::TAG)?
                .switch_ref(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsRemoveWorktreeRpc, _, _>(|ctx, input| async move {
            adapter(&ctx, WsVcsRemoveWorktreeRpc::TAG)?
                .remove_worktree(&ctx.session_id, input)
                .await
                .map_err(vcs_error)
        })?;
        registry.register::<WsVcsInitRpc, _, _>(|ctx, input| async move {
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
