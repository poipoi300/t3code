use super::super::{Registry, register};
use t3_contracts::acp_registry::*;
pub fn register_module(registry: &mut Registry) {
    register::<AcpRegistryAcceptUrlAuthInput>(registry, "AcpRegistryAcceptUrlAuthInput");
    register::<AcpRegistryAcceptUrlAuthResult>(registry, "AcpRegistryAcceptUrlAuthResult");
    register::<AcpRegistryConfigurableProvider>(registry, "AcpRegistryConfigurableProvider");
    register::<AcpRegistryDeleteSessionInput>(registry, "AcpRegistryDeleteSessionInput");
    register::<AcpRegistryDeleteSessionResult>(registry, "AcpRegistryDeleteSessionResult");
    register::<AcpRegistryDisableProviderInput>(registry, "AcpRegistryDisableProviderInput");
    register::<AcpRegistryDisableProviderResult>(registry, "AcpRegistryDisableProviderResult");
    register::<AcpRegistryDistribution>(registry, "AcpRegistryDistribution");
    register::<AcpRegistryImportSessionInput>(registry, "AcpRegistryImportSessionInput");
    register::<AcpRegistryImportSessionResult>(registry, "AcpRegistryImportSessionResult");
    register::<AcpRegistryIntegrity>(registry, "AcpRegistryIntegrity");
    register::<AcpRegistryListProvidersInput>(registry, "AcpRegistryListProvidersInput");
    register::<AcpRegistryListProvidersResult>(registry, "AcpRegistryListProvidersResult");
    register::<AcpRegistryListSessionsInput>(registry, "AcpRegistryListSessionsInput");
    register::<AcpRegistryListSessionsResult>(registry, "AcpRegistryListSessionsResult");
    register::<AcpRegistryLogoutInput>(registry, "AcpRegistryLogoutInput");
    register::<AcpRegistryLogoutResult>(registry, "AcpRegistryLogoutResult");
    register::<AcpRegistryManagedBinaryUninstallInput>(
        registry,
        "AcpRegistryManagedBinaryUninstallInput",
    );
    register::<AcpRegistryManagedBinaryUninstallResult>(
        registry,
        "AcpRegistryManagedBinaryUninstallResult",
    );
    register::<AcpRegistryOperationError>(registry, "AcpRegistryOperationError");
    register::<AcpRegistryOperationErrorReason>(registry, "AcpRegistryOperationErrorReason");
    register::<AcpRegistryPrepareInput>(registry, "AcpRegistryPrepareInput");
    register::<AcpRegistryPrepareResult>(registry, "AcpRegistryPrepareResult");
    register::<AcpRegistryProbeAuthMethod>(registry, "AcpRegistryProbeAuthMethod");
    register::<AcpRegistryProbeModel>(registry, "AcpRegistryProbeModel");
    register::<AcpRegistryProbeResult>(registry, "AcpRegistryProbeResult");
    register::<AcpRegistrySearchAgent>(registry, "AcpRegistrySearchAgent");
    register::<AcpRegistrySearchInput>(registry, "AcpRegistrySearchInput");
    register::<AcpRegistrySearchResult>(registry, "AcpRegistrySearchResult");
    register::<AcpRegistrySession>(registry, "AcpRegistrySession");
    register::<AcpRegistrySetProviderInput>(registry, "AcpRegistrySetProviderInput");
    register::<AcpRegistrySetProviderResult>(registry, "AcpRegistrySetProviderResult");
    register::<AcpRegistryUrlAuthAction>(registry, "AcpRegistryUrlAuthAction");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("acp_registry", register_module)
}
