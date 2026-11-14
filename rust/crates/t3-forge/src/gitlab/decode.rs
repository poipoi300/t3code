use super::*;
pub(super) fn actor(raw: &Value) -> Value {
    text(raw,"username").map(|login|json!({"login":login,"name":text(raw,"name"),"avatarUrl":text(raw,"avatar_url").or_else(||text(raw,"avatarUrl"))})).unwrap_or(Value::Null)
}
pub(super) fn state(raw: &Value) -> &'static str {
    let state = text(raw, "state").unwrap_or("").to_ascii_lowercase();
    if text(raw, "merged_at").is_some() || state == "merged" {
        "merged"
    } else if state == "closed" {
        "closed"
    } else {
        "open"
    }
}
pub(super) fn check_status(status: &str) -> &'static str {
    match status.trim().to_lowercase().as_str() {
        "success" => "success",
        "failed" => "failure",
        "canceled" | "canceling" => "cancelled",
        "skipped" => "skipped",
        "manual" => "neutral",
        _ => "pending",
    }
}
pub(super) fn labels(raw: &Value) -> Vec<Value> {
    rows(raw)
        .filter_map(|v| {
            v.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|name| json!({"name":name,"color":null}))
        })
        .collect()
}
pub(super) fn listing(raw: &Value) -> Result<Value> {
    let number = raw["iid"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| error("decodeMergeRequest", "Invalid merge request number."))?;
    for key in [
        "title",
        "web_url",
        "source_branch",
        "target_branch",
        "created_at",
        "updated_at",
    ] {
        if raw[key].as_str().is_none() {
            return Err(error(
                "decodeMergeRequest",
                "Invalid merge request content.",
            ));
        }
    }
    let merge_status = text(raw, "merge_status").unwrap_or("").to_ascii_lowercase();
    let mergeability = if raw["has_conflicts"] == true || merge_status == "cannot_be_merged" {
        "conflicting"
    } else if merge_status == "can_be_merged" {
        "mergeable"
    } else {
        "unknown"
    };
    Ok(
        json!({"number":number,"title":raw["title"],"url":raw["web_url"],"author":actor(&raw["author"]),"headBranch":raw["source_branch"],"baseBranch":raw["target_branch"],"state":state(raw),"isDraft":raw["draft"].as_bool().or_else(||raw["work_in_progress"].as_bool()).unwrap_or(false),"mergeability":mergeability,"additions":0,"deletions":0,"createdAt":raw["created_at"],"updatedAt":raw["updated_at"],"closedAt":text(raw,"closed_at"),"mergedAt":text(raw,"merged_at"),"reviewRequestLogins":rows(&raw["reviewers"]).filter_map(|r|text(r,"username")).collect::<Vec<_>>(),"labels":labels(&raw["labels"])}),
    )
}
pub(super) fn graph_raw(raw: &Value) -> Value {
    json!({"iid":raw["iid"].as_str().and_then(|s|s.parse::<u64>().ok()).map(Value::from).unwrap_or_else(||raw["iid"].clone()),"title":raw["title"],"web_url":raw["webUrl"],"author":{"username":raw["author"]["username"],"name":raw["author"]["name"],"avatar_url":raw["author"]["avatarUrl"]},"source_branch":raw["sourceBranch"],"target_branch":raw["targetBranch"],"state":raw["state"],"draft":raw["draft"],"has_conflicts":raw["conflicts"],"merge_status":raw["mergeStatusEnum"],"created_at":raw["createdAt"],"updated_at":raw["updatedAt"],"merged_at":raw["mergedAt"],"reviewers":nodes(&raw["reviewers"]).cloned().collect::<Vec<_>>(),"labels":nodes(&raw["labels"]).map(|n|n["title"].clone()).collect::<Vec<_>>()})
}
pub(super) fn apply_states(value: &mut Value, raw: &Value, enterprise: Option<bool>) {
    let status = text(&raw["headPipeline"], "status");
    value["checksState"] = json!(status.and_then(|s| match check_status(s) {
        "success" => Some("passing"),
        "failure" | "cancelled" => Some("failing"),
        "pending" => Some("pending"),
        _ => None,
    }));
    let approvals = raw["approvedBy"]["nodes"].as_array().map(Vec::len);
    if nodes(&raw["reviewers"])
        .any(|n| n["mergeRequestInteraction"]["reviewState"] == "REQUESTED_CHANGES")
    {
        value["reviewDecision"] = json!("changes-requested");
    } else if let (Some(approved), Some(count)) = (raw["approved"].as_bool(), approvals)
        && !(!approved && count == 0 && enterprise.is_none())
    {
        value["reviewDecision"] = json!(if !approved && (enterprise == Some(true) || count > 0) {
            Some("review-required")
        } else if count > 0 {
            Some("approved")
        } else {
            None
        });
    }
}
pub(super) fn merge_capabilities(raw: &Value) -> Value {
    json!({"merge":raw["merge_method"]=="merge","rebase":matches!(raw["merge_method"].as_str(),Some("rebase_merge"|"ff")),"squash":matches!(raw["squash_option"].as_str(),Some("always"|"default_on"|"default_off"))})
}
pub(super) fn permissions(
    raw: &Value,
    can_labels: bool,
    can_maintainer: bool,
    can_delete: bool,
) -> Value {
    let can_merge = raw["user"]["can_merge"] != false;
    let actions: Vec<_> = [
        "merge",
        "ready",
        "draft",
        "close",
        "reopen",
        "delete-branch",
        "update-branch",
        "enable-auto-merge",
        "disable-auto-merge",
        "revert",
    ]
    .into_iter()
    .filter(|a| {
        if *a == "delete-branch" {
            can_delete
        } else {
            can_merge
                || !matches!(
                    *a,
                    "merge" | "enable-auto-merge" | "disable-auto-merge" | "update-branch"
                )
        }
    })
    .collect();
    let mut value = json!({"actions":actions,"comment":true,"resolve":true,"verdicts":["comment","approve","request-changes"],"requestReviewers":true,"maintainerEdits":can_maintainer,"labels":can_labels});
    if can_merge {
        value["updateMethods"] = json!(["rebase"]);
    }
    value
}
pub(super) fn detail(
    raw: &Value,
    permissions: Value,
    capabilities: Value,
    checks: Vec<Value>,
) -> Result<Value> {
    let mut value = listing(raw)?;
    value["body"] = json!(raw["description"].as_str().unwrap_or(""));
    value["changedFiles"] = json!(
        raw["changes_count"]
            .as_str()
            .and_then(|s| s
                .trim()
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse::<u64>()
                .ok())
            .unwrap_or(0)
    );
    value["reviewers"] = json!(
        rows(&raw["reviewers"])
            .map(actor)
            .filter(|a| !a.is_null())
            .collect::<Vec<_>>()
    );
    value["checks"] = json!(checks);
    value["viewerPermissions"] = permissions;
    value["mergeCapabilities"] = capabilities;
    value["baseComparison"] = json!(
        raw["diverged_commits_count"]
            .as_u64()
            .map(|n| if n > 0 { "behind" } else { "up-to-date" })
            .unwrap_or("unknown")
    );
    if let Some(behind) = raw["diverged_commits_count"].as_u64() {
        value["behindBy"] = json!(behind);
    }
    if let Some(sha) = text(raw, "sha") {
        value["headSha"] = json!(sha);
    }
    if raw["source_project_id"].is_number()
        && raw["target_project_id"].is_number()
        && raw["source_project_id"] != raw["target_project_id"]
    {
        value["maintainerCanModify"] = json!(raw["allow_collaboration"] == true);
    }
    if raw["auto_merge_enabled"].is_boolean() || raw["merge_when_pipeline_succeeds"].is_boolean() {
        value["autoMergeEnabled"] =
            json!(raw["auto_merge_enabled"] == true || raw["merge_when_pipeline_succeeds"] == true);
        if value["autoMergeEnabled"] == true && raw["squash_on_merge"] == true {
            value["autoMergeMethod"] = json!("squash");
        }
    }
    Ok(value)
}
pub(super) fn stages(raw: &Value, origin: &str) -> Option<Vec<Value>> {
    let pipeline = &raw["project"]["mergeRequest"]["headPipeline"];
    let stages = &pipeline["stages"];
    if stages["pageInfo"]["hasNextPage"] == true
        || rows(&stages["nodes"]).any(|s| text(s, "name").is_none())
    {
        return None;
    }
    let link = |path: Option<&str>| {
        path.filter(|p| p.starts_with('/'))
            .map(|p| format!("{origin}{p}"))
    };
    let mut checks = Vec::new();
    for stage in nodes(stages) {
        let name = text(stage, "name")?;
        let jobs = &stage["jobs"];
        if rows(&jobs["nodes"]).next().is_none()
            || jobs["pageInfo"]["hasNextPage"] == true
            || rows(&jobs["nodes"]).any(|j| text(j, "name").is_none())
        {
            checks.push(json!({"name":name,"status":check_status(text(stage,"status").unwrap_or("")),"description":text(&stage["detailedStatus"],"label"),"url":link(text(&stage["detailedStatus"],"detailsPath").or_else(||text(pipeline,"path")))}));
            continue;
        }
        for job in nodes(jobs).filter(|j| j["retried"] != true) {
            let downstream = &job["downstreamPipeline"];
            let source = if downstream.is_null() {
                job
            } else {
                downstream
            };
            let status = check_status(text(source, "status").unwrap_or(""));
            checks.push(json!({"name":format!("{name} / {}",text(job,"name")?),"status":if status=="failure"&&job["allowFailure"]==true{"neutral"}else{status},"description":if downstream.is_null(){text(&job["detailedStatus"],"label")}else{Some("Downstream pipeline")},"url":link(if downstream.is_null(){text(job,"webPath")}else{text(downstream,"path")})}));
        }
    }
    if checks.is_empty() {
        None
    } else {
        Some(checks)
    }
}
pub(super) fn note(raw: &Value) -> Option<Value> {
    if raw["system"] == true || raw["id"].as_i64().is_none() || raw["created_at"].as_str().is_none()
    {
        return None;
    }
    let body = raw["body"].as_str().unwrap_or("");
    if body.trim().is_empty() {
        return None;
    }
    Some(
        json!({"id":raw["id"].to_string(),"kind":if raw["type"]=="DiffNote"{"review-comment"}else{"issue-comment"},"body":body,"author":actor(&raw["author"]),"createdAt":raw["created_at"],"url":null,"path":text(&raw["position"],"new_path").or_else(||text(&raw["position"],"old_path")),"reviewState":null}),
    )
}
pub(super) fn patch(raw: &Value) -> (String, bool) {
    let mut patch = String::new();
    let mut truncated = false;
    for file in rows(raw) {
        let (Some(old), Some(new)) = (file["old_path"].as_str(), file["new_path"].as_str()) else {
            truncated = true;
            continue;
        };
        let hunks = file["diff"].as_str().unwrap_or("");
        truncated |= hunks.is_empty() && (file["too_large"] == true || file["collapsed"] == true);
        patch.push_str(&format!(
            "diff --git {} {}\n",
            quote_patch_path(&format!("a/{old}")),
            quote_patch_path(&format!("b/{new}"))
        ));
        if file["new_file"] == true {
            patch.push_str(&format!(
                "new file mode {}\n",
                text(file, "b_mode").unwrap_or("100644")
            ));
        }
        if file["deleted_file"] == true {
            patch.push_str(&format!(
                "deleted file mode {}\n",
                text(file, "a_mode").unwrap_or("100644")
            ));
        }
        if file["renamed_file"] == true {
            patch.push_str(&format!(
                "similarity index {}%\nrename from {}\nrename to {}\n",
                if hunks.is_empty() && file["too_large"] != true && file["collapsed"] != true {
                    100
                } else {
                    0
                },
                quote_patch_path(old),
                quote_patch_path(new)
            ));
        }
        patch.push_str(&format!(
            "--- {}\n+++ {}\n",
            if file["new_file"] == true {
                "/dev/null".into()
            } else {
                quote_patch_path(&format!("a/{old}"))
            },
            if file["deleted_file"] == true {
                "/dev/null".into()
            } else {
                quote_patch_path(&format!("b/{new}"))
            }
        ));
        if !hunks.is_empty() {
            patch.push_str(hunks.trim_end_matches('\n'));
            patch.push('\n');
        }
    }
    (patch, truncated)
}
pub(super) fn award_name(content: &PullRequestReactionContent) -> &'static str {
    match content {
        PullRequestReactionContent::ThumbsUp => "thumbsup",
        PullRequestReactionContent::ThumbsDown => "thumbsdown",
        PullRequestReactionContent::Laugh => "laughing",
        PullRequestReactionContent::Hooray => "tada",
        PullRequestReactionContent::Confused => "confused",
        PullRequestReactionContent::Heart => "heart",
        PullRequestReactionContent::Rocket => "rocket",
        PullRequestReactionContent::Eyes => "eyes",
    }
}
