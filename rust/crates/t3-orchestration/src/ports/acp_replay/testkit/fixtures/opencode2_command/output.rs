use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code2_command_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[OPENCODE2_COMMAND_PROMPT])?;
    assert_assistant_text_includes(&p, "HELLO WORLD")?;
    assert_field_sequence(&p, "providerTurns", "status", &["completed"])
}
