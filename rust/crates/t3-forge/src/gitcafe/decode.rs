use super::*;
pub(super) fn actor(raw: &Value, host: &str) -> Value {
    let login = match raw["kind"].as_str() {
        Some("local") => text(raw, "handle"),
        Some("github") => text(raw, "login"),
        _ => None,
    };
    let Some(login) = login else {
        return Value::Null;
    };
    let avatar = text(raw, "avatarUrl")
        .and_then(|s| {
            url::Url::parse(&format!("https://{host}"))
                .ok()?
                .join(s)
                .ok()
        })
        .map(|u| u.to_string());
    let mut value = json!({"login":login,"name":if raw["kind"]=="local"{text(raw,"displayName")}else{None},"avatarUrl":avatar});
    if raw["bot"] == true || raw["accountType"] == "bot" {
        value["isBot"] = json!(true);
    }
    value
}
pub(super) fn listing(raw: &Value, repo: &ProviderRepositoryRef) -> Result<Value> {
    let number = raw["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| error("decode", "Invalid GitCafe pull request number."))?;
    if text(raw, "id").is_none()
        || text(raw, "title").is_none()
        || !matches!(raw["state"].as_str(), Some("open" | "closed" | "merged"))
        || !raw["draft"].is_boolean()
        || raw["sourceBranch"].as_str().is_none()
        || raw["targetBranch"].as_str().is_none()
        || text(raw, "createdAt").is_none()
        || text(raw, "updatedAt").is_none()
    {
        return Err(error("decode", "Invalid GitCafe pull request content."));
    }
    let mut logins = vec![];
    for reviewer in rows(&raw["reviewers"]) {
        let a = actor(&reviewer["actor"], &repo.host);
        if let Some(login) = text(&a, "login") {
            logins.push(login.to_string());
        }
        if reviewer["actor"]["kind"] == "github"
            && let Some(handle) = text(&reviewer["actor"]["linkedProfile"], "handle")
        {
            logins.push(handle.into());
        }
    }
    Ok(
        json!({"number":number,"title":raw["title"],"url":format!("https://{}/{}/pulls/{number}",repo.host,repo.repository),"author":actor(&raw["author"],&repo.host),"headBranch":raw["sourceBranch"],"baseBranch":raw["targetBranch"],"state":raw["state"],"isDraft":raw["draft"],"mergeability":"unknown","additions":0,"deletions":0,"createdAt":raw["createdAt"],"updatedAt":raw["updatedAt"],"reviewRequestLogins":logins,"labels":rows(&raw["labels"]).map(|l|json!({"name":l["name"],"color":l["color"]})).collect::<Vec<_>>()}),
    )
}
pub(super) fn settled(raw: &Value, value: &mut Value) {
    if raw["state"] == "open" && raw["providerMerge"].is_object() {
        value["state"] = json!("merged");
        value["mergedAt"] = raw["providerMerge"]["mergedAt"].clone();
    }
}
pub(super) fn permissions(raw: &Value) -> Value {
    let caps = &raw["capabilities"];
    let edit = caps["edit"] == true;
    let merge = caps["merge"] == true;
    let open = caps["comment"] == true && (raw["lockedAt"].is_null() || caps["moderate"] == true);
    let viewer = raw["viewerActorId"].as_str();
    let author = viewer.is_some_and(|viewer| {
        raw["author"]["actorId"] == viewer
            || raw["author"]["kind"] == "github"
                && raw["author"]["linkedProfile"]["actorId"] == viewer
    });
    let mut actions = vec![];
    if edit && raw["state"] == "open" {
        actions.extend([
            if raw["draft"] == true {
                "ready"
            } else {
                "draft"
            },
            "close",
        ]);
    } else if edit && raw["state"] == "closed" {
        actions.push("reopen");
    }
    if merge && raw["state"] == "open" && raw["draft"] != true {
        actions.push("merge");
    }
    json!({"actions":actions,"stackRebase":merge,"comment":open,"resolve":open,"verdicts":if caps["review"]!=true{vec![]}else if author{vec!["comment"]}else{vec!["approve","request-changes","comment"]},"requestReviewers":edit&&raw["state"]=="open","labels":edit&&raw["state"]=="open","editChangeRequest":edit,"updateMethods":[]})
}
pub(super) fn emoji(content: &PullRequestReactionContent) -> &'static str {
    match content {
        PullRequestReactionContent::ThumbsUp => "👍",
        PullRequestReactionContent::ThumbsDown => "👎",
        PullRequestReactionContent::Laugh => "😄",
        PullRequestReactionContent::Hooray => "🎉",
        PullRequestReactionContent::Confused => "😕",
        PullRequestReactionContent::Heart => "❤️",
        PullRequestReactionContent::Rocket => "🚀",
        PullRequestReactionContent::Eyes => "👀",
    }
}
pub(super) fn reaction(emoji: &str) -> Option<&'static str> {
    match emoji.replace('\u{fe0f}', "").as_str() {
        "👍" => Some("thumbs-up"),
        "👎" => Some("thumbs-down"),
        "😄" => Some("laugh"),
        "🎉" => Some("hooray"),
        "😕" => Some("confused"),
        "❤" => Some("heart"),
        "🚀" => Some("rocket"),
        "👀" => Some("eyes"),
        _ => None,
    }
}
pub(super) fn checks(raw: &[Value]) -> Vec<Value> {
    let mut kept = vec![];
    let mut keys = HashMap::<String, (usize, String)>::new();
    for raw in raw {
        let order = text(raw, "id")
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .map(|s| s.trim_start_matches('0'))
            .unwrap_or("");
        if order.is_empty() {
            kept.push(raw);
            continue;
        }
        let key = json!([raw["provider"], raw["githubApp"], raw["name"]]).to_string();
        if let Some((index, previous)) = keys.get_mut(&key) {
            if (order.len(), order) > (previous.len(), previous.as_str()) {
                kept[*index] = raw;
                *previous = order.into();
            }
        } else {
            keys.insert(key, (kept.len(), order.into()));
            kept.push(raw);
        }
    }
    let mut checks:Vec<_>=kept.into_iter().map(|raw|{let status=if raw["status"]!="completed"{"pending"}else{match raw["conclusion"].as_str(){Some("success")=>"success",Some("neutral")=>"neutral",Some("skipped")=>"skipped",Some("cancelled")=>"cancelled",Some("action_required")=>"action-required",_=>"failure"}};json!({"name":raw["name"],"status":status,"description":raw["summary"],"url":raw["detailsUrl"]})}).collect();
    checks.sort_by_key(|c| !matches!(c["status"].as_str(), Some("failure" | "cancelled")));
    checks
}
pub(super) fn checks_state(checks: &[Value]) -> Option<&'static str> {
    let has = |name: &str| checks.iter().any(|c| c["status"] == name);
    if has("failure") || has("cancelled") {
        Some("failing")
    } else if has("pending") || has("action-required") {
        Some("pending")
    } else if has("success") {
        Some("passing")
    } else {
        None
    }
}
pub(super) fn stack(
    raw: &Value,
    repo: &ProviderRepositoryRef,
) -> Result<ProviderChangeRequestStack> {
    let number = raw["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| error("stack", "Invalid stack number."))?;
    let mut members: Vec<_> = rows(&raw["members"]).collect();
    members.sort_by_key(|m| m["position"].as_u64());
    parse(
        json!({"id":raw["id"],"number":number,"url":format!("https://{}/{}/stacks/{number}",repo.host,repo.repository),"base":raw["landingBase"],"layers":members.into_iter().map(|m|{let mut value=json!({"number":m["pullRequestNumber"],"title":m["title"],"headBranch":m["sourceBranch"],"state":m["state"],"isDraft":m["draft"]});if let Some(head)=text(m,"headOid"){value["headSha"]=json!(head);}value}).collect::<Vec<_>>()}),
        "stack",
    )
}
pub(super) fn activity(
    host: &str,
    head: Option<&str>,
    comments: (Vec<Value>, bool),
    reviews: (Vec<Value>, bool),
    commits: Vec<Value>,
    reactions: Option<Vec<Value>>,
) -> Result<ProviderChangeRequestActivity> {
    let (mut comments, comments_next) = comments;
    let (reviews, reviews_next) = reviews;
    let mut root_reactions = vec![];
    let mut by_comment = BTreeMap::<String, Vec<Value>>::new();
    if let Some(reactions) = &reactions {
        for r in reactions {
            let Some(content) = text(&r["emoji"], "value")
                .and_then(reaction)
                .filter(|_| r["emoji"]["kind"] == "unicode")
            else {
                continue;
            };
            let value = json!({"content":content,"count":r["count"],"actors":rows(&r["reactors"]).map(|a|actor(a,host)).filter_map(|a|text(&a,"login").map(str::to_string)).collect::<Vec<_>>(),"viewerHasReacted":!r["viewerReactionId"].is_null()});
            if r["subject"]["kind"] == "pull_request" {
                root_reactions.push(value);
            } else if r["subject"]["kind"] == "pull_request_comment"
                && let Some(id) = text(&r["subject"], "id")
            {
                by_comment.entry(id.into()).or_default().push(value);
            }
        }
    }
    let remark = |raw: &Value| {
        let mut v = json!({"id":raw["id"],"author":actor(&raw["author"],host),"body":raw["body"].as_str().unwrap_or(""),"createdAt":raw["createdAt"],"url":null});
        if raw["capabilities"].is_object() {
            v["canEdit"] = json!(raw["capabilities"]["edit"] == true);
        }
        if reactions.is_some() {
            v["reactions"] = json!(
                text(raw, "id")
                    .and_then(|id| by_comment.get(id))
                    .cloned()
                    .unwrap_or_default()
            );
        }
        v
    };
    comments.sort_by_key(|c| text(c, "createdAt").map(str::to_string));
    let mut groups = Vec::<(String, Vec<&Value>)>::new();
    for c in &comments {
        if text(c, "path").is_none()
            || c["line"].as_u64().is_none_or(|n| n == 0)
            || !matches!(c["side"].as_str(), Some("left" | "right"))
        {
            continue;
        }
        let Some(id) = text(c, "threadId") else {
            continue;
        };
        if let Some((_, group)) = groups.iter_mut().find(|(k, _)| k == id) {
            group.push(c);
        } else {
            groups.push((id.into(), vec![c]));
        }
    }
    let threads:Vec<_>=groups.into_iter().map(|(id,group)|{let first=group[0];let latest=group[group.len()-1];let root=group.iter().copied().find(|c|c["id"]==id);let mut value=json!({"id":id,"path":first["path"],"line":first["line"],"side":first["side"],"isResolved":root.is_some_and(|r|!r["resolvedAt"].is_null()),"isOutdated":head.is_some_and(|head|text(latest,"commitOid").is_some_and(|oid|oid!=head)),"comments":group.iter().map(|c|remark(c)).collect::<Vec<_>>()});if let Some(root)=root.filter(|r|r["capabilities"].is_object()){value["canResolve"]=json!(root["capabilities"][if root["resolvedAt"].is_null(){"resolve"}else{"unresolve"}]==true);}value}).collect();
    let mut notes: Vec<_> = comments
        .iter()
        .map(|raw| {
            let mut value = remark(raw);
            value["kind"] = json!(if raw["path"].is_null() {
                "issue-comment"
            } else {
                "review-comment"
            });
            value["path"] = raw["path"].clone();
            value["reviewState"] = Value::Null;
            value
        })
        .collect();
    for review in reviews {
        notes.push(json!({"id":review["id"],"kind":"review","author":actor(&review["author"],host),"body":review["body"].as_str().unwrap_or(""),"createdAt":review["createdAt"],"url":null,"path":null,"reviewState":if !review["dismissedAt"].is_null(){"dismissed"}else{match review["verdict"].as_str(){Some("approve")=>"approved",Some("request_changes")=>"changes_requested",_=>"commented"}}}));
    }
    notes.sort_by_key(|c| text(c, "createdAt").map(str::to_string));
    let mut value = json!({"commentCount":notes.len(),"comments":notes,"commentsTruncated":comments_next||reviews_next,"reviewThreads":threads,"commits":commits.iter().filter_map(|c|{let time=c["time"].as_f64()?;let date=chrono::DateTime::from_timestamp_millis((time*1000.0)as i64)?;Some(json!({"oid":c["oid"],"messageHeadline":c["summary"],"committedDate":date.to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"authors":[]}))}).collect::<Vec<_>>()});
    if reactions.is_some() {
        value["reactions"] = json!(root_reactions);
    }
    parse(value, "activity")
}
pub(super) fn diff(files: &[Value]) -> Result<ProviderDiffSlice> {
    let mut sections = vec![];
    let mut omitted = vec![];
    for f in files {
        let path = text(f, "path").ok_or_else(|| error("diff", "Missing diff path."))?;
        let old = f["oldPath"].as_str().unwrap_or(path);
        let a = quote_patch_path(&format!("a/{old}"));
        let b = quote_patch_path(&format!("b/{path}"));
        let before = if f["status"] == "added" {
            "/dev/null"
        } else {
            &a
        };
        let after = if f["status"] == "deleted" {
            "/dev/null"
        } else {
            &b
        };
        let mode = if f["isSubmodule"] == true {
            "160000"
        } else {
            "100644"
        };
        let mut lines = vec![format!("diff --git {a} {b}")];
        if f["status"] == "added" {
            lines.push(format!("new file mode {mode}"));
        }
        if f["status"] == "deleted" {
            lines.push(format!("deleted file mode {mode}"));
        }
        if matches!(f["status"].as_str(), Some("renamed" | "copied")) {
            let action = if f["status"] == "renamed" {
                "rename"
            } else {
                "copy"
            };
            let changes = rows(&f["hunks"]).next().is_some()
                || f["additions"].as_u64().unwrap_or(0) > 0
                || f["deletions"].as_u64().unwrap_or(0) > 0
                || f["binary"] == true
                || f["tooLarge"] == true
                || f["hunksOmitted"] == true;
            lines.extend([
                format!("similarity index {}%", if changes { 0 } else { 100 }),
                format!("{action} from {}", quote_patch_path(old)),
                format!("{action} to {}", quote_patch_path(path)),
            ]);
        }
        if f["binary"] == true {
            lines.push(format!("Binary files {before} and {after} differ"));
        } else if f["tooLarge"] == true || f["hunksOmitted"] == true {
            omitted.push(json!({"path":path,"additions":f["additions"].as_u64().unwrap_or(0),"deletions":f["deletions"].as_u64().unwrap_or(0)}));
        } else if f["isSubmodule"] == true {
            let has_old = f["status"] != "added" && text(f, "oldOid").is_some();
            let has_new = f["status"] != "deleted" && text(f, "newOid").is_some();
            lines.extend([
                format!("--- {before}"),
                format!("+++ {after}"),
                format!(
                    "@@ -{} +{} @@",
                    if has_old { "1" } else { "0,0" },
                    if has_new { "1" } else { "0,0" }
                ),
            ]);
            if has_old {
                lines.push(format!(
                    "-Subproject commit {}",
                    f["oldOid"].as_str().unwrap_or("")
                ));
            }
            if has_new {
                lines.push(format!(
                    "+Subproject commit {}",
                    f["newOid"].as_str().unwrap_or("")
                ));
            }
        } else {
            if rows(&f["hunks"]).next().is_some() {
                lines.extend([format!("--- {before}"), format!("+++ {after}")]);
            }
            for h in rows(&f["hunks"]) {
                lines.push(format!(
                    "@@ -{},{} +{},{} @@",
                    h["oldStart"], h["oldLines"], h["newStart"], h["newLines"]
                ));
                let hunks: Vec<_> = rows(&h["lines"]).collect();
                for (i, line) in hunks.iter().enumerate() {
                    let origin = line["origin"].as_str().unwrap_or("");
                    if matches!(origin, "=" | ">" | "<") {
                        lines.push("\\ No newline at end of file".into());
                    } else {
                        let content = line["content"].as_str().unwrap_or("");
                        lines.push(format!(
                            "{origin}{}",
                            content.strip_suffix('\n').unwrap_or(content)
                        ));
                        if matches!(origin, "+" | "-" | " ")
                            && !content.ends_with('\n')
                            && !hunks.get(i + 1).is_some_and(|l| {
                                matches!(l["origin"].as_str(), Some("=" | ">" | "<"))
                            })
                        {
                            lines.push("\\ No newline at end of file".into());
                        }
                    }
                }
            }
        }
        sections.push(lines.join("\n"));
    }
    parse(
        json!({"patch":if sections.is_empty(){String::new()}else{sections.join("\n")+"\n"},"truncated":!omitted.is_empty(),"nextCursor":null,"omittedFileStats":omitted}),
        "diff",
    )
}
