use super::*;

fn require(ok: bool, operation: &str, detail: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(operation, detail))
    }
}
fn nonempty(body: &str, operation: &str) -> Result<()> {
    require(
        !body.trim().is_empty(),
        operation,
        "A comment cannot be empty.",
    )
}

impl PullRequestService {
    async fn finish_mutation(&self, ctx: &Context, result: Result<()>) -> Result<()> {
        self.0.persistent.invalidate(&ctx.scope()).await;
        result?;
        self.bump_scope(&ctx.scope());
        self.0.list_epoch.fetch_add(1, Ordering::AcqRel);
        self.0.refreshes.send_modify(|v| *v = v.saturating_add(1));
        Ok(())
    }
    pub async fn comment(&self, input: PullRequestCommentInput) -> Result<()> {
        nonempty(&input.body, "comment")?;
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host.capabilities().comment,
            "comment",
            "This host cannot post a comment on a change request.",
        )?;
        let permissions = self
            .protect(&ctx, true, ctx.host.permissions(&ctx, false))
            .await?;
        require(
            permissions.comment,
            "comment",
            "You need write access on this repository to comment on a change request.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.comment(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn update(&self, input: PullRequestUpdateInput) -> Result<()> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        let edit = ctx
            .host
            .capabilities()
            .edit
            .as_ref()
            .and_then(Option::as_ref);
        require(
            edit.is_some_and(|e| e.change_request),
            "update",
            "This host cannot rewrite a change request.",
        )?;
        require(
            input
                .maintainer_can_modify
                .as_ref()
                .and_then(Option::as_ref)
                .is_none()
                || edit.is_some_and(|e| e.maintainer_edits == Some(Some(true))),
            "update",
            "This host does not support maintainer edits.",
        )?;
        require(
            input.title.as_ref().and_then(Option::as_ref).is_some()
                || input.body.as_ref().and_then(Option::as_ref).is_some()
                || input
                    .maintainer_can_modify
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some(),
            "update",
            "Nothing was changed.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.update(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn update_comment(&self, input: PullRequestCommentUpdateInput) -> Result<()> {
        nonempty(&input.body, "updateComment")?;
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host
                .capabilities()
                .edit
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|e| e.comment),
            "updateComment",
            "This host cannot rewrite a comment.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.update_comment(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn submit_review(&self, input: PullRequestSubmitReviewInput) -> Result<()> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        let caps = &ctx.host.capabilities().review;
        require(
            caps.verdicts.contains(&input.verdict),
            "submitReview",
            "This host cannot submit this review verdict.",
        )?;
        require(
            input.comments.is_empty() || caps.inline_comment,
            "submitReview",
            "This host cannot comment on a line of a change request.",
        )?;
        require(
            input.verdict == PullRequestReviewVerdict::Approve
                || !input.body.trim().is_empty()
                || !input.comments.is_empty(),
            "submitReview",
            "A review needs a summary or at least one comment.",
        )?;
        let viewer = self
            .protect(&ctx, true, ctx.host.permissions(&ctx, false))
            .await?;
        require(
            viewer.verdicts.contains(&input.verdict),
            "submitReview",
            "You need write access on this repository to submit this review verdict.",
        )?;
        require(
            input.comments.is_empty() || viewer.comment,
            "submitReview",
            "You need write access on this repository to comment on a line of a change request.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.submit_review(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn reply_to_thread(&self, input: PullRequestThreadReplyInput) -> Result<()> {
        nonempty(&input.body, "replyToThread")?;
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host.capabilities().review.reply,
            "replyToThread",
            "This host cannot reply to a review conversation.",
        )?;
        let viewer = self
            .protect(&ctx, true, ctx.host.permissions(&ctx, false))
            .await?;
        require(
            viewer.comment,
            "replyToThread",
            "You need write access on this repository to reply to a review conversation.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.reply_to_thread(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn set_thread_resolution(
        &self,
        input: PullRequestThreadResolutionInput,
    ) -> Result<()> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host.capabilities().review.resolve,
            "setThreadResolution",
            "This host cannot resolve a review conversation.",
        )?;
        let viewer = self
            .protect(&ctx, true, ctx.host.permissions(&ctx, false))
            .await?;
        require(
            viewer.resolve,
            "setThreadResolution",
            "You need write access on this repository, or to have opened this change request, to resolve a review conversation.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.set_thread_resolution(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn set_reaction(&self, input: PullRequestReactionInput) -> Result<()> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host.capabilities().reactions == Some(Some(true)),
            "setReaction",
            "This host has no reactions.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.set_reaction(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn reviewer_candidates(
        &self,
        input: PullRequestReviewerCandidatesInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host.capabilities().reviewers.list_candidates,
            "reviewerCandidates",
            "This host cannot say who may review a change request.",
        )?;
        require(
            self.protect(&ctx, true, ctx.host.permissions(&ctx, false))
                .await?
                .request_reviewers,
            "reviewerCandidates",
            "You need write access on this repository to ask for a review.",
        )?;
        self.protect(&ctx, true, ctx.host.reviewer_candidates(&ctx, &input))
            .await
    }
    pub async fn request_reviewers(&self, input: PullRequestReviewerRequestInput) -> Result<()> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host.capabilities().reviewers.request,
            "requestReviewers",
            "This host cannot ask somebody for a review.",
        )?;
        require(
            self.protect(&ctx, true, ctx.host.permissions(&ctx, false))
                .await?
                .request_reviewers,
            "requestReviewers",
            "You need write access on this repository to ask for a review.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.request_reviewers(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn label_candidates(
        &self,
        input: PullRequestRef,
    ) -> Result<PullRequestLabelCandidateList> {
        let ctx = self.context(&input).await?;
        require(
            ctx.host.capabilities().labels == Some(Some(true)),
            "labelCandidates",
            "This host cannot change the labels on a change request.",
        )?;
        require(
            self.protect(&ctx, true, ctx.host.permissions(&ctx, false))
                .await?
                .labels
                != Some(Some(false)),
            "labelCandidates",
            "You need triage access on this repository to change its labels.",
        )?;
        self.protect(&ctx, true, ctx.host.label_candidates(&ctx))
            .await
    }
    pub async fn set_labels(&self, input: PullRequestLabelChangeInput) -> Result<()> {
        let ctx = self.context(&crate::convert(&input)?).await?;
        require(
            ctx.host.capabilities().labels == Some(Some(true)),
            "setLabels",
            "This host cannot change the labels on a change request.",
        )?;
        require(
            self.protect(&ctx, true, ctx.host.permissions(&ctx, false))
                .await?
                .labels
                != Some(Some(false)),
            "setLabels",
            "You need triage access on this repository to change its labels.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.set_labels(&ctx, &input))
            .await;
        self.finish_mutation(&ctx, result).await
    }
    pub async fn run_action(
        &self,
        input: PullRequestActionInput,
    ) -> Result<Option<PullRequestSummary>> {
        let mut ctx = self.context(&crate::convert(&input)?).await?;
        if input.action == PullRequestAction::DeleteBranch {
            let verified = ctx.host.verify(&ctx.project).await?;
            ctx.host = verified.host;
            ctx.credential_namespace = verified.credential_fingerprint;
            ctx.viewer = Some(verified.viewer);
        }
        let caps = ctx.host.capabilities();
        require(
            input.action != PullRequestAction::DeleteBranch
                || caps.delete_branch == Some(Some(true)),
            "runAction",
            "This host cannot delete a change request's source branch.",
        )?;
        let stack = input.stack_number.flatten();
        let update = input.update_method.clone().flatten();
        let merge = input.merge_method.clone().flatten();
        require(
            stack.is_none()
                || (caps.stack_actions == Some(Some(true))
                    && input
                        .expected_stack_heads
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some()
                    && (input.action == PullRequestAction::Merge
                        || (input.action == PullRequestAction::UpdateBranch
                            && update == Some(PullRequestUpdateMethod::Rebase)))),
            "runAction",
            "This stack action is not supported or has no expected head revision.",
        )?;
        require(
            caps.actions.contains(&input.action),
            "runAction",
            "This host cannot run this change request action.",
        )?;
        require(
            merge
                .as_ref()
                .is_none_or(|m| caps.merge_methods.contains(m)),
            "runAction",
            "This host cannot merge with this strategy.",
        )?;
        require(
            update.as_ref().is_none_or(|m| {
                caps.update_methods
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|ms| ms.contains(m))
            }),
            "runAction",
            "This host cannot update a branch with this strategy.",
        )?;
        let viewer = self
            .protect(
                &ctx,
                true,
                ctx.host
                    .permissions(&ctx, input.action == PullRequestAction::UpdateBranch),
            )
            .await?;
        let rebase = stack.is_some() && input.action == PullRequestAction::UpdateBranch;
        require(
            if rebase {
                viewer.stack_rebase == Some(Some(true))
            } else {
                viewer.actions.contains(&input.action)
            },
            "runAction",
            "You need write access on this repository to run this action.",
        )?;
        require(
            rebase
                || update.as_ref().is_none_or(|m| {
                    viewer
                        .update_methods
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|ms| ms.contains(m))
                }),
            "runAction",
            "You need write access on this repository to update its branch.",
        )?;
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = self
            .protect(&ctx, true, ctx.host.run_action(&ctx, &input))
            .await;
        if stack.is_some() {
            let _ = self.refresh_after_turn(ctx.project.id.clone(), false).await;
        }
        self.0.persistent.invalidate(&ctx.scope()).await;
        let result = result?;
        self.bump_scope(&ctx.scope());
        self.0.list_epoch.fetch_add(1, Ordering::AcqRel);
        self.0.refreshes.send_modify(|v| *v = v.saturating_add(1));
        if input.action == PullRequestAction::Merge {
            let confirmed = match result.as_ref().filter(|s| {
                s.state == PullRequestState::Merged
                    && s.merged_at.as_ref().and_then(Option::as_ref).is_some()
            }) {
                Some(s) => Some(s.clone()),
                None => match ctx.host.summary(&ctx).await {
                    Ok(s) if s.state == PullRequestState::Merged => Some(s),
                    _ => result
                        .clone()
                        .filter(|s| s.state == PullRequestState::Merged),
                },
            };
            if let Some(summary) = confirmed {
                let merged_at = summary
                    .merged_at
                    .flatten()
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| crate::guide::iso_time(self.0.clock.now_ms()));
                let _ = self.0.merges.send(MergeEvent {
                    reference: ctx.reference.clone(),
                    merged_at,
                });
            }
        }
        Ok(
            if input
                .reply_with_summary
                .as_ref()
                .and_then(Option::as_ref)
                .is_some()
                && stack.is_none()
            {
                result
            } else {
                None
            },
        )
    }
}
