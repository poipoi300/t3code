use super::{
    GitHub,
    decode::*,
    repository::{Repository, encode, parse_repository_selector},
    transport::{Request, error},
};
use crate::{FailureReason, Forge, ForgeError, Result, forge::*, model::*};
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};
pub(super) fn repo_parts(repository: &str) -> Result<(&str, &str)> {
    let parts: Vec<_> = repository
        .trim()
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    match parts.as_slice() {
        [owner, name]
            if parts.iter().all(|p| {
                *p != "."
                    && *p != ".."
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            }) =>
        {
            Ok((owner, name))
        }
        _ => Err(error(
            "repository",
            FailureReason::Failed,
            "Repositories are named owner/name.",
        )),
    }
}
fn variables(input: &PullRequestInput) -> Result<Value> {
    let (owner, name) = repo_parts(&input.repo.repository)?;
    if input.number == 0 || input.number > i32::MAX as u64 {
        return Err(error(
            "reference",
            FailureReason::Failed,
            "Invalid pull request number.",
        ));
    }
    Ok(json!({"owner":owner,"name":name,"number":input.number}))
}
fn repo_variables(input: &ProviderRepositoryRef) -> Result<Value> {
    let (owner, name) = repo_parts(&input.repository)?;
    Ok(json!({"owner":owner,"name":name}))
}
fn path(input: &PullRequestInput, suffix: &str) -> Result<String> {
    let (owner, name) = repo_parts(&input.repo.repository)?;
    Ok(format!(
        "repos/{owner}/{name}/pulls/{}{suffix}",
        input.number
    ))
}
fn json_error(operation: &str) -> ForgeError {
    error(
        operation,
        FailureReason::Failed,
        "GitHub returned invalid JSON.",
    )
}
impl GitHub {
    pub(crate) async fn rest_json(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let response = self
            .http
            .rest(host, cwd, operation, method, path, body)
            .await?;
        if response.truncated {
            return Err(error(
                operation,
                FailureReason::Failed,
                "GitHub's answer exceeded the response byte limit.",
            ));
        }
        if response.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.body).map_err(|_| json_error(operation))
    }
    pub(super) async fn query(
        &self,
        input: &PullRequestInput,
        operation: &str,
        extra: Value,
    ) -> Result<Value> {
        let mut vars = variables(input)?;
        if let Some(extra) = extra.as_object() {
            vars.as_object_mut()
                .ok_or_else(|| json_error(operation))?
                .extend(extra.clone());
        }
        self.http
            .graphql(&input.repo.host, &input.repo.cwd, operation, vars)
            .await
    }
    async fn node_id(&self, input: &PullRequestInput) -> Result<String> {
        let credential = self
            .http
            .credential(&input.repo.host, std::path::Path::new(&input.repo.cwd))
            .await?;
        let key = format!(
            "{}\0{}\0{}\0{}",
            credential.fingerprint,
            input.repo.host.to_lowercase(),
            input.repo.repository.to_lowercase(),
            input.number
        );
        let mut cache = self.node_ids.lock().await;
        if let Some((at, id)) = cache.get_mut(&key) {
            *at = super::transport::now();
            return Ok(id.clone());
        }
        drop(cache);
        let value = self.query(input, "PullRequestNodeId", Value::Null).await?;
        let id = text(pr(&value)?, "id")
            .map(str::to_string)
            .ok_or_else(|| json_error("PullRequestNodeId"))?;
        let mut cache = self.node_ids.lock().await;
        if cache.len() >= 128
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(key, (at, _))| (*at, (*key).clone()))
                .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(key, (super::transport::now(), id.clone()));
        Ok(id)
    }
    async fn subject_scope(&self, input: &PullRequestInput, id: &str) -> Result<()> {
        let value = self
            .query(input, "PullRequestSubject", json!({"subjectId":id}))
            .await?;
        let target = &pr(&value)?["id"];
        let node = &value["node"];
        if target.is_null()
            || ![
                &node["id"],
                &node["pullRequest"]["id"],
                &node["reviewCommentPullRequest"]["id"],
                &node["reviewPullRequest"]["id"],
            ]
            .contains(&target)
        {
            return Err(error(
                "subjectScope",
                FailureReason::Forbidden,
                "The subject does not belong to this pull request.",
            ));
        }
        Ok(())
    }
    async fn mutation(
        &self,
        input: &PullRequestInput,
        operation: &str,
        vars: Value,
    ) -> Result<Value> {
        self.http
            .graphql(&input.repo.host, &input.repo.cwd, operation, vars)
            .await
    }
    pub(super) async fn core(
        &self,
        input: &PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        let first=self.query(input,"PullRequestCore",json!({"headRef":format!("refs/pull/{}/head",input.number),"required":input.repo.host.eq_ignore_ascii_case("github.com"),"requestedReviewers":100,"mergeQueue":input.repo.host.eq_ignore_ascii_case("github.com")})).await?;
        let raw = pr(&first)?;
        let sha = raw["headRefOid"].clone();
        let conn = &raw["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
        let mut contexts: Vec<Value> = nodes(conn).into_iter().cloned().collect();
        let mut after = next_cursor(conn);
        let mut seen = HashSet::new();
        for _ in 1..10 {
            let Some(cursor) = after.take() else {
                break;
            };
            if !seen.insert(cursor.clone()) {
                after = Some(cursor);
                break;
            }
            let value=self.query(input,"PullRequestCheckContexts",json!({"required":input.repo.host.eq_ignore_ascii_case("github.com"),"after":cursor})).await?;
            let raw = pr(&value)?;
            if raw["headRefOid"] != sha {
                return Err(error(
                    "getChangeRequest",
                    FailureReason::Failed,
                    "The pull request head changed while reading checks.",
                ));
            }
            let conn = &raw["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
            contexts.extend(nodes(conn).into_iter().cloned());
            after = next_cursor(conn);
        }
        let mut result = detail(&first, &input.repo.host, &contexts, after.is_some())?;
        self.enrich_workflows(input, &first, &mut result).await?;
        Ok(result)
    }
    async fn files_connection(
        &self,
        input: &PullRequestInput,
        operation: &str,
    ) -> Result<(Vec<Value>, bool)> {
        let mut after = None::<String>;
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        for _ in 0..5 {
            let value = self.query(input, operation, json!({"after":after})).await?;
            let conn = &pr(&value)?["files"];
            files.extend(nodes(conn).into_iter().cloned());
            after = next_cursor(conn);
            let Some(cursor) = &after else {
                return Ok((files, false));
            };
            if !seen.insert(cursor.clone()) {
                return Ok((files, true));
            }
        }
        Ok((files, after.is_some()))
    }
}
#[async_trait]
impl Forge for GitHub {
    async fn shutdown(&self) {
        self.event_stop.cancel();
        self.event_tasks.wait().await;
    }
    fn settings_revision(&self) -> u64 {
        self.http.settings_revision()
    }
    async fn discover(
        &self,
        cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        self.discover_github(&cwd).await
    }
    async fn refine_unknown_remote(
        &self,
        input: CheckoutInput,
    ) -> Result<Option<SourceControlProviderInfo>> {
        self.refine_unknown(input).await
    }
    async fn change_request_events(&self, input: PullRequestInput) -> Result<ChangeRequestEvents> {
        Ok(self.events(input))
    }
    async fn fetch_media(&self, input: MediaRequest) -> Result<Option<MediaResponse>> {
        self.media(input).await
    }
    async fn sign_in_start(
        &self,
        host: String,
    ) -> Result<t3_contracts::source_control::SourceControlSignInPrompt> {
        self.app_sign_in
            .as_ref()
            .ok_or_else(|| ForgeError::unsupported(&self.kind(), "sign_in_start"))?
            .start(&host)
            .await
    }
    async fn sign_in_wait(
        &self,
        host: String,
    ) -> Result<t3_contracts::source_control::SourceControlSignInResult> {
        let app = self
            .app_sign_in
            .as_ref()
            .ok_or_else(|| ForgeError::unsupported(&self.kind(), "sign_in_wait"))?;
        app.wait(&host).await?;
        self.coverage.lock().await.clear();
        let credential = app.token(&host).await?.ok_or_else(|| {
            error(
                "signIn",
                FailureReason::Unauthenticated,
                "The GitHub app is signed out.",
            )
        })?;
        let http = self.http.pinned(host.clone(), credential);
        let response = http
            .rest(&host, ".", "appViewer", Method::GET, "user", None)
            .await?;
        let value: Value =
            serde_json::from_slice(&response.body).map_err(|_| json_error("appViewer"))?;
        parse(json!({"account":value["login"]}), "appViewer")
    }
    async fn sign_in_cancel(&self, host: String) -> Result<()> {
        if let Some(app) = &self.app_sign_in {
            app.cancel(&host).await;
        }
        Ok(())
    }
    async fn sign_in_access(
        &self,
        host: String,
        repository: String,
    ) -> Result<t3_contracts::source_control::SourceControlAppAccessStatus> {
        self.app_reach(&host, &repository).await
    }
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("github".into())
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.capabilities
    }
    fn supports(&self, operation: &str) -> bool {
        if matches!(
            operation,
            "sign_in_start" | "sign_in_wait" | "sign_in_cancel" | "sign_in_access"
        ) {
            return self.app_sign_in.is_some();
        }
        if matches!(
            operation,
            "change_request_events"
                | "fetch_media"
                | "discover"
                | "refine_unknown_remote"
                | "forget_change_request_reads"
        ) {
            return true;
        }
        !matches!(
            operation,
            "change_request_events" | "get_file_revisions" | "get_diff_file_paths"
        ) && SUPPORTED_OPERATIONS.contains(&operation)
    }
    fn reads_public_host_without_checkout(&self) -> bool {
        true
    }
    fn batches_reads(&self) -> bool {
        true
    }
    async fn forget_change_request_reads(&self, scope: Option<ReadScope>) {
        let mut checks = self.check_entries.lock().await;
        if let Some(scope) = scope {
            let repository = format!("\0{}\0", scope.repository.to_lowercase());
            checks.retain(|key, _| {
                !key.contains(&repository)
                    || scope
                        .number
                        .is_some_and(|number| !key.ends_with(&format!("\0{number}")))
            });
        } else {
            checks.clear();
            self.search_coverage.lock().await.clear();
        }
    }
    fn head_branch_probe(&self, selectors: &[String], default: Option<&str>) -> BranchProbe {
        BranchProbe {
            head_selectors: default.map(|s| vec![s.into()]).unwrap_or_else(|| {
                selectors
                    .iter()
                    .filter(|s| !s.contains(':'))
                    .cloned()
                    .collect()
            }),
            limit: 100,
        }
    }
    fn unavailable_requirement(&self) -> Option<UnavailableRequirement> {
        Some(UnavailableRequirement {missing:"No GitHub credential on the server. Set GH_TOKEN, or install the GitHub CLI (https://cli.github.com/) and run `gh auth login`.".into(),unauthenticated:"GitHub has no working credential for this host. Run `gh auth login`, or check the account and hosts in Settings → Source Control.".into()})
    }
    fn merge_message_rewrite(&self, message: &str) -> String {
        super::merge_message::remove_agent_credits(message)
    }
    async fn resolve_remote_web_host(&self, input: CheckoutInput) -> Result<Option<String>> {
        Ok(input
            .context
            .and_then(|c| super::repository::api_host_for_remote(&c.remote_url)))
    }
    async fn record_branch_base(&self, input: RecordBranchBaseInput) -> Result<()> {
        let _ = self
            .git_driver
            .at(input.cwd)
            .with_environment(self.git_environment.clone())
            .write_config_value(
                &format!("branch.{}.gh-merge-base", input.branch),
                &input.base_branch,
            )
            .await;
        Ok(())
    }
    async fn creates_change_requests_in(&self, input: CreatesInInput) -> Result<bool> {
        let expected_host = input
            .checkout
            .context
            .as_ref()
            .and_then(|c| url::Url::parse(&c.provider.base_url).ok())
            .map(|u| u.authority().to_lowercase());
        let Some(expected) = input.repository else {
            return Ok(false);
        };
        match self.resolve_repository(&input.checkout).await {
            Ok(repo) => Ok(expected_host.as_deref() == Some(&repo.host)
                && format!("{}/{}", repo.owner, repo.name).eq_ignore_ascii_case(&expected)),
            Err(_) => Ok(true),
        }
    }
    async fn resolve_link(&self, input: LinkInput) -> Result<Option<LinkSubject>> {
        let url = input.url;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Ok(None);
        }
        let segments: Vec<_> = url.path_segments().into_iter().flatten().collect();
        if segments.len() < 4
            || !matches!(segments[2], "issues" | "pull")
            || segments[3].parse::<u64>().ok().filter(|n| *n > 0).is_none()
        {
            return Ok(None);
        }
        if repo_parts(&format!("{}/{}", segments[0], segments[1])).is_err() {
            return Ok(None);
        }
        let endpoint = format!(
            "repos/{}/{}/issues/{}",
            segments[0], segments[1], segments[3]
        );
        let response = self
            .http
            .send(Request {
                host: "github.com",
                cwd: &input.cwd.to_string_lossy(),
                operation: "resolveLink",
                method: Method::GET,
                path: &endpoint,
                body: None,
                graphql: false,
                interactive: false,
                accept: "application/vnd.github+json",
                max_bytes: 1_000_000,
                timeout: Duration::from_secs(3),
                if_none_match: None,
            })
            .await?;
        let raw: Value =
            serde_json::from_slice(&response.body).map_err(|_| json_error("resolveLink"))?;
        Ok(Some(LinkSubject {
            title: raw["title"]
                .as_str()
                .ok_or_else(|| json_error("resolveLink"))?
                .into(),
            body: raw["body"].as_str().map(str::to_string),
        }))
    }
    async fn is_repository_starred(&self, host: String, repository: String) -> Result<bool> {
        let Ok((owner, name)) = repo_parts(&repository) else {
            return Ok(false);
        };
        Ok(self
            .http
            .rest(
                &host,
                ".",
                "isRepositoryStarred",
                Method::GET,
                &format!("user/starred/{}/{}", encode(owner), encode(name)),
                None,
            )
            .await
            .is_ok_and(|r| r.status == 204))
    }
    async fn star_repository(&self, host: String, repository: String) -> Result<bool> {
        let Ok((owner, name)) = repo_parts(&repository) else {
            return Ok(false);
        };
        Ok(self
            .http
            .rest(
                &host,
                ".",
                "starRepository",
                Method::PUT,
                &format!("user/starred/{}/{}", encode(owner), encode(name)),
                None,
            )
            .await
            .is_ok_and(|r| r.status == 204))
    }
    async fn read_change_request_template(&self, input: TemplateInput) -> Result<Option<String>> {
        Ok(self.template(input).await.unwrap_or(None))
    }
    async fn get_viewer(&self, input: ViewerInput) -> Result<String> {
        Ok(self.get_routing_identity(input).await?.viewer)
    }
    async fn with_verified_credential(
        &self,
        input: VerifiedCredentialInput,
    ) -> Result<VerifiedForge> {
        let credential = if let Some(repository) = &input.repository {
            self.repository_credential(&input.host, repository, &input.cwd.to_string_lossy())
                .await?
        } else {
            self.http.credential(&input.host, &input.cwd).await?
        };
        let mut pinned = self.clone();
        pinned.http = std::sync::Arc::new(self.http.pinned(input.host.clone(), credential.clone()));
        let identity = pinned
            .read_identity(&input.host, &input.cwd, credential.fingerprint)
            .await?;
        Ok(VerifiedForge {
            identity,
            forge: std::sync::Arc::new(pinned),
        })
    }
    async fn get_routing_identity(&self, input: ViewerInput) -> Result<RoutingIdentity> {
        Ok(self
            .with_verified_credential(VerifiedCredentialInput {
                cwd: input.cwd,
                host: input.host.unwrap_or_else(|| "github.com".into()),
                repository: None,
            })
            .await?
            .identity)
    }

    async fn checkout_list_change_requests(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        self.checkout_list(input).await
    }
    async fn checkout_get_change_request(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        self.checkout_record(input).await
    }
    async fn create_change_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        let repo = self.resolve_repository(&input.checkout).await?;
        let body = tokio::fs::read_to_string(input.body_file)
            .await
            .map_err(|_| {
                error(
                    "createChangeRequest",
                    FailureReason::Failed,
                    "The pull request description could not be read.",
                )
            })?;
        self.rest_json(&repo.host,&input.checkout.cwd.to_string_lossy(),"createPullRequest",Method::POST,&format!("repos/{}/{}/pulls",encode(&repo.owner),encode(&repo.name)),Some(json!({"base":input.base_ref_name,"head":input.head_selector,"title":input.title,"body":body,"maintainer_can_modify":true}))).await?;
        Ok(())
    }
    async fn get_repository_clone_urls(
        &self,
        input: CloneUrlsInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let host = if let Some(host) = input.host {
            host
        } else {
            self.resolve_repository(&input.checkout)
                .await
                .map(|r| r.host)
                .unwrap_or_else(|_| self.default_host.clone())
        };
        let repo = if let Some(repo) = parse_repository_selector(&input.repository, &host) {
            repo
        } else {
            let name = input.repository.trim().trim_end_matches(".git");
            if name.is_empty() || name.contains(['/', ' ']) {
                return Err(error(
                    "getRepositoryCloneUrls",
                    FailureReason::Failed,
                    "Repositories are named owner/name.",
                ));
            }
            let viewer = self
                .rest_json(
                    &host,
                    &input.checkout.cwd.to_string_lossy(),
                    "getViewer",
                    Method::GET,
                    "user",
                    None,
                )
                .await?;
            Repository {
                host,
                owner: text(&viewer, "login")
                    .ok_or_else(|| json_error("getViewer"))?
                    .into(),
                name: name.into(),
            }
        };
        let value = self
            .repository_json(&repo, &input.checkout.cwd.to_string_lossy())
            .await?;
        parse(
            json!({"nameWithOwner":value["full_name"],"url":value["html_url"],"sshUrl":value["ssh_url"]}),
            "getRepositoryCloneUrls",
        )
    }
    async fn create_repository(
        &self,
        input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let locator = parse_repository_selector(&input.repository, &self.default_host);
        let host = locator
            .as_ref()
            .map_or(self.default_host.as_str(), |r| r.host.as_str());
        let cwd = input.cwd.to_string_lossy();
        let viewer = if locator.is_some() {
            self.rest_json(host, &cwd, "getViewer", Method::GET, "user", None)
                .await?
        } else {
            Value::Null
        };
        let endpoint = if locator.as_ref().is_none_or(|r| {
            text(&viewer, "login").is_some_and(|s| s.eq_ignore_ascii_case(&r.owner))
        }) {
            "user/repos".into()
        } else {
            format!(
                "orgs/{}/repos",
                encode(
                    &locator
                        .as_ref()
                        .ok_or_else(|| json_error("createRepository"))?
                        .owner
                )
            )
        };
        let name = locator
            .as_ref()
            .map_or(input.repository.trim(), |r| r.name.as_str());
        let value=self.rest_json(host,&cwd,"createRepository",Method::POST,&endpoint,Some(json!({"name":name,"private":input.visibility==SourceControlRepositoryVisibility::Private}))).await?;
        parse(
            json!({"nameWithOwner":value["full_name"],"url":value["html_url"],"sshUrl":value["ssh_url"]}),
            "createRepository",
        )
    }
    async fn get_default_branch(&self, input: CheckoutInput) -> Result<Option<String>> {
        let repo = self.resolve_repository(&input).await?;
        let value = self
            .repository_json(&repo, &input.cwd.to_string_lossy())
            .await?;
        Ok(text(&value, "default_branch").map(str::to_string))
    }
    async fn get_change_request(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        self.core(&input).await
    }
    async fn get_change_request_checks(&self, input: PullRequestInput) -> Result<CheckResult> {
        self.conditional_checks(input).await
    }
    async fn get_change_request_preview(&self, input: PullRequestInput) -> Result<ProviderPreview> {
        let value = self
            .query(&input, "PullRequestPreview", Value::Null)
            .await?;
        let raw = pr(&value)?;
        parse(
            json!({"number":raw["number"],"title":raw["title"],"url":raw["url"],"state":state(raw),"isDraft":raw["isDraft"],"createdAt":raw["createdAt"],"author":actor(&raw["author"],&input.repo.host)}),
            "getChangeRequestPreview",
        )
    }
    async fn get_change_request_summary(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        let value = self
            .query(
                &input,
                "PullRequestSummary",
                json!({"stacks":input.repo.host.eq_ignore_ascii_case("github.com")}),
            )
            .await?;
        summary(pr(&value)?, &input.repo.host)
    }
    async fn get_change_request_watch_fingerprint(
        &self,
        input: PullRequestInput,
    ) -> Result<Option<ProviderChangeRequestWatchFingerprint>> {
        let value = self
            .query(
                &input,
                "PullRequestWatchFingerprint",
                json!({"mergeQueue":input.repo.host.eq_ignore_ascii_case("github.com")}),
            )
            .await?;
        Ok(Some(watch_fingerprint(pr(&value)?)))
    }
    async fn get_viewer_permissions(
        &self,
        input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        if input.include_update_branch {
            match self.core(&input.pr).await {
                Ok(d) => return Ok(d.viewer_permissions),
                Err(e) if e.reason() == Some(FailureReason::RateLimited) => return Err(e),
                _ => {}
            }
        }
        let value = self
            .query(&input.pr, "ViewerPermissions", Value::Null)
            .await?;
        if value["repository"].is_null() {
            return Err(error(
                "getViewerPermissions",
                FailureReason::NotFound,
                "Repository not found.",
            ));
        }
        viewer_permissions(&value["repository"], false)
    }
    async fn comment(&self, input: CommentInput) -> Result<()> {
        let subject = self.node_id(&input.pr).await?;
        self.mutation(
            &input.pr,
            "AddComment",
            json!({"subjectId":subject,"body":input.body}),
        )
        .await?;
        Ok(())
    }
    async fn update_change_request(&self, input: UpdateInput) -> Result<()> {
        let id = self.node_id(&input.pr).await?;
        let mut vars = json!({"pullRequestId":id});
        if let Some(title) = input.title {
            vars["title"] = json!(title);
        }
        if let Some(body) = input.body {
            vars["body"] = json!(body);
        }
        if let Some(flag) = input.maintainer_can_modify {
            vars["maintainerCanModify"] = json!(flag);
        }
        self.mutation(&input.pr, "UpdatePullRequest", vars).await?;
        Ok(())
    }
    async fn update_comment(&self, input: UpdateCommentInput) -> Result<()> {
        self.subject_scope(&input.pr, &input.comment_id).await?;
        self.mutation(
            &input.pr,
            if input.kind == PullRequestCommentUpdateInputKind::IssueComment {
                "UpdateIssueComment"
            } else {
                "UpdateReviewComment"
            },
            json!({"commentId":input.comment_id,"body":input.body}),
        )
        .await?;
        Ok(())
    }
    async fn submit_review(&self, input: ReviewInput) -> Result<()> {
        let body = review_submission(&input)?;
        self.rest_json(
            &input.pr.repo.host,
            &input.pr.repo.cwd,
            "submitReview",
            Method::POST,
            &path(&input.pr, "/reviews")?,
            Some(body),
        )
        .await?;
        Ok(())
    }
    async fn reply_to_thread(&self, input: ThreadReplyInput) -> Result<()> {
        self.mutation(
            &input.pr,
            "AddReviewThreadReply",
            json!({"threadId":input.thread_id,"body":input.body}),
        )
        .await?;
        Ok(())
    }
    async fn set_thread_resolution(&self, input: ThreadResolutionInput) -> Result<()> {
        self.mutation(
            &input.pr,
            if input.resolved {
                "ResolveReviewThread"
            } else {
                "UnresolveReviewThread"
            },
            json!({"threadId":input.thread_id}),
        )
        .await?;
        Ok(())
    }
    async fn set_reaction(&self, input: ReactionInput) -> Result<()> {
        let subject = if let Some(subject) = input.subject_id {
            self.subject_scope(&input.pr, &subject).await?;
            subject
        } else {
            self.node_id(&input.pr).await?
        };
        let content = match input.content {
            PullRequestReactionContent::ThumbsUp => "THUMBS_UP",
            PullRequestReactionContent::ThumbsDown => "THUMBS_DOWN",
            PullRequestReactionContent::Laugh => "LAUGH",
            PullRequestReactionContent::Hooray => "HOORAY",
            PullRequestReactionContent::Confused => "CONFUSED",
            PullRequestReactionContent::Heart => "HEART",
            PullRequestReactionContent::Rocket => "ROCKET",
            PullRequestReactionContent::Eyes => "EYES",
        };
        self.mutation(
            &input.pr,
            if input.reacted {
                "AddReaction"
            } else {
                "RemoveReaction"
            },
            json!({"subjectId":subject,"content":content}),
        )
        .await?;
        Ok(())
    }
    async fn set_reviewer_request(&self, input: ReviewerRequestInput) -> Result<()> {
        let mut users = Vec::new();
        let mut teams = Vec::new();
        for reviewer in input.reviewers {
            if reviewer.kind == PullRequestReviewerKind::Team {
                teams.push(reviewer.id);
            } else {
                users.push(reviewer.id);
            }
        }
        self.rest_json(
            &input.pr.repo.host,
            &input.pr.repo.cwd,
            "setReviewerRequest",
            if input.requested {
                Method::POST
            } else {
                Method::DELETE
            },
            &path(&input.pr, "/requested_reviewers")?,
            Some(json!({"reviewers":users,"team_reviewers":teams})),
        )
        .await?;
        Ok(())
    }
    async fn set_labels(&self, input: LabelsInput) -> Result<()> {
        let (owner, name) = repo_parts(&input.pr.repo.repository)?;
        let path = format!("repos/{owner}/{name}/issues/{}/labels", input.pr.number);
        if input.applied {
            self.rest_json(
                &input.pr.repo.host,
                &input.pr.repo.cwd,
                "setLabels",
                Method::POST,
                &path,
                Some(json!({"labels":input.labels})),
            )
            .await?;
        } else {
            for label in input.labels {
                self.rest_json(
                    &input.pr.repo.host,
                    &input.pr.repo.cwd,
                    "setLabels",
                    Method::DELETE,
                    &format!("{path}/{}", encode(&label)),
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }
    async fn get_files_viewed(&self, input: PullRequestInput) -> Result<ProviderFilesViewed> {
        let (files, truncated) = self
            .files_connection(&input, "PullRequestFilesViewed")
            .await?;
        let files:Vec<_>=files.iter().filter_map(|f|Some(json!({"path":text(f,"path")?,"state":match text(f,"viewerViewedState"){Some("VIEWED")=>"viewed",Some("DISMISSED")=>"dismissed",_=>"unviewed"}}))).collect();
        parse(
            json!({"files":files,"truncated":truncated}),
            "getFilesViewed",
        )
    }
    async fn get_file_stats(&self, input: PullRequestInput) -> Result<ProviderFileStats> {
        let (files, truncated) = self
            .files_connection(&input, "PullRequestFileStats")
            .await?;
        let files:Vec<_>=files.iter().filter_map(|f|Some(json!({"path":text(f,"path")?,"additions":f["additions"],"deletions":f["deletions"]}))).collect();
        parse(json!({"files":files,"truncated":truncated}), "getFileStats")
    }
    async fn set_files_viewed(&self, input: SetFilesViewedInput) -> Result<()> {
        if input.files.is_empty() {
            return Ok(());
        }
        let id = self.node_id(&input.pr).await?;
        for files in input.files.chunks(100) {
            let mut fields = String::new();
            let mut vars = json!({"id":id});
            let mut declarations = vec!["$id: ID!".to_string()];
            for (i, file) in files.iter().enumerate() {
                declarations.push(format!("$p{i}: String!"));
                vars[format!("p{i}")] = json!(file.path);
                fields.push_str(&format!(
                    "f{i}: {}(input: {{pullRequestId: $id, path: $p{i}}}) {{clientMutationId}}\n",
                    if file.viewed {
                        "markFileAsViewed"
                    } else {
                        "unmarkFileAsViewed"
                    }
                ));
            }
            let document = format!(
                "mutation SetFilesViewed({}) {{{fields}}}",
                declarations.join(", ")
            );
            self.http
                .graphql_document(
                    &input.pr.repo.host,
                    &input.pr.repo.cwd,
                    "SetFilesViewed",
                    &document,
                    vars,
                )
                .await?;
        }
        Ok(())
    }

    async fn list_change_requests(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        self.list_page(input).await
    }
    async fn list_change_requests_across(
        &self,
        input: ListAcrossInput,
    ) -> Result<ProviderBatchedChangeRequestPage> {
        let q = search_query(&input.list, &input.repositories)?;
        if input.repositories.is_empty() {
            return Err(error(
                "listChangeRequestsAcross",
                FailureReason::Failed,
                "At least one repository is required.",
            ));
        }
        let value=self.http.graphql(&input.list.repo.host,&input.list.repo.cwd,"PullRequestSearch",json!({"q":q,"first":input.list.limit.saturating_add(1).min(100),"stacks":input.list.repo.host.eq_ignore_ascii_case("github.com"),"stats":false})).await?;
        let connection = &value["search"];
        let raw = nodes(connection);
        let truncated =
            raw.len() > input.list.limit || connection["pageInfo"]["hasNextPage"] == true;
        let mut items = Vec::new();
        for row in raw {
            if let (Some(repository), Ok(item)) = (
                text(&row["repository"], "nameWithOwner"),
                list_item(row, &input.list.repo.host),
            ) {
                let mut value = serde_json::to_value(item)
                    .map_err(|_| json_error("listChangeRequestsAcross"))?;
                value["repository"] = json!(repository);
                items.push(parse(value, "listChangeRequestsAcross")?);
            }
        }
        items.truncate(input.list.limit);
        let answered: HashSet<_> = items
            .iter()
            .map(|item: &ProviderBatchedChangeRequest| item.repository.to_lowercase())
            .collect();
        let credential = self
            .http
            .credential(
                &input.list.repo.host,
                std::path::Path::new(&input.list.repo.cwd),
            )
            .await?;
        let coverage_key = |repository: &str| {
            format!(
                "{}\0{}\0{}",
                credential.fingerprint,
                input.list.repo.host.to_lowercase(),
                repository.trim().to_lowercase()
            )
        };
        {
            let mut cache = self.search_coverage.lock().await;
            for repository in &answered {
                cache.insert(coverage_key(repository), (super::transport::now(), true));
            }
            if cache.len() > 4096 {
                cache.retain(|_, (at, _)| super::transport::now().saturating_sub(*at) < 3_600_000);
            }
        }
        let silent: Vec<String> = input
            .repositories
            .iter()
            .filter(|repo| !answered.contains(&repo.to_lowercase()))
            .cloned()
            .collect();
        let unresolved:Vec<_>=stream::iter(silent).map(|repository| {
            let coverage_key=&coverage_key; let input=&input;
            async move {
                if input.list.cursor.is_none() && truncated {return Some(repository.clone());}
                let key=coverage_key(&repository);
                if let Some((_,covered))=self.search_coverage.lock().await.get(&key).filter(|(at,_)|super::transport::now().saturating_sub(*at)<3_600_000) {return (!covered).then(||repository.clone());}
                let coverage=ListInput {repo:ProviderRepositoryRef {repository:repository.clone(),..input.list.repo.clone()},state:PullRequestListState::All,involvement:PullRequestInvolvement::All,viewer:input.list.viewer.clone(),limit:1,query:None,cursor:None,filters:None};
                let q=match search_query(&coverage,std::slice::from_ref(&repository)) {Ok(q)=>q,Err(_)=>return Some(repository.clone())};
                match self.http.graphql(&coverage.repo.host,&coverage.repo.cwd,"PullRequestSearch",json!({"q":q,"first":2,"stacks":coverage.repo.host.eq_ignore_ascii_case("github.com"),"stats":false})).await {
                    Ok(value)=>{
                        let covered=nodes(&value["search"]).iter().any(|r|list_item(r,&coverage.repo.host).is_ok());
                        let mut cache=self.search_coverage.lock().await;
                        if cache.len()>=4096 {cache.clear();}
                        cache.insert(key,(super::transport::now(),covered));
                        (!covered).then(||repository.clone())
                    }
                    Err(_)=>Some(repository.clone())
                }
            }
        }).buffered(4).filter_map(|value|async move {value}).collect().await;
        Ok(ProviderBatchedChangeRequestPage {
            items,
            truncated,
            unresolved: Some(unresolved),
        })
    }
    async fn list_change_request_stats(
        &self,
        input: StatsInput,
    ) -> Result<Vec<ProviderChangeRequestStat>> {
        let results: Vec<Result<Option<ProviderChangeRequestStat>>> = stream::iter(input.change_requests).map(|key| {
            let host=&input.host; let cwd=&input.cwd;
            async move {
                let target=PullRequestInput {repo:ProviderRepositoryRef {cwd:cwd.to_string_lossy().into(),repository:key.repository.clone(),host:host.clone()},number:key.number};
                match self.query(&target,"PullRequestStats",Value::Null).await {
                    Ok(value)=>if let Ok(raw)=pr(&value) {parse(json!({"repository":key.repository,"number":key.number,"additions":raw["additions"],"deletions":raw["deletions"]}),"listChangeRequestStats").map(Some)} else {Ok(None)},
                    Err(error) if error.reason()==Some(FailureReason::NotFound)=>Ok(None),
                    Err(error)=>Err(error),
                }
            }
        }).buffered(100).collect().await;
        Ok(results
            .into_iter()
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect())
    }
    async fn list_reviewer_candidates(
        &self,
        input: CandidateInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        let value = self
            .query(
                &input.pr,
                "ReviewerCandidates",
                json!({"query":input.query}),
            )
            .await?;
        let repo = &value["repository"];
        if repo.is_null() {
            return Err(error(
                "listReviewerCandidates",
                FailureReason::NotFound,
                "Repository not found.",
            ));
        }
        let author = text(&repo["pullRequest"]["author"], "login");
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for node in nodes(&repo["pullRequest"]["reviewRequests"]) {
            let raw = &node["requestedReviewer"];
            let team = text(raw, "slug");
            let Some(id) = team.or_else(|| text(raw, "login")) else {
                continue;
            };
            let kind = if team.is_some() { "team" } else { "user" };
            if !seen.insert((kind.to_string(), id.to_string())) {
                continue;
            }
            candidates.push(json!({"id":id,"kind":kind,"login":if team.is_some(){text(raw,"combinedSlug").unwrap_or(id)}else{id},"name":text(raw,"teamName").or_else(||text(raw,"name")),"avatarUrl":text(raw,"teamAvatarUrl").or_else(||text(raw,"avatarUrl")),"isRequested":true}));
        }
        let mut matches = HashSet::new();
        for raw in nodes(&repo["assignableUsers"]) {
            let Some(login) = text(raw, "login") else {
                continue;
            };
            matches.insert(login.to_string());
            if Some(login) == author || !seen.insert(("user".into(), login.into())) {
                continue;
            }
            candidates.push(json!({"id":login,"kind":"user","login":login,"name":text(raw,"name"),"avatarUrl":text(raw,"avatarUrl"),"isRequested":false}));
        }
        if input.query.as_ref().is_some_and(|s| !s.trim().is_empty()) {
            candidates.retain(|v| {
                v["kind"] == "user" && v["id"].as_str().is_some_and(|s| matches.contains(s))
            });
        }
        parse(
            json!({"candidates":candidates,"truncated":repo["assignableUsers"]["pageInfo"]["hasNextPage"]==true}),
            "listReviewerCandidates",
        )
    }
    async fn list_label_candidates(
        &self,
        input: PullRequestInput,
    ) -> Result<PullRequestLabelCandidateList> {
        let value = self.query(&input, "LabelCandidates", Value::Null).await?;
        let repo = &value["repository"];
        if repo.is_null() {
            return Err(error(
                "listLabelCandidates",
                FailureReason::NotFound,
                "Repository not found.",
            ));
        }
        let applied: Vec<_> = nodes(&repo["pullRequest"]["labels"])
            .into_iter()
            .filter_map(|l| text(l, "name"))
            .collect();
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for label in nodes(&repo["labels"]) {
            let Some(name) = text(label, "name") else {
                continue;
            };
            if !seen.insert(name) {
                continue;
            }
            candidates.push(json!({"name":name,"color":text(label,"color"),"description":text(label,"description"),"isApplied":applied.contains(&name)}));
        }
        let mut missing: Vec<_> = applied
            .iter()
            .filter(|n| !seen.contains(*n))
            .map(|name| json!({"name":name,"color":null,"description":null,"isApplied":true}))
            .collect();
        missing.extend(candidates);
        parse(
            json!({"candidates":missing,"truncated":repo["labels"]["pageInfo"]["hasNextPage"]==true}),
            "listLabelCandidates",
        )
    }
    async fn list_repository_labels(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestLabel>> {
        let mut vars = repo_variables(&input.repo)?;
        vars["query"] = json!(input.query);
        let value = self
            .http
            .graphql(&input.repo.host, &input.repo.cwd, "RepositoryLabels", vars)
            .await?;
        if value["repository"].is_null() {
            return Err(error(
                "listRepositoryLabels",
                FailureReason::NotFound,
                "Repository not found.",
            ));
        }
        let conn = &value["repository"]["labels"];
        let items: Vec<_> = nodes(conn)
            .into_iter()
            .filter_map(|l| Some(json!({"name":text(l,"name")?,"color":text(l,"color")})))
            .collect();
        parse(
            json!({"items":items,"truncated":conn["pageInfo"]["hasNextPage"]==true}),
            "listRepositoryLabels",
        )
    }
    async fn search_repository_authors(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestActor>> {
        let mut vars = repo_variables(&input.repo)?;
        vars["query"] = json!(input.query);
        let value = self
            .http
            .graphql(&input.repo.host, &input.repo.cwd, "RepositoryAuthors", vars)
            .await?;
        if value["repository"].is_null() {
            return Err(error(
                "searchRepositoryAuthors",
                FailureReason::NotFound,
                "Repository not found.",
            ));
        }
        let conn = &value["repository"]["mentionableUsers"];
        let items: Vec<_> = nodes(conn)
            .into_iter()
            .map(|v| actor(v, &input.repo.host))
            .filter(|v| !v.is_null())
            .collect();
        parse(
            json!({"items":items,"truncated":conn["pageInfo"]["hasNextPage"]==true}),
            "searchRepositoryAuthors",
        )
    }
    async fn search_repository_references(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestReferenceCandidate>> {
        let query = input.query.as_deref().unwrap_or("").trim();
        let exact = query.parse::<i32>().ok().filter(|n| *n > 0);
        let q = format!(
            "repo:{} {}sort:updated-desc",
            input.repo.repository,
            if query.is_empty() || query.bytes().all(|c| c.is_ascii_digit()) {
                String::new()
            } else {
                format!("in:title {} ", search_phrase(query))
            }
        );
        let mut vars = repo_variables(&input.repo)?;
        vars["q"] = json!(q);
        vars["number"] = json!(exact.unwrap_or(1));
        vars["exact"] = json!(exact.is_some());
        let value = self
            .http
            .graphql(
                &input.repo.host,
                &input.repo.cwd,
                "RepositoryReferences",
                vars,
            )
            .await?;
        let exact = &value["repository"]["issueOrPullRequest"];
        let mut rows = if exact.is_null() { vec![] } else { vec![exact] };
        rows.extend(nodes(&value["search"]));
        let items:Vec<_>=rows.into_iter().filter_map(|r|{let kind=match r["__typename"].as_str(){Some("Issue")=>"issue",Some("PullRequest")=>"change-request",_=>return None};let s=if kind=="issue"{&r["issueState"]}else{&r["pullRequestState"]};Some(json!({"reference":format!("#{}",r["number"]),"kind":kind,"number":r["number"],"title":r["title"],"state":if s=="OPEN"{"open"}else if s=="MERGED"{"merged"}else{"closed"}}))}).collect();
        parse(
            json!({"items":items,"truncated":value["search"]["pageInfo"]["hasNextPage"]==true}),
            "searchRepositoryReferences",
        )
    }
    async fn get_change_request_stack(
        &self,
        input: StackInput,
    ) -> Result<Option<ProviderChangeRequestStack>> {
        let (owner, name) = repo_parts(&input.pr.repo.repository)?;
        let path = format!(
            "repos/{owner}/{name}/stacks?pull_request={}",
            input.pr.number
        );
        let value = match self
            .rest_json(
                &input.pr.repo.host,
                &input.pr.repo.cwd,
                "getPullRequestStack",
                Method::GET,
                &path,
                None,
            )
            .await
        {
            Ok(value) => value,
            Err(e) if e.reason() == Some(FailureReason::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(mut raw) = value.as_array().and_then(|a| a.first()).cloned() else {
            return Ok(None);
        };
        if input.include_details {
            raw = self
                .rest_json(
                    &input.pr.repo.host,
                    &input.pr.repo.cwd,
                    "getPullRequestStack",
                    Method::GET,
                    &format!("repos/{owner}/{name}/stacks/{}", raw["number"]),
                    None,
                )
                .await?;
        }
        let layers:Vec<_>=raw["pull_requests"].as_array().into_iter().flatten().map(|p|{let mut layer=json!({"number":p["number"],"headBranch":p["head"]["ref"],"state":if !p["merged_at"].is_null(){"merged"}else if p["state"]=="closed"{"closed"}else{"open"}});for (from,to)in[("title","title"),("draft","isDraft")] {if let Some(v)=p.get(from){layer[to]=v.clone();}}if let Some(sha)=p["head"].get("sha"){layer["headSha"]=sha.clone();}layer}).collect();
        let id = if raw["id"].is_null() {
            text(&raw, "node_id")
                .map(str::to_string)
                .unwrap_or_else(|| raw["number"].to_string())
        } else if let Some(s) = raw["id"].as_str() {
            s.into()
        } else {
            raw["id"].to_string()
        };
        parse(json!({"id":id,"number":raw["number"],"url":text(&raw,"html_url").or_else(||text(&raw,"url")),"base":raw["base"].as_str().map(Value::from).unwrap_or_else(||raw["base"]["ref"].clone()),"layers":layers}),"getPullRequestStack").map(Some)
    }
    async fn get_diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        self.read_diff(input).await
    }
    async fn get_diff_file_contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        self.read_file_contents(input).await
    }
    async fn run_action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        self.action(input).await
    }
    async fn checkout_change_request(&self, input: CheckoutGetInput) -> Result<()> {
        self.checkout(input).await
    }
    async fn get_change_request_activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        self.activity(input).await
    }
    async fn get_review_thread_comments(
        &self,
        input: ThreadCommentsInput,
    ) -> Result<PullRequestThreadCommentsResult> {
        self.thread_comments(input).await
    }
}
const SUPPORTED_OPERATIONS: &[&str] = &[
    "resolve_remote_web_host",
    "record_branch_base",
    "creates_change_requests_in",
    "resolve_link",
    "is_repository_starred",
    "star_repository",
    "read_change_request_template",
    "get_viewer",
    "get_routing_identity",
    "with_verified_credential",
    "checkout_list_change_requests",
    "checkout_get_change_request",
    "create_change_request",
    "get_repository_clone_urls",
    "create_repository",
    "get_default_branch",
    "get_change_request",
    "get_change_request_checks",
    "get_change_request_preview",
    "get_change_request_summary",
    "get_change_request_watch_fingerprint",
    "get_viewer_permissions",
    "comment",
    "update_change_request",
    "update_comment",
    "submit_review",
    "reply_to_thread",
    "set_thread_resolution",
    "set_reaction",
    "set_reviewer_request",
    "set_labels",
    "get_files_viewed",
    "get_file_stats",
    "set_files_viewed",
    "list_change_requests",
    "list_change_requests_across",
    "list_change_request_stats",
    "list_reviewer_candidates",
    "list_label_candidates",
    "list_repository_labels",
    "search_repository_authors",
    "search_repository_references",
    "get_change_request_stack",
    "get_diff",
    "get_diff_file_contents",
    "run_action",
    "checkout_change_request",
    "get_change_request_activity",
    "get_review_thread_comments",
];
pub fn search_phrase(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn search_query(input: &ListInput, repositories: &[String]) -> Result<String> {
    let mut parts = vec!["is:pr".to_string()];
    match input.state {
        PullRequestListState::Open => parts.push("is:open".into()),
        PullRequestListState::Closed => parts.extend(["is:closed".into(), "is:unmerged".into()]),
        PullRequestListState::Merged => parts.push("is:merged".into()),
        PullRequestListState::All => {}
    }
    match input.involvement {
        PullRequestInvolvement::Authored => parts.push(format!("author:{}", input.viewer)),
        PullRequestInvolvement::Reviewing => {
            parts.push(format!("review-requested:{}", input.viewer))
        }
        _ => {}
    }
    if let Some(query) = input.query.as_deref().filter(|q| !q.trim().is_empty()) {
        parts.push(search_phrase(query.trim()));
    }
    if let Some(cursor) = &input.cursor {
        parts.push(format!("updated:<= {}", cursor.updated_before).replace("<= ", "<="));
    }
    if let Some(filters) = &input.filters {
        let f = serde_json::to_value(filters).map_err(|_| json_error("filters"))?;
        if let Some(groups) = f["labels"].as_array() {
            for group in groups {
                let values: Vec<_> = group
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(qualifier)
                    .collect();
                if !values.is_empty() {
                    parts.push(format!("label:{}", values.join(",")));
                }
            }
        }
        if let Some(labels) = f["excludedLabels"].as_array() {
            for label in labels.iter().filter_map(Value::as_str) {
                parts.push(format!("-label:{}", qualifier(label)));
            }
        }
        if let Some(author) = text(&f, "author") {
            parts.push(format!(
                "author:{}",
                qualifier(if author == "@me" {
                    &input.viewer
                } else {
                    author
                })
            ));
        }
        if let Some(draft) = text(&f, "draft") {
            parts.push(format!("draft:{}", draft == "only"));
        }
        if let Some(review) = text(&f, "review") {
            parts.push(format!(
                "review:{}",
                match review {
                    "changes-requested" => "changes_requested",
                    "review-required" => "required",
                    _ => review,
                }
            ));
        }
        if let Some(checks) = text(&f, "checks") {
            parts.push(format!(
                "status:{}",
                if checks == "passing" {
                    "success"
                } else {
                    "failure"
                }
            ));
        }
    }
    parts.push("sort:updated-desc".into());
    for repository in repositories {
        repo_parts(repository)?;
        parts.push(format!("repo:{repository}"));
    }
    Ok(parts.join(" "))
}
fn qualifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "").trim())
}
impl GitHub {
    async fn list_page(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        let q = search_query(&input, std::slice::from_ref(&input.repo.repository))?;
        let rows = input.limit.saturating_add(1);
        let (raw, raw_count, mut truncated) = self.collect_listing(&input, &q, true, rows).await?;
        let mut items: Vec<_> = raw
            .iter()
            .filter_map(|r| list_item(r, &input.repo.host).ok())
            .collect();
        let mut continues = true;
        if items.is_empty()
            && input.cursor.is_none()
            && input.query.as_deref().is_none_or(|q| q.trim().is_empty())
        {
            continues = false;
            let max = rows.max(1000);
            let mut requested = rows;
            loop {
                let (raw, count, cut) = self.collect_listing(&input, &q, false, requested).await?;
                items = raw
                    .iter()
                    .filter_map(|r| list_item(r, &input.repo.host).ok().map(|item| (item, r)))
                    .filter(|(item, raw)| matches_listing(item, raw, &input))
                    .map(|(item, _)| item)
                    .collect();
                if items.len() < input.limit && count >= requested && requested < max {
                    requested = requested.saturating_mul(2).min(max);
                    continue;
                }
                truncated = cut || items.len() > input.limit || count >= requested;
                break;
            }
        } else {
            truncated |= raw_count > input.limit;
        }
        items.truncate(input.limit);
        Ok(ProviderChangeRequestPage {
            items,
            truncated,
            cursor_advance: None,
            resume_after: None,
            continues,
        })
    }
    async fn collect_listing(
        &self,
        input: &ListInput,
        q: &str,
        search: bool,
        rows: usize,
    ) -> Result<(Vec<Value>, usize, bool)> {
        let (owner, name) = repo_parts(&input.repo.repository)?;
        let states = match input.state {
            PullRequestListState::Open => vec!["OPEN"],
            PullRequestListState::Closed => vec!["CLOSED"],
            PullRequestListState::Merged => vec!["MERGED"],
            PullRequestListState::All => vec!["OPEN", "CLOSED", "MERGED"],
        };
        let (mut after, mut seen, mut raw) = (None::<String>, HashSet::new(), Vec::new());
        loop {
            let first = rows.saturating_sub(raw.len()).clamp(1, 100);
            let vars = if search {
                json!({"q":q,"first":first,"after":after,"stacks":false,"stats":true})
            } else {
                json!({"owner":owner,"name":name,"states":states,"first":first,"after":after,"stacks":false,"stats":true})
            };
            let value = self
                .http
                .graphql(
                    &input.repo.host,
                    &input.repo.cwd,
                    if search {
                        "PullRequestSearch"
                    } else {
                        "PullRequestList"
                    },
                    vars,
                )
                .await?;
            let connection = if search {
                &value["search"]
            } else {
                &value["repository"]["pullRequests"]
            };
            raw.extend(nodes(connection).into_iter().cloned());
            let Some(next) = next_cursor(connection) else {
                let count = raw.len();
                return Ok((raw, count, false));
            };
            if !seen.insert(next.clone()) || raw.len() >= rows {
                let count = raw.len();
                return Ok((raw, count, true));
            }
            after = Some(next);
        }
    }
}

impl GitHub {
    async fn action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        if input.stack_number.is_some() {
            self.stack_action(input).await?;
            return Ok(None);
        }
        if input.action == PullRequestAction::ApproveWorkflows {
            self.approve_workflows(&input.pr).await?;
            return Ok(None);
        }
        if input.action == PullRequestAction::DeleteBranch {
            self.delete_branch(&input.pr).await?;
            return Ok(None);
        }
        let simple = match input.action {
            PullRequestAction::Ready => Some((
                "MarkPullRequestReadyForReview",
                "markPullRequestReadyForReview",
            )),
            PullRequestAction::Draft => {
                Some(("ConvertPullRequestToDraft", "convertPullRequestToDraft"))
            }
            PullRequestAction::Close => Some(("ClosePullRequest", "closePullRequest")),
            PullRequestAction::Reopen => Some(("ReopenPullRequest", "reopenPullRequest")),
            PullRequestAction::DisableAutoMerge => {
                Some(("DisablePullRequestAutoMerge", "disablePullRequestAutoMerge"))
            }
            _ => None,
        };
        if let Some((operation, field)) = simple {
            let id = self.node_id(&input.pr).await?;
            let value = self
                .mutation(&input.pr, operation, json!({"pullRequestId":id}))
                .await?;
            return Ok(summary(&value[field]["pullRequest"], &input.pr.repo.host).ok());
        }
        if input.action == PullRequestAction::Revert {
            let id = self.node_id(&input.pr).await?;
            self.mutation(&input.pr, "RevertPullRequest", json!({"pullRequestId":id}))
                .await?;
            return Ok(None);
        }
        let value = self
            .query(
                &input.pr,
                "PullRequestActionState",
                json!({"headRef":format!("refs/pull/{}/head",input.pr.number)}),
            )
            .await?;
        let raw = pr(&value)?;
        if input.action == PullRequestAction::UpdateBranch {
            if raw["baseRef"]["compare"]["behindBy"] == 0 {
                return Ok(None);
            }
            let value=self.mutation(&input.pr,"UpdatePullRequestBranch",json!({"pullRequestId":raw["id"],"expectedHeadOid":raw["headRefOid"],"updateMethod":if input.update_method==Some(PullRequestUpdateMethod::Rebase){"REBASE"}else{"MERGE"}})).await?;
            return Ok(summary(
                &value["updatePullRequestBranch"]["pullRequest"],
                &input.pr.repo.host,
            )
            .ok());
        }
        if !matches!(
            input.action,
            PullRequestAction::Merge | PullRequestAction::EnableAutoMerge
        ) {
            return Err(ForgeError::unsupported(&self.kind(), "action"));
        }
        let method = match input.merge_method {
            Some(PullRequestMergeMethod::Squash) => "SQUASH",
            Some(PullRequestMergeMethod::Rebase) => "REBASE",
            _ => "MERGE",
        };
        let mut vars = json!({"pullRequestId":raw["id"],"mergeMethod":method});
        if input.remove_agent_credits_on_merge && method != "REBASE" {
            let message = self
                .query(
                    &input.pr,
                    "PullRequestMergeMessage",
                    json!({"method":method}),
                )
                .await?;
            let msg = pr(&message)?;
            if msg["isMergeQueueEnabled"] != true
                && let Some(body) = msg["viewerMergeBodyText"].as_str()
            {
                let cleaned = super::merge_message::remove_agent_credits(body);
                if cleaned != body {
                    vars["commitBody"] = json!(cleaned);
                    vars["expectedHeadOid"] = msg["headRefOid"].clone();
                }
            }
        }
        let auto = raw["isMergeQueueEnabled"] == true
            || (input.action == PullRequestAction::EnableAutoMerge
                && !matches!(
                    text(raw, "mergeStateStatus"),
                    Some("CLEAN" | "HAS_HOOKS" | "UNSTABLE")
                ));
        let (operation, field) = if auto {
            ("EnablePullRequestAutoMerge", "enablePullRequestAutoMerge")
        } else {
            ("MergePullRequest", "mergePullRequest")
        };
        let value = self
            .mutation(&input.pr, operation, json!({"input":vars}))
            .await?;
        Ok(summary(&value[field]["pullRequest"], &input.pr.repo.host).ok())
    }
    async fn delete_branch(&self, input: &PullRequestInput) -> Result<()> {
        let value = self
            .query(input, "PullRequestBranchToDelete", Value::Null)
            .await?;
        let raw = pr(&value)?;
        let refused = |why| error("delete-branch", FailureReason::Failed, why);
        if raw["state"] == "OPEN" {
            return Err(refused(
                "An open pull request's source branch cannot be deleted.",
            ));
        }
        let repository = text(&raw["headRepository"], "id")
            .ok_or_else(|| refused("The source repository is unavailable."))?;
        let branch =
            text(raw, "headRefName").ok_or_else(|| refused("The source branch is unavailable."))?;
        let sha = text(raw, "headRefOid")
            .ok_or_else(|| refused("The source revision is unavailable."))?;
        if raw["baseRefName"] == branch {
            return Err(refused("The base branch cannot be deleted."));
        }
        let reference = format!("refs/heads/{branch}");
        let value = self
            .mutation(
                input,
                "PullRequestSourceBranch",
                json!({"repositoryId":repository,"ref":reference}),
            )
            .await?;
        let repo = &value["node"];
        if repo.is_null() {
            return Err(refused("The source repository is unavailable."));
        }
        if repo["ref"].is_null() {
            return Ok(());
        }
        if repo["defaultBranchRef"].is_null() || repo["defaultBranchRef"]["name"] == branch {
            return Err(refused("The default branch cannot be deleted."));
        }
        if repo["ref"]["target"]["oid"] != sha {
            return Err(refused(
                "The source branch changed after this pull request.",
            ));
        }
        self.mutation(input,"DeletePullRequestBranch",json!({"input":{"repositoryId":repository,"refUpdates":[{"name":reference,"beforeOid":sha,"afterOid":"0000000000000000000000000000000000000000"}]}})).await?;
        Ok(())
    }
    async fn checkout(&self, input: CheckoutGetInput) -> Result<()> {
        let record = self.checkout_record(input.clone()).await?;
        let base = if let Some(link) = crate::remote::parse_pull_request_url(input.reference.trim())
            .filter(|l| l.provider == "github")
        {
            parse_repository_selector(&link.repository, &link.authority)
                .ok_or_else(|| json_error("checkout"))?
        } else {
            self.resolve_repository(&input.checkout).await?
        };
        let cwd = &input.checkout.cwd;
        let head = record.head_ref_name.0;
        let head_repo = record
            .head_repository_name_with_owner
            .flatten()
            .map(|s| s.0);
        let cross = record.is_cross_repository.flatten().unwrap_or_else(|| {
            head_repo
                .as_ref()
                .is_some_and(|r| !r.eq_ignore_ascii_case(&format!("{}/{}", base.owner, base.name)))
        });
        let owner = record
            .head_repository_owner_login
            .flatten()
            .map(|s| s.0)
            .or_else(|| {
                head_repo
                    .as_ref()
                    .and_then(|s| s.split('/').next().map(str::to_string))
            });
        let default = if cross {
            let v = self.repository_json(&base, &cwd.to_string_lossy()).await?;
            text(&v, "default_branch").map(str::to_string)
        } else {
            None
        };
        if cross && default.is_none() {
            return Err(error(
                "checkout",
                FailureReason::Failed,
                "The base repository's default branch is unavailable.",
            ));
        }
        let branch = super::repository::checkout_branch_name(
            &head,
            owner.as_deref(),
            cross,
            default.as_deref(),
        );
        let remotes = self
            .git(cwd, vec!["remote".into(), "-v".into()], false)
            .await?;
        let remotes = crate::registry::parse_fetch_remotes(&remotes);
        let remote_for = |repository: &str| {
            remotes
                .iter()
                .find(|(_, url)| {
                    crate::remote::parse_remote(url).is_some_and(|r| {
                        super::repository::api_host_for_remote(url).unwrap_or(r.host) == base.host
                            && r.repository
                                .is_some_and(|s| s.eq_ignore_ascii_case(repository))
                    })
                })
                .map(|(name, _)| name.clone())
        };
        let base_remote = remote_for(&format!("{}/{}", base.owner, base.name))
            .or_else(|| {
                remotes
                    .iter()
                    .find(|(name, _)| name == "origin")
                    .map(|r| r.0.clone())
            })
            .or_else(|| remotes.first().map(|r| r.0.clone()))
            .ok_or_else(|| {
                error(
                    "checkout",
                    FailureReason::Failed,
                    "No repository remote is available.",
                )
            })?;
        let mut head_remote = if cross {
            head_repo.as_deref().and_then(remote_for)
        } else {
            Some(base_remote.clone())
        };
        let local = self
            .git(
                cwd,
                vec![
                    "for-each-ref".into(),
                    "--format=%(refname:short)".into(),
                    "refs/heads/".into(),
                ],
                false,
            )
            .await?;
        let exists = local.lines().any(|s| s == branch);
        let git = self
            .git_driver
            .at(cwd)
            .with_environment(self.git_environment.clone());
        if cross
            && head_remote.is_none()
            && let Some(repository) = &head_repo
        {
            let fork = parse_repository_selector(repository, &base.host)
                .ok_or_else(|| json_error("checkout"))?;
            if let Ok(value) = self.repository_json(&fork, &cwd.to_string_lossy()).await {
                let origin = remotes
                    .iter()
                    .find(|(name, _)| name == "origin")
                    .and_then(|(_, url)| crate::remote::parse_remote(url));
                let remote_url = if origin.is_some_and(|r| r.ssh) {
                    text(&value, "ssh_url")
                } else {
                    text(&value, "html_url")
                };
                if let Some(remote_url) = remote_url.filter(|url| {
                    crate::remote::parse_remote(url).is_some_and(|r| {
                        r.host == base.host
                            && r.repository
                                .is_some_and(|name| name.eq_ignore_ascii_case(repository))
                    })
                }) {
                    head_remote = git
                        .ensure_remote(owner.as_deref().unwrap_or("fork"), remote_url)
                        .await
                        .ok();
                }
            }
        }
        let target = if let Some(remote) = &head_remote {
            git.fetch_remote_tracking_branch(remote, &head)
                .await
                .ok()
                .map(|_| format!("refs/remotes/{remote}/{head}"))
        } else {
            None
        };
        let (target, tracking) = if let Some(target) = target {
            (target, head_remote)
        } else {
            self.git(
                cwd,
                vec![
                    "fetch".into(),
                    "--quiet".into(),
                    "--no-tags".into(),
                    "--".into(),
                    base_remote,
                    format!("refs/pull/{}/head", record.number),
                ],
                false,
            )
            .await?;
            let sha = self
                .git(
                    cwd,
                    vec![
                        "rev-parse".into(),
                        "--verify".into(),
                        "FETCH_HEAD^{commit}".into(),
                    ],
                    false,
                )
                .await?;
            (sha.trim().into(), None)
        };
        let failure = |_| {
            error(
                "checkout",
                FailureReason::Failed,
                "The pull request could not be checked out with git.",
            )
        };
        if !exists {
            git.create_branch(&branch, &target).await.map_err(failure)?;
        }
        git.switch_ref(&branch).await.map_err(failure)?;
        if exists {
            let args = if input.force {
                vec!["reset".into(), "--hard".into(), "--quiet".into(), target]
            } else {
                vec!["merge".into(), "--ff-only".into(), "--quiet".into(), target]
            };
            self.git(cwd, args, false).await?;
        }
        if let Some(remote) = tracking {
            git.set_branch_upstream(&branch, &remote, &head)
                .await
                .map_err(failure)?;
        }
        Ok(())
    }
    async fn read_diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        if let Some(commit) = &input.commit
            && !valid_sha(commit)
        {
            return Err(error(
                "getDiff",
                FailureReason::Failed,
                "Invalid diff commit.",
            ));
        }
        if input.cursor.is_none() && input.commit.is_none() {
            let endpoint = path(&input.pr, "")?;
            let result = self
                .http
                .send(Request {
                    host: &input.pr.repo.host,
                    cwd: &input.pr.repo.cwd,
                    operation: "getPullRequestDiff",
                    method: Method::GET,
                    path: &endpoint,
                    body: None,
                    graphql: false,
                    interactive: true,
                    accept: "application/vnd.github.diff",
                    max_bytes: 8 * 1024 * 1024,
                    timeout: Duration::from_secs(60),
                    if_none_match: None,
                })
                .await;
            match result {
                Ok(response) if !response.truncated => {
                    return Ok(ProviderDiffSlice {
                        patch: String::from_utf8_lossy(&response.body).into(),
                        truncated: false,
                        next_cursor: None,
                        omitted_file_stats: None,
                    });
                }
                Err(e) if e.reason() != Some(FailureReason::Failed) => return Err(e),
                _ => {}
            }
        }
        let page = if let Some(cursor) = input.cursor {
            if cursor.len() > 7
                || cursor.starts_with('0')
                || !cursor.bytes().all(|c| c.is_ascii_digit())
            {
                return Err(error(
                    "getDiff",
                    FailureReason::Failed,
                    "Invalid diff cursor.",
                ));
            }
            cursor.parse::<u32>().map_err(|_| json_error("getDiff"))?
        } else {
            1
        };
        let endpoint = if let Some(commit) = &input.commit {
            let (owner, name) = repo_parts(&input.pr.repo.repository)?;
            format!("repos/{owner}/{name}/commits/{commit}?per_page=100&page={page}")
        } else {
            path(&input.pr, &format!("/files?per_page=100&page={page}"))?
        };
        let response = self
            .http
            .send(Request {
                host: &input.pr.repo.host,
                cwd: &input.pr.repo.cwd,
                operation: "getPullRequestDiff",
                method: Method::GET,
                path: &endpoint,
                body: None,
                graphql: false,
                interactive: true,
                accept: "application/vnd.github+json",
                max_bytes: 8 * 1024 * 1024,
                timeout: Duration::from_secs(60),
                if_none_match: None,
            })
            .await?;
        if response.truncated {
            return Err(error(
                "getDiff",
                FailureReason::Failed,
                "Changed files page exceeded the byte limit.",
            ));
        }
        let value: Value =
            serde_json::from_slice(&response.body).map_err(|_| json_error("getDiff"))?;
        let rows = if input.commit.is_some() {
            value["files"].as_array()
        } else {
            value.as_array()
        }
        .ok_or_else(|| json_error("getDiff"))?;
        let (mut patch, mut omitted) = (String::new(), Vec::new());
        for raw in rows {
            let Some(name) = text(raw, "filename") else {
                continue;
            };
            let status = text(raw, "status").unwrap_or("");
            let old = if status == "renamed" {
                text(raw, "previous_filename").unwrap_or(name)
            } else {
                name
            };
            let hunks = raw["patch"].as_str().unwrap_or("");
            let additions = raw["additions"].as_u64().unwrap_or(0);
            let deletions = raw["deletions"].as_u64().unwrap_or(0);
            if hunks.is_empty() && additions + deletions > 0 {
                omitted.push(json!({"path":name,"additions":additions,"deletions":deletions}));
            }
            patch.push_str(&format!(
                "diff --git {} {}\n",
                quote_patch_path(&format!("a/{old}")),
                quote_patch_path(&format!("b/{name}"))
            ));
            if status == "added" {
                patch.push_str("new file mode 100644\n");
            }
            if status == "removed" {
                patch.push_str("deleted file mode 100644\n");
            }
            if status == "renamed" {
                patch.push_str(&format!(
                    "similarity index {}%\nrename from {}\nrename to {}\n",
                    if hunks.is_empty() && additions + deletions == 0 {
                        100
                    } else {
                        0
                    },
                    quote_patch_path(old),
                    quote_patch_path(name)
                ));
            }
            patch.push_str(&format!(
                "--- {}\n+++ {}\n",
                if status == "added" {
                    "/dev/null".into()
                } else {
                    quote_patch_path(&format!("a/{old}"))
                },
                if status == "removed" {
                    "/dev/null".into()
                } else {
                    quote_patch_path(&format!("b/{name}"))
                }
            ));
            if !hunks.is_empty() {
                patch.push_str(hunks.trim_end_matches('\n'));
                patch.push('\n');
            }
        }
        parse(
            json!({"patch":patch,"truncated":!omitted.is_empty(),"nextCursor":if rows.len()>=100{Some((page+1).to_string())}else{None},"omittedFileStats":omitted}),
            "getDiff",
        )
    }
    async fn read_file_contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        let (owner, name) = repo_parts(&input.pr.repo.repository)?;
        if input.commit.as_deref().is_some_and(|s| !valid_sha(s)) {
            return Err(error(
                "getDiffFileContents",
                FailureReason::Failed,
                "Invalid diff commit.",
            ));
        }
        let endpoint = if let Some(commit) = &input.commit {
            format!("repos/{owner}/{name}/commits/{commit}")
        } else {
            path(&input.pr, "")?
        };
        let refs = self
            .rest_json(
                &input.pr.repo.host,
                &input.pr.repo.cwd,
                "getPullRequestDiffFileContents",
                Method::GET,
                &endpoint,
                None,
            )
            .await?;
        let head = if input.commit.is_some() {
            text(&refs, "sha")
        } else {
            text(&refs["head"], "sha")
        }
        .ok_or_else(|| {
            error(
                "getDiffFileContents",
                FailureReason::Failed,
                "Diff revisions are unavailable.",
            )
        })?;
        let base = if input.commit.is_some() {
            text(&refs["parents"][0], "sha").unwrap_or("")
        } else {
            text(&refs["base"], "sha").unwrap_or("")
        };
        if !valid_sha(head)
            || (!valid_sha(base)
                && !(input.commit.is_some()
                    && input.change_type == PullRequestDiffFileContentsInputChangeType::New
                    && base.is_empty()))
        {
            return Err(error(
                "getDiffFileContents",
                FailureReason::Failed,
                "Diff revisions are unavailable.",
            ));
        }
        let mut old = base.to_string();
        if input.commit.is_none()
            && input.change_type != PullRequestDiffFileContentsInputChangeType::New
            && let Ok(value) = self
                .rest_json(
                    &input.pr.repo.host,
                    &input.pr.repo.cwd,
                    "getPullRequestDiffFileContents",
                    Method::GET,
                    &format!("repos/{owner}/{name}/compare/{base}...{head}?per_page=1&page=2"),
                    None,
                )
                .await
            && let Some(sha) = text(&value["merge_base_commit"], "sha").filter(|s| valid_sha(s))
        {
            old = sha.into();
        }
        let repository_ref = &input.pr.repo;
        let read = |revision: String, file: String| async move {
            let encoded = file.split('/').map(encode).collect::<Vec<_>>().join("/");
            let endpoint = format!(
                "repos/{owner}/{name}/contents/{encoded}?ref={}",
                encode(&revision)
            );
            let response = self
                .http
                .send(Request {
                    host: &repository_ref.host,
                    cwd: &repository_ref.cwd,
                    operation: "getPullRequestDiffFileContents",
                    method: Method::GET,
                    path: &endpoint,
                    body: None,
                    graphql: false,
                    interactive: true,
                    accept: "application/vnd.github.raw+json",
                    max_bytes: 1024 * 1024,
                    timeout: Duration::from_secs(60),
                    if_none_match: None,
                })
                .await?;
            if response.truncated || response.body.contains(&0) {
                return Err(error(
                    "getDiffFileContents",
                    FailureReason::Failed,
                    "File contents are oversized or binary.",
                ));
            }
            String::from_utf8(response.body).map_err(|_| {
                error(
                    "getDiffFileContents",
                    FailureReason::Failed,
                    "File contents are binary.",
                )
            })
        };
        let old_contents = if input.change_type == PullRequestDiffFileContentsInputChangeType::New {
            String::new()
        } else {
            read(old, input.old_path.clone()).await?
        };
        let new_contents =
            if input.change_type == PullRequestDiffFileContentsInputChangeType::Deleted {
                String::new()
            } else {
                read(head.into(), input.new_path.clone()).await?
            };
        Ok(ProviderDiffFileContents {
            old_contents,
            new_contents,
        })
    }
}
fn valid_sha(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|c| c.is_ascii_hexdigit())
}
fn quote_patch_path(value: &str) -> String {
    let mut body = String::new();
    let mut quoting = false;
    for c in value.chars() {
        let escape = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\u{7}' => Some("\\a"),
            '\u{8}' => Some("\\b"),
            '\t' => Some("\\t"),
            '\n' => Some("\\n"),
            '\u{b}' => Some("\\v"),
            '\u{c}' => Some("\\f"),
            '\r' => Some("\\r"),
            _ => None,
        };
        if let Some(escape) = escape {
            quoting = true;
            body.push_str(escape);
        } else if (c as u32) < 32 || c == '\u{7f}' {
            quoting = true;
            body.push_str(&format!("\\{:03o}", c as u32));
        } else {
            body.push(c);
        }
    }
    if quoting { format!("\"{body}\"") } else { body }
}

