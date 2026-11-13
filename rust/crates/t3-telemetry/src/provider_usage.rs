use crate::{
    analytics::AnalyticsService,
    product::{Properties, catalog_model_label, compact},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
use t3_contracts::{
    model_selection::ModelSelection, orchestration_v2::OrchestrationV2ProviderTurn,
    server::ServerProvider, wire::WireDateTime,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOrigin {
    pub created_by: String,
    pub creation_source: String,
    pub scheduled: bool,
    pub subagent: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnContext {
    pub run_id: Option<String>,
    pub model_selection: ModelSelection,
    pub runtime_mode: Option<String>,
    pub interaction_mode: Option<String>,
    pub origin: Option<TurnOrigin>,
    pub segment: Option<String>,
    pub restart_continuation: Option<bool>,
}
#[derive(Clone)]
pub struct TurnReport {
    pub driver: String,
    pub provider_turn: OrchestrationV2ProviderTurn,
    pub context: Option<TurnContext>,
}
pub struct SettledReport {
    pub run_id: String,
    pub status: String,
    pub completed_at: WireDateTime,
    pub children_running: bool,
}

fn millis(date: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(date)
        .ok()
        .map(|d| d.timestamp_millis())
}
fn selection(context: Option<&TurnContext>, label: &impl Fn(&str) -> Option<String>) -> Properties {
    let Some(context) = context else {
        return Properties::new();
    };
    let options = serde_json::to_value(&context.model_selection.options).unwrap_or(Value::Null);
    let option = |id: &str| {
        options
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| o["id"] == id)
            .and_then(|o| o.get("value"))
            .cloned()
    };
    let effort = option("reasoningEffort")
        .or_else(|| option("effort"))
        .or_else(|| option("variant"));
    let effort = effort.as_ref().and_then(Value::as_str).filter(|v| {
        [
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "ultrathink",
            "ultracode",
            "brutal",
        ]
        .contains(v)
    });
    let tier = option("serviceTier");
    let tier = if let Some(tier) = tier.as_ref().and_then(Value::as_str) {
        match tier {
            "default" => Some("standard"),
            "flex" => Some("flex"),
            "fast" | "priority" => Some("fast"),
            "ultrafast" => Some("ultrafast"),
            _ => None,
        }
    } else {
        option("fastMode")
            .and_then(|v| v.as_bool())
            .map(|fast| if fast { "fast" } else { "standard" })
    };
    let mut properties = compact(
        json!({"model":label(&context.model_selection.model),"effort":effort,"serviceTier":tier,"runtimeMode":context.runtime_mode,"interactionMode":context.interaction_mode}),
    );
    if let Some(origin) = &context.origin {
        properties.extend(compact(serde_json::to_value(origin).unwrap_or(Value::Null)));
    }
    properties
}
fn observation(usages: &[&Value], label: &impl Fn(&str) -> Option<String>) -> Properties {
    let models: BTreeSet<_> = usages
        .iter()
        .flat_map(|u| {
            u["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    if models.len() > 1 {
        return compact(json!({"mixedModels":true}));
    }
    if models.is_empty()
        || usages
            .iter()
            .any(|u| u["models"].as_array().is_none_or(Vec::is_empty))
    {
        return Properties::new();
    }
    compact(json!({"mixedModels":false,"observedModel":models.first().and_then(|m|label(m))}))
}
fn copy_counters(usage: &Value) -> Properties {
    [
        "inputTokens",
        "cachedInputTokens",
        "cacheCreationTokens",
        "outputTokens",
        "reasoningTokens",
    ]
    .into_iter()
    .filter_map(|key| {
        usage
            .get(key)
            .filter(|v| !v.is_null())
            .map(|v| (key.to_owned(), v.clone()))
    })
    .collect()
}
pub fn turn_properties(report: &TurnReport, label: impl Fn(&str) -> Option<String>) -> Properties {
    let turn = serde_json::to_value(&report.provider_turn).unwrap_or(Value::Null);
    let usage = &turn["turnTokenUsage"];
    let duration = turn["startedAt"]
        .as_str()
        .and_then(millis)
        .zip(turn["completedAt"].as_str().and_then(millis))
        .map(|(a, b)| (b - a).max(0));
    let context = report.context.as_ref();
    let child_kind = if context.and_then(|c| c.segment.as_deref()) == Some("native_child") {
        Some("native")
    } else if context
        .and_then(|c| c.origin.as_ref())
        .is_some_and(|o| o.subagent)
    {
        Some("t3")
    } else {
        None
    };
    let mut properties = compact(
        json!({"reportVersion":2,"provider":report.driver,"terminalStatus":turn["status"],"usageStatus":usage.get("usageStatus").unwrap_or(&json!("unavailable")),"usageScope":usage.get("usageScope").unwrap_or(&json!("main_agent")),
        "usageReported":!usage.is_null(),"hasSubagents":usage["hasSubagents"],"localCommand":(usage["localCommand"]==true).then_some(true),"otherInputTokens":usage["otherUsage"]["inputTokens"],"otherOutputTokens":usage["otherUsage"]["outputTokens"],"segment":context.and_then(|c|c.segment.as_ref()),"childKind":child_kind,"durationMs":duration}),
    );
    properties.extend(copy_counters(usage));
    properties.extend(selection(context, &label));
    properties.extend(observation(&[usage], &label));
    properties
}

#[derive(Clone)]
struct RequestTurn {
    child: bool,
    segment: Option<String>,
    turn: Value,
}
struct RequestEntry {
    driver: Option<String>,
    context: Option<TurnContext>,
    lifecycle_observed: bool,
    turns: BTreeMap<String, RequestTurn>,
}
#[derive(Default)]
pub struct RequestUsageLedger {
    open: BTreeMap<String, RequestEntry>,
    open_order: VecDeque<String>,
    settled: BTreeSet<String>,
    settled_order: VecDeque<String>,
}
fn combined_status(usages: &[&Value]) -> &'static str {
    if !usages.is_empty() && usages.iter().all(|u| u["usageStatus"] == "complete") {
        "complete"
    } else if usages
        .iter()
        .any(|u| u.get("inputTokens").is_some() || u.get("outputTokens").is_some())
    {
        "partial"
    } else {
        "unavailable"
    }
}
fn sum(usages: &[&Value], key: &str, require_all: bool) -> Option<u64> {
    let values: Vec<_> = usages.iter().filter_map(|u| u[key].as_u64()).collect();
    if values.is_empty() || (require_all && values.len() < usages.len()) {
        None
    } else {
        Some(values.into_iter().fold(0, u64::saturating_add))
    }
}
impl RequestUsageLedger {
    fn insert(&mut self, id: String, entry: RequestEntry) {
        self.open_order.push_back(id.clone());
        self.open.insert(id, entry);
        if self.open.len() > 2048
            && let Some(oldest) = self.open_order.pop_front()
        {
            self.open.remove(&oldest);
        }
    }
    pub fn start(&mut self, run_id: &str, driver: &str, context: TurnContext) {
        if self.settled.contains(run_id) || self.open.contains_key(run_id) {
            return;
        }
        let lifecycle_observed = context.restart_continuation != Some(true);
        self.insert(
            run_id.into(),
            RequestEntry {
                driver: Some(driver.into()),
                context: Some(context),
                lifecycle_observed,
                turns: BTreeMap::new(),
            },
        );
    }
    pub fn add(&mut self, report: &TurnReport) {
        let Some(run_id) = report.context.as_ref().and_then(|c| c.run_id.as_deref()) else {
            return;
        };
        if self.settled.contains(run_id) {
            return;
        }
        let turn = serde_json::to_value(&report.provider_turn).unwrap_or(Value::Null);
        if !["completed", "failed", "interrupted", "cancelled"]
            .contains(&turn["status"].as_str().unwrap_or(""))
        {
            return;
        }
        if !self.open.contains_key(run_id) {
            self.insert(
                run_id.into(),
                RequestEntry {
                    driver: None,
                    context: None,
                    lifecycle_observed: false,
                    turns: BTreeMap::new(),
                },
            );
        }
        let child =
            report.context.as_ref().and_then(|c| c.segment.as_deref()) == Some("native_child");
        let Some(entry) = self.open.get_mut(run_id) else {
            return;
        };
        if !child {
            if entry.driver.is_none() {
                entry.driver = Some(report.driver.clone());
            }
            if entry.context.is_none() {
                entry.context = report.context.clone();
            }
        }
        let key = turn["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| turn["id"].to_string());
        entry.turns.insert(
            key,
            RequestTurn {
                child,
                segment: report.context.as_ref().and_then(|c| c.segment.clone()),
                turn,
            },
        );
    }
    pub fn settle(
        &mut self,
        report: &SettledReport,
        label: impl Fn(&str) -> Option<String>,
    ) -> Option<Properties> {
        if !self.settled.insert(report.run_id.clone()) {
            return None;
        }
        self.settled_order.push_back(report.run_id.clone());
        if self.settled.len() > 2048
            && let Some(oldest) = self.settled_order.pop_front()
        {
            self.settled.remove(&oldest);
        }
        let entry = self.open.remove(&report.run_id)?;
        self.open_order.retain(|id| id != &report.run_id);
        let root: Vec<_> = entry.turns.values().filter(|t| !t.child).collect();
        let children: Vec<_> = entry.turns.values().filter(|t| t.child).collect();
        let root_usage: Vec<_> = root.iter().map(|t| &t.turn["turnTokenUsage"]).collect();
        let child_usage: Vec<_> = children.iter().map(|t| &t.turn["turnTokenUsage"]).collect();
        let measured_root: Vec<_> = root_usage
            .iter()
            .copied()
            .filter(|u| !u.is_null())
            .collect();
        let measured_children: Vec<_> = child_usage
            .iter()
            .copied()
            .filter(|u| !u.is_null())
            .collect();
        let status = combined_status(&root_usage);
        let root_complete = status == "complete";
        let all_other_known =
            !measured_root.is_empty() && measured_root.iter().all(|u| !u["otherUsage"].is_null());
        let any_subagents = measured_root.iter().any(|u| u["hasSubagents"] == true);
        let child_status = if children.is_empty() && !any_subagents && !report.children_running {
            "none"
        } else if children.is_empty() {
            "unavailable"
        } else if report.children_running && combined_status(&child_usage) == "complete" {
            "partial"
        } else {
            combined_status(&child_usage)
        };
        let duration = entry
            .turns
            .values()
            .filter_map(|t| t.turn["startedAt"].as_str().and_then(millis))
            .min()
            .zip(millis(&report.completed_at.0))
            .map(|(a, b)| (b - a).max(0));
        let origin = entry.context.as_ref().and_then(|c| c.origin.as_ref());
        let kind = origin
            .map(|o| {
                if o.scheduled {
                    "scheduled"
                } else if o.subagent {
                    "t3_child"
                } else if o.created_by == "user" {
                    "user"
                } else if o.creation_source == "provider" {
                    "provider_continuation"
                } else {
                    &o.created_by
                }
            })
            .unwrap_or("unknown");
        let mut properties = compact(
            json!({"reportVersion":2,"provider":entry.driver.unwrap_or_else(||"unknown".into()),"requestKind":kind,"restartContinuation":entry.context.as_ref().is_some_and(|c|c.restart_continuation==Some(true)).then_some(true),"terminalStatus":report.status,"lifecycleObserved":entry.lifecycle_observed,
            "providerTurns":root.len(),"continuations":root.iter().filter(|t|t.segment.as_deref()==Some("continuation")).count(),"steeringRestarts":root.iter().filter(|t|t.segment.as_deref()==Some("steering_restart")).count(),"usageStatus":if entry.lifecycle_observed||status=="unavailable"{status}else{"partial"},
            "inputTokens":sum(&measured_root,"inputTokens",root_complete),"cachedInputTokens":sum(&measured_root,"cachedInputTokens",true),"cacheCreationTokens":sum(&measured_root,"cacheCreationTokens",true),"outputTokens":sum(&measured_root,"outputTokens",root_complete),"reasoningTokens":sum(&measured_root,"reasoningTokens",true),
            "localCommand":measured_root.iter().any(|u|u["localCommand"]==true).then_some(true),"otherInputTokens":all_other_known.then(||measured_root.iter().map(|u|u["otherUsage"]["inputTokens"].as_u64().unwrap_or(0)).sum::<u64>()),"otherOutputTokens":all_other_known.then(||measured_root.iter().map(|u|u["otherUsage"]["outputTokens"].as_u64().unwrap_or(0)).sum::<u64>()),
            "nativeChildTurns":children.len(),"childUsageStatus":child_status,"childInputTokens":sum(&measured_children,"inputTokens",false),"childOutputTokens":sum(&measured_children,"outputTokens",false),"childrenRunning":report.children_running.then_some(true),"durationMs":duration}),
        );
        properties.extend(selection(entry.context.as_ref(), &label));
        let all_usage: Vec<_> = root_usage.into_iter().chain(child_usage).collect();
        properties.extend(observation(&all_usage, &label));
        Some(properties)
    }
}

pub struct ProviderTurnAnalytics {
    analytics: Arc<AnalyticsService>,
    ledger: tokio::sync::Mutex<RequestUsageLedger>,
}
impl ProviderTurnAnalytics {
    pub fn new(analytics: Arc<AnalyticsService>) -> Self {
        Self {
            analytics,
            ledger: tokio::sync::Mutex::new(RequestUsageLedger::default()),
        }
    }
    pub async fn run_started(&self, run_id: &str, driver: &str, context: TurnContext) {
        self.ledger.lock().await.start(run_id, driver, context);
    }
    pub async fn turn_completed(&self, report: &TurnReport, providers: &[ServerProvider]) {
        self.ledger.lock().await.add(report);
        self.analytics
            .record(
                "provider.turn.completed",
                turn_properties(report, |model| catalog_model_label(providers, model)),
            )
            .await;
    }
    pub async fn run_settled(&self, report: &SettledReport, providers: &[ServerProvider]) {
        if let Some(properties) = self
            .ledger
            .lock()
            .await
            .settle(report, |model| catalog_model_label(providers, model))
        {
            self.analytics
                .record("provider.request.completed", properties)
                .await;
        }
    }
}

#[derive(Default)]
pub struct DelegatedTaskAnalytics {
    reported: BTreeSet<String>,
}
pub struct DelegatedCompletion<'a> {
    pub id: &'a str,
    pub app_owned: bool,
    pub status: &'a str,
    pub caller: crate::product::ProviderDimensions,
    pub target: crate::product::ProviderDimensions,
    pub completion_wake: Option<&'a str>,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
}
impl DelegatedTaskAnalytics {
    pub async fn record(&mut self, analytics: &AnalyticsService, task: DelegatedCompletion<'_>) {
        if !task.app_owned
            || !["completed", "failed", "cancelled", "interrupted"].contains(&task.status)
            || self.reported.contains(task.id)
        {
            return;
        }
        let properties = compact(
            json!({"status":task.status,"callerProvider":task.caller.provider,"callerModel":task.caller.model,"targetProvider":task.target.provider,"targetModel":task.target.model,
            "crossProvider":crate::product::cross_provider(&task.caller.provider,&task.target.provider),"completionWake":task.completion_wake,"durationSeconds":task.started_at_ms.zip(task.completed_at_ms).map(|(a,b)|((b.saturating_sub(a) as f64)/1000.0).round() as u64)}),
        );
        analytics
            .record("mcp.delegated_task.finished", properties)
            .await;
        self.reported.insert(task.id.into());
    }
}
