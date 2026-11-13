use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::json;

pub fn assert_turn_interrupt_mid_tool_pi_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let tool=transcript.entries.iter().position(|entry|matches!(entry,ReplayEntry::EmitInbound(entry) if entry.frame["type"]=="tool_execution_start"));
    let abort = transcript.entries.iter().position(
        |entry| matches!(entry,ReplayEntry::ExpectOutbound(entry) if entry.frame["type"]=="abort"),
    );
    require(
        tool.zip(abort).is_some_and(|(tool, abort)| abort > tool),
        "Pi Stop must follow tool start",
    )?;
    assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "command_execution",
            "run_interrupt_request",
            "run_interrupt_result",
        ],
    )?;
    assert_user_messages_include(&p, &[TURN_INTERRUPT_MID_TOOL_PROMPT])?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "command_execution")
            .is_some_and(|item| {
                item["input"]
                    .as_str()
                    .is_some_and(|input| input.contains("node -e"))
                    && item["status"] == "interrupted"
            }),
        "command must become interrupted",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "run_interrupt_result")
            .is_some_and(|item| item["status"] == "interrupted"),
        "interrupt result status",
    )?;
    assert_field_sequence(&p, "providerTurns", "status", &["interrupted"])?;
    require(
        p["providerTurns"][0]["nativeTurnRef"]["strength"] == "strong",
        "stopped native turn identity",
    )?;
    require(
        json!(
            rows(&p, "providerSessions")
                .map(|session| json!([session["status"], session["lastError"]]))
                .collect::<Vec<_>>()
        ) == json!([["stopped", null]]),
        "Stop-with-restart must retire Pi without reporting an error",
    )
}
