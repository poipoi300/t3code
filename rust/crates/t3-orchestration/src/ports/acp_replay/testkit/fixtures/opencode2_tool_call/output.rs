use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_open_code2_tool_call_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_turn_item_type_sequence(
        &p,
        &[
            "user_message",
            "assistant_message",
            "dynamic_tool",
            "command_execution",
            "assistant_message",
            "checkpoint",
        ],
    )?;
    assert_user_messages_include(&p, &[OPENCODE2_TOOL_CALL_PROMPT])?;
    assert_assistant_text_includes(&p, "DONE")?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "dynamic_tool")
            .is_some_and(|item| {
                item["status"] == "completed"
                    && item["title"] == "Read hello.txt"
                    && item["output"]
                        .as_str()
                        .is_some_and(|text| text.contains("hello from the spike"))
            }),
        "read tool output",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "command_execution")
            .is_some_and(|item| {
                item["status"] == "completed"
                    && item["input"] == "echo TOOL_OK"
                    && item["output"] == "TOOL_OK\n"
                    && item["exitCode"] == 0
            }),
        "shell output/exit code",
    )?;
    assert_includes_fields(
        &p["providerTurns"][0]["turnTokenUsage"],
        &json!({"usageStatus":"complete","inputTokens":8767+487+45+9311,"outputTokens":59+33}),
        "summed turn usage",
    )?;
    assert_includes_fields(
        &p["providerTurns"][0]["tokenUsage"],
        &json!({"usedTokens":45+9311+33,"maxTokens":160000}),
        "latest context usage",
    )
}
