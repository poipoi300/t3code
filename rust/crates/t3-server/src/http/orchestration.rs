//! Orchestration HTTP reads (`apps/server/src/orchestration-v2/http.ts`): the shell snapshot and
//! the thread snapshot, bounded snapshot and history page. Order on every route: the protocol
//! header gate, authentication with `orchestration:read`, then the scope-checked read.
//! `GET /api/orchestration/threads/{id}/transcript` has no Rust service yet and is not mounted.
use super::{auth::split, error::ApiError};
use crate::{adapters::orchestration, services::ServiceRegistry};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    base_schemas::ThreadId,
    environment_http::{self as h, EnvironmentInternalErrorReason as Internal},
};
use t3_orchestration::ports::transport::http::{
    BoundedSnapshotQuery, HistoryPageQuery, HttpReadError, ShellSnapshotQuery,
};

pub fn routes(router: Router<Arc<ServiceRegistry>>) -> Router<Arc<ServiceRegistry>> {
    router
        .route("/api/orchestration/shell", get(shell))
        .route("/api/orchestration/threads/{thread_id}", get(thread))
        .route(
            "/api/orchestration/threads/{thread_id}/bounded",
            get(bounded),
        )
        .route(
            "/api/orchestration/threads/{thread_id}/history",
            get(history),
        )
}

/// First occurrence wins, as URLSearchParams.get does.
fn query(request: &axum::extract::Request) -> HashMap<String, String> {
    let mut pairs = HashMap::new();
    for (key, value) in
        url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
    {
        pairs
            .entry(key.into_owned())
            .or_insert_with(|| value.into_owned());
    }
    pairs
}

/// The gate, authentication and the adapter, in that order; `Err` is the finished response.
async fn guard(
    services: &ServiceRegistry,
    remote: SocketAddr,
    request: axum::extract::Request,
    internal: Internal,
) -> Result<
    (
        Arc<orchestration::OrchestrationAdapter>,
        Vec<AuthEnvironmentScope>,
        HashMap<String, String>,
    ),
    Response,
> {
    let protocol = request
        .headers()
        .get(orchestration::PROTOCOL_HEADER)
        .and_then(|value| value.to_str().ok());
    if !orchestration::protocol_header_accepted(protocol) {
        return Err((
            StatusCode::UPGRADE_REQUIRED,
            Json(orchestration::protocol_mismatch_body()),
        )
            .into_response());
    }
    let query = query(&request);
    let (req, _, _) = split(remote, request);
    let session = services
        .local
        .auth
        .require(&req, AuthEnvironmentScope::OrchestrationRead)
        .await
        .map_err(|error| ApiError::from_auth(error, internal).into_response())?;
    let adapter = services
        .orchestration()
        .ok_or_else(|| ApiError::internal(internal).into_response())?;
    Ok((adapter, session.scopes, query))
}

fn thread_id(value: String) -> Result<ThreadId, Response> {
    serde_json::from_value(serde_json::Value::String(value))
        .map_err(|_| not_found(h::EnvironmentResourceNotFoundReason::ThreadNotFound))
}

fn not_found(reason: h::EnvironmentResourceNotFoundReason) -> Response {
    ApiError::json(
        StatusCode::NOT_FOUND,
        h::EnvironmentResourceNotFoundError {
            tag: h::EnvironmentResourceNotFoundErrorTag::EnvironmentResourceNotFoundError,
            code: h::EnvironmentResourceNotFoundErrorCode::NotFound,
            reason,
            trace_id: super::error::trace_id(),
        },
    )
    .into_response()
}

fn answer<T: serde::Serialize>(result: Result<T, HttpReadError>, internal: Internal) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(HttpReadError::Invalid(reason)) => ApiError::invalid_request(reason).into_response(),
        Err(HttpReadError::NotFound(reason)) => not_found(reason),
        Err(HttpReadError::Internal { reason, source }) => {
            tracing::warn!(error = %source, "orchestration read failed");
            ApiError::internal(reason).into_response()
        }
        // `guard` required the scope already; a refusal here is a server fault, not a grant.
        Err(HttpReadError::ScopeNotGranted) => ApiError::internal(internal).into_response(),
    }
}

async fn shell(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let internal = Internal::OrchestrationSnapshotFailed;
    let (adapter, scopes, query) = match guard(&s, remote, request, internal).await {
        Ok(guarded) => guarded,
        Err(response) => return response,
    };
    let read = adapter.http_read();
    let result = read
        .shell_snapshot(
            &scopes,
            ShellSnapshotQuery {
                omit_idle_subagents: query.get("omitIdleSubagents").map(String::as_str),
                omit_older_settled: query.get("omitOlderSettled").map(String::as_str),
                resume_from: query.get("resumeFrom").map(String::as_str),
            },
        )
        .await;
    answer(result, internal)
}

async fn thread(
    State(s): State<Arc<ServiceRegistry>>,
    Path(id): Path<String>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let internal = Internal::OrchestrationThreadSnapshotFailed;
    let (adapter, scopes, _) = match guard(&s, remote, request, internal).await {
        Ok(guarded) => guarded,
        Err(response) => return response,
    };
    let id = match thread_id(id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    answer(
        adapter.http_read().thread_snapshot(&scopes, id).await,
        internal,
    )
}

async fn bounded(
    State(s): State<Arc<ServiceRegistry>>,
    Path(id): Path<String>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let internal = Internal::OrchestrationThreadHistoryFailed;
    let (adapter, scopes, query) = match guard(&s, remote, request, internal).await {
        Ok(guarded) => guarded,
        Err(response) => return response,
    };
    let id = match thread_id(id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let flag = |name: &str| query.get(name).map(String::as_str);
    let result = adapter
        .http_read()
        .bounded_thread_snapshot(
            &scopes,
            id,
            BoundedSnapshotQuery {
                defer_work: flag("deferWork"),
                compact_turn_items: flag("compactTurnItems"),
                dedupe_bodies: flag("dedupeBodies"),
                compact_ids: flag("compactIds"),
            },
        )
        .await;
    answer(result, internal)
}

async fn history(
    State(s): State<Arc<ServiceRegistry>>,
    Path(id): Path<String>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let internal = Internal::OrchestrationThreadHistoryFailed;
    let (adapter, scopes, query) = match guard(&s, remote, request, internal).await {
        Ok(guarded) => guarded,
        Err(response) => return response,
    };
    let id = match thread_id(id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(cursor) = query.get("cursor") else {
        return ApiError::invalid_request(h::EnvironmentRequestInvalidReason::InvalidHistoryCursor)
            .into_response();
    };
    let flag = |name: &str| query.get(name).map(String::as_str);
    let result = adapter
        .http_read()
        .history_page(
            &scopes,
            id,
            HistoryPageQuery {
                cursor,
                through_entry_id: flag("throughEntryId"),
                view: flag("view"),
                defer_work: flag("deferWork"),
                compact_ids: flag("compactIds"),
            },
        )
        .await;
    answer(result, internal)
}