fn reactions(raw: &Value, viewer: Option<&str>) -> Value {
    let values:Vec<_>=raw["reactionGroups"].as_array().into_iter().flatten().filter_map(|r|{let content=match r["content"].as_str()?{"THUMBS_UP"=>"thumbs-up","THUMBS_DOWN"=>"thumbs-down","LAUGH"=>"laugh","HOORAY"=>"hooray","CONFUSED"=>"confused","HEART"=>"heart","ROCKET"=>"rocket","EYES"=>"eyes",_=>return None};let logins:Vec<_>=nodes(&r["reactors"]).into_iter().filter_map(|n|text(n,"login")).collect();let count=r["reactors"]["totalCount"].as_u64().unwrap_or(0).max(logins.len()as u64);if count==0{return None;}Some(json!({"content":content,"count":count,"actors":logins.into_iter().filter(|l|viewer.is_none_or(|v|!v.eq_ignore_ascii_case(l))).collect::<Vec<_>>(),"viewerHasReacted":r["viewerHasReacted"]==true}))}).collect();
    json!(values)
}
fn thread_comment(raw: &Value, host: &str, viewer: Option<&str>) -> Value {
    let mut value = json!({"id":raw["id"],"author":actor(&raw["author"],host),"body":raw["body"].as_str().unwrap_or(""),"createdAt":raw["createdAt"],"editedAt":raw["lastEditedAt"],"url":text(raw,"url"),"reactions":reactions(raw,viewer)});
    if let Some(url) = text(&raw["pullRequestReview"], "url") {
        value["reviewUrl"] = json!(url);
    }
    value
}
fn commit_rows(raw: &Value, host: &str) -> Vec<Value> {
    nodes(&raw["commits"]).into_iter().map(|n|{let c=&n["commit"];let authors:Vec<_>=nodes(&c["authors"]).into_iter().map(|a|{let login=text(&a["user"],"login").or_else(||text(a,"name")).or_else(||text(a,"email")).unwrap_or("");actor(&json!({"login":login,"name":text(a,"name"),"avatarUrl":text(a,"avatarUrl")}),host)}).filter(|a|!a.is_null()).collect();json!({"oid":c["oid"],"messageHeadline":c["messageHeadline"],"committedDate":c["committedDate"],"authors":authors})}).collect()
}
impl GitHub {
    async fn thread_comments(
        &self,
        input: ThreadCommentsInput,
    ) -> Result<PullRequestThreadCommentsResult> {
        let value = self
            .query(
                &input.pr,
                "ReviewThreadComments",
                json!({"threadId":input.thread_id,"after":input.cursor}),
            )
            .await?;
        let id = &pr(&value)?["id"];
        let thread = &value["node"];
        if id.is_null() || thread["pullRequest"]["id"] != *id {
            return Err(error(
                "getReviewThreadComments",
                FailureReason::Forbidden,
                "The review thread does not belong to this pull request.",
            ));
        }
        let loaded = nodes(&thread["comments"]);
        let visible: Vec<_> = loaded
            .iter()
            .filter(|c| c["isMinimized"] != true)
            .map(|c| thread_comment(c, &input.pr.repo.host, text(&value["viewer"], "login")))
            .collect();
        parse(
            json!({"comments":visible,"hiddenCount":loaded.len()-visible.len(),"nextCursor":next_cursor(&thread["comments"])}),
            "getReviewThreadComments",
        )
    }
    async fn activity(&self, input: PullRequestInput) -> Result<ProviderChangeRequestActivity> {
        let mut comments = Vec::new();
        let mut commits = Vec::new();
        let mut author = Value::Null;
        let (mut comments_after, mut reviews_after) = (None::<String>, None::<String>);
        let (mut with_comments, mut with_reviews) = (true, true);
        let mut seen_comments = HashSet::new();
        let mut seen_reviews = HashSet::new();
        let mut ids = Vec::new();
        for page in 0..10 {
            let value=self.query(&input,"PullRequestActivity",json!({"head":page==0,"withComments":with_comments,"commentsAfter":comments_after,"withReviews":with_reviews,"reviewsAfter":reviews_after})).await?;
            let raw = pr(&value)?;
            if page == 0 {
                author = actor(&raw["author"], &input.repo.host);
                commits = commit_rows(raw, &input.repo.host);
                ids = nodes(&raw["commits"])
                    .into_iter()
                    .filter(|c| c["commit"]["parents"]["totalCount"].as_u64().unwrap_or(0) <= 1)
                    .filter_map(|c| text(&c["commit"], "id").map(str::to_string))
                    .collect();
            }
            for (connection, kind, date) in [
                ("comments", "issue-comment", "createdAt"),
                ("reviews", "review", "submittedAt"),
            ] {
                for c in nodes(&raw[connection]) {
                    if c["isMinimized"] == true {
                        continue;
                    }
                    let Some(created) = text(c, date) else {
                        continue;
                    };
                    let body = c["body"].as_str().unwrap_or("");
                    if kind == "review"
                        && body.trim().is_empty()
                        && !matches!(
                            text(c, "state"),
                            Some("APPROVED" | "CHANGES_REQUESTED" | "DISMISSED")
                        )
                    {
                        continue;
                    }
                    comments.push(json!({"id":c["id"],"kind":kind,"body":body,"author":actor(&c["author"],&input.repo.host),"createdAt":created,"editedAt":c["lastEditedAt"],"url":text(c,"url"),"path":null,"reviewState":if kind=="review"{text(c,"state")}else{None}}));
                }
            }
            if with_comments {
                comments_after = next_cursor(&raw["comments"]);
                with_comments = comments_after
                    .as_ref()
                    .is_some_and(|c| seen_comments.insert(c.clone()));
            }
            if with_reviews {
                reviews_after = next_cursor(&raw["reviews"]);
                with_reviews = reviews_after
                    .as_ref()
                    .is_some_and(|c| seen_reviews.insert(c.clone()));
            }
            if !with_comments && !with_reviews {
                break;
            }
        }
        if !ids.is_empty()
            && let Ok(value) = self
                .http
                .graphql(
                    &input.repo.host,
                    &input.repo.cwd,
                    "PullRequestCommitStats",
                    json!({"ids":ids}),
                )
                .await
        {
            for stat in value["nodes"].as_array().into_iter().flatten() {
                for commit in &mut commits {
                    if commit["oid"] == stat["oid"] {
                        commit["additions"] = json!(stat["additions"].as_i64().unwrap_or(0).max(0));
                        commit["deletions"] = json!(stat["deletions"].as_i64().unwrap_or(0).max(0));
                    }
                }
            }
        }
        let base_comment_count = comments.len();
        let base_commits = commits.clone();
        let mut actor_metadata = BTreeMap::new();
        let mut review_failed = false;
        let mut dismissal_after = None;
        let mut threads = Vec::new();
        let mut reviewers = Vec::new();
        let mut reaction_groups = json!([]);
        let mut truncated = comments_after.is_some() || reviews_after.is_some();
        let mut threads_truncated = false;
        let mut comment_count = comments.len();
        let mut after = None::<String>;
        let mut seen = HashSet::new();
        let mut reactions_by_id = BTreeMap::new();
        let mut edits_by_id = BTreeMap::new();
        let mut dismissals = BTreeMap::new();
        for page in 0..10 {
            let result = self
                .query(
                    &input,
                    if page == 0 {
                        "ReviewThreads"
                    } else {
                        "ReviewThreadPages"
                    },
                    if page == 0 {
                        json!({"requestedReviewers":50})
                    } else {
                        json!({"after":after})
                    },
                )
                .await;
            let value = match result {
                Ok(value) => value,
                Err(_) => {
                    review_failed = true;
                    truncated = true;
                    threads_truncated = true;
                    break;
                }
            };
            let raw = match pr(&value) {
                Ok(raw) => raw,
                Err(_) => {
                    review_failed = true;
                    break;
                }
            };
            collect_actor_metadata(raw, &mut actor_metadata);
            let viewer = text(&value["viewer"], "login");
            if page == 0 {
                dismissal_after = next_cursor(&raw["reviewDismissals"]);
                let thread_commits = commit_rows(raw, &input.repo.host);
                if !thread_commits.is_empty() {
                    commits = thread_commits;
                    for commit in &mut commits {
                        if let Some(original) = base_commits
                            .iter()
                            .find(|original| original["oid"] == commit["oid"])
                        {
                            for key in ["additions", "deletions"] {
                                if let Some(value) = original.get(key) {
                                    commit[key] = value.clone();
                                }
                            }
                        }
                    }
                }
                reaction_groups = reactions(raw, viewer);
                reviewers = unique_actors(
                    nodes(&raw["reviewRequests"])
                        .into_iter()
                        .map(|r| actor(&r["requestedReviewer"], &input.repo.host))
                        .chain(
                            nodes(&raw["latestReviews"])
                                .into_iter()
                                .filter(|r| r["isMinimized"] != true)
                                .map(|r| actor(&r["author"], &input.repo.host)),
                        ),
                );
                for connection in ["comments", "reviews"] {
                    for remark in nodes(&raw[connection]) {
                        if let Some(id) = text(remark, "id") {
                            reactions_by_id.insert(id.to_string(), reactions(remark, viewer));
                            if let Some(edit) = text(remark, "lastEditedAt") {
                                edits_by_id.insert(id.to_string(), edit.to_string());
                            }
                        }
                    }
                }
                for event in nodes(&raw["reviewDismissals"]) {
                    if let (Some(id), Some(reason)) = (
                        text(&event["review"], "id"),
                        text(event, "dismissalMessage"),
                    ) {
                        dismissals.insert(id.to_string(), reason.to_string());
                    }
                }
            }
            for thread in nodes(&raw["reviewThreads"]) {
                let (Some(id), Some(path)) = (text(thread, "id"), text(thread, "path")) else {
                    continue;
                };
                let loaded = nodes(&thread["comments"]);
                let visible: Vec<_> = loaded.iter().filter(|c| c["isMinimized"] != true).collect();
                if visible.is_empty() {
                    continue;
                }
                let count = thread["comments"]["totalCount"]
                    .as_u64()
                    .unwrap_or(loaded.len() as u64)
                    .saturating_sub((loaded.len() - visible.len()) as u64);
                let tail = next_cursor(&thread["comments"]);
                let cs: Vec<_> = visible
                    .into_iter()
                    .map(|c| thread_comment(c, &input.repo.host, viewer))
                    .collect();
                for c in &cs {
                    comments.push(json!({"id":c["id"],"kind":"review-comment","author":c["author"],"body":c["body"],"createdAt":c["createdAt"],"editedAt":c["editedAt"],"url":c["url"],"path":path,"reviewState":null,"reactions":c["reactions"]}));
                }
                comment_count += count as usize;
                if tail.is_some() {
                    truncated = true;
                }
                threads.push(json!({"id":id,"path":path,"line":thread["line"].as_u64().filter(|n|*n>0),"side":if thread["diffSide"]=="LEFT"{"left"}else{"right"},"isResolved":thread["isResolved"]==true,"isOutdated":thread["isOutdated"]==true,"comments":cs,"commentCount":count,"nextCommentsCursor":tail}));
            }
            after = next_cursor(&raw["reviewThreads"]);
            let Some(cursor) = &after else {
                break;
            };
            if !seen.insert(cursor.clone()) {
                threads_truncated = true;
                break;
            }
        }
        let mut seen_dismissals = HashSet::new();
        for _ in 0..10 {
            let Some(cursor) = dismissal_after
                .take()
                .filter(|c| seen_dismissals.insert(c.clone()))
            else {
                break;
            };
            let result = self
                .query(&input, "ReviewDismissals", json!({"after":cursor}))
                .await;
            let value = match result {
                Ok(value) => value,
                Err(_) => {
                    review_failed = true;
                    break;
                }
            };
            let raw = match pr(&value) {
                Ok(raw) => raw,
                Err(_) => {
                    review_failed = true;
                    break;
                }
            };
            let connection = &raw["timelineItems"];
            for event in nodes(connection) {
                if let (Some(id), Some(reason)) = (
                    text(&event["review"], "id"),
                    text(event, "dismissalMessage"),
                ) {
                    dismissals.insert(id.to_string(), reason.to_string());
                }
            }
            dismissal_after = next_cursor(connection);
        }
        if review_failed {
            comments.truncate(base_comment_count);
            comment_count = base_comment_count;
            threads.clear();
            reviewers.clear();
            reaction_groups = json!([]);
            reactions_by_id.clear();
            edits_by_id.clear();
            dismissals.clear();
            actor_metadata.clear();
            commits = base_commits;
            threads_truncated = true;
            truncated = true;
        }
        threads_truncated |= after.is_some();
        truncated |= threads_truncated;
        for c in &mut comments {
            if let Some(id) = c["id"].as_str().map(str::to_string) {
                if let Some(reactions) = reactions_by_id.get(&id) {
                    c["reactions"] = reactions.clone();
                }
                if let Some(edit) = edits_by_id.get(&id) {
                    c["editedAt"] = json!(edit);
                }
                if c["kind"] == "review"
                    && c["reviewState"] == "DISMISSED"
                    && c["body"].as_str().is_none_or(renders_empty)
                    && let Some(reason) = dismissals.get(&id)
                {
                    c["body"] = json!(reason);
                }
            }
        }
        enrich_actor_metadata(&mut author, &actor_metadata, &input.repo.host);
        for row in &mut reviewers {
            enrich_actor_metadata(row, &actor_metadata, &input.repo.host);
        }
        for row in &mut comments {
            enrich_actor_metadata(row, &actor_metadata, &input.repo.host);
            if row.get("reactions").is_none() {
                row["reactions"] = json!([]);
            }
        }
        for row in &mut threads {
            enrich_actor_metadata(row, &actor_metadata, &input.repo.host);
        }
        for row in &mut commits {
            enrich_actor_metadata(row, &actor_metadata, &input.repo.host);
        }
        comments.sort_by(|a, b| a["createdAt"].as_str().cmp(&b["createdAt"].as_str()));
        parse(
            json!({"author":author,"reviewers":reviewers,"comments":comments,"commentCount":comment_count,"commentsTruncated":truncated,"reviewThreadsTruncated":threads_truncated,"reviewThreads":threads,"commits":commits,"reactions":reaction_groups}),
            "getChangeRequestActivity",
        )
    }
}

