#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed", "completed", "completed"],
    assistant_texts: &["first turn complete"],
    item_types: &["assistant_message"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_thread_rollback_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(
        result,
        transcript,
        3,
        None,
        Some(&["completed", "rolled_back", "completed"]),
    )?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;
    shared::assert_semantic_projection_integrity(&projection)?;
    shared::assert_run_ordinals(&projection, &[1, 2, 3])?;
    shared::assert_turn_item_type_sequence(
        &projection,
        &[
            "user_message",
            "assistant_message",
            "checkpoint",
            "user_message",
            "assistant_message",
            "checkpoint",
            "user_message",
            "assistant_message",
            "checkpoint",
        ],
    )?;
    shared::assert_visible_turn_item_type_sequence(
        &projection,
        &[
            "user_message",
            "assistant_message",
            "checkpoint",
            "user_message",
            "assistant_message",
            "checkpoint",
        ],
    )?;
    shared::assert_user_messages_include(
        &projection,
        &[
            shared::THREAD_ROLLBACK_FIRST_PROMPT,
            shared::THREAD_ROLLBACK_SECOND_PROMPT,
            shared::THREAD_ROLLBACK_AFTER_PROMPT,
        ],
    )?;
    shared::assert_visible_user_messages_include(
        &projection,
        &[
            shared::THREAD_ROLLBACK_FIRST_PROMPT,
            shared::THREAD_ROLLBACK_AFTER_PROMPT,
        ],
    )?;
    shared::assert_visible_user_messages_exclude(
        &projection,
        &[shared::THREAD_ROLLBACK_SECOND_PROMPT],
    )?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "thread_rollback",
        result,
        transcript,
        &projection,
    )
}
