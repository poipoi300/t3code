use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_grok_background_bash_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_background_command_output(
        result,
        transcript,
        super::input::GROK_BACKGROUND_BASH_PROMPT,
        super::input::GROK_BACKGROUND_BASH_TICKS,
        "Run three tock echoes in the background",
        "command",
    )
}
pub fn assert_background_command_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
    prompt: &str,
    ticks: &[&str],
    description: &str,
    kind: &str,
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
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[prompt])?;
    let root = &p["runs"][0];
    let wake = &p["runs"][1];
    let commands: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "command_execution")
        .collect();
    require(commands.len() == 1, "expected one background command")?;
    let command = commands[0];
    require(
        command["runId"] == root["id"]
            && command["status"] == "completed"
            && command["output"]
                == ticks
                    .iter()
                    .map(|tick| format!("{tick}\n"))
                    .collect::<String>(),
        "background command ownership/status/output",
    )?;
    let events: Vec<Value> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    let updates: Vec<_> = events
        .iter()
        .filter(|event| {
            event["type"] == "turn-item.updated"
                && event["payload"]["id"] == command["id"]
                && event["payload"]["type"] == "command_execution"
        })
        .map(|event| &event["payload"])
        .collect();
    let completed = updates
        .iter()
        .position(|update| update["status"] == "completed");
    for tick in ticks.iter().take(ticks.len().saturating_sub(1)) {
        let update = updates.iter().position(|update| {
            update["output"]
                .as_str()
                .is_some_and(|output| output.contains(tick))
        });
        require(
            update.zip(completed).is_some_and(|(update, completed)| {
                update < completed && updates[update]["status"] == "running"
            }),
            format!("command completed before {tick}"),
        )?;
    }
    let command_end = events.iter().position(|event| {
        event["type"] == "turn-item.updated"
            && event["payload"]["id"] == command["id"]
            && event["payload"]["status"] == "completed"
    });
    let root_end = events.iter().position(|event| {
        event["type"] == "run.updated"
            && event["payload"]["id"] == root["id"]
            && event["payload"]["status"] == "completed"
    });
    require(
        command_end
            .zip(root_end)
            .is_some_and(|(command, root)| root > command),
        "root completed before background work ended",
    )?;
    require(
        rows(&p, "turnItems").any(|item| {
            item["runId"] == root["id"]
                && item["type"] == "assistant_message"
                && item["text"]
                    .as_str()
                    .is_some_and(|text| text.trim() == "ROOT_DONE")
        }),
        "missing root acknowledgment",
    )?;
    require(
        json!(background_notifications(&p))
            == json!([{"summary":format!("{} \"{description}\" finished (exit 0)",if kind=="monitor" {"Monitor"}else{"Command"}),"outcome":"completed","source":{"kind":kind}}]),
        "background completion notification",
    )?;
    require(
        rows(&p, "messages")
            .find(|message| message["id"] == wake["userMessageId"])
            .is_some_and(|message| {
                message["createdBy"] == "agent" && message["creationSource"] == "provider"
            }),
        "continuation must be provider-created",
    )?;
    require(
        rows(&p, "turnItems")
            .any(|item| item["runId"] == wake["id"] && item["type"] == "assistant_message"),
        "missing continuation response",
    )
}
