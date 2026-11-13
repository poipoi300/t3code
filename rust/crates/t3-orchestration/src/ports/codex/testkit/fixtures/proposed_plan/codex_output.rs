#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &[],
    item_types: &["proposed_plan"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_proposed_plan_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(
        result,
        transcript,
        1,
        None,
        Some(&["completed"]),
    )?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;
    shared::assert_semantic_projection_integrity(&projection)?;
    shared::assert_visible_turn_items_mirror_local_turn_items(&projection)?;
    shared::assert_run_ordinals(&projection, &[1])?;
    shared::assert_execution_node_kinds(&projection, &["root_turn", "plan"])?;
    shared::assert_turn_item_types(&projection, &["user_message", "proposed_plan"])?;
    shared::assert_user_messages_include(&projection, &[shared::PROPOSED_PLAN_PROMPT])?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "proposed_plan",
        result,
        transcript,
        &projection,
    )
}
