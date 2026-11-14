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
