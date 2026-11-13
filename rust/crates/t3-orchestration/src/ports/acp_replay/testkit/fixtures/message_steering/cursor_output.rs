use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::json;

pub fn assert_cursor_message_steering_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    require(
        transcript.provider.0 == "cursor",
        "expected Cursor transcript",
    )?;
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
    let native=transcript.entries.iter().any(|entry|matches!(entry,ReplayEntry::ExpectOutbound(entry) if entry.frame["type"]=="run.steer"));
    let attempts: Vec<_> = rows(&p, "attempts")
        .map(|attempt| json!([attempt["reason"], attempt["status"]]))
        .collect();
    require(
        json!(attempts)
            == if native {
                json!([["initial", "completed"]])
            } else {
                json!([["initial", "superseded"], ["steering_restart", "completed"]])
            },
        "steering attempt reasons/statuses",
    )?;
    assert_field_sequence(
        &p,
        "providerTurns",
        "status",
        if native {
            &["completed"]
        } else {
            &["interrupted", "completed"]
        },
    )?;
    let attempt = rows(&p, "attempts").next_back();
    let run = rows(&p, "runs").next();
    require(
        run.zip(attempt).is_some_and(|(run, attempt)| {
            run["activeAttemptId"] == attempt["id"] && run["rootNodeId"] == attempt["rootNodeId"]
        }),
        "run must reference the final steering attempt",
    )?;
    require(
        !rows(&p, "turnItems").any(|item| {
            matches!(
                item["type"].as_str(),
                Some("run_interrupt_request" | "run_interrupt_result")
            )
        }),
        "steering must not create hard-stop items",
    )
}
