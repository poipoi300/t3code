use crate::{
    api::*,
    error::Result,
    projections::read::{id_text, integer, one, require_thread, rows},
};
use serde_json::Value;
use t3_persistence::rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRunMetadata {
    pub id: RunId,
    pub ordinal: u64,
    pub status: OrchestrationV2RunStatus,
}
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointScopeMetadata {
    pub id: CheckpointScopeId,
    pub run_id: Option<RunId>,
    pub kind: CheckpointScopeKind,
    pub cwd: TrimmedNonEmptyString,
}
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointMetadata {
    pub scope_id: CheckpointScopeId,
    pub run_id: Option<RunId>,
    pub ordinal_within_scope: u64,
    pub app_run_ordinal: Option<u64>,
    pub status: CheckpointStatus,
    #[serde(rename = "ref")]
    pub reference: CheckpointRef,
}
#[derive(Debug, Clone)]
pub struct CheckpointContext {
    pub runs: Vec<CheckpointRunMetadata>,
    pub checkpoint_scopes: Vec<CheckpointScopeMetadata>,
    pub checkpoints: Vec<CheckpointMetadata>,
}
pub fn checkpoint_context(db: &Connection, id: &ThreadId) -> Result<CheckpointContext> {
    super::read::ensure_thread(db, id)?;
    Ok(CheckpointContext {
        runs: rows(
            db,
            "SELECT json_object('id',run_id,'ordinal',ordinal,'status',status) FROM orchestration_v2_projection_runs WHERE thread_id=?1 ORDER BY ordinal",
            [&id.0],
        )?,
        checkpoint_scopes: rows(
            db,
            "SELECT json_object('id',scope_id,'runId',run_id,'kind',kind,'cwd',json_extract(payload_json,'$.cwd')) FROM orchestration_v2_projection_checkpoint_scopes WHERE thread_id=?1 ORDER BY ordinal_within_parent,scope_id",
            [&id.0],
        )?,
        checkpoints: rows(
            db,
            "SELECT json_object('scopeId',scope_id,'runId',run_id,'ordinalWithinScope',ordinal_within_scope,'appRunOrdinal',app_run_ordinal,'status',status,'ref',json_extract(payload_json,'$.ref')) FROM orchestration_v2_projection_checkpoints WHERE thread_id=?1 ORDER BY scope_id,ordinal_within_scope",
            [&id.0],
        )?,
    })
}
pub fn runtime_response_context(
    db: &Connection,
    id: &ThreadId,
    request_id: &RuntimeRequestId,
) -> Result<Option<ControlContext>> {
    let thread = require_thread(db, id)?;
    let owned:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_runtime_requests WHERE thread_id=?1 AND runtime_request_id=?2)",params![id.0,request_id.0],|row|row.get(0))?;
    if owned {
        return control(db, &ControlQuery::RuntimeResponse(request_id.clone()));
    }
    Ok(Some(ControlContext {
        thread,
        run: None,
        attempt: None,
        node: None,
        provider_session: None,
        provider_thread: None,
        provider_turn: None,
        request: None,
        message: None,
        item: None,
    }))
}

pub fn provider_target(
    db: &Connection,
    query: &ProviderTargetQuery,
) -> Result<Option<ControlContext>> {
    let thread = require_thread(db, &query.thread_id)?;
    let provider_thread = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id=?2 AND (thread_id=?1 OR owner_node_id IN(SELECT node_id FROM orchestration_v2_projection_nodes WHERE thread_id=?1) OR provider_thread_id IN(SELECT provider_thread_id FROM orchestration_v2_projection_subagents WHERE thread_id=?1))",
        params![query.thread_id.0, query.provider_thread_id.0],
    )?;
    let provider_turn: Option<OrchestrationV2ProviderTurn> = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_provider_turns WHERE thread_id=?1 AND provider_turn_id=?2",
        params![query.thread_id.0, query.provider_turn_id.0],
    )?;
    let attempt = if let Some(id) = &query.attempt_id {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_run_attempts WHERE thread_id=?1 AND attempt_id=?2",
            params![query.thread_id.0, id.0],
        )?
    } else {
        None
    };
    let message = if let Some(id) = &query.message_id {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_messages WHERE thread_id=?1 AND message_id=?2",
            params![query.thread_id.0, id.0],
        )?
    } else {
        None
    };
    let run = if query.message_id.is_some()
        && let Some(turn) = &provider_turn
    {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND json_extract(payload_json,'$.activeAttemptId') IS ?2 ORDER BY ordinal ASC LIMIT 1",
            params![
                query.thread_id.0,
                turn.run_attempt_id.as_ref().map(id_text).transpose()?
            ],
        )?
    } else {
        None
    };
    Ok(Some(ControlContext {
        thread,
        run,
        attempt,
        provider_thread,
        provider_turn,
        message,
        node: None,
        provider_session: None,
        request: None,
        item: None,
    }))
}

