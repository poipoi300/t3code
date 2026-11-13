use crate::{
    Error, Result,
    context::{Context, Project},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
use t3_contracts::pull_request::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub status: String,
    pub remarks: String,
}
pub type Changes = std::pin::Pin<Box<dyn futures_util::Stream<Item = ()> + Send>>;
pub struct VerifiedHost {
    pub account_id: String,
    pub viewer: String,
    pub credential_fingerprint: String,
    pub host: Arc<dyn Host>,
}
#[derive(Debug, Clone)]
pub struct RoutingIdentity {
    pub account_id: String,
    pub viewer: String,
    pub credential_fingerprint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListCursor {
    pub updated_before: String,
    pub delivered: u64,
    pub seen_at: Vec<u64>,
    pub resume_after: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ListQuery {
    pub input: PullRequestListInput,
    pub viewer: String,
    pub limit: usize,
    pub cursor: Option<ListCursor>,
}
#[derive(Debug, Clone)]
pub struct ListPage {
    pub entries: Vec<PullRequestListEntry>,
    pub truncated: bool,
    pub continues: bool,
    pub cursor_advance: Option<u64>,
    pub resume_after: Option<String>,
}
pub struct AcrossPage {
    pub entries: Vec<PullRequestListEntry>,
    pub truncated: bool,
    pub unresolved: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct DiffPaths {
    pub files: Vec<(String, String)>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone)]
pub struct FileStats {
    pub files: Vec<(String, u64, u64)>,
    pub truncated: bool,
}
#[derive(Debug, Clone)]
pub struct FileRevisions {
    pub revisions: BTreeMap<String, String>,
    pub complete: bool,
}

#[async_trait]
pub trait Host: Send + Sync {
    fn kind(&self) -> &str;
    fn capabilities(&self) -> &PullRequestCapabilities;
    fn revision(&self) -> u64 {
        0
    }
    fn batches_reads(&self) -> bool {
        false
    }
    fn reads_public_host(&self, _host: &str) -> bool {
        false
    }
    async fn verify(&self, _project: &Project) -> Result<VerifiedHost> {
        Err(Error::unsupported("routeIdentity"))
    }
    async fn routing_identity(&self, _project: &Project) -> Result<RoutingIdentity> {
        Err(Error::unsupported("routeIdentity"))
    }
    async fn viewer(&self, project: &Project) -> Result<String> {
        Ok(self.routing_identity(project).await?.viewer)
    }
    async fn changes(&self, _context: &Context) -> Result<Changes> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
    async fn forget_reads(&self, _context: Option<&Context>) {}
    async fn list_across(
        &self,
        _context: &Context,
        _repositories: &[String],
        _query: ListQuery,
    ) -> Result<Option<AcrossPage>> {
        Ok(None)
    }
    async fn summary(&self, _context: &Context) -> Result<PullRequestSummary> {
        Err(Error::unsupported("summary"))
    }
    async fn detail(&self, _context: &Context) -> Result<PullRequestDetail> {
        Err(Error::unsupported("detail"))
    }
    async fn preview(&self, _context: &Context) -> Result<PullRequestPreview> {
        Err(Error::unsupported("preview"))
    }
    async fn checks(&self, _context: &Context) -> Result<Option<PullRequestChecks>> {
        Err(Error::unsupported("checks"))
    }
    async fn watch_fingerprint(&self, _context: &Context) -> Result<Option<Fingerprint>> {
        Err(Error::unsupported("watch_fingerprint"))
    }
    async fn activity(&self, _context: &Context) -> Result<PullRequestActivity> {
        Err(Error::unsupported("activity"))
    }
    async fn files_viewed(&self, _context: &Context) -> Result<PullRequestFilesViewedResult> {
        Err(Error::unsupported("files_viewed"))
    }
    async fn file_stats(&self, _context: &Context) -> Result<FileStats> {
        Err(Error::unsupported("file_stats"))
    }
    async fn label_candidates(&self, _context: &Context) -> Result<PullRequestLabelCandidateList> {
        Err(Error::unsupported("label_candidates"))
    }
    async fn stack(
        &self,
        _context: &Context,
        _include_details: bool,
    ) -> Result<Option<PullRequestStack>> {
        Err(Error::unsupported("stack"))
    }
    async fn permissions(
        &self,
        _context: &Context,
        _include_update_branch: bool,
    ) -> Result<PullRequestViewerPermissions> {
        Err(Error::unsupported("permissions"))
    }
    async fn list(&self, _context: &Context, _query: ListQuery) -> Result<ListPage> {
        Err(Error::unsupported("list"))
    }
    async fn diff(
        &self,
        _context: &Context,
        _input: &PullRequestDiffInput,
    ) -> Result<PullRequestDiffResult> {
        Err(Error::unsupported("diff"))
    }
    async fn diff_paths(
        &self,
        _context: &Context,
        _input: &PullRequestDiffInput,
    ) -> Result<DiffPaths> {
        Err(Error::unsupported("diff_paths"))
    }
    async fn diff_file_contents(
        &self,
        _context: &Context,
        _input: &PullRequestDiffFileContentsInput,
    ) -> Result<PullRequestDiffFileContentsResult> {
        Err(Error::unsupported("diff_file_contents"))
    }
    async fn thread_comments(
        &self,
        _context: &Context,
        _input: &PullRequestThreadCommentsInput,
    ) -> Result<PullRequestThreadCommentsResult> {
        Err(Error::unsupported("thread_comments"))
    }
    async fn file_revisions(&self, _context: &Context, _paths: &[String]) -> Result<FileRevisions> {
        Err(Error::unsupported("file_revisions"))
    }
    async fn list_stats(
        &self,
        _context: &Context,
        _refs: &[PullRequestRef],
    ) -> Result<Vec<PullRequestDiffStat>> {
        Err(Error::unsupported("list_stats"))
    }
    async fn filter_options(
        &self,
        _context: &Context,
        _input: &PullRequestFilterOptionsInput,
    ) -> Result<PullRequestFilterOptions> {
        Err(Error::unsupported("filter_options"))
    }
    async fn reviewer_candidates(
        &self,
        _context: &Context,
        _input: &PullRequestReviewerCandidatesInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        Err(Error::unsupported("reviewer_candidates"))
    }
    async fn update(&self, _context: &Context, _input: &PullRequestUpdateInput) -> Result<()> {
        Err(Error::unsupported("update"))
    }
    async fn comment(&self, _context: &Context, _input: &PullRequestCommentInput) -> Result<()> {
        Err(Error::unsupported("comment"))
    }
    async fn update_comment(
        &self,
        _context: &Context,
        _input: &PullRequestCommentUpdateInput,
    ) -> Result<()> {
        Err(Error::unsupported("update_comment"))
    }
    async fn submit_review(
        &self,
        _context: &Context,
        _input: &PullRequestSubmitReviewInput,
    ) -> Result<()> {
        Err(Error::unsupported("submit_review"))
    }
    async fn reply_to_thread(
        &self,
        _context: &Context,
        _input: &PullRequestThreadReplyInput,
    ) -> Result<()> {
        Err(Error::unsupported("reply_to_thread"))
    }
    async fn set_thread_resolution(
        &self,
        _context: &Context,
        _input: &PullRequestThreadResolutionInput,
    ) -> Result<()> {
        Err(Error::unsupported("set_thread_resolution"))
    }
    async fn set_reaction(
        &self,
        _context: &Context,
        _input: &PullRequestReactionInput,
    ) -> Result<()> {
        Err(Error::unsupported("set_reaction"))
    }
    async fn request_reviewers(
        &self,
        _context: &Context,
        _input: &PullRequestReviewerRequestInput,
    ) -> Result<()> {
        Err(Error::unsupported("request_reviewers"))
    }
    async fn set_labels(
        &self,
        _context: &Context,
        _input: &PullRequestLabelChangeInput,
    ) -> Result<()> {
        Err(Error::unsupported("set_labels"))
    }
    async fn set_files_viewed(
        &self,
        _context: &Context,
        _input: &PullRequestSetFilesViewedInput,
    ) -> Result<()> {
        Err(Error::unsupported("set_files_viewed"))
    }
    async fn run_action(
        &self,
        _context: &Context,
        _input: &PullRequestActionInput,
    ) -> Result<Option<PullRequestSummary>> {
        Err(Error::unsupported("runAction"))
    }
}
