use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_muse_subagent_interrupt_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_field_sequence(&p, "runs", "status", &["interrupted"])?;
    require(
        replay_frames(transcript)
            .iter()
            .any(|(inbound, frame)| !*inbound && frame["method"] == "turn/interrupt"),
        "Stop must reach Muse",
    )?;
    require(
        !rows(&p, "turnItems").any(|item| item["status"] == "running")
            && rows(&p["providerThreads"][0], "pendingBackgroundTasks")
                .next()
                .is_none(),
        "Muse Stop must clear active/background items",
    )?;
    require(
        rows(&p, "subagents").count() == 1 && p["subagents"][0]["status"] == "interrupted",
        "child must interrupt",
    )?;
    let subagent = &p["subagents"][0];
    let child = result
        .projections
        .get(subagent["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing child projection")?;
    let child = child.expect("child asserted");
    require(
        !rows(&child, "nodes").any(|node| node["status"] == "running"),
        "child nodes must stop",
    )?;
    require(
        json!(
            rows(&child, "messages")
                .map(|message| json!([message["role"], message["text"]]))
                .collect::<Vec<_>>()
        ) == json!([["user", subagent["prompt"]]]),
        "interrupted child must not invent an answer",
    )
}
