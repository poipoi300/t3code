use super::{decode as d, *};
use base64::{Engine, engine::general_purpose::STANDARD};
impl Forgejo {
    pub(super) async fn list(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        let state = if input.state == PullRequestListState::Merged {
            "closed"
        } else {
            match input.state {
                PullRequestListState::All => "all",
                PullRequestListState::Closed => "closed",
                _ => "open",
            }
        };
        let mut params = vec![
            ("state", state.to_string()),
            ("sort", "recentupdate".into()),
        ];
        let filters = input
            .filters
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| error("list", "Invalid filters."))?
            .unwrap_or(Value::Null);
        if let Some(label) = rows(&filters["labels"])
            .find(|g| rows(g).count() == 1)
            .and_then(|g| g[0].as_str())
        {
            let (labels, _) = self
                .page(
                    &input.repo,
                    &format!("{}/labels", repo_path(&input.repo.repository)),
                    500,
                )
                .await?;
            if let Some(found) = labels
                .iter()
                .find(|l| text(l, "name").is_some_and(|n| n.eq_ignore_ascii_case(label.trim())))
            {
                params.push(("labels", found["id"].to_string()));
            }
        }
        let author = text(&filters, "author");
        if let Some(author) = author {
            params.push(("poster", resolve_author(author, &input.viewer).into()));
        }
        let query = format!(
            "{}/pulls?{}",
            repo_path(&input.repo.repository),
            params
                .iter()
                .map(|(k, v)| format!("{k}={}", encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        );
        let repo = &input.repo;
        let read_page = |index| {
            let query = &query;
            async move {
                let (raw, link) = self
                    .api(
                        repo,
                        &format!("{query}&limit=50&page={index}"),
                        Method::GET,
                        None,
                    )
                    .await?;
                let rows = if raw.is_null() {
                    vec![]
                } else {
                    raw.as_array()
                        .cloned()
                        .ok_or_else(|| error("list", "Invalid pull request page."))?
                };
                let more = !rows.is_empty() && link.as_ref().is_none_or(|l| has_next(l));
                Ok::<_, ForgeError>((rows, more))
            }
        };
        let first = match read_page(1).await {
            Ok(v) => v,
            Err(ForgeError::Provider {
                status: Some(400), ..
            }) if author.is_some() => {
                return Ok(ProviderChangeRequestPage {
                    items: vec![],
                    truncated: false,
                    continues: false,
                    cursor_advance: None,
                    resume_after: None,
                });
            }
            Err(e) => return Err(e),
        };
        let page_size = if first.0.is_empty() {
            50
        } else {
            first.0.len()
        };
        let offset = input.cursor.as_ref().map_or(0, |c| c.delivered) as usize;
        let first_index = offset / page_size + 1;
        let mut items = vec![];
        let mut consumed = 0;
        let mut more = true;
        for index in first_index..first_index.saturating_add(10000) {
            let (rows, page_more) = if index == 1 {
                first.clone()
            } else {
                read_page(index).await?
            };
            let start = if index == first_index {
                offset % page_size
            } else {
                0
            };
            let mut taken = 0;
            for row in rows.iter().skip(start) {
                if items.len() >= input.limit {
                    break;
                }
                consumed += 1;
                taken += 1;
                if row.is_null() {
                    continue;
                }
                let value = d::listing(row)?;
                if input.state != PullRequestListState::All
                    && value["state"]
                        != match input.state {
                            PullRequestListState::Closed => "closed",
                            PullRequestListState::Merged => "merged",
                            _ => "open",
                        }
                {
                    continue;
                }
                items.push(value);
            }
            more = page_more || rows.len().saturating_sub(start) > taken;
            if !more || items.len() >= input.limit {
                break;
            }
        }
        parse(
            json!({"items":items,"truncated":more,"continues":true,"cursorAdvance":consumed}),
            "listChangeRequests",
        )
    }
    pub(super) async fn checks(
        &self,
        repo: &ProviderRepositoryRef,
        pr: &Value,
    ) -> Result<Vec<Value>> {
        let status_path = format!(
            "{}/statuses/{}?sort=recentupdate",
            repo_path(&repo.repository),
            encode(pr["head"]["sha"].as_str().unwrap_or(""))
        );
        let policy_path = format!(
            "{}/branches/{}",
            repo_path(&repo.repository),
            pr["base"]["ref"]
                .as_str()
                .unwrap_or("")
                .split('/')
                .map(encode)
                .collect::<Vec<_>>()
                .join("/")
        );
        let (statuses, policy) = tokio::join!(
            self.page(repo, &status_path, 500),
            self.api(repo, &policy_path, Method::GET, None)
        );
        let (statuses, _) = statuses?;
        let policy = match policy {
            Ok((v, _)) => v,
            Err(ForgeError::Provider {
                reason: FailureReason::Forbidden | FailureReason::NotFound,
                ..
            }) => Value::Null,
            Err(e) => return Err(e),
        };
        let mut kept = Vec::<Value>::new();
        let mut indexes = BTreeMap::new();
        for status in statuses {
            let name = status["context"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("check")
                .to_string();
            if let Some(index) = indexes.get(&name).copied() {
                let old: &Value = &kept[index];
                let date = status["updated_at"].as_str().unwrap_or("");
                let old_date = old["updated_at"].as_str().unwrap_or("");
                let comparison = match (
                    chrono::DateTime::parse_from_rfc3339(date),
                    chrono::DateTime::parse_from_rfc3339(old_date),
                ) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    _ => date.cmp(old_date),
                };
                if comparison.is_lt()
                    || comparison.is_eq()
                        && status["id"]
                            .as_u64()
                            .zip(old["id"].as_u64())
                            .is_some_and(|(a, b)| a < b)
                {
                    continue;
                }
                kept[index] = status;
            } else {
                indexes.insert(name, kept.len());
                kept.push(status);
            }
        }
        let mut checks:Vec<_>=kept.into_iter().map(|raw|{let url=text(&raw,"target_url").and_then(|s|url::Url::parse(pr["html_url"].as_str().unwrap_or("")).ok()?.join(s).ok()).map(|u|u.to_string());json!({"name":text(&raw,"context").unwrap_or("check"),"description":text(&raw,"description"),"url":url,"status":match raw["status"].as_str(){Some("success")=>"success",Some("failure"|"error")=>"failure",Some("skipped")=>"skipped",Some("warning")=>"neutral",_=>"pending"}})}).collect();
        checks.sort_by_key(|c| c["status"] != "failure");
        let contexts: std::collections::BTreeSet<_> = if policy["enable_status_check"] == true {
            rows(&policy["status_check_contexts"])
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        } else {
            Default::default()
        };
        let unsupported: Vec<_> = contexts
            .iter()
            .filter(|p| p.contains(['[', ']', '{', '}', '\\']))
            .collect();
        if !unsupported.is_empty() {
            for name in unsupported {
                checks.push(json!({"name":name,"status":"pending","description":"Required check pattern cannot be evaluated.","url":null}));
            }
            return Ok(checks);
        }
        for check in &mut checks {
            if contexts
                .iter()
                .any(|p| glob(p, check["name"].as_str().unwrap_or("")))
            {
                check["required"] = json!(true);
            }
        }
        for pattern in contexts {
            if !pattern.trim().is_empty()
                && !checks
                    .iter()
                    .any(|c| glob(&pattern, c["name"].as_str().unwrap_or("")))
            {
                checks.push(json!({"name":pattern,"status":"pending","description":null,"url":null,"required":true}));
            }
        }
        Ok(checks)
    }
    pub(super) async fn detail(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        let repository_path = repo_path(&input.repo.repository);
        let (pr, repo, viewer) = tokio::try_join!(
            self.raw(&input),
            self.api(&input.repo, &repository_path, Method::GET, None),
            self.viewer(&input.repo)
        )?;
        let checks = self.checks(&input.repo, &pr).await?;
        let repo = repo.0;
        let mut value = d::listing(&pr)?;
        value["headSha"] = pr["head"]["sha"].clone();
        value["body"] = json!(pr["body"].as_str().unwrap_or(""));
        value["changedFiles"] = json!(pr["changed_files"].as_u64().unwrap_or(0));
        value["reviewers"] = json!(
            rows(&pr["requested_reviewers"])
                .map(d::actor)
                .filter(|a| !a.is_null())
                .collect::<Vec<_>>()
        );
        value["checks"] = json!(checks);
        value["baseComparison"] = json!(if text(&pr, "merge_base").is_none() {
            "unknown"
        } else if pr["merge_base"] == pr["base"]["sha"] {
            "up-to-date"
        } else {
            "behind"
        });
        value["viewerPermissions"] = d::permissions(&repo, &pr, &viewer);
        value["mergeCapabilities"] = json!({"merge":repo["allow_merge_commits"].as_bool().unwrap_or(true),"squash":repo["allow_squash_merge"].as_bool().unwrap_or(true),"rebase":repo["allow_rebase"].as_bool().unwrap_or(true),"fast-forward":repo["allow_fast_forward_only_merge"].as_bool().unwrap_or(false)});
        parse(value, "getChangeRequest")
    }
    pub(super) async fn contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        let pr = self.raw(&input.pr).await?;
        let commit = if let Some(commit) = &input.commit {
            let path = format!(
                "{}/git/commits/{}?files=false&verification=false",
                repo_path(&input.pr.repo.repository),
                encode(commit)
            );
            Some(self.api(&input.pr.repo, &path, Method::GET, None).await?.0)
        } else {
            None
        };
        let old = if let Some(commit) = &commit {
            text(&commit["parents"][0], "sha")
        } else {
            text(&pr, "merge_base").or_else(|| text(&pr["base"], "sha"))
        };
        let new = input
            .commit
            .as_deref()
            .or_else(|| text(&pr["head"], "sha"))
            .ok_or_else(|| error("getDiffFileContents", "Missing head revision."))?;
        let source = text(&pr["head"]["repo"], "full_name").unwrap_or(&input.pr.repo.repository);
        let old_read = async {
            if input.change_type == PullRequestDiffFileContentsInputChangeType::New || old.is_none()
            {
                Ok(String::new())
            } else {
                self.content(
                    &input.pr.repo,
                    &input.pr.repo.repository,
                    old.unwrap_or(""),
                    &input.old_path,
                )
                .await
            }
        };
        let new_read = async {
            if input.change_type == PullRequestDiffFileContentsInputChangeType::Deleted {
                Ok(String::new())
            } else {
                self.content(&input.pr.repo, source, new, &input.new_path)
                    .await
            }
        };
        let (old_contents, new_contents) = tokio::try_join!(old_read, new_read)?;
        Ok(ProviderDiffFileContents {
            old_contents,
            new_contents,
        })
    }
    async fn content(
        &self,
        repo: &ProviderRepositoryRef,
        repository: &str,
        revision: &str,
        path: &str,
    ) -> Result<String> {
        let mut input = repo.clone();
        input.repository = repository.into();
        let (raw, _) = self
            .api(
                &input,
                &format!(
                    "{}/contents/{}?ref={}",
                    repo_path(repository),
                    path.split('/').map(encode).collect::<Vec<_>>().join("/"),
                    encode(revision)
                ),
                Method::GET,
                None,
            )
            .await?;
        if raw["encoding"] != "base64" {
            return Err(error("getDiffFileContents", "Unsupported file encoding."));
        }
        let content = raw["content"]
            .as_str()
            .ok_or_else(|| error("getDiffFileContents", "Missing file contents."))?
            .replace(['\n', '\r', ' '], "");
        let bytes = STANDARD
            .decode(content)
            .map_err(|_| error("getDiffFileContents", "Invalid encoded file contents."))?;
        Ok(String::from_utf8_lossy(&bytes).into())
    }
    pub(super) async fn activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        let comments_path = format!("{}/comments", issue_path(&input));
        let reviews_path = format!("{}/reviews", pull_path(&input));
        let commits_path = format!(
            "{}/commits?files=false&verification=false",
            pull_path(&input)
        );
        let reactions_path = format!("{}/reactions", issue_path(&input));
        let (comments, reviews, commits, reactions, viewer) = tokio::try_join!(
            self.api(&input.repo, &comments_path, Method::GET, None),
            self.page(&input.repo, &reviews_path, 500),
            self.page(&input.repo, &commits_path, 500),
            self.page(&input.repo, &reactions_path, 500),
            self.viewer(&input.repo)
        )?;
        let all_comments = if comments.0.is_null() {
            vec![]
        } else {
            comments
                .0
                .as_array()
                .cloned()
                .ok_or_else(|| error("activity", "Invalid comment list."))?
        };
        let comments_truncated = all_comments.len() > 500 || reviews.1;
        let mut entries: Vec<(Value, Option<String>)> = all_comments
            .into_iter()
            .take(500)
            .map(|c| (d::comment(&c), Some(c["id"].to_string())))
            .collect();
        let mut inline = vec![];
        use futures_util::StreamExt;
        let review_paths: Vec<_> = reviews
            .0
            .iter()
            .filter(|r| r["comments_count"].as_u64().unwrap_or(0) > 0 && r["state"] != "PENDING")
            .map(|r| format!("{}/reviews/{}/comments", pull_path(&input), r["id"]))
            .collect();
        let inline_pages: Vec<_> =
            futures_util::stream::iter(review_paths.into_iter().map(|path| {
                let repo = &input.repo;
                async move {
                    let (raw, _) = self.api(repo, &path, Method::GET, None).await?;
                    if raw.is_null() {
                        Ok(vec![])
                    } else {
                        raw.as_array()
                            .cloned()
                            .ok_or_else(|| error("activity", "Invalid inline comment list."))
                    }
                }
            }))
            .buffered(4)
            .collect()
            .await;
        for page in inline_pages {
            inline.extend(page?);
        }
        let inline_truncated = inline.len() > 500;
        inline.truncate(500);
        for review in &reviews.0 {
            if !matches!(review["state"].as_str(), Some("PENDING" | "REQUEST_REVIEW")) {
                entries.push((d::review(review), review_comment_id(review)));
            }
        }
        for c in &inline {
            let mut value = d::comment(c);
            value["kind"] = json!("review-comment");
            value["path"] = c["path"].clone();
            entries.push((value, Some(c["id"].to_string())));
        }
        let enriched: Vec<_> =
            futures_util::stream::iter(entries.into_iter().map(|(mut comment, id)| {
                let viewer = &viewer;
                let repo = &input.repo;
                async move {
                    let reactions = if let Some(id) = id {
                        let path = format!(
                            "{}/issues/comments/{id}/reactions",
                            repo_path(&repo.repository)
                        );
                        let (raw, _) = self.api(repo, &path, Method::GET, None).await?;
                        if raw.is_null() {
                            vec![]
                        } else {
                            raw.as_array()
                                .cloned()
                                .ok_or_else(|| error("activity", "Invalid reaction list."))?
                        }
                    } else {
                        vec![]
                    };
                    comment["reactions"] = json!(d::reactions(&reactions, viewer));
                    Ok::<_, ForgeError>(comment)
                }
            }))
            .buffered(4)
            .collect()
            .await;
        let mut comments = enriched.into_iter().collect::<Result<Vec<_>>>()?;
        comments.sort_by_key(|c| text(c, "createdAt").map(str::to_string));
        let threads:Vec<_>=inline.iter().map(|raw|{let old=raw["position"]==0&&raw["original_position"].as_u64().is_some_and(|n|n>0);let line=raw[if old{"original_position"}else{"position"}].as_u64().filter(|n|*n>0);let id=raw["id"].to_string();let comment=comments.iter().find(|c|c["id"]==id).cloned().unwrap_or_else(||d::comment(raw));json!({"id":id,"path":raw["path"],"line":line,"side":if old{"left"}else{"right"},"isResolved":!raw["resolver"].is_null(),"isOutdated":false,"comments":[comment]})}).collect();
        parse(
            json!({"commentCount":comments.len(),"comments":comments,"commentsTruncated":comments_truncated||inline_truncated,"reviewThreads":threads,"commits":commits.0.iter().map(d::commit).collect::<Vec<_>>(),"reactions":d::reactions(&reactions.0,&viewer)}),
            "activity",
        )
    }
}
pub(super) fn review_comment_id(review: &Value) -> Option<String> {
    text(review, "html_url")?
        .rsplit_once("#issuecomment-")
        .map(|(_, id)| id)
        .filter(|id| {
            !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
                && id.parse::<u64>().is_ok_and(|n| n > 0)
        })
        .map(str::to_string)
}
fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<_> = pattern.chars().collect();
    let text: Vec<_> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    let mut resume = 0;
    while t < text.len() {
        if pattern.get(p) == Some(&'*') {
            star = Some(p);
            p += 1;
            resume = t;
        } else if pattern.get(p).is_some_and(|c| *c == '?' || *c == text[t]) {
            p += 1;
            t += 1;
        } else if let Some(star) = star {
            p = star + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    while pattern.get(p) == Some(&'*') {
        p += 1;
    }
    p == pattern.len()
}
