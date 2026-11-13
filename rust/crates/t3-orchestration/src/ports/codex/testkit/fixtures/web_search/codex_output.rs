#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &["web search fixture complete"],
    item_types: &["web_search", "assistant_message"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_web_search_output(
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
    shared::assert_execution_node_kinds(
        &projection,
        &["root_turn", "tool_call", "assistant_message"],
    )?;
    shared::assert_conversation_message_roles(&projection, &["user", "assistant", "assistant"])?;
    shared::assert_turn_item_types(
        &projection,
        &["user_message", "web_search", "assistant_message"],
    )?;
    shared::assert_runtime_request_counts(&projection, 0, None)?;
    shared::assert_user_messages_include(&projection, &[shared::WEB_SEARCH_PROMPT])?;
    shared::assert_assistant_text_includes(&projection, "web search fixture complete")?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "web_search",
        result,
        transcript,
        &projection,
    )
}

pub const WEB_SEARCH_QUERY: &str = "FIFA World Cup 2026 ticket prices official";
