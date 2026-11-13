//! MCP OAuth routes: `apps/server/src/auth/mcpOAuthHttp.ts`. The decisions (client and redirect
//! verification, PKCE, resource binding, CSRF, one-time codes) live in `t3_auth::oauth`.
//!
//! Not ported: the hosted T3 Connect approval page. `authorize` always continues on this
//! environment's own `/connect-agent` page because it needs `cloud/CloudLink.ts`.
use super::{
    auth::{credential_json, split},
    body::{self, BodyError},
    error::ApiError,
};
use crate::services::ServiceRegistry;
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Map, Value, json};
use std::{net::SocketAddr, sync::Arc};
use t3_auth::{
    Error,
    oauth::{AuthorizationError, AuthorizationRequest},
};
use t3_contracts::{auth::*, environment_http::EnvironmentInternalErrorReason as Internal};

const APPROVAL_PAGE_PATH: &str = "/connect-agent";
const PAGE_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'";

pub fn routes(router: Router<Arc<ServiceRegistry>>) -> Router<Arc<ServiceRegistry>> {
    router
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .route("/oauth/mcp/register", post(register))
        .route("/oauth/mcp/authorize", get(authorize))
        .route("/oauth/mcp/approval", post(approval))
        .route("/oauth/mcp/decision", post(decision))
        .route("/oauth/mcp/token", post(token))
}

fn oauth_failure(status: StatusCode, error: &str, description: &str) -> Response {
    ApiError::json(
        status,
        json!({"error": error, "error_description": description}),
    )
    .into_response()
}
fn approval_failure(message: impl Into<String>) -> Response {
    ApiError::json(
        StatusCode::BAD_REQUEST,
        json!({"_tag": "AuthMcpApprovalError", "message": message.into()}),
    )
    .into_response()
}
fn body_error(error: BodyError) -> Response {
    error.into_response()
}

async fn protected_resource(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let (req, _, _) = split(remote, request);
    match s.local.auth.mcp_protected_resource(&req) {
        Ok(metadata) => Json(metadata).into_response(),
        Err(e) => ApiError::from_auth(e, Internal::InternalError).into_response(),
    }
}

async fn authorization_server(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let (req, _, _) = split(remote, request);
    match s.local.auth.mcp_authorization_server(&req) {
        Ok(metadata) => Json(metadata).into_response(),
        Err(e) => ApiError::from_auth(e, Internal::InternalError).into_response(),
    }
}

async fn register(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let (_, headers, body) = split(remote, request);
    let input: AuthMcpClientRegistration = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s.local.auth.mcp_register(input) {
        Ok(client) => credential_json(client),
        Err(Error::OAuth { error, description }) => {
            oauth_failure(StatusCode::BAD_REQUEST, error, &description)
        }
        Err(e) => ApiError::from_auth(e, Internal::InternalError).into_response(),
    }
}

