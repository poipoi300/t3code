use crate::{
    api::{OrchestrationV2DomainEvent, OrchestrationV2StoredEvent, ProjectionWriter},
    error::{Error, Result},
};
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::thread_pull_request::ThreadPullRequestKey;
use t3_persistence::rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter, types::Value as SqlValue,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteProjectionWriter;
impl ProjectionWriter for SqliteProjectionWriter {
    fn apply(&self, tx: &Transaction<'_>, event: &OrchestrationV2StoredEvent) -> Result<()> {
        apply(tx, event)
    }
}

pub struct ParsedPullRequestKey {
    pub key: ThreadPullRequestKey,
    pub authority: Option<String>,
}

pub trait PullRequestIdentityRegistry: Send + Sync {
    fn parse_change_request_url(&self, url: &str) -> Option<ParsedPullRequestKey>;
    fn canonical_repository_key(&self, key: &str) -> String;
    fn contains_repository(&self, key: &str) -> bool;
}

pub struct RegistryProjectionWriter {
    registry: Arc<dyn PullRequestIdentityRegistry>,
}
impl RegistryProjectionWriter {
    pub fn new(registry: Arc<dyn PullRequestIdentityRegistry>) -> Self {
        Self { registry }
    }
}
impl ProjectionWriter for RegistryProjectionWriter {
    fn apply(&self, tx: &Transaction<'_>, event: &OrchestrationV2StoredEvent) -> Result<()> {
        apply_with_registry(tx, event, Some(self.registry.as_ref()))
    }
}
fn sql_value(value: &Value) -> Result<SqlValue> {
    Ok(match value {
        Value::Null => SqlValue::Null,
        Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
        Value::Number(value) => SqlValue::Integer(value.as_i64().ok_or_else(|| Error::Decode {
            entity: "projection index",
            detail: "integer outside SQLite range".into(),
        })?),
        Value::String(value) => SqlValue::Text(value.clone()),
        _ => {
            return Err(Error::Decode {
                entity: "projection index",
                detail: "expected scalar".into(),
            });
        }
    })
}
fn existing(db: &Connection, table: &str, column: &str, id: &Value) -> Result<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            &format!("SELECT payload_json FROM {table} WHERE {column}=?1"),
            [sql_value(id)?],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw.map(|raw| serde_json::from_str(&raw)).transpose()?)
}

pub fn apply(tx: &Transaction<'_>, stored: &OrchestrationV2StoredEvent) -> Result<()> {
    apply_with_registry(tx, stored, None)
}

