#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &[],
    item_types: &["command_execution", "assistant_message"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_tool_call_workspace_never_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;
    shared::assert_semantic_projection_integrity(&projection)?;
    shared::assert_visible_turn_items_mirror_local_turn_items(&projection)?;
    shared::assert_turn_item_types(
        &projection,
        &["user_message", "command_execution", "assistant_message"],
    )?;
    shared::assert_user_messages_include(&projection, &[shared::TOOL_CALL_WRITE_PROMPT])?;
    shared::assert_runtime_request_counts(&projection, 0, None)?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "tool_call_workspace_never",
        result,
        transcript,
        &projection,
    )
}
