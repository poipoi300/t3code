use super::{GitHub, decode::*, transport::error};
use crate::{
    FailureReason, Result,
    forge::*,
    model::*,
    registry::parse_fetch_remotes,
    remote::{parse_owner_ref, parse_remote},
};
use reqwest::Method;
use serde_json::{Value, json};
use std::path::Path;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub host: String,
    pub owner: String,
    pub name: String,
}
pub fn parse_repository_selector(value: &str, host: &str) -> Option<Repository> {
    let value = value.trim();
    let parsed = url::Url::parse(value)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"));
    let (host, path) = if let Some(url) = &parsed {
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        (url.authority().to_lowercase(), url.path())
    } else {
        (host.to_string(), value)
    };
    let path = path.trim_end_matches('/');
    let path = if path.to_lowercase().ends_with(".git") {
        &path[..path.len() - 4]
    } else {
        path
    };
    let parts: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [owner, name]
            if super::transport::valid_host(&host)
                && super::adapter::repo_parts(&format!("{owner}/{name}")).is_ok() =>
        {
            Some(Repository {
                host,
                owner: (*owner).into(),
                name: (*name).into(),
            })
        }
        [h, owner, name]
            if super::transport::valid_host(h)
                && super::adapter::repo_parts(&format!("{owner}/{name}")).is_ok() =>
        {
            Some(Repository {
                host: h.to_lowercase(),
                owner: (*owner).into(),
                name: (*name).into(),
            })
        }
        _ => None,
    }
}
pub fn is_github_host(host: &str) -> bool {
    let host = host.to_lowercase();
    host == "github.com" || host.ends_with(".ghe.com") || host.split('.').any(|s| s == "github")
}
pub fn api_host_for_remote(value: &str) -> Option<String> {
    let remote = parse_remote(value)?;
    let h = &remote.hostname;
    if remote.ssh {
        if !h.contains('.') {
            return h.contains("github").then(|| "github.com".into());
        }
        if let Some(suffix) = h.strip_prefix("github.com-")
            && !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Some("github.com".into());
        }
    }
    is_github_host(h).then_some(remote.host)
}
pub fn select_base_repository(remotes: &str, resolved: &str, host: &str) -> Option<Repository> {
    let mut repos = Vec::new();
    for (name, url) in parse_fetch_remotes(remotes) {
        let remote = parse_remote(&url)?;
        let h = api_host_for_remote(&url).unwrap_or(remote.host);
        if h != host.to_lowercase() {
            return None;
        }
        let mut repo = parse_repository_selector(remote.repository.as_deref()?, host)?;
        repo.owner = repo.owner.to_lowercase();
        repo.name = repo.name.to_lowercase();
        repos.push((name, repo));
    }
    let marks: Vec<_> = resolved
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(char::is_whitespace)?;
            let name = key.strip_prefix("remote.")?.strip_suffix(".gh-resolved")?;
            repos
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, repo)| (repo, value.trim()))
        })
        .collect();
    if marks.len() > 1 {
        return None;
    }
    if let Some((repo, value)) = marks.first() {
        return if *value == "base" {
            Some((*repo).clone())
        } else {
            parse_repository_selector(value, host)
        };
    }
    let score = |name: &str| match name.to_lowercase().as_str() {
        "upstream" => 3,
        "github" => 2,
        "origin" => 1,
        _ => 0,
    };
    repos.sort_by_key(|(name, _)| std::cmp::Reverse(score(name)));
    let top = repos.first()?;
    if repos
        .get(1)
        .is_some_and(|next| score(&next.0) == score(&top.0))
    {
        None
    } else {
        Some(top.1.clone())
    }
}
pub fn checkout_branch_name(
    head: &str,
    owner: Option<&str>,
    cross: bool,
    default: Option<&str>,
) -> String {
    if cross && default == Some(head) {
        owner
            .map(|owner| format!("{owner}/{head}"))
            .unwrap_or_else(|| head.into())
    } else {
        head.into()
    }
}
pub(crate) fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
impl GitHub {
    pub(crate) async fn git(
        &self,
        cwd: &Path,
        args: Vec<String>,
        allow_nonzero: bool,
    ) -> Result<String> {
        let output = self
            .git_driver
            .at(cwd)
            .with_environment(self.git_environment.clone())
            .execute(
                &args,
                t3_git::ExecuteOptions {
                    allow_nonzero,
                    output_mode: t3_git::OutputMode::Error,
                    max_output_bytes: 8 * 1024 * 1024,
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| {
                error(
                    "git",
                    FailureReason::Failed,
                    "The repository could not be read or updated with git.",
                )
            })?;
        Ok(if output.code == Some(0) {
            output.stdout_text()
        } else {
            String::new()
        })
    }
    pub(crate) async fn resolve_repository(&self, input: &CheckoutInput) -> Result<Repository> {
        let hint = input
            .context
            .as_ref()
            .and_then(|c| url::Url::parse(&c.provider.base_url).ok())
            .map(|u| u.authority().to_lowercase());
        let host = hint.as_deref().unwrap_or(&self.default_host);
        if let Some(selector) = &self.repository_override
            && let Some(repo) = parse_repository_selector(selector, host)
        {
            return Ok(repo);
        }
        let remotes = self
            .git(&input.cwd, vec!["remote".into(), "-v".into()], true)
            .await?;
        let marks = self
            .git(
                &input.cwd,
                vec![
                    "config".into(),
                    "--get-regexp".into(),
                    "^remote\\..*\\.gh-resolved$".into(),
                ],
                true,
            )
            .await?;
        let parsed = parse_fetch_remotes(&remotes);
        let host = hint
            .or_else(|| {
                parsed
                    .iter()
                    .find(|(n, u)| n == "origin" && api_host_for_remote(u).is_some())
                    .and_then(|(_, u)| api_host_for_remote(u))
            })
            .or_else(|| parsed.iter().find_map(|(_, u)| api_host_for_remote(u)))
            .unwrap_or_else(|| self.default_host.clone());
        if let Some(repo) = select_base_repository(&remotes, &marks, &host) {
            return Ok(repo);
        }
        let mut parsed = parsed;
        parsed.sort_by_key(|(n, _)| match n.to_lowercase().as_str() {
            "upstream" => 0,
            "github" => 1,
            "origin" => 2,
            _ => 3,
        });
        for (_, url) in parsed {
            if let Some(remote) = parse_remote(&url)
                && api_host_for_remote(&url).unwrap_or(remote.host) == host
                && let Some(repo) = remote
                    .repository
                    .and_then(|r| parse_repository_selector(&r, &host))
            {
                return Ok(repo);
            }
        }
        Err(error(
            "resolveRepository",
            FailureReason::Failed,
            "Could not resolve the GitHub repository.",
        ))
    }
    pub(crate) async fn repository_json(
        &self,
        repository: &Repository,
        cwd: &str,
    ) -> Result<Value> {
        self.rest_json(
            &repository.host,
            cwd,
            "getRepository",
            Method::GET,
            &format!(
                "repos/{}/{}",
                encode(&repository.owner),
                encode(&repository.name)
            ),
            None,
        )
        .await
    }
    pub(crate) async fn checkout_record(&self, input: CheckoutGetInput) -> Result<ChangeRequest> {
        let reference = input.reference.trim();
        let link =
            crate::remote::parse_pull_request_url(reference).filter(|l| l.provider == "github");
        let repository = if let Some(link) = &link {
            parse_repository_selector(&link.repository, &link.authority).ok_or_else(|| {
                error(
                    "getChangeRequest",
                    FailureReason::Failed,
                    "Invalid repository.",
                )
            })?
        } else {
            self.resolve_repository(&input.checkout).await?
        };
        let cwd = input.checkout.cwd.to_string_lossy();
        let numbered = reference
            .trim_start_matches('#')
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0);
        let number = link.map(|l| l.number).or(numbered);
        if let Some(number) = number {
            let value = self
                .http
                .graphql(
                    &repository.host,
                    &cwd,
                    "PullRequestByNumber",
                    json!({"owner":repository.owner,"name":repository.name,"number":number}),
                )
                .await?;
            return change_request(pr(&value)?);
        }
        for state in [Some(ChangeRequestState::Open), None] {
            let records = self
                .checkout_list(CheckoutListInput {
                    checkout: input.checkout.clone(),
                    source: None,
                    head_selector: reference.into(),
                    state,
                    limit: Some(1),
                })
                .await?;
            if let Some(record) = records.into_iter().next() {
                return Ok(record);
            }
        }
        Err(error(
            "getChangeRequest",
            FailureReason::NotFound,
            "No pull request has this head branch.",
        ))
    }

