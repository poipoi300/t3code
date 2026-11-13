#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::Value;

pub fn assert_turn_interrupt_claude_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;

    require(
        transcript.provider.0 == "claudeAgent",
        "wrong replay provider",
    )?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "run_interrupt_request",
            "run_interrupt_result",
        ],
    )?;
    assert_user_messages_include(&p, &[TURN_INTERRUPT_PROMPT])?;
    let request = rows(&p, "turnItems")
        .find(|i| i["type"] == "run_interrupt_request")
        .unwrap_or(&Value::Null);
    let result = rows(&p, "turnItems")
        .find(|i| i["type"] == "run_interrupt_result")
        .unwrap_or(&Value::Null);
    require(
        request["status"] == "completed"
            && result["status"] == "interrupted"
            && result["parentItemId"] == request["id"],
        "interrupt request/result linkage differs",
    )?;
    assert_field_sequence(&p, "attempts", "status", &["interrupted"])?;
    assert_field_sequence(&p, "nodes", "status", &["interrupted"])?;
    require(
        p["providerThreads"][0]["status"] == "idle"
            && p["providerTurns"][0]["status"] == "interrupted",
        "native turn/thread did not settle",
    )?;
    Ok(())
}
