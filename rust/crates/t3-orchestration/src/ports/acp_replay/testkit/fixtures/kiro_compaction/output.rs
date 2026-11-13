use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_kiro_compaction_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let outbound: Vec<_> = replay_frames(transcript)
        .into_iter()
        .filter(|(inbound, _)| !*inbound)
        .map(|(_, frame)| frame)
        .collect();
    let prompts: Vec<_> = outbound
        .iter()
        .filter(|frame| frame["method"] == "session/prompt")
        .collect();
    require(
        prompts.len() == 2
            && outbound
                .iter()
                .filter(|frame| frame["method"] == "_kiro/session/compact")
                .count()
                == 1,
        "compact must use the native compact RPC",
    )?;
    require(
        prompts.iter().all(|frame| {
            rows(&frame["params"], "prompt")
                .map(|block| block["text"].as_str().unwrap_or("undefined"))
                .collect::<String>()
                .starts_with("<t3_code_instructions>")
        }),
        "compaction must resend T3 instructions",
    )?;
    assert_base_projection(
        result,
        transcript,
        3,
        None,
        Some(&["completed", "completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|item| item["type"] == "compaction")
                .map(|item| json!([item["runId"], item["status"], item["title"]]))
                .collect::<Vec<_>>()
        ) == json!([[p["runs"][1]["id"], "completed", "Context compacted"]]),
        "native compaction must project under the compact run",
    )?;
    let recall = rows(&p, "turnItems")
        .filter(|item| item["type"] == "assistant_message" && item["runId"] == p["runs"][2]["id"])
        .filter_map(|item| item["text"].as_str())
        .collect::<String>();
    require(
        recall.contains(super::input::KIRO_COMPACTION_MARKER),
        "model forgot the marker after compaction",
    )
}
