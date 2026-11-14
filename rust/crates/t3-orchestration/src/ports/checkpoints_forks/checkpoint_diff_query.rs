use super::checkpoint_service::CheckpointMetadata;
use crate::{api::*, error::Error};
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use t3_contracts::checkpoint_diff::*;

#[derive(Debug, thiserror::Error)]
pub enum CheckpointDiffQueryError {
    #[error("Checkpoint invariant violation in {operation}: Thread '{id}' not found.", id = .thread_id.0)]
    ThreadNotFound {
        operation: &'static str,
        thread_id: ThreadId,
    },
    #[error("Checkpoint invariant violation in {operation}: Workspace path missing for thread '{id}' when computing turn diff.", id = .thread_id.0)]
    WorkspacePathMissing {
        operation: &'static str,
        thread_id: ThreadId,
    },
    #[error("Checkpoint unavailable for thread {id} turn {requested_turn_count}: Turn diff range exceeds current turn count: requested {requested_turn_count}, current {available_turn_count}.", id = .thread_id.0)]
    TurnRangeUnavailable {
        operation: &'static str,
        thread_id: ThreadId,
        requested_turn_count: u64,
        available_turn_count: u64,
    },
    #[error("Checkpoint unavailable for thread {id} turn {turn_count}: Checkpoint ref is unavailable for turn {turn_count}.", id = .thread_id.0)]
    RefUnavailable {
        operation: &'static str,
        thread_id: ThreadId,
        turn_count: u64,
        checkpoint: &'static str,
    },
    #[error(transparent)]
    Backend(#[from] Error),
}
impl CheckpointDiffQueryError {
    pub fn cause(&self) -> serde_json::Value {
        use serde_json::json;
        let (tag, operation, thread_id) = match self {
            Self::ThreadNotFound {
                operation,
                thread_id,
            } => ("CheckpointThreadNotFoundError", operation, thread_id),
            Self::WorkspacePathMissing {
                operation,
                thread_id,
            } => ("CheckpointWorkspacePathMissingError", operation, thread_id),
            Self::TurnRangeUnavailable {
                operation,
                thread_id,
                ..
            } => ("CheckpointTurnRangeUnavailableError", operation, thread_id),
            Self::RefUnavailable {
                operation,
                thread_id,
                ..
            } => ("CheckpointRefUnavailableError", operation, thread_id),
            Self::Backend(error) => return json!({"message":error.to_string()}),
        };
        let mut cause = json!({"_tag":tag,"operation":operation,"threadId":thread_id,"message":self.to_string()});
        match self {
            Self::TurnRangeUnavailable {
                requested_turn_count,
                available_turn_count,
                ..
            } => {
                cause["requestedTurnCount"] = json!(requested_turn_count);
                cause["availableTurnCount"] = json!(available_turn_count);
            }
            Self::RefUnavailable {
                turn_count,
                checkpoint,
                ..
            } => {
                cause["turnCount"] = json!(turn_count);
                cause["checkpoint"] = json!(checkpoint);
            }
            _ => {}
        }
        cause
    }
}
pub type DiffQueryResult<T> = std::result::Result<T, CheckpointDiffQueryError>;
pub struct CheckpointDiffQuery {
    projections: Arc<dyn ProjectionReader>,
    checkpoints: Arc<dyn CheckpointMetadata>,
}
impl CheckpointDiffQuery {
    pub fn new(
        projections: Arc<dyn ProjectionReader>,
        checkpoints: Arc<dyn CheckpointMetadata>,
    ) -> Self {
        Self {
            projections,
            checkpoints,
        }
    }
    pub async fn get_turn_diff(
        &self,
        input: OrchestrationGetTurnDiffInput,
    ) -> DiffQueryResult<OrchestrationGetTurnDiffResult> {
        let mut result = ThreadTurnDiff {
            thread_id: input.thread_id.clone(),
            from_turn_count: input.from_turn_count,
            to_turn_count: input.to_turn_count,
            diff: String::new(),
        };
        if input.from_turn_count == input.to_turn_count {
            return Ok(result);
        }
        let operation = "CheckpointDiffQuery.getTurnDiff";
        let context = self
            .projections
            .checkpoint_context(input.thread_id.clone())
            .await
            .map_err(|_| CheckpointDiffQueryError::ThreadNotFound {
                operation,
                thread_id: input.thread_id.clone(),
            })?;
        let completed: HashSet<_> = context
            .runs
            .iter()
            .filter(|run| run.status == OrchestrationV2RunStatus::Completed)
            .map(|run| &run.id)
            .collect();
        let ready: Vec<_> = context
            .checkpoints
            .iter()
            .filter(|checkpoint| {
                checkpoint.status == CheckpointStatus::Ready
                    && checkpoint.app_run_ordinal.is_some()
                    && checkpoint
                        .run_id
                        .as_ref()
                        .is_some_and(|run| completed.contains(run))
            })
            .collect();
        let available = ready
            .iter()
            .filter_map(|checkpoint| checkpoint.app_run_ordinal)
            .max()
            .unwrap_or(0);
        if input.to_turn_count > available {
            return Err(CheckpointDiffQueryError::TurnRangeUnavailable {
                operation,
                thread_id: input.thread_id,
                requested_turn_count: input.to_turn_count,
                available_turn_count: available,
            });
        }
        let unavailable = |checkpoint, turn_count| CheckpointDiffQueryError::RefUnavailable {
            operation,
            thread_id: input.thread_id.clone(),
            checkpoint,
            turn_count,
        };
        let to = ready
            .iter()
            .find(|checkpoint| checkpoint.app_run_ordinal == Some(input.to_turn_count))
            .ok_or_else(|| unavailable("to", input.to_turn_count))?;
        let scope = context
            .checkpoint_scopes
            .iter()
            .find(|scope| scope.id == to.scope_id)
            .ok_or_else(|| CheckpointDiffQueryError::WorkspacePathMissing {
                operation,
                thread_id: input.thread_id.clone(),
            })?;
        let parent = if input.to_turn_count.checked_sub(1) == Some(input.from_turn_count) {
            context.checkpoints.iter().find(|checkpoint| {
                checkpoint.status == CheckpointStatus::Ready
                    && checkpoint.scope_id == to.scope_id
                    && to.ordinal_within_scope.checked_sub(1)
                        == Some(checkpoint.ordinal_within_scope)
            })
        } else {
            None
        };
        let from = if input.from_turn_count == 0 {
            context
                .checkpoint_scopes
                .iter()
                .find(|scope| scope.kind == CheckpointScopeKind::RootRun)
                .map(|scope| self.checkpoints.reference(&scope.id, 0))
        } else {
            parent
                .or_else(|| {
                    ready.iter().copied().find(|checkpoint| {
                        checkpoint.app_run_ordinal == Some(input.from_turn_count)
                    })
                })
                .map(|checkpoint| checkpoint.reference.clone())
        }
        .ok_or_else(|| unavailable("from", input.from_turn_count))?;
        result.diff = self
            .checkpoints
            .query_diff(
                PathBuf::from(&scope.cwd.0),
                from,
                to.reference.clone(),
                input.ignore_whitespace.unwrap_or(true),
            )
            .await?;
        Ok(result)
    }
    pub async fn get_full_thread_diff(
        &self,
        input: OrchestrationGetFullThreadDiffInput,
    ) -> DiffQueryResult<OrchestrationGetFullThreadDiffResult> {
        self.get_turn_diff(OrchestrationGetTurnDiffInput {
            thread_id: input.thread_id,
            from_turn_count: 0,
            to_turn_count: input.to_turn_count,
            ignore_whitespace: input.ignore_whitespace,
        })
        .await
    }
}
