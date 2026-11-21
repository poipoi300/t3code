use crate::services::ServiceRegistry;
use axum::{Router, routing::get};
use std::sync::Arc;
#[cfg(feature = "local-services")]
mod auth;
#[cfg(feature = "local-services")]
mod body;
mod cors;
#[cfg(feature = "local-services")]
mod error;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "local-services")]
mod mcp_oauth;
#[cfg(all(feature = "local-services", feature = "orchestration"))]
mod orchestration;
#[cfg(feature = "local-services")]
mod static_files;

pub fn router(services: Arc<ServiceRegistry>) -> Router {
    let router = Router::new().route(
        "/healthz",
        get(|| async {
            axum::Json(serde_json::json!({"status":"ok", "orchestrationReady":false}))
        }),
    );
    #[cfg(feature = "local-services")]
    let (router, cors) = {
        let config = &services.local.config;
        let cors = cors::Cors::new(
            config
                .dev_url
                .as_ref()
                .map(|url| url.origin().ascii_serialization()),
            &config.dev_allowed_origins,
        );
        let router = router.route("/ws", get(crate::ws::upgrade));
        let router = mcp_oauth::routes(auth::routes(router));
        #[cfg(feature = "pull-requests")]
        let router = router.route(
            "/api/pull-requests/diff",
            axum::routing::post(pull_request_diff),
        );
        #[cfg(feature = "mcp")]
        let router = mcp::routes(router, &services);
        #[cfg(feature = "preview")]
        let router = router.merge(crate::handlers::preview::http_routes());
        #[cfg(feature = "workspace")]
        let router = router.merge(crate::handlers::workspace_terminal::http_routes());
        #[cfg(feature = "scheduler")]
        let router = crate::adapters::scheduler::routes(router);
        #[cfg(feature = "orchestration")]
        let router = orchestration::routes(router);
        #[cfg(feature = "cloud")]
        let router = crate::adapters::cloud::routes(router);
        // The device router owns its absolute path and its own policy-then-auth order.
        #[cfg(feature = "device")]
        let router = match services.domain::<crate::adapters::device::DeviceAdapter>() {
            Some(device) => {
                router.route_service("/api/device-hub/{*path}", device.runtime.router())
            }
            None => router,
        };
        let router = router.fallback(static_files::serve);
        (router, cors)
    };
    #[cfg(not(feature = "local-services"))]
    let cors = cors::Cors::production();
    router
        .layer(axum::middleware::from_fn_with_state(cors, cors::layer))
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(services)
}

#[cfg(feature = "local-services")]
pub(crate) use auth::error as auth_error;

#[cfg(feature = "pull-requests")]
async fn pull_request_diff(
    axum::extract::State(services): axum::extract::State<Arc<ServiceRegistry>>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<std::net::SocketAddr>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::{Json, http::StatusCode, response::IntoResponse};
    use t3_contracts::{
        auth::AuthEnvironmentScope, environment_http::EnvironmentInternalErrorReason,
    };
    let (req, headers, body) = auth::split(remote, request);
    if let Err(error) = auth::authenticate(
        &services,
        &req,
        Some(AuthEnvironmentScope::OrchestrationRead),
    )
    .await
    {
        return error.into_response();
    }
    let input = match body::json(&headers, body).await {
        Ok(payload) => match decode_diff_input(payload) {
            Some(input) => input,
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        Err(error) => return error.into_response(),
    };
    let Some(adapter) = services.domain::<crate::adapters::pull_requests::PullRequestAdapter>()
    else {
        return error::ApiError::internal(EnvironmentInternalErrorReason::InternalError)
            .into_response();
    };
    match adapter.service.diff(input).await {
        Ok(page) => Json(page).into_response(),
        Err(error) => match error.unavailable_error() {
            Some(unavailable) => {
                (StatusCode::SERVICE_UNAVAILABLE, Json(unavailable)).into_response()
            }
            None => (StatusCode::BAD_GATEWAY, Json(error.operation_error())).into_response(),
        },
    }
}

#[cfg(feature = "pull-requests")]
fn decode_diff_input(
    mut payload: serde_json::Value,
) -> Option<t3_contracts::pull_request::PullRequestDiffInput> {
    // HTTP JSON has no RPC encoding for undefined. Optional fields must be absent or typed.
    let fields = payload.as_object_mut()?;
    if [
        "host",
        "expectedAccountId",
        "allowStale",
        "cursor",
        "commit",
    ]
    .iter()
    .any(|key| fields.get(*key).is_some_and(serde_json::Value::is_null))
    {
        return None;
    }
    let number = fields.get_mut("number")?;
    let value = t3_contracts::wire::JsNumber::from_f64(number.as_f64()?)?.as_u64()?;
    if value == 0 {
        return None;
    }
    *number = value.into();
    serde_json::from_value(payload).ok()
}
