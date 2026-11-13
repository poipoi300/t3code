#![cfg(feature = "claude-adapter")]
use super::super::turn_interrupt_mid_tool::claude_output::assert_claude_interrupt_after_tool_use;
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::Value;

pub fn assert_turn_interrupt_restart_claude_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["interrupted", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    require(
        transcript.provider.0 == "claudeAgent",
        "wrong replay provider",
    )?;
    assert_claude_interrupt_after_tool_use(transcript)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1, 2])?;
    assert_conversation_message_roles(&p, &["user", "user", "assistant"])?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "command_execution",
            "run_interrupt_request",
            "run_interrupt_result",
            "assistant_message",
        ],
    )?;
    assert_user_messages_include(
        &p,
        &[
            TURN_INTERRUPT_MID_TOOL_PROMPT,
            TURN_INTERRUPT_RECOVERY_PROMPT,
        ],
    )?;
    assert_assistant_text_includes(&p, "interrupt recovery fixture complete")?;
    assert_field_sequence(&p, "attempts", "status", &["interrupted", "completed"])?;
    assert_field_sequence(&p, "providerTurns", "status", &["interrupted", "completed"])?;
    require(
        p["providerThreads"][0]["status"] == "idle",
        "native thread not idle after recovery",
    )?;
    let command = rows(&p, "turnItems")
        .find(|i| i["type"] == "command_execution")
        .unwrap_or(&Value::Null);
    require(
        command["status"] == "interrupted"
            && command["input"]
                .as_str()
                .is_some_and(|s| s.contains("node -e")),
        "interrupted command changed on restart",
    )?;
    let frames = transcript
        .entries
        .iter()
        .filter_map(|e| match e {
            ReplayEntry::ExpectOutbound(e) => Some(&e.frame),
            _ => None,
        })
        .collect::<Vec<_>>();
    let opens = frames
        .iter()
        .filter(|f| f["type"] == "query.open")
        .collect::<Vec<_>>();
    require(
        opens.len() == 2 && opens[1]["options"]["resume"].is_string(),
        "restart must resume the native conversation",
    )?;
    require(
        frames
            .iter()
            .filter(|f| f["type"] == "query.interrupt")
            .count()
            == 1,
        "expected one native interrupt",
    )?;

    Ok(())
}
