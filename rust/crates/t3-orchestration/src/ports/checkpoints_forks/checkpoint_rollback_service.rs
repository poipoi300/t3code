use super::{
    checkpoint_capture_service::domain_event,
    checkpoint_restore_safety::{
        ReservationKind, SHARED_WORKSPACE_RESTORE_MESSAGE, WorkspaceReservations, expanded_id,
        is_checkpoint_restore_isolated,
    },
    checkpoint_service::{CheckpointService, WorkspaceLocks},
};
use crate::{
    api::*,
    error::{Error, Result},
    provider::{OpenSession, RollbackInput, RollbackTarget, RuntimePolicy, TurnMessage},
    stores::StoreDatabase,
};
use async_trait::async_trait;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const ROLLBACK_FAILED_MESSAGE: &str = "The provider could not roll back this conversation. Try again; if it keeps failing, check the provider and server logs.";
pub const INDEX_LOCKED_OPERATION: &str = "checkpoint.git.index-locked";
pub const POLICY_CEILING_OPERATION: &str = "runtime-policy.above-ceiling";

pub fn rollback_failure_client_message(error: &Error) -> &str {
    match error {
        Error::Service { operation, detail }
            if matches!(
                *operation,
                INDEX_LOCKED_OPERATION | POLICY_CEILING_OPERATION
            ) =>
        {
            detail
        }
        _ => ROLLBACK_FAILED_MESSAGE,
    }
}

#[async_trait]
pub trait RollbackPolicy: Send + Sync {
    async fn resolve(&self, thread: &OrchestrationV2AppThread) -> Result<RuntimePolicy>;
    async fn resolve_configured(
        &self,
        thread: &OrchestrationV2AppThread,
    ) -> Result<crate::provider::ConfiguredRuntimePolicy> {
        Ok(self.resolve(thread).await?.with_reasoning_effort(None))
    }
}

pub struct ResolvedRollbackPolicy(pub Arc<dyn RuntimePolicyResolver>);

#[async_trait]
impl RollbackPolicy for ResolvedRollbackPolicy {
    async fn resolve(&self, thread: &OrchestrationV2AppThread) -> Result<RuntimePolicy> {
        let resolved = self.resolve_configured(thread).await?;
        if resolved.options.reasoning_effort.is_some() {
            return Err(Error::Unsupported("rollback session reasoning policy"));
        }
        Ok(resolved.policy)
    }
    async fn resolve_configured(
        &self,
        thread: &OrchestrationV2AppThread,
    ) -> Result<crate::provider::ConfiguredRuntimePolicy> {
        self.0
            .resolve(thread.clone(), thread.model_selection.clone())
            .await
    }
}

#[async_trait]
pub trait RollbackWorkspaceRecovery: Send + Sync {
    async fn is_managed(&self, path: &Path) -> Result<bool>;
    async fn recreate(
        &self,
        thread: &OrchestrationV2AppThread,
        cancellation: CancellationToken,
    ) -> Result<()>;
}

pub struct RollbackRequest {
    pub thread_id: ThreadId,
    pub provider_thread_id: ProviderThreadId,
    pub checkpoint_id: CheckpointId,
    pub scope_id: CheckpointScopeId,
    pub restore_files: bool,
}

pub struct CheckpointRollbackService {
    pub command_lanes: Arc<crate::ports::decisions::thread_command_executor::KeyedLanes>,
    pub checkpoints: Arc<CheckpointService>,
    pub projections: Arc<dyn ProjectionReader>,
    pub sessions: Arc<dyn SessionDirectory>,
    pub providers: Arc<dyn ProviderRegistry>,
    pub policy: Arc<dyn RollbackPolicy>,
    pub workspace_recovery: Arc<dyn RollbackWorkspaceRecovery>,
    pub events: Arc<dyn EventCommitter>,
    pub database: StoreDatabase,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
}

#[async_trait]
impl CheckpointRollback for CheckpointRollbackService {
    async fn rollback(
        &self,
        input: RollbackRequest,
        cancellation: CancellationToken,
    ) -> Result<()> {
        self.execute(input, cancellation).await
    }
}

fn reject(reason: &'static str) -> Error {
    Error::Service {
        operation: "checkpoint.rollback",
        detail: reason.into(),
    }
}

