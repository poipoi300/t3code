use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::Value;

pub fn assert_open_code2_nested_background_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    fn texts(p: &Value) -> Vec<&str> {
        rows(p, "turnItems")
            .filter(|item| item["type"] == "assistant_message")
            .map(|item| item["text"].as_str().unwrap_or_default().trim())
            .collect()
    }
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_provider_native_subagent_root_turns(result)?;
    assert_user_messages_include(&p, &[OPENCODE2_NESTED_BACKGROUND_PROMPT])?;
    require(
        texts(&p) == ["ROOT_OK"] && rows(&p, "subagents").count() == 1,
        "nested root acknowledgment/child count",
    )?;
    let middle = &p["subagents"][0];
    require(
        middle["status"] == "completed" && middle["result"] == "MIDDLE_OK",
        "middle completion",
    )?;
    let mid = result
        .projections
        .get(middle["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(mid.is_some(), "missing middle projection")?;
    let mid = mid.expect("middle asserted");
    require(
        rows(&mid, "subagents").count() == 1,
        "expected one grandchild",
    )?;
    let grandchild = &mid["subagents"][0];
    require(
        grandchild["status"] == "completed" && grandchild["result"] == "GRANDCHILD_OK",
        "grandchild completion",
    )?;
    let child = result
        .projections
        .get(grandchild["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(
        child
            .as_ref()
            .is_some_and(|child| texts(child) == ["GRANDCHILD_OK"])
            && texts(&mid) == ["MIDDLE_OK", "GRANDCHILD_OK"],
        "nested outputs must remain in their threads",
    )?;
    let events: Vec<Value> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    let waited = events.iter().position(|event| {
        event["type"] == "run.updated"
            && event["payload"]["id"] == p["runs"][0]["id"]
            && event["payload"]["status"] == "waiting"
    });
    let ended = events.iter().position(|event| {
        event["type"] == "subagent.updated"
            && event["payload"]["id"] == grandchild["id"]
            && event["payload"]["status"] == "completed"
    });
    let completed = events.iter().position(|event| {
        event["type"] == "run.updated"
            && event["payload"]["id"] == p["runs"][0]["id"]
            && event["payload"]["status"] == "completed"
    });
    require(
        waited
            .zip(ended)
            .zip(completed)
            .is_some_and(|((waited, ended), completed)| waited < ended && ended < completed),
        "app run must wait for nested background child",
    )?;
    let shell = serde_json::to_value(&result.shell_snapshot)?;
    require(
        rows(&shell, "threads")
            .find(|thread| thread["id"] == p["thread"]["id"])
            .is_none_or(|thread| rows(thread, "pendingBackgroundTasks").next().is_none()),
        "nested pending work must clear",
    )
}
