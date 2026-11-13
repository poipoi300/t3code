use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_kiro_message_steering_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let frames = replay_frames(transcript);
    let counts: Vec<_> = ["session/prompt", "_session/steer", "session/cancel"]
        .iter()
        .map(|method| {
            frames
                .iter()
                .filter(|(inbound, frame)| !*inbound && frame["method"] == *method)
                .count()
        })
        .collect();
    require(
        counts == [1, 1, 0],
        "Kiro steering must be native, without cancel/re-prompt",
    )?;
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(
        &p,
        &[
            super::input::KIRO_MESSAGE_STEERING_PROMPT,
            MESSAGE_STEERING_STEER_PROMPT,
        ],
    )?;
    assert_user_message_input_intents(&p, &["turn_start", "steer"])?;
    assert_assistant_text_includes(&p, "steering fixture observed")?;
    let text = rows(&p, "turnItems")
        .filter(|item| item["type"] == "assistant_message")
        .filter_map(|item| item["text"].as_str())
        .collect::<String>();
    require(
        !text.contains("[STEERING") && rows(&p, "providerTurns").count() == 1,
        "native steering must omit Kiro UI notes and preserve the provider turn",
    )
}
