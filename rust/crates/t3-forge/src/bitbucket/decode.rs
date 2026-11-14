use super::*;
pub(super) fn actor(raw: &Value) -> Value {
    let login = text(raw, "nickname").or_else(|| text(raw, "display_name"));
    login.map(|login|json!({"login":login,"name":text(raw,"display_name"),"avatarUrl":text(&raw["links"]["avatar"],"href")})).unwrap_or(Value::Null)
}
fn date(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| {
            json!(
                d.to_utc()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            )
        })
        .unwrap_or_else(|| value.clone())
}
pub(super) fn state(raw: &Value) -> &'static str {
    match text(raw, "state").unwrap_or("").to_uppercase().as_str() {
        "MERGED" => "merged",
        "DECLINED" | "SUPERSEDED" => "closed",
        _ => "open",
    }
}
pub(super) fn listing(raw: &Value) -> Result<Value> {
    if raw["id"].as_u64().is_none_or(|n| n == 0)
        || raw["title"].as_str().is_none()
        || text(&raw["links"]["html"], "href").is_none()
        || text(&raw["source"]["branch"], "name").is_none()
        || text(&raw["destination"]["branch"], "name").is_none()
        || raw["created_on"].as_str().is_none()
        || raw["updated_on"].as_str().is_none()
    {
        return Err(error("decodePullRequest", "Invalid pull request."));
    }
    let mut value = json!({"number":raw["id"],"title":raw["title"],"url":raw["links"]["html"]["href"],"author":actor(&raw["author"]),"headBranch":raw["source"]["branch"]["name"],"baseBranch":raw["destination"]["branch"]["name"],"state":state(raw),"isDraft":raw["draft"]==true,"mergeability":"unknown","additions":0,"deletions":0,"createdAt":date(&raw["created_on"]),"updatedAt":date(&raw["updated_on"]),"reviewRequestLogins":rows(&raw["reviewers"]).map(actor).filter_map(|a|a["login"].as_str().map(str::to_string)).collect::<Vec<_>>(),"labels":[]});
    if let Some(repo) = text(&raw["source"]["repository"], "full_name") {
        value["headRepositoryNameWithOwner"] = json!(repo);
    }
    if let Some(sha) = text(&raw["source"]["commit"], "hash") {
        value["headSha"] = json!(sha);
    }
    Ok(value)
}
pub(super) fn comment(raw: &Value) -> Option<Value> {
    if raw["id"].as_i64().is_none()
        || raw["created_on"].as_str().is_none()
        || raw["deleted"] == true
        || raw["pending"] == true
        || text(&raw["content"], "raw").is_none()
    {
        return None;
    }
    Some(
        json!({"id":raw["id"].to_string(),"kind":if text(&raw["inline"],"path").is_some(){"review-comment"}else{"issue-comment"},"author":actor(&raw["user"]),"body":raw["content"]["raw"],"createdAt":date(&raw["created_on"]),"url":text(&raw["links"]["html"],"href"),"path":text(&raw["inline"],"path"),"reviewState":null}),
    )
}
pub(super) fn threads(entries: &[Value]) -> Vec<Value> {
    let by_id: BTreeMap<_, _> = entries
        .iter()
        .filter_map(|e| Some((e["id"].as_i64()?, e)))
        .collect();
    let mut groups: BTreeMap<i64, (&Value, Vec<&Value>)> = BTreeMap::new();
    for entry in entries {
        let mut root = entry;
        for _ in 0..by_id.len() {
            let Some(parent) = root["parent"]["id"].as_i64().and_then(|id| by_id.get(&id)) else {
                break;
            };
            root = parent;
        }
        if let Some(id) = root["id"].as_i64()
            && text(&root["inline"], "path").is_some()
        {
            groups.entry(id).or_insert((root, vec![])).1.push(entry);
        }
    }
    groups.into_iter().map(|(id,(root,mut notes))|{notes.sort_by_key(|n|text(n,"created_on"));let inline=&root["inline"];let right=inline["to"].is_number();json!({"id":id.to_string(),"path":inline["path"],"line":inline[if right{"to"}else{"from"}].as_u64().filter(|n|*n>0),"side":if right{"right"}else{"left"},"isResolved":!root["resolution"].is_null(),"isOutdated":inline["outdated"]==true,"comments":notes.into_iter().filter_map(|n|comment(n).map(|mut c|{if let Some(o)=c.as_object_mut(){for key in ["kind","path","reviewState"]{o.remove(key);}}c})).collect::<Vec<_>>()})}).collect()
}
pub(super) fn reviews(raw: &Value) -> Vec<Value> {
    rows(&raw["participants"]).filter_map(|p|{let user=actor(&p["user"]);let login=user["login"].as_str()?;let at=text(p,"participated_on")?;let status=text(p,"state").or_else(||(p["approved"]==true).then_some("approved"))?;Some(json!({"id":format!("{}:{login}",raw["id"]),"kind":"review","author":user,"body":"","createdAt":date(&json!(at)),"url":null,"path":null,"reviewState":status}))}).collect()
}
pub(super) fn commit(raw: &Value) -> Option<Value> {
    let oid = text(raw, "hash")?;
    let at = text(raw, "date")?;
    let linked = actor(&raw["author"]["user"]);
    let author = if linked.is_null() {
        text(&raw["author"], "raw")
            .map(|raw| json!({"login":raw,"name":raw,"avatarUrl":null}))
            .unwrap_or(Value::Null)
    } else {
        linked
    };
    Some(
        json!({"oid":oid,"messageHeadline":raw["message"].as_str().unwrap_or("").lines().next().unwrap_or(""),"committedDate":date(&json!(at)),"authors":if author.is_null(){vec![]}else{vec![author]}}),
    )
}
pub(super) fn check(raw: &Value) -> Option<Value> {
    let name = text(raw, "name").or_else(|| text(raw, "key"))?;
    let status = match text(raw, "state").unwrap_or("").to_uppercase().as_str() {
        "SUCCESSFUL" => "success",
        "FAILED" => "failure",
        "STOPPED" => "cancelled",
        "INPROGRESS" => "pending",
        _ => "neutral",
    };
    Some(
        json!({"name":name,"status":status,"description":text(raw,"description"),"url":text(raw,"url")}),
    )
}
pub(super) fn checkout(raw: &Value) -> Result<ChangeRequest> {
    let listing = listing(raw)?;
    let source = text(&raw["source"]["repository"], "full_name");
    let target = text(&raw["destination"]["repository"], "full_name");
    let mut value = json!({"provider":"bitbucket","number":raw["id"],"title":raw["title"],"url":listing["url"],"baseRefName":listing["baseBranch"],"headRefName":listing["headBranch"],"state":listing["state"],"updatedAt":{"_tag":"Some","value":listing["updatedAt"]}});
    if let Some(sha) = text(&raw["source"]["commit"], "hash") {
        value["headSha"] = json!(sha);
    }
    if raw["draft"] == true {
        value["isDraft"] = json!(true);
    }
    if let Some(source) = source {
        value["headRepositoryNameWithOwner"] = json!(source);
        value["headRepositoryOwnerLogin"] = json!(source.split('/').next());
    }
    if let (Some(source), Some(target)) = (source, target) {
        value["isCrossRepository"] = json!(!source.eq_ignore_ascii_case(target));
    }
    parse(value, "checkout")
}
pub(super) fn urls(raw: &Value) -> Result<SourceControlRepositoryCloneUrls> {
    let clones = &raw["links"]["clone"];
    let find = |kind: &str| {
        rows(clones)
            .find(|r| r["name"] == kind)
            .and_then(|r| text(r, "href"))
    };
    parse(
        json!({"nameWithOwner":raw["full_name"],"url":find("https"),"sshUrl":find("ssh")}),
        "cloneUrls",
    )
}
