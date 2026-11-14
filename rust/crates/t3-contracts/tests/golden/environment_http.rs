use super::super::{Registry, register};
use t3_contracts::environment_http::*;

pub fn register_module(registry: &mut Registry) {
    register::<EnvironmentConnectApprovalPendingError>(
        registry,
        "EnvironmentConnectApprovalPendingError",
    );
    register::<AuthClientSessionRevokeResult>(registry, "AuthClientSessionRevokeResult");
    register::<AuthOtherClientSessionsRevokeResult>(
        registry,
        "AuthOtherClientSessionsRevokeResult",
    );
    register::<AuthPairingLinkRevokeResult>(registry, "AuthPairingLinkRevokeResult");
    register::<EnvironmentAuthInvalidError>(registry, "EnvironmentAuthInvalidError");
    register::<EnvironmentAuthInvalidReason>(registry, "EnvironmentAuthInvalidReason");
    register::<EnvironmentCloudEndpointUnavailableError>(
        registry,
        "EnvironmentCloudEndpointUnavailableError",
    );
    register::<EnvironmentCloudLinkStateResult>(registry, "EnvironmentCloudLinkStateResult");
    register::<EnvironmentCloudPreferencesRequest>(registry, "EnvironmentCloudPreferencesRequest");
    register::<EnvironmentCloudRelayConfigResult>(registry, "EnvironmentCloudRelayConfigResult");
    register::<EnvironmentConnectReachability>(registry, "EnvironmentConnectReachability");
    register::<EnvironmentConnectReachabilityCause>(
        registry,
        "EnvironmentConnectReachabilityCause",
    );
    register::<EnvironmentHttpBadRequestError>(registry, "EnvironmentHttpBadRequestError");
    register::<EnvironmentHttpCommonError>(registry, "EnvironmentHttpCommonError");
    register::<EnvironmentHttpConflictError>(registry, "EnvironmentHttpConflictError");
    register::<EnvironmentHttpForbiddenError>(registry, "EnvironmentHttpForbiddenError");
    register::<EnvironmentHttpInternalServerError>(registry, "EnvironmentHttpInternalServerError");
    register::<EnvironmentHttpUnauthorizedError>(registry, "EnvironmentHttpUnauthorizedError");
    register::<EnvironmentInternalError>(registry, "EnvironmentInternalError");
    register::<EnvironmentInternalErrorReason>(registry, "EnvironmentInternalErrorReason");
    register::<EnvironmentOperationForbiddenError>(registry, "EnvironmentOperationForbiddenError");
    register::<EnvironmentOperationForbiddenReason>(
        registry,
        "EnvironmentOperationForbiddenReason",
    );
    register::<EnvironmentOrchestrationShellSnapshotQuery>(
        registry,
        "EnvironmentOrchestrationShellSnapshotQuery",
    );
    register::<EnvironmentOrchestrationThreadBoundedSnapshotQuery>(
        registry,
        "EnvironmentOrchestrationThreadBoundedSnapshotQuery",
    );
    register::<EnvironmentOrchestrationThreadHistoryQuery>(
        registry,
        "EnvironmentOrchestrationThreadHistoryQuery",
    );
    register::<EnvironmentRequestInvalidError>(registry, "EnvironmentRequestInvalidError");
    register::<EnvironmentRequestInvalidReason>(registry, "EnvironmentRequestInvalidReason");
    register::<EnvironmentResourceNotFoundError>(registry, "EnvironmentResourceNotFoundError");
    register::<EnvironmentResourceNotFoundReason>(registry, "EnvironmentResourceNotFoundReason");
    register::<EnvironmentScopeRequiredError>(registry, "EnvironmentScopeRequiredError");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("environment_http", register_module)
}
