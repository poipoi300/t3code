use super::{decode as d, *};
impl AzureDevOps {
    pub(super) async fn iterations(
        &self,
        input: &PullRequestInput,
        location: &Location,
    ) -> Result<Vec<Iteration>> {
        let raw = self
            .invoke(
                input,
                location,
                "pullRequestIterations",
                vec![],
                None,
                8 * 1024 * 1024,
            )
            .await?;
        let values = raw["value"]
            .as_array()
            .ok_or_else(|| error("iterations", "Invalid iteration list."))?;
        let mut result: Vec<_> = values
            .iter()
            .filter_map(|r| {
                Some(Iteration {
                    id: r["id"].as_u64().filter(|n| *n > 0)?,
                    head: text(&r["sourceRefCommit"], "commitId")?.into(),
                    base: text(&r["commonRefCommit"], "commitId")?.into(),
                })
            })
            .collect();
        result.sort_by_key(|i| i.id);
        Ok(result)
    }
    pub(super) async fn scope(
        &self,
        input: &PullRequestInput,
    ) -> Result<Option<(Location, Vec<Iteration>)>> {
        let Some(location) = self.location(input).await? else {
            return Ok(None);
        };
        let iterations = self.iterations(input, &location).await?;
        Ok(Some((location, iterations)))
    }
    pub(super) async fn changes(
        &self,
        input: &PullRequestInput,
        location: &Location,
        id: u64,
    ) -> Result<(Vec<Change>, bool)> {
        let mut changes = vec![];
        let mut skip = 0;
        loop {
            let raw = self
                .invoke(
                    input,
                    location,
                    "pullRequestIterationChanges",
                    vec!["$top=2000".into(), format!("$skip={skip}")],
                    Some(id),
                    8 * 1024 * 1024,
                )
                .await?;
            let (page, next) = d::changes(&raw)?;
            changes.extend(page);
            match next {
                None => return Ok((changes, false)),
                Some(next) if next <= skip || next >= 10_000 => return Ok((changes, true)),
                Some(next) => skip = next,
            }
        }
    }
    async fn item(
        &self,
        input: &PullRequestInput,
        location: &Location,
        path: &str,
        commit: &str,
    ) -> Result<(String, bool)> {
        let _permit = self
            .spawns
            .acquire()
            .await
            .map_err(|_| error("items", "The item reader is closed."))?;
        let raw = self
            .invoke(
                input,
                location,
                "items",
                vec![
                    format!(
                        "path={}",
                        encode(&format!("/{}", path.trim_start_matches('/')))
                    ),
                    "versionDescriptor.versionType=commit".into(),
                    format!("versionDescriptor.version={commit}"),
                    "includeContent=true".into(),
                    "includeContentMetadata=true".into(),
                    "$format=json".into(),
                ],
                None,
                4 * 1024 * 1024,
            )
            .await?;
        if !raw.is_object() {
            return Err(error("items", "Invalid item content response."));
        }
        Ok((
            raw["content"].as_str().unwrap_or("").into(),
            raw["contentMetadata"]["isBinary"] == true,
        ))
    }
    async fn texts(
        &self,
        input: &PullRequestInput,
        location: &Location,
        iteration: &Iteration,
        change: &Change,
    ) -> Result<(String, String, bool)> {
        let old = async {
            if change.kind == "new" {
                Ok((String::new(), false))
            } else {
                self.item(input, location, &change.old, &iteration.base)
                    .await
            }
        };
        let new = async {
            if change.kind == "deleted" {
                Ok((String::new(), false))
            } else {
                self.item(input, location, &change.path, &iteration.head)
                    .await
            }
        };
        let ((old, old_binary), (new, new_binary)) = tokio::try_join!(old, new)?;
        Ok((old, new, old_binary || new_binary))
    }
    pub(super) async fn detail(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestDetail> {
        let raw = self.raw(&input).await?;
        let location = d::location(&raw);
        let iterations = if let Some(location) = &location {
            self.iterations(&input, location).await.unwrap_or_default()
        } else {
            vec![]
        };
        let mut changed = 0;
        if let (Some(location), Some(latest)) = (&location, iterations.last()) {
            changed = self
                .changes(&input, location, latest.id)
                .await
                .map(|(changes, _)| changes.len())
                .unwrap_or(0);
        }
        let mut v = d::listing(&raw)?;
        v["body"] = json!(raw["description"].as_str().unwrap_or(""));
        v["changedFiles"] = json!(changed);
        v["closedAt"] = json!(if d::state(&raw) == "closed" {
            text(&raw, "closedDate")
        } else {
            None
        });
        v["mergedAt"] = json!(if d::state(&raw) == "merged" {
            text(&raw, "closedDate")
        } else {
            None
        });
        v["reviewers"] = json!(
            rows(&raw["reviewers"])
                .map(d::actor)
                .filter(|a| !a.is_null())
                .collect::<Vec<_>>()
        );
        v["checks"] = json!([]);
        v["mergeCapabilities"] = json!({"merge":true,"squash":true,"rebase":false});
        v["viewerPermissions"] = permissions();
        if let Some(latest) = iterations.last() {
            v["headSha"] = json!(latest.head);
        }
        d::auto_merge(&raw, &mut v);
        parse(v, "getChangeRequest")
    }
    pub(super) async fn activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        let raw = self.raw(&input).await?;
        let mut comments = vec![];
        let mut truncated = true;
        if let Some(location) = d::location(&raw)
            && let Ok(raw) = self
                .invoke(
                    &input,
                    &location,
                    "pullRequestThreads",
                    vec![],
                    None,
                    8 * 1024 * 1024,
                )
                .await
            && let Some(threads) = raw["value"].as_array()
        {
            truncated = false;
            for thread in threads {
                if thread["isDeleted"] == true {
                    continue;
                }
                let path = thread["threadContext"]["filePath"]
                    .as_str()
                    .map(|s| s.trim_start_matches('/'))
                    .filter(|s| !s.is_empty());
                for note in rows(&thread["comments"]) {
                    if note["isDeleted"] == true
                        || text(note, "commentType")
                            .is_some_and(|s| s.eq_ignore_ascii_case("system"))
                        || text(note, "content").is_none()
                    {
                        continue;
                    }
                    let Some(date) = text(note, "publishedDate") else {
                        continue;
                    };
                    comments.push(json!({"id":format!("{}:{}",thread["id"],note["id"].as_u64().unwrap_or(0)),"kind":if path.is_some(){"review-comment"}else{"issue-comment"},"author":d::actor(&note["author"]),"body":note["content"],"createdAt":date,"url":null,"path":path,"reviewState":null}));
                }
            }
        }
        comments.sort_by_key(|c| text(c, "createdAt").map(str::to_string));
        parse(
            json!({"comments":comments,"commentCount":comments.len(),"commentsTruncated":truncated,"reviewThreads":[],"commits":[]}),
            "getActivity",
        )
    }
    pub(super) async fn list(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        let mut skip = input.cursor.as_ref().map_or(0, |c| c.delivered);
        let mut advance = 0;
        let mut items = vec![];
        let filters = input
            .filters
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| error("list", "Invalid filters."))?
            .unwrap_or(Value::Null);
        let mut truncated = false;
        for _ in 0..10_000 {
            let remaining = input.limit.saturating_sub(items.len());
            let top = remaining.saturating_add(1);
            let mut args = vec![
                "repos".into(),
                "pr".into(),
                "list".into(),
                "--detect=true".into(),
                format!("--repository={}", value(&input.repo.repository)?),
                format!(
                    "--status={}",
                    match input.state {
                        PullRequestListState::Open => "active",
                        PullRequestListState::Closed => "abandoned",
                        PullRequestListState::Merged => "completed",
                        PullRequestListState::All => "all",
                    }
                ),
                "--include-links".into(),
                format!("--top={top}"),
            ];
            match input.involvement {
                PullRequestInvolvement::Authored => {
                    args.push(format!("--creator={}", value(&input.viewer)?))
                }
                PullRequestInvolvement::Reviewing => {
                    args.push(format!("--reviewer={}", value(&input.viewer)?))
                }
                _ => {}
            }
            if input.involvement != PullRequestInvolvement::Authored
                && let Some(author) = text(&filters, "author")
            {
                args.push(format!(
                    "--creator={}",
                    value(resolve_author(author, &input.viewer))?
                ));
            }
            if skip > 0 {
                args.push(format!("--skip={skip}"));
            }
            let raw = self
                .json(
                    Path::new(&input.repo.cwd),
                    "listPullRequests",
                    args,
                    16 * 1024 * 1024,
                )
                .await?;
            if raw.is_null() {
                truncated = false;
                break;
            }
            let raw = raw
                .as_array()
                .ok_or_else(|| error("list", "Invalid pull request list."))?;
            let mut consumed = 0;
            for row in raw {
                if items.len() >= input.limit {
                    break;
                }
                consumed += 1;
                if let Ok(value) = d::listing(row) {
                    items.push(value);
                }
            }
            advance += consumed as u64;
            skip = skip.saturating_add(consumed as u64);
            truncated = consumed < raw.len() || raw.len() == top;
            if items.len() >= input.limit || raw.len() < top {
                break;
            }
        }
        parse(
            json!({"items":items,"truncated":truncated,"cursorAdvance":advance,"continues":true}),
            "listChangeRequests",
        )
    }
    pub(super) async fn diff(&self, input: DiffInput) -> Result<ProviderDiffSlice> {
        let empty = || ProviderDiffSlice {
            patch: String::new(),
            truncated: false,
            next_cursor: None,
            omitted_file_stats: None,
        };
        let Some((location, iterations)) = self.scope(&input.pr).await? else {
            return Ok(empty());
        };
        let cursor = input.cursor.as_deref().and_then(parse_cursor);
        let iteration = if let Some((id, _)) = cursor {
            iterations.iter().find(|i| i.id == id)
        } else {
            iterations.last()
        };
        let Some(iteration) = iteration else {
            return Ok(empty());
        };
        let (changes, mut truncated) = self.changes(&input.pr, &location, iteration.id).await?;
        let mut index = cursor.map_or(0, |(_, index)| index);
        let mut patch = String::new();
        let mut edits = 0;
        let mut files = 0;
        while index < changes.len() {
            let batch = &changes[index..(index + 4).min(changes.len())];
            let results = futures_util::future::join_all(
                batch
                    .iter()
                    .map(|change| self.texts(&input.pr, &location, iteration, change)),
            )
            .await;
            let mut full = false;
            for (change, texts) in batch.iter().zip(results) {
                let file = match texts {
                    Ok((old, new, binary)) => {
                        let change = change.clone();
                        tokio::task::spawn_blocking(move || patch::file(change, old, new, binary))
                            .await
                            .map_err(|_| error("getDiff", "The text diff worker failed."))?
                    }
                    Err(e)
                        if matches!(
                            e.reason(),
                            Some(
                                FailureReason::MissingTool
                                    | FailureReason::Unauthenticated
                                    | FailureReason::RateLimited
                            )
                        ) =>
                    {
                        return Err(e);
                    }
                    Err(_) => patch::unreadable(change),
                };
                patch.push_str(&file.section);
                truncated |= file.truncated;
                edits += file.edits;
                files += 1;
                index += 1;
                if patch.len() >= 256 * 1024
                    || edits + 2000 > 6000
                    || files >= 300
                    || file.abandoned
                {
                    full = true;
                    break;
                }
            }
            if full {
                break;
            }
        }
        Ok(ProviderDiffSlice {
            patch,
            truncated,
            next_cursor: (index < changes.len()).then(|| format!("{}:{index}", iteration.id)),
            omitted_file_stats: None,
        })
    }
    pub(super) async fn contents(
        &self,
        input: FileContentsInput,
    ) -> Result<ProviderDiffFileContents> {
        let Some((location, iterations)) = self.scope(&input.pr).await? else {
            return Ok(ProviderDiffFileContents {
                old_contents: String::new(),
                new_contents: String::new(),
            });
        };
        let Some(iteration) = iterations.last() else {
            return Ok(ProviderDiffFileContents {
                old_contents: String::new(),
                new_contents: String::new(),
            });
        };
        let change = Change {
            path: input.new_path,
            old: input.old_path,
            kind: match input.change_type {
                PullRequestDiffFileContentsInputChangeType::New => "new",
                PullRequestDiffFileContentsInputChangeType::Deleted => "deleted",
                _ => "change",
            },
            oid: None,
        };
        let (old_contents, new_contents, _) =
            self.texts(&input.pr, &location, iteration, &change).await?;
        Ok(ProviderDiffFileContents {
            old_contents,
            new_contents,
        })
    }
    pub(super) async fn revisions(
        &self,
        input: FileRevisionsInput,
    ) -> Result<ProviderFileRevisions> {
        let mut revisions = BTreeMap::new();
        if input.paths.is_empty() {
            return Ok(ProviderFileRevisions {
                revisions,
                complete: None,
            });
        }
        let Some((location, iterations)) = self.scope(&input.pr).await? else {
            return Ok(ProviderFileRevisions {
                revisions,
                complete: None,
            });
        };
        let (changes, truncated) = if let Some(iteration) = iterations.last() {
            self.changes(&input.pr, &location, iteration.id).await?
        } else {
            (vec![], false)
        };
        for change in changes {
            if input.paths.contains(&change.path)
                && let Some(oid) = change.oid
            {
                revisions.insert(change.path, oid);
            }
        }
        if !truncated {
            for path in input.paths {
                revisions.entry(path).or_default();
            }
        }
        Ok(ProviderFileRevisions {
            revisions,
            complete: None,
        })
    }
}
fn parse_cursor(raw: &str) -> Option<(u64, usize)> {
    let (id, index) = raw.split_once(':')?;
    if id.is_empty()
        || index.is_empty()
        || !id.bytes().chain(index.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let id = id
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)?;
    let index = index
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= 9_007_199_254_740_991)?;
    Some((id, usize::try_from(index).ok()?))
}
