use crate::{
    error::{ForgeError, Result},
    model::*,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutContext {
    pub provider: SourceControlProviderInfo,
    pub remote_name: String,
    pub remote_url: String,
    pub requested_host: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefSelector {
    pub ref_name: String,
    pub owner: Option<String>,
    pub repository: Option<String>,
}
#[derive(Debug, Clone)]
pub struct CheckoutInput {
    pub cwd: PathBuf,
    pub context: Option<CheckoutContext>,
}
#[derive(Debug, Clone)]
pub struct CheckoutListInput {
    pub checkout: CheckoutInput,
    pub source: Option<RefSelector>,
    pub head_selector: String,
    /// None lists every state, including merged and closed requests.
    pub state: Option<ChangeRequestState>,
    pub limit: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct CheckoutGetInput {
    pub checkout: CheckoutInput,
    pub reference: String,
    pub force: bool,
}
#[derive(Debug, Clone)]
pub struct CreateChangeRequestInput {
    pub checkout: CheckoutInput,
    pub source: Option<RefSelector>,
    pub target: Option<RefSelector>,
    pub base_ref_name: String,
    pub head_selector: String,
    pub title: String,
    pub body_file: PathBuf,
}
#[derive(Debug, Clone)]
pub struct CloneUrlsInput {
    pub checkout: CheckoutInput,
    pub repository: String,
    pub host: Option<String>,
}
#[derive(Debug, Clone)]
pub struct CreateRepositoryInput {
    pub cwd: PathBuf,
    pub repository: String,
    pub visibility: SourceControlRepositoryVisibility,
}
#[derive(Debug, Clone)]
pub struct ViewerInput {
    pub cwd: PathBuf,
    pub host: Option<String>,
}
#[derive(Debug, Clone)]
pub struct RoutingIdentity {
    pub account_id: String,
    pub viewer: String,
    pub credential_fingerprint: String,
}
#[derive(Debug, Clone)]
pub struct PullRequestInput {
    pub repo: ProviderRepositoryRef,
    pub number: u64,
}
#[derive(Debug, Clone)]
pub struct ListInput {
    pub repo: ProviderRepositoryRef,
    pub state: PullRequestListState,
    pub involvement: PullRequestInvolvement,
    pub viewer: String,
    pub limit: usize,
    pub query: Option<String>,
    pub cursor: Option<ProviderListCursor>,
    pub filters: Option<PullRequestListFilters>,
}
#[derive(Debug, Clone)]
pub struct ListAcrossInput {
    pub list: ListInput,
    pub repositories: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct StatsInput {
    pub cwd: PathBuf,
    pub host: String,
    pub change_requests: Vec<ChangeRequestKey>,
}
#[derive(Debug, Clone)]
pub struct ChangeRequestKey {
    pub repository: String,
    pub number: u64,
}
#[derive(Debug, Clone)]
pub struct ReadScope {
    pub repository: String,
    pub number: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct StackInput {
    pub pr: PullRequestInput,
    pub include_details: bool,
}
#[derive(Debug, Clone)]
pub struct ThreadCommentsInput {
    pub pr: PullRequestInput,
    pub thread_id: String,
    pub cursor: String,
}
#[derive(Debug, Clone)]
pub struct PermissionsInput {
    pub pr: PullRequestInput,
    pub include_update_branch: bool,
}
#[derive(Debug, Clone)]
pub struct DiffInput {
    pub pr: PullRequestInput,
    pub cursor: Option<String>,
    pub commit: Option<String>,
}
#[derive(Debug, Clone)]
pub struct FileContentsInput {
    pub pr: PullRequestInput,
    pub commit: Option<String>,
    pub change_type: PullRequestDiffFileContentsInputChangeType,
    pub old_path: String,
    pub new_path: String,
}
#[derive(Debug, Clone)]
pub struct ViewedFile {
    pub path: String,
    pub viewed: bool,
}
#[derive(Debug, Clone)]
pub struct SetFilesViewedInput {
    pub pr: PullRequestInput,
    pub files: Vec<ViewedFile>,
}
#[derive(Debug, Clone)]
pub struct FileRevisionsInput {
    pub pr: PullRequestInput,
    pub paths: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct ActionInput {
    pub pr: PullRequestInput,
    pub action: PullRequestAction,
    pub stack_number: Option<u64>,
    pub expected_stack_heads: Option<Vec<PullRequestStackHead>>,
    pub remove_agent_credits_on_merge: bool,
    pub merge_method: Option<PullRequestMergeMethod>,
    pub update_method: Option<PullRequestUpdateMethod>,
}
#[derive(Debug, Clone)]
pub struct UpdateInput {
    pub pr: PullRequestInput,
    pub maintainer_can_modify: Option<bool>,
    pub title: Option<String>,
    pub body: Option<String>,
}
#[derive(Debug, Clone)]
pub struct CommentInput {
    pub pr: PullRequestInput,
    pub body: String,
}
#[derive(Debug, Clone)]
pub struct UpdateCommentInput {
    pub pr: PullRequestInput,
    pub comment_id: String,
    pub kind: PullRequestCommentUpdateInputKind,
    pub body: String,
}
#[derive(Debug, Clone)]
pub struct ReviewInput {
    pub pr: PullRequestInput,
    pub verdict: PullRequestReviewVerdict,
    pub body: String,
    pub comments: Vec<PullRequestReviewCommentDraft>,
}
#[derive(Debug, Clone)]
pub struct CandidateInput {
    pub pr: PullRequestInput,
    pub query: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ReviewerRequestInput {
    pub pr: PullRequestInput,
    pub reviewers: Vec<PullRequestReviewerRequestInputReviewers>,
    pub requested: bool,
}
#[derive(Debug, Clone)]
pub struct RepositoryQueryInput {
    pub repo: ProviderRepositoryRef,
    pub query: Option<String>,
    pub sigil: Option<PullRequestReferenceSigil>,
}
#[derive(Debug, Clone)]
pub struct LabelsInput {
    pub pr: PullRequestInput,
    pub labels: Vec<String>,
    pub applied: bool,
}
#[derive(Debug, Clone)]
pub struct ThreadReplyInput {
    pub pr: PullRequestInput,
    pub thread_id: String,
    pub body: String,
}
#[derive(Debug, Clone)]
pub struct ReactionInput {
    pub pr: PullRequestInput,
    pub subject_id: Option<String>,
    pub content: PullRequestReactionContent,
    pub reacted: bool,
}
#[derive(Debug, Clone)]
pub struct ThreadResolutionInput {
    pub pr: PullRequestInput,
    pub thread_id: String,
    pub resolved: bool,
}
#[derive(Debug, Clone)]
pub struct LinkSubject {
    pub title: String,
    pub body: Option<String>,
}
#[derive(Debug, Clone)]
pub struct BranchProbe {
    pub head_selectors: Vec<String>,
    pub limit: usize,
}
#[derive(Debug, Clone)]
pub struct TemplateInput {
    pub cwd: PathBuf,
    pub treeish: String,
}
#[derive(Debug, Clone)]
pub struct RecordBranchBaseInput {
    pub cwd: PathBuf,
    pub branch: String,
    pub base_branch: String,
}
#[derive(Debug, Clone)]
pub struct LinkInput {
    pub cwd: PathBuf,
    pub url: url::Url,
}
#[derive(Debug, Clone)]
pub struct CreatesInInput {
    pub checkout: CheckoutInput,
    pub repository: Option<String>,
}
#[derive(Debug, Clone)]
pub struct MediaRequest {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub repository: Option<String>,
}
#[derive(Debug, Clone)]
pub struct MediaResponse {
    pub status: u16,
    pub headers: std::collections::BTreeMap<String, String>,
    pub body: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct UnavailableRequirement {
    pub missing: String,
    pub unauthenticated: String,
}
#[derive(Debug, Clone)]
pub struct VerifiedCredentialInput {
    pub cwd: PathBuf,
    pub host: String,
    pub repository: Option<String>,
}
pub struct VerifiedForge {
    pub identity: RoutingIdentity,
    pub forge: std::sync::Arc<dyn Forge>,
}
pub type ChangeRequestEvents = std::pin::Pin<Box<dyn futures_util::Stream<Item = ()> + Send>>;

/// Optional TS methods return Unsupported unless an adapter explicitly implements them.
#[async_trait]
pub trait Forge: Send + Sync {
    async fn shutdown(&self) {}
    fn settings_revision(&self) -> u64 {
        0
    }
    async fn refine_unknown_remote(
        &self,
        _input: CheckoutInput,
    ) -> Result<Option<SourceControlProviderInfo>> {
        Ok(None)
    }
    fn kind(&self) -> SourceControlProviderKind;
    fn capabilities(&self) -> &PullRequestCapabilities;
    fn supports(&self, operation: &str) -> bool;
    fn reads_public_host_without_checkout(&self) -> bool {
        false
    }
    fn batches_reads(&self) -> bool {
        false
    }
    fn serves_host(&self, _host: &str) -> bool {
        true
    }
    fn repository_key(&self, canonical_key: &str) -> String {
        canonical_key.into()
    }
    fn merge_message_rewrite(&self, message: &str) -> String {
        message.into()
    }
    fn head_branch_probe(
        &self,
        head_selectors: &[String],
        default_branch_head_selector: Option<&str>,
    ) -> BranchProbe {
        BranchProbe {
            head_selectors: default_branch_head_selector
                .map(|s| vec![s.into()])
                .unwrap_or_else(|| head_selectors.to_vec()),
            limit: 100,
        }
    }
    async fn checkout_list_change_requests(
        &self,
        _input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "checkout_list_change_requests",
        ))
    }
    async fn checkout_get_change_request(&self, _input: CheckoutGetInput) -> Result<ChangeRequest> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "checkout_get_change_request",
        ))
    }
    async fn create_change_request(&self, _input: CreateChangeRequestInput) -> Result<()> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "create_change_request",
        ))
    }
    async fn get_repository_clone_urls(
        &self,
        _input: CloneUrlsInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_repository_clone_urls",
        ))
    }
    async fn create_repository(
        &self,
        _input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        Err(ForgeError::unsupported(&self.kind(), "create_repository"))
    }
    async fn get_default_branch(&self, _input: CheckoutInput) -> Result<Option<String>> {
        Err(ForgeError::unsupported(&self.kind(), "get_default_branch"))
    }
    async fn checkout_change_request(&self, _input: CheckoutGetInput) -> Result<()> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "checkout_change_request",
        ))
    }
    async fn get_viewer(&self, _input: ViewerInput) -> Result<String> {
        Err(ForgeError::unsupported(&self.kind(), "get_viewer"))
    }
    async fn with_verified_credential(
        &self,
        _input: VerifiedCredentialInput,
    ) -> Result<VerifiedForge> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "with_verified_credential",
        ))
    }
    async fn get_routing_identity(&self, _input: ViewerInput) -> Result<RoutingIdentity> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_routing_identity",
        ))
    }
    async fn list_change_requests(&self, _input: ListInput) -> Result<ProviderChangeRequestPage> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "list_change_requests",
        ))
    }
    async fn list_change_requests_across(
        &self,
        _input: ListAcrossInput,
    ) -> Result<ProviderBatchedChangeRequestPage> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "list_change_requests_across",
        ))
    }
    async fn list_change_request_stats(
        &self,
        _input: StatsInput,
    ) -> Result<Vec<ProviderChangeRequestStat>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "list_change_request_stats",
        ))
    }
    async fn get_change_request_checks(&self, _input: PullRequestInput) -> Result<CheckResult> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_change_request_checks",
        ))
    }
    async fn get_change_request(
        &self,
        _input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        Err(ForgeError::unsupported(&self.kind(), "get_change_request"))
    }
    async fn get_change_request_preview(
        &self,
        _input: PullRequestInput,
    ) -> Result<ProviderPreview> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_change_request_preview",
        ))
    }
    async fn get_change_request_summary(
        &self,
        _input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_change_request_summary",
        ))
    }
    async fn get_change_request_watch_fingerprint(
        &self,
        _input: PullRequestInput,
    ) -> Result<Option<ProviderChangeRequestWatchFingerprint>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_change_request_watch_fingerprint",
        ))
    }
    async fn change_request_events(&self, _input: PullRequestInput) -> Result<ChangeRequestEvents> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "change_request_events",
        ))
    }
    async fn get_change_request_stack(
        &self,
        _input: StackInput,
    ) -> Result<Option<ProviderChangeRequestStack>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_change_request_stack",
        ))
    }
    async fn get_change_request_activity(
        &self,
        _input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_change_request_activity",
        ))
    }
    async fn get_review_thread_comments(
        &self,
        _input: ThreadCommentsInput,
    ) -> Result<PullRequestThreadCommentsResult> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_review_thread_comments",
        ))
    }
    async fn get_viewer_permissions(
        &self,
        _input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_viewer_permissions",
        ))
    }
    async fn get_diff(&self, _input: DiffInput) -> Result<ProviderDiffSlice> {
        Err(ForgeError::unsupported(&self.kind(), "get_diff"))
    }
    async fn get_diff_file_paths(&self, _input: DiffInput) -> Result<ProviderDiffFilePaths> {
        Err(ForgeError::unsupported(&self.kind(), "get_diff_file_paths"))
    }
    async fn get_diff_file_contents(
        &self,
        _input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "get_diff_file_contents",
        ))
    }
    async fn get_files_viewed(&self, _input: PullRequestInput) -> Result<ProviderFilesViewed> {
        Err(ForgeError::unsupported(&self.kind(), "get_files_viewed"))
    }
    async fn get_file_stats(&self, _input: PullRequestInput) -> Result<ProviderFileStats> {
        Err(ForgeError::unsupported(&self.kind(), "get_file_stats"))
    }
    async fn set_files_viewed(&self, _input: SetFilesViewedInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "set_files_viewed"))
    }
    async fn get_file_revisions(
        &self,
        _input: FileRevisionsInput,
    ) -> Result<ProviderFileRevisions> {
        Err(ForgeError::unsupported(&self.kind(), "get_file_revisions"))
    }
    async fn run_action(
        &self,
        _input: ActionInput,
    ) -> Result<Option<ProviderChangeRequestSummary>> {
        Err(ForgeError::unsupported(&self.kind(), "run_action"))
    }
    async fn update_change_request(&self, _input: UpdateInput) -> Result<()> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "update_change_request",
        ))
    }
    async fn comment(&self, _input: CommentInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "comment"))
    }
    async fn update_comment(&self, _input: UpdateCommentInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "update_comment"))
    }
    async fn submit_review(&self, _input: ReviewInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "submit_review"))
    }
    async fn list_reviewer_candidates(
        &self,
        _input: CandidateInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "list_reviewer_candidates",
        ))
    }
    async fn set_reviewer_request(&self, _input: ReviewerRequestInput) -> Result<()> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "set_reviewer_request",
        ))
    }
    async fn list_label_candidates(
        &self,
        _input: PullRequestInput,
    ) -> Result<PullRequestLabelCandidateList> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "list_label_candidates",
        ))
    }
    async fn list_repository_labels(
        &self,
        _input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestLabel>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "list_repository_labels",
        ))
    }
    async fn search_repository_authors(
        &self,
        _input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestActor>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "search_repository_authors",
        ))
    }
    async fn search_repository_references(
        &self,
        _input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestReferenceCandidate>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "search_repository_references",
        ))
    }
    async fn set_labels(&self, _input: LabelsInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "set_labels"))
    }
    async fn reply_to_thread(&self, _input: ThreadReplyInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "reply_to_thread"))
    }
    async fn set_reaction(&self, _input: ReactionInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "set_reaction"))
    }
    async fn set_thread_resolution(&self, _input: ThreadResolutionInput) -> Result<()> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "set_thread_resolution",
        ))
    }
    fn unavailable_requirement(&self) -> Option<UnavailableRequirement> {
        None
    }
    fn repository_name_from_remote_url(&self, url: &str) -> Option<String> {
        crate::remote::parse_remote(url)?.repository
    }
    async fn read_change_request_template(&self, _input: TemplateInput) -> Result<Option<String>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "read_change_request_template",
        ))
    }
    async fn refine_repository_identity(
        &self,
        _identity: RepositoryIdentity,
    ) -> Result<RepositoryIdentity> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "refine_repository_identity",
        ))
    }
    async fn resolve_remote_web_host(&self, _input: CheckoutInput) -> Result<Option<String>> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "resolve_remote_web_host",
        ))
    }
    async fn resolve_link(&self, _input: LinkInput) -> Result<Option<LinkSubject>> {
        Err(ForgeError::unsupported(&self.kind(), "resolve_link"))
    }
    async fn creates_change_requests_in(&self, _input: CreatesInInput) -> Result<bool> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "creates_change_requests_in",
        ))
    }
    async fn record_branch_base(&self, _input: RecordBranchBaseInput) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "record_branch_base"))
    }
    async fn discover(
        &self,
        _cwd: PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        Err(ForgeError::unsupported(&self.kind(), "discover"))
    }
    async fn fetch_media(&self, _input: MediaRequest) -> Result<Option<MediaResponse>> {
        Err(ForgeError::unsupported(&self.kind(), "fetch_media"))
    }
    async fn sign_in_start(
        &self,
        _host: String,
    ) -> Result<t3_contracts::source_control::SourceControlSignInPrompt> {
        Err(ForgeError::unsupported(&self.kind(), "sign_in_start"))
    }
    async fn sign_in_wait(
        &self,
        _host: String,
    ) -> Result<t3_contracts::source_control::SourceControlSignInResult> {
        Err(ForgeError::unsupported(&self.kind(), "sign_in_wait"))
    }
    async fn sign_in_cancel(&self, _host: String) -> Result<()> {
        Err(ForgeError::unsupported(&self.kind(), "sign_in_cancel"))
    }
    async fn sign_in_access(
        &self,
        _host: String,
        _repository: String,
    ) -> Result<t3_contracts::source_control::SourceControlAppAccessStatus> {
        Err(ForgeError::unsupported(&self.kind(), "sign_in_access"))
    }
    async fn is_repository_starred(&self, _host: String, _repository: String) -> Result<bool> {
        Err(ForgeError::unsupported(
            &self.kind(),
            "is_repository_starred",
        ))
    }
    async fn star_repository(&self, _host: String, _repository: String) -> Result<bool> {
        Err(ForgeError::unsupported(&self.kind(), "star_repository"))
    }
    async fn forget_change_request_reads(&self, _scope: Option<ReadScope>) {}
}
