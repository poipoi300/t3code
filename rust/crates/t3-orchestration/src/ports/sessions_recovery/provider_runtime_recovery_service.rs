use crate::{
    api::*,
    effects::{EffectRequest, EffectRow, EffectStatus, PendingEffect},
    error::{Error, Result},
    stores::{
        StoreDatabase, outbox,
        receipts::{AggregateId, CommandReceipt, ReceiptStatus},
    },
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use tokio::sync::Mutex;

pub const PROCESS_BOUND_EFFECT_TYPES: &[&str] = &[
    "provider-session.apply-model-selection",
    "provider-turn.start",
    "provider-turn.interrupt",
    "provider-turn.steer",
    "provider-turn.restart",
    "runtime-request.respond",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryTrigger {
    Startup,
    Shutdown,
}
impl RecoveryTrigger {
    fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Shutdown => "shutdown",
        }
    }
    fn verb(self) -> &'static str {
        match self {
            Self::Startup => "restarted",
            Self::Shutdown => "shut down",
        }
    }
}

pub struct RecoveryPlan {
    pub commit: CommitRequest,
    pub summary: RecoverySummary,
}

struct Events<'a> {
    ids: &'a dyn IdGenerator,
    thread: &'a Value,
    now: &'a IsoDateTime,
    values: Vec<OrchestrationV2DomainEvent>,
    written: BTreeSet<(String, String)>,
}
impl Events<'_> {
    fn push(&mut self, kind: &str, payload: Value, scope: Value) -> Result<()> {
        let mut event = json!({"id": self.ids.next(IdKind::Event), "type": kind, "threadId":self.thread["id"], "occurredAt":self.now, "payload":payload});
        for key in ["runId", "nodeId", "driver", "providerInstanceId"] {
            if let Some(value) = scope.get(key).filter(|v| !v.is_null()) {
                event[key] = value.clone();
            }
        }
        if let Some(id) = payload["id"].as_str() {
            self.written.insert((kind.into(), id.into()));
        }
        self.values.push(serde_json::from_value(event)?);
        Ok(())
    }
    fn has(&self, kind: &str, id: &Value) -> bool {
        id.as_str()
            .is_some_and(|id| self.written.contains(&(kind.into(), id.into())))
    }
}

fn active(value: &Value) -> bool {
    matches!(
        value["status"].as_str(),
        Some("pending" | "running" | "waiting")
    )
}
fn background(value: &Value) -> bool {
    matches!(
        value["type"].as_str(),
        Some("command_execution" | "dynamic_tool" | "subagent")
    )
}
fn delegation(value: &Value) -> bool {
    value["origin"] == "app_owned"
        && (!value["childThreadId"].is_null() || value.get("remoteChild").is_some())
}
fn never_started(run: &Value) -> bool {
    run["startedAt"].is_null() && run.get("restartContinuationOfRunId").is_none()
}
fn cancelled(mut value: Value, now: &IsoDateTime) -> Value {
    value["status"] = json!("cancelled");
    value["completedAt"] = json!(now);
    value["updatedAt"] = json!(now);
    if value.get("streaming").is_some() {
        value["streaming"] = json!(false);
    }
    if value["type"] == "secret_request" && value["secretStatus"] == "pending" {
        value["secretStatus"] = json!("cancelled");
    }
    value
}

