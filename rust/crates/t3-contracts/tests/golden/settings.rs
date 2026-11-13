use super::super::{Registry, register};
use t3_contracts::settings::*;

pub fn register_module(registry: &mut Registry) {
    register::<AntigravityAuthMethod>(registry, "AntigravityAuthMethod");
    register::<AntigravitySettings>(registry, "AntigravitySettings");
    register::<AppearanceContrast>(registry, "AppearanceContrast");
    register::<BackgroundActivityOverrides>(registry, "BackgroundActivityOverrides");
    register::<BackgroundActivityProfile>(registry, "BackgroundActivityProfile");
    register::<BackgroundActivityProfileSelection>(registry, "BackgroundActivityProfileSelection");
    register::<BackgroundActivitySettings>(registry, "BackgroundActivitySettings");
    register::<BranchNamingMode>(registry, "BranchNamingMode");
    register::<BrowserCustomSearchUrl>(registry, "BrowserCustomSearchUrl");
    register::<BrowserLinkTarget>(registry, "BrowserLinkTarget");
    register::<BrowserSearchEngine>(registry, "BrowserSearchEngine");
    register::<ChatWidth>(registry, "ChatWidth");
    register::<ClaudeSettings>(registry, "ClaudeSettings");
    register::<ClientSettingsPatch>(registry, "ClientSettingsPatch");
    register::<ClientSettingsSchema>(registry, "ClientSettingsSchema");
    register::<CloudSpeechCredentials>(registry, "CloudSpeechCredentials");
    register::<CodeFontSize>(registry, "CodeFontSize");
    register::<CodexSettings>(registry, "CodexSettings");
    register::<DatabaseReclaimError>(registry, "DatabaseReclaimError");
    register::<DatabaseStorageStatus>(registry, "DatabaseStorageStatus");
    register::<DefaultThemePreference>(registry, "DefaultThemePreference");
    register::<DiffColorScheme>(registry, "DiffColorScheme");
    register::<DiffLayout>(registry, "DiffLayout");
    register::<EffortDisplay>(registry, "EffortDisplay");
    register::<EnvironmentIdentificationMode>(registry, "EnvironmentIdentificationMode");
    register::<FontFamilyPreference>(registry, "FontFamilyPreference");
    register::<GlassOpacity>(registry, "GlassOpacity");
    register::<IdleCompactionMode>(registry, "IdleCompactionMode");
    register::<InterfaceFontSize>(registry, "InterfaceFontSize");
    register::<ListTextGeneratorModelsInput>(registry, "ListTextGeneratorModelsInput");
    register::<ListTextGeneratorModelsResult>(registry, "ListTextGeneratorModelsResult");
    register::<LoadBalancingWeights>(registry, "LoadBalancingWeights");
    register::<ModelFavorite>(registry, "ModelFavorite");
    register::<ModelFavorites>(registry, "ModelFavorites");
    register::<NotificationMode>(registry, "NotificationMode");
    register::<ObservabilitySettings>(registry, "ObservabilitySettings");
    register::<PanelAnimationDurationMs>(registry, "PanelAnimationDurationMs");
    register::<ProjectProviderInstanceOverride>(registry, "ProjectProviderInstanceOverride");
    register::<ProjectSettingsOverrides>(registry, "ProjectSettingsOverrides");
    register::<PromptFontSize>(registry, "PromptFontSize");
    register::<ProviderModelPreferences>(registry, "ProviderModelPreferences");
    register::<QuitConfirmationMode>(registry, "QuitConfirmationMode");
    register::<ResponseStreamingMode>(registry, "ResponseStreamingMode");
    register::<ScreenReaderAnnouncements>(registry, "ScreenReaderAnnouncements");
    register::<SecretProviderConfig>(registry, "SecretProviderConfig");
    register::<SecretProviderKind>(registry, "SecretProviderKind");
    register::<SecretProviderLabel>(registry, "SecretProviderLabel");
    register::<ServerSettings>(registry, "ServerSettings");
    register::<ServerSettingsError>(registry, "ServerSettingsError");
    register::<ServerSettingsOperation>(registry, "ServerSettingsOperation");
    register::<ServerSettingsPatch>(registry, "ServerSettingsPatch");
    register::<SidebarAutoSettleAfterDays>(registry, "SidebarAutoSettleAfterDays");
    register::<SidebarProjectGroupingMode>(registry, "SidebarProjectGroupingMode");
    register::<SidebarProjectSortOrder>(registry, "SidebarProjectSortOrder");
    register::<SidebarThreadSortOrder>(registry, "SidebarThreadSortOrder");
    register::<SnapShotKeyChord>(registry, "SnapShotKeyChord");
    register::<SnapShotModifier>(registry, "SnapShotModifier");
    register::<SnapShotShortcut>(registry, "SnapShotShortcut");
    register::<SnapShotSound>(registry, "SnapShotSound");
    register::<SnoozePresetDay>(registry, "SnoozePresetDay");
    register::<SnoozePresetSetting>(registry, "SnoozePresetSetting");
    register::<SnoozePresetSettings>(registry, "SnoozePresetSettings");
    register::<SourceControlHostSettings>(registry, "SourceControlHostSettings");
    register::<SourceControlWritingStyleMode>(registry, "SourceControlWritingStyleMode");
    register::<SourceControlWritingStyleSettings>(registry, "SourceControlWritingStyleSettings");
    register::<StorageCleanupReport>(registry, "StorageCleanupReport");
    register::<StorageCleanupReportEntry>(registry, "StorageCleanupReportEntry");
    register::<StorageCleanupSettings>(registry, "StorageCleanupSettings");
    register::<TerminalFontSize>(registry, "TerminalFontSize");
    register::<TextGeneratorConfig>(registry, "TextGeneratorConfig");
    register::<TextGeneratorError>(registry, "TextGeneratorError");
    register::<TextGeneratorKind>(registry, "TextGeneratorKind");
    register::<TextGeneratorModel>(registry, "TextGeneratorModel");
    register::<TimestampFormat>(registry, "TimestampFormat");
    register::<ToolCallVisibility>(registry, "ToolCallVisibility");
    register::<UsageLimitSourceConfig>(registry, "UsageLimitSourceConfig");
    register::<UsageModelPriceOverride>(registry, "UsageModelPriceOverride");
    register::<VoiceMicrophoneEntry>(registry, "VoiceMicrophoneEntry");
    register::<VoiceShortcutMode>(registry, "VoiceShortcutMode");
    register::<VoiceTranscriptionDestination>(registry, "VoiceTranscriptionDestination");
    register::<WorktreeCleanup>(registry, "WorktreeCleanup");
    register::<WorktreeCleanupRules>(registry, "WorktreeCleanupRules");
    register::<WorktreeKeepWhen>(registry, "WorktreeKeepWhen");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("settings", register_module)
}

