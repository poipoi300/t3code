use crate::{
    InvocationScope, McpServer, Modes, SessionRegistry,
    server::{ProtocolHandler, RequestScope},
};
use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, HttpBody},
    extract::State,
    http::{HeaderMap, Request, Response, StatusCode},
    middleware::{self, Next},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpService,
    session::{SessionManager, local::LocalSessionManager},
    tower::StreamableHttpServerConfig,
};
use serde_json::json;
use std::sync::Arc;
use t3_contracts::provider_policy::{ProviderInteractionMode, RuntimeMode};

#[async_trait]
pub trait ClientAuthenticator: Send + Sync {
    async fn authenticate(
        &self,
        headers: &HeaderMap,
        uri: &http::Uri,
    ) -> Option<AuthenticatedClient>;
}

pub struct AuthenticatedClient {
    pub scope: InvocationScope,
    pub scopes: Vec<t3_contracts::auth::AuthEnvironmentScope>,
}

pub struct HttpState {
    pub registry: Arc<SessionRegistry>,
    pub clients: Option<Arc<dyn ClientAuthenticator>>,
    pub linked_environments_enabled: std::sync::atomic::AtomicBool,
    pub runtime_ceiling: Option<RuntimeMode>,
}

pub fn router(server: Arc<McpServer>, auth: Arc<HttpState>) -> Router {
    let sessions = Arc::new(LocalSessionManager::default());
    let transport = StreamableHttpService::new(
        move || Ok(ProtocolHandler(server.clone())),
        sessions.clone(),
        StreamableHttpServerConfig::default()
            .disable_allowed_hosts()
            .with_json_response(true),
    );
    Router::new()
        .route_service("/mcp", transport)
        .layer(middleware::from_fn_with_state(
            AuthMiddlewareState { auth, sessions },
            authenticate,
        ))
}

#[derive(Clone)]
struct AuthMiddlewareState {
    auth: Arc<HttpState>,
    sessions: Arc<LocalSessionManager>,
}

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let header = headers.get("authorization")?.to_str().ok()?;
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() { None } else { Some(token) }
}

fn response(status: StatusCode, error: &str, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(Body::from(
            json!({"error":error,"message":message}).to_string(),
        ))
        .expect("static HTTP response")
}

