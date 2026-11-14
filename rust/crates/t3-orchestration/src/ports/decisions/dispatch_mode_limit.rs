use std::collections::{HashMap, HashSet};
use t3_contracts::{
    base_schemas::ThreadId,
    orchestration_v2::OrchestrationV2DomainEvent,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchModes {
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeLimit {
    Runtime,
    Interaction,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModeCeiling {
    pub organization: String,
    pub max_runtime_mode: RuntimeMode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeilingRefused {
    Mode,
    Turn,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModeCeilingRefusal {
    pub thread_id: ThreadId,
    pub runtime_mode: RuntimeMode,
    pub refused: CeilingRefused,
}

pub fn is_runtime_mode_above(mode: RuntimeMode, limit: RuntimeMode) -> bool {
    let rank = |mode| match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    };
    rank(mode) > rank(limit)
}

pub fn exceeded_dispatch_mode_limit(
    limit: DispatchModes,
    modes: DispatchModes,
) -> Option<ModeLimit> {
    if is_runtime_mode_above(modes.runtime_mode, limit.runtime_mode) {
        return Some(ModeLimit::Runtime);
    }
    (modes.interaction_mode == ProviderInteractionMode::Default
        && limit.interaction_mode == ProviderInteractionMode::Plan)
        .then_some(ModeLimit::Interaction)
}

pub fn narrow_to_ceiling(
    limit: Option<DispatchModes>,
    ceiling: Option<&RuntimeModeCeiling>,
) -> Option<DispatchModes> {
    limit.map(|mut limit| {
        if let Some(ceiling) = ceiling
            .filter(|ceiling| is_runtime_mode_above(limit.runtime_mode, ceiling.max_runtime_mode))
        {
            limit.runtime_mode = ceiling.max_runtime_mode;
        }
        limit
    })
}

pub fn runtime_mode_ceiling_refusal(
    max_runtime_mode: RuntimeMode,
    events: &[OrchestrationV2DomainEvent],
    runtime_modes: &HashMap<ThreadId, RuntimeMode>,
) -> Option<RuntimeModeCeilingRefusal> {
    use OrchestrationV2DomainEvent as Event;
    let mut planned = runtime_modes.clone();
    for event in events {
        let (thread_id, runtime_mode, refused) = match event {
            Event::ThreadCreated(event) | Event::ThreadRuntimeModeUpdated(event) => {
                planned.insert(event.thread_id.clone(), event.payload.runtime_mode);
                (
                    &event.thread_id,
                    event.payload.runtime_mode,
                    CeilingRefused::Mode,
                )
            }
            Event::RunCreated(event) => {
                let Some(mode) = planned.get(&event.thread_id) else {
                    continue;
                };
                (&event.thread_id, *mode, CeilingRefused::Turn)
            }
            Event::RunAttemptCreated(event) => {
                let Some(mode) = planned.get(&event.thread_id) else {
                    continue;
                };
                (&event.thread_id, *mode, CeilingRefused::Turn)
            }
            _ => continue,
        };
        if is_runtime_mode_above(runtime_mode, max_runtime_mode) {
            return Some(RuntimeModeCeilingRefusal {
                thread_id: thread_id.clone(),
                runtime_mode,
                refused,
            });
        }
    }
    None
}

pub fn threads_starting_work(events: &[OrchestrationV2DomainEvent]) -> HashSet<ThreadId> {
    use OrchestrationV2DomainEvent as Event;
    let mut created = HashSet::new();
    let mut starting = HashSet::new();
    for event in events {
        let thread_id = match event {
            Event::ThreadCreated(event) => {
                created.insert(event.thread_id.clone());
                continue;
            }
            Event::RunCreated(event) => &event.thread_id,
            Event::RunAttemptCreated(event) => &event.thread_id,
            _ => continue,
        };
        if !created.contains(thread_id) {
            starting.insert(thread_id.clone());
        }
    }
    starting
}

impl RuntimeModeCeiling {
    /// The document's `limits.maxRuntimeMode`, the one source for dispatch and turn checks.
    pub fn from_managed(
        policy: &t3_contracts::managed_policy::ManagedPolicyDocument,
    ) -> Option<Self> {
        Some(Self {
            organization: policy.organization.0.clone(),
            max_runtime_mode: policy.limits.as_ref()?.max_runtime_mode?,
        })
    }

    pub fn message(&self, refused: CeilingRefused) -> String {
        let label = match self.max_runtime_mode {
            RuntimeMode::ApprovalRequired => "Supervised",
            RuntimeMode::AutoAcceptEdits => "Auto-accept edits",
            RuntimeMode::Auto => "Auto",
            RuntimeMode::FullAccess => "Full access",
        };
        let suffix = if refused == CeilingRefused::Turn {
            " Switch this thread's mode to continue."
        } else {
            ""
        };
        format!("{} limits threads to {label}.{suffix}", self.organization)
    }
}
