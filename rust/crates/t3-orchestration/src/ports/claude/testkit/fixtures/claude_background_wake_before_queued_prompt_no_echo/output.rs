#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::Value;

pub fn assert_claude_background_wake_before_queued_prompt_no_echo_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    for (prompt,reply) in [(super::super::claude_background_subagent_lifecycle::input::CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_STOP_PROMPT,"B_STOPPED"),(super::super::claude_background_subagent_lifecycle::input::CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_RESUME_PROMPT,"Agent B has been confirmed killed.")] {
 let run=rows(&p,"runs").find(|r|rows(&p,"messages").any(|m|m["id"]==r["userMessageId"]&&m["text"]==prompt)).unwrap_or(&Value::Null);
 let replies=rows(&p,"turnItems").filter(|i|i["runId"]==run["id"]&&i["type"]=="assistant_message").map(|i|i["text"].as_str().unwrap_or_default().trim()).collect::<Vec<_>>();require(replies==[reply],"no-echo wake contaminated a queued user reply")?;
}
    require(
        rows(&p, "runs").all(|r| r["status"] == "completed"),
        "no-echo queued run did not complete",
    )?;

    Ok(())
}