pub fn plan_recovery(
    projection: &OrchestrationV2ThreadProjection,
    effects: &[EffectRow],
    trigger: RecoveryTrigger,
    now: &IsoDateTime,
    ids: &dyn IdGenerator,
    continuation: Option<RunId>,
) -> Result<RecoveryPlan> {
    let state = t3_contracts::stored::to_value(&projection)?;
    let thread = &state["thread"];
    let rows = |key: &str| -> Result<&Vec<Value>> {
        state[key].as_array().ok_or_else(|| Error::Decode {
            entity: "recovery projection",
            detail: format!("missing {key}"),
        })
    };
    let all_runs = rows("runs")?;
    let items = rows("turnItems")?;
    let nodes = rows("nodes")?;
    let requests = rows("runtimeRequests")?;
    let attempts = rows("attempts")?;
    let turns = rows("providerTurns")?;
    let messages = rows("messages")?;
    let sessions = rows("providerSessions")?;
    let provider_threads = rows("providerThreads")?;
    let tasks = rows("subagents")?;
    let command_id = CommandId(format!(
        "command:runtime-reconcile:{}:{}:{}",
        trigger.name(),
        string_id(&thread["id"])?,
        now.0
    ));
    let mut events = Events {
        ids,
        thread,
        now,
        values: Vec::new(),
        written: BTreeSet::new(),
    };
    let native_child = thread["lineage"]["relationshipToParent"] == "subagent"
        && thread["creationSource"] == "provider";
    let questions = requests
        .iter()
        .filter(|request| {
            request["status"] == "pending"
                && request["kind"] == "user_input"
                && request["responseCapability"]["type"] == "live"
                && !native_child
                && items.iter().any(|item| {
                    item["type"] == "user_input_request"
                        && item["requestId"] == request["id"]
                        && item["nodeId"] == request["nodeId"]
                        && item["questions"].as_array().is_some_and(|questions| {
                            !questions.is_empty()
                                && questions.iter().all(|q| {
                                    q["question"].as_str().is_some_and(|s| !s.trim().is_empty())
                                })
                        })
                })
        })
        .collect::<Vec<_>>();
    let question_nodes = requests
        .iter()
        .filter(|request| {
            request["status"] == "pending"
                && (request["responseCapability"]["type"] == "message"
                    || questions.contains(request))
        })
        .map(|r| &r["nodeId"])
        .collect::<Vec<_>>();
    let closed_requests = requests
        .iter()
        .filter(|r| {
            r["status"] == "pending"
                && r["responseCapability"]["type"] != "message"
                && !questions.contains(r)
        })
        .collect::<Vec<_>>();
    let delegated_nodes = tasks
        .iter()
        .filter(|t| delegation(t))
        .map(|t| &t["id"])
        .chain(
            items
                .iter()
                .filter(|i| i["type"] == "subagent" && delegation(i))
                .map(|i| &i["subagentId"]),
        )
        .collect::<Vec<_>>();
    let mut runs = all_runs.iter().filter(|run| matches!(run["status"].as_str(), Some("preparing" | "starting" | "running" | "waiting")) && !(run["status"] == "waiting" && effects.iter().any(|effect| {
        matches!(&effect.request, EffectRequest::Capture { run_id, .. } if t3_contracts::stored::to_value(&run_id).ok().as_ref() == Some(&run["id"]))
            && effect.command_id.0 == format!("command:effect:checkpoint.capture:{}", run["id"].as_str().unwrap_or(""))
            && matches!(effect.status, EffectStatus::Pending | EffectStatus::Running)
    }))).collect::<Vec<_>>();
    let retryable = runs
        .iter()
        .copied()
        .filter(|run| {
            run["status"] == "starting"
                && never_started(run)
                && !turns
                    .iter()
                    .any(|turn| turn["runAttemptId"] == run["activeAttemptId"])
                && !requests.iter().any(|r| {
                    r["status"] == "pending"
                        && nodes
                            .iter()
                            .any(|n| n["id"] == r["nodeId"] && n["runId"] == run["id"])
                })
                && messages.iter().any(|message| {
                    message["id"] == run["userMessageId"]
                        && message["createdBy"] == "user"
                        && ["scheduledTaskId", "notification", "delegatedCompletion"]
                            .iter()
                            .all(|key| message.get(key).is_none())
                })
        })
        .collect::<Vec<_>>();
    for item in items.iter().filter(|item| {
        item["type"] == "user_message"
            && retryable
                .iter()
                .any(|run| run["id"] == item["runId"] && run["userMessageId"] == item["messageId"])
    }) {
        let mut item = item.clone();
        item["inputIntent"] = json!("queued_turn");
        item["status"] = json!("pending");
        item["startedAt"] = Value::Null;
        item["completedAt"] = Value::Null;
        item["updatedAt"] = json!(now);
        events.push(
            "turn-item.updated",
            item.clone(),
            json!({"runId":item["runId"]}),
        )?;
    }
    runs.retain(|run| !retryable.contains(run));
    let mut queued = all_runs
        .iter()
        .filter(|run| run["status"] == "queued")
        .collect::<Vec<_>>();
    let automatic = |run: &Value| {
        messages
            .iter()
            .any(|m| m["id"] == run["userMessageId"] && m.get("delegatedCompletion").is_some())
    };
    queued.sort_by(|a, b| {
        automatic(b)
            .cmp(&automatic(a))
            .then_with(|| {
                a["queuePosition"]
                    .as_u64()
                    .or_else(|| a["ordinal"].as_u64())
                    .cmp(
                        &b["queuePosition"]
                            .as_u64()
                            .or_else(|| b["ordinal"].as_u64()),
                    )
            })
            .then_with(|| a["ordinal"].as_u64().cmp(&b["ordinal"].as_u64()))
    });
    for (index, run) in retryable.iter().copied().chain(queued).enumerate() {
        let buffered = messages.iter().any(|m| {
            m["id"] == run["userMessageId"]
                && m["createdBy"] == "agent"
                && m["creationSource"] == "provider"
                && m.get("notification").is_some()
                && m.get("delegatedCompletion").is_none()
        });
        if buffered {
            runs.push(run);
            continue;
        }
        if run["queueHeld"] == true && retryable.is_empty() {
            continue;
        }
        let mut held = run.clone();
        held["status"] = json!("queued");
        held["queueHeld"] = json!(true);
        if !retryable.is_empty() {
            held["queuePosition"] = json!(index + 1);
        }
        events.push(
            "run.updated",
            held,
            json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]}),
        )?;
    }
    let unsent = runs
        .iter()
        .filter(|r| r["startedAt"].is_null() && r.get("usageLimitContinuationOfRunId").is_some())
        .map(|r| &r["id"])
        .collect::<Vec<_>>();
    if thread["limitRecovery"]["autoResume"] == true && !unsent.is_empty() {
        let mut metadata = thread.clone();
        metadata["limitRecovery"]["requestId"] = json!(command_id);
        metadata["updatedAt"] = json!(now);
        events.push("thread.metadata-updated", metadata, json!({}))?;
    }
    let hidden_prompts = items
        .iter()
        .filter(|i| i["type"] == "user_message" && unsent.contains(&&i["runId"]))
        .collect::<Vec<_>>();
    for item in &hidden_prompts {
        let mut payload = if active(item) {
            cancelled((*item).clone(), now)
        } else {
            (*item).clone()
        };
        payload["inputIntent"] = json!("queued_turn");
        payload["updatedAt"] = json!(now);
        events.push("turn-item.updated", payload, json!({"runId":item["runId"]}))?;
    }
    for question in questions {
        let mut payload = question.clone();
        payload["responseCapability"] = json!({"type":"message"});
        events.push(
            "runtime-request.updated",
            payload,
            json!({"nodeId":question["nodeId"]}),
        )?;
    }
    for request in &closed_requests {
        let mut payload = (*request).clone();
        payload["status"] = json!(if trigger == RecoveryTrigger::Startup {
            "expired"
        } else {
            "cancelled"
        });
        payload["responseCapability"] = json!({"type":"not_resumable","reason":format!("The server {} before this runtime request was resolved.", trigger.verb())});
        payload["resolvedAt"] = json!(now);
        events.push(
            "runtime-request.updated",
            payload,
            json!({"nodeId":request["nodeId"]}),
        )?;
    }
    let detail = format!(
        "Cancelled because the server {} before the provider work completed.",
        trigger.verb()
    );
    let mut lost = LostWork::default();
    for run in &runs {
        let scope = json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]});
        let mut payload = (*run).clone();
        payload["status"] = json!("cancelled");
        payload["queuePosition"] = Value::Null;
        payload["completedAt"] = json!(now);
        events.push("run.updated", payload, scope.clone())?;
        for attempt in attempts.iter().filter(|a| {
            a["runId"] == run["id"] && matches!(a["status"].as_str(), Some("pending" | "running"))
        }) {
            let mut payload = attempt.clone();
            payload["status"] = json!("cancelled");
            payload["completedAt"] = json!(now);
            events.push(
                "run-attempt.updated",
                payload,
                with_node(&scope, &attempt["rootNodeId"]),
            )?;
        }
        for node in nodes.iter().filter(|node| {
            node["runId"] == run["id"]
                && active(node)
                && !question_nodes.contains(&&node["id"])
                && !delegated_nodes.contains(&&node["id"])
        }) {
            let mut payload = node.clone();
            payload["status"] = json!("cancelled");
            payload["completedAt"] = json!(now);
            events.push("node.updated", payload, with_node(&scope, &node["id"]))?;
        }
        for turn in turns.iter().filter(|turn| {
            !turn["runAttemptId"].is_null()
                && attempts
                    .iter()
                    .any(|a| a["id"] == turn["runAttemptId"] && a["runId"] == run["id"])
                && matches!(turn["status"].as_str(), Some("pending" | "running"))
        }) {
            let mut payload = turn.clone();
            payload["status"] = json!("cancelled");
            payload["completedAt"] = json!(now);
            events.push(
                "provider-turn.updated",
                payload,
                with_node(&scope, &turn["nodeId"]),
            )?;
        }
        for message in messages
            .iter()
            .filter(|message| message["runId"] == run["id"] && message["streaming"] == true)
        {
            let mut payload = message.clone();
            payload["streaming"] = json!(false);
            payload["updatedAt"] = json!(now);
            events.push(
                "message.updated",
                payload,
                with_node(&scope, &message["nodeId"]),
            )?;
        }
        for item in items.iter().filter(|item| {
            item["runId"] == run["id"]
                && active(item)
                && !hidden_prompts.contains(item)
                && !question_nodes.contains(&&item["nodeId"])
                && !(item["type"] == "subagent" && delegation(item))
                && item["type"] != "link_request"
        }) {
            if run["status"] == "waiting"
                || (item["type"] == "subagent" && item["origin"] == "provider_native")
            {
                lost.item(item, all_runs, provider_threads);
            }
            events.push(
                "turn-item.updated",
                cancelled(item.clone(), now),
                with_node(&scope, &item["nodeId"]),
            )?;
        }
        if run["status"] == "queued" || unsent.contains(&&run["id"]) {
            continue;
        }
        events.push("turn-item.updated", json!({
            "id":format!("turn-item:run:{}:signal:server-recovery", encode_id(string_id(&run["id"])?)),"threadId":thread["id"],"runId":run["id"],"nodeId":run["rootNodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":0,"status":"completed","title":if trigger == RecoveryTrigger::Startup { "Server restarted" } else { "Server shut down" },"startedAt":now,"completedAt":now,"updatedAt":now,"type":"system_notice","message":detail
        }), with_node(&scope, &run["rootNodeId"]))?;
    }
    for task in tasks.iter().filter(|t| {
        t["origin"] == "app_owned"
            && active(t)
            && t["completionWake"] == "settled_only"
            && runs.iter().any(|r| r["id"] == t["runId"])
    }) {
        let mut payload = task.clone();
        payload["completionWake"] = json!("always");
        payload["updatedAt"] = json!(now);
        events.push("subagent.updated", payload, json!({"runId":task["runId"],"nodeId":task["id"],"driver":task["driver"],"providerInstanceId":task["providerInstanceId"]}))?;
    }
    for item in items.iter().filter(|item| {
        background(item)
            && active(item)
            && !delegation(item)
            && !runs.iter().any(|r| r["id"] == item["runId"])
    }) {
        lost.item(item, all_runs, provider_threads);
        let instance = item_instance(item, all_runs, provider_threads, thread);
        let scope =
            json!({"runId":item["runId"],"nodeId":item["nodeId"],"providerInstanceId":instance});
        events.push(
            "turn-item.updated",
            cancelled(item.clone(), now),
            scope.clone(),
        )?;
        if let Some(node) = nodes
            .iter()
            .find(|n| n["id"] == item["nodeId"] && active(n))
            && !events.has("node.updated", &node["id"])
        {
            let mut payload = node.clone();
            payload["status"] = json!("cancelled");
            payload["completedAt"] = json!(now);
            events.push("node.updated", payload, scope)?;
        }
    }
    for task in tasks.iter().filter(|t| t["origin"] == "provider_native") {
        let scope = json!({"runId":task["runId"],"nodeId":task["id"],"providerInstanceId":task["providerInstanceId"]});
        if active(task) && !events.has("subagent.updated", &task["id"]) {
            let mut payload = cancelled(task.clone(), now);
            payload["updatedAt"] = json!(now);
            let mut scope = scope.clone();
            scope["driver"] = task["driver"].clone();
            events.push("subagent.updated", payload, scope)?;
        }
        if let Some(node) = nodes
            .iter()
            .find(|node| node["id"] == task["id"] && active(node))
            && !events.has("node.updated", &node["id"])
        {
            let mut payload = node.clone();
            payload["status"] = json!("cancelled");
            payload["completedAt"] = json!(now);
            events.push("node.updated", payload, scope.clone())?;
        }
        if let Some(item) = items.iter().find(|item| {
            item["type"] == "subagent" && item["subagentId"] == task["id"] && active(item)
        }) && !events.has("turn-item.updated", &item["id"])
        {
            events.push("turn-item.updated", cancelled(item.clone(), now), scope)?;
        }
    }
    for node in nodes
        .iter()
        .filter(|n| n["kind"] == "root_turn" && n["runId"].is_null() && active(n))
    {
        let scope = json!({"nodeId":node["id"],"providerInstanceId":thread["providerInstanceId"]});
        if !events.has("node.updated", &node["id"]) {
            let mut payload = node.clone();
            payload["status"] = json!("cancelled");
            payload["completedAt"] = json!(now);
            events.push("node.updated", payload, scope.clone())?;
        }
        for item in items
            .iter()
            .filter(|i| i["nodeId"] == node["id"] && i["runId"].is_null() && active(i))
        {
            if !events.has("turn-item.updated", &item["id"]) {
                events.push(
                    "turn-item.updated",
                    cancelled(item.clone(), now),
                    scope.clone(),
                )?;
            }
        }
        for turn in turns
            .iter()
            .filter(|t| t["nodeId"] == node["id"] && t["runAttemptId"].is_null() && active(t))
        {
            if !events.has("provider-turn.updated", &turn["id"]) {
                let mut payload = turn.clone();
                payload["status"] = json!("cancelled");
                payload["completedAt"] = json!(now);
                events.push("provider-turn.updated", payload, scope.clone())?;
            }
        }
    }
    for provider_thread in provider_threads {
        if provider_thread["status"] != "active"
            && !provider_thread["pendingBackgroundTasks"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        {
            continue;
        }
        if provider_thread["ownerNodeId"].is_null() {
            for task in provider_thread["pendingBackgroundTasks"]
                .as_array()
                .into_iter()
                .flatten()
            {
                lost.roster(&provider_thread["id"], task);
            }
        }
        let mut payload = provider_thread.clone();
        if payload["status"] == "active" {
            payload["status"] = json!("idle");
        }
        payload["pendingBackgroundTasks"] = json!([]);
        payload["updatedAt"] = json!(now);
        events.push("provider-thread.updated", payload, json!({"driver":provider_thread["driver"],"providerInstanceId":provider_thread["providerInstanceId"]}))?;
    }
    for session in sessions {
        if session["status"] == "stopped"
            || (session["status"] == "error"
                && !session["extensionStatuses"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty()))
        {
            continue;
        }
        let mut payload = session.clone();
        if payload["status"] != "error" {
            payload["status"] = json!("stopped");
            payload["lastError"] = Value::Null;
        }
        if payload.get("extensionStatuses").is_some() {
            payload["extensionStatuses"] = json!([]);
        }
        payload["updatedAt"] = json!(now);
        events.push(
            "provider-session.updated",
            payload,
            json!({"driver":session["driver"],"providerInstanceId":session["providerInstanceId"]}),
        )?;
    }
    for (thread, work) in lost.threads {
        let latest = all_runs
            .iter()
            .filter(|r| {
                r["providerThreadId"] == thread
                    && r["status"] != "queued"
                    && r["status"] != "rolled_back"
                    && !never_started(r)
            })
            .max_by(|a, b| {
                let end = |r: &Value| {
                    r["completedAt"]
                        .as_str()
                        .and_then(super::usage_limit_recovery_worker::epoch_millis)
                        .unwrap_or(i64::MAX)
                };
                end(a)
                    .cmp(&end(b))
                    .then_with(|| a["ordinal"].as_u64().cmp(&b["ordinal"].as_u64()))
            });
        let Some(run) = latest else { continue };
        let mut merged = run["restartCancelledBackgroundWork"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for entry in work {
            let identity = |v: &Value| {
                v["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{}\0{}", v["kind"], v["label"]))
            };
            if !merged.iter().any(|v| identity(v) == identity(&entry)) {
                merged.push(entry);
            }
        }
        events.push(
            "run.background-work-cancelled",
            json!({"runId":run["id"],"restartCancelledBackgroundWork":merged}),
            json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]}),
        )?;
    }
    let thread_id = ThreadId(string_id(&thread["id"])?.into());
    let effects = continuation
        .map(|source_run_id| PendingEffect {
            id: format!("effect:restart-continuation:{}", source_run_id.0),
            command_id: command_id.clone(),
            thread_id: thread_id.clone(),
            request: EffectRequest::Continue { source_run_id },
            available_at: None,
        })
        .into_iter()
        .collect();
    Ok(RecoveryPlan {
        summary: RecoverySummary {
            terminalized_runs: runs.len() as u64,
            stopped_sessions: sessions
                .iter()
                .filter(|s| s["status"] != "stopped" && s["status"] != "error")
                .count() as u64,
            closed_requests: closed_requests.len() as u64,
            ..RecoverySummary::default()
        },
        commit: CommitRequest {
            receipt: Some(CommandReceipt {
                command_id,
                aggregate: AggregateId::Thread(thread_id.clone()),
                command_type: "provider-runtime.reconcile".into(),
                accepted_at: now.0.clone(),
                result_sequence: 0,
                status: ReceiptStatus::Accepted,
                error: None,
            }),
            guard: CommitGuard::None,
            decision: Decision {
                events: events.values,
                effects,
                cancellations: vec![CancelEffects {
                    thread_id,
                    effect_types: PROCESS_BOUND_EFFECT_TYPES
                        .iter()
                        .map(|s| (*s).into())
                        .collect(),
                    reason: detail,
                }],
            },
        },
    })
}

