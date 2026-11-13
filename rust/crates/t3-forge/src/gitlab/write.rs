use super::{decode as d, queries as q, *};
use crate::Forge;
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;

pub(super) struct WriteGuard {
    generation: Arc<std::sync::atomic::AtomicU64>,
}
impl WriteGuard {
    pub(super) fn new(forge: &GitLab) -> Self {
        forge
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self {
            generation: forge.generation.clone(),
        }
    }
}
impl Drop for WriteGuard {
    fn drop(&mut self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
}
struct BranchCleanup {
    forge: GitLab,
    repo: ProviderRepositoryRef,
    path: String,
    armed: bool,
}
impl Drop for BranchCleanup {
    fn drop(&mut self) {
        if self.armed {
            let mut forge = self.forge.clone();
            forge.stop = t3_process::CancellationToken::new();
            let repo = self.repo.clone();
            let path = self.path.clone();
            self.forge.tasks.spawn(async move {
                let _ = forge
                    .api(&repo, "revertCleanup", "DELETE", &path, None)
                    .await;
            });
        }
    }
}
impl GitLab {
    pub(super) async fn action(
        &self,
        input: ActionInput,
    ) -> Result<Option<ProviderChangeRequestSummary>> {
        let _write = WriteGuard::new(self);
        let path = mr(&input.pr)?;
        if input.stack_number.is_some() {
            return Err(ForgeError::unsupported(&self.kind(), "stack_action"));
        }
        if input.action == PullRequestAction::DeleteBranch {
            self.delete_source(&input.pr).await?;
            return Ok(None);
        }
        if input.action == PullRequestAction::Revert {
            self.revert(&input.pr).await?;
            return Ok(None);
        }
        if input.action == PullRequestAction::DisableAutoMerge {
            let raw = self
                .api(
                    &input.pr.repo,
                    "disableAutoMerge",
                    "POST",
                    &format!("{path}/cancel_merge_when_pipeline_succeeds"),
                    None,
                )
                .await?;
            return match d::listing(&raw) {
                Ok(mut v) => {
                    for field in ["additions", "deletions"] {
                        v.as_object_mut().map(|o| o.remove(field));
                    }
                    v["headSha"] = json!(text(&raw, "sha"));
                    v["autoMergeEnabled"] = json!(
                        raw["auto_merge_enabled"] == true
                            || raw["merge_when_pipeline_succeeds"] == true
                    );
                    parse(v, "disableAutoMerge").map(Some)
                }
                Err(_) => Ok(None),
            };
        }
        let (subcommand, mut flags) = match input.action {
            PullRequestAction::Merge => {
                ("merge", vec!["--auto-merge=false".into(), "--yes".into()])
            }
            PullRequestAction::EnableAutoMerge => {
                ("merge", vec!["--auto-merge=true".into(), "--yes".into()])
            }
            PullRequestAction::Ready => ("update", vec!["--ready".into()]),
            PullRequestAction::Draft => ("update", vec!["--draft".into()]),
            PullRequestAction::Close => ("close", vec![]),
            PullRequestAction::Reopen => ("reopen", vec![]),
            PullRequestAction::UpdateBranch => ("rebase", vec![]),
            _ => return Err(ForgeError::unsupported(&self.kind(), "action")),
        };
        if matches!(
            input.action,
            PullRequestAction::Merge | PullRequestAction::EnableAutoMerge
        ) {
            if input.merge_method == Some(PullRequestMergeMethod::Rebase) {
                self.mr_command(&input.pr, "rebase", vec![]).await?;
            }
            if input.merge_method == Some(PullRequestMergeMethod::Squash) {
                flags.push("--squash".into());
            }
            let refs = self.refs(&input.pr).await?;
            flags.push(format!(
                "--sha={}",
                text(&refs, "head_sha").ok_or_else(|| error("merge", "Missing head revision."))?
            ));
            if input.remove_agent_credits_on_merge {
                let messages = self
                    .graphql(
                        &input.pr.repo,
                        "getMergeMessages",
                        q::MERGE_MESSAGES_GRAPHQL_QUERY,
                        Self::vars(&input.pr),
                    )
                    .await?;
                let raw = &messages["project"]["mergeRequest"];
                for (field, flag) in [
                    ("defaultMergeCommitMessage", "--message"),
                    ("defaultSquashCommitMessage", "--squash-message"),
                ] {
                    if field == "defaultSquashCommitMessage"
                        && input.merge_method != Some(PullRequestMergeMethod::Squash)
                        && raw["squashOnMerge"] != true
                    {
                        continue;
                    }
                    if let Some(message) = raw[field].as_str() {
                        let clean = crate::github::merge_message::remove_agent_credits(message);
                        let clean = if clean.trim().is_empty() {
                            text(raw, "title").unwrap_or(message).to_string()
                        } else {
                            clean
                        };
                        if clean != message {
                            if flag == "--squash-message" && !flags.iter().any(|f| f == "--squash")
                            {
                                flags.push("--squash".into());
                            }
                            flags.push(format!("{flag}={clean}"));
                        }
                    }
                }
            }
        }
        self.mr_command(&input.pr, subcommand, flags).await?;
        Ok(None)
    }
    async fn mr_command(
        &self,
        input: &PullRequestInput,
        subcommand: &str,
        flags: Vec<String>,
    ) -> Result<()> {
        mr(input)?;
        let mut args = vec![
            "mr".into(),
            subcommand.into(),
            format!("--repo={}", input.repo.repository),
        ];
        args.extend(flags);
        args.extend(["--".into(), input.number.to_string()]);
        self.execute(
            Path::new(&input.repo.cwd),
            Some(&input.repo.host),
            "runMergeRequestAction",
            args,
            None,
            8 * 1024 * 1024,
        )
        .await?;
        Ok(())
    }
    async fn delete_source(&self, input: &PullRequestInput) -> Result<()> {
        let value = self
            .graphql(
                &input.repo,
                "getSourceBranch",
                q::SOURCE_BRANCH_GRAPHQL_QUERY,
                Self::vars(input),
            )
            .await?;
        let raw = &value["project"]["mergeRequest"];
        if raw.is_null()
            || raw["state"] == "opened"
            || raw["userPermissions"]["removeSourceBranch"] != true
        {
            return Err(error(
                "deleteBranch",
                "The source branch cannot be deleted.",
            ));
        }
        let branch = text(raw, "sourceBranch")
            .ok_or_else(|| error("deleteBranch", "The source branch is unavailable."))?;
        let source = text(&raw["sourceProject"], "fullPath")
            .ok_or_else(|| error("deleteBranch", "The source project is unavailable."))?;
        let expected = text(raw, "diffHeadSha")
            .filter(|s| sha(s))
            .ok_or_else(|| error("deleteBranch", "The source revision is unavailable."))?;
        let path = format!(
            "{}/repository/branches/{}",
            project(source)?,
            encode(branch)
        );
        let branch = self
            .api(&input.repo, "getSourceBranch", "GET", &path, None)
            .await?;
        if branch["default"] == true || branch["commit"]["id"] != expected {
            return Err(error(
                "deleteBranch",
                "The source branch is protected or changed after the merge request.",
            ));
        }
        let mut args = vec!["api".into(), "--method=DELETE".into()];
        if let Some(date) = text(&branch["commit"], "authored_date")
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        {
            args.push(format!(
                "--header=If-Unmodified-Since: {}",
                date.with_timezone(&chrono::Utc)
                    .format("%a, %d %b %Y %H:%M:%S GMT")
            ));
        }
        args.extend(["--".into(), path]);
        self.execute(
            Path::new(&input.repo.cwd),
            Some(&input.repo.host),
            "deleteSourceBranch",
            args,
            None,
            8 * 1024 * 1024,
        )
        .await?;
        Ok(())
    }
    async fn revert(&self, input: &PullRequestInput) -> Result<()> {
        let raw = self
            .api(&input.repo, "revertMergeRequest", "GET", &mr(input)?, None)
            .await?;
        if raw["state"] != "merged" {
            return Err(error(
                "revert",
                "Only a merged merge request can be reverted.",
            ));
        }
        let mut revision = text(&raw, "merge_commit_sha")
            .or_else(|| text(&raw, "squash_commit_sha"))
            .map(str::to_string);
        if revision.is_none() {
            let commits = self
                .api(
                    &input.repo,
                    "revertMergeRequest",
                    "GET",
                    &format!("{}/commits?per_page=2", mr(input)?),
                    None,
                )
                .await?;
            if let Some(rows) = commits.as_array().filter(|r| r.len() == 1) {
                revision = text(&rows[0], "id").map(str::to_string);
            }
        }
        let revision = revision.filter(|s| sha(s)).ok_or_else(|| {
            error(
                "revert",
                "The merge request landed as several commits and cannot be reverted as one.",
            )
        })?;
        let target = text(&raw, "target_branch")
            .ok_or_else(|| error("revert", "The target branch is unavailable."))?;
        let branch = format!("revert-{}", &revision[..8.min(revision.len())]);
        let root = project(&input.repo.repository)?;
        self.api(
            &input.repo,
            "revertMergeRequest",
            "POST",
            &format!("{root}/repository/branches"),
            Some(json!({"branch":branch,"ref":target})),
        )
        .await?;
        let mut cleanup = BranchCleanup {
            forge: self.clone(),
            repo: input.repo.clone(),
            path: format!("{root}/repository/branches/{}", encode(&branch)),
            armed: true,
        };
        self.api(
            &input.repo,
            "revertMergeRequest",
            "POST",
            &format!("{root}/repository/commits/{revision}/revert"),
            Some(json!({"branch":branch})),
        )
        .await?;
        self.api(&input.repo,"revertMergeRequest","POST",&format!("{root}/merge_requests"),Some(json!({"source_branch":branch,"target_branch":target,"title":format!("Revert \"{}\"",raw["title"].as_str().unwrap_or("")),"description":format!("This reverts merge request !{}",input.number),"remove_source_branch":true}))).await?;
        cleanup.armed = false;
        Ok(())
    }
    pub(super) async fn update(&self, input: UpdateInput) -> Result<()> {
        let _write = WriteGuard::new(self);
        let mut body = json!({});
        if let Some(title) = input.title {
            body["title"] = json!(title);
        }
        if let Some(body_text) = input.body {
            body["description"] = json!(body_text);
        }
        if let Some(allow) = input.maintainer_can_modify {
            body["allow_collaboration"] = json!(allow);
        }
        self.api(
            &input.pr.repo,
            "updateMergeRequest",
            "PUT",
            &mr(&input.pr)?,
            Some(body),
        )
        .await?;
        Ok(())
    }
    pub(super) async fn set_reviewers(&self, input: ReviewerRequestInput) -> Result<()> {
        let _write = WriteGuard::new(self);
        let raw = self.raw_detail(&input.pr).await?;
        let mut ids: BTreeSet<_> = rows(&raw["reviewers"])
            .filter_map(|r| r["id"].as_u64())
            .collect();
        for reviewer in input.reviewers {
            let value = serde_json::to_value(reviewer)
                .map_err(|_| error("setReviewers", "Invalid reviewer."))?;
            if let Some(id) = value["id"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|id| *id > 0 && *id <= 9_007_199_254_740_991)
            {
                if input.requested {
                    ids.insert(id);
                } else {
                    ids.remove(&id);
                }
            }
        }
        self.api(
            &input.pr.repo,
            "setReviewerRequest",
            "PUT",
            &mr(&input.pr)?,
            Some(json!({"reviewer_ids":ids})),
        )
        .await?;
        Ok(())
    }
    pub(super) async fn set_project_labels(&self, input: LabelsInput) -> Result<()> {
        let _write = WriteGuard::new(self);
        if input.labels.iter().any(|l| l.contains(',')) {
            return Err(error(
                "setLabels",
                "GitLab label names cannot contain commas.",
            ));
        }
        if input.applied {
            for label in &input.labels {
                let raw = self
                    .api(
                        &input.pr.repo,
                        "lookupLabel",
                        "GET",
                        &format!(
                            "{}/labels/{}?include_ancestor_groups=true",
                            project(&input.pr.repo.repository)?,
                            encode(label)
                        ),
                        None,
                    )
                    .await?;
                if text(&raw, "name") != Some(label.as_str()) {
                    return Err(error(
                        "setLabels",
                        "The label does not exist in this project.",
                    ));
                }
            }
        }
        let mut body = json!({});
        body[if input.applied {
            "add_labels"
        } else {
            "remove_labels"
        }] = json!(input.labels.join(","));
        self.api(
            &input.pr.repo,
            "setLabels",
            "PUT",
            &mr(&input.pr)?,
            Some(body),
        )
        .await?;
        Ok(())
    }
    pub(super) async fn react(&self, input: ReactionInput) -> Result<()> {
        let _write = WriteGuard::new(self);
        let mut path = mr(&input.pr)?;
        if let Some(note) = input.subject_id {
            path.push_str(&format!("/notes/{}", encode(&note)));
        }
        path.push_str("/award_emoji");
        let name = d::award_name(&input.content);
        if input.reacted {
            self.api(
                &input.pr.repo,
                "setReaction",
                "POST",
                &format!("{path}?name={}", encode(name)),
                None,
            )
            .await?;
            return Ok(());
        }
        let viewer = self
            .get_viewer(ViewerInput {
                cwd: input.pr.repo.cwd.clone().into(),
                host: Some(input.pr.repo.host.clone()),
            })
            .await?;
        let listed = self
            .api(&input.pr.repo, "setReaction", "GET", &path, None)
            .await?;
        if let Some(id) = rows(&listed)
            .find(|v| {
                v["name"] == name
                    && text(&v["user"], "username").is_some_and(|s| s.eq_ignore_ascii_case(&viewer))
            })
            .and_then(|v| v["id"].as_u64())
        {
            self.api(
                &input.pr.repo,
                "setReaction",
                "DELETE",
                &format!("{path}/{id}"),
                None,
            )
            .await?;
        }
        Ok(())
    }
    async fn mutate_graph(&self, input: &PullRequestInput, query: &str, vars: Value) -> Result<()> {
        let raw = self
            .graphql(&input.repo, "submitReview", query, vars)
            .await?;
        if raw
            .as_object()
            .is_none_or(|o| o.values().any(|v| rows(&v["errors"]).next().is_some()))
        {
            return Err(error("submitReview", "GitLab refused this review."));
        }
        Ok(())
    }
    pub(super) async fn review(&self, input: ReviewInput) -> Result<()> {
        let _write = WriteGuard::new(self);
        let target = mr(&input.pr)?;
        if input.verdict == PullRequestReviewVerdict::RequestChanges {
            let viewer = self
                .get_viewer(ViewerInput {
                    cwd: input.pr.repo.cwd.clone().into(),
                    host: Some(input.pr.repo.host.clone()),
                })
                .await?;
            self.mutate_graph(&input.pr,q::SET_SELF_AS_REVIEWER_MUTATION,json!({"projectPath":input.pr.repo.repository,"iid":input.pr.number.to_string(),"reviewers":[viewer]})).await?;
            self.mutate_graph(
                &input.pr,
                q::REQUEST_CHANGES_MUTATION,
                json!({"projectPath":input.pr.repo.repository,"iid":input.pr.number.to_string()}),
            )
            .await?;
        }
        if !input.comments.is_empty() {
            let refs = self.refs(&input.pr).await?;
            let comments: Vec<_> = input
                .comments
                .into_iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| error("submitReview", "Invalid review comment."))?;
            let headers: Vec<_> = comments
                .iter()
                .filter(|c| {
                    c["startPosition"].is_object()
                        && (c["startPosition"]["kind"] != "context"
                            || c["position"]["kind"] != "context")
                })
                .filter_map(|c| {
                    c["path"].as_str().map(|path| {
                        format!(
                            "diff --git {} {}\n",
                            quote_patch_path(&format!("a/{}", text(c, "oldPath").unwrap_or(path))),
                            quote_patch_path(&format!("b/{path}"))
                        )
                    })
                })
                .collect();
            let mut patch = String::new();
            let mut cursor = None;
            for _ in 0..1000 {
                if headers
                    .iter()
                    .all(|h| format!("\n{patch}").contains(&format!("\n{h}")))
                {
                    break;
                }
                match self
                    .diff(DiffInput {
                        pr: input.pr.clone(),
                        cursor,
                        commit: None,
                    })
                    .await
                {
                    Ok(slice) => {
                        patch.push_str(&slice.patch);
                        cursor = slice.next_cursor;
                        if cursor.is_none() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            for comment in comments {
                let path = text(&comment, "path")
                    .ok_or_else(|| error("submitReview", "Missing review path."))?;
                let old = text(&comment, "oldPath").unwrap_or(path);
                let mut position = json!({"base_sha":refs["base_sha"],"head_sha":refs["head_sha"],"start_sha":refs["start_sha"],"position_type":"text","old_path":old,"new_path":path});
                position
                    .as_object_mut()
                    .ok_or_else(|| error("submitReview", "Invalid review position."))?
                    .extend(position_lines(&comment["position"]));
                if comment["startPosition"].is_object() {
                    let hash = format!("{:x}", Sha1::digest(path.as_bytes()));
                    let header = format!(
                        "diff --git {} {}\n",
                        quote_patch_path(&format!("a/{old}")),
                        quote_patch_path(&format!("b/{path}"))
                    );
                    let start = range_end(&hash, &patch, &header, &comment["startPosition"]);
                    let end = range_end(&hash, &patch, &header, &comment["position"]);
                    if start["line_code"] != end["line_code"] {
                        position["line_range"] = json!({"start":start,"end":end});
                    }
                }
                self.api(
                    &input.pr.repo,
                    "submitReview",
                    "POST",
                    &format!("{target}/discussions"),
                    Some(json!({"body":comment["body"],"position":position})),
                )
                .await?;
            }
        }
        if !input.body.trim().is_empty() {
            self.api(
                &input.pr.repo,
                "submitReview",
                "POST",
                &format!("{target}/notes"),
                Some(json!({"body":input.body})),
            )
            .await?;
        }
        if input.verdict == PullRequestReviewVerdict::Approve {
            self.api(
                &input.pr.repo,
                "submitReview",
                "POST",
                &format!("{target}/approve"),
                None,
            )
            .await?;
        }
        Ok(())
    }
}
fn position_lines(position: &Value) -> serde_json::Map<String, Value> {
    let mut lines = serde_json::Map::new();
    match position["kind"].as_str() {
        Some("added") => {
            lines.insert("new_line".into(), position["newLine"].clone());
        }
        Some("deleted") => {
            lines.insert("old_line".into(), position["oldLine"].clone());
        }
        _ => {
            lines.insert("old_line".into(), position["oldLine"].clone());
            lines.insert("new_line".into(), position["newLine"].clone());
        }
    }
    lines
}
fn range_end(hash: &str, patch: &str, header: &str, position: &Value) -> Value {
    let lines = position_lines(position);
    let mut old = lines.get("old_line").and_then(Value::as_u64).unwrap_or(0);
    let mut new = lines.get("new_line").and_then(Value::as_u64).unwrap_or(0);
    if position["kind"] != "context"
        && let Some(start) = format!("\n{patch}").find(&format!("\n{header}"))
    {
        let section = &patch[start..];
        let end = section.find("\ndiff --git ").unwrap_or(section.len());
        let mut counters = None;
        let mut old_counter = 0;
        let mut new_counter = 0;
        for line in section[..end].lines() {
            if let Some(hunk) = line.strip_prefix("@@ -") {
                let mut parts = hunk.split(' ');
                old_counter = parts
                    .next()
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                new_counter = parts
                    .next()
                    .and_then(|s| s.trim_start_matches('+').split(',').next())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
            } else if line.starts_with('+') && !line.starts_with("+++") {
                if position["kind"] == "added" && new_counter == new {
                    counters = Some((old_counter, new_counter));
                    break;
                }
                new_counter += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                if position["kind"] == "deleted" && old_counter == old {
                    counters = Some((old_counter, new_counter));
                    break;
                }
                old_counter += 1;
            } else if line.starts_with(' ') {
                old_counter += 1;
                new_counter += 1;
            }
        }
        if let Some((a, b)) = counters {
            old = a;
            new = b;
        }
    }
    let mut value = json!({"line_code":format!("{hash}_{old}_{new}"),"type":match position["kind"].as_str(){Some("added")=>Some("new"),Some("deleted")=>Some("old"),_=>None}});
    if let Some(object) = value.as_object_mut() {
        object.extend(lines);
    }
    value
}
