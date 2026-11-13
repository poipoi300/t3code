use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use t3_contracts::orchestration_v2::{
    OrchestrationV2SubagentWorkflow, OrchestrationV2WorkflowRunHandles,
};

pub const CLAUDE_WORKFLOW_TASK_TYPE: &str = "local_workflow";

fn count(value: &Value) -> Option<u64> {
    let number = value.as_f64()?;
    ((0.0..=9_007_199_254_740_991.0).contains(&number) && number.fract() == 0.0)
        .then_some(number as u64)
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.trim().is_empty())
}

pub fn has_claude_workflow_snapshot(message: &Value) -> bool {
    message["workflow_progress"]
        .as_array()
        .is_some_and(|v| !v.is_empty())
}

pub fn parse_claude_workflow_run_handles(
    value: &Value,
) -> Option<OrchestrationV2WorkflowRunHandles> {
    if value["taskType"] != CLAUDE_WORKFLOW_TASK_TYPE {
        return None;
    }
    let handles: Map<String, Value> = ["runId", "transcriptDir", "scriptPath"]
        .into_iter()
        .filter_map(|key| text(&value[key]).map(|v| (key.into(), json!(v))))
        .collect();
    if handles.is_empty() {
        return None;
    }
    serde_json::from_value(Value::Object(handles)).ok()
}

fn agent(entry: &Value) -> Option<Value> {
    let index = count(&entry["index"])?;
    let label = text(&entry["label"])?;
    let started = count(&entry["startedAt"]).filter(|v| *v <= 8_640_000_000_000_000);
    let state = match entry["state"].as_str() {
        Some("queued") => "queued",
        Some("start") if started.is_none() => "queued",
        Some("done") => "completed",
        Some("error") => "failed",
        _ => "running",
    };
    let mut result = json!({"index":index,"label":label,"state":state});
    for key in ["agentId", "phaseTitle", "model"] {
        if let Some(v) = text(&entry[key]) {
            result[key] = json!(v);
        }
    }
    for (source, target) in [
        ("phaseIndex", "phaseIndex"),
        ("attempt", "attempt"),
        ("tokens", "totalTokens"),
        ("toolCalls", "toolCalls"),
        ("durationMs", "durationMs"),
    ] {
        if let Some(v) = count(&entry[source]) {
            result[target] = json!(v);
        }
    }
    for key in ["queuedAt", "startedAt"] {
        if let Some(v) = count(&entry[key]).filter(|v| *v <= 8_640_000_000_000_000) {
            result[key] = json!(v);
        }
    }
    if matches!(state, "completed" | "failed")
        && let Some(v) = count(&entry["lastProgressAt"]).filter(|v| *v <= 8_640_000_000_000_000)
    {
        result["completedAt"] = json!(v);
    }
    for (source, target) in [("promptPreview", "prompt"), ("resultPreview", "result")] {
        if entry[source].as_str().is_some_and(|s| !s.is_empty()) {
            result[target] = entry[source].clone();
        }
    }
    Some(result)
}

pub fn merge_claude_workflow_progress(
    previous: Option<&OrchestrationV2SubagentWorkflow>,
    name: Option<&str>,
    run_handles: Option<&OrchestrationV2WorkflowRunHandles>,
    message: &Value,
) -> Option<OrchestrationV2SubagentWorkflow> {
    let mut phases = BTreeMap::<u64, Value>::new();
    let mut agents = BTreeMap::<u64, Value>::new();
    let mut result = previous
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or_else(|| json!({"phases":[],"agents":[]}));
    for (key, destination) in [("phases", &mut phases), ("agents", &mut agents)] {
        if let Some(entries) = result[key].as_array() {
            for entry in entries {
                if let Some(index) = count(&entry["index"]) {
                    destination.insert(index, entry.clone());
                }
            }
        }
    }
    let mut evidence = previous.is_some() || name.is_some() || run_handles.is_some();
    if let Some(entries) = message["workflow_progress"].as_array() {
        for entry in entries {
            if entry["type"] == "workflow_phase" {
                if let (Some(index), Some(title)) = (count(&entry["index"]), text(&entry["title"]))
                {
                    phases.insert(index, json!({"index":index,"title":title}));
                    evidence = true;
                }
            } else if entry["type"] == "workflow_agent" {
                let Some(mut next) = agent(entry) else {
                    continue;
                };
                let index = count(&next["index"])?;
                if let Some(prior) = agents.get(&index) {
                    let attempt = count(&next["attempt"])
                        .or_else(|| count(&prior["attempt"]))
                        .unwrap_or(1);
                    let old_attempt = count(&prior["attempt"]).unwrap_or(1);
                    if attempt < old_attempt {
                        continue;
                    }
                    let restarted = attempt > old_attempt;
                    let regressed = !restarted
                        && ((matches!(prior["state"].as_str(), Some("completed" | "failed"))
                            && next["state"] != prior["state"])
                            || (prior["state"] == "running" && next["state"] == "queued"));
                    if regressed {
                        next.as_object_mut()?.extend(prior.as_object()?.clone());
                    } else {
                        let mut merged = if restarted {
                            ["phaseIndex", "phaseTitle"]
                                .into_iter()
                                .filter_map(|key| prior.get(key).map(|v| (key.into(), v.clone())))
                                .collect::<Map<_, _>>()
                        } else {
                            prior.as_object()?.clone()
                        };
                        merged.extend(next.as_object()?.clone());
                        next = Value::Object(merged);
                    }
                }
                agents.insert(index, next);
                evidence = true;
            }
        }
    }
    if !evidence {
        return None;
    }
    if let Some(name) = name {
        result["name"] = json!(name);
    }
    if let Some(handles) = run_handles {
        let mut merged = result["runHandles"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        merged.extend(serde_json::to_value(handles).ok()?.as_object()?.clone());
        result["runHandles"] = Value::Object(merged);
    }
    result["phases"] = json!(phases.into_values().collect::<Vec<_>>());
    result["agents"] = json!(agents.into_values().collect::<Vec<_>>());
    for (source, target) in [
        ("total_tokens", "totalTokens"),
        ("tool_uses", "toolCalls"),
        ("duration_ms", "durationMs"),
    ] {
        if let Some(n) = count(&message["usage"][source]) {
            result[target] = json!(n);
        }
    }
    serde_json::from_value(result).ok()
}
