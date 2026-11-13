#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};
pub fn assert_claude_background_task_interrupt_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    assert_user_messages_include(&p, &[super::input::CLAUDE_BACKGROUND_TASK_INTERRUPT_PROMPT])?;
    let events = serde_json::to_value(&result.domain_events)?;
    let events = events
        .as_array()
        .ok_or_else(|| crate::error::Error::Service {
            operation: "replay fixture assertion",
            detail: "missing events".into(),
        })?;
    let roster = events.iter().position(|e| {
        e["type"] == "provider-thread.updated"
            && e["payload"]["pendingBackgroundTasks"]
                .as_array()
                .is_some_and(|tasks| tasks.iter().any(|t| t["taskId"] == "bt3kw2qpn"))
    });
    let interrupted = events
        .iter()
        .position(|e| e["type"] == "run.updated" && e["payload"]["status"] == "interrupted");
    require(
        roster.is_some() && interrupted.is_some() && roster < interrupted,
        "background roster must precede interrupt",
    )?;
    require(
        p["providerThreads"][0]["status"] == "idle"
            && p["providerThreads"][0]["pendingBackgroundTasks"]
                .as_array()
                .is_none_or(Vec::is_empty),
        "interruption did not clear native background work",
    )?;
    let shell = serde_json::to_value(&result.shell_snapshot)?;
    let shell = rows(&shell, "threads")
        .find(|t| t["id"] == p["thread"]["id"])
        .unwrap_or(&Value::Null);
    require(
        shell["pendingBackgroundTasks"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "interrupted shell retained background work",
    )?;
    require(
        rows(&p, "subagents").count() == 0,
        "background Bash created a subagent",
    )?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|i| i["type"] == "command_execution")
                .map(|i| i["status"].clone())
                .collect::<Vec<_>>()
        ) == json!(["completed", "interrupted"]),
        "launch must complete and foreground command must interrupt",
    )?;

    Ok(())
}
