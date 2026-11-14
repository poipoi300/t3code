use super::{decode as d, *};
use std::collections::HashMap;
use tokio::{sync::Mutex, time::Instant};
#[derive(Default)]
pub(super) struct Unfiltered {
    pulls: Option<BTreeMap<u64, Value>>,
    newest: Option<i64>,
    scanned_at: Option<Instant>,
}
pub(super) type BranchLists = Arc<Mutex<HashMap<String, Arc<Mutex<Unfiltered>>>>>;
fn updated(raw: &Value) -> Option<i64> {
    text(raw, "updated_at")
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|v| v.timestamp_millis())
}
fn matching(
    raw: &Value,
    branch: &str,
    source: Option<&RefSelector>,
    state: Option<&ChangeRequestState>,
) -> Result<Option<ChangeRequest>> {
    let item = d::checkout(raw)?;
    let belongs = item.head_ref_name.0 == branch
        && source.is_none_or(|s| {
            s.repository
                .as_ref()
                .is_none_or(|r| raw["head"]["repo"]["full_name"] == *r)
                && s.owner
                    .as_ref()
                    .is_none_or(|o| raw["head"]["repo"]["owner"]["login"] == *o)
        });
    Ok((belongs && state.is_none_or(|s| item.state == *s)).then_some(item))
}
impl Forgejo {
    pub(super) async fn branch_list(&self, input: CheckoutListInput) -> Result<Vec<ChangeRequest>> {
        let target = self
            .checkout_target(&input.checkout, None, None, None)
            .await?;
        let source = input
            .source
            .or_else(|| crate::remote::parse_owner_ref(&input.head_selector));
        let branch = source
            .as_ref()
            .map(|s| s.ref_name.as_str())
            .unwrap_or(input.head_selector.trim());
        let state = match input.state {
            Some(ChangeRequestState::Merged | ChangeRequestState::Closed) => "closed",
            Some(ChangeRequestState::Open) => "open",
            None => "all",
        };
        let limit = input.limit.unwrap_or(20);
        if limit == 0 {
            return Ok(vec![]);
        }
        let key = format!(
            "{}\0{}\0{}",
            target.login.url, target.login.name, target.repository
        );
        let mut unfiltered = self.branch_lists.lock().await.get(&key).cloned();
        let mut first_page = None;
        if unfiltered.is_none() {
            let mut result = vec![];
            for page in 1..=10_000 {
                let items = self
                    .target_json(
                        &target,
                        &input.checkout.cwd,
                        &format!(
                            "{}/pulls?state={state}&head={}&sort=recentupdate&limit=50&page={page}",
                            repo_path(&target.repository),
                            encode(branch)
                        ),
                        Method::GET,
                        None,
                    )
                    .await?;
                let items = items
                    .as_array()
                    .ok_or_else(|| error("checkoutList", "Invalid pull request array."))?;
                for item in items {
                    d::checkout(item)?;
                }
                if items
                    .iter()
                    .any(|v| v["head"]["ref"] != branch && v["head"]["label"] != branch)
                {
                    let mut cache = self.branch_lists.lock().await;
                    if cache.len() >= 128 {
                        cache.clear();
                    }
                    unfiltered = Some(cache.entry(key.clone()).or_default().clone());
                    if page == 1 && input.state.is_none() {
                        first_page = Some(items.clone());
                    }
                    break;
                }
                for raw in items {
                    if let Some(item) =
                        matching(raw, branch, source.as_ref(), input.state.as_ref())?
                    {
                        result.push(item);
                    }
                }
                if result.len() >= limit || items.is_empty() {
                    result.truncate(limit);
                    return Ok(result);
                }
                if page == 10_000 {
                    return Err(error(
                        "checkoutList",
                        "Pull request pagination exceeded the safety limit.",
                    ));
                }
            }
        }
        let unfiltered =
            unfiltered.ok_or_else(|| error("checkoutList", "Missing branch lookup state."))?;
        let pulls = {
            let mut state = unfiltered.lock().await;
            let now = Instant::now();
            let since = state
                .scanned_at
                .filter(|at| now.duration_since(*at) < std::time::Duration::from_secs(3600))
                .and(state.newest);
            let mut pulls = if since.is_some() {
                state.pulls.clone().unwrap_or_default()
            } else {
                BTreeMap::new()
            };
            let mut newest = since;
            for page in 1..=10_000 {
                let items = if page == 1 && state.pulls.is_none() && first_page.is_some() {
                    first_page.take().unwrap_or_default()
                } else {
                    self.target_json(
                        &target,
                        &input.checkout.cwd,
                        &format!(
                            "{}/pulls?state=all&sort=recentupdate&limit=50&page={page}",
                            repo_path(&target.repository)
                        ),
                        Method::GET,
                        None,
                    )
                    .await?
                    .as_array()
                    .cloned()
                    .ok_or_else(|| error("checkoutList", "Invalid pull request array."))?
                };
                let dates = items.iter().filter_map(updated).collect::<Vec<_>>();
                if page == 1 && !dates.is_empty() {
                    newest = dates.iter().copied().max();
                }
                for item in &items {
                    let number = d::checkout(item)?.number;
                    pulls.insert(number, item.clone());
                }
                if items.is_empty() || since.is_some_and(|mark| dates.iter().any(|v| *v < mark)) {
                    break;
                }
                if page == 10_000 {
                    return Err(error(
                        "checkoutList",
                        "Pull request pagination exceeded the safety limit.",
                    ));
                }
            }
            if since.is_none() {
                state.scanned_at = Some(now);
            }
            state.newest = newest;
            state.pulls = Some(pulls.clone());
            pulls
        };
        let mut candidates = pulls
            .values()
            .filter_map(
                |raw| match matching(raw, branch, source.as_ref(), input.state.as_ref()) {
                    Ok(Some(item)) => Some(Ok((updated(raw).unwrap_or(0), item.number))),
                    Ok(None) => None,
                    Err(e) => Some(Err(e)),
                },
            )
            .collect::<Result<Vec<_>>>()?;
        candidates.sort_unstable_by(|a, b| b.cmp(a));
        let mut results = vec![];
        for (_, number) in candidates {
            if results.len() >= limit {
                break;
            }
            let current = self
                .target_json(
                    &target,
                    &input.checkout.cwd,
                    &format!("{}/pulls/{number}", repo_path(&target.repository)),
                    Method::GET,
                    None,
                )
                .await;
            let raw = match current {
                Err(e)
                    if matches!(
                        &e,
                        ForgeError::Provider {
                            status: Some(404),
                            ..
                        }
                    ) =>
                {
                    None
                }
                Err(e) => return Err(e),
                Ok(raw) if raw["number"].as_u64() == Some(number) => Some(raw),
                _ => None,
            };
            if let Some(raw) = raw {
                if let Some(item) = matching(&raw, branch, source.as_ref(), input.state.as_ref())? {
                    results.push(item);
                }
            } else {
                if let Some(pulls) = &mut unfiltered.lock().await.pulls {
                    pulls.remove(&number);
                }
            }
        }
        Ok(results)
    }
}
