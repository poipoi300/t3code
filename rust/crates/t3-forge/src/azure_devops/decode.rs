use super::*;
pub(super) fn actor(raw: &Value) -> Value {
    text(raw,"uniqueName").or_else(||text(raw,"displayName")).map(|login|json!({"login":login,"name":text(raw,"displayName"),"avatarUrl":text(raw,"imageUrl")})).unwrap_or(Value::Null)
}
pub(super) fn state(raw: &Value) -> &'static str {
    match text(raw, "status").unwrap_or("").to_lowercase().as_str() {
        "completed" => "merged",
        "abandoned" => "closed",
        _ => "open",
    }
}
pub(super) fn web_url(raw: &Value) -> Option<String> {
    if let Some(link) = text(&raw["_links"]["web"], "href") {
        return Some(link.into());
    }
    if let Some(url) = text(&raw["repository"], "webUrl") {
        return Some(format!(
            "{}/pullrequest/{}",
            url.trim_end_matches('/'),
            raw["pullRequestId"]
        ));
    }
    let raw_url = text(raw, "url")?;
    if let Ok(url) = url::Url::parse(raw_url)
        && url
            .path_segments()
            .is_some_and(|mut p| p.any(|s| s.eq_ignore_ascii_case("_apis")))
        && let (Some(project), Some(repository)) = (
            text(&raw["repository"]["project"], "name"),
            text(&raw["repository"], "name"),
        )
    {
        let base = if url.host_str() == Some("dev.azure.com") {
            url.path_segments()
                .and_then(|mut p| p.next())
                .map(|org| format!("{}/{org}", url.origin().ascii_serialization()))
        } else if url
            .host_str()
            .is_some_and(|h| h.ends_with(".visualstudio.com"))
        {
            Some(url.origin().ascii_serialization())
        } else {
            None
        };
        if let Some(base) = base {
            return Some(format!(
                "{base}/{}/_git/{}/pullrequest/{}",
                encode(project),
                encode(repository),
                raw["pullRequestId"]
            ));
        }
    }
    Some(raw_url.into())
}
pub(super) fn listing(raw: &Value) -> Result<Value> {
    let number = raw["pullRequestId"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| error("decode", "Invalid pull request number."))?;
    let url = web_url(raw)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("decode", "Missing pull request URL."))?;
    let head = text(raw, "sourceRefName")
        .map(|s| s.strip_prefix("refs/heads/").unwrap_or(s))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("decode", "Missing source branch."))?;
    let base = text(raw, "targetRefName")
        .map(|s| s.strip_prefix("refs/heads/").unwrap_or(s))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("decode", "Missing target branch."))?;
    if raw["title"].as_str().is_none() || text(raw, "creationDate").is_none() {
        return Err(error("decode", "Invalid pull request content."));
    }
    let mergeability = match text(raw, "mergeStatus")
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "succeeded" => "mergeable",
        "conflicts" | "failure" | "rejectedbypolicy" => "conflicting",
        _ => "unknown",
    };
    let reviewers: Vec<_> = rows(&raw["reviewers"])
        .map(actor)
        .filter_map(|a| a["login"].as_str().map(str::to_string))
        .collect();
    Ok(
        json!({"number":number,"title":raw["title"],"url":url,"author":actor(&raw["createdBy"]),"headBranch":head,"baseBranch":base,"state":state(raw),"isDraft":raw["isDraft"]==true,"mergeability":mergeability,"additions":0,"deletions":0,"createdAt":raw["creationDate"],"updatedAt":text(raw,"closedDate").or_else(||text(raw,"creationDate")),"reviewRequestLogins":reviewers,"labels":[]}),
    )
}
pub(super) fn summary(raw: &Value) -> Result<Value> {
    let mut value = listing(raw)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("additions");
        object.remove("deletions");
    }
    value["closedAt"] = json!(if state(raw) == "closed" {
        text(raw, "closedDate")
    } else {
        None
    });
    value["mergedAt"] = json!(if state(raw) == "merged" {
        text(raw, "closedDate")
    } else {
        None
    });
    Ok(value)
}
pub(super) fn auto_merge(raw: &Value, value: &mut Value) {
    let enabled = raw["autoCompleteSetBy"].is_object();
    value["autoMergeEnabled"] = json!(enabled);
    if !enabled {
        return;
    }
    let method = match text(&raw["completionOptions"], "mergeStrategy")
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "squash" => Some("squash"),
        "rebase" | "rebasemerge" => Some("rebase"),
        "nofastforward" => Some("merge"),
        _ => {
            if raw["completionOptions"]["squashMerge"] == true {
                Some("squash")
            } else {
                None
            }
        }
    };
    if let Some(method) = method {
        value["autoMergeMethod"] = json!(method);
    }
}
pub(super) fn location(raw: &Value) -> Option<Location> {
    Some(Location {
        project: text(&raw["repository"]["project"], "name")?.into(),
        repository: text(&raw["repository"], "name")?.into(),
    })
}
pub(super) fn checkout(raw: &Value) -> Result<ChangeRequest> {
    if raw["status"].as_str().is_none() {
        return Err(error("checkout", "Missing pull request status."));
    }
    let mut normalized = raw.clone();
    if text(raw, "creationDate").is_none() {
        normalized["creationDate"] = json!("1970-01-01T00:00:00Z");
    }
    let summary = summary(&normalized)?;
    let date = text(raw, "closedDate")
        .or_else(|| text(raw, "creationDate"))
        .filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok());
    let mut value = json!({"provider":"azure-devops","number":summary["number"],"title":summary["title"],"url":summary["url"],"baseRefName":summary["baseBranch"],"headRefName":summary["headBranch"],"state":summary["state"],"isCrossRepository":!raw["forkSource"].is_null(),"closedAt":summary["closedAt"],"mergedAt":summary["mergedAt"],"updatedAt":date.map(|s|json!({"_tag":"Some","value":s})).unwrap_or(json!({"_tag":"None"}))});
    if raw["isDraft"] == true {
        value["isDraft"] = json!(true);
    }
    if let Some(sha) = text(&raw["lastMergeSourceCommit"], "commitId") {
        value["headSha"] = json!(sha);
    }
    parse(value, "checkout")
}
pub(super) fn changes(raw: &Value) -> Result<(Vec<Change>, Option<u64>)> {
    let entries = raw["changeEntries"]
        .as_array()
        .ok_or_else(|| error("changes", "Invalid iteration change list."))?;
    let changes = entries
        .iter()
        .filter_map(|raw| {
            let item = &raw["item"];
            if item["isFolder"] == true
                || text(item, "gitObjectType").is_some_and(|s| !s.eq_ignore_ascii_case("blob"))
            {
                return None;
            }
            let path = item["path"].as_str()?.trim_start_matches('/');
            if path.is_empty() {
                return None;
            }
            let old = raw["sourceServerItem"]
                .as_str()
                .or_else(|| raw["originalPath"].as_str())
                .map(|s| s.trim_start_matches('/'))
                .filter(|s| !s.is_empty())
                .unwrap_or(path);
            let parts: Vec<_> = text(raw, "changeType")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .collect();
            let has = |s: &str| parts.iter().any(|p| p == s);
            let kind = if has("delete") {
                "deleted"
            } else if old != path || has("rename") {
                if has("edit") {
                    "rename-changed"
                } else {
                    "rename-pure"
                }
            } else if has("add") {
                "new"
            } else {
                "change"
            };
            Some(Change {
                path: path.into(),
                old: old.into(),
                kind,
                oid: text(item, "objectId").map(str::to_string),
            })
        })
        .collect();
    Ok((changes, raw["nextSkip"].as_u64().filter(|s| *s > 0)))
}
