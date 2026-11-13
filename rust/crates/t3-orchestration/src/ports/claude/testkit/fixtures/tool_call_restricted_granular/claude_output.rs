#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};

pub fn assert_tool_call_restricted_granular_claude_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "command_execution",
            "approval_request",
            "assistant_message",
        ],
    )?;
    assert_user_messages_include(&p, &[TOOL_CALL_WRITE_PROMPT])?;
    assert_assistant_text_includes(&p, "codex app-server approval fixture")?;
    assert_runtime_request_counts(&p, 1, Some(1))?;
    assert_runtime_request_kinds(&p, &["command"])?;
    assert_all_runtime_requests_resolved(&p)?;
    assert_replay_label_prefix_count(transcript, "permission.request:", 1)?;
    assert_replay_label_prefix_count(transcript, "permission.response:", 1)?;
    Ok(())
}
