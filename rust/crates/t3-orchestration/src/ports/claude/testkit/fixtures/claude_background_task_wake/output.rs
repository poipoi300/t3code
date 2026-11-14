#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};
use t3_contracts::orchestration_v2::OrchestrationV2Notification;
pub fn assert_claude_background_task_wake_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        3,
        None,
        Some(&["completed", "completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    assert_user_messages_include(
        &p,
        &[
            super::input::CLAUDE_BACKGROUND_TASK_WAKE_PROMPT,
            super::input::CLAUDE_BACKGROUND_TASK_WAKE_FOLLOW_UP_PROMPT,
        ],
    )?;
    for (index, text) in ["STARTED", "WAKE_DONE", "USER_REPLY"].iter().enumerate() {
        let replies = rows(&p, "turnItems")
            .filter(|i| i["runId"] == p["runs"][index]["id"] && i["type"] == "assistant_message")
            .map(|i| i["text"].as_str().unwrap_or_default().trim())
            .collect::<Vec<_>>();
        require(
            replies == [*text],
            "background wake reply attached to the wrong run",
        )?;
    }
    let creators = rows(&p, "runs")
        .map(|r| {
            let m = rows(&p, "messages")
                .find(|m| m["id"] == r["userMessageId"])
                .unwrap_or(&Value::Null);
            format!(
                "{}:{}",
                m["createdBy"].as_str().unwrap_or_default(),
                m["creationSource"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    require(
        creators == ["user:web", "agent:provider", "user:web"],
        "wake creator/source differs",
    )?;
    require(
        rows(&p, "messages")
            .find(|m| m["id"] == p["runs"][1]["userMessageId"])
            .is_some_and(|m| {
                m["text"] == r#"Background command "Background sleep test" completed (exit code 0)"#
            }),
        "native wake summary changed",
    )?;
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
                .is_some_and(|tasks| tasks.iter().any(|t| t["taskId"] == "bdqirlcyw"))
    });
    let cleared = events.iter().enumerate().find_map(|(i, e)| {
        (Some(i) > roster
            && e["type"] == "provider-thread.updated"
            && e["payload"]["pendingBackgroundTasks"]
                .as_array()
                .is_none_or(Vec::is_empty))
        .then_some(i)
    });
    require(
        roster.is_some() && cleared.is_some() && cleared > roster,
        "background task must be rostered then cleared",
    )?;
    require(
        p["providerThreads"][0]["pendingBackgroundTasks"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "final roster not empty",
    )?;
    let task = events
        .iter()
        .filter(|e| e["type"] == "provider-thread.updated")
        .flat_map(|e| {
            e["payload"]["pendingBackgroundTasks"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .find(|t| t["taskId"] == "bdqirlcyw");
    require(
        task == Some(
            &json!({"taskId":"bdqirlcyw","description":"Background sleep test","kind":"command"}),
        ),
        "command roster fields differ",
    )?;
    let expected_notifications: Vec<OrchestrationV2Notification> = serde_json::from_value(
        json!([{"summary":r#"Command "Background sleep test" finished"#,"outcome":"completed","source":{"kind":"command"}}]),
    )?;
    require(
        json!(background_notifications(&p)) == serde_json::to_value(expected_notifications)?,
        "background completion notification differs",
    )?;
    require(
        rows(&p, "subagents").count() == 0 && !rows(&p, "nodes").any(|n| n["kind"] == "subagent"),
        "background Bash must not create subagents",
    )?;

    Ok(())
}
