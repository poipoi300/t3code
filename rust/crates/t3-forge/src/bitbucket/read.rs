use super::{decode as d, *};
impl Bitbucket {
    pub(super) async fn list(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        self.check_host(&input.repo)?;
        let states = match input.state {
            PullRequestListState::Open => vec!["OPEN"],
            PullRequestListState::Closed => vec!["DECLINED", "SUPERSEDED"],
            PullRequestListState::Merged => vec!["MERGED"],
            PullRequestListState::All => vec!["OPEN", "MERGED", "DECLINED", "SUPERSEDED"],
        };
        let filters = input
            .filters
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| error("list", "Invalid filters."))?
            .unwrap_or(Value::Null);
        let mut predicates = Vec::new();
        if let Some(query) = input
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let query = literal(query);
            predicates.push(format!(
                "(title ~ \"{query}\" OR description ~ \"{query}\")"
            ));
        }
        if let Some(author) = filters["author"].as_str() {
            let author = literal(resolve_author(author, &input.viewer));
            predicates.push(format!(
                "(author.nickname = \"{author}\" OR author.display_name = \"{author}\")"
            ));
        }
        if let Some(draft) = filters["draft"].as_str() {
            predicates.push(format!("draft = {}", draft == "only"));
        }
        if let Some(cursor) = &input.cursor {
            let at = chrono::DateTime::parse_from_rfc3339(&cursor.updated_before)
                .map_err(|_| error("list", "Invalid list cursor."))?;
            let next = at
                .checked_add_signed(chrono::Duration::milliseconds(1))
                .ok_or_else(|| error("list", "Invalid list cursor."))?;
            predicates.push(format!(
                "updated_on < {}",
                next.to_utc()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ));
            for id in cursor.seen_at.iter().flatten() {
                predicates.push(format!("id != {id}"));
            }
        }
        let mut path = Some(format!(
            "{}/pullrequests?{}&pagelen=50&sort=-updated_on&fields=%2Bvalues.reviewers{}",
            repository(&input.repo.repository)?,
            states
                .into_iter()
                .map(|s| format!("state={s}"))
                .collect::<Vec<_>>()
                .join("&"),
            if predicates.is_empty() {
                String::new()
            } else {
                format!("&q={}", encode(&predicates.join(" AND ")))
            }
        ));
        let mut items = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut extra = false;
        for _ in 0..10 {
            let Some(url) = path.take() else {
                break;
            };
            if !visited.insert(url.clone()) {
                return Err(error("list", "Repeated pagination cursor."));
            }
            let raw = self
                .api("listPullRequests", Method::GET, &url, None)
                .await?;
            let values = raw["values"]
                .as_array()
                .ok_or_else(|| error("list", "Invalid pull request list."))?;
            let valid: Vec<_> = values.iter().filter_map(|v| d::listing(v).ok()).collect();
            items.extend(valid);
            path = text(&raw, "next").map(str::to_string);
            if items.len() >= input.limit {
                extra = items.len() > input.limit;
                items.truncate(input.limit);
                break;
            }
        }
        parse(
            json!({"items":items,"truncated":extra||path.is_some(),"continues":true}),
            "listChangeRequests",
        )
    }
    pub(super) async fn can_write(&self, repo: &ProviderRepositoryRef) -> Result<bool> {
        self.check_host(repo)?;
        repository(&repo.repository)?;
        let workspace = repo.repository.split('/').next().unwrap_or("");
        let result = self
            .api(
                "getRepositoryPermission",
                Method::GET,
                &format!(
                    "/user/workspaces/{}/permissions/repositories?q={}",
                    encode(workspace),
                    encode(&format!(
                        "repository.full_name=\"{}\"",
                        literal(&repo.repository)
                    ))
                ),
                None,
            )
            .await;
        let raw = match result {
            Ok(v) => v,
            Err(ForgeError::Provider {
                status: Some(403 | 404 | 410),
                ..
            }) => return Ok(true),
            Err(e) => return Err(e),
        };
        let permission = text(&raw["values"][0], "permission").map(str::to_lowercase);
        Ok(permission.is_none_or(|v| matches!(v.as_str(), "admin" | "write")))
    }
    pub(super) async fn checks(&self, input: &PullRequestInput) -> Result<Vec<Value>> {
        self.check_host(&input.repo)?;
        let (rows, _) = self
            .pages(
                "listChecks",
                format!("{}/statuses?pagelen=50", request_path(input)?),
                1000,
            )
            .await?;
        let mut unique = Vec::<Value>::new();
        let mut indexes = BTreeMap::new();
        let mut workflows = Vec::new();
        for raw in rows {
            if let Some(check) = d::check(&raw) {
                let key = (
                    check["name"].as_str().unwrap_or("").to_string(),
                    text(&raw, "key").map(str::to_string),
                );
                if let Some(index) = indexes.get(&key).copied() {
                    unique[index] = check;
                } else {
                    indexes.insert(key.clone(), unique.len());
                    workflows.push(key.1);
                    unique.push(check);
                }
            }
        }
        let mut counts = BTreeMap::new();
        for check in &unique {
            *counts
                .entry(check["name"].as_str().unwrap_or("").to_string())
                .or_insert(0) += 1;
        }
        for (check, workflow) in unique.iter_mut().zip(workflows) {
            let name = check["name"].as_str().unwrap_or("");
            if counts.get(name).copied().unwrap_or(0) > 1
                && let Some(workflow) = workflow
            {
                check["name"] = json!(format!("{workflow} / {name}"));
            }
        }
        unique.sort_by_key(|check| {
            !matches!(check["status"].as_str(), Some("failure" | "cancelled"))
        });
        Ok(unique)
    }
    pub(super) async fn detail(&self, input: &PullRequestInput) -> Result<Value> {
        self.check_host(&input.repo)?;
        let conflicts_path = format!("{}/conflicts", request_path(input)?);
        let (raw, stats, conflicts, checks, write) = tokio::join!(
            self.raw(input),
            self.pages(
                "getDiffStat",
                format!("{}/diffstat?pagelen=50", request_path(input)?),
                1000
            ),
            self.api("getMergeability", Method::GET, &conflicts_path, None),
            self.checks(input),
            self.can_write(&input.repo)
        );
        let raw = raw?;
        let (stats, _) = stats?;
        let mut additions = 0;
        let mut deletions = 0;
        let mut changed = 0;
        for stat in stats.iter().filter(|v| v.is_object()) {
            additions += stat["lines_added"].as_u64().unwrap_or(0);
            deletions += stat["lines_removed"].as_u64().unwrap_or(0);
            changed += 1;
        }
        let conflicts = recover(conflicts, Value::Null)?;
        let mergeability = conflicts["values"]
            .as_array()
            .map(|v| {
                if v.is_empty() {
                    "mergeable"
                } else {
                    "conflicting"
                }
            })
            .unwrap_or("unknown");
        let checks = recover(checks, vec![])?;
        let write = recover(write, true)?;
        let mut value = d::listing(&raw)?;
        value["body"] = json!(raw["description"].as_str().unwrap_or(""));
        value["additions"] = json!(additions);
        value["deletions"] = json!(deletions);
        value["changedFiles"] = json!(changed);
        value["mergeability"] = json!(mergeability);
        value["reviewers"] = json!(
            rows(&raw["reviewers"])
                .map(d::actor)
                .filter(|a| !a.is_null())
                .collect::<Vec<_>>()
        );
        value["checks"] = json!(checks);
        value["mergeCapabilities"] = json!({"merge":true,"squash":true,"rebase":true});
        value["viewerPermissions"] = permissions(write);
        value["closedAt"] = if d::state(&raw) == "closed" {
            raw["closed_on"].clone()
        } else {
            Value::Null
        };
        value["mergedAt"] = if d::state(&raw) == "merged" {
            raw["closed_on"].clone()
        } else {
            Value::Null
        };
        Ok(value)
    }
    pub(super) async fn activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        let comments_path = format!("{}/comments?pagelen=50", request_path(&input)?);
        let commits_path = format!("{}/commits?pagelen=50", request_path(&input)?);
        let (raw, comments, commits) = tokio::join!(
            self.raw(&input),
            self.pages("listComments", comments_path, 10),
            self.pages("listCommits", commits_path, 1000)
        );
        let raw = raw?;
        let (entries, truncated) = recover(comments, (vec![], true))?;
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|v| d::comment(v).is_some())
            .collect();
        let threads = d::threads(&entries);
        let mut comments: Vec<_> = entries
            .iter()
            .filter_map(d::comment)
            .chain(d::reviews(&raw))
            .collect();
        comments.sort_by_key(|c| text(c, "createdAt").map(str::to_string));
        let (commits, _) = recover(commits, (vec![], true))?;
        let commits: Vec<_> = commits.iter().rev().filter_map(d::commit).collect();
        parse(
            json!({"commentCount":comments.len(),"comments":comments,"commentsTruncated":truncated,"reviewThreads":threads,"commits":commits}),
            "activity",
        )
    }
    pub(super) async fn diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        self.check_host(&input.pr.repo)?;
        if input.commit.as_ref().is_some_and(|c| !sha(c)) {
            return Err(error("getDiff", "Invalid commit SHA."));
        }
        let path = if let Some(commit) = input.commit {
            format!("{}/diff/{commit}", repository(&input.pr.repo.repository)?)
        } else {
            format!("{}/diff", request_path(&input.pr)?)
        };
        let response = self
            .http
            .request(
                "getPullRequestDiff",
                Method::GET,
                &path,
                None,
                8 * 1024 * 1024,
            )
            .await?;
        Ok(ProviderDiffSlice {
            patch: String::from_utf8_lossy(&response.bytes).into(),
            truncated: response.truncated,
            next_cursor: None,
            omitted_file_stats: None,
        })
    }
    pub(super) async fn revisions(
        &self,
        input: FileRevisionsInput,
    ) -> Result<ProviderFileRevisions> {
        if input.paths.is_empty() {
            return Ok(ProviderFileRevisions {
                revisions: BTreeMap::new(),
                complete: Some(false),
            });
        }
        self.check_host(&input.pr.repo)?;
        let key = format!(
            "{}:{}:{}",
            self.http.settings_revision(),
            input.pr.repo.repository,
            input.pr.number
        );
        let cached = self
            .revisions
            .lock()
            .await
            .get(&key)
            .filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(5))
            .map(|(_, r)| r.clone());
        let mut revisions = if let Some(value) = cached {
            value
        } else {
            let diff = self
                .diff(DiffInput {
                    pr: input.pr,
                    cursor: None,
                    commit: None,
                })
                .await?;
            let result = ProviderFileRevisions {
                revisions: crate::diff::revisions(&diff.patch),
                complete: Some(!diff.truncated),
            };
            let mut cache = self.revisions.lock().await;
            if cache.len() >= 16 {
                cache.clear();
            }
            cache.insert(key, (tokio::time::Instant::now(), result.clone()));
            result
        };
        if revisions.complete == Some(true) {
            for path in input.paths {
                revisions.revisions.entry(path).or_default();
            }
        }
        Ok(revisions)
    }
}
