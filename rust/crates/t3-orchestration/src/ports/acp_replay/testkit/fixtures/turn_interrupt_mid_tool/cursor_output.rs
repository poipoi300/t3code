use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};

pub fn assert_turn_interrupt_mid_tool_cursor_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let tool=transcript.entries.iter().position(|entry|matches!(entry,ReplayEntry::EmitInbound(entry) if entry.frame["type"]=="interaction.update" && entry.frame["update"]["type"]=="tool-call-started"));
    let cancel=transcript.entries.iter().position(|entry|matches!(entry,ReplayEntry::ExpectOutbound(entry) if entry.frame["type"]=="run.cancel"));
    require(
        tool.zip(cancel).is_some_and(|(tool, cancel)| cancel > tool),
        "cancel must follow the first tool start",
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
    let command = rows(&p, "turnItems")
        .find(|item| item["type"] == "command_execution")
        .expect("fixture structure validated above");
    let request = rows(&p, "turnItems")
        .find(|item| item["type"] == "run_interrupt_request")
        .expect("fixture structure validated above");
    let result = rows(&p, "turnItems")
        .find(|item| item["type"] == "run_interrupt_result")
        .expect("fixture structure validated above");
    require(
        command["input"]
            .as_str()
            .is_some_and(|text| text.contains("node -e"))
            && command["status"] == "interrupted",
        "command must be interrupted",
    )?;
    require(
        request["status"] == "completed"
            && result["status"] == "interrupted"
            && result["parentItemId"] == request["id"],
        "interrupt item status/parent",
    )?;
    assert_field_sequence(&p, "attempts", "status", &["interrupted"])?;
    require(
        p["providerThreads"][0]["status"] == "idle"
            && p["providerTurns"][0]["status"] == "interrupted",
        "provider must stop its turn and return idle",
    )
}
