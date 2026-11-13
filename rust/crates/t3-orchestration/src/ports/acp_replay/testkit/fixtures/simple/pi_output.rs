use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_pi_settled_token_usage(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let frames = replay_frames(transcript);
    let stats: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| {
            *inbound && frame["type"] == "response" && frame["command"] == "get_session_stats"
        })
        .map(|(_, frame)| &frame["data"])
        .collect();
    let p = projection_value_for(result, &transcript.scenario.0)?;
    require(
        rows(&p, "providerTurns").count() == stats.len(),
        "settled usage count must match session stats responses",
    )?;
    for (turn, stats) in rows(&p, "providerTurns").zip(stats) {
        let mut usage = turn["tokenUsage"].as_object().cloned().unwrap_or_default();
        usage.remove("updatedAt");
        require(
            Value::Object(usage)
                == json!({"usedTokens":stats["contextUsage"]["tokens"],"maxTokens":stats["contextUsage"]["contextWindow"],"inputTokens":stats["tokens"]["input"],"cachedInputTokens":stats["tokens"]["cacheRead"],"outputTokens":stats["tokens"]["output"]}),
            "settled Pi usage must follow each response in order",
        )?;
    }
    Ok(())
}
pub fn assert_pi_simple_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_simple_fixture_output(result, transcript)?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    let frames = replay_frames(transcript);
    let thinking: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| {
            *inbound && frame["assistantMessageEvent"]["type"] == "thinking_end"
        })
        .map(|(_, frame)| frame["assistantMessageEvent"]["content"].clone())
        .collect();
    require(
        thinking.len() == 1
            && json!(
                rows(&p, "turnItems")
                    .filter(|item| item["type"] == "reasoning")
                    .map(|item| item["text"].clone())
                    .collect::<Vec<_>>()
            ) == json!(thinking),
        "Pi thinking must project as one complete reasoning item",
    )?;
    assert_pi_settled_token_usage(result, transcript)?;
    let mut totals = Vec::new();
    for (_, frame) in frames
        .iter()
        .filter(|(inbound, frame)| *inbound && frame["type"] == "message_update")
    {
        let total = &frame["usage"]["totalTokens"];
        if total != 0 && !totals.contains(total) {
            totals.push(total.clone());
        }
    }
    let events: Vec<Value> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    let live: Vec<_> = events
        .iter()
        .filter(|event| {
            event["type"] == "provider-turn.updated"
                && event["payload"]["status"] == "running"
                && event["payload"].get("tokenUsage").is_some()
        })
        .map(|event| event["payload"]["tokenUsage"]["usedTokens"].clone())
        .collect();
    require(
        live == totals,
        "streamed usage must deduplicate nonzero cumulative totals",
    )?;
    let entries = frames
        .iter()
        .rfind(|(inbound, frame)| {
            *inbound && frame["type"] == "response" && frame["command"] == "get_entries"
        })
        .map(|(_, frame)| &frame["data"]);
    require(
        entries.is_some(),
        "recording must include get_entries response",
    )?;
    let user = entries
        .and_then(|data| {
            rows(data, "entries")
                .find(|entry| entry["type"] == "message" && entry["message"]["role"] == "user")
        })
        .and_then(|entry| entry["id"].as_str());
    require(
        user.is_some()
            && p["providerTurns"][0]["nativeTurnRef"]["nativeId"] == json!(user)
            && p["providerTurns"][0]["nativeTurnRef"]["strength"] == "strong",
        "native turn ref must use the session-tree user entry",
    )
}
