use super::{decode as d, *};
impl GitCafe {
    pub(super) async fn action(
        &self,
        input: ActionInput,
    ) -> Result<Option<ProviderChangeRequestSummary>> {
        if let Some(number) = input.stack_number {
            self.stack_action(&input, number).await?;
            return Ok(None);
        }
        if input.action == PullRequestAction::Merge {
            let pull = self.raw(&input.pr, "merge").await?;
            if pull["state"] == "merged" {
                return Ok(None);
            }
            if pull["mergeRoute"] == "unsupported" {
                return Err(error(
                    "merge",
                    "This pull request has no supported merge route.",
                ));
            }
            let base = if pull["mergeRoute"] == "provider" {
                pull["observedBaseOid"].clone()
            } else {
                self.api(
                    &input.pr.repo.host,
                    "merge",
                    Method::GET,
                    &format!(
                        "{}/commit?ref={}",
                        repository(&input.pr.repo.repository)?,
                        encode(&format!(
                            "refs/heads/{}",
                            pull["targetBranch"].as_str().unwrap_or("")
                        ))
                    ),
                    None,
                )
                .await?["oid"]
                    .clone()
            };
            if pull["headOid"].is_null() || base.is_null() {
                return Err(error("merge", "Missing immutable merge revisions."));
            }
            let strategy =
                serde_json::to_value(input.merge_method.unwrap_or(PullRequestMergeMethod::Merge))
                    .map_err(|_| error("merge", "Invalid merge strategy."))?;
            let result=self.api(&input.pr.repo.host,"merge",Method::POST,&format!("{}/merge",pull_path(&input.pr)?),Some(json!({"requestId":request_id(),"expectedVersion":pull["version"],"headOid":pull["headOid"],"baseOid":base,"strategy":strategy}))).await?;
            if result["id"].as_str().is_none()
                || !matches!(
                    result["state"].as_str(),
                    Some("accepted" | "completed" | "failed")
                )
            {
                return Err(error("merge", "Invalid merge outcome."));
            }
            if result["state"] == "failed" {
                return Err(error("merge", "GitCafe could not merge the pull request."));
            }
            return Ok(None);
        }
        let name = match input.action {
            PullRequestAction::Ready => "ready",
            PullRequestAction::Draft => "draft",
            PullRequestAction::Close => "close",
            PullRequestAction::Reopen => "reopen",
            _ => return Err(ForgeError::unsupported(&self.kind(), "run_action")),
        };
        let pull = self.raw(&input.pr, name).await?;
        let raw = self
            .api(
                &input.pr.repo.host,
                name,
                Method::POST,
                &format!("{}/{name}", pull_path(&input.pr)?),
                Some(json!({"expectedVersion":pull["version"]})),
            )
            .await?;
        let Ok(mut value) = d::listing(&raw, &input.pr.repo) else {
            return Ok(None);
        };
        if let Some(object) = value.as_object_mut() {
            for field in ["additions", "deletions", "mergeability"] {
                object.remove(field);
            }
        }
        if let Some(head) = text(&raw, "headOid") {
            value["headSha"] = json!(head);
        }
        for field in ["closedAt", "mergedAt"] {
            if let Some(v) = raw.get(field) {
                value[field] = v.clone();
            }
        }
        parse(value, "runAction").map(Some)
    }
    async fn stack_action(&self, input: &ActionInput, number: u64) -> Result<()> {
        let land = input.action == PullRequestAction::Merge;
        if !land && input.action != PullRequestAction::UpdateBranch {
            return Err(ForgeError::unsupported(&self.kind(), "stack_action"));
        }
        let stack = self
            .stack(&input.pr)
            .await?
            .filter(|s| s["number"] == number)
            .ok_or_else(|| error("stackAction", "The stack changed. Refresh it and retry."))?;
        let mut members: Vec<_> = rows(&stack["members"]).collect();
        members.sort_by_key(|m| m["position"].as_u64());
        let through = members
            .iter()
            .position(|m| m["pullRequestNumber"] == input.pr.number)
            .ok_or_else(|| error("stackAction", "The target is no longer in the stack."))?;
        let moved: Vec<_> = members
            .into_iter()
            .enumerate()
            .filter(|(index, m)| (!land || *index <= through) && m["state"] != "merged")
            .map(|(_, m)| m)
            .collect();
        let expected = input
            .expected_stack_heads
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| error("stackAction", "Invalid reviewed heads."))?
            .ok_or_else(|| error("stackAction", "The reviewed stack heads are required."))?;
        if moved.len() != rows(&expected).count()
            || !moved.iter().all(|m| {
                text(m, "headOid").is_some()
                    && rows(&expected).any(|e| {
                        e["number"] == m["pullRequestNumber"] && e["headSha"] == m["headOid"]
                    })
            })
        {
            return Err(error(
                "stackAction",
                "The reviewed stack heads changed. Refresh it and retry.",
            ));
        }
        let mut body = json!({"expectedRevision":stack["revision"],"requestId":request_id()});
        if land {
            body["throughPullRequestNumber"] = json!(input.pr.number);
            body["strategy"] = serde_json::to_value(
                input
                    .merge_method
                    .clone()
                    .unwrap_or(PullRequestMergeMethod::Merge),
            )
            .map_err(|_| error("stackAction", "Invalid strategy."))?;
        }
        let result = self
            .api(
                &input.pr.repo.host,
                "stackAction",
                Method::POST,
                &format!(
                    "{}/pulls/stacks/{number}/{}",
                    repository(&input.pr.repo.repository)?,
                    if land { "land-through" } else { "restack" }
                ),
                Some(body),
            )
            .await?;
        if result["id"].as_str().is_none() || result["state"].as_str().is_none() {
            return Err(error("stackAction", "Invalid stack outcome."));
        }
        if matches!(
            result["state"].as_str(),
            Some("failed" | "stopped" | "cancelled" | "reconciliation_required")
        ) {
            return Err(error(
                "stackAction",
                "GitCafe did not complete the stack operation.",
            ));
        }
        Ok(())
    }
    pub(super) async fn toggle(
        &self,
        input: &PullRequestInput,
        name: &str,
        key: &str,
        ids: Vec<String>,
        on: bool,
    ) -> Result<()> {
        let path = pull_path(input)?;
        let pull = self.raw(input, "toggleList").await?;
        let (current, _) = self
            .pages(&input.repo, "toggleList", &format!("{path}/{name}"))
            .await?;
        let current: Vec<_> = current
            .iter()
            .filter_map(|v| text(v, "id").map(str::to_string))
            .collect();
        let mut next = current.clone();
        if on {
            for id in ids {
                if !next.contains(&id) {
                    next.push(id);
                }
            }
        } else {
            next.retain(|id| !ids.contains(id));
        }
        if next == current {
            return Ok(());
        }
        let mut body = json!({"expectedVersion":pull["version"]});
        body[key] = json!(next);
        self.api(
            &input.repo.host,
            "toggleList",
            Method::PUT,
            &format!("{path}/{name}"),
            Some(body),
        )
        .await?;
        Ok(())
    }
    pub(super) async fn reaction(&self, input: ReactionInput) -> Result<()> {
        let path = format!("{}/reactions/", pull_path(&input.pr)?);
        let subject = if let Some(id) = input.subject_id {
            json!({"kind":"pull_request_comment","id":id})
        } else {
            let raw = self.raw(&input.pr, "setReaction").await?;
            json!({"kind":"pull_request","id":raw["id"]})
        };
        let (reactions, _) = self.pages(&input.pr.repo, "setReaction", &path).await?;
        let content = serde_json::to_value(&input.content)
            .map_err(|_| error("setReaction", "Invalid reaction."))?;
        let mine = reactions
            .iter()
            .find(|r| {
                r["subject"] == subject
                    && r["emoji"]["kind"] == "unicode"
                    && text(&r["emoji"], "value")
                        .and_then(d::reaction)
                        .is_some_and(|s| content == s)
            })
            .and_then(|r| text(r, "viewerReactionId"));
        if input.reacted == mine.is_some() {
            return Ok(());
        }
        if input.reacted {
            self.api(&input.pr.repo.host,"setReaction",Method::POST,&path,Some(json!({"subject":subject,"emoji":{"kind":"unicode","value":d::emoji(&input.content)}}))).await?;
        } else if let Some(mine) = mine {
            self.api(
                &input.pr.repo.host,
                "setReaction",
                Method::DELETE,
                &format!("{path}{}", id(mine)?),
                None,
            )
            .await?;
        }
        Ok(())
    }
}