fn with_node(scope: &Value, node: &Value) -> Value {
    let mut scope = scope.clone();
    scope["nodeId"] = node.clone();
    scope
}
fn encode_id(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn compact(value: &Value, limit: usize) -> Option<String> {
    let text = value
        .as_str()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    if text.encode_utf16().count() <= limit {
        return Some(text);
    }
    let mut units = 0;
    let mut truncated = String::new();
    for ch in text.chars() {
        if units + ch.len_utf16() >= limit {
            break;
        }
        units += ch.len_utf16();
        truncated.push(ch);
    }
    truncated.push('…');
    Some(truncated)
}
fn label_with_id(name: &Value, fallback: &str, suffix: &str) -> String {
    let budget = 160_usize.saturating_sub(suffix.encode_utf16().count());
    if budget < 2 {
        return compact(&json!(format!("{fallback}{suffix}")), 160)
            .unwrap_or_else(|| fallback.into());
    }
    format!(
        "{}{suffix}",
        compact(name, budget)
            .or_else(|| compact(&json!(fallback), budget))
            .unwrap_or_default()
    )
}
#[derive(Default)]
struct LostWork {
    threads: Vec<(Value, Vec<Value>)>,
    native_ids: BTreeSet<String>,
}
impl LostWork {
    fn record(&mut self, thread: &Value, work: Value) {
        if thread.is_null() {
            return;
        }
        if let Some((_, items)) = self.threads.iter_mut().find(|(id, _)| id == thread) {
            items.push(work);
        } else {
            self.threads.push((thread.clone(), vec![work]));
        }
    }
    fn item(&mut self, item: &Value, runs: &[Value], threads: &[Value]) {
        if !background(item) {
            return;
        }
        if let Some(id) = item["nativeItemRef"]["nativeId"].as_str() {
            self.native_ids.insert(id.into());
        }
        let run_thread = runs
            .iter()
            .find(|r| r["id"] == item["runId"])
            .map(|r| &r["providerThreadId"])
            .filter(|t| !t.is_null());
        let own_thread = (!item["providerThreadId"].is_null()).then_some(&item["providerThreadId"]);
        let awaiting = if item["type"] == "subagent" && item["origin"] == "provider_native" {
            run_thread.or(own_thread)
        } else {
            own_thread.or(run_thread)
        };
        let Some(awaiting) = awaiting else { return };
        let mut work = match item["type"].as_str() {
            Some("subagent") => {
                let resume = threads
                    .iter()
                    .find(|t| t["id"] == item["providerThreadId"] && t["id"] != *awaiting)
                    .and_then(|t| t["nativeThreadRef"]["nativeId"].as_str())
                    .or_else(|| item["nativeItemRef"]["nativeId"].as_str());
                let suffix = if item["origin"] == "provider_native" {
                    resume.map(|id| format!(" (id {id})")).unwrap_or_default()
                } else {
                    String::new()
                };
                let name = if compact(&item["title"], 160).is_some() {
                    &item["title"]
                } else {
                    &item["prompt"]
                };
                json!({"kind":"subagent","label":label_with_id(name,"subagent",&suffix),"id":item["id"]})
            }
            Some("command_execution") => {
                json!({"kind":if item["monitor"] == true {"monitor"} else {"shell"},"label":compact(&item["input"],160).or_else(||compact(&item["title"],160)).unwrap_or_else(||"background command".into()),"id":item["id"]})
            }
            Some("dynamic_tool") => {
                json!({"kind":if item["input"]["persistent"] == true {"monitor"} else {"task"},"label":compact(&item["title"],160).or_else(||compact(&item["toolName"],160)).unwrap_or_else(||"background tool".into()),"id":item["id"]})
            }
            _ => return,
        };
        if item["type"] == "command_execution" && item["completionWake"] == "always" {
            work["awaited"] = json!(true);
        }
        self.record(awaiting, work);
    }
    fn roster(&mut self, thread: &Value, task: &Value) {
        let Some(id) = task["taskId"].as_str() else {
            return;
        };
        if self.native_ids.contains(id)
            || task["parentTaskId"]
                .as_str()
                .is_some_and(|p| self.native_ids.contains(p))
        {
            return;
        }
        self.native_ids.insert(id.into());
        let mut work = if let Some(parent) = task["parentTaskId"].as_str() {
            json!({"kind":"subagent","label":label_with_id(&task["description"],"background task",&format!(" (subagent {parent})")),"id":parent})
        } else {
            let kind = match task["kind"].as_str() {
                Some("command") => "shell",
                Some("subagent") => "subagent",
                Some("monitor") => "monitor",
                _ => "task",
            };
            let label = compact(&task["description"], 160)
                .map(|d| format!("{d} (id {id})"))
                .unwrap_or_else(|| id.into());
            json!({"kind":kind,"label":compact(&json!(label),160).unwrap_or_else(||"background task".into()),"id":id})
        };
        if task.get("parentTaskId").is_none() && task["awaited"] == true {
            work["awaited"] = json!(true);
        }
        self.record(thread, work);
    }
}
fn string_id(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| Error::Decode {
        entity: "recovery identifier",
        detail: "service requires expanded IDs".into(),
    })
}
fn item_instance<'a>(
    item: &Value,
    runs: &'a [Value],
    threads: &'a [Value],
    thread: &'a Value,
) -> Value {
    if let Some(run) = runs
        .iter()
        .find(|run| !item["runId"].is_null() && run["id"] == item["runId"])
    {
        return run["providerInstanceId"].clone();
    }
    if item["type"] == "subagent" {
        return item["providerInstanceId"].clone();
    }
    threads
        .iter()
        .find(|t| !item["providerThreadId"].is_null() && t["id"] == item["providerThreadId"])
        .or_else(|| threads.first())
        .unwrap_or(thread)["providerInstanceId"]
        .clone()
}

