#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};

pub fn assert_claude_message_steering_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    require(
        transcript.provider.0 == "claudeAgent",
        "wrong replay provider",
    )?;
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
        rows(&p, "runs").count() == 1 && rows(&p, "providerTurns").count() == 1,
        "steering must share the active run and provider turn",
    )?;
    Ok(())
}
