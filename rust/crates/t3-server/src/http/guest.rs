//! The guest cookie is accepted only by these routes. No owner authentication fallback.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router, body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures::StreamExt;
use t3_cloud::{Error, Secret, guest::GuestFailure, http::Sensitive};
use t3_contracts::{
    base_schemas::{GuestShareId, ThreadId, TurnItemId},
    guest_share::{
        GUEST_ROUTE_PREFIX, GUEST_SHARE_PARAM, GUEST_SHARE_TTL_MS, GuestRedeemRequest,
        GuestSubmitRequest,
    },
};

use crate::{adapters::guest::GuestAdapter, services::ServiceRegistry};

pub fn routes(router: Router<Arc<ServiceRegistry>>) -> Router<Arc<ServiceRegistry>> {
    router
        .route("/api/guest/redeem", post(redeem))
        .route("/api/guest/session", get(session))
        .route("/api/guest/stream", get(stream))
        .route("/api/guest/item", get(item))
        .route("/api/guest/history", get(history))
        .route("/api/guest/messages", post(submit))
        .route("/api/guest/command-output", get(command_output))
}

fn denied(reason: GuestFailure) -> Error {
    Error::Guest(reason)
}

fn error_response(error: Error) -> Response {
    use GuestFailure::*;
    let Error::Guest(reason) = error else {
        return error_response(denied(Rejected));
    };
    let (status, reason, message) = match reason {
        Invalid => (403, "invalid", "This guest link is not valid."),
        Inactive => (403, "inactive", "This guest link is no longer active."),
        Used => (409, "used", "This guest link was already used."),
        Busy => (
            409,
            "busy",
            "Wait for the owner to answer your earlier messages.",
        ),
        StalePage => (
            409,
            "stale_page",
            "This page is for an older guest link. Reload it to continue.",
        ),
        Unavailable => (403, "unavailable", "This item is not available."),
        Rejected => (409, "rejected", "The owner's server rejected this request."),
        Empty => (400, "empty", "Write a message or attach a file."),
        TooManyFiles => (400, "too_many_files", "Attach up to 4 files to a message."),
        TooLarge => (
            400,
            "too_large",
            "Attached files can total up to 10 MB per message.",
        ),
        UnsupportedFile => (
            400,
            "unsupported_file",
            "This file cannot be attached. Images must be PNG, JPEG, GIF, or WebP.",
        ),
        FilesBusy => (
            409,
            "files_busy",
            "Wait for the owner to answer your earlier messages before you attach more files.",
        ),
    };
    uncached(
        (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(serde_json::json!({"reason": reason, "message": message})),
        )
            .into_response(),
    )
}

fn uncached(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn bad_request() -> Response {
    uncached(
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "reason": "invalid", "message": "The request was not valid."
            })),
        )
            .into_response(),
    )
}

fn param(request: &Request, key: &str) -> Option<String> {
    url::form_urlencoded::parse(request.uri().query()?.as_bytes())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn cookie_name(share: &GuestShareId) -> String {
    format!("t3_guest_{}", share.0)
}

fn share_cookie(headers: &HeaderMap, share: &GuestShareId) -> Option<Secret> {
    let name = cookie_name(share);
    let token = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|cookie| cookie.to_str().ok())
        .flat_map(|cookie| cookie.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(key, value)| (key == name && !value.is_empty()).then_some(value))?;
    Some(Secret::new(token))
}

fn credential(request: &Request) -> Result<(GuestShareId, Secret), Error> {
    let share = param(request, GUEST_SHARE_PARAM)
        .map(GuestShareId)
        .ok_or(denied(GuestFailure::Invalid))?;
    let token = share_cookie(request.headers(), &share).ok_or(denied(GuestFailure::Invalid))?;
    Ok((share, token))
}

async fn authorize(
    services: &ServiceRegistry,
    credential: Result<(GuestShareId, Secret), Error>,
) -> Result<(Arc<GuestAdapter>, Secret, ThreadId), Error> {
    let (share, token) = credential?;
    let adapter =
        GuestAdapter::from_domains(&services.domains).ok_or(denied(GuestFailure::Inactive))?;
    let thread = adapter.sharing.thread_for_session(&token, &share).await?;
    Ok((adapter, token, thread))
}

async fn redeem(State(services): State<Arc<ServiceRegistry>>, request: Request) -> Response {
    let Ok(bytes) = body::to_bytes(request.into_body(), 64 * 1024).await else {
        return bad_request();
    };
    let Ok(mut input) = serde_json::from_slice::<GuestRedeemRequest>(&bytes) else {
        return bad_request();
    };
    input.credential.0 = t3_contracts::wire::trim_js(&input.credential.0).to_owned();
    input.name.0 = t3_contracts::wire::trim_js(&input.name.0).to_owned();
    if input.credential.0.is_empty() || !t3_cloud::guest::valid_guest_name(&input.name.0) {
        return bad_request();
    }
    let Some(adapter) = GuestAdapter::from_domains(&services.domains) else {
        return error_response(denied(GuestFailure::Inactive));
    };
    if services.lifecycle.admit_command().is_err() {
        return error_response(denied(GuestFailure::Rejected));
    }
    let (token, share) = match adapter.sharing.redeem(Sensitive::new(input)).await {
        Ok(value) => value,
        Err(error) => return error_response(error),
    };
    let Ok(mut cookie) = HeaderValue::from_str(&format!(
        "{}={}; Max-Age={}; Path={GUEST_ROUTE_PREFIX}; HttpOnly; Secure; SameSite=Strict",
        cookie_name(&share),
        token.expose(),
        GUEST_SHARE_TTL_MS / 1_000,
    )) else {
        return error_response(denied(GuestFailure::Rejected));
    };
    cookie.set_sensitive(true);
    let mut response = uncached(StatusCode::NO_CONTENT.into_response());
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
}

