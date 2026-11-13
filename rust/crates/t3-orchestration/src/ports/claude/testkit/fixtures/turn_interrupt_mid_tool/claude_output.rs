#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::Value;

pub fn assert_claude_interrupt_after_tool_use(transcript: &ReplayTranscript) -> Result<()> {
    let tool=transcript.entries.iter().position(|e|matches!(e,ReplayEntry::EmitInbound(e) if e.frame["type"]=="assistant"&&e.frame["message"]["content"].as_array().is_some_and(|c|c.iter().any(|b|b["type"]=="tool_use"))));
    let interrupt = transcript.entries.iter().position(
        |e| matches!(e,ReplayEntry::ExpectOutbound(e) if e.frame["type"]=="query.interrupt"),
    );
    require(
        tool.is_some() && interrupt.is_some() && interrupt > tool,
        "interrupt must follow a recorded tool start",
    )
}

pub fn assert_turn_interrupt_mid_tool_claude_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    require(
        transcript.provider.0 == "claudeAgent",
        "wrong replay provider",
    )?;
    assert_claude_interrupt_after_tool_use(transcript)?;
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
    let command = rows(&p, "turnItems")
        .find(|i| i["type"] == "command_execution")
        .unwrap_or(&Value::Null);
    let request = rows(&p, "turnItems")
        .find(|i| i["type"] == "run_interrupt_request")
        .unwrap_or(&Value::Null);
    let stopped = rows(&p, "turnItems")
        .find(|i| i["type"] == "run_interrupt_result")
        .unwrap_or(&Value::Null);
    require(
        command["status"] == "interrupted"
            && command["input"]
                .as_str()
                .is_some_and(|s| s.contains("node -e")),
        "foreground command did not interrupt",
    )?;
    require(
        request["status"] == "completed"
            && stopped["status"] == "interrupted"
            && stopped["parentItemId"] == request["id"],
        "interrupt linkage differs",
    )?;
    assert_field_sequence(&p, "attempts", "status", &["interrupted"])?;
    require(
        p["providerThreads"][0]["status"] == "idle"
            && p["providerTurns"][0]["status"] == "interrupted",
        "native interrupt did not settle",
    )?;

    Ok(())
}