pub type RestartSelector = Arc<
    dyn Fn(&OrchestrationV2ThreadProjection, &IsoDateTime) -> Result<Option<RunId>> + Send + Sync,
>;

pub struct ProviderRuntimeRecoveryService {
    stores: StoreDatabase,
    projections: Arc<dyn ProjectionReader>,
    committer: Arc<dyn EventCommitter>,
    sessions: Arc<dyn SessionDirectory>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    restart: Option<RestartSelector>,
    restart_settings: Option<Arc<dyn SettingsService>>,
    signals: crate::runtime::worker::OutboxSignals,
    lane: Mutex<()>,
    shutdown_failures: std::sync::Mutex<Vec<(ThreadId, String)>>,
}
impl ProviderRuntimeRecoveryService {
    pub fn new(
        stores: StoreDatabase,
        projections: Arc<dyn ProjectionReader>,
        committer: Arc<dyn EventCommitter>,
        sessions: Arc<dyn SessionDirectory>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signals: crate::runtime::worker::OutboxSignals,
    ) -> Self {
        Self {
            stores,
            projections,
            committer,
            sessions,
            clock,
            ids,
            restart: None,
            restart_settings: None,
            signals,
            lane: Mutex::new(()),
            shutdown_failures: std::sync::Mutex::new(Vec::new()),
        }
    }
    pub fn with_restart_selector(mut self, selector: RestartSelector) -> Self {
        self.restart = Some(selector);
        self
    }

