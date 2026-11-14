use super::*;
pub(super) fn reference(value: &str) -> (Option<String>, Option<u64>) {
    let value = value.trim();
    if let Ok(url) = url::Url::parse(value) {
        if !matches!(url.scheme(), "http" | "https")
            || !matches!(url.host_str(), Some("cursor.com" | "www.cursor.com"))
        {
            return (None, None);
        }
        let parts = url
            .path()
            .trim_start_matches('/')
            .split('/')
            .collect::<Vec<_>>();
        if parts.len() >= 5 && parts[0] == "codebase" && parts[3] == "pull" {
            return (
                Some(format!("{}/{}", parts[1], parts[2]).to_ascii_lowercase()),
                parts[4].parse().ok().filter(|n| *n > 0),
            );
        }
        return (None, None);
    }
    (
        None,
        value
            .strip_prefix('#')
            .unwrap_or(value)
            .parse()
            .ok()
            .filter(|n| *n > 0),
    )
}
fn urls(raw: &Value, fallback: &str) -> Result<SourceControlRepositoryCloneUrls> {
    let name = text(raw, "fullName")
        .map(str::to_string)
        .or_else(|| {
            Some(format!(
                "{}/{}",
                text(&raw["owner"], "slug")?,
                text(raw, "name")?
            ))
        })
        .unwrap_or(fallback.into());
    let clone = text(raw, "cloneUrl")
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://origin.cursor.com/{name}.git"));
    parse(
        json!({"nameWithOwner":name,"url":clone,"sshUrl":clone}),
        "getRepositoryCloneUrls",
    )
}
impl CursorOrigin {
    pub(super) async fn checkout_repo(
        &self,
        input: &CheckoutInput,
    ) -> Result<ProviderRepositoryRef> {
        let remote = if let Some(context) = &input.context {
            context.remote_url.clone()
        } else {
            let remote = self.primary_remote(&input.cwd).await?;
            self.git
                .at(&input.cwd)
                .execute(
                    &["config", "--get", &format!("remote.{remote}.url")],
                    Default::default(),
                )
                .await
                .map_err(|_| error("resolveRepository", "Could not read the checkout remote."))?
                .stdout_text()
                .trim()
                .to_string()
        };
        let parsed = crate::remote::parse_remote(&remote)
            .filter(|r| r.hostname == "origin.cursor.com")
            .and_then(|r| r.repository)
            .ok_or_else(|| {
                ForgeError::provider(
                    "cursor-origin",
                    "resolveRepository",
                    FailureReason::NotFound,
                    "This checkout has no Cursor Origin remote.",
                )
            })?;
        Ok(ProviderRepositoryRef {
            cwd: input.cwd.to_string_lossy().into(),
            host: "origin.cursor.com".into(),
            repository: parsed,
        })
    }
    pub(super) async fn primary_remote(&self, cwd: &Path) -> Result<String> {
        self.git
            .at(cwd)
            .primary_remote_name()
            .await
            .map_err(|_| error("resolveRepository", "Could not read repository remotes."))?
            .ok_or_else(|| error("resolveRepository", "This checkout has no remote."))
    }
    pub(super) async fn checkout_get(
        &self,
        input: &CheckoutGetInput,
    ) -> Result<(ProviderRepositoryRef, Value)> {
        let (repository, number) = reference(&input.reference);
        let number = number.ok_or_else(|| {
            ForgeError::provider(
                "cursor-origin",
                "getChangeRequest",
                FailureReason::NotFound,
                "Specify a pull request number or Cursor Origin pull request URL.",
            )
        })?;
        let repo = if let Some(repository) = repository {
            ProviderRepositoryRef {
                cwd: input.checkout.cwd.to_string_lossy().into(),
                host: "origin.cursor.com".into(),
                repository,
            }
        } else {
            self.checkout_repo(&input.checkout).await?
        };
        let raw = self
            .raw(&PullRequestInput {
                repo: repo.clone(),
                number,
            })
            .await?;
        Ok((repo, raw))
    }
    pub(super) async fn checkout_list(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        let repo = self.checkout_repo(&input.checkout).await?;
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let branch = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(input.head_selector.trim());
        let state = match input.state {
            Some(ChangeRequestState::Open) => "open",
            Some(ChangeRequestState::Closed) => "closed",
            Some(ChangeRequestState::Merged) => "merged",
            None => "all",
        };
        let path = format!(
            "{}/pulls?head={}&state={state}&pageSize={}&sortBy=updated",
            repo_path(&repo.repository),
            encode(branch),
            input.limit.unwrap_or(20).clamp(1, 100)
        );
        let page = self
            .api(
                &repo,
                "checkoutListChangeRequests",
                &path,
                Method::GET,
                None,
            )
            .await?;
        if !page["pullRequests"].is_null() && !page["pullRequests"].is_array() {
            return Err(error(
                "checkoutListChangeRequests",
                "Invalid pull request collection.",
            ));
        }
        rows(&page["pullRequests"]).map(decode::checkout).collect()
    }
    pub(super) async fn checkout_request(&self, input: CheckoutGetInput) -> Result<()> {
        let repo = self.checkout_repo(&input.checkout).await?;
        let (linked, number) = reference(&input.reference);
        if linked
            .as_ref()
            .is_some_and(|r| !r.eq_ignore_ascii_case(&repo.repository))
            || (number.is_none()
                && (input.reference.trim().starts_with('-')
                    || url::Url::parse(input.reference.trim()).is_ok()))
            || input.reference.trim().is_empty()
        {
            return Err(error(
                "checkoutChangeRequest",
                "Specify a pull request in this checkout's repository.",
            ));
        }
        let remote = if let Some(context) = input.checkout.context {
            context.remote_name
        } else {
            self.primary_remote(&input.checkout.cwd).await?
        };
        let mut args = vec![
            "pr".into(),
            "checkout".into(),
            format!("--repo={}", repo.repository),
            format!("--remote={remote}"),
        ];
        if input.force {
            args.push("--force".into());
        }
        args.push(
            number
                .map(|n| n.to_string())
                .unwrap_or_else(|| input.reference.trim().into()),
        );
        self.transport
            .execute(&input.checkout.cwd, "checkoutChangeRequest", args)
            .await
            .map(|_| ())
    }
    pub(super) async fn personal_namespace(&self, cwd: &Path) -> Result<String> {
        let repo = ProviderRepositoryRef {
            cwd: cwd.to_string_lossy().into(),
            host: "origin.cursor.com".into(),
            repository: String::new(),
        };
        let value = self
            .api(
                &repo,
                "personalNamespace",
                "v1/origin/namespaces",
                Method::GET,
                None,
            )
            .await?;
        rows(&value["namespaces"])
            .find(|n| n["namespace"]["type"] == "user")
            .and_then(|n| text(&n["namespace"], "slug"))
            .map(str::to_string)
            .ok_or_else(|| {
                ForgeError::provider(
                    "cursor-origin",
                    "personalNamespace",
                    FailureReason::NotFound,
                    "Claim an Origin namespace at cursor.com/codebase.",
                )
            })
    }
    pub(super) async fn clone_urls(
        &self,
        input: CloneUrlsInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let repo = ProviderRepositoryRef {
            cwd: input.checkout.cwd.to_string_lossy().into(),
            host: "origin.cursor.com".into(),
            repository: input.repository,
        };
        let value = self
            .api(
                &repo,
                "getRepositoryCloneUrls",
                &repo_path(&repo.repository),
                Method::GET,
                None,
            )
            .await?;
        urls(&value, &repo.repository)
    }
    pub(super) async fn create_repo(
        &self,
        input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        if input.visibility == SourceControlRepositoryVisibility::Public {
            return Err(error(
                "createRepository",
                "Cursor Origin repositories are private. Choose private visibility.",
            ));
        }
        let parts = input.repository.split('/').collect::<Vec<_>>();
        if parts.len() > 2 || parts.iter().any(|s| s.trim().is_empty()) {
            return Err(error(
                "createRepository",
                "Specify an Origin repository as owner/name or name.",
            ));
        }
        let owner = if parts.len() == 2 {
            parts[0].to_string()
        } else {
            self.personal_namespace(&input.cwd).await?
        };
        let name = *parts
            .last()
            .ok_or_else(|| error("createRepository", "Missing repository name."))?;
        let repo = ProviderRepositoryRef {
            cwd: input.cwd.to_string_lossy().into(),
            host: "origin.cursor.com".into(),
            repository: format!("{owner}/{name}"),
        };
        self.api(
            &repo,
            "createRepository",
            &format!("v1/origin/repos/{}", encode(&owner)),
            Method::POST,
            Some(json!({"name":name})),
        )
        .await?;
        let mut result = Err(error(
            "createRepository",
            "Could not make the created repository private.",
        ));
        for attempt in 0..3 {
            result = self
                .api(
                    &repo,
                    "createRepository",
                    &repo_path(&repo.repository),
                    Method::PATCH,
                    Some(json!({"visibility":"private"})),
                )
                .await;
            if result.is_ok() || result.as_ref().is_err_and(|e| protected(e, false)) {
                break;
            }
            if attempt < 2 {
                tokio::select! {_=self.transport_cancelled()=>return Err(error("createRepository","The operation was cancelled.")),_=tokio::time::sleep(Duration::from_secs(1))=>{}}
            }
        }
        let value=result.map_err(|mut e|{if let ForgeError::Provider{detail,..}=&mut e{*detail=format!("Created {}, but could not make it private, so your Cursor team can still see it. Set it to private on cursor.com.",repo.repository).into_boxed_str();}e})?;
        urls(&value, &repo.repository)
    }
    async fn transport_cancelled(&self) {
        self.transport.cancelled().await;
    }
    pub(super) async fn viewer(&self, cwd: &Path) -> Result<String> {
        let repo = ProviderRepositoryRef {
            cwd: cwd.to_string_lossy().into(),
            host: "origin.cursor.com".into(),
            repository: String::new(),
        };
        let key = self.key(&repo, "viewer");
        if let Some((_, v)) = self
            .cache
            .lock()
            .await
            .get(&key)
            .filter(|(until, _)| *until > Instant::now())
        {
            return Ok(v.as_str().unwrap_or_default().into());
        }
        let response = self
            .transport
            .execute(
                cwd,
                "getViewer",
                vec!["auth".into(), "status".into(), "--json".into()],
            )
            .await?;
        let value: Value = serde_json::from_slice(&response.stdout)
            .map_err(|_| error("getViewer", "Origin returned an unreadable auth status."))?;
        let viewer = text(&value["account"], "email")
            .filter(|_| value["authenticated"] == true && value["tokenValid"] != false)
            .ok_or_else(|| {
                ForgeError::provider(
                    "cursor-origin",
                    "getViewer",
                    FailureReason::Unauthenticated,
                    "Sign in using origin auth login.",
                )
            })?
            .to_string();
        self.keep(key, json!(viewer), 300).await;
        Ok(viewer)
    }
    pub(super) async fn discover_cli(
        &self,
        cwd: PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        let version = self
            .transport
            .execute(&cwd, "discover", vec!["--version".into()])
            .await;
        let mut auth = json!({"status":"unauthenticated","account":{"_tag":"None"},"host":{"_tag":"Some","value":"origin.cursor.com"},"detail":{"_tag":"None"}});
        if version.is_ok() {
            match self.viewer(&cwd).await {
                Ok(_) => match self.personal_namespace(&cwd).await {
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
                            });
                    }
                },
                Err(e) => {
                    auth["status"] = json!(if e.reason() == Some(FailureReason::Unauthenticated) {
                        "unauthenticated"
                    } else {
                        "unknown"
                    });
                }
            }
        }
        let version_line = version.as_ref().ok().map(|r| {
            crate::remote::transport_safe_error_value(
                String::from_utf8_lossy(&r.stdout)
                    .lines()
                    .next()
                    .unwrap_or(""),
            )
        });
        parse(
            json!({"kind":"cursor-origin","label":"Cursor Origin","executable":"origin","status":if version.as_ref().is_err_and(|e|e.reason()==Some(FailureReason::MissingTool)){"missing"}else{"available"},"version":version_line.filter(|s|!s.trim().is_empty()).map(|s|json!({"_tag":"Some","value":s})).unwrap_or(json!({"_tag":"None"})),"installHint":"Install Cursor Origin CLI and run origin auth login.","detail":{"_tag":"None"},"auth":auth}),
            "discover",
        )
    }
}
