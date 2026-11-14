use crate::{Error, Result};
use serde_json::Value;
use t3_contracts::rpc::{self, RpcMethod};

pub(super) fn decode(method: &str, input: &Value) -> Result<Option<Value>> {
    if !input.is_object() {
        return Err(Error::InvalidPayload(format!(
            "{method} payload must be an object"
        )));
    }
    match method {
        "server.upsertKeybinding" => validate::<rpc::WsServerUpsertKeybindingRpc>(input, false),
        "server.removeKeybinding" => validate::<rpc::WsServerRemoveKeybindingRpc>(input, false),
        "server.probe" => validate::<rpc::WsServerProbeRpc>(input, false),
        "server.getConfig" => validate::<rpc::WsServerGetConfigRpc>(input, false),
        "server.refreshProviders" => validate::<rpc::WsServerRefreshProvidersRpc>(input, false),
        "server.updateProvider" => validate::<rpc::WsServerUpdateProviderRpc>(input, false),
        "provider.consumeResetCredit" => {
            validate::<rpc::WsProviderConsumeResetCreditRpc>(input, false)
        }
        "provider.auth.start" => validate::<rpc::WsProviderAuthStartRpc>(input, false),
        "provider.auth.respond" => validate::<rpc::WsProviderAuthRespondRpc>(input, false),
        "provider.auth.complete" => validate::<rpc::WsProviderAuthCompleteRpc>(input, false),
        "provider.chatgpt.reconnect-profile" => {
            validate::<rpc::WsChatGptReconnectProfileRpc>(input, false)
        }
        "provider.chatgpt.import-profile" => {
            validate::<rpc::WsChatGptImportProfileRpc>(input, false)
        }
        "provider.chatgpt.handoff.subscribe" => {
            validate::<rpc::WsChatGptHandoffSubscribeRpc>(input, false)
        }
        "provider.auth.cancel" => validate::<rpc::WsProviderAuthCancelRpc>(input, false),
        "provider.auth.removeMethod" => {
            validate::<rpc::WsProviderAuthRemoveMethodRpc>(input, false)
        }
        "provider.auth.logout" => validate::<rpc::WsProviderAuthLogoutRpc>(input, false),
        "provider.auth.subscribe" => validate::<rpc::WsProviderAuthSubscribeRpc>(input, false),
        "provider.install.start" => validate::<rpc::WsProviderInstallStartRpc>(input, false),
        "provider.install.cancel" => validate::<rpc::WsProviderInstallCancelRpc>(input, false),
        "provider.install.subscribe" => {
            validate::<rpc::WsProviderInstallSubscribeRpc>(input, false)
        }
        "provider.install.remove" => validate::<rpc::WsProviderInstallRemoveRpc>(input, false),
        "server.getNpmRelease" => validate::<rpc::WsServerGetNpmReleaseRpc>(input, false),
        "server.updateServer" => validate::<rpc::WsServerUpdateServerRpc>(input, false),
        "server.updateServerWithProgress" => {
            validate::<rpc::WsServerUpdateServerWithProgressRpc>(input, false)
        }
        "server.commitDesktopUpdate" => {
            validate::<rpc::WsServerCommitDesktopUpdateRpc>(input, false)
        }
        "server.runStorageCleanup" => validate::<rpc::WsServerRunStorageCleanupRpc>(input, false),
        "server.getStorageCleanupReport" => {
            validate::<rpc::WsServerGetStorageCleanupReportRpc>(input, false)
        }
        "server.getDatabaseStorage" => validate::<rpc::WsServerGetDatabaseStorageRpc>(input, false),
        "server.reclaimDatabaseSpace" => {
            validate::<rpc::WsServerReclaimDatabaseSpaceRpc>(input, false)
        }
        "peerLinks.list" => validate::<rpc::WsPeerLinksListRpc>(input, false),
        "peerLinks.link" => validate::<rpc::WsPeerLinksLinkRpc>(input, false),
        "peerLinks.unlink" => validate::<rpc::WsPeerLinksUnlinkRpc>(input, false),
        "peerLinks.answerRequest" => validate::<rpc::WsPeerLinksAnswerRequestRpc>(input, false),
        "server.updateSettings" => validate::<rpc::WsServerUpdateSettingsRpc>(input, true),
        "server.discoverSourceControl" => {
            validate::<rpc::WsServerDiscoverSourceControlRpc>(input, false)
        }
        "server.checkSecretSource" => validate::<rpc::WsServerCheckSecretSourceRpc>(input, false),
        "server.searchAcpRegistry" => validate::<rpc::WsServerSearchAcpRegistryRpc>(input, false),
        "server.prepareAcpRegistryAgent" => {
            validate::<rpc::WsServerPrepareAcpRegistryAgentRpc>(input, false)
        }
        "server.uninstallAcpRegistryManagedBinary" => {
            validate::<rpc::WsServerUninstallAcpRegistryManagedBinaryRpc>(input, false)
        }
        "server.acceptAcpRegistryUrlAuth" => {
            validate::<rpc::WsServerAcceptAcpRegistryUrlAuthRpc>(input, false)
        }
        "server.listAcpRegistrySessions" => {
            validate::<rpc::WsServerListAcpRegistrySessionsRpc>(input, false)
        }
        "server.importAcpRegistrySession" => {
            validate::<rpc::WsServerImportAcpRegistrySessionRpc>(input, false)
        }
        "server.deleteAcpRegistrySession" => {
            validate::<rpc::WsServerDeleteAcpRegistrySessionRpc>(input, false)
        }
        "server.listAcpRegistryProviders" => {
            validate::<rpc::WsServerListAcpRegistryProvidersRpc>(input, false)
        }
        "server.setAcpRegistryProvider" => {
            validate::<rpc::WsServerSetAcpRegistryProviderRpc>(input, false)
        }
        "server.disableAcpRegistryProvider" => {
            validate::<rpc::WsServerDisableAcpRegistryProviderRpc>(input, false)
        }
        "server.logoutAcpRegistry" => validate::<rpc::WsServerLogoutAcpRegistryRpc>(input, false),
        "server.getTraceDiagnostics" => {
            validate::<rpc::WsServerGetTraceDiagnosticsRpc>(input, false)
        }
        "server.getProcessDiagnostics" => {
            validate::<rpc::WsServerGetProcessDiagnosticsRpc>(input, false)
        }
        "server.getHostResources" => validate::<rpc::WsServerGetHostResourcesRpc>(input, false),
        "server.getProcessResourceHistory" => {
            validate::<rpc::WsServerGetProcessResourceHistoryRpc>(input, false)
        }
        "server.getResourceTelemetryHistory" => {
            validate::<rpc::WsServerGetResourceTelemetryHistoryRpc>(input, false)
        }
        "server.retryResourceTelemetry" => {
            validate::<rpc::WsServerRetryResourceTelemetryRpc>(input, false)
        }
        "server.checkOtlpEndpoint" => validate::<rpc::WsServerCheckOtlpEndpointRpc>(input, false),
        "server.listTextGeneratorModels" => {
            validate::<rpc::WsServerListTextGeneratorModelsRpc>(input, false)
        }
        "server.prepareTriage" => validate::<rpc::WsServerPrepareTriageRpc>(input, false),
        "server.getUsageSummary" => validate::<rpc::WsServerGetUsageSummaryRpc>(input, false),
        "server.refreshUsageRates" => validate::<rpc::WsServerRefreshUsageRatesRpc>(input, false),
        "server.signalProcess" => validate::<rpc::WsServerSignalProcessRpc>(input, false),
        "cloud.getRelayClientStatus" => {
            validate::<rpc::WsCloudGetRelayClientStatusRpc>(input, false)
        }
        "cloud.installRelayClient" => validate::<rpc::WsCloudInstallRelayClientRpc>(input, false),
        "server.reportClientActivity" => {
            validate::<rpc::WsServerReportClientActivityRpc>(input, false)
        }
        "pullRequests.list" => validate::<rpc::WsPullRequestsListRpc>(input, false),
        "pullRequests.listStats" => validate::<rpc::WsPullRequestsListStatsRpc>(input, false),
        "pullRequests.filterOptions" => {
            validate::<rpc::WsPullRequestsFilterOptionsRpc>(input, false)
        }
        "pullRequests.routing" => validate::<rpc::WsPullRequestsRoutingRpc>(input, false),
        "pullRequests.routingIdentity" => {
            validate::<rpc::WsPullRequestsRoutingIdentityRpc>(input, false)
        }
        "pullRequests.summary" => validate::<rpc::WsPullRequestsSummaryRpc>(input, false),
        "pullRequests.stack" => validate::<rpc::WsPullRequestsStackRpc>(input, false),
        "pullRequests.linkedThreads" => {
            validate::<rpc::WsPullRequestsLinkedThreadsRpc>(input, false)
        }
        "pullRequests.detail" => validate::<rpc::WsPullRequestsDetailRpc>(input, false),
        "pullRequests.preview" => validate::<rpc::WsPullRequestsPreviewRpc>(input, false),
        "pullRequests.checks" => validate::<rpc::WsPullRequestsChecksRpc>(input, false),
        "pullRequests.activity" => validate::<rpc::WsPullRequestsActivityRpc>(input, false),
        "pullRequests.reviewGuide" => validate::<rpc::WsPullRequestsReviewGuideRpc>(input, false),
        "pullRequests.generateReviewGuide" => {
            validate::<rpc::WsPullRequestsGenerateReviewGuideRpc>(input, false)
        }
        "pullRequests.threadComments" => {
            validate::<rpc::WsPullRequestsThreadCommentsRpc>(input, false)
        }
        "pullRequests.diffFileContents" => {
            validate::<rpc::WsPullRequestsDiffFileContentsRpc>(input, false)
        }
        "pullRequests.filesViewed" => validate::<rpc::WsPullRequestsFilesViewedRpc>(input, false),
        "pullRequests.changeBreakdown" => {
            validate::<rpc::WsPullRequestsChangeBreakdownRpc>(input, false)
        }
        "pullRequests.setFilesViewed" => {
            validate::<rpc::WsPullRequestsSetFilesViewedRpc>(input, false)
        }
        "pullRequests.runAction" => validate::<rpc::WsPullRequestsRunActionRpc>(input, false),
        "pullRequests.update" => validate::<rpc::WsPullRequestsUpdateRpc>(input, false),
        "pullRequests.comment" => validate::<rpc::WsPullRequestsCommentRpc>(input, false),
        "pullRequests.updateComment" => {
            validate::<rpc::WsPullRequestsUpdateCommentRpc>(input, false)
        }
        "pullRequests.submitReview" => validate::<rpc::WsPullRequestsSubmitReviewRpc>(input, false),
        "pullRequests.replyToThread" => {
            validate::<rpc::WsPullRequestsReplyToThreadRpc>(input, false)
        }
        "pullRequests.setThreadResolution" => {
            validate::<rpc::WsPullRequestsSetThreadResolutionRpc>(input, false)
        }
        "pullRequests.setReaction" => validate::<rpc::WsPullRequestsSetReactionRpc>(input, false),
        "pullRequests.invalidate" => validate::<rpc::WsPullRequestsInvalidateRpc>(input, false),
        "pullRequests.reportState" => validate::<rpc::WsPullRequestsReportStateRpc>(input, false),
        "pullRequests.subscribeRefreshes" => {
            validate::<rpc::WsPullRequestsSubscribeRefreshesRpc>(input, false)
        }
        "pullRequests.subscribeLinkedThreadRefreshes" => {
            validate::<rpc::WsPullRequestsSubscribeLinkedThreadRefreshesRpc>(input, false)
        }
        "pullRequests.reviewerCandidates" => {
            validate::<rpc::WsPullRequestsReviewerCandidatesRpc>(input, false)
        }
        "pullRequests.requestReviewers" => {
            validate::<rpc::WsPullRequestsRequestReviewersRpc>(input, false)
        }
        "pullRequests.labelCandidates" => {
            validate::<rpc::WsPullRequestsLabelCandidatesRpc>(input, false)
        }
        "pullRequests.setLabels" => validate::<rpc::WsPullRequestsSetLabelsRpc>(input, false),
        "sourceControl.lookupRepository" => {
            validate::<rpc::WsSourceControlLookupRepositoryRpc>(input, false)
        }
        "sourceControl.cloneRepository" => {
            validate::<rpc::WsSourceControlCloneRepositoryRpc>(input, false)
        }
        "sourceControl.signInStart" => validate::<rpc::WsSourceControlSignInStartRpc>(input, false),
        "sourceControl.signInWait" => validate::<rpc::WsSourceControlSignInWaitRpc>(input, false),
        "sourceControl.signInCancel" => {
            validate::<rpc::WsSourceControlSignInCancelRpc>(input, false)
        }
        "sourceControl.appAccess" => validate::<rpc::WsSourceControlAppAccessRpc>(input, false),
        "sourceControl.starred" => validate::<rpc::WsSourceControlStarredRpc>(input, false),
        "sourceControl.star" => validate::<rpc::WsSourceControlStarRpc>(input, false),
        "projectClone.start" => validate::<rpc::WsProjectCloneStartRpc>(input, false),
        "projectClone.cancel" => validate::<rpc::WsProjectCloneCancelRpc>(input, false),
        "projectClone.retry" => validate::<rpc::WsProjectCloneRetryRpc>(input, false),
        "subscribeProjectClones" => validate::<rpc::WsSubscribeProjectClonesRpc>(input, false),
        "sourceControl.publishRepository" => {
            validate::<rpc::WsSourceControlPublishRepositoryRpc>(input, false)
        }
        "projects.searchEntries" => validate::<rpc::WsProjectsSearchEntriesRpc>(input, false),
        "projects.searchContents" => validate::<rpc::WsProjectsSearchContentsRpc>(input, false),
        "projects.listEntries" => validate::<rpc::WsProjectsListEntriesRpc>(input, false),
        "projects.listIgnoredFiles" => validate::<rpc::WsProjectsListIgnoredFilesRpc>(input, false),
        "projects.readFile" => validate::<rpc::WsProjectsReadFileRpc>(input, false),
        "projects.writeFile" => validate::<rpc::WsProjectsWriteFileRpc>(input, false),
        "projects.mutate" => validate::<rpc::WsProjectsMutateRpc>(input, true),
        "projects.ensureScratch" => validate::<rpc::WsProjectsEnsureScratchRpc>(input, false),
        "projects.createNew" => validate::<rpc::WsProjectsCreateNewRpc>(input, false),
        "projects.createFromThread" => validate::<rpc::WsProjectsCreateFromThreadRpc>(input, false),
        "shell.openInEditor" => validate::<rpc::WsShellOpenInEditorRpc>(input, false),
        "filesystem.getMetadata" => validate::<rpc::WsFilesystemGetMetadataRpc>(input, false),
        "filesystem.browse" => validate::<rpc::WsFilesystemBrowseRpc>(input, false),
        "agentSessions.scan" => validate::<rpc::WsAgentSessionsScanRpc>(input, false),
        "agentSessions.import" => validate::<rpc::WsAgentSessionsImportRpc>(input, false),
        "assets.createUrl" => validate::<rpc::WsAssetsCreateUrlRpc>(input, true),
        "assets.persistChatAttachments" => {
            validate::<rpc::WsAssetsPersistChatAttachmentsRpc>(input, false)
        }
        "attachments.createUploadUrl" => {
            validate::<rpc::WsAttachmentsCreateUploadUrlRpc>(input, false)
        }
        "attachments.delete" => validate::<rpc::WsAttachmentsDeleteRpc>(input, false),
        "mcpApps.callTool" => validate::<rpc::WsMcpAppsCallToolRpc>(input, false),
        "mcpApps.toolInfo" => validate::<rpc::WsMcpAppsToolInfoRpc>(input, false),
        "mcpApps.updateModelContext" => {
            validate::<rpc::WsMcpAppsUpdateModelContextRpc>(input, false)
        }
        "mcpApps.readResource" => validate::<rpc::WsMcpAppsReadResourceRpc>(input, false),
        "provider.uploadFeedback" => validate::<rpc::WsProviderUploadFeedbackRpc>(input, false),
        "subscribeVcsBranch" => validate::<rpc::WsSubscribeVcsBranchRpc>(input, false),
        "vcs.refreshBranch" => validate::<rpc::WsVcsRefreshBranchRpc>(input, false),
        "subscribeVcsStatus" => validate::<rpc::WsSubscribeVcsStatusRpc>(input, false),
        "vcs.pull" => validate::<rpc::WsVcsPullRpc>(input, false),
        "vcs.refreshStatus" => validate::<rpc::WsVcsRefreshStatusRpc>(input, false),
        "subscribeWorktreeSetup" => validate::<rpc::WsSubscribeWorktreeSetupRpc>(input, false),
        "worktreeSetup.cancel" => validate::<rpc::WsWorktreeSetupCancelRpc>(input, false),
        "git.runStackedAction" => validate::<rpc::WsGitRunStackedActionRpc>(input, true),
        "git.resolvePullRequest" => validate::<rpc::WsGitResolvePullRequestRpc>(input, false),
        "git.preparePullRequestThread" => {
            validate::<rpc::WsGitPreparePullRequestThreadRpc>(input, true)
        }
        "vcs.listRefs" => validate::<rpc::WsVcsListRefsRpc>(input, false),
        "vcs.createWorktree" => validate::<rpc::WsVcsCreateWorktreeRpc>(input, false),
        "vcs.removeWorktree" => validate::<rpc::WsVcsRemoveWorktreeRpc>(input, false),
        "vcs.createRef" => validate::<rpc::WsVcsCreateRefRpc>(input, false),
        "vcs.switchRef" => validate::<rpc::WsVcsSwitchRefRpc>(input, false),
        "vcs.init" => validate::<rpc::WsVcsInitRpc>(input, false),
        "vcs.trustRepository" => validate::<rpc::WsVcsTrustRepositoryRpc>(input, false),
        "review.getBranchCommits" => validate::<rpc::WsReviewGetBranchCommitsRpc>(input, false),
        "review.getDiffPreview" => validate::<rpc::WsReviewGetDiffPreviewRpc>(input, false),
        "review.getDiffFileContents" => {
            validate::<rpc::WsReviewGetDiffFileContentsRpc>(input, false)
        }
        "terminal.open" => validate::<rpc::WsTerminalOpenRpc>(input, false),
        "terminal.attach" => validate::<rpc::WsTerminalAttachRpc>(input, false),
        "terminal.observe" => validate::<rpc::WsTerminalObserveRpc>(input, false),
        "terminal.write" => validate::<rpc::WsTerminalWriteRpc>(input, false),
        "terminal.resize" => validate::<rpc::WsTerminalResizeRpc>(input, false),
        "terminal.clear" => validate::<rpc::WsTerminalClearRpc>(input, false),
        "terminal.close" => validate::<rpc::WsTerminalCloseRpc>(input, false),
        "preview.open" => validate::<rpc::WsPreviewOpenRpc>(input, false),
        "preview.resize" => validate::<rpc::WsPreviewResizeRpc>(input, false),
        "preview.adjust" => validate::<rpc::WsPreviewAdjustRpc>(input, false),
        "preview.close" => validate::<rpc::WsPreviewCloseRpc>(input, false),
        "preview.list" => validate::<rpc::WsPreviewListRpc>(input, false),
        "preview.clearProfile" => validate::<rpc::WsPreviewClearProfileRpc>(input, false),
        "preview.reportProfiles" => validate::<rpc::WsPreviewReportProfilesRpc>(input, false),
        "preview.reportStatus" => validate::<rpc::WsPreviewReportStatusRpc>(input, false),
        "subscribePreviewEvents" => validate::<rpc::WsSubscribePreviewEventsRpc>(input, false),
        "preview.stopLocalServer" => validate::<rpc::WsPreviewStopLocalServerRpc>(input, false),
        "subscribeDiscoveredLocalServers" => {
            validate::<rpc::WsSubscribeDiscoveredLocalServersRpc>(input, false)
        }
        "device.testHost" => validate::<rpc::WsDeviceTestHostRpc>(input, false),
        "device.easAccount" => validate::<rpc::WsDeviceEasAccountRpc>(input, false),
        "device.simlockIdentity" => validate::<rpc::WsDeviceSimlockIdentityRpc>(input, false),
        "device.list" => validate::<rpc::WsDeviceListRpc>(input, true),
        "device.configure" => validate::<rpc::WsDeviceConfigureRpc>(input, false),
        "device.open" => validate::<rpc::WsDeviceOpenRpc>(input, false),
        "device.close" => validate::<rpc::WsDeviceCloseRpc>(input, false),
        "device.shutdown" => validate::<rpc::WsDeviceShutdownRpc>(input, false),
        "device.detail" => validate::<rpc::WsDeviceDetailRpc>(input, false),
        "device.action" => validate::<rpc::WsDeviceActionRpc>(input, false),
        "subscribeDeviceState" => validate::<rpc::WsSubscribeDeviceStateRpc>(input, false),
        "orchestration.dispatchCommand" => {
            validate::<rpc::WsOrchestrationV2DispatchCommandRpc>(input, false)
        }
        "orchestration.getTurnDiff" => {
            validate::<rpc::WsOrchestrationV2GetTurnDiffRpc>(input, false)
        }
        "orchestration.getFullThreadDiff" => {
            validate::<rpc::WsOrchestrationV2GetFullThreadDiffRpc>(input, false)
        }
        "orchestration.searchThread" => {
            validate::<rpc::WsOrchestrationV2SearchThreadRpc>(input, false)
        }
        "orchestration.searchThreadStream" => {
            validate::<rpc::WsOrchestrationV2SearchThreadStreamRpc>(input, false)
        }
        "orchestration.searchThreads" => {
            validate::<rpc::WsOrchestrationV2SearchThreadsRpc>(input, false)
        }
        "orchestration.getArchivedShellSnapshot" => {
            validate::<rpc::WsOrchestrationV2GetArchivedShellSnapshotRpc>(input, false)
        }
        "orchestration.getThreadFamily" => {
            validate::<rpc::WsOrchestrationV2GetThreadFamilyRpc>(input, false)
        }
        "orchestration.getThreadTitles" => {
            validate::<rpc::WsOrchestrationV2GetThreadTitlesRpc>(input, false)
        }
        "orchestration.getThreadLinkMetadata" => {
            validate::<rpc::WsOrchestrationV2GetThreadLinkMetadataRpc>(input, false)
        }
        "orchestration.getWorktreeThreadIds" => {
            validate::<rpc::WsOrchestrationV2GetWorktreeThreadIdsRpc>(input, false)
        }
        "orchestration.getOlderSettledThreads" => {
            validate::<rpc::WsOrchestrationV2GetOlderSettledThreadsRpc>(input, false)
        }
        "orchestration.getThreadProjection" => {
            validate::<rpc::WsOrchestrationV2GetThreadProjectionRpc>(input, false)
        }
        "orchestration.stopWorkflow" => {
            validate::<rpc::WsOrchestrationV2StopWorkflowRpc>(input, false)
        }
        "orchestration.forceStopThread" => {
            validate::<rpc::WsOrchestrationV2ForceStopThreadRpc>(input, false)
        }
        "orchestration.getWorkflowScript" => {
            validate::<rpc::WsOrchestrationV2GetWorkflowScriptRpc>(input, false)
        }
        "orchestration.getNativeChildWork" => {
            validate::<rpc::WsOrchestrationV2GetNativeChildWorkRpc>(input, false)
        }
        "orchestration.getTurnItem" => {
            validate::<rpc::WsOrchestrationV2GetTurnItemRpc>(input, false)
        }
        "orchestration.launchThread" => {
            validate::<rpc::WsOrchestrationV2LaunchThreadRpc>(input, false)
        }
        "orchestration.subscribeShell" => {
            validate::<rpc::WsOrchestrationV2SubscribeShellRpc>(input, false)
        }
        "orchestration.subscribeThread" => {
            validate::<rpc::WsOrchestrationV2SubscribeThreadRpc>(input, false)
        }
        "orchestration.subscribeCommandOutput" => {
            validate::<rpc::WsOrchestrationV2SubscribeCommandOutputRpc>(input, false)
        }
        "orchestration.subscribePromptSuggestion" => {
            validate::<rpc::WsOrchestrationV2SubscribePromptSuggestionRpc>(input, false)
        }
        "subscribeTerminalMetadata" => {
            validate::<rpc::WsSubscribeTerminalMetadataRpc>(input, false)
        }
        "subscribeServerConfig" => validate::<rpc::WsSubscribeServerConfigRpc>(input, false),
        "subscribeServerLifecycle" => validate::<rpc::WsSubscribeServerLifecycleRpc>(input, false),
        "scheduledTasks.subscribe" => validate::<rpc::WsScheduledTasksSubscribeRpc>(input, false),
        "scheduledTasks.upsert" => validate::<rpc::WsScheduledTasksUpsertRpc>(input, false),
        "scheduledTasks.setEnabled" => validate::<rpc::WsScheduledTasksSetEnabledRpc>(input, false),
        "scheduledTasks.delete" => validate::<rpc::WsScheduledTasksDeleteRpc>(input, false),
        "scheduledTasks.runNow" => validate::<rpc::WsScheduledTasksRunNowRpc>(input, false),
        "scheduledTasks.rotateWebhookToken" => {
            validate::<rpc::WsScheduledTasksRotateWebhookTokenRpc>(input, false)
        }
        "guestShare.create" => validate::<rpc::WsGuestShareCreateRpc>(input, false),
        "secrets.answerRequest" => validate::<rpc::WsSecretsAnswerRequestRpc>(input, false),
        "scheduledTasks.listWebhookDeliveries" => {
            validate::<rpc::WsScheduledTasksListWebhookDeliveriesRpc>(input, false)
        }
        "scheduledTasks.getWebhookDelivery" => {
            validate::<rpc::WsScheduledTasksGetWebhookDeliveryRpc>(input, false)
        }
        "subscribeAuthAccess" => validate::<rpc::WsSubscribeAuthAccessRpc>(input, false),
        "subscribeResourceTelemetry" => {
            validate::<rpc::WsSubscribeResourceTelemetryRpc>(input, false)
        }
        _ => Err(Error::UnknownRpc(format!(
            "RPC method {method} has no declared authorization scope."
        ))),
    }
}

fn validate<M: RpcMethod>(input: &Value, canonical: bool) -> Result<Option<Value>> {
    let payload: M::Payload = serde::Deserialize::deserialize(input)
        .map_err(|_| Error::InvalidPayload(format!("{} payload does not decode", M::TAG)))?;
    if canonical {
        Ok(Some(serde_json::to_value(payload)?))
    } else {
        Ok(None)
    }
}