pub fn control(db: &Connection, query: &ControlQuery) -> Result<Option<ControlContext>> {
    let thread_id=match query {
        ControlQuery::TurnStart{thread_id,..}|ControlQuery::RunningTurn(thread_id)|ControlQuery::Checkpoint(thread_id)|ControlQuery::Settlement(thread_id)|ControlQuery::Recovery(thread_id)=>Some(thread_id.clone()),
        ControlQuery::RuntimeResponse(id)=>db.query_row("SELECT thread_id FROM orchestration_v2_projection_runtime_requests WHERE runtime_request_id=?1",[&id.0],|row|row.get::<_,String>(0)).optional()?.map(ThreadId),
        ControlQuery::ProviderThread(id)=>db.query_row("SELECT COALESCE(thread_id,(SELECT thread_id FROM orchestration_v2_projection_nodes WHERE node_id=owner_node_id),(SELECT thread_id FROM orchestration_v2_projection_subagents AS subagent WHERE subagent.provider_thread_id=provider.provider_thread_id ORDER BY updated_at DESC,subagent_id ASC LIMIT 1)) FROM orchestration_v2_projection_provider_threads AS provider WHERE provider_thread_id=?1",[&id.0],|row|row.get::<_,Option<String>>(0)).optional()?.flatten().map(ThreadId),
    };
    let Some(id) = thread_id else {
        return Ok(None);
    };
    let Some(thread) = super::read::thread(db, &id)? else {
        return Ok(None);
    };
    let run: Option<OrchestrationV2Run> = match query {
        ControlQuery::TurnStart { run_id, .. } => one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2",
            params![id.0, run_id.0],
        )?,
        ControlQuery::RunningTurn(_) => one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND status='running' ORDER BY ordinal ASC LIMIT 1",
            [&id.0],
        )?,
        ControlQuery::ProviderThread(provider_id) => one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND provider_thread_id=?2 ORDER BY ordinal DESC LIMIT 1",
            params![id.0, provider_id.0],
        )?,
        ControlQuery::RuntimeResponse(_) => None,
        _ => one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND (status<>'cancelled' OR json_extract(payload_json,'$.startedAt') IS NOT NULL) ORDER BY ordinal DESC LIMIT 1",
            [&id.0],
        )?,
    };
    let raw = run
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    let attempt = if matches!(
        query,
        ControlQuery::TurnStart { .. } | ControlQuery::ProviderThread(_)
    ) {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_run_attempts WHERE thread_id=?1 AND attempt_id=?2",
            params![id.0, raw["activeAttemptId"].as_str()],
        )?
    } else {
        None
    };
    let provider_id = match query {
        ControlQuery::ProviderThread(provider_id) => Some(provider_id.0.as_str()),
        _ => raw["providerThreadId"].as_str(),
    };
    let provider_thread: Option<OrchestrationV2ProviderThread> = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id=?1",
        [provider_id],
    )?;
    let provider_turn = if matches!(
        query,
        ControlQuery::TurnStart { .. }
            | ControlQuery::RunningTurn(_)
            | ControlQuery::ProviderThread(_)
    ) {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_provider_turns WHERE provider_thread_id=?1 AND run_attempt_id IS ?2 AND node_id=?3 AND (?4=0 OR status='running') ORDER BY ordinal DESC LIMIT 1",
            params![
                provider_id,
                raw["activeAttemptId"].as_str(),
                raw["rootNodeId"].as_str(),
                matches!(query, ControlQuery::RunningTurn(_))
            ],
        )?
    } else {
        None
    };
    let request: Option<OrchestrationV2RuntimeRequest> = match query {
        ControlQuery::RuntimeResponse(request_id) => one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_runtime_requests WHERE thread_id=?1 AND runtime_request_id=?2",
            params![id.0, request_id.0],
        )?,
        _ => None,
    };
    let request_value = request
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    let node_id = if matches!(query, ControlQuery::RuntimeResponse(_)) {
        request_value["nodeId"].as_str()
    } else {
        raw["rootNodeId"].as_str()
    };
    let node = if matches!(
        query,
        ControlQuery::RuntimeResponse(_) | ControlQuery::TurnStart { .. }
    ) {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_nodes WHERE thread_id=?1 AND node_id=?2",
            params![id.0, node_id],
        )?
    } else {
        None
    };
    let item = if let ControlQuery::RuntimeResponse(request_id) = query {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE thread_id=?1 AND node_id=?2 AND type IN ('approval_request','user_input_request') AND json_extract(payload_json,'$.requestId')=?3 ORDER BY ordinal ASC LIMIT 1",
            params![id.0, node_id, request_id.0],
        )?
    } else {
        None
    };
    let provider_session_id = provider_thread
        .as_ref()
        .and_then(|thread| thread.provider_session_id.as_ref())
        .map(id_text)
        .transpose()?;
    let session_id = if matches!(query, ControlQuery::RuntimeResponse(_)) {
        (request_value["responseCapability"]["type"] == "live")
            .then(|| request_value["responseCapability"]["providerSessionId"].as_str())
            .flatten()
    } else {
        provider_session_id
    };
    let provider_session = if matches!(
        query,
        ControlQuery::RuntimeResponse(_) | ControlQuery::TurnStart { .. }
    ) {
        one(
            db,
            "SELECT session.payload_json FROM orchestration_v2_projection_provider_sessions AS session JOIN orchestration_v2_projection_provider_session_bindings AS binding ON binding.provider_session_id=session.provider_session_id WHERE binding.thread_id=?1 AND session.provider_session_id=?2",
            params![id.0, session_id],
        )?
    } else {
        None
    };
    let message = if matches!(
        query,
        ControlQuery::TurnStart { .. } | ControlQuery::ProviderThread(_)
    ) {
        one(
            db,
            "SELECT payload_json FROM orchestration_v2_projection_messages WHERE thread_id=?1 AND message_id=?2",
            params![id.0, raw["userMessageId"].as_str()],
        )?
    } else {
        None
    };
    Ok(Some(ControlContext {
        thread,
        run,
        attempt,
        node,
        provider_session,
        provider_thread,
        provider_turn,
        request,
        message,
        item,
    }))
}

