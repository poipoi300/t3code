use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_tool_call_read_only_on_request_fx_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[TOOL_CALL_WRITE_PROMPT])?;
    assert_no_acp_client_file_or_terminal_requests(transcript)?;
    require(
        rows(&p, "runtimeRequests").count() == 1
            && p["runtimeRequests"][0]["status"] == "resolved"
            && p["runtimeRequests"][0]["decision"] == "accept"
            && p["runtimeRequests"][0]["kind"] == "file-change",
        "fx write must ask exactly once",
    )?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|item| item["type"] == "file_change"
                    && item["fileName"]
                        .as_str()
                        .is_some_and(|name| name.ends_with(".codex-probe-write-action.txt")))
                .map(|item| &item["status"])
                .collect::<Vec<_>>()
        ) == json!(["completed"]),
        "approved fx file write",
    )?;
    let frames = replay_frames(transcript);
    require(
        json!(
            frames
                .iter()
                .filter(
                    |(inbound, frame)| *inbound && frame["method"] == "session/request_permission"
                )
                .map(|(_, frame)| &frame["params"]["toolCall"]["kind"])
                .collect::<Vec<_>>()
        ) == json!(["edit"]),
        "fx must request its own edit",
    )
}
