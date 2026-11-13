use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_tool_call_read_only_on_request_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[TOOL_CALL_WRITE_PROMPT])?;
    require(
        rows(&p, "runtimeRequests").count() == 1
            && p["runtimeRequests"][0]["status"] == "resolved"
            && p["runtimeRequests"][0]["decision"] == "accept"
            && matches!(
                p["runtimeRequests"][0]["kind"].as_str(),
                Some("command" | "file-change")
            ),
        "write must ask exactly once",
    )?;
    let request = &p["runtimeRequests"][0];
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|item| item["type"] == "approval_request")
                .map(|item| json!([item["requestId"], item["requestKind"]]))
                .collect::<Vec<_>>()
        ) == json!([[request["id"], request["kind"]]]),
        "approval must show the resolved request",
    )?;
    let writes: Vec<_> = rows(&p, "turnItems")
        .filter(|item| {
            if request["kind"] == "command" {
                item["type"] == "command_execution"
                    && item["input"]
                        .as_str()
                        .is_some_and(|input| input.contains(".codex-probe-write-action.txt"))
            } else {
                item["type"] == "file_change"
                    && item["fileName"]
                        .as_str()
                        .is_some_and(|name| name.ends_with(".codex-probe-write-action.txt"))
            }
        })
        .collect();
    require(
        !writes.is_empty() && writes.iter().any(|item| item["status"] == "completed"),
        "approved write must project and complete",
    )?;
    for item in writes {
        let content = if item["type"] == "command_execution" {
            item["input"].as_str()
        } else {
            item["newStr"].as_str().or_else(|| item["diffStr"].as_str())
        };
        require(
            content.is_some_and(|content| content.contains("codex app-server approval fixture")),
            "approved write must retain requested content",
        )?;
    }
    Ok(())
}
pub fn assert_tool_call_read_only_on_request_grok_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_tool_call_read_only_on_request_output(result, transcript)?;
    assert_no_acp_client_file_or_terminal_requests(transcript)?;
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
        "Grok must request its own edit",
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "approval_request")
            .is_some_and(|approval| {
                json!(values(approval, "options", "decision"))
                    == json!(["cancel", "decline", "acceptForSession", "accept"])
            }),
        "Grok supported session choices",
    )
}
pub fn assert_tool_call_read_only_on_request_kiro_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[TOOL_CALL_WRITE_PROMPT])?;
    assert_no_acp_client_file_or_terminal_requests(transcript)?;
    require(
        json!(
            rows(&p, "runtimeRequests")
                .map(|request| json!([request["status"], request["decision"]]))
                .collect::<Vec<_>>()
        ) == json!([["resolved", "accept"], ["resolved", "accept"]]),
        "write and review must both reach user",
    )?;
    require(
        rows(&p, "turnItems").any(|item| {
            matches!(
                item["type"].as_str(),
                Some("command_execution" | "file_change")
            ) && item["status"] == "completed"
        }),
        "approved write must complete",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "approval_request")
            .is_some_and(|approval| {
                json!(values(approval, "options", "decision"))
                    == json!(["accept", "decline", "cancel"])
            }),
        "Kiro supported permission choices",
    )
}
