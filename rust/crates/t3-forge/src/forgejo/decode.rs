use super::*;
pub(super) fn actor(raw: &Value) -> Value {
    text(raw,"login").map(|login|json!({"login":login,"name":text(raw,"full_name"),"avatarUrl":text(raw,"avatar_url")})).unwrap_or(Value::Null)
}
fn date(v: &Value) -> Value {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| {
            json!(
                d.to_utc()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            )
        })
        .unwrap_or_else(|| v.clone())
}
pub(super) fn head(raw: &Value) -> String {
    let head = &raw["head"];
    let branch = head["ref"].as_str().unwrap_or("");
    if branch.starts_with("refs/pull/") && !head["repo"].is_null() && raw["flow"] != 1 {
        head["label"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(branch)
            .into()
    } else {
        branch.into()
    }
}
pub(super) fn state(raw: &Value) -> &'static str {
    if raw["merged"] == true {
        "merged"
    } else if raw["state"] == "closed" {
        "closed"
    } else {
        "open"
    }
}
pub(super) fn draft(raw: &Value) -> bool {
    raw["draft"].as_bool().unwrap_or_else(|| {
        let title = raw["title"].as_str().unwrap_or("").to_ascii_uppercase();
        title.starts_with("[WIP]") || title.starts_with("WIP:")
    })
}
pub(super) fn listing(raw: &Value) -> Result<Value> {
    if raw["number"].as_u64().is_none_or(|n| n == 0)
        || raw["title"].as_str().is_none()
        || text(raw, "html_url").is_none()
        || raw["head"]["ref"].as_str().is_none()
        || raw["head"]["sha"].as_str().is_none()
        || raw["base"]["ref"].as_str().is_none()
        || raw["created_at"].as_str().is_none()
        || raw["updated_at"].as_str().is_none()
    {
        return Err(error("decode", "Invalid Forgejo pull request."));
    }
    Ok(
        json!({"number":raw["number"],"title":raw["title"],"url":raw["html_url"],"author":actor(&raw["user"]),"headBranch":head(raw),"baseBranch":raw["base"]["ref"],"headRepositoryNameWithOwner":raw["head"]["repo"]["full_name"],"state":state(raw),"isDraft":draft(raw),"mergeability":if raw["mergeable"]==true{"mergeable"}else{"unknown"},"additions":raw["additions"].as_u64().unwrap_or(0),"deletions":raw["deletions"].as_u64().unwrap_or(0),"createdAt":date(&raw["created_at"]),"updatedAt":date(&raw["updated_at"]),"closedAt":date(&raw["closed_at"]),"mergedAt":date(&raw["merged_at"]),"reviewRequestLogins":rows(&raw["requested_reviewers"]).filter_map(|u|text(u,"login")).collect::<Vec<_>>(),"labels":rows(&raw["labels"]).map(|l|json!({"name":l["name"],"color":l["color"],"description":text(l,"description")})).collect::<Vec<_>>()}),
    )
}
pub(super) fn permissions(repo: &Value, pr: &Value, viewer: &str) -> Value {
    let write = repo["permissions"]["push"] == true;
    let edit = write || pr["user"]["login"] == viewer;
    let active = repo["archived"] != true;
    let actions = if active {
        ["merge", "close", "reopen", "update-branch"]
            .into_iter()
            .filter(|a| {
                if matches!(*a, "merge" | "update-branch") {
                    write
                } else {
                    edit
                }
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    json!({"actions":actions,"comment":active&&(pr["is_locked"]!=true||write),"resolve":false,"verdicts":if !active{vec![]}else if pr["user"]["login"]==viewer{vec!["comment"]}else{vec!["comment","approve","request-changes"]},"requestReviewers":active&&edit,"labels":active&&write,"updateMethods":if active&&write{if repo["allow_rebase_update"]==true{vec!["merge","rebase"]}else{vec!["merge"]}}else{vec![]}})
}
pub(super) fn comment(raw: &Value) -> Value {
    let mut value = json!({"id":raw["id"].to_string(),"kind":"issue-comment","author":actor(&raw["user"]),"body":raw["body"],"createdAt":date(&raw["created_at"]),"url":text(raw,"html_url"),"path":null,"reviewState":null});
    if !raw["updated_at"].is_null() && date(&raw["updated_at"]) != value["createdAt"] {
        value["editedAt"] = date(&raw["updated_at"]);
    }
    value
}
pub(super) fn review(raw: &Value) -> Value {
    json!({"id":format!("review:{}",raw["id"]),"kind":"review","author":actor(&raw["user"]),"body":raw["body"],"createdAt":date(&raw["submitted_at"]),"url":text(raw,"html_url"),"path":null,"reviewState":if raw["dismissed"]==true{"DISMISSED"}else{match raw["state"].as_str(){Some("REQUEST_CHANGES")=>"CHANGES_REQUESTED",Some("COMMENT")=>"COMMENTED",_=>raw["state"].as_str().unwrap_or("")}}})
}
pub(super) fn commit(raw: &Value) -> Value {
    let author = actor(&raw["author"]);
    let mut value = json!({"oid":raw["sha"],"messageHeadline":raw["commit"]["message"].as_str().unwrap_or("").lines().next().unwrap_or(""),"committedDate":date(&raw["commit"]["committer"]["date"]),"authors":if author.is_null(){vec![]}else{vec![author]}});
    if raw["stats"].is_object() {
        value["additions"] = raw["stats"]["additions"].clone();
        value["deletions"] = raw["stats"]["deletions"].clone();
    }
    value
}
pub(super) fn reaction_name(content: &PullRequestReactionContent) -> &'static str {
    match content {
        PullRequestReactionContent::ThumbsUp => "+1",
        PullRequestReactionContent::ThumbsDown => "-1",
        PullRequestReactionContent::Laugh => "laugh",
        PullRequestReactionContent::Hooray => "hooray",
        PullRequestReactionContent::Confused => "confused",
        PullRequestReactionContent::Heart => "heart",
        PullRequestReactionContent::Rocket => "rocket",
        PullRequestReactionContent::Eyes => "eyes",
    }
}
pub(super) fn reactions(raw: &[Value], viewer: &str) -> Vec<Value> {
    [ ("thumbs-up","+1"),("thumbs-down","-1"),("laugh","laugh"),("hooray","hooray"),("confused","confused"),("heart","heart"),("rocket","rocket"),("eyes","eyes")].into_iter().filter_map(|(content,name)|{let matching:Vec<_>=raw.iter().filter(|r|r["content"]==name).collect();if matching.is_empty(){return None;}Some(json!({"content":content,"count":matching.len(),"actors":matching.iter().filter_map(|r|text(&r["user"],"login").filter(|login|*login!=viewer)).collect::<Vec<_>>(),"viewerHasReacted":matching.iter().any(|r|r["user"]["login"]==viewer)}))}).collect()
}
pub(super) fn checkout(raw: &Value) -> Result<ChangeRequest> {
    let mut v = json!({"provider":"forgejo","number":raw["number"],"title":raw["title"],"url":raw["html_url"],"state":state(raw),"isDraft":draft(raw),"baseRefName":raw["base"]["ref"],"headRefName":head(raw),"closedAt":raw["closed_at"],"mergedAt":raw["merged_at"],"updatedAt":text(raw,"updated_at").filter(|s|chrono::DateTime::parse_from_rfc3339(s).is_ok()).map(|s|json!({"_tag":"Some","value":s})).unwrap_or(json!({"_tag":"None"})),"isCrossRepository":raw["head"]["repo"].is_object()&&raw["base"]["repo"].is_object()&&raw["head"]["repo"]["full_name"]!=raw["base"]["repo"]["full_name"],"headRepositoryNameWithOwner":raw["head"]["repo"]["full_name"],"headRepositoryOwnerLogin":raw["head"]["repo"]["owner"]["login"]});
    if let Some(sha) = text(&raw["head"], "sha") {
        v["headSha"] = json!(sha);
    }
    parse(v, "checkout")
}
