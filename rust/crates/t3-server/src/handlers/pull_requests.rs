//! The `pullRequests.*` RPCs over the one `PullRequestAdapter` (`ws.ts` pull-request block). The
//! RPC guard has already applied each method's scope.
//!
//! `generateReviewGuide` is registered beside its generator (`handlers/git_actions.rs`).
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

#[cfg(feature = "pull-requests")]
mod live {
    use super::*;
    use crate::{adapters::pull_requests::PullRequestAdapter, registry::Failure};
    use std::sync::Arc;
    use t3_pull_request::service::SummaryOptions;

    /// `PullRequestUnavailableError` for missing tools, sign-in and unsupported hosts; every other
    /// failure is a `PullRequestOperationError` with the service's fixed detail.
    fn wire(error: t3_pull_request::Error) -> Failure<PullRequestRpcError> {
        Failure::Typed(match error.unavailable_error() {
            Some(unavailable) => {
                PullRequestRpcError::PullRequestUnavailableError(Box::new(unavailable))
            }
            None => {
                PullRequestRpcError::PullRequestOperationError(Box::new(error.operation_error()))
            }
        })
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        adapter: Arc<PullRequestAdapter>,
    ) -> crate::Result<()> {
        macro_rules! unary {
            ($rpc:ty, |$a:ident, $input:ident| $call:expr) => {{
                let adapter = adapter.clone();
                registry.register::<$rpc, _, _>(move |_, $input| {
                    let $a = adapter.clone();
                    async move { $call.await.map_err(wire) }
                })?;
            }};
        }
        unary!(WsPullRequestsListRpc, |a, input| a.service.list(input));
        unary!(WsPullRequestsListStatsRpc, |a, input| a
            .service
            .list_stats(input));
        unary!(WsPullRequestsFilterOptionsRpc, |a, input| a
            .service
            .filter_options(input));
        unary!(WsPullRequestsRoutingRpc, |a, input| a
            .service
            .routing(input));
        unary!(WsPullRequestsRoutingIdentityRpc, |a, input| a
            .service
            .routing_identity(input));
        unary!(WsPullRequestsSummaryRpc, |a, input| a
            .service
            .summary(input, SummaryOptions::default()));
        unary!(WsPullRequestsStackRpc, |a, input| a
            .service
            .stack(input, true));
        unary!(WsPullRequestsLinkedThreadsRpc, |a, input| a
            .linked_threads(input));
        unary!(WsPullRequestsDetailRpc, |a, input| a.service.detail(input));
        unary!(WsPullRequestsPreviewRpc, |a, input| a
            .service
            .preview(input));
        unary!(WsPullRequestsChecksRpc, |a, input| a.service.checks(input));
        unary!(WsPullRequestsActivityRpc, |a, input| a
            .service
            .activity(input));
        unary!(WsPullRequestsReviewGuideRpc, |a, input| a.guides.get(input));
        unary!(WsPullRequestsThreadCommentsRpc, |a, input| a
            .service
            .thread_comments(input));
        unary!(WsPullRequestsDiffFileContentsRpc, |a, input| a
            .service
            .diff_file_contents(input));
        unary!(WsPullRequestsFilesViewedRpc, |a, input| a
            .service
            .files_viewed(input));
        unary!(WsPullRequestsChangeBreakdownRpc, |a, input| a
            .service
            .change_breakdown(input));
        unary!(WsPullRequestsSetFilesViewedRpc, |a, input| a
            .service
            .set_files_viewed(input));
        unary!(WsPullRequestsRunActionRpc, |a, input| a
            .service
            .run_action(input));
        unary!(WsPullRequestsUpdateRpc, |a, input| a.service.update(input));
        unary!(WsPullRequestsCommentRpc, |a, input| a
            .service
            .comment(input));
        unary!(WsPullRequestsUpdateCommentRpc, |a, input| a
            .service
            .update_comment(input));
        unary!(WsPullRequestsSubmitReviewRpc, |a, input| a
            .service
            .submit_review(input));
        unary!(WsPullRequestsReplyToThreadRpc, |a, input| a
            .service
            .reply_to_thread(input));
        unary!(WsPullRequestsSetThreadResolutionRpc, |a, input| a
            .service
            .set_thread_resolution(input));
        unary!(WsPullRequestsSetReactionRpc, |a, input| a
            .service
            .set_reaction(input));
        unary!(WsPullRequestsInvalidateRpc, |a, input| a
            .service
            .invalidate(input, true));
        unary!(WsPullRequestsReportStateRpc, |a, input| a
            .service
            .report_state(input));
        unary!(WsPullRequestsReviewerCandidatesRpc, |a, input| a
            .service
            .reviewer_candidates(input));
        unary!(WsPullRequestsRequestReviewersRpc, |a, input| a
            .service
            .request_reviewers(input));
        unary!(WsPullRequestsLabelCandidatesRpc, |a, input| a
            .service
            .label_candidates(input));
        unary!(WsPullRequestsSetLabelsRpc, |a, input| a
            .service
            .set_labels(input));
        // `SubscriptionRef.changes` filtered to revisions above zero: the current revision first
        // when there is one, then each change. Dropping the stream unsubscribes.
        fn revisions(
            mut revisions: tokio::sync::watch::Receiver<u64>,
        ) -> impl futures::Stream<
            Item = Result<u64, Failure<t3_contracts::auth::EnvironmentAuthorizationError>>,
        > {
            async_stream::stream! {
                loop {
                    let current = *revisions.borrow_and_update();
                    if current > 0 { yield Ok(current); }
                    if revisions.changed().await.is_err() { break; }
                }
            }
        }
        let service = adapter.service.clone();
        registry.register_stream::<WsPullRequestsSubscribeRefreshesRpc, _, _, _>(move |_, _| {
            let stream = revisions(service.subscribe_refreshes());
            async move { Ok(stream) }
        })?;
        registry.register_stream::<WsPullRequestsSubscribeLinkedThreadRefreshesRpc, _, _, _>(
            move |_, _| {
                let stream = revisions(adapter.service.subscribe_linked_thread_refreshes());
                async move { Ok(stream) }
            },
        )
    }
}

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "pull-requests")]
    if let Some(adapter) = domains.get::<crate::adapters::pull_requests::PullRequestAdapter>() {
        return live::register(registry, adapter);
    }
    let _ = domains;
    registry.unimplemented::<WsPullRequestsListRpc>()?;
    registry.unimplemented::<WsPullRequestsListStatsRpc>()?;
    registry.unimplemented::<WsPullRequestsFilterOptionsRpc>()?;
    registry.unimplemented::<WsPullRequestsRoutingRpc>()?;
    registry.unimplemented::<WsPullRequestsRoutingIdentityRpc>()?;
    registry.unimplemented::<WsPullRequestsSummaryRpc>()?;
    registry.unimplemented::<WsPullRequestsStackRpc>()?;
    registry.unimplemented::<WsPullRequestsLinkedThreadsRpc>()?;
    registry.unimplemented::<WsPullRequestsDetailRpc>()?;
    registry.unimplemented::<WsPullRequestsPreviewRpc>()?;
    registry.unimplemented::<WsPullRequestsChecksRpc>()?;
    registry.unimplemented::<WsPullRequestsActivityRpc>()?;
    registry.unimplemented::<WsPullRequestsReviewGuideRpc>()?;
    registry.unimplemented::<WsPullRequestsGenerateReviewGuideRpc>()?;
    registry.unimplemented::<WsPullRequestsThreadCommentsRpc>()?;
    registry.unimplemented::<WsPullRequestsDiffFileContentsRpc>()?;
    registry.unimplemented::<WsPullRequestsFilesViewedRpc>()?;
    registry.unimplemented::<WsPullRequestsChangeBreakdownRpc>()?;
    registry.unimplemented::<WsPullRequestsSetFilesViewedRpc>()?;
    registry.unimplemented::<WsPullRequestsRunActionRpc>()?;
    registry.unimplemented::<WsPullRequestsUpdateRpc>()?;
    registry.unimplemented::<WsPullRequestsCommentRpc>()?;
    registry.unimplemented::<WsPullRequestsUpdateCommentRpc>()?;
    registry.unimplemented::<WsPullRequestsSubmitReviewRpc>()?;
    registry.unimplemented::<WsPullRequestsReplyToThreadRpc>()?;
    registry.unimplemented::<WsPullRequestsSetThreadResolutionRpc>()?;
    registry.unimplemented::<WsPullRequestsSetReactionRpc>()?;
    registry.unimplemented::<WsPullRequestsInvalidateRpc>()?;
    registry.unimplemented::<WsPullRequestsReportStateRpc>()?;
    registry.unimplemented::<WsPullRequestsSubscribeRefreshesRpc>()?;
    registry.unimplemented::<WsPullRequestsSubscribeLinkedThreadRefreshesRpc>()?;
    registry.unimplemented::<WsPullRequestsReviewerCandidatesRpc>()?;
    registry.unimplemented::<WsPullRequestsRequestReviewersRpc>()?;
    registry.unimplemented::<WsPullRequestsLabelCandidatesRpc>()?;
    registry.unimplemented::<WsPullRequestsSetLabelsRpc>()
}
