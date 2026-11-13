use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_muse_subagent_approval_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_field_sequence(&p, "runs", "status", &["completed"])?;
    require(
        rows(&p, "runtimeRequests").count() == 1
            && p["runtimeRequests"][0]["kind"] == "command"
            && p["runtimeRequests"][0]["status"] == "resolved"
            && p["runtimeRequests"][0]["decision"] == "accept",
        "Muse child approval must resolve on parent",
    )?;
    require(
        replay_frames(transcript)
            .iter()
            .any(|(inbound, frame)| !*inbound && frame["method"] == "approval/decide"),
        "answer must reach Muse",
    )?;
    require(rows(&p, "subagents").count() == 1, "expected one child")?;
    let subagent = &p["subagents"][0];
    require(
        subagent["status"] == "completed"
            && matches!(subagent["result"].as_str(), Some("Written" | "Written.")),
        "child must report written",
    )?;
    let child = result
        .projections
        .get(subagent["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing child projection")?;
    let child = child.expect("child asserted");
    require(
        rows(&child, "runtimeRequests").next().is_none(),
        "native Muse child cannot own approval requests",
    )?;
    require(
        json!(
            rows(&child, "messages")
                .map(|message| json!([message["role"], message["text"]]))
                .collect::<Vec<_>>()
        ) == json!([
            ["user", subagent["prompt"]],
            ["assistant", subagent["result"]]
        ]),
        "child conversation must be prompt and result",
    )
}
