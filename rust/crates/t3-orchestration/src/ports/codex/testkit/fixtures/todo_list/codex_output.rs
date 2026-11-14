#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &["todo list fixture complete"],
    item_types: &["todo_list", "command_execution"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_todo_list_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;
    shared::assert_semantic_projection_integrity(&projection)?;
    shared::assert_visible_turn_items_mirror_local_turn_items(&projection)?;
    shared::assert_run_ordinals(&projection, &[1])?;
    shared::assert_execution_node_kinds(&projection, &["root_turn", "todo_list", "tool_call"])?;
    shared::assert_turn_item_types(
        &projection,
        &["user_message", "todo_list", "command_execution"],
    )?;
    shared::assert_user_messages_include(&projection, &[shared::TODO_LIST_PROMPT])?;
    shared::assert_assistant_text_includes(&projection, "todo list fixture complete")?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "todo_list",
        result,
        transcript,
        &projection,
    )
}
