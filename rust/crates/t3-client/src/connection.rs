//! Connection policy and a shared native/web runtime, driven on the shell's local executor.
//!
//! [`connect`] accepts a platform, origin and bearer or pairing token. It returns
//! a [`ConnectedClient`] once RPC and config are available, plus a
//! [`ConnectionTask`] the shell retains and polls for updates and reconnects.
//! [`socket::PlatformSessionFactory`] adapts host sockets to the existing RPC
//! protocol; [`runtime`] executes the supervisor's actions and owns their scopes.
//! Lower-level catalog users share one `RemoteAuthorization` with the registry
//! and cloud-account service. `load_authorized` restores cloud-bound attempts.

use crate::{
    platform::{NetworkStatus, Platform, PlatformEvent},
    types::{EnvironmentId, Generation},
};
use futures::{
    FutureExt, StreamExt,
    future::{Either, LocalBoxFuture, select},
    stream::{FuturesUnordered, LocalBoxStream},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use url::{Host, Url};

pub mod runtime;
pub mod socket;
pub mod threads;
pub use runtime::{ConnectedClient, ConnectionTask, ServerCredential, connect, connect_into};
pub use threads::ThreadLease;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionMode {
    Local,
    Remote,
    Relay,
    Tunnel,
    Ssh,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub id: String,
    pub environment: EnvironmentId,
    pub mode: ConnectionMode,
    pub http_base: String,
    pub ws_base: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Available,
    Offline,
    Connecting,
    Backoff,
    Connected,
    Blocked,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptStage {
    Preparing,
    Opening,
    Synchronizing,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureKind {
    Network,
    Timeout,
    Transport,
    EndpointUnavailable,
    RelayUnavailable,
    RemoteUnavailable,
    SessionPending,
    Authentication,
    Configuration,
    Permission,
    Unsupported,
    /// The host has not yet approved this T3 Connect device; a wait, not a failure.
    ApprovalPending,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFailure {
    pub kind: FailureKind,
    pub detail: String,
    pub http_status: Option<u16>,
    pub trace_id: Option<String>,
    pub hop: Option<ConnectionHop>,
    pub via: Option<ConnectionVia>,
    pub cause: Option<FailureCause>,
    pub server_update_required: bool,
    /// The code this device shows while the host decides whether to trust it.
    pub verification_code: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionHop {
    Device,
    Relay,
    Tunnel,
    Server,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionVia {
    Direct,
    Relay,
}
#[derive(Debug, Clone, PartialEq)]
pub enum FailureCause {
    Endpoint(t3_contracts::relay::RelayEnvironmentEndpointFailure),
    ClockSkew,
    NotLinked,
    RelayError,
    TunnelPending,
    TunnelNotEnabled,
    ApprovalPending,
}
impl Eq for FailureCause {}

pub const NETWORK_BLOCKING_HINT: &str = "Your DNS or firewall may be blocking T3 Connect. Try another network, such as a different Wi-Fi or mobile data.";

pub fn profile_missing_error(connection_id: &str) -> ConnectionFailure {
    let mut error = ConnectionFailure::new(
        FailureKind::Configuration,
        format!("Connection profile {connection_id} is unavailable."),
    );
    error.hop = Some(ConnectionHop::Device);
    error
}
pub fn credential_missing_error(connection_id: &str) -> ConnectionFailure {
    let mut error = ConnectionFailure::new(
        FailureKind::Authentication,
        format!("Connection credential {connection_id} is unavailable."),
    );
    error.hop = Some(ConnectionHop::Device);
    error
}
pub fn environment_mismatch_error(
    expected: &EnvironmentId,
    actual: &EnvironmentId,
) -> ConnectionFailure {
    ConnectionFailure::new(
        FailureKind::Configuration,
        format!(
            "Connected environment {} does not match {}.",
            actual.0, expected.0
        ),
    )
}
pub fn map_remote_environment_error(
    error: &crate::authorization::AuthorizationError,
    via: ConnectionVia,
) -> ConnectionFailure {
    use crate::authorization::AuthorizationError as E;
    let kind = match error {
        E::Remote { status, details } => {
            return map_declared_remote_error(*status, details.expose(), via);
        }
        E::SessionPending => FailureKind::SessionPending,
        E::Timeout => FailureKind::Timeout,
        E::Network => FailureKind::Network,
        E::HttpStatus(401)
        | E::CredentialRejected
        | E::CredentialMissing
        | E::Proof
        | E::TokenType
        | E::SessionChanged => FailureKind::Authentication,
        E::HttpStatus(403) => FailureKind::Permission,
        E::InvalidUrl | E::EnvironmentMismatch | E::Storage | E::HttpStatus(400 | 404) => {
            FailureKind::Configuration
        }
        E::InvalidResponse | E::HttpStatus(_) | E::Relay => FailureKind::RemoteUnavailable,
        E::Cancelled => FailureKind::Transport,
    };
    let mut detail = match error {
        E::CredentialRejected | E::HttpStatus(401) => {
            "The environment credential is invalid.".into()
        }
        E::HttpStatus(403) => {
            "The environment credential does not grant the required access.".into()
        }
        E::HttpStatus(400) => "The environment rejected the authentication request.".into(),
        E::HttpStatus(404) => "The environment endpoint could not be found.".into(),
        E::SessionPending => "Waiting for the T3 Connect sign-in to load.".into(),
        _ => error.to_string(),
    };
    let unreachable = matches!(kind, FailureKind::Timeout | FailureKind::Network);
    if unreachable && via == ConnectionVia::Relay {
        detail = format!("{detail} {NETWORK_BLOCKING_HINT}");
    }
    let mut failure = ConnectionFailure::new(kind, detail);
    failure.via = Some(via);
    failure.hop = Some(
        if matches!(
            error,
            E::SessionPending | E::CredentialMissing | E::Proof | E::Storage | E::SessionChanged
        ) {
            ConnectionHop::Device
        } else if unreachable && via == ConnectionVia::Relay {
            ConnectionHop::Tunnel
        } else {
            ConnectionHop::Server
        },
    );
    if let E::HttpStatus(status) = error {
        failure.http_status = Some(*status);
    }
    failure
}

fn map_declared_remote_error(
    status: u16,
    error: &t3_contracts::environment_http::EnvironmentHttpCommonError,
    via: ConnectionVia,
) -> ConnectionFailure {
    use t3_contracts::environment_http::EnvironmentHttpCommonError as E;
    let (kind, detail, trace) = match error {
        E::EnvironmentConnectApprovalPendingError(e) => (
            FailureKind::ApprovalPending,
            "This device is waiting for approval on the host.",
            &e.trace_id,
        ),
        E::EnvironmentAuthInvalidError(e) => (
            FailureKind::Authentication,
            "The environment credential is invalid.",
            &e.trace_id,
        ),
        E::EnvironmentScopeRequiredError(e) => (
            FailureKind::Permission,
            "The environment credential does not grant the required access.",
            &e.trace_id,
        ),
        E::EnvironmentOperationForbiddenError(e) => (
            FailureKind::Permission,
            "The environment credential does not grant the required access.",
            &e.trace_id,
        ),
        E::EnvironmentRequestInvalidError(e) => (
            FailureKind::Configuration,
            "The environment rejected the authentication request.",
            &e.trace_id,
        ),
        E::EnvironmentResourceNotFoundError(e) => (
            FailureKind::Configuration,
            "The environment endpoint could not be found.",
            &e.trace_id,
        ),
        E::EnvironmentInternalError(e) => (
            FailureKind::RemoteUnavailable,
            "The environment could not authorize the connection.",
            &e.trace_id,
        ),
    };
    let mut failure = ConnectionFailure::new(kind, detail);
    failure.http_status = Some(status);
    failure.trace_id = Some(trace.0.clone());
    failure.hop = Some(ConnectionHop::Server);
    failure.via = Some(via);
    if failure.kind == FailureKind::ApprovalPending {
        failure.cause = Some(FailureCause::ApprovalPending);
    }
    failure
}

pub fn map_remote_dpop_environment_error(
    error: &crate::authorization::AuthorizationError,
) -> ConnectionFailure {
    let mut failure = map_remote_environment_error(error, ConnectionVia::Relay);
    if let crate::authorization::AuthorizationError::Remote { details, .. } = error
        && let t3_contracts::environment_http::EnvironmentHttpCommonError::EnvironmentAuthInvalidError(error) = details.expose().as_ref()
        && error.reason == t3_contracts::environment_http::EnvironmentAuthInvalidReason::InvalidCredential {
                let hint = match error.dpop_failure_reason {
                    Some(t3_contracts::base_schemas::DpopFailureReason::TimeWindow) => { failure.cause = Some(FailureCause::ClockSkew); crate::relay::DPOP_CLOCK_HINT },
                    None => crate::relay::DPOP_UNKNOWN_HINT,
                    _ => crate::relay::DPOP_RETRY_HINT,
                };
                failure.detail = format!("{} {hint}", failure.detail);
    }
    failure
}

pub fn map_managed_relay_error(error: &crate::relay::RelayError) -> ConnectionFailure {
    use crate::relay::RelayError as E;
    use t3_contracts::relay::*;
    let mut failure = match error {
        E::Request {
            error: Some(error), ..
        } => {
            let error = error.expose();
            let mut failure = ConnectionFailure::new(
                FailureKind::RelayUnavailable,
                crate::relay::relay_protected_error_message(error),
            );
            failure.hop = Some(ConnectionHop::Relay);
            match error {
                RelayProtectedError::RelayAuthInvalidError(e) => {
                    failure.kind = FailureKind::Authentication;
                    failure.trace_id = Some(e.trace_id.0.clone());
                    if e.dpop_failure_reason
                        == Some(t3_contracts::base_schemas::DpopFailureReason::TimeWindow)
                    {
                        failure.cause = Some(FailureCause::ClockSkew);
                    }
                }
                RelayProtectedError::RelayEnvironmentLinkProofExpiredError(e) => {
                    failure.kind = FailureKind::Authentication;
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayAgentActivityPublishProofExpiredError(e) => {
                    failure.kind = FailureKind::Authentication;
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayAgentActivityPublishProofInvalidError(e) => {
                    failure.kind = FailureKind::Authentication;
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayEnvironmentConnectNotAuthorizedError(e) => {
                    failure.trace_id = Some(e.trace_id.0.clone());
                    failure.kind = FailureKind::Permission;
                    match e.reason.flatten() {
                        Some(RelayEnvironmentConnectNotAuthorizedReason::ManagedEndpointAllocationNotReady) => { failure.kind = FailureKind::EndpointUnavailable; failure.hop = Some(ConnectionHop::Tunnel); failure.cause = Some(FailureCause::TunnelPending); }
                        Some(RelayEnvironmentConnectNotAuthorizedReason::EndpointProviderNotManaged) => { failure.hop = Some(ConnectionHop::Tunnel); failure.cause = Some(FailureCause::TunnelNotEnabled); }
                        Some(RelayEnvironmentConnectNotAuthorizedReason::EnvironmentLinkNotFound) => failure.cause = Some(FailureCause::NotLinked),
                        _ => {}
                    }
                }
                RelayProtectedError::RelayEnvironmentEndpointTimedOutError(e) => {
                    failure.kind = FailureKind::Timeout;
                    failure.hop = Some(ConnectionHop::Tunnel);
                    failure.cause = Some(FailureCause::Endpoint(
                        RelayEnvironmentEndpointFailure::Timeout,
                    ));
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayEnvironmentEndpointUnavailableError(e) => {
                    let cause = e.endpoint_failure.or((e.reason
                        == RelayEnvironmentEndpointUnavailableReason::EndpointResponseInvalid)
                        .then_some(RelayEnvironmentEndpointFailure::InvalidResponse));
                    failure.kind = FailureKind::EndpointUnavailable;
                    failure.hop = Some(
                        if matches!(
                            cause,
                            Some(
                                RelayEnvironmentEndpointFailure::OriginUnreachable
                                    | RelayEnvironmentEndpointFailure::InvalidResponse
                            )
                        ) {
                            ConnectionHop::Server
                        } else {
                            ConnectionHop::Tunnel
                        },
                    );
                    failure.via = Some(ConnectionVia::Relay);
                    failure.cause = cause.map(FailureCause::Endpoint);
                    failure.http_status = e.endpoint_status.as_ref().and_then(encoded_http_status);
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayEnvironmentLinkUnavailableError(e) => {
                    failure.kind = FailureKind::EndpointUnavailable;
                    failure.hop = Some(ConnectionHop::Tunnel);
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayEnvironmentLinkProofInvalidError(e) => {
                    failure.kind = FailureKind::Permission;
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayEnvironmentLinkLimitExceededError(e) => {
                    failure.kind = FailureKind::Permission;
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayEnvironmentLinkFailedError(e) => {
                    failure.cause = Some(FailureCause::RelayError);
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
                RelayProtectedError::RelayInternalError(e) => {
                    failure.cause = Some(FailureCause::RelayError);
                    failure.trace_id = Some(e.trace_id.0.clone());
                }
            }
            failure
        }
        _ => {
            let kind = match error {
                E::Timeout => FailureKind::Timeout,
                E::InvalidUrl | E::Storage | E::EnvironmentMismatch => FailureKind::Configuration,
                E::Key | E::Proof | E::SessionChanged | E::SignedOut => FailureKind::Authentication,
                E::UnexpectedScopes => FailureKind::Permission,
                _ => FailureKind::RelayUnavailable,
            };
            let mut failure = ConnectionFailure::new(kind, error.to_string());
            failure.hop = Some(if matches!(error, E::Key | E::Proof) {
                ConnectionHop::Device
            } else {
                ConnectionHop::Relay
            });
            if let E::Request {
                status, trace_id, ..
            } = error
            {
                failure.http_status = Some(*status);
                failure.cause = Some(FailureCause::RelayError);
                failure.trace_id = trace_id.as_ref().map(|id| id.expose().clone());
            }
            failure
        }
    };
    if failure.trace_id.is_none()
        && let E::Request { trace_id, .. } = error
    {
        failure.trace_id = trace_id.as_ref().map(|id| id.expose().clone());
    }
    failure
}
fn encoded_http_status(value: &t3_contracts::auth::EncodedNumber) -> Option<u16> {
    match value {
        t3_contracts::auth::EncodedNumber::Finite(n) => {
            n.as_u64().and_then(|n| u16::try_from(n).ok())
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopResult {
    Ok,
    Failed,
    Unchecked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosisHop {
    pub hop: ConnectionHop,
    pub label: String,
    pub result: HopResult,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionDiagnosis {
    pub headline: String,
    pub hops: Vec<DiagnosisHop>,
    pub steps: Vec<String>,
    pub facts: Vec<(String, String)>,
    pub more_info_url: Option<String>,
}
pub struct DiagnosisInput<'a> {
    pub host_label: &'a str,
    pub state: &'a ConnectionState,
    pub via: ConnectionVia,
    pub relay_status: Option<&'a t3_contracts::relay::RelayEnvironmentStatusResponse>,
    pub client_version: Option<&'a str>,
    pub server_version: Option<&'a str>,
}
#[derive(Clone)]
enum LocatedCause {
    Offline,
    Outdated,
    Released,
    SessionPending,
    Failure(FailureCause),
}
struct Located {
    hop: ConnectionHop,
    cause: Option<LocatedCause>,
    guessed: bool,
}
fn locate_failure(input: &DiagnosisInput<'_>) -> Option<Located> {
    use ConnectionHop as H;
    use LocatedCause as C;
    if input.state.network == NetworkStatus::Offline || input.state.phase == Phase::Offline {
        return Some(Located {
            hop: H::Device,
            cause: Some(C::Offline),
            guessed: false,
        });
    }
    if let Some(failure) = &input.state.last_failure {
        if failure.server_update_required {
            return Some(Located {
                hop: H::Server,
                cause: Some(C::Outdated),
                guessed: false,
            });
        }
        if failure.kind == FailureKind::SessionPending {
            return Some(Located {
                hop: H::Device,
                cause: Some(C::SessionPending),
                guessed: false,
            });
        }
        if let Some(hop) = failure.hop {
            return Some(Located {
                hop,
                cause: failure.cause.clone().map(C::Failure),
                guessed: false,
            });
        }
        return Some(Located {
            hop: if matches!(
                failure.kind,
                FailureKind::Network | FailureKind::Timeout | FailureKind::Transport
            ) && failure.via.unwrap_or(input.via) == ConnectionVia::Relay
            {
                H::Tunnel
            } else {
                H::Server
            },
            cause: None,
            guessed: true,
        });
    }
    let status = input
        .relay_status
        .filter(|s| s.status == t3_contracts::relay::RelayEnvironmentStatusValue::Offline)?;
    if status.offline_reason.flatten().is_some() {
        return Some(Located {
            hop: H::Tunnel,
            cause: Some(C::Released),
            guessed: false,
        });
    }
    let cause = status.endpoint_failure.flatten();
    let hop = if matches!(
        cause,
        Some(
            t3_contracts::relay::RelayEnvironmentEndpointFailure::OriginUnreachable
                | t3_contracts::relay::RelayEnvironmentEndpointFailure::InvalidResponse
        )
    ) {
        H::Server
    } else {
        H::Tunnel
    };
    Some(Located {
        hop,
        cause: cause.map(|c| C::Failure(FailureCause::Endpoint(c))),
        guessed: false,
    })
}
impl LocatedCause {
    fn key(&self) -> &'static str {
        use t3_contracts::relay::RelayEnvironmentEndpointFailure as E;
        match self {
            Self::Offline => "offline",
            Self::Outdated => "outdated",
            Self::Released => "released",
            Self::SessionPending => "session_pending",
            Self::Failure(FailureCause::ClockSkew) => "clock_skew",
            Self::Failure(FailureCause::NotLinked) => "not_linked",
            Self::Failure(FailureCause::RelayError) => "relay_error",
            Self::Failure(FailureCause::TunnelPending) => "tunnel_pending",
            Self::Failure(FailureCause::TunnelNotEnabled) => "tunnel_not_enabled",
            Self::Failure(FailureCause::ApprovalPending) => "approval_pending",
            Self::Failure(FailureCause::Endpoint(cause)) => match cause {
                E::TunnelDown => "tunnel_down",
                E::OriginUnreachable => "origin_unreachable",
                E::Dns => "dns",
                E::Tls => "tls",
                E::Refused => "refused",
                E::Timeout => "timeout",
                E::InvalidResponse => "invalid_response",
                E::Unknown => "unknown",
            },
        }
    }
}
impl FailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::EndpointUnavailable => "endpoint-unavailable",
            Self::RelayUnavailable => "relay-unavailable",
            Self::RemoteUnavailable => "remote-unavailable",
            Self::SessionPending => "session-pending",
            Self::Authentication => "authentication",
            Self::Configuration => "configuration",
            Self::Permission => "permission",
            Self::Unsupported => "unsupported",
            Self::ApprovalPending => "approval-pending",
        }
    }
}
pub fn present_connection_diagnosis(input: DiagnosisInput<'_>) -> Option<ConnectionDiagnosis> {
    use ConnectionHop as H;
    let located = locate_failure(&input)?;
    let host = input.host_label;
    let failure = input.state.last_failure.as_ref();
    let cause = located.cause.as_ref().map(LocatedCause::key);
    let on_this_device = failure.is_some_and(|f| {
        matches!(
            f.kind,
            FailureKind::Network | FailureKind::Transport | FailureKind::Timeout
        )
    });
    let auth = failure.is_some_and(|f| {
        matches!(
            f.kind,
            FailureKind::Authentication | FailureKind::Permission
        )
    });
    let relay_auth = failure.is_some_and(|f| f.kind == FailureKind::Authentication);
    let headline = match cause {
        Some("offline") => "This device is offline".into(),
        Some("session_pending") => "This device is still loading its T3 Connect sign-in".into(),
        Some("approval_pending") => format!("{host} is waiting for someone to approve this device"),
        Some("outdated") => format!("{host} runs an older T3 Code"),
        Some("clock_skew") => "This device's clock is off".into(),
        Some("not_linked") => format!("{host} is not linked to your T3 Connect account"),
        Some("relay_error") => "T3 Connect returned an error".into(),
        Some("tunnel_pending") => format!("{host}'s tunnel is still being set up"),
        Some("tunnel_not_enabled") => format!("T3 Connect is not turned on for {host}"),
        Some("released" | "tunnel_down") => format!("{host}'s tunnel is offline"),
        Some("origin_unreachable") => format!("Tunnel works, but T3 Code on {host} did not answer"),
        Some("invalid_response") => format!("T3 Code on {host} sent an unexpected answer"),
        Some("dns") => format!("{host}'s tunnel address did not resolve"),
        Some("tls") => format!("Secure connection to {host}'s tunnel failed"),
        Some("refused") => format!("{host}'s tunnel refused the connection"),
        Some("timeout") => format!("{host} did not answer in time"),
        _ => match located.hop {
            H::Device => "This device could not prepare the connection".into(),
            H::Relay if relay_auth => "T3 Connect did not accept this device's sign-in".into(),
            H::Relay => "This device can't reach T3 Connect".into(),
            H::Tunnel if on_this_device => format!("This device can't reach {host}'s tunnel"),
            H::Tunnel => format!("T3 Connect could not reach {host}'s tunnel"),
            H::Server if auth => format!("T3 Code on {host} did not accept this device"),
            H::Server => format!("T3 Code on {host} did not answer"),
        },
    };
    let check_host =
        format!("On {host}, check Settings > Sharing & access or run `t3 connect status`.");
    let steps: Vec<String> = match cause {
        Some("offline") => {
            vec!["Connect this device to the internet. T3 Code reconnects on its own.".into()]
        }
        Some("session_pending") => vec![
            "Wait a moment. T3 Code retries on its own.".into(),
            "If it keeps waiting, reload or restart T3 Code on this device.".into(),
        ],
        Some("approval_pending") => {
            let code = failure.and_then(|f| f.verification_code.as_deref());
            let approve = code.map_or_else(
                || "approve this device".to_owned(),
                |code| format!("approve the device showing {code}"),
            );
            vec![
                format!(
                    "On a device already connected to {host}, open Settings > Sharing & access and {approve}."
                ),
                format!(
                    "Or run `t3 auth connect-device approve {}` on {host}.",
                    code.unwrap_or("<code>")
                ),
            ]
        }
        Some("outdated") => vec![format!("Update T3 Code on {host}.")],
        Some("clock_skew") => vec![format!(
            "Turn on automatic date and time on this device and on {host}, then retry."
        )],
        Some("not_linked") => vec![
            "Check that this device is signed in to the same T3 Connect account as the host."
                .into(),
            format!("On {host}, open Settings > Sharing & access and turn T3 Connect on."),
        ],
        Some("tunnel_pending") => vec![
            "Wait a minute. T3 Code retries on its own.".into(),
            check_host,
        ],
        Some("relay_error") => vec![
            "Wait a few minutes. T3 Code retries on its own.".into(),
            "If it keeps failing, copy the diagnostics and report it.".into(),
        ],
        Some("tunnel_not_enabled") => vec![format!(
            "On {host}, open Settings > Sharing & access and turn T3 Connect on."
        )],
        Some("released" | "tunnel_down") => vec![
            format!("Make sure {host} is awake and T3 Code is running on it."),
            check_host,
            format!(
                "A VPN or Tailscale exit node on {host} can block the tunnel (port 7844). Turn it off or allow the tunnel."
            ),
        ],
        Some("origin_unreachable" | "invalid_response") => {
            vec![format!("Open or restart T3 Code on {host}."), check_host]
        }
        Some("dns" | "tls" | "refused" | "timeout" | "unknown") => {
            vec![format!("Make sure {host} is awake and online."), check_host]
        }
        _ => match located.hop {
            H::Device => vec![
                "Restart T3 Code on this device.".into(),
                format!("If it keeps failing, remove {host} from this device and add it again."),
            ],
            H::Relay if relay_auth => vec!["Sign out of T3 Connect and sign in again.".into()],
            H::Relay => vec![
                "Check this device's internet connection.".into(),
                "A VPN, firewall, or ad blocker on this device can block T3 Connect.".into(),
            ],
            H::Tunnel if on_this_device => vec![
                "A VPN, firewall, or DNS filter on this device can block the tunnel.".into(),
                "Try another network, or add a direct route such as a Tailscale address.".into(),
            ],
            H::Tunnel => vec![format!("Make sure {host} is awake and online."), check_host],
            H::Server => vec![
                format!("Make sure T3 Code is running on {host}."),
                "Retry. If it keeps failing, copy the diagnostics.".into(),
            ],
        },
    };
    let relay_path = matches!(located.hop, H::Relay | H::Tunnel)
        || (failure.is_none()
            && input.relay_status.is_some_and(|s| {
                s.status == t3_contracts::relay::RelayEnvironmentStatusValue::Offline
            }))
        || failure.and_then(|f| f.via).unwrap_or(input.via) == ConnectionVia::Relay;
    let order = if relay_path {
        vec![H::Device, H::Relay, H::Tunnel, H::Server]
    } else {
        vec![H::Device, H::Server]
    };
    let failed_index = order.iter().position(|h| *h == located.hop);
    let hops = order
        .into_iter()
        .enumerate()
        .map(|(index, hop)| DiagnosisHop {
            hop,
            label: match hop {
                H::Device => "This device".into(),
                H::Relay => "T3 Connect".into(),
                H::Tunnel => format!("{host} tunnel"),
                H::Server => format!("T3 Code on {host}"),
            },
            result: if failed_index == Some(index) {
                HopResult::Failed
            } else if failed_index.is_some_and(|failed| index < failed) && !located.guessed {
                HopResult::Ok
            } else {
                HopResult::Unchecked
            },
        })
        .collect();
    let section = match cause {
        Some("offline") => "this-device-cant-reach-t3-connect",
        Some("outdated") => "the-host-runs-an-older-t3-code",
        Some("clock_skew") => "the-clock-is-off",
        Some("approval_pending" | "session_pending" | "not_linked" | "tunnel_not_enabled") => {
            "sign-in-account-or-link-problems"
        }
        Some("relay_error") => "t3-connect-returned-an-error",
        Some("released" | "tunnel_down" | "tunnel_pending") => "the-hosts-tunnel-is-offline",
        Some("origin_unreachable" | "invalid_response") => {
            "the-tunnel-works-but-t3-code-does-not-answer"
        }
        Some("dns" | "tls" | "refused" | "timeout" | "unknown") => {
            "the-t3-connect-address-does-not-resolve-or-answer"
        }
        _ => match located.hop {
            H::Device => "this-device-cant-reach-t3-connect",
            H::Relay if relay_auth => "sign-in-account-or-link-problems",
            H::Relay => "this-device-cant-reach-t3-connect",
            H::Tunnel if on_this_device => "this-device-cant-reach-t3-connect",
            H::Tunnel => "the-hosts-tunnel-is-offline",
            H::Server if auth => "sign-in-account-or-link-problems",
            H::Server => "the-tunnel-works-but-t3-code-does-not-answer",
        },
    };
    let mut facts = Vec::new();
    let mut add = |label: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            facts.push((label.into(), value));
        }
    };
    add(
        "Cause",
        cause
            .map(str::to_owned)
            .or_else(|| failure.map(|f| f.kind.as_str().into())),
    );
    add(
        "HTTP status",
        if let Some(failure) = failure {
            failure.http_status
        } else {
            input
                .relay_status
                .and_then(|s| s.endpoint_status.as_ref().and_then(Option::as_ref))
                .and_then(encoded_http_status)
        }
        .map(|s| s.to_string()),
    );
    add(
        "Attempts",
        (input.state.attempt > 0).then(|| input.state.attempt.to_string()),
    );
    add(
        "Next retry",
        input.state.retry_at.map(crate::time::iso_from_epoch_ms),
    );
    add(
        "Route",
        Some(if relay_path { "T3 Connect" } else { "Direct" }.into()),
    );
    add("App version", input.client_version.map(str::to_owned));
    add("Host version", input.server_version.map(str::to_owned));
    add("Error", failure.map(|f| f.detail.clone()));
    add("Trace ID", failure.and_then(|f| f.trace_id.clone()));
    let more_info_url = (relay_path || cause == Some("session_pending")).then(|| format!("https://github.com/pingdotgg/t3code/blob/main/docs/user/t3-connect-troubleshooting.md#{section}"));
    Some(ConnectionDiagnosis {
        headline,
        hops,
        steps,
        facts,
        more_info_url,
    })
}
pub fn connection_diagnosis_text(host: &str, diagnosis: &ConnectionDiagnosis) -> String {
    let mut lines = vec![
        format!("T3 Code connection to {host}: {}", diagnosis.headline),
        String::new(),
    ];
    lines.extend(diagnosis.hops.iter().map(|h| {
        format!(
            "{}: {}",
            h.label,
            match h.result {
                HopResult::Ok => "ok",
                HopResult::Failed => "FAILED",
                HopResult::Unchecked => "not checked",
            }
        )
    }));
    lines.push(String::new());
    lines.extend(
        diagnosis
            .facts
            .iter()
            .map(|(label, value)| format!("{label}: {value}")),
    );
    if let Some(url) = &diagnosis.more_info_url {
        lines.push(String::new());
        lines.push(format!("More info: {url}"));
    }
    lines.join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileAuthorization {
    #[serde(rename = "t3-connect")]
    T3Connect,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileNetwork {
    #[serde(rename = "tailscale")]
    Tailscale,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ConnectionProfile {
    #[serde(rename = "BearerConnectionProfile", rename_all = "camelCase")]
    Bearer {
        connection_id: String,
        environment_id: EnvironmentId,
        label: String,
        http_base_url: String,
        ws_base_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        learned: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authorization: Option<ProfileAuthorization>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        network: Option<ProfileNetwork>,
    },
    #[serde(rename = "SshConnectionProfile", rename_all = "camelCase")]
    Ssh {
        connection_id: String,
        environment_id: EnvironmentId,
        label: String,
        target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
    },
}
impl ConnectionProfile {
    pub fn connection_id(&self) -> &str {
        match self {
            Self::Bearer { connection_id, .. } | Self::Ssh { connection_id, .. } => connection_id,
        }
    }
    pub fn environment_id(&self) -> &EnvironmentId {
        match self {
            Self::Bearer { environment_id, .. } | Self::Ssh { environment_id, .. } => {
                environment_id
            }
        }
    }
    fn valid(&self) -> bool {
        !matches!(
            self,
            Self::Bearer {
                learned: Some(false),
                ..
            }
        ) && !self.connection_id().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ConnectionCredential {
    #[serde(rename = "BearerConnectionCredential")]
    Bearer {
        token: crate::authorization::Sensitive<String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConnectionCredential {
    connection_id: String,
    credential: ConnectionCredential,
}
impl ConnectionCredential {
    pub fn token(&self) -> &crate::authorization::Sensitive<String> {
        match self {
            Self::Bearer { token } => token,
        }
    }
}
#[derive(Clone)]
pub struct ConnectionStores {
    platform: Rc<dyn Platform>,
}
impl ConnectionStores {
    pub fn new(platform: Rc<dyn Platform>) -> Self {
        Self { platform }
    }
    fn key(
        namespace: &str,
        _environment: &EnvironmentId,
        connection: &str,
    ) -> crate::platform::StorageKey {
        crate::platform::StorageKey {
            namespace: namespace.into(),
            environment: None,
            record: connection.into(),
        }
    }
    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        area: crate::platform::StorageArea,
        namespace: &str,
        environment: &EnvironmentId,
        connection: &str,
    ) -> Result<Option<T>, ConnectionFailure> {
        let key = Self::key(namespace, environment, connection);
        let bytes = self
            .platform
            .get(area, key.clone())
            .await
            .map_err(map_connection_platform_error)?;
        bytes
            .map(|bytes| {
                crate::persistence::decode_record::<T>(&bytes, &key, 1)
                    .map(|r| r.payload)
                    .map_err(|_| {
                        ConnectionFailure::new(
                            FailureKind::Configuration,
                            "Invalid saved connection record; original bytes retained.",
                        )
                    })
            })
            .transpose()
    }
    pub async fn profile(
        &self,
        environment: &EnvironmentId,
        connection: &str,
    ) -> Result<Option<ConnectionProfile>, ConnectionFailure> {
        let profile: Option<ConnectionProfile> = self
            .read(
                crate::platform::StorageArea::Secure,
                "connection-profile",
                environment,
                connection,
            )
            .await?;
        if profile.as_ref().is_some_and(|p| {
            !p.valid() || p.environment_id() != environment || p.connection_id() != connection
        }) {
            return Err(profile_missing_error(connection));
        }
        Ok(profile)
    }
    pub async fn credential(
        &self,
        environment: &EnvironmentId,
        connection: &str,
    ) -> Result<Option<ConnectionCredential>, ConnectionFailure> {
        let stored: Option<StoredConnectionCredential> = self
            .read(
                crate::platform::StorageArea::Secure,
                "connection-credential",
                environment,
                connection,
            )
            .await?;
        match stored {
            Some(stored) if stored.connection_id == connection => Ok(Some(stored.credential)),
            Some(_) => Err(credential_missing_error(connection)),
            None => Ok(None),
        }
    }
    pub async fn put_profile(&self, profile: ConnectionProfile) -> Result<(), ConnectionFailure> {
        self.register(profile, None).await
    }
    pub async fn register(
        &self,
        profile: ConnectionProfile,
        credential: Option<ConnectionCredential>,
    ) -> Result<(), ConnectionFailure> {
        use crate::platform::{StorageTransaction, StorageWrite};
        if !profile.valid() {
            return Err(profile_missing_error(profile.connection_id()));
        }
        let key = Self::key(
            "connection-profile",
            profile.environment_id(),
            profile.connection_id(),
        );
        let value = crate::persistence::Record::new(
            &key.namespace,
            key.environment.clone(),
            &key.record,
            1,
            &profile,
        )
        .encode()
        .map_err(|_| {
            ConnectionFailure::new(
                FailureKind::Configuration,
                "Could not encode connection profile.",
            )
        })?;
        let mut transaction = StorageTransaction {
            secure: vec![StorageWrite {
                key,
                value: Some(value),
            }],
            ..StorageTransaction::default()
        };
        if let Some(credential) = credential {
            let key = Self::key(
                "connection-credential",
                profile.environment_id(),
                profile.connection_id(),
            );
            let value = crate::persistence::Record::new(
                &key.namespace,
                key.environment.clone(),
                &key.record,
                1,
                StoredConnectionCredential {
                    connection_id: profile.connection_id().into(),
                    credential,
                },
            )
            .encode()
            .map_err(|_| {
                ConnectionFailure::new(
                    FailureKind::Configuration,
                    "Could not encode connection credential.",
                )
            })?;
            transaction.secure.push(StorageWrite {
                key,
                value: Some(value),
            });
        }
        if !self
            .platform
            .transaction(transaction)
            .await
            .map_err(map_connection_platform_error)?
        {
            return Err(ConnectionFailure::new(
                FailureKind::Configuration,
                "Connection records changed concurrently.",
            ));
        }
        Ok(())
    }
    pub async fn put_credential(
        &self,
        environment: &EnvironmentId,
        connection: &str,
        credential: ConnectionCredential,
    ) -> Result<(), ConnectionFailure> {
        let key = Self::key("connection-credential", environment, connection);
        let value = crate::persistence::Record::new(
            &key.namespace,
            key.environment.clone(),
            &key.record,
            1,
            StoredConnectionCredential {
                connection_id: connection.into(),
                credential,
            },
        )
        .encode()
        .map_err(|_| {
            ConnectionFailure::new(
                FailureKind::Configuration,
                "Could not encode connection credential.",
            )
        })?;
        self.platform
            .write(
                crate::platform::StorageArea::Secure,
                vec![crate::platform::StorageWrite {
                    key,
                    value: Some(value),
                }],
            )
            .await
            .map_err(map_connection_platform_error)
    }
    pub async fn remove(
        &self,
        environment: &EnvironmentId,
        connection: &str,
    ) -> Result<(), ConnectionFailure> {
        use crate::platform::{StorageTransaction, StorageWrite};
        let transaction = StorageTransaction {
            secure: vec![
                StorageWrite {
                    key: Self::key("connection-profile", environment, connection),
                    value: None,
                },
                StorageWrite {
                    key: Self::key("connection-credential", environment, connection),
                    value: None,
                },
            ],
            ..StorageTransaction::default()
        };
        if !self
            .platform
            .transaction(transaction)
            .await
            .map_err(map_connection_platform_error)?
        {
            return Err(ConnectionFailure::new(
                FailureKind::Configuration,
                "Connection records changed concurrently.",
            ));
        }
        Ok(())
    }
}

fn map_connection_platform_error(error: crate::platform::PlatformError) -> ConnectionFailure {
    use crate::platform::PlatformError as E;
    let kind = match error {
        E::Unsupported(_) | E::RandomUnavailable => FailureKind::Unsupported,
        E::Denied(_) => FailureKind::Permission,
        E::Storage(_) | E::InvalidData(_) | E::TooLarge => FailureKind::Configuration,
        E::Network(_) => FailureKind::Network,
        E::Closed | E::Cancelled => FailureKind::Transport,
    };
    ConnectionFailure::new(
        kind,
        match error {
            E::Unsupported(_) => "This client does not support this connection capability.",
            E::RandomUnavailable => "This client has no secure random source.",
            E::Denied(_) => "Permission to use this connection capability was denied.",
            E::Storage(_) => "Could not access saved connection data.",
            E::Network(_) => "Could not reach the environment endpoint.",
            E::Closed => "The connection closed.",
            E::Cancelled => "The connection operation was cancelled.",
            E::InvalidData(_) | E::TooLarge => "The connection capability returned invalid data.",
        },
    )
}

#[derive(Clone)]
pub struct ConnectionResolver {
    platform: Rc<dyn Platform>,
    pub(crate) remote: crate::authorization::RemoteAuthorization,
    stores: ConnectionStores,
}

#[derive(Clone, Default)]
pub struct PairingInput {
    pub pairing_url: Option<String>,
    pub host: Option<String>,
    pub pairing_code: Option<String>,
    pub expected_environment: Option<EnvironmentId>,
}
pub struct ResolvedPairingTarget {
    pub credential: crate::authorization::Sensitive<String>,
    pub http_base: String,
    pub ws_base: String,
}
pub struct PreparedRegistration {
    pub registration: crate::environment::EnvironmentRegistration,
    pub profile: ConnectionProfile,
    pub credential: ConnectionCredential,
}
pub fn resolve_pairing_target(
    input: &PairingInput,
) -> Result<ResolvedPairingTarget, ConnectionFailure> {
    let configuration = |detail| ConnectionFailure::new(FailureKind::Configuration, detail);
    let normalize_host = |host: &str| -> Result<String, ConnectionFailure> {
        let host = host.trim().trim_start_matches('/');
        if host.is_empty() {
            return Err(configuration("Enter a backend URL."));
        }
        let base = if host.contains("://") {
            host.into()
        } else {
            format!("https://{host}")
        };
        crate::environment::normalize_http_base_url(&base)
            .map_err(|_| configuration("Backend URL is invalid."))
    };
    let pairing = input.pairing_url.as_deref().unwrap_or("").trim();
    let (http_base, credential) = if !pairing.is_empty() {
        let url = Url::parse(pairing).map_err(|_| configuration("Pairing URL is invalid."))?;
        if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
            return Err(configuration("Pairing URL is invalid."));
        }
        let from_pairs = |pairs: Vec<(String, String)>, key: &str| {
            pairs
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let query: Vec<_> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let hash = url::form_urlencoded::parse(url.fragment().unwrap_or("").as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let credential = from_pairs(hash, "token")
            .or_else(|| from_pairs(query.clone(), "token"))
            .ok_or_else(|| configuration("Pairing URL is missing its token."))?;
        let base = if let Some(host) = from_pairs(query, "host") {
            normalize_host(&host)?
        } else {
            crate::environment::normalize_http_base_url(pairing)
                .map_err(|_| configuration("Pairing URL is invalid."))?
        };
        (base, credential)
    } else {
        let base = normalize_host(input.host.as_deref().unwrap_or(""))?;
        let credential = input.pairing_code.as_deref().unwrap_or("").trim();
        if credential.is_empty() {
            return Err(configuration("Enter a pairing code."));
        }
        (base, credential.into())
    };
    let ws_base = crate::environment::derive_ws_base_url(&http_base)
        .map_err(|_| configuration("Backend URL is invalid."))?;
    Ok(ResolvedPairingTarget {
        credential: crate::authorization::Sensitive(credential),
        http_base,
        ws_base,
    })
}
pub async fn prepare_pairing_registration(
    platform: &dyn Platform,
    input: &PairingInput,
    assert_known_address: impl Fn(&Route) -> Result<(), ConnectionFailure>,
) -> Result<PreparedRegistration, ConnectionFailure> {
    let target = resolve_pairing_target(input)?;
    let descriptor =
        crate::environment::fetch_remote_environment_descriptor(platform, &target.http_base, None)
            .await
            .map_err(|_| {
                ConnectionFailure::new(
                    FailureKind::RemoteUnavailable,
                    "Could not discover the environment descriptor.",
                )
            })?;
    if input
        .expected_environment
        .as_ref()
        .is_some_and(|id| id != &descriptor.environment_id)
    {
        return Err(ConnectionFailure::new(
            FailureKind::Configuration,
            format!(
                "That address reaches {}, a different machine. Add it as its own environment instead.",
                descriptor.label.0
            ),
        ));
    }
    let origin = Url::parse(&target.http_base)
        .map_err(|_| ConnectionFailure::new(FailureKind::Configuration, "Backend URL is invalid."))?
        .origin()
        .ascii_serialization();
    let id = format!("bearer:{}:{origin}", descriptor.environment_id.0);
    let route = Route {
        id: id.clone(),
        environment: descriptor.environment_id.clone(),
        mode: ConnectionMode::Remote,
        http_base: target.http_base.clone(),
        ws_base: target.ws_base.clone(),
    };
    if input.expected_environment.is_none() {
        assert_known_address(&route)?;
    }
    if let Some(failure) = orchestration_protocol_compatibility_error(&descriptor)
        && !failure.server_update_required
    {
        return Err(failure);
    }
    let metadata = platform
        .client_presentation()
        .await
        .map_err(map_connection_platform_error)?;
    let access = crate::authorization::bootstrap_remote_session(
        platform,
        &target.http_base,
        target.credential.expose(),
        None,
        Some(&metadata),
        None,
    )
    .await
    .map_err(|error| map_remote_environment_error(&error, ConnectionVia::Direct))?
    .into_inner();
    Ok(PreparedRegistration {
        registration: crate::environment::EnvironmentRegistration {
            id: descriptor.environment_id.clone(),
            label: descriptor.label.0.clone(),
            routes: vec![route],
            enabled: true,
        },
        profile: ConnectionProfile::Bearer {
            connection_id: id,
            environment_id: descriptor.environment_id,
            label: descriptor.label.0,
            http_base_url: target.http_base,
            ws_base_url: target.ws_base,
            learned: None,
            authorization: None,
            network: None,
        },
        credential: ConnectionCredential::Bearer {
            token: crate::authorization::Sensitive(access.access_token.0),
        },
    })
}
pub fn prepare_bearer_update(
    environment: &EnvironmentId,
    label: &str,
    base: &str,
    saved: Option<&ConnectionProfile>,
    credential: Option<&ConnectionCredential>,
) -> Result<PreparedRegistration, ConnectionFailure> {
    let Some(ConnectionProfile::Bearer {
        connection_id,
        environment_id,
        ..
    }) = saved
    else {
        return Err(ConnectionFailure::new(
            FailureKind::Configuration,
            "Only saved bearer environments can be edited.",
        ));
    };
    if environment_id != environment {
        return Err(environment_mismatch_error(environment, environment_id));
    }
    let credential = credential
        .ok_or_else(|| {
            ConnectionFailure::new(
                FailureKind::Authentication,
                "The saved bearer credential is unavailable.",
            )
        })?
        .clone();
    let label = label.trim();
    if label.is_empty() {
        return Err(ConnectionFailure::new(
            FailureKind::Configuration,
            "Environment label cannot be empty.",
        ));
    }
    let http_base = crate::environment::normalize_http_base_url(base).map_err(|_| {
        ConnectionFailure::new(
            FailureKind::Configuration,
            "The environment URL is invalid.",
        )
    })?;
    let ws_base = crate::environment::derive_ws_base_url(&http_base).map_err(|_| {
        ConnectionFailure::new(
            FailureKind::Configuration,
            "The environment URL is invalid.",
        )
    })?;
    let route = Route {
        id: connection_id.clone(),
        environment: environment.clone(),
        mode: ConnectionMode::Remote,
        http_base: http_base.clone(),
        ws_base: ws_base.clone(),
    };
    Ok(PreparedRegistration {
        registration: crate::environment::EnvironmentRegistration {
            id: environment.clone(),
            label: label.into(),
            routes: vec![route],
            enabled: true,
        },
        profile: ConnectionProfile::Bearer {
            connection_id: connection_id.clone(),
            environment_id: environment.clone(),
            label: label.into(),
            http_base_url: http_base,
            ws_base_url: ws_base,
            learned: None,
            authorization: None,
            network: None,
        },
        credential,
    })
}
pub fn ssh_target_key(
    target: &t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
) -> Result<String, crate::ClientError> {
    Ok(serde_json::to_string(&(
        &target.alias,
        &target.hostname,
        &target.username,
        &target.port,
    ))?)
}
pub async fn prepare_ssh_registration(
    platform: &dyn Platform,
    target: t3_contracts::ipc::DesktopSshEnvironmentTargetSchema,
    label: Option<&str>,
    expected: Option<&EnvironmentId>,
) -> Result<PreparedRegistration, ConnectionFailure> {
    let provisioned = platform
        .ssh_provision(target, expected.cloned())
        .await
        .map_err(map_connection_platform_error)?;
    if expected.is_some_and(|id| *id != provisioned.environment_id) {
        return Err(environment_mismatch_error(
            expected.unwrap_or(&provisioned.environment_id),
            &provisioned.environment_id,
        ));
    }
    let id = format!(
        "ssh:{}:{}",
        provisioned.environment_id.0,
        ssh_target_key(&provisioned.bootstrap.target).map_err(|_| ConnectionFailure::new(
            FailureKind::Configuration,
            "Invalid SSH target."
        ))?
    );
    let label = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if provisioned.label.is_empty() {
                &provisioned.bootstrap.target.alias
            } else {
                &provisioned.label
            }
        })
        .to_owned();
    let route = Route {
        id: id.clone(),
        environment: provisioned.environment_id.clone(),
        mode: ConnectionMode::Ssh,
        http_base: String::new(),
        ws_base: String::new(),
    };
    Ok(PreparedRegistration {
        registration: crate::environment::EnvironmentRegistration {
            id: provisioned.environment_id.clone(),
            label: label.clone(),
            routes: vec![route],
            enabled: true,
        },
        profile: ConnectionProfile::Ssh {
            connection_id: id,
            environment_id: provisioned.environment_id,
            label,
            target: provisioned.bootstrap.target,
        },
        credential: ConnectionCredential::Bearer {
            token: crate::authorization::Sensitive(provisioned.bearer_token),
        },
    })
}
impl ConnectionResolver {
    pub fn new(
        platform: Rc<dyn Platform>,
        remote: crate::authorization::RemoteAuthorization,
    ) -> Self {
        Self {
            stores: ConnectionStores::new(platform.clone()),
            platform,
            remote,
        }
    }
    pub async fn prepare(
        &self,
        route: &Route,
        identity: &crate::environment::SessionIdentity,
    ) -> Result<crate::authorization::AuthorizedEnvironment, ConnectionFailure> {
        let mut prepared = self.prepare_for_update(route, identity).await?;
        if let Some(descriptor) = &prepared.descriptor
            && let Some(failure) = orchestration_protocol_compatibility_error(descriptor)
        {
            return Err(failure);
        }
        prepared.socket_url = crate::authorization::Sensitive(
            append_orchestration_protocol(prepared.socket_url.expose()).map_err(|_| {
                ConnectionFailure::new(FailureKind::Configuration, "Invalid prepared socket URL.")
            })?,
        );
        Ok(prepared)
    }
    pub async fn prepare_for_update(
        &self,
        route: &Route,
        identity: &crate::environment::SessionIdentity,
    ) -> Result<crate::authorization::AuthorizedEnvironment, ConnectionFailure> {
        use crate::authorization::{AuthorizedEnvironment, HttpAuthorization, Sensitive};
        let via = if route.mode == ConnectionMode::Relay {
            ConnectionVia::Relay
        } else {
            ConnectionVia::Direct
        };
        let auth_error = |error| map_remote_environment_error(&error, via);
        if route.environment != identity.environment {
            return Err(environment_mismatch_error(
                &identity.environment,
                &route.environment,
            ));
        }
        self.remote
            .ensure_session(identity)
            .await
            .map_err(auth_error)?;
        let mut prepared = match route.mode {
            ConnectionMode::Local => {
                let bearer = self
                    .platform
                    .primary_bearer()
                    .await
                    .map_err(map_connection_platform_error)?;
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(auth_error)?;
                if let Some(bearer) = bearer {
                    self.remote
                        .authorize_bearer(
                            identity,
                            &route.http_base,
                            &route.ws_base,
                            Sensitive(bearer),
                            "direct",
                        )
                        .await
                        .map_err(auth_error)?
                } else {
                    let mut url = Url::parse(&route.ws_base).map_err(|_| {
                        ConnectionFailure::new(
                            FailureKind::Configuration,
                            "Invalid primary socket URL.",
                        )
                    })?;
                    if url.path() == "/" || url.path().is_empty() {
                        url.set_path("/ws");
                    }
                    let metadata = self
                        .platform
                        .client_presentation()
                        .await
                        .map_err(map_connection_platform_error)?;
                    self.remote
                        .validate_session(identity)
                        .await
                        .map_err(auth_error)?;
                    let (owner, device) =
                        crate::authorization::read_client_owner_params(self.platform.as_ref())
                            .await;
                    self.remote
                        .validate_session(identity)
                        .await
                        .map_err(auth_error)?;
                    crate::authorization::append_client_connection_params(
                        &mut url,
                        Some(&metadata),
                        Some("direct"),
                        owner.as_deref(),
                        device.as_deref(),
                    )
                    .map_err(auth_error)?;
                    AuthorizedEnvironment {
                        session: identity.clone(),
                        http_base_url: route.http_base.clone(),
                        socket_url: Sensitive(url.into()),
                        authorization: HttpAuthorization::Cookie,
                        descriptor: None,
                    }
                }
            }
            ConnectionMode::Relay => self
                .remote
                .authorize_dpop(identity, None)
                .await
                .map_err(auth_error)?,
            ConnectionMode::Remote | ConnectionMode::Tunnel => {
                let profile = self
                    .stores
                    .profile(&route.environment, &route.id)
                    .await?
                    .ok_or_else(|| profile_missing_error(&route.id))?;
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(auth_error)?;
                let ConnectionProfile::Bearer {
                    http_base_url,
                    ws_base_url,
                    authorization,
                    ..
                } = profile
                else {
                    return Err(profile_missing_error(&route.id));
                };
                if authorization == Some(ProfileAuthorization::T3Connect) {
                    self.remote
                        .authorize_dpop(identity, Some((&http_base_url, &ws_base_url)))
                        .await
                        .map_err(auth_error)?
                } else {
                    let credential = self
                        .stores
                        .credential(&route.environment, credential_connection_id(&route.id))
                        .await?
                        .ok_or_else(|| credential_missing_error(&route.id))?;
                    self.remote
                        .validate_session(identity)
                        .await
                        .map_err(auth_error)?;
                    self.remote
                        .authorize_bearer(
                            identity,
                            &http_base_url,
                            &ws_base_url,
                            credential.token().clone(),
                            "direct",
                        )
                        .await
                        .map_err(auth_error)?
                }
            }
            ConnectionMode::Ssh => {
                let profile = self
                    .stores
                    .profile(&route.environment, &route.id)
                    .await?
                    .ok_or_else(|| profile_missing_error(&route.id))?;
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(auth_error)?;
                let ConnectionProfile::Ssh {
                    connection_id,
                    environment_id,
                    label,
                    target,
                } = profile
                else {
                    return Err(profile_missing_error(&route.id));
                };
                let saved = self
                    .stores
                    .credential(&route.environment, &route.id)
                    .await?;
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(auth_error)?;
                let prepared = self
                    .platform
                    .ssh_prepare(
                        route.id.clone(),
                        target.clone(),
                        route.environment.clone(),
                        saved.as_ref().map(|c| c.token().expose().clone()),
                    )
                    .await
                    .map_err(map_connection_platform_error)?;
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(auth_error)?;
                if saved
                    .as_ref()
                    .is_none_or(|c| c.token().expose() != &prepared.bearer_token)
                {
                    self.stores
                        .put_credential(
                            &route.environment,
                            &route.id,
                            ConnectionCredential::Bearer {
                                token: Sensitive(prepared.bearer_token.clone()),
                            },
                        )
                        .await?;
                    self.remote
                        .validate_session(identity)
                        .await
                        .map_err(auth_error)?;
                }
                if prepared.bootstrap.target != target {
                    PullRequestRoutingPermissions::new(self.platform.clone())
                        .forget(&route.environment)
                        .await?;
                    self.remote
                        .validate_session(identity)
                        .await
                        .map_err(auth_error)?;
                }
                self.stores
                    .put_profile(ConnectionProfile::Ssh {
                        connection_id,
                        environment_id,
                        label,
                        target: prepared.bootstrap.target,
                    })
                    .await?;
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(auth_error)?;
                self.remote
                    .authorize_bearer(
                        identity,
                        &prepared.bootstrap.http_base_url,
                        &prepared.bootstrap.ws_base_url,
                        Sensitive(prepared.bearer_token),
                        "ssh",
                    )
                    .await
                    .map_err(auth_error)?
            }
        };
        self.remote
            .validate_session(identity)
            .await
            .map_err(auth_error)?;
        if prepared.session != *identity {
            return Err(ConnectionFailure::new(
                FailureKind::Authentication,
                "Authorization session changed.",
            ));
        }
        if prepared.descriptor.is_none() {
            prepared.descriptor = Some(
                crate::environment::fetch_remote_environment_descriptor(
                    self.platform.as_ref(),
                    &prepared.http_base_url,
                    None,
                )
                .await
                .map_err(|_| {
                    ConnectionFailure::new(
                        FailureKind::RemoteUnavailable,
                        "Could not read the environment descriptor.",
                    )
                })?,
            );
            self.remote
                .validate_session(identity)
                .await
                .map_err(auth_error)?;
        }
        if prepared
            .descriptor
            .as_ref()
            .is_some_and(|d| d.environment_id != identity.environment)
        {
            return Err(ConnectionFailure::new(
                FailureKind::Configuration,
                "Connected environment identity does not match the saved environment.",
            ));
        }
        Ok(prepared)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionState {
    pub desired: bool,
    pub network: NetworkStatus,
    pub phase: Phase,
    pub stage: Option<AttemptStage>,
    pub attempt: u32,
    pub generation: Generation,
    pub last_failure: Option<ConnectionFailure>,
    pub retry_at: Option<u64>,
}
impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            desired: false,
            network: NetworkStatus::Unknown,
            phase: Phase::Available,
            stage: None,
            attempt: 0,
            generation: Generation(0),
            last_failure: None,
            retry_at: None,
        }
    }
}

pub const RETRY_BASE_DELAY_MS: u64 = 1_000;
pub const RETRY_MAX_DELAY_MS: u64 = 300_000;
pub const SESSION_LOSS_RETRY_DELAY_MS: u64 = 250;
pub const ESTABLISHMENT_TIMEOUT_MS: u64 = 15_000;
pub const SYNCHRONIZATION_TIMEOUT_MS: u64 = 75_000;
pub const CONNECTION_PROBE_TIMEOUT_MS: u64 = 15_000;
pub const QUICK_PROBE_TIMEOUT_MS: u64 = 3_000;
pub const LONG_RESUME_PROBE_TIMEOUT_MS: u64 = 1_500;
pub const SILENT_SOCKET_CHECK_AFTER_MS: u64 = 500;
pub const SILENT_SOCKET_RECHECK_MS: u64 = 1_000;
pub const REMOTE_SILENT_SOCKET_AFTER_MS: u64 = 2_500;
pub const BACKOFF_RESET_AFTER_MS: u64 = 30_000;
pub const BETTER_ROUTE_CHECK_INTERVAL_MS: u64 = 60_000;
pub const SILENT_ROUTE_BACKOFF_MAX_MS: u64 = 1_800_000;
pub const BETTER_ROUTE_COOLDOWN_MS: u64 = 300_000;
pub const ROUTE_CHECK_TIMEOUT_MS: u64 = 2_500;
pub const ROUTE_CHECK_HEAD_START_MS: u64 = 500;
pub const LONG_BACKGROUND_AFTER_MS: u64 = 10_000;

/// How often a device waiting for the host's approval checks back.
pub const APPROVAL_PENDING_RETRY_DELAY_MS: u64 = 4_000;
/// A pending request lives 10 minutes: 150 checks at 4 s.
pub const APPROVAL_PENDING_FAST_RETRIES: u32 = 150;

pub fn retry_delay_ms(failure_count: u32, random: f64) -> u64 {
    let ceiling = RETRY_BASE_DELAY_MS
        .saturating_mul(
            1_u64
                .checked_shl(failure_count.saturating_add(1))
                .unwrap_or(u64::MAX),
        )
        .min(RETRY_MAX_DELAY_MS);
    ((ceiling as f64 / 2.0) * (1.0 + unit_random(random))).round() as u64
}

pub fn session_loss_delay_ms(random: f64) -> u64 {
    (SESSION_LOSS_RETRY_DELAY_MS as f64 * (0.5 + unit_random(random) / 2.0)).round() as u64
}

fn unit_random(random: f64) -> f64 {
    if random.is_nan() {
        0.0
    } else {
        random.clamp(0.0, 1.0)
    }
}

impl ConnectionFailure {
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            http_status: None,
            trace_id: None,
            hop: None,
            via: None,
            cause: None,
            server_update_required: false,
            verification_code: None,
        }
    }

    pub fn is_blocked(&self) -> bool {
        matches!(
            self.kind,
            FailureKind::Authentication
                | FailureKind::Configuration
                | FailureKind::Permission
                | FailureKind::Unsupported
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Time {
    pub monotonic_ms: u64,
    pub epoch_ms: u64,
}

impl Time {
    pub fn from_platform(platform: &dyn Platform) -> Self {
        Self {
            monotonic_ms: platform.monotonic_ms(),
            epoch_ms: platform.now_epoch_ms(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AttemptId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCheckId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteCheckId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wakeup {
    Foreground,
    ForegroundProbe,
    LongResume,
    CredentialsChanged,
    NetworkChanged,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundPolicy {
    TolerateBusyHost,
    ProbeImmediately,
}

impl Wakeup {
    pub fn resets_backoff(self) -> bool {
        self != Self::CredentialsChanged
    }
    pub fn resubscribes(self) -> bool {
        matches!(
            self,
            Self::Foreground | Self::ForegroundProbe | Self::LongResume
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteCheck {
    Answered,
    Silent,
    Unchecked,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePreflight {
    Ready,
    Unprepared,
    Silent,
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEvent {
    Connect,
    Disconnect,
    Retry,
    Restart,
    SetEnabled(bool),
    SetRoutes(Vec<Route>),
    Network(NetworkStatus),
    Wake(Wakeup),
    Tick,
    Progress {
        attempt: AttemptId,
        stage: AttemptStage,
    },
    Ready {
        attempt: AttemptId,
        route_id: String,
    },
    Failed {
        attempt: AttemptId,
        failure: ConnectionFailure,
    },
    Lost {
        generation: Generation,
        failure: ConnectionFailure,
    },
    ProbeResult {
        probe: ProbeId,
        result: Result<(), ConnectionFailure>,
    },
    ConfigReady {
        generation: Generation,
    },
    Frame {
        generation: Generation,
    },
    HostResult {
        check: HostCheckId,
        answered: bool,
    },
    RoutesChecked {
        check: RouteCheckId,
        results: Vec<(String, RoutePreflight)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionAction {
    Establish {
        attempt: AttemptId,
        generation: Generation,
        routes: Vec<Route>,
    },
    CancelAttempt {
        attempt: AttemptId,
    },
    Release {
        generation: Generation,
    },
    Probe {
        generation: Generation,
        probe: ProbeId,
    },
    CancelProbe {
        probe: ProbeId,
    },
    CheckHost {
        generation: Generation,
        check: HostCheckId,
    },
    CancelHostCheck {
        check: HostCheckId,
    },
    CheckRoutes {
        generation: Generation,
        check: RouteCheckId,
        routes: Vec<Route>,
    },
    CancelRouteCheck {
        check: RouteCheckId,
    },
    Resubscribe {
        generation: Generation,
    },
}

#[derive(Debug)]
struct SetupBudget {
    remaining: u64,
    accounted_at: u64,
    paused: bool,
}
#[derive(Debug)]
struct SilentSocket {
    started: u64,
    due: Option<u64>,
    check: Option<(HostCheckId, u64)>,
    armed: bool,
    answered: bool,
    recheck: bool,
}
#[derive(Debug)]
struct Probe {
    id: ProbeId,
    deadline: u64,
    timeout: u64,
    tolerates_miss: bool,
    long_started: Option<u64>,
    silent: Option<SilentSocket>,
}
#[derive(Debug)]
struct SilentRoute {
    failures: u32,
    until: u64,
}

/// A shell drives emitted actions and feeds completions back with their original IDs.
#[derive(Debug)]
pub struct Supervisor {
    state: ConnectionState,
    routes: Vec<Route>,
    attempt_routes: Vec<Route>,
    primary: bool,
    enabled: bool,
    raw_network: NetworkStatus,
    serial: u64,
    generation: Generation,
    attempt: Option<AttemptId>,
    setup: Option<SetupBudget>,
    failures: u32,
    /// Checks made while the host's approval was pending; they do not count in `failures`.
    approval_waits: u32,
    retry_due: Option<u64>,
    active_route: Option<String>,
    connected_at: Option<u64>,
    probe: Option<Probe>,
    config_ready: bool,
    replacing: bool,
    preferred: Option<String>,
    next_route_check: Option<u64>,
    route_check: Option<(RouteCheckId, Vec<String>, u64)>,
    route_epoch: u64,
    silent_routes: BTreeMap<String, SilentRoute>,
    cooldowns: BTreeMap<String, u64>,
    label: String,
    foreground_policy: ForegroundPolicy,
    suspended_at: Option<u64>,
}

impl Supervisor {
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    pub fn with_foreground_policy(mut self, policy: ForegroundPolicy) -> Self {
        self.foreground_policy = policy;
        self
    }
    pub(crate) fn with_lineage(mut self, generation: Generation, serial: u64) -> Self {
        self.generation = generation;
        self.state.generation = generation;
        self.serial = serial;
        self
    }

    pub(crate) fn lineage(&self) -> (Generation, u64) {
        (
            Generation(self.state.generation.0.max(self.generation.0)),
            self.serial,
        )
    }
    pub fn new(routes: Vec<Route>, primary: bool, enabled: bool, network: NetworkStatus) -> Self {
        let network_state = if primary && network == NetworkStatus::Offline {
            NetworkStatus::Unknown
        } else {
            network
        };
        Self {
            state: ConnectionState {
                network: network_state,
                ..ConnectionState::default()
            },
            routes,
            attempt_routes: Vec::new(),
            primary,
            enabled,
            raw_network: network,
            serial: 0,
            generation: Generation(0),
            attempt: None,
            setup: None,
            failures: 0,
            approval_waits: 0,
            retry_due: None,
            active_route: None,
            connected_at: None,
            probe: None,
            config_ready: false,
            replacing: false,
            preferred: None,
            next_route_check: None,
            route_check: None,
            route_epoch: 0,
            silent_routes: BTreeMap::new(),
            cooldowns: BTreeMap::new(),
            label: "Environment".into(),
            foreground_policy: ForegroundPolicy::TolerateBusyHost,
            suspended_at: None,
        }
    }

    pub fn state(&self) -> &ConnectionState {
        &self.state
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }
    pub fn active_attempt(&self) -> Option<AttemptId> {
        self.attempt
    }
    pub fn active_route(&self) -> Option<&str> {
        self.active_route.as_deref()
    }
    pub fn attempt_routes(&self) -> &[Route] {
        &self.attempt_routes
    }
    pub fn active_probe(&self) -> Option<ProbeId> {
        self.probe.as_ref().map(|p| p.id)
    }
    pub fn host_check(&self) -> Option<HostCheckId> {
        self.probe
            .as_ref()?
            .silent
            .as_ref()?
            .check
            .map(|(id, _)| id)
    }
    pub fn route_check(&self) -> Option<RouteCheckId> {
        self.route_check.as_ref().map(|(id, _, _)| *id)
    }
    pub fn accepts_session(&self, generation: Generation) -> bool {
        self.state.phase == Phase::Connected && self.generation == generation
    }

    pub fn next_deadline(&self) -> Option<u64> {
        let setup = self
            .setup
            .as_ref()
            .filter(|s| !s.paused)
            .map(|s| s.accounted_at.saturating_add(s.remaining));
        let probe = self.probe.as_ref();
        let silent = probe.and_then(|p| p.silent.as_ref());
        let silent_due = silent.and_then(|s| s.due);
        let check_due = silent.and_then(|s| s.check.map(|(_, due)| due));
        let answered_due = silent.filter(|s| s.answered).map(|s| {
            if probe.is_some_and(|p| p.long_started.is_some()) || self.nearby() {
                s.started.saturating_add(SILENT_SOCKET_CHECK_AFTER_MS)
            } else {
                s.started.saturating_add(REMOTE_SILENT_SOCKET_AFTER_MS)
            }
        });
        [
            setup,
            self.retry_due,
            probe.map(|p| p.deadline),
            silent_due.filter(|_| self.config_ready),
            check_due,
            answered_due,
            self.next_route_check,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn platform_event(
        &mut self,
        event: PlatformEvent,
        time: Time,
        random: f64,
    ) -> Vec<ConnectionAction> {
        let event = match event {
            PlatformEvent::Network(network) => ConnectionEvent::Network(network),
            PlatformEvent::Foreground => {
                let suspended = self.suspended_at.take();
                let wakeup = match self.foreground_policy {
                    ForegroundPolicy::TolerateBusyHost => Wakeup::Foreground,
                    ForegroundPolicy::ProbeImmediately
                        if suspended.is_some_and(|at| {
                            time.monotonic_ms.saturating_sub(at) >= LONG_BACKGROUND_AFTER_MS
                        }) =>
                    {
                        Wakeup::LongResume
                    }
                    ForegroundPolicy::ProbeImmediately => Wakeup::ForegroundProbe,
                };
                ConnectionEvent::Wake(wakeup)
            }
            PlatformEvent::Suspended => {
                self.suspended_at = Some(time.monotonic_ms);
                return Vec::new();
            }
            PlatformEvent::CredentialsChanged => ConnectionEvent::Wake(Wakeup::CredentialsChanged),
            PlatformEvent::RegistrationsChanged | PlatformEvent::StorageChanged(_) => {
                return Vec::new();
            }
        };
        self.handle(event, time, random)
    }

    pub fn handle(
        &mut self,
        event: ConnectionEvent,
        time: Time,
        random: f64,
    ) -> Vec<ConnectionAction> {
        self.account_setup(time.monotonic_ms);
        let mut actions = Vec::new();
        match event {
            ConnectionEvent::Connect if self.enabled => {
                self.state.desired = true;
                if matches!(
                    self.state.phase,
                    Phase::Available | Phase::Offline | Phase::Blocked | Phase::Backoff
                ) {
                    self.start(time, &mut actions);
                }
            }
            ConnectionEvent::Disconnect | ConnectionEvent::SetEnabled(false) => {
                if matches!(event, ConnectionEvent::SetEnabled(false)) {
                    self.enabled = false;
                }
                self.state.desired = false;
                self.failures = 0;
                self.approval_waits = 0;
                self.state.last_failure = None;
                self.clear(&mut actions);
                self.idle(Phase::Available);
                self.state.attempt = 0;
            }
            ConnectionEvent::SetEnabled(true) => {
                if !self.enabled {
                    self.enabled = true;
                    self.state.desired = true;
                    self.failures = 0;
                    self.approval_waits = 0;
                    self.state.last_failure = None;
                    self.start(time, &mut actions);
                }
            }
            ConnectionEvent::Retry if self.state.desired => {
                if self.state.phase == Phase::Connected {
                    self.start_probe(Wakeup::ForegroundProbe, true, time, &mut actions);
                } else {
                    self.failures = 0;
                    self.approval_waits = 0;
                    self.state.last_failure = None;
                    self.clear(&mut actions);
                    self.start(time, &mut actions);
                }
            }
            ConnectionEvent::Restart if self.state.desired => {
                self.failures = 0;
                self.approval_waits = 0;
                self.state.last_failure = None;
                self.clear(&mut actions);
                self.start(time, &mut actions);
            }
            ConnectionEvent::Network(network) => self.network(network, time, &mut actions),
            ConnectionEvent::Wake(reason) if self.state.desired => {
                self.wake(reason, time, &mut actions)
            }
            ConnectionEvent::Progress { attempt, stage } if self.attempt == Some(attempt) => {
                self.state.stage = Some(stage);
                if let Some(setup) = &mut self.setup {
                    if stage == AttemptStage::Synchronizing {
                        setup.remaining =
                            setup.remaining.saturating_add(SYNCHRONIZATION_TIMEOUT_MS);
                    }
                    setup.paused = stage == AttemptStage::Preparing
                        && self
                            .attempt_routes
                            .iter()
                            .all(|r| r.mode == ConnectionMode::Ssh);
                }
            }
            ConnectionEvent::Ready { attempt, route_id }
                if self.attempt == Some(attempt)
                    && self.state.desired
                    && self.attempt_routes.iter().any(|r| r.id == route_id) =>
            {
                self.attempt = None;
                self.setup = None;
                actions.push(ConnectionAction::CancelAttempt { attempt });
                self.generation = self.state.generation;
                if let Some(preferred) = self.preferred.take().filter(|id| *id != route_id) {
                    self.cooldowns.insert(
                        preferred,
                        time.monotonic_ms.saturating_add(BETTER_ROUTE_COOLDOWN_MS),
                    );
                }
                self.active_route = Some(route_id);
                self.connected_at = Some(time.monotonic_ms);
                self.config_ready = false;
                self.replacing = false;
                self.state.last_failure = None;
                self.state.phase = Phase::Connected;
                self.state.stage = None;
                self.next_route_check = Some(
                    time.monotonic_ms
                        .saturating_add(BETTER_ROUTE_CHECK_INTERVAL_MS),
                );
            }
            ConnectionEvent::Failed { attempt, failure } if self.attempt == Some(attempt) => {
                self.fail(failure, false, time, random, &mut actions);
            }
            ConnectionEvent::Lost {
                generation,
                failure,
            } if self.accepts_session(generation) => {
                self.fail(failure, true, time, random, &mut actions);
            }
            ConnectionEvent::ConfigReady { generation } if self.accepts_session(generation) => {
                self.config_ready = true
            }
            ConnectionEvent::Frame { generation } if self.accepts_session(generation) => {
                self.frame(time, &mut actions)
            }
            ConnectionEvent::ProbeResult { probe, result }
                if self.active_probe() == Some(probe) =>
            {
                match result {
                    Ok(()) => self.cancel_probe(&mut actions),
                    Err(failure) => self.fail(failure, true, time, random, &mut actions),
                }
            }
            ConnectionEvent::HostResult { check, answered } if self.host_check() == Some(check) => {
                if let Some(silent) = self.probe.as_mut().and_then(|p| p.silent.as_mut()) {
                    silent.check = None;
                    silent.answered = answered;
                    silent.due = if answered {
                        None
                    } else {
                        Some(time.monotonic_ms.saturating_add(SILENT_SOCKET_RECHECK_MS))
                    };
                }
            }
            ConnectionEvent::RoutesChecked { check, results } => {
                self.routes_checked(check, results, time, &mut actions)
            }
            ConnectionEvent::SetRoutes(routes) => {
                self.routes = routes;
                self.forget_silent_routes(&mut actions);
                if self.state.phase == Phase::Connected {
                    if self
                        .active_route
                        .as_ref()
                        .is_some_and(|id| !self.routes.iter().any(|r| &r.id == id))
                    {
                        self.replace(false, time, &mut actions);
                    } else {
                        self.check_routes(time, &mut actions);
                    }
                } else if self.state.desired {
                    self.clear(&mut actions);
                    self.start(time, &mut actions);
                }
            }
            _ => {}
        }
        self.service_timers(time, random, &mut actions);
        actions
    }

    fn next_id(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    fn account_setup(&mut self, now: u64) {
        if let Some(setup) = &mut self.setup {
            if !setup.paused {
                setup.remaining = setup
                    .remaining
                    .saturating_sub(now.saturating_sub(setup.accounted_at));
            }
            setup.accounted_at = now;
        }
    }

    fn idle(&mut self, phase: Phase) {
        self.state.phase = phase;
        self.state.stage = None;
        self.state.generation = self.generation;
        self.state.retry_at = None;
        self.retry_due = None;
    }

    fn start(&mut self, time: Time, actions: &mut Vec<ConnectionAction>) {
        self.retry_due = None;
        self.state.retry_at = None;
        if !self.enabled || !self.state.desired {
            return;
        }
        if self.state.network == NetworkStatus::Offline && !self.replacing {
            self.idle(Phase::Offline);
            self.state.attempt = self
                .failures
                .saturating_add(self.approval_waits)
                .saturating_add(1);
            return;
        }
        if self.routes.is_empty() {
            self.idle(Phase::Blocked);
            self.state.last_failure = Some(ConnectionFailure::new(
                FailureKind::Configuration,
                "A saved environment needs at least one route.",
            ));
            return;
        }
        let attempt = AttemptId(self.next_id());
        self.attempt = Some(attempt);
        self.state.phase = Phase::Connecting;
        self.state.stage = Some(AttemptStage::Preparing);
        self.state.attempt = self
            .failures
            .saturating_add(self.approval_waits)
            .saturating_add(1);
        self.state.generation = Generation(self.generation.0 + 1);
        self.attempt_routes = self.routes.clone();
        if let Some(index) = self
            .preferred
            .as_ref()
            .and_then(|id| self.attempt_routes.iter().position(|r| &r.id == id))
        {
            let preferred = self.attempt_routes.remove(index);
            self.attempt_routes.insert(0, preferred);
        }
        self.setup = Some(SetupBudget {
            remaining: ESTABLISHMENT_TIMEOUT_MS.saturating_mul(self.routes.len() as u64),
            accounted_at: time.monotonic_ms,
            paused: self.routes.iter().all(|r| r.mode == ConnectionMode::Ssh),
        });
        actions.push(ConnectionAction::Establish {
            attempt,
            generation: self.state.generation,
            routes: self.attempt_routes.clone(),
        });
    }

    fn cancel_probe(&mut self, actions: &mut Vec<ConnectionAction>) {
        if let Some(probe) = self.probe.take() {
            actions.push(ConnectionAction::CancelProbe { probe: probe.id });
            if let Some((check, _)) = probe.silent.and_then(|s| s.check) {
                actions.push(ConnectionAction::CancelHostCheck { check });
            }
        }
    }

    fn clear(&mut self, actions: &mut Vec<ConnectionAction>) {
        if let Some(attempt) = self.attempt.take() {
            actions.push(ConnectionAction::CancelAttempt { attempt });
        }
        self.setup = None;
        self.cancel_probe(actions);
        if self.active_route.take().is_some() {
            actions.push(ConnectionAction::Release {
                generation: self.generation,
            });
        }
        if let Some((check, _, _)) = self.route_check.take() {
            actions.push(ConnectionAction::CancelRouteCheck { check });
        }
        self.next_route_check = None;
        self.connected_at = None;
        self.config_ready = false;
        self.retry_due = None;
        self.state.retry_at = None;
    }

    fn replace(&mut self, ignore_offline: bool, time: Time, actions: &mut Vec<ConnectionAction>) {
        self.clear(actions);
        self.failures = 0;
        self.approval_waits = 0;
        self.replacing = ignore_offline;
        self.start(time, actions);
    }

    fn fail(
        &mut self,
        failure: ConnectionFailure,
        established: bool,
        time: Time,
        random: f64,
        actions: &mut Vec<ConnectionAction>,
    ) {
        let failed_probe = self.probe.is_some();
        let failure_kind = failure.kind.clone();
        if established
            && self
                .connected_at
                .is_some_and(|at| time.monotonic_ms.saturating_sub(at) >= BACKOFF_RESET_AFTER_MS)
        {
            self.failures = 0;
            self.approval_waits = 0;
        }
        self.clear(actions);
        self.replacing = false;
        if !established {
            self.state.last_failure = Some(failure.clone());
        }
        if failure.is_blocked() {
            self.idle(Phase::Blocked);
            self.state.last_failure = Some(failure);
            return;
        }
        if failed_probe {
            self.failures = 0;
            self.approval_waits = 0;
            self.start(time, actions);
            return;
        }
        // Someone may be approving this device on the host: check back every 4 s for the
        // request's 10 minutes. Those waits do not climb the ladder, so a later outage starts
        // low; past them, a forgotten device backs off but never polls faster than 4 s.
        let approval = failure_kind == FailureKind::ApprovalPending;
        if approval {
            self.approval_waits = self.approval_waits.saturating_add(1);
        }
        let awaiting_approval = approval && self.approval_waits <= APPROVAL_PENDING_FAST_RETRIES;
        let delay = if awaiting_approval {
            APPROVAL_PENDING_RETRY_DELAY_MS
        } else if established && self.failures == 0 {
            session_loss_delay_ms(random)
        } else if approval {
            retry_delay_ms(self.failures, random).max(APPROVAL_PENDING_RETRY_DELAY_MS)
        } else {
            retry_delay_ms(self.failures, random)
        };
        if !awaiting_approval {
            self.failures = self.failures.saturating_add(1);
        }
        self.idle(Phase::Backoff);
        self.retry_due = Some(time.monotonic_ms.saturating_add(delay));
        self.state.retry_at = Some(time.epoch_ms.saturating_add(delay));
    }

    fn network(&mut self, network: NetworkStatus, time: Time, actions: &mut Vec<ConnectionAction>) {
        let previous = self.raw_network;
        self.raw_network = network;
        let mapped = if self.primary && network == NetworkStatus::Offline {
            NetworkStatus::Unknown
        } else {
            network
        };
        if self.state.network == mapped {
            return;
        }
        self.state.network = mapped;
        if network != NetworkStatus::Offline {
            self.forget_silent_routes(actions);
        }
        if !self.state.desired {
            return;
        }
        if self.state.phase == Phase::Connected {
            if network == NetworkStatus::Offline {
                self.start_probe(Wakeup::ForegroundProbe, true, time, actions);
            } else {
                self.check_routes(time, actions);
            }
            return;
        }
        if self.state.phase == Phase::Connecting {
            if self.primary {
                if previous == NetworkStatus::Offline && network != NetworkStatus::Offline {
                    self.replace(true, time, actions);
                }
            } else if network == NetworkStatus::Offline {
                self.clear(actions);
                self.idle(Phase::Offline);
            }
            return;
        }
        self.failures = 0;
        self.approval_waits = 0;
        self.start(time, actions);
    }

    fn wake(&mut self, reason: Wakeup, time: Time, actions: &mut Vec<ConnectionAction>) {
        if matches!(reason, Wakeup::NetworkChanged | Wakeup::LongResume) {
            self.forget_silent_routes(actions);
        }
        if self.state.phase == Phase::Connecting {
            if reason == Wakeup::LongResume {
                self.replace(true, time, actions);
            } else if reason == Wakeup::CredentialsChanged
                && self.routes.iter().any(|r| r.mode == ConnectionMode::Relay)
            {
                self.clear(actions);
                self.start(time, actions);
            }
            return;
        }
        if self.state.phase != Phase::Connected {
            if reason.resets_backoff() {
                self.failures = 0;
                self.approval_waits = 0;
            }
            self.start(time, actions);
            return;
        }
        if reason == Wakeup::CredentialsChanged {
            if self.active_route.as_ref().is_some_and(|id| {
                self.routes
                    .iter()
                    .any(|r| r.id == *id && r.mode == ConnectionMode::Relay)
            }) {
                self.clear(actions);
                self.start(time, actions);
            }
            return;
        }
        if reason.resubscribes() {
            actions.push(ConnectionAction::Resubscribe {
                generation: self.generation,
            });
        }
        self.check_routes(time, actions);
        self.start_probe(reason, false, time, actions);
    }

    fn nearby(&self) -> bool {
        self.active_route
            .as_ref()
            .and_then(|id| self.routes.iter().find(|r| &r.id == id))
            .is_some_and(Route::is_nearby)
    }

    fn start_probe(
        &mut self,
        reason: Wakeup,
        quick: bool,
        time: Time,
        actions: &mut Vec<ConnectionAction>,
    ) {
        let timeout = if quick {
            QUICK_PROBE_TIMEOUT_MS
        } else {
            match reason {
                Wakeup::Foreground => CONNECTION_PROBE_TIMEOUT_MS,
                Wakeup::LongResume => LONG_RESUME_PROBE_TIMEOUT_MS,
                Wakeup::ForegroundProbe | Wakeup::NetworkChanged if self.nearby() => {
                    QUICK_PROBE_TIMEOUT_MS
                }
                Wakeup::ForegroundProbe | Wakeup::NetworkChanged => CONNECTION_PROBE_TIMEOUT_MS,
                Wakeup::CredentialsChanged => return,
            }
        };
        let deadline = time.monotonic_ms.saturating_add(timeout);
        if let Some(probe) = &mut self.probe {
            probe.deadline = probe.deadline.min(deadline);
            if reason != Wakeup::Foreground || quick {
                probe.tolerates_miss = false;
            }
            if reason == Wakeup::LongResume && probe.long_started.is_none() {
                probe.long_started = Some(time.monotonic_ms);
            }
            if reason != Wakeup::Foreground || quick {
                if let Some(silent) = &mut probe.silent {
                    silent.recheck = true;
                } else {
                    probe.silent = Some(Self::silent_socket(time.monotonic_ms));
                }
            }
            return;
        }
        let id = ProbeId(self.next_id());
        self.probe = Some(Probe {
            id,
            deadline,
            timeout,
            tolerates_miss: reason == Wakeup::Foreground && !quick,
            long_started: (reason == Wakeup::LongResume).then_some(time.monotonic_ms),
            silent: (reason != Wakeup::Foreground || quick)
                .then(|| Self::silent_socket(time.monotonic_ms)),
        });
        actions.push(ConnectionAction::Probe {
            generation: self.generation,
            probe: id,
        });
    }

    fn silent_socket(started: u64) -> SilentSocket {
        SilentSocket {
            started,
            due: Some(started.saturating_add(SILENT_SOCKET_CHECK_AFTER_MS)),
            check: None,
            armed: false,
            answered: false,
            recheck: false,
        }
    }

    fn frame(&mut self, time: Time, actions: &mut Vec<ConnectionAction>) {
        let Some(probe) = &mut self.probe else {
            return;
        };
        let Some(silent) = &probe.silent else {
            return;
        };
        if !silent.armed {
            return;
        }
        if let Some((check, _)) = silent.check {
            actions.push(ConnectionAction::CancelHostCheck { check });
        }
        let recheck = silent.recheck;
        if let Some(started) = probe.long_started {
            probe.deadline = probe
                .deadline
                .max(started.saturating_add(QUICK_PROBE_TIMEOUT_MS));
        }
        probe.silent = recheck.then(|| Self::silent_socket(time.monotonic_ms));
    }

    fn forget_silent_routes(&mut self, actions: &mut Vec<ConnectionAction>) {
        self.route_epoch += 1;
        self.silent_routes.clear();
        if let Some((check, _, _)) = self.route_check.take() {
            actions.push(ConnectionAction::CancelRouteCheck { check });
        }
    }

    fn check_routes(&mut self, time: Time, actions: &mut Vec<ConnectionAction>) {
        if self.route_check.is_some() || self.state.phase != Phase::Connected {
            return;
        }
        let Some(index) = self
            .active_route
            .as_ref()
            .and_then(|id| self.routes.iter().position(|r| &r.id == id))
        else {
            return;
        };
        let routes: Vec<_> = self.routes[..index]
            .iter()
            .filter(|r| {
                self.cooldowns.get(&r.id).copied().unwrap_or(0) <= time.monotonic_ms
                    && self
                        .silent_routes
                        .get(&r.id)
                        .is_none_or(|s| s.until <= time.monotonic_ms)
            })
            .cloned()
            .collect();
        if routes.is_empty() {
            return;
        }
        let check = RouteCheckId(self.next_id());
        self.route_check = Some((
            check,
            routes.iter().map(|r| r.id.clone()).collect(),
            self.route_epoch,
        ));
        actions.push(ConnectionAction::CheckRoutes {
            generation: self.generation,
            check,
            routes,
        });
    }

    fn routes_checked(
        &mut self,
        check: RouteCheckId,
        results: Vec<(String, RoutePreflight)>,
        time: Time,
        actions: &mut Vec<ConnectionAction>,
    ) {
        if self
            .route_check
            .as_ref()
            .is_none_or(|(id, _, epoch)| *id != check || *epoch != self.route_epoch)
        {
            return;
        }
        let Some((_, ids, _)) = self.route_check.take() else {
            return;
        };
        let mut preferred = None;
        for id in ids {
            let Some((_, result)) = results.iter().find(|(key, _)| key == &id) else {
                continue;
            };
            if *result == RoutePreflight::Silent {
                let failures = self
                    .silent_routes
                    .get(&id)
                    .map_or(1, |s| s.failures.saturating_add(1));
                let delay = BETTER_ROUTE_CHECK_INTERVAL_MS
                    .saturating_mul(1_u64.checked_shl(failures).unwrap_or(u64::MAX))
                    .min(SILENT_ROUTE_BACKOFF_MAX_MS);
                self.silent_routes.insert(
                    id,
                    SilentRoute {
                        failures,
                        until: time.monotonic_ms.saturating_add(delay),
                    },
                );
            } else {
                self.silent_routes.remove(&id);
                if *result == RoutePreflight::Ready && preferred.is_none() {
                    preferred = Some(id);
                }
            }
        }
        if let Some(id) = preferred {
            self.preferred = Some(id);
            self.replace(true, time, actions);
        }
    }

    fn service_timers(&mut self, time: Time, random: f64, actions: &mut Vec<ConnectionAction>) {
        let now = time.monotonic_ms;
        if self
            .setup
            .as_ref()
            .is_some_and(|s| !s.paused && s.remaining == 0)
        {
            self.fail(
                ConnectionFailure::new(
                    FailureKind::Timeout,
                    format!(
                        "{} did not respond during connection setup.{}",
                        self.label,
                        if self.routes.iter().any(|r| r.mode == ConnectionMode::Relay) {
                            format!(" {NETWORK_BLOCKING_HINT}")
                        } else {
                            String::new()
                        }
                    ),
                ),
                false,
                time,
                random,
                actions,
            );
            return;
        }
        if self.retry_due.is_some_and(|due| now >= due) {
            self.start(time, actions);
        }
        if self.next_route_check.is_some_and(|due| now >= due) {
            self.next_route_check = Some(now.saturating_add(BETTER_ROUTE_CHECK_INTERVAL_MS));
            self.check_routes(time, actions);
        }
        let silent_dead = self.probe.as_ref().is_some_and(|p| {
            p.silent.as_ref().is_some_and(|s| {
                s.answered
                    && (p.long_started.is_some()
                        || self.nearby()
                        || now.saturating_sub(s.started) >= REMOTE_SILENT_SOCKET_AFTER_MS)
            })
        });
        if silent_dead {
            self.replace(true, time, actions);
            return;
        }
        if self.probe.as_ref().is_some_and(|p| now >= p.deadline) {
            if self.probe.as_ref().is_some_and(|p| p.tolerates_miss) {
                let id = ProbeId(self.next_id());
                if let Some(probe) = &mut self.probe {
                    actions.push(ConnectionAction::CancelProbe { probe: probe.id });
                    probe.id = id;
                    probe.tolerates_miss = false;
                    probe.deadline = now.saturating_add(probe.timeout);
                }
                actions.push(ConnectionAction::Probe {
                    generation: self.generation,
                    probe: id,
                });
            } else if self
                .probe
                .as_ref()
                .is_some_and(|p| p.long_started.is_some())
            {
                self.replace(true, time, actions);
            } else {
                self.fail(
                    ConnectionFailure::new(
                        FailureKind::Timeout,
                        format!(
                            "{} did not respond to a connection health check.",
                            self.label
                        ),
                    ),
                    true,
                    time,
                    random,
                    actions,
                );
            }
            return;
        }
        let check_timed_out = self
            .probe
            .as_ref()
            .and_then(|p| p.silent.as_ref())
            .and_then(|s| s.check)
            .filter(|(_, due)| now >= *due);
        if let Some((check, _)) = check_timed_out {
            actions.push(ConnectionAction::CancelHostCheck { check });
            if let Some(silent) = self.probe.as_mut().and_then(|p| p.silent.as_mut()) {
                silent.check = None;
                silent.due = Some(now.saturating_add(SILENT_SOCKET_RECHECK_MS));
            }
        }
        let check_due = self.config_ready
            && self
                .probe
                .as_ref()
                .and_then(|p| p.silent.as_ref())
                .is_some_and(|s| s.due.is_some_and(|due| now >= due));
        if check_due {
            let check = HostCheckId(self.next_id());
            if let Some(silent) = self.probe.as_mut().and_then(|p| p.silent.as_mut()) {
                silent.due = None;
                silent.armed = true;
                silent.check = Some((check, now.saturating_add(ROUTE_CHECK_TIMEOUT_MS)));
            }
            actions.push(ConnectionAction::CheckHost {
                generation: self.generation,
                check,
            });
        }
    }
}

impl Route {
    pub fn is_nearby(&self) -> bool {
        matches!(
            route_kind(self, false),
            RouteKind::Loopback | RouteKind::Lan
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    Loopback,
    Lan,
    Tailnet,
    Vpn,
    Public,
    Ssh,
    Relay,
}
impl RouteKind {
    fn rank(self) -> u8 {
        match self {
            Self::Loopback => 0,
            Self::Lan => 1,
            Self::Tailnet | Self::Vpn => 2,
            Self::Public => 3,
            Self::Ssh => 4,
            Self::Relay => 5,
        }
    }
}

pub fn route_kind(route: &Route, tailscale: bool) -> RouteKind {
    match route.mode {
        ConnectionMode::Relay => return RouteKind::Relay,
        ConnectionMode::Ssh => return RouteKind::Ssh,
        _ => {}
    }
    address_kind(&route.http_base, tailscale)
}
pub fn is_nearby_address(base: &str) -> bool {
    matches!(
        address_kind(base, false),
        RouteKind::Loopback | RouteKind::Lan
    )
}
fn address_kind(base: &str, tailscale: bool) -> RouteKind {
    let Ok(url) = Url::parse(base) else {
        return RouteKind::Public;
    };
    let Some(host) = url.host() else {
        return RouteKind::Public;
    };
    let (loopback, tailnet, shared, private) = match host {
        Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_lowercase();
            (
                host == "localhost",
                host.ends_with(".ts.net"),
                false,
                !host.contains('.')
                    || host.ends_with(".localhost")
                    || host.ends_with(".local")
                    || host == "home.arpa"
                    || host.ends_with(".home.arpa"),
            )
        }
        Host::Ipv4(address) => {
            let [a, b, _, _] = address.octets();
            (
                a == 127,
                false,
                a == 100 && (64..=127).contains(&b),
                private_ipv4(address.octets()),
            )
        }
        Host::Ipv6(address) => {
            let words = address.segments();
            let loopback = words == [0, 0, 0, 0, 0, 0, 0, 1];
            let tailnet = words[..3] == [0xfd7a, 0x115c, 0xa1e0];
            let mapped = address
                .to_ipv4_mapped()
                .is_some_and(|v| private_ipv4(v.octets()));
            (
                loopback,
                tailnet,
                false,
                loopback
                    || words == [0; 8]
                    || words[0] & 0xfe00 == 0xfc00
                    || words[0] & 0xffc0 == 0xfe80
                    || mapped,
            )
        }
    };
    if loopback {
        RouteKind::Loopback
    } else if tailscale || tailnet {
        RouteKind::Tailnet
    } else if shared {
        RouteKind::Vpn
    } else if private {
        RouteKind::Lan
    } else {
        RouteKind::Public
    }
}
fn private_ipv4([a, b, _, _]: [u8; 4]) -> bool {
    matches!(a, 0 | 10 | 127)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
        || (a == 198 && (18..=19).contains(&b))
}

pub fn route_keeps_its_host(route: &Route) -> bool {
    route.http_base.starts_with("https:") || route_kind(route, false) == RouteKind::Loopback
}

pub fn insert_route(routes: &mut Vec<Route>, route: Route, ahead_of_same_kind: bool) {
    let kind = route_kind(&route, false);
    let index = routes
        .iter()
        .position(|r| {
            let existing = route_kind(r, false);
            existing.rank() > kind.rank() || (ahead_of_same_kind && existing == kind)
        })
        .unwrap_or(routes.len());
    routes.insert(index, route);
}
pub fn upsert_route(routes: &mut Vec<Route>, route: Route) {
    if let Some(existing) = routes.iter_mut().find(|r| r.id == route.id) {
        *existing = route;
    } else {
        insert_route(routes, route, true);
    }
}
pub fn pending_route_order(pending: Option<&[String]>, saved: &[String]) -> Option<Vec<String>> {
    let pending = pending?;
    if pending.len() != saved.len()
        || !pending.iter().all(|id| saved.contains(id))
        || pending == saved
    {
        None
    } else {
        Some(pending.to_vec())
    }
}
pub fn credential_connection_id(id: &str) -> &str {
    if id.starts_with("learned:") {
        id.split_once('@').map_or(id, |(_, credential)| credential)
    } else {
        id
    }
}

pub fn sends_token_in_clear(routes: &[Route], route: &Route, tailscale: bool) -> bool {
    if route.mode != ConnectionMode::Remote
        || !route.id.starts_with("learned:")
        || tailscale
        || !route.http_base.starts_with("http:")
    {
        return false;
    }
    let credential_id = credential_connection_id(&route.id);
    if credential_id == route.id {
        return false;
    }
    !token_allows_plain_http(routes, credential_id)
}
fn token_allows_plain_http(routes: &[Route], credential_id: &str) -> bool {
    routes
        .iter()
        .find(|r| r.id == credential_id)
        .is_some_and(|r| {
            r.http_base.starts_with("http:")
                && matches!(route_kind(r, false), RouteKind::Lan | RouteKind::Public)
        })
}
pub fn usable_routes(routes: &[Route], tailscale_route_ids: &[String]) -> Vec<Route> {
    routes
        .iter()
        .filter(|r| !sends_token_in_clear(routes, r, tailscale_route_ids.contains(&r.id)))
        .cloned()
        .collect()
}
pub fn routes_after_removing(routes: &[Route], removed_id: &str) -> Vec<Route> {
    let Some(removed) = routes.iter().find(|r| r.id == removed_id) else {
        return routes.to_vec();
    };
    routes
        .iter()
        .filter(|r| {
            if r.id == removed_id {
                return false;
            }
            if !r.id.starts_with("learned:") {
                return true;
            }
            let credential_id = credential_connection_id(&r.id);
            if credential_id == r.id {
                removed.mode != ConnectionMode::Relay
            } else {
                credential_id != removed_id
            }
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedEndpoint {
    pub http_base_url: String,
    pub kind: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnedRoutes {
    pub routes: Vec<Route>,
    pub tailscale_route_ids: Vec<String>,
}

pub fn merge_learned_routes(
    saved: &[Route],
    active: &Route,
    reported: &[ReportedEndpoint],
    allow_insecure: bool,
    cloud_authorization: bool,
    tailscale_route_ids: &[String],
) -> Option<LearnedRoutes> {
    if !matches!(
        active.mode,
        ConnectionMode::Relay | ConnectionMode::Remote | ConnectionMode::Tunnel
    ) {
        return None;
    }
    let cloud = cloud_authorization || active.mode == ConnectionMode::Relay;
    let credential = (!cloud).then(|| credential_connection_id(&active.id));
    let mut endpoints: Vec<(String, Url, bool)> = Vec::new();
    for endpoint in reported {
        let Ok(url) = Url::parse(&endpoint.http_base_url) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") || (url.scheme() == "http" && !allow_insecure)
        {
            continue;
        }
        let mut candidate = active.clone();
        candidate.http_base = url.to_string();
        candidate.mode = ConnectionMode::Remote;
        if route_kind(&candidate, false) == RouteKind::Loopback {
            continue;
        }
        let origin = url.origin().ascii_serialization();
        if let Some((_, old_url, tailnet)) = endpoints.iter_mut().find(|(key, _, _)| key == &origin)
        {
            *old_url = url;
            *tailnet |= endpoint.kind.as_deref() == Some("tailnet");
        } else {
            endpoints.push((origin, url, endpoint.kind.as_deref() == Some("tailnet")));
        }
    }
    let normalized = |url: &str| url.trim_end_matches('/').to_owned();
    let mut known: std::collections::BTreeSet<String> = saved
        .iter()
        .filter(|r| !r.id.starts_with("learned:") && !r.http_base.is_empty())
        .map(|r| normalized(&r.http_base))
        .collect();
    let mut routes = Vec::new();
    let mut tailscale = Vec::new();
    for route in saved {
        let endpoint = endpoints
            .iter()
            .find(|(origin, _, _)| *origin == normalized(&route.http_base));
        let tailnet = endpoint.map_or_else(
            || tailscale_route_ids.contains(&route.id),
            |(_, _, tailnet)| *tailnet,
        );
        if route.id.starts_with("learned:") {
            if endpoint.is_none()
                || known.contains(&normalized(&route.http_base))
                || sends_token_in_clear(saved, route, tailnet)
            {
                continue;
            }
            known.insert(normalized(&route.http_base));
        }
        if tailnet {
            tailscale.push(route.id.clone());
        }
        routes.push(route.clone());
    }
    for (origin, url, tailnet) in endpoints {
        if known.contains(&origin) {
            continue;
        }
        if url.scheme() == "http"
            && !tailnet
            && credential.is_some_and(|id| !token_allows_plain_http(saved, id))
        {
            continue;
        }
        let id = match credential {
            None => format!("learned:{}:{origin}", active.environment.0),
            Some(credential) => format!("learned:{}:{origin}@{credential}", active.environment.0),
        };
        let route = Route {
            id: id.clone(),
            environment: active.environment.clone(),
            mode: ConnectionMode::Remote,
            http_base: format!("{origin}/"),
            ws_base: format!(
                "{}://{}/",
                if url.scheme() == "https" { "wss" } else { "ws" },
                &url[url::Position::BeforeHost..url::Position::AfterPort]
            ),
        };
        let kind = route_kind(&route, tailnet);
        let index = routes
            .iter()
            .position(|r| route_kind(r, tailscale.contains(&r.id)).rank() > kind.rank())
            .unwrap_or(routes.len());
        routes.insert(index, route);
        if tailnet {
            tailscale.push(id);
        }
    }
    if saved == routes
        && tailscale_route_ids.iter().all(|id| tailscale.contains(id))
        && tailscale.len() == tailscale_route_ids.len()
    {
        None
    } else {
        Some(LearnedRoutes {
            routes,
            tailscale_route_ids: tailscale,
        })
    }
}

pub fn routes_connection_key(routes: &[Route]) -> Option<String> {
    let routes: Vec<_> = routes
        .iter()
        .filter(|r| !r.id.starts_with("learned:"))
        .collect();
    let mut keys = Vec::new();
    for route in routes {
        let key = if route.mode == ConnectionMode::Relay {
            serde_json::to_string(&("RelayConnectionTarget", &route.environment)).ok()?
        } else {
            let tag = match route.mode {
                ConnectionMode::Local => "PrimaryConnectionTarget",
                ConnectionMode::Ssh => return None,
                _ => "BearerConnectionTarget",
            };
            let http = Url::parse(&route.http_base).ok()?;
            let ws = Url::parse(&route.ws_base).ok()?;
            if !matches!(http.scheme(), "http" | "https")
                || !matches!(ws.scheme(), "ws" | "wss")
                || [&http, &ws]
                    .iter()
                    .any(|u| !u.username().is_empty() || u.password().is_some())
            {
                return None;
            }
            serde_json::to_string(&(
                tag,
                &route.environment,
                http.as_str().trim_end_matches('/'),
                ws.as_str().trim_end_matches('/'),
            ))
            .ok()?
        };
        keys.push(key);
    }
    if keys.len() == 1 {
        keys.pop()
    } else {
        keys.sort();
        serde_json::to_string(&keys).ok()
    }
}

pub fn profiled_routes_connection_key(
    routes: &[Route],
    profiles: &BTreeMap<String, ConnectionProfile>,
) -> Option<String> {
    let mut keys = Vec::new();
    for route in routes {
        let profile = profiles.get(&route.id);
        if matches!(
            profile,
            Some(ConnectionProfile::Bearer {
                learned: Some(true),
                ..
            })
        ) {
            continue;
        }
        let key = match (&route.mode, profile) {
            (ConnectionMode::Relay | ConnectionMode::Local, _) => {
                routes_connection_key(std::slice::from_ref(route))?
            }
            (
                ConnectionMode::Ssh,
                Some(ConnectionProfile::Ssh {
                    environment_id,
                    target,
                    ..
                }),
            ) if *environment_id == route.environment => serde_json::to_string(&(
                "SshConnectionTarget",
                &route.environment,
                &target.alias,
                &target.hostname,
                &target.username,
                &target.port,
            ))
            .ok()?,
            (
                ConnectionMode::Remote | ConnectionMode::Tunnel,
                Some(ConnectionProfile::Bearer {
                    environment_id,
                    http_base_url,
                    ws_base_url,
                    ..
                }),
            ) if *environment_id == route.environment => {
                let mut saved = route.clone();
                saved.http_base = http_base_url.clone();
                saved.ws_base = ws_base_url.clone();
                routes_connection_key(std::slice::from_ref(&saved))?
            }
            _ => return None,
        };
        keys.push(key);
    }
    if keys.len() == 1 {
        keys.pop()
    } else {
        keys.sort();
        serde_json::to_string(&keys).ok()
    }
}

#[derive(Clone)]
pub struct PullRequestRoutingPermissions {
    platform: Rc<dyn Platform>,
    stores: ConnectionStores,
}
impl PullRequestRoutingPermissions {
    pub fn new(platform: Rc<dyn Platform>) -> Self {
        Self {
            stores: ConnectionStores::new(platform.clone()),
            platform,
        }
    }
    async fn connection_key(
        &self,
        entry: &crate::environment::EnvironmentRegistration,
    ) -> Result<Option<String>, ConnectionFailure> {
        if entry.routes.is_empty() || entry.routes.iter().any(|r| r.environment != entry.id) {
            return Ok(None);
        }
        let mut profiles = BTreeMap::new();
        for route in &entry.routes {
            if !matches!(route.mode, ConnectionMode::Relay | ConnectionMode::Local)
                && let Some(profile) = self.stores.profile(&entry.id, &route.id).await?
            {
                profiles.insert(route.id.clone(), profile);
            }
        }
        Ok(profiled_routes_connection_key(&entry.routes, &profiles))
    }
    pub async fn get(
        &self,
        entry: &crate::environment::EnvironmentRegistration,
    ) -> Result<crate::persistence::RoutingAccess, ConnectionFailure> {
        let Some(key) = self.connection_key(entry).await? else {
            return Ok(crate::persistence::RoutingAccess::Off);
        };
        let grant = crate::persistence::load_routing_grant(self.platform.as_ref(), &entry.id)
            .await
            .map_err(|_| {
                ConnectionFailure::new(
                    FailureKind::Configuration,
                    "Could not read routing permission.",
                )
            })?;
        Ok(grant
            .filter(|g| g.grant.connection_key == key)
            .map_or(crate::persistence::RoutingAccess::Off, |g| {
                g.grant.permission
            }))
    }
    pub async fn set(
        &self,
        entry: &crate::environment::EnvironmentRegistration,
        permission: crate::persistence::RoutingAccess,
    ) -> Result<(), ConnectionFailure> {
        let key = self.connection_key(entry).await?.ok_or_else(|| {
            ConnectionFailure::new(
                FailureKind::Configuration,
                "This environment does not have a saved connection endpoint.",
            )
        })?;
        let previous = crate::persistence::load_routing_grant(self.platform.as_ref(), &entry.id)
            .await
            .map_err(|_| {
                ConnectionFailure::new(
                    FailureKind::Configuration,
                    "Could not read routing permission.",
                )
            })?;
        crate::persistence::compare_exchange_routing_grant(
            self.platform.as_ref(),
            &entry.id,
            previous.map(|g| g.revision),
            key,
            permission,
        )
        .await
        .map_err(|_| {
            ConnectionFailure::new(
                FailureKind::Configuration,
                "Could not save routing permission; reload its current value.",
            )
        })?;
        Ok(())
    }
    pub async fn forget(&self, environment: &EnvironmentId) -> Result<(), ConnectionFailure> {
        let previous = crate::persistence::load_routing_grant(self.platform.as_ref(), environment)
            .await
            .map_err(|_| {
                ConnectionFailure::new(
                    FailureKind::Configuration,
                    "Could not read routing permission.",
                )
            })?;
        if let Some(previous) = previous {
            crate::persistence::compare_exchange_routing_grant(
                self.platform.as_ref(),
                environment,
                Some(previous.revision),
                previous.grant.connection_key,
                crate::persistence::RoutingAccess::Off,
            )
            .await
            .map_err(|_| {
                ConnectionFailure::new(
                    FailureKind::Configuration,
                    "Could not revoke routing permission.",
                )
            })?;
        }
        Ok(())
    }
    pub fn changes(
        &self,
        environment: EnvironmentId,
    ) -> LocalBoxStream<'static, Result<Option<crate::persistence::RoutingGrant>, crate::ClientError>>
    {
        let events = self.platform.events();
        let initial_platform = self.platform.clone();
        let initial_environment = environment.clone();
        let initial = futures::stream::once(async move {
            crate::persistence::load_routing_grant(initial_platform.as_ref(), &initial_environment)
                .await
                .map_err(|_| {
                    crate::ClientError::InvalidPayload("could not read routing permission".into())
                })
        });
        let platform = self.platform.clone();
        let updates = events.filter_map(move |event| {
            let platform = platform.clone(); let environment = environment.clone();
            async move {
                if matches!(event, PlatformEvent::StorageChanged(ref key) if key.namespace == "routing-grant" && key.environment.as_ref() == Some(&environment)) {
                    Some(crate::persistence::load_routing_grant(platform.as_ref(), &environment).await.map_err(|_| crate::ClientError::InvalidPayload("could not read routing permission".into())))
                } else { None }
            }
        });
        initial.chain(updates).boxed_local()
    }
}

/// Ordered route checks share a single head start; route attempts keep independent leases.
#[derive(Debug)]
pub struct RouteSelection {
    routes: Vec<Route>,
    checks: Vec<Option<RouteCheck>>,
    attempted: Vec<bool>,
    cursor: usize,
    deferred: Vec<usize>,
    head_start: u64,
    in_flight: bool,
    stopped: bool,
    transient: Option<ConnectionFailure>,
    blocked: Option<ConnectionFailure>,
}

pub fn orchestration_protocol_compatibility_error(
    descriptor: &t3_contracts::environment::ExecutionEnvironmentDescriptor,
) -> Option<ConnectionFailure> {
    use t3_contracts::environment::{ORCHESTRATION_PROTOCOL_VERSION, ServerSelfUpdateCapability};
    let protocol = descriptor.orchestration_protocol_version.unwrap_or(1);
    if protocol == ORCHESTRATION_PROTOCOL_VERSION {
        return None;
    }
    let newer_server = protocol > ORCHESTRATION_PROTOCOL_VERSION;
    let detail = if newer_server {
        format!(
            "This client is not supported by this server. Update your app or use a compatible release to connect to {}.",
            descriptor.label.0
        )
    } else {
        format!(
            "This client requires a newer server. Update T3 Code on {} to connect.",
            descriptor.label.0
        )
    };
    let can_update = descriptor
        .capabilities
        .server_self_update
        .as_ref()
        .is_some_and(|method| {
            *method != ServerSelfUpdateCapability::DesktopManaged
                || descriptor.capabilities.desktop_app_update == Some(true)
        });
    let mut failure = ConnectionFailure::new(FailureKind::Unsupported, detail);
    failure.server_update_required = !newer_server && can_update;
    Some(failure)
}
pub fn append_orchestration_protocol(socket_url: &str) -> Result<String, crate::ClientError> {
    use t3_contracts::environment::{
        CLIENT_CAPABILITIES_QUERY_PARAM, ORCHESTRATION_PROTOCOL_QUERY_PARAM,
        ORCHESTRATION_PROTOCOL_VERSION_TEXT, SETTINGS_SECRET_REFERENCES_CAPABILITY,
    };
    let mut url = Url::parse(socket_url)
        .map_err(|_| crate::ClientError::InvalidPayload("invalid socket URL".into()))?;
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| {
            key != ORCHESTRATION_PROTOCOL_QUERY_PARAM && key != CLIENT_CAPABILITIES_QUERY_PARAM
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    pairs.push((
        ORCHESTRATION_PROTOCOL_QUERY_PARAM.into(),
        ORCHESTRATION_PROTOCOL_VERSION_TEXT.into(),
    ));
    pairs.push((
        CLIENT_CAPABILITIES_QUERY_PARAM.into(),
        SETTINGS_SECRET_REFERENCES_CAPABILITY.into(),
    ));
    url.set_query(None);
    url.query_pairs_mut().extend_pairs(pairs);
    Ok(url.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationPhase {
    Available,
    Offline,
    Connecting,
    Reconnecting,
    Connected,
    Error,
    Unsupported,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPresentation {
    pub phase: PresentationPhase,
    pub error: Option<String>,
    pub trace_id: Option<String>,
    /// Reconnecting because the host has not approved this device yet: `error` says where to
    /// approve it and shows the code, and is not a failure.
    pub awaiting_approval: bool,
}
pub fn present_connection_state(state: &ConnectionState) -> ConnectionPresentation {
    if matches!(state.phase, Phase::Connecting | Phase::Backoff)
        && let Some(failure) = state
            .last_failure
            .as_ref()
            .filter(|f| f.kind == FailureKind::ApprovalPending)
    {
        return ConnectionPresentation {
            phase: PresentationPhase::Reconnecting,
            error: Some(failure.detail.clone()),
            trace_id: failure.trace_id.clone(),
            awaiting_approval: true,
        };
    }
    let phase = match state.phase {
        Phase::Available => PresentationPhase::Available,
        Phase::Offline => PresentationPhase::Offline,
        Phase::Connecting if state.attempt <= 1 && state.last_failure.is_none() => {
            PresentationPhase::Connecting
        }
        Phase::Connecting | Phase::Backoff => PresentationPhase::Reconnecting,
        Phase::Connected => PresentationPhase::Connected,
        Phase::Blocked
            if state
                .last_failure
                .as_ref()
                .is_some_and(|f| f.kind == FailureKind::Unsupported) =>
        {
            PresentationPhase::Unsupported
        }
        Phase::Blocked => PresentationPhase::Error,
    };
    let failure = if matches!(
        phase,
        PresentationPhase::Reconnecting | PresentationPhase::Error | PresentationPhase::Unsupported
    ) {
        state.last_failure.as_ref()
    } else {
        None
    };
    ConnectionPresentation {
        phase,
        error: failure.map(|f| f.detail.clone()),
        trace_id: failure.and_then(|f| f.trace_id.clone()),
        awaiting_approval: false,
    }
}
pub fn connection_status_text(presentation: &ConnectionPresentation) -> String {
    match presentation.phase {
        PresentationPhase::Available => "Available".into(),
        PresentationPhase::Offline => "Offline".into(),
        PresentationPhase::Connecting => "Connecting...".into(),
        PresentationPhase::Reconnecting if presentation.awaiting_approval => presentation
            .error
            .clone()
            .unwrap_or_else(|| "Waiting for approval on the host".into()),
        PresentationPhase::Reconnecting => presentation.error.as_ref().map_or_else(
            || "Reconnecting...".into(),
            |error| format!("Failed to connect. Reconnecting... Reason: {error}"),
        ),
        PresentationPhase::Connected => "Connected".into(),
        PresentationPhase::Unsupported => "Client not supported".into(),
        PresentationPhase::Error => presentation.error.as_ref().map_or_else(
            || "Connection failed".into(),
            |error| format!("Connection failed. Reason: {error}"),
        ),
    }
}
pub fn connection_status_title(presentation: &ConnectionPresentation) -> String {
    if presentation.awaiting_approval {
        return "Waiting for approval".into();
    }
    if presentation.phase == PresentationPhase::Reconnecting && presentation.error.is_some() {
        "Failed to connect. Reconnecting...".into()
    } else {
        connection_status_text(&ConnectionPresentation {
            error: None,
            ..presentation.clone()
        })
    }
}
pub fn environment_mcp_url(
    routes: &[Route],
    connected_route: Option<&str>,
    relay_http_base: Option<&str>,
) -> Option<String> {
    let to_mcp = |base: &str| crate::environment::environment_endpoint_url(base, "/mcp").ok();
    if let Some(url) = relay_http_base.and_then(to_mcp) {
        return Some(url);
    }
    let connected = connected_route.and_then(|id| routes.iter().find(|r| r.id == id));
    connected
        .into_iter()
        .chain(routes.iter())
        .filter(|r| r.mode != ConnectionMode::Ssh && !r.http_base.is_empty())
        .find_map(|r| to_mcp(&r.http_base))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFreezeNotice {
    pub title: String,
    pub description: String,
}
pub fn host_freeze_notice(
    host: &str,
    stall: Option<&t3_contracts::server::ServerEventLoopStall>,
) -> Option<HostFreezeNotice> {
    let stall = stall?;
    let load = match &stall.load_average {
        Some(t3_contracts::auth::EncodedNumber::Finite(load)) => load.as_f64(),
        _ => None,
    };
    let description = match load {
        Some(load) if load > stall.cpu_count as f64 => format!(
            "Its CPU is overloaded (load {} on {} cores).",
            load.round(),
            stall.cpu_count
        ),
        _ => format!("T3 Code on {host} was too busy to answer."),
    };
    Some(HostFreezeNotice {
        title: format!(
            "{host} froze for {}s",
            (stall.duration_ms as f64 / 1000.0).round()
        ),
        description,
    })
}

pub const OUTDATED_HOST_RESTART_TIMEOUT_MS: u64 = 240_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutdatedHostUpdateStage {
    Downloading,
    Installing,
    Resuming,
}
#[derive(Debug, Clone, PartialEq)]
pub enum OutdatedHostUpdateResult {
    AlreadyCompatible { target_version: String },
    Updated(t3_contracts::server::ServerSelfUpdateResult),
}
impl ConnectionResolver {
    pub async fn describe_outdated_host(
        &self,
        routes: &[Route],
        identity: &crate::environment::SessionIdentity,
    ) -> Result<crate::authorization::AuthorizedEnvironment, ConnectionFailure> {
        let mut failure = ConnectionFailure::new(
            FailureKind::EndpointUnavailable,
            "Environment did not answer on any saved route.",
        );
        for route in usable_routes(routes, &[]) {
            match self.prepare_for_update(&route, identity).await {
                Ok(prepared) => return Ok(prepared),
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "update transport, destination, input and lifecycle callbacks are separate integration seams"
    )]
    pub async fn update_outdated_host<F: SessionFactory>(
        &self,
        factory: &F,
        routes: &[Route],
        identity: &crate::environment::SessionIdentity,
        input: t3_contracts::server::ServerSelfUpdateInput,
        on_stage: impl Fn(OutdatedHostUpdateStage),
        after_update: impl Fn() -> LocalBoxFuture<'static, Result<(), crate::ClientError>>,
    ) -> Result<OutdatedHostUpdateResult, crate::ClientError> {
        use crate::rpc::RpcEndpoint;
        use t3_contracts::{
            environment::{
                ORCHESTRATION_PROTOCOL_VERSION, ServerSelfUpdateCapability as C,
                ServerSelfUpdateMethod as M,
            },
            rpc::*,
            server::*,
        };
        let prepared = self
            .describe_outdated_host(routes, identity)
            .await
            .map_err(|f| crate::ClientError::InvalidPayload(f.detail))?;
        let descriptor = prepared.descriptor.as_ref().ok_or_else(|| {
            crate::ClientError::InvalidPayload("host descriptor is unavailable".into())
        })?;
        if descriptor.orchestration_protocol_version.unwrap_or(1) == ORCHESTRATION_PROTOCOL_VERSION
        {
            after_update().await?;
            return Ok(OutdatedHostUpdateResult::AlreadyCompatible {
                target_version: descriptor.server_version.0.clone(),
            });
        }
        let capability = descriptor
            .capabilities
            .server_self_update
            .as_ref()
            .filter(|c| {
                **c != C::DesktopManaged || descriptor.capabilities.desktop_app_update == Some(true)
            })
            .ok_or_else(|| {
                crate::ClientError::InvalidPayload(format!(
                    "Update T3 Code on {} manually; it cannot update itself.",
                    descriptor.label.0
                ))
            })?;
        let accepted = || match capability {
            C::BootService | C::Respawn => Some(ServerSelfUpdateResult {
                target_version: input.target_version.clone(),
                method: if *capability == C::BootService {
                    M::BootService
                } else {
                    M::Respawn
                },
                update_id: None,
                desktop_update_token: None,
            }),
            C::DesktopManaged => None,
        };
        let result = {
            let session = match select(
                factory.open(&prepared),
                self.platform.sleep(ESTABLISHMENT_TIMEOUT_MS),
            )
            .await
            {
                Either::Left((result, _)) => {
                    result.map_err(|f| crate::ClientError::InvalidPayload(f.detail))?
                }
                Either::Right(_) => {
                    return Err(crate::ClientError::InvalidPayload(
                        "Outdated host did not open its update connection.".into(),
                    ));
                }
            };
            self.remote
                .validate_session(identity)
                .await
                .map_err(|_| crate::ClientError::Cancelled)?;
            let result = if descriptor.capabilities.server_self_update_progress == Some(true) {
                use crate::services::updates::{UpdateStreamEnd, resolve_update_progress_result};
                let mut stream =
                    session.subscribe::<WsServerUpdateServerWithProgressRpc>(input.clone());
                let mut terminal = None;
                let mut received_progress = false;
                let mut end = UpdateStreamEnd::Complete;
                while let Some(event) = stream.next().await {
                    self.remote
                        .validate_session(identity)
                        .await
                        .map_err(|_| crate::ClientError::Cancelled)?;
                    match event {
                        Ok(ServerSelfUpdateProgressEvent::Complete(event)) => {
                            terminal = Some(event.result)
                        }
                        Ok(ServerSelfUpdateProgressEvent::Progress(event)) => {
                            received_progress = true;
                            on_stage(match event.stage {
                                ServerSelfUpdateProgressStage::Downloading => {
                                    OutdatedHostUpdateStage::Downloading
                                }
                                ServerSelfUpdateProgressStage::Installing => {
                                    OutdatedHostUpdateStage::Installing
                                }
                            });
                        }
                        Err(crate::ClientError::Disconnected) => {
                            end = UpdateStreamEnd::TransportHandoff;
                            break;
                        }
                        Err(crate::ClientError::Cancelled) => {
                            end = UpdateStreamEnd::Cancelled;
                            break;
                        }
                        Err(error) => return Err(error),
                    }
                }
                resolve_update_progress_result(
                    &input.target_version.0,
                    terminal,
                    end,
                    if received_progress { accepted() } else { None },
                )?
            } else {
                match session.call::<WsServerUpdateServerRpc>(input.clone()).await {
                    Ok(result) => result,
                    Err(crate::ClientError::Disconnected) => {
                        accepted().ok_or(crate::ClientError::Disconnected)?
                    }
                    Err(error) => return Err(error),
                }
            };
            self.remote
                .validate_session(identity)
                .await
                .map_err(|_| crate::ClientError::Cancelled)?;
            if result.method == M::DesktopApp
                && let Some(token) = &result.desktop_update_token
            {
                match session
                    .call::<WsServerCommitDesktopUpdateRpc>(DesktopUpdateCommitInput {
                        request_id: token.clone(),
                    })
                    .await
                {
                    Ok(_) | Err(crate::ClientError::Disconnected) => {}
                    Err(error) => return Err(error),
                }
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(|_| crate::ClientError::Cancelled)?;
            }
            result
        };
        on_stage(OutdatedHostUpdateStage::Resuming);
        let mut urls = vec![prepared.http_base_url.clone()];
        for route in routes {
            if !route.http_base.is_empty() && !urls.contains(&route.http_base) {
                urls.push(route.http_base.clone());
            }
        }
        let wait = async {
            loop {
                for url in &urls {
                    let descriptor = crate::environment::fetch_remote_environment_descriptor(
                        self.platform.as_ref(),
                        url,
                        None,
                    )
                    .await;
                    self.remote
                        .validate_session(identity)
                        .await
                        .map_err(|_| crate::ClientError::Cancelled)?;
                    if let Ok(descriptor) = descriptor
                        && descriptor.environment_id == identity.environment
                        && descriptor.orchestration_protocol_version.unwrap_or(1)
                            == ORCHESTRATION_PROTOCOL_VERSION
                    {
                        return Ok::<_, crate::ClientError>(descriptor);
                    }
                }
                self.platform.sleep(1_000).await;
                self.remote
                    .validate_session(identity)
                    .await
                    .map_err(|_| crate::ClientError::Cancelled)?;
            }
        }
        .boxed_local();
        let resumed =
            match select(wait, self.platform.sleep(OUTDATED_HOST_RESTART_TIMEOUT_MS)).await {
                Either::Left((result, _)) => result?,
                Either::Right(_) => {
                    return Err(crate::ClientError::InvalidPayload(format!(
                        "{} did not come back on a compatible T3 Code version.",
                        descriptor.label.0
                    )));
                }
            };
        after_update().await?;
        Ok(OutdatedHostUpdateResult::Updated(ServerSelfUpdateResult {
            target_version: resumed.server_version,
            ..result
        }))
    }
}

impl RouteSelection {
    pub fn new(routes: Vec<Route>, now: u64) -> Self {
        let len = routes.len();
        Self {
            routes,
            checks: vec![
                if len == 1 {
                    Some(RouteCheck::Unchecked)
                } else {
                    None
                };
                len
            ],
            attempted: vec![false; len],
            cursor: 0,
            deferred: Vec::new(),
            head_start: now.saturating_add(ROUTE_CHECK_HEAD_START_MS),
            in_flight: false,
            stopped: false,
            transient: None,
            blocked: None,
        }
    }
    pub fn checked(&mut self, id: &str, check: RouteCheck) {
        if let Some(index) = self.routes.iter().position(|r| r.id == id) {
            self.checks[index] = Some(check);
        }
    }
    pub fn next_route(&mut self, now: u64) -> Option<Route> {
        if self.in_flight || self.stopped {
            return None;
        }
        while self.cursor < self.routes.len() {
            let index = self.cursor;
            if self.checks[index].is_none() && now < self.head_start {
                return None;
            }
            self.cursor += 1;
            if matches!(self.checks[index], None | Some(RouteCheck::Silent)) {
                self.deferred.push(index);
                continue;
            }
            return Some(self.select(index));
        }
        if self
            .deferred
            .iter()
            .any(|&index| self.checks[index].is_none())
        {
            return None;
        }
        let index = self
            .deferred
            .iter()
            .copied()
            .find(|&i| !self.attempted[i] && self.checks[i] != Some(RouteCheck::Silent))
            .or_else(|| {
                if self.transient.is_none() {
                    self.deferred.iter().copied().find(|&i| !self.attempted[i])
                } else {
                    None
                }
            })?;
        Some(self.select(index))
    }
    fn select(&mut self, index: usize) -> Route {
        self.attempted[index] = true;
        self.in_flight = true;
        self.routes[index].clone()
    }
    pub fn failed(&mut self, failure: ConnectionFailure) {
        self.in_flight = false;
        if failure.kind == FailureKind::Unsupported {
            self.stopped = true;
            self.blocked = Some(failure);
            self.transient = None;
        } else if failure.is_blocked() {
            self.blocked.get_or_insert(failure);
        } else {
            self.transient.get_or_insert(failure);
        }
    }
    pub fn failure(&self) -> Option<&ConnectionFailure> {
        self.transient.as_ref().or(self.blocked.as_ref())
    }
    pub fn finished(&self) -> bool {
        self.stopped
            || (!self.in_flight
                && self.cursor == self.routes.len()
                && self.deferred.iter().all(|&i| {
                    self.attempted[i]
                        || self.checks[i]
                            .is_some_and(|c| c == RouteCheck::Silent && self.transient.is_some())
                }))
    }
}

pub trait ConnectionDriver {
    type Lease;
    fn check_route<'a>(&'a self, route: &'a Route) -> LocalBoxFuture<'a, RouteCheck>;
    /// A failed or dropped attempt releases its provisional session before returning.
    fn connect_route<'a>(
        &'a self,
        route: &'a Route,
    ) -> LocalBoxFuture<'a, Result<Self::Lease, ConnectionFailure>>;
}

pub trait SessionFactory {
    type Session: crate::rpc::RpcEndpoint;
    fn open<'a>(
        &'a self,
        prepared: &'a crate::authorization::AuthorizedEnvironment,
    ) -> LocalBoxFuture<'a, Result<Self::Session, ConnectionFailure>>;
    fn ready<'a>(
        &'a self,
        session: &'a Self::Session,
    ) -> LocalBoxFuture<'a, Result<(), ConnectionFailure>>;
}
pub struct EnvironmentConnectionLease<S> {
    pub route_id: String,
    pub prepared: crate::authorization::AuthorizedEnvironment,
    pub session: S,
}
pub struct ResolvedConnectionDriver<F> {
    resolver: ConnectionResolver,
    factory: F,
    identity: crate::environment::SessionIdentity,
    progress: Rc<dyn Fn(AttemptStage)>,
}
impl<F: SessionFactory> ResolvedConnectionDriver<F> {
    pub fn new(
        resolver: ConnectionResolver,
        factory: F,
        identity: crate::environment::SessionIdentity,
        progress: Rc<dyn Fn(AttemptStage)>,
    ) -> Self {
        Self {
            resolver,
            factory,
            identity,
            progress,
        }
    }
    pub async fn connect(
        &self,
        routes: Vec<Route>,
    ) -> Result<EnvironmentConnectionLease<F::Session>, ConnectionFailure> {
        self.resolver
            .remote
            .begin_session(&self.identity)
            .await
            .map_err(|error| map_remote_environment_error(&error, ConnectionVia::Direct))?;
        (self.progress)(AttemptStage::Preparing);
        connect_over_routes(self.resolver.platform.as_ref(), self, routes).await
    }
    pub async fn check_host(
        &self,
        prepared: &crate::authorization::AuthorizedEnvironment,
    ) -> RouteCheck {
        self.check_descriptor(
            &prepared.http_base_url,
            prepared.http_base_url.starts_with("https:"),
        )
        .await
    }
    async fn check_descriptor(&self, base: &str, keeps_host: bool) -> RouteCheck {
        let descriptor = crate::environment::fetch_remote_environment_descriptor(
            self.resolver.platform.as_ref(),
            base,
            Some(ROUTE_CHECK_TIMEOUT_MS),
        )
        .await
        .ok();
        if self
            .resolver
            .remote
            .validate_session(&self.identity)
            .await
            .is_err()
        {
            return RouteCheck::Silent;
        }
        let answered = descriptor
            .as_ref()
            .is_some_and(|d| d.environment_id == self.identity.environment);
        if self
            .resolver
            .remote
            .remember_checked_descriptor(base, descriptor, keeps_host)
            .is_err()
        {
            return RouteCheck::Silent;
        }
        if answered {
            RouteCheck::Answered
        } else {
            RouteCheck::Silent
        }
    }
    pub async fn preflight(
        &self,
        routes: &[Route],
        route: &Route,
        tailscale: bool,
    ) -> RoutePreflight {
        if sends_token_in_clear(routes, route, tailscale) {
            return RoutePreflight::Silent;
        }
        match self.check_route(route).await {
            RouteCheck::Silent => RoutePreflight::Silent,
            RouteCheck::Unchecked => RoutePreflight::Unchecked,
            RouteCheck::Answered => {
                if self.resolver.prepare(route, &self.identity).await.is_ok() {
                    RoutePreflight::Ready
                } else {
                    RoutePreflight::Unprepared
                }
            }
        }
    }
}
impl<F: SessionFactory> ConnectionDriver for ResolvedConnectionDriver<F> {
    type Lease = EnvironmentConnectionLease<F::Session>;
    fn check_route<'a>(&'a self, route: &'a Route) -> LocalBoxFuture<'a, RouteCheck> {
        async move {
            if matches!(route.mode, ConnectionMode::Relay | ConnectionMode::Ssh)
                || route.http_base.is_empty()
            {
                return RouteCheck::Unchecked;
            }
            self.check_descriptor(&route.http_base, route_keeps_its_host(route))
                .await
        }
        .boxed_local()
    }
    fn connect_route<'a>(
        &'a self,
        route: &'a Route,
    ) -> LocalBoxFuture<'a, Result<Self::Lease, ConnectionFailure>> {
        async move {
            (self.progress)(AttemptStage::Preparing);
            let prepared = self.resolver.prepare(route, &self.identity).await?;
            (self.progress)(AttemptStage::Opening);
            let session = self.factory.open(&prepared).await?;
            self.resolver
                .remote
                .validate_session(&self.identity)
                .await
                .map_err(|e| map_remote_environment_error(&e, ConnectionVia::Direct))?;
            (self.progress)(AttemptStage::Synchronizing);
            self.factory.ready(&session).await?;
            self.resolver
                .remote
                .validate_session(&self.identity)
                .await
                .map_err(|e| map_remote_environment_error(&e, ConnectionVia::Direct))?;
            Ok(EnvironmentConnectionLease {
                route_id: route.id.clone(),
                prepared,
                session,
            })
        }
        .boxed_local()
    }
}

pub async fn connect_over_routes<D: ConnectionDriver>(
    platform: &dyn Platform,
    driver: &D,
    routes: Vec<Route>,
) -> Result<D::Lease, ConnectionFailure> {
    let routes = usable_routes(&routes, &[]);
    let started = platform.monotonic_ms();
    let mut selection = RouteSelection::new(routes.clone(), started);
    let mut checks = FuturesUnordered::new();
    if routes.len() > 1 {
        for route in &routes {
            checks.push(async move {
                let result = match select(
                    driver.check_route(route),
                    platform.sleep(ROUTE_CHECK_TIMEOUT_MS),
                )
                .await
                {
                    Either::Left((check, _)) => check,
                    Either::Right(_) => RouteCheck::Silent,
                };
                (&route.id, result)
            });
        }
    }
    loop {
        // Start all checks before selecting the first route, including immediately-ready checks.
        while let Some(Some((id, check))) = checks.next().now_or_never() {
            selection.checked(id, check);
        }
        if let Some(route) = selection.next_route(platform.monotonic_ms()) {
            let mut connection = driver.connect_route(&route);
            let result = loop {
                if checks.is_empty() {
                    break connection.await;
                }
                match select(connection.as_mut(), checks.next()).await {
                    Either::Left((result, _)) => break result,
                    Either::Right((Some((id, check)), _)) => selection.checked(id, check),
                    Either::Right((None, _)) => {}
                }
            };
            match result {
                Ok(lease) => return Ok(lease),
                Err(failure) => selection.failed(failure),
            }
            continue;
        }
        if selection.finished() {
            return Err(selection.failure().cloned().unwrap_or_else(|| {
                ConnectionFailure::new(
                    FailureKind::EndpointUnavailable,
                    "Environment did not answer on any saved route.",
                )
            }));
        }
        let head_start = started.saturating_add(ROUTE_CHECK_HEAD_START_MS);
        if platform.monotonic_ms() < head_start {
            match select(
                checks.next().boxed_local(),
                platform.sleep(head_start - platform.monotonic_ms()),
            )
            .await
            {
                Either::Left((Some((id, check)), _)) => selection.checked(id, check),
                Either::Left((None, _)) | Either::Right(_) => {}
            }
        } else if let Some((id, check)) = checks.next().await {
            selection.checked(id, check);
        }
    }
}

/// The left candidate wins a simultaneous completion; dropping the other cancels its lease scope.
pub async fn race_candidates<T>(
    direct: LocalBoxFuture<'_, Result<T, ConnectionFailure>>,
    relay: LocalBoxFuture<'_, Result<T, ConnectionFailure>>,
) -> Result<T, ConnectionFailure> {
    let (first, other) = match select(direct, relay).await {
        Either::Left((first, other)) | Either::Right((first, other)) => (first, other),
    };
    match first {
        Ok(winner) => Ok(winner),
        Err(error) if error.kind == FailureKind::Unsupported => Err(error),
        Err(first) => match other.await {
            Ok(winner) => Ok(winner),
            Err(second)
                if second.kind == FailureKind::Unsupported
                    || (first.is_blocked() && !second.is_blocked()) =>
            {
                Err(second)
            }
            Err(_) => Err(first),
        },
    }
}

/// The caller retains this stream; dropping it cancels the host event subscription and timer.
pub fn supervise(
    supervisor: Rc<RefCell<Supervisor>>,
    platform: Rc<dyn Platform>,
    events: LocalBoxStream<'static, ConnectionEvent>,
) -> LocalBoxStream<'static, Result<ConnectionAction, crate::ClientError>> {
    struct Driver {
        supervisor: Rc<RefCell<Supervisor>>,
        host_events: LocalBoxStream<'static, PlatformEvent>,
        events: LocalBoxStream<'static, ConnectionEvent>,
        platform: Rc<dyn Platform>,
        pending: VecDeque<ConnectionAction>,
        closed: bool,
        host_closed: bool,
    }
    impl Drop for Driver {
        fn drop(&mut self) {
            self.supervisor.borrow_mut().handle(
                ConnectionEvent::Disconnect,
                Time::from_platform(self.platform.as_ref()),
                0.0,
            );
        }
    }
    let state = Driver {
        supervisor,
        host_events: platform.events(),
        events,
        platform,
        pending: VecDeque::new(),
        closed: false,
        host_closed: false,
    };
    futures::stream::unfold(state, |mut driver| async move {
        loop {
            if let Some(action) = driver.pending.pop_front() {
                return Some((Ok(action), driver));
            }
            if driver.closed {
                return None;
            }
            enum Input {
                Event(Option<ConnectionEvent>),
                Host(Option<PlatformEvent>),
                Timer,
            }
            let deadline = driver.supervisor.borrow().next_deadline();
            let input = {
                let timer = if let Some(deadline) = deadline {
                    driver
                        .platform
                        .sleep(deadline.saturating_sub(driver.platform.monotonic_ms()))
                } else {
                    futures::future::pending().boxed_local()
                };
                let host = if driver.host_closed {
                    futures::future::pending().boxed_local()
                } else {
                    driver.host_events.next().boxed_local()
                };
                let signal = driver.events.next();
                futures::pin_mut!(timer, signal, host);
                // A queued disconnect is consumed before timers or lifecycle wakeups at the same tick.
                match select(signal, select(host, timer).boxed_local()).await {
                    Either::Left((event, _)) => Input::Event(event),
                    Either::Right((Either::Left((event, _)), _)) => Input::Host(event),
                    Either::Right((Either::Right(_), _)) => Input::Timer,
                }
            };
            match input {
                Input::Event(None) => {
                    driver.closed = true;
                    driver.pending.extend(driver.supervisor.borrow_mut().handle(
                        ConnectionEvent::Disconnect,
                        Time::from_platform(driver.platform.as_ref()),
                        0.0,
                    ));
                }
                Input::Host(None) => driver.host_closed = true,
                input => {
                    let time = Time::from_platform(driver.platform.as_ref());
                    let needs_random = matches!(
                        &input,
                        Input::Event(Some(
                            ConnectionEvent::Failed { .. }
                                | ConnectionEvent::Lost { .. }
                                | ConnectionEvent::ProbeResult { result: Err(_), .. }
                        ))
                    ) || {
                        let state = driver.supervisor.borrow();
                        (state.state().phase == Phase::Connecting || state.active_probe().is_some())
                            && state
                                .next_deadline()
                                .is_some_and(|due| due <= time.monotonic_ms)
                    };
                    let random = if needs_random {
                        match driver.platform.random_bytes(8) {
                            Ok(bytes) => match <[u8; 8]>::try_from(bytes) {
                                Ok(bytes) => {
                                    (u64::from_le_bytes(bytes) >> 11) as f64 / (1_u64 << 53) as f64
                                }
                                Err(_) => {
                                    return Some((
                                        Err(crate::ClientError::InvalidPayload(
                                            "host returned invalid random bytes".into(),
                                        )),
                                        driver,
                                    ));
                                }
                            },
                            Err(error) => return Some((Err(error.into()), driver)),
                        }
                    } else {
                        0.0
                    };
                    let actions = match input {
                        Input::Host(Some(event)) => driver
                            .supervisor
                            .borrow_mut()
                            .platform_event(event, time, random),
                        Input::Event(Some(event)) => {
                            driver.supervisor.borrow_mut().handle(event, time, random)
                        }
                        Input::Timer => driver.supervisor.borrow_mut().handle(
                            ConnectionEvent::Tick,
                            time,
                            random,
                        ),
                        _ => Vec::new(),
                    };
                    driver.pending.extend(actions);
                }
            }
        }
    })
    .boxed_local()
}