async fn authenticate(
    State(state): State<AuthMiddlewareState>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let auth = state.auth;
    let token = bearer(request.headers()).unwrap_or_default();
    let scope = auth.registry.resolve(token);
    let scope = match scope {
        Some(scope) => Some((scope, None)),
        None if !token.is_empty() => match &auth.clients {
            Some(clients) => clients
                .authenticate(request.headers(), request.uri())
                .await
                .map(|client| (client.scope, Some(client.scopes))),
            None => None,
        },
        None => None,
    };
    let Some((mut scope, granted_scopes)) = scope else {
        let reason = match request
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
        {
            None => "missing_authorization_header",
            Some(header)
                if !header
                    .split(' ')
                    .next()
                    .is_some_and(|s| s.eq_ignore_ascii_case("bearer")) =>
            {
                "unsupported_scheme"
            }
            Some(_) if token.is_empty() => "missing_bearer_token",
            Some(_) => "unknown_or_expired_token",
        };
        tracing::warn!(reason, "rejected MCP request with an unusable credential");
        let provider_token = !token.is_empty() && !token.contains('.');
        let mut challenge = "Bearer".to_owned();
        if auth.clients.is_some() && !provider_token {
            let host = request
                .headers()
                .get("host")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("127.0.0.1");
            let scheme = if request
                .headers()
                .get("x-forwarded-proto")
                .is_some_and(|value| value == "https")
            {
                "https"
            } else {
                request.uri().scheme_str().unwrap_or("http")
            };
            challenge.push_str(&format!(
                " resource_metadata=\"{scheme}://{host}/.well-known/oauth-protected-resource/mcp\""
            ));
        }
        if !token.is_empty() {
            challenge.push_str(if challenge == "Bearer" {
                " error=\"invalid_token\""
            } else {
                ", error=\"invalid_token\""
            });
        }
        let mut result = response(
            StatusCode::UNAUTHORIZED,
            "invalid_mcp_credential",
            "A valid T3 Code MCP credential is required.",
        );
        if let Ok(value) = challenge.parse() {
            result.headers_mut().insert("www-authenticate", value);
        }
        return result;
    };
    if let Some(client) = &mut scope.client {
        if client.linked
            && let Some(sender) = request
                .headers()
                .get("t3-sender-thread")
                .and_then(|header| header.to_str().ok())
                .and_then(sender_thread)
        {
            client.sender_thread = Some(sender);
        }
        if let Some(header) = request.headers().get("t3-mode-limit") {
            let parsed = header.to_str().ok().and_then(|header| {
                let parts = header.trim().split('/').collect::<Vec<_>>();
                if parts.len() != 2 {
                    return None;
                }
                Some(Modes {
                    runtime_mode: serde_json::from_value::<RuntimeMode>(json!(parts[0])).ok()?,
                    interaction_mode: serde_json::from_value::<ProviderInteractionMode>(json!(
                        parts[1]
                    ))
                    .ok()?,
                })
            });
            let Some(modes) = parsed else {
                return response(
                    StatusCode::BAD_REQUEST,
                    "invalid_mode_limit",
                    "T3-Mode-Limit must be <runtimeMode>/<interactionMode>, e.g. auto/default.",
                );
            };
            client.narrowed_to = Some(modes);
        }
        if let Some(ceiling) = auth.runtime_ceiling {
            client.narrowed_to = Some(client.narrowed_to.unwrap_or(Modes::full()).narrow(Modes {
                runtime_mode: ceiling,
                interaction_mode: ProviderInteractionMode::Default,
            }));
        }
    }
    let linked = auth
        .linked_environments_enabled
        .load(std::sync::atomic::Ordering::Relaxed);
    if scope.client.as_ref().is_some_and(|client| client.linked) && !linked {
        return response(
            StatusCode::FORBIDDEN,
            "linked_environments_off",
            "Linked environments is off on this machine. Turn it on in its Settings → Labs.",
        );
    }
    scope.linked_environments = Some(linked);
    let deferred = request
        .headers()
        .get("t3-tool-exposure")
        .is_some_and(|value| value == "deferred");
    request.extensions_mut().insert(RequestScope {
        scope,
        deferred,
        granted_scopes,
    });
    let terminating = request.method() == http::Method::DELETE;
    if terminating
        && let Some(id) = request
            .headers()
            .get("mcp-session-id")
            .and_then(|header| header.to_str().ok())
    {
        match state.sessions.has_session(&id.to_owned().into()).await {
            Ok(false) => {
                return response(
                    StatusCode::NOT_FOUND,
                    "session_not_found",
                    "Session not found",
                );
            }
            Err(_) => {
                return response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Session lookup failed",
                );
            }
            Ok(true) => {}
        }
    }
    let mut result = next.run(request).await;
    if terminating && result.status() == StatusCode::ACCEPTED {
        *result.status_mut() = StatusCode::NO_CONTENT;
    }
    if result.status() == StatusCode::OK && result.body().size_hint().exact() == Some(0) {
        *result.status_mut() = if terminating {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::ACCEPTED
        };
    }
    result
}

fn sender_thread(header: &str) -> Option<serde_json::Value> {
    let parts = header.trim().split('/').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let decode = |part: &str| {
        let bytes = part.as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'%'
                && !(bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                    && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
            {
                return None;
            }
        }
        percent_encoding::percent_decode_str(part)
            .decode_utf8()
            .ok()
            .map(|s| s.into_owned())
    };
    let value = json!({"environmentId":decode(parts[0])?,"threadId":decode(parts[1])?,"title":decode(parts[2])?});
    serde_json::from_value::<t3_contracts::orchestration_v2::OrchestrationV2LinkedSender>(
        value.clone(),
    )
    .ok()?;
    Some(value)
}
