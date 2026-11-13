#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &["Hello."],
    item_types: &["subagent", "assistant_message"],
    request_kinds: &[],
    subagent_results: Some(&["Hello."]),
};

pub fn assert_subagent_v2_output(
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
    shared::assert_turn_item_types(
        &projection,
        &["user_message", "subagent", "assistant_message"],
    )?;
    shared::assert_execution_node_kinds(&projection, &["root_turn", "subagent"])?;
    shared::assert_user_messages_include(&projection, &[shared::SUBAGENT_V2_PROMPT])?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "subagent_v2",
        result,
        transcript,
        &projection,
    )
}