impl GitHub {
    async fn template(&self, input: TemplateInput) -> Result<Option<String>> {
        let mut input = input;
        input.treeish = self
            .git_driver
            .at(&input.cwd)
            .with_environment(self.git_environment.clone())
            .resolve_commit(&input.treeish)
            .await
            .map_err(|_| {
                error(
                    "template",
                    FailureReason::Failed,
                    "Could not resolve the template revision.",
                )
            })?;
        let paths = [
            ".github/pull_request_template.md",
            ".github/PULL_REQUEST_TEMPLATE.md",
            ".github/pull_request_template.txt",
            ".github/PULL_REQUEST_TEMPLATE.txt",
            "pull_request_template.md",
            "PULL_REQUEST_TEMPLATE.md",
            "pull_request_template.txt",
            "PULL_REQUEST_TEMPLATE.txt",
            "docs/pull_request_template.md",
            "docs/PULL_REQUEST_TEMPLATE.md",
            "docs/pull_request_template.txt",
            "docs/PULL_REQUEST_TEMPLATE.txt",
        ];
        let directories = [
            ".github/PULL_REQUEST_TEMPLATE",
            ".github/pull_request_template",
            "PULL_REQUEST_TEMPLATE",
            "pull_request_template",
            "docs/PULL_REQUEST_TEMPLATE",
            "docs/pull_request_template",
        ];
        let git = self
            .git_driver
            .at(&input.cwd)
            .with_environment(self.git_environment.clone());
        let mut grep = vec![
            "grep".to_string(),
            "-G".into(),
            "--no-color".into(),
            "-l".into(),
            "-z".into(),
            "--full-name".into(),
            "--no-recurse-submodules".into(),
            "-e".into(),
            "^".into(),
            input.treeish.clone(),
            "--".into(),
        ];
        grep.extend(paths.iter().map(|p| format!(":(top,icase,literal){p}")));
        let found = git
            .execute(
                &grep,
                t3_git::ExecuteOptions {
                    max_output_bytes: 100_000,
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await
            .ok()
            .filter(|r| r.code == Some(0) && !r.stdout_truncated)
            .map(|r| r.stdout_text())
            .unwrap_or_default();
        let mut args = vec![
            "ls-tree".to_string(),
            "-r".into(),
            "-z".into(),
            "--full-tree".into(),
            input.treeish.clone(),
            "--".into(),
        ];
        args.extend(paths.map(str::to_string));
        args.extend(
            found
                .split('\0')
                .filter_map(|p| p.strip_prefix(&format!("{}:", input.treeish)))
                .map(str::to_string),
        );
        args.extend(directories.map(str::to_string));
        let result = git
            .execute(
                &args,
                t3_git::ExecuteOptions {
                    max_output_bytes: 100_000,
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| {
                error(
                    "template",
                    FailureReason::Failed,
                    "Could not read pull request templates.",
                )
            })?;
        if result.stdout_truncated {
            return Ok(None);
        }
        let out = result.stdout_text();
        let entries: Vec<_> = out
            .split('\0')
            .filter_map(|row| {
                let (meta, path) = row.split_once('\t')?;
                let parts: Vec<_> = meta.split(' ').collect();
                if parts.len() != 3
                    || !matches!(parts[0], "100644" | "100755")
                    || parts[1] != "blob"
                    || !(40..=64).contains(&parts[2].len())
                    || !parts[2].bytes().all(|c| c.is_ascii_hexdigit())
                {
                    return None;
                }
                Some((parts[2], path))
            })
            .collect();
        let mut candidates = Vec::new();
        for location in [".github/", "", "docs/"] {
            let preferred: Vec<_> = paths
                .iter()
                .filter(|p| {
                    p.rsplit_once('/')
                        .map_or("", |(parent, _)| &p[..parent.len() + 1])
                        == location
                })
                .collect();
            for path in &preferred {
                candidates.extend(entries.iter().filter(|(_, p)| p == *path).copied());
            }
            candidates.extend(
                entries
                    .iter()
                    .filter(|(_, p)| {
                        !preferred.iter().any(|candidate| **candidate == *p)
                            && preferred.iter().any(|x| x.eq_ignore_ascii_case(p))
                    })
                    .copied(),
            );
        }
        for (id, _) in candidates {
            if let Some(template) = self.template_blob(&git, id).await? {
                return Ok(Some(template));
            }
        }
        for dir in directories {
            let prefix = format!("{dir}/");
            let mut templates = Vec::new();
            for (id, path) in &entries {
                if let Some(name) = path.strip_prefix(&prefix).filter(|name| {
                    !name.contains('/')
                        && (name.to_lowercase().ends_with(".md")
                            || name.to_lowercase().ends_with(".txt"))
                }) {
                    let _ = name;
                    if let Some(template) = self.template_blob(&git, id).await? {
                        templates.push(template);
                        if templates.len() > 1 {
                            return Ok(None);
                        }
                    }
                }
            }
            if let Some(template) = templates.pop() {
                return Ok(Some(template));
            }
        }
        Ok(None)
    }
    async fn template_blob(&self, git: &t3_git::Git, id: &str) -> Result<Option<String>> {
        let result = git
            .execute(
                &["cat-file", "blob", id],
                t3_git::ExecuteOptions {
                    max_output_bytes: 8000,
                    append_truncation_marker: true,
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| {
                error(
                    "template",
                    FailureReason::Failed,
                    "Could not read pull request template.",
                )
            })?;
        let text = result.stdout_text();
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            if result.stdout_truncated && !text.ends_with("[truncated]") {
                format!("{text}\n\n[truncated]")
            } else {
                text.into()
            },
        ))
    }
}

fn matches_listing(item: &ProviderChangeRequest, raw: &Value, input: &ListInput) -> bool {
    if input.state != PullRequestListState::All
        && serde_json::to_value(&input.state).ok() != serde_json::to_value(&item.state).ok()
    {
        return false;
    }
    let author = item.author.as_ref().map(|a| a.login.0.to_lowercase());
    let viewer = input.viewer.to_lowercase();
    match input.involvement {
        PullRequestInvolvement::Authored if author.as_deref() != Some(&viewer) => return false,
        PullRequestInvolvement::Reviewing
            if !item
                .review_request_logins
                .iter()
                .any(|l| l.eq_ignore_ascii_case(&viewer))
                && !nodes(&raw["reviewRequests"])
                    .iter()
                    .any(|r| text(&r["requestedReviewer"], "slug").is_some()) =>
        {
            return false;
        }
        _ => {}
    }
    let Some(filters) = &input.filters else {
        return true;
    };
    let Ok(f) = serde_json::to_value(filters) else {
        return false;
    };
    if let Some(draft) = text(&f, "draft")
        && item.is_draft != (draft == "only")
    {
        return false;
    }
    if let Some(review) = text(&f, "review") {
        let expected = if review == "none" {
            Value::Null
        } else {
            json!(review)
        };
        if serde_json::to_value(&item.review_decision).ok() != Some(expected) {
            return false;
        }
    }
    if let Some(checks) = text(&f, "checks")
        && serde_json::to_value(&item.checks_state).ok() != Some(json!(checks))
    {
        return false;
    }
    let labels: HashSet<_> = item
        .labels
        .iter()
        .map(|l| l.name.0.trim().to_lowercase())
        .collect();
    if f["labels"].as_array().is_some_and(|groups| {
        groups.iter().any(|g| {
            !g.as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|l| labels.contains(&l.trim().to_lowercase()))
        })
    }) {
        return false;
    }
    if f["excludedLabels"].as_array().is_some_and(|ls| {
        ls.iter()
            .filter_map(Value::as_str)
            .any(|l| labels.contains(&l.trim().to_lowercase()))
    }) {
        return false;
    }
    if let Some(a) = text(&f, "author")
        && author.as_deref() != Some(&if a == "@me" { viewer } else { a.to_lowercase() })
    {
        return false;
    }
    true
}

impl GitHub {
    async fn read_identity(
        &self,
        host: &str,
        cwd: &std::path::Path,
        fingerprint: String,
    ) -> Result<RoutingIdentity> {
        let key = format!("{}\0{}", host.trim().to_lowercase(), fingerprint);
        let gate = {
            let mut gates = self.routing_identity_gates.lock().await;
            gates.retain(|_, weak| weak.strong_count() > 0);
            if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(key.clone(), std::sync::Arc::downgrade(&gate));
                gate
            }
        };
        let _gate = gate.lock().await;
        let identities = self.routing_identities.lock().await;
        if let Some((_, identity)) = identities
            .get(&key)
            .filter(|(at, _)| super::transport::now().saturating_sub(*at) < 600_000)
        {
            return Ok(identity.clone());
        }
        drop(identities);
        let request = super::transport::with_interactive(self.rest_json(
            host,
            &cwd.to_string_lossy(),
            "getRoutingIdentity",
            Method::GET,
            "user",
            None,
        ))
        .await;
        let value = match request {
            Ok(value) => value,
            Err(error) if error.reason() == Some(FailureReason::Forbidden) => {
                let value = super::transport::with_interactive(self.http.graphql(
                    host,
                    &cwd.to_string_lossy(),
                    "RoutingIdentityViewer",
                    json!({}),
                ))
                .await?;
                json!({"id":value["viewer"]["databaseId"],"login":value["viewer"]["login"]})
            }
            Err(error) => return Err(error),
        };
        let id = value["id"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| json_error("getRoutingIdentity"))?;
        let viewer = text(&value, "login").ok_or_else(|| json_error("getRoutingIdentity"))?;
        let identity = RoutingIdentity {
            account_id: id.to_string(),
            viewer: viewer.into(),
            credential_fingerprint: fingerprint,
        };
        let mut identities = self.routing_identities.lock().await;
        if identities.len() >= 128
            && let Some(oldest) = identities
                .iter()
                .min_by_key(|(key, (at, _))| (*at, (*key).clone()))
                .map(|(key, _)| key.clone())
        {
            identities.remove(&oldest);
        }
        identities.insert(key, (super::transport::now(), identity.clone()));
        Ok(identity)
    }
}

