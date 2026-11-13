#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed"],
    assistant_texts: &["plan questions fixture complete"],
    item_types: &["user_input_request", "assistant_message"],
    request_kinds: &["user_input"],
    subagent_results: None,
};

pub fn assert_plan_questions_output_base(
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
    shared::assert_run_ordinals(&projection, &[1])?;
    shared::assert_execution_node_kinds(
        &projection,
        &["root_turn", "user_input_request", "assistant_message"],
    )?;
    shared::assert_runtime_request_counts(&projection, 1, Some(1))?;
    shared::assert_runtime_request_kinds(&projection, &["user_input"])?;
    shared::assert_all_runtime_requests_resolved(&projection)?;
    shared::assert_turn_item_types(
        &projection,
        &["user_message", "user_input_request", "assistant_message"],
    )?;
    shared::assert_user_messages_include(&projection, &[shared::PLAN_QUESTIONS_PROMPT])?;
    shared::assert_assistant_text_includes(&projection, "plan questions fixture complete")?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "plan_questions_base",
        result,
        transcript,
        &projection,
    )
}

pub fn assert_plan_questions_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    assert_plan_questions_output_base(result, transcript)?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::assert_question_id(
        result,
        transcript,
        "schema_vs_ui_flexibility",
    )
}
pub fn assert_codex_plan_questions_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    assert_plan_questions_output_base(result, transcript)?;
    crate::ports::codex::adapters::codex_adapter_v2_testkit::assert_question_id(
        result,
        transcript,
        "schema_preference",
    )
}
