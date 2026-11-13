#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &[],
    item_types: &["file_change", "approval_request", "assistant_message"],
    request_kinds: &["file-change"],
    subagent_results: None,
};

pub fn assert_tool_call_restricted_granular_output(
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
    shared::assert_turn_item_types(
        &projection,
        &[
            "user_message",
            "file_change",
            "approval_request",
            "assistant_message",
        ],
    )?;
    shared::assert_runtime_request_counts(&projection, 1, Some(1))?;
    shared::assert_runtime_request_kinds(&projection, &["file-change"])?;
    shared::assert_all_runtime_requests_resolved(&projection)?;
    shared::assert_user_messages_include(&projection, &[shared::TOOL_CALL_WRITE_PROMPT])?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "tool_call_restricted_granular",
        result,
        transcript,
        &projection,
    )
}
