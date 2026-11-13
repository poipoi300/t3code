#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};
pub fn assert_claude_background_task_after_root_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    assert_user_messages_include(
        &p,
        &[super::input::CLAUDE_BACKGROUND_TASK_AFTER_ROOT_PROMPT],
    )?;
    let root = &p["runs"][0];
    let wake = &p["runs"][1];
    require(
        root.is_object() && wake.is_object(),
        "missing root/wake runs",
    )?;
    let events = serde_json::to_value(&result.domain_events)?;
    let events = events
        .as_array()
        .ok_or_else(|| crate::error::Error::Service {
            operation: "replay fixture assertion",
            detail: "missing events".into(),
        })?;
    let pending = events.iter().position(|e| {
        e["type"] == "provider-thread.updated"
            && e["payload"]["pendingBackgroundTasks"]
                .as_array()
                .is_some_and(|tasks| tasks.iter().any(|t| t["taskId"] == "bc9gkn8ei"))
    });
    let waiting = events.iter().position(|e| {
        e["type"] == "run.updated"
            && e["runId"] == root["id"]
            && e["payload"]["status"] == "waiting"
    });
    let node = events.iter().position(|e| {
        e["type"] == "node.updated"
            && e["payload"]["runId"] == root["id"]
            && e["payload"]["kind"] == "root_turn"
            && e["payload"]["status"] == "waiting"
    });
    let created = events
        .iter()
        .position(|e| e["type"] == "run.created" && e["payload"]["id"] == wake["id"]);
    let cleared = events.iter().enumerate().find_map(|(i, e)| {
        (Some(i) > waiting
            && e["type"] == "provider-thread.updated"
            && e["payload"]["pendingBackgroundTasks"]
                .as_array()
                .is_some_and(Vec::is_empty))
        .then_some(i)
    });
    require(
        [pending, waiting, node, created, cleared]
            .iter()
            .all(Option::is_some)
            && waiting > pending
            && node > pending
            && created > waiting
            && cleared > created,
        "background completion was cleared before the wake consumed it",
    )?;
    require(
        root["status"] == "completed"
            && rows(&p, "nodes").any(|n| {
                n["runId"] == root["id"] && n["kind"] == "root_turn" && n["status"] == "completed"
            }),
        "root run/node did not settle",
    )?;
    require(
        rows(&p, "providerThreads").count() == 1
            && p["providerThreads"][0]["status"] == "idle"
            && p["providerThreads"][0]["pendingBackgroundTasks"]
                .as_array()
                .is_none_or(Vec::is_empty)
            && rows(&p, "subagents").count() == 0,
        "incorrect final native thread state",
    )?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|i| i["type"] == "assistant_message")
                .map(|i| i["text"].clone())
                .collect::<Vec<_>>()
        ) == json!(["L2_STARTED", "L2_WAKE: L2_BG_DONE"]),
        "late wake reply order differs",
    )?;
    let shell = serde_json::to_value(&result.shell_snapshot)?;
    let shell = rows(&shell, "threads")
        .find(|t| t["id"] == p["thread"]["id"])
        .unwrap_or(&Value::Null);
    require(
        shell.is_object()
            && shell["activeRunId"].is_null()
            && shell["status"] == "completed"
            && shell["pendingBackgroundTasks"]
                .as_array()
                .is_none_or(Vec::is_empty),
        "completed shell retains an active run/background task",
    )?;

    Ok(())
}