    pub fn with_restart_settings(mut self, settings: Arc<dyn SettingsService>) -> Self {
        self.restart_settings = Some(settings);
        self.restart = Some(Arc::new(|projection, now| {
            crate::ports::runs_ingestion::restart_continuation::restart_continuation_run(
                projection, now,
            )?
            .map(|run| match &run.id {
                IdOrIndex::Id(id) => Ok(id.clone()),
                IdOrIndex::Index(_) => Err(Error::Decode {
                    entity: "restart run ID",
                    detail: "compact ID in persisted projection".into(),
                }),
            })
            .transpose()
        }));
        self
    }

    async fn restart_preferences(&self) -> Result<Option<t3_contracts::settings::ServerSettings>> {
        let Some(settings) = &self.restart_settings else {
            return Ok(None);
        };
        match settings.get().await {
            Ok(settings) => Ok(Some(settings)),
            Err(Error::Cancelled | Error::Provider(crate::provider::ProviderError::Cancelled)) => {
                Err(Error::Cancelled)
            }
            Err(_) => Ok(None),
        }
    }

    fn restart_enabled(
        &self,
        projection: &OrchestrationV2ThreadProjection,
        settings: Option<&t3_contracts::settings::ServerSettings>,
    ) -> bool {
        if self.restart_settings.is_none() {
            return true;
        }
        let Some(settings) = settings else {
            return false;
        };
        let IdOrIndex::Id(project) = &projection.thread.project_id else {
            return false;
        };
        settings
            .project_settings_overrides
            .get(project)
            .and_then(|overrides| overrides.continue_threads_after_server_update)
            .unwrap_or(settings.continue_threads_after_server_update)
    }

