#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed", "completed"],
    assistant_texts: &[
        "first fixture turn complete",
        "second fixture turn complete",
    ],
    item_types: &["assistant_message"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_queued_turn_output(
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
    shared::assert_visible_turn_items_mirror_local_turn_items(&projection)?;
    shared::assert_run_ordinals(&projection, &[1, 2])?;
    shared::assert_conversation_message_roles(
        &projection,
        &["user", "assistant", "user", "assistant"],
    )?;
    shared::assert_turn_item_types(&projection, &["user_message", "assistant_message"])?;
    shared::assert_user_messages_include(
        &projection,
        &[
            shared::MULTI_TURN_FIRST_PROMPT,
            shared::MULTI_TURN_SECOND_PROMPT,
        ],
    )?;
    shared::assert_user_message_input_intents(&projection, &["turn_start", "queued_turn"])?;
    shared::assert_assistant_text_includes(&projection, "first fixture turn complete")?;
    shared::assert_assistant_text_includes(&projection, "second fixture turn complete")?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "queued_turn",
        result,
        transcript,
        &projection,
    )
}
