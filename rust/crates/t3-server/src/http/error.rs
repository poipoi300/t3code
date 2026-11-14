//! HTTP error envelopes shared by every route in this module.
use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use std::hash::{BuildHasher, Hasher};
use t3_auth::Error;
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    environment_http::{self as h, EnvironmentInternalErrorReason as Internal},
};

/// A fully rendered failure. Credential-bearing routes always add `no-store`.
pub struct ApiError {
    status: StatusCode,
    body: Option<Value>,
    dpop_challenge: bool,
}

impl ApiError {
    pub fn json(status: StatusCode, body: impl serde::Serialize) -> Self {
        Self {
            status,
            body: Some(serde_json::to_value(body).unwrap_or(Value::Null)),
            dpop_challenge: false,
        }
    }
    pub fn challenge(mut self, challenge: bool) -> Self {
        self.dpop_challenge = challenge;
        self
    }
    pub fn internal(reason: Internal) -> Self {
        Self::internal_because(reason, None)
    }
    /// `failEnvironmentInternal`: the trace id the client receives is logged with the reason
    /// and the cause, so an operator can find the failure a user reports.
    pub fn internal_because(reason: Internal, cause: Option<&dyn std::fmt::Display>) -> Self {
        let trace_id = trace_id();
        match cause {
            Some(cause) => tracing::error!(
                ?reason,
                trace_id = %trace_id.0,
                %cause,
                "environment api operation failed"
            ),
            None => tracing::error!(
                ?reason,
                trace_id = %trace_id.0,
                "environment api operation failed"
            ),
        }
        Self::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            h::EnvironmentInternalError {
                tag: h::EnvironmentInternalErrorTag::EnvironmentInternalError,
                code: h::EnvironmentInternalErrorCode::InternalError,
                reason,
                trace_id,
            },
        )
    }
    pub fn invalid_request(reason: h::EnvironmentRequestInvalidReason) -> Self {
        Self::json(
            StatusCode::BAD_REQUEST,
            h::EnvironmentRequestInvalidError {
                tag: h::EnvironmentRequestInvalidErrorTag::EnvironmentRequestInvalidError,
                code: h::EnvironmentRequestInvalidErrorCode::InvalidRequest,
                reason,
                trace_id: trace_id(),
            },
        )
    }
    pub fn forbidden_message(message: String) -> Self {
        Self::json(
            StatusCode::FORBIDDEN,
            h::EnvironmentHttpForbiddenError {
                tag: h::EnvironmentHttpForbiddenErrorTag::EnvironmentHttpForbiddenError,
                message,
            },
        )
    }
    /// Maps a `t3-auth` failure. `internal` is the reason the TypeScript route reports for
    /// failures that are not the caller's fault.
    pub fn from_auth(error: Error, internal: Internal) -> Self {
        match error {
            Error::Authorization(e) => Self::json(
                StatusCode::FORBIDDEN,
                h::EnvironmentScopeRequiredError {
                    tag: h::EnvironmentScopeRequiredErrorTag::EnvironmentScopeRequiredError,
                    code: h::EnvironmentScopeRequiredErrorCode::InsufficientScope,
                    required_scope: e.required_scope,
                    required_permission: e.required_permission,
                    trace_id: trace_id(),
                },
            ),
            Error::SelfRevocation => Self::json(
                StatusCode::FORBIDDEN,
                h::EnvironmentOperationForbiddenError {
                    tag:
                        h::EnvironmentOperationForbiddenErrorTag::EnvironmentOperationForbiddenError,
                    code: h::EnvironmentOperationForbiddenErrorCode::OperationForbidden,
                    reason: h::EnvironmentOperationForbiddenReason::CurrentSessionRevokeNotAllowed,
                    trace_id: trace_id(),
                },
            ),
            Error::InvalidScope => {
                Self::invalid_request(h::EnvironmentRequestInvalidReason::InvalidScope)
            }
            Error::ScopeNotGranted => {
                Self::invalid_request(h::EnvironmentRequestInvalidReason::ScopeNotGranted)
            }
            Error::PolicyRefused(message) => Self::forbidden_message(message),
            // A Connect device waiting for approval is not a credential failure: no recovery.
            Error::ConnectApprovalPending => Self::json(
                StatusCode::FORBIDDEN,
                Error::ConnectApprovalPending.connect_approval_pending_response(trace_id()),
            ),
            e if e.is_credential() => {
                let (reason, dpop_failure_reason) = match &e {
                    Error::MissingCredential => {
                        (h::EnvironmentAuthInvalidReason::MissingCredential, None)
                    }
                    Error::InvalidCredential {
                        dpop_failure_reason,
                        ..
                    } => (
                        h::EnvironmentAuthInvalidReason::InvalidCredential,
                        *dpop_failure_reason,
                    ),
                    _ => (h::EnvironmentAuthInvalidReason::InvalidCredential, None),
                };
                Self::json(
                    StatusCode::UNAUTHORIZED,
                    h::EnvironmentAuthInvalidError {
                        tag: h::EnvironmentAuthInvalidErrorTag::EnvironmentAuthInvalidError,
                        code: h::EnvironmentAuthInvalidErrorCode::AuthInvalid,
                        reason,
                        dpop_failure_reason,
                        trace_id: trace_id(),
                    },
                )
            }
            other => Self::internal_because(internal, Some(&other)),
        }
    }
    pub fn dpop_failed(&self) -> bool {
        self.status == StatusCode::UNAUTHORIZED
            && self
                .body
                .as_ref()
                .is_some_and(|b| b.get("dpopFailureReason").is_some())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = match self.body {
            Some(body) => (self.status, Json(body)).into_response(),
            None => self.status.into_response(),
        };
        no_store(&mut response);
        if self.dpop_challenge && self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert("www-authenticate", HeaderValue::from_static("DPoP"));
        }
        response
    }
}

pub fn no_store(response: &mut Response) {
    for (name, value) in t3_auth::environment::CREDENTIAL_RESPONSE_HEADERS {
        response
            .headers_mut()
            .insert(*name, HeaderValue::from_static(value));
    }
}

/// A 32-hex-digit identifier; the Rust server has no tracing span to take it from.
pub fn trace_id() -> TrimmedNonEmptyString {
    let state = std::collections::hash_map::RandomState::new();
    let part = |salt: u8| {
        let mut hasher = state.build_hasher();
        hasher.write_u8(salt);
        hasher.finish()
    };
    TrimmedNonEmptyString(format!("{:016x}{:016x}", part(0), part(1)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_connect_device_is_forbidden_not_a_credential_failure() {
        let error = ApiError::from_auth(Error::ConnectApprovalPending, Internal::InternalError);
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(!error.dpop_failed());
        let body = error.body.unwrap();
        assert_eq!(body["_tag"], "EnvironmentConnectApprovalPendingError");
        assert_eq!(body["code"], "connect_approval_pending");
        assert!(body["traceId"].is_string(), "{body}");
    }
}
