use std::collections::BTreeSet;
use t3_contracts::{orchestration_v2::OrchestrationV2InternalCommand, wire::TaggedUnion};
use t3_orchestration::{
    decision::engine::command_decision,
    ports::decisions::orchestrator::should_prepare_legacy_import_handoff,
};

#[test]
fn imported_context_is_reissued_until_a_v2_run_completes_unless_native_history_exists() {
    assert!(should_prepare_legacy_import_handoff(
        Some("v1_import"),
        false,
        false,
        2
    ));
    assert!(!should_prepare_legacy_import_handoff(
        Some("v1_import"),
        true,
        false,
        2
    ));
    assert!(!should_prepare_legacy_import_handoff(
        Some("v1_import"),
        false,
        true,
        2
    ));
    assert!(!should_prepare_legacy_import_handoff(None, false, false, 2));
    assert!(!should_prepare_legacy_import_handoff(
        Some("v1_import"),
        false,
        false,
        0
    ));
}

#[test]
fn every_fixture_command_has_a_decision() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/orchestration.json")).unwrap();
    let mut tags: BTreeSet<_> = fixture["OrchestrationV2Command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["tag"].as_str().unwrap().to_owned())
        .collect();
    tags.extend(
        OrchestrationV2InternalCommand::KNOWN_TAGS
            .iter()
            .map(|tag| (*tag).to_owned()),
    );
    let count = tags.len();
    let missing: Vec<_> = tags
        .into_iter()
        .filter(|tag| command_decision(tag).is_none())
        .collect();
    eprintln!(
        "Command decision coverage: {}/{} tags",
        count - missing.len(),
        count
    );
    assert!(
        missing.is_empty(),
        "commands with no decision: {}",
        missing.join(", ")
    );
}
