use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_tool_call_read_only_cursor_output(
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
    let messages: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "assistant_message")
        .collect();
    require(
        json!(
            messages
                .iter()
                .map(|item| item["text"].as_str().unwrap_or_default().trim_end())
                .collect::<Vec<_>>()
        ) == json!(["Reading both files now.", "read only tool fixture complete"]),
        "Cursor progress and final response must remain separate messages",
    )?;
    let reads: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "dynamic_tool")
        .collect();
    require(reads.len() == 2, "expected two file reads")?;
    require(
        messages[0]["ordinal"].as_f64() < reads[0]["ordinal"].as_f64()
            && reads[1]["ordinal"].as_f64() < messages[1]["ordinal"].as_f64(),
        "read/progress item order",
    )?;
    for expected in ["cursor-read-only-fixture", "ES2022"] {
        require(
            reads
                .iter()
                .any(|item| item["output"].to_string().contains(expected)),
            format!("missing tool output {expected}"),
        )?;
    }
    let mut paths: Vec<_> = reads
        .iter()
        .map(|item| item["input"]["path"].as_str().unwrap_or_default())
        .collect();
    paths.sort();
    require(
        paths
            == [
                "/tmp/claude-replay-tool_call_read_only/package.json",
                "/tmp/claude-replay-tool_call_read_only/tsconfig.json",
            ],
        "Cursor paths differ from the recorded prompt",
    )
}
