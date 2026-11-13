#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed", "completed"],
    assistant_texts: &["initial subagent response", "continued subagent response"],
    item_types: &["subagent", "assistant_message"],
    request_kinds: &[],
    subagent_results: Some(&["continued subagent response"]),
};

pub fn assert_subagent_continue_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;
    shared::assert_semantic_projection_integrity(&projection)?;
    shared::assert_user_messages_include(
        &projection,
        &[
            shared::SUBAGENT_CONTINUE_PROMPT,
            shared::SUBAGENT_CONTINUE_PARENT_PROMPT,
        ],
    )?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "subagent_continue",
        result,
        transcript,
        &projection,
    )
}
