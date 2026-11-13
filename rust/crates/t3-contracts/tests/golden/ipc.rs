use super::super::{Registry, register};
use t3_contracts::ipc::*;

pub fn register_module(registry: &mut Registry) {
    register::<ContextMenuItemSchema>(registry, "ContextMenuItemSchema");
    register::<DesktopAppBrandingSchema>(registry, "DesktopAppBrandingSchema");
    register::<DesktopAppStageLabelSchema>(registry, "DesktopAppStageLabelSchema");
    register::<DesktopCaptureConfigApplied>(registry, "DesktopCaptureConfigApplied");
    register::<DesktopCaptureConfigPreview>(registry, "DesktopCaptureConfigPreview");
    register::<DesktopCaptureConfigRequest>(registry, "DesktopCaptureConfigRequest");
    register::<DesktopCaptureExtensionState>(registry, "DesktopCaptureExtensionState");
    register::<DesktopCaptureHelperState>(registry, "DesktopCaptureHelperState");
    register::<DesktopCliCommandStateSchema>(registry, "DesktopCliCommandStateSchema");
    register::<DesktopDiscoveredSshHostSchema>(registry, "DesktopDiscoveredSshHostSchema");
    register::<DesktopEnvironmentBootstrapSchema>(registry, "DesktopEnvironmentBootstrapSchema");
    register::<DesktopMediaReadSchema>(registry, "DesktopMediaReadSchema");
    register::<DesktopPendingSnapShot>(registry, "DesktopPendingSnapShot");
    register::<DesktopPreviewAnnotationSendEnabledInputSchema>(
        registry,
        "DesktopPreviewAnnotationSendEnabledInputSchema",
    );
    register::<DesktopPreviewAnnotationThemeInputSchema>(
        registry,
        "DesktopPreviewAnnotationThemeInputSchema",
    );
    register::<DesktopPreviewAnnotationThemeSchema>(
        registry,
        "DesktopPreviewAnnotationThemeSchema",
    );
    register::<DesktopPreviewAnnotationVoiceConfigSchema>(
        registry,
        "DesktopPreviewAnnotationVoiceConfigSchema",
    );
    register::<DesktopPreviewAnnotationVoiceEventSchema>(
        registry,
        "DesktopPreviewAnnotationVoiceEventSchema",
    );
    register::<DesktopPreviewAnnotationVoiceStateSchema>(
        registry,
        "DesktopPreviewAnnotationVoiceStateSchema",
    );
    register::<DesktopPreviewArtifactInputSchema>(registry, "DesktopPreviewArtifactInputSchema");
    register::<DesktopPreviewClearDataInputSchema>(registry, "DesktopPreviewClearDataInputSchema");
    register::<DesktopPreviewColorSchemeSchema>(registry, "DesktopPreviewColorSchemeSchema");
    register::<DesktopPreviewConfigInputSchema>(registry, "DesktopPreviewConfigInputSchema");
    register::<DesktopPreviewCreateTabInputSchema>(registry, "DesktopPreviewCreateTabInputSchema");
    register::<DesktopPreviewFindInPageInputSchema>(
        registry,
        "DesktopPreviewFindInPageInputSchema",
    );
    register::<DesktopPreviewNavigateInputSchema>(registry, "DesktopPreviewNavigateInputSchema");
    register::<DesktopPreviewPickElementInputSchema>(
        registry,
        "DesktopPreviewPickElementInputSchema",
    );
    register::<DesktopPreviewRecordingArtifactSchema>(
        registry,
        "DesktopPreviewRecordingArtifactSchema",
    );
    register::<DesktopPreviewRecordingInputSchema>(registry, "DesktopPreviewRecordingInputSchema");
    register::<DesktopPreviewRecordingSaveInputSchema>(
        registry,
        "DesktopPreviewRecordingSaveInputSchema",
    );
    register::<DesktopPreviewRegisterWebviewInputSchema>(
        registry,
        "DesktopPreviewRegisterWebviewInputSchema",
    );
    register::<DesktopPreviewScreenshotArtifactSchema>(
        registry,
        "DesktopPreviewScreenshotArtifactSchema",
    );
    register::<DesktopPreviewSetAudioMutedInputSchema>(
        registry,
        "DesktopPreviewSetAudioMutedInputSchema",
    );
    register::<DesktopPreviewSetColorSchemeInputSchema>(
        registry,
        "DesktopPreviewSetColorSchemeInputSchema",
    );
    register::<DesktopPreviewSetShownInputSchema>(registry, "DesktopPreviewSetShownInputSchema");
    register::<DesktopPreviewSetZoomFactorInputSchema>(
        registry,
        "DesktopPreviewSetZoomFactorInputSchema",
    );
    register::<DesktopPreviewTabIdSchema>(registry, "DesktopPreviewTabIdSchema");
    register::<DesktopPreviewTabInputSchema>(registry, "DesktopPreviewTabInputSchema");
    register::<DesktopPreviewWebviewConfigSchema>(registry, "DesktopPreviewWebviewConfigSchema");
    register::<DesktopRuntimeArchSchema>(registry, "DesktopRuntimeArchSchema");
    register::<DesktopServerExposureModeSchema>(registry, "DesktopServerExposureModeSchema");
    register::<DesktopServerExposureStateSchema>(registry, "DesktopServerExposureStateSchema");
    register::<DesktopSnapShot>(registry, "DesktopSnapShot");
    register::<DesktopSnapShotAnimationDestination>(
        registry,
        "DesktopSnapShotAnimationDestination",
    );
    register::<DesktopSnapShotEvent>(registry, "DesktopSnapShotEvent");
    register::<DesktopSnapShotId>(registry, "DesktopSnapShotId");
    register::<DesktopSnapShotMode>(registry, "DesktopSnapShotMode");
    register::<DesktopSnapShotSetupAction>(registry, "DesktopSnapShotSetupAction");
    register::<DesktopSnapShotShortcutAvailability>(
        registry,
        "DesktopSnapShotShortcutAvailability",
    );
    register::<DesktopSnapShotState>(registry, "DesktopSnapShotState");
    register::<DesktopSshBearerBootstrapInputSchema>(
        registry,
        "DesktopSshBearerBootstrapInputSchema",
    );
    register::<DesktopSshBearerRequestInputSchema>(registry, "DesktopSshBearerRequestInputSchema");
    register::<DesktopSshEnvironmentBootstrapSchema>(
        registry,
        "DesktopSshEnvironmentBootstrapSchema",
    );
    register::<DesktopSshEnvironmentEnsureInputSchema>(
        registry,
        "DesktopSshEnvironmentEnsureInputSchema",
    );
    register::<DesktopSshEnvironmentEnsureOptionsSchema>(
        registry,
        "DesktopSshEnvironmentEnsureOptionsSchema",
    );
    register::<DesktopSshEnvironmentEnsureResultSchema>(
        registry,
        "DesktopSshEnvironmentEnsureResultSchema",
    );
    register::<DesktopSshEnvironmentTargetSchema>(registry, "DesktopSshEnvironmentTargetSchema");
    register::<DesktopSshHostSourceSchema>(registry, "DesktopSshHostSourceSchema");
    register::<DesktopSshHttpBaseUrlInputSchema>(registry, "DesktopSshHttpBaseUrlInputSchema");
    register::<DesktopSshPasswordPromptCancelledResultSchema>(
        registry,
        "DesktopSshPasswordPromptCancelledResultSchema",
    );
    register::<DesktopSshPasswordPromptResolutionInputSchema>(
        registry,
        "DesktopSshPasswordPromptResolutionInputSchema",
    );
    register::<DesktopThemeSchema>(registry, "DesktopThemeSchema");
    register::<DesktopUpdateActionResultSchema>(registry, "DesktopUpdateActionResultSchema");
    register::<DesktopUpdateChannelSchema>(registry, "DesktopUpdateChannelSchema");
    register::<DesktopUpdateCheckResultSchema>(registry, "DesktopUpdateCheckResultSchema");
    register::<DesktopUpdateReleaseNoteSchema>(registry, "DesktopUpdateReleaseNoteSchema");
    register::<DesktopUpdateStateSchema>(registry, "DesktopUpdateStateSchema");
    register::<DesktopUpdateStatusSchema>(registry, "DesktopUpdateStatusSchema");
    register::<DesktopWslDistroSchema>(registry, "DesktopWslDistroSchema");
    register::<DesktopWslStateSchema>(registry, "DesktopWslStateSchema");
    register::<PersistedSavedEnvironmentRecordSchema>(
        registry,
        "PersistedSavedEnvironmentRecordSchema",
    );
    register::<PickFolderOptionsSchema>(registry, "PickFolderOptionsSchema");
    register::<PickedElementPayloadSchema>(registry, "PickedElementPayloadSchema");
    register::<PickedElementStackFrameSchema>(registry, "PickedElementStackFrameSchema");
    register::<PickedThemeFileSchema>(registry, "PickedThemeFileSchema");
    register::<PreviewAnnotationElementTargetSchema>(
        registry,
        "PreviewAnnotationElementTargetSchema",
    );
    register::<PreviewAnnotationPayloadSchema>(registry, "PreviewAnnotationPayloadSchema");
    register::<PreviewAnnotationPointSchema>(registry, "PreviewAnnotationPointSchema");
    register::<PreviewAnnotationRectSchema>(registry, "PreviewAnnotationRectSchema");
    register::<PreviewAnnotationRegionTargetSchema>(
        registry,
        "PreviewAnnotationRegionTargetSchema",
    );
    register::<PreviewAnnotationScreenshotSchema>(registry, "PreviewAnnotationScreenshotSchema");
    register::<PreviewAnnotationStrokeTargetSchema>(
        registry,
        "PreviewAnnotationStrokeTargetSchema",
    );
    register::<PreviewAnnotationStyleChangeSchema>(registry, "PreviewAnnotationStyleChangeSchema");
    register::<PreviewAnnotationSubmissionResultSchema>(
        registry,
        "PreviewAnnotationSubmissionResultSchema",
    );
    register::<PreviewAnnotationSubmissionSchema>(registry, "PreviewAnnotationSubmissionSchema");
    register::<SystemSettingsPaneSchema>(registry, "SystemSettingsPaneSchema");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("ipc", register_module)
}
