#![cfg(feature = "claude-adapter")]

use super::super::claude_compact_after_peer_turn::output::claude_compact_runs;
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_claude_compact_after_peer_turn_no_echo_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    require(
        claude_compact_runs(&p)
            == json!([{"text":super::super::claude_compact_after_peer_turn::input::CLAUDE_COMPACT_FIRST_PROMPT,"fromUser":true,"status":"completed","replies":["compact probe first turn"],"compactions":[]},{"text":"/compact","fromUser":true,"status":"completed","replies":["PEER_ACK"],"compactions":[]}]),
        "compact/wake ownership differs",
    )?;
    Ok(())
}
