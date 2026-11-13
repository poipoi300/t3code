#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::{Value, json};

pub fn assert_claude_background_wake_before_queued_prompt_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let offer=transcript.entries.iter().position(|entry|matches!(entry,ReplayEntry::ExpectOutbound(e) if e.label.as_ref().and_then(Option::as_ref).is_some_and(|l|l.0=="prompt.offer:3")));
    require(offer.is_some(), "missing resume prompt offer")?;
    let offer = offer.unwrap_or_default();
    let resume_uuid = match &transcript.entries[offer] {
        ReplayEntry::ExpectOutbound(e) => e.frame["message"]["uuid"].as_str(),
        _ => None,
    };
    require(resume_uuid.is_some(), "resume prompt must carry a UUID")?;
    let results = transcript.entries[offer + 1..]
        .iter()
        .filter_map(|e| match e {
            ReplayEntry::EmitInbound(e) if e.frame["type"] == "result" => Some(&e.frame),
            _ => None,
        })
        .collect::<Vec<_>>();
    require(
        results.len() >= 2
            && results[0]["origin"]["kind"] == "task-notification"
            && results[0].get("user_message_uuid").is_none()
            && results[1]["user_message_uuid"].as_str() == resume_uuid,
        "recording must place native wake before the offered prompt's result",
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    let mut user_replies = Vec::new();
    let mut continuations = Vec::new();
    for run in rows(&p, "runs") {
        let message = rows(&p, "messages")
            .find(|m| m["id"] == run["userMessageId"])
            .unwrap_or(&Value::Null);
        let replies = rows(&p, "turnItems")
            .filter(|i| i["runId"] == run["id"] && i["type"] == "assistant_message")
            .map(|i| json!(i["text"].as_str().unwrap_or_default().trim()))
            .collect::<Vec<_>>();
        if message["createdBy"] == "user" {
            user_replies.push(json!([message["text"], replies]));
        } else {
            continuations.extend(replies);
        }
    }
    use super::super::claude_background_subagent_lifecycle::input::*;
    require(
        json!(user_replies)
            == json!([
                [
                    super::input::CLAUDE_BACKGROUND_WAKE_BEFORE_QUEUED_PROMPT_LAUNCH_PROMPT,
                    ["LAUNCHED"]
                ],
                [
                    CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_STOP_PROMPT,
                    ["B_STOPPED"]
                ],
                [
                    CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_RESUME_PROMPT,
                    ["RESUMED"]
                ],
                [
                    CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_FINAL_PROMPT,
                    ["ALL_DONE"]
                ]
            ]),
        "native wake contaminated queued user replies",
    )?;
    let mut actual = continuations
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = [
        "A_REPORTED",
        "Agent B's kill is confirmed by the notification. No further action needed — the task is complete.",
    ];
    expected.sort();
    require(actual == expected, "native continuation replies differ")?;
    require(
        rows(&p, "runs").all(|r| r["status"] == "completed"),
        "queued run failed to settle",
    )?;
    require(
        json!(
            background_notifications(&p)
                .iter()
                .map(|n| json!([n["summary"], n["source"]["kind"]]))
                .collect::<Vec<_>>()
        ) == json!([
            [r#"Subagent "Agent A" finished"#, "subagent"],
            [
                r#"Subagent "Agent B" and command "Sleep 60 seconds then echo B_DONE" were stopped"#,
                "background_task"
            ]
        ]),
        "aggregated background wake notification differs",
    )?;

    Ok(())
}
