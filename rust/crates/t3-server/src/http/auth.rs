//! Environment auth routes: `apps/server/src/auth/http.ts` and `packages/contracts/src/environmentHttp.ts`.
//!
//! Order on guarded routes: authenticate (401), require the scope (403), then read the body
//! (400/413). The TypeScript decodes the payload before the scope check; refusing first means a
//! caller without the scope never makes the server read its body.
use super::{
    body::{self, BodyError},
    error::{ApiError, no_store},
};
use crate::{
    adapters::auth::{VerifiedSession, cookie_header, request},
    services::ServiceRegistry,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::json;
use std::{net::SocketAddr, sync::Arc};
use t3_auth::{Request, environment::dpop_challenge};
use t3_contracts::{
    auth::*, environment::ExecutionEnvironmentDescriptor,
    environment_http::EnvironmentInternalErrorReason as Internal,
};

pub fn routes(router: Router<Arc<ServiceRegistry>>) -> Router<Arc<ServiceRegistry>> {
    router
        .route("/.well-known/t3/environment", get(descriptor))
        .route("/api/auth/session", get(session))
        .route("/api/auth/browser-session", post(browser_session))
        .route("/oauth/token", post(exchange))
        .route("/api/auth/websocket-ticket", post(ticket))
        .route("/api/auth/websocket-ticket/{audience}", post(ticket_for))
        .route("/api/auth/pairing-token", post(pairing))
        .route("/api/auth/pairing-links/revoke", post(revoke_pairing_link))
        .route("/api/auth/clients/revoke", post(revoke))
        .route("/api/auth/clients/revoke-others", post(revoke_others))
        .route(APPROVE_CONNECT_DEVICE, post(approve_connect_device))
        .route(REVOKE_CONNECT_DEVICE, post(revoke_connect_device))
}

const APPROVE_CONNECT_DEVICE: &str = "/api/auth/connect-devices/approve";
const REVOKE_CONNECT_DEVICE: &str = "/api/auth/connect-devices/revoke";

/// Connect device routes check their scope through `t3-auth`'s route table.
async fn connect_device_session(
    s: &ServiceRegistry,
    req: &Request,
    path: &str,
) -> Result<(), ApiError> {
    let session = authenticate(s, req, None).await?;
    t3_auth::authorization::authorize_connect_device_http(&session.scopes, "POST", path)
        .map_err(|e| ApiError::from_auth(e, Internal::InternalError))
}

async fn approve_connect_device(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request_in);
    if let Err(error) = connect_device_session(&s, &req, APPROVE_CONNECT_DEVICE).await {
        return error.into_response();
    }
    let input: AuthApproveConnectDeviceInput = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s
        .local
        .auth
        .approve_connect_device(&input.pairing_link_id.0)
        .await
    {
        Ok(approved) => credential_json(AuthApproveConnectDeviceResult { approved }),
        Err(e) => ApiError::from_auth(e, Internal::ConnectDeviceApproveFailed).into_response(),
    }
}

async fn revoke_connect_device(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request_in);
    if let Err(error) = connect_device_session(&s, &req, REVOKE_CONNECT_DEVICE).await {
        return error.into_response();
    }
    let input: AuthRevokeConnectDeviceInput = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s
        .local
        .auth
        .revoke_connect_device(&input.proof_key_thumbprint.0)
        .await
    {
        Ok(revoked) => credential_json(AuthRevokeConnectDeviceResult { revoked }),
        Err(e) => ApiError::from_auth(e, Internal::ConnectDeviceRevokeFailed).into_response(),
    }
}

/// `Json` plus the credential cache headers.
pub fn credential_json(value: impl Serialize) -> Response {
    let mut response = Json(value).into_response();
    no_store(&mut response);
    response
}

/// The request as `t3-auth` sees it, with the headers and body left for the route.
pub fn split(remote: SocketAddr, request_in: axum::extract::Request) -> (Request, HeaderMap, Body) {
    let (parts, body) = request_in.into_parts();
    let req = request(
        parts.method.as_str(),
        &parts.uri,
        &parts.headers,
        Some(remote),
    );
    (req, parts.headers, body)
}

pub(super) async fn authenticate(
    s: &ServiceRegistry,
    req: &Request,
    scope: Option<AuthEnvironmentScope>,
) -> Result<VerifiedSession, ApiError> {
    let result = match scope {
        Some(scope) => s.local.auth.require(req, scope).await,
        None => s.local.auth.authenticate(req).await,
    };
    result
        .map_err(|e| ApiError::from_auth(e, Internal::InternalError).challenge(dpop_challenge(req)))
}

fn body_error(error: BodyError) -> Response {
    error.into_response()
}

async fn descriptor(State(s): State<Arc<ServiceRegistry>>) -> Json<ExecutionEnvironmentDescriptor> {
    Json(s.local.environment.clone())
}

async fn session(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, _, _) = split(remote, request_in);
    let state = match s.local.auth.session_state(&req).await {
        Ok(state) => state,
        Err(e) => return ApiError::from_auth(e, Internal::InternalError).into_response(),
    };
    // A legacy-named cookie is re-issued under the current name so the client migrates.
    let refreshed = match (
        &state.session_method,
        &state.expires_at,
        state.authenticated,
    ) {
        (Some(ServerAuthSessionMethod::BrowserSessionCookie), Some(expires), true) => s
            .local
            .auth
            .legacy_cookie_token(&req)
            .map(|token| (token.to_owned(), expires.clone())),
        _ => None,
    };
    let mut response = credential_json(&state);
    if let Some((token, expires)) = refreshed {
        let secure = req.absolute_url().is_some_and(|u| u.scheme() == "https");
        let cookie = t3_auth::environment::Cookie::session(
            s.local.auth.cookie_name(),
            token,
            expires,
            secure,
        );
        match cookie_header(cookie) {
            Ok(value) => {
                response.headers_mut().append("set-cookie", value);
            }
            Err(_) => {
                return ApiError::internal(Internal::BrowserSessionCookieFailed).into_response();
            }
        }
    }
    response
}

