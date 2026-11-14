use super::{decode as d, *};
#[derive(Clone)]
pub(super) struct Revision {
    pub version: u64,
    pub head: String,
    pub base: String,
}
impl GitCafe {
    pub(super) async fn stack(&self, input: &PullRequestInput) -> Result<Option<Value>> {
        let raw = self
            .api(
                &input.repo.host,
                "getStack",
                Method::GET,
                &format!("{}/stack", pull_path(input)?),
                None,
            )
            .await?;
        let mut stack = raw["stack"].clone();
        if stack.is_null() {
            return Ok(None);
        }
        if stack["revision"].as_u64().is_none_or(|n| n == 0)
            || stack["number"].as_u64().is_none_or(|n| n == 0)
        {
            return Err(error("stack", "Invalid stack revision or number."));
        }
        let members = stack["members"]
            .as_array_mut()
            .ok_or_else(|| error("stack", "Invalid stack members."))?;
        use futures_util::StreamExt;
        let resolved: Vec<_> =
            futures_util::stream::iter(members.iter().cloned().map(|mut member| async move {
                if member["state"] != "merged" {
                    let number = member["pullRequestNumber"]
                        .as_u64()
                        .ok_or_else(|| error("stack", "Invalid layer number."))?;
                    let raw = self
                        .raw(
                            &PullRequestInput {
                                repo: input.repo.clone(),
                                number,
                            },
                            "getStackLayer",
                        )
                        .await?;
                    if !raw["headOid"].is_null() {
                        member["headOid"] = raw["headOid"].clone();
                    }
                }
                Ok(member)
            }))
            .buffered(4)
            .collect()
            .await;
        *members = resolved.into_iter().collect::<Result<_>>()?;
        Ok(Some(stack))
    }
    pub(super) async fn diff_page(
        &self,
        input: &DiffInput,
        limit: usize,
    ) -> Result<(Vec<Value>, Revision, Option<String>)> {
        if input.commit.is_some() {
            return Err(error("getDiff", "GitCafe commit diffs are not supported."));
        }
        let cursor = input
            .cursor
            .as_ref()
            .map(|s| {
                serde_json::from_str::<Value>(s)
                    .map_err(|_| error("getDiff", "Invalid diff cursor."))
            })
            .transpose()?;
        let expected = if let Some(cursor) = &cursor {
            if cursor["version"].as_u64().is_none()
                || text(cursor, "headOid").is_none()
                || text(cursor, "baseOid").is_none()
                || text(cursor, "after").is_none()
            {
                return Err(error("getDiff", "Invalid diff cursor."));
            }
            cursor.clone()
        } else {
            self.raw(&input.pr, "getDiff").await?
        };
        let version = expected["version"]
            .as_u64()
            .ok_or_else(|| error("getDiff", "Missing pull request version."))?;
        let mut params = vec![
            ("expectedVersion", version.to_string()),
            ("limit", limit.to_string()),
        ];
        if let Some(cursor) = &cursor {
            params.push(("after", cursor["after"].as_str().unwrap_or("").into()));
        }
        let page = self
            .api(
                &input.pr.repo.host,
                "getDiff",
                Method::GET,
                &format!("{}/changes?{}", pull_path(&input.pr)?, query(&params)),
                None,
            )
            .await?;
        let files = page["items"]
            .as_array()
            .ok_or_else(|| error("getDiff", "Invalid changed-path page."))?
            .clone();
        if page["version"] != version
            || !expected["headOid"].is_null() && page["headOid"] != expected["headOid"]
            || cursor.as_ref().is_some_and(|c| {
                page["comparisonBaseOid"] != c["baseOid"]
                    || !page["next"].is_null() && page["next"] == c["after"]
            })
        {
            return Err(error(
                "getDiff",
                "The pull request revision changed. Refresh the diff.",
            ));
        }
        let revision = Revision {
            version,
            head: text(&page, "headOid")
                .ok_or_else(|| error("getDiff", "Missing head revision."))?
                .into(),
            base: text(&page, "comparisonBaseOid")
                .ok_or_else(|| error("getDiff", "Missing comparison base."))?
                .into(),
        };
        let next=text(&page,"next").map(|after|json!({"version":version,"headOid":revision.head,"baseOid":revision.base,"after":after}).to_string());
        Ok((files, revision, next))
    }
    pub(super) async fn diff_files(
        &self,
        input: &PullRequestInput,
        revision: &Revision,
        paths: Vec<String>,
    ) -> Result<Vec<Value>> {
        if paths.is_empty() {
            return Ok(vec![]);
        }
        let raw = self
            .api(
                &input.repo.host,
                "getDiff",
                Method::POST,
                &format!("{}/diff-files", pull_path(input)?),
                Some(json!({"paths":paths,"expectedVersion":revision.version})),
            )
            .await?;
        if raw["version"] != revision.version
            || raw["headOid"] != revision.head
            || raw["comparisonBaseOid"] != revision.base
        {
            return Err(error(
                "getDiff",
                "The pull request revision changed. Refresh the diff.",
            ));
        }
        raw["items"]
            .as_array()
            .cloned()
            .ok_or_else(|| error("getDiff", "Invalid detailed file list."))
    }
    pub(super) async fn line_stats(&self, input: &PullRequestInput, pull: &Value) -> Result<Value> {
        let tips = text(pull, "headOid")
            .zip(text(pull, "observedBaseOid"))
            .map(|(head, base)| {
                format!(
                    "{} {} {} {} {} {head} {base}",
                    self.credentials.revision(),
                    input.repo.host,
                    input.repo.repository.to_lowercase(),
                    input.number,
                    pull["targetBranch"]
                )
            });
        if let Some(key) = &tips
            && let Some(stats) = self.stats.lock().await.get(key).cloned()
        {
            return Ok(stats);
        }
        let version = pull["version"]
            .as_u64()
            .ok_or_else(|| error("stats", "Missing version."))?;
        let mut files = vec![];
        let mut revision = None::<Revision>;
        let mut after = None::<String>;
        for _ in 0..20 {
            let mut params = vec![
                ("expectedVersion", version.to_string()),
                ("limit", "500".into()),
            ];
            if let Some(after) = &after {
                params.push(("after", after.clone()));
            }
            let raw = self
                .api(
                    &input.repo.host,
                    "getLineStats",
                    Method::GET,
                    &format!("{}/changes?{}", pull_path(input)?, query(&params)),
                    None,
                )
                .await?;
            let items = raw["items"]
                .as_array()
                .ok_or_else(|| error("stats", "Invalid changed-file list."))?;
            if raw["version"] != version
                || revision
                    .as_ref()
                    .is_some_and(|r| raw["headOid"] != r.head || raw["comparisonBaseOid"] != r.base)
            {
                return Err(error(
                    "stats",
                    "The pull request revision changed while counting lines.",
                ));
            }
            revision.get_or_insert(Revision {
                version,
                head: text(&raw, "headOid").unwrap_or("").into(),
                base: text(&raw, "comparisonBaseOid").unwrap_or("").into(),
            });
            files.extend(items.iter().cloned());
            let next = text(&raw, "next").map(str::to_string);
            let stop = next.is_none() || next == after || items.is_empty();
            after = next;
            if stop {
                break;
            }
        }
        let Some(revision) = revision else {
            return Ok(json!({"changedFiles":0,"additions":0,"deletions":0}));
        };
        let key = format!(
            "{} {} {} {} {} {}",
            self.credentials.revision(),
            input.repo.host,
            input.repo.repository.to_lowercase(),
            input.number,
            revision.base,
            revision.head
        );
        let held = self.line_totals.lock().await.get(&key).cloned();
        let mut result = json!({"changedFiles":files.len()});
        let mut cacheable = false;
        if let Some(held) = held {
            result["additions"] = held["additions"].clone();
            result["deletions"] = held["deletions"].clone();
            cacheable = after.is_none();
        } else {
            let details = if files.iter().any(|f| f["additions"].is_number()) {
                Some(files.clone())
            } else if files.len() > 256 {
                cacheable = after.is_none();
                None
            } else {
                let paths: Vec<_> = files
                    .iter()
                    .filter(|f| f["binary"] != true && f["tooLarge"] != true)
                    .filter_map(|f| text(f, "path").map(str::to_string))
                    .collect();
                let batches = futures_util::future::join_all(
                    paths
                        .chunks(64)
                        .map(|paths| self.diff_files(input, &revision, paths.to_vec())),
                )
                .await;
                let collected: Result<Vec<Vec<Value>>> = batches.into_iter().collect();
                collected.ok().map(|v| v.into_iter().flatten().collect())
            };
            if let Some(details) = details {
                let additions: u64 = details
                    .iter()
                    .map(|f| f["additions"].as_u64().unwrap_or(0))
                    .sum();
                let deletions: u64 = details
                    .iter()
                    .map(|f| f["deletions"].as_u64().unwrap_or(0))
                    .sum();
                result["additions"] = json!(additions);
                result["deletions"] = json!(deletions);
                cacheable = after.is_none();
                if cacheable {
                    Self::remember(
                        &self.line_totals,
                        key,
                        json!({"additions":additions,"deletions":deletions}),
                    )
                    .await;
                }
            }
        }
        if cacheable
            && revision.head == pull["headOid"]
            && let Some(tips) = tips
        {
            Self::remember(&self.stats, tips, result.clone()).await;
        }
        Ok(result)
    }
    pub(super) async fn detail(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        let raw = self.raw(&input, "getChangeRequest").await?;
        let path = pull_path(&input)?;
        let base = repository(&input.repo.repository)?;
        let status_path = format!("{path}/status");
        let reviewers_path = format!("{path}/reviewers");
        let labels_path = format!("{path}/labels");
        let checks_path = format!(
            "{base}/commits/{}/checks",
            encode(text(&raw, "headOid").unwrap_or(""))
        );
        let checks_read = async {
            if raw["headOid"].is_null() {
                Ok((vec![], None))
            } else {
                self.pages(&input.repo, "getChecks", &checks_path).await
            }
        };
        let stats_read = async {
            if raw["headOid"].is_null() {
                Ok(Value::Null)
            } else {
                self.line_stats(&input, &raw).await
            }
        };
        let (stats, status, checks, reviewers, labels) = tokio::join!(
            stats_read,
            self.api(
                &input.repo.host,
                "getStatus",
                Method::GET,
                &status_path,
                None
            ),
            checks_read,
            self.pages(&input.repo, "getReviewers", &reviewers_path),
            self.pages(&input.repo, "getLabels", &labels_path)
        );
        let stats = if raw["headOid"].is_null() {
            Value::Null
        } else {
            stats.unwrap_or(Value::Null)
        };
        let status = status?;
        let (checks, next) = checks?;
        let reviewers = match reviewers {
            Ok((v, _)) => v,
            Err(ForgeError::Provider {
                status: Some(403), ..
            }) => vec![],
            Err(e) => return Err(e),
        };
        let (labels, _) = labels?;
        let mut listed = raw.clone();
        listed["reviewers"] = json!(reviewers);
        listed["labels"] = json!(labels);
        let mut value = d::listing(&listed, &input.repo)?;
        let checks = d::checks(&checks);
        let state = if !raw["headOid"].is_null() && next.is_none() {
            d::checks_state(&checks)
        } else if status["checks"]["total"] == 0 {
            None
        } else if status["checks"]["failing"].as_u64().unwrap_or(0) > 0 {
            Some("failing")
        } else if status["checks"]["pending"].as_u64().unwrap_or(0) > 0 {
            Some("pending")
        } else {
            Some("passing")
        };
        let merging = rows(&status["merge"]["reasons"]).any(|r| {
            matches!(
                r.as_str(),
                Some("provider-merge-pending" | "native-operation-pending")
            )
        });
        let mut permissions = d::permissions(&raw);
        if status["merge"]["permitted"] == false || merging {
            permissions["actions"] = json!(
                rows(&permissions["actions"])
                    .filter(|a| **a != "merge")
                    .cloned()
                    .collect::<Vec<_>>()
            );
        }
        value["autoMergeEnabled"] = json!(merging);
        value["headSha"] = raw["headOid"].clone();
        value["body"] = json!(raw["description"].as_str().unwrap_or(""));
        if raw["sourceRepo"].is_object() {
            value["headRepositoryNameWithOwner"] = json!(format!(
                "{}/{}",
                raw["sourceRepo"]["owner"].as_str().unwrap_or(""),
                raw["sourceRepo"]["name"].as_str().unwrap_or("")
            ));
        }
        for (field, fallback) in [("additions", 0), ("deletions", 0), ("changedFiles", 0)] {
            value[field] = json!(stats[field].as_u64().unwrap_or(fallback));
        }
        value["closedAt"] = raw["closedAt"].clone();
        value["mergedAt"] = raw["mergedAt"].clone();
        d::settled(&raw, &mut value);
        value["reviewers"] = json!(
            rows(&listed["reviewers"])
                .map(|r| d::actor(&r["actor"], &input.repo.host))
                .filter(|a| !a.is_null())
                .collect::<Vec<_>>()
        );
        value["checks"] = json!(checks);
        if let Some(state) = state {
            value["checksState"] = json!(state);
        }
        value["mergeability"] = json!(match status["merge"]["conflicts"].as_str() {
            Some("conflicting") => "conflicting",
            Some("none") => "mergeable",
            _ => "unknown",
        });
        value["baseComparison"] = json!(match status["merge"]["fastForward"].as_bool() {
            Some(true) => "up-to-date",
            Some(false) => "behind",
            None => "unknown",
        });
        let mut capabilities = json!({});
        for strategy in ["merge", "squash", "rebase"] {
            capabilities[strategy] = json!(
                status["merge"]["permitted"] != false
                    && rows(&status["merge"]["strategies"]).any(|s| s == strategy)
                    && !rows(&status["merge"]["blockers"])
                        .any(|b| rows(&b["blockedStrategies"]).any(|s| s == strategy))
            );
        }
        value["mergeCapabilities"] = capabilities;
        value["viewerPermissions"] = permissions;
        parse(value, "getChangeRequest")
    }
    pub(super) async fn contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        if input.commit.is_some() {
            return Err(error(
                "getDiffFileContents",
                "GitCafe commit file contents are not supported.",
            ));
        }
        let pull = self.raw(&input.pr, "getDiffFileContents").await?;
        let head = text(&pull, "headOid")
            .filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| error("getDiffFileContents", "Missing immutable head revision."))?;
        let snapshot = self
            .api(
                &input.pr.repo.host,
                "getDiffFileContents",
                Method::GET,
                &format!(
                    "{}/changes?expectedVersion={}&limit=1",
                    pull_path(&input.pr)?,
                    pull["version"]
                ),
                None,
            )
            .await?;
        if snapshot["version"] != pull["version"] || snapshot["headOid"] != head {
            return Err(error(
                "getDiffFileContents",
                "The pull request revision changed.",
            ));
        }
        let base = text(&snapshot, "comparisonBaseOid")
            .ok_or_else(|| error("getDiffFileContents", "Missing comparison base."))?;
        let source = if pull["sourceRepo"].is_object() {
            format!(
                "{}/{}",
                pull["sourceRepo"]["owner"].as_str().unwrap_or(""),
                pull["sourceRepo"]["name"].as_str().unwrap_or("")
            )
        } else {
            input.pr.repo.repository.clone()
        };
        let old = async {
            if input.change_type == PullRequestDiffFileContentsInputChangeType::New {
                Ok(String::new())
            } else {
                self.blob(&input.pr.repo, base, &input.old_path).await
            }
        };
        let new = async {
            if input.change_type == PullRequestDiffFileContentsInputChangeType::Deleted {
                Ok(String::new())
            } else {
                let mut repo = input.pr.repo.clone();
                repo.repository = source;
                self.blob(&repo, head, &input.new_path).await
            }
        };
        let (old_contents, new_contents) = tokio::try_join!(old, new)?;
        Ok(ProviderDiffFileContents {
            old_contents,
            new_contents,
        })
    }
    async fn blob(
        &self,
        repo: &ProviderRepositoryRef,
        revision: &str,
        path: &str,
    ) -> Result<String> {
        let raw = self
            .api(
                &repo.host,
                "getDiffFileContents",
                Method::GET,
                &format!(
                    "{}/blob?{}",
                    repository(&repo.repository)?,
                    query(&[("oid", revision.into()), ("path", path.into())])
                ),
                None,
            )
            .await?;
        if raw["commit"] != revision || raw["path"] != path {
            return Err(error(
                "getDiffFileContents",
                "The source revision or path changed.",
            ));
        }
        let content = raw["content"]
            .as_str()
            .ok_or_else(|| error("getDiffFileContents", "The source file is unavailable."))?;
        if raw["size"].as_u64().is_none_or(|size| size > 1024 * 1024)
            || content.len() > 1024 * 1024
            || raw["binary"] == true
            || content.contains('\0')
        {
            return Err(error(
                "getDiffFileContents",
                "The source file is binary or exceeds the byte limit.",
            ));
        }
        Ok(content.into())
    }
}
