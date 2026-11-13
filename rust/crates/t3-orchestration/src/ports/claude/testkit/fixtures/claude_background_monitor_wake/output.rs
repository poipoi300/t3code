#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::json;
pub fn assert_claude_background_monitor_wake_output(
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

    assert_user_messages_include(&p, &[super::input::CLAUDE_BACKGROUND_MONITOR_WAKE_PROMPT])?;
    let events = serde_json::to_value(&result.domain_events)?;
    let tasks = events
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "provider-thread.updated")
        .flat_map(|e| {
            e["payload"]["pendingBackgroundTasks"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .filter(|t| t["taskId"] == "b1htjkjev")
        .collect::<Vec<_>>();
    require(
        tasks.len() > 1,
        "Monitor roster must survive launch correlation",
    )?;
    for task in tasks.iter().skip(1) {
        require(
            **task
                == json!({"taskId":"b1htjkjev","description":"Background monitor test","kind":"monitor","nativeItemId":"toolu_019b4EUbhEcYP4TS7vZn27N1"}),
            "Monitor classification or native identity differs",
        )?;
    }
    require(
        p["providerThreads"][0]["pendingBackgroundTasks"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "Monitor roster did not clear",
    )?;
    require(
        json!(background_notifications(&p))
            == json!([{"summary":r#"Monitor "Background monitor test" finished"#,"outcome":"completed","source":{"kind":"monitor"}}]),
        "Monitor completion notification differs",
    )?;
    for (index, text) in ["STARTED", "WAKE_DONE"].iter().enumerate() {
        let replies = rows(&p, "turnItems")
            .filter(|i| i["runId"] == p["runs"][index]["id"] && i["type"] == "assistant_message")
            .map(|i| i["text"].as_str().unwrap_or_default().trim())
            .collect::<Vec<_>>();
        require(
            replies == [*text],
            "Monitor wake reply attached to the wrong run",
        )?;
    }
    require(
        rows(&p, "subagents").count() == 0,
        "Monitor must not create a subagent",
    )?;

    Ok(())
}
