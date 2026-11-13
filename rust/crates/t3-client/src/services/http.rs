use crate::{
    authorization::{AuthorizationError, AuthorizedEnvironment, HttpAuthorization, Sensitive},
    device::DeviceHubAccess,
    permissions::EnvironmentHttpClient,
};
use futures::future::LocalBoxFuture;
use serde_json::Value;
use std::collections::BTreeMap;
use t3_contracts::{auth::*, environment_http::EnvironmentHttpCommonError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerPairingCodeError {
    #[error("Your session there cannot create pairing codes. Paste a code from there instead.")]
    NotAllowed,
    #[error("Could not get a pairing code from there. Paste a code from there instead.")]
    Unavailable,
}
impl From<AuthorizationError> for PeerPairingCodeError {
    fn from(error: AuthorizationError) -> Self {
        match error {
            AuthorizationError::Remote { details, .. }
                if matches!(
                    details.expose().as_ref(),
                    EnvironmentHttpCommonError::EnvironmentScopeRequiredError(_)
                        | EnvironmentHttpCommonError::EnvironmentOperationForbiddenError(_)
                ) =>
            {
                Self::NotAllowed
            }
            _ => Self::Unavailable,
        }
    }
}
pub fn peer_pairing_payload(access: AuthMcpClientAccess) -> AuthCreatePairingCredentialInput {
    let scopes = match access {
        AuthMcpClientAccess::ReadOnly => vec![AuthGrantScope::OrchestrationRead],
        AuthMcpClientAccess::FullAccess => vec![
            AuthGrantScope::OrchestrationRead,
            AuthGrantScope::OrchestrationOperate,
            AuthGrantScope::SettingsWrite,
            AuthGrantScope::SourceControlWrite,
        ],
        _ => vec![
            AuthGrantScope::OrchestrationRead,
            AuthGrantScope::OrchestrationOperate,
        ],
    };
    AuthCreatePairingCredentialInput {
        label: Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
            "T3 Code link".into(),
        )),
        scopes: Some(scopes),
        access: None,
    }
}
pub async fn mint_peer_pairing_code(
    client: &EnvironmentHttpClient,
    access: AuthMcpClientAccess,
) -> Result<Sensitive<String>, PeerPairingCodeError> {
    let minted = client
        .create_pairing_credential(&peer_pairing_payload(access))
        .await?;
    Ok(Sensitive(minted.into_inner().credential.0))
}
pub async fn mint_mcp_approval_code(
    client: &EnvironmentHttpClient,
    access: AuthMcpClientAccess,
) -> Result<Sensitive<AuthPairingCredentialResult>, PeerPairingCodeError> {
    client
        .create_pairing_credential(&AuthCreatePairingCredentialInput {
            label: Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "App sign-in".into(),
            )),
            scopes: None,
            access: Some(access),
        })
        .await
        .map_err(Into::into)
}

pub async fn resolve_device_hub_access(
    client: &EnvironmentHttpClient,
    prepared: &AuthorizedEnvironment,
    hub_base_path: &str,
) -> Result<Sensitive<DeviceHubAccess>, AuthorizationError> {
    let http_base =
        crate::environment::environment_endpoint_url(&prepared.http_base_url, hub_base_path)
            .map_err(|_| AuthorizationError::InvalidUrl)?;
    let mut ws_base = url::Url::parse(&http_base).map_err(|_| AuthorizationError::InvalidUrl)?;
    let scheme = match ws_base.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => return Err(AuthorizationError::InvalidUrl),
    };
    ws_base
        .set_scheme(scheme)
        .map_err(|_| AuthorizationError::InvalidUrl)?;
    let cookie = matches!(prepared.authorization, HttpAuthorization::Cookie);
    let query = if cookie {
        BTreeMap::new()
    } else {
        let ticket = client
            .execute::<AuthWebSocketTicketResult>(
                "POST",
                "/api/auth/websocket-ticket/media",
                None,
                8_000,
            )
            .await?
            .into_inner();
        BTreeMap::from([("wsTicket".into(), ticket.ticket.0)])
    };
    Ok(Sensitive(DeviceHubAccess {
        http_base,
        ws_base: ws_base.into(),
        query,
        credentials: cookie,
    }))
}

#[derive(Clone, PartialEq, Eq)]
pub enum ConnectMcpApprovalAnswer {
    Details(AuthMcpApprovalDetails),
    Redirect(String),
    Refused(String),
}
#[derive(Clone)]
pub enum ConnectMcpApprovalCall {
    Approval(AuthMcpAuthorizationRequest),
    Decision(AuthMcpApprovalDecisionRequest),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Could not reach that environment through T3 Connect with this account.")]
pub struct ConnectMcpApprovalUnavailable;

/// The auth adapter must retry a DPoP rejection once, including a successful response without CSRF.
pub trait ConnectApprovalHttp {
    fn post(
        &self,
        path: &'static str,
        payload: Value,
        timeout_ms: u64,
        unauthorized: fn(&Value) -> bool,
    ) -> LocalBoxFuture<'_, Result<Sensitive<Value>, AuthorizationError>>;
}
pub fn approval_without_csrf(value: &Value) -> bool {
    serde_json::from_value::<AuthMcpApprovalDetails>(value.clone())
        .is_ok_and(|details| details.csrf_token.is_none())
}
pub fn approval_answer(value: Value) -> Result<ConnectMcpApprovalAnswer, AuthorizationError> {
    if let Ok(redirect) = serde_json::from_value::<AuthMcpApprovalRedirect>(value.clone()) {
        return Ok(ConnectMcpApprovalAnswer::Redirect(redirect.redirect_to));
    }
    if let Ok(refused) = serde_json::from_value::<AuthMcpApprovalError>(value.clone()) {
        return Ok(ConnectMcpApprovalAnswer::Refused(refused.message));
    }
    serde_json::from_value(value)
        .map(ConnectMcpApprovalAnswer::Details)
        .map_err(|_| AuthorizationError::InvalidResponse)
}
pub async fn call_connect_mcp_approval(
    http: &impl ConnectApprovalHttp,
    call: ConnectMcpApprovalCall,
) -> Result<Sensitive<ConnectMcpApprovalAnswer>, ConnectMcpApprovalUnavailable> {
    let (path, payload, unauthorized): (_, _, fn(&Value) -> bool) = match call {
        ConnectMcpApprovalCall::Approval(payload) => (
            "/oauth/mcp/approval",
            serde_json::to_value(payload),
            approval_without_csrf,
        ),
        ConnectMcpApprovalCall::Decision(payload) => {
            ("/oauth/mcp/decision", serde_json::to_value(payload), |_| {
                false
            })
        }
    };
    let value = http
        .post(
            path,
            payload.map_err(|_| ConnectMcpApprovalUnavailable)?,
            10_000,
            unauthorized,
        )
        .await
        .map_err(|_| ConnectMcpApprovalUnavailable)?;
    approval_answer(value.into_inner())
        .map(Sensitive)
        .map_err(|_| ConnectMcpApprovalUnavailable)
}
