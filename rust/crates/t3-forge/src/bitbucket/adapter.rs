use super::{decode as d, *};
use async_trait::async_trait;
#[async_trait]
impl Forge for Bitbucket {
    fn settings_revision(&self) -> u64 {
        self.http.settings_revision()
    }
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("bitbucket".into())
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.capabilities
    }
    fn supports(&self, op: &str) -> bool {
        SUPPORTED.contains(&op)
    }
    fn reads_public_host_without_checkout(&self) -> bool {
        true
    }
    fn serves_host(&self, host: &str) -> bool {
        self.serves(host)
    }
    async fn shutdown(&self) {
        self.http.shutdown();
    }
    fn unavailable_requirement(&self) -> Option<UnavailableRequirement> {
        Some(UnavailableRequirement {
            missing: "Bitbucket needs API credentials in Settings → Source Control.".into(),
            unauthenticated:
                "Bitbucket rejected the configured credentials. Check Settings → Source Control."
                    .into(),
        })
    }
    async fn discover(
        &self,
        _cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        let auth = if !self.http.credential_configured() {
            json!({"status":"unauthenticated","account":{"_tag":"None"},"host":{"_tag":"None"},"detail":{"_tag":"None"}})
        } else {
            match self.api("probeAuth", Method::GET, "/user", None).await {
                Ok(raw) => {
                    json!({"status":"authenticated","account":{"_tag":"Some","value":text(&raw,"nickname").or_else(||text(&raw,"display_name")).unwrap_or("Bitbucket account")},"host":{"_tag":"Some","value":"bitbucket.org"},"detail":{"_tag":"None"}})
                }
                Err(e) => {
                    json!({"status":if e.reason()==Some(FailureReason::Unauthenticated){"unauthenticated"}else{"unknown"},"account":{"_tag":"None"},"host":{"_tag":"None"},"detail":{"_tag":"None"}})
                }
            }
        };
        parse(
            json!({"kind":"bitbucket","label":"Bitbucket","executable":"api","status":"available","version":{"_tag":"None"},"installHint":"Add a Bitbucket token in Settings → Source Control.","detail":{"_tag":"None"},"auth":auth}),
            "discover",
        )
    }
    async fn get_viewer(&self, _input: ViewerInput) -> Result<String> {
        let raw = self.api("getViewer", Method::GET, "/user", None).await?;
        text(&raw, "nickname")
            .or_else(|| text(&raw, "display_name"))
            .map(str::to_string)
            .ok_or_else(|| error("getViewer", "No account name was returned."))
    }
    async fn checkout_list_change_requests(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        self.checkout_list(input).await
    }
    async fn checkout_get_change_request(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        d::checkout(&self.checkout_raw(&input).await?.1)
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
        if input.host.as_deref().is_some_and(|h| !self.serves(h)) {
            return Err(error("cloneUrls", "Unsupported Bitbucket host."));
        }
        d::urls(
            &self
                .api(
                    "getRepository",
                    Method::GET,
                    &repository(&input.repository)?,
                    None,
                )
                .await?,
        )
    }
    async fn create_repository(
        &self,
        input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        d::urls(&self.api("createRepository",Method::POST,&repository(&input.repository)?,Some(json!({"scm":"git","is_private":input.visibility==SourceControlRepositoryVisibility::Private}))).await?)
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
        parse(self.detail(&input).await?, "getChangeRequest")
    }
    async fn get_change_request_preview(&self, input: PullRequestInput) -> Result<ProviderPreview> {
        parse(
            d::listing(&self.raw(&input).await?)?,
            "getChangeRequestPreview",
        )
    }
    async fn get_change_request_checks(&self, input: PullRequestInput) -> Result<CheckResult> {
        let (raw, checks) = tokio::try_join!(self.raw(&input), self.checks(&input))?;
        parse(
            json!({"state":d::state(&raw),"checks":checks}),
            "getChangeRequestChecks",
        )
    }
    async fn get_viewer_permissions(
        &self,
        input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        parse(
            permissions(self.can_write(&input.pr.repo).await?),
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
    async fn get_file_revisions(&self, input: FileRevisionsInput) -> Result<ProviderFileRevisions> {
        self.revisions(input).await
    }
    async fn list_reviewer_candidates(
        &self,
        input: CandidateInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        let raw = self.raw(&input.pr).await?;
        let workspace = input.pr.repo.repository.split('/').next().unwrap_or("");
        let members = self
            .api(
                "listReviewerCandidates",
                Method::GET,
                &format!("/workspaces/{}/members?pagelen=50", encode(workspace)),
                None,
            )
            .await?;
        let mut candidates = vec![];
        let author = d::actor(&raw["author"]);
        for member in rows(&members["values"]) {
            let mut actor = d::actor(&member["user"]);
            let Some(id) = text(&member["user"], "uuid") else {
                continue;
            };
            if actor.is_null() || actor["login"] == author["login"] {
                continue;
            }
            actor["id"] = json!(id);
            actor["kind"] = json!("user");
            actor["isRequested"] = json!(rows(&raw["reviewers"]).any(|r| r["uuid"] == id));
            candidates.push(actor);
        }
        parse(
            json!({"candidates":candidates,"truncated":text(&members,"next").is_some()}),
            "listReviewerCandidates",
        )
    }
    async fn set_reviewer_request(&self, input: ReviewerRequestInput) -> Result<()> {
        let raw = self.raw(&input.pr).await?;
        let mut ids: std::collections::BTreeSet<_> = rows(&raw["reviewers"])
            .filter_map(|r| text(r, "uuid").map(str::to_string))
            .collect();
        for reviewer in input.reviewers {
            let value = serde_json::to_value(reviewer)
                .map_err(|_| error("setReviewers", "Invalid reviewer."))?;
            if let Some(id) = text(&value, "id") {
                if input.requested {
                    ids.insert(id.into());
                } else {
                    ids.remove(id);
                }
            }
        }
        self.invalidate().await;
        self.api("setReviewerRequest",Method::PUT,&request_path(&input.pr)?,Some(json!({"reviewers":ids.into_iter().map(|id|json!({"uuid":id})).collect::<Vec<_>>()}))).await?;
        Ok(())
    }
    async fn run_action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        self.check_host(&input.pr.repo)?;
        let (path, body) = match input.action {
            PullRequestAction::Merge => (
                "merge",
                Some(
                    json!({"merge_strategy":match input.merge_method{Some(PullRequestMergeMethod::Squash)=>"squash",Some(PullRequestMergeMethod::Rebase)=>"rebase_fast_forward",_=>"merge_commit"}}),
                ),
            ),
            PullRequestAction::Close => ("decline", None),
            _ => return Err(ForgeError::unsupported(&self.kind(), "run_action")),
        };
        self.invalidate().await;
        let raw = self
            .api(
                "runAction",
                Method::POST,
                &format!("{}/{path}", request_path(&input.pr)?),
                body,
            )
            .await?;
        let Ok(mut value) = d::listing(&raw) else {
            return Ok(None);
        };
        if let Some(value) = value.as_object_mut() {
            for key in ["additions", "deletions", "mergeability"] {
                value.remove(key);
            }
        }
        parse(value, "runAction").map(Some)
    }
    async fn update_change_request(&self, input: UpdateInput) -> Result<()> {
        self.check_host(&input.pr.repo)?;
        let mut body = json!({});
        if let Some(title) = input.title {
            body["title"] = json!(title);
        }
        if let Some(description) = input.body {
            body["description"] = json!(description);
        }
        self.invalidate().await;
        self.api(
            "updateChangeRequest",
            Method::PUT,
            &request_path(&input.pr)?,
            Some(body),
        )
        .await?;
        Ok(())
    }
    async fn comment(&self, input: CommentInput) -> Result<()> {
        self.check_host(&input.pr.repo)?;
        self.api(
            "comment",
            Method::POST,
            &format!("{}/comments", request_path(&input.pr)?),
            Some(json!({"content":{"raw":input.body}})),
        )
        .await?;
        Ok(())
    }
    async fn update_comment(&self, input: UpdateCommentInput) -> Result<()> {
        self.check_host(&input.pr.repo)?;
        self.api(
            "updateComment",
            Method::PUT,
            &format!(
                "{}/comments/{}",
                request_path(&input.pr)?,
                encode(&input.comment_id)
            ),
            Some(json!({"content":{"raw":input.body}})),
        )
        .await?;
        Ok(())
    }
    async fn reply_to_thread(&self, input: ThreadReplyInput) -> Result<()> {
        self.check_host(&input.pr.repo)?;
        let id = input
            .thread_id
            .parse::<u64>()
            .map_err(|_| error("reply", "Invalid parent comment id."))?;
        self.api(
            "replyToComment",
            Method::POST,
            &format!("{}/comments", request_path(&input.pr)?),
            Some(json!({"content":{"raw":input.body},"parent":{"id":id}})),
        )
        .await?;
        Ok(())
    }
    async fn set_thread_resolution(&self, input: ThreadResolutionInput) -> Result<()> {
        self.check_host(&input.pr.repo)?;
        self.api(
            "setCommentResolution",
            if input.resolved {
                Method::POST
            } else {
                Method::DELETE
            },
            &format!(
                "{}/comments/{}/resolve",
                request_path(&input.pr)?,
                encode(&input.thread_id)
            ),
            None,
        )
        .await?;
        Ok(())
    }
    async fn submit_review(&self, input: ReviewInput) -> Result<()> {
        self.check_host(&input.pr.repo)?;
        let path = request_path(&input.pr)?;
        for comment in input.comments {
            let c = serde_json::to_value(comment)
                .map_err(|_| error("submitReview", "Invalid inline comment."))?;
            let p = &c["position"];
            let left = p["kind"] == "deleted" || p["kind"] == "context" && p["side"] == "left";
            let mut inline = json!({"path":c["path"]});
            inline[if left { "from" } else { "to" }] =
                p[if left { "oldLine" } else { "newLine" }].clone();
            self.api(
                "submitReview",
                Method::POST,
                &format!("{path}/comments"),
                Some(json!({"content":{"raw":c["body"]},"inline":inline})),
            )
            .await?;
        }
        if !input.body.trim().is_empty() {
            self.api(
                "submitReview",
                Method::POST,
                &format!("{path}/comments"),
                Some(json!({"content":{"raw":input.body}})),
            )
            .await?;
        }
        let verdict = match input.verdict {
            PullRequestReviewVerdict::Approve => Some("approve"),
            PullRequestReviewVerdict::RequestChanges => Some("request-changes"),
            _ => None,
        };
        if let Some(verdict) = verdict {
            self.api(
                "submitReview",
                Method::POST,
                &format!("{path}/{verdict}"),
                None,
            )
            .await?;
        }
        Ok(())
    }
    async fn forget_change_request_reads(&self, _scope: Option<ReadScope>) {
        self.invalidate().await;
    }
}
const SUPPORTED: &[&str] = &[
    "discover",
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
    "get_change_request_checks",
    "get_viewer_permissions",
    "get_change_request_activity",
    "get_diff",
    "get_file_revisions",
    "list_reviewer_candidates",
    "set_reviewer_request",
    "run_action",
    "update_change_request",
    "comment",
    "update_comment",
    "reply_to_thread",
    "set_thread_resolution",
    "submit_review",
    "forget_change_request_reads",
];
