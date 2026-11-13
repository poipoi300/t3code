use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::json;

pub fn assert_grok_auto_blocked_command_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    use super::input::*;
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[GROK_AUTO_BLOCKED_COMMAND_PROMPT])?;
    assert_no_acp_client_file_or_terminal_requests(transcript)?;
    let frames: Vec<_> = transcript
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::ExpectOutbound(entry) => Some((false, &entry.frame)),
            ReplayEntry::EmitInbound(entry) => Some((true, &entry.frame)),
            _ => None,
        })
        .collect();
    require(
        frames
            .iter()
            .find(|(_, frame)| frame["method"] == "initialize")
            .is_some_and(|(_, frame)| {
                frame["params"]["_meta"] == json!({"clientType":"extension"})
            }),
        "Grok must advertise permission prompt support",
    )?;
    for text in [GROK_AUTO_ROUTINE_MARKER, GROK_AUTO_BLOCKED_PATH] {
        require(
            rows(&p, "turnItems")
                .find(|item| {
                    item["type"] == "command_execution"
                        && item["input"]
                            .as_str()
                            .is_some_and(|input| input.contains(text))
                })
                .is_some_and(|item| item["status"] == "completed"),
            format!("command must run: {text}"),
        )?;
    }
    let permissions: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| *inbound && frame["method"] == "session/request_permission")
        .map(|(_, frame)| frame["params"]["toolCall"].to_string())
        .collect();
    require(
        permissions.len() == 1
            && permissions[0].contains(GROK_AUTO_BLOCKED_PATH)
            && !permissions[0].contains(GROK_AUTO_ROUTINE_MARKER),
        "Grok must ask only about the blocked command",
    )?;
    assert_runtime_request_counts(&p, 1, None)?;
    let request = &p["runtimeRequests"][0];
    require(
        request["kind"] == "command"
            && request["status"] == "resolved"
            && request["decision"] == "accept",
        "approved Grok runtime request",
    )?;
    let pending = result
        .captured_shell_snapshots
        .get(GROK_AUTO_PENDING_SHELL_KEY)
        .map(serde_json::to_value)
        .transpose()?;
    require(
        pending
            .as_ref()
            .and_then(|shell| {
                rows(shell, "threads").find(|thread| thread["id"] == p["thread"]["id"])
            })
            .is_some_and(|thread| thread["pendingRuntimeRequest"]["id"] == request["id"]),
        "blocked command must appear as pending in the shell",
    )?;
    require(
        frames
            .iter()
            .find(|(inbound, frame)| {
                !*inbound
                    && frame["kind"] == "response"
                    && frame["method"] == "session/request_permission"
            })
            .is_some_and(|(_, frame)| {
                frame["result"] == json!({"outcome":{"outcome":"selected","optionId":"allow-once"}})
            }),
        "Grok permission response",
    )?;
    let approval = rows(&p, "turnItems")
        .find(|item| item["type"] == "approval_request" && item["requestId"] == request["id"]);
    require(
        approval.is_some_and(|item| {
            json!(values(item, "options", "decision")) == json!(["cancel", "decline", "accept"])
        }),
        "project-wide always-allow must not become a session choice",
    )
}
