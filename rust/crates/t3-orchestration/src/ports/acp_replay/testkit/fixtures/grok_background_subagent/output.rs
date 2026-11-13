use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_grok_background_subagent_output(
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
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[super::input::GROK_BACKGROUND_SUBAGENT_PROMPT])?;
    let root = &p["runs"][0];
    let wake = &p["runs"][1];
    require(
        rows(&p, "subagents").count() == 1,
        "expected one background subagent",
    )?;
    let subagent = &p["subagents"][0];
    require(
        subagent["status"] == "completed"
            && subagent["origin"] == "provider_native"
            && subagent["runId"] == root["id"],
        "native subagent ownership/status",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "subagent")
            .is_some_and(|item| item["status"] == "completed" && item["runId"] == root["id"]),
        "subagent item belongs to the spawning run",
    )?;
    let events: Vec<Value> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    let child_end = events.iter().position(|event| {
        event["type"] == "turn-item.updated"
            && event["payload"]["type"] == "subagent"
            && event["payload"]["status"] == "completed"
    });
    let root_end = events.iter().position(|event| {
        event["type"] == "run.updated"
            && event["payload"]["id"] == root["id"]
            && event["payload"]["status"] == "completed"
    });
    require(
        child_end
            .zip(root_end)
            .is_some_and(|(child, root)| root > child),
        "root ended before the subagent",
    )?;
    let root_texts: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["runId"] == root["id"] && item["type"] == "assistant_message")
        .map(|item| item["text"].as_str().unwrap_or_default().trim())
        .collect();
    require(
        root_texts.contains(&"ROOT_DONE") && !root_texts.contains(&"SUBAGENT_DONE"),
        "child output leaked into root",
    )?;
    let root_done = events.iter().position(|event| {
        event["type"] == "turn-item.updated"
            && event["payload"]["runId"] == root["id"]
            && event["payload"]["type"] == "assistant_message"
            && event["payload"]["status"] == "completed"
            && event["payload"]["text"]
                .as_str()
                .is_some_and(|text| text.trim() == "ROOT_DONE")
    });
    require(
        root_done
            .zip(child_end)
            .is_some_and(|(root, child)| root < child),
        "ROOT_DONE streamed until the child finished",
    )?;
    let child_id = subagent["childThreadId"].as_str().unwrap_or_default();
    let child = result
        .projections
        .get(child_id)
        .map(serde_json::to_value)
        .transpose()?;
    require(
        child
            .as_ref()
            .is_some_and(|child| assert_assistant_text_includes(child, "SUBAGENT_DONE").is_ok()),
        "missing child reply",
    )?;
    require(
        json!(background_notifications(&p))
            == json!([{"summary":"Subagent \"Sleep then reply done\" finished","outcome":"completed","source":{"kind":"subagent","childThreadId":subagent["childThreadId"]}}]),
        "subagent completion notification",
    )?;
    require(
        rows(&p, "messages")
            .find(|message| message["id"] == wake["userMessageId"])
            .is_some_and(|message| {
                message["createdBy"] == "agent" && message["creationSource"] == "provider"
            }),
        "provider continuation source",
    )?;
    require(
        rows(&p, "turnItems")
            .any(|item| item["runId"] == wake["id"] && item["type"] == "assistant_message"),
        "missing continuation response",
    )
}
