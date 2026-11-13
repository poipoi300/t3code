#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_claude_idle_resume_output(
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
        &[
            super::input::CLAUDE_IDLE_RESUME_PROMPT_1,
            super::input::CLAUDE_IDLE_RESUME_PROMPT_2,
        ],
    )?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|i| i["type"] == "assistant_message")
                .map(|i| i["text"].clone())
                .collect::<Vec<_>>()
        ) == json!([
            "idle resume first turn complete",
            "idle resume second turn complete"
        ]),
        "idle-resume replies differ",
    )?;
    require(
        json!(
            rows(&p, "providerTurns")
                .map(|t| json!([
                    t["turnTokenUsage"]["outputTokens"],
                    t["turnTokenUsage"]["otherUsage"]
                ]))
                .collect::<Vec<_>>()
        ) == json!([[7,{"inputTokens":0,"outputTokens":0}],[7,{"inputTokens":0,"outputTokens":0}]]),
        "resumed ledger counted the first process twice",
    )?;
    Ok(())
}
