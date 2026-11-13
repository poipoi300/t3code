use super::super::{Registry, register};
use t3_contracts::pull_request::*;
pub fn register_module(registry: &mut Registry) {
    register::<PullRequestAction>(registry, "PullRequestAction");
    register::<PullRequestActionInput>(registry, "PullRequestActionInput");
    register::<PullRequestActivity>(registry, "PullRequestActivity");
    register::<PullRequestActor>(registry, "PullRequestActor");
    register::<PullRequestBaseComparison>(registry, "PullRequestBaseComparison");
    register::<PullRequestCapabilities>(registry, "PullRequestCapabilities");
    register::<PullRequestChangeBreakdown>(registry, "PullRequestChangeBreakdown");
    register::<PullRequestCheck>(registry, "PullRequestCheck");
    register::<PullRequestCheckStatus>(registry, "PullRequestCheckStatus");
    register::<PullRequestChecks>(registry, "PullRequestChecks");
    register::<PullRequestChecksState>(registry, "PullRequestChecksState");
    register::<PullRequestComment>(registry, "PullRequestComment");
    register::<PullRequestCommentInput>(registry, "PullRequestCommentInput");
    register::<PullRequestCommentKind>(registry, "PullRequestCommentKind");
    register::<PullRequestCommentUpdateInput>(registry, "PullRequestCommentUpdateInput");
    register::<PullRequestCommit>(registry, "PullRequestCommit");
    register::<PullRequestDetail>(registry, "PullRequestDetail");
    register::<PullRequestDetailView>(registry, "PullRequestDetailView");
    register::<PullRequestDiffFileContentsInput>(registry, "PullRequestDiffFileContentsInput");
    register::<PullRequestDiffFileContentsResult>(registry, "PullRequestDiffFileContentsResult");
    register::<PullRequestDiffInput>(registry, "PullRequestDiffInput");
    register::<PullRequestDiffResult>(registry, "PullRequestDiffResult");
    register::<PullRequestDiffSide>(registry, "PullRequestDiffSide");
    register::<PullRequestDiffStat>(registry, "PullRequestDiffStat");
    register::<PullRequestEditCapabilities>(registry, "PullRequestEditCapabilities");
    register::<PullRequestFileViewed>(registry, "PullRequestFileViewed");
    register::<PullRequestFileViewedState>(registry, "PullRequestFileViewedState");
    register::<PullRequestFilesViewedResult>(registry, "PullRequestFilesViewedResult");
    register::<PullRequestFilterOptionKind>(registry, "PullRequestFilterOptionKind");
    register::<PullRequestFilterOptions>(registry, "PullRequestFilterOptions");
    register::<PullRequestFilterOptionsInput>(registry, "PullRequestFilterOptionsInput");
    register::<PullRequestInvalidateInput>(registry, "PullRequestInvalidateInput");
    register::<PullRequestInvolvement>(registry, "PullRequestInvolvement");
    register::<PullRequestLabel>(registry, "PullRequestLabel");
    register::<PullRequestLabelCandidate>(registry, "PullRequestLabelCandidate");
    register::<PullRequestLabelCandidateList>(registry, "PullRequestLabelCandidateList");
    register::<PullRequestLabelChangeInput>(registry, "PullRequestLabelChangeInput");
    register::<PullRequestLinkedThreadsResult>(registry, "PullRequestLinkedThreadsResult");
    register::<PullRequestListCursors>(registry, "PullRequestListCursors");
    register::<PullRequestListEntry>(registry, "PullRequestListEntry");
    register::<PullRequestListFilters>(registry, "PullRequestListFilters");
    register::<PullRequestListInput>(registry, "PullRequestListInput");
    register::<PullRequestListProjectError>(registry, "PullRequestListProjectError");
    register::<PullRequestListResult>(registry, "PullRequestListResult");
    register::<PullRequestListState>(registry, "PullRequestListState");
    register::<PullRequestListStatsInput>(registry, "PullRequestListStatsInput");
    register::<PullRequestListStatsResult>(registry, "PullRequestListStatsResult");
    register::<PullRequestMergeCapabilities>(registry, "PullRequestMergeCapabilities");
    register::<PullRequestMergeMethod>(registry, "PullRequestMergeMethod");
    register::<PullRequestMergeQueueRemoval>(registry, "PullRequestMergeQueueRemoval");
    register::<PullRequestMergeability>(registry, "PullRequestMergeability");
    register::<PullRequestOmittedFileStat>(registry, "PullRequestOmittedFileStat");
    register::<PullRequestOperationError>(registry, "PullRequestOperationError");
    register::<PullRequestOverviewThread>(registry, "PullRequestOverviewThread");
    register::<PullRequestPreview>(registry, "PullRequestPreview");
    register::<PullRequestProviderSummary>(registry, "PullRequestProviderSummary");
    register::<PullRequestReaction>(registry, "PullRequestReaction");
    register::<PullRequestReactionContent>(registry, "PullRequestReactionContent");
    register::<PullRequestReactionInput>(registry, "PullRequestReactionInput");
    register::<PullRequestRef>(registry, "PullRequestRef");
    register::<PullRequestReferenceCandidate>(registry, "PullRequestReferenceCandidate");
    register::<PullRequestReferenceSigil>(registry, "PullRequestReferenceSigil");
    register::<PullRequestReportStateInput>(registry, "PullRequestReportStateInput");
    register::<PullRequestReviewCapabilities>(registry, "PullRequestReviewCapabilities");
    register::<PullRequestReviewCommentDraft>(registry, "PullRequestReviewCommentDraft");
    register::<PullRequestReviewDecision>(registry, "PullRequestReviewDecision");
    register::<PullRequestReviewPosition>(registry, "PullRequestReviewPosition");
    register::<PullRequestReviewThread>(registry, "PullRequestReviewThread");
    register::<PullRequestReviewVerdict>(registry, "PullRequestReviewVerdict");
    register::<PullRequestReviewerCandidate>(registry, "PullRequestReviewerCandidate");
    register::<PullRequestReviewerCandidateList>(registry, "PullRequestReviewerCandidateList");
    register::<PullRequestReviewerCandidatesInput>(registry, "PullRequestReviewerCandidatesInput");
    register::<PullRequestReviewerCapabilities>(registry, "PullRequestReviewerCapabilities");
    register::<PullRequestReviewerKind>(registry, "PullRequestReviewerKind");
    register::<PullRequestReviewerRequestInput>(registry, "PullRequestReviewerRequestInput");
    register::<PullRequestRoutingIdentityInput>(registry, "PullRequestRoutingIdentityInput");
    register::<PullRequestRoutingIdentityResult>(registry, "PullRequestRoutingIdentityResult");
    register::<PullRequestRoutingResult>(registry, "PullRequestRoutingResult");
    register::<PullRequestSetFilesViewedInput>(registry, "PullRequestSetFilesViewedInput");
    register::<PullRequestStack>(registry, "PullRequestStack");
    register::<PullRequestStackHead>(registry, "PullRequestStackHead");
    register::<PullRequestStackMembership>(registry, "PullRequestStackMembership");
    register::<PullRequestState>(registry, "PullRequestState");
    register::<PullRequestSubmitReviewInput>(registry, "PullRequestSubmitReviewInput");
    register::<PullRequestSummary>(registry, "PullRequestSummary");
    register::<PullRequestThreadComment>(registry, "PullRequestThreadComment");
    register::<PullRequestThreadCommentsInput>(registry, "PullRequestThreadCommentsInput");
    register::<PullRequestThreadCommentsResult>(registry, "PullRequestThreadCommentsResult");
    register::<PullRequestThreadReplyInput>(registry, "PullRequestThreadReplyInput");
    register::<PullRequestThreadResolutionInput>(registry, "PullRequestThreadResolutionInput");
    register::<PullRequestUnavailableError>(registry, "PullRequestUnavailableError");
    register::<PullRequestUnavailableReason>(registry, "PullRequestUnavailableReason");
    register::<PullRequestUpdateInput>(registry, "PullRequestUpdateInput");
    register::<PullRequestUpdateMethod>(registry, "PullRequestUpdateMethod");
    register::<PullRequestViewedFilesStore>(registry, "PullRequestViewedFilesStore");
    register::<PullRequestViewerPermissions>(registry, "PullRequestViewerPermissions");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("pull_request", register_module)
}

