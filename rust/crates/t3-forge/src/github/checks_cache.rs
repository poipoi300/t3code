use super::{
    GitHub,
    adapter::repo_parts,
    transport::{Request, error, now},
};
use crate::{FailureReason, ForgeError, Result, forge::PullRequestInput, model::CheckResult};
use futures_util::{StreamExt, stream};
use reqwest::Method;
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Default)]
pub(super) struct Entry {
    validators: HashMap<String, Validator>,
    head: Option<Value>,
    value: Option<(u64, CheckResult)>,
    unsupported: bool,
    touched: u64,
}
#[derive(Clone)]
struct Validator {
    etag: Option<String>,
    next: bool,
    body: Vec<u8>,
}
pub(super) type Entries = Arc<Mutex<HashMap<String, Arc<Mutex<Entry>>>>>;
impl GitHub {
    async fn check_validator(
        &self,
        input: &PullRequestInput,
        path: &str,
        previous: Option<&Validator>,
    ) -> Result<(Validator, bool)> {
        let response = self
            .http
            .send(Request {
                host: &input.repo.host,
                cwd: &input.repo.cwd,
                operation: "revalidateChecks",
                method: Method::GET,
                path,
                body: None,
                graphql: false,
                interactive: false,
                accept: "application/vnd.github+json",
                max_bytes: 8 * 1024 * 1024,
                timeout: Duration::from_secs(30),
                if_none_match: previous.and_then(|v| v.etag.as_deref()),
            })
            .await?;
        if response.status == 304
            && let Some(previous) = previous
        {
            return Ok((previous.clone(), false));
        }
        if response.status != 200 || response.truncated {
            return Err(error(
                "revalidateChecks",
                FailureReason::Failed,
                "GitHub returned invalid check validators.",
            ));
        }
        Ok((
            Validator {
                etag: response.headers.get("etag").map(|v| v.trim().into()),
                next: response
                    .headers
                    .get("link")
                    .is_some_and(|v| v.contains("rel=\"next\"")),
                body: response.body,
            },
            true,
        ))
    }
    pub(super) async fn conditional_checks(&self, input: PullRequestInput) -> Result<CheckResult> {
        if !self.http.is_pinned() {
            let detail = self.core(&input).await?;
            return Ok(CheckResult {
                state: detail.state,
                checks: detail.checks,
            });
        }
        let credential = self
            .http
            .credential(&input.repo.host, std::path::Path::new(&input.repo.cwd))
            .await?;
        let key = format!(
            "{}\0{}\0{}\0{}",
            credential.fingerprint,
            input.repo.host.to_lowercase(),
            input.repo.repository.to_lowercase(),
            input.number
        );
        let entry = {
            let mut entries = self.check_entries.lock().await;
            entries.retain(|_, entry| {
                entry
                    .try_lock()
                    .map_or(true, |e| now().saturating_sub(e.touched) < 1_800_000)
            });
            if entries.len() >= 128 && !entries.contains_key(&key) {
                entries.clear();
            }
            entries.entry(key).or_default().clone()
        };
        let mut entry = entry.lock().await;
        entry.touched = now();
        if entry.unsupported {
            let detail = self.core(&input).await?;
            return Ok(CheckResult {
                state: detail.state,
                checks: detail.checks,
            });
        }
        let (owner, name) = repo_parts(&input.repo.repository)?;
        let root = format!("repos/{owner}/{name}");
        let head_path = format!("{root}/pulls/{}", input.number);
        let head = self
            .check_validator(&input, &head_path, entry.validators.get(&head_path))
            .await;
        let head = match head {
            Ok((validator, changed)) => {
                if changed {
                    let value: Value = serde_json::from_slice(&validator.body).map_err(|_| {
                        error(
                            "revalidateChecks",
                            FailureReason::Failed,
                            "GitHub returned an invalid pull request head.",
                        )
                    })?;
                    if !value["head"]["sha"].as_str().is_some_and(|s| {
                        matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
                    }) || value["base"]["repo"]["id"]
                        .as_u64()
                        .is_none_or(|id| id == 0)
                    {
                        return Err(error(
                            "revalidateChecks",
                            FailureReason::Failed,
                            "GitHub returned an invalid pull request head.",
                        ));
                    }
                    if entry
                        .head
                        .as_ref()
                        .is_none_or(|h| h["head"]["sha"] != value["head"]["sha"])
                    {
                        entry.validators.clear();
                    }
                    entry.head = Some(value);
                    entry.value = None;
                }
                entry.validators.insert(head_path, validator);
                entry.head.clone()
            }
            Err(error) => {
                disable_unsupported(&mut entry, &error)?;
                None
            }
        };
        if let Some(head) = head {
            let sha = head["head"]["sha"].as_str().ok_or_else(|| {
                error(
                    "revalidateChecks",
                    FailureReason::Failed,
                    "GitHub returned no pull request head.",
                )
            })?;
            let mut endpoints = vec![
                format!("{root}/commits/{sha}/check-runs?filter=all"),
                format!("{root}/commits/{sha}/status"),
            ];
            if head["head"]["repo"]["id"] != head["base"]["repo"]["id"] {
                endpoints.push(format!(
                    "{root}/actions/runs?head_sha={sha}&event=pull_request"
                ));
            }
            let results: Vec<_> = stream::iter(endpoints)
                .map(|endpoint| {
                    let validators = &entry.validators;
                    let input = &input;
                    async move {
                        let mut pages = Vec::new();
                        for page in 1..=100 {
                            let path = format!(
                                "{endpoint}{}per_page=100&page={page}",
                                if endpoint.contains('?') { "&" } else { "?" }
                            );
                            let (validator, changed) = self
                                .check_validator(input, &path, validators.get(&path))
                                .await?;
                            let next = validator.next;
                            pages.push((path, validator, changed));
                            if !next {
                                return Ok(pages);
                            }
                        }
                        Err(error(
                            "revalidateChecks",
                            FailureReason::Failed,
                            "GitHub's check validator pages are truncated.",
                        ))
                    }
                })
                .buffered(3)
                .collect()
                .await;
            let mut available = true;
            for result in results {
                match result {
                    Ok(pages) => {
                        for (path, validator, changed) in pages {
                            if changed {
                                entry.value = None;
                            }
                            entry.validators.insert(path, validator);
                        }
                    }
                    Err(error) => {
                        disable_unsupported(&mut entry, &error)?;
                        available = false;
                    }
                }
            }
            if available
                && let Some((at, value)) = &entry.value
                && now().saturating_sub(*at) < 300_000
            {
                return Ok(value.clone());
            }
        }
        let detail = self.core(&input).await?;
        let result = CheckResult {
            state: detail.state,
            checks: detail.checks,
        };
        if detail.workflow_approvals_required.is_some()
            && entry.head.as_ref().is_some_and(|h| {
                h["head"]["sha"].as_str() == detail.head_sha.as_ref().and_then(|s| s.as_deref())
            })
        {
            entry.value = Some((now(), result.clone()));
        }
        Ok(result)
    }
}
fn disable_unsupported(entry: &mut Entry, error: &ForgeError) -> Result<()> {
    if matches!(
        error,
        ForgeError::Provider {
            status: Some(404 | 405 | 501),
            ..
        }
    ) {
        entry.unsupported = true;
    }
    if matches!(
        error,
        ForgeError::Provider {
            status: Some(_),
            ..
        }
    ) {
        entry.value = None;
        Ok(())
    } else {
        Err(error.clone())
    }
}
