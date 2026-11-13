use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_pi_message_steering_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let frames = replay_frames(transcript);
    let prompts: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| !*inbound && frame["type"] == "prompt")
        .map(|(_, frame)| json!([frame["message"], frame["streamingBehavior"]]))
        .collect();
    require(
        json!(prompts)
            == json!([
                [MESSAGE_STEERING_INITIAL_PROMPT, "steer"],
                [MESSAGE_STEERING_STEER_PROMPT, "steer"]
            ])
            && !frames
                .iter()
                .any(|(inbound, frame)| !*inbound && frame["type"] == "abort"),
        "Pi steering must queue steer prompts without abort",
    )?;
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
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
    assert_field_sequence(&p, "providerTurns", "status", &["completed"])
}
