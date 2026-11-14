use super::*;
pub(super) const UNKNOWN_TIME: &str = "1970-01-01T00:00:00.000Z";
pub(super) fn number(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}
pub(super) fn actor_id(v: &Value) -> Option<&str> {
    text(&v["user"], "id")
        .or_else(|| text(&v["app"], "id"))
        .or_else(|| text(&v["serviceAccount"], "id"))
}
pub(super) fn actor(v: &Value, avatars: &HashMap<String, String>) -> Value {
    let avatar = actor_id(v)
        .and_then(|id| avatars.get(id))
        .filter(|s| !s.is_empty());
    let user = &v["user"];
    if let Some(login) = text(user, "handle")
        .or_else(|| text(user, "email"))
        .or_else(|| text(user, "displayName"))
    {
        return json!({"login":login,"name":text(user,"displayName"),"avatarUrl":avatar});
    }
    let bot = if v["app"].is_object() {
        &v["app"]
    } else {
        &v["serviceAccount"]
    };
    text(bot, "displayName")
        .or_else(|| text(bot, "id"))
        .map(|name| json!({"login":name,"name":name,"avatarUrl":avatar,"isBot":true}))
        .unwrap_or(Value::Null)
}
pub(super) fn avatars(v: &Value, cache: &mut HashMap<String, String>) {
    if let (Some(id), Some(avatar)) = (v["id"].as_str(), v["avatarUrl"].as_str()) {
        cache.insert(id.into(), avatar.trim().into());
    }
    if let Some(items) = v.as_array() {
        for item in items {
            avatars(item, cache);
        }
    }
    if let Some(object) = v.as_object() {
        for item in object.values() {
            avatars(item, cache);
        }
    }
}
pub(super) fn state(v: &Value) -> &'static str {
    if v["merged"] == true || text(v, "mergedAt").is_some() || v["state"] == "merged" {
        "merged"
    } else if v["state"] == "closed" {
        "closed"
    } else {
        "open"
    }
}
pub(super) fn updated(v: &Value) -> &str {
    text(v, "updatedAt")
        .or_else(|| text(v, "createdAt"))
        .unwrap_or(UNKNOWN_TIME)
}
pub(super) fn listing(
    v: &Value,
    avatars: &HashMap<String, String>,
) -> Result<ProviderChangeRequest> {
    let number = number(&v["number"])
        .filter(|n| *n > 0)
        .ok_or_else(|| error("getChangeRequest", "Invalid pull request number."))?;
    parse(
        json!({"number":number,"title":v["title"],"url":v["webUrl"],"author":actor(&v["author"],avatars),"headBranch":v["head"]["ref"],"baseBranch":v["base"]["ref"],"state":state(v),"isDraft":v["draft"]==true,"mergeability":"unknown","additions":number_value(&v["additions"]),"deletions":number_value(&v["deletions"]),"createdAt":text(v,"createdAt").unwrap_or(updated(v)),"updatedAt":updated(v),"closedAt":text(v,"closedAt"),"mergedAt":text(v,"mergedAt"),"reviewRequestLogins":[],"labels":rows(&v["labels"]).map(|label|json!({"name":label["name"],"color":text(label,"color").unwrap_or("888888").trim_start_matches('#'),"description":text(label,"description")})).collect::<Vec<_>>()}),
        "getChangeRequest",
    )
}
pub(super) fn number_value(v: &Value) -> u64 {
    number(v).unwrap_or(0)
}
pub(super) fn checkout(v: &Value) -> Result<ChangeRequest> {
    let row = listing(v, &HashMap::new())?;
    let updated = text(v, "updatedAt")
        .filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
        .map(|s| json!({"_tag":"Some","value":s}))
        .unwrap_or(json!({"_tag":"None"}));
    let mut result = json!({"provider":"cursor-origin","number":row.number,"title":row.title,"url":row.url,"baseRefName":row.base_branch,"headRefName":row.head_branch,"state":row.state,"closedAt":text(v,"closedAt"),"mergedAt":text(v,"mergedAt"),"updatedAt":updated});
    if let Some(head) = text(&v["head"], "sha") {
        result["headSha"] = json!(head);
    }
    if row.is_draft {
        result["isDraft"] = json!(true);
    }
    parse(result, "checkoutChangeRequest")
}
pub(super) fn summary(
    v: &Value,
    avatars: &HashMap<String, String>,
) -> Result<ProviderChangeRequestSummary> {
    let row = serde_json::to_value(listing(v, avatars)?)
        .map_err(|_| error("getSummary", "Invalid summary."))?;
    let mut result = row;
    result["changedFiles"] = json!(number_value(&v["changedFiles"]));
    parse(result, "getSummary")
}
pub(super) fn check(v: &Value) -> Value {
    let status = if v["status"] != "completed" {
        if v["status"] == "failing" {
            "failure"
        } else {
            "pending"
        }
    } else {
        match v["conclusion"].as_str() {
            Some("success") => "success",
            Some("skipped") => "skipped",
            Some("neutral" | "stale") => "neutral",
            Some("cancelled") => "cancelled",
            Some("action_required") => "action-required",
            _ => "failure",
        }
    };
    json!({"name":v["name"],"status":status,"description":text(&v["output"],"title"),"url":text(v,"detailsUrl")})
}
pub(super) fn rollup(checks: &[Value]) -> Option<&'static str> {
    if checks.is_empty() {
        None
    } else if checks.iter().any(|c| {
        matches!(
            c["status"].as_str(),
            Some("failure" | "cancelled" | "action-required")
        )
    }) {
        Some("failing")
    } else if checks.iter().any(|c| c["status"] == "pending") {
        Some("pending")
    } else {
        Some("passing")
    }
}
pub(super) fn version(v: &Value) -> Option<String> {
    Some(format!(
        "{}...{}",
        text(&v["version"], "baseSha").or_else(|| text(&v["base"], "sha"))?,
        text(&v["version"], "headSha").or_else(|| text(&v["head"], "sha"))?
    ))
}
pub(super) fn patch(files: &[Value]) -> Result<String> {
    let mut patch = String::new();
    for file in files {
        let path = text(file, "filename").ok_or_else(|| error("getDiff", "Invalid file name."))?;
        let old = text(file, "previousFilename").unwrap_or(path);
        let added = file["status"] == "added";
        let removed = matches!(file["status"].as_str(), Some("removed" | "deleted"));
        let before = quote_patch_path(&format!("a/{old}"));
        let after = quote_patch_path(&format!("b/{path}"));
        patch.push_str(&format!("diff --git {before} {after}\n"));
        if added {
            patch.push_str("new file mode 100644\n");
        } else if removed {
            patch.push_str("deleted file mode 100644\n");
        }
        if old != path {
            let action = if file["status"] == "copied" {
                "copy"
            } else {
                "rename"
            };
            let similarity = if text(file, "patch").is_some() || number_value(&file["changes"]) > 0
            {
                0
            } else {
                100
            };
            patch.push_str(&format!(
                "similarity index {similarity}%\n{action} from {}\n{action} to {}\n",
                quote_patch_path(old),
                quote_patch_path(path)
            ));
        }
        if let Some(hunks) = file["patch"].as_str().filter(|s| !s.is_empty()) {
            patch.push_str(&format!(
                "--- {}\n+++ {}\n{}\n",
                if added { "/dev/null" } else { &before },
                if removed { "/dev/null" } else { &after },
                hunks.strip_suffix('\n').unwrap_or(hunks)
            ));
        }
    }
    Ok(patch)
}
pub(super) fn activity(
    comments: &[Value],
    reviews: &[Value],
    commits: &[Value],
    avatars: &HashMap<String, String>,
    head: Option<&str>,
    truncated: bool,
    threads_truncated: bool,
) -> Result<ProviderChangeRequestActivity> {
    let mut discussion = vec![];
    let mut threads: Vec<Value> = vec![];
    for c in comments {
        let edited = if text(c, "updatedAt").is_some_and(|s| Some(s) != text(c, "createdAt")) {
            text(c, "updatedAt")
        } else {
            None
        };
        let comment = json!({"id":c["id"],"author":actor(&c["author"],avatars),"body":c["body"].as_str().unwrap_or(""),"createdAt":c["createdAt"],"editedAt":edited,"url":null});
        let thread = &c["thread"];
        if let Some(path) = text(thread, "path") {
            if !thread["id"].is_string() {
                return Err(error("getActivity", "Invalid thread ID."));
            }
            let index = threads.iter().position(|t| t["id"] == thread["id"]);
            let index=index.unwrap_or_else(||{let line=number(&thread["endLine"]).or_else(||number(&thread["startLine"])).filter(|n|*n>0);threads.push(json!({"id":thread["id"],"path":path,"line":line,"side":if thread["side"]=="left"{"left"}else{"right"},"isResolved":text(thread,"resolvedAt").is_some(),"isOutdated":head.zip(text(&thread["version"],"headSha")).is_some_and(|(a,b)|a!=b),"comments":[]}));threads.len()-1});
            threads[index]["comments"]
                .as_array_mut()
                .ok_or_else(|| error("getActivity", "Invalid thread."))?
                .push(comment);
        } else {
            let mut c = comment;
            c["kind"] = json!("issue-comment");
            c["path"] = Value::Null;
            c["reviewState"] = Value::Null;
            discussion.push(c);
        }
    }
    for r in reviews {
        let state = if text(&r["dismissal"], "dismissedAt").is_some() {
            "DISMISSED"
        } else {
            match r["verdict"].as_str() {
                Some("approve") => "APPROVED",
                Some("request_changes") => "CHANGES_REQUESTED",
                _ => "COMMENTED",
            }
        };
        discussion.push(json!({"id":r["id"],"kind":"review","author":actor(&r["author"],avatars),"body":r["body"].as_str().unwrap_or(""),"createdAt":r["submittedAt"],"url":null,"path":null,"reviewState":state}));
    }
    discussion.sort_by(|a, b| a["createdAt"].as_str().cmp(&b["createdAt"].as_str()));
    let commits=commits.iter().map(|c|{let mut value=json!({"oid":c["sha"],"messageHeadline":c["commit"]["message"].as_str().unwrap_or("").lines().next().unwrap_or(""),"committedDate":text(&c["commit"]["committer"],"date").or_else(||text(&c["commit"]["author"],"date")).unwrap_or(UNKNOWN_TIME)});for field in ["additions","deletions"]{if let Some(n)=number(&c["stats"][field]){value[field]=json!(n);}}value}).collect::<Vec<_>>();
    parse(
        json!({"commentCount":discussion.len(),"comments":discussion,"commentsTruncated":truncated,"reviewThreadsTruncated":threads_truncated,"reviewThreads":threads,"commits":commits}),
        "getActivity",
    )
}