pub fn latest_provider_turn_for_attempt<'a>(
    turns: &'a [OrchestrationV2ProviderTurn],
    attempt: Option<&t3_contracts::orchestration_v2_id_table::IdOrIndex<RunAttemptId>>,
) -> Option<&'a OrchestrationV2ProviderTurn> {
    let attempt = attempt?;
    turns
        .iter()
        .filter(|turn| turn.run_attempt_id.as_ref() == Some(attempt))
        .fold(None, |latest, turn| {
            if latest
                .is_none_or(|latest: &OrchestrationV2ProviderTurn| turn.ordinal > latest.ordinal)
            {
                Some(turn)
            } else {
                latest
            }
        })
}

impl CheckpointRollbackService {
    pub async fn execute(
        &self,
        input: RollbackRequest,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let thread = self
            .projections
            .thread(input.thread_id.clone())
            .await?
            .ok_or_else(|| Error::ThreadNotFound(input.thread_id.clone()))?;
        let reservations = WorkspaceReservations::global();
        // The reservation excludes overlapping turn starts throughout provider rollback and restore.
        let _reservation = if input.restore_files {
            match &thread.worktree_path {
                Some(path) => Some(
                    reservations
                        .reserve(Path::new(&path.0), ReservationKind::Restore, &cancellation)
                        .await?,
                ),
                None => None,
            }
        } else {
            None
        };
        // Storage cleanup must share this lease; the snapshot lock remains non-reentrant and separate.
        let leases = WorkspaceLocks::workspace_leases();
        let _lease = match &thread.worktree_path {
            Some(path) => Some(leases.acquire(Path::new(&path.0)).await?),
            None => None,
        };
        let context = self
            .projections
            .checkpoint_rollback(
                input.thread_id.clone(),
                input.provider_thread_id.clone(),
                input.checkpoint_id.clone(),
                input.scope_id.clone(),
            )
            .await?;
        let (Some(provider), Some(checkpoint), Some(scope)) = (
            context.provider_thread.as_ref(),
            context.checkpoint.as_ref(),
            context.scope.as_ref(),
        ) else {
            return Err(reject("rollback-target-invalid"));
        };
        let Some(session_id) = provider.provider_session_id.as_ref() else {
            return Err(reject("rollback-target-invalid"));
        };
        if checkpoint.scope_id != scope.id
            || scope.thread_id != context.thread.id
            || checkpoint.thread_id != context.thread.id
            || expanded_id(&checkpoint.id)? != input.checkpoint_id
            || expanded_id(&scope.id)? != input.scope_id
            || !(checkpoint.status == CheckpointStatus::Ready
                || (!input.restore_files && checkpoint.status == CheckpointStatus::Missing))
        {
            return Err(reject("rollback-target-invalid"));
        }
        if context.thread.worktree_path != thread.worktree_path {
            return Err(reject("active-provider-changed"));
        }
        self.checkpoints
            .validate_owned_checkpoint(scope, checkpoint)?;
        for stale in &context.stale_checkpoints {
            self.checkpoints.validate_owned_checkpoint(scope, stale)?;
        }
        if context.thread.active_provider_thread_id.as_ref() != Some(&provider.id)
            || expanded_id(&provider.id)? != input.provider_thread_id
            || expanded_id(&provider.provider_instance_id)?
                != context.thread.model_selection.instance_id
        {
            return Err(reject("active-provider-changed"));
        }
        let adapter = self
            .providers
            .adapter(context.thread.model_selection.instance_id.clone())
            .await?;
        let capabilities = adapter.capabilities().await?;
        if !capabilities.threads.can_rollback_thread
            || !capabilities
                .checkpointing
                .provider_can_rollback_conversation
        {
            return Err(Error::Unsupported("provider conversation rollback"));
        }
        let target_ordinal = checkpoint.app_run_ordinal.unwrap_or(0);
        let target = if target_ordinal == 0 {
            RollbackTarget::ThreadStart {
                checkpoint_id: input.checkpoint_id.clone(),
            }
        } else {
            let attempt = context
                .target_attempt
                .as_ref()
                .ok_or_else(|| reject("provider-turn-unavailable"))?;
            let target =
                latest_provider_turn_for_attempt(&context.provider_turns, Some(&attempt.id))
                    .or_else(|| {
                        context
                            .provider_turns
                            .iter()
                            .find(|turn| Some(&turn.id) == attempt.provider_turn_id.as_ref())
                    })
                    .ok_or_else(|| reject("provider-turn-unavailable"))?;
            if target.provider_thread_id != provider.id {
                return Err(reject("provider-turn-unavailable"));
            }
            let message = self
                .projections
                .run_message(input.thread_id.clone(), expanded_id(&attempt.run_id)?)
                .await?;
            let message = message
                .map(|message| -> Result<TurnMessage> {
                    Ok(TurnMessage {
                        message_id: expanded_id(&message.id)?,
                        text: message.text,
                        attachments: message.attachments,
                        created_by: message.created_by,
                        creation_source: Some(message.creation_source),
                        scheduled_task_id: message
                            .scheduled_task_id
                            .flatten()
                            .as_ref()
                            .map(expanded_id)
                            .transpose()?,
                        sender_thread_id: message
                            .sender_thread_id
                            .flatten()
                            .as_ref()
                            .map(expanded_id)
                            .transpose()?,
                    })
                })
                .transpose()?;
            RollbackTarget::ProviderTurn {
                checkpoint_id: input.checkpoint_id.clone(),
                app_run_ordinal: target_ordinal,
                provider_turn: Box::new(target.clone()),
                message,
            }
        };
        if input.restore_files
            && !is_checkpoint_restore_isolated(
                &context.thread,
                scope,
                self.projections.as_ref(),
                &self.database,
            )
            .await?
        {
            return Err(reject(SHARED_WORKSPACE_RESTORE_MESSAGE));
        }
        let configured_policy = self.policy.resolve_configured(&context.thread).await?;
        self.recover_workspace(&context.thread, input.restore_files, &cancellation)
            .await?;
        if input.restore_files
            && !is_checkpoint_restore_isolated(
                &context.thread,
                scope,
                self.projections.as_ref(),
                &self.database,
            )
            .await?
        {
            return Err(reject(SHARED_WORKSPACE_RESTORE_MESSAGE));
        }
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let native_id = provider
            .native_thread_ref
            .as_ref()
            .and_then(|r| r.native_id.as_ref())
            .map(|id| id.0.clone());
        let identity_version = provider
            .native_metadata
            .as_ref()
            .and_then(|m| m.as_ref())
            .and_then(|m| m.item_identity_version.clone())
            .flatten()
            .map(u64::from)
            .map(|n| n as u8);
        let session = self
            .sessions
            .attach_with_policy(
                context.thread.model_selection.instance_id.clone(),
                OpenSession {
                    thread_id: input.thread_id.clone(),
                    provider_session_id: expanded_id(session_id)?,
                    model_selection: context.thread.model_selection.clone(),
                    runtime_policy: configured_policy.policy,
                    prompt_suggestions: None,
                    resume_from_session: context.session.clone(),
                    initial_native_thread_id: native_id,
                    initial_provider_item_identity_version: identity_version,
                },
                configured_policy.options,
            )
            .await?;
        let mut turns = Vec::new();
        for turn in &context.provider_turns {
            if turn.provider_thread_id != provider.id {
                continue;
            }
            if let Some(attempt) = &turn.run_attempt_id
                && context
                    .rolled_back_attempt_ids
                    .contains(&expanded_id(attempt)?)
            {
                continue;
            }
            turns.push(turn.clone());
        }
        let mut restored_provider = if context.runs_to_rollback.is_empty() {
            provider.clone()
        } else {
            session
                .rollback(RollbackInput {
                    provider_thread: provider.clone(),
                    target,
                    provider_thread_turns: turns,
                })
                .await?
                .provider_thread
        };
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        // Re-read ownership after a slow provider operation, while still holding the reservation.
        if input.restore_files {
            let current = self
                .projections
                .thread(input.thread_id.clone())
                .await?
                .ok_or_else(|| Error::ThreadNotFound(input.thread_id.clone()))?;
            if current.active_provider_thread_id != context.thread.active_provider_thread_id
                || current.model_selection.instance_id != context.thread.model_selection.instance_id
                || current.worktree_path != context.thread.worktree_path
            {
                return Err(reject("active-provider-changed"));
            }
            if !is_checkpoint_restore_isolated(
                &context.thread,
                scope,
                self.projections.as_ref(),
                &self.database,
            )
            .await?
            {
                return Err(reject(SHARED_WORKSPACE_RESTORE_MESSAGE));
            }
            self.checkpoints
                .restore(scope.clone(), checkpoint, cancellation)
                .await?;
        }
        if !context.stale_checkpoints.is_empty() {
            self.checkpoints
                .delete_stale_refs(scope.clone(), context.stale_checkpoints.clone())
                .await?;
        }
        // Share the dispatcher's lane through the final full-row writes and their commit.
        let _lane = self
            .command_lanes
            .acquire([input.thread_id.0.clone()])
            .await;
        let now = self.clock.now();
        restored_provider.last_run_ordinal = (target_ordinal > 0).then_some(target_ordinal);
        restored_provider.updated_at = now.clone();
        let provider_metadata = json!({ "driver": provider.driver, "providerInstanceId": provider.provider_instance_id });
        let mut events = vec![domain_event(
            self.ids.as_ref(),
            &input.thread_id,
            &now,
            "provider-thread.updated",
            restored_provider,
            provider_metadata,
        )?];
        for mut checkpoint in context.stale_checkpoints {
            let mut metadata = json!({ "nodeId": checkpoint.node_id, "providerInstanceId": provider.provider_instance_id });
            if let Some(run_id) = &checkpoint.run_id {
                metadata["runId"] = serde_json::to_value(run_id)?;
            }
            checkpoint.status = CheckpointStatus::Stale;
            events.push(domain_event(
                self.ids.as_ref(),
                &input.thread_id,
                &now,
                "checkpoint.captured",
                checkpoint,
                metadata,
            )?);
        }
        for mut run in context.runs_to_rollback {
            let root = context
                .root_nodes
                .iter()
                .find(|node| Some(&node.id) == run.root_node_id.as_ref());
            let mut metadata =
                json!({ "runId": run.id, "providerInstanceId": run.provider_instance_id });
            if let Some(root) = root {
                metadata["nodeId"] = serde_json::to_value(&root.id)?;
            }
            run.status = OrchestrationV2RunStatus::RolledBack;
            run.completed_at = Some(now.clone());
            events.push(domain_event(
                self.ids.as_ref(),
                &input.thread_id,
                &now,
                "run.updated",
                run,
                metadata.clone(),
            )?);
            if let Some(root) = root {
                let mut root = root.clone();
                root.status = ExecutionNodeStatus::RolledBack;
                root.completed_at = Some(now.clone());
                events.push(domain_event(
                    self.ids.as_ref(),
                    &input.thread_id,
                    &now,
                    "node.updated",
                    root,
                    metadata,
                )?);
            }
        }
        let committed = self
            .events
            .commit(CommitRequest {
                receipt: None,
                decision: Decision {
                    events,
                    ..Default::default()
                },
                guard: CommitGuard::ProviderThreadUnchanged(Box::new(provider.clone())),
            })
            .await?;
        if !committed.committed {
            return Err(Error::Stale);
        }
        Ok(())
    }

