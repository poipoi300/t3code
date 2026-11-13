pub(super) const PIPELINE_STAGES_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iid: String!) {
  project(fullPath: $fullPath) {
    mergeRequest(iid: $iid) {
      headPipeline {
        path
        stages(first: 100) {
          nodes {
            name
            status
            detailedStatus { label detailsPath }
            jobs(first: 100) {
              nodes {
                name status allowFailure retried webPath detailedStatus { label }
                downstreamPipeline { status path }
              }
              pageInfo { hasNextPage }
            }
          }
          pageInfo { hasNextPage }
        }
      }
    }
  }
}"###;
pub(super) const AWARD_EMOJI_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iid: String!, $cursor: String) {
  currentUser { username }
  project(fullPath: $fullPath) {
    mergeRequest(iid: $iid) {
      awardEmoji { nodes { name user { username } } }
      notes(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes { id author { username bot } lastEditedAt lastEditedBy { username } resolved awardEmoji { nodes { name user { username } } } }
      }
    }
  }
}"###;
pub(super) const REPOSITORY_BLOBS_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $ref: String!, $paths: [String!]!) {
  project(fullPath: $fullPath) {
    repository {
      blobs(ref: $ref, paths: $paths) {
        nodes { path oid }
      }
    }
  }
}"###;
pub(super) const SOURCE_BRANCH_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iid: String!) {
  project(fullPath: $fullPath) {
    mergeRequest(iid: $iid) {
      state sourceBranch diffHeadSha
      sourceProject { fullPath }
      userPermissions { removeSourceBranch }
    }
  }
}"###;
pub(super) const MERGE_MESSAGES_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iid: String!) {
  project(fullPath: $fullPath) {
    mergeRequest(iid: $iid) { title squashOnMerge defaultMergeCommitMessage defaultSquashCommitMessage }
  }
}"###;
pub(super) const LIST_STATE_GRAPHQL_FIELDS: &str = r###"iid approved approvedBy(first: 1) { nodes { username } }
  reviewers(first: 20) { nodes { username mergeRequestInteraction { reviewState } } }
  headPipeline { status }"###;
pub(super) const LIST_STATES_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iids: [String!]) {
  metadata { enterprise }
  project(fullPath: $fullPath) {
    mergeRequests(iids: $iids, first: 100) {
      nodes { iid approved approvedBy(first: 1) { nodes { username } }
  reviewers(first: 20) { nodes { username mergeRequestInteraction { reviewState } } }
  headPipeline { status } }
    }
  }
}"###;
pub(super) const LIST_ITEM_GRAPHQL_FIELDS: &str = r###"iid approved approvedBy(first: 1) { nodes { username } }
  reviewers(first: 20) { nodes { username mergeRequestInteraction { reviewState } } }
  headPipeline { status }
  title webUrl state draft createdAt updatedAt mergedAt sourceBranch targetBranch
  conflicts mergeStatusEnum author { username name avatarUrl } labels(first: 100) { nodes { title } }"###;
pub(super) const MERGE_REQUEST_STATS_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iids: [String!]) {
  project(fullPath: $fullPath) {
    mergeRequests(iids: $iids, first: 100) {
      nodes { iid diffStatsSummary { additions deletions } }
    }
  }
}"###;
pub(super) const LABEL_PERMISSION_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iid: String!) {
  project(fullPath: $fullPath) {
    mergeRequest(iid: $iid) {
      userPermissions { adminMergeRequest }
    }
  }
}"###;
pub(super) const MAINTAINER_EDITS_PERMISSION_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iid: String!) {
  currentUser { username }
  project(fullPath: $fullPath) {
    visibility
    mergeRequest(iid: $iid) {
      author { username }
      userPermissions { pushToSourceBranch }
      sourceBranchProtected
      sourceProject { visibility }
    }
  }
}"###;
pub(super) const MERGE_REQUEST_FILE_STATS_GRAPHQL_QUERY: &str = r###"query($fullPath: ID!, $iid: String!) {
  project(fullPath: $fullPath) {
    mergeRequest(iid: $iid) {
      diffStatsSummary { fileCount }
      diffStats { path additions deletions }
    }
  }
}"###;
pub(super) const SET_SELF_AS_REVIEWER_MUTATION: &str = r###"mutation($projectPath: ID!, $iid: String!, $reviewers: [String!]!) {
  mergeRequestSetReviewers(
    input: { projectPath: $projectPath, iid: $iid, reviewerUsernames: $reviewers, operationMode: APPEND }
  ) { errors }
}"###;
pub(super) const REQUEST_CHANGES_MUTATION: &str = r###"mutation($projectPath: ID!, $iid: String!) {
  mergeRequestRequestChanges(input: { projectPath: $projectPath, iid: $iid }) { errors }
}"###;
pub(super) const SERVER_VERSION_GRAPHQL_QUERY: &str = r###"{ metadata { version } }"###;
