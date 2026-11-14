use super::{AuthorizationError, Sensitive};
use crate::connection::{
    ConnectionFailure, ConnectionHop, ConnectionVia, FailureCause, FailureKind,
};

pub(super) fn map_token_exchange_error(
    error: AuthorizationError,
    host_label: &str,
    thumbprint: &str,
) -> AuthorizationError {
    if let AuthorizationError::Remote { details, .. } = &error
        && let t3_contracts::environment_http::EnvironmentHttpCommonError::EnvironmentConnectApprovalPendingError(pending) = details.expose().as_ref()
    {
        return AuthorizationError::ApprovalPending(Sensitive(Box::new(approval_pending_failure(host_label, thumbprint, &pending.trace_id.0))));
    }
    error
}

pub fn connect_device_verification_code(thumbprint: &str) -> String {
    const BASE64URL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    const PAIRING: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut code = String::new();
    let prefix: Vec<u16> = thumbprint.encode_utf16().take(8).collect();
    for character in char::decode_utf16(prefix) {
        let index = character
            .ok()
            .and_then(|c| {
                BASE64URL
                    .iter()
                    .position(|byte| u32::from(*byte) == u32::from(c))
            })
            .unwrap_or(0);
        code.push(char::from(PAIRING[index % PAIRING.len()]));
    }
    code.insert(code.len().min(4), '-');
    code
}

pub fn approval_pending_failure(
    host_label: &str,
    thumbprint: &str,
    trace_id: &str,
) -> ConnectionFailure {
    let code = connect_device_verification_code(thumbprint);
    let mut failure = ConnectionFailure::new(
        FailureKind::ApprovalPending,
        format!("Approve this device on {host_label}. It shows code {code}."),
    );
    failure.http_status = Some(403);
    failure.verification_code = Some(code);
    failure.trace_id = Some(trace_id.into());
    failure.hop = Some(ConnectionHop::Server);
    failure.via = Some(ConnectionVia::Relay);
    failure.cause = Some(FailureCause::ApprovalPending);
    failure
}
