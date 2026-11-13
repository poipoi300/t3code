#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};

pub fn assert_tool_call_read_only_claude_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_turn_item_types(&p, &["user_message", "dynamic_tool", "assistant_message"])?;
    assert_user_messages_include(&p, &[TOOL_CALL_READ_ONLY_PROMPT])?;
    assert_assistant_text_includes(&p, "read only tool fixture complete")?;
    assert_runtime_request_counts(&p, 0, None)?;
    assert_replay_label_prefix_count(transcript, "permission.request:", 0)?;
    assert_replay_label_prefix_count(transcript, "permission.response:", 0)?;
    let recording = serde_json::to_string(&transcript.entries)?;
    require(
        !recording.contains("\"is_error\":true") && !recording.contains("File does not exist"),
        "read-only replay must have no native read errors",
    )?;
    let tools = rows(&p, "turnItems")
        .filter(|i| i["type"] == "dynamic_tool")
        .collect::<Vec<_>>();
    require(
        tools.len() == 2 && tools.iter().all(|i| i["toolName"] == "Read"),
        "expected two Read tools and no discovery Glob",
    )?;
    for text in ["claude-read-only-fixture", "ESNext"] {
        require(
            tools.iter().any(|i| i["output"].to_string().contains(text)),
            format!("missing read output {text}"),
        )?;
    }
    require(
        !rows(&p, "turnItems").any(|i| i["type"] == "approval_request"),
        "unexpected read approval",
    )?;
    Ok(())
}
