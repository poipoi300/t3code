use super::{decode as d, queries as q, write::WriteGuard, *};
use crate::Forge;
use async_trait::async_trait;
use std::collections::BTreeMap;
#[async_trait]
impl Forge for GitLab {
    async fn shutdown(&self) {
        self.stop.cancel();
        self.tasks.wait().await;
    }
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("gitlab".into())
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.capabilities
    }
    fn settings_revision(&self) -> u64 {
        self.revision
    }
    fn supports(&self, operation: &str) -> bool {
        SUPPORTED.contains(&operation)
    }
    fn merge_message_rewrite(&self, message: &str) -> String {
        crate::github::merge_message::remove_agent_credits(message)
    }
    fn unavailable_requirement(&self) -> Option<UnavailableRequirement> {
        Some(UnavailableRequirement {
            missing: "Install GitLab CLI (`glab`) from https://gitlab.com/gitlab-org/cli.".into(),
            unauthenticated: "GitLab CLI is not authenticated. Run `glab auth login` and retry."
                .into(),
        })
    }
    async fn discover(
        &self,
        cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        self.discover_glab(&cwd).await
    }
    async fn refine_unknown_remote(
        &self,
        input: CheckoutInput,
    ) -> Result<Option<SourceControlProviderInfo>> {
        self.refine_unknown(input).await
    }
    async fn refine_repository_identity(
        &self,
        identity: RepositoryIdentity,
    ) -> Result<RepositoryIdentity> {
        self.refine_identity(identity).await
    }
    async fn checkout_list_change_requests(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        self.checkout_list(input).await
    }
    async fn checkout_get_change_request(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        self.checkout_get(input).await
    }
    async fn create_change_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        self.create_request(input).await
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
        self.default_branch(input).await
    }
    async fn checkout_change_request(&self, input: CheckoutGetInput) -> Result<()> {
        self.checkout_request(input).await
    }
    async fn read_change_request_template(&self, input: TemplateInput) -> Result<Option<String>> {
        self.template(input).await
    }
    async fn resolve_link(&self, input: LinkInput) -> Result<Option<LinkSubject>> {
        if input.url.host_str() != Some("gitlab.com") || input.url.port().is_some() {
            return Ok(None);
        }
        let Some((repository, tail)) = input.url.path().trim_matches('/').split_once("/-/") else {
            return Ok(None);
        };
        let mut parts = tail.split('/');
        let kind = parts.next().unwrap_or("");
        let number = parts.next().unwrap_or("");
        if !matches!(kind, "merge_requests" | "issues")
            || number.parse::<u64>().ok().is_none_or(|n| n == 0)
        {
            return Ok(None);
        }
        let repo = ProviderRepositoryRef {
            cwd: input.cwd.to_string_lossy().into(),
            host: "gitlab.com".into(),
            repository: repository.into(),
        };
        let raw = self
            .api(
                &repo,
                "resolveLink",
                "GET",
                &format!("{}/{kind}/{number}", project(repository)?),
                None,
            )
            .await?;
        let title = text(&raw, "title")
            .ok_or_else(|| error("resolveLink", "Invalid linked issue or merge request."))?;
        Ok(Some(LinkSubject {
            title: title.into(),
            body: text(&raw, "description").map(str::to_string),
        }))
    }
    async fn get_viewer(&self, input: ViewerInput) -> Result<String> {
        let repo = ProviderRepositoryRef {
            cwd: input.cwd.to_string_lossy().into(),
            host: input.host.unwrap_or_else(|| "gitlab.com".into()),
            repository: String::new(),
        };
        let raw = self
            .api(&repo, "getViewerUsername", "GET", "user", None)
            .await?;
        text(&raw, "username").map(str::to_string).ok_or_else(|| {
            error(
                "getViewerUsername",
                "GitLab returned no username for the authenticated account.",
            )
        })
    }
    async fn list_change_requests(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        self.list_requests(input).await
    }
    async fn list_change_request_stats(
        &self,
        input: StatsInput,
    ) -> Result<Vec<ProviderChangeRequestStat>> {
        let mut groups: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for request in &input.change_requests {
            groups
                .entry(request.repository.clone())
                .or_default()
                .push(request.number);
        }
        let mut found = BTreeMap::new();
        for (repository, numbers) in groups {
            for chunk in numbers.chunks(100) {
                let repo = ProviderRepositoryRef {
                    cwd: input.cwd.to_string_lossy().into(),
                    host: input.host.clone(),
                    repository: repository.clone(),
                };
                let raw=match self.graphql(&repo,"listMergeRequestStats",q::MERGE_REQUEST_STATS_GRAPHQL_QUERY,json!({"fullPath":repository,"iids":chunk.iter().map(u64::to_string).collect::<Vec<_>>()})).await{Ok(raw)=>raw,Err(e)if !matches!(e.reason(),Some(FailureReason::MissingTool|FailureReason::Unauthenticated|FailureReason::RateLimited))=>continue,Err(e)=>return Err(e)};
                for row in nodes(&raw["project"]["mergeRequests"]) {
                    if let (Some(number), Some(additions), Some(deletions)) = (
                        text(row, "iid").and_then(|s| s.parse::<u64>().ok()),
                        row["diffStatsSummary"]["additions"].as_u64(),
                        row["diffStatsSummary"]["deletions"].as_u64(),
                    ) {
                        found.insert((repository.clone(), number), (additions, deletions));
                    }
                }
            }
        }
        Ok(input
            .change_requests
            .into_iter()
            .filter_map(|key| {
                found
                    .get(&(key.repository.clone(), key.number))
                    .map(|(additions, deletions)| ProviderChangeRequestStat {
                        repository: key.repository,
                        number: key.number,
                        additions: *additions,
                        deletions: *deletions,
                    })
            })
            .collect())
    }
    async fn get_change_request(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        parse(self.read_detail(&input, true).await?, "getChangeRequest")
    }
    async fn get_change_request_checks(&self, input: PullRequestInput) -> Result<CheckResult> {
        parse(
            self.read_detail(&input, true).await?,
            "getChangeRequestChecks",
        )
    }
    async fn get_change_request_preview(&self, input: PullRequestInput) -> Result<ProviderPreview> {
        parse(
            d::listing(&self.raw_detail(&input).await?)?,
            "getChangeRequestPreview",
        )
    }
    async fn get_change_request_summary(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        let raw = self.raw_detail(&input).await?;
        let mut value = d::detail(&raw, Value::Null, Value::Null, vec![])?;
        if let Some(head) = text(&raw, "sha") {
            value["headSha"] = json!(head);
        }
        super::read::optional(self.add_stats(&input, &mut value).await, ())?;
        super::read::optional(self.add_states(&input, &mut value).await, ())?;
        parse(value, "getChangeRequestSummary")
    }
    async fn get_viewer_permissions(
        &self,
        input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        let raw = self.raw_detail(&input.pr).await?;
        parse(
            self.permission_values(&input.pr, &raw).await?,
            "getViewerPermissions",
        )
    }
    async fn get_change_request_activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        self.activity(input).await
    }
    async fn get_diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        self.diff(input).await
    }
    async fn get_diff_file_contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        self.file_contents(input).await
    }
    async fn get_file_revisions(&self, input: FileRevisionsInput) -> Result<ProviderFileRevisions> {
        self.file_revisions(input).await
    }
    async fn get_file_stats(&self, input: PullRequestInput) -> Result<ProviderFileStats> {
        let raw = self
            .graphql(
                &input.repo,
                "getMergeRequestFileStats",
                q::MERGE_REQUEST_FILE_STATS_GRAPHQL_QUERY,
                Self::vars(&input),
            )
            .await?;
        let request = &raw["project"]["mergeRequest"];
        let files: Vec<_> = rows(&request["diffStats"])
            .map(
                |v| json!({"path":v["path"],"additions":v["additions"],"deletions":v["deletions"]}),
            )
            .collect();
        let truncated = request["diffStatsSummary"]["fileCount"]
            .as_u64()
            .is_none_or(|n| files.len() < n as usize);
        parse(json!({"files":files,"truncated":truncated}), "getFileStats")
    }
    async fn run_action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        self.action(input).await
    }
    async fn update_change_request(&self, input: UpdateInput) -> Result<()> {
        self.update(input).await
    }
    async fn comment(&self, input: CommentInput) -> Result<()> {
        let _guard = WriteGuard::new(self);
        self.api(
            &input.pr.repo,
            "commentOnMergeRequest",
            "POST",
            &format!("{}/notes", mr(&input.pr)?),
            Some(json!({"body":input.body})),
        )
        .await?;
        Ok(())
    }
    async fn update_comment(&self, input: UpdateCommentInput) -> Result<()> {
        let _guard = WriteGuard::new(self);
        self.api(
            &input.pr.repo,
            "updateNote",
            "PUT",
            &format!("{}/notes/{}", mr(&input.pr)?, encode(&input.comment_id)),
            Some(json!({"body":input.body})),
        )
        .await?;
        Ok(())
    }
    async fn submit_review(&self, input: ReviewInput) -> Result<()> {
        self.review(input).await
    }
    async fn reply_to_thread(&self, input: ThreadReplyInput) -> Result<()> {
        let _guard = WriteGuard::new(self);
        self.api(
            &input.pr.repo,
            "replyToDiscussion",
            "POST",
            &format!(
                "{}/discussions/{}/notes",
                mr(&input.pr)?,
                encode(&input.thread_id)
            ),
            Some(json!({"body":input.body})),
        )
        .await?;
        Ok(())
    }
    async fn set_thread_resolution(&self, input: ThreadResolutionInput) -> Result<()> {
        let _guard = WriteGuard::new(self);
        self.api(
            &input.pr.repo,
            "setDiscussionResolution",
            "PUT",
            &format!(
                "{}/discussions/{}",
                mr(&input.pr)?,
                encode(&input.thread_id)
            ),
            Some(json!({"resolved":input.resolved})),
        )
        .await?;
        Ok(())
    }
    async fn set_reaction(&self, input: ReactionInput) -> Result<()> {
        self.react(input).await
    }
    async fn list_reviewer_candidates(
        &self,
        input: CandidateInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        self.reviewers(input).await
    }
    async fn set_reviewer_request(&self, input: ReviewerRequestInput) -> Result<()> {
        self.set_reviewers(input).await
    }
    async fn list_label_candidates(
        &self,
        input: PullRequestInput,
    ) -> Result<PullRequestLabelCandidateList> {
        self.label_candidates(input).await
    }
    async fn set_labels(&self, input: LabelsInput) -> Result<()> {
        self.set_project_labels(input).await
    }
    async fn list_repository_labels(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestLabel>> {
        let raw = self
            .project_labels(&input.repo, input.query.as_deref())
            .await?;
        let labels: Vec<_> = rows(&raw)
            .filter(|r| r["archived"] != true)
            .filter_map(|r| {
                text(r, "name").map(|name| json!({"name":name,"color":text(r,"color")}))
            })
            .collect();
        parse(
            json!({"items":labels,"truncated":rows(&raw).count()>=100}),
            "listRepositoryLabels",
        )
    }
    async fn search_repository_authors(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestActor>> {
        let raw = self.users(&input).await?;
        parse(
            json!({"items":rows(&raw).map(d::actor).filter(|a|!a.is_null()).collect::<Vec<_>>(),"truncated":rows(&raw).count()>=100}),
            "searchRepositoryAuthors",
        )
    }
    async fn search_repository_references(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestReferenceCandidate>> {
        self.references(input).await
    }
    async fn forget_change_request_reads(&self, scope: Option<ReadScope>) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(scope) = scope {
            self.details.lock().await.retain(|key, _| {
                !key.contains(&format!("\0{}\0", scope.repository))
                    || scope
                        .number
                        .is_some_and(|n| !key.ends_with(&format!("\0{n}")))
            });
        } else {
            self.details.lock().await.clear();
        }
    }
}
const SUPPORTED: &[&str] = &[
    "discover",
    "refine_unknown_remote",
    "refine_repository_identity",
    "checkout_list_change_requests",
    "checkout_get_change_request",
    "checkout_change_request",
    "create_change_request",
    "get_repository_clone_urls",
    "create_repository",
    "get_default_branch",
    "read_change_request_template",
    "resolve_link",
    "get_viewer",
    "list_change_requests",
    "list_change_request_stats",
    "get_change_request",
    "get_change_request_checks",
    "get_change_request_preview",
    "get_change_request_summary",
    "get_viewer_permissions",
    "get_change_request_activity",
    "get_diff",
    "get_diff_file_contents",
    "get_file_revisions",
    "get_file_stats",
    "run_action",
    "update_change_request",
    "comment",
    "update_comment",
    "submit_review",
    "reply_to_thread",
    "set_thread_resolution",
    "set_reaction",
    "list_reviewer_candidates",
    "set_reviewer_request",
    "list_label_candidates",
    "set_labels",
    "list_repository_labels",
    "search_repository_authors",
    "search_repository_references",
    "forget_change_request_reads",
];
