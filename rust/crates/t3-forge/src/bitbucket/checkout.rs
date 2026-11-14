use super::{decode as d, *};
impl Bitbucket {
    pub(super) async fn checkout_repository(
        &self,
        input: &CheckoutInput,
        reference: Option<&str>,
    ) -> Result<String> {
        if let Some(context) = &input.context {
            if context.provider.kind.0 != "bitbucket"
                || !self.serves(
                    &crate::remote::parse_remote(&context.remote_url)
                        .map(|r| r.host)
                        .unwrap_or_default(),
                )
            {
                return Err(error(
                    "resolveRepository",
                    "No supported Bitbucket remote was selected.",
                ));
            }
            if let Some(repository) =
                crate::remote::parse_remote(&context.remote_url).and_then(|r| r.repository)
            {
                super::repository(&repository)?;
                return Ok(repository);
            }
        }
        if let Some(url) = reference.and_then(|s| url::Url::parse(s).ok())
            && let Some(link) = crate::remote::parse_pull_request_url(url.as_str())
            && link.provider == "bitbucket"
            && self.serves(&link.host)
        {
            super::repository(&link.repository)?;
            return Ok(link.repository);
        }
        let git = self.git.at(&input.cwd);
        let output = git
            .execute(&["remote", "-v"], Default::default())
            .await
            .map_err(|_| error("resolveRepository", "Could not list repository remotes."))?;
        for line in output.stdout_text().lines() {
            let Some(url) = line.split_whitespace().nth(1) else {
                continue;
            };
            if let Some(remote) = crate::remote::parse_remote(url)
                && self.serves(&remote.host)
                && let Some(repo) = remote.repository
            {
                super::repository(&repo)?;
                return Ok(repo);
            }
        }
        Err(error(
            "resolveRepository",
            "No Bitbucket repository remote was found.",
        ))
    }
    pub(super) async fn checkout_raw(&self, input: &CheckoutGetInput) -> Result<(String, Value)> {
        let repository = self
            .checkout_repository(&input.checkout, Some(&input.reference))
            .await?;
        let number = if let Ok(number) = input.reference.parse::<u64>() {
            number
        } else {
            url::Url::parse(&input.reference)
                .ok()
                .and_then(|u| crate::remote::parse_pull_request_url(u.as_str()))
                .filter(|l| l.provider == "bitbucket")
                .map(|l| l.number)
                .ok_or_else(|| error("reference", "Invalid Bitbucket pull request reference."))?
        };
        if number == 0 {
            return Err(error("reference", "Invalid pull request number."));
        }
        let raw = self
            .api(
                "getPullRequest",
                Method::GET,
                &format!("{}/pullrequests/{number}", super::repository(&repository)?),
                None,
            )
            .await?;
        d::listing(&raw)?;
        Ok((repository, raw))
    }
    pub(super) async fn checkout_list(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        let repo = self.checkout_repository(&input.checkout, None).await?;
        let state = match input.state {
            Some(ChangeRequestState::Merged) => vec!["MERGED"],
            Some(ChangeRequestState::Closed) => vec!["DECLINED", "SUPERSEDED"],
            Some(ChangeRequestState::Open) => vec!["OPEN"],
            None => vec!["OPEN", "MERGED", "DECLINED", "SUPERSEDED"],
        };
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let branch = source.map(|s| s.ref_name).unwrap_or(input.head_selector);
        let query = format!(
            "source.branch.name = \"{}\" AND ({})",
            literal(&branch),
            state
                .iter()
                .map(|s| format!("state = \"{s}\""))
                .collect::<Vec<_>>()
                .join(" OR ")
        );
        let raw = self
            .api(
                "listPullRequests",
                Method::GET,
                &format!(
                    "{}/pullrequests?pagelen={}&sort=-updated_on&q={}&{}",
                    super::repository(&repo)?,
                    input.limit.unwrap_or(20).clamp(1, 50),
                    encode(&query),
                    state
                        .into_iter()
                        .map(|s| format!("state={s}"))
                        .collect::<Vec<_>>()
                        .join("&")
                ),
                None,
            )
            .await?;
        let values = raw["values"]
            .as_array()
            .ok_or_else(|| error("checkoutList", "Invalid pull request list."))?;
        values.iter().map(d::checkout).collect()
    }
    pub(super) async fn checkout_request(&self, input: CheckoutGetInput) -> Result<()> {
        let (repository, raw) = self.checkout_raw(&input).await?;
        let source = text(&raw["source"]["repository"], "full_name").unwrap_or(&repository);
        let target = text(&raw["destination"]["repository"], "full_name").unwrap_or(&repository);
        let cross = source != target;
        let git = self.git.at(&input.checkout.cwd);
        let urls = d::urls(
            &self
                .api(
                    "getRepository",
                    Method::GET,
                    &super::repository(source)?,
                    None,
                )
                .await?,
        )?;
        let origin = git
            .execute(
                &["config", "--get", "remote.origin.url"],
                t3_git::ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| error("checkout", "Could not read the repository remote."))?;
        let ssh = crate::remote::parse_remote(origin.stdout_text().trim()).is_some_and(|r| r.ssh);
        let remote = git
            .ensure_remote(
                source.split('/').next().unwrap_or("bitbucket"),
                if ssh { &urls.ssh_url.0 } else { &urls.url.0 },
            )
            .await
            .map_err(|_| error("checkout", "Could not configure the pull request remote."))?;
        let branch = text(&raw["source"]["branch"], "name")
            .ok_or_else(|| error("checkout", "Missing source branch."))?;
        let local = if cross {
            format!(
                "t3code/pr-{}/{}",
                raw["id"],
                branch
                    .chars()
                    .map(|c| if c.is_alphanumeric() || "/._-".contains(c) {
                        c
                    } else {
                        '-'
                    })
                    .collect::<String>()
                    .trim_matches('/')
            )
        } else {
            branch.into()
        };
        let exists = git
            .list_local_branch_names()
            .await
            .map_err(|_| error("checkout", "Could not list local branches."))?
            .contains(&local);
        if input.force || !exists {
            git.fetch_remote_branch(&remote, branch, &local).await
        } else {
            git.fetch_remote_tracking_branch(&remote, branch).await
        }
        .map_err(|_| error("checkout", "Could not fetch the pull request branch."))?;
        git.set_branch_upstream(&local, &remote, branch)
            .await
            .map_err(|_| error("checkout", "Could not set the branch upstream."))?;
        git.switch_ref(&local)
            .await
            .map_err(|_| error("checkout", "Could not switch to the pull request branch."))?;
        Ok(())
    }
    pub(super) async fn create_request(&self, input: CreateChangeRequestInput) -> Result<()> {
        let repo = self.checkout_repository(&input.checkout, None).await?;
        let body = tokio::fs::read_to_string(input.body_file)
            .await
            .map_err(|_| {
                error(
                    "createPullRequest",
                    "Could not read the pull request description.",
                )
            })?;
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let branch = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(&input.head_selector);
        let mut payload = json!({"title":input.title,"description":body,"source":{"branch":{"name":branch}},"destination":{"branch":{"name":input.target.map(|t|t.ref_name).unwrap_or(input.base_ref_name)}}});
        if let Some(source) = source
            && let Some(owner) = source.owner
        {
            payload["source"]["repository"] = json!({"full_name":format!("{owner}/{}",source.repository.as_deref().unwrap_or(repo.split('/').nth(1).unwrap_or("")))});
        }
        self.api(
            "createPullRequest",
            Method::POST,
            &format!("{}/pullrequests", super::repository(&repo)?),
            Some(payload),
        )
        .await?;
        Ok(())
    }
    pub(super) async fn default_branch(&self, input: CheckoutInput) -> Result<Option<String>> {
        let repo = self.checkout_repository(&input, None).await?;
        let path = super::repository(&repo)?;
        let raw = self.api("getRepository", Method::GET, &path, None).await?;
        let model = self
            .api(
                "getBranchingModel",
                Method::GET,
                &format!("{path}/branching-model"),
                None,
            )
            .await
            .unwrap_or(Value::Null);
        let development = &model["development"];
        let branch = if development["use_mainbranch"] != true && development["is_valid"] != false {
            text(&development["branch"], "name")
                .or_else(|| text(development, "name"))
                .filter(|s| *s != "null")
        } else {
            None
        };
        Ok(branch
            .or_else(|| text(&raw["mainbranch"], "name"))
            .map(str::to_string))
    }
}