fn redirect(location: &str, referrer_policy: bool) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(location) {
        headers.insert(header::LOCATION, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if referrer_policy {
        headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    }
    response
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `apps/server/src/auth/mcpOAuthHtml.ts`.
fn error_page(description: &str) -> String {
    let description = escape_html(description);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n  <head>\n    <meta charset=\"utf-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <meta name=\"color-scheme\" content=\"light dark\" />\n    <title>Sign-in failed</title>\n    <style>{STYLES}</style>\n  </head>\n  <body>\n    <main><h1>This sign-in cannot continue</h1>\n    <p>{description}</p>\n    <p>Close this page and start the sign-in again from your agent.</p></main>\n  </body>\n</html>"
    )
}
const STYLES: &str = "\n  :root { color-scheme: light dark; font-family: ui-sans-serif, -apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif; }\n  body { margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 32px 16px; background: #f6f7f9; color: #17191f; }\n  main { width: min(100%, 440px); }\n  h1 { margin: 0 0 8px; font-size: 20px; }\n  p { margin: 0 0 12px; line-height: 1.5; color: #4b5060; }\n  @media (prefers-color-scheme: dark) {\n    body { background: #0f1115; color: #e6e8ee; }\n    p { color: #a0a6b4; }\n  }\n";

fn page_response(description: &str) -> Response {
    let mut response = (StatusCode::BAD_REQUEST, error_page(description)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(PAGE_CSP),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Query parameters as the TypeScript's `Object.fromEntries`: a repeated key keeps its last value.
fn query_request(query: Option<&str>) -> Option<AuthMcpAuthorizationRequest> {
    let mut fields = Map::new();
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        fields.insert(key.into_owned(), Value::String(value.into_owned()));
    }
    serde_json::from_value(Value::Object(fields)).ok()
}

async fn authorize(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let query = request.uri().query().map(str::to_owned);
    let (req, _, _) = split(remote, request);
    let Some(authorization) = query_request(query.as_deref()) else {
        return ApiError::internal(Internal::InternalError).into_response();
    };
    match s.local.auth.mcp_validate(&req, authorization) {
        Ok(_) => {
            let search = query.map(|q| format!("?{q}")).unwrap_or_default();
            redirect(&format!("{APPROVAL_PAGE_PATH}{search}"), true)
        }
        Err(AuthorizationError::Page(description)) => page_response(&description),
        Err(error) => {
            let issuer = t3_auth::oauth::OAuthUrls::from_request(&req).issuer;
            match error.redirect(&issuer) {
                Some(location) => redirect(&location, true),
                None => ApiError::internal(Internal::InternalError).into_response(),
            }
        }
    }
}

/// A verified request, or the response that ends the call: an unverified client or redirect is
/// reported, never redirected; any other problem goes back through the verified redirect URI.
fn forward(
    s: &ServiceRegistry,
    req: &t3_auth::Request,
    authorization: AuthMcpAuthorizationRequest,
) -> Result<AuthorizationRequest, Box<Response>> {
    match s.local.auth.mcp_validate(req, authorization) {
        Ok(authorization) => Ok(authorization),
        Err(AuthorizationError::Page(description)) => Err(Box::new(approval_failure(description))),
        Err(error) => {
            let issuer = t3_auth::oauth::OAuthUrls::from_request(req).issuer;
            match error.redirect(&issuer) {
                Some(location) => Err(Box::new(credential_json(json!({"redirectTo": location})))),
                None => Err(Box::new(
                    ApiError::internal(Internal::InternalError).into_response(),
                )),
            }
        }
    }
}

async fn approval(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request);
    let input: AuthMcpAuthorizationRequest = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    let authorization = match forward(&s, &req, input) {
        Ok(authorization) => authorization,
        Err(response) => return *response,
    };
    match s
        .local
        .auth
        .mcp_approval_details(&req, &authorization)
        .await
    {
        Ok(details) => credential_json(details),
        Err(e) => ApiError::from_auth(e, Internal::InternalError).into_response(),
    }
}

async fn decision(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request);
    let input: AuthMcpApprovalDecisionRequest = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    let authorization = match forward(&s, &req, input.authorization) {
        Ok(authorization) => authorization,
        Err(response) => return *response,
    };
    let result = match input.decision {
        AuthMcpApprovalDecision::AuthMcpApprovalDeny(_) => s
            .local
            .auth
            .mcp_deny(&authorization)
            .map(|redirect_to| json!({"redirectTo": redirect_to})),
        decision => s
            .local
            .auth
            .mcp_decide(&req, &authorization, decision)
            .await
            .map(|approved| json!({"redirectTo": approved.redirect})),
    };
    match result {
        Ok(body) => credential_json(body),
        Err(e @ (Error::McpApprovalCode { .. } | Error::ApprovalPage(_))) => {
            approval_failure(e.to_string())
        }
        Err(e) => ApiError::from_auth(e, Internal::InternalError).into_response(),
    }
}

async fn token(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request);
    let input: AuthMcpTokenRequest = match body::form_or_json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s.local.auth.mcp_token(&req, input).await {
        Ok(result) => credential_json(result),
        Err(Error::OAuth { error, description }) => {
            oauth_failure(StatusCode::BAD_REQUEST, error, &description)
        }
        // The session store failed after the code was spent; the source stays out of the response.
        Err(Error::OAuthSessionIssue { .. }) => oauth_failure(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The session could not be issued.",
        ),
        Err(e) => ApiError::from_auth(e, Internal::InternalError).into_response(),
    }
}
