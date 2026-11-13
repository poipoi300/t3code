use crate::provider::SelectionTransitionPlan;
use t3_contracts::{
    model_selection::ModelSelection,
    provider_instance::ProviderDriverKind,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSessionTransition {
    Reuse,
    SwitchModelInSession,
    RestartAndResume,
    CreateWithHandoff,
    Reject { reason: String },
}

#[derive(Debug, Clone)]
pub struct ContinuationIdentity {
    pub driver: ProviderDriverKind,
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct SessionTransitionState {
    pub driver: ProviderDriverKind,
    pub continuation_identity: ContinuationIdentity,
    pub model_selection: ModelSelection,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub workspace: String,
}

pub fn model_selections_equal(left: &ModelSelection, right: &ModelSelection) -> bool {
    if left.instance_id != right.instance_id || left.model != right.model {
        return false;
    }
    let left = left.options.as_ref().map(|v| v.0.as_slice()).unwrap_or(&[]);
    let right = right
        .options
        .as_ref()
        .map(|v| v.0.as_slice())
        .unwrap_or(&[]);
    left.len() == right.len()
        && left.iter().all(|option| {
            left.iter().filter(|candidate| *candidate == option).count()
                == right
                    .iter()
                    .filter(|candidate| *candidate == option)
                    .count()
        })
}

pub fn decide_provider_session_transition(
    current: Option<&SessionTransitionState>,
    target: &SessionTransitionState,
    available: bool,
    selection_transition: Option<&SelectionTransitionPlan>,
) -> ProviderSessionTransition {
    use ProviderSessionTransition::*;
    if !available {
        return Reject {
            reason: "The target provider instance is unavailable.".into(),
        };
    }
    let Some(current) = current else {
        return CreateWithHandoff;
    };
    if current.driver != target.driver
        || current.continuation_identity.driver != target.continuation_identity.driver
        || current.continuation_identity.key != target.continuation_identity.key
    {
        return CreateWithHandoff;
    }
    let instance_changed =
        current.model_selection.instance_id != target.model_selection.instance_id;
    let selection_changed =
        !model_selections_equal(&current.model_selection, &target.model_selection);
    if selection_changed && !instance_changed {
        match selection_transition {
            Some(SelectionTransitionPlan::CreateWithHandoff) => return CreateWithHandoff,
            Some(SelectionTransitionPlan::Reject { reason }) => {
                return Reject {
                    reason: reason.clone(),
                };
            }
            None => {
                return Reject {
                    reason: "The provider adapter did not classify the selection change.".into(),
                };
            }
            _ => {}
        }
    }
    if instance_changed
        || current.runtime_mode != target.runtime_mode
        || current.workspace != target.workspace
    {
        return RestartAndResume;
    }
    if selection_changed {
        match selection_transition {
            Some(SelectionTransitionPlan::ApplyOnNextTurn) => return SwitchModelInSession,
            Some(SelectionTransitionPlan::RestartSession) => return RestartAndResume,
            _ => {}
        }
    }
    Reuse
}
