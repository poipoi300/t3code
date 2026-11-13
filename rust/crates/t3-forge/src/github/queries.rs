pub fn document(name: &str) -> Option<&'static str> {
    match name {
        "PullRequestSummary" => Some(include_str!("graphql/PullRequestSummary.graphql")),
        "PullRequestStats" => Some(include_str!("graphql/PullRequestStats.graphql")),
        "PullRequestStackMembership" => {
            Some(include_str!("graphql/PullRequestStackMembership.graphql"))
        }
        "PullRequestWatchFingerprint" => {
            Some(include_str!("graphql/PullRequestWatchFingerprint.graphql"))
        }
        "PullRequestSearch" => Some(include_str!("graphql/PullRequestSearch.graphql")),
        "PullRequestList" => Some(include_str!("graphql/PullRequestList.graphql")),
        "AddReviewThreadReply" => Some(include_str!("graphql/AddReviewThreadReply.graphql")),
        "AddReaction" => Some(include_str!("graphql/AddReaction.graphql")),
        "RemoveReaction" => Some(include_str!("graphql/RemoveReaction.graphql")),
        "ResolveReviewThread" => Some(include_str!("graphql/ResolveReviewThread.graphql")),
        "UnresolveReviewThread" => Some(include_str!("graphql/UnresolveReviewThread.graphql")),
        "UpdatePullRequest" => Some(include_str!("graphql/UpdatePullRequest.graphql")),
        "RevertPullRequest" => Some(include_str!("graphql/RevertPullRequest.graphql")),
        "UpdateIssueComment" => Some(include_str!("graphql/UpdateIssueComment.graphql")),
        "UpdateReviewComment" => Some(include_str!("graphql/UpdateReviewComment.graphql")),
        "SetFileViewed" => Some(include_str!("graphql/SetFileViewed.graphql")),
        "MergePullRequest" => Some(include_str!("graphql/MergePullRequest.graphql")),
        "EnablePullRequestAutoMerge" => {
            Some(include_str!("graphql/EnablePullRequestAutoMerge.graphql"))
        }
        "DisablePullRequestAutoMerge" => {
            Some(include_str!("graphql/DisablePullRequestAutoMerge.graphql"))
        }
        "UpdatePullRequestBranch" => Some(include_str!("graphql/UpdatePullRequestBranch.graphql")),
        "MarkPullRequestReadyForReview" => Some(include_str!(
            "graphql/MarkPullRequestReadyForReview.graphql"
        )),
        "ConvertPullRequestToDraft" => {
            Some(include_str!("graphql/ConvertPullRequestToDraft.graphql"))
        }
        "ClosePullRequest" => Some(include_str!("graphql/ClosePullRequest.graphql")),
        "ReopenPullRequest" => Some(include_str!("graphql/ReopenPullRequest.graphql")),
        "AddComment" => Some(include_str!("graphql/AddComment.graphql")),
        "DeletePullRequestBranch" => Some(include_str!("graphql/DeletePullRequestBranch.graphql")),
        "RoutingIdentityViewer" => Some(include_str!("graphql/RoutingIdentityViewer.graphql")),
        "ActorAvatars" => Some(include_str!("graphql/ActorAvatars.graphql")),
        "PullRequestCore" => Some(include_str!("graphql/PullRequestCore.graphql")),
        "PullRequestCheckContexts" => {
            Some(include_str!("graphql/PullRequestCheckContexts.graphql"))
        }
        "PullRequestPreview" => Some(include_str!("graphql/PullRequestPreview.graphql")),
        "PullRequestActivity" => Some(include_str!("graphql/PullRequestActivity.graphql")),
        "PullRequestCommitStats" => Some(include_str!("graphql/PullRequestCommitStats.graphql")),
        "PullRequestHeads" => Some(include_str!("graphql/PullRequestHeads.graphql")),
        "PullRequestNodeId" => Some(include_str!("graphql/PullRequestNodeId.graphql")),
        "PullRequestSubject" => Some(include_str!("graphql/PullRequestSubject.graphql")),
        "ViewerPermissions" => Some(include_str!("graphql/ViewerPermissions.graphql")),
        "ReviewerCandidates" => Some(include_str!("graphql/ReviewerCandidates.graphql")),
        "LabelCandidates" => Some(include_str!("graphql/LabelCandidates.graphql")),
        "RepositoryLabels" => Some(include_str!("graphql/RepositoryLabels.graphql")),
        "RepositoryReferences" => Some(include_str!("graphql/RepositoryReferences.graphql")),
        "RepositoryAuthors" => Some(include_str!("graphql/RepositoryAuthors.graphql")),
        "PullRequestFilesViewed" => Some(include_str!("graphql/PullRequestFilesViewed.graphql")),
        "PullRequestFileStats" => Some(include_str!("graphql/PullRequestFileStats.graphql")),
        "PullRequestMergeMessage" => Some(include_str!("graphql/PullRequestMergeMessage.graphql")),
        "PullRequestActionState" => Some(include_str!("graphql/PullRequestActionState.graphql")),
        "PullRequestBranchToDelete" => {
            Some(include_str!("graphql/PullRequestBranchToDelete.graphql"))
        }
        "PullRequestSourceBranch" => Some(include_str!("graphql/PullRequestSourceBranch.graphql")),
        "ReviewThreads" => Some(include_str!("graphql/ReviewThreads.graphql")),
        "ReviewThreadPages" => Some(include_str!("graphql/ReviewThreadPages.graphql")),
        "ReviewThreadComments" => Some(include_str!("graphql/ReviewThreadComments.graphql")),
        "ReviewDismissals" => Some(include_str!("graphql/ReviewDismissals.graphql")),
        "PullRequestsByHead" => Some(include_str!("graphql/PullRequestsByHead.graphql")),
        "PullRequestByNumber" => Some(include_str!("graphql/PullRequestByNumber.graphql")),
        "PullRequestBranchAccess" => Some(include_str!("graphql/PullRequestBranchAccess.graphql")),
        _ => None,
    }
}
