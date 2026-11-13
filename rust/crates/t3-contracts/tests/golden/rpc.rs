use super::super::{Registry, register};
use t3_contracts::rpc::*;

pub fn register_module(registry: &mut Registry) {
    register::<<WsAgentSessionsImportRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.agentSessions.import.error",
    );
    register::<<WsAgentSessionsScanRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.agentSessions.scan.error",
    );
    register::<<WsAssetsCreateUrlRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.assets.createUrl.error",
    );
    register::<<WsAssetsPersistChatAttachmentsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.assets.persistChatAttachments.error",
    );
    register::<<WsAttachmentsCreateUploadUrlRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.attachments.createUploadUrl.error",
    );
    register::<<WsAttachmentsDeleteRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.attachments.delete.error",
    );
    register::<<WsCloudGetRelayClientStatusRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.cloud.getRelayClientStatus.error",
    );
    register::<<WsCloudGetRelayClientStatusRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.cloud.getRelayClientStatus.payload",
    );
    register::<<WsCloudInstallRelayClientRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.cloud.installRelayClient.error",
    );
    register::<<WsCloudInstallRelayClientRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.cloud.installRelayClient.payload",
    );
    register::<<WsDeviceActionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.action.error",
    );
    register::<<WsDeviceCloseRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.close.error",
    );
    register::<<WsDeviceConfigureRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.configure.error",
    );
    register::<<WsDeviceDetailRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.detail.error",
    );
    register::<<WsDeviceEasAccountRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.easAccount.error",
    );
    register::<<WsDeviceListRpc as RpcMethod>::Error>(registry, "rpc.WsRpcGroup.device.list.error");
    register::<<WsDeviceOpenRpc as RpcMethod>::Error>(registry, "rpc.WsRpcGroup.device.open.error");
    register::<<WsDeviceShutdownRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.shutdown.error",
    );
    register::<<WsDeviceSimlockIdentityRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.simlockIdentity.error",
    );
    register::<<WsDeviceTestHostRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.device.testHost.error",
    );
    register::<<WsFilesystemBrowseRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.filesystem.browse.error",
    );
    register::<<WsFilesystemGetMetadataRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.filesystem.getMetadata.error",
    );
    register::<<WsGitPreparePullRequestThreadRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.git.preparePullRequestThread.error",
    );
    register::<<WsGitResolvePullRequestRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.git.resolvePullRequest.error",
    );
    register::<<WsGitRunStackedActionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.git.runStackedAction.error",
    );
    register::<<WsGuestShareCreateRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.guestShare.create.error",
    );
    register::<<WsMcpAppsCallToolRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.mcpApps.callTool.error",
    );
    register::<<WsMcpAppsReadResourceRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.mcpApps.readResource.error",
    );
    register::<<WsMcpAppsToolInfoRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.mcpApps.toolInfo.error",
    );
    register::<<WsMcpAppsUpdateModelContextRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.mcpApps.updateModelContext.error",
    );
    register::<<WsOrchestrationV2DispatchCommandRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.dispatchCommand.error",
    );
    register::<<WsOrchestrationV2ForceStopThreadRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.forceStopThread.error",
    );
    register::<<WsOrchestrationV2GetArchivedShellSnapshotRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getArchivedShellSnapshot.error",
    );
    register::<<WsOrchestrationV2GetArchivedShellSnapshotRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.orchestration.getArchivedShellSnapshot.payload",
    );
    register::<<WsOrchestrationV2GetFullThreadDiffRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getFullThreadDiff.error",
    );
    register::<<WsOrchestrationV2GetNativeChildWorkRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getNativeChildWork.error",
    );
    register::<<WsOrchestrationV2GetOlderSettledThreadsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getOlderSettledThreads.error",
    );
    register::<<WsOrchestrationV2GetThreadFamilyRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getThreadFamily.error",
    );
    register::<<WsOrchestrationV2GetThreadLinkMetadataRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getThreadLinkMetadata.error",
    );
    register::<<WsOrchestrationV2GetThreadLinkMetadataRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.orchestration.getThreadLinkMetadata.success",
    );
    register::<<WsOrchestrationV2GetThreadProjectionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getThreadProjection.error",
    );
    register::<<WsOrchestrationV2GetThreadTitlesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getThreadTitles.error",
    );
    register::<<WsOrchestrationV2GetTurnDiffRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getTurnDiff.error",
    );
    register::<<WsOrchestrationV2GetTurnItemRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getTurnItem.error",
    );
    register::<<WsOrchestrationV2GetWorkflowScriptRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getWorkflowScript.error",
    );
    register::<<WsOrchestrationV2GetWorktreeThreadIdsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.getWorktreeThreadIds.error",
    );
    register::<<WsOrchestrationV2LaunchThreadRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.launchThread.error",
    );
    register::<<WsOrchestrationV2SearchThreadRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.searchThread.error",
    );
    register::<<WsOrchestrationV2SearchThreadStreamRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.searchThreadStream.error",
    );
    register::<<WsOrchestrationV2SearchThreadsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.searchThreads.error",
    );
    register::<<WsOrchestrationV2StopWorkflowRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.stopWorkflow.error",
    );
    register::<<WsOrchestrationV2SubscribeCommandOutputRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.subscribeCommandOutput.error",
    );
    register::<<WsOrchestrationV2SubscribePromptSuggestionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.subscribePromptSuggestion.error",
    );
    register::<<WsOrchestrationV2SubscribeShellRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.subscribeShell.error",
    );
    register::<<WsOrchestrationV2SubscribeThreadRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.orchestration.subscribeThread.error",
    );
    register::<<WsPeerLinksAnswerRequestRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.peerLinks.answerRequest.error",
    );
    register::<<WsPeerLinksLinkRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.peerLinks.link.error",
    );
    register::<<WsPeerLinksListRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.peerLinks.list.error",
    );
    register::<<WsPeerLinksListRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.peerLinks.list.payload",
    );
    register::<<WsPeerLinksListRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.peerLinks.list.success",
    );
    register::<<WsPeerLinksUnlinkRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.peerLinks.unlink.error",
    );
    register::<<WsPreviewAdjustRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.adjust.error",
    );
    register::<<WsPreviewClearProfileRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.clearProfile.error",
    );
    register::<<WsPreviewCloseRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.close.error",
    );
    register::<<WsPreviewListRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.list.error",
    );
    register::<<WsPreviewOpenRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.open.error",
    );
    register::<<WsPreviewReportProfilesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.reportProfiles.error",
    );
    register::<<WsPreviewReportStatusRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.reportStatus.error",
    );
    register::<<WsPreviewResizeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.resize.error",
    );
    register::<<WsPreviewStopLocalServerRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.preview.stopLocalServer.error",
    );
    register::<<WsProjectCloneCancelRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projectClone.cancel.error",
    );
    register::<<WsProjectCloneRetryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projectClone.retry.error",
    );
    register::<<WsProjectCloneStartRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projectClone.start.error",
    );
    register::<<WsProjectsCreateFromThreadRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.createFromThread.error",
    );
    register::<<WsProjectsCreateNewRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.createNew.error",
    );
    register::<<WsProjectsEnsureScratchRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.ensureScratch.error",
    );
    register::<<WsProjectsEnsureScratchRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.projects.ensureScratch.payload",
    );
    register::<<WsProjectsListEntriesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.listEntries.error",
    );
    register::<<WsProjectsListIgnoredFilesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.listIgnoredFiles.error",
    );
    register::<<WsProjectsMutateRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.mutate.error",
    );
    register::<<WsProjectsReadFileRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.readFile.error",
    );
    register::<<WsProjectsSearchContentsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.searchContents.error",
    );
    register::<<WsProjectsSearchEntriesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.searchEntries.error",
    );
    register::<<WsProjectsWriteFileRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.projects.writeFile.error",
    );
    register::<<WsProviderAuthCancelRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.auth.cancel.error",
    );
    register::<<WsProviderAuthCompleteRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.auth.complete.error",
    );
    register::<<WsProviderAuthLogoutRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.auth.logout.error",
    );
    register::<<WsProviderAuthRemoveMethodRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.auth.removeMethod.error",
    );
    register::<<WsProviderAuthRespondRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.auth.respond.error",
    );
    register::<<WsProviderAuthStartRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.auth.start.error",
    );
    register::<<WsProviderAuthSubscribeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.auth.subscribe.error",
    );
    register::<<WsChatGptHandoffSubscribeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.chatgpt.handoff.subscribe.error",
    );
    register::<<WsChatGptImportProfileRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.chatgpt.import-profile.error",
    );
    register::<<WsChatGptReconnectProfileRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.chatgpt.reconnect-profile.error",
    );
    register::<<WsChatGptReconnectProfileRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.provider.chatgpt.reconnect-profile.success",
    );
    register::<<WsProviderConsumeResetCreditRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.consumeResetCredit.error",
    );
    register::<<WsProviderInstallCancelRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.install.cancel.error",
    );
    register::<<WsProviderInstallRemoveRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.install.remove.error",
    );
    register::<<WsProviderInstallStartRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.install.start.error",
    );
    register::<<WsProviderInstallSubscribeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.install.subscribe.error",
    );
    register::<<WsProviderUploadFeedbackRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.provider.uploadFeedback.error",
    );
    register::<<WsPullRequestsActivityRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.activity.error",
    );
    register::<<WsPullRequestsChangeBreakdownRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.changeBreakdown.error",
    );
    register::<<WsPullRequestsChecksRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.checks.error",
    );
    register::<<WsPullRequestsChecksRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.pullRequests.checks.success",
    );
    register::<<WsPullRequestsCommentRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.comment.error",
    );
    register::<<WsPullRequestsDetailRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.detail.error",
    );
    register::<<WsPullRequestsDiffFileContentsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.diffFileContents.error",
    );
    register::<<WsPullRequestsFilesViewedRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.filesViewed.error",
    );
    register::<<WsPullRequestsFilterOptionsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.filterOptions.error",
    );
    register::<<WsPullRequestsGenerateReviewGuideRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.generateReviewGuide.error",
    );
    register::<<WsPullRequestsInvalidateRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.invalidate.error",
    );
    register::<<WsPullRequestsLabelCandidatesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.labelCandidates.error",
    );
    register::<<WsPullRequestsLinkedThreadsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.linkedThreads.error",
    );
    register::<<WsPullRequestsListRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.list.error",
    );
    register::<<WsPullRequestsListStatsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.listStats.error",
    );
    register::<<WsPullRequestsPreviewRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.preview.error",
    );
    register::<<WsPullRequestsReplyToThreadRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.replyToThread.error",
    );
    register::<<WsPullRequestsReportStateRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.reportState.error",
    );
    register::<<WsPullRequestsRequestReviewersRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.requestReviewers.error",
    );
    register::<<WsPullRequestsReviewGuideRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.reviewGuide.error",
    );
    register::<<WsPullRequestsReviewerCandidatesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.reviewerCandidates.error",
    );
    register::<<WsPullRequestsRoutingRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.routing.error",
    );
    register::<<WsPullRequestsRoutingIdentityRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.routingIdentity.error",
    );
    register::<<WsPullRequestsRunActionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.runAction.error",
    );
    register::<<WsPullRequestsRunActionRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.pullRequests.runAction.success",
    );
    register::<<WsPullRequestsSetFilesViewedRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.setFilesViewed.error",
    );
    register::<<WsPullRequestsSetLabelsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.setLabels.error",
    );
    register::<<WsPullRequestsSetReactionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.setReaction.error",
    );
    register::<<WsPullRequestsSetThreadResolutionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.setThreadResolution.error",
    );
    register::<<WsPullRequestsStackRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.stack.error",
    );
    register::<<WsPullRequestsStackRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.pullRequests.stack.success",
    );
    register::<<WsPullRequestsSubmitReviewRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.submitReview.error",
    );
    register::<<WsPullRequestsSubscribeLinkedThreadRefreshesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.subscribeLinkedThreadRefreshes.error",
    );
    register::<<WsPullRequestsSubscribeLinkedThreadRefreshesRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.pullRequests.subscribeLinkedThreadRefreshes.payload",
    );
    register::<<WsPullRequestsSubscribeRefreshesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.subscribeRefreshes.error",
    );
    register::<<WsPullRequestsSubscribeRefreshesRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.pullRequests.subscribeRefreshes.payload",
    );
    register::<<WsPullRequestsSummaryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.summary.error",
    );
    register::<<WsPullRequestsThreadCommentsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.threadComments.error",
    );
    register::<<WsPullRequestsUpdateRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.update.error",
    );
    register::<<WsPullRequestsUpdateCommentRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.pullRequests.updateComment.error",
    );
    register::<<WsReviewGetBranchCommitsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.review.getBranchCommits.error",
    );
    register::<<WsReviewGetDiffFileContentsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.review.getDiffFileContents.error",
    );
    register::<<WsReviewGetDiffPreviewRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.review.getDiffPreview.error",
    );
    register::<<WsScheduledTasksDeleteRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.delete.error",
    );
    register::<<WsScheduledTasksGetWebhookDeliveryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.getWebhookDelivery.error",
    );
    register::<<WsScheduledTasksListWebhookDeliveriesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.listWebhookDeliveries.error",
    );
    register::<<WsScheduledTasksRotateWebhookTokenRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.rotateWebhookToken.error",
    );
    register::<<WsScheduledTasksRunNowRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.runNow.error",
    );
    register::<<WsScheduledTasksSetEnabledRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.setEnabled.error",
    );
    register::<<WsScheduledTasksSubscribeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.subscribe.error",
    );
    register::<<WsScheduledTasksUpsertRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.scheduledTasks.upsert.error",
    );
    register::<<WsSecretsAnswerRequestRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.secrets.answerRequest.error",
    );
    register::<<WsServerAcceptAcpRegistryUrlAuthRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.acceptAcpRegistryUrlAuth.error",
    );
    register::<<WsServerCheckOtlpEndpointRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.checkOtlpEndpoint.error",
    );
    register::<<WsServerCheckSecretSourceRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.checkSecretSource.error",
    );
    register::<<WsServerCheckSecretSourceRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.checkSecretSource.payload",
    );
    register::<<WsServerCommitDesktopUpdateRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.commitDesktopUpdate.error",
    );
    register::<<WsServerDeleteAcpRegistrySessionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.deleteAcpRegistrySession.error",
    );
    register::<<WsServerDisableAcpRegistryProviderRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.disableAcpRegistryProvider.error",
    );
    register::<<WsServerDiscoverSourceControlRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.discoverSourceControl.error",
    );
    register::<<WsServerDiscoverSourceControlRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.discoverSourceControl.payload",
    );
    register::<<WsServerGetConfigRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getConfig.error",
    );
    register::<<WsServerGetConfigRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.getConfig.payload",
    );
    register::<<WsServerGetDatabaseStorageRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getDatabaseStorage.error",
    );
    register::<<WsServerGetDatabaseStorageRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.getDatabaseStorage.payload",
    );
    register::<<WsServerGetDatabaseStorageRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.server.getDatabaseStorage.success",
    );
    register::<<WsServerGetHostResourcesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getHostResources.error",
    );
    register::<<WsServerGetHostResourcesRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.getHostResources.payload",
    );
    register::<<WsServerGetNpmReleaseRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getNpmRelease.error",
    );
    register::<<WsServerGetNpmReleaseRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.server.getNpmRelease.success",
    );
    register::<<WsServerGetProcessDiagnosticsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getProcessDiagnostics.error",
    );
    register::<<WsServerGetProcessDiagnosticsRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.getProcessDiagnostics.payload",
    );
    register::<<WsServerGetProcessResourceHistoryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getProcessResourceHistory.error",
    );
    register::<<WsServerGetResourceTelemetryHistoryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getResourceTelemetryHistory.error",
    );
    register::<<WsServerGetStorageCleanupReportRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getStorageCleanupReport.error",
    );
    register::<<WsServerGetStorageCleanupReportRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.getStorageCleanupReport.payload",
    );
    register::<<WsServerGetStorageCleanupReportRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.server.getStorageCleanupReport.success",
    );
    register::<<WsServerGetTraceDiagnosticsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getTraceDiagnostics.error",
    );
    register::<<WsServerGetTraceDiagnosticsRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.getTraceDiagnostics.payload",
    );
    register::<<WsServerGetUsageSummaryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.getUsageSummary.error",
    );
    register::<<WsServerImportAcpRegistrySessionRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.importAcpRegistrySession.error",
    );
    register::<<WsServerListAcpRegistryProvidersRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.listAcpRegistryProviders.error",
    );
    register::<<WsServerListAcpRegistrySessionsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.listAcpRegistrySessions.error",
    );
    register::<<WsServerListTextGeneratorModelsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.listTextGeneratorModels.error",
    );
    register::<<WsServerLogoutAcpRegistryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.logoutAcpRegistry.error",
    );
    register::<<WsServerPrepareAcpRegistryAgentRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.prepareAcpRegistryAgent.error",
    );
    register::<<WsServerPrepareTriageRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.prepareTriage.error",
    );
    register::<<WsServerProbeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.probe.error",
    );
    register::<<WsServerProbeRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.probe.payload",
    );
    register::<<WsServerProbeRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.server.probe.success",
    );
    register::<<WsServerReclaimDatabaseSpaceRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.reclaimDatabaseSpace.error",
    );
    register::<<WsServerReclaimDatabaseSpaceRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.reclaimDatabaseSpace.payload",
    );
    register::<<WsServerReclaimDatabaseSpaceRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.server.reclaimDatabaseSpace.success",
    );
    register::<<WsServerRefreshProvidersRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.refreshProviders.error",
    );
    register::<<WsServerRefreshProvidersRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.refreshProviders.payload",
    );
    register::<<WsServerRefreshUsageRatesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.refreshUsageRates.error",
    );
    register::<<WsServerRefreshUsageRatesRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.refreshUsageRates.payload",
    );
    register::<<WsServerRemoveKeybindingRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.removeKeybinding.error",
    );
    register::<<WsServerReportClientActivityRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.reportClientActivity.error",
    );
    register::<<WsServerRetryResourceTelemetryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.retryResourceTelemetry.error",
    );
    register::<<WsServerRetryResourceTelemetryRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.retryResourceTelemetry.payload",
    );
    register::<<WsServerRunStorageCleanupRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.runStorageCleanup.error",
    );
    register::<<WsServerRunStorageCleanupRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.runStorageCleanup.payload",
    );
    register::<<WsServerSearchAcpRegistryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.searchAcpRegistry.error",
    );
    register::<<WsServerSetAcpRegistryProviderRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.setAcpRegistryProvider.error",
    );
    register::<<WsServerSignalProcessRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.signalProcess.error",
    );
    register::<<WsServerUninstallAcpRegistryManagedBinaryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.uninstallAcpRegistryManagedBinary.error",
    );
    register::<<WsServerUpdateProviderRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.updateProvider.error",
    );
    register::<<WsServerUpdateServerRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.updateServer.error",
    );
    register::<<WsServerUpdateServerWithProgressRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.updateServerWithProgress.error",
    );
    register::<<WsServerUpdateSettingsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.updateSettings.error",
    );
    register::<<WsServerUpdateSettingsRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.server.updateSettings.payload",
    );
    register::<<WsServerUpsertKeybindingRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.server.upsertKeybinding.error",
    );
    register::<<WsShellOpenInEditorRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.shell.openInEditor.error",
    );
    register::<<WsSourceControlAppAccessRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.appAccess.error",
    );
    register::<<WsSourceControlCloneRepositoryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.cloneRepository.error",
    );
    register::<<WsSourceControlLookupRepositoryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.lookupRepository.error",
    );
    register::<<WsSourceControlPublishRepositoryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.publishRepository.error",
    );
    register::<<WsSourceControlSignInCancelRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.signInCancel.error",
    );
    register::<<WsSourceControlSignInStartRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.signInStart.error",
    );
    register::<<WsSourceControlSignInWaitRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.signInWait.error",
    );
    register::<<WsSourceControlStarRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.star.error",
    );
    register::<<WsSourceControlStarredRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.sourceControl.starred.error",
    );
    register::<<WsSourceControlStarredRpc as RpcMethod>::Success>(
        registry,
        "rpc.WsRpcGroup.sourceControl.starred.success",
    );
    register::<<WsSubscribeAuthAccessRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeAuthAccess.error",
    );
    register::<<WsSubscribeAuthAccessRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribeAuthAccess.payload",
    );
    register::<<WsSubscribeDeviceStateRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeDeviceState.error",
    );
    register::<<WsSubscribeDeviceStateRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribeDeviceState.payload",
    );
    register::<<WsSubscribeDiscoveredLocalServersRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeDiscoveredLocalServers.error",
    );
    register::<<WsSubscribeDiscoveredLocalServersRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribeDiscoveredLocalServers.payload",
    );
    register::<<WsSubscribePreviewEventsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribePreviewEvents.error",
    );
    register::<<WsSubscribePreviewEventsRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribePreviewEvents.payload",
    );
    register::<<WsSubscribeProjectClonesRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeProjectClones.error",
    );
    register::<<WsSubscribeResourceTelemetryRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeResourceTelemetry.error",
    );
    register::<<WsSubscribeResourceTelemetryRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribeResourceTelemetry.payload",
    );
    register::<<WsSubscribeServerConfigRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeServerConfig.error",
    );
    register::<<WsSubscribeServerConfigRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribeServerConfig.payload",
    );
    register::<<WsSubscribeServerLifecycleRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeServerLifecycle.error",
    );
    register::<<WsSubscribeServerLifecycleRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribeServerLifecycle.payload",
    );
    register::<<WsSubscribeTerminalMetadataRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeTerminalMetadata.error",
    );
    register::<<WsSubscribeTerminalMetadataRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.subscribeTerminalMetadata.payload",
    );
    register::<<WsSubscribeVcsBranchRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeVcsBranch.error",
    );
    register::<<WsSubscribeVcsStatusRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeVcsStatus.error",
    );
    register::<<WsSubscribeWorktreeSetupRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.subscribeWorktreeSetup.error",
    );
    register::<<WsTerminalAttachRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.terminal.attach.error",
    );
    register::<<WsTerminalClearRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.terminal.clear.error",
    );
    register::<<WsTerminalCloseRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.terminal.close.error",
    );
    register::<<WsTerminalObserveRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.terminal.observe.error",
    );
    register::<<WsTerminalOpenRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.terminal.open.error",
    );
    register::<<WsTerminalResizeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.terminal.resize.error",
    );
    register::<<WsTerminalWriteRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.terminal.write.error",
    );
    register::<<WsVcsCreateRefRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.vcs.createRef.error",
    );
    register::<<WsVcsCreateWorktreeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.vcs.createWorktree.error",
    );
    register::<<WsVcsCreateWorktreeRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.vcs.createWorktree.payload",
    );
    register::<<WsVcsInitRpc as RpcMethod>::Error>(registry, "rpc.WsRpcGroup.vcs.init.error");
    register::<<WsVcsListRefsRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.vcs.listRefs.error",
    );
    register::<<WsVcsPullRpc as RpcMethod>::Error>(registry, "rpc.WsRpcGroup.vcs.pull.error");
    register::<<WsVcsRefreshBranchRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.vcs.refreshBranch.error",
    );
    register::<<WsVcsRefreshStatusRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.vcs.refreshStatus.error",
    );
    register::<<WsVcsRefreshStatusRpc as RpcMethod>::Payload>(
        registry,
        "rpc.WsRpcGroup.vcs.refreshStatus.payload",
    );
    register::<<WsVcsRemoveWorktreeRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.vcs.removeWorktree.error",
    );
    register::<<WsVcsSwitchRefRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.vcs.switchRef.error",
    );
    register::<<WsWorktreeSetupCancelRpc as RpcMethod>::Error>(
        registry,
        "rpc.WsRpcGroup.worktreeSetup.cancel.error",
    );
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("rpc", register_module)
}