pub fn checkpoint_capture(
    db: &Connection,
    id: &ThreadId,
    run_id: &RunId,
    scope_id: &CheckpointScopeId,
) -> Result<CheckpointCaptureContext> {
    let thread = require_thread(db, id)?;
    let raw_thread = serde_json::to_value(thread)?;
    let run: Option<OrchestrationV2Run> = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2",
        params![id.0, run_id.0],
    )?;
    let raw = run
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    let root_node = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_nodes WHERE thread_id=?1 AND node_id=?2",
        params![id.0, raw["rootNodeId"].as_str()],
    )?;
    let scope = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_checkpoint_scopes WHERE thread_id=?1 AND scope_id=?2",
        params![id.0, scope_id.0],
    )?;
    let provider_thread = owned_provider_thread(db, id, raw["providerThreadId"].as_str())?;
    let mut statement=db.prepare("SELECT ordinal_within_scope FROM orchestration_v2_projection_checkpoints WHERE thread_id=?1 AND scope_id=?2 AND (status='ready' OR (status='missing' AND run_id IS NOT NULL)) ORDER BY ordinal_within_scope ASC")?;
    let preserved_ordinals = statement
        .query_map(params![id.0, scope_id.0], |row| row.get::<_, i64>(0))?
        .map(|result| result.map(|ordinal| ordinal as u64))
        .collect::<std::result::Result<_, _>>()?;
    Ok(CheckpointCaptureContext {
        run,
        root_node,
        scope,
        provider_thread,
        preserved_ordinals,
        side_chat: raw_thread["lineage"]["sideChat"] == true,
    })
}
fn owned_provider_thread(
    db: &Connection,
    id: &ThreadId,
    provider_id: Option<&str>,
) -> Result<Option<OrchestrationV2ProviderThread>> {
    one(
        db,
        &format!(
            "SELECT payload_json FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id=?2 AND (thread_id=?1 OR owner_node_id IN ({}) OR provider_thread_id IN (SELECT provider_thread_id FROM orchestration_v2_projection_subagents WHERE thread_id=?1))",
            super::read::OWNED_PROVIDER_NODES
        ),
        params![id.0, provider_id],
    )
}
pub fn checkpoint_rollback(
    db: &Connection,
    id: &ThreadId,
    provider_id: &ProviderThreadId,
    checkpoint_id: &CheckpointId,
    scope_id: &CheckpointScopeId,
) -> Result<CheckpointRollbackContext> {
    let thread = require_thread(db, id)?;
    let provider_thread = owned_provider_thread(db, id, Some(&provider_id.0))?;
    let checkpoint = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_checkpoints WHERE thread_id=?1 AND checkpoint_id=?2",
        params![id.0, checkpoint_id.0],
    )?;
    let scope = one(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_checkpoint_scopes WHERE thread_id=?1 AND scope_id=?2",
        params![id.0, scope_id.0],
    )?;
    let mut context = CheckpointRollbackContext {
        thread,
        provider_thread,
        checkpoint,
        scope,
        session: None,
        target_attempt: None,
        provider_turns: vec![],
        rolled_back_attempt_ids: vec![],
        runs_to_rollback: vec![],
        root_nodes: vec![],
        stale_checkpoints: vec![],
    };
    let (Some(provider), Some(checkpoint), Some(scope)) = (
        &context.provider_thread,
        &context.checkpoint,
        &context.scope,
    ) else {
        return Ok(context);
    };
    if provider.provider_session_id.is_none()
        || checkpoint.scope_id != scope.id
        || !matches!(
            checkpoint.status,
            CheckpointStatus::Ready | CheckpointStatus::Missing
        )
        || Some(&provider.id) != context.thread.active_provider_thread_id.as_ref()
        || id_text(&provider.provider_instance_id)? != context.thread.model_selection.instance_id.0
    {
        return Ok(context);
    }
    let ordinal = integer(checkpoint.app_run_ordinal.unwrap_or(0))?;
    context.target_attempt = one(
        db,
        "SELECT attempt.payload_json FROM orchestration_v2_projection_run_attempts AS attempt JOIN orchestration_v2_projection_runs AS run ON attempt.attempt_id=json_extract(run.payload_json,'$.activeAttemptId') WHERE run.thread_id=?1 AND run.ordinal=?2 AND run.status<>'rolled_back' AND attempt.thread_id=?1 LIMIT 1",
        params![id.0, ordinal],
    )?;
    context.runs_to_rollback = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND ordinal>?2 AND status IN ('completed','interrupted','failed','cancelled') ORDER BY ordinal ASC",
        params![id.0, ordinal],
    )?;
    context.session = one(
        db,
        "SELECT session.payload_json FROM orchestration_v2_projection_provider_sessions AS session JOIN orchestration_v2_projection_provider_session_bindings AS binding ON binding.provider_session_id=session.provider_session_id WHERE binding.thread_id=?1 AND session.provider_session_id=?2",
        params![
            id.0,
            provider
                .provider_session_id
                .as_ref()
                .map(id_text)
                .transpose()?
        ],
    )?;
    context.rolled_back_attempt_ids=rows::<OrchestrationV2RunAttempt>(db,"SELECT attempt.payload_json FROM orchestration_v2_projection_run_attempts AS attempt JOIN orchestration_v2_projection_runs AS run ON run.run_id=attempt.run_id WHERE run.thread_id=?1 AND run.status='rolled_back' AND attempt.thread_id=?1 ORDER BY run.ordinal,attempt.attempt_ordinal",[&id.0])?.into_iter().map(|attempt|Ok(RunAttemptId(id_text(&attempt.id)?.to_owned()))).collect::<Result<_>>()?;
    context.stale_checkpoints = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_checkpoints WHERE thread_id=?1 AND scope_id=?2 AND status='ready' AND app_run_ordinal>?3 ORDER BY ordinal_within_scope ASC",
        params![id.0, id_text(&scope.id)?, ordinal],
    )?;
    let nodes: Vec<_> = context
        .runs_to_rollback
        .iter()
        .map(|run| &run.root_node_id)
        .collect();
    context.root_nodes = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_nodes WHERE thread_id=?1 AND node_id IN (SELECT value FROM json_each(?2)) ORDER BY node_id ASC",
        params![id.0, serde_json::to_string(&nodes)?],
    )?;
    let attempt = context.target_attempt.as_ref();
    context.provider_turns = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_provider_turns WHERE thread_id=?1 AND (provider_thread_id=?2 OR provider_turn_id=?3 OR run_attempt_id=?4) ORDER BY provider_thread_id ASC,ordinal ASC",
        params![
            id.0,
            provider_id.0,
            attempt
                .and_then(|attempt| attempt.provider_turn_id.as_ref())
                .map(id_text)
                .transpose()?,
            attempt.map(|attempt| id_text(&attempt.id)).transpose()?
        ],
    )?;
    Ok(context)
}

