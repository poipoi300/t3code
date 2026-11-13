use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_stop_background_work_after_failed_turn_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["failed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(
        &p,
        &[super::input::STOP_BACKGROUND_WORK_AFTER_FAILED_TURN_PROMPT],
    )?;
    let before = result
        .captured_shell_snapshots
        .get("before-stop")
        .map(serde_json::to_value)
        .transpose()?;
    require(
        before
            .as_ref()
            .and_then(|shell| {
                rows(shell, "threads").find(|thread| thread["id"] == p["thread"]["id"])
            })
            .is_some_and(|thread| {
                json!(values(thread, "pendingBackgroundTasks", "kind")) == json!(["command"])
            }),
        "failed turn must retain pending command before Stop",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "command_execution")
            .is_some_and(|item| item["status"] == "interrupted" && !item["completedAt"].is_null()),
        "Stop must interrupt the retained command",
    )?;
    let shell = serde_json::to_value(&result.shell_snapshot)?;
    require(
        rows(&shell, "threads")
            .find(|thread| thread["id"] == p["thread"]["id"])
            .is_none_or(|thread| rows(thread, "pendingBackgroundTasks").next().is_none()),
        "Stop must clear failed turn's background work",
    )?;
    assert_turn_item_types(&p, &["run_interrupt_request"])?;
    require(
        p["runs"][0]["status"] == "failed",
        "stopping background work must retain failed run status",
    )
}
