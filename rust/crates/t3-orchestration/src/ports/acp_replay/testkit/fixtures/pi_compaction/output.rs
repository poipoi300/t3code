use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_pi_compaction_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let frames = replay_frames(transcript);
    require(
        !frames.iter().any(|(inbound, frame)| {
            !*inbound
                && frame["type"] == "prompt"
                && frame["message"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("/compact"))
        }),
        "compact must not reach Pi as a prompt",
    )?;
    let commands: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| !*inbound && frame["type"] == "compact")
        .map(|(_, frame)| *frame)
        .collect();
    require(
        json!(commands)
            == json!([{"type":"compact","customInstructions":format!("keep the opaque marker {}",super::input::PI_COMPACTION_MARKER)},{"type":"compact"}]),
        "Pi native compaction arguments",
    )?;
    let ends: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| *inbound && frame["type"] == "compaction_end")
        .map(|(_, frame)| *frame)
        .collect();
    require(
        ends.len() == 2 && ends[0].get("result").is_none() && ends[1].get("result").is_some(),
        "Pi must omit failed result and report successful result",
    )?;
    let compacted = &ends[1]["result"];
    assert_base_projection(result, transcript, 6, None, Some(&["completed"; 6]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    let compactions: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "compaction")
        .collect();
    require(
        json!(
            compactions
                .iter()
                .map(|item| json!([item["runId"], item["status"], item["title"]]))
                .collect::<Vec<_>>()
        ) == json!([
            [p["runs"][1]["id"], "failed", "Context compaction failed"],
            [p["runs"][4]["id"], "completed", "Context compacted"]
        ]),
        "Pi compaction projection/run identity",
    )?;
    require(
        compactions[0]["summary"] == ends[0]["errorMessage"]
            && compactions[1]["summary"] == compacted["summary"]
            && compactions[1]["beforeTokenCount"] == compacted["tokensBefore"]
            && compactions[1]["afterTokenCount"] == compacted["estimatedTokensAfter"],
        "compaction summary/token estimates",
    )?;
    require(
        !rows(&p, "turnItems").any(|item| {
            item["type"] == "assistant_message"
                && (item["runId"] == p["runs"][1]["id"] || item["runId"] == p["runs"][4]["id"])
        }),
        "compaction must not run a model turn",
    )?;
    require(
        rows(&p, "providerTurns")
            .find(|turn| turn["runAttemptId"] == p["runs"][4]["activeAttemptId"])
            .is_some_and(|turn| {
                turn["tokenUsage"]["usedTokens"] == compacted["estimatedTokensAfter"]
            }),
        "retain post-compaction token estimate",
    )?;
    require(
        rows(&p, "turnItems")
            .rfind(|item| item["type"] == "assistant_message")
            .is_some_and(|item| {
                item["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(super::input::PI_COMPACTION_MARKER))
            }),
        "model forgot marker after compaction",
    )
}
