use super::super::{Registry, register};
use t3_contracts::server::*;

pub fn register_module(registry: &mut Registry) {
    register::<DesktopUpdateCommitInput>(registry, "DesktopUpdateCommitInput");
    register::<EnvironmentTheme>(registry, "EnvironmentTheme");
    register::<EnvironmentThemeColor>(registry, "EnvironmentThemeColor");
    register::<EnvironmentThemeFile>(registry, "EnvironmentThemeFile");
    register::<EnvironmentThemeId>(registry, "EnvironmentThemeId");
    register::<OtlpEndpointCheckInput>(registry, "OtlpEndpointCheckInput");
    register::<OtlpEndpointCheckResult>(registry, "OtlpEndpointCheckResult");
    register::<OtlpEndpointSource>(registry, "OtlpEndpointSource");
    register::<OtlpSignal>(registry, "OtlpSignal");
    register::<ServerConfig>(registry, "ServerConfig");
    register::<ServerConfigEnvironmentThemesUpdatedPayload>(
        registry,
        "ServerConfigEnvironmentThemesUpdatedPayload",
    );
    register::<ServerConfigIssue>(registry, "ServerConfigIssue");
    register::<ServerConfigKeybindingsUpdatedPayload>(
        registry,
        "ServerConfigKeybindingsUpdatedPayload",
    );
    register::<ServerConfigProviderStatusDeltasPayload>(
        registry,
        "ServerConfigProviderStatusDeltasPayload",
    );
    register::<ServerConfigProviderStatusesPayload>(
        registry,
        "ServerConfigProviderStatusesPayload",
    );
    register::<ServerConfigSettingsUpdatedPayload>(registry, "ServerConfigSettingsUpdatedPayload");
    register::<ServerConfigStreamEnvironmentThemesUpdatedEvent>(
        registry,
        "ServerConfigStreamEnvironmentThemesUpdatedEvent",
    );
    register::<ServerConfigStreamEvent>(registry, "ServerConfigStreamEvent");
    register::<ServerConfigStreamKeybindingsUpdatedEvent>(
        registry,
        "ServerConfigStreamKeybindingsUpdatedEvent",
    );
    register::<ServerConfigStreamProviderStatusDeltasEvent>(
        registry,
        "ServerConfigStreamProviderStatusDeltasEvent",
    );
    register::<ServerConfigStreamProviderStatusesEvent>(
        registry,
        "ServerConfigStreamProviderStatusesEvent",
    );
    register::<ServerConfigStreamSettingsUpdatedEvent>(
        registry,
        "ServerConfigStreamSettingsUpdatedEvent",
    );
    register::<ServerConfigStreamSnapshotEvent>(registry, "ServerConfigStreamSnapshotEvent");
    register::<ServerConfigStreamUsageLimitSourcesUpdatedEvent>(
        registry,
        "ServerConfigStreamUsageLimitSourcesUpdatedEvent",
    );
    register::<ServerConfigUpdatedPayload>(registry, "ServerConfigUpdatedPayload");
    register::<ServerConfigUsageLimitSourcesUpdatedPayload>(
        registry,
        "ServerConfigUsageLimitSourcesUpdatedPayload",
    );
    register::<ServerDirectEndpoint>(registry, "ServerDirectEndpoint");
    register::<ServerDirectEndpointKind>(registry, "ServerDirectEndpointKind");
    register::<ServerEventLoopStall>(registry, "ServerEventLoopStall");
    register::<ServerLifecycleLegacyThreadMigrationPayload>(
        registry,
        "ServerLifecycleLegacyThreadMigrationPayload",
    );
    register::<ServerLifecycleReadyPayload>(registry, "ServerLifecycleReadyPayload");
    register::<ServerLifecycleStreamEvent>(registry, "ServerLifecycleStreamEvent");
    register::<ServerLifecycleStreamLegacyThreadMigrationEvent>(
        registry,
        "ServerLifecycleStreamLegacyThreadMigrationEvent",
    );
    register::<ServerLifecycleStreamReadyEvent>(registry, "ServerLifecycleStreamReadyEvent");
    register::<ServerLifecycleStreamWelcomeEvent>(registry, "ServerLifecycleStreamWelcomeEvent");
    register::<ServerLifecycleWelcomePayload>(registry, "ServerLifecycleWelcomePayload");
    register::<ServerNpmRelease>(registry, "ServerNpmRelease");
    register::<ServerNpmReleaseError>(registry, "ServerNpmReleaseError");
    register::<ServerNpmReleaseInput>(registry, "ServerNpmReleaseInput");
    register::<ServerObservability>(registry, "ServerObservability");
    register::<ServerPrepareTriageError>(registry, "ServerPrepareTriageError");
    register::<ServerPrepareTriageInput>(registry, "ServerPrepareTriageInput");
    register::<ServerPrepareTriageResult>(registry, "ServerPrepareTriageResult");
    register::<ServerProcessDiagnosticsEntry>(registry, "ServerProcessDiagnosticsEntry");
    register::<ServerProcessDiagnosticsResult>(registry, "ServerProcessDiagnosticsResult");
    register::<ServerProcessResourceHistoryBucket>(registry, "ServerProcessResourceHistoryBucket");
    register::<ServerProcessResourceHistoryFailureTag>(
        registry,
        "ServerProcessResourceHistoryFailureTag",
    );
    register::<ServerProcessResourceHistoryInput>(registry, "ServerProcessResourceHistoryInput");
    register::<ServerProcessResourceHistoryResult>(registry, "ServerProcessResourceHistoryResult");
    register::<ServerProcessResourceHistorySummary>(
        registry,
        "ServerProcessResourceHistorySummary",
    );
    register::<ServerProcessSignal>(registry, "ServerProcessSignal");
    register::<ServerProvider>(registry, "ServerProvider");
    register::<ServerProviderAuth>(registry, "ServerProviderAuth");
    register::<ServerProviderAuthStatus>(registry, "ServerProviderAuthStatus");
    register::<ServerProviderAvailability>(registry, "ServerProviderAvailability");
    register::<ServerProviderCompatibilityAdvisory>(
        registry,
        "ServerProviderCompatibilityAdvisory",
    );
    register::<ServerProviderCompatibilityStatus>(registry, "ServerProviderCompatibilityStatus");
    register::<ServerProviderContinuation>(registry, "ServerProviderContinuation");
    register::<ServerProviderDelta>(registry, "ServerProviderDelta");
    register::<ServerProviderModel>(registry, "ServerProviderModel");
    register::<ServerProviderSkill>(registry, "ServerProviderSkill");
    register::<ServerProviderSlashCommand>(registry, "ServerProviderSlashCommand");
    register::<ServerProviderSlashCommandInput>(registry, "ServerProviderSlashCommandInput");
    register::<ServerProviderState>(registry, "ServerProviderState");
    register::<ServerProviderUpdateError>(registry, "ServerProviderUpdateError");
    register::<ServerProviderUpdateInput>(registry, "ServerProviderUpdateInput");
    register::<ServerProviderUpdateRequiredModel>(registry, "ServerProviderUpdateRequiredModel");
    register::<ServerProviderUpdateState>(registry, "ServerProviderUpdateState");
    register::<ServerProviderUpdateStatus>(registry, "ServerProviderUpdateStatus");
    register::<ServerProviderUpdatedPayload>(registry, "ServerProviderUpdatedPayload");
    register::<ServerProviderVersionAdvisory>(registry, "ServerProviderVersionAdvisory");
    register::<ServerProviderVersionAdvisoryStatus>(
        registry,
        "ServerProviderVersionAdvisoryStatus",
    );
    register::<ServerProviderWorkspaceSnapshot>(registry, "ServerProviderWorkspaceSnapshot");
    register::<ServerProviders>(registry, "ServerProviders");
    register::<ServerRemoveKeybindingInput>(registry, "ServerRemoveKeybindingInput");
    register::<ServerRemoveKeybindingResult>(registry, "ServerRemoveKeybindingResult");
    register::<ServerSelfUpdateError>(registry, "ServerSelfUpdateError");
    register::<ServerSelfUpdateInput>(registry, "ServerSelfUpdateInput");
    register::<ServerSelfUpdateOutcome>(registry, "ServerSelfUpdateOutcome");
    register::<ServerSelfUpdateProgressEvent>(registry, "ServerSelfUpdateProgressEvent");
    register::<ServerSelfUpdateProgressStage>(registry, "ServerSelfUpdateProgressStage");
    register::<ServerSelfUpdateResult>(registry, "ServerSelfUpdateResult");
    register::<ServerSignalProcessInput>(registry, "ServerSignalProcessInput");
    register::<ServerSignalProcessResult>(registry, "ServerSignalProcessResult");
    register::<ServerTraceDiagnosticsErrorKind>(registry, "ServerTraceDiagnosticsErrorKind");
    register::<ServerTraceDiagnosticsFailureSummary>(
        registry,
        "ServerTraceDiagnosticsFailureSummary",
    );
    register::<ServerTraceDiagnosticsLogEvent>(registry, "ServerTraceDiagnosticsLogEvent");
    register::<ServerTraceDiagnosticsRecentFailure>(
        registry,
        "ServerTraceDiagnosticsRecentFailure",
    );
    register::<ServerTraceDiagnosticsResult>(registry, "ServerTraceDiagnosticsResult");
    register::<ServerTraceDiagnosticsSpanOccurrence>(
        registry,
        "ServerTraceDiagnosticsSpanOccurrence",
    );
    register::<ServerTraceDiagnosticsSpanSummary>(registry, "ServerTraceDiagnosticsSpanSummary");
    register::<ServerUpsertKeybindingInput>(registry, "ServerUpsertKeybindingInput");
    register::<ServerUpsertKeybindingResult>(registry, "ServerUpsertKeybindingResult");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("server", register_module)
}

