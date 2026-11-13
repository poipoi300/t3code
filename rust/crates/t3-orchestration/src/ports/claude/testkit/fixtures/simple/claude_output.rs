#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};

pub fn assert_simple_claude_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1])?;
    assert_execution_node_kinds(&p, &["root_turn", "assistant_message"])?;
    assert_conversation_message_roles(&p, &["user", "assistant"])?;
    assert_turn_item_types(&p, &["user_message", "assistant_message"])?;
    assert_user_messages_include(&p, &[SIMPLE_PROMPT])?;
    assert_assistant_text_includes(&p, "fixture simple ok")?;
    Ok(())
}