#[test]
fn method_table_matches_manifest() -> Result<(), Box<dyn std::error::Error>> {
    use serde::Deserialize;
    use std::collections::BTreeMap;
    use t3_contracts::{
        auth::AuthEnvironmentScope,
        client_rpc_permissions::{CLIENT_GUARDED_RPC_SCOPES, client_rpc_required_scope},
    };
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Method {
        group: String,
        tag: String,
        streams: bool,
        required_scope: Option<AuthEnvironmentScope>,
    }
    let manifest: Vec<Method> =
        serde_json::from_str(include_str!("../../../../fixtures/rpc.json"))?;
    let expected: BTreeMap<_, _> = manifest
        .into_iter()
        .filter(|method| method.group == "WsRpcGroup")
        .map(|method| (method.tag.clone(), method))
        .collect();
    let mut actual = BTreeMap::new();
    for method in WsRpcGroup::METHODS {
        assert!(
            actual.insert(method.tag, method).is_none(),
            "Duplicate method {}",
            method.tag
        );
    }
    assert_eq!(actual.len(), expected.len(), "WebSocket method count");
    for (tag, expected) in &expected {
        let actual = actual
            .get(tag.as_str())
            .unwrap_or_else(|| panic!("Missing Rust method {tag}"));
        assert_eq!(actual.tag, expected.tag);
        assert_eq!(actual.streams, expected.streams, "{tag} streaming");
        assert_eq!(
            actual.required_scope, expected.required_scope,
            "{tag} scope"
        );
        assert_eq!(
            client_rpc_required_scope(tag),
            expected.required_scope,
            "{tag} client scope"
        );
    }
    for guarded in CLIENT_GUARDED_RPC_SCOPES {
        assert!(
            actual.contains_key(guarded.tag()),
            "Guarded method missing from table: {}",
            guarded.tag()
        );
    }
    Ok(())
}

#[test]
fn worktree_creation_requires_explicit_null_path() -> Result<(), serde_json::Error> {
    use serde_json::{from_value, json};
    type Payload = <WsVcsCreateWorktreeRpc as RpcMethod>::Payload;
    from_value::<Payload>(json!({"cwd": "repo", "refName": "main", "path": null}))?;
    for input in [
        json!({"cwd": "repo", "refName": "main"}),
        json!({"cwd": "repo", "refName": "main", "path": "custom"}),
    ] {
        assert!(from_value::<Payload>(input).is_err());
    }
    Ok(())
}
