#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_claude_background_subagent_after_root_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(
        &p,
        &[super::input::CLAUDE_BACKGROUND_SUBAGENT_AFTER_ROOT_PROMPT],
    )?;
    let texts = |p: &Value| {
        rows(p, "turnItems")
            .filter(|i| i["type"] == "assistant_message")
            .map(|i| i["text"].as_str().unwrap_or_default().trim().to_owned())
            .collect::<Vec<_>>()
    };
    let commands = |p: &Value| {
        rows(p, "turnItems")
            .filter(|i| i["type"] == "command_execution")
            .map(|i| i["input"].to_string())
            .collect::<Vec<_>>()
    };
    let expected_texts = ["SUB_STEP_1", "SUB_STEP_2", "SUB_FINAL_REPORT"];
    let expected_commands = ["sleep 3 && echo SUB_DONE_1", "sleep 3 && echo SUB_DONE_2"];
    let parent_texts = texts(&p);
    require(
        ["ROOT_STARTED", "ROOT_WAKE_DONE"]
            .iter()
            .all(|s| parent_texts.iter().any(|t| t == s)),
        "missing root replies",
    )?;
    require(
        expected_texts
            .iter()
            .all(|s| !parent_texts.iter().any(|t| t == s))
            && expected_commands
                .iter()
                .all(|s| !commands(&p).iter().any(|t| t.contains(s))),
        "child work leaked into the parent thread",
    )?;
    require(
        rows(&p, "subagents").count() == 1 && rows(&p, "runs").count() == 2,
        "late child created extra runs/agents",
    )?;
    let agent = &p["subagents"][0];
    require(
        agent["status"] == "completed"
            && agent["origin"] == "provider_native"
            && agent["runId"] == p["runs"][0]["id"],
        "late subagent changed its owning run",
    )?;
    require(
        rows(&p, "nodes").any(|n| {
            n["id"] == agent["id"] && n["status"] == "completed" && n["runId"] == p["runs"][0]["id"]
        }),
        "late child node changed its owning run",
    )?;
    require(
        rows(&p, "messages")
            .any(|m| m["id"] == p["runs"][1]["userMessageId"] && m["text"] == "SUB_FINAL_REPORT"),
        "wake must open with the child result",
    )?;
    let child = agent["childThreadId"]
        .as_str()
        .and_then(|id| result.projections.get(id))
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    require(child.is_object(), "missing late child thread")?;
    require(
        json!(background_notifications(&p))
            == json!([{"summary":r#"Subagent "Background subagent test" finished"#,"outcome":"completed","source":{"kind":"subagent","childThreadId":agent["childThreadId"]}}]),
        "late child completion notification differs",
    )?;
    require(
        texts(&child)
            .iter()
            .filter(|t| expected_texts.contains(&t.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>()
            == expected_texts,
        "child narration/final result must occur once in order",
    )?;
    for command in expected_commands {
        require(
            commands(&child).iter().any(|c| c.contains(command)),
            format!("missing child command {command}"),
        )?;
    }
    let events = serde_json::to_value(&result.domain_events)?;
    let events = events
        .as_array()
        .ok_or_else(|| crate::error::Error::Service {
            operation: "replay fixture assertion",
            detail: "missing events".into(),
        })?;
    require(
        !events.iter().any(|e| {
            e["type"] == "provider-thread.updated"
                && e["payload"]["pendingBackgroundTasks"]
                    .as_array()
                    .is_some_and(|t| !t.is_empty())
        }),
        "native subagent must not become a background command roster",
    )?;
    let wake = events.iter().position(|e| {
        e["type"] == "run.updated"
            && e["payload"]["id"] == p["runs"][1]["id"]
            && e["payload"]["status"] == "starting"
    });
    require(wake.is_some(), "missing wake admission")?;
    let wake = wake.unwrap_or_default();
    for text in expected_texts {
        let index = events.iter().position(|e| {
            e["type"] == "message.updated"
                && e["payload"]["threadId"] == agent["childThreadId"]
                && e["payload"]["text"]
                    .as_str()
                    .is_some_and(|s| s.trim() == text)
        });
        require(
            index.is_some_and(|i| i < wake),
            format!("child text {text} must persist before the root wakes"),
        )?;
    }
    for command in expected_commands {
        let index = events.iter().position(|e| {
            e["type"] == "turn-item.updated"
                && e["payload"]["threadId"] == agent["childThreadId"]
                && e["payload"]["type"] == "command_execution"
                && e["payload"]["status"] == "completed"
                && e["payload"]["input"]
                    .as_str()
                    .is_some_and(|s| s.contains(command))
        });
        require(
            index.is_some_and(|i| i < wake),
            format!("child command {command} must finish before the root wakes"),
        )?;
    }
    for progress in [
        "Running Sleep 3 seconds then echo SUB_DONE_1",
        "Running Sleep 3 seconds then echo SUB_DONE_2",
    ] {
        let index = events.iter().position(|e| {
            e["type"] == "subagent.updated"
                && e["payload"]["id"] == agent["id"]
                && e["payload"]["progress"] == progress
        });
        require(
            index.is_some_and(|i| i < wake),
            "child progress must persist before the root wakes",
        )?;
    }
    require(
        !events[wake..].iter().any(|e| {
            matches!(
                e["type"].as_str(),
                Some("message.updated" | "turn-item.updated")
            ) && e["payload"]["threadId"] == agent["childThreadId"]
        }),
        "continuation must not re-store the child's work",
    )?;

    Ok(())
}
