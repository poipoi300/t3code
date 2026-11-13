use super::transport::error;
use crate::{FailureReason, Result, forge::ReviewInput, model::*};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
pub(crate) fn parse<T: DeserializeOwned>(value: Value, operation: &str) -> Result<T> {
    serde_json::from_value(value).map_err(|_| {
        error(
            operation,
            FailureReason::Failed,
            "GitHub returned an invalid response.",
        )
    })
}
pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value[key].as_str().map(str::trim).filter(|s| !s.is_empty())
}
pub(crate) fn nodes(value: &Value) -> Vec<&Value> {
    value["nodes"]
        .as_array()
        .map(|a| a.iter().filter(|v| !v.is_null()).collect())
        .unwrap_or_default()
}
pub(crate) fn next_cursor(value: &Value) -> Option<String> {
    (value["pageInfo"]["hasNextPage"] == true)
        .then(|| text(&value["pageInfo"], "endCursor").map(str::to_string))
        .flatten()
}
pub(crate) fn pr(value: &Value) -> Result<&Value> {
    value
        .get("repository")
        .and_then(|r| r.get("pullRequest"))
        .filter(|p| !p.is_null())
        .ok_or_else(|| {
            error(
                "getChangeRequest",
                FailureReason::NotFound,
                "Pull request not found.",
            )
        })
}
pub fn actor(value: &Value, host: &str) -> Value {
    let Some(login) = text(value, "login").or_else(|| text(value, "combinedSlug")) else {
        return Value::Null;
    };
    let avatar = text(value, "avatarUrl")
        .or_else(|| text(value, "teamAvatarUrl"))
        .map(str::to_string)
        .or_else(|| {
            (login.len() <= 39
                && login
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && login
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            .then(|| format!("https://{host}/{login}.png?size=80"))
        });
    let mut out = json!({"login":login,"name":text(value,"name").or_else(|| text(value,"teamName")),"avatarUrl":avatar});
    if value["__typename"] == "Bot" || login.ends_with("[bot]") {
        out["isBot"] = json!(true);
    }
    out
}
pub fn state(value: &Value) -> &'static str {
    if !value["mergedAt"].is_null()
        || value["state"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("MERGED"))
    {
        "merged"
    } else if value["state"]
        .as_str()
        .is_some_and(|s| s.eq_ignore_ascii_case("CLOSED"))
    {
        "closed"
    } else {
        "open"
    }
}
pub(crate) fn mergeability(value: &Value) -> &'static str {
    match value.as_str().unwrap_or("").to_uppercase().as_str() {
        "MERGEABLE" => "mergeable",
        "CONFLICTING" => "conflicting",
        _ => "unknown",
    }
}
pub fn check_status(raw: &Value) -> &'static str {
    let status = text(raw, "status").unwrap_or("").to_uppercase();
    if !status.is_empty() && status != "COMPLETED" {
        return "pending";
    }
    match text(raw, "conclusion")
        .or_else(|| text(raw, "state"))
        .unwrap_or("")
        .to_uppercase()
        .as_str()
    {
        "SUCCESS" => "success",
        "ACTION_REQUIRED" => "action-required",
        "FAILURE" | "ERROR" | "TIMED_OUT" | "STARTUP_FAILURE" => "failure",
        "CANCELLED" => "cancelled",
        "SKIPPED" => "skipped",
        "PENDING" | "EXPECTED" => "pending",
        _ => "neutral",
    }
}
fn timestamp(value: &str) -> Option<(i64, String)> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    let fraction = value
        .split_once('.')
        .map(|(_, tail)| {
            tail.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .unwrap_or_default();
    Some((parsed.timestamp(), fraction))
}
fn newer(candidate: Option<&str>, kept: Option<&str>) -> bool {
    match (candidate, kept) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => {
            if let (Some((ta, mut fa)), Some((tb, mut fb))) = (timestamp(a), timestamp(b)) {
                let precision = fa.len().max(fb.len());
                fa.extend(std::iter::repeat_n('0', precision - fa.len()));
                fb.extend(std::iter::repeat_n('0', precision - fb.len()));
                (ta, fa) >= (tb, fb)
            } else {
                a >= b
            }
        }
    }
}
pub fn checks(contexts: &[Value]) -> Result<Vec<PullRequestCheck>> {
    let mut ordered: Vec<(String, String, Option<String>, Value)> = Vec::new();
    let mut indices = HashMap::new();
    for raw in contexts {
        let Some(name) = text(raw, "name").or_else(|| text(raw, "context")) else {
            continue;
        };
        let workflow = text(raw, "workflowName")
            .or_else(|| text(&raw["checkSuite"]["workflowRun"]["workflow"], "name"))
            .unwrap_or("");
        let status = check_status(raw);
        let at = if status == "pending" {
            Some("9999-12-31T23:59:59Z")
        } else {
            text(raw, "completedAt")
                .filter(|t| *t != "0001-01-01T00:00:00Z")
                .or_else(|| text(raw, "startedAt").filter(|t| *t != "0001-01-01T00:00:00Z"))
        };
        let mut check = json!({"name":name,"status":status,"description":text(raw,"description"),"url":text(raw,"detailsUrl").or_else(|| text(raw,"targetUrl"))});
        if raw["isRequired"].is_boolean() {
            check["required"] = raw["isRequired"].clone();
        }
        let key = (workflow.to_string(), name.to_string());
        if let Some(index) = indices.get(&key).copied() {
            let kept: &(String, String, Option<String>, Value) = &ordered[index];
            if !newer(at, kept.2.as_deref()) {
                continue;
            }
            ordered[index] = (workflow.into(), name.into(), at.map(str::to_string), check);
        } else {
            indices.insert(key, ordered.len());
            ordered.push((workflow.into(), name.into(), at.map(str::to_string), check));
        }
    }
    let mut counts = HashMap::new();
    for (_, name, _, _) in &ordered {
        *counts.entry(name.clone()).or_insert(0usize) += 1;
    }
    let mut values: Vec<_> = ordered
        .into_iter()
        .map(|(workflow, name, _, mut check)| {
            if !workflow.is_empty() && counts[&name] > 1 {
                check["name"] = json!(format!("{workflow} / {name}"));
            }
            check
        })
        .collect();
    values.sort_by_key(|check| !matches!(check["status"].as_str(), Some("failure" | "cancelled")));
    parse(json!(values), "checks")
}
pub fn checks_state(checks: &[PullRequestCheck]) -> Option<PullRequestChecksState> {
    if checks.iter().any(|c| {
        matches!(
            c.status,
            PullRequestCheckStatus::Failure | PullRequestCheckStatus::Cancelled
        )
    }) {
        Some(PullRequestChecksState::Failing)
    } else if checks.iter().any(|c| {
        matches!(
            c.status,
            PullRequestCheckStatus::Pending | PullRequestCheckStatus::ActionRequired
        )
    }) {
        Some(PullRequestChecksState::Pending)
    } else if checks
        .iter()
        .any(|c| c.status == PullRequestCheckStatus::Success)
    {
        Some(PullRequestChecksState::Passing)
    } else {
        None
    }
}
pub(crate) fn review_decision(raw: &Value) -> Value {
    let decision = match raw["reviewDecision"].as_str().unwrap_or("") {
        "APPROVED" => Some("approved"),
        "CHANGES_REQUESTED" => Some("changes-requested"),
        "REVIEW_REQUIRED" => Some("review-required"),
        _ => None,
    };
    let states: Vec<_> = nodes(&raw["latestReviews"])
        .into_iter()
        .filter(|r| r["isMinimized"] != true)
        .filter_map(|r| text(r, "state"))
        .collect();
    json!(
        if matches!(decision, Some("approved" | "changes-requested")) {
            decision
        } else if states.contains(&"CHANGES_REQUESTED") {
            Some("changes-requested")
        } else if decision.is_none() && states.contains(&"APPROVED") {
            Some("approved")
        } else {
            decision
        }
    )
}
pub fn list_item(raw: &Value, host: &str) -> Result<ProviderChangeRequest> {
    if raw["number"].as_u64().filter(|n| *n > 0).is_none()
        || [
            "title",
            "url",
            "headRefName",
            "baseRefName",
            "createdAt",
            "updatedAt",
        ]
        .iter()
        .any(|key| text(raw, key).is_none())
    {
        return Err(error(
            "listChangeRequests",
            FailureReason::Failed,
            "Malformed pull request row.",
        ));
    }
    let rollup = &raw["commits"]["nodes"][0]["commit"]["statusCheckRollup"];
    let state_checks = match text(rollup, "state") {
        Some("SUCCESS") => json!("passing"),
        Some("FAILURE" | "ERROR") => json!("failing"),
        Some("PENDING" | "EXPECTED") => json!("pending"),
        _ => Value::Null,
    };
    let logins: Vec<_> = nodes(&raw["reviewRequests"])
        .into_iter()
        .filter_map(|r| text(&r["requestedReviewer"], "login"))
        .collect();
    let labels: Vec<_> = nodes(&raw["labels"]).into_iter().filter_map(|l| Some(json!({"name":text(l,"name")?,"color":text(l,"color"),"description":text(l,"description")}))).collect();
    let mut value = json!({"number":raw["number"],"title":raw["title"],"url":raw["url"],"author":actor(&raw["author"],host),"headBranch":raw["headRefName"],"baseBranch":raw["baseRefName"],"state":state(raw),"isDraft":raw["isDraft"].as_bool().unwrap_or(false),"mergeability":mergeability(&raw["mergeable"]),"additions":raw["additions"].as_u64().unwrap_or(0),"deletions":raw["deletions"].as_u64().unwrap_or(0),"createdAt":raw["createdAt"],"updatedAt":raw["updatedAt"],"reviewRequestLogins":logins,"labels":labels,"reviewDecision":review_decision(raw),"checksState":state_checks});
    if let Some(stack) = stack_membership(raw) {
        value["stack"] = stack;
    }
    parse(value, "listChangeRequests")
}
pub(crate) fn stack_membership(raw: &Value) -> Option<Value> {
    let s = raw.get("stack")?.as_object()?;
    Some(
        json!({"number":s.get("number")?,"size":s.get("size")?,"base":s.get("baseRefName")?,"position":raw["stackEntry"]["position"]}),
    )
}
pub fn summary(raw: &Value, host: &str) -> Result<ProviderChangeRequestSummary> {
    let mut value = json!({"number":raw["number"],"title":raw["title"],"url":raw["url"],"headBranch":raw["headRefName"],"baseBranch":raw["baseRefName"],"state":state(raw),"isDraft":raw["isDraft"],"closedAt":text(raw,"closedAt"),"mergedAt":text(raw,"mergedAt"),"updatedAt":raw["updatedAt"]});
    for key in ["additions", "deletions", "changedFiles"] {
        if let Some(v) = raw.get(key) {
            value[key] = v.clone();
        }
    }
    if raw.get("author").is_some() {
        value["author"] = actor(&raw["author"], host);
    }
    if raw.get("commits").is_some() {
        let contexts: Vec<_> = nodes(&raw["commits"])
            .into_iter()
            .filter_map(|node| {
                text(&node["commit"]["statusCheckRollup"], "state")
                    .map(|state| json!({"state":state}))
            })
            .collect();
        value["checksState"] = json!(context_checks_state(&contexts)?);
    }
    if raw.get("reviewDecision").is_some() {
        value["reviewDecision"] = review_decision(raw);
    }
    if let Some(v) = raw.get("mergeable") {
        value["mergeability"] = json!(mergeability(v));
    }
    if raw.get("stack").is_some() {
        value["stack"] = stack_membership(raw).unwrap_or(Value::Null);
    }
    if raw.get("autoMergeRequest").is_some() {
        value["autoMergeEnabled"] = json!(!raw["autoMergeRequest"].is_null());
        if let Some(method) = text(&raw["autoMergeRequest"], "mergeMethod") {
            value["autoMergeMethod"] = json!(method.to_lowercase());
        }
    }
    if let Some(sha) = text(raw, "headRefOid") {
        value["headSha"] = json!(sha);
    }
    parse(value, "getChangeRequestSummary")
}
pub fn viewer_permissions(
    repository: &Value,
    include_update: bool,
) -> Result<PullRequestViewerPermissions> {
    let pr = &repository["pullRequest"];
    let permission = text(repository, "viewerPermission").unwrap_or("");
    let write = repository["viewerPermission"].is_null()
        || matches!(permission, "ADMIN" | "MAINTAIN" | "WRITE");
    let triage = write || permission == "TRIAGE";
    let author = pr["viewerDidAuthor"] == true;
    let update = pr["viewerCanUpdate"] == true;
    let branch = include_update
        && pr["viewerCanUpdateBranch"] == true
        && state(pr) == "open"
        && !pr["baseRef"]["compare"].is_null();
    let mut actions = Vec::new();
    if matches!(
        text(&pr["headRepository"], "viewerPermission"),
        Some("ADMIN" | "MAINTAIN" | "WRITE")
    ) && (!include_update || !pr["headRef"].is_null())
    {
        actions.push("delete-branch");
    }
    if write {
        actions.push("merge");
        if repository["autoMergeAllowed"] == true
            && !matches!(
                text(pr, "mergeStateStatus"),
                Some("CLEAN" | "HAS_HOOKS" | "UNSTABLE")
            )
        {
            actions.push("enable-auto-merge");
        }
        actions.extend(["disable-auto-merge", "revert", "approve-workflows"]);
    }
    if update {
        actions.extend(["ready", "draft", "close", "reopen"]);
    }
    if branch {
        actions.push("update-branch");
    }
    let mut value = json!({"actions":actions,"comment":true,"resolve":write||author,"verdicts":if author {vec!["comment"]} else {vec!["comment","approve","request-changes"]},"requestReviewers":write,"labels":triage});
    if write {
        value["stackRebase"] = json!(true);
    }
    if branch {
        value["updateMethods"] = json!(["merge", "rebase"]);
    }
    parse(value, "getViewerPermissions")
}
pub fn detail(
    value: &Value,
    host: &str,
    contexts: &[Value],
    truncated: bool,
) -> Result<ProviderChangeRequestDetail> {
    let raw = pr(value)?;
    let mut item = serde_json::to_value(list_item(raw, host)?)
        .map_err(|_| error("detail", FailureReason::Failed, "Invalid detail."))?;
    let mut checks = checks(contexts)?;
    if truncated {
        checks.push(parse(json!({"name":"Additional checks","status":"pending","description":"Some checks could not be read.","url":null,"placeholder":true}),"checks")?);
    }
    let mut permissions = viewer_permissions(&value["repository"], true)?;
    permissions.maintainer_edits = Some(Some(
        state(raw) == "open"
            && raw["isCrossRepository"] == true
            && raw["viewerDidAuthor"] == true
            && raw["headRepositoryOwner"]["__typename"] == "User"
            && !raw["headRepository"].is_null()
            && !raw["headRef"].is_null(),
    ));
    let reviewers: Vec<_> = nodes(&raw["reviewRequests"])
        .iter()
        .map(|r| actor(&r["requestedReviewer"], host))
        .filter(|v| !v.is_null())
        .collect();
    let behind = raw["baseRef"]["compare"]["behindBy"].as_u64();
    item["headSha"] = json!(text(raw, "headRefOid"));
    item["headRepositoryNameWithOwner"] = json!(text(&raw["headRepository"], "nameWithOwner"));
    item["body"] = json!(raw["body"].as_str().unwrap_or(""));
    item["changedFiles"] = json!(raw["changedFiles"].as_u64().unwrap_or(0));
    item["mergedAt"] = json!(text(raw, "mergedAt"));
    item["closedAt"] = json!(text(raw, "closedAt"));
    item["checksState"] = json!(context_checks_state(contexts)?);
    item["checks"] = json!(checks);
    item["reviewers"] = json!(reviewers);
    item["mergeCapabilities"] = json!({"merge":value["repository"]["mergeCommitAllowed"],"squash":value["repository"]["squashMergeAllowed"],"rebase":value["repository"]["rebaseMergeAllowed"]});
    item["viewerPermissions"] = json!(permissions);
    item["maintainerCanModify"] = raw["maintainerCanModify"].clone();
    item["baseComparison"] = json!(if state(raw) != "open" || behind.is_none() {
        "unknown"
    } else if behind == Some(0) {
        "up-to-date"
    } else {
        "behind"
    });
    if let Some(behind) = behind {
        item["behindBy"] = json!(behind);
    }
    if let Some(v) = raw.get("autoMergeRequest") {
        item["autoMergeEnabled"] = json!(!v.is_null());
        if let Some(method) = text(v, "mergeMethod") {
            item["autoMergeMethod"] = json!(method.to_lowercase());
        }
    }
    if let Some(removal) = nodes(&raw["mergeQueueRemovals"]).last()
        && let Some(id) = text(removal, "id")
    {
        item["mergeQueueRemoval"] =
            json!({"id":id,"removedAt":removal["createdAt"],"reason":text(removal,"reason")});
    }
    parse(item, "getChangeRequest")
}
pub fn review_submission(input: &ReviewInput) -> Result<Value> {
    let event = match input.verdict {
        PullRequestReviewVerdict::Comment => "COMMENT",
        PullRequestReviewVerdict::Approve => "APPROVE",
        PullRequestReviewVerdict::RequestChanges => "REQUEST_CHANGES",
    };
    let mut comments = Vec::new();
    for comment in &input.comments {
        let raw = serde_json::to_value(comment).map_err(|_| {
            error(
                "submitReview",
                FailureReason::Failed,
                "Invalid review comment.",
            )
        })?;
        let position = |p: &Value| -> Result<(Value, &'static str)> {
            match p["kind"].as_str() {
                Some("added") => Ok((p["newLine"].clone(), "RIGHT")),
                Some("deleted") => Ok((p["oldLine"].clone(), "LEFT")),
                Some("context") if p["side"] == "left" => Ok((p["oldLine"].clone(), "LEFT")),
                Some("context") => Ok((p["newLine"].clone(), "RIGHT")),
                _ => Err(error(
                    "submitReview",
                    FailureReason::Failed,
                    "Invalid review position.",
                )),
            }
        };
        let (line, side) = position(&raw["position"])?;
        let mut entry = json!({"path":raw["path"],"body":raw["body"],"line":line,"side":side});
        if let Some(start) = raw.get("startPosition").filter(|v| !v.is_null()) {
            let (start_line, start_side) = position(start)?;
            if start_line != line || start_side != side {
                entry["start_line"] = start_line;
                entry["start_side"] = json!(start_side);
            }
        }
        comments.push(entry);
    }
    Ok(json!({"event":event,"body":input.body,"comments":comments}))
}
pub fn watch_fingerprint(raw: &Value) -> ProviderChangeRequestWatchFingerprint {
    let contexts = &raw["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
    let counts = |v: &Value| {
        let mut counts: Vec<_> = v
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v["count"].as_u64().unwrap_or(0) > 0)
            .map(|v| format!("{}:{}", v["state"].as_str().unwrap_or(""), v["count"]))
            .collect();
        counts.sort();
        counts.join(",")
    };
    let edit = |connection: &Value| {
        nodes(connection)
            .into_iter()
            .filter_map(|v| text(v, "lastEditedAt"))
            .max()
            .unwrap_or("")
            .to_string()
    };
    ProviderChangeRequestWatchFingerprint {
        status: format!(
            "{} {} {} {} {} {}",
            raw["state"].as_str().unwrap_or(""),
            raw["mergeable"].as_str().unwrap_or(""),
            raw["headRefOid"].as_str().unwrap_or(""),
            counts(&contexts["checkRunCountsByState"]),
            counts(&contexts["statusContextCountsByState"]),
            raw["mergeQueueRemovals"]["filteredCount"]
                .as_u64()
                .unwrap_or(0)
        ),
        remarks: format!(
            "{} {} {} {} {}",
            raw["comments"]["totalCount"].as_u64().unwrap_or(0),
            edit(&raw["comments"]),
            raw["reviews"]["totalCount"].as_u64().unwrap_or(0),
            edit(&raw["reviews"]),
            raw["reviewThreads"]["totalCount"].as_u64().unwrap_or(0)
        ),
    }
}
pub(crate) fn unique_actors(values: impl IntoIterator<Item = Value>) -> Vec<Value> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|v| {
            v["login"]
                .as_str()
                .is_some_and(|s| seen.insert(s.to_string()))
        })
        .collect()
}

pub fn context_checks_state(contexts: &[Value]) -> Result<Option<PullRequestChecksState>> {
    let checks = checks(contexts)?;
    let mut statuses: Vec<_> = checks
        .iter()
        .map(|c| serde_json::to_value(&c.status).unwrap_or(Value::Null))
        .collect();
    statuses.extend(
        contexts
            .iter()
            .filter(|c| text(c, "name").is_none() && text(c, "context").is_none())
            .map(|c| json!(check_status(c))),
    );
    Ok(
        if statuses.iter().any(|s| s == "failure" || s == "cancelled") {
            Some(PullRequestChecksState::Failing)
        } else if statuses
            .iter()
            .any(|s| s == "pending" || s == "action-required")
        {
            Some(PullRequestChecksState::Pending)
        } else if statuses.iter().any(|s| s == "success") {
            Some(PullRequestChecksState::Passing)
        } else {
            None
        },
    )
}
