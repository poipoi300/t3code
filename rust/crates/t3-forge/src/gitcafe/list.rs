use super::{decode as d, *};
fn author_key(repo: &ProviderRepositoryRef, login: &str) -> String {
    format!("{}\n{}", repo.host.to_lowercase(), login.to_lowercase())
}
impl GitCafe {
    async fn learn_authors(&self, repo: &ProviderRepositoryRef, pulls: &[Value]) {
        let mut known = self.known_authors.lock().await;
        for pull in pulls {
            let author = &pull["author"];
            if author["kind"] == "github"
                && let (Some(login), Some(id)) = (text(author, "login"), text(author, "actorId"))
            {
                if known.len() >= 10_000 {
                    known.clear();
                }
                known.insert(author_key(repo, login), id.into());
            }
        }
    }
    async fn list_page(
        &self,
        input: &ListInput,
        params: &[(&str, String)],
        actor: Option<&str>,
        limit: usize,
        after: Option<&str>,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let mut args = vec![("limit", limit.to_string()), ("sort", "newest".into())];
        if input.state != PullRequestListState::All {
            args.push((
                "state",
                serde_json::to_value(&input.state)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or("open".into()),
            ));
        }
        if let Some(search) = input
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            args.push(("q", search.into()));
        }
        args.extend_from_slice(params);
        if let Some(actor) = actor {
            args.push((
                if input.involvement == PullRequestInvolvement::Reviewing {
                    "reviewers"
                } else {
                    "authors"
                },
                json!([actor]).to_string(),
            ));
        }
        if let Some(after) = after {
            args.push(("after", after.into()));
        }
        let raw = self
            .api(
                &input.repo.host,
                "listChangeRequests",
                Method::GET,
                &format!(
                    "{}/pulls?{}",
                    repository(&input.repo.repository)?,
                    query(&args)
                ),
                None,
            )
            .await?;
        let items = raw["items"]
            .as_array()
            .ok_or_else(|| error("list", "Invalid pull request page."))?
            .clone();
        for pull in &items {
            d::listing(pull, &input.repo)?;
        }
        self.learn_authors(&input.repo, &items).await;
        Ok((items, text(&raw, "next").map(str::to_string)))
    }
    pub(super) async fn list(&self, input: ListInput) -> Result<ProviderChangeRequestPage> {
        if !valid_host(&input.repo.host) {
            return Err(error("list", "Unsupported GitCafe host."));
        }
        let base = repository(&input.repo.repository)?;
        let mut actor_id = None;
        let empty = |truncated| ProviderChangeRequestPage {
            items: vec![],
            truncated,
            cursor_advance: None,
            resume_after: None,
            continues: false,
        };
        if input.involvement != PullRequestInvolvement::All {
            match self
                .api(
                    &input.repo.host,
                    "listPrincipal",
                    Method::GET,
                    "/auth/principal",
                    None,
                )
                .await
            {
                Ok(p) => {
                    if text(&p, "handle").is_some_and(|h| !h.eq_ignore_ascii_case(&input.viewer)) {
                        return Ok(empty(false));
                    }
                    actor_id = text(&p, "actorId").map(str::to_string);
                }
                Err(e)
                    if matches!(
                        e.reason(),
                        Some(FailureReason::Unauthenticated | FailureReason::RateLimited)
                    ) =>
                {
                    return Err(e);
                }
                Err(_) => {}
            }
        }
        let filters = input
            .filters
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| error("list", "Invalid filters."))?
            .unwrap_or(Value::Null);
        let mut params = vec![];
        if let Some(draft) = text(&filters, "draft") {
            params.push((
                if draft == "only" {
                    "states"
                } else {
                    "stateNots"
                },
                json!(["draft"]).to_string(),
            ));
        }
        if rows(&filters["labels"]).next().is_some()
            || rows(&filters["excludedLabels"]).next().is_some()
        {
            let (labels, _) = self
                .pages(&input.repo, "listFilterLabels", &format!("{base}/labels"))
                .await?;
            let ids: BTreeMap<_, _> = labels
                .iter()
                .filter_map(|l| Some((text(l, "name")?.to_lowercase(), text(l, "id")?.to_string())))
                .collect();
            let ids_of = |group: &Value| {
                rows(group)
                    .filter_map(|v| {
                        v.as_str()
                            .and_then(|s| ids.get(&s.trim().to_lowercase()))
                            .cloned()
                    })
                    .collect::<Vec<_>>()
            };
            if let Some(group) =
                rows(&filters["labels"]).find(|g| ids_of(g).len() == rows(g).count())
            {
                params.push(("labels", json!(ids_of(group)).to_string()));
            }
            let excluded = ids_of(&filters["excludedLabels"]);
            if !excluded.is_empty() {
                params.push(("labelNots", json!(excluded).to_string()));
            }
        }
        let mut unresolved = None;
        if !(actor_id.is_some() && input.involvement == PullRequestInvolvement::Authored)
            && let Some(author) = text(&filters, "author")
        {
            let author = resolve_author(author, &input.viewer);
            let result = self
                .api(
                    &input.repo.host,
                    "listFilterAuthor",
                    Method::GET,
                    &format!("/users/{}", encode(author)),
                    None,
                )
                .await;
            let found = match result {
                Ok(user) => text(&user, "actorId").map(str::to_string),
                Err(e)
                    if matches!(
                        e.reason(),
                        Some(FailureReason::Unauthenticated | FailureReason::RateLimited)
                    ) =>
                {
                    return Err(e);
                }
                Err(_) => self
                    .known_authors
                    .lock()
                    .await
                    .get(&author_key(&input.repo, author))
                    .cloned(),
            };
            if let Some(found) = found {
                params.push(("authors", json!([found]).to_string()));
            } else {
                unresolved = Some(author.to_string());
            }
        }
        if let Some(author) = unresolved {
            let mut after = None;
            let mut found = None;
            for _ in 0..5 {
                let (items, next) = self
                    .list_page(&input, &params, actor_id.as_deref(), 100, after.as_deref())
                    .await?;
                after = if items.is_empty() { None } else { next };
                found = self
                    .known_authors
                    .lock()
                    .await
                    .get(&author_key(&input.repo, &author))
                    .cloned();
                if found.is_some() || after.is_none() {
                    break;
                }
            }
            let Some(found) = found else {
                return Ok(empty(after.is_some()));
            };
            params.push(("authors", json!([found]).to_string()));
        }
        let resume = input
            .cursor
            .as_ref()
            .and_then(|c| c.resume_after.as_deref())
            .and_then(|s| s.rsplit_once('~'))
            .and_then(|(date, number)| {
                Some((
                    chrono::DateTime::parse_from_rfc3339(date)
                        .ok()?
                        .timestamp_millis(),
                    number.parse::<u64>().ok()?,
                ))
            });
        let limit = input.limit.max(1);
        let mut items = vec![];
        let mut after = None::<String>;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let (page, next) = self
                .list_page(
                    &input,
                    &params,
                    actor_id.as_deref(),
                    if resume.is_some() {
                        100
                    } else {
                        limit.saturating_sub(items.len()).clamp(1, 100)
                    },
                    after.as_deref(),
                )
                .await?;
            let empty = page.is_empty();
            for raw in page {
                let is_new = resume.is_none_or(|(date, number)| {
                    text(&raw, "createdAt")
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .is_some_and(|d| {
                            d.timestamp_millis() < date
                                || d.timestamp_millis() == date
                                    && raw["number"].as_u64().is_some_and(|n| n < number)
                        })
                });
                if is_new {
                    items.push(raw);
                }
            }
            after = if empty { None } else { next };
            if items.len() >= limit || after.is_none() {
                break;
            }
            if !seen.insert(after.clone()) {
                return Err(error("list", "GitCafe repeated a listing cursor."));
            }
        }
        let truncated = after.is_some() || items.len() > limit;
        items.truncate(limit);
        let resume_after = items.last().map(|last| {
            format!(
                "{}~{}",
                last["createdAt"].as_str().unwrap_or(""),
                last["number"]
            )
        });
        let items = items
            .into_iter()
            .map(|raw| {
                let mut value = d::listing(&raw, &input.repo)?;
                if input.involvement == PullRequestInvolvement::Reviewing && actor_id.is_some() {
                    let mut logins: Vec<String> = rows(&value["reviewRequestLogins"])
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect();
                    if !logins.contains(&input.viewer) {
                        logins.push(input.viewer.clone());
                    }
                    value["reviewRequestLogins"] = json!(logins);
                }
                parse(value, "listChangeRequests")
            })
            .collect::<Result<_>>()?;
        Ok(ProviderChangeRequestPage {
            items,
            truncated,
            cursor_advance: None,
            resume_after,
            continues: true,
        })
    }
}
