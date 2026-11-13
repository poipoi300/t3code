use serde::{Deserialize, Serialize};
use t3_contracts::{
    base_schemas::*,
    model_selection::ModelSelection,
    provider_policy::{ProviderApprovalDecision, ProviderUserInputAnswers},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum EffectRequest {
    #[serde(rename = "provider-runtime.continue")]
    Continue { source_run_id: RunId },
    #[serde(rename = "provider-session.detach")]
    Detach {
        provider_session_id: ProviderSessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revoke_mcp_credential: Option<bool>,
    },
    #[serde(rename = "provider-session.apply-model-selection")]
    ApplySelection {
        provider_session_id: ProviderSessionId,
        provider_thread_id: ProviderThreadId,
        model_selection: ModelSelection,
    },
    #[serde(rename = "provider-turn.start")]
    Start { run_id: RunId },
    #[serde(rename = "provider-turn.interrupt")]
    Interrupt {
        provider_session_id: ProviderSessionId,
        provider_thread_id: ProviderThreadId,
        provider_turn_id: ProviderTurnId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent: Option<InterruptSubagent>,
    },
    #[serde(rename = "provider-turn.steer")]
    Steer {
        provider_session_id: ProviderSessionId,
        provider_thread_id: ProviderThreadId,
        provider_turn_id: ProviderTurnId,
        message_id: MessageId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_selection: Option<ModelSelection>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steer_follow_up_after_run_ordinal: Option<u64>,
    },
    #[serde(rename = "provider-turn.restart")]
    Restart {
        provider_session_id: ProviderSessionId,
        provider_thread_id: ProviderThreadId,
        provider_turn_id: ProviderTurnId,
        interrupted_attempt_id: RunAttemptId,
        run_id: RunId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_transition: Option<SessionTransition>,
    },
    #[serde(rename = "runtime-request.respond")]
    Respond {
        provider_session_id: ProviderSessionId,
        request_id: RuntimeRequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<ProviderApprovalDecision>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answers: Option<ProviderUserInputAnswers>,
    },
    #[serde(rename = "provider-thread.rollback")]
    Rollback {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        restore_files: Option<bool>,
        provider_thread_id: ProviderThreadId,
        checkpoint_id: CheckpointId,
        scope_id: CheckpointScopeId,
    },
    #[serde(rename = "checkpoint.capture")]
    Capture {
        run_id: RunId,
        scope_id: CheckpointScopeId,
    },
    #[serde(rename = "scheduled-tasks.cleanup")]
    CleanupScheduledTasks,
    #[serde(rename = "terminal.cleanup")]
    CleanupTerminals,
    #[serde(rename = "preview.cleanup")]
    CleanupPreviews,
    #[serde(rename = "attachment.cleanup")]
    CleanupAttachments { attachment_ids: Vec<String> },
    #[serde(rename = "thread-title.generate")]
    GenerateTitle { kind: TitleKind },
    #[serde(rename = "delegated-tasks.stop")]
    StopDelegated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptSubagent {
    pub id: NodeId,
    pub native_task_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionTransition {
    Replace {
        replacement_provider_session_id: ProviderSessionId,
    },
    Detach,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TitleKind {
    Initial { message_id: MessageId },
    Regenerate,
}

impl EffectRequest {
    pub fn effect_type(&self) -> &'static str {
        match self {
            Self::Continue { .. } => "provider-runtime.continue",
            Self::Detach { .. } => "provider-session.detach",
            Self::ApplySelection { .. } => "provider-session.apply-model-selection",
            Self::Start { .. } => "provider-turn.start",
            Self::Interrupt { .. } => "provider-turn.interrupt",
            Self::Steer { .. } => "provider-turn.steer",
            Self::Restart { .. } => "provider-turn.restart",
            Self::Respond { .. } => "runtime-request.respond",
            Self::Rollback { .. } => "provider-thread.rollback",
            Self::Capture { .. } => "checkpoint.capture",
            Self::CleanupScheduledTasks => "scheduled-tasks.cleanup",
            Self::CleanupTerminals => "terminal.cleanup",
            Self::CleanupPreviews => "preview.cleanup",
            Self::CleanupAttachments { .. } => "attachment.cleanup",
            Self::GenerateTitle { .. } => "thread-title.generate",
            Self::StopDelegated { .. } => "delegated-tasks.stop",
        }
    }
    pub fn process_bound(&self) -> bool {
        matches!(
            self,
            Self::ApplySelection { .. }
                | Self::Start { .. }
                | Self::Interrupt { .. }
                | Self::Steer { .. }
                | Self::Restart { .. }
                | Self::Respond { .. }
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PendingEffect {
    pub id: String,
    pub command_id: CommandId,
    pub thread_id: ThreadId,
    pub request: EffectRequest,
    pub available_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct EffectRow {
    pub id: String,
    pub command_id: CommandId,
    pub thread_id: ThreadId,
    pub request: EffectRequest,
    pub status: EffectStatus,
    pub attempt_count: i64,
    pub available_at: String,
    pub lease_owner: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, Default)]
pub struct ClaimOptions {
    pub worker_id: String,
    pub exclude_restart_continuations: bool,
    pub max_running_title_generations: Option<i64>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciliation {
    pub requeued: usize,
    pub cancelled: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EffectExecutionOptions {
    pub will_retry: bool,
}