#[derive(Debug, Clone)]
pub struct TurnStartContext {
    pub projection: OrchestrationV2ThreadProjection,
    pub has_conversation: bool,
}
pub fn turn_start_context(
    db: &Connection,
    id: &ThreadId,
    run_id: &RunId,
) -> Result<TurnStartContext> {
    let thread = require_thread(db, id)?;
    let runs = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 ORDER BY ordinal ASC",
        [&id.0],
    )?;
    let attempts = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_run_attempts WHERE thread_id=?1 ORDER BY run_id ASC,attempt_ordinal ASC",
        [&id.0],
    )?;
    let nodes: Vec<OrchestrationV2ExecutionNode> = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_nodes INDEXED BY sqlite_autoindex_orchestration_v2_projection_nodes_1 WHERE thread_id=?1 AND node_id IN (SELECT json_extract(payload_json,'$.rootNodeId') FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2) ORDER BY node_id ASC",
        params![id.0, run_id.0],
    )?;
    let subagents = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_subagents WHERE thread_id=?1 AND status NOT IN ('interrupted','failed','cancelled') ORDER BY subagent_id ASC",
        [&id.0],
    )?;
    let provider_threads = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_provider_threads WHERE thread_id=?1 ORDER BY COALESCE(first_run_ordinal,0),provider_thread_id ASC",
        [&id.0],
    )?;
    let provider_turns = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_provider_turns WHERE thread_id=?1 ORDER BY provider_thread_id ASC,ordinal ASC",
        [&id.0],
    )?;
    let scope_ids = nodes
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|value| value["checkpointScopeId"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let checkpoint_scopes = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_checkpoint_scopes WHERE thread_id=?1 AND scope_id IN (SELECT value FROM json_each(?2)) ORDER BY ordinal_within_parent ASC,scope_id ASC",
        params![id.0, serde_json::to_string(&scope_ids)?],
    )?;
    let context_handoffs: Vec<OrchestrationV2ContextHandoff> = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_context_handoffs WHERE thread_id=?1 ORDER BY rowid ASC",
        [&id.0],
    )?;
    let context_transfers: Vec<OrchestrationV2ContextTransfer> = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_context_transfers WHERE source_thread_id=?1 OR target_thread_id=?1 ORDER BY rowid ASC",
        [&id.0],
    )?;
    let turn_items = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_turn_items INDEXED BY orchestration_v2_projection_turn_items_run_ordinal_idx WHERE thread_id=?1 AND run_id=?2 ORDER BY ordinal ASC,turn_item_id ASC",
        params![id.0, run_id.0],
    )?;
    let provider_sessions = rows(
        db,
        "SELECT sessions.payload_json FROM orchestration_v2_projection_provider_sessions AS sessions JOIN orchestration_v2_projection_provider_session_bindings AS bindings ON bindings.provider_session_id=sessions.provider_session_id WHERE bindings.thread_id=?1 ORDER BY sessions.updated_at ASC,sessions.provider_session_id ASC",
        [&id.0],
    )?;
    let whitespace = " \t\n\r\u{000b}\u{000c}\u{00a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
    let messages = rows(
        db,
        "SELECT payload_json FROM orchestration_v2_projection_messages WHERE thread_id=?1 AND (message_id IN (SELECT json_extract(payload_json,'$.userMessageId') FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2) OR (role='user' AND lower(trim(json_extract(payload_json,'$.text'),?3))='/compact') OR (role='user' AND trim(json_extract(payload_json,'$.text'),?3) GLOB '/goal*')) ORDER BY created_at ASC,message_id ASC",
        params![id.0, run_id.0, whitespace],
    )?;
    let mut has_conversation:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_messages WHERE thread_id=?1 AND role='user' AND (lower(trim(json_extract(payload_json,'$.text'),?2))<>'/compact' OR json_array_length(payload_json,'$.attachments')>0))",params![id.0,whitespace],|row|row.get(0))?;
    let thread_value = serde_json::to_value(&thread)?;
    if !has_conversation && thread_value["forkedFrom"]["type"] == "run" {
        let transfers = context_transfers
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let resolution = transfers
            .iter()
            .find(|transfer| {
                transfer["type"] == "fork" && transfer["targetThreadId"] == thread_value["id"]
            })
            .map(|transfer| &transfer["resolution"]);
        has_conversation = if resolution
            .is_none_or(|resolution| resolution["strategy"] != "portable_context")
        {
            true
        } else {
            let handoffs = context_handoffs
                .iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            handoffs.iter().any(|handoff| {
                resolution.is_some_and(|resolution| handoff["id"] == resolution["contextHandoffId"])
                    && handoff["history"]["messages"]
                        .as_array()
                        .is_some_and(|messages| !messages.is_empty())
            })
        };
    }
    Ok(TurnStartContext {
        has_conversation,
        projection: OrchestrationV2ThreadProjection {
            updated_at: thread.updated_at.clone(),
            thread,
            runs,
            attempts,
            nodes,
            subagents,
            provider_sessions,
            provider_threads,
            provider_turns,
            runtime_requests: vec![],
            messages,
            plans: vec![],
            turn_items,
            checkpoint_scopes,
            checkpoints: vec![],
            context_handoffs,
            context_transfers,
            visible_turn_items: vec![],
        },
    })
}