async fn browser_session(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request_in);
    let input: AuthBrowserSessionRequest = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    let result = match s.local.auth.browser_session(&req, input).await {
        Ok(result) => result,
        Err(e) => {
            return ApiError::from_auth(e, Internal::BrowserSessionIssuanceFailed).into_response();
        }
    };
    let mut values: Vec<HeaderValue> = Vec::new();
    for cookie in result.cookies {
        match cookie_header(cookie) {
            Ok(value) => values.push(value),
            Err(_) => {
                return ApiError::internal(Internal::BrowserSessionCookieFailed).into_response();
            }
        }
    }
    let mut response = credential_json(&result.response);
    for value in values {
        response.headers_mut().append("set-cookie", value);
    }
    response
}

async fn exchange(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request_in);
    let input: AuthTokenExchangeRequest = match body::form_or_json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s.local.auth.exchange(&req, input).await {
        Ok(result) => credential_json(&result),
        Err(e) => {
            let error = ApiError::from_auth(e, Internal::AccessTokenIssuanceFailed);
            // Only a rejected proof earns the challenge here, not a rejected bootstrap credential.
            let challenge = error.dpop_failed();
            error.challenge(challenge).into_response()
        }
    }
}

async fn ticket(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, _, _) = split(remote, request_in);
    issue_ticket(&s, &req, None).await
}

async fn ticket_for(
    State(s): State<Arc<ServiceRegistry>>,
    Path(audience): Path<String>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, _, _) = split(remote, request_in);
    let audience = match audience.as_str() {
        "rpc" => AuthWebSocketTicketUse::Rpc,
        "media" => AuthWebSocketTicketUse::Media,
        _ => {
            // The session is checked first so an unauthenticated caller learns nothing about routes.
            return match authenticate(&s, &req, None).await {
                Ok(_) => StatusCode::BAD_REQUEST.into_response(),
                Err(e) => e.into_response(),
            };
        }
    };
    issue_ticket(&s, &req, Some(audience)).await
}

async fn issue_ticket(
    s: &ServiceRegistry,
    req: &Request,
    audience: Option<AuthWebSocketTicketUse>,
) -> Response {
    let session = match authenticate(s, req, None).await {
        Ok(session) => session,
        Err(e) => return e.into_response(),
    };
    match s.local.auth.ticket(&session, audience).await {
        Ok(result) => credential_json(&result),
        Err(e) => ApiError::from_auth(e, Internal::WebsocketTicketIssuanceFailed).into_response(),
    }
}

async fn pairing(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request_in);
    let session = match authenticate(&s, &req, Some(AuthEnvironmentScope::AccessWrite)).await {
        Ok(session) => session,
        Err(e) => return e.into_response(),
    };
    let input: AuthCreatePairingCredentialInput = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s.local.auth.pairing(&session, input).await {
        Ok(result) => credential_json(&result),
        Err(e) => ApiError::from_auth(e, Internal::PairingCredentialIssuanceFailed).into_response(),
    }
}

async fn revoke_pairing_link(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request_in);
    if let Err(e) = authenticate(&s, &req, Some(AuthEnvironmentScope::AccessWrite)).await {
        return e.into_response();
    }
    let input: AuthRevokePairingLinkInput = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s.local.auth.revoke_pairing_link(&input.id.0).await {
        Ok(revoked) => credential_json(json!({"revoked": revoked})),
        Err(e) => ApiError::from_auth(e, Internal::PairingLinkRevokeFailed).into_response(),
    }
}

async fn revoke(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, headers, body) = split(remote, request_in);
    let session = match authenticate(&s, &req, Some(AuthEnvironmentScope::AccessWrite)).await {
        Ok(session) => session,
        Err(e) => return e.into_response(),
    };
    let input: AuthRevokeClientSessionInput = match body::json(&headers, body).await {
        Ok(input) => input,
        Err(e) => return body_error(e),
    };
    match s.local.auth.revoke(&session, &input.session_id).await {
        Ok(revoked) => credential_json(json!({"revoked": revoked})),
        Err(e) => ApiError::from_auth(e, Internal::ClientSessionRevokeFailed).into_response(),
    }
}

async fn revoke_others(
    State(s): State<Arc<ServiceRegistry>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request_in: axum::extract::Request,
) -> Response {
    let (req, _, _) = split(remote, request_in);
    let session = match authenticate(&s, &req, Some(AuthEnvironmentScope::AccessWrite)).await {
        Ok(session) => session,
        Err(e) => return e.into_response(),
    };
    match s.local.auth.revoke_others(&session).await {
        Ok(count) => credential_json(json!({"revokedCount": count})),
        Err(e) => ApiError::from_auth(e, Internal::ClientSessionRevokeFailed).into_response(),
    }
}

/// Failure rendering for `/ws`, which authenticates outside these routes.
pub fn error(error: crate::adapters::auth::Error) -> Response {
    ApiError::from_auth(error, Internal::InternalError).into_response()
}
