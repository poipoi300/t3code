//! `/mcp`: the streamable HTTP MCP endpoint (`McpHttpServer.ts` `layerMcpTransport`). The
//! router, its authentication middleware (registry tokens first, then OAuth client tokens for
//! this request's origin; mode limits, managed ceiling, linked-environments gate) and the tool
//! calls are `t3-mcp`'s; this module only mounts them when the adapter started.
use crate::services::ServiceRegistry;
use axum::Router;
use std::sync::Arc;

pub fn routes(
    router: Router<Arc<ServiceRegistry>>,
    services: &ServiceRegistry,
) -> Router<Arc<ServiceRegistry>> {
    match services.domain::<crate::adapters::mcp::McpAdapter>() {
        Some(mcp) => router.route_service(
            "/mcp",
            t3_mcp::transport::router(mcp.server.clone(), mcp.http.clone()),
        ),
        None => router,
    }
}
