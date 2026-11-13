use crate::{
    api::{Decision, IdGenerator, IdKind},
    effects::{EffectRequest, PendingEffect},
    error::Result,
};
use serde_json::{Value, json};
use t3_contracts::{base_schemas::*, orchestration_v2::*};

/// Plans deletion without releasing resources before the transaction commits.
pub fn plan_thread_deletion(
    command_id: &CommandId,
    thread_id: &ThreadId,
    projection: &OrchestrationV2ThreadProjection,
    attachment_ids: &[String],
    now: &IsoDateTime,
    ids: &dyn IdGenerator,
) -> Result<Decision> {
    let source = serde_json::to_value(projection)?;
    let mut decision = Decision::default();
    let mut emit = |kind: &str, payload: Value, metadata: Value| -> Result<()> {
        let mut event = json!({
            "id": ids.next(IdKind::Event), "threadId": thread_id,
            "type": kind, "occurredAt": now, "payload": payload,
        });
        if let (Some(target), Some(metadata)) = (event.as_object_mut(), metadata.as_object()) {
            target.extend(metadata.clone());
        }
        decision.events.push(serde_json::from_value(event)?);
        Ok(())
    };
    let mut thread = source["thread"].clone();
    if thread["deletedAt"].is_null() {
        thread["deletedAt"] = json!(now);
    }
    thread["titleRegeneration"] = Value::Null;
    thread["updatedAt"] = json!(now);
    emit(
        "thread.deleted",
        thread,
        json!({"providerInstanceId":source["thread"]["providerInstanceId"]}),
    )?;
    let mut runs = Vec::new();
    let mut active_ids = Vec::new();
    for source_run in source["runs"].as_array().into_iter().flatten() {
        let mut run = source_run.clone();
        if matches!(
            run["status"].as_str(),
            Some("preparing" | "queued" | "starting" | "running" | "waiting")
        ) {
            active_ids.push(run["id"].clone());
            run["status"] = json!("cancelled");
            run["queuePosition"] = Value::Null;
            run["completedAt"] = json!(now);
            emit(
                "run.updated",
                run.clone(),
                json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]}),
            )?;
        }
        runs.push(run);
    }
    for (field, kind, statuses, node_key) in [
        (
            "attempts",
            "run-attempt.updated",
            &["pending", "running"][..],
            "rootNodeId",
        ),
        (
            "nodes",
            "node.updated",
            &["pending", "running", "waiting"][..],
            "id",
        ),
    ] {
        for source_record in source[field].as_array().into_iter().flatten() {
            if !active_ids.contains(&source_record["runId"])
                || !statuses.contains(&source_record["status"].as_str().unwrap_or_default())
            {
                continue;
            }
            let Some(run) = runs.iter().find(|run| run["id"] == source_record["runId"]) else {
                continue;
            };
            let mut record = source_record.clone();
            record["status"] = json!("cancelled");
            record["completedAt"] = json!(now);
            emit(
                kind,
                record,
                json!({"runId":run["id"],"nodeId":source_record[node_key],"providerInstanceId":run["providerInstanceId"]}),
            )?;
        }
    }
    for request in source["runtimeRequests"].as_array().into_iter().flatten() {
        if request["status"] != "pending" {
            continue;
        }
        let mut cancelled = request.clone();
        cancelled["status"] = json!("cancelled");
        cancelled["responseCapability"] =
            json!({"type":"not_resumable","reason":"The thread was deleted."});
        cancelled["resolvedAt"] = json!(now);
        emit(
            "runtime-request.updated",
            cancelled,
            json!({"nodeId":request["nodeId"]}),
        )?;
    }
    let tasks: Vec<_> = source["subagents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|task| task["origin"] == "app_owned")
        .collect();
    let mut cohort_ids: Vec<_> = runs
        .iter()
        .filter(|run| run.get("delegatedCompletion").is_some())
        .map(|run| run["id"].clone())
        .collect();
    for task in &tasks {
        if !task["runId"].is_null() && !cohort_ids.contains(&task["runId"]) {
            cohort_ids.push(task["runId"].clone());
        }
    }
    for cohort_id in cohort_ids {
        let Some(run) = runs.iter().find(|run| run["id"] == cohort_id) else {
            continue;
        };
        let mut disposed = run.clone();
        disposed["delegatedCompletion"] = json!({
            "disposition":"disposed", "nextGeneration":run["delegatedCompletion"]["nextGeneration"].as_u64().unwrap_or(1), "delivery":null,
        });
        let mut metadata =
            json!({"runId":run["id"],"providerInstanceId":run["providerInstanceId"]});
        if !run["rootNodeId"].is_null() {
            metadata["nodeId"] = run["rootNodeId"].clone();
        }
        emit("run.updated", disposed, metadata)?;
        for task in tasks.iter().filter(|task| task["runId"] == run["id"]) {
            if matches!(
                task["completionDelivery"]["state"].as_str(),
                Some("acknowledged" | "delivered" | "disposed")
            ) {
                continue;
            }
            let mut disposed = (*task).clone();
            disposed["completionDelivery"] = json!({"state":"disposed","observedByRunId":null});
            disposed["updatedAt"] = json!(now);
            emit(
                "subagent.updated",
                disposed,
                json!({"runId":run["id"],"nodeId":task["id"],"driver":task["driver"],"providerInstanceId":task["providerInstanceId"]}),
            )?;
        }
    }
    let mut requests = Vec::new();
    for session in source["providerSessions"].as_array().into_iter().flatten() {
        if session["status"] == "stopped" {
            continue;
        }
        emit(
            "provider-session.detached",
            json!({"providerSessionId":session["id"],"detachedAt":now,"reason":"Thread deleted."}),
            json!({"driver":session["driver"],"providerInstanceId":session["providerInstanceId"]}),
        )?;
        let session_id: ProviderSessionId = serde_json::from_value(session["id"].clone())?;
        requests.push((
            format!("provider-session.detach:{}", session_id.0),
            EffectRequest::Detach {
                provider_session_id: session_id,
                detail: Some("Thread deleted.".into()),
                revoke_mcp_credential: Some(true),
            },
        ));
    }
    requests.extend([
        (
            "scheduled-tasks.cleanup".into(),
            EffectRequest::CleanupScheduledTasks,
        ),
        ("terminal.cleanup".into(), EffectRequest::CleanupTerminals),
        ("preview.cleanup".into(), EffectRequest::CleanupPreviews),
    ]);
    let mut attachments = Vec::new();
    for id in attachment_ids {
        if !attachments.contains(id) {
            attachments.push(id.clone());
        }
    }
    if !attachments.is_empty() {
        requests.push((
            "attachment.cleanup".into(),
            EffectRequest::CleanupAttachments {
                attachment_ids: attachments,
            },
        ));
    }
    decision.effects = requests
        .into_iter()
        .map(|(suffix, request)| PendingEffect {
            id: format!("effect:{}:{suffix}", command_id.0),
            command_id: command_id.clone(),
            thread_id: thread_id.clone(),
            request,
            available_at: None,
        })
        .collect();
    Ok(decision)
}