#[test]
fn lenient_optional_arrays_preserve_presence_and_defaults_decode() -> Result<(), serde_json::Error>
{
    use serde_json::json;
    let common = json!({"stackRebase": null, "actions": ["merge", "future"], "comment": true, "resolve": true, "verdicts": ["comment", "future"], "requestReviewers": false});
    for optional in [None, Some(json!(null)), Some(json!(["rebase", "future"]))] {
        let mut value = common.clone();
        if let Some(optional) = optional {
            value["updateMethods"] = optional;
        }
        let decoded: PullRequestViewerPermissions = serde_json::from_value(value.clone())?;
        value["actions"] = json!(["merge"]);
        value["verdicts"] = json!(["comment"]);
        if value["updateMethods"].is_array() {
            value["updateMethods"] = json!(["rebase"]);
        }
        assert_eq!(serde_json::to_value(decoded)?, value);
    }
    let options: PullRequestFilterOptions =
        serde_json::from_value(json!({"labels": [], "authors": [], "truncated": false}))?;
    assert_eq!(
        serde_json::to_value(options)?,
        json!({"labels": [], "authors": [], "references": [], "truncated": false})
    );
    let count = json!({"additions": 0, "deletions": 0});
    let mut value = json!({"code": count, "tests": count, "mocks": count, "generated": count});
    let breakdown: PullRequestChangeBreakdown = serde_json::from_value(value.clone())?;
    value["docs"] = count;
    assert_eq!(serde_json::to_value(breakdown)?, value);
    Ok(())
}
