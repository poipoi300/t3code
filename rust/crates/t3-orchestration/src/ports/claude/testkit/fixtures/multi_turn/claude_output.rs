#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::{ReplayEntry, ReplayTranscript},
};

pub fn assert_multi_turn_claude_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_multi_turn_fixture_output(result, transcript)?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    require(
        rows(&p, "providerThreads").count() == 1,
        "multiple native threads",
    )?;
    let thread = &p["providerThreads"][0];
    require(
        !thread["nativeThreadRef"].is_null(),
        "missing native thread",
    )?;
    require(
        rows(&p, "runs").all(|run| run["providerThreadId"] == thread["id"]),
        "runs must share their native thread",
    )?;
    let outbound = transcript
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::ExpectOutbound(e) => Some(&e.frame),
            _ => None,
        })
        .collect::<Vec<_>>();
    let opens = outbound
        .iter()
        .filter(|frame| frame["type"] == "query.open")
        .collect::<Vec<_>>();
    require(
        outbound
            .iter()
            .filter(|frame| frame["type"] == "prompt.offer")
            .count()
            == 2,
        "expected two prompt offers",
    )?;
    if transcript.scenario.0 == "multi_turn_restart" {
        require(
            opens.len() == 2 && opens[1]["options"]["resume"].is_string(),
            "restart must resume the native conversation",
        )?;
    } else {
        require(opens.len() == 1, "reuse must keep one native query")?;
    }
    Ok(())
}
