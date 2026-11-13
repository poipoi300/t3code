#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &["Written."],
    item_types: &[
        "subagent",
        "approval_request",
        "command_execution",
        "assistant_message",
    ],
    request_kinds: &["command"],
    subagent_results: Some(&["Written.", "Written."]),
};

pub fn assert_subagent_v2_nested_approval_output(
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
    shared::assert_user_messages_include(
        &projection,
        &[shared::SUBAGENT_V2_NESTED_APPROVAL_PROMPT],
    )?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "subagent_v2_nested_approval",
        result,
        transcript,
        &projection,
    )
}
