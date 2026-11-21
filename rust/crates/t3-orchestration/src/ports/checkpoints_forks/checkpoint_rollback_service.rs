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
    provider::{
        OpenSession, RollbackInput, RollbackTarget, RuntimePolicy, SessionRuntime, TurnMessage,
    },
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
/// The folder no longer belongs to the repository it was bound to (t3-git H-1).
pub const REPOSITORY_CHANGED_OPERATION: &str = "checkpoint.git.repository-changed";
/// A file restore refused because the thread still has writers; the detail names them.
pub const WRITERS_RUNNING_OPERATION: &str = "checkpoint.rollback.writers-running";

pub fn rollback_failure_client_message(error: &Error) -> &str {
    match error {
        Error::Service { operation, detail }
            if matches!(
                *operation,
                INDEX_LOCKED_OPERATION
                    | POLICY_CEILING_OPERATION
                    | WRITERS_RUNNING_OPERATION
                    | REPOSITORY_CHANGED_OPERATION
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
    /// The host's terminals, dev servers and scripts for the thread, held through a restore.
    pub writers: Arc<dyn ThreadWriters>,
    /// The host's per-thread process scope: processes the agent started, even detached ones.
    pub agent_scope: Arc<dyn AgentScope>,
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

fn reject(reason: impl Into<String>) -> Error {
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

/// A writer's name as clients see it: one line of at most 80 characters (re-review 4 L4).
fn writer_name(name: &str) -> String {
    let line = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() <= 80 {
        line
    } else {
        format!("{}…", line.chars().take(79).collect::<String>())
    }
}

/// Provider work that outlives a turn. A session that can neither report it nor promise there
/// is none cannot be proven quiet (re-review 4 F9).
async fn background_writers(
    session: &dyn SessionRuntime,
    provider: &OrchestrationV2ProviderThread,
) -> Vec<String> {
    let running = match session.extensions() {
        Some(extensions) => extensions
            .pending_background_work(Some(provider.clone()), true)
            .await
            .unwrap_or(true)
            .then_some("background work its provider started"),
        None if session.leaves_no_background_work() => None,
        None => Some("work its provider may still be running (this provider cannot report it)"),
    };
    running.into_iter().map(str::to_owned).collect()
}

fn writers_running(running: &[String]) -> Result<()> {
    if running.is_empty() {
        return Ok(());
    }
    Err(Error::Service {
        operation: WRITERS_RUNNING_OPERATION,
        detail: format!(
            "Files can't be restored while this thread is still writing to its folder: {}. Stop them, then try again.",
            running.join(", ")
        ),
    })
}

impl CheckpointRollbackService {
    /// Refuses a file restore while anything of this thread can write to its workspace, naming
    /// each writer, and otherwise holds the host's writers until the returned hold drops.
    /// TypeScript restores under live writers; refusing is the Rust hardening (t3-git H-2).
    async fn quiesce(
        &self,
        thread_id: &ThreadId,
        cwd: &str,
        session: Option<&dyn SessionRuntime>,
        provider: &OrchestrationV2ProviderThread,
    ) -> Result<WriterHold> {
        let mut running = self.running_writers(thread_id, session, provider).await?;
        match self.agent_scope.live_processes(thread_id.clone()).await {
            Ok(None | Some(0)) => {}
            Ok(Some(1)) => running.push("a process the agent started".into()),
            Ok(Some(count)) => running.push(format!("{count} processes the agent started")),
            Err(_) => {
                running.push("processes the agent started (they could not be checked)".into())
            }
        }
        let hold = self
            .writers
            .hold(thread_id.clone(), PathBuf::from(cwd))
            .await?;
        running.extend(hold.running.iter().map(|name| writer_name(name)));
        writers_running(&running)?;
        Ok(hold)
    }

    /// The folder a file restore writes: the thread's worktree, or its project's root.
    async fn restore_folder(&self, thread: &OrchestrationV2AppThread) -> Result<Option<PathBuf>> {
        if let Some(path) = &thread.worktree_path {
            return Ok(Some(PathBuf::from(&path.0)));
        }
        let project_id = expanded_id(&thread.project_id)?;
        Ok(self
            .database
            .read(move |db| crate::stores::projects::get(db, &project_id, true))
            .await?
            .map(|project| PathBuf::from(project.workspace_root)))
    }

    /// The thread's own writers: a running turn, provider commands and, with a session to ask,
    /// provider background work.
    async fn running_writers(
        &self,
        thread_id: &ThreadId,
        session: Option<&dyn SessionRuntime>,
        provider: &OrchestrationV2ProviderThread,
    ) -> Result<Vec<String>> {
        // Narrow reads: history may hold payloads older runs no longer decode.
        let shells = self
            .projections
            .thread_shells(ShellQuery {
                archived: true,
                thread_ids: Some(vec![thread_id.clone()]),
                ..Default::default()
            })
            .await?;
        let shell = shells
            .threads
            .into_iter()
            .chain(shells.archived_threads)
            .next()
            .ok_or_else(|| Error::ThreadNotFound(thread_id.clone()))?;
        let mut running = Vec::new();
        if shell.active_run_id.is_some() {
            running.push("a running turn".to_owned());
        }
        let commands = self
            .projections
            .records(
                thread_id.clone(),
                vec![RecordKind::TurnItems],
                RecordFilter {
                    turn_item_types: Some(vec!["command_execution".into()]),
                    turn_item_statuses: Some(vec![
                        OrchestrationV2TurnItemStatus::Pending,
                        OrchestrationV2TurnItemStatus::Running,
                        OrchestrationV2TurnItemStatus::Waiting,
                    ]),
                    ..Default::default()
                },
            )
            .await?;
        for set in commands.records {
            let ProjectionRecordSet::TurnItems(items) = set else {
                continue;
            };
            for item in items {
                if let OrchestrationV2TurnItem::CommandExecution(item) = item {
                    running.push(match item.title.as_deref().map(writer_name) {
                        Some(title) if !title.is_empty() => format!("command `{title}`"),
                        _ => "a command".to_owned(),
                    });
                }
            }
        }
        if let Some(session) = session {
            running.extend(background_writers(session, provider).await);
        }
        Ok(running)
    }

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
        // Every turn start on an overlapping folder (composed through `CoordinatedTurnStart`)
        // waits from here until the rollback commits (re-review 4 F2).
        let _reservation = if input.restore_files {
            match self.restore_folder(&thread).await? {
                Some(path) => Some(
                    reservations
                        .reserve(&path, ReservationKind::Restore, &cancellation)
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
        // Writers are checked, and the host's held until Git returns, before recovery may move a
        // leftover folder aside (re-review 4 F8). A live session answers for its background work
        // now; the session opened below answers again before the provider's rollback.
        let host_writers = if input.restore_files {
            let live = self.sessions.get(expanded_id(session_id)?).await?;
            Some(
                self.quiesce(&input.thread_id, &scope.cwd.0, live.as_deref(), provider)
                    .await?,
            )
        } else {
            None
        };
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
        // Before the provider's rollback, so these refusals change nothing (re-review 4 F7, F9).
        // The thread lane is not held here because ingesting the provider's rollback events
        // takes it.
        if input.restore_files {
            writers_running(&background_writers(session.as_ref(), provider).await)?;
            self.checkpoints.preflight_restore(scope).await?;
        }
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
        // The thread's lane keeps new turns, sends and queue starts out until the rollback commits.
        let lane = self
            .command_lanes
            .acquire([input.thread_id.0.clone()])
            .await;
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
            // A turn or command may have started during the provider's rollback; the lane now
            // keeps new ones out until the commit.
            writers_running(
                &self
                    .running_writers(&input.thread_id, Some(session.as_ref()), provider)
                    .await?,
            )?;
            self.checkpoints
                .restore(scope.clone(), checkpoint, cancellation)
                .await?;
        }
        drop(host_writers);
        if !context.stale_checkpoints.is_empty() {
            self.checkpoints
                .delete_stale_refs(scope.clone(), context.stale_checkpoints.clone())
                .await?;
        }
        // Share the dispatcher's lane through the final full-row writes and their commit.
        let _lane = lane;
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
            // A missing .git does not make user files disposable. Keep the named folder
            // intact when its contents cannot be positively identified as build outputs.
            use t3_workspace::restore::{is_leftover, remove_leftovers, verify_target};
            if !std::fs::symlink_metadata(&path)?.is_dir() || !is_leftover(&path) {
                return Err(reject(
                    "Worktree recovery refused to replace a folder containing user files.",
                ));
            }
            let parent = path
                .parent()
                .ok_or_else(|| reject("worktree has no parent"))?;
            let real_parent = t3_workspace::paths::nearest_real(parent)
                .map_err(|e| reject(e.to_string()))?
                .0;
            let aside = tempfile::Builder::new()
                .prefix(".t3-replaced-")
                .tempdir_in(&real_parent)?
                .keep();
            let saved = aside.join("folder");
            let unchanged =
                || verify_target(parent, &real_parent).map_err(|e| reject(e.to_string()));
            unchanged()?;
            std::fs::rename(&path, &saved)?;
            let result = if is_leftover(&saved) {
                self.workspace_recovery
                    .recreate(thread, cancellation.clone())
                    .await
            } else {
                Err(reject("Worktree contents changed during recovery."))
            };
            unchanged()?;
            match result {
                Ok(()) => {
                    if !remove_leftovers(&saved, &saved).map_err(|e| {
                        reject(format!("Saved worktree at {}: {e}", aside.display()))
                    })? {
                        return Err(reject(format!(
                            "Worktree data was preserved at {}",
                            saved.display()
                        )));
                    }
                    std::fs::remove_dir(&aside)?;
                }
                Err(error) => {
                    // Never recursively erase a partial checkout: a concurrent file may be there.
                    let partial = aside.join("incomplete");
                    match std::fs::rename(&path, &partial) {
                        Ok(()) => {
                            std::fs::rename(&saved, &path)?;
                            return Err(reject(format!(
                                "{error}. Incomplete checkout preserved at {}",
                                partial.display()
                            )));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                    std::fs::rename(&saved, &path)?;
                    std::fs::remove_dir(&aside)?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}
