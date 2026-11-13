use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_malformed_session_update_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[super::input::MALFORMED_SESSION_UPDATE_PROMPT])?;
    assert_turn_item_types(&p, &["user_message", "assistant_message"])?;
    require(
        !rows(&p, "turnItems").any(|item| item["type"] == "error"),
        "malformed update must not become an error item",
    )?;
    assert_assistant_text_includes(&p, "malformed update fixture ok")
}