#[test]
#[allow(non_snake_case)]
fn empty_object_matches_DEFAULT_CLIENT_SETTINGS() -> Result<(), serde_json::Error> {
    let input: serde_json::Value = serde_json::from_str(r#"{}"#)?;
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
  "notificationMode": "off",
  "inAppNotificationsEnabled": false,
  "diffColorScheme": "red-green",
  "chatWidth": "comfortable",
  "effortDisplay": "label",
  "loadBalancingEnabled": false,
  "loadBalancingWeights": {},
  "appearanceContrast": 100,
  "panelAnimationDurationMs": 0,
  "browserDefaultViewport": {
    "_tag": "fill"
  },
  "browserDefaultZoomFactor": 1,
  "browserDefaultAppearance": "system",
  "browserRecordingFrameRate": 30,
  "browserRecordingShowKeyPresses": false,
  "browserRecordingShowMousePresses": false,
  "browserLinkTarget": "system",
  "browserSearchEngine": "duckduckgo",
  "browserCustomSearchUrl": "",
  "browserAutoShowFloatingPreview": true,
  "browserProfiles": [],
  "browserDefaultProfileId": "default",
  "confirmQuit": "hold",
  "confirmThreadArchive": false,
  "confirmThreadDelete": true,
  "confirmThreadUnpin": false,
  "diffFilesCollapsed": true,
  "diffIgnoreWhitespace": true,
  "diffLayout": "stacked",
  "environmentIdentificationMode": "artwork",
  "glassOpacity": 80,
  "fontSizeInterface": 16,
  "fontSizePrompt": 14,
  "fontSizeCode": 13,
  "fontSizeTerminal": 12,
  "fontFamilyCode": "",
  "fontFamilyComposer": "",
  "fontFamilySans": "",
  "fontFamilyTerminal": "",
  "fontSmoothing": true,
  "onboardingCompletedAt": null,
  "persistComposerContextStrip": false,
  "favorites": [],
  "providerModelPreferences": {},
  "pullRequestMergeMethodOverrides": {},
  "pullRequestLinksOpenInApp": true,
  "composerRichTextEnabled": true,
  "sendShortcut": "enter",
  "followUpBehavior": "steer",
  "proactivePanelsEnabled": false,
  "showSkillsInSlashMenu": true,
  "boringSidebarEnabled": false,
  "boringSidebarMultilineRows": false,
  "toolCallVisibility": "showAll",
  "screenReaderAnnouncements": "status",
  "screenReaderWindowTitle": false,
  "threadDetailsUsageEnabled": false,
  "threadDetailsItems": {},
  "sidebarWorkingShelfEnabled": true,
  "sidebarProjectGroupingMode": "repository",
  "sidebarProjectGroupingOverrides": {},
  "sidebarProjectSortOrder": "updated_at",
  "sidebarThreadSortOrder": "updated_at",
  "timestampFormat": "locale",
  "voiceMicrophone": "",
  "voiceMicrophones": [],
  "voiceShortcutMode": "auto",
  "speechLanguage": "auto",
  "speechTranslateToEnglish": false,
  "speechCustomWords": [],
  "speechSilenceFiltering": true,
  "speechRemoveFillerWords": true,
  "speechPostProcessingPrompt": {
    "mode": "default",
    "customInstructions": ""
  },
  "speechCorrectionWord": "",
  "language": "en",
  "snapShotEnabled": false,
  "snapShotIncludeAccessibility": true,
  "snapShotShortcut": {
    "kind": "both-shift-keys"
  },
  "snapShotPlaySound": true,
  "snapShotSound": "soft-pop",
  "snapShotFlash": true,
  "snapShotAnimations": true,
  "wordWrap": true,
  "mathRenderingEnabled": false
}"#,
    )?;
    let settings: ClientSettingsSchema = serde_json::from_value(input)?;
    assert_eq!(serde_json::to_value(settings)?, expected);
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn partial_object_matches_DEFAULT_CLIENT_SETTINGS_and_DEFAULT_QUIT_CONFIRMATION_MODE()
-> Result<(), serde_json::Error> {
    let input: serde_json::Value = serde_json::from_str(
        r#"{
  "notificationMode": "sound",
  "confirmQuit": false,
  "fontSizePrompt": 20,
  "providerModelPreferences": {
    "codex": {
      "modelOrder": [
        "custom"
      ]
    }
  }
}"#,
    )?;
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
  "notificationMode": "sound",
  "inAppNotificationsEnabled": false,
  "diffColorScheme": "red-green",
  "chatWidth": "comfortable",
  "effortDisplay": "label",
  "loadBalancingEnabled": false,
  "loadBalancingWeights": {},
  "appearanceContrast": 100,
  "panelAnimationDurationMs": 0,
  "browserDefaultViewport": {
    "_tag": "fill"
  },
  "browserDefaultZoomFactor": 1,
  "browserDefaultAppearance": "system",
  "browserRecordingFrameRate": 30,
  "browserRecordingShowKeyPresses": false,
  "browserRecordingShowMousePresses": false,
  "browserLinkTarget": "system",
  "browserSearchEngine": "duckduckgo",
  "browserCustomSearchUrl": "",
  "browserAutoShowFloatingPreview": true,
  "browserProfiles": [],
  "browserDefaultProfileId": "default",
  "confirmQuit": "direct",
  "confirmThreadArchive": false,
  "confirmThreadDelete": true,
  "confirmThreadUnpin": false,
  "diffFilesCollapsed": true,
  "diffIgnoreWhitespace": true,
  "diffLayout": "stacked",
  "environmentIdentificationMode": "artwork",
  "glassOpacity": 80,
  "fontSizeInterface": 16,
  "fontSizePrompt": 20,
  "fontSizeCode": 13,
  "fontSizeTerminal": 12,
  "fontFamilyCode": "",
  "fontFamilyComposer": "",
  "fontFamilySans": "",
  "fontFamilyTerminal": "",
  "fontSmoothing": true,
  "onboardingCompletedAt": null,
  "persistComposerContextStrip": false,
  "favorites": [],
  "providerModelPreferences": {
    "codex": {
      "hiddenModels": [],
      "modelOrder": [
        "custom"
      ]
    }
  },
  "pullRequestMergeMethodOverrides": {},
  "pullRequestLinksOpenInApp": true,
  "composerRichTextEnabled": true,
  "sendShortcut": "enter",
  "followUpBehavior": "steer",
  "proactivePanelsEnabled": false,
  "showSkillsInSlashMenu": true,
  "boringSidebarEnabled": false,
  "boringSidebarMultilineRows": false,
  "toolCallVisibility": "showAll",
  "screenReaderAnnouncements": "status",
  "screenReaderWindowTitle": false,
  "threadDetailsUsageEnabled": false,
  "threadDetailsItems": {},
  "sidebarWorkingShelfEnabled": true,
  "sidebarProjectGroupingMode": "repository",
  "sidebarProjectGroupingOverrides": {},
  "sidebarProjectSortOrder": "updated_at",
  "sidebarThreadSortOrder": "updated_at",
  "timestampFormat": "locale",
  "voiceMicrophone": "",
  "voiceMicrophones": [],
  "voiceShortcutMode": "auto",
  "speechLanguage": "auto",
  "speechTranslateToEnglish": false,
  "speechCustomWords": [],
  "speechSilenceFiltering": true,
  "speechRemoveFillerWords": true,
  "speechPostProcessingPrompt": {
    "mode": "default",
    "customInstructions": ""
  },
  "speechCorrectionWord": "",
  "language": "en",
  "snapShotEnabled": false,
  "snapShotIncludeAccessibility": true,
  "snapShotShortcut": {
    "kind": "both-shift-keys"
  },
  "snapShotPlaySound": true,
  "snapShotSound": "soft-pop",
  "snapShotFlash": true,
  "snapShotAnimations": true,
  "wordWrap": true,
  "mathRenderingEnabled": false
}"#,
    )?;
    let settings: ClientSettingsSchema = serde_json::from_value(input)?;
    assert_eq!(serde_json::to_value(settings)?, expected);
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn empty_object_matches_DEFAULT_SERVER_SETTINGS_DEFAULT_SNOOZE_PRESETS_and_DEFAULT_TEXT_GENERATION_MODEL()
-> Result<(), serde_json::Error> {
    let input: serde_json::Value = serde_json::from_str(r#"{}"#)?;
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
  "favorites": null,
  "providerModelPreferences": null,
  "worktreeCleanup": null,
  "storageCleanup": {
    "worktreeKeepWhen": "uncommitted-changes",
    "worktreeAfterDays": null,
    "worktreeSettledAfterDays": null,
    "worktreeOnMerge": false,
    "worktreeOnDelete": false,
    "worktreeUnchanged": false,
    "browserArtifactsAfterDays": null,
    "logsAfterDays": null
  },
  "worktreesDirectory": "",
  "fastWorktreeCheckout": false,
  "previousWorktreesDirectories": [],
  "responseStreamingMode": "paragraph",
  "enableProviderUpdateChecks": true,
  "continueThreadsAfterServerUpdate": true,
  "keepAwakeWhileAgentsWork": true,
  "enableAgentBrowserAccess": true,
  "cloudSpeechCredentials": {
    "openai": "",
    "xai": "",
    "openrouter": ""
  },
  "speechModelId": "handy-computer/parakeet-unified-en-0.6b-gguf",
  "speechAcceleration": "auto",
  "speechModelUnloadTimeout": "min_15",
  "speechProjectCustomWords": [],
  "speechPostProcessingModelSelection": null,
  "projectAgentBrowserAccessOverrides": {},
  "defaultAutoPull": false,
  "defaultProjectScripts": [],
  "projectScriptOverrides": {},
  "projectAutoPullOverrides": {},
  "defaultModelSelection": null,
  "fallbackModelSelection": null,
  "defaultRuntimeMode": "full-access",
  "projectSettingsOverrides": {},
  "projectSettingsFolded": false,
  "enableAgentDeviceAccess": false,
  "enableDeviceSupport": false,
  "deviceOnboardingCompleted": false,
  "deviceHosts": [],
  "easDeviceHosts": [],
  "simlockDeviceHosts": [],
  "simlockDeviceHostsEnabled": false,
  "sidebarAutoSettleAfterDays": 7,
  "compactIdleThreads": false,
  "snoozeLimitedThreads": false,
  "autoResumeLimitedThreads": false,
  "snoozePresets": [
    {
      "kind": "after",
      "minutes": 60
    },
    {
      "kind": "after",
      "minutes": 180
    },
    {
      "kind": "at",
      "day": "today",
      "time": "18:00"
    },
    {
      "kind": "at",
      "day": "tomorrow",
      "time": "09:00"
    },
    {
      "kind": "at",
      "day": "monday",
      "time": "09:00"
    }
  ],
  "hiddenThreadTags": [],
  "hideTaggedThreads": false,
  "guestSharingEnabled": false,
  "linkedEnvironmentsEnabled": false,
  "promptSuggestionsEnabled": false,
  "sidebarAutoSettleKeepPullRequests": false,
  "sidebarAutoSettleOnMerge": true,
  "backgroundActivity": {
    "schemaVersion": 1,
    "profile": "balanced",
    "overrides": {}
  },
  "automaticGitFetchInterval": 30000,
  "providerHealthRefreshInterval": 300000,
  "backgroundActivityProfile": "balanced",
  "defaultTheme": "",
  "defaultThemeSetAt": "",
  "environmentIcon": null,
  "newWorktreesStartFromOrigin": true,
  "worktreeSubmodules": null,
  "addProjectBaseDirectory": "",
  "ghqCloneLayout": false,
  "textGenerationModelSelection": {
    "instanceId": "codex",
    "model": "gpt-6-luna",
    "options": [
      {
        "id": "reasoningEffort",
        "value": "low"
      }
    ]
  },
  "threadTitleInstructions": "",
  "branchNamingMode": "static",
  "branchNamePrefix": "t3",
  "branchNameInstructions": "",
  "removeAgentCreditsOnMerge": false,
  "sourceControlWritingStyle": {
    "mode": "repo_conventions",
    "customInstructions": "",
    "followChangeRequestTemplates": true
  },
  "sourceControlWriterModelSelection": null,
  "reviewGuidesEnabled": false,
  "reviewGuideModelSelection": null,
  "reviewGuideInstructions": "",
  "pullRequestMergeMethod": null,
  "providerInstances": {},
  "observability": {
    "otlpTracesUrl": "",
    "otlpMetricsUrl": "",
    "otlpLogsUrl": ""
  },
  "sourceControlHosts": {},
  "offeredSourceControlProviders": {},
  "usageLimitSources": {},
  "textGenerators": {},
  "secretProviders": {},
  "usagePriceOverrides": {},
  "usageModelAliases": {}
}"#,
    )?;
    let settings: ServerSettings = serde_json::from_value(input)?;
    assert_eq!(serde_json::to_value(settings)?, expected);
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn partial_object_matches_DEFAULT_SERVER_SETTINGS_and_DEFAULT_BACKGROUND_ACTIVITY_PROFILE()
-> Result<(), serde_json::Error> {
    let input: serde_json::Value = serde_json::from_str(
        r#"{
  "favorites": [],
  "providerModelPreferences": {},
  "storageCleanup": {
    "logsAfterDays": 14
  },
  "backgroundActivity": {
    "profile": "custom",
    "overrides": {
      "pauseWhenHostLocked": true
    }
  },
  "observability": {
    "otlpLogsUrl": " https://logs.example "
  },
  "cloudSpeechCredentials": {
    "xai": "key"
  },
  "sourceControlWritingStyle": "﻿conventional commits ",
  "hiddenThreadTags": [
    "work",
    null,
    42,
    "next"
  ],
  "defaultThreadEnvMode": null,
  "worktreeSubmodules": "future",
  "environmentIcon": "future",
  "projectSettingsOverrides": {
    "project": {
      "sourceControlWritingStyle": " follow repo ",
      "futureSetting": {
        "x": 1
      }
    }
  }
}"#,
    )?;
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
  "favorites": [],
  "providerModelPreferences": {},
  "worktreeCleanup": null,
  "storageCleanup": {
    "worktreeKeepWhen": "uncommitted-changes",
    "worktreeAfterDays": null,
    "worktreeSettledAfterDays": null,
    "worktreeOnMerge": false,
    "worktreeOnDelete": false,
    "worktreeUnchanged": false,
    "browserArtifactsAfterDays": null,
    "logsAfterDays": 14
  },
  "worktreesDirectory": "",
  "fastWorktreeCheckout": false,
  "previousWorktreesDirectories": [],
  "responseStreamingMode": "paragraph",
  "enableProviderUpdateChecks": true,
  "continueThreadsAfterServerUpdate": true,
  "keepAwakeWhileAgentsWork": true,
  "enableAgentBrowserAccess": true,
  "cloudSpeechCredentials": {
    "openai": "",
    "xai": "key",
    "openrouter": ""
  },
  "speechModelId": "handy-computer/parakeet-unified-en-0.6b-gguf",
  "speechAcceleration": "auto",
  "speechModelUnloadTimeout": "min_15",
  "speechProjectCustomWords": [],
  "speechPostProcessingModelSelection": null,
  "projectAgentBrowserAccessOverrides": {},
  "defaultAutoPull": false,
  "defaultProjectScripts": [],
  "projectScriptOverrides": {},
  "projectAutoPullOverrides": {},
  "defaultModelSelection": null,
  "fallbackModelSelection": null,
  "defaultRuntimeMode": "full-access",
  "projectSettingsOverrides": {
    "project": {
      "sourceControlWritingStyle": {
        "mode": "custom",
        "customInstructions": "follow repo",
        "followChangeRequestTemplates": true
      },
      "futureSetting": {
        "x": 1
      }
    }
  },
  "projectSettingsFolded": false,
  "enableAgentDeviceAccess": false,
  "enableDeviceSupport": false,
  "deviceOnboardingCompleted": false,
  "deviceHosts": [],
  "easDeviceHosts": [],
  "simlockDeviceHosts": [],
  "simlockDeviceHostsEnabled": false,
  "sidebarAutoSettleAfterDays": 7,
  "compactIdleThreads": false,
  "snoozeLimitedThreads": false,
  "autoResumeLimitedThreads": false,
  "snoozePresets": [
    {
      "kind": "after",
      "minutes": 60
    },
    {
      "kind": "after",
      "minutes": 180
    },
    {
      "kind": "at",
      "day": "today",
      "time": "18:00"
    },
    {
      "kind": "at",
      "day": "tomorrow",
      "time": "09:00"
    },
    {
      "kind": "at",
      "day": "monday",
      "time": "09:00"
    }
  ],
  "hiddenThreadTags": [
    "work",
    "next"
  ],
  "hideTaggedThreads": false,
  "guestSharingEnabled": false,
  "linkedEnvironmentsEnabled": false,
  "promptSuggestionsEnabled": false,
  "sidebarAutoSettleKeepPullRequests": false,
  "sidebarAutoSettleOnMerge": true,
  "backgroundActivity": {
    "schemaVersion": 1,
    "profile": "custom",
    "overrides": {
      "pauseWhenHostLocked": true
    }
  },
  "automaticGitFetchInterval": 30000,
  "providerHealthRefreshInterval": 300000,
  "backgroundActivityProfile": "balanced",
  "defaultTheme": "",
  "defaultThemeSetAt": "",
  "environmentIcon": null,
  "newWorktreesStartFromOrigin": true,
  "worktreeSubmodules": null,
  "addProjectBaseDirectory": "",
  "ghqCloneLayout": false,
  "textGenerationModelSelection": {
    "instanceId": "codex",
    "model": "gpt-6-luna",
    "options": [
      {
        "id": "reasoningEffort",
        "value": "low"
      }
    ]
  },
  "threadTitleInstructions": "",
  "branchNamingMode": "static",
  "branchNamePrefix": "t3",
  "branchNameInstructions": "",
  "removeAgentCreditsOnMerge": false,
  "sourceControlWritingStyle": {
    "mode": "custom",
    "customInstructions": "conventional commits",
    "followChangeRequestTemplates": true
  },
  "sourceControlWriterModelSelection": null,
  "reviewGuidesEnabled": false,
  "reviewGuideModelSelection": null,
  "reviewGuideInstructions": "",
  "pullRequestMergeMethod": null,
  "providerInstances": {},
  "observability": {
    "otlpTracesUrl": "",
    "otlpMetricsUrl": "",
    "otlpLogsUrl": "https://logs.example"
  },
  "sourceControlHosts": {},
  "offeredSourceControlProviders": {},
  "usageLimitSources": {},
  "textGenerators": {},
  "secretProviders": {},
  "usagePriceOverrides": {},
  "usageModelAliases": {}
}"#,
    )?;
    let settings: ServerSettings = serde_json::from_value(input)?;
    assert_eq!(serde_json::to_value(settings)?, expected);
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn empty_codex_settings_applies_makeBinaryPathSetting_codex_default()
-> Result<(), serde_json::Error> {
    let input: serde_json::Value = serde_json::from_str(r#"{}"#)?;
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
  "enabled": true,
  "binaryPath": "codex",
  "homePath": "",
  "shadowHomePath": "",
  "launchArgs": "",
  "customModels": []
}"#,
    )?;
    let settings: CodexSettings = serde_json::from_value(input)?;
    assert_eq!(serde_json::to_value(settings)?, expected);
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn empty_claude_settings_applies_makeBinaryPathSetting_claude_default()
-> Result<(), serde_json::Error> {
    let input: serde_json::Value = serde_json::from_str(r#"{}"#)?;
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
  "enabled": true,
  "binaryPath": "claude",
  "homePath": "",
  "customModels": [],
  "launchArgs": "",
  "autoCompactWindow": "",
  "idleCompaction": "default"
}"#,
    )?;
    let settings: ClaudeSettings = serde_json::from_value(input)?;
    assert_eq!(serde_json::to_value(settings)?, expected);
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn empty_antigravity_settings_applies_oauth_personal_default() -> Result<(), serde_json::Error> {
    let input: serde_json::Value = serde_json::from_str(r#"{}"#)?;
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
  "enabled": false,
  "authMethod": "oauth-personal",
  "apiKey": "",
  "gcpProject": "",
  "gcpLocation": "",
  "binaryPath": "",
  "customModels": []
}"#,
    )?;
    let settings: AntigravitySettings = serde_json::from_value(input)?;
    assert_eq!(serde_json::to_value(settings)?, expected);
    Ok(())
}

