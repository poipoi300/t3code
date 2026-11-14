use super::{decode as d, *};
use async_trait::async_trait;
#[async_trait]
impl Forge for CursorOrigin {
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("cursor-origin".into())
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.capabilities
    }
    fn supports(&self, operation: &str) -> bool {
        SUPPORTED.contains(&operation)
    }
    fn settings_revision(&self) -> u64 {
        self.transport.revision()
    }
    fn reads_public_host_without_checkout(&self) -> bool {
        true
    }
    async fn shutdown(&self) {
        self.transport.shutdown();
    }
    fn unavailable_requirement(&self) -> Option<UnavailableRequirement> {
        Some(UnavailableRequirement {
            missing: "Install Cursor Origin CLI (`origin`).".into(),
            unauthenticated: "Run origin auth login.".into(),
        })
    }
    async fn discover(
        &self,
        cwd: PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        self.discover_cli(cwd).await
    }
    async fn refine_repository_identity(
        &self,
        mut identity: RepositoryIdentity,
    ) -> Result<RepositoryIdentity> {
        if identity
            .provider
            .as_ref()
            .is_some_and(|p| p.0 == "cursor-origin")
            && let Some(path) = crate::remote::parse_remote(&identity.locator.remote_url.0)
                .and_then(|r| r.repository)
        {
            identity.web_url = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(format!(
                "https://cursor.com/codebase/{path}"
            )));
        }
        Ok(identity)
    }
    async fn get_viewer(&self, input: ViewerInput) -> Result<String> {
        self.viewer(&input.cwd).await
    }
    async fn checkout_list_change_requests(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        self.checkout_list(input).await
    }
    async fn checkout_get_change_request(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        d::checkout(&self.checkout_get(&input).await?.1)
    }
    async fn checkout_change_request(&self, input: CheckoutGetInput) -> Result<()> {
        self.checkout_request(input).await
    }
    async fn create_change_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        let mut repo = self.checkout_repo(&input.checkout).await?;
        if let Some(target) = &input.target
            && let Some(repository) = &target.repository
        {
            repo.repository = repository.clone();
        }
        let body = tokio::fs::read_to_string(input.body_file)
            .await
            .map_err(|_| {
                error(
                    "createChangeRequest",
                    "Could not read the authorized body file.",
                )
            })?;
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let head = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(input.head_selector.trim());
        let base = input
            .target
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(&input.base_ref_name);
        self.api(
            &repo,
            "createChangeRequest",
            &format!("{}/pulls", repo_path(&repo.repository)),
            Method::POST,
            Some(json!({"title":input.title,"body":body,"head":head,"base":base,"draft":false})),
        )
        .await
        .map(|_| ())
    }
    async fn get_repository_clone_urls(
        &self,
        input: CloneUrlsInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        self.clone_urls(input).await
    }
    async fn create_repository(
        &self,
        input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        self.create_repo(input).await
    }
    async fn get_default_branch(&self, input: CheckoutInput) -> Result<Option<String>> {
        let repo = self.checkout_repo(&input).await?;
        let raw = self
            .api(
                &repo,
                "getDefaultBranch",
                &repo_path(&repo.repository),
                Method::GET,
                None,
            )
            .await?;
        Ok(text(&raw, "defaultBranch").map(str::to_string))
    }
    async fn list_change_requests(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        self.list(input).await
    }
    async fn get_change_request_summary(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        d::summary(&self.raw(&input).await?, &*self.avatars.lock().await)
    }
    async fn get_change_request_checks(&self, input: PullRequestInput) -> Result<CheckResult> {
        let raw = self.raw(&input).await?;
        let (checks, _) = self.checks(&input, &raw).await?;
        parse(
            json!({"state":d::state(&raw),"checks":checks}),
            "getChangeRequestChecks",
        )
    }
    async fn get_change_request(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        self.detail(&input).await
    }
    async fn get_change_request_activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        self.activity(&input).await
    }
    async fn get_viewer_permissions(
        &self,
        _input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        parse(permissions(), "getViewerPermissions")
    }
    async fn get_diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        self.diff(&input).await
    }
    async fn run_action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        let path = pull_path(&input.pr);
        let (path, method, body) = match input.action {
            PullRequestAction::Merge => {
                let raw = self.raw(&input.pr).await?;
                if let Some(parent) = d::number(&raw["stack"]["parentPullRequest"]["number"]) {
                    let raw = self
                        .raw(&PullRequestInput {
                            repo: input.pr.repo.clone(),
                            number: parent,
                        })
                        .await?;
                    if d::state(&raw) == "open" {
                        return Err(error(
                            "runAction",
                            &format!(
                                "This pull request is stacked on #{parent}. Merge that parent first, or merge the stack on Cursor Origin."
                            ),
                        ));
                    }
                }
                (
                    format!("{path}/merge"),
                    Method::POST,
                    json!({"mergeMethod":if input.merge_method==Some(PullRequestMergeMethod::Squash){"squash"}else{"merge"}}),
                )
            }
            PullRequestAction::Close => (path, Method::PATCH, json!({"state":"closed"})),
            PullRequestAction::Reopen => (path, Method::PATCH, json!({"state":"open"})),
            PullRequestAction::Ready => (path, Method::PATCH, json!({"draft":false})),
            PullRequestAction::Draft => (path, Method::PATCH, json!({"draft":true})),
            _ => return Err(ForgeError::unsupported(&self.kind(), "run_action")),
        };
        self.write(&input.pr, "runAction", &path, method, body)
            .await?;
        Ok(None)
    }
    async fn update_change_request(&self, input: UpdateInput) -> Result<()> {
        let mut body = json!({});
        if let Some(title) = input.title {
            body["title"] = json!(title);
        }
        if let Some(text) = input.body {
            body["body"] = json!(text);
        }
        self.write(
            &input.pr,
            "updateChangeRequest",
            &pull_path(&input.pr),
            Method::PATCH,
            body,
        )
        .await
    }
    async fn comment(&self, input: CommentInput) -> Result<()> {
        self.write(
            &input.pr,
            "comment",
            &format!("{}/comments", pull_path(&input.pr)),
            Method::POST,
            json!({"body":input.body}),
        )
        .await
    }
    async fn update_comment(&self, input: UpdateCommentInput) -> Result<()> {
        self.write(
            &input.pr,
            "updateComment",
            &format!(
                "{}/pulls/comments/{}",
                repo_path(&input.pr.repo.repository),
                encode(&input.comment_id)
            ),
            Method::PATCH,
            json!({"body":input.body}),
        )
        .await
    }
    async fn submit_review(&self, input: ReviewInput) -> Result<()> {
        let comments=input.comments.iter().map(|c|{
            let p=serde_json::to_value(&c.position).map_err(|_|error("submitReview","Invalid review position."))?;
            let side=match p["kind"].as_str(){Some("deleted")=>"left",Some("added")=>"right",_=>p["side"].as_str().unwrap_or("right")};let line=if side=="left"{&p["oldLine"]}else{&p["newLine"]};
            Ok(json!({"body":c.body,"inline":{"path":c.path,"side":side,"startLine":line,"endLine":line}}))
        }).collect::<Result<Vec<_>>>()?;
        let verdict = serde_json::to_value(input.verdict)
            .map_err(|_| error("submitReview", "Invalid verdict."))?;
        let verdict = if verdict == "request-changes" {
            "request_changes"
        } else {
            verdict.as_str().unwrap_or("comment")
        };
        self.write(
            &input.pr,
            "submitReview",
            &format!("{}/reviews", pull_path(&input.pr)),
            Method::POST,
            json!({"verdict":verdict,"body":input.body,"comments":comments}),
        )
        .await
    }
    async fn reply_to_thread(&self, input: ThreadReplyInput) -> Result<()> {
        self.write(
            &input.pr,
            "replyToThread",
            &format!("{}/comments", pull_path(&input.pr)),
            Method::POST,
            json!({"body":input.body,"threadId":input.thread_id}),
        )
        .await
    }
    async fn set_thread_resolution(&self, input: ThreadResolutionInput) -> Result<()> {
        self.write(
            &input.pr,
            "setThreadResolution",
            &format!(
                "{}/pulls/threads/{}",
                repo_path(&input.pr.repo.repository),
                encode(&input.thread_id)
            ),
            Method::PATCH,
            json!({"resolved":input.resolved}),
        )
        .await
    }
    async fn forget_change_request_reads(&self, _scope: Option<ReadScope>) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.cache.lock().await.clear();
    }
}
const SUPPORTED: &[&str] = &[
    "discover",
    "refine_repository_identity",
    "get_viewer",
    "checkout_list_change_requests",
    "checkout_get_change_request",
    "checkout_change_request",
    "create_change_request",
    "get_repository_clone_urls",
    "create_repository",
    "get_default_branch",
    "list_change_requests",
    "get_change_request_summary",
    "get_change_request_checks",
    "get_change_request",
    "get_change_request_activity",
    "get_viewer_permissions",
    "get_diff",
    "run_action",
    "update_change_request",
    "comment",
    "update_comment",
    "submit_review",
    "reply_to_thread",
    "set_thread_resolution",
    "forget_change_request_reads",
];
