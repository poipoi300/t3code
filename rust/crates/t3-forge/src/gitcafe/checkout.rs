use super::*;
#[derive(Clone)]
pub(super) struct Target {
    pub host: String,
    pub repository: String,
}
fn remote(raw: &str) -> Option<Target> {
    let parsed = crate::remote::parse_remote(raw)?;
    if !valid_host(&parsed.hostname) {
        return None;
    }
    let repository = parsed.repository?;
    super::repository(&repository).ok()?;
    Some(Target {
        host: parsed.hostname,
        repository,
    })
}
fn clone_urls(target: &Target) -> Result<SourceControlRepositoryCloneUrls> {
    parse(
        json!({"nameWithOwner":target.repository,"url":format!("https://{}/{}",target.host,target.repository),"sshUrl":format!("ssh@{}:{}.git",target.host,target.repository)}),
        "cloneUrls",
    )
}
fn record(raw: &Value, target: &Target) -> Result<ChangeRequest> {
    let number = raw["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| error("checkout", "Invalid pull request number."))?;
    let updated =
        text(raw, "updatedAt").filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok());
    let source = &raw["sourceRepo"];
    let source_repo = if source.is_object() {
        format!(
            "{}/{}",
            source["owner"].as_str().unwrap_or(""),
            source["name"].as_str().unwrap_or("")
        )
    } else {
        target.repository.clone()
    };
    let mut value = json!({"provider":"gitcafe","number":number,"title":raw["title"],"url":format!("https://{}/{}/pulls/{number}",target.host,target.repository),"baseRefName":raw["targetBranch"],"headRefName":raw["sourceBranch"],"state":raw["state"],"closedAt":raw["closedAt"],"mergedAt":raw["mergedAt"],"updatedAt":updated.map(|s|json!({"_tag":"Some","value":s})).unwrap_or(json!({"_tag":"None"})),"isCrossRepository":source.is_object(),"headRepositoryNameWithOwner":source_repo,"headRepositoryOwnerLogin":source_repo.split('/').next()});
    if raw["draft"] == true {
        value["isDraft"] = json!(true);
    }
    if let Some(head) = text(raw, "headOid") {
        value["headSha"] = json!(head);
    }
    parse(value, "checkout")
}
impl GitCafe {
    pub(super) async fn target(
        &self,
        input: &CheckoutInput,
        named: Option<&str>,
    ) -> Result<Target> {
        if let Some(target) = named.and_then(remote) {
            return Ok(target);
        }
        let remote = if let Some(context) = &input.context {
            Some(context.remote_url.clone())
        } else {
            let git = self.git.at(&input.cwd);
            let read = async {
                let name = git.primary_remote_name().await?.unwrap_or("origin".into());
                git.read_config_value(&format!("remote.{name}.url")).await
            };
            match read.await {
                Ok(value) => value,
                Err(_) if named.is_some() => None,
                Err(_) => return Err(error("resolveTarget", "Could not read the GitCafe remote.")),
            }
        }
        .as_deref()
        .and_then(remote);
        if let Some(named) = named {
            let repository = named.trim().trim_end_matches('/').trim_end_matches(".git");
            super::repository(repository)?;
            return Ok(Target {
                host: remote.map(|r| r.host).unwrap_or("git.cafe".into()),
                repository: repository.into(),
            });
        }
        remote.ok_or_else(|| error("resolveTarget", "No GitCafe repository remote was found."))
    }
    pub(super) async fn checkout_raw(&self, input: &CheckoutGetInput) -> Result<(Target, Value)> {
        let link = crate::remote::parse_pull_request_url(input.reference.trim())
            .filter(|l| l.provider == "gitcafe" && valid_host(&l.host));
        let (target, number) = if let Some(link) = link {
            (
                Target {
                    host: link.host,
                    repository: link.repository,
                },
                link.number,
            )
        } else {
            let number = input
                .reference
                .trim()
                .trim_start_matches('#')
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    error(
                        "checkout",
                        "A GitCafe pull request is a number or GitCafe URL.",
                    )
                })?;
            (self.target(&input.checkout, None).await?, number)
        };
        let raw = self
            .api(
                &target.host,
                "checkoutGet",
                Method::GET,
                &format!("{}/pulls/{number}", repository(&target.repository)?),
                None,
            )
            .await?;
        record(&raw, &target)?;
        Ok((target, raw))
    }
    pub(super) async fn checkout_get(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        let (target, raw) = self.checkout_raw(&input).await?;
        record(&raw, &target)
    }
    pub(super) async fn checkout_list(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        let target = self.target(&input.checkout, None).await?;
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let branch = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(&input.head_selector);
        let limit = input.limit.unwrap_or(20);
        let mut items = vec![];
        let mut after = None::<String>;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10000 {
            let mut params = vec![
                ("sourceBranches", json!([branch]).to_string()),
                ("limit", limit.min(100).to_string()),
                ("sort", "newest".into()),
            ];
            if input.state.is_some() {
                params.push((
                    "state",
                    match input.state {
                        Some(ChangeRequestState::Closed) => "closed",
                        Some(ChangeRequestState::Merged) => "merged",
                        _ => "open",
                    }
                    .into(),
                ));
            }
            if let Some(after) = &after {
                params.push(("after", after.clone()));
            }
            let raw = self
                .api(
                    &target.host,
                    "checkoutList",
                    Method::GET,
                    &format!(
                        "{}/pulls?{}",
                        repository(&target.repository)?,
                        query(&params)
                    ),
                    None,
                )
                .await?;
            let page = raw["items"]
                .as_array()
                .ok_or_else(|| error("checkoutList", "Invalid pull request page."))?;
            for pull in page {
                if pull["sourceBranch"] != branch || items.len() >= limit {
                    continue;
                }
                let number = pull["number"]
                    .as_u64()
                    .ok_or_else(|| error("checkoutList", "Invalid pull request number."))?;
                let detail = self
                    .checkout_get(CheckoutGetInput {
                        checkout: input.checkout.clone(),
                        reference: format!(
                            "https://{}/{}/pulls/{number}",
                            target.host, target.repository
                        ),
                        force: false,
                    })
                    .await?;
                if source.as_ref().is_none_or(|s| {
                    s.repository.as_ref().is_none_or(|r| {
                        detail
                            .head_repository_name_with_owner
                            .as_ref()
                            .and_then(|r| r.as_ref())
                            .is_some_and(|v| v.0 == *r)
                    }) && s.owner.as_ref().is_none_or(|owner| {
                        detail
                            .head_repository_owner_login
                            .as_ref()
                            .and_then(|v| v.as_ref())
                            .is_some_and(|v| v.0 == *owner)
                    })
                }) {
                    items.push(detail);
                }
            }
            let next = text(&raw, "next").map(str::to_string);
            if page.is_empty() || next.is_none() || next == after || items.len() >= limit {
                break;
            }
            if !seen.insert(next.clone()) {
                return Err(error("checkoutList", "GitCafe repeated a listing cursor."));
            }
            after = next;
        }
        Ok(items)
    }
    pub(super) async fn checkout_request(&self, input: CheckoutGetInput) -> Result<()> {
        let (target, raw) = self.checkout_raw(&input).await?;
        let pull = record(&raw, &target)?;
        let source = pull
            .head_repository_name_with_owner
            .as_ref()
            .and_then(|v| v.as_ref())
            .ok_or_else(|| error("checkout", "The source repository is unavailable."))?;
        let git = self.git.at(&input.checkout.cwd);
        let context_remote = input
            .checkout
            .context
            .as_ref()
            .and_then(|c| remote(&c.remote_url));
        let remote_name = if let Some(context) = input.checkout.context.as_ref().filter(|_| {
            context_remote
                .as_ref()
                .is_some_and(|r| r.host == target.host && r.repository == source.0)
        }) {
            context.remote_name.clone()
        } else {
            let ssh = input
                .checkout
                .context
                .as_ref()
                .and_then(|c| crate::remote::parse_remote(&c.remote_url))
                .is_some_and(|r| r.ssh);
            let url = if ssh {
                format!("ssh@{}:{}.git", target.host, source.0)
            } else {
                format!("https://{}/{}.git", target.host, source.0)
            };
            git.ensure_remote("gitcafe", &url)
                .await
                .map_err(|_| error("checkout", "Could not configure the pull request remote."))?
        };
        let branch = &pull.head_ref_name.0;
        let cross = pull
            .is_cross_repository
            .as_ref()
            .and_then(|v| *v)
            .unwrap_or(false);
        let local = if cross {
            format!("pr-{}/{branch}", pull.number)
        } else {
            branch.clone()
        };
        let exists = git
            .list_local_branch_names()
            .await
            .map_err(|_| error("checkout", "Could not list local branches."))?
            .contains(&local);
        if !input.force && !exists {
            git.fetch_remote_branch(&remote_name, branch, &local).await
        } else {
            git.fetch_remote_tracking_branch(&remote_name, branch).await
        }
        .map_err(|_| error("checkout", "Could not fetch the pull request branch."))?;
        if input.force {
            git.execute(
                &[
                    "checkout",
                    "--no-track",
                    "-B",
                    &local,
                    &format!("refs/remotes/{remote_name}/{branch}"),
                    "--",
                ],
                Default::default(),
            )
            .await
            .map_err(|_| error("checkout", "Could not reset the pull request branch."))?;
        }
        git.set_branch_upstream(&local, &remote_name, branch)
            .await
            .map_err(|_| error("checkout", "Could not set branch upstream."))?;
        git.switch_ref(&local)
            .await
            .map_err(|_| error("checkout", "Could not switch to the pull request branch."))?;
        Ok(())
    }
    pub(super) async fn clone_urls(
        &self,
        input: CloneUrlsInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let target = self
            .target(&input.checkout, Some(&input.repository))
            .await?;
        self.api(
            &target.host,
            "getRepositoryCloneUrls",
            Method::GET,
            &repository(&target.repository)?,
            None,
        )
        .await?;
        clone_urls(&target)
    }
    pub(super) async fn create_repo(
        &self,
        input: CreateRepositoryInput,
    ) -> Result<SourceControlRepositoryCloneUrls> {
        let target = self
            .target(
                &CheckoutInput {
                    cwd: input.cwd,
                    context: None,
                },
                Some(&input.repository),
            )
            .await?;
        let (owner, name) = target
            .repository
            .split_once('/')
            .ok_or_else(|| error("createRepository", "Invalid repository."))?;
        let pending=self.api(&target.host,"createRepository",Method::POST,&format!("/orgs/{owner}/repos"),Some(json!({"name":name,"requestKey":request_id(),"visibility":input.visibility,"defaultBranch":"main"}))).await?;
        let poll = async {
            let mut current = pending.clone();
            loop {
                if matches!(current["state"].as_str(), Some("complete" | "blocked")) {
                    break Ok(current);
                }
                let owner = text(&current, "owner")
                    .ok_or_else(|| error("createRepository", "Invalid repository admission."))?;
                let admission = id(text(&current, "repoId")
                    .ok_or_else(|| error("createRepository", "Missing admission identifier."))?)?;
                current = self
                    .api(
                        &target.host,
                        "createRepository",
                        Method::GET,
                        &format!("/orgs/{}/admissions/{admission}", encode(owner)),
                        None,
                    )
                    .await?;
                if !matches!(current["state"].as_str(), Some("complete" | "blocked")) {
                    tokio::select! {_ = self.stop.cancelled()=>return Err(error("createRepository","Repository admission was cancelled.")),_ = tokio::time::sleep(std::time::Duration::from_millis(500))=>{}}
                }
            }
        };
        let settled = tokio::time::timeout(std::time::Duration::from_secs(30), poll)
            .await
            .unwrap_or(Ok(pending))?;
        if settled["state"] != "complete" {
            return Err(error(
                "createRepository",
                "Repository admission did not complete. Check GitCafe before retrying.",
            ));
        }
        let repository = format!(
            "{}/{}",
            settled["owner"].as_str().unwrap_or(""),
            settled["name"].as_str().unwrap_or("")
        );
        super::repository(&repository)?;
        clone_urls(&Target {
            host: target.host,
            repository,
        })
    }
    pub(super) async fn create_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        let target = self
            .target(
                &input.checkout,
                input.target.as_ref().and_then(|t| t.repository.as_deref()),
            )
            .await?;
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let source_repo = source
            .as_ref()
            .and_then(|s| s.repository.clone())
            .or_else(|| {
                source.as_ref().and_then(|s| s.owner.as_ref()).map(|owner| {
                    format!(
                        "{owner}/{}",
                        target.repository.split('/').nth(1).unwrap_or("")
                    )
                })
            })
            .unwrap_or(target.repository.clone());
        let head = self
            .api(
                &target.host,
                "createChangeRequest",
                Method::GET,
                &repository(&source_repo)?,
                None,
            )
            .await?;
        let head_id = text(&head, "id")
            .ok_or_else(|| error("createChangeRequest", "Missing source repository id."))?;
        let body = tokio::fs::read_to_string(input.body_file)
            .await
            .map_err(|_| {
                error(
                    "createChangeRequest",
                    "Could not read the pull request description.",
                )
            })?;
        let branch = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(&input.head_selector);
        let mut payload = json!({"repo":head_id,"source":branch,"target":input.target.map(|t|t.ref_name).unwrap_or(input.base_ref_name),"title":input.title});
        if !body.trim().is_empty() {
            payload["description"] = json!(body);
        }
        self.api(
            &target.host,
            "createChangeRequest",
            Method::POST,
            &format!("{}/pulls", repository(&target.repository)?),
            Some(payload),
        )
        .await?;
        Ok(())
    }
}