pub fn apply_with_registry(
    tx: &Transaction<'_>,
    stored: &OrchestrationV2StoredEvent,
    registry: Option<&dyn PullRequestIdentityRegistry>,
) -> Result<()> {
    use OrchestrationV2DomainEvent::*;
    let event = t3_contracts::stored::to_value(&stored.event)?;
    let mut payload = event["payload"].clone();
    if let Some(fields) = payload.as_object() {
        for (key, value) in fields {
            let scalar_id = key == "id" || key.ends_with("Id");
            let id_array = key.ends_with("Ids");
            if (scalar_id && !value.is_null() && !value.is_string())
                || (id_array
                    && value
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| !id.is_string())))
            {
                return Err(Error::Decode {
                    entity: "projection ID",
                    detail: format!("{key} must contain expanded string IDs"),
                });
            }
        }
    }
    if matches!(&stored.event, ProviderTurnUpdated(_))
        && payload.get("tokenUsage").is_none()
        && let Some(previous) = existing(
            tx,
            "orchestration_v2_projection_provider_turns",
            "provider_turn_id",
            &payload["id"],
        )?
        && let Some(usage) = previous.get("tokenUsage")
    {
        payload["tokenUsage"] = usage.clone();
    }
    if matches!(&stored.event, ProviderThreadUpdated(_)) {
        for (key, default) in [
            ("pendingBackgroundTasks", json!([])),
            ("contextUsage", Value::Null),
            ("nativeMetadata", Value::Null),
            ("goal", Value::Null),
        ] {
            if payload.get(key).is_none() {
                payload[key] = default;
            }
        }
        payload = t3_contracts::stored::to_value(&serde_json::from_value::<
            crate::api::OrchestrationV2ProviderThread,
        >(payload)?)?;
    }
    if matches!(&stored.event, TurnItemUpdated(_)) {
        order_discriminator(
            &mut payload,
            "type",
            &[
                "deferredWork",
                "toolNonExecutionKind",
                "toolSurface",
                "toolIcon",
                "toolSource",
                "id",
                "threadId",
                "runId",
                "nodeId",
                "providerThreadId",
                "providerTurnId",
                "nativeItemRef",
                "parentItemId",
                "ordinal",
                "status",
                "title",
                "startedAt",
                "completedAt",
                "updatedAt",
                "createdBy",
                "creationSource",
            ],
        );
    }
    if matches!(&stored.event, PlanUpdated(_)) {
        order_discriminator(
            &mut payload,
            "kind",
            &[
                "id",
                "threadId",
                "runId",
                "nodeId",
                "status",
                "detailInTurnItem",
            ],
        );
    }
    let payload_json = crate::stores::event_store::encode_json(&payload)?;
    let statement: Option<(&str, Vec<SqlValue>)> = match &stored.event {
        ThreadVisitRecorded(_) => None,
        ThreadCreated(_)
        | ThreadArchived(_)
        | ThreadUnarchived(_)
        | ThreadDeleted(_)
        | ThreadSettled(_)
        | ThreadUnsettled(_)
        | ThreadSnoozed(_)
        | ThreadUnsnoozed(_)
        | ThreadAutoSettleSet(_)
        | ThreadPinned(_)
        | ThreadUnpinned(_)
        | ThreadPinReordered(_)
        | ThreadActiveReordered(_)
        | ThreadTagsUpdated(_)
        | ThreadVisited(_)
        | ThreadMarkedUnread(_)
        | ThreadMetadataUpdated(_)
        | ThreadPullRequestSynced(_)
        | ThreadRuntimeModeUpdated(_)
        | ThreadInteractionModeUpdated(_)
        | ThreadModelSelectionUpdated(_)
        | ThreadProviderSwitched(_)
        | ThreadGuestShareUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_threads (thread_id, project_id, title, default_provider, provider_instance_id, runtime_mode, interaction_mode, active_provider_thread_id, created_at, updated_at, archived_at, deleted_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) ON CONFLICT(thread_id) DO UPDATE SET project_id = excluded.project_id, title = excluded.title, default_provider = excluded.default_provider, provider_instance_id = excluded.provider_instance_id, runtime_mode = excluded.runtime_mode, interaction_mode = excluded.interaction_mode, active_provider_thread_id = excluded.active_provider_thread_id, created_at = excluded.created_at, updated_at = excluded.updated_at, archived_at = excluded.archived_at, deleted_at = excluded.deleted_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["projectId"])?,
                sql_value(&payload["title"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["runtimeMode"])?,
                sql_value(&payload["interactionMode"])?,
                sql_value(&payload["activeProviderThreadId"])?,
                sql_value(&payload["createdAt"])?,
                sql_value(&payload["updatedAt"])?,
                sql_value(&payload["archivedAt"])?,
                sql_value(&payload["deletedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        RunCreated(_) | RunUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_runs (run_id, thread_id, ordinal, provider, provider_instance_id, provider_thread_id, status, requested_at, completed_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(run_id) DO UPDATE SET thread_id = excluded.thread_id, ordinal = excluded.ordinal, provider = excluded.provider, provider_instance_id = excluded.provider_instance_id, provider_thread_id = excluded.provider_thread_id, status = excluded.status, requested_at = excluded.requested_at, completed_at = excluded.completed_at, payload_json = CASE WHEN json_type(excluded.payload_json, '$.restartCancelledBackgroundWork') IS NULL AND json_type(orchestration_v2_projection_runs.payload_json, '$.restartCancelledBackgroundWork') IS NOT NULL THEN json_set(CASE WHEN json_type(excluded.payload_json, '$.delegatedCompletion') IS NULL AND json_type(orchestration_v2_projection_runs.payload_json, '$.delegatedCompletion') IS NOT NULL THEN json_set(excluded.payload_json, '$.delegatedCompletion', json_extract(orchestration_v2_projection_runs.payload_json, '$.delegatedCompletion')) ELSE excluded.payload_json END, '$.restartCancelledBackgroundWork', json_extract(orchestration_v2_projection_runs.payload_json, '$.restartCancelledBackgroundWork')) ELSE CASE WHEN json_type(excluded.payload_json, '$.delegatedCompletion') IS NULL AND json_type(orchestration_v2_projection_runs.payload_json, '$.delegatedCompletion') IS NOT NULL THEN json_set(excluded.payload_json, '$.delegatedCompletion', json_extract(orchestration_v2_projection_runs.payload_json, '$.delegatedCompletion')) ELSE excluded.payload_json END END",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["ordinal"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["providerThreadId"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["requestedAt"])?,
                sql_value(&payload["completedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        RunAttemptCreated(_) | RunAttemptUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_run_attempts (attempt_id, thread_id, run_id, attempt_ordinal, root_node_id, provider, provider_instance_id, provider_thread_id, provider_turn_id, status, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ON CONFLICT(attempt_id) DO UPDATE SET thread_id = excluded.thread_id, run_id = excluded.run_id, attempt_ordinal = excluded.attempt_ordinal, root_node_id = excluded.root_node_id, provider = excluded.provider, provider_instance_id = excluded.provider_instance_id, provider_thread_id = excluded.provider_thread_id, provider_turn_id = excluded.provider_turn_id, status = excluded.status, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&event["threadId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["attemptOrdinal"])?,
                sql_value(&payload["rootNodeId"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["providerThreadId"])?,
                sql_value(&payload["providerTurnId"])?,
                sql_value(&payload["status"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        NodeUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_nodes (node_id, thread_id, run_id, parent_node_id, root_node_id, kind, status, provider_thread_id, provider_turn_id, started_at, completed_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT(node_id) DO UPDATE SET thread_id = excluded.thread_id, run_id = excluded.run_id, parent_node_id = excluded.parent_node_id, root_node_id = excluded.root_node_id, kind = excluded.kind, status = excluded.status, provider_thread_id = excluded.provider_thread_id, provider_turn_id = excluded.provider_turn_id, started_at = excluded.started_at, completed_at = excluded.completed_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["parentNodeId"])?,
                sql_value(&payload["rootNodeId"])?,
                sql_value(&payload["kind"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["providerThreadId"])?,
                sql_value(&payload["providerTurnId"])?,
                sql_value(&payload["startedAt"])?,
                sql_value(&payload["completedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        SubagentUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_subagents (subagent_id, thread_id, run_id, parent_node_id, provider, driver, provider_instance_id, provider_thread_id, child_thread_id, origin, status, started_at, completed_at, updated_at, completion_delivery_state, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) ON CONFLICT(subagent_id) DO UPDATE SET thread_id = excluded.thread_id, run_id = excluded.run_id, parent_node_id = excluded.parent_node_id, provider = excluded.provider, driver = excluded.driver, provider_instance_id = excluded.provider_instance_id, provider_thread_id = excluded.provider_thread_id, child_thread_id = excluded.child_thread_id, origin = excluded.origin, status = excluded.status, started_at = excluded.started_at, completed_at = excluded.completed_at, updated_at = excluded.updated_at, completion_delivery_state = COALESCE( excluded.completion_delivery_state, orchestration_v2_projection_subagents.completion_delivery_state ), payload_json = CASE WHEN json_type(excluded.payload_json, '$.completionDelivery') IS NULL AND json_type(orchestration_v2_projection_subagents.payload_json, '$.completionDelivery') IS NOT NULL THEN json_set( excluded.payload_json, '$.completionDelivery', json_extract( orchestration_v2_projection_subagents.payload_json, '$.completionDelivery' ) ) ELSE excluded.payload_json END",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["parentNodeId"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["driver"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["providerThreadId"])?,
                sql_value(&payload["childThreadId"])?,
                sql_value(&payload["origin"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["startedAt"])?,
                sql_value(&payload["completedAt"])?,
                sql_value(&payload["updatedAt"])?,
                sql_value(&payload["completionDelivery"]["state"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        ProviderSessionAttached(_) | ProviderSessionUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_provider_sessions (provider_session_id, thread_id, provider, driver, provider_instance_id, status, updated_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(provider_session_id) DO UPDATE SET thread_id = excluded.thread_id, provider = excluded.provider, driver = excluded.driver, provider_instance_id = excluded.provider_instance_id, status = excluded.status, updated_at = excluded.updated_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&event["threadId"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["driver"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["updatedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        ProviderThreadUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_provider_threads (provider_thread_id, thread_id, owner_node_id, provider, driver, provider_instance_id, provider_session_id, status, first_run_ordinal, last_run_ordinal, updated_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT(provider_thread_id) DO UPDATE SET thread_id = excluded.thread_id, owner_node_id = excluded.owner_node_id, provider = excluded.provider, driver = excluded.driver, provider_instance_id = excluded.provider_instance_id, provider_session_id = excluded.provider_session_id, status = excluded.status, first_run_ordinal = excluded.first_run_ordinal, last_run_ordinal = excluded.last_run_ordinal, updated_at = excluded.updated_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["appThreadId"])?,
                sql_value(&payload["ownerNodeId"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["driver"])?,
                sql_value(&payload["providerInstanceId"])?,
                sql_value(&payload["providerSessionId"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["firstRunOrdinal"])?,
                sql_value(&payload["lastRunOrdinal"])?,
                sql_value(&payload["updatedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        ProviderTurnUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_provider_turns (provider_turn_id, thread_id, provider_thread_id, node_id, run_attempt_id, ordinal, status, started_at, completed_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(provider_turn_id) DO UPDATE SET thread_id = excluded.thread_id, provider_thread_id = excluded.provider_thread_id, node_id = excluded.node_id, run_attempt_id = excluded.run_attempt_id, ordinal = excluded.ordinal, status = excluded.status, started_at = excluded.started_at, completed_at = excluded.completed_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&event["threadId"])?,
                sql_value(&payload["providerThreadId"])?,
                sql_value(&payload["nodeId"])?,
                sql_value(&payload["runAttemptId"])?,
                sql_value(&payload["ordinal"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["startedAt"])?,
                sql_value(&payload["completedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        RuntimeRequestUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_runtime_requests (runtime_request_id, thread_id, node_id, provider_turn_id, kind, status, created_at, resolved_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(runtime_request_id) DO UPDATE SET thread_id = excluded.thread_id, node_id = excluded.node_id, provider_turn_id = excluded.provider_turn_id, kind = excluded.kind, status = excluded.status, created_at = excluded.created_at, resolved_at = excluded.resolved_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&event["threadId"])?,
                sql_value(&payload["nodeId"])?,
                sql_value(&payload["providerTurnId"])?,
                sql_value(&payload["kind"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["createdAt"])?,
                sql_value(&payload["resolvedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        MessageUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_messages (message_id, thread_id, run_id, node_id, role, streaming, created_at, updated_at, payload_json, search_text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(message_id) DO UPDATE SET thread_id = excluded.thread_id, run_id = excluded.run_id, node_id = excluded.node_id, role = excluded.role, streaming = excluded.streaming, created_at = excluded.created_at, updated_at = excluded.updated_at, payload_json = excluded.payload_json, search_text = excluded.search_text",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["nodeId"])?,
                sql_value(&payload["role"])?,
                sql_value(&payload["streaming"])?,
                sql_value(&payload["createdAt"])?,
                sql_value(&payload["updatedAt"])?,
                SqlValue::Text(payload_json.clone()),
                readable_search_text(payload["text"].as_str().unwrap_or_default())?
                    .map_or(SqlValue::Null, SqlValue::Text),
            ],
        )),
        PlanUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_plans (plan_id, thread_id, run_id, node_id, kind, status, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(plan_id) DO UPDATE SET thread_id = excluded.thread_id, run_id = excluded.run_id, node_id = excluded.node_id, kind = excluded.kind, status = excluded.status, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["nodeId"])?,
                sql_value(&payload["kind"])?,
                sql_value(&payload["status"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        TurnItemUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_turn_items (turn_item_id, thread_id, run_id, node_id, provider_thread_id, provider_turn_id, parent_item_id, ordinal, type, status, updated_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT(turn_item_id) DO UPDATE SET thread_id = excluded.thread_id, run_id = excluded.run_id, node_id = excluded.node_id, provider_thread_id = excluded.provider_thread_id, provider_turn_id = excluded.provider_turn_id, parent_item_id = excluded.parent_item_id, ordinal = excluded.ordinal, type = excluded.type, status = excluded.status, updated_at = excluded.updated_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["nodeId"])?,
                sql_value(&payload["providerThreadId"])?,
                sql_value(&payload["providerTurnId"])?,
                sql_value(&payload["parentItemId"])?,
                sql_value(&payload["ordinal"])?,
                sql_value(&payload["type"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["updatedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        CheckpointScopeCreated(_) => Some((
            "INSERT INTO orchestration_v2_projection_checkpoint_scopes (scope_id, thread_id, run_id, node_id, provider_thread_id, kind, ordinal_within_parent, advances_app_run_count, created_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(scope_id) DO UPDATE SET thread_id = excluded.thread_id, run_id = excluded.run_id, node_id = excluded.node_id, provider_thread_id = excluded.provider_thread_id, kind = excluded.kind, ordinal_within_parent = excluded.ordinal_within_parent, advances_app_run_count = excluded.advances_app_run_count, created_at = excluded.created_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["nodeId"])?,
                sql_value(&payload["providerThreadId"])?,
                sql_value(&payload["kind"])?,
                sql_value(&payload["ordinalWithinParent"])?,
                sql_value(&payload["advancesAppRunCount"])?,
                sql_value(&payload["createdAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        CheckpointCaptured(_) => Some((
            "INSERT INTO orchestration_v2_projection_checkpoints (checkpoint_id, thread_id, scope_id, run_id, node_id, ordinal_within_scope, app_run_ordinal, status, captured_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(checkpoint_id) DO UPDATE SET thread_id = excluded.thread_id, scope_id = excluded.scope_id, run_id = excluded.run_id, node_id = excluded.node_id, ordinal_within_scope = excluded.ordinal_within_scope, app_run_ordinal = excluded.app_run_ordinal, status = excluded.status, captured_at = excluded.captured_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["scopeId"])?,
                sql_value(&payload["runId"])?,
                sql_value(&payload["nodeId"])?,
                sql_value(&payload["ordinalWithinScope"])?,
                sql_value(&payload["appRunOrdinal"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["capturedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        ContextHandoffUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_context_handoffs (context_handoff_id, thread_id, target_run_id, to_provider_thread_id, strategy, status, updated_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(context_handoff_id) DO UPDATE SET thread_id = excluded.thread_id, target_run_id = excluded.target_run_id, to_provider_thread_id = excluded.to_provider_thread_id, strategy = excluded.strategy, status = excluded.status, updated_at = excluded.updated_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["threadId"])?,
                sql_value(&payload["targetRunId"])?,
                sql_value(&payload["toProviderThreadId"])?,
                sql_value(&payload["strategy"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["updatedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        ContextTransferCreated(_) | ContextTransferUpdated(_) => Some((
            "INSERT INTO orchestration_v2_projection_context_transfers (context_transfer_id, source_thread_id, target_thread_id, target_run_id, type, status, updated_at, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(context_transfer_id) DO UPDATE SET source_thread_id = excluded.source_thread_id, target_thread_id = excluded.target_thread_id, target_run_id = excluded.target_run_id, type = excluded.type, status = excluded.status, updated_at = excluded.updated_at, payload_json = excluded.payload_json",
            vec![
                sql_value(&payload["id"])?,
                sql_value(&payload["sourceThreadId"])?,
                sql_value(&payload["targetThreadId"])?,
                sql_value(&payload["targetRunId"])?,
                sql_value(&payload["type"])?,
                sql_value(&payload["status"])?,
                sql_value(&payload["updatedAt"])?,
                SqlValue::Text(payload_json.clone()),
            ],
        )),
        ThreadPullRequestLinkUpdated(_) => {
            let mut thread = existing(
                tx,
                "orchestration_v2_projection_threads",
                "thread_id",
                &event["threadId"],
            )?
            .ok_or_else(|| {
                Error::ThreadNotFound(t3_contracts::base_schemas::ThreadId(
                    event["threadId"].as_str().unwrap_or_default().into(),
                ))
            })?;
            update_pull_request(&mut thread, &payload, registry);
            let mut next = event.clone();
            next["type"] = "thread.pull-request-synced".into();
            next["payload"] = thread;
            apply(
                tx,
                &OrchestrationV2StoredEvent {
                    sequence: stored.sequence,
                    command_id: stored.command_id.clone(),
                    event: serde_json::from_value(next)?,
                },
            )?;
            None
        }
        RunBackgroundWorkCancelled(_) => {
            tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json=json_set(payload_json,'$.restartCancelledBackgroundWork',json(?1)) WHERE run_id=?2 AND thread_id=?3", params![crate::stores::event_store::encode_json(&payload["restartCancelledBackgroundWork"])?, sql_value(&payload["runId"])?, sql_value(&event["threadId"])?])?;
            None
        }
        ProviderSessionDetached(_) => {
            tx.execute("DELETE FROM orchestration_v2_projection_provider_session_bindings WHERE provider_session_id=?1 AND thread_id=?2",params![sql_value(&payload["providerSessionId"])?,sql_value(&event["threadId"])?])?;
            None
        }
        CheckpointRollbackRequested(_) => None,
    };
    if let Some((sql, values)) = statement {
        tx.execute(sql, params_from_iter(values))?;
    }
    if matches!(&stored.event, ProviderSessionAttached(_)) {
        tx.execute("INSERT OR IGNORE INTO orchestration_v2_projection_provider_session_bindings(provider_session_id,thread_id) VALUES(?1,?2)", params![sql_value(&payload["id"])?,sql_value(&event["threadId"])?])?;
    }
    if matches!(&stored.event, ProviderThreadUpdated(_))
        && !payload["appThreadId"].is_null()
        && !(payload["status"] == "not_loaded"
            && payload["firstRunOrdinal"].is_null()
            && payload["nativeThreadRef"].is_null()
            && payload["providerSessionId"].is_null())
    {
        tx.execute("UPDATE orchestration_v2_projection_threads SET active_provider_thread_id=?1,updated_at=?2,payload_json=json_set(payload_json,'$.activeProviderThreadId',?1,'$.updatedAt',?2) WHERE thread_id=?3", params![sql_value(&payload["id"])?,sql_value(&payload["updatedAt"])?,sql_value(&payload["appThreadId"]) ?])?;
    }
    if !event["type"]
        .as_str()
        .unwrap_or_default()
        .starts_with("thread.")
    {
        tx.execute("UPDATE orchestration_v2_projection_threads SET updated_at=?1,payload_json=json_set(payload_json,'$.updatedAt',?1) WHERE thread_id=?2", params![sql_value(&event["occurredAt"])?,sql_value(&event["threadId"])?])?;
    }
    Ok(())
}

fn pull_request_key(
    link: &Value,
    registry: Option<&dyn PullRequestIdentityRegistry>,
) -> (String, String, Value) {
    if let Some(registry) = registry {
        let parsed = link["url"]
            .as_str()
            .and_then(|url| registry.parse_change_request_url(url));
        let repository = link["repository"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let authority = link["authority"].as_str().or_else(|| {
            parsed
                .as_ref()
                .filter(|parsed| {
                    parsed.key.repository.0 == repository
                        && Some(parsed.key.number) == link["number"].as_u64()
                })
                .and_then(|parsed| parsed.authority.as_deref())
        });
        let host = authority
            .or_else(|| link["host"].as_str())
            .unwrap_or("unknown")
            .trim()
            .to_lowercase();
        let canonical = registry.canonical_repository_key(&format!("{host}/{repository}"));
        if let Some((host, repository)) = canonical.split_once('/') {
            return (host.into(), repository.into(), link["number"].clone());
        }
    }
    let host = link["authority"]
        .as_str()
        .or_else(|| link["host"].as_str())
        .unwrap_or_else(|| {
            link["url"]
                .as_str()
                .and_then(|url| url.split("://").nth(1))
                .and_then(|path| path.split('/').next())
                .unwrap_or("unknown")
        });
    let host = host.trim().to_lowercase();
    (
        host,
        link["repository"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_lowercase(),
        link["number"].clone(),
    )
}
pub(crate) fn pull_request_links(
    thread: &Value,
    registry: Option<&dyn PullRequestIdentityRegistry>,
) -> Vec<Value> {
    match thread["pullRequests"].as_array() {
        Some(links) => links.clone(),
        None if thread["linkedPullRequest"].is_null() => Vec::new(),
        None => {
            let linked = &thread["linkedPullRequest"];
            let parsed = registry.and_then(|registry| {
                linked["url"]
                    .as_str()
                    .and_then(|url| registry.parse_change_request_url(url))
                    .filter(|parsed| {
                        Some(parsed.key.number) == linked["number"].as_u64()
                            && (parsed.authority.is_some()
                                || registry.contains_repository(&format!(
                                    "{}/{}",
                                    parsed.key.host.0, parsed.key.repository.0
                                )))
                    })
            });
            let (host, repository, number) = if let Some(parsed) = parsed {
                let legacy = json!({"host":parsed.key.host,"authority":parsed.authority,"repository":parsed.key.repository,"number":parsed.key.number,"url":linked["url"]});
                pull_request_key(&legacy, registry)
            } else {
                pull_request_key(linked, None)
            };
            vec![
                json!({"host":host,"repository":repository,"number":number,"url":linked["url"],"source":"manual","linkedAt":"1970-01-01T00:00:00.000Z","snapshot":null,"stack":null}),
            ]
        }
    }
}
fn update_pull_request(
    thread: &mut Value,
    updated: &Value,
    registry: Option<&dyn PullRequestIdentityRegistry>,
) {
    let mut links = pull_request_links(thread, registry);
    for link in &mut links {
        if pull_request_key(link, registry) == pull_request_key(updated, registry) {
            *link = updated.clone();
        }
    }
    thread["pullRequests"] = Value::Array(links);
}

fn readable_search_text(text: &str) -> Result<Option<String>> {
    let mut output = String::new();
    let mut remaining = text;
    let prefix = "[Assistant quote](t3-citation://v1/";
    while let Some(start) = remaining.find(prefix) {
        output.push_str(&remaining[..start]);
        let citation = &remaining[start + prefix.len()..];
        let Some(end) = citation.find(')') else {
            output.push_str(&remaining[start..]);
            remaining = "";
            break;
        };
        let href = &citation[..end];
        if let Some(readable) = citation_text(href) {
            output.push_str(&readable);
        } else {
            output.push_str(&remaining[start..start + prefix.len() + end + 1]);
        }
        remaining = &citation[end + 1..];
    }
    output.push_str(remaining);
    Ok((output != text).then_some(output))
}
fn percent_decode(text: &str, plus: bool) -> Option<String> {
    let input = text.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            let decoded = input
                .get(index + 1)
                .zip(input.get(index + 2))
                .and_then(|(a, b)| {
                    Some(((*a as char).to_digit(16)? * 16 + (*b as char).to_digit(16)?) as u8)
                });
            if let Some(value) = decoded {
                output.push(value);
                index += 3;
                continue;
            }
            if !plus {
                return None;
            }
        }
        output.push(if plus && input[index] == b'+' {
            b' '
        } else {
            input[index]
        });
        index += 1;
    }
    if plus {
        Some(String::from_utf8_lossy(&output).into_owned())
    } else {
        String::from_utf8(output).ok()
    }
}
fn citation_text(href: &str) -> Option<String> {
    let (path, query) = href.split_once('?')?;
    if path.split('/').count() != 3
        || href.contains('#')
        || href.encode_utf16().count() + "t3-citation://v1/".len() > 256_168
        || href
            .chars()
            .any(|character| t3_contracts::wire::trim_js(&character.to_string()).is_empty())
    {
        return None;
    }
    for part in path.split('/') {
        let decoded = percent_decode(part, false)?;
        if matches!(decoded.as_str(), "." | "..")
            || t3_contracts::wire::trim_js(&decoded).is_empty()
            || decoded.encode_utf16().count() > 4096
        {
            return None;
        }
    }
    let mut fields = std::collections::BTreeMap::new();
    for field in query.split('&') {
        if field.is_empty() {
            continue;
        }
        let (key, value) = field.split_once('=').unwrap_or((field, ""));
        let key = percent_decode(key, true)?;
        if fields.insert(key, percent_decode(value, true)?).is_some() {
            return None;
        }
    }
    for key in ["text", "start", "end", "prefix", "suffix"] {
        if !fields.contains_key(key) {
            return None;
        }
    }
    if fields.len() != 5 + usize::from(fields.contains_key("comment")) {
        return None;
    }
    for key in ["start", "end"] {
        let field = &fields[key];
        if field.is_empty() || field.len() > 16 || !field.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    let start = fields["start"].parse::<u64>().ok()?;
    let end = fields["end"].parse::<u64>().ok()?;
    if end <= start
        || end > 9_007_199_254_740_991
        || t3_contracts::wire::trim_js(&fields["text"]).is_empty()
        || fields["text"].encode_utf16().count() > 8000
        || fields
            .get("comment")
            .is_some_and(|text| text.encode_utf16().count() > 8000)
        || ["prefix", "suffix"]
            .iter()
            .any(|key| fields[*key].encode_utf16().count() > 32)
    {
        return None;
    }
    Some(match fields.get("comment") {
        Some(comment) => format!("{}\nComment: {}", fields["text"], comment),
        None => fields["text"].clone(),
    })
}

pub fn empty_projection(
    event: &OrchestrationV2DomainEvent,
) -> Result<crate::api::OrchestrationV2ThreadProjection> {
    let OrchestrationV2DomainEvent::ThreadCreated(event) = event else {
        return Err(Error::Unsupported(
            "empty projection requires thread.created",
        ));
    };
    let mut value = serde_json::Map::new();
    value.insert(
        "thread".into(),
        t3_contracts::stored::to_value(&event.payload)?,
    );
    for field in [
        "runs",
        "attempts",
        "nodes",
        "subagents",
        "providerSessions",
        "providerThreads",
        "providerTurns",
        "runtimeRequests",
        "messages",
        "plans",
        "turnItems",
        "checkpointScopes",
        "checkpoints",
        "contextHandoffs",
        "contextTransfers",
        "visibleTurnItems",
    ] {
        value.insert(field.into(), json!([]));
    }
    value.insert(
        "updatedAt".into(),
        t3_contracts::stored::to_value(&event.occurred_at)?,
    );
    Ok(serde_json::from_value(Value::Object(value))?)
}
pub fn apply_to_projection(
    projection: &crate::api::OrchestrationV2ThreadProjection,
    event: &OrchestrationV2DomainEvent,
) -> Result<crate::api::OrchestrationV2ThreadProjection> {
    apply_to_projection_with_registry(projection, event, None)
}
pub fn apply_to_projection_with_registry(
    projection: &crate::api::OrchestrationV2ThreadProjection,
    event: &OrchestrationV2DomainEvent,
    registry: Option<&dyn PullRequestIdentityRegistry>,
) -> Result<crate::api::OrchestrationV2ThreadProjection> {
    use OrchestrationV2DomainEvent::*;
    let mut value = t3_contracts::stored::to_value(projection)?;
    let event_value = t3_contracts::stored::to_value(event)?;
    let payload = &event_value["payload"];
    value["thread"]["updatedAt"] = event_value["occurredAt"].clone();
    value["updatedAt"] = event_value["occurredAt"].clone();
    let field = match event {
        ThreadVisitRecorded(_) => return Ok(projection.clone()),
        ThreadPullRequestLinkUpdated(_) => {
            value = t3_contracts::stored::to_value(projection)?;
            update_pull_request(&mut value["thread"], payload, registry);
            None
        }
        ThreadVisited(_) | ThreadMarkedUnread(_) => {
            value = t3_contracts::stored::to_value(projection)?;
            value["thread"] = payload.clone();
            None
        }
        ThreadActiveReordered(_) | ThreadTagsUpdated(_) | ThreadPullRequestSynced(_) => {
            value["thread"] = payload.clone();
            value["updatedAt"] = payload["updatedAt"].clone();
            None
        }
        ThreadCreated(_)
        | ThreadArchived(_)
        | ThreadUnarchived(_)
        | ThreadDeleted(_)
        | ThreadSettled(_)
        | ThreadUnsettled(_)
        | ThreadSnoozed(_)
        | ThreadUnsnoozed(_)
        | ThreadAutoSettleSet(_)
        | ThreadPinned(_)
        | ThreadUnpinned(_)
        | ThreadPinReordered(_)
        | ThreadMetadataUpdated(_)
        | ThreadRuntimeModeUpdated(_)
        | ThreadInteractionModeUpdated(_)
        | ThreadModelSelectionUpdated(_)
        | ThreadProviderSwitched(_)
        | ThreadGuestShareUpdated(_) => {
            value["thread"] = payload.clone();
            None
        }
        RunCreated(_) | RunUpdated(_) => Some("runs"),
        RunBackgroundWorkCancelled(_) => {
            if let Some(runs) = value["runs"].as_array_mut() {
                for run in runs {
                    if run["id"] == payload["runId"] {
                        run["restartCancelledBackgroundWork"] =
                            payload["restartCancelledBackgroundWork"].clone();
                    }
                }
            }
            None
        }
        RunAttemptCreated(_) | RunAttemptUpdated(_) => Some("attempts"),
        NodeUpdated(_) => Some("nodes"),
        SubagentUpdated(_) => Some("subagents"),
        ProviderSessionAttached(_) | ProviderSessionUpdated(_) => Some("providerSessions"),
        ProviderSessionDetached(_) => {
            if let Some(sessions) = value["providerSessions"].as_array_mut() {
                sessions.retain(|session| session["id"] != payload["providerSessionId"]);
            }
            None
        }
        ProviderThreadUpdated(_) => {
            if payload["appThreadId"] == value["thread"]["id"]
                && !(payload["status"] == "not_loaded"
                    && payload["firstRunOrdinal"].is_null()
                    && payload["nativeThreadRef"].is_null()
                    && payload["providerSessionId"].is_null())
            {
                value["thread"]["activeProviderThreadId"] = payload["id"].clone();
            }
            Some("providerThreads")
        }
        ProviderTurnUpdated(_) => Some("providerTurns"),
        RuntimeRequestUpdated(_) => Some("runtimeRequests"),
        MessageUpdated(_) => Some("messages"),
        TurnItemUpdated(_) => Some("turnItems"),
        PlanUpdated(_) => Some("plans"),
        CheckpointScopeCreated(_) => Some("checkpointScopes"),
        CheckpointCaptured(_) => Some("checkpoints"),
        CheckpointRollbackRequested(_) => None,
        ContextHandoffUpdated(_) => Some("contextHandoffs"),
        ContextTransferCreated(_) | ContextTransferUpdated(_) => Some("contextTransfers"),
    };
    if let Some(field) = field {
        let mut next = payload.clone();
        let records = value[field].as_array_mut().ok_or_else(|| Error::Decode {
            entity: "projection",
            detail: format!("missing {field}"),
        })?;
        if let Some(index) = records.iter().position(|record| record["id"] == next["id"]) {
            let keys: &[&str] = match field {
                "runs" => &["delegatedCompletion", "restartCancelledBackgroundWork"],
                "subagents" => &["completionDelivery"],
                "providerTurns" => &["tokenUsage"],
                _ => &[],
            };
            for key in keys {
                if next.get(*key).is_none()
                    && let Some(current) = records[index].get(*key)
                {
                    next[*key] = current.clone();
                }
            }
            records[index] = next;
        } else {
            records.push(next);
        }
    }
    let mut result: crate::api::OrchestrationV2ThreadProjection = serde_json::from_value(value)?;
    if matches!(
        event,
        RunCreated(_)
            | RunUpdated(_)
            | RunAttemptCreated(_)
            | RunAttemptUpdated(_)
            | TurnItemUpdated(_)
    ) {
        let runs = result
            .runs
            .iter()
            .map(t3_contracts::stored::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let attempts = result
            .attempts
            .iter()
            .map(t3_contracts::stored::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let items = result
            .turn_items
            .iter()
            .map(t3_contracts::stored::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        result.visible_turn_items.clear();
        for (item, raw) in result.turn_items.iter().zip(&items) {
            if super::read::visible(raw, &runs, &attempts, &items, false) {
                let mut row = super::read::item_row(
                    item.clone(),
                    crate::api::ProjectedTurnItemVisibility::Local,
                )?;
                row.position = result.visible_turn_items.len() as u64;
                result.visible_turn_items.push(row);
            }
        }
    }
    Ok(result)
}

fn order_discriminator(payload: &mut Value, tag: &str, before: &[&str]) {
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let mut ordered = serde_json::Map::new();
    for key in before {
        if let Some(value) = object.shift_remove(*key) {
            ordered.insert((*key).into(), value);
        }
    }
    if let Some(value) = object.shift_remove(tag) {
        ordered.insert(tag.into(), value);
    }
    ordered.extend(std::mem::take(object));
    *object = ordered;
}
