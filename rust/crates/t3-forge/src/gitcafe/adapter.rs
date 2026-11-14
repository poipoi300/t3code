use super::{decode as d, *};
use async_trait::async_trait;
#[async_trait]
impl Forge for GitCafe {
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("gitcafe".into())
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.capabilities
    }
    fn supports(&self, operation: &str) -> bool {
        SUPPORTED.contains(&operation)
    }
    fn settings_revision(&self) -> u64 {
        self.credentials.revision()
    }
    fn serves_host(&self, host: &str) -> bool {
        valid_host(host)
    }
    async fn shutdown(&self) {
        self.stop.cancel();
        self.credentials.shutdown();
        for http in self.http.values() {
            http.shutdown();
        }
    }
    async fn discover(
        &self,
        cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        self.discover_cli(cwd).await
    }
    async fn get_viewer(&self, input: ViewerInput) -> Result<String> {
        let host = input.host.unwrap_or("git.cafe".into());
        let raw = self
            .api(&host, "getViewer", Method::GET, "/auth/principal", None)
            .await?;
        text(&raw, "handle").map(str::to_string).ok_or_else(|| {
            error(
                "getViewer",
                "GitCafe did not report the authenticated account.",
            )
        })
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
    async fn checkout_change_request(&self, input: CheckoutGetInput) -> Result<()> {
        self.checkout_request(input).await
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
    async fn create_change_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        self.create_request(input).await
    }
    async fn get_default_branch(&self, input: CheckoutInput) -> Result<Option<String>> {
        let target = self.target(&input, None).await?;
        let raw = self
            .api(
                &target.host,
                "getDefaultBranch",
                Method::GET,
                &repository(&target.repository)?,
                None,
            )
            .await?;
        Ok(text(&raw, "defaultBranch").map(str::to_string))
    }
    async fn list_change_requests(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        self.list(input).await
    }
    async fn get_change_request(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        self.detail(input).await
    }
    async fn get_change_request_preview(&self, input: PullRequestInput) -> Result<ProviderPreview> {
        let raw = self.raw(&input, "getPreview").await?;
        let mut value = d::listing(&raw, &input.repo)?;
        d::settled(&raw, &mut value);
        parse(value, "getPreview")
    }
    async fn get_change_request_summary(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        let raw = self.raw(&input, "getSummary").await?;
        let mut value = d::listing(&raw, &input.repo)?;
        if let Some(object) = value.as_object_mut() {
            object.remove("additions");
            object.remove("deletions");
        }
        if !raw["headOid"].is_null()
            && let Ok(stats) = self.line_stats(&input, &raw).await
            && let (Some(value), Some(stats)) = (value.as_object_mut(), stats.as_object())
        {
            value.extend(stats.clone());
        }
        value["closedAt"] = raw["closedAt"].clone();
        value["mergedAt"] = raw["mergedAt"].clone();
        d::settled(&raw, &mut value);
        parse(value, "getSummary")
    }
    async fn get_change_request_stack(
        &self,
        input: StackInput,
    ) -> Result<Option<ProviderChangeRequestStack>> {
        self.stack(&input.pr)
            .await?
            .map(|s| d::stack(&s, &input.pr.repo))
            .transpose()
    }
    async fn get_viewer_permissions(
        &self,
        input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        parse(
            d::permissions(&self.raw(&input.pr, "getViewerPermissions").await?),
            "getViewerPermissions",
        )
    }
    async fn get_change_request_activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        let pull = self.raw(&input, "getActivity").await?;
        let path = pull_path(&input)?;
        let comments_path = format!("{path}/comments");
        let reviews_path = format!("{path}/reviews");
        let commits_path = format!("{path}/commits");
        let reactions_path = format!("{path}/reactions/");
        let commits_read = async {
            if pull["headOid"].is_null() {
                Ok((vec![], None))
            } else {
                match self
                    .pages(&input.repo, "getActivityCommits", &commits_path)
                    .await
                {
                    Ok(value) => Ok(value),
                    Err(e) if e.reason() == Some(FailureReason::NotFound) => Ok((vec![], None)),
                    Err(e) => Err(e),
                }
            }
        };
        let (comments, reviews, commits, reactions) = tokio::join!(
            self.pages(&input.repo, "getActivityComments", &comments_path),
            self.pages(&input.repo, "getActivityReviews", &reviews_path),
            commits_read,
            self.pages(&input.repo, "getActivityReactions", &reactions_path)
        );
        let (comments, comments_next) = comments?;
        let (reviews, reviews_next) = reviews?;
        let (commits, _) = commits?;
        d::activity(
            &input.repo.host,
            text(&pull, "headOid"),
            (comments, comments_next.is_some()),
            (reviews, reviews_next.is_some()),
            commits,
            reactions.ok().map(|(r, _)| r),
        )
    }
    async fn get_diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        let (files, revision, next) = self.diff_page(&input, 64).await?;
        let paths = files
            .iter()
            .filter(|f| f["binary"] != true && f["tooLarge"] != true)
            .filter_map(|f| text(f, "path").map(str::to_string))
            .collect();
        let details = self.diff_files(&input.pr, &revision, paths).await?;
        let files: Vec<_> = files
            .into_iter()
            .map(|f| {
                details
                    .iter()
                    .find(|d| d["path"] == f["path"])
                    .cloned()
                    .unwrap_or(f)
            })
            .collect();
        let mut diff = d::diff(&files)?;
        diff.next_cursor = next;
        Ok(diff)
    }
    async fn get_diff_file_paths(&self, input: DiffInput) -> Result<ProviderDiffFilePaths> {
        let (files, _, next) = self.diff_page(&input, 500).await?;
        parse(
            json!({"files":files.iter().map(|f|json!({"oldPath":f["oldPath"].as_str().or_else(||f["path"].as_str()),"newPath":f["path"]})).collect::<Vec<_>>(),"nextCursor":next}),
            "getDiffFilePaths",
        )
    }
    async fn get_diff_file_contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        self.contents(input).await
    }
    async fn run_action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        self.action(input).await
    }
    async fn update_change_request(&self, input: UpdateInput) -> Result<()> {
        let raw = self.raw(&input.pr, "updateChangeRequest").await?;
        let mut body = json!({"expectedVersion":raw["version"]});
        if let Some(title) = input.title {
            body["title"] = json!(title);
        }
        if let Some(description) = input.body {
            body["description"] = json!(description);
        }
        self.api(
            &input.pr.repo.host,
            "updateChangeRequest",
            Method::PATCH,
            &pull_path(&input.pr)?,
            Some(body),
        )
        .await?;
        Ok(())
    }
    async fn comment(&self, input: CommentInput) -> Result<()> {
        self.api(
            &input.pr.repo.host,
            "comment",
            Method::POST,
            &format!("{}/comments/", pull_path(&input.pr)?),
            Some(json!({"body":input.body})),
        )
        .await?;
        Ok(())
    }
    async fn update_comment(&self, input: UpdateCommentInput) -> Result<()> {
        let path = format!(
            "{}/comments/{}",
            pull_path(&input.pr)?,
            id(&input.comment_id)?
        );
        let detail = self
            .api(
                &input.pr.repo.host,
                "updateComment",
                Method::GET,
                &format!("{path}/detail"),
                None,
            )
            .await?;
        let version = detail["version"]
            .as_u64()
            .ok_or_else(|| error("updateComment", "Missing comment version."))?;
        self.api(
            &input.pr.repo.host,
            "updateComment",
            Method::PATCH,
            &path,
            Some(json!({"body":input.body,"expectedVersion":version})),
        )
        .await?;
        Ok(())
    }
    async fn submit_review(&self, input: ReviewInput) -> Result<()> {
        if !input.comments.is_empty() {
            return Err(ForgeError::unsupported(
                &self.kind(),
                "inline_review_comments",
            ));
        }
        let pull = self.raw(&input.pr, "submitReview").await?;
        let head = text(&pull, "headOid")
            .ok_or_else(|| error("submitReview", "Missing pull request head."))?;
        let mut body = json!({"verdict":if input.verdict==PullRequestReviewVerdict::RequestChanges{json!("request_changes")}else{serde_json::to_value(input.verdict).map_err(|_|error("submitReview","Invalid verdict."))?},"commitOid":head,"expectedVersion":pull["version"],"requestId":request_id()});
        if !input.body.is_empty() {
            body["body"] = json!(input.body);
        }
        self.api(
            &input.pr.repo.host,
            "submitReview",
            Method::POST,
            &format!("{}/reviews", pull_path(&input.pr)?),
            Some(body),
        )
        .await?;
        Ok(())
    }
    async fn list_reviewer_candidates(
        &self,
        input: CandidateInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        let path = pull_path(&input.pr)?;
        let candidate_path = format!("{path}/reviewer-candidates");
        let reviewers_path = format!("{path}/reviewers");
        let (pull, candidates, reviewers) = tokio::try_join!(
            self.raw(&input.pr, "reviewerCandidates"),
            self.pages(&input.pr.repo, "reviewerCandidates", &candidate_path),
            self.pages(&input.pr.repo, "reviewerCandidates", &reviewers_path)
        )?;
        let truncated = candidates.1.is_some();
        let candidates = candidates
            .0
            .into_iter()
            .filter_map(|c| {
                if c["id"] == pull["author"]["actorId"] {
                    return None;
                }
                let mut actor = d::actor(&c["actor"], &input.pr.repo.host);
                if actor.is_null() {
                    return None;
                }
                actor["id"] = c["id"].clone();
                actor["kind"] = json!("user");
                actor["isRequested"] = json!(reviewers.0.iter().any(|r| r["id"] == c["id"]));
                Some(actor)
            })
            .collect::<Vec<_>>();
        parse(
            json!({"candidates":candidates,"truncated":truncated}),
            "reviewerCandidates",
        )
    }
    async fn set_reviewer_request(&self, input: ReviewerRequestInput) -> Result<()> {
        let ids = input
            .reviewers
            .into_iter()
            .map(|r| {
                serde_json::to_value(r).map_err(|_| error("setReviewers", "Invalid reviewer."))
            })
            .collect::<Result<Vec<_>>>()?;
        let ids = ids
            .iter()
            .filter_map(|r| text(r, "id").map(str::to_string))
            .collect();
        self.toggle(&input.pr, "reviewers", "actorIds", ids, input.requested)
            .await
    }
    async fn list_label_candidates(
        &self,
        input: PullRequestInput,
    ) -> Result<PullRequestLabelCandidateList> {
        let base = repository(&input.repo.repository)?;
        let label_path = format!("{base}/labels");
        let applied_path = format!("{}/labels", pull_path(&input)?);
        let ((labels, next), (applied, _)) = tokio::try_join!(
            self.pages(&input.repo, "labelCandidates", &label_path),
            self.pages(&input.repo, "labelCandidates", &applied_path)
        )?;
        parse(
            json!({"candidates":labels.iter().map(|l|json!({"name":l["name"],"color":l["color"],"description":l["description"],"isApplied":applied.iter().any(|a|a["id"]==l["id"])})).collect::<Vec<_>>(),"truncated":next.is_some()}),
            "labelCandidates",
        )
    }
    async fn list_repository_labels(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestLabel>> {
        let (labels, next) = self
            .pages(
                &input.repo,
                "repositoryLabels",
                &format!("{}/labels", repository(&input.repo.repository)?),
            )
            .await?;
        parse(
            json!({"items":labels.iter().map(|l|json!({"name":l["name"],"color":l["color"]})).collect::<Vec<_>>(),"truncated":next.is_some()}),
            "repositoryLabels",
        )
    }
    async fn search_repository_authors(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestActor>> {
        let search = input.query.as_deref().unwrap_or("").trim();
        let path = if search.chars().count() < 2 {
            format!(
                "{}/collaborators?limit=50",
                repository(&input.repo.repository)?
            )
        } else {
            format!("/users?q={}&limit=50", encode(search))
        };
        let raw = self
            .api(&input.repo.host, "searchAuthors", Method::GET, &path, None)
            .await?;
        let people=rows(&raw["items"]).filter_map(|p|{let handle=text(p,"handle")?;Some(d::actor(&json!({"kind":"local","handle":handle,"displayName":p["displayName"],"avatarUrl":p["avatarUrl"]}),&input.repo.host))}).collect::<Vec<_>>();
        parse(
            json!({"items":people,"truncated":text(&raw,"next").is_some()}),
            "searchAuthors",
        )
    }
    async fn set_labels(&self, input: LabelsInput) -> Result<()> {
        let (labels, _) = self
            .pages(
                &input.pr.repo,
                "setLabels",
                &format!("{}/labels", repository(&input.pr.repo.repository)?),
            )
            .await?;
        let selected: Vec<_> = labels
            .iter()
            .filter(|l| text(l, "name").is_some_and(|n| input.labels.iter().any(|s| s == n)))
            .filter_map(|l| text(l, "id").map(str::to_string))
            .collect();
        if selected.len() != input.labels.len() {
            return Err(error(
                "setLabels",
                "One or more requested labels could not be found.",
            ));
        }
        self.toggle(&input.pr, "labels", "labelIds", selected, input.applied)
            .await
    }
    async fn reply_to_thread(&self, input: ThreadReplyInput) -> Result<()> {
        self.api(
            &input.pr.repo.host,
            "replyToThread",
            Method::POST,
            &format!(
                "{}/comments/{}/replies",
                pull_path(&input.pr)?,
                id(&input.thread_id)?
            ),
            Some(json!({"body":input.body})),
        )
        .await?;
        Ok(())
    }
    async fn set_reaction(&self, input: ReactionInput) -> Result<()> {
        self.reaction(input).await
    }
    async fn set_thread_resolution(&self, input: ThreadResolutionInput) -> Result<()> {
        let path = pull_path(&input.pr)?;
        let (comments, _) = self
            .pages(
                &input.pr.repo,
                "setThreadResolution",
                &format!("{path}/comments"),
            )
            .await?;
        let root = comments
            .iter()
            .find(|c| c["id"] == input.thread_id && c["threadId"] == input.thread_id)
            .ok_or_else(|| {
                error(
                    "setThreadResolution",
                    "GitCafe did not return the thread root.",
                )
            })?;
        let version = root["version"]
            .as_u64()
            .ok_or_else(|| error("setThreadResolution", "Missing thread root version."))?;
        if input.resolved == !root["resolvedAt"].is_null() {
            return Ok(());
        }
        self.api(
            &input.pr.repo.host,
            "setThreadResolution",
            Method::POST,
            &format!(
                "{path}/comments/{}/{}",
                id(&input.thread_id)?,
                if input.resolved {
                    "resolve"
                } else {
                    "unresolve"
                }
            ),
            Some(json!({"expectedVersion":version})),
        )
        .await?;
        Ok(())
    }
}
const SUPPORTED: &[&str] = &[
    "discover",
    "get_viewer",
    "checkout_list_change_requests",
    "checkout_get_change_request",
    "checkout_change_request",
    "get_repository_clone_urls",
    "create_repository",
    "create_change_request",
    "get_default_branch",
    "list_change_requests",
    "get_change_request",
    "get_change_request_preview",
    "get_change_request_summary",
    "get_change_request_stack",
    "get_viewer_permissions",
    "get_change_request_activity",
    "get_diff",
    "get_diff_file_paths",
    "get_diff_file_contents",
    "run_action",
    "update_change_request",
    "comment",
    "update_comment",
    "submit_review",
    "list_reviewer_candidates",
    "set_reviewer_request",
    "list_label_candidates",
    "list_repository_labels",
    "search_repository_authors",
    "set_labels",
    "reply_to_thread",
    "set_reaction",
    "set_thread_resolution",
];
