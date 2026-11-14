use super::{decode as d, queries as q, *};
use std::collections::{BTreeMap, HashSet};
impl GitLab {
    async fn pages(&self, input: &PullRequestInput, suffix: &str) -> Result<(Vec<Value>, bool)> {
        let mut result = Vec::new();
        let mut truncated = false;
        for page in 1..=10 {
            let raw = self
                .api(
                    &input.repo,
                    "getMergeRequestActivity",
                    "GET",
                    &format!(
                        "{}/{suffix}{}per_page=100&page={page}",
                        mr(input)?,
                        if suffix.contains('?') { "&" } else { "?" }
                    ),
                    None,
                )
                .await?;
            let values = raw
                .as_array()
                .ok_or_else(|| error("getActivity", "Invalid conversation list."))?;
            result.extend(values.iter().cloned());
            truncated = values.len() >= 100;
            if !truncated {
                break;
            }
        }
        Ok((result, truncated))
    }
    pub(super) async fn activity(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestActivity> {
        let commits_path = format!("{}/commits?per_page=100", mr(&input)?);
        let (notes, discussions, commits, detail, awards) = tokio::join!(
            self.pages(&input, "notes?order_by=created_at&sort=asc"),
            self.pages(&input, "discussions"),
            self.api(&input.repo, "listCommits", "GET", &commits_path, None),
            self.raw_detail(&input),
            self.awards(&input)
        );
        let recover = |error: &ForgeError| {
            !matches!(
                error.reason(),
                Some(
                    FailureReason::RateLimited
                        | FailureReason::MissingTool
                        | FailureReason::Unauthenticated
                )
            )
        };
        let (notes, note_truncated) = match notes {
            Ok(v) => v,
            Err(e) if recover(&e) => (vec![], true),
            Err(e) => return Err(e),
        };
        let (discussions, discussion_truncated) = match discussions {
            Ok(v) => v,
            Err(e) if recover(&e) => (vec![], true),
            Err(e) => return Err(e),
        };
        let commits = match commits {
            Ok(v) => v,
            Err(e) if recover(&e) => json!([]),
            Err(e) => return Err(e),
        };
        let detail = match detail {
            Ok(v) => v,
            Err(e) if recover(&e) => Value::Null,
            Err(e) => return Err(e),
        };
        let awards = match awards {
            Ok(v) => v,
            Err(e) if recover(&e) => Awards::default(),
            Err(e) => return Err(e),
        };
        let url = text(&detail, "web_url");
        let decorate = |value: &mut Value| {
            let id = value["id"].as_str().unwrap_or("").to_string();
            value["url"] = json!(url.map(|url| format!("{url}#note_{id}")));
            value["reactions"] = awards.notes.get(&id).cloned().unwrap_or(json!([]));
            if let Some(edit) = awards.edits.get(&id) {
                value["editedAt"] = json!(edit);
            }
            if value["author"]["login"]
                .as_str()
                .is_some_and(|login| awards.bots.contains(&login.to_lowercase()))
            {
                value["author"]["isBot"] = json!(true);
            }
        };
        let mut threads = Vec::new();
        let mut overview = Vec::new();
        let mut finding_ids = HashSet::new();
        for discussion in discussions {
            let Some(id) = text(&discussion, "id") else {
                continue;
            };
            let notes: Vec<_> = rows(&discussion["notes"])
                .filter(|n| {
                    n["system"] != true && n["id"].is_number() && n["created_at"].is_string()
                })
                .collect();
            let Some(root) = notes.first() else {
                continue;
            };
            let position = &root["position"];
            let (path, line, side) = if position.is_null() {
                if root["resolvable"] != true {
                    continue;
                }
                if root["resolved"] != true {
                    finding_ids.insert(root["id"].to_string());
                }
                (None, None, "right")
            } else {
                match text(position, "position_type") {
                    Some("text") => {
                        if position["new_line"].is_null() {
                            (
                                text(position, "old_path"),
                                position["old_line"].as_u64().filter(|n| *n > 0),
                                "left",
                            )
                        } else {
                            (
                                text(position, "new_path"),
                                position["new_line"].as_u64().filter(|n| *n > 0),
                                "right",
                            )
                        }
                    }
                    Some("file" | "image") => (
                        text(position, "new_path").or_else(|| text(position, "old_path")),
                        None,
                        "right",
                    ),
                    _ => continue,
                }
            };
            if !position.is_null() && path.is_none() {
                continue;
            }
            let comments:Vec<_>=notes.iter().map(|note|{let mut value=json!({"id":note["id"].to_string(),"author":d::actor(&note["author"]),"body":note["body"].as_str().unwrap_or(""),"createdAt":note["created_at"],"url":null});decorate(&mut value);value}).collect();
            let refs = &detail["diff_refs"];
            let outdated = ["base_sha", "head_sha", "start_sha"]
                .iter()
                .all(|key| position[*key].is_string() && refs[*key].is_string())
                && ["base_sha", "head_sha", "start_sha"]
                    .iter()
                    .any(|key| position[*key] != refs[*key]);
            let value = json!({"id":id,"path":path,"line":line,"side":side,"isResolved":root["resolved"]==true,"isOutdated":outdated,"comments":comments});
            if path.is_some() {
                threads.push(value);
            } else {
                overview.push(value);
            }
        }
        let comments: Vec<_> = notes
            .iter()
            .filter_map(d::note)
            .map(|mut value| {
                if value["id"]
                    .as_str()
                    .is_some_and(|id| finding_ids.contains(id))
                {
                    value["kind"] = json!("review-comment");
                }
                decorate(&mut value);
                value
            })
            .collect();
        let commits:Vec<_>=rows(&commits).filter_map(|commit|{let oid=text(commit,"id")?;let date=text(commit,"committed_date").or_else(||text(commit,"created_at"))?;let author=text(commit,"author_name").or_else(||text(commit,"author_email"));let mut value=json!({"oid":oid,"messageHeadline":commit["title"].as_str().unwrap_or(""),"committedDate":date,"authors":author.map(|name|vec![json!({"login":name,"name":text(commit,"author_name"),"avatarUrl":null})]).unwrap_or_default()});if rows(&commit["parent_ids"]).count()<=1{for key in ["additions","deletions"]{if let Some(count)=commit["stats"][key].as_i64(){value[key]=json!(count.max(0));}}}Some(value)}).collect();
        parse(
            json!({"comments":comments,"commentCount":comments.len(),"commentsTruncated":note_truncated||discussion_truncated,"reviewThreads":threads,"overviewThreads":overview,"commits":commits,"reactions":awards.root}),
            "getChangeRequestActivity",
        )
    }
    async fn awards(&self, input: &PullRequestInput) -> Result<Awards> {
        let mut result = Awards::default();
        let mut after = None::<String>;
        let mut seen = HashSet::new();
        for _ in 0..10 {
            let mut vars = Self::vars(input);
            vars["cursor"] = json!(after);
            let raw = self
                .graphql(
                    &input.repo,
                    "listReactions",
                    q::AWARD_EMOJI_GRAPHQL_QUERY,
                    vars,
                )
                .await?;
            let viewer = text(&raw["currentUser"], "username");
            let request = &raw["project"]["mergeRequest"];
            if after.is_none() {
                result.root = reactions(&request["awardEmoji"], viewer);
            }
            for note in nodes(&request["notes"]) {
                if let Some(id) = text(note, "id").and_then(|id| id.rsplit('/').next()) {
                    result
                        .notes
                        .insert(id.into(), reactions(&note["awardEmoji"], viewer));
                    if let Some(edit) = text(note, "lastEditedAt") {
                        result.edits.insert(id.into(), edit.into());
                    }
                    if note["author"]["bot"] == true
                        && let Some(login) = text(&note["author"], "username")
                    {
                        result.bots.insert(login.to_lowercase());
                    }
                }
            }
            let connection = &request["notes"];
            if connection["pageInfo"]["hasNextPage"] != true {
                break;
            }
            after = text(&connection["pageInfo"], "endCursor").map(str::to_string);
            if after.is_none() || !seen.insert(after.clone()) {
                break;
            }
        }
        Ok(result)
    }
    pub(super) async fn users(&self, input: &RepositoryQueryInput) -> Result<Value> {
        let mut query = "per_page=100".to_string();
        if let Some(search) = input
            .query
            .as_ref()
            .filter(|s| s.trim().chars().count() >= 3)
        {
            query.push_str(&format!("&search={}", encode(search.trim())));
        }
        let raw = self
            .api(
                &input.repo,
                "listProjectUsers",
                "GET",
                &format!("{}/users?{query}", project(&input.repo.repository)?),
                None,
            )
            .await?;
        if !raw.is_array() {
            return Err(error("listProjectUsers", "Invalid project user list."));
        }
        Ok(raw)
    }
    pub(super) async fn project_labels(
        &self,
        repo: &ProviderRepositoryRef,
        search: Option<&str>,
    ) -> Result<Value> {
        let query = search
            .filter(|s| !s.trim().is_empty())
            .map(|s| format!("&search={}", encode(s.trim())))
            .unwrap_or_default();
        let raw = self
            .api(
                repo,
                "listProjectLabels",
                "GET",
                &format!(
                    "{}/labels?per_page=100&include_ancestor_groups=true{query}",
                    project(&repo.repository)?
                ),
                None,
            )
            .await?;
        if !raw.is_array() {
            return Err(error("listProjectLabels", "Invalid project label list."));
        }
        Ok(raw)
    }
    pub(super) async fn reviewers(
        &self,
        input: CandidateInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        let users_input = RepositoryQueryInput {
            repo: input.pr.repo.clone(),
            query: input.query.clone(),
            sigil: None,
        };
        let (raw, users) = tokio::try_join!(self.raw_detail(&input.pr), self.users(&users_input))?;
        let requested: HashSet<_> = rows(&raw["reviewers"])
            .filter_map(|v| v["id"].as_u64())
            .collect();
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        let author = text(&raw["author"], "username");
        let existing = if input.query.as_ref().is_none_or(|q| q.trim().is_empty()) {
            rows(&raw["reviewers"]).collect::<Vec<_>>()
        } else {
            vec![]
        };
        for user in existing.into_iter().chain(rows(&users)) {
            let Some(id) = user["id"].as_u64() else {
                continue;
            };
            let mut value = d::actor(user);
            if value.is_null() || text(user, "username") == author || !seen.insert(id) {
                continue;
            }
            value["id"] = json!(id.to_string());
            value["kind"] = json!("user");
            value["isRequested"] = json!(requested.contains(&id));
            candidates.push(value);
        }
        parse(
            json!({"candidates":candidates,"truncated":rows(&users).count()>=100}),
            "listReviewerCandidates",
        )
    }
    pub(super) async fn label_candidates(
        &self,
        input: PullRequestInput,
    ) -> Result<PullRequestLabelCandidateList> {
        let (raw, labels) = tokio::try_join!(
            self.raw_detail(&input),
            self.project_labels(&input.repo, None)
        )?;
        let applied: HashSet<_> = rows(&raw["labels"]).filter_map(Value::as_str).collect();
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for label in rows(&labels) {
            let Some(name) = text(label, "name") else {
                continue;
            };
            if label["archived"] == true && !applied.contains(name) {
                continue;
            }
            seen.insert(name);
            candidates.push(json!({"name":name,"color":text(label,"color"),"description":text(label,"description"),"isApplied":applied.contains(name)}));
        }
        for name in applied {
            if !seen.contains(name) {
                candidates
                    .push(json!({"name":name,"color":null,"description":null,"isApplied":true}));
            }
        }
        parse(
            json!({"candidates":candidates,"truncated":rows(&labels).count()>=100}),
            "listLabelCandidates",
        )
    }
    pub(super) async fn references(
        &self,
        input: RepositoryQueryInput,
    ) -> Result<ProviderFilterOptionPage<PullRequestReferenceCandidate>> {
        let search = input.query.as_deref().unwrap_or("").trim();
        let numeric = !search.is_empty() && search.bytes().all(|b| b.is_ascii_digit());
        let mut items = Vec::new();
        let mut truncated = false;
        for kind in ["issue", "change-request"] {
            if input.sigil.as_ref().is_some_and(|s| {
                serde_json::to_value(s).ok() != Some(json!(if kind == "issue" { "#" } else { "!" }))
            }) {
                continue;
            }
            for exact in if numeric {
                vec![true, false]
            } else {
                vec![false]
            } {
                let query = if exact {
                    format!("iids[]={}", encode(search))
                } else {
                    format!(
                        "order_by=updated_at&sort=desc{}",
                        if !numeric && search.chars().count() >= 3 {
                            format!("&search={}&in=title", encode(search))
                        } else {
                            String::new()
                        }
                    )
                };
                let read = self
                    .api(
                        &input.repo,
                        "searchProjectReferences",
                        "GET",
                        &format!(
                            "{}/{}?{query}&per_page=20",
                            project(&input.repo.repository)?,
                            if kind == "issue" {
                                "issues"
                            } else {
                                "merge_requests"
                            }
                        ),
                        None,
                    )
                    .await;
                let raw = match read {
                    Ok(raw) => raw,
                    Err(e)
                        if !matches!(
                            e.reason(),
                            Some(
                                FailureReason::MissingTool
                                    | FailureReason::Unauthenticated
                                    | FailureReason::RateLimited
                            )
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                truncated |= rows(&raw).count() >= 20;
                for row in rows(&raw) {
                    if let Some(number) = row["iid"].as_u64().filter(|n| *n > 0)
                        && row["title"].is_string()
                    {
                        items.push(json!({"reference":format!("{}{number}",if kind=="issue"{"#"}else{"!"}),"kind":kind,"number":number,"title":row["title"],"state":d::state(row)}));
                    }
                }
            }
        }
        parse(
            json!({"items":items,"truncated":truncated}),
            "searchRepositoryReferences",
        )
    }
}
#[derive(Default)]
struct Awards {
    root: Value,
    notes: BTreeMap<String, Value>,
    edits: BTreeMap<String, String>,
    bots: HashSet<String>,
}
fn reactions(connection: &Value, viewer: Option<&str>) -> Value {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for award in nodes(connection) {
        if let (Some(name), Some(login)) = (text(award, "name"), text(&award["user"], "username")) {
            groups.entry(name).or_default().push(login);
        }
    }
    json!(groups.into_iter().filter_map(|(name,logins)|{let content=match name{"thumbsup"=>"thumbs-up","thumbsdown"=>"thumbs-down","laughing"=>"laugh","tada"=>"hooray","confused"=>"confused","heart"=>"heart","rocket"=>"rocket","eyes"=>"eyes",_=>return None};Some(json!({"content":content,"count":logins.len(),"actors":logins.iter().filter(|login|viewer.is_none_or(|viewer|!login.eq_ignore_ascii_case(viewer))).collect::<Vec<_>>(),"viewerHasReacted":logins.iter().any(|login|viewer.is_some_and(|v|login.eq_ignore_ascii_case(v)))}))}).collect::<Vec<_>>())
}
