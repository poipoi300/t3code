#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &["fixture simple ok"],
    item_types: &["assistant_message"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_simple_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;
    shared::assert_semantic_projection_integrity(&projection)?;
    shared::assert_visible_turn_items_mirror_local_turn_items(&projection)?;
    shared::assert_run_ordinals(&projection, &[1])?;
    shared::assert_execution_node_kinds(&projection, &["root_turn", "assistant_message"])?;
    shared::assert_conversation_message_roles(&projection, &["user", "assistant"])?;
    shared::assert_turn_item_types(&projection, &["user_message", "assistant_message"])?;
    shared::assert_user_messages_include(&projection, &[shared::SIMPLE_PROMPT])?;
    shared::assert_assistant_text_includes(&projection, "fixture simple ok")?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "simple",
        result,
        transcript,
        &projection,
    )
}
