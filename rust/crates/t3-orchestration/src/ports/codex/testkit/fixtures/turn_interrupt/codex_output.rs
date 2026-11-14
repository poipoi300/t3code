#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["interrupted"],
    assistant_texts: &[],
    item_types: &[],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_turn_interrupt_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;
    shared::assert_semantic_projection_integrity(&projection)?;
    shared::assert_visible_turn_items_mirror_local_turn_items(&projection)?;
    shared::assert_turn_item_types(
        &projection,
        &[
            "user_message",
            "run_interrupt_request",
            "run_interrupt_result",
        ],
    )?;
    shared::assert_user_messages_include(&projection, &[shared::TURN_INTERRUPT_PROMPT])?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "turn_interrupt",
        result,
        transcript,
        &projection,
    )
}
