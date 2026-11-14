use super::{decode as d, *};
impl CursorOrigin {
    pub(super) async fn list(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        let token = if let Some(resume) =
            input.cursor.as_ref().and_then(|c| c.resume_after.as_ref())
        {
            Some(self.tokens.lock().await.get(resume).cloned().ok_or_else(||{
                let mut e=error("listChangeRequests","This list position has expired. Refresh the list to load more pull requests.");
                if let ForgeError::Provider{access_hint,..}=&mut e{*access_hint=Some("This list position has expired. Refresh the list to load more pull requests.".into());}e
            })?)
        } else {
            None
        };
        let state = serde_json::to_value(&input.state)
            .map_err(|_| error("listChangeRequests", "Invalid state."))?;
        let path = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair("state", state.as_str().unwrap_or("all"))
                .append_pair("pageSize", &input.limit.clamp(1, 100).to_string())
                .append_pair("sortBy", "updated");
            if input.involvement == PullRequestInvolvement::Authored {
                query.append_pair("author", &input.viewer);
            }
            if let Some(token) = token {
                query.append_pair("pageToken", &token);
            }
            format!(
                "{}/pulls?{}",
                repo_path(&input.repo.repository),
                query.finish()
            )
        };
        let page = self
            .api(&input.repo, "listChangeRequests", &path, Method::GET, None)
            .await?;
        if !page["pullRequests"].is_null() && !page["pullRequests"].is_array() {
            return Err(error(
                "listChangeRequests",
                "Invalid pull request collection.",
            ));
        }
        let missing_avatars = {
            let avatars = self.avatars.lock().await;
            rows(&page["pullRequests"])
                .filter_map(|p| d::actor_id(&p["author"]))
                .any(|id| !avatars.contains_key(id))
        };
        let avatars = if input.cursor.is_none() && missing_avatars {
            self.read_avatars(
                &input.repo,
                vec![
                    "pr".into(),
                    "list".into(),
                    "-R".into(),
                    input.repo.repository.clone(),
                    "-s".into(),
                    state.as_str().unwrap_or("all").into(),
                    "-L".into(),
                    input.limit.clamp(1, 100).to_string(),
                    "--json".into(),
                    "author".into(),
                ],
            )
            .await
        } else {
            self.avatars.lock().await.clone()
        };
        let items = rows(&page["pullRequests"])
            .map(|p| d::listing(p, &avatars))
            .collect::<Result<_>>()?;
        let next = text(&page, "nextPageToken");
        let resume_after = if let Some(next) = next {
            use sha2::{Digest, Sha256};
            let key = format!("{:x}", Sha256::digest(next.as_bytes()));
            let mut tokens = self.tokens.lock().await;
            if tokens.len() >= 256 {
                tokens.clear();
            }
            tokens.insert(key.clone(), next.into());
            Some(key)
        } else {
            None
        };
        Ok(ProviderChangeRequestPage {
            items,
            truncated: next.is_some(),
            continues: next.is_some(),
            resume_after,
            cursor_advance: None,
        })
    }
    pub(super) async fn checks(
        &self,
        pr: &PullRequestInput,
        raw: &Value,
    ) -> Result<(Vec<Value>, bool)> {
        let Some(head) = text(&raw["head"], "sha") else {
            return Ok((vec![], false));
        };
        let path = format!(
            "{}/commits/{}/check-runs",
            repo_path(&pr.repo.repository),
            encode(head)
        );
        let key = self.key(&pr.repo, &path);
        if let Some((_, value)) = self
            .cache
            .lock()
            .await
            .get(&key)
            .filter(|(until, _)| *until > Instant::now())
        {
            return Ok((
                rows(&value["checks"]).cloned().collect(),
                value["truncated"] == true,
            ));
        }
        let (checks, truncated) = self
            .pages(&pr.repo, "getChangeRequestChecks", &path, "checkRuns")
            .await?;
        let checks = checks.iter().map(d::check).collect::<Vec<_>>();
        self.keep(key, json!({"checks":checks,"truncated":truncated}), 1)
            .await;
        Ok((checks, truncated))
    }
    pub(super) async fn mergeability(&self, pr: &PullRequestInput, raw: &Value) -> Result<String> {
        let Some(head) = text(&raw["head"], "sha").filter(|_| d::state(raw) == "open") else {
            return Ok("unknown".into());
        };
        let branch = text(&raw["base"], "ref").unwrap_or("");
        let tip = recover(
            self.cached_api(
                &pr.repo,
                "getBaseTip",
                &format!(
                    "{}/git/ref/{}",
                    repo_path(&pr.repo.repository),
                    encode(&format!("heads/{branch}"))
                ),
                2,
            )
            .await,
            Value::Null,
            false,
        )?;
        let Some(tip) = text(&tip["object"], "sha") else {
            return Ok("unknown".into());
        };
        let potential = &raw["version"]["potentialMergeCommit"];
        if Some(tip) == text(potential, "baseSha").or_else(|| text(&raw["version"], "baseSha")) {
            return Ok(if Some(head) != text(&raw["version"], "headSha") {
                "unknown"
            } else {
                match potential["state"].as_str() {
                    Some("prepared") => "mergeable",
                    Some("merge_conflict") => "conflicting",
                    _ => "unknown",
                }
            }
            .into());
        }
        let path = format!(
            "{}/mergeability?expectedHeadSha={}",
            pull_path(pr),
            encode(head)
        );
        let key = self.key(&pr.repo, &format!("{path}\0{tip}\0{branch}"));
        if let Some((_, v)) = self
            .cache
            .lock()
            .await
            .get(&key)
            .filter(|(until, _)| *until > Instant::now())
        {
            return Ok(v.as_str().unwrap_or("unknown").into());
        }
        let answer = recover(
            self.api(&pr.repo, "getMergeability", &path, Method::GET, None)
                .await,
            Value::Null,
            false,
        )?;
        let result = if text(&answer, "headSha").is_some_and(|h| h != head) {
            "unknown"
        } else if rows(&answer["blockers"]).any(|b| b["kind"] == "merge_conflict") {
            "conflicting"
        } else if answer["verdict"] == "mergeable" {
            "mergeable"
        } else {
            "unknown"
        };
        if result != "unknown" {
            self.keep(key, json!(result), 3600).await;
        }
        Ok(result.into())
    }
    pub(super) async fn detail(
        &self,
        pr: &PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        let raw = self.raw(pr).await?;
        let repository_path = repo_path(&pr.repo.repository);
        let authors = [raw["author"].clone()];
        let (checks, mergeability, settings, avatars) = tokio::join!(
            self.checks(pr, &raw),
            self.mergeability(pr, &raw),
            self.cached_api(&pr.repo, "getRepository", &repository_path, 300),
            self.avatars_for(pr, &authors, "author")
        );
        let (checks, truncated) = recover(checks, (vec![], false), false)?;
        let settings = recover(settings, Value::Null, false)?;
        let mut value = serde_json::to_value(d::listing(&raw, &avatars)?)
            .map_err(|_| error("getChangeRequest", "Invalid detail."))?;
        let rollup = d::rollup(&checks);
        value["checksState"] = json!(if truncated && rollup == Some("passing") {
            Some("pending")
        } else {
            rollup
        });
        value["mergeability"] = json!(mergeability?);
        value["headSha"] = json!(text(&raw["head"], "sha"));
        value["body"] = json!(raw["body"].as_str().unwrap_or(""));
        value["changedFiles"] = json!(d::number_value(&raw["changedFiles"]));
        value["reviewers"] = json!([]);
        value["checks"] = json!(checks);
        value["viewerPermissions"] = permissions();
        value["mergeCapabilities"] = json!({"merge":settings["allowMergeCommit"].as_bool().unwrap_or(true),"squash":settings["allowSquashMerge"].as_bool().unwrap_or(true),"rebase":false});
        parse(value, "getChangeRequest")
    }
    pub(super) async fn activity(
        &self,
        pr: &PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        let comments_path = format!("{}/comments", pull_path(pr));
        let reviews_path = format!("{}/reviews", pull_path(pr));
        let commits_path = format!("{}/commits", pull_path(pr));
        let (comments, reviews, commits, raw) = tokio::join!(
            self.pages(&pr.repo, "getActivity", &comments_path, "comments"),
            self.pages(&pr.repo, "getActivity", &reviews_path, "reviews"),
            self.pages(&pr.repo, "getActivity", &commits_path, "commits"),
            self.raw(pr)
        );
        let (comments, comments_truncated) = recover(comments, (vec![], true), true)?;
        let (reviews, reviews_truncated) = recover(reviews, (vec![], true), true)?;
        let (commits, _) = recover(commits, (vec![], true), true)?;
        let raw = recover(raw, Value::Null, false)?;
        let actors = comments
            .iter()
            .chain(&reviews)
            .map(|v| v["author"].clone())
            .collect::<Vec<_>>();
        let avatars = self
            .avatars_for(pr, &actors, "author,reviews,comments")
            .await;
        d::activity(
            &comments,
            &reviews,
            &commits,
            &avatars,
            text(&raw["head"], "sha"),
            comments_truncated || reviews_truncated,
            comments_truncated,
        )
    }
    pub(super) async fn diff(&self, input: &DiffInput) -> Result<ProviderDiffSlice> {
        let (path, version) = if let Some(commit) = &input.commit {
            (
                format!(
                    "{}/commits/{}/files",
                    repo_path(&input.pr.repo.repository),
                    encode(commit)
                ),
                Some(commit.clone()),
            )
        } else {
            (
                format!("{}/files", pull_path(&input.pr)),
                d::version(&self.raw(&input.pr).await?),
            )
        };
        let key = version
            .as_ref()
            .map(|v| self.key(&input.pr.repo, &format!("{path}\0{v}")));
        if let Some(key) = &key
            && let Some((_, value)) = self
                .cache
                .lock()
                .await
                .get(key)
                .filter(|(until, _)| *until > Instant::now())
        {
            return parse(value.clone(), "getDiff");
        }
        let (files, truncated) = self
            .pages(&input.pr.repo, "getDiff", &path, "files")
            .await?;
        let value = json!({"patch":d::patch(&files)?,"truncated":truncated||files.iter().any(|f|f["patch"].is_null()),"nextCursor":null});
        let pinned = if input.commit.is_some() {
            true
        } else if let Some(version) = version {
            let current = recover(
                self.api(
                    &input.pr.repo,
                    "getDiff",
                    &pull_path(&input.pr),
                    Method::GET,
                    None,
                )
                .await,
                Value::Null,
                false,
            )?;
            d::version(&current).as_ref() == Some(&version)
        } else {
            false
        };
        if pinned && let Some(key) = key {
            self.keep(key, value.clone(), 3600).await;
        }
        parse(value, "getDiff")
    }
}