    pub(crate) async fn checkout_list(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        let repo = self.resolve_repository(&input.checkout).await?;
        let cwd = input.checkout.cwd.to_string_lossy();
        let state = match input.state {
            Some(ChangeRequestState::Open) => vec!["OPEN"],
            Some(ChangeRequestState::Closed) => vec!["CLOSED"],
            Some(ChangeRequestState::Merged) => vec!["MERGED"],
            None => vec!["OPEN", "CLOSED", "MERGED"],
        };
        let owner = parse_owner_ref(&input.head_selector);
        let head = owner
            .as_ref()
            .map_or(input.head_selector.as_str(), |s| s.ref_name.as_str());
        let limit = input
            .limit
            .unwrap_or(if state == ["OPEN"] { 1 } else { 20 });
        if owner.is_some() {
            let encoded_query = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query
                    .append_pair("head", input.head_selector.trim())
                    .append_pair(
                        "state",
                        if state == ["OPEN"] {
                            "open"
                        } else if state.len() == 3 {
                            "all"
                        } else {
                            "closed"
                        },
                    )
                    .append_pair("sort", "created")
                    .append_pair("direction", "desc")
                    .append_pair("per_page", "100");
                query.finish()
            };
            let endpoint = format!(
                "repos/{}/{}/pulls?{}",
                encode(&repo.owner),
                encode(&repo.name),
                encoded_query
            );
            let value = self
                .rest_json(
                    &repo.host,
                    &cwd,
                    "listPullRequestsByHead",
                    Method::GET,
                    &endpoint,
                    None,
                )
                .await?;
            let rows = value.as_array().ok_or_else(|| {
                error(
                    "listChangeRequests",
                    FailureReason::Failed,
                    "GitHub returned an invalid change request list.",
                )
            })?;
            return Ok(rows
                .iter()
                .filter_map(|r| rest_change_request(r).ok())
                .filter(|r| input.state.as_ref().is_none_or(|s| r.state == *s))
                .take(limit.clamp(1, 100))
                .collect());
        }
        let value=self.http.graphql(&repo.host,&cwd,"PullRequestsByHead",json!({"owner":repo.owner,"name":repo.name,"head":head,"states":state,"first":if owner.is_some(){100}else{limit.min(100)}})).await?;
        Ok(nodes(&value["repository"]["pullRequests"])
            .into_iter()
            .filter(|raw| {
                owner
                    .as_ref()
                    .and_then(|s| s.owner.as_deref())
                    .is_none_or(|owner| {
                        text(&raw["headRepositoryOwner"], "login")
                            .is_some_and(|login| login.eq_ignore_ascii_case(owner))
                    })
            })
            .filter_map(|v| change_request(v).ok())
            .take(limit)
            .collect())
    }
}
pub fn change_request(raw: &Value) -> Result<ChangeRequest> {
    let mut value = json!({"provider":"github","number":raw["number"],"title":raw["title"].as_str().unwrap_or("").trim(),"url":raw["url"],"baseRefName":raw["baseRefName"],"headRefName":raw["headRefName"],"headSha":text(raw,"headRefOid"),"state":state(raw),"isDraft":raw["isDraft"].as_bool().unwrap_or(false),"closedAt":text(raw,"closedAt"),"mergedAt":text(raw,"mergedAt"),"updatedAt":{"_tag":"None"},"isCrossRepository":raw["isCrossRepository"],"headRepositoryNameWithOwner":text(&raw["headRepository"],"nameWithOwner"),"headRepositoryOwnerLogin":text(&raw["headRepositoryOwner"],"login")});
    if let Some(updated) =
        text(raw, "updatedAt").filter(|v| chrono::DateTime::parse_from_rfc3339(v).is_ok())
    {
        value["updatedAt"] = json!({"_tag":"Some","value":updated});
    }
    parse(value, "getChangeRequest")
}

fn rest_change_request(raw: &Value) -> Result<ChangeRequest> {
    let node = json!({"number":raw["number"],"title":raw["title"],"url":raw["html_url"],"baseRefName":raw["base"]["ref"],"headRefName":raw["head"]["ref"],"headRefOid":raw["head"]["sha"],"state":if !raw["merged_at"].is_null(){"MERGED"}else if raw["state"]=="closed"{"CLOSED"}else{"OPEN"},"isDraft":raw["draft"]==true,"mergedAt":raw["merged_at"],"closedAt":raw["closed_at"],"updatedAt":raw["updated_at"],"isCrossRepository":raw["head"]["repo"]["full_name"]!=raw["base"]["repo"]["full_name"],"headRepository":{"nameWithOwner":raw["head"]["repo"]["full_name"]},"headRepositoryOwner":{"login":raw["head"]["repo"]["owner"]["login"]}});
    change_request(&node)
}