#[test]
fn nullable_patches_preserve_absence_and_explicit_null() -> Result<(), serde_json::Error> {
    use serde_json::json;
    for value in [
        json!({}),
        json!({"onboardingCompletedAt": null}),
        json!({"onboardingCompletedAt": "done"}),
    ] {
        let patch: ClientSettingsPatch = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(patch)?, value);
    }
    for value in [
        json!({}),
        json!({"worktreeCleanup": null}),
        json!({"sidebarAutoSettleAfterDays": null}),
        json!({"defaultModelSelection": null}),
    ] {
        let patch: ServerSettingsPatch = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(patch)?, value);
    }
    assert!(
        serde_json::from_value::<ClientSettingsPatch>(json!({"notificationMode": null})).is_err()
    );
    assert!(
        serde_json::from_value::<ServerSettingsPatch>(json!({"defaultAutoPull": null})).is_err()
    );
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn LegacyConfirmQuit_normalizes_and_QuitConfirmationMode_stays_strict()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    for (input, expected) in [
        (json!(true), json!("hold")),
        (json!(false), json!("direct")),
        (json!("double-click"), json!("double-click")),
    ] {
        let settings: ClientSettingsSchema = serde_json::from_value(json!({"confirmQuit": input}))?;
        assert_eq!(serde_json::to_value(settings)?["confirmQuit"], expected);
    }
    assert!(serde_json::from_value::<QuitConfirmationMode>(json!(true)).is_err());
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn LegacySourceControlWritingStyle_normalizes_in_ProjectSettingsOverrides()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    for (input, mode, instructions) in [
        ("\u{feff}  \u{00a0}", "repo_conventions", ""),
        ("  follow repository  ", "custom", "follow repository"),
    ] {
        let settings: ProjectSettingsOverrides =
            serde_json::from_value(json!({"sourceControlWritingStyle": input}))?;
        assert_eq!(
            serde_json::to_value(settings)?,
            json!({"sourceControlWritingStyle": {"mode": mode, "customInstructions": instructions, "followChangeRequestTemplates": true}})
        );
    }
    assert!(
        serde_json::from_value::<SourceControlWritingStyleSettings>(json!("follow repository"))
            .is_err()
    );
    Ok(())
}

#[test]
fn encoded_empty_binary_paths_remain_empty_and_missing_paths_use_defaults()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    let explicit: CodexSettings = serde_json::from_value(json!({"binaryPath": ""}))?;
    let missing: CodexSettings = serde_json::from_value(json!({}))?;
    assert_eq!(serde_json::to_value(explicit)?["binaryPath"], "");
    assert_eq!(serde_json::to_value(missing)?["binaryPath"], "codex");
    Ok(())
}
