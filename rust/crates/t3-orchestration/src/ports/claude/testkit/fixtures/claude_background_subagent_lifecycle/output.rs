#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::{Value, json};

pub fn assert_claude_background_subagent_lifecycle_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let starts = transcript
        .entries
        .iter()
        .filter_map(|e| match e {
            ReplayEntry::EmitInbound(e)
                if e.frame["subtype"] == "task_started"
                    && e.frame["task_id"] == "a1a715b7d0bdfefea" =>
            {
                Some(e.frame["tool_use_id"].clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let sends = transcript
        .entries
        .iter()
        .filter_map(|e| match e {
            ReplayEntry::EmitInbound(e) => e.frame["message"]["content"].as_array(),
            _ => None,
        })
        .flatten()
        .filter(|b| b["type"] == "tool_use" && b["name"] == "SendMessage")
        .map(|b| b["id"].clone())
        .collect::<Vec<_>>();
    require(
        starts.len() == 2 && starts[1..] == sends,
        "SendMessage must restart the same native task under its own tool-use id",
    )?;
    assert_base_projection(result, transcript, 7, None, Some(&["completed"; 7]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    use super::input::*;
    assert_user_messages_include(
        &p,
        &[
            CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_LAUNCH_PROMPT,
            CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_STOP_PROMPT,
            CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_RESUME_PROMPT,
            CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_FINAL_PROMPT,
        ],
    )?;
    let creators = rows(&p, "runs")
        .map(|r| {
            let m = rows(&p, "messages")
                .find(|m| m["id"] == r["userMessageId"])
                .unwrap_or(&Value::Null);
            format!(
                "{}:{}",
                m["createdBy"].as_str().unwrap_or_default(),
                m["creationSource"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    require(
        creators
            == [
                "user:web",
                "agent:provider",
                "user:web",
                "agent:provider",
                "user:web",
                "agent:provider",
                "user:web",
            ],
        "lifecycle wake creator/source differs",
    )?;
    let texts = |p: &Value, run: Option<&Value>| {
        rows(p, "turnItems")
            .filter(|i| {
                i["type"] == "assistant_message" && run.is_none_or(|run| i["runId"] == *run)
            })
            .map(|i| i["text"].as_str().unwrap_or_default().trim().to_owned())
            .collect::<Vec<_>>()
    };
    for (index, text) in [
        (0, "LAUNCHED"),
        (1, "A_REPORTED"),
        (2, "B_STOPPED"),
        (4, "RESUMED"),
        (5, "A_RESUME_REPORTED"),
        (6, "ALL_DONE"),
    ] {
        require(
            texts(&p, Some(&p["runs"][index]["id"])) == [text],
            "lifecycle reply attached to wrong run",
        )?;
    }
    require(
        texts(&p, Some(&p["runs"][3]["id"])).len() == 1,
        "stop notification must wake the root once",
    )?;
    require(
        texts(&p, None)
            .iter()
            .all(|t| !matches!(t.as_str(), "A_FIRST" | "A_SECOND")),
        "subagent text leaked into root",
    )?;
    require(
        rows(&p, "subagents").count() == 2,
        "expected two native subagents",
    )?;
    let a = rows(&p, "subagents")
        .find(|a| a["nativeTaskRef"]["nativeId"] == "a1a715b7d0bdfefea")
        .unwrap_or(&Value::Null);
    let b = rows(&p, "subagents")
        .find(|a| a["nativeTaskRef"]["nativeId"] == "af44d5c14aa3ce867")
        .unwrap_or(&Value::Null);
    require(
        a["status"] == "completed"
            && b["status"] == "cancelled"
            && a["result"] == "A_SECOND"
            && a["model"] == "claude-haiku-4-5"
            && a["runId"] == p["runs"][4]["id"],
        "resumed task identity/result/model/owner differs",
    )?;
    let notification = |name: &str, outcome: &str, child: &Value| {
        let mut source = json!({"kind":"subagent"});
        if !child.is_null() {
            source["childThreadId"] = child.clone();
        }
        json!({"summary":format!(r#"Subagent "{name}" {}"#,if outcome=="completed"{"finished"}else{"was stopped"}),"outcome":outcome,"source":source})
    };
    require(
        json!(background_notifications(&p))
            == json!([
                notification("Agent A", "completed", &a["childThreadId"]),
                notification("Agent B", "cancelled", &b["childThreadId"]),
                notification("Agent A", "completed", &a["childThreadId"])
            ]),
        "lifecycle completion notifications differ",
    )?;
    let events = serde_json::to_value(&result.domain_events)?;
    let events = events
        .as_array()
        .ok_or_else(|| crate::error::Error::Service {
            operation: "replay fixture assertion",
            detail: "missing events".into(),
        })?;
    let statuses = events
        .iter()
        .filter(|e| {
            e["type"] == "subagent.updated"
                && e["payload"]["nativeTaskRef"]["nativeId"] == "a1a715b7d0bdfefea"
        })
        .map(|e| &e["payload"]["status"])
        .collect::<Vec<_>>();
    let completed = statuses.iter().position(|s| **s == "completed");
    let running = statuses.iter().rposition(|s| **s == "running");
    require(
        completed.is_some()
            && running.is_some()
            && running > completed
            && statuses.last().is_some_and(|s| **s == "completed"),
        "resumed task must re-enter running then complete",
    )?;
    let child = |agent: &Value| -> Result<Value> {
        Ok(agent["childThreadId"]
            .as_str()
            .and_then(|id| result.projections.get(id))
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or(Value::Null))
    };
    let ca = child(a)?;
    let cb = child(b)?;
    require(
        ca.is_object()
            && ca["thread"]["modelSelection"]["model"] == "claude-haiku-4-5"
            && texts(&ca, None) == ["A_FIRST", "A_SECOND"],
        "resumed child conversation/model differs",
    )?;
    let mut items = rows(&ca, "turnItems").collect::<Vec<_>>();
    items.sort_by(|a, b| {
        a["ordinal"]
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&b["ordinal"].as_f64().unwrap_or_default())
    });
    let conversation = items
        .iter()
        .filter_map(|i| match i["type"].as_str() {
            Some("user_message") => Some(format!(
                "user:{}",
                i["text"].as_str().unwrap_or_default().trim()
            )),
            Some("assistant_message") => Some(format!(
                "assistant:{}",
                i["text"].as_str().unwrap_or_default().trim()
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    require(
        conversation
            == [
                "user:Reply with exactly: A_FIRST",
                "assistant:A_FIRST",
                "user:Reply with exactly: A_SECOND",
                "assistant:A_SECOND",
            ],
        "SendMessage resume must append a new child prompt/result",
    )?;
    let thinking = |p: &Value| {
        rows(p, "turnItems")
            .filter(|i| i["type"] == "reasoning" && i["title"] == "Thinking")
            .cloned()
            .collect::<Vec<_>>()
    };
    let ta = thinking(&ca);
    let tb = thinking(&cb);
    require(
        ta.len() == 2
            && ta[0]["text"]
                .as_str()
                .is_some_and(|s| s.contains("\"A_FIRST\""))
            && ta[1]["text"]
                .as_str()
                .is_some_and(|s| s.contains("\"A_SECOND\"")),
        "resumed child thinking lost/reordered",
    )?;
    require(
        ta.iter()
            .all(|i| i["status"] == "completed" && i["streaming"] == false && i["runId"].is_null()),
        "child thinking must settle outside app runs",
    )?;
    let order = rows(&ca, "turnItems")
        .filter_map(|i| {
            if i["type"] == "assistant_message" {
                Some(i["text"].as_str().unwrap_or_default().trim())
            } else if i["type"] == "reasoning" && i["title"] == "Thinking" {
                Some("thinking")
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    require(
        order == ["thinking", "A_FIRST", "thinking", "A_SECOND"],
        "child thinking/result order differs",
    )?;
    require(
        tb.len() == 1 && tb[0]["text"].as_str().is_some_and(|s| s.contains("B_DONE")),
        "stopped child thinking missing",
    )?;
    let thoughts = ta
        .iter()
        .chain(&tb)
        .map(|i| i["text"].clone())
        .collect::<Vec<_>>();
    require(
        !thinking(&p).iter().any(|i| thoughts.contains(&i["text"])),
        "subagent thinking leaked into root",
    )?;
    require(
        b["progress"] == "Running Wait 90 seconds then print B_DONE",
        "stop lost native progress",
    )?;
    for child in [&ca, &cb] {
        require(
            rows(child, "turnItems").all(|i| {
                !matches!(
                    i["status"].as_str(),
                    Some("pending" | "running" | "waiting")
                ) && !(i["type"] == "reasoning" && i["title"] != "Thinking")
            }),
            "stopped/finished child retained unsettled work or synthetic progress thinking",
        )?;
    }
    require(
        !events.iter().any(|e| {
            e["type"] == "provider-thread.updated"
                && e["payload"]["pendingBackgroundTasks"]
                    .as_array()
                    .is_some_and(|tasks| !tasks.is_empty())
        }),
        "native subagents must not become command background roster entries",
    )?;

    Ok(())
}
