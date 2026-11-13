use super::provider_event_ingestor::epoch_millis;
use crate::{api::*, error::Result};
use serde_json::Value;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleWindow {
    pub run_id: RunId,
    pub start_ms: i64,
    pub end_ms: i64,
}
pub fn idle_compaction_window(
    projection: &OrchestrationV2ThreadProjection,
    now_ms: i64,
) -> Result<Option<IdleWindow>> {
    window_value(&t3_contracts::stored::to_value(projection)?, now_ms)
}
pub(crate) fn latest_run(records: &Value) -> Option<&Value> {
    records["runs"]
        .as_array()?
        .iter()
        .filter(|run| run["status"] != "cancelled" || !run["startedAt"].is_null())
        .max_by_key(|run| run["ordinal"].as_u64())
}
pub(crate) fn window_value(records: &Value, now_ms: i64) -> Result<Option<IdleWindow>> {
    let thread = &records["thread"];
    if !thread["deletedAt"].is_null()
        || !thread["archivedAt"].is_null()
        || !thread["settledAt"].is_null()
        || thread["settledOverride"] == "settled"
        || thread["snoozedUntil"]
            .as_str()
            .and_then(epoch_millis)
            .is_some_and(|until| until > now_ms)
        || thread["lineage"]["relationshipToParent"] == "subagent"
        || thread["lineage"]["sideChat"] == true
    {
        return Ok(None);
    }
    let Some(run) = latest_run(records)
        .filter(|run| run["status"] == "completed" && !run["completedAt"].is_null())
    else {
        return Ok(None);
    };
    let Some(message) = records["messages"].as_array().and_then(|messages| {
        messages
            .iter()
            .find(|message| message["id"] == run["userMessageId"])
    }) else {
        return Ok(None);
    };
    if message["text"]
        .as_str()
        .is_some_and(|text| t3_contracts::wire::trim_js(text).eq_ignore_ascii_case("/compact"))
    {
        return Ok(None);
    }
    let array = |key| {
        records[key]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    if array("runtimeRequests")
        .iter()
        .any(|request| request["status"] == "pending")
        || array("contextTransfers").iter().any(|transfer| {
            transfer["type"] == "merge_back"
                && transfer["targetThreadId"] == thread["id"]
                && transfer["status"] == "pending"
        })
    {
        return Ok(None);
    }
    let has_active = array("runs").iter().any(|run| {
        matches!(
            run["status"].as_str(),
            Some("preparing" | "starting" | "running")
        )
    });
    if !has_active {
        let providers = array("providerThreads").iter().filter(|provider| {
            thread["activeProviderThreadId"].is_null()
                || provider["id"] == thread["activeProviderThreadId"]
        });
        if providers.into_iter().any(|provider| {
            provider["pendingBackgroundTasks"]
                .as_array()
                .is_some_and(|tasks| {
                    tasks
                        .iter()
                        .any(|task| task["taskId"].as_str().is_some_and(|id| !id.is_empty()))
                })
        }) {
            return Ok(None);
        }
        if array("turnItems").iter().any(|item| {
            matches!(
                item["type"].as_str(),
                Some("command_execution" | "dynamic_tool" | "subagent")
            ) && matches!(
                item["status"].as_str(),
                Some("pending" | "running" | "waiting")
            ) && !(item["type"] == "dynamic_tool" && item["input"]["persistent"] == true)
                && !array("runs")
                    .iter()
                    .any(|run| run["id"] == item["runId"] && run["status"] == "rolled_back")
        }) {
            return Ok(None);
        }
    }
    let Some(provider) = array("providerThreads")
        .iter()
        .find(|provider| provider["id"] == thread["activeProviderThreadId"])
    else {
        return Ok(None);
    };
    if provider["driver"] != "claudeAgent"
        || provider["id"] != run["providerThreadId"]
        || provider["providerInstanceId"] != thread["providerInstanceId"]
        || run["providerInstanceId"] != thread["providerInstanceId"]
    {
        return Ok(None);
    }
    let usage = array("providerTurns")
        .iter()
        .filter(|turn| turn["providerThreadId"] == provider["id"])
        .filter_map(|turn| turn.get("tokenUsage"))
        .max_by_key(|usage| usage["updatedAt"].as_str().and_then(epoch_millis));
    let Some(usage) =
        usage.filter(|usage| usage["usedTokens"].as_f64().unwrap_or(0.0) >= 200_000.0)
    else {
        return Ok(None);
    };
    let Some(end_ms) = usage["promptCacheExpiresAt"]
        .as_str()
        .and_then(epoch_millis)
        .filter(|end| *end > now_ms)
    else {
        return Ok(None);
    };
    Ok(Some(IdleWindow {
        run_id: serde_json::from_value(run["id"].clone())?,
        start_ms: end_ms - 120_000,
        end_ms,
    }))
}