pub fn runtime_recovery(db: &Connection, id: &ThreadId) -> Result<OrchestrationV2ThreadProjection> {
    let thread = require_thread(db, id)?;
    let runs = rows(
        db,
        r#"SELECT payload_json FROM orchestration_v2_projection_runs AS run
              WHERE run.thread_id = ?1
                AND (
                  run.status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                  OR run.run_id = (
                    SELECT latest.run_id FROM orchestration_v2_projection_runs AS latest
                    WHERE latest.thread_id = ?1
                    ORDER BY latest.ordinal DESC LIMIT 1
                  )
                  OR run.run_id IN (
                    SELECT (
                      SELECT ended.run_id FROM orchestration_v2_projection_runs AS ended
                      WHERE ended.thread_id = ?1
                        AND ended.provider_thread_id = roster.provider_thread_id
                        AND ended.status NOT IN ('queued', 'rolled_back')
                        AND NOT (
                          json_type(ended.payload_json, '$.startedAt') = 'null'
                          AND json_type(ended.payload_json, '$.restartContinuationOfRunId') IS NULL
                        )
                      ORDER BY ended.completed_at IS NULL DESC, ended.completed_at DESC,
                        ended.ordinal DESC
                      LIMIT 1
                    )
                    FROM orchestration_v2_projection_provider_threads AS roster
                    WHERE roster.thread_id = ?1
                      AND CASE WHEN json_valid(roster.payload_json)
                        THEN json_array_length(roster.payload_json, '$.pendingBackgroundTasks') > 0
                        ELSE 0 END
                  )
                  OR run.run_id IN (
                    SELECT item.run_id FROM orchestration_v2_projection_turn_items AS item
                    WHERE item.thread_id = ?1
                      AND item.type IN ('command_execution', 'dynamic_tool', 'subagent')
                      AND item.status IN ('pending', 'running', 'waiting')
                      AND item.run_id IS NOT NULL
                  )
                )
              ORDER BY run.ordinal ASC"#,
        [&id.0],
    )?;
    let attempts = rows(
        db,
        r#"SELECT attempt.payload_json
              FROM orchestration_v2_projection_run_attempts AS attempt
              WHERE attempt.thread_id = ?1
                AND attempt.run_id IN (
                  SELECT run_id FROM orchestration_v2_projection_runs
                  WHERE thread_id = ?1
                    AND status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                )
              ORDER BY attempt.run_id ASC, attempt.attempt_ordinal ASC"#,
        [&id.0],
    )?;
    let nodes = rows(
        db,
        r#"SELECT node.payload_json FROM orchestration_v2_projection_nodes AS node
                INDEXED BY orchestration_v2_projection_nodes_live_idx
              WHERE node.thread_id = ?1
                AND node.status IN ('pending', 'starting', 'running', 'waiting')
                AND (
                  (node.run_id IS NULL AND node.kind = 'root_turn')
                  OR node.node_id IN (
                    SELECT node_id FROM orchestration_v2_projection_runtime_requests
                    WHERE thread_id = ?1 AND status = 'pending' AND kind = 'user_input'
                  )
                  OR node.run_id IN (
                    SELECT run_id FROM orchestration_v2_projection_runs
                    WHERE thread_id = ?1
                      AND status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                  )
                  OR node.node_id IN (
                    SELECT item.node_id FROM orchestration_v2_projection_turn_items AS item
                    WHERE item.thread_id = ?1
                      AND item.type IN ('command_execution', 'dynamic_tool', 'subagent')
                      AND item.status IN ('pending', 'running', 'waiting')
                  )
                  OR node.node_id IN (
                    SELECT json_extract(item.payload_json, '$.subagentId')
                    FROM orchestration_v2_projection_turn_items AS item
                    WHERE item.thread_id = ?1 AND item.type = 'subagent'
                      AND item.status IN ('pending', 'running', 'waiting')
                  )
                  OR node.node_id IN (
                    SELECT subagent_id FROM orchestration_v2_projection_subagents
                    WHERE thread_id = ?1 AND origin = 'provider_native'
                      AND status IN ('pending', 'running', 'waiting')
                  )
                )
              ORDER BY COALESCE(node.started_at, ''), node.node_id ASC"#,
        [&id.0],
    )?;
    let subagents = rows(
        db,
        r#"SELECT subagent.payload_json FROM orchestration_v2_projection_subagents AS subagent
              WHERE subagent.thread_id = ?1
                AND subagent.status IN ('pending', 'starting', 'running', 'waiting')
                AND (
                  subagent.origin = 'provider_native'
                  OR subagent.run_id IN (
                    SELECT run_id FROM orchestration_v2_projection_runs
                    WHERE thread_id = ?1
                      AND status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                  )
                  OR subagent.subagent_id IN (
                    SELECT json_extract(item.payload_json, '$.subagentId')
                    FROM orchestration_v2_projection_turn_items AS item
                    WHERE item.thread_id = ?1 AND item.type = 'subagent'
                      AND item.status IN ('pending', 'running', 'waiting')
                  )
                )
              ORDER BY COALESCE(subagent.started_at, ''), subagent.subagent_id ASC"#,
        [&id.0],
    )?;
    let provider_sessions = rows(
        db,
        r#"SELECT session.payload_json
              FROM orchestration_v2_projection_provider_sessions AS session
              WHERE (
                  session.provider_session_id IN (
                    SELECT binding.provider_session_id
                    FROM orchestration_v2_projection_provider_session_bindings AS binding
                    WHERE binding.thread_id = ?1
                  )
                  AND (
                    session.status NOT IN ('stopped', 'error')
                    OR (session.status = 'error' AND CASE WHEN json_valid(session.payload_json)
                      THEN json_array_length(session.payload_json, '$.extensionStatuses') > 0
                      ELSE 0 END)
                    OR session.provider_session_id IN (
                      SELECT provider_thread.provider_session_id
                      FROM orchestration_v2_projection_provider_threads AS provider_thread
                      WHERE provider_thread.provider_thread_id IN (
                        SELECT run.provider_thread_id
                        FROM orchestration_v2_projection_runs AS run
                        WHERE run.thread_id = ?1
                          AND run.status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                      )
                    )
                  )
                )
                OR (
                  session.thread_id = ?1
                  AND session.status NOT IN ('stopped', 'error')
                  AND NOT EXISTS (
                    SELECT 1 FROM orchestration_v2_projection_provider_session_bindings AS binding
                    WHERE binding.provider_session_id = session.provider_session_id
                  )
                )
              ORDER BY session.updated_at ASC, session.provider_session_id ASC"#,
        [&id.0],
    )?;
    let provider_threads = rows(
        db,
        r#"SELECT provider_thread.payload_json
              FROM orchestration_v2_projection_provider_threads AS provider_thread
              WHERE provider_thread.provider_thread_id IN (
                  SELECT provider_thread_id FROM orchestration_v2_projection_provider_threads
                  WHERE thread_id = ?1
                  UNION
                  SELECT owned.provider_thread_id
                  FROM orchestration_v2_projection_provider_threads AS owned
                    INDEXED BY orchestration_v2_projection_provider_threads_owner_idx
                  CROSS JOIN orchestration_v2_projection_nodes AS owner
                    ON owner.node_id = owned.owner_node_id
                  WHERE owned.owner_node_id IS NOT NULL AND owner.thread_id = ?1
                  UNION
                  SELECT provider_thread_id FROM orchestration_v2_projection_subagents
                  WHERE thread_id = ?1 AND provider_thread_id IS NOT NULL
                    AND origin = 'provider_native'
                )
                AND (
                  provider_thread.status = 'active'
                  OR CASE WHEN json_valid(provider_thread.payload_json)
                    THEN json_array_length(provider_thread.payload_json, '$.pendingBackgroundTasks') > 0
                    ELSE 0 END
                  OR provider_thread.provider_thread_id IN (
                    SELECT run.provider_thread_id FROM orchestration_v2_projection_runs AS run
                    WHERE run.thread_id = ?1
                      AND run.status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                  )
                  OR provider_thread.provider_thread_id IN (
                    SELECT item.provider_thread_id
                    FROM orchestration_v2_projection_turn_items AS item
                    WHERE item.thread_id = ?1
                      AND item.status IN ('pending', 'running', 'waiting')
                  )
                  OR provider_thread.provider_thread_id IN (
                    SELECT run.provider_thread_id
                    FROM orchestration_v2_projection_turn_items AS item
                    JOIN orchestration_v2_projection_runs AS run ON run.run_id = item.run_id
                    WHERE item.thread_id = ?1
                      AND item.status IN ('pending', 'running', 'waiting')
                  )
                )
              ORDER BY COALESCE(provider_thread.first_run_ordinal, 0), provider_thread.provider_thread_id ASC"#,
        [&id.0],
    )?;
    let provider_turns = rows(
        db,
        r#"SELECT provider_turn.payload_json
              FROM orchestration_v2_projection_provider_turns AS provider_turn
              WHERE provider_turn.thread_id = ?1
                AND provider_turn.status IN ('pending', 'starting', 'running', 'waiting')
                AND (
                  provider_turn.run_attempt_id IN (
                    SELECT attempt_id FROM orchestration_v2_projection_run_attempts
                    WHERE thread_id = ?1
                      AND run_id IN (
                        SELECT run_id FROM orchestration_v2_projection_runs
                        WHERE thread_id = ?1
                          AND status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                      )
                  )
                  OR (
                    provider_turn.run_attempt_id IS NULL
                    AND provider_turn.node_id IN (
                      SELECT node_id FROM orchestration_v2_projection_nodes
                      WHERE thread_id = ?1
                        AND run_id IS NULL
                        AND kind = 'root_turn'
                        AND status IN ('pending', 'running', 'waiting')
                    )
                  )
                )
              ORDER BY provider_turn.provider_thread_id ASC, provider_turn.ordinal ASC"#,
        [&id.0],
    )?;
    let runtime_requests = rows(
        db,
        r#"SELECT payload_json FROM orchestration_v2_projection_runtime_requests
              WHERE thread_id = ?1 AND status = 'pending'
              ORDER BY created_at ASC, runtime_request_id ASC"#,
        [&id.0],
    )?;
    let messages = rows(
        db,
        r#"SELECT message.payload_json FROM orchestration_v2_projection_runs AS message_run
              CROSS JOIN orchestration_v2_projection_messages AS message
                INDEXED BY orchestration_v2_projection_messages_run_idx ON message.run_id = message_run.run_id
              WHERE message_run.thread_id = ?1
                AND message_run.status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                AND message.thread_id = ?1 AND message.streaming = 1
              ORDER BY message.created_at ASC, message.message_id ASC"#,
        [&id.0],
    )?;
    let turn_items = rows(
        db,
        r#"SELECT item.payload_json FROM orchestration_v2_projection_turn_items AS item
              WHERE item.thread_id = ?1
                AND item.status IN ('pending', 'running', 'waiting')
                AND (
                  item.run_id IN (
                    SELECT run_id FROM orchestration_v2_projection_runs
                    WHERE thread_id = ?1
                      AND status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
                  )
                  OR item.type IN ('command_execution', 'dynamic_tool', 'subagent')
                  OR (
                    item.type = 'user_input_request'
                    AND json_extract(item.payload_json, '$.requestId') IN (
                      SELECT runtime_request_id FROM orchestration_v2_projection_runtime_requests
                      WHERE thread_id = ?1 AND status = 'pending' AND kind = 'user_input'
                    )
                  )
                  OR (
                    item.run_id IS NULL
                    AND item.node_id IN (
                      SELECT node_id FROM orchestration_v2_projection_nodes
                      WHERE thread_id = ?1
                        AND run_id IS NULL
                        AND kind = 'root_turn'
                        AND status IN ('pending', 'running', 'waiting')
                    )
                  )
                )
              ORDER BY item.ordinal ASC, item.turn_item_id ASC"#,
        [&id.0],
    )?;
    Ok(OrchestrationV2ThreadProjection {
        updated_at: thread.updated_at.clone(),
        thread,
        runs,
        attempts,
        nodes,
        subagents,
        provider_sessions,
        provider_threads,
        provider_turns,
        runtime_requests,
        messages,
        turn_items,
        plans: vec![],
        checkpoint_scopes: vec![],
        checkpoints: vec![],
        context_handoffs: vec![],
        context_transfers: vec![],
        visible_turn_items: vec![],
    })
}
