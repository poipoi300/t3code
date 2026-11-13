use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_kiro_fork_from_turn_output(
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
    let mut target = None;
    for p in result.projections.values() {
        let p = serde_json::to_value(p)?;
        if p["thread"]["lineage"]["relationshipToParent"] == "fork" {
            target = Some(p);
            break;
        }
    }
    require(target.is_some(), "fork must create a target thread")?;
    let target = target.expect("target asserted");
    assert_semantic_projection_integrity(&target)?;
    require(
        target["contextTransfers"][0]["resolution"]["strategy"] == "native_fork",
        "Kiro must use native fork",
    )?;
    let frames = replay_frames(transcript);
    let first_id = frames
        .iter()
        .filter(|(inbound, _)| *inbound)
        .map(|(_, frame)| &frame["params"]["update"]["_meta"]["kiro"])
        .find(|kiro| kiro["kind"] == "user_message_id_assigned")
        .and_then(|kiro| kiro.get("userMessageId"));
    let forks: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| !*inbound && frame["method"] == "session/fork")
        .collect();
    require(
        forks.len() == 1 && forks[0].1["params"]["_meta"] == json!({"kiro":{"messageId":first_id}}),
        "fork boundary must use the first prompt's native user message ID",
    )?;
    let text = rows(&target, "turnItems")
        .map(|item| {
            if item["type"] == "assistant_message" {
                item["text"].as_str().unwrap_or_default()
            } else {
                ""
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    require(
        text.contains("fork boundary alpha") && !text.contains("fork boundary beta"),
        "fork copied turns past the selected boundary",
    )
}
