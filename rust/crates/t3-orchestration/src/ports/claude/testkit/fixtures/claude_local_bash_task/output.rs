#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_claude_local_bash_task_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;

    require(
        result.projections.len() == 1,
        "local Bash lifecycle created a child thread",
    )?;
    assert_user_messages_include(&p, &[CLAUDE_LOCAL_BASH_TASK_PROMPT])?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|i| i["type"] != "checkpoint")
                .map(|i| i["type"].clone())
                .collect::<Vec<_>>()
        ) == json!([
            "user_message",
            "assistant_message",
            "command_execution",
            "assistant_message"
        ]),
        "local Bash item order differs",
    )?;
    require(
        rows(&p, "subagents").count() == 0 && !rows(&p, "nodes").any(|n| n["kind"] == "subagent"),
        "local Bash must not project a subagent",
    )?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|i| i["type"] == "assistant_message")
                .map(|i| i["text"].clone())
                .collect::<Vec<_>>()
        ) == json!([
            "I'll run the typecheck command now.",
            "claude local bash task fixture complete"
        ]),
        "local Bash replies differ",
    )?;
    let command = rows(&p, "turnItems")
        .find(|i| i["type"] == "command_execution")
        .ok_or_else(|| crate::error::Error::Service {
            operation: "replay fixture assertion",
            detail: "missing Bash command".into(),
        })?;
    require(
        command
            .to_string()
            .contains("vp run --filter @t3tools/web typecheck")
            && command["output"].to_string().contains("tsgo --noEmit"),
        "local Bash input/output missing",
    )?;
    Ok(())
}