#[test]
fn provider_defaults_do_not_turn_omitted_delta_lists_into_updates() -> Result<(), serde_json::Error>
{
    let mut value = serde_json::json!({
        "instanceId": "provider", "driver": "codex", "enabled": true,
        "installed": true, "version": null, "status": "ready",
        "auth": {"status": "unknown"}, "checkedAt": "", "models": [],
    });
    let provider: ServerProvider = serde_json::from_value(value.clone())?;
    value["slashCommands"] = serde_json::json!([]);
    value["skills"] = serde_json::json!([]);
    assert_eq!(serde_json::to_value(provider)?, value);
    for key in ["models", "slashCommands", "skills"] {
        value.as_object_mut().expect("provider object").remove(key);
    }
    let delta: ServerProviderDelta = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(delta)?, value);
    let advisory: ServerProviderVersionAdvisory = serde_json::from_value(serde_json::json!({
        "status": "unknown", "currentVersion": null, "latestVersion": null,
        "updateCommand": null, "checkedAt": null, "message": null,
    }))?;
    assert!(!advisory.can_update);
    Ok(())
}

#[test]
fn compatible_arrays_drop_unreadable_entries() -> Result<(), serde_json::Error> {
    let valid = serde_json::json!({"kind": "keybindings.malformed-config", "message": "bad file"});
    let issues: ServerConfigIssues = serde_json::from_value(serde_json::json!([
        valid, {"kind": "future.issue"}, {"kind": "keybindings.invalid-entry"}, null,
    ]))?;
    assert_eq!(serde_json::to_value(issues)?, serde_json::json!([valid]));
    Ok(())
}
