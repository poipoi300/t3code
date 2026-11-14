use super::{decode as d, *};
use async_trait::async_trait;
fn number(reference: &str) -> Result<u64> {
    reference
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .or_else(|| {
            crate::remote::parse_pull_request_url(reference)
                .filter(|l| l.provider == "azure-devops")
                .map(|l| l.number)
        })
        .ok_or_else(|| error("reference", "Invalid Azure pull request reference."))
}
fn repo_args(repository: &str) -> Result<Vec<String>> {
    let parts: Vec<_> = repository
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let name = parts.last().copied().unwrap_or("");
    if name.is_empty() {
        return Err(error("repository", "Missing repository name."));
    }
    let mut args = vec![format!("--repository={}", value(name)?)];
    if parts.len() > 1 {
        args.push(format!("--project={}", value(parts[parts.len() - 2])?));
    }
    Ok(args)
}
fn urls(raw: &Value) -> Result<SourceControlRepositoryCloneUrls> {
    let name = text(raw, "name").ok_or_else(|| error("repository", "Missing repository name."))?;
    let full = text(&raw["project"], "name")
        .map(|project| format!("{project}/{name}"))
        .unwrap_or(name.into());
    parse(
        json!({"nameWithOwner":full,"url":raw["remoteUrl"],"sshUrl":raw["sshUrl"]}),
        "cloneUrls",
    )
}
#[async_trait]
impl Forge for AzureDevOps {
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("azure-devops".into())
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
    async fn refine_repository_identity(
        &self,
        mut identity: RepositoryIdentity,
    ) -> Result<RepositoryIdentity> {
        if identity
            .provider
            .as_ref()
            .is_none_or(|p| p.0 != "azure-devops")
        {
            return Ok(identity);
        }
        if let Some(canonical) =
            crate::remote::azure_canonical_repository_key(&identity.canonical_key.0)
        {
            identity.canonical_key = t3_contracts::base_schemas::TrimmedNonEmptyString(canonical);
        }
        if let Some(remote) = crate::remote::parse_remote(&identity.locator.remote_url.0)
            && let Some(repository) = remote.repository
        {
            let key = format!("{}/{repository}", remote.host);
            if let Some(web) = crate::remote::azure_repository_key_from_remote(&key) {
                identity.web_url = Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                    format!("https://{web}"),
                ));
            }
        }
        Ok(identity)
    }
    async fn shutdown(&self) {
        self.stop.cancel();
    }
    fn unavailable_requirement(&self) -> Option<UnavailableRequirement> {
        Some(UnavailableRequirement {
            missing: "Install Azure CLI (`az`) and run `az extension add --name azure-devops`."
                .into(),
            unauthenticated: "Azure CLI is not signed in. Run `az login` and retry.".into(),
        })
    }
    fn head_branch_probe(&self, selectors: &[String], _default: Option<&str>) -> BranchProbe {
        BranchProbe {
            head_selectors: selectors
                .iter()
                .filter(|s| !s.contains(':'))
                .cloned()
                .collect(),
            limit: 100,
        }
    }
    async fn discover(
        &self,
        cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        let probe = |args: Vec<String>| {
            self.cli.run(CliRequest {
                program: "az",
                provider: "azure-devops",
                operation: "discover".into(),
                cwd: cwd.clone(),
                args,
                stdin: None,
                environment: self.environment.clone(),
                max_bytes: 16_000,
                timeout: Duration::from_secs(20),
                cancellation: self.stop.clone(),
            })
        };
        let version = probe(vec!["--version".into()]).await;
        let available = version.as_ref().is_ok_and(|v| v.code == 0);
        let mut v = json!({"kind":"azure-devops","label":"Azure DevOps","executable":"az","status":if available{"available"}else{"missing"},"version":{"_tag":"None"},"installHint":"Install Azure CLI and the azure-devops extension.","detail":{"_tag":"None"},"auth":{"status":"unknown","account":{"_tag":"None"},"host":{"_tag":"None"},"detail":{"_tag":"None"}}});
        if available {
            if let Ok(version) = version
                && let Some(line) = String::from_utf8_lossy(&version.stdout)
                    .lines()
                    .find(|s| !s.trim().is_empty())
            {
                v["version"] =
                    json!({"_tag":"Some","value":crate::remote::transport_safe_error_value(line)});
            }
            if let Ok(auth) = probe(vec![
                "account".into(),
                "show".into(),
                "--query=user.name".into(),
                "--output=tsv".into(),
            ])
            .await
            {
                let stdout = String::from_utf8_lossy(&auth.stdout);
                let account = stdout
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                v["auth"]["status"] = json!(if auth.code != 0 {
                    "unauthenticated"
                } else if account.is_some() {
                    "authenticated"
                } else {
                    "unknown"
                });
                v["auth"]["host"] = json!({"_tag":"Some","value":"dev.azure.com"});
                if auth.code == 0
                    && let Some(account) = account
                {
                    v["auth"]["account"] = json!({"_tag":"Some","value":crate::remote::transport_safe_error_value(account)});
                }
            }
        }
        parse(v, "discover")
    }
    async fn get_viewer(&self, input: ViewerInput) -> Result<String> {
        let account = self
            .json(
                &input.cwd,
                "getViewer",
                vec!["account".into(), "show".into(), "--query=user".into()],
                8 * 1024 * 1024,
            )
            .await;
        match account {
            Ok(raw) if text(&raw, "name").is_some() => {
                return Ok(text(&raw, "name").unwrap_or("").into());
            }
            Err(e) if e.reason() == Some(FailureReason::MissingTool) => return Err(e),
            _ => {}
        }
        let raw = self
            .json(
                &input.cwd,
                "getViewerFallback",
                vec![
                    "repos".into(),
                    "pr".into(),
                    "list".into(),
                    "--detect=true".into(),
                    "--creator=me".into(),
                    "--status=all".into(),
                    "--top=1".into(),
                ],
                8 * 1024 * 1024,
            )
            .await?;
        rows(&raw)
            .filter_map(|r| {
                text(&r["createdBy"], "uniqueName").or_else(|| text(&r["createdBy"], "displayName"))
            })
            .next()
            .map(str::to_string)
            .ok_or_else(|| error("getViewer", "Azure CLI returned no account name."))
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
        parse(
            d::listing(&self.raw(&input).await?)?,
            "getChangeRequestPreview",
        )
    }
    async fn get_change_request_summary(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        parse(
            d::summary(&self.raw(&input).await?)?,
            "getChangeRequestSummary",
        )
    }
    async fn get_change_request_activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        self.activity(input).await
    }
    async fn get_viewer_permissions(
        &self,
        _input: PermissionsInput,
    ) -> Result<PullRequestViewerPermissions> {
        parse(permissions(), "getViewerPermissions")
    }
    async fn get_diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        self.diff(input).await
    }
    async fn get_diff_file_contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        self.contents(input).await
    }
    async fn get_file_revisions(&self, input: FileRevisionsInput) -> Result<ProviderFileRevisions> {
        self.revisions(input).await
    }
    async fn set_reviewer_request(&self, input: ReviewerRequestInput) -> Result<()> {
        let mut ids = vec![];
        for reviewer in input.reviewers {
            let raw = serde_json::to_value(reviewer)
                .map_err(|_| error("reviewers", "Invalid reviewer."))?;
            let id = text(&raw, "id")
                .filter(|s| !s.starts_with('-'))
                .ok_or_else(|| error("reviewers", "Invalid reviewer name."))?;
            ids.push(value(id)?.to_string());
        }
        if ids.is_empty() {
            return Ok(());
        }
        let mut args = vec![
            "repos".into(),
            "pr".into(),
            "reviewer".into(),
            if input.requested { "add" } else { "remove" }.into(),
            "--detect=true".into(),
            format!("--id={}", input.pr.number),
            "--reviewers".into(),
        ];
        args.extend(ids);
        self.json(
            Path::new(&input.pr.repo.cwd),
            "setReviewerRequest",
            args,
            8 * 1024 * 1024,
        )
        .await?;
        Ok(())
    }
    async fn update_change_request(&self, input: UpdateInput) -> Result<()> {
        let mut args = vec![
            "repos".into(),
            "pr".into(),
            "update".into(),
            "--detect=true".into(),
            format!("--id={}", input.pr.number),
        ];
        if let Some(title) = input.title {
            args.push(format!("--title={}", value(&title)?));
        }
        if let Some(body) = input.body {
            args.push(format!("--description={}", value(&body)?));
        }
        self.json(
            Path::new(&input.pr.repo.cwd),
            "updatePullRequest",
            args,
            8 * 1024 * 1024,
        )
        .await?;
        Ok(())
    }
    async fn run_action(&self, input: ActionInput) -> Result<Option<ProviderChangeRequestSummary>> {
        let mut args = vec![
            "repos".into(),
            "pr".into(),
            "update".into(),
            "--detect=true".into(),
            format!("--id={}", input.pr.number),
        ];
        match input.action {
            PullRequestAction::Merge => args.extend([
                "--status=completed".into(),
                format!(
                    "--squash={}",
                    input.merge_method == Some(PullRequestMergeMethod::Squash)
                ),
            ]),
            PullRequestAction::EnableAutoMerge => {
                args.push("--auto-complete=true".into());
                if let Some(method) = input.merge_method {
                    args.push(format!(
                        "--squash={}",
                        method == PullRequestMergeMethod::Squash
                    ));
                }
            }
            PullRequestAction::DisableAutoMerge => args.push("--auto-complete=false".into()),
            PullRequestAction::Ready => args.push("--draft=false".into()),
            PullRequestAction::Draft => args.push("--draft=true".into()),
            PullRequestAction::Close => args.push("--status=abandoned".into()),
            PullRequestAction::Reopen => args.push("--status=active".into()),
            _ => return Err(ForgeError::unsupported(&self.kind(), "run_action")),
        }
        let raw = self
            .json(
                Path::new(&input.pr.repo.cwd),
                "runPullRequestAction",
                args,
                8 * 1024 * 1024,
            )
            .await?;
        let Ok(mut summary) = d::summary(&raw) else {
            return Ok(None);
        };
        d::auto_merge(&raw, &mut summary);
        parse(summary, "runAction").map(Some)
    }
    async fn checkout_get_change_request(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        let raw = self
            .json(
                &input.checkout.cwd,
                "getPullRequest",
                vec![
                    "repos".into(),
                    "pr".into(),
                    "show".into(),
                    "--detect=true".into(),
                    format!("--id={}", number(&input.reference)?),
                ],
                8 * 1024 * 1024,
            )
            .await?;
        d::checkout(&raw)
    }
    async fn checkout_list_change_requests(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let branch = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(&input.head_selector);
        let prefix = ["refs/heads/", "refs/pull/", "refs/tags/"]
            .into_iter()
            .find(|p| branch.starts_with(p))
            .unwrap_or("");
        let encoded = format!(
            "{prefix}{}",
            encode(branch.strip_prefix(prefix).unwrap_or(branch))
        );
        let args = vec![
            "repos".into(),
            "pr".into(),
            "list".into(),
            "--detect=true".into(),
            format!("--source-branch={encoded}"),
            format!(
                "--status={}",
                match input.state {
                    Some(ChangeRequestState::Closed) => "abandoned",
                    Some(ChangeRequestState::Merged) => "completed",
                    None => "all",
                    _ => "active",
                }
            ),
            format!("--top={}", input.limit.unwrap_or(20)),
        ];
        let raw = self
            .json(&input.checkout.cwd, "checkoutList", args, 16 * 1024 * 1024)
            .await?;
        if raw.is_null() {
            return Ok(vec![]);
        }
        let raw = raw
            .as_array()
            .ok_or_else(|| error("checkoutList", "Invalid pull request list."))?;
        Ok(raw.iter().filter_map(|r| d::checkout(r).ok()).collect())
    }
    async fn checkout_change_request(&self, input: CheckoutGetInput) -> Result<()> {
        let remote = input
            .checkout
            .context
            .as_ref()
            .map(|c| c.remote_name.as_str())
            .unwrap_or("origin");
        self.json(
            &input.checkout.cwd,
            "checkoutPullRequest",
            vec![
                "repos".into(),
                "pr".into(),
                "checkout".into(),
                "--detect=true".into(),
                format!("--id={}", number(&input.reference)?),
                format!("--remote-name={}", value(remote)?),
            ],
            8 * 1024 * 1024,
        )
        .await?;
        Ok(())
    }
    async fn get_repository_clone_urls(
        &self,
        input: CloneUrlsInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let mut args = vec!["repos".into(), "show".into(), "--detect=true".into()];
        args.extend(repo_args(&input.repository)?);
        urls(
            &self
                .json(
                    &input.checkout.cwd,
                    "getRepositoryCloneUrls",
                    args,
                    8 * 1024 * 1024,
                )
                .await?,
        )
    }
    async fn create_repository(
        &self,
        input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let mut spec = repo_args(&input.repository)?;
        spec[0] = spec[0].replacen("--repository=", "--name=", 1);
        let mut args = vec!["repos".into(), "create".into(), "--detect=true".into()];
        args.extend(spec);
        urls(
            &self
                .json(&input.cwd, "createRepository", args, 8 * 1024 * 1024)
                .await?,
        )
    }
    async fn create_change_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let source = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(&input.head_selector);
        self.json(
            &input.checkout.cwd,
            "createPullRequest",
            vec![
                "repos".into(),
                "pr".into(),
                "create".into(),
                "--detect=true".into(),
                format!("--source-branch={}", value(source)?),
                format!(
                    "--target-branch={}",
                    value(
                        input
                            .target
                            .as_ref()
                            .map(|t| t.ref_name.as_str())
                            .unwrap_or(&input.base_ref_name)
                    )?
                ),
                format!("--title={}", value(&input.title)?),
                format!("--description=@{}", input.body_file.to_string_lossy()),
            ],
            8 * 1024 * 1024,
        )
        .await?;
        Ok(())
    }
    async fn get_default_branch(&self, input: CheckoutInput) -> Result<Option<String>> {
        let Some(context) = input.context else {
            return Ok(None);
        };
        let remote = context.remote_url.trim_end_matches('/');
        let raw_name = url::Url::parse(remote)
            .ok()
            .map(|u| u.path().to_string())
            .unwrap_or(remote.into());
        let Some(name) = raw_name.rsplit(['/', ':']).next().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let name = decode_path(name);
        if name.starts_with('@') {
            return Ok(None);
        }
        let raw = self
            .json(
                &input.cwd,
                "getDefaultBranch",
                vec![
                    "repos".into(),
                    "show".into(),
                    "--detect=true".into(),
                    format!("--repository={}", value(&name)?),
                ],
                8 * 1024 * 1024,
            )
            .await?;
        Ok(text(&raw, "defaultBranch")
            .map(|s| s.strip_prefix("refs/heads/").unwrap_or(s))
            .filter(|s| !s.is_empty())
            .map(str::to_string))
    }
}
fn decode_path(path: &str) -> String {
    let mut out = vec![];
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(a), Some(b)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((a * 16 + b) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or(path.into())
}
const SUPPORTED: &[&str] = &[
    "refine_repository_identity",
    "discover",
    "get_viewer",
    "list_change_requests",
    "get_change_request",
    "get_change_request_preview",
    "get_change_request_summary",
    "get_change_request_activity",
    "get_viewer_permissions",
    "get_diff",
    "get_diff_file_contents",
    "get_file_revisions",
    "set_reviewer_request",
    "update_change_request",
    "run_action",
    "checkout_get_change_request",
    "checkout_list_change_requests",
    "checkout_change_request",
    "get_repository_clone_urls",
    "create_repository",
    "create_change_request",
    "get_default_branch",
];
