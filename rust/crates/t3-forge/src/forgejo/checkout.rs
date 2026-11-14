use super::{
    decode as d,
    transport::{Login, match_login, parse_remote},
    *,
};
impl Forgejo {
    pub(super) async fn checkout_target(
        &self,
        input: &CheckoutInput,
        repository: Option<&str>,
        reference: Option<&str>,
        host: Option<&str>,
    ) -> Result<Target> {
        self.transport
            .resolve(
                ResolveInput {
                    cwd: &input.cwd,
                    context: input.context.as_ref(),
                    repository,
                    reference,
                    host,
                },
                false,
            )
            .await
    }
    pub(super) async fn target_json(
        &self,
        target: &Target,
        cwd: &Path,
        path: &str,
        method: Method,
        body: Option<Value>,
    ) -> Result<Value> {
        let response = self
            .transport
            .request(target, cwd, path, method, body)
            .await?;
        if response.truncated {
            return Err(error("api", "The response exceeded the byte limit."));
        }
        if response.stdout.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.stdout)
            .map_err(|_| error("api", "Invalid Forgejo API response."))
    }
    pub(super) async fn checkout_get_raw(
        &self,
        input: &CheckoutGetInput,
    ) -> Result<(Target, Value)> {
        let target = self
            .checkout_target(&input.checkout, None, Some(&input.reference), None)
            .await?;
        let number = input
            .reference
            .trim()
            .trim_start_matches('#')
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .or_else(|| {
                let url = url::Url::parse(&input.reference).ok()?;
                let segments: Vec<_> = url.path_segments()?.collect();
                let at = segments.iter().position(|s| *s == "pulls")?;
                segments.get(at + 1)?.parse::<u64>().ok().filter(|n| *n > 0)
            })
            .ok_or_else(|| error("checkout", "Specify a pull request number or Forgejo URL."))?;
        let raw = self
            .target_json(
                &target,
                &input.checkout.cwd,
                &format!("{}/pulls/{number}", repo_path(&target.repository)),
                Method::GET,
                None,
            )
            .await?;
        d::checkout(&raw)?;
        Ok((target, raw))
    }
    pub(super) async fn create_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        let target = self
            .checkout_target(&input.checkout, None, None, None)
            .await?;
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let owner = source.as_ref().and_then(|s| {
            s.owner
                .as_deref()
                .or_else(|| s.repository.as_deref().and_then(|r| r.split('/').next()))
        });
        let head = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(&input.head_selector);
        let body = tokio::fs::read_to_string(input.body_file)
            .await
            .map_err(|_| {
                error(
                    "createChangeRequest",
                    "Could not read the pull request description.",
                )
            })?;
        self.transport.request(&target,&input.checkout.cwd,&format!("{}/pulls",repo_path(input.target.as_ref().and_then(|t|t.repository.as_deref()).unwrap_or(&target.repository))),Method::POST,Some(json!({"base":input.target.map(|t|t.ref_name).unwrap_or(input.base_ref_name),"head":owner.map(|owner|format!("{owner}:{head}")).unwrap_or(head.into()),"title":input.title,"body":body}))).await?;
        Ok(())
    }
    pub(super) async fn clone_urls(
        &self,
        input: CloneUrlsInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let target = self
            .checkout_target(
                &input.checkout,
                Some(&input.repository),
                None,
                input.host.as_deref(),
            )
            .await?;
        urls(
            &self
                .target_json(
                    &target,
                    &input.checkout.cwd,
                    &repo_path(&target.repository),
                    Method::GET,
                    None,
                )
                .await?,
        )
    }
    pub(super) async fn create_repo(
        &self,
        input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let checkout = CheckoutInput {
            cwd: input.cwd,
            context: None,
        };
        let target = self
            .checkout_target(&checkout, Some(&input.repository), None, None)
            .await?;
        let user = self
            .target_json(&target, &checkout.cwd, "user", Method::GET, None)
            .await?;
        let (owner, name) = target
            .repository
            .split_once('/')
            .ok_or_else(|| error("createRepository", "Invalid repository name."))?;
        let path = if user["login"] == owner {
            "user/repos".into()
        } else {
            format!("orgs/{}/repos", encode(owner))
        };
        urls(&self.target_json(&target,&checkout.cwd,&path,Method::POST,Some(json!({"name":name,"private":input.visibility==SourceControlRepositoryVisibility::Private,"auto_init":false}))).await?)
    }
    pub(super) async fn default_branch(&self, input: CheckoutInput) -> Result<Option<String>> {
        let target = self.checkout_target(&input, None, None, None).await?;
        let raw = self
            .target_json(
                &target,
                &input.cwd,
                &repo_path(&target.repository),
                Method::GET,
                None,
            )
            .await?;
        Ok(text(&raw, "default_branch").map(str::to_string))
    }
    pub(super) async fn checkout_request(&self, input: CheckoutGetInput) -> Result<()> {
        let (target, raw) = self.checkout_get_raw(&input).await?;
        let number = raw["number"]
            .as_u64()
            .ok_or_else(|| error("checkout", "Invalid pull request number."))?;
        let git = self.transport.git.at(&input.checkout.cwd);
        if target.command == "fj" {
            let repo = self
                .target_json(
                    &target,
                    &input.checkout.cwd,
                    &repo_path(&target.repository),
                    Method::GET,
                    None,
                )
                .await?;
            let ssh = input
                .checkout
                .context
                .as_ref()
                .and_then(|c| parse_remote(&c.remote_url))
                .is_some_and(|r| r.ssh);
            let url = text(&repo, if ssh { "ssh_url" } else { "clone_url" })
                .ok_or_else(|| error("checkout", "Missing clone URL."))?;
            git.execute(
                &["fetch", "--", url, &format!("refs/pull/{number}/head")],
                Default::default(),
            )
            .await
            .map_err(|_| error("checkout", "Could not fetch the pull request."))?;
            let branch = format!("pulls/{number}");
            if git
                .list_local_branch_names()
                .await
                .map_err(|_| error("checkout", "Could not inspect local branches."))?
                .contains(&branch)
            {
                git.switch_ref(&branch)
                    .await
                    .map_err(|_| error("checkout", "Could not switch to the pull request."))?;
            } else {
                git.execute(
                    &["checkout", "-b", &branch, "FETCH_HEAD", "--"],
                    Default::default(),
                )
                .await
                .map_err(|_| error("checkout", "Could not create the pull request branch."))?;
            }
        } else {
            self.transport
                .execute(
                    "tea",
                    &input.checkout.cwd,
                    "checkoutPullRequest",
                    vec![
                        "pulls".into(),
                        "checkout".into(),
                        format!("--login={}", target.login.name),
                        format!("--repo={}", target.repository),
                        "--branch".into(),
                        number.to_string(),
                    ],
                    None,
                )
                .await?;
        }
        if input.force {
            let sha = text(&raw["head"], "sha")
                .filter(|s| crate::support::sha(s))
                .ok_or_else(|| error("checkout", "Forgejo reported an invalid commit SHA."))?;
            git.execute(&["reset", "--keep", sha], Default::default())
                .await
                .map_err(|_| {
                    error(
                        "checkout",
                        "Could not update the pull request branch while preserving local changes.",
                    )
                })?;
        }
        Ok(())
    }
    pub(super) async fn refine(&self, input: &CheckoutInput) -> Result<Option<Login>> {
        let Some(context) = &input.context else {
            return Ok(None);
        };
        let Some(remote) = parse_remote(&context.remote_url) else {
            return Ok(None);
        };
        let mut failure = None;
        for command in ["fj", "tea"] {
            let logins = match self
                .transport
                .logins(&input.cwd, command, Some(&context.remote_url))
                .await
            {
                Ok(logins) => logins,
                Err(e) if e.reason() == Some(FailureReason::MissingTool) => continue,
                Err(e) => {
                    failure.get_or_insert(e);
                    continue;
                }
            };
            if let Some(login) =
                match_login(&logins, &remote, context.requested_host.as_deref(), false)
            {
                return Ok(Some(login));
            }
        }
        if let Some(failure) = failure {
            return Err(failure);
        }
        Ok(None)
    }
    pub(super) async fn discover_cli(
        &self,
        cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        let remote = self
            .transport
            .git
            .at(&cwd)
            .execute(
                &["remote", "get-url", "origin"],
                t3_git::ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await
            .ok()
            .map(|r| r.stdout_text().trim().to_string())
            .unwrap_or_default();
        let fj = self.transport.logins(&cwd, "fj", Some(&remote)).await;
        let logins = fj.as_ref().cloned().unwrap_or_default();
        let selected = parse_remote(&remote)
            .and_then(|r| match_login(&logins, &r, None, false))
            .or_else(|| logins.iter().find(|l| l.default == "true").cloned())
            .or_else(|| logins.first().cloned());
        let version = self
            .transport
            .execute(
                "fj",
                &self.transport.config.authentication_cwd,
                "discover",
                vec!["version".into()],
                None,
            )
            .await;
        let mut command = "fj";
        let mut auth = json!({"status":"unauthenticated","account":{"_tag":"None"},"host":{"_tag":"None"},"detail":{"_tag":"None"}});
        let mut version_line = version.as_ref().ok().map(|r| {
            String::from_utf8_lossy(&r.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_string()
        });
        let mut available = version.is_ok();
        if available && (selected.is_some() || fj.is_err()) {
            if fj.is_err() {
                auth["status"] = json!("unknown");
            } else if let Some(login) = selected {
                auth["host"] = parse_remote(&login.url)
                    .map(|r| json!({"_tag":"Some","value":r.host}))
                    .unwrap_or(json!({"_tag":"None"}));
                match self.transport.account(&cwd, &login).await {
                    Ok(account) => {
                        auth["status"] = json!("authenticated");
                        auth["account"] = json!({"_tag":"Some","value":account});
                    }
                    Err(e) => {
                        auth["status"] =
                            json!(if e.reason() == Some(FailureReason::Unauthenticated) {
                                "unauthenticated"
                            } else {
                                "unknown"
                            })
                    }
                }
            }
        } else {
            let tea = self
                .transport
                .execute("tea", &cwd, "discover", vec!["--version".into()], None)
                .await;
            if tea.is_ok()
                || version
                    .as_ref()
                    .is_err_and(|e| e.reason() == Some(FailureReason::MissingTool))
            {
                command = "tea";
                available = tea.is_ok();
                version_line = tea.as_ref().ok().map(|r| {
                    String::from_utf8_lossy(&r.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string()
                });
                if available
                    && let Ok(output) = self
                        .transport
                        .execute(
                            "tea",
                            &cwd,
                            "discover",
                            vec!["login".into(), "status".into(), "--output=json".into()],
                            None,
                        )
                        .await
                    && let Ok(logins) = serde_json::from_slice::<Vec<Login>>(&output.stdout)
                    && let Some(login) = logins
                        .iter()
                        .find(|l| l.default == "true")
                        .or_else(|| logins.first())
                {
                    auth["status"] = json!(if login.valid.as_deref() == Some("true") {
                        "authenticated"
                    } else {
                        "unauthenticated"
                    });
                    auth["account"] = json!({"_tag":"Some","value":login.user});
                    auth["host"] = parse_remote(&login.url)
                        .map(|r| json!({"_tag":"Some","value":r.host}))
                        .unwrap_or(json!({"_tag":"None"}));
                }
            }
        }
        parse(
            json!({"kind":"forgejo","label":"Forgejo / Gitea","executable":command,"status":if available{"available"}else{"missing"},"version":version_line.filter(|s|!s.trim().is_empty()).map(|s|json!({"_tag":"Some","value":crate::remote::transport_safe_error_value(&s)})).unwrap_or(json!({"_tag":"None"})),"installHint":"Install fj 0.6 or later, or tea 0.16 or later, and authenticate each server.","detail":{"_tag":"None"},"auth":auth}),
            "discover",
        )
    }
}
fn urls(raw: &Value) -> Result<SourceControlRepositoryCloneUrls> {
    parse(
        json!({"nameWithOwner":raw["full_name"],"url":raw["clone_url"],"sshUrl":raw["ssh_url"]}),
        "cloneUrls",
    )
}