fn renders_empty(body: &str) -> bool {
    static COMMENTS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?s)<!--[\s\S]*?-->").expect("HTML comment regex")
    });
    COMMENTS.replace_all(body, "").trim().is_empty()
}
fn collect_actor_metadata(value: &Value, actors: &mut BTreeMap<String, (Option<String>, bool)>) {
    if let Some(object) = value.as_object() {
        if let Some(login) = text(value, "login") {
            let avatar = text(value, "avatarUrl").map(str::to_string);
            let bot = value["__typename"] == "Bot";
            let entry = actors.entry(login.to_string()).or_default();
            if avatar.is_some() {
                entry.0 = avatar;
            }
            entry.1 |= bot;
        }
        for child in object.values() {
            collect_actor_metadata(child, actors);
        }
    } else if let Some(array) = value.as_array() {
        for child in array {
            collect_actor_metadata(child, actors);
        }
    }
}
fn enrich_actor_metadata(
    value: &mut Value,
    actors: &BTreeMap<String, (Option<String>, bool)>,
    host: &str,
) {
    if let Some(login) = value["login"].as_str().map(str::to_string)
        && let Some((avatar, bot)) = actors.get(&login)
    {
        if (value["avatarUrl"].is_null()
            || value["avatarUrl"] == format!("https://{host}/{login}.png?size=80"))
            && let Some(avatar) = avatar
        {
            value["avatarUrl"] = json!(avatar);
        }
        if *bot {
            value["isBot"] = json!(true);
        }
    }
    if let Some(object) = value.as_object_mut() {
        for child in object.values_mut() {
            enrich_actor_metadata(child, actors, host);
        }
    } else if let Some(array) = value.as_array_mut() {
        for child in array {
            enrich_actor_metadata(child, actors, host);
        }
    }
}
