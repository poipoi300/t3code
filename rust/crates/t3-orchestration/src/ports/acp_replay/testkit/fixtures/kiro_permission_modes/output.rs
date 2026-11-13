use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

fn permission_modes(transcript: &ReplayTranscript) -> (Vec<Value>, Vec<&'static str>) {
    let frames = replay_frames(transcript);
    let writes = frames
        .iter()
        .filter(|(inbound, frame)| {
            !*inbound
                && frame["kind"] == "request"
                && frame["method"] == "session/set_config_option"
                && frame["params"]["configId"] == "autopilot"
        })
        .map(|(_, frame)| frame["params"]["value"].clone())
        .collect();
    let kinds = frames
        .iter()
        .filter(|(inbound, frame)| {
            *inbound
                && frame["kind"] == "request"
                && frame["method"] == "session/request_permission"
        })
        .map(|(_, frame)| {
            if frame["params"]["_meta"]["kiro"]["type"] == "turn_approval" {
                "turn"
            } else {
                "tool"
            }
        })
        .collect();
    (writes, kinds)
}
fn completed_write(result: &ScenarioResult, transcript: &ReplayTranscript) -> Result<Value> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[super::input::KIRO_PERMISSION_PROMPT])?;
    Ok(p)
}
pub fn assert_kiro_supervised_write_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = completed_write(result, transcript)?;
    let (writes, kinds) = permission_modes(transcript);
    require(
        json!(writes) == json!(["off"]) && kinds == ["tool", "turn"],
        "supervised Kiro must disable Autopilot and ask about the tool and review",
    )?;
    assert_field_sequence(&p, "runtimeRequests", "status", &["resolved", "resolved"])
}
pub fn assert_kiro_full_access_write_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = completed_write(result, transcript)?;
    let (writes, kinds) = permission_modes(transcript);
    require(
        writes.is_empty() && kinds == ["tool"],
        "full-access Kiro leaves Autopilot on and skips turn review",
    )?;
    assert_runtime_request_counts(&p, 0, None)
}
