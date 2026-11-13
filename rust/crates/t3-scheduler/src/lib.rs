//! Native scheduled tasks, SQLite persistence and transport-neutral dispatch seams.
//! This crate does not target wasm32-unknown-unknown.

mod clock;
mod engine;
mod relay;
pub mod schedule;
mod service;
mod store;
mod validation;
pub mod webhook;
mod webhook_service;

pub use clock::{Clock, SystemClock, TokioClock};
pub use engine::{DueHint, Registration, Scheduler, SchedulerSource};
pub use relay::RelayDeliveryProof;
pub use service::TaskSubscription;
pub use service::{
    DispatchReceipt, DispatchRequest, Orchestration, RunNowResult, ScheduledTaskService,
    SchedulerOptions, Trigger,
};
pub use webhook_service::{TriggerResult, VerifiedRelayDelivery, WebhookTriggerRequest};
pub use webhook_service::{WEBHOOK_MAX_BODY_BYTES, relay_hook_base_url};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Persistence(#[from] t3_persistence::Error),
    #[error(transparent)]
    Sqlite(#[from] t3_persistence::rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Schedule task not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("Schedule task is already running: {0}")]
    AlreadyRunning(String),
    #[error("{0}")]
    Dispatch(String),
    #[error("{0}")]
    Skipped(String),
    #[error("The run started, but completion could not be recorded: {0}")]
    CompletionAfterDispatch(Box<Error>),
    #[error("{0}")]
    Policy(String),
    #[error("Scheduler has shut down")]
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
