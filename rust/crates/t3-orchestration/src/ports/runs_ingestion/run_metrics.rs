use super::provider_event_ingestor::{epoch_millis, expanded_id};
use crate::{api::*, error::Result};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Mutex,
};
use t3_contracts::provider_instance::ProviderInstanceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub run_id: RunId,
    pub provider: ProviderInstanceId,
    pub model_family: Option<&'static str>,
    pub outcome: &'static str,
    pub delegated: bool,
    pub queue_wait_ms: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(Default)]
pub struct RunOutcomeRecorder(Mutex<CountedRuns>);
#[derive(Default)]
struct CountedRuns {
    ids: HashSet<RunId>,
    order: VecDeque<RunId>,
}

impl RunOutcomeRecorder {
    // Prepare within the write transaction; record after commit with an I/O-free callback.
    pub fn prepare(
        events: &[OrchestrationV2StoredEvent],
        mut is_subagent_thread: impl FnMut(&ThreadId) -> Result<bool>,
    ) -> Result<Vec<RunOutcome>> {
        let mut delegated_threads = HashMap::new();
        let mut outcomes = Vec::new();
        for stored in events {
            let OrchestrationV2DomainEvent::RunUpdated(event) = &stored.event else {
                continue;
            };
            let run = &event.payload;
            let outcome = match run.status {
                OrchestrationV2RunStatus::Completed => "completed",
                OrchestrationV2RunStatus::Failed => "failed",
                OrchestrationV2RunStatus::Cancelled => "cancelled",
                OrchestrationV2RunStatus::Interrupted => "interrupted",
                _ => continue,
            };
            let delegated = if run
                .delegated_task_id
                .as_ref()
                .and_then(Option::as_ref)
                .is_some()
            {
                true
            } else {
                let thread = expanded_id(&run.thread_id)?;
                *delegated_threads
                    .entry(thread.clone())
                    .or_insert_with(|| is_subagent_thread(&thread).unwrap_or(false))
            };
            let started = run
                .started_at
                .as_ref()
                .and_then(|date| epoch_millis(&date.0));
            let requested = epoch_millis(&run.requested_at.0);
            let completed = run
                .completed_at
                .as_ref()
                .and_then(|date| epoch_millis(&date.0));
            outcomes.push(RunOutcome {
                run_id: expanded_id(&run.id)?,
                provider: expanded_id(&run.provider_instance_id)?,
                model_family: model_family(&run.model_selection.model),
                outcome,
                delegated,
                queue_wait_ms: started
                    .zip(requested)
                    .map(|(started, requested)| started - requested),
                duration_ms: completed
                    .zip(started)
                    .map(|(completed, started)| completed - started),
            });
        }
        Ok(outcomes)
    }

    pub fn record(
        &self,
        outcomes: impl IntoIterator<Item = RunOutcome>,
        mut record: impl FnMut(&RunOutcome),
    ) {
        let mut counted = self.0.lock().unwrap_or_else(|error| error.into_inner());
        for outcome in outcomes {
            if !counted.ids.insert(outcome.run_id.clone()) {
                continue;
            }
            counted.order.push_back(outcome.run_id.clone());
            if counted.order.len() > 4096
                && let Some(oldest) = counted.order.pop_front()
            {
                counted.ids.remove(&oldest);
            }
            record(&outcome);
        }
    }
}

fn model_family(model: &str) -> Option<&'static str> {
    let model = model.trim().to_lowercase();
    if model.is_empty() {
        return None;
    }
    Some(if model.contains("gpt") {
        "gpt"
    } else if model.contains("claude") {
        "claude"
    } else if model.contains("gemini") {
        "gemini"
    } else {
        "other"
    })
}
