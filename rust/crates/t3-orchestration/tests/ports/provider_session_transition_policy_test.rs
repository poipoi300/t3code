use t3_contracts::{
    model_selection::ModelSelection, provider_instance::ProviderDriverKind, provider_policy::*,
};
use t3_orchestration::{provider::SelectionTransitionPlan, runtime::sessions::*};

fn state() -> SessionTransitionState {
    SessionTransitionState {
        driver: ProviderDriverKind("codex".into()),
        continuation_identity: ContinuationIdentity {
            driver: ProviderDriverKind("codex".into()),
            key: "codex:account:one".into(),
        },
        model_selection: serde_json::from_value(
            serde_json::json!({"instanceId":"codex","model":"gpt-5.1-codex"}),
        )
        .expect("test value"),
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        workspace: "/repo".into(),
    }
}

#[test]
fn reuses_sessions_with_turn_scoped_interaction_mode() {
    let current = state();
    let mut target = current.clone();
    target.interaction_mode = ProviderInteractionMode::Plan;
    assert_eq!(
        decide_provider_session_transition(Some(&current), &target, true, None),
        ProviderSessionTransition::Reuse
    );
}
#[test]
fn uses_adapter_classification_for_model_changes_before_workspace_changes() {
    let current = state();
    let mut target = current.clone();
    target.model_selection.model = "gpt-5.2-codex".into();
    for (classification, expected) in [
        (
            SelectionTransitionPlan::ApplyOnNextTurn,
            ProviderSessionTransition::SwitchModelInSession,
        ),
        (
            SelectionTransitionPlan::RestartSession,
            ProviderSessionTransition::RestartAndResume,
        ),
        (
            SelectionTransitionPlan::CreateWithHandoff,
            ProviderSessionTransition::CreateWithHandoff,
        ),
        (
            SelectionTransitionPlan::Reject {
                reason: "unsupported".into(),
            },
            ProviderSessionTransition::Reject {
                reason: "unsupported".into(),
            },
        ),
    ] {
        assert_eq!(
            decide_provider_session_transition(
                Some(&current),
                &target,
                true,
                Some(&classification)
            ),
            expected
        );
    }
    target.workspace = "/other".into();
    assert_eq!(
        decide_provider_session_transition(
            Some(&current),
            &target,
            true,
            Some(&SelectionTransitionPlan::CreateWithHandoff)
        ),
        ProviderSessionTransition::CreateWithHandoff
    );
    assert_eq!(
        decide_provider_session_transition(Some(&current), &target, true, None),
        ProviderSessionTransition::Reject {
            reason: "The provider adapter did not classify the selection change.".into()
        }
    );
}
#[test]
fn restarts_for_workspace_runtime_or_compatible_instance_changes() {
    let current = state();
    let mut targets = [current.clone(), current.clone(), current.clone()];
    targets[0].workspace = "/other".into();
    targets[1].runtime_mode = RuntimeMode::ApprovalRequired;
    targets[2].model_selection.instance_id.0 = "codex_other".into();
    for target in targets {
        assert_eq!(
            decide_provider_session_transition(Some(&current), &target, true, None),
            ProviderSessionTransition::RestartAndResume
        );
    }
}
#[test]
fn hands_off_for_incompatible_identity_and_rejects_unavailable_targets() {
    let current = state();
    let mut target = current.clone();
    target.continuation_identity.key = "other".into();
    assert_eq!(
        decide_provider_session_transition(Some(&current), &target, true, None),
        ProviderSessionTransition::CreateWithHandoff
    );
    assert_eq!(
        decide_provider_session_transition(None, &current, true, None),
        ProviderSessionTransition::CreateWithHandoff
    );
    assert_eq!(
        decide_provider_session_transition(Some(&current), &current, false, None),
        ProviderSessionTransition::Reject {
            reason: "The target provider instance is unavailable.".into()
        }
    );
}
#[test]
fn selection_equality_ignores_option_order_and_empty_options() {
    let parse = |value| serde_json::from_value::<ModelSelection>(value).expect("test value");
    let current = state();
    let mut target = current.clone();
    target.model_selection =
        parse(serde_json::json!({"instanceId":"codex","model":"gpt-5.1-codex","options":[]}));
    assert_eq!(
        decide_provider_session_transition(Some(&current), &target, true, None),
        ProviderSessionTransition::Reuse
    );
    let mut current = current;
    current.model_selection = parse(
        serde_json::json!({"instanceId":"codex","model":"m","options":[{"id":"effort","value":"high"},{"id":"fast","value":false}]}),
    );
    target.model_selection = parse(
        serde_json::json!({"instanceId":"codex","model":"m","options":[{"id":"fast","value":false},{"id":"effort","value":"high"}]}),
    );
    assert_eq!(
        decide_provider_session_transition(Some(&current), &target, true, None),
        ProviderSessionTransition::Reuse
    );
}
