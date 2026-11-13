use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code_running_child_approval_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    super::super::opencode_child_approval::output::assert_open_code_child_approval_output(
        result, transcript,
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    let run = &p["runs"][0];
    let subagent = &p["subagents"][0];
    let request = &p["runtimeRequests"][0];
    let child = result
        .projections
        .get(subagent["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing running child projection")?;
    let child = child.expect("child asserted");
    require(
        rows(&p, "providerTurns")
            .find(|turn| turn["id"] == request["providerTurnId"])
            .is_some_and(|turn| turn["runAttemptId"] == run["activeAttemptId"]),
        "child request must route through parent's root provider turn",
    )?;
    require(
        rows(&p, "nodes")
            .find(|node| node["id"] == request["nodeId"])
            .is_some_and(|node| {
                node["kind"] == "approval_request"
                    && node["runId"] == run["id"]
                    && node["parentNodeId"] == subagent["id"]
                    && node["status"] == "completed"
            }),
        "approval node must hang off asking subagent",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "approval_request")
            .is_some_and(|item| {
                item["requestId"] == request["id"]
                    && item["prompt"]
                        .as_str()
                        .is_some_and(|text| text.contains("running-child.txt"))
            }),
        "running child approval prompt/request",
    )?;
    let pending = result
        .captured_shell_snapshots
        .get(super::input::OPENCODE_RUNNING_CHILD_APPROVAL_PENDING_SHELL_KEY)
        .map(serde_json::to_value)
        .transpose()?;
    require(pending.is_some(), "missing pending shell snapshot")?;
    let pending = pending.expect("snapshot asserted");
    require(
        rows(&pending, "threads")
            .find(|thread| thread["id"] == p["thread"]["id"])
            .is_some_and(|thread| thread["pendingRuntimeRequest"]["id"] == request["id"]),
        "parent shell must carry the pending approval",
    )?;
    require(
        rows(&pending, "threads")
            .find(|thread| thread["id"] == child["thread"]["id"])
            .is_some_and(|thread| thread["pendingRuntimeRequest"].is_null()),
        "pending native child must exist without owning an approval",
    )?;
    require(
        rows(&child, "runtimeRequests").count() == 0
            && !rows(&child, "turnItems").any(|item| item["type"] == "approval_request")
            && rows(&child, "turnItems")
                .find(|item| item["type"] == "command_execution")
                .is_some_and(|item| item["status"] == "completed"),
        "native child approval must route through parent",
    )?;
    assert_assistant_text_includes(&child, "CHILD_OK")
}
