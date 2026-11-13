use super::super::{Registry, register};
use t3_contracts::preview_automation::*;

pub fn register_module(registry: &mut Registry) {
    register::<BrowserNavigationTarget>(registry, "BrowserNavigationTarget");
    register::<McpCapabilityUnavailableError>(registry, "McpCapabilityUnavailableError");
    register::<PreviewAutomationActInput>(registry, "PreviewAutomationActInput");
    register::<PreviewAutomationActionEvent>(registry, "PreviewAutomationActionEvent");
    register::<PreviewAutomationClickInput>(registry, "PreviewAutomationClickInput");
    register::<PreviewAutomationClientDisconnectedError>(
        registry,
        "PreviewAutomationClientDisconnectedError",
    );
    register::<PreviewAutomationClientId>(registry, "PreviewAutomationClientId");
    register::<PreviewAutomationColorScheme>(registry, "PreviewAutomationColorScheme");
    register::<PreviewAutomationConnectionId>(registry, "PreviewAutomationConnectionId");
    register::<PreviewAutomationConsoleEntry>(registry, "PreviewAutomationConsoleEntry");
    register::<PreviewAutomationControlInterruptedError>(
        registry,
        "PreviewAutomationControlInterruptedError",
    );
    register::<PreviewAutomationControlReason>(registry, "PreviewAutomationControlReason");
    register::<PreviewAutomationDialogInput>(registry, "PreviewAutomationDialogInput");
    register::<PreviewAutomationDragInput>(registry, "PreviewAutomationDragInput");
    register::<PreviewAutomationElement>(registry, "PreviewAutomationElement");
    register::<PreviewAutomationError>(registry, "PreviewAutomationError");
    register::<PreviewAutomationEvaluateInput>(registry, "PreviewAutomationEvaluateInput");
    register::<PreviewAutomationExecutionError>(registry, "PreviewAutomationExecutionError");
    register::<PreviewAutomationHost>(registry, "PreviewAutomationHost");
    register::<PreviewAutomationHostFocus>(registry, "PreviewAutomationHostFocus");
    register::<PreviewAutomationHostIdentity>(registry, "PreviewAutomationHostIdentity");
    register::<PreviewAutomationHoverInput>(registry, "PreviewAutomationHoverInput");
    register::<PreviewAutomationInvalidSelectorError>(
        registry,
        "PreviewAutomationInvalidSelectorError",
    );
    register::<PreviewAutomationLoadError>(registry, "PreviewAutomationLoadError");
    register::<PreviewAutomationMalformedResponseError>(
        registry,
        "PreviewAutomationMalformedResponseError",
    );
    register::<PreviewAutomationNavigateInput>(registry, "PreviewAutomationNavigateInput");
    register::<PreviewAutomationNetworkEntry>(registry, "PreviewAutomationNetworkEntry");
    register::<PreviewAutomationNoAvailableHostError>(
        registry,
        "PreviewAutomationNoAvailableHostError",
    );
    register::<PreviewAutomationOpenInput>(registry, "PreviewAutomationOpenInput");
    register::<PreviewAutomationOperation>(registry, "PreviewAutomationOperation");
    register::<PreviewAutomationPressInput>(registry, "PreviewAutomationPressInput");
    register::<PreviewAutomationProfile>(registry, "PreviewAutomationProfile");
    register::<PreviewAutomationRecordingArtifact>(registry, "PreviewAutomationRecordingArtifact");
    register::<PreviewAutomationRecordingDeadlineExpiredError>(
        registry,
        "PreviewAutomationRecordingDeadlineExpiredError",
    );
    register::<PreviewAutomationRecordingDesktopUpdateRequiredError>(
        registry,
        "PreviewAutomationRecordingDesktopUpdateRequiredError",
    );
    register::<PreviewAutomationRecordingStatus>(registry, "PreviewAutomationRecordingStatus");
    register::<PreviewAutomationRecordingTooLargeError>(
        registry,
        "PreviewAutomationRecordingTooLargeError",
    );
    register::<PreviewAutomationRecordingTransferError>(
        registry,
        "PreviewAutomationRecordingTransferError",
    );
    register::<PreviewAutomationRemoteUnavailableError>(
        registry,
        "PreviewAutomationRemoteUnavailableError",
    );
    register::<PreviewAutomationRequest>(registry, "PreviewAutomationRequest");
    register::<PreviewAutomationRequestQueueClosedError>(
        registry,
        "PreviewAutomationRequestQueueClosedError",
    );
    register::<PreviewAutomationResizeInput>(registry, "PreviewAutomationResizeInput");
    register::<PreviewAutomationResizeResult>(registry, "PreviewAutomationResizeResult");
    register::<PreviewAutomationResponse>(registry, "PreviewAutomationResponse");
    register::<PreviewAutomationResultTooLargeError>(
        registry,
        "PreviewAutomationResultTooLargeError",
    );
    register::<PreviewAutomationScrollInput>(registry, "PreviewAutomationScrollInput");
    register::<PreviewAutomationSelectInput>(registry, "PreviewAutomationSelectInput");
    register::<PreviewAutomationSelectResult>(registry, "PreviewAutomationSelectResult");
    register::<PreviewAutomationSetColorSchemeInput>(
        registry,
        "PreviewAutomationSetColorSchemeInput",
    );
    register::<PreviewAutomationSetColorSchemeResult>(
        registry,
        "PreviewAutomationSetColorSchemeResult",
    );
    register::<PreviewAutomationSnapshot>(registry, "PreviewAutomationSnapshot");
    register::<PreviewAutomationSnapshotInput>(registry, "PreviewAutomationSnapshotInput");
    register::<PreviewAutomationStatus>(registry, "PreviewAutomationStatus");
    register::<PreviewAutomationStreamEvent>(registry, "PreviewAutomationStreamEvent");
    register::<PreviewAutomationTabNotFoundError>(registry, "PreviewAutomationTabNotFoundError");
    register::<PreviewAutomationTabOwner>(registry, "PreviewAutomationTabOwner");
    register::<PreviewAutomationTabTargetInput>(registry, "PreviewAutomationTabTargetInput");
    register::<PreviewAutomationTargetNotEditableError>(
        registry,
        "PreviewAutomationTargetNotEditableError",
    );
    register::<PreviewAutomationTimeoutError>(registry, "PreviewAutomationTimeoutError");
    register::<PreviewAutomationTypeInput>(registry, "PreviewAutomationTypeInput");
    register::<PreviewAutomationUnavailableError>(registry, "PreviewAutomationUnavailableError");
    register::<PreviewAutomationUnsupportedClientError>(
        registry,
        "PreviewAutomationUnsupportedClientError",
    );
    register::<PreviewAutomationUploadInput>(registry, "PreviewAutomationUploadInput");
    register::<PreviewAutomationWaitForInput>(registry, "PreviewAutomationWaitForInput");
    register::<PreviewUrlResolution>(registry, "PreviewUrlResolution");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("preview_automation", register_module)
}
