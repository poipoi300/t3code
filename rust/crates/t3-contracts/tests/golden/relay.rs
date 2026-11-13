use super::super::{Registry, register};
use t3_contracts::relay::*;

pub fn register_module(registry: &mut Registry) {
    register::<RelayAgentActivityAggregateRow>(registry, "RelayAgentActivityAggregateRow");
    register::<RelayAgentActivityAggregateState>(registry, "RelayAgentActivityAggregateState");
    register::<RelayAgentActivityPublishProofExpiredError>(
        registry,
        "RelayAgentActivityPublishProofExpiredError",
    );
    register::<RelayAgentActivityPublishProofInvalidError>(
        registry,
        "RelayAgentActivityPublishProofInvalidError",
    );
    register::<RelayAgentActivityPublishProofInvalidReason>(
        registry,
        "RelayAgentActivityPublishProofInvalidReason",
    );
    register::<RelayAgentActivityPublishProofPayload>(
        registry,
        "RelayAgentActivityPublishProofPayload",
    );
    register::<RelayAgentActivityPublishRequest>(registry, "RelayAgentActivityPublishRequest");
    register::<RelayAgentActivitySnapshotResponse>(registry, "RelayAgentActivitySnapshotResponse");
    register::<RelayAgentActivityState>(registry, "RelayAgentActivityState");
    register::<RelayAgentAwarenessPhase>(registry, "RelayAgentAwarenessPhase");
    register::<RelayAgentAwarenessPlatform>(registry, "RelayAgentAwarenessPlatform");
    register::<RelayAgentAwarenessPreferences>(registry, "RelayAgentAwarenessPreferences");
    register::<RelayApnsEnvironment>(registry, "RelayApnsEnvironment");
    register::<RelayAuthInvalidError>(registry, "RelayAuthInvalidError");
    register::<RelayAuthInvalidReason>(registry, "RelayAuthInvalidReason");
    register::<RelayAuthorizationServerMetadata>(registry, "RelayAuthorizationServerMetadata");
    register::<RelayBearerRequestHeaders>(registry, "RelayBearerRequestHeaders");
    register::<RelayClientDeviceRecord>(registry, "RelayClientDeviceRecord");
    register::<RelayClientEnvironmentRecord>(registry, "RelayClientEnvironmentRecord");
    register::<RelayCloudEnvironmentHealthProof>(registry, "RelayCloudEnvironmentHealthProof");
    register::<RelayCloudEnvironmentHealthProofPayload>(
        registry,
        "RelayCloudEnvironmentHealthProofPayload",
    );
    register::<RelayCloudEnvironmentHealthRequest>(registry, "RelayCloudEnvironmentHealthRequest");
    register::<RelayCloudMintCredentialProof>(registry, "RelayCloudMintCredentialProof");
    register::<RelayCloudMintCredentialProofPayload>(
        registry,
        "RelayCloudMintCredentialProofPayload",
    );
    register::<RelayCloudMintCredentialRequest>(registry, "RelayCloudMintCredentialRequest");
    register::<RelayDeliveryKind>(registry, "RelayDeliveryKind");
    register::<RelayDeliveryResult>(registry, "RelayDeliveryResult");
    register::<RelayDeviceRegistrationRequest>(registry, "RelayDeviceRegistrationRequest");
    register::<RelayDeviceUnregistrationParams>(registry, "RelayDeviceUnregistrationParams");
    register::<RelayDpopAccessTokenRequest>(registry, "RelayDpopAccessTokenRequest");
    register::<RelayDpopAccessTokenResponse>(registry, "RelayDpopAccessTokenResponse");
    register::<RelayDpopAccessTokenScope>(registry, "RelayDpopAccessTokenScope");
    register::<RelayDpopFailureReason>(registry, "RelayDpopFailureReason");
    register::<RelayDpopProofRequestHeaders>(registry, "RelayDpopProofRequestHeaders");
    register::<RelayDpopRequestHeaders>(registry, "RelayDpopRequestHeaders");
    register::<RelayEnvironmentConfigRequest>(registry, "RelayEnvironmentConfigRequest");
    register::<RelayEnvironmentConnectNotAuthorizedError>(
        registry,
        "RelayEnvironmentConnectNotAuthorizedError",
    );
    register::<RelayEnvironmentConnectNotAuthorizedReason>(
        registry,
        "RelayEnvironmentConnectNotAuthorizedReason",
    );
    register::<RelayEnvironmentConnectRequest>(registry, "RelayEnvironmentConnectRequest");
    register::<RelayEnvironmentConnectResponse>(registry, "RelayEnvironmentConnectResponse");
    register::<RelayEnvironmentEndpointFailure>(registry, "RelayEnvironmentEndpointFailure");
    register::<RelayEnvironmentEndpointTimedOutError>(
        registry,
        "RelayEnvironmentEndpointTimedOutError",
    );
    register::<RelayEnvironmentEndpointUnavailableError>(
        registry,
        "RelayEnvironmentEndpointUnavailableError",
    );
    register::<RelayEnvironmentEndpointUnavailableReason>(
        registry,
        "RelayEnvironmentEndpointUnavailableReason",
    );
    register::<RelayEnvironmentHealthResponse>(registry, "RelayEnvironmentHealthResponse");
    register::<RelayEnvironmentHealthResponseProofPayload>(
        registry,
        "RelayEnvironmentHealthResponseProofPayload",
    );
    register::<RelayEnvironmentLinkChallengeRequest>(
        registry,
        "RelayEnvironmentLinkChallengeRequest",
    );
    register::<RelayEnvironmentLinkChallengeResponse>(
        registry,
        "RelayEnvironmentLinkChallengeResponse",
    );
    register::<RelayEnvironmentLinkFailedError>(registry, "RelayEnvironmentLinkFailedError");
    register::<RelayEnvironmentLinkFailedReason>(registry, "RelayEnvironmentLinkFailedReason");
    register::<RelayEnvironmentLinkLimitExceededError>(
        registry,
        "RelayEnvironmentLinkLimitExceededError",
    );
    register::<RelayEnvironmentLinkPreferencesRequest>(
        registry,
        "RelayEnvironmentLinkPreferencesRequest",
    );
    register::<RelayEnvironmentLinkProof>(registry, "RelayEnvironmentLinkProof");
    register::<RelayEnvironmentLinkProofExpiredError>(
        registry,
        "RelayEnvironmentLinkProofExpiredError",
    );
    register::<RelayEnvironmentLinkProofInvalidError>(
        registry,
        "RelayEnvironmentLinkProofInvalidError",
    );
    register::<RelayEnvironmentLinkProofInvalidReason>(
        registry,
        "RelayEnvironmentLinkProofInvalidReason",
    );
    register::<RelayEnvironmentLinkProofPayload>(registry, "RelayEnvironmentLinkProofPayload");
    register::<RelayEnvironmentLinkRequest>(registry, "RelayEnvironmentLinkRequest");
    register::<RelayEnvironmentLinkResponse>(registry, "RelayEnvironmentLinkResponse");
    register::<RelayEnvironmentLinkScope>(registry, "RelayEnvironmentLinkScope");
    register::<RelayEnvironmentLinkUnavailableError>(
        registry,
        "RelayEnvironmentLinkUnavailableError",
    );
    register::<RelayEnvironmentLinkUnavailableReason>(
        registry,
        "RelayEnvironmentLinkUnavailableReason",
    );
    register::<RelayEnvironmentMintResponse>(registry, "RelayEnvironmentMintResponse");
    register::<RelayEnvironmentMintResponseProofPayload>(
        registry,
        "RelayEnvironmentMintResponseProofPayload",
    );
    register::<RelayEnvironmentStatusResponse>(registry, "RelayEnvironmentStatusResponse");
    register::<RelayEnvironmentStatusValue>(registry, "RelayEnvironmentStatusValue");
    register::<RelayEnvironmentUnlinkParams>(registry, "RelayEnvironmentUnlinkParams");
    register::<RelayHealthResponse>(registry, "RelayHealthResponse");
    register::<RelayHookDeliveryProofPayload>(registry, "RelayHookDeliveryProofPayload");
    register::<RelayInternalError>(registry, "RelayInternalError");
    register::<RelayInternalErrorReason>(registry, "RelayInternalErrorReason");
    register::<RelayLinkProofRequest>(registry, "RelayLinkProofRequest");
    register::<RelayListDevicesResponse>(registry, "RelayListDevicesResponse");
    register::<RelayListDevicesResponseV1>(registry, "RelayListDevicesResponseV1");
    register::<RelayListEnvironmentsResponse>(registry, "RelayListEnvironmentsResponse");
    register::<RelayLiveActivityRegistrationRequest>(
        registry,
        "RelayLiveActivityRegistrationRequest",
    );
    register::<RelayManagedEndpoint>(registry, "RelayManagedEndpoint");
    register::<RelayManagedEndpointOrigin>(registry, "RelayManagedEndpointOrigin");
    register::<RelayManagedEndpointProviderKind>(registry, "RelayManagedEndpointProviderKind");
    register::<RelayManagedEndpointRecoveryProofPayload>(
        registry,
        "RelayManagedEndpointRecoveryProofPayload",
    );
    register::<RelayManagedEndpointRecoveryRegistrationRequest>(
        registry,
        "RelayManagedEndpointRecoveryRegistrationRequest",
    );
    register::<RelayManagedEndpointRecoveryRegistrationResponse>(
        registry,
        "RelayManagedEndpointRecoveryRegistrationResponse",
    );
    register::<RelayManagedEndpointRecoveryRequest>(
        registry,
        "RelayManagedEndpointRecoveryRequest",
    );
    register::<RelayManagedEndpointRecoveryResponse>(
        registry,
        "RelayManagedEndpointRecoveryResponse",
    );
    register::<RelayManagedEndpointRuntimeConfig>(registry, "RelayManagedEndpointRuntimeConfig");
    register::<RelayMobileTelemetrySignal>(registry, "RelayMobileTelemetrySignal");
    register::<RelayOkResponse>(registry, "RelayOkResponse");
    register::<RelayProtectedError>(registry, "RelayProtectedError");
    register::<RelayProtectedResourceMetadata>(registry, "RelayProtectedResourceMetadata");
    register::<RelayPublicClientId>(registry, "RelayPublicClientId");
    register::<RelayPublishResponse>(registry, "RelayPublishResponse");
    register::<RelayT3ChatLink>(registry, "RelayT3ChatLink");
    register::<RelayT3ChatLinkRequestResponse>(registry, "RelayT3ChatLinkRequestResponse");
    register::<RelayT3ChatLinkResponse>(registry, "RelayT3ChatLinkResponse");
    register::<RelayT3ChatPlan>(registry, "RelayT3ChatPlan");
    register::<RelayWakeHeldHooksResponse>(registry, "RelayWakeHeldHooksResponse");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("relay", register_module)
}
