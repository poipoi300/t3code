#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn claude_compact_runs(p: &Value) -> Value {
    json!(rows(p,"runs").map(|run|{
        let message=rows(p,"messages").find(|m|m["id"]==run["userMessageId"]).unwrap_or(&Value::Null);
        let items=rows(p,"turnItems").filter(|i|i["runId"]==run["id"]).collect::<Vec<_>>();
        json!({"text":message["text"],"fromUser":message["createdBy"]=="user","status":run["status"],"replies":items.iter().filter(|i|i["type"]=="assistant_message").map(|i|i["text"].as_str().unwrap_or_default().trim()).collect::<Vec<_>>(),"compactions":items.iter().filter(|i|i["type"]=="compaction").map(|i|json!([i["beforeTokenCount"],i["afterTokenCount"]])).collect::<Vec<_>>()})
    }).collect::<Vec<_>>())
}

pub fn assert_claude_compact_after_peer_turn_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    let runs = claude_compact_runs(&p);
    require(
        runs[0]
            == json!({"text":super::input::CLAUDE_COMPACT_FIRST_PROMPT,"fromUser":true,"status":"completed","replies":["compact probe first turn"],"compactions":[]}),
        "initial compact probe differs",
    )?;
    require(
        runs[1]
            == json!({"text":"/compact","fromUser":true,"status":"completed","replies":[],"compactions":[[27445,1192]]}),
        "queued compaction differs",
    )?;
    require(
        runs.as_array().is_some_and(|r| r.len() == 3)
            && runs[2]["fromUser"] == false
            && runs[2]["status"] == "completed"
            && runs[2]["replies"] == json!(["PEER_ACK"])
            && runs[2]["compactions"] == json!([]),
        "peer continuation differs",
    )?;
    let usage = rows(&p, "runs")
        .map(|r| {
            rows(&p, "providerTurns")
                .find(|t| t["runAttemptId"] == r["activeAttemptId"])
                .map(|t| t["turnTokenUsage"].clone())
                .unwrap_or(Value::Null)
        })
        .collect::<Vec<_>>();
    assert_includes_fields(
        &usage[0],
        &json!({"outputTokens":7,"otherUsage":{"inputTokens":1015,"outputTokens":12}}),
        "initial ledger differs",
    )?;
    assert_includes_fields(
        &usage[1],
        &json!({"usageStatus":"complete","inputTokens":0,"outputTokens":0,"localCommand":true}),
        "compact must be a zero-token local command",
    )?;
    assert_includes_fields(
        &usage[2],
        &json!({"usageStatus":"complete","outputTokens":4}),
        "peer ledger differs",
    )?;
    require(
        usage[1].get("otherUsage").is_none() && usage[2].get("otherUsage").is_none(),
        "missing peer ledger must reset its baseline",
    )?;
    Ok(())
}