async fn session(State(services): State<Arc<ServiceRegistry>>, request: Request) -> Response {
    match authorize(&services, credential(&request)).await {
        Ok(_) => uncached(StatusCode::NO_CONTENT.into_response()),
        Err(error) => error_response(error),
    }
}

async fn stream(State(services): State<Arc<ServiceRegistry>>, request: Request) -> Response {
    let (adapter, token, _) = match authorize(&services, credential(&request)).await {
        Ok(value) => value,
        Err(error) => return error_response(error),
    };
    let events = match adapter.sharing.stream(&token).await {
        Ok(events) => events,
        Err(error) => return error_response(error),
    };
    sse(events, &services)
}

fn sse(
    events: impl futures::Stream<Item = impl serde::Serialize> + Send + 'static,
    services: &ServiceRegistry,
) -> Response {
    let data = events
        .filter_map(|event| {
            let data = Event::default()
                .json_data(event)
                .ok()
                .map(Ok::<_, Infallible>);
            futures::future::ready(data)
        })
        .take_until(services.shutdown.clone().cancelled_owned());
    let mut response = Sse::new(data)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(20))
                .text("ping"),
        )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-transform"),
    );
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

async fn item(State(services): State<Arc<ServiceRegistry>>, request: Request) -> Response {
    let Some(id) = param(&request, "item").filter(|item| !item.is_empty()) else {
        return bad_request();
    };
    let (adapter, _, thread) = match authorize(&services, credential(&request)).await {
        Ok(value) => value,
        Err(error) => return error_response(error),
    };
    match adapter.item(thread, TurnItemId(id)).await {
        Ok(item) => uncached(Json(item).into_response()),
        Err(error) => error_response(error),
    }
}

async fn history(State(services): State<Arc<ServiceRegistry>>, request: Request) -> Response {
    let Some(cursor) = param(&request, "cursor").filter(|cursor| !cursor.is_empty()) else {
        return bad_request();
    };
    let (adapter, _, thread) = match authorize(&services, credential(&request)).await {
        Ok(value) => value,
        Err(error) => return error_response(error),
    };
    match adapter.history(thread, cursor).await {
        Ok(page) => uncached(Json(page).into_response()),
        Err(error) => error_response(error),
    }
}

async fn submit(State(services): State<Arc<ServiceRegistry>>, request: Request) -> Response {
    let credential = credential(&request);
    let Some(share) = credential.as_ref().ok().map(|(share, _)| share.clone()) else {
        return error_response(denied(GuestFailure::Invalid));
    };
    let (adapter, token, _) = match authorize(&services, credential).await {
        Ok(value) => value,
        Err(error) => return error_response(error),
    };
    let Ok(bytes) = body::to_bytes(request.into_body(), 64 * 1024).await else {
        return bad_request();
    };
    let Ok(mut submission) = serde_json::from_slice::<GuestSubmitRequest>(&bytes) else {
        return bad_request();
    };
    if submission.share_id != share {
        return bad_request();
    }
    submission.text.0 = t3_contracts::wire::trim_js(&submission.text.0).to_owned();
    if submission.text.0.encode_utf16().count()
        > t3_contracts::guest_share::GUEST_MESSAGE_MAX_LENGTH
    {
        return bad_request();
    }
    if services.lifecycle.admit_command().is_err() {
        return error_response(denied(GuestFailure::Rejected));
    }
    match adapter.sharing.submit(&token, submission).await {
        Ok(()) => uncached(StatusCode::NO_CONTENT.into_response()),
        Err(error) => error_response(error),
    }
}

async fn command_output(
    State(services): State<Arc<ServiceRegistry>>,
    request: Request,
) -> Response {
    let Some(id) = param(&request, "item").filter(|item| !item.is_empty()) else {
        return bad_request();
    };
    let (adapter, token, thread) = match authorize(&services, credential(&request)).await {
        Ok(value) => value,
        Err(error) => return error_response(error),
    };
    let mut output = match adapter.command_output(thread, TurnItemId(id)).await {
        Ok(output) => output,
        Err(error) => return error_response(error),
    };
    if let Err(error) = adapter.sharing.check(&token).await {
        return error_response(error);
    }
    let revoked = async move {
        let spacing = Duration::from_secs(30);
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + spacing, spacing);
        loop {
            interval.tick().await;
            if adapter.sharing.check(&token).await.is_err() {
                return;
            }
        }
    };
    let events = async_stream::stream! {
        while let Ok(Some(frame)) = output.pull().await {
            yield frame;
        }
    };
    sse(events.take_until(revoked), &services)
}