    async fn recover_workspace(
        &self,
        thread: &OrchestrationV2AppThread,
        restore_files: bool,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let (Some(worktree), Some(_)) = (&thread.worktree_path, &thread.branch) else {
            return Ok(());
        };
        let path = PathBuf::from(&worktree.0);
        let exists = match std::fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let leftover = restore_files && exists && !path.join(".git").try_exists()?;
        if (!exists || leftover) && self.workspace_recovery.is_managed(&path).await? {
            if !leftover {
                return self
                    .workspace_recovery
                    .recreate(thread, cancellation.clone())
                    .await;
            }
            let parent = path
                .parent()
                .ok_or_else(|| reject("worktree has no parent"))?;
            let aside = tempfile::Builder::new()
                .prefix(".t3-replaced-")
                .tempdir_in(parent)?;
            let aside = aside.keep();
            let saved = aside.join("folder");
            std::fs::rename(&path, &saved)?;
            match self
                .workspace_recovery
                .recreate(thread, cancellation.clone())
                .await
            {
                Ok(()) => {
                    std::fs::remove_dir_all(&aside)?;
                }
                Err(error) => {
                    // Preserve the set-aside data if checkout removal or putting it back fails.
                    if path.try_exists()? {
                        std::fs::remove_dir_all(&path)?;
                    }
                    std::fs::rename(aside.join("folder"), &path)?;
                    std::fs::remove_dir(&aside)?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}
