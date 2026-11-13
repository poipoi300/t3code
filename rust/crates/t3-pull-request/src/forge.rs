use crate::{
    Error, FailureReason, Result,
    context::{Context, Project},
    host::*,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::pull_request::*;
use t3_forge::{Forge, forge as f};

#[derive(Clone)]
pub struct ForgeHost {
    forge: Arc<dyn Forge>,
    kind: String,
}
impl ForgeHost {
    fn error(&self, error: t3_forge::ForgeError) -> Error {
        let mut error = map_error(error);
        if let Some(requirement) = self.forge.unavailable_requirement() {
            error.requirement = match error.reason {
                FailureReason::MissingTool => Some(requirement.missing.into_boxed_str()),
                FailureReason::Unauthenticated => {
                    Some(requirement.unauthenticated.into_boxed_str())
                }
                _ => None,
            };
        }
        error
    }
    pub fn new(forge: Arc<dyn Forge>) -> Self {
        Self {
            kind: forge.kind().0.clone(),
            forge,
        }
    }
    fn input(ctx: &Context) -> f::PullRequestInput {
        f::PullRequestInput {
            repo: t3_forge::ProviderRepositoryRef {
                cwd: ctx.project.workspace_root.clone(),
                host: ctx.project.host.clone(),
                repository: ctx.project.repository.clone(),
            },
            number: ctx.reference.number,
        }
    }
    fn augment(&self, ctx: &Context, value: impl serde::Serialize) -> Result<Value> {
        let mut value = serde_json::to_value(value)
            .map_err(|_| Error::invalid("decode", "Invalid host response."))?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| Error::invalid("decode", "Invalid host response."))?;
        object.insert("provider".into(), json!(self.kind));
        object.insert("projectId".into(), json!(ctx.project.id));
        object.insert("projectTitle".into(), json!(ctx.project.title));
        object.insert("workspaceRoot".into(), json!(ctx.project.workspace_root));
        object.insert("repository".into(), json!(ctx.project.repository));
        object.insert("host".into(), json!(ctx.project.host));
        Ok(value)
    }
    fn summary_value(
        &self,
        ctx: &Context,
        value: impl serde::Serialize,
        reply: bool,
    ) -> Result<PullRequestSummary> {
        let mut value = self.augment(ctx, value)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| Error::invalid("summary", "Invalid summary."))?;
        if !reply {
            object.entry("closedAt").or_insert(Value::Null);
            object.entry("mergedAt").or_insert(Value::Null);
        }
        if let Some(head) = object
            .get("headSha")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let head = head.to_string();
            object.insert("headSha".into(), json!(head));
        }
        crate::convert(value)
    }
    fn list_entry(
        &self,
        ctx: &Context,
        item: t3_forge::ProviderChangeRequest,
        viewer: &str,
    ) -> Result<PullRequestListEntry> {
        let requested = item
            .author
            .as_ref()
            .is_none_or(|a| !a.login.as_str().eq_ignore_ascii_case(viewer))
            && item
                .review_request_logins
                .iter()
                .any(|v| v.eq_ignore_ascii_case(viewer));
        let mut value = self.augment(ctx, item)?;
        value["viewerReviewRequested"] = json!(requested);
        crate::convert(value)
    }
    pub async fn discover_branch(
        &self,
        ctx: &Context,
        branch: &str,
        context: Option<f::CheckoutContext>,
    ) -> Result<Vec<t3_contracts::source_control::ChangeRequest>> {
        self.forge
            .checkout_list_change_requests(f::CheckoutListInput {
                checkout: f::CheckoutInput {
                    cwd: ctx.project.workspace_root.clone().into(),
                    context,
                },
                source: None,
                head_selector: branch.into(),
                state: None,
                limit: Some(100),
            })
            .await
            .map_err(|error| self.error(error))
    }
    pub async fn create(
        &self,
        ctx: &Context,
        base_ref_name: String,
        head_selector: String,
        title: String,
        body: String,
    ) -> Result<()> {
        let directory = tempfile::tempdir()
            .map_err(|_| Error::invalid("create", "Could not create a pull request body file."))?;
        let body_file = directory.path().join("body.md");
        tokio::fs::write(&body_file, body)
            .await
            .map_err(|_| Error::invalid("create", "Could not write a pull request body file."))?;
        self.forge
            .create_change_request(f::CreateChangeRequestInput {
                checkout: f::CheckoutInput {
                    cwd: ctx.project.workspace_root.clone().into(),
                    context: None,
                },
                source: None,
                target: None,
                base_ref_name,
                head_selector,
                title,
                body_file,
            })
            .await
            .map_err(|error| self.error(error))
    }
}
pub fn map_error(error: t3_forge::ForgeError) -> Error {
    match error {
        t3_forge::ForgeError::Provider {
            provider,
            operation,
            reason,
            detail,
            retry_at,
            access_hint,
            ..
        } => Error {
            provider: Some(Box::new(provider)),
            requirement: None,
            operation,
            detail: t3_forge::remote::transport_safe_error_value(&detail),
            transient: reason == t3_forge::FailureReason::Failed,
            reason: match reason {
                t3_forge::FailureReason::MissingTool => FailureReason::MissingTool,
                t3_forge::FailureReason::Unauthenticated => FailureReason::Unauthenticated,
                t3_forge::FailureReason::Forbidden => FailureReason::Forbidden,
                t3_forge::FailureReason::RateLimited => FailureReason::RateLimited,
                t3_forge::FailureReason::NotFound => FailureReason::NotFound,
                t3_forge::FailureReason::Failed => FailureReason::Failed,
            },
            retry_at,
            access_hint: access_hint
                .map(|s| t3_forge::remote::transport_safe_error_value(&s).into_boxed_str()),
            list_result: None,
        },
        t3_forge::ForgeError::Unsupported {
            provider,
            operation,
        } => Error {
            provider: Some(Box::new(provider)),
            ..Error::unsupported(&operation)
        },
        _ => Error::invalid(
            "pullRequests",
            "The host could not complete this operation.",
        ),
    }
}
#[async_trait]
impl Host for ForgeHost {
    fn kind(&self) -> &str {
        &self.kind
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        self.forge.capabilities()
    }
    fn revision(&self) -> u64 {
        self.forge.settings_revision()
    }
    fn batches_reads(&self) -> bool {
        self.forge.batches_reads()
    }
    fn reads_public_host(&self, host: &str) -> bool {
        self.forge.reads_public_host_without_checkout()
            && self.forge.serves_host(host)
            && self.kind == "github"
            && host.eq_ignore_ascii_case("github.com")
    }
    async fn verify(&self, project: &Project) -> Result<VerifiedHost> {
        let verified = self
            .forge
            .with_verified_credential(f::VerifiedCredentialInput {
                cwd: project.workspace_root.clone().into(),
                host: project.host.clone(),
                repository: Some(project.repository.clone()),
            })
            .await
            .map_err(|error| self.error(error))?;
        Ok(VerifiedHost {
            account_id: verified.identity.account_id,
            viewer: verified.identity.viewer,
            credential_fingerprint: verified.identity.credential_fingerprint,
            host: Arc::new(Self::new(verified.forge)),
        })
    }
    async fn routing_identity(&self, project: &Project) -> Result<RoutingIdentity> {
        let identity = self
            .forge
            .get_routing_identity(f::ViewerInput {
                cwd: project.workspace_root.clone().into(),
                host: Some(project.host.clone()),
            })
            .await
            .map_err(|error| self.error(error))?;
        Ok(RoutingIdentity {
            account_id: identity.account_id,
            viewer: identity.viewer,
            credential_fingerprint: identity.credential_fingerprint,
        })
    }
    async fn viewer(&self, project: &Project) -> Result<String> {
        self.forge
            .get_viewer(f::ViewerInput {
                cwd: project.workspace_root.clone().into(),
                host: Some(project.host.clone()),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn changes(&self, ctx: &Context) -> Result<Changes> {
        if !self.forge.supports("change_request_events") {
            return Ok(Box::pin(futures_util::stream::empty()));
        }
        match self.forge.change_request_events(Self::input(ctx)).await {
            Ok(s) => Ok(s),
            Err(t3_forge::ForgeError::Unsupported { .. }) => {
                Ok(Box::pin(futures_util::stream::empty()))
            }
            Err(e) => Err(map_error(e)),
        }
    }
    async fn forget_reads(&self, ctx: Option<&Context>) {
        self.forge
            .forget_change_request_reads(ctx.map(|c| f::ReadScope {
                repository: c.project.repository.clone(),
                number: Some(c.reference.number),
            }))
            .await;
    }
    async fn summary(&self, ctx: &Context) -> Result<PullRequestSummary> {
        if self.forge.supports("get_change_request_summary") {
            self.summary_value(
                ctx,
                self.forge
                    .get_change_request_summary(Self::input(ctx))
                    .await
                    .map_err(|error| self.error(error))?,
                false,
            )
        } else {
            self.summary_value(
                ctx,
                self.forge
                    .get_change_request(Self::input(ctx))
                    .await
                    .map_err(|error| self.error(error))?,
                false,
            )
        }
    }
    async fn detail(&self, ctx: &Context) -> Result<PullRequestDetail> {
        let detail = self
            .forge
            .get_change_request(Self::input(ctx))
            .await
            .map_err(|error| self.error(error))?;
        let mut value = self.augment(ctx, detail)?;
        value["capabilities"] = json!(self.forge.capabilities());
        let viewer = match &ctx.viewer {
            Some(v) => Some(v.clone()),
            None => self.viewer(&ctx.project).await.ok(),
        };
        if let Some(viewer) = viewer.filter(|v| !v.trim().is_empty()) {
            value["viewer"] = json!(viewer);
        }
        crate::convert(value)
    }
    async fn preview(&self, ctx: &Context) -> Result<PullRequestPreview> {
        if self.forge.supports("get_change_request_preview") {
            crate::convert(
                self.augment(
                    ctx,
                    self.forge
                        .get_change_request_preview(Self::input(ctx))
                        .await
                        .map_err(|error| self.error(error))?,
                )?,
            )
        } else {
            crate::convert(
                self.augment(
                    ctx,
                    self.forge
                        .get_change_request(Self::input(ctx))
                        .await
                        .map_err(|error| self.error(error))?,
                )?,
            )
        }
    }
    async fn checks(&self, ctx: &Context) -> Result<Option<PullRequestChecks>> {
        if !self.forge.supports("get_change_request_checks") {
            return Ok(None);
        }
        crate::convert(
            self.forge
                .get_change_request_checks(Self::input(ctx))
                .await
                .map_err(|error| self.error(error))?,
        )
        .map(Some)
    }
    async fn watch_fingerprint(&self, ctx: &Context) -> Result<Option<Fingerprint>> {
        if !self.forge.supports("get_change_request_watch_fingerprint") {
            return Ok(None);
        }
        self.forge
            .get_change_request_watch_fingerprint(Self::input(ctx))
            .await
            .map_err(|error| self.error(error))?
            .map(crate::convert)
            .transpose()
    }
    async fn stack(
        &self,
        ctx: &Context,
        include_details: bool,
    ) -> Result<Option<PullRequestStack>> {
        if !self.forge.supports("get_change_request_stack") {
            return Ok(None);
        }
        crate::convert(
            self.forge
                .get_change_request_stack(f::StackInput {
                    pr: Self::input(ctx),
                    include_details,
                })
                .await
                .map_err(|error| self.error(error))?,
        )
    }
    async fn activity(&self, ctx: &Context) -> Result<PullRequestActivity> {
        crate::convert(
            self.forge
                .get_change_request_activity(Self::input(ctx))
                .await
                .map_err(|error| self.error(error))?,
        )
    }
    async fn permissions(
        &self,
        ctx: &Context,
        include_update_branch: bool,
    ) -> Result<PullRequestViewerPermissions> {
        self.forge
            .get_viewer_permissions(f::PermissionsInput {
                pr: Self::input(ctx),
                include_update_branch,
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn thread_comments(
        &self,
        ctx: &Context,
        input: &PullRequestThreadCommentsInput,
    ) -> Result<PullRequestThreadCommentsResult> {
        self.forge
            .get_review_thread_comments(f::ThreadCommentsInput {
                pr: Self::input(ctx),
                thread_id: input.thread_id.to_string(),
                cursor: input.cursor.to_string(),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn list(&self, ctx: &Context, query: ListQuery) -> Result<ListPage> {
        let cursor = query.cursor.map(|c| t3_forge::ProviderListCursor {
            updated_before: c.updated_before,
            delivered: c.delivered,
            seen_at: Some(c.seen_at),
            resume_after: c.resume_after,
        });
        let page = self
            .forge
            .list_change_requests(f::ListInput {
                repo: Self::input(ctx).repo,
                state: query.input.state,
                involvement: query
                    .input
                    .involvement
                    .flatten()
                    .unwrap_or(PullRequestInvolvement::All),
                viewer: query.viewer.clone(),
                limit: query.limit,
                query: query.input.query.flatten().map(|q| q.to_string()),
                cursor,
                filters: query.input.filters.flatten(),
            })
            .await
            .map_err(|error| self.error(error))?;
        let entries = page
            .items
            .into_iter()
            .map(|item| self.list_entry(ctx, item, &query.viewer))
            .collect::<Result<_>>()?;
        Ok(ListPage {
            entries,
            truncated: page.truncated,
            continues: page.continues,
            cursor_advance: page.cursor_advance,
            resume_after: page.resume_after,
        })
    }
    async fn list_across(
        &self,
        ctx: &Context,
        repositories: &[String],
        query: ListQuery,
    ) -> Result<Option<AcrossPage>> {
        if !self.forge.supports("list_change_requests_across") {
            return Ok(None);
        }
        let list = f::ListInput {
            repo: Self::input(ctx).repo,
            state: query.input.state,
            involvement: query
                .input
                .involvement
                .flatten()
                .unwrap_or(PullRequestInvolvement::All),
            viewer: query.viewer.clone(),
            limit: query.limit,
            query: query.input.query.flatten().map(|q| q.to_string()),
            cursor: query.cursor.map(|c| t3_forge::ProviderListCursor {
                updated_before: c.updated_before,
                delivered: c.delivered,
                seen_at: None,
                resume_after: None,
            }),
            filters: query.input.filters.flatten(),
        };
        let page = self
            .forge
            .list_change_requests_across(f::ListAcrossInput {
                list,
                repositories: repositories.to_vec(),
            })
            .await
            .map_err(|error| self.error(error))?;
        let mut entries = Vec::new();
        for item in page.items {
            let repository = item.repository.clone();
            let item: t3_forge::ProviderChangeRequest = crate::convert(item)?;
            let mut ctx = ctx.clone();
            ctx.project.repository = repository;
            entries.push(self.list_entry(&ctx, item, &query.viewer)?);
        }
        Ok(Some(AcrossPage {
            entries,
            truncated: page.truncated,
            unresolved: page.unresolved.unwrap_or_default(),
        }))
    }
    async fn list_stats(
        &self,
        ctx: &Context,
        refs: &[PullRequestRef],
    ) -> Result<Vec<PullRequestDiffStat>> {
        let stats = self
            .forge
            .list_change_request_stats(f::StatsInput {
                cwd: ctx.project.workspace_root.clone().into(),
                host: ctx.project.host.clone(),
                change_requests: refs
                    .iter()
                    .map(|r| f::ChangeRequestKey {
                        repository: r.repository.to_string(),
                        number: r.number,
                    })
                    .collect(),
            })
            .await
            .map_err(|error| self.error(error))?;
        stats.into_iter().filter_map(|s|refs.iter().find(|r|r.number==s.number&&r.repository.as_str().eq_ignore_ascii_case(&s.repository)).map(|r|crate::convert(json!({"projectId":r.project_id,"repository":r.repository,"number":s.number,"additions":s.additions,"deletions":s.deletions})))).collect()
    }
    async fn diff(
        &self,
        ctx: &Context,
        input: &PullRequestDiffInput,
    ) -> Result<PullRequestDiffResult> {
        crate::convert(
            self.forge
                .get_diff(f::DiffInput {
                    pr: Self::input(ctx),
                    cursor: input
                        .cursor
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(ToString::to_string),
                    commit: input
                        .commit
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(ToString::to_string),
                })
                .await
                .map_err(|error| self.error(error))?,
        )
    }
    async fn diff_paths(&self, ctx: &Context, input: &PullRequestDiffInput) -> Result<DiffPaths> {
        let request = f::DiffInput {
            pr: Self::input(ctx),
            cursor: input
                .cursor
                .as_ref()
                .and_then(Option::as_ref)
                .map(ToString::to_string),
            commit: input
                .commit
                .as_ref()
                .and_then(Option::as_ref)
                .map(ToString::to_string),
        };
        if self.forge.supports("get_diff_file_paths") {
            let paths = self
                .forge
                .get_diff_file_paths(request)
                .await
                .map_err(|error| self.error(error))?;
            return Ok(DiffPaths {
                files: paths
                    .files
                    .into_iter()
                    .map(|p| (p.old_path, p.new_path))
                    .collect(),
                next_cursor: paths.next_cursor,
            });
        }
        let diff = self
            .forge
            .get_diff(request)
            .await
            .map_err(|error| self.error(error))?;
        Ok(DiffPaths {
            files: parse_diff_paths(&diff.patch),
            next_cursor: diff.next_cursor,
        })
    }
    async fn diff_file_contents(
        &self,
        ctx: &Context,
        input: &PullRequestDiffFileContentsInput,
    ) -> Result<PullRequestDiffFileContentsResult> {
        crate::convert(
            self.forge
                .get_diff_file_contents(f::FileContentsInput {
                    pr: Self::input(ctx),
                    commit: input
                        .commit
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(ToString::to_string),
                    change_type: input.change_type.clone(),
                    old_path: input.old_path.to_string(),
                    new_path: input.new_path.to_string(),
                })
                .await
                .map_err(|error| self.error(error))?,
        )
    }
    async fn files_viewed(&self, ctx: &Context) -> Result<PullRequestFilesViewedResult> {
        crate::convert(
            self.forge
                .get_files_viewed(Self::input(ctx))
                .await
                .map_err(|error| self.error(error))?,
        )
    }
    async fn set_files_viewed(
        &self,
        ctx: &Context,
        input: &PullRequestSetFilesViewedInput,
    ) -> Result<()> {
        self.forge
            .set_files_viewed(f::SetFilesViewedInput {
                pr: Self::input(ctx),
                files: input
                    .files
                    .iter()
                    .map(|f| f::ViewedFile {
                        path: f.path.clone(),
                        viewed: f.viewed,
                    })
                    .collect(),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn file_stats(&self, ctx: &Context) -> Result<FileStats> {
        let stats = self
            .forge
            .get_file_stats(Self::input(ctx))
            .await
            .map_err(|error| self.error(error))?;
        Ok(FileStats {
            files: stats
                .files
                .into_iter()
                .map(|f| (f.path, f.additions, f.deletions))
                .collect(),
            truncated: stats.truncated,
        })
    }
    async fn file_revisions(&self, ctx: &Context, paths: &[String]) -> Result<FileRevisions> {
        let result = self
            .forge
            .get_file_revisions(f::FileRevisionsInput {
                pr: Self::input(ctx),
                paths: paths.to_vec(),
            })
            .await
            .map_err(|error| self.error(error))?;
        Ok(FileRevisions {
            revisions: result.revisions,
            complete: result.complete == Some(true),
        })
    }
    async fn run_action(
        &self,
        ctx: &Context,
        input: &PullRequestActionInput,
    ) -> Result<Option<PullRequestSummary>> {
        self.forge
            .run_action(f::ActionInput {
                pr: Self::input(ctx),
                action: input.action.clone(),
                stack_number: input.stack_number.flatten(),
                expected_stack_heads: input.expected_stack_heads.clone().flatten(),
                remove_agent_credits_on_merge: ctx.project.remove_agent_credits_on_merge
                    && input.stack_number.flatten().is_none(),
                merge_method: input.merge_method.clone().flatten(),
                update_method: input.update_method.clone().flatten(),
            })
            .await
            .map_err(|error| self.error(error))?
            .map(|s| self.summary_value(ctx, s, true))
            .transpose()
    }
    async fn update(&self, ctx: &Context, input: &PullRequestUpdateInput) -> Result<()> {
        self.forge
            .update_change_request(f::UpdateInput {
                pr: Self::input(ctx),
                maintainer_can_modify: input.maintainer_can_modify.flatten(),
                title: input
                    .title
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(ToString::to_string),
                body: input.body.clone().flatten(),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn comment(&self, ctx: &Context, input: &PullRequestCommentInput) -> Result<()> {
        self.forge
            .comment(f::CommentInput {
                pr: Self::input(ctx),
                body: input.body.clone(),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn update_comment(
        &self,
        ctx: &Context,
        input: &PullRequestCommentUpdateInput,
    ) -> Result<()> {
        self.forge
            .update_comment(f::UpdateCommentInput {
                pr: Self::input(ctx),
                comment_id: input.comment_id.to_string(),
                kind: input.kind.clone(),
                body: input.body.clone(),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn submit_review(
        &self,
        ctx: &Context,
        input: &PullRequestSubmitReviewInput,
    ) -> Result<()> {
        self.forge
            .submit_review(f::ReviewInput {
                pr: Self::input(ctx),
                verdict: input.verdict.clone(),
                body: input.body.clone(),
                comments: input.comments.clone(),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn reply_to_thread(
        &self,
        ctx: &Context,
        input: &PullRequestThreadReplyInput,
    ) -> Result<()> {
        self.forge
            .reply_to_thread(f::ThreadReplyInput {
                pr: Self::input(ctx),
                thread_id: input.thread_id.to_string(),
                body: input.body.clone(),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn set_thread_resolution(
        &self,
        ctx: &Context,
        input: &PullRequestThreadResolutionInput,
    ) -> Result<()> {
        self.forge
            .set_thread_resolution(f::ThreadResolutionInput {
                pr: Self::input(ctx),
                thread_id: input.thread_id.to_string(),
                resolved: input.resolved,
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn set_reaction(&self, ctx: &Context, input: &PullRequestReactionInput) -> Result<()> {
        self.forge
            .set_reaction(f::ReactionInput {
                pr: Self::input(ctx),
                subject_id: input
                    .subject_id
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(ToString::to_string),
                content: input.content.clone(),
                reacted: input.reacted,
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn reviewer_candidates(
        &self,
        ctx: &Context,
        input: &PullRequestReviewerCandidatesInput,
    ) -> Result<PullRequestReviewerCandidateList> {
        self.forge
            .list_reviewer_candidates(f::CandidateInput {
                pr: Self::input(ctx),
                query: input
                    .query
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(ToString::to_string),
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn request_reviewers(
        &self,
        ctx: &Context,
        input: &PullRequestReviewerRequestInput,
    ) -> Result<()> {
        self.forge
            .set_reviewer_request(f::ReviewerRequestInput {
                pr: Self::input(ctx),
                reviewers: input.reviewers.clone(),
                requested: input.requested,
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn label_candidates(&self, ctx: &Context) -> Result<PullRequestLabelCandidateList> {
        self.forge
            .list_label_candidates(Self::input(ctx))
            .await
            .map_err(|error| self.error(error))
    }
    async fn set_labels(&self, ctx: &Context, input: &PullRequestLabelChangeInput) -> Result<()> {
        self.forge
            .set_labels(f::LabelsInput {
                pr: Self::input(ctx),
                labels: input.labels.iter().map(ToString::to_string).collect(),
                applied: input.applied,
            })
            .await
            .map_err(|error| self.error(error))
    }
    async fn filter_options(
        &self,
        ctx: &Context,
        input: &PullRequestFilterOptionsInput,
    ) -> Result<PullRequestFilterOptions> {
        let request = f::RepositoryQueryInput {
            repo: Self::input(ctx).repo,
            query: input
                .query
                .as_ref()
                .and_then(Option::as_ref)
                .map(ToString::to_string),
            sigil: input.sigil.clone().flatten(),
        };
        let mut result = PullRequestFilterOptions {
            labels: Vec::new(),
            authors: Vec::new(),
            references: Vec::new(),
            truncated: false,
        };
        match input.kind {
            PullRequestFilterOptionKind::Labels => {
                let page = self
                    .forge
                    .list_repository_labels(request)
                    .await
                    .map_err(|error| self.error(error))?;
                result.labels = page.items;
                result.truncated = page.truncated;
            }
            PullRequestFilterOptionKind::Authors => {
                let page = self
                    .forge
                    .search_repository_authors(request)
                    .await
                    .map_err(|error| self.error(error))?;
                result.authors = page.items;
                result.truncated = page.truncated;
            }
            PullRequestFilterOptionKind::References => {
                let page = self
                    .forge
                    .search_repository_references(request)
                    .await
                    .map_err(|error| self.error(error))?;
                result.references = page.items;
                result.truncated = page.truncated;
            }
        }
        Ok(result)
    }
}

pub fn parse_diff_paths(patch: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut old: Option<String> = None;
    let mut new = None;
    let mut in_hunks = false;
    for line in patch.lines().chain(std::iter::once("diff --git ")) {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if old.is_some() || new.is_some() {
                let a = old.take().or_else(|| new.clone()).unwrap_or_default();
                let b = new.take().unwrap_or_else(|| a.clone());
                files.push((a, b));
            }
            (old, new) = header_paths(rest);
            in_hunks = false;
        } else if line.starts_with("@@") {
            in_hunks = true;
        } else if !in_hunks && line.starts_with("--- ") {
            let path = crate::guide::unquote_git_path(line[4..].split('\t').next().unwrap_or(""));
            old = path.strip_prefix("a/").map(str::to_string);
        } else if !in_hunks && line.starts_with("+++ ") {
            let path = crate::guide::unquote_git_path(line[4..].split('\t').next().unwrap_or(""));
            new = path.strip_prefix("b/").map(str::to_string);
        } else if !in_hunks && line.starts_with("rename from ") {
            old = Some(crate::guide::unquote_git_path(&line[12..]));
        } else if !in_hunks && line.starts_with("rename to ") {
            new = Some(crate::guide::unquote_git_path(&line[10..]));
        }
    }
    files
}
fn header_paths(rest: &str) -> (Option<String>, Option<String>) {
    let side = |token: &str, prefix: &str| {
        crate::guide::unquote_git_path(token)
            .strip_prefix(prefix)
            .map(str::to_string)
    };
    if rest.starts_with('"') {
        let mut escaped = false;
        for (at, c) in rest.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == '"' {
                if rest.as_bytes().get(at + 1) != Some(&b' ') {
                    return (None, None);
                }
                return (side(&rest[..at + 1], "a/"), side(&rest[at + 2..], "b/"));
            }
        }
        return (None, None);
    }
    if rest.ends_with('"') {
        if let Some(opens) = rest
            .find('"')
            .filter(|i| *i > 0 && rest.as_bytes()[i - 1] == b' ')
        {
            return (side(&rest[..opens - 1], "a/"), side(&rest[opens..], "b/"));
        }
        return (None, None);
    }
    if !rest.starts_with("a/") {
        return (None, None);
    }
    let splits = rest
        .match_indices(" b/")
        .map(|(at, _)| at)
        .collect::<Vec<_>>();
    let chosen = splits
        .iter()
        .find(|at| rest[2..**at] == rest[*at + 3..])
        .copied()
        .or_else(|| (splits.len() == 1).then(|| splits[0]));
    chosen.map_or((None, None), |at| {
        (Some(rest[2..at].into()), Some(rest[at + 3..].into()))
    })
}
