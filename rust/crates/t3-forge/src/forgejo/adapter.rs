use super::{decode as d, *};
use async_trait::async_trait;
#[async_trait]
impl Forge for Forgejo {
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("forgejo".into())
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.capabilities
    }
    fn supports(&self, operation: &str) -> bool {
        SUPPORTED.contains(&operation)
    }
    fn settings_revision(&self) -> u64 {
        self.revision
    }
    fn reads_public_host_without_checkout(&self) -> bool {
        true
    }
    async fn shutdown(&self) {
        self.transport.shutdown().await;
    }
    fn unavailable_requirement(&self) -> Option<UnavailableRequirement> {
        Some(UnavailableRequirement {
            missing: "Install fj 0.6 or later, or tea 0.16 or later.".into(),
            unauthenticated:
                "Authenticate with fj for configured fj accounts; otherwise use tea login add."
                    .into(),
        })
    }
    async fn discover(
        &self,
        cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        self.discover_cli(cwd).await
    }
    fn repository_name_from_remote_url(&self, url: &str) -> Option<String> {
        let remote = transport::parse_remote(url)?;
        if url.to_ascii_lowercase().starts_with("http") {
            let parts: Vec<_> = remote.path.split('/').collect();
            (parts.len() >= 2).then(|| parts[parts.len() - 2..].join("/"))
        } else {
            Some(remote.path)
        }
    }
    async fn refine_unknown_remote(
        &self,
        input: CheckoutInput,
    ) -> Result<Option<SourceControlProviderInfo>> {
        self.refine(&input)
            .await?
            .map(|l| {
                parse(
                    json!({"kind":"forgejo","name":"Forgejo / Gitea","baseUrl":l.url}),
                    "refineUnknownRemote",
                )
            })
            .transpose()
    }
    async fn refine_repository_identity(
        &self,
        mut identity: RepositoryIdentity,
    ) -> Result<RepositoryIdentity> {
        if identity.root_path.is_none()
            || identity
                .provider
                .as_ref()
                .is_some_and(|p| !matches!(p.0.as_str(), "unknown" | "forgejo"))
        {
            return Ok(identity);
        }
        let Some(remote) = transport::parse_remote(&identity.locator.remote_url.0) else {
            return Ok(identity);
        };
        let context = CheckoutContext {
            provider: parse(
                json!({"kind":"unknown","name":"Unknown","baseUrl":""}),
                "refineIdentity",
            )?,
            remote_name: identity.locator.remote_name.0.clone(),
            remote_url: identity.locator.remote_url.0.clone(),
            requested_host: None,
        };
        let Some(login) = self
            .refine(&CheckoutInput {
                cwd: identity
                    .root_path
                    .as_ref()
                    .map(|s| s.0.clone())
                    .unwrap_or_default()
                    .into(),
                context: Some(context),
            })
            .await?
        else {
            return Ok(identity);
        };
        let base = url::Url::parse(&login.url)
            .map_err(|_| error("refineIdentity", "Invalid serving login URL."))?;
        let mount = base.path().trim_matches('/');
        let path = if !remote.ssh && !mount.is_empty() {
            remote
                .path
                .strip_prefix(&format!("{mount}/"))
                .unwrap_or(&remote.path)
        } else {
            &remote.path
        };
        identity.provider = Some(self.kind());
        identity.web_url = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(format!(
            "{}/{path}",
            login.url.trim_end_matches('/')
        )));
        Ok(identity)
    }
    async fn resolve_remote_web_host(&self, input: CheckoutInput) -> Result<Option<String>> {
        if input
            .context
            .as_ref()
            .and_then(|c| transport::parse_remote(&c.remote_url))
            .is_none_or(|r| !r.ssh)
        {
            return Ok(None);
        }
        Ok(self
            .refine(&input)
            .await?
            .and_then(|l| transport::parse_remote(&l.url))
            .map(|r| r.host))
    }
    async fn get_viewer(&self, input: ViewerInput) -> Result<String> {
        self.viewer(&ProviderRepositoryRef {
            cwd: input.cwd.to_string_lossy().into(),
            host: input.host.unwrap_or_default(),
            repository: String::new(),
        })
        .await
    }
    async fn checkout_list_change_requests(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        self.checkout_list(input).await
    }
    async fn checkout_get_change_request(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        d::checkout(&self.checkout_get_raw(&input).await?.1)
    }
    async fn checkout_change_request(&self, input: CheckoutGetInput) -> Result<()> {
        self.checkout_request(input).await
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
        parse(d::listing(&self.raw(&input).await?)?, "getPreview")
    }
    async fn get_change_request_summary(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        parse(d::listing(&self.raw(&input).await?)?, "getSummary")
    }
    async fn get_change_request_checks(&self, input: PullRequestInput) -> Result<CheckResult> {
        let pr = self.raw(&input).await?;
        parse(
            json!({"state":d::state(&pr),"checks":self.checks(&input.repo,&pr).await?}),
            "getChecks",
        )
    }
    async fn get_viewer_permissions(
        &self,
        input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        let path = repo_path(&input.pr.repo.repository);
        let (repo, pr, viewer) = tokio::try_join!(
            self.api(&input.pr.repo, &path, Method::GET, None),
            self.raw(&input.pr),
            self.viewer(&input.pr.repo)
        )?;
        parse(
            d::permissions(&repo.0, &pr, &viewer),
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
        let path = if let Some(commit) = input.commit {
            format!(
                "{}/git/commits/{}.diff",
                repo_path(&input.pr.repo.repository),
                encode(&commit)
            )
        } else {
            format!("{}.diff", pull_path(&input.pr))
        };
        let response = self
            .request(&input.pr.repo, &path, Method::GET, None)
            .await?;
        Ok(ProviderDiffSlice {
            patch: String::from_utf8_lossy(&response.stdout).into(),
            truncated: response.truncated,
            next_cursor: None,
            omitted_file_stats: None,
        })
    }
    async fn get_file_revisions(&self, input: FileRevisionsInput) -> Result<ProviderFileRevisions> {
        let response = self
            .request(
                &input.pr.repo,
                &format!("{}.diff", pull_path(&input.pr)),
                Method::GET,
                None,
            )
            .await?;
        if response.truncated {
            return Err(error(
                "getFileRevisions",
                "The diff exceeded the byte limit.",
            ));
        }
        let mut revisions = crate::diff::revisions(&String::from_utf8_lossy(&response.stdout));
        for path in input.paths {
            revisions.entry(path).or_default();
        }
        Ok(ProviderFileRevisions {
            revisions,
            complete: Some(true),
        })
    }
    async fn get_diff_file_contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        self.contents(input).await
    }
    async fn run_action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        match input.action {
            PullRequestAction::Merge => {
                let repo = self
                    .api(
                        &input.pr.repo,
                        &repo_path(&input.pr.repo.repository),
                        Method::GET,
                        None,
                    )
                    .await?
                    .0;
                let delete = repo["default_delete_branch_after_merge"] == true;
                let method = if input.merge_method == Some(PullRequestMergeMethod::FastForward) {
                    json!("fast-forward-only")
                } else {
                    serde_json::to_value(
                        input.merge_method.unwrap_or(PullRequestMergeMethod::Merge),
                    )
                    .map_err(|_| error("merge", "Invalid merge method."))?
                };
                let result = self
                    .request(
                        &input.pr.repo,
                        &format!("{}/merge", pull_path(&input.pr)),
                        Method::POST,
                        Some(json!({"Do":method,"delete_branch_after_merge":delete})),
                    )
                    .await;
                match result {
                    Ok(_) => Ok(None),
                    Err(e) if delete => match self.raw(&input.pr).await {
                        Ok(pr) if pr["merged"] == true => Ok(None),
                        _ => Err(e),
                    },
                    Err(e) => Err(e),
                }
            }
            PullRequestAction::Close | PullRequestAction::Reopen => {
                let response=self.request(&input.pr.repo,&pull_path(&input.pr),Method::PATCH,Some(json!({"state":if input.action==PullRequestAction::Close{"closed"}else{"open"}}))).await?;
                let raw = serde_json::from_slice::<Value>(&response.stdout).unwrap_or(Value::Null);
                let Ok(mut summary) = d::listing(&raw) else {
                    return Ok(None);
                };
                if summary["mergeability"] == "unknown"
                    && let Some(object) = summary.as_object_mut()
                {
                    object.remove("mergeability");
                }
                for field in ["additions", "deletions"] {
                    if raw[field].is_null()
                        && let Some(object) = summary.as_object_mut()
                    {
                        object.remove(field);
                    }
                }
                if let Some(sha) = text(&raw["head"], "sha") {
                    summary["headSha"] = json!(sha);
                }
                parse(summary, "runAction").map(Some)
            }
            PullRequestAction::UpdateBranch => {
                let style = serde_json::to_value(
                    input
                        .update_method
                        .unwrap_or(PullRequestUpdateMethod::Merge),
                )
                .map_err(|_| error("updateBranch", "Invalid update method."))?;
                self.request(
                    &input.pr.repo,
                    &format!(
                        "{}/update?style={}",
                        pull_path(&input.pr),
                        style.as_str().unwrap_or("merge")
                    ),
                    Method::POST,
                    None,
                )
                .await?;
                Ok(None)
            }
            _ => Err(ForgeError::unsupported(&self.kind(), "run_action")),
        }
    }
    async fn update_change_request(&self, input: UpdateInput) -> Result<()> {
        let mut body = json!({});
        if let Some(title) = input.title {
            body["title"] = json!(title);
        }
        if let Some(description) = input.body {
            body["body"] = json!(description);
        }
        self.request(
            &input.pr.repo,
            &pull_path(&input.pr),
            Method::PATCH,
            Some(body),
        )
        .await?;
        Ok(())
    }
    async fn comment(&self, input: CommentInput) -> Result<()> {
        self.request(
            &input.pr.repo,
            &format!("{}/comments", issue_path(&input.pr)),
            Method::POST,
            Some(json!({"body":input.body})),
        )
        .await?;
        Ok(())
    }
    async fn update_comment(&self, input: UpdateCommentInput) -> Result<()> {
        self.request(
            &input.pr.repo,
            &format!(
                "{}/issues/comments/{}",
                repo_path(&input.pr.repo.repository),
                encode(&input.comment_id)
            ),
            Method::PATCH,
            Some(json!({"body":input.body})),
        )
        .await?;
        Ok(())
    }
    async fn submit_review(&self, input: ReviewInput) -> Result<()> {
        let pr = self.raw(&input.pr).await?;
        let comments=input.comments.into_iter().map(|c|{let raw=serde_json::to_value(c).map_err(|_|error("submitReview","Invalid comment."))?;let position=&raw["position"];let old=position["kind"]=="deleted"||position["kind"]=="context"&&position["side"]=="left";Ok(json!({"path":if old{raw["oldPath"].as_str().or_else(||raw["path"].as_str())}else{raw["path"].as_str()},"body":raw["body"],"old_position":if old{position["oldLine"].clone()}else{json!(0)},"new_position":if old{json!(0)}else{position["newLine"].clone()}}))}).collect::<Result<Vec<_>>>()?;
        let event = match input.verdict {
            PullRequestReviewVerdict::Approve => "APPROVED",
            PullRequestReviewVerdict::RequestChanges => "REQUEST_CHANGES",
            _ => "COMMENT",
        };
        self.request(&input.pr.repo,&format!("{}/reviews",pull_path(&input.pr)),Method::POST,Some(json!({"event":event,"body":input.body,"commit_id":pr["head"]["sha"],"comments":comments}))).await?;
        Ok(())
    }
    async fn list_reviewer_candidates(
        &self,
        input: CandidateInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        let path = format!("{}/assignees", repo_path(&input.pr.repo.repository));
        let (pr, users) = tokio::try_join!(
            self.raw(&input.pr),
            self.api(&input.pr.repo, &path, Method::GET, None)
        )?;
        let candidates = rows(&users.0)
            .filter(|u| u["login"] != pr["user"]["login"])
            .filter_map(|u| {
                let mut actor = d::actor(u);
                if actor.is_null() {
                    return None;
                }
                actor["id"] = u["login"].clone();
                actor["kind"] = json!("user");
                actor["isRequested"] =
                    json!(rows(&pr["requested_reviewers"]).any(|r| r["login"] == u["login"]));
                Some(actor)
            })
            .collect::<Vec<_>>();
        parse(
            json!({"candidates":candidates,"truncated":false}),
            "listReviewerCandidates",
        )
    }
    async fn set_reviewer_request(&self, input: ReviewerRequestInput) -> Result<()> {
        let reviewers = input
            .reviewers
            .into_iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| error("setReviewers", "Invalid reviewers."))?;
        self.request(&input.pr.repo,&format!("{}/requested_reviewers",pull_path(&input.pr)),if input.requested{Method::POST}else{Method::DELETE},Some(json!({"reviewers":reviewers.iter().filter_map(|r|text(r,"id")).collect::<Vec<_>>()}))).await?;
        Ok(())
    }
    async fn list_label_candidates(
        &self,
        input: PullRequestInput,
    ) -> Result<PullRequestLabelCandidateList> {
        let path = format!("{}/labels", repo_path(&input.repo.repository));
        let (pr, (labels, truncated)) =
            tokio::try_join!(self.raw(&input), self.page(&input.repo, &path, 500))?;
        parse(
            json!({"candidates":labels.iter().map(|l|json!({"name":l["name"],"color":l["color"],"description":l["description"],"isApplied":rows(&pr["labels"]).any(|a|a["id"]==l["id"])})).collect::<Vec<_>>(),"truncated":truncated}),
            "labelCandidates",
        )
    }
    async fn list_repository_labels(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestLabel>> {
        let (labels, truncated) = self
            .page(
                &input.repo,
                &format!("{}/labels", repo_path(&input.repo.repository)),
                500,
            )
            .await?;
        parse(
            json!({"items":labels.iter().map(|l|json!({"name":l["name"],"color":l["color"]})).collect::<Vec<_>>(),"truncated":truncated}),
            "repositoryLabels",
        )
    }
    async fn search_repository_authors(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestActor>> {
        let query = input.query.as_deref().unwrap_or("").trim();
        let path = if query.is_empty() {
            format!("{}/assignees", repo_path(&input.repo.repository))
        } else {
            format!("users/search?q={}&limit=50", encode(query))
        };
        let (raw, link) = self.api(&input.repo, &path, Method::GET, None).await?;
        let users = if query.is_empty() { &raw } else { &raw["data"] };
        let items = rows(users)
            .map(d::actor)
            .filter(|a| !a.is_null())
            .collect::<Vec<_>>();
        let truncated =
            !query.is_empty() && (link.as_ref().is_some_and(|l| has_next(l)) || items.len() >= 50);
        parse(
            json!({"items":items,"truncated":truncated}),
            "searchAuthors",
        )
    }
    async fn set_labels(&self, input: LabelsInput) -> Result<()> {
        let (labels, _) = self
            .page(
                &input.pr.repo,
                &format!("{}/labels", repo_path(&input.pr.repo.repository)),
                500,
            )
            .await?;
        let selected: Vec<_> = labels
            .iter()
            .filter(|l| text(l, "name").is_some_and(|n| input.labels.iter().any(|s| s == n)))
            .collect();
        if selected.len() != input.labels.len() {
            return Err(error(
                "setLabels",
                "One or more requested labels could not be found.",
            ));
        }
        if input.applied {
            self.request(
                &input.pr.repo,
                &format!("{}/labels", issue_path(&input.pr)),
                Method::POST,
                Some(json!({"labels":selected.iter().map(|l|l["id"].clone()).collect::<Vec<_>>()})),
            )
            .await?;
        } else {
            for label in selected {
                self.request(
                    &input.pr.repo,
                    &format!("{}/labels/{}", issue_path(&input.pr), label["id"]),
                    Method::DELETE,
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }
    async fn set_reaction(&self, input: ReactionInput) -> Result<()> {
        let mut comment = input.subject_id;
        if let Some(id) = comment.as_ref().and_then(|s| s.strip_prefix("review:")) {
            if id.parse::<u64>().ok().is_none_or(|n| n == 0) {
                return Err(error("setReaction", "Invalid review identifier."));
            }
            let raw = self
                .api(
                    &input.pr.repo,
                    &format!("{}/reviews/{id}", pull_path(&input.pr)),
                    Method::GET,
                    None,
                )
                .await?
                .0;
            comment = Some(super::read::review_comment_id(&raw).ok_or_else(|| {
                error("setReaction", "No review comment identifier was returned.")
            })?);
        }
        if comment.as_ref().is_some_and(|id| {
            id.parse::<u64>().ok().is_none_or(|n| n == 0) || !id.bytes().all(|b| b.is_ascii_digit())
        }) {
            return Err(error("setReaction", "Invalid comment identifier."));
        }
        let path = comment
            .map(|id| {
                format!(
                    "{}/issues/comments/{id}/reactions",
                    repo_path(&input.pr.repo.repository)
                )
            })
            .unwrap_or_else(|| format!("{}/reactions", issue_path(&input.pr)));
        self.request(
            &input.pr.repo,
            &path,
            if input.reacted {
                Method::POST
            } else {
                Method::DELETE
            },
            Some(json!({"content":d::reaction_name(&input.content)})),
        )
        .await?;
        Ok(())
    }
}
const SUPPORTED: &[&str] = &[
    "discover",
    "refine_unknown_remote",
    "refine_repository_identity",
    "resolve_remote_web_host",
    "get_viewer",
    "checkout_list_change_requests",
    "checkout_get_change_request",
    "checkout_change_request",
    "create_change_request",
    "get_repository_clone_urls",
    "create_repository",
    "get_default_branch",
    "list_change_requests",
    "get_change_request",
    "get_change_request_preview",
    "get_change_request_summary",
    "get_change_request_checks",
    "get_viewer_permissions",
    "get_change_request_activity",
    "get_diff",
    "get_file_revisions",
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
    "set_reaction",
];
