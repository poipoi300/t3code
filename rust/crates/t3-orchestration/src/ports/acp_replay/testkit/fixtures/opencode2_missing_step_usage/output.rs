use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_open_code2_missing_step_usage_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    let turn = &p["providerTurns"][0];
    require(
        turn["status"] == "completed",
        "turn must complete without all step usage",
    )?;
    assert_includes_fields(
        &turn["turnTokenUsage"],
        &json!({"usageStatus":"partial","inputTokens":8701+489,"outputTokens":113+147,"reasoningTokens":147,"models":["opencode/space-bunny-free"]}),
        "missing step report must preserve measured partial total",
    )
}
