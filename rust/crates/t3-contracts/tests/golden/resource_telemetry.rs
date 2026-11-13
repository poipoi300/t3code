use super::super::{Registry, register};
use t3_contracts::resource_telemetry::*;

pub fn register_module(registry: &mut Registry) {
    register::<DesktopElectronProcessMetric>(registry, "DesktopElectronProcessMetric");
    register::<DesktopElectronProcessType>(registry, "DesktopElectronProcessType");
    register::<DesktopHostTelemetryHello>(registry, "DesktopHostTelemetryHello");
    register::<DesktopHostTelemetryMessage>(registry, "DesktopHostTelemetryMessage");
    register::<DesktopHostTelemetrySnapshot>(registry, "DesktopHostTelemetrySnapshot");
    register::<DesktopShellEnvironmentSnapshot>(registry, "DesktopShellEnvironmentSnapshot");
    register::<DesktopTelemetryCancelDesktopUpdate>(
        registry,
        "DesktopTelemetryCancelDesktopUpdate",
    );
    register::<DesktopTelemetryCommitDesktopUpdate>(
        registry,
        "DesktopTelemetryCommitDesktopUpdate",
    );
    register::<DesktopTelemetryControlMessage>(registry, "DesktopTelemetryControlMessage");
    register::<DesktopTelemetryRequestDesktopUpdate>(
        registry,
        "DesktopTelemetryRequestDesktopUpdate",
    );
    register::<DesktopTelemetrySetDiagnosticsDemand>(
        registry,
        "DesktopTelemetrySetDiagnosticsDemand",
    );
    register::<DesktopTelemetrySetHostPowerIntervals>(
        registry,
        "DesktopTelemetrySetHostPowerIntervals",
    );
    register::<DesktopUpdateRemoteOutcome>(registry, "DesktopUpdateRemoteOutcome");
    register::<DesktopUpdateStatusReport>(registry, "DesktopUpdateStatusReport");
    register::<HostResourcesSnapshot>(registry, "HostResourcesSnapshot");
    register::<ResourceAttributionEntry>(registry, "ResourceAttributionEntry");
    register::<ResourceAttributionSnapshot>(registry, "ResourceAttributionSnapshot");
    register::<ResourceMonitorCapabilities>(registry, "ResourceMonitorCapabilities");
    register::<ResourceMonitorCommand>(registry, "ResourceMonitorCommand");
    register::<ResourceMonitorConfigureCommand>(registry, "ResourceMonitorConfigureCommand");
    register::<ResourceMonitorErrorEvent>(registry, "ResourceMonitorErrorEvent");
    register::<ResourceMonitorEvent>(registry, "ResourceMonitorEvent");
    register::<ResourceMonitorExternalProcess>(registry, "ResourceMonitorExternalProcess");
    register::<ResourceMonitorHelloEvent>(registry, "ResourceMonitorHelloEvent");
    register::<ResourceMonitorHistoryChunkEvent>(registry, "ResourceMonitorHistoryChunkEvent");
    register::<ResourceMonitorProcessSample>(registry, "ResourceMonitorProcessSample");
    register::<ResourceMonitorProcessTableCommand>(registry, "ResourceMonitorProcessTableCommand");
    register::<ResourceMonitorProcessTableEntry>(registry, "ResourceMonitorProcessTableEntry");
    register::<ResourceMonitorProcessTableEvent>(registry, "ResourceMonitorProcessTableEvent");
    register::<ResourceMonitorReadHistoryCommand>(registry, "ResourceMonitorReadHistoryCommand");
    register::<ResourceMonitorSampleNowCommand>(registry, "ResourceMonitorSampleNowCommand");
    register::<ResourceMonitorSetExternalProcessesCommand>(
        registry,
        "ResourceMonitorSetExternalProcessesCommand",
    );
    register::<ResourceMonitorSetSampleIntervalCommand>(
        registry,
        "ResourceMonitorSetSampleIntervalCommand",
    );
    register::<ResourceMonitorSetStreamingCommand>(registry, "ResourceMonitorSetStreamingCommand");
    register::<ResourceMonitorShutdownCommand>(registry, "ResourceMonitorShutdownCommand");
    register::<ResourceMonitorSnapshotEvent>(registry, "ResourceMonitorSnapshotEvent");
    register::<ResourceTelemetryAggregate>(registry, "ResourceTelemetryAggregate");
    register::<ResourceTelemetryGroups>(registry, "ResourceTelemetryGroups");
    register::<ResourceTelemetryHealth>(registry, "ResourceTelemetryHealth");
    register::<ResourceTelemetryHistory>(registry, "ResourceTelemetryHistory");
    register::<ResourceTelemetryHistoryBucket>(registry, "ResourceTelemetryHistoryBucket");
    register::<ResourceTelemetryHistoryInput>(registry, "ResourceTelemetryHistoryInput");
    register::<ResourceTelemetryIoSemantics>(registry, "ResourceTelemetryIoSemantics");
    register::<ResourceTelemetryProcess>(registry, "ResourceTelemetryProcess");
    register::<ResourceTelemetryProcessCategory>(registry, "ResourceTelemetryProcessCategory");
    register::<ResourceTelemetryProcessIdentity>(registry, "ResourceTelemetryProcessIdentity");
    register::<ResourceTelemetryProcessSummary>(registry, "ResourceTelemetryProcessSummary");
    register::<ResourceTelemetryRetryResult>(registry, "ResourceTelemetryRetryResult");
    register::<ResourceTelemetrySnapshot>(registry, "ResourceTelemetrySnapshot");
    register::<ResourceTelemetrySourceHealth>(registry, "ResourceTelemetrySourceHealth");
    register::<ResourceTelemetrySourceStatus>(registry, "ResourceTelemetrySourceStatus");
    register::<ShellEnvironmentPatch>(registry, "ShellEnvironmentPatch");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("resource_telemetry", register_module)
}
