use super::super::{Registry, register};
use t3_contracts::auth::*;

pub fn register_module(registry: &mut Registry) {
    register::<AuthRevokeConnectDeviceResult>(registry, "AuthRevokeConnectDeviceResult");
    register::<AuthRevokeConnectDeviceInput>(registry, "AuthRevokeConnectDeviceInput");
    register::<AuthConnectDeviceRequest>(registry, "AuthConnectDeviceRequest");
    register::<AuthConnectDevice>(registry, "AuthConnectDevice");
    register::<AuthApproveConnectDeviceResult>(registry, "AuthApproveConnectDeviceResult");
    register::<AuthApproveConnectDeviceInput>(registry, "AuthApproveConnectDeviceInput");
    register::<AuthAccessSnapshot>(registry, "AuthAccessSnapshot");
    register::<AuthAccessStreamClientRemovedEvent>(registry, "AuthAccessStreamClientRemovedEvent");
    register::<AuthAccessStreamClientUpsertedEvent>(
        registry,
        "AuthAccessStreamClientUpsertedEvent",
    );
    register::<AuthAccessStreamError>(registry, "AuthAccessStreamError");
    register::<AuthAccessStreamEvent>(registry, "AuthAccessStreamEvent");
    register::<AuthAccessStreamPairingLinkRemovedEvent>(
        registry,
        "AuthAccessStreamPairingLinkRemovedEvent",
    );
    register::<AuthAccessStreamPairingLinkUpsertedEvent>(
        registry,
        "AuthAccessStreamPairingLinkUpsertedEvent",
    );
    register::<AuthAccessStreamSnapshotEvent>(registry, "AuthAccessStreamSnapshotEvent");
    register::<AuthAccessTokenResult>(registry, "AuthAccessTokenResult");
    register::<AuthBrowserSessionRequest>(registry, "AuthBrowserSessionRequest");
    register::<AuthBrowserSessionResult>(registry, "AuthBrowserSessionResult");
    register::<AuthClientMetadata>(registry, "AuthClientMetadata");
    register::<AuthClientMetadataDeviceType>(registry, "AuthClientMetadataDeviceType");
    register::<AuthClientPresentationMetadata>(registry, "AuthClientPresentationMetadata");
    register::<AuthClientSession>(registry, "AuthClientSession");
    register::<AuthCreatePairingCredentialInput>(registry, "AuthCreatePairingCredentialInput");
    register::<AuthEnvironmentScope>(registry, "AuthEnvironmentScope");
    register::<AuthEnvironmentScopes>(registry, "AuthEnvironmentScopes");
    register::<AuthGrantScope>(registry, "AuthGrantScope");
    register::<AuthGrantScopes>(registry, "AuthGrantScopes");
    register::<AuthMcpApprovalDecision>(registry, "AuthMcpApprovalDecision");
    register::<AuthMcpApprovalDecisionRequest>(registry, "AuthMcpApprovalDecisionRequest");
    register::<AuthMcpApprovalDetails>(registry, "AuthMcpApprovalDetails");
    register::<AuthMcpApprovalError>(registry, "AuthMcpApprovalError");
    register::<AuthMcpApprovalRedirect>(registry, "AuthMcpApprovalRedirect");
    register::<AuthMcpAuthorizationRequest>(registry, "AuthMcpAuthorizationRequest");
    register::<AuthMcpAuthorizationServerMetadata>(registry, "AuthMcpAuthorizationServerMetadata");
    register::<AuthMcpClientAccess>(registry, "AuthMcpClientAccess");
    register::<AuthMcpClientRegistration>(registry, "AuthMcpClientRegistration");
    register::<AuthMcpProtectedResourceMetadata>(registry, "AuthMcpProtectedResourceMetadata");
    register::<AuthMcpRegisteredClient>(registry, "AuthMcpRegisteredClient");
    register::<AuthMcpRegistrationError>(registry, "AuthMcpRegistrationError");
    register::<AuthMcpTokenError>(registry, "AuthMcpTokenError");
    register::<AuthMcpTokenRequest>(registry, "AuthMcpTokenRequest");
    register::<AuthMcpTokenResult>(registry, "AuthMcpTokenResult");
    register::<AuthPairingCredentialResult>(registry, "AuthPairingCredentialResult");
    register::<AuthPairingLink>(registry, "AuthPairingLink");
    register::<AuthRevokeClientSessionInput>(registry, "AuthRevokeClientSessionInput");
    register::<AuthRevokePairingLinkInput>(registry, "AuthRevokePairingLinkInput");
    register::<AuthSessionState>(registry, "AuthSessionState");
    register::<AuthTokenExchangeRequest>(registry, "AuthTokenExchangeRequest");
    register::<AuthWebSocketTicketParams>(registry, "AuthWebSocketTicketParams");
    register::<AuthWebSocketTicketResult>(registry, "AuthWebSocketTicketResult");
    register::<AuthWebSocketTicketUse>(registry, "AuthWebSocketTicketUse");
    register::<EnvironmentAuthorizationError>(registry, "EnvironmentAuthorizationError");
    register::<ServerAuthBootstrapMethod>(registry, "ServerAuthBootstrapMethod");
    register::<ServerAuthDescriptor>(registry, "ServerAuthDescriptor");
    register::<ServerAuthPolicy>(registry, "ServerAuthPolicy");
    register::<ServerAuthSessionMethod>(registry, "ServerAuthSessionMethod");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("auth", register_module)
}

#[test]
fn permissions_filter_unknown_entries_but_scopes_and_grants_stay_strict()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    let value = json!({
        "authenticated": true,
        "scopes": ["orchestration:read"],
        "permissions": ["orchestration:read", "future:permission", null, 42],
        "sessionMethod": "bearer-access-token",
        "expiresAt": "2026-01-01T00:00:00Z"
    });
    let decoded: AuthBrowserSessionResult = serde_json::from_value(value.clone())?;
    assert_eq!(
        decoded.permissions,
        Some(vec![AuthEnvironmentScope::OrchestrationRead])
    );
    let mut invalid = value.clone();
    invalid["permissions"] = json!(null);
    assert!(serde_json::from_value::<AuthBrowserSessionResult>(invalid).is_err());
    let mut invalid = value.clone();
    invalid["scopes"] = json!(["future:permission"]);
    assert!(serde_json::from_value::<AuthBrowserSessionResult>(invalid).is_err());
    let mut invalid = value;
    invalid["authenticated"] = json!(false);
    assert!(serde_json::from_value::<AuthBrowserSessionResult>(invalid).is_err());
    assert!(serde_json::from_value::<AuthEnvironmentScope>(json!("review:write")).is_ok());
    assert!(serde_json::from_value::<AuthGrantScope>(json!("review:write")).is_err());
    Ok(())
}

#[test]
fn encoded_numbers_preserve_non_finite_markers_and_reject_other_strings()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    for value in [
        json!("NaN"),
        json!("Infinity"),
        json!("-Infinity"),
        json!(9_007_199_254_740_991_u64),
    ] {
        let number: EncodedNumber = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(number)?, value);
    }
    for value in [json!("42"), json!(null), json!(true)] {
        assert!(serde_json::from_value::<EncodedNumber>(value).is_err());
    }
    Ok(())
}
