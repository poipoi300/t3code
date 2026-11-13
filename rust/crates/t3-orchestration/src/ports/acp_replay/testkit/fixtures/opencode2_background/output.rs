use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_open_code2_background_output(
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
    assert_provider_native_subagent_root_turns(result)?;
    assert_user_messages_include(&p, &[OPENCODE2_BACKGROUND_PROMPT])?;
    let root = &p["runs"][0];
    let wake = &p["runs"][1];
    require(
        rows(&p, "subagents").count() == 1,
        "expected one background child",
    )?;
    let subagent = &p["subagents"][0];
    assert_includes_fields(
        subagent,
        &json!({"origin":"provider_native","status":"completed","runId":root["id"],"title":"Sleep and reply","result":"CHILD_OK"}),
        "background child metadata",
    )?;
    let texts = |run: &Value| {
        rows(&p, "turnItems")
            .filter(|item| item["runId"] == run["id"] && item["type"] == "assistant_message")
            .map(|item| item["text"].as_str().unwrap_or_default().trim())
            .collect::<Vec<_>>()
    };
    require(texts(root) == ["PARENT_OK"], "root acknowledgment")?;
    let events: Vec<Value> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    let waiting = events.iter().position(|event| {
        event["type"] == "run.updated"
            && event["payload"]["id"] == root["id"]
            && event["payload"]["status"] == "waiting"
    });
    let done = events.iter().position(|event| {
        event["type"] == "turn-item.updated"
            && event["payload"]["type"] == "subagent"
            && event["payload"]["status"] == "completed"
    });
    let woke = events
        .iter()
        .position(|event| event["type"] == "run.updated" && event["payload"]["id"] == wake["id"]);
    require(
        waiting
            .zip(done)
            .zip(woke)
            .is_some_and(|((waiting, done), woke)| waiting < done && done < woke),
        "background child completion must follow root wait and precede continuation",
    )?;
    let child = result
        .projections
        .get(subagent["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing child projection")?;
    let child = child.expect("child asserted");
    require(
        rows(&child, "turnItems").any(|item| {
            item["type"] == "command_execution"
                && item["input"]
                    .as_str()
                    .is_some_and(|text| text.contains("sleep 20"))
        }) && rows(&child, "turnItems")
            .any(|item| item["type"] == "assistant_message" && item["text"] == "CHILD_OK")
            && !rows(&p, "turnItems").any(|item| item["type"] == "command_execution"),
        "native shell/reply must remain in child",
    )?;
    require(
        rows(&p, "messages")
            .find(|message| message["id"] == wake["userMessageId"])
            .is_some_and(|message| {
                message["createdBy"] == "agent"
                    && message["creationSource"] == "provider"
                    && message["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("CHILD_OK"))
            }),
        "wake message source/content",
    )?;
    require(
        json!(background_notifications(&p))
            == json!([{"summary":"Subagent \"Sleep and reply\" finished","outcome":"completed","source":{"kind":"subagent","childThreadId":subagent["childThreadId"]}}])
            && texts(wake) == ["The background subagent finished and returned `CHILD_OK`."],
        "background notification/reply",
    )?;
    let shell = serde_json::to_value(&result.shell_snapshot)?;
    require(
        rows(&shell, "threads")
            .find(|thread| thread["id"] == p["thread"]["id"])
            .is_none_or(|thread| rows(thread, "pendingBackgroundTasks").next().is_none())
            && !rows(&p, "turnItems")
                .any(|item| matches!(item["status"].as_str(), Some("running" | "waiting"))),
        "settled background work must clear pending state",
    )
}