    async fn projection(&self, thread: ThreadId) -> Result<OrchestrationV2ThreadProjection> {
        self.projections.runtime_recovery(thread).await
    }

    pub fn shutdown_preparation_failures(&self) -> Vec<(ThreadId, String)> {
        self.shutdown_failures
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    async fn prepare_thread(
        &self,
        id: ThreadId,
        selector: &RestartSelector,
        settings: Option<&t3_contracts::settings::ServerSettings>,
    ) -> Result<()> {
        let projection = self.projection(id.clone()).await?;
        if !self.restart_enabled(&projection, settings) {
            return Ok(());
        }
        let Some(run) = selector(&projection, &self.clock.now())? else {
            return Ok(());
        };
        let command = CommandId(format!("command:restart-prepare:{}", run.0));
        self.committer
            .commit_with_options(
                CommitRequest {
                    receipt: None,
                    guard: CommitGuard::None,
                    decision: Decision {
                        effects: vec![PendingEffect {
                            id: format!("effect:restart-continuation:{}", run.0),
                            command_id: command.clone(),
                            thread_id: id,
                            request: EffectRequest::Continue { source_run_id: run },
                            available_at: None,
                        }],
                        ..Decision::default()
                    },
                },
                CommitOptions {
                    command_id: Some(command),
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }

    pub async fn reconcile_shutdown(&self) -> Result<RecoverySummary> {
        self.reconcile_scope(None, RecoveryTrigger::Shutdown).await
    }

    async fn reconcile_scope(
        &self,
        thread_id: Option<ThreadId>,
        trigger: RecoveryTrigger,
    ) -> Result<RecoverySummary> {
        // Startup/shutdown reconciliation owns admission; concurrent passes cannot observe half a pass.
        let _lane = self.lane.lock().await;
        let target = thread_id.clone();
        let ids = match thread_id {
            Some(id) => vec![id],
            None => self.projections.recovery_thread_ids().await?,
        };
        let now = self.clock.now();
        let restart_settings = self.restart_preferences().await?;
        let mut summary = RecoverySummary::default();
        let mut live_at_start = BTreeMap::<String, OrchestrationV2ProviderSession>::new();
        for id in ids {
            let projection = self.projection(id.clone()).await?;
            let mut original = projection.clone();
            for session in &mut original.provider_sessions {
                let key = serde_json::to_string(&session.id)?;
                if let Some(previous) = live_at_start.get(&key) {
                    *session = previous.clone();
                } else if !matches!(
                    session.status,
                    ProviderSessionStatus::Stopped | ProviderSessionStatus::Error
                ) {
                    live_at_start.insert(key, session.clone());
                }
            }
            let continuation = if trigger == RecoveryTrigger::Startup
                && self.restart_enabled(&original, restart_settings.as_ref())
            {
                self.restart
                    .as_ref()
                    .map(|s| s(&original, &now))
                    .transpose()?
                    .flatten()
            } else {
                None
            };
            let lookup = id.clone();
            let effects = self.stores.read(move |db| {
                let ids = db.prepare("SELECT effect_id FROM orchestration_v2_effect_outbox WHERE thread_id=?1 AND status IN ('pending','running') ORDER BY rowid")?.query_map([lookup.0], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
                ids.iter().filter_map(|id| outbox::get(db, id).transpose()).collect::<Result<Vec<_>>>()
            }).await?;
            let plan = plan_recovery(
                &projection,
                &effects,
                trigger,
                &now,
                self.ids.as_ref(),
                continuation,
            )?;
            for session in &projection.provider_sessions {
                if let IdOrIndex::Id(id) = &session.id
                    && self.sessions.get(id.clone()).await?.is_some()
                {
                    self.sessions.detach(id.clone(), false).await?;
                }
            }
            if plan.commit.decision.events.is_empty() {
                let cancellation = plan.commit.decision.cancellations[0].clone();
                let at = now.0.clone();
                let cancelled = self
                    .stores
                    .transaction(move |tx| {
                        outbox::cancel_unsettled(
                            tx,
                            &cancellation.thread_id,
                            PROCESS_BOUND_EFFECT_TYPES,
                            &cancellation.reason,
                            &at,
                        )
                    })
                    .await?;
                summary.retired_effects += cancelled.len() as u64;
                self.signals.cancel(&cancelled);
                continue;
            }
            let result = self.committer.commit(plan.commit).await?;
            if result.committed {
                summary.terminalized_runs += plan.summary.terminalized_runs;
                summary.stopped_sessions += plan.summary.stopped_sessions;
                summary.closed_requests += plan.summary.closed_requests;
                summary.retired_effects += result.cancelled_effect_ids.len() as u64;
            }
        }
        let at = now.0;
        let (reconciliation, cancelled) = self.stores.transaction(move |tx| {
            let Some(thread) = target else {
                let types = serde_json::to_string(PROCESS_BOUND_EFFECT_TYPES)?;
                let cancelled = tx.prepare("SELECT effect_id FROM orchestration_v2_effect_outbox WHERE status IN ('pending','running') AND effect_type IN (SELECT value FROM json_each(?1)) ORDER BY rowid")?.query_map([types], |row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
                return Ok((outbox::reconcile_after_process_loss(tx, &at)?, cancelled));
            };
            let ids = tx.prepare("SELECT effect_id FROM orchestration_v2_effect_outbox WHERE thread_id=?1 AND status IN ('pending','running') ORDER BY rowid")?.query_map([&thread.0],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let mut result = crate::effects::Reconciliation::default();
            let mut cancelled = Vec::new();
            for id in ids {
                let Some(effect) = outbox::get(tx,&id)? else { continue };
                if effect.request.process_bound() {
                    result.cancelled += tx.execute("UPDATE orchestration_v2_effect_outbox SET status='cancelled',lease_owner=NULL,completed_at=?2,updated_at=?2,last_error='Cancelled because the server process ended before the effect completed.' WHERE effect_id=?1 AND status IN ('pending','running')",t3_persistence::rusqlite::params![id,at])?;
                    cancelled.push(id);
                } else if effect.status == EffectStatus::Running {
                    result.requeued += tx.execute("UPDATE orchestration_v2_effect_outbox SET status='pending',lease_owner=NULL,available_at=?2,updated_at=?2,last_error='Requeued after the previous server process ended.' WHERE effect_id=?1 AND status='running'",t3_persistence::rusqlite::params![id,at])?;
                }
            }
            Ok((result,cancelled))
        }).await?;
        summary.retired_effects += reconciliation.cancelled as u64;
        summary.requeued_effects += reconciliation.requeued as u64;
        self.signals.cancel(&cancelled);
        self.signals.notify();
        Ok(summary)
    }
}

#[async_trait]
impl RecoveryService for ProviderRuntimeRecoveryService {
    async fn recover(&self) -> Result<RecoverySummary> {
        self.reconcile_scope(None, RecoveryTrigger::Startup).await
    }
    async fn reconcile(&self, thread_id: Option<ThreadId>) -> Result<RecoverySummary> {
        self.reconcile_scope(thread_id, RecoveryTrigger::Startup)
            .await
    }
    async fn prepare_for_shutdown(&self) -> Result<()> {
        let Some(selector) = &self.restart else {
            return Ok(());
        };
        let settings = self.restart_preferences().await?;
        if self.restart_settings.is_some()
            && settings.as_ref().is_none_or(|settings| {
                !settings.continue_threads_after_server_update
                    && !settings
                        .project_settings_overrides
                        .values()
                        .any(|overrides| {
                            overrides.continue_threads_after_server_update == Some(true)
                        })
            })
        {
            return Ok(());
        }
        let ids = self.projections.recovery_thread_ids().await?;
        let settings = settings.as_ref();
        let prepare = |id: ThreadId| async move {
            let result = self.prepare_thread(id.clone(), selector, settings).await;
            match result {
                Err(
                    Error::Cancelled | Error::Provider(crate::provider::ProviderError::Cancelled),
                ) => Err(Error::Cancelled),
                result => Ok((id, result)),
            }
        };
        let mut failures = Vec::new();
        for pair in ids.chunks(2) {
            let results = match pair {
                [first, second] => {
                    let (left, right) =
                        tokio::try_join!(prepare(first.clone()), prepare(second.clone()))?;
                    vec![left, right]
                }
                [id] => vec![prepare(id.clone()).await?],
                _ => unreachable!(),
            };
            for (id, result) in results {
                match result {
                    Ok(()) => {}
                    Err(
                        Error::Cancelled
                        | Error::Provider(crate::provider::ProviderError::Cancelled),
                    ) => return Err(Error::Cancelled),
                    Err(error) => failures.push((id, error.to_string())),
                }
            }
        }
        *self
            .shutdown_failures
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = failures;
        Ok(())
    }
}
