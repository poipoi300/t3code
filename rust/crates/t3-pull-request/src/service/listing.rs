use super::*;
use futures_util::{StreamExt, stream};
use std::collections::HashSet;
type BatchResult = (
    Context,
    String,
    Result<(Vec<PullRequestListEntry>, bool, Option<String>)>,
);

pub fn parse_cursor(raw: &str) -> Result<ListCursor> {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let regex=PATTERN.get_or_init(||regex::Regex::new(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2}))\|(\d{1,9})\|(\d{1,9}(?:,\d{1,9})*)?(?:\|([\w.:~+-]{1,128}))?$").expect("constant cursor pattern"));
    let captures = regex
        .captures(raw)
        .ok_or_else(|| Error::invalid("list", "Invalid pull request list cursor."))?;
    Ok(ListCursor {
        updated_before: captures[1].into(),
        delivered: captures[2]
            .parse()
            .map_err(|_| Error::invalid("list", "Invalid cursor offset."))?,
        seen_at: captures.get(3).map_or_else(Vec::new, |s| {
            s.as_str()
                .split(',')
                .filter_map(|v| v.parse().ok())
                .collect()
        }),
        resume_after: captures.get(4).map(|s| s.as_str().into()),
    })
}
fn cursor_after(
    previous: Option<&ListCursor>,
    page: &ListPage,
    delivered: usize,
) -> Option<String> {
    let boundary = page.entries.iter().map(|e| e.updated_at.0.as_str()).min()?;
    let mut seen = previous
        .filter(|p| p.updated_before == boundary)
        .map_or_else(Vec::new, |p| p.seen_at.clone());
    for entry in &page.entries {
        if entry.updated_at.0 == boundary && !seen.contains(&entry.number) {
            seen.push(entry.number);
        }
    }
    let base = format!(
        "{boundary}|{}|{}",
        previous
            .map_or(0, |p| p.delivered)
            .saturating_add(page.cursor_advance.unwrap_or(delivered as u64)),
        seen.iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    Some(match &page.resume_after {
        Some(resume) => format!("{base}|{resume}"),
        None => base,
    })
}
fn matches_filters(
    entry: &PullRequestListEntry,
    filters: Option<&PullRequestListFilters>,
    viewer: &str,
) -> bool {
    let Some(filters) = filters else {
        return true;
    };
    let labels: HashSet<_> = entry
        .labels
        .iter()
        .map(|l| l.name.as_str().trim().to_lowercase())
        .collect();
    let holds = |s: &t3_contracts::base_schemas::TrimmedNonEmptyString| {
        labels.contains(&s.as_str().trim().to_lowercase())
    };
    if let Some(draft) = filters.draft.as_ref().and_then(Option::as_ref)
        && entry.is_draft != (*draft == PullRequestListFiltersDraft::Only)
    {
        return false;
    }
    if let Some(review) = filters.review.as_ref().and_then(Option::as_ref)
        && let Some(decision) = &entry.review_decision
    {
        let actual = decision
            .as_ref()
            .and_then(|v| serde_json::to_value(v).ok())
            .unwrap_or(json!("none"));
        if serde_json::to_value(review).ok() != Some(actual) {
            return false;
        }
    }
    if let Some(checks) = filters.checks.as_ref().and_then(Option::as_ref)
        && let Some(actual) = &entry.checks_state
        && serde_json::to_value(checks).ok() != serde_json::to_value(actual).ok()
    {
        return false;
    }
    if let Some(groups) = filters.labels.as_ref().and_then(Option::as_ref)
        && !groups.iter().all(|g| g.iter().any(holds))
    {
        return false;
    }
    if let Some(excluded) = filters.excluded_labels.as_ref().and_then(Option::as_ref)
        && excluded.iter().any(holds)
    {
        return false;
    }
    if let Some(author) = filters.author.as_ref().and_then(Option::as_ref) {
        let author = if author.as_str() == "@me" {
            viewer
        } else {
            author.as_str()
        };
        if !entry
            .author
            .as_ref()
            .is_some_and(|a| a.login.as_str().eq_ignore_ascii_case(author))
        {
            return false;
        }
    }
    true
}
impl PullRequestService {
    async fn selected(
        &self,
        project_id: Option<&ProjectId>,
        project_ids: Option<&Vec<ProjectId>>,
        host: Option<&str>,
    ) -> Result<Vec<crate::context::Binding>> {
        Ok(self
            .current_projects()
            .await?
            .into_iter()
            .filter(|p| {
                project_id.is_none_or(|id| *id == p.project.id)
                    && project_ids.is_none_or(|ids| ids.contains(&p.project.id))
                    && host.is_none_or(|h| p.project.host.eq_ignore_ascii_case(h))
            })
            .collect())
    }
    fn binding_context(binding: crate::context::Binding) -> Result<Context> {
        let reference = crate::convert(
            json!({"projectId":binding.project.id,"host":binding.project.host,"repository":binding.project.repository,"number":1}),
        )?;
        Ok(Context {
            project: binding.project,
            host: binding.host,
            reference,
            credential_namespace: String::new(),
            viewer: None,
            activity: None,
        })
    }
    async fn listing_repository(
        &self,
        ctx: Context,
        input: PullRequestListInput,
        viewer: String,
        cursor: Option<ListCursor>,
        limit: usize,
    ) -> Result<(Vec<PullRequestListEntry>, bool, Option<String>)> {
        let mut cursor = cursor;
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut next_cursor = None;
        for pass in 0..5 {
            let query = ListQuery {
                input: input.clone(),
                viewer: viewer.clone(),
                limit,
                cursor: cursor.clone(),
            };
            let page = match self.protect(&ctx, false, ctx.host.list(&ctx, query)).await {
                Ok(p) => p,
                Err(_) if pass > 0 => break,
                Err(e) => return Err(e),
            };
            let delivered: Vec<_> = page
                .entries
                .iter()
                .filter(|e| {
                    cursor.as_ref().is_none_or(|c| {
                        e.updated_at.0 != c.updated_before || !c.seen_at.contains(&e.number)
                    })
                })
                .cloned()
                .collect();
            next_cursor = if page.truncated && page.continues {
                cursor_after(cursor.as_ref(), &page, delivered.len())
            } else {
                None
            };
            entries.extend(delivered.into_iter().filter(|e| {
                matches_filters(e, input.filters.as_ref().and_then(Option::as_ref), &viewer)
            }));
            truncated = page.truncated;
            if input.filters.as_ref().and_then(Option::as_ref).is_none()
                || entries.len() >= limit
                || next_cursor.is_none()
            {
                break;
            }
            cursor = next_cursor.as_ref().map(|s| parse_cursor(s)).transpose()?;
        }
        Ok((entries, truncated, next_cursor))
    }
    async fn listing_group(
        &self,
        group: Vec<(Context, String, Option<ListCursor>)>,
        input: PullRequestListInput,
        limit: usize,
    ) -> Vec<BatchResult> {
        let (first, viewer, cursor) = &group[0];
        let repositories = group
            .iter()
            .map(|(ctx, _, _)| ctx.project.repository.clone())
            .collect::<Vec<_>>();
        let query = ListQuery {
            input: input.clone(),
            viewer: viewer.clone(),
            limit,
            cursor: cursor.clone(),
        };
        if let Ok(Some(page)) = self
            .protect(
                first,
                false,
                first.host.list_across(first, &repositories, query),
            )
            .await
        {
            let boundary = page
                .entries
                .iter()
                .map(|e| e.updated_at.0.as_str())
                .min()
                .map(str::to_string);
            let mut results = Vec::new();
            for (ctx, viewer, cursor) in group {
                let key = format!(
                    "{} {}",
                    ctx.project.host,
                    ctx.project.repository.to_lowercase()
                );
                let fetched = page
                    .entries
                    .iter()
                    .filter(|e| {
                        e.repository
                            .as_str()
                            .eq_ignore_ascii_case(&ctx.project.repository)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if fetched.is_empty()
                    && page
                        .unresolved
                        .iter()
                        .any(|r| r.eq_ignore_ascii_case(&ctx.project.repository))
                {
                    let result = self
                        .listing_repository(ctx.clone(), input.clone(), viewer, cursor, limit)
                        .await;
                    results.push((ctx, key, result));
                    continue;
                }
                let mut delivered = fetched
                    .iter()
                    .filter(|e| {
                        cursor.as_ref().is_none_or(|c| {
                            e.updated_at.0 != c.updated_before || !c.seen_at.contains(&e.number)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let next = if page.truncated {
                    boundary.as_ref().map(|boundary| {
                        let mut seen = cursor
                            .as_ref()
                            .filter(|c| c.updated_before == *boundary)
                            .map_or_else(Vec::new, |c| c.seen_at.clone());
                        for entry in &fetched {
                            if entry.updated_at.0 == *boundary && !seen.contains(&entry.number) {
                                seen.push(entry.number);
                            }
                        }
                        format!(
                            "{boundary}|{}|{}",
                            cursor.as_ref().map_or(0, |c| c.delivered) + delivered.len() as u64,
                            seen.iter()
                                .map(u64::to_string)
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    })
                } else {
                    None
                };
                delivered.retain(|e| {
                    matches_filters(e, input.filters.as_ref().and_then(Option::as_ref), &viewer)
                });
                for entry in &mut delivered {
                    entry.project_id = ctx.project.id.clone();
                    entry.project_title = ctx.project.title.clone();
                }
                results.push((ctx, key, Ok((delivered, page.truncated, next))));
            }
            return results;
        }
        stream::iter(group.into_iter().map(|(ctx, viewer, cursor)| {
            let input = input.clone();
            async move {
                let key = format!(
                    "{} {}",
                    ctx.project.host,
                    ctx.project.repository.to_lowercase()
                );
                let result = self
                    .listing_repository(ctx.clone(), input, viewer, cursor, limit)
                    .await;
                (ctx, key, result)
            }
        }))
        .buffered(12)
        .collect()
        .await
    }
    pub async fn list(&self, input: PullRequestListInput) -> Result<PullRequestListResult> {
        let selected = self
            .selected(
                input.project_id.as_ref().and_then(Option::as_ref),
                input.project_ids.as_ref().and_then(Option::as_ref),
                input
                    .host
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|s| s.as_str()),
            )
            .await?;
        let Some(first) = selected.first().cloned() else {
            return self.list_uncached(input).await;
        };
        let fingerprint = json!([
            &input,
            selected
                .iter()
                .map(|b| json!([b.project, b.host.revision()]))
                .collect::<Vec<_>>(),
            self.0.list_epoch.load(Ordering::Acquire)
        ])
        .to_string();
        let ctx = Self::binding_context(first)?;
        let service = self.clone();
        self.read(
            ctx,
            ReadPolicy {
                kind: "list",
                ttl: 30_000,
                bypass: false,
                allow_stale: false,
                persist: false,
            },
            fingerprint,
            async move { service.list_uncached(input).await },
        )
        .await
    }
    async fn list_uncached(&self, input: PullRequestListInput) -> Result<PullRequestListResult> {
        let epoch_at_start = self.0.epoch.load(Ordering::Acquire);
        let scopes_at_start = self
            .0
            .versions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let _activity = self.0.tasks.token();
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::invalid("list", "Pull request service is shut down."));
        }
        let cursor_input = input.cursors.as_ref().and_then(Option::as_ref);
        let cursors: Option<BTreeMap<String, ListCursor>> = cursor_input
            .map(|cs| {
                cs.iter()
                    .map(|(k, v)| Ok((k.as_str().into(), parse_cursor(v.as_str())?)))
                    .collect()
            })
            .transpose()?;
        let limit = input.limit.flatten().unwrap_or(99);
        if !(1..=999).contains(&limit) {
            return Err(Error::invalid("list", "Invalid pull request list limit."));
        }
        let selected = self
            .selected(
                input.project_id.as_ref().and_then(Option::as_ref),
                input.project_ids.as_ref().and_then(Option::as_ref),
                input
                    .host
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|s| s.as_str()),
            )
            .await?;
        let mut groups: BTreeMap<String, Vec<crate::context::Binding>> = BTreeMap::new();
        for binding in selected {
            groups
                .entry(binding.project.host.clone())
                .or_default()
                .push(binding);
        }
        let mut viewers = BTreeMap::new();
        let mut providers = Vec::new();
        let mut readable = Vec::new();
        let mut errors = Vec::new();
        let mut blocking = None;
        for (host, bindings) in groups {
            let first = &bindings[0];
            let mut viewer = None;
            let mut failure = None;
            for binding in &bindings {
                let ctx = Self::binding_context(binding.clone())?;
                match self.viewer_cached(&ctx).await {
                    Ok(v) if !v.trim().is_empty() => {
                        viewer = Some(v);
                        break;
                    }
                    Err(e) => failure = Some(e),
                    _ => {}
                }
            }
            providers.push(crate::convert(json!({"host":host,"kind":first.host.kind(),"searchesOnHost":first.host.capabilities().search,"actions":first.host.capabilities().actions,"projectCount":bindings.len(),"configured":viewer.is_some(),"detail":failure.as_ref().filter(|_|viewer.is_none()).map(|e|e.user_detail())}))?);
            if let Some(viewer) = viewer {
                viewers.insert(host, viewer.clone());
                for binding in bindings {
                    readable.push((Self::binding_context(binding)?, viewer.clone()));
                }
            } else {
                for binding in bindings {
                    errors.push(crate::convert(json!({"projectId":binding.project.id,"projectTitle":binding.project.title,"message":format!("{} could not be read.",binding.project.repository)}))?);
                }
                if blocking.is_none() {
                    blocking = failure;
                }
            }
        }
        let mut seen = HashSet::new();
        readable.retain(|(c, _)| {
            seen.insert(format!(
                "{} {}",
                c.project.host,
                c.project.repository.to_lowercase()
            ))
        });
        let seed_contexts = readable
            .iter()
            .map(|(ctx, _)| (ctx.project.id.clone(), ctx.clone()))
            .collect::<HashMap<_, _>>();
        let mut read_groups: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for (ctx, viewer) in readable {
            let key = format!(
                "{} {}",
                ctx.project.host,
                ctx.project.repository.to_lowercase()
            );
            if cursors.as_ref().is_some_and(|cs| !cs.contains_key(&key)) {
                continue;
            }
            let cursor = cursors.as_ref().and_then(|cs| cs.get(&key)).cloned();
            let group_key = json!([
                ctx.host.kind(),
                ctx.project.host,
                cursor.as_ref().map(|c| &c.updated_before)
            ])
            .to_string();
            read_groups
                .entry(group_key)
                .or_default()
                .push((ctx, viewer, cursor));
        }
        let mut requests = Vec::new();
        for group in read_groups.into_values() {
            for chunk in group.chunks(100) {
                let group = chunk.to_vec();
                let service = self.clone();
                let input = input.clone();
                requests
                    .push(async move { service.listing_group(group, input, limit as usize).await });
            }
        }
        let results = stream::iter(requests)
            .buffered(12)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let mut entries = Vec::new();
        let mut next_cursors = BTreeMap::new();
        let mut truncated = false;
        for (ctx, key, result) in results {
            match result {
                Ok((rows, more, cursor)) => {
                    entries.extend(rows);
                    truncated |= more;
                    if let Some(cursor) = cursor {
                        next_cursors.insert(key.into(), cursor.into());
                    }
                }
                Err(_) => {
                    errors.push(crate::convert(json!({"projectId":ctx.project.id,"projectTitle":ctx.project.title,"message":format!("{} could not be read.",ctx.project.repository)}))?);
                }
            }
        }
        entries.sort_by(|a, b| b.updated_at.0.cmp(&a.updated_at.0));
        let unchanged = self.0.epoch.load(Ordering::Acquire) == epoch_at_start
            && *self.0.versions.lock().unwrap_or_else(|e| e.into_inner()) == scopes_at_start;
        if unchanged {
            let now = self.0.clock.now_ms();
            for entry in entries.iter().take(64).rev() {
                let Some(mut context) = seed_contexts.get(&entry.project_id).cloned() else {
                    continue;
                };
                context.reference.number = entry.number;
                let key = self.cache_key(&context, "preview", "");
                if self
                    .held(&key)
                    .is_some_and(|h| now < h.at.saturating_add(h.ttl))
                {
                    continue;
                }
                let preview = preview_fields(entry)?;
                let ttl = if entry.state == PullRequestState::Merged {
                    600_000
                } else {
                    60_000
                };
                self.store(key, "preview", json!(preview), now, ttl, epoch_at_start);
            }
        }
        let result = PullRequestListResult {
            viewers: crate::convert(viewers)?,
            providers,
            entries,
            errors,
            truncated,
            next_cursors,
        };
        if result.entries.is_empty()
            && result.viewers.is_empty()
            && let Some(mut error) = blocking
        {
            error.list_result = Some(Box::new(result));
            return Err(error);
        }
        Ok(result)
    }
    pub async fn list_stats(
        &self,
        input: PullRequestListStatsInput,
    ) -> Result<PullRequestListStatsResult> {
        let _activity = self.admit()?;
        let mut groups: BTreeMap<String, (Context, Vec<PullRequestRef>)> = BTreeMap::new();
        let mut seen = HashSet::new();
        let mut stats = Vec::new();
        let mut contexts = HashMap::new();
        let now = self.0.clock.now_ms();
        for reference in input.refs {
            if !seen.insert(json!([reference.project_id, reference.number]).to_string()) {
                continue;
            }
            let Ok(ctx) = self.context(&reference).await else {
                continue;
            };
            if ctx.project.id != reference.project_id
                || !ctx
                    .project
                    .repository
                    .eq_ignore_ascii_case(reference.repository.as_str())
            {
                continue;
            }
            if let Some(held) = self.held(&self.cache_key(&ctx, "stat", ""))
                && now < held.at.saturating_add(held.ttl)
            {
                stats.push(crate::convert(held.value)?);
                continue;
            }
            contexts.insert(
                json!([
                    ctx.reference.project_id,
                    ctx.reference.repository,
                    ctx.reference.number
                ])
                .to_string(),
                ctx.clone(),
            );
            let key =
                json!([ctx.host.kind(), ctx.project.host, ctx.credential_namespace]).to_string();
            groups
                .entry(key)
                .or_insert_with(|| (ctx.clone(), Vec::new()))
                .1
                .push(ctx.reference);
        }
        for (_, (ctx, refs)) in groups {
            let suffix = json!([
                &refs,
                refs.iter()
                    .filter_map(|r| contexts
                        .get(&json!([r.project_id, r.repository, r.number]).to_string()))
                    .map(|c| self.cache_key(c, "stat", ""))
                    .collect::<Vec<_>>()
            ])
            .to_string();
            let key = self.cache_key(&ctx, "stats", &suffix);
            let next = ctx.clone();
            let requested = refs.clone();
            if let Ok(values) = self
                .read(
                    ctx.clone(),
                    ReadPolicy {
                        kind: "stats",
                        ttl: 60_000,
                        bypass: false,
                        allow_stale: false,
                        persist: false,
                    },
                    suffix,
                    async move { next.host.list_stats(&next, &requested).await },
                )
                .await
            {
                let at = self.held(&key).map_or(now, |h| h.at);
                for stat in values.into_iter().filter(|s| {
                    refs.iter().any(|r| {
                        r.project_id == s.project_id
                            && r.repository
                                .as_str()
                                .eq_ignore_ascii_case(s.repository.as_str())
                            && r.number == s.number
                    })
                }) {
                    if let Some(context) = contexts
                        .get(&json!([stat.project_id, stat.repository, stat.number]).to_string())
                    {
                        self.store(
                            self.cache_key(context, "stat", ""),
                            "stat",
                            json!(stat),
                            at,
                            60_000,
                            self.0.epoch.load(Ordering::Acquire),
                        );
                    }
                    stats.push(stat);
                }
            }
        }
        Ok(PullRequestListStatsResult { stats })
    }
    pub async fn filter_options(
        &self,
        input: PullRequestFilterOptionsInput,
    ) -> Result<PullRequestFilterOptions> {
        let _activity = self.admit()?;
        let selected = self
            .selected(
                input.project_id.as_ref().and_then(Option::as_ref),
                input.project_ids.as_ref().and_then(Option::as_ref),
                input
                    .host
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|s| s.as_str()),
            )
            .await?;
        let mut seen = HashSet::new();
        let mut requests = Vec::new();
        for binding in selected {
            if !seen.insert((
                binding.project.host.clone(),
                binding.project.repository.to_lowercase(),
            )) {
                continue;
            }
            let ctx = Self::binding_context(binding)?;
            let service = self.clone();
            let input = input.clone();
            requests.push(async move {
                service
                    .protect(&ctx, false, ctx.host.filter_options(&ctx, &input))
                    .await
            });
        }
        let pages = stream::iter(requests)
            .buffered(12)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();
        let mut result = PullRequestFilterOptions {
            labels: Vec::new(),
            authors: Vec::new(),
            references: Vec::new(),
            truncated: pages.iter().any(|p| p.truncated),
        };
        for page in pages {
            result.labels.extend(page.labels);
            result.authors.extend(page.authors);
            result.references.extend(page.references);
        }
        let query = input
            .query
            .as_ref()
            .and_then(Option::as_ref)
            .map_or("", |s| s.as_str());
        result.truncated |= rank_options(
            &mut result.labels,
            query,
            |l| l.name.to_string(),
            |l| vec![l.name.to_string()],
            false,
        )?;
        result.truncated |= rank_options(
            &mut result.authors,
            query,
            |a| a.login.to_string(),
            |a| vec![a.login.to_string(), a.name.clone().unwrap_or_default()],
            false,
        )?;
        result.truncated |= rank_options(
            &mut result.references,
            query,
            |r| r.reference.to_string(),
            |r| vec![r.number.to_string(), r.title.clone()],
            true,
        )?;
        Ok(result)
    }
}
fn rank_options<T>(
    items: &mut Vec<T>,
    query: &str,
    key: impl Fn(&T) -> String,
    texts: impl Fn(&T) -> Vec<String>,
    host_order: bool,
) -> Result<bool> {
    let collator = filter_collator()?;
    let query = query.trim().to_lowercase();
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(key(item).to_lowercase()));
    let rank = |item: &T| {
        let texts = texts(item)
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>();
        if query.is_empty() || texts.contains(&query) {
            0
        } else if texts.iter().any(|s| s.starts_with(&query)) {
            1
        } else if texts.iter().any(|s| s.contains(&query)) {
            2
        } else {
            3
        }
    };
    items.retain(|item| rank(item) < 3);
    items.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            if host_order {
                std::cmp::Ordering::Equal
            } else {
                collator.compare(&key(a), &key(b))
            }
        })
    });
    let truncated = items.len() > 100;
    items.truncate(100);
    Ok(truncated)
}
fn filter_collator() -> Result<&'static icu_collator::CollatorBorrowed<'static>> {
    static COLLATOR: std::sync::OnceLock<
        std::result::Result<icu_collator::CollatorBorrowed<'static>, String>,
    > = std::sync::OnceLock::new();
    COLLATOR
        .get_or_init(|| {
            let mut options = icu_collator::options::CollatorOptions::default();
            options.strength = Some(icu_collator::options::Strength::Primary);
            icu_collator::Collator::try_new(Default::default(), options).map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|_| Error::invalid("filterOptions", "Could not initialize filter ordering."))
}
