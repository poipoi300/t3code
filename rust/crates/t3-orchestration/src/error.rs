use t3_contracts::base_schemas::{CommandId, ThreadId};

pub type Result<T> = std::result::Result<T, Error>;

/// Forge failures whose kind drives host pauses, missing-PR backoff and watch ending.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PullRequestHostError {
    #[error("pull request host failed: {detail}")]
    Failed { transient: bool, detail: String },
    #[error("pull request host rate limited: {detail}")]
    RateLimited {
        retry_at: Option<t3_contracts::base_schemas::IsoDateTime>,
        detail: String,
    },
    #[error("pull request not found: {detail}")]
    NotFound { detail: String },
    #[error("pull request host refused: {detail}")]
    Permanent { detail: String },
}

/// Why the runtime refused a caller's command, with TypeScript's texts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallerRefusal {
    /// TS `OrchestratorThreadAboveModeLimitError`: a target already runs above the caller's modes.
    #[error(
        "Thread {} now runs in {runtime_mode}/{interaction_mode} mode, above what this caller may change.",
        thread_id.0
    )]
    AboveModeLimit {
        thread_id: ThreadId,
        mode: crate::ports::decisions::dispatch_mode_limit::ModeLimit,
        runtime_mode: String,
        interaction_mode: String,
    },
    /// TS `resolveRuntimeMode` / `resolveInteractionMode`: the command would make a thread
    /// broader than the caller.
    #[error("Child {} mode {requested} is broader than parent mode {limit}.", match mode {
        crate::ports::decisions::dispatch_mode_limit::ModeLimit::Runtime => "runtime",
        crate::ports::decisions::dispatch_mode_limit::ModeLimit::Interaction => "interaction",
    })]
    Escalation {
        mode: crate::ports::decisions::dispatch_mode_limit::ModeLimit,
        requested: String,
        limit: String,
    },
    /// TS `assertSameLink`: a fenced caller may change only threads its link started.
    #[error(
        "This work was started from {label} through a linked environment, so it may change only threads that link started. A link paired with full access is not limited this way."
    )]
    OutsideLink { label: String },
}

impl CallerRefusal {
    /// TS `OrchestratorMcpFailure.code` for the refusal.
    pub fn code(&self) -> &'static str {
        use crate::ports::decisions::dispatch_mode_limit::ModeLimit;
        match self {
            Self::AboveModeLimit {
                mode: ModeLimit::Runtime,
                ..
            }
            | Self::Escalation {
                mode: ModeLimit::Runtime,
                ..
            } => "runtime_mode_escalation_denied",
            Self::AboveModeLimit {
                mode: ModeLimit::Interaction,
                ..
            }
            | Self::Escalation {
                mode: ModeLimit::Interaction,
                ..
            } => "interaction_mode_escalation_denied",
            Self::OutsideLink { .. } => "capability_denied",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Persistence(#[from] t3_persistence::Error),
    #[error(transparent)]
    Sql(#[from] t3_persistence::rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid persisted {entity}: {detail}")]
    Decode {
        entity: &'static str,
        detail: String,
    },
    #[error("command {command_id:?} rejected: {reason}")]
    Rejected {
        command_id: CommandId,
        reason: String,
    },
    /// A host-side caller's command refused by its authority (`composition::CommandCaller`).
    #[error("{refusal}")]
    CallerRefused {
        command_id: CommandId,
        refusal: CallerRefusal,
    },
    #[error("thread {0:?} not found")]
    ThreadNotFound(ThreadId),
    #[error("stale commit guard")]
    Stale,
    #[error("effect {effect_id} no longer belongs to worker {worker_id}")]
    LeaseLost {
        effect_id: String,
        worker_id: String,
    },
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),
    #[error("operation cancelled")]
    Cancelled,
    #[error("event subscriber lagged; resnapshot required")]
    Lagged,
    #[error("provider: {0}")]
    Provider(#[from] crate::provider::ProviderError),
    #[error(transparent)]
    RuntimeResponse(
        #[from]
        Box<crate::ports::runs_ingestion::runtime_request_service::RuntimeRequestResponseExecutionError>,
    ),
    #[error(transparent)]
    ProviderTurnControl(
        #[from] Box<crate::ports::runs_ingestion::provider_turn_control_service::ProviderTurnControlError>,
    ),
    #[error(transparent)]
    PullRequestHost(PullRequestHostError),
    /// `orchestration.stopWorkflow`'s typed failure, as the wire carries it.
    #[error("stop workflow failed: {0:?}")]
    StopWorkflow(Box<t3_contracts::orchestration_v2::OrchestrationV2StopWorkflowError>),
    /// `orchestration.forceStopThread`'s typed failure, as the wire carries it.
    #[error("force stop failed: {0:?}")]
    ForceStopThread(Box<t3_contracts::orchestration_v2::OrchestrationV2ForceStopThreadError>),
    #[error("service {operation}: {detail}")]
    Service {
        operation: &'static str,
        detail: String,
    },
}
