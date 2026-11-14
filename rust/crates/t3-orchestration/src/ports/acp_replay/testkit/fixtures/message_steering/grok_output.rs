use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

// fx and ACP Registry still use this interrupt/restart assertion after Grok switched to queueing.
pub fn assert_grok_message_steering_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_turn_item_types(&p, &["user_message", "assistant_message"])?;
    assert_runtime_request_counts(&p, 0, None)?;
    assert_user_messages_include(
        &p,
        &[
            MESSAGE_STEERING_INITIAL_PROMPT,
            MESSAGE_STEERING_STEER_PROMPT,
        ],
    )?;
    assert_user_message_input_intents(&p, &["turn_start", "steer"])?;
    assert_assistant_text_includes(&p, "steering fixture observed")?;
    require(
        json!(
            rows(&p, "attempts")
                .map(|attempt| json!([attempt["reason"], attempt["status"]]))
                .collect::<Vec<_>>()
        ) == json!([["initial", "superseded"], ["steering_restart", "completed"]]),
        "Grok steering attempt lifecycle",
    )?;
    assert_field_sequence(&p, "providerTurns", "status", &["interrupted", "completed"])?;
    require(
        p["runs"][0]["activeAttemptId"] == p["attempts"][1]["id"]
            && p["runs"][0]["rootNodeId"] == p["attempts"][1]["rootNodeId"],
        "steered run must reference the replacement attempt",
    )?;
    require(
        !rows(&p, "turnItems").any(|item| {
            matches!(
                item["type"].as_str(),
                Some("run_interrupt_request" | "run_interrupt_result")
            )
        }),
        "steering must not project hard-stop items",
    )
}
