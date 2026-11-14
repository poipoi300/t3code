use super::*;

methods! {
    WsServerUpsertKeybindingRpc => (
        "server.upsertKeybinding",
        crate::server::ServerUpsertKeybindingInput,
        crate::server::ServerUpsertKeybindingResult,
        WsServerUpsertKeybindingError,
        false,
        None
    ),
    WsServerRemoveKeybindingRpc => (
        "server.removeKeybinding",
        crate::server::ServerRemoveKeybindingInput,
        crate::server::ServerRemoveKeybindingResult,
        WsServerUpsertKeybindingError,
        false,
        None
    ),
    WsServerProbeRpc => (
        "server.probe",
        Empty,
        Empty,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerGetConfigRpc => (
        "server.getConfig",
        Empty,
        crate::server::ServerConfig,
        WsServerGetConfigError,
        false,
        None
    ),
    WsServerRefreshProvidersRpc => (
        "server.refreshProviders",
        WsServerRefreshProvidersPayload,
        crate::server::ServerProviderUpdatedPayload,
        WsServerRefreshProvidersError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ServerRefreshProviders.scope())
    ),
    WsServerUpdateProviderRpc => (
        "server.updateProvider",
        crate::server::ServerProviderUpdateInput,
        crate::server::ServerProviderUpdatedPayload,
        WsServerUpdateProviderError,
        false,
        None
    ),
    WsProviderConsumeResetCreditRpc => (
        "provider.consumeResetCredit",
        crate::provider_usage_limits::ProviderConsumeResetCreditInput,
        crate::provider_usage_limits::ProviderConsumeResetCreditResult,
        WsProviderConsumeResetCreditError,
        false,
        None
    ),
    WsProviderAuthStartRpc => (
        "provider.auth.start",
        crate::provider_setup::ProviderAuthStartInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsProviderAuthRespondRpc => (
        "provider.auth.respond",
        crate::provider_setup::ProviderAuthRespondInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsProviderAuthCompleteRpc => (
        "provider.auth.complete",
        crate::provider_setup::ProviderAuthCompleteInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsChatGptReconnectProfileRpc => (
        "provider.chatgpt.reconnect-profile",
        crate::provider_setup::ChatGptReconnectProfileInput,
        Option<crate::provider_setup::ChatGptReconnectProfile>,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsChatGptImportProfileRpc => (
        "provider.chatgpt.import-profile",
        crate::provider_setup::ChatGptImportProfileInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsChatGptHandoffSubscribeRpc => (
        "provider.chatgpt.handoff.subscribe",
        crate::provider_setup::ChatGptHandoffInput,
        crate::provider_setup::ChatGptHandoffState,
        ProviderSetupRpcError,
        true,
        None
    ),
    WsProviderAuthCancelRpc => (
        "provider.auth.cancel",
        crate::provider_setup::ProviderAuthCancelInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsProviderAuthRemoveMethodRpc => (
        "provider.auth.removeMethod",
        crate::provider_setup::ProviderAuthRemoveMethodInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ProviderAuthRemoveMethod.scope())
    ),
    WsProviderAuthLogoutRpc => (
        "provider.auth.logout",
        crate::provider_setup::ProviderSetupInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsProviderAuthSubscribeRpc => (
        "provider.auth.subscribe",
        crate::provider_setup::ProviderSetupInput,
        crate::provider_setup::ProviderAuthState,
        ProviderSetupRpcError,
        true,
        None
    ),
    WsProviderInstallStartRpc => (
        "provider.install.start",
        crate::provider_setup::ProviderSetupInput,
        crate::provider_setup::ProviderInstallState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsProviderInstallCancelRpc => (
        "provider.install.cancel",
        crate::provider_setup::ProviderInstallCancelInput,
        crate::provider_setup::ProviderInstallState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsProviderInstallSubscribeRpc => (
        "provider.install.subscribe",
        crate::provider_setup::ProviderSetupInput,
        crate::provider_setup::ProviderInstallState,
        ProviderSetupRpcError,
        true,
        None
    ),
    WsProviderInstallRemoveRpc => (
        "provider.install.remove",
        crate::provider_setup::ProviderSetupInput,
        crate::provider_setup::ProviderInstallState,
        ProviderSetupRpcError,
        false,
        None
    ),
    WsServerGetNpmReleaseRpc => (
        "server.getNpmRelease",
        crate::server::ServerNpmReleaseInput,
        Option<crate::server::ServerNpmRelease>,
        WsServerGetNpmReleaseError,
        false,
        None
    ),
    WsServerUpdateServerRpc => (
        "server.updateServer",
        crate::server::ServerSelfUpdateInput,
        crate::server::ServerSelfUpdateResult,
        WsServerUpdateServerError,
        false,
        None
    ),
    WsServerUpdateServerWithProgressRpc => (
        "server.updateServerWithProgress",
        crate::server::ServerSelfUpdateInput,
        crate::server::ServerSelfUpdateProgressEvent,
        WsServerUpdateServerError,
        true,
        None
    ),
    WsServerCommitDesktopUpdateRpc => (
        "server.commitDesktopUpdate",
        crate::server::DesktopUpdateCommitInput,
        crate::server::ServerSelfUpdateResult,
        WsServerUpdateServerError,
        false,
        None
    ),
    WsServerRunStorageCleanupRpc => (
        "server.runStorageCleanup",
        Empty,
        crate::settings::StorageCleanupReport,
        WsServerRunStorageCleanupError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ServerRunStorageCleanup.scope())
    ),
    WsServerGetStorageCleanupReportRpc => (
        "server.getStorageCleanupReport",
        Empty,
        Option<crate::settings::StorageCleanupReport>,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsServerGetDatabaseStorageRpc => (
        "server.getDatabaseStorage",
        Empty,
        Option<crate::settings::DatabaseStorageStatus>,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsServerReclaimDatabaseSpaceRpc => (
        "server.reclaimDatabaseSpace",
        Empty,
        (),
        WsServerReclaimDatabaseSpaceError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ServerReclaimDatabaseSpace.scope())
    ),
    WsPeerLinksListRpc => (
        "peerLinks.list",
        Empty,
        WsPeerLinksListSuccess,
        WsPeerLinksListError,
        false,
        None
    ),
    WsPeerLinksLinkRpc => (
        "peerLinks.link",
        crate::peer_link::PeerLinkCreateInput,
        crate::peer_link::PeerLink,
        WsPeerLinksListError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PeerLinksLink.scope())
    ),
    WsPeerLinksUnlinkRpc => (
        "peerLinks.unlink",
        crate::peer_link::PeerLinkRemoveInput,
        crate::peer_link::PeerLinkRemoveResult,
        WsPeerLinksListError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PeerLinksUnlink.scope())
    ),
    WsPeerLinksAnswerRequestRpc => (
        "peerLinks.answerRequest",
        crate::peer_link::PeerLinkRequestAnswerInput,
        (),
        WsPeerLinksAnswerRequestError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PeerLinksAnswerRequest.scope())
    ),
    WsServerUpdateSettingsRpc => (
        "server.updateSettings",
        WsServerUpdateSettingsPayload,
        crate::settings::ServerSettings,
        WsServerRunStorageCleanupError,
        false,
        None
    ),
    WsServerDiscoverSourceControlRpc => (
        "server.discoverSourceControl",
        Empty,
        crate::source_control::SourceControlDiscoveryResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerCheckSecretSourceRpc => (
        "server.checkSecretSource",
        WsServerCheckSecretSourcePayload,
        crate::settings_secret_value::SecretSourceCheckResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ServerCheckSecretSource.scope())
    ),
    WsServerSearchAcpRegistryRpc => (
        "server.searchAcpRegistry",
        crate::acp_registry::AcpRegistrySearchInput,
        crate::acp_registry::AcpRegistrySearchResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerPrepareAcpRegistryAgentRpc => (
        "server.prepareAcpRegistryAgent",
        crate::acp_registry::AcpRegistryPrepareInput,
        crate::acp_registry::AcpRegistryPrepareResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerUninstallAcpRegistryManagedBinaryRpc => (
        "server.uninstallAcpRegistryManagedBinary",
        crate::acp_registry::AcpRegistryManagedBinaryUninstallInput,
        crate::acp_registry::AcpRegistryManagedBinaryUninstallResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerAcceptAcpRegistryUrlAuthRpc => (
        "server.acceptAcpRegistryUrlAuth",
        crate::acp_registry::AcpRegistryAcceptUrlAuthInput,
        crate::acp_registry::AcpRegistryAcceptUrlAuthResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerListAcpRegistrySessionsRpc => (
        "server.listAcpRegistrySessions",
        crate::acp_registry::AcpRegistryListSessionsInput,
        crate::acp_registry::AcpRegistryListSessionsResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerImportAcpRegistrySessionRpc => (
        "server.importAcpRegistrySession",
        crate::acp_registry::AcpRegistryImportSessionInput,
        crate::acp_registry::AcpRegistryImportSessionResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerDeleteAcpRegistrySessionRpc => (
        "server.deleteAcpRegistrySession",
        crate::acp_registry::AcpRegistryDeleteSessionInput,
        crate::acp_registry::AcpRegistryDeleteSessionResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerListAcpRegistryProvidersRpc => (
        "server.listAcpRegistryProviders",
        crate::acp_registry::AcpRegistryListProvidersInput,
        crate::acp_registry::AcpRegistryListProvidersResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerSetAcpRegistryProviderRpc => (
        "server.setAcpRegistryProvider",
        crate::acp_registry::AcpRegistrySetProviderInput,
        crate::acp_registry::AcpRegistrySetProviderResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerDisableAcpRegistryProviderRpc => (
        "server.disableAcpRegistryProvider",
        crate::acp_registry::AcpRegistryDisableProviderInput,
        crate::acp_registry::AcpRegistryDisableProviderResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerLogoutAcpRegistryRpc => (
        "server.logoutAcpRegistry",
        crate::acp_registry::AcpRegistryLogoutInput,
        crate::acp_registry::AcpRegistryLogoutResult,
        WsServerSearchAcpRegistryError,
        false,
        None
    ),
    WsServerGetTraceDiagnosticsRpc => (
        "server.getTraceDiagnostics",
        Empty,
        crate::server::ServerTraceDiagnosticsResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerGetProcessDiagnosticsRpc => (
        "server.getProcessDiagnostics",
        Empty,
        crate::server::ServerProcessDiagnosticsResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerGetHostResourcesRpc => (
        "server.getHostResources",
        Empty,
        crate::resource_telemetry::HostResourcesSnapshot,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerGetProcessResourceHistoryRpc => (
        "server.getProcessResourceHistory",
        crate::server::ServerProcessResourceHistoryInput,
        crate::server::ServerProcessResourceHistoryResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerGetResourceTelemetryHistoryRpc => (
        "server.getResourceTelemetryHistory",
        crate::resource_telemetry::ResourceTelemetryHistoryInput,
        crate::resource_telemetry::ResourceTelemetryHistory,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerRetryResourceTelemetryRpc => (
        "server.retryResourceTelemetry",
        Empty,
        crate::resource_telemetry::ResourceTelemetryRetryResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerCheckOtlpEndpointRpc => (
        "server.checkOtlpEndpoint",
        crate::server::OtlpEndpointCheckInput,
        crate::server::OtlpEndpointCheckResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ServerCheckOtlpEndpoint.scope())
    ),
    WsServerListTextGeneratorModelsRpc => (
        "server.listTextGeneratorModels",
        crate::settings::ListTextGeneratorModelsInput,
        crate::settings::ListTextGeneratorModelsResult,
        WsServerListTextGeneratorModelsError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ServerListTextGeneratorModels.scope())
    ),
    WsServerPrepareTriageRpc => (
        "server.prepareTriage",
        crate::server::ServerPrepareTriageInput,
        crate::server::ServerPrepareTriageResult,
        WsServerPrepareTriageError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ServerPrepareTriage.scope())
    ),
    WsServerGetUsageSummaryRpc => (
        "server.getUsageSummary",
        crate::usage::UsageSummaryInput,
        crate::usage::UsageSummary,
        WsServerGetUsageSummaryError,
        false,
        None
    ),
    WsServerRefreshUsageRatesRpc => (
        "server.refreshUsageRates",
        Empty,
        crate::usage::UsagePricing,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsServerSignalProcessRpc => (
        "server.signalProcess",
        crate::server::ServerSignalProcessInput,
        crate::server::ServerSignalProcessResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsCloudGetRelayClientStatusRpc => (
        "cloud.getRelayClientStatus",
        Empty,
        crate::relay_client::RelayClientStatusSchema,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsCloudInstallRelayClientRpc => (
        "cloud.installRelayClient",
        Empty,
        crate::relay_client::RelayClientInstallProgressEventSchema,
        WsCloudInstallRelayClientError,
        true,
        None
    ),
    WsServerReportClientActivityRpc => (
        "server.reportClientActivity",
        crate::background::ClientActivityReportInput,
        (),
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsPullRequestsListRpc => (
        "pullRequests.list",
        crate::pull_request::PullRequestListInput,
        crate::pull_request::PullRequestListResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsListStatsRpc => (
        "pullRequests.listStats",
        crate::pull_request::PullRequestListStatsInput,
        crate::pull_request::PullRequestListStatsResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsFilterOptionsRpc => (
        "pullRequests.filterOptions",
        crate::pull_request::PullRequestFilterOptionsInput,
        crate::pull_request::PullRequestFilterOptions,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsRoutingRpc => (
        "pullRequests.routing",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestRoutingResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsRoutingIdentityRpc => (
        "pullRequests.routingIdentity",
        crate::pull_request::PullRequestRoutingIdentityInput,
        crate::pull_request::PullRequestRoutingIdentityResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsSummaryRpc => (
        "pullRequests.summary",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestSummary,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsStackRpc => (
        "pullRequests.stack",
        crate::pull_request::PullRequestRef,
        Option<crate::pull_request::PullRequestStack>,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsLinkedThreadsRpc => (
        "pullRequests.linkedThreads",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestLinkedThreadsResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsDetailRpc => (
        "pullRequests.detail",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestDetail,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsPreviewRpc => (
        "pullRequests.preview",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestPreview,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsChecksRpc => (
        "pullRequests.checks",
        crate::pull_request::PullRequestRef,
        Option<crate::pull_request::PullRequestChecks>,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsActivityRpc => (
        "pullRequests.activity",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestActivity,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsReviewGuideRpc => (
        "pullRequests.reviewGuide",
        crate::pull_request::PullRequestRef,
        crate::pull_request_review_guide::PullRequestReviewGuideResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsGenerateReviewGuideRpc => (
        "pullRequests.generateReviewGuide",
        crate::pull_request::PullRequestRef,
        crate::pull_request_review_guide::PullRequestReviewGuideResult,
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsGenerateReviewGuide.scope())
    ),
    WsPullRequestsThreadCommentsRpc => (
        "pullRequests.threadComments",
        crate::pull_request::PullRequestThreadCommentsInput,
        crate::pull_request::PullRequestThreadCommentsResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsDiffFileContentsRpc => (
        "pullRequests.diffFileContents",
        crate::pull_request::PullRequestDiffFileContentsInput,
        crate::pull_request::PullRequestDiffFileContentsResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsFilesViewedRpc => (
        "pullRequests.filesViewed",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestFilesViewedResult,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsChangeBreakdownRpc => (
        "pullRequests.changeBreakdown",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestChangeBreakdown,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsSetFilesViewedRpc => (
        "pullRequests.setFilesViewed",
        crate::pull_request::PullRequestSetFilesViewedInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsSetFilesViewed.scope())
    ),
    WsPullRequestsRunActionRpc => (
        "pullRequests.runAction",
        crate::pull_request::PullRequestActionInput,
        Option<crate::pull_request::PullRequestSummary>,
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsRunAction.scope())
    ),
    WsPullRequestsUpdateRpc => (
        "pullRequests.update",
        crate::pull_request::PullRequestUpdateInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsUpdate.scope())
    ),
    WsPullRequestsCommentRpc => (
        "pullRequests.comment",
        crate::pull_request::PullRequestCommentInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsComment.scope())
    ),
    WsPullRequestsUpdateCommentRpc => (
        "pullRequests.updateComment",
        crate::pull_request::PullRequestCommentUpdateInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsUpdateComment.scope())
    ),
    WsPullRequestsSubmitReviewRpc => (
        "pullRequests.submitReview",
        crate::pull_request::PullRequestSubmitReviewInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsSubmitReview.scope())
    ),
    WsPullRequestsReplyToThreadRpc => (
        "pullRequests.replyToThread",
        crate::pull_request::PullRequestThreadReplyInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsReplyToThread.scope())
    ),
    WsPullRequestsSetThreadResolutionRpc => (
        "pullRequests.setThreadResolution",
        crate::pull_request::PullRequestThreadResolutionInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsSetThreadResolution.scope())
    ),
    WsPullRequestsSetReactionRpc => (
        "pullRequests.setReaction",
        crate::pull_request::PullRequestReactionInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsSetReaction.scope())
    ),
    WsPullRequestsInvalidateRpc => (
        "pullRequests.invalidate",
        crate::pull_request::PullRequestInvalidateInput,
        (),
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsReportStateRpc => (
        "pullRequests.reportState",
        crate::pull_request::PullRequestReportStateInput,
        (),
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsSubscribeRefreshesRpc => (
        "pullRequests.subscribeRefreshes",
        Empty,
        u64,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsPullRequestsSubscribeLinkedThreadRefreshesRpc => (
        "pullRequests.subscribeLinkedThreadRefreshes",
        Empty,
        u64,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsPullRequestsReviewerCandidatesRpc => (
        "pullRequests.reviewerCandidates",
        crate::pull_request::PullRequestReviewerCandidatesInput,
        crate::pull_request::PullRequestReviewerCandidateList,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsRequestReviewersRpc => (
        "pullRequests.requestReviewers",
        crate::pull_request::PullRequestReviewerRequestInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsRequestReviewers.scope())
    ),
    WsPullRequestsLabelCandidatesRpc => (
        "pullRequests.labelCandidates",
        crate::pull_request::PullRequestRef,
        crate::pull_request::PullRequestLabelCandidateList,
        PullRequestRpcError,
        false,
        None
    ),
    WsPullRequestsSetLabelsRpc => (
        "pullRequests.setLabels",
        crate::pull_request::PullRequestLabelChangeInput,
        (),
        PullRequestRpcError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PullRequestsSetLabels.scope())
    ),
    WsSourceControlLookupRepositoryRpc => (
        "sourceControl.lookupRepository",
        crate::source_control::SourceControlRepositoryLookupInput,
        crate::source_control::SourceControlRepositoryInfo,
        WsSourceControlLookupRepositoryError,
        false,
        None
    ),
    WsSourceControlCloneRepositoryRpc => (
        "sourceControl.cloneRepository",
        crate::source_control::SourceControlCloneRepositoryInput,
        crate::source_control::SourceControlCloneRepositoryResult,
        WsSourceControlLookupRepositoryError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::SourceControlCloneRepository.scope())
    ),
    WsSourceControlSignInStartRpc => (
        "sourceControl.signInStart",
        crate::source_control::SourceControlSignInInput,
        crate::source_control::SourceControlSignInPrompt,
        WsSourceControlSignInStartError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::SourceControlSignInStart.scope())
    ),
    WsSourceControlSignInWaitRpc => (
        "sourceControl.signInWait",
        crate::source_control::SourceControlSignInInput,
        crate::source_control::SourceControlSignInResult,
        WsSourceControlSignInStartError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::SourceControlSignInWait.scope())
    ),
    WsSourceControlSignInCancelRpc => (
        "sourceControl.signInCancel",
        crate::source_control::SourceControlSignInInput,
        (),
        WsSourceControlSignInStartError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::SourceControlSignInCancel.scope())
    ),
    WsSourceControlAppAccessRpc => (
        "sourceControl.appAccess",
        crate::source_control::SourceControlAppAccessInput,
        crate::source_control::SourceControlAppAccess,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsSourceControlStarredRpc => (
        "sourceControl.starred",
        crate::source_control::SourceControlStarredInput,
        bool,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsSourceControlStarRpc => (
        "sourceControl.star",
        crate::source_control::SourceControlStarredInput,
        bool,
        crate::auth::EnvironmentAuthorizationError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::SourceControlStar.scope())
    ),
    WsProjectCloneStartRpc => (
        "projectClone.start",
        crate::project_clone::ProjectCloneStartInput,
        crate::project_clone::ProjectCloneStartResult,
        WsProjectCloneStartError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ProjectCloneStart.scope())
    ),
    WsProjectCloneCancelRpc => (
        "projectClone.cancel",
        crate::project_clone::ProjectCloneActionInput,
        crate::project_clone::ProjectCloneActionResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ProjectCloneCancel.scope())
    ),
    WsProjectCloneRetryRpc => (
        "projectClone.retry",
        crate::project_clone::ProjectCloneActionInput,
        crate::project_clone::ProjectCloneActionResult,
        WsSourceControlLookupRepositoryError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ProjectCloneRetry.scope())
    ),
    WsSubscribeProjectClonesRpc => (
        "subscribeProjectClones",
        crate::project_clone::ProjectCloneSubscribeInput,
        crate::project_clone::ProjectCloneListEvent,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsSourceControlPublishRepositoryRpc => (
        "sourceControl.publishRepository",
        crate::source_control::SourceControlPublishRepositoryInput,
        crate::source_control::SourceControlPublishRepositoryResult,
        WsSourceControlLookupRepositoryError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::SourceControlPublishRepository.scope())
    ),
    WsProjectsSearchEntriesRpc => (
        "projects.searchEntries",
        crate::project::ProjectSearchEntriesInput,
        crate::project::ProjectSearchEntriesResult,
        WsProjectsSearchEntriesError,
        false,
        None
    ),
    WsProjectsSearchContentsRpc => (
        "projects.searchContents",
        crate::project::ProjectSearchContentsInput,
        crate::project::ProjectSearchContentsResult,
        WsProjectsSearchContentsError,
        false,
        None
    ),
    WsProjectsListEntriesRpc => (
        "projects.listEntries",
        crate::project::ProjectListEntriesInput,
        crate::project::ProjectListEntriesResult,
        WsProjectsListEntriesError,
        false,
        None
    ),
    WsProjectsListIgnoredFilesRpc => (
        "projects.listIgnoredFiles",
        crate::project::ProjectListIgnoredFilesInput,
        crate::project::ProjectListIgnoredFilesResult,
        WsProjectsListEntriesError,
        false,
        None
    ),
    WsProjectsReadFileRpc => (
        "projects.readFile",
        crate::project::ProjectReadFileInput,
        crate::project::ProjectReadFileResult,
        WsProjectsReadFileError,
        false,
        None
    ),
    WsProjectsWriteFileRpc => (
        "projects.writeFile",
        crate::project::ProjectWriteFileInput,
        crate::project::ProjectWriteFileResult,
        WsProjectsWriteFileError,
        false,
        None
    ),
    WsProjectsMutateRpc => (
        "projects.mutate",
        crate::project::ProjectMutation,
        crate::project::Project,
        WsProjectsMutateError,
        false,
        None
    ),
    WsProjectsEnsureScratchRpc => (
        "projects.ensureScratch",
        Empty,
        crate::project::ProjectEnsureScratchResult,
        WsProjectsEnsureScratchError,
        false,
        None
    ),
    WsProjectsCreateNewRpc => (
        "projects.createNew",
        crate::project::ProjectCreateNewInput,
        crate::project::ProjectCreateNewResult,
        WsProjectsEnsureScratchError,
        false,
        None
    ),
    WsProjectsCreateFromThreadRpc => (
        "projects.createFromThread",
        crate::project::ProjectCreateFromThreadInput,
        crate::project::ProjectCreateFromThreadResult,
        WsProjectsEnsureScratchError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ProjectsCreateFromThread.scope())
    ),
    WsShellOpenInEditorRpc => (
        "shell.openInEditor",
        crate::editor::LaunchEditorInput,
        (),
        WsShellOpenInEditorError,
        false,
        None
    ),
    WsFilesystemGetMetadataRpc => (
        "filesystem.getMetadata",
        crate::filesystem::FilesystemGetMetadataInput,
        crate::filesystem::FilesystemGetMetadataResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsFilesystemBrowseRpc => (
        "filesystem.browse",
        crate::filesystem::FilesystemBrowseInput,
        crate::filesystem::FilesystemBrowseResult,
        WsFilesystemBrowseError,
        false,
        None
    ),
    WsAgentSessionsScanRpc => (
        "agentSessions.scan",
        crate::agent_sessions::AgentSessionScanInput,
        crate::agent_sessions::AgentSessionScanResult,
        WsAgentSessionsScanError,
        false,
        None
    ),
    WsAgentSessionsImportRpc => (
        "agentSessions.import",
        crate::agent_sessions::AgentSessionImportInput,
        crate::agent_sessions::AgentSessionImportResult,
        WsAgentSessionsImportError,
        false,
        None
    ),
    WsAssetsCreateUrlRpc => (
        "assets.createUrl",
        crate::assets::AssetCreateUrlInput,
        crate::assets::AssetCreateUrlResult,
        WsAssetsCreateUrlError,
        false,
        None
    ),
    WsAssetsPersistChatAttachmentsRpc => (
        "assets.persistChatAttachments",
        crate::chat_attachment::PersistChatAttachmentsInput,
        crate::chat_attachment::PersistChatAttachmentsResult,
        WsAssetsPersistChatAttachmentsError,
        false,
        None
    ),
    WsAttachmentsCreateUploadUrlRpc => (
        "attachments.createUploadUrl",
        crate::assets::AttachmentCreateUploadUrlInput,
        crate::assets::AttachmentCreateUploadUrlResult,
        WsAttachmentsCreateUploadUrlError,
        false,
        None
    ),
    WsAttachmentsDeleteRpc => (
        "attachments.delete",
        crate::assets::AttachmentDeleteInput,
        (),
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsMcpAppsCallToolRpc => (
        "mcpApps.callTool",
        crate::mcp_apps::McpAppCallToolInput,
        crate::mcp_apps::McpAppCallToolResult,
        WsMcpAppsCallToolError,
        false,
        None
    ),
    WsMcpAppsToolInfoRpc => (
        "mcpApps.toolInfo",
        crate::mcp_apps::McpAppToolInfoInput,
        crate::mcp_apps::McpAppToolInfo,
        WsMcpAppsCallToolError,
        false,
        None
    ),
    WsMcpAppsUpdateModelContextRpc => (
        "mcpApps.updateModelContext",
        crate::mcp_apps::McpAppUpdateModelContextInput,
        (),
        WsMcpAppsCallToolError,
        false,
        None
    ),
    WsMcpAppsReadResourceRpc => (
        "mcpApps.readResource",
        crate::mcp_apps::McpAppReadResourceInput,
        crate::mcp_apps::McpAppReadResourceResult,
        WsMcpAppsCallToolError,
        false,
        None
    ),
    WsProviderUploadFeedbackRpc => (
        "provider.uploadFeedback",
        crate::provider::ProviderUploadFeedbackInput,
        crate::provider::ProviderUploadFeedbackResult,
        WsProviderUploadFeedbackError,
        false,
        None
    ),
    WsSubscribeVcsBranchRpc => (
        "subscribeVcsBranch",
        crate::git::VcsStatusInput,
        crate::git::VcsBranchStatus,
        WsSubscribeVcsBranchError,
        true,
        None
    ),
    WsVcsRefreshBranchRpc => (
        "vcs.refreshBranch",
        crate::git::VcsStatusInput,
        (),
        WsSubscribeVcsBranchError,
        false,
        None
    ),
    WsSubscribeVcsStatusRpc => (
        "subscribeVcsStatus",
        crate::git::VcsStatusSubscriptionInput,
        crate::git::VcsStatusStreamEvent,
        WsSubscribeVcsBranchError,
        true,
        None
    ),
    WsVcsPullRpc => (
        "vcs.pull",
        crate::git::VcsPullInput,
        crate::git::VcsPullResult,
        WsVcsPullError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::VcsPull.scope())
    ),
    WsVcsRefreshStatusRpc => (
        "vcs.refreshStatus",
        WsVcsRefreshStatusPayload,
        crate::git::VcsStatusResult,
        WsSubscribeVcsBranchError,
        false,
        None
    ),
    WsSubscribeWorktreeSetupRpc => (
        "subscribeWorktreeSetup",
        crate::worktree_setup::WorktreeSetupSubscribeInput,
        crate::worktree_setup::WorktreeSetupStreamEvent,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsWorktreeSetupCancelRpc => (
        "worktreeSetup.cancel",
        crate::worktree_setup::WorktreeSetupCancelInput,
        crate::worktree_setup::WorktreeSetupCancelResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsGitRunStackedActionRpc => (
        "git.runStackedAction",
        crate::git::GitRunStackedActionInput,
        crate::git::GitActionProgressEvent,
        WsSubscribeVcsBranchError,
        true,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::GitRunStackedAction.scope())
    ),
    WsGitResolvePullRequestRpc => (
        "git.resolvePullRequest",
        crate::git::GitPullRequestRefInput,
        crate::git::GitResolvePullRequestResult,
        WsSubscribeVcsBranchError,
        false,
        None
    ),
    WsGitPreparePullRequestThreadRpc => (
        "git.preparePullRequestThread",
        crate::git::GitPreparePullRequestThreadInput,
        crate::git::GitPreparePullRequestThreadResult,
        WsSubscribeVcsBranchError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::GitPreparePullRequestThread.scope())
    ),
    WsVcsListRefsRpc => (
        "vcs.listRefs",
        crate::git::VcsListRefsInput,
        crate::git::VcsListRefsResult,
        WsVcsPullError,
        false,
        None
    ),
    WsVcsCreateWorktreeRpc => (
        "vcs.createWorktree",
        WsVcsCreateWorktreePayload,
        crate::git::VcsCreateWorktreeResult,
        WsVcsPullError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::VcsCreateWorktree.scope())
    ),
    WsVcsRemoveWorktreeRpc => (
        "vcs.removeWorktree",
        crate::git::VcsRemoveWorktreeInput,
        (),
        WsVcsPullError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::VcsRemoveWorktree.scope())
    ),
    WsVcsCreateRefRpc => (
        "vcs.createRef",
        crate::git::VcsCreateRefInput,
        crate::git::VcsCreateRefResult,
        WsVcsPullError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::VcsCreateRef.scope())
    ),
    WsVcsSwitchRefRpc => (
        "vcs.switchRef",
        crate::git::VcsSwitchRefInput,
        crate::git::VcsSwitchRefResult,
        WsVcsPullError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::VcsSwitchRef.scope())
    ),
    WsVcsInitRpc => (
        "vcs.init",
        crate::git::VcsInitInput,
        (),
        WsVcsInitError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::VcsInit.scope())
    ),
    WsVcsTrustRepositoryRpc => (
        "vcs.trustRepository",
        crate::git::VcsTrustRepositoryInput,
        crate::git::VcsTrustRepositoryResult,
        WsVcsPullError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::VcsTrustRepository.scope())
    ),
    WsReviewGetBranchCommitsRpc => (
        "review.getBranchCommits",
        crate::review::ReviewBranchCommitsInput,
        crate::review::ReviewBranchCommitsResult,
        WsReviewGetBranchCommitsError,
        false,
        None
    ),
    WsReviewGetDiffPreviewRpc => (
        "review.getDiffPreview",
        crate::review::ReviewDiffPreviewInput,
        crate::review::ReviewDiffPreviewResult,
        WsReviewGetBranchCommitsError,
        false,
        None
    ),
    WsReviewGetDiffFileContentsRpc => (
        "review.getDiffFileContents",
        crate::review::ReviewDiffFileContentsInput,
        crate::review::ReviewDiffFileContentsResult,
        WsReviewGetBranchCommitsError,
        false,
        None
    ),
    WsTerminalOpenRpc => (
        "terminal.open",
        crate::terminal::TerminalOpenInput,
        crate::terminal::TerminalSessionSnapshot,
        WsTerminalOpenError,
        false,
        None
    ),
    WsTerminalAttachRpc => (
        "terminal.attach",
        crate::terminal::TerminalAttachInput,
        crate::terminal::TerminalAttachStreamEvent,
        WsTerminalOpenError,
        true,
        None
    ),
    WsTerminalObserveRpc => (
        "terminal.observe",
        crate::terminal::TerminalObserveInput,
        crate::terminal::TerminalAttachStreamEvent,
        WsTerminalOpenError,
        true,
        None
    ),
    WsTerminalWriteRpc => (
        "terminal.write",
        crate::terminal::TerminalWriteInput,
        (),
        WsTerminalOpenError,
        false,
        None
    ),
    WsTerminalResizeRpc => (
        "terminal.resize",
        crate::terminal::TerminalResizeInput,
        (),
        WsTerminalOpenError,
        false,
        None
    ),
    WsTerminalClearRpc => (
        "terminal.clear",
        crate::terminal::TerminalClearInput,
        (),
        WsTerminalOpenError,
        false,
        None
    ),
    WsTerminalCloseRpc => (
        "terminal.close",
        crate::terminal::TerminalCloseInput,
        (),
        WsTerminalOpenError,
        false,
        None
    ),
    WsPreviewOpenRpc => (
        "preview.open",
        crate::preview::PreviewOpenInput,
        crate::preview::PreviewSessionSnapshot,
        WsPreviewOpenError,
        false,
        None
    ),
    WsPreviewResizeRpc => (
        "preview.resize",
        crate::preview::PreviewResizeInput,
        crate::preview::PreviewSessionSnapshot,
        WsPreviewOpenError,
        false,
        None
    ),
    WsPreviewAdjustRpc => (
        "preview.adjust",
        crate::preview::PreviewAdjustInput,
        crate::preview::PreviewSessionSnapshot,
        WsPreviewOpenError,
        false,
        None
    ),
    WsPreviewCloseRpc => (
        "preview.close",
        crate::preview::PreviewCloseInput,
        (),
        WsPreviewOpenError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PreviewClose.scope())
    ),
    WsPreviewListRpc => (
        "preview.list",
        crate::preview::PreviewListInput,
        crate::preview::PreviewListResult,
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsPreviewClearProfileRpc => (
        "preview.clearProfile",
        crate::preview::PreviewClearProfileInput,
        (),
        WsPreviewClearProfileError,
        false,
        None
    ),
    WsPreviewReportProfilesRpc => (
        "preview.reportProfiles",
        crate::preview::PreviewReportProfilesInput,
        (),
        crate::auth::EnvironmentAuthorizationError,
        false,
        None
    ),
    WsPreviewReportStatusRpc => (
        "preview.reportStatus",
        crate::preview::PreviewReportStatusInput,
        (),
        WsPreviewOpenError,
        false,
        None
    ),
    WsSubscribePreviewEventsRpc => (
        "subscribePreviewEvents",
        Empty,
        crate::preview::PreviewEvent,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsPreviewStopLocalServerRpc => (
        "preview.stopLocalServer",
        crate::preview::PreviewStopLocalServerInput,
        (),
        WsPreviewStopLocalServerError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::PreviewStopLocalServer.scope())
    ),
    WsSubscribeDiscoveredLocalServersRpc => (
        "subscribeDiscoveredLocalServers",
        WsSubscribeDiscoveredLocalServersPayload,
        crate::preview::DiscoveredLocalServerList,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsDeviceTestHostRpc => (
        "device.testHost",
        crate::device::SshDeviceHostConfig,
        crate::device::DeviceHostSummary,
        WsDeviceTestHostError,
        false,
        None
    ),
    WsDeviceEasAccountRpc => (
        "device.easAccount",
        crate::device::DeviceEasAccountInput,
        crate::device::DeviceEasAccount,
        WsDeviceEasAccountError,
        false,
        None
    ),
    WsDeviceSimlockIdentityRpc => (
        "device.simlockIdentity",
        crate::device::DeviceSimlockIdentityInput,
        crate::device::DeviceSimlockIdentity,
        WsDeviceSimlockIdentityError,
        false,
        None
    ),
    WsDeviceListRpc => (
        "device.list",
        crate::device::DeviceListInput,
        crate::device::DeviceServiceState,
        WsDeviceTestHostError,
        false,
        None
    ),
    WsDeviceConfigureRpc => (
        "device.configure",
        crate::device::DeviceConfigureInput,
        crate::device::DeviceServiceState,
        WsDeviceTestHostError,
        false,
        None
    ),
    WsDeviceOpenRpc => (
        "device.open",
        crate::device::DeviceOpenInput,
        crate::device::DeviceSession,
        WsDeviceTestHostError,
        false,
        None
    ),
    WsDeviceCloseRpc => (
        "device.close",
        crate::device::DeviceCloseInput,
        (),
        WsDeviceTestHostError,
        false,
        None
    ),
    WsDeviceShutdownRpc => (
        "device.shutdown",
        crate::device::DeviceShutdownInput,
        (),
        WsDeviceTestHostError,
        false,
        None
    ),
    WsDeviceDetailRpc => (
        "device.detail",
        crate::device::DeviceDetailInput,
        crate::device::DeviceDetail,
        WsDeviceTestHostError,
        false,
        None
    ),
    WsDeviceActionRpc => (
        "device.action",
        crate::device::DeviceActionInput,
        crate::device::DeviceDetail,
        WsDeviceTestHostError,
        false,
        None
    ),
    WsSubscribeDeviceStateRpc => (
        "subscribeDeviceState",
        Empty,
        crate::device::DeviceServiceState,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsOrchestrationV2DispatchCommandRpc => (
        "orchestration.dispatchCommand",
        crate::orchestration_v2::OrchestrationV2Command,
        crate::orchestration_v2::OrchestrationV2DispatchCommandResult,
        WsOrchestrationV2DispatchCommandError,
        false,
        None
    ),
    WsOrchestrationV2GetTurnDiffRpc => (
        "orchestration.getTurnDiff",
        crate::checkpoint_diff::OrchestrationGetTurnDiffInput,
        crate::checkpoint_diff::OrchestrationGetFullThreadDiffResult,
        WsOrchestrationV2GetTurnDiffError,
        false,
        None
    ),
    WsOrchestrationV2GetFullThreadDiffRpc => (
        "orchestration.getFullThreadDiff",
        crate::checkpoint_diff::OrchestrationGetFullThreadDiffInput,
        crate::checkpoint_diff::OrchestrationGetFullThreadDiffResult,
        WsOrchestrationV2GetFullThreadDiffError,
        false,
        None
    ),
    WsOrchestrationV2SearchThreadRpc => (
        "orchestration.searchThread",
        crate::orchestration_v2::OrchestrationV2SearchThreadInput,
        crate::orchestration_v2::OrchestrationV2SearchThreadResult,
        WsOrchestrationV2SearchThreadError,
        false,
        None
    ),
    WsOrchestrationV2SearchThreadStreamRpc => (
        "orchestration.searchThreadStream",
        crate::orchestration_v2::OrchestrationV2SearchThreadInput,
        crate::orchestration_v2::OrchestrationV2SearchThreadResult,
        WsOrchestrationV2SearchThreadError,
        true,
        None
    ),
    WsOrchestrationV2SearchThreadsRpc => (
        "orchestration.searchThreads",
        crate::orchestration_v2::OrchestrationV2SearchThreadsInput,
        crate::orchestration_v2::OrchestrationV2SearchThreadsResult,
        WsOrchestrationV2SearchThreadsError,
        false,
        None
    ),
    WsOrchestrationV2GetArchivedShellSnapshotRpc => (
        "orchestration.getArchivedShellSnapshot",
        Empty,
        crate::orchestration_v2::OrchestrationV2ArchivedShellSnapshot,
        WsOrchestrationV2GetArchivedShellSnapshotError,
        false,
        None
    ),
    WsOrchestrationV2GetThreadFamilyRpc => (
        "orchestration.getThreadFamily",
        crate::orchestration_v2::OrchestrationV2GetThreadFamilyInput,
        crate::orchestration_v2::OrchestrationV2ThreadFamily,
        WsOrchestrationV2GetArchivedShellSnapshotError,
        false,
        None
    ),
    WsOrchestrationV2GetThreadTitlesRpc => (
        "orchestration.getThreadTitles",
        crate::orchestration_v2::OrchestrationV2GetThreadTitlesInput,
        crate::orchestration_v2::OrchestrationV2ThreadTitles,
        WsOrchestrationV2GetArchivedShellSnapshotError,
        false,
        None
    ),
    WsOrchestrationV2GetThreadLinkMetadataRpc => (
        "orchestration.getThreadLinkMetadata",
        crate::orchestration_v2::OrchestrationV2GetThreadTitlesInput,
        std::collections::BTreeMap<crate::base_schemas::ThreadId, crate::orchestration_v2::OrchestrationV2ThreadLinkMetadata>,
        WsOrchestrationV2GetArchivedShellSnapshotError,
        false,
        None
    ),
    WsOrchestrationV2GetWorktreeThreadIdsRpc => (
        "orchestration.getWorktreeThreadIds",
        crate::orchestration_v2::OrchestrationV2GetWorktreeThreadIdsInput,
        crate::orchestration_v2::OrchestrationV2WorktreeThreadIds,
        WsOrchestrationV2GetArchivedShellSnapshotError,
        false,
        None
    ),
    WsOrchestrationV2GetOlderSettledThreadsRpc => (
        "orchestration.getOlderSettledThreads",
        crate::orchestration_v2::OrchestrationV2GetOlderSettledThreadsInput,
        crate::orchestration_v2::OrchestrationV2OlderSettledThreadsPage,
        WsOrchestrationV2GetArchivedShellSnapshotError,
        false,
        None
    ),
    WsOrchestrationV2GetThreadProjectionRpc => (
        "orchestration.getThreadProjection",
        crate::orchestration_v2::OrchestrationV2GetThreadProjectionInput,
        crate::orchestration_v2::OrchestrationV2ThreadProjection,
        WsOrchestrationV2GetThreadProjectionError,
        false,
        None
    ),
    WsOrchestrationV2StopWorkflowRpc => (
        "orchestration.stopWorkflow",
        crate::orchestration_v2::OrchestrationV2StopWorkflowInput,
        (),
        WsOrchestrationV2StopWorkflowError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::StopWorkflow.scope())
    ),
    WsOrchestrationV2ForceStopThreadRpc => (
        "orchestration.forceStopThread",
        crate::orchestration_v2::OrchestrationV2ForceStopThreadInput,
        crate::orchestration_v2::OrchestrationV2ForceStopThreadResult,
        WsOrchestrationV2ForceStopThreadError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ForceStopThread.scope())
    ),
    WsOrchestrationV2GetWorkflowScriptRpc => (
        "orchestration.getWorkflowScript",
        crate::orchestration_v2::OrchestrationV2GetWorkflowScriptInput,
        crate::orchestration_v2::OrchestrationV2GetWorkflowScriptResult,
        WsOrchestrationV2GetWorkflowScriptError,
        false,
        None
    ),
    WsOrchestrationV2GetNativeChildWorkRpc => (
        "orchestration.getNativeChildWork",
        crate::orchestration_v2::OrchestrationV2GetNativeChildWorkInput,
        crate::orchestration_v2::OrchestrationV2GetNativeChildWorkResult,
        WsOrchestrationV2GetNativeChildWorkError,
        false,
        None
    ),
    WsOrchestrationV2GetTurnItemRpc => (
        "orchestration.getTurnItem",
        crate::orchestration_v2::OrchestrationV2GetTurnItemInput,
        crate::orchestration_v2::OrchestrationV2GetTurnItemResult,
        WsOrchestrationV2GetThreadProjectionError,
        false,
        None
    ),
    WsOrchestrationV2LaunchThreadRpc => (
        "orchestration.launchThread",
        crate::orchestration_v2::OrchestrationV2ThreadLaunchInput,
        crate::orchestration_v2::OrchestrationV2ThreadLaunchResult,
        WsOrchestrationV2LaunchThreadError,
        false,
        None
    ),
    WsOrchestrationV2SubscribeShellRpc => (
        "orchestration.subscribeShell",
        crate::orchestration_v2::OrchestrationV2SubscribeShellInput,
        crate::orchestration_v2::OrchestrationV2ShellStreamItem,
        WsOrchestrationV2GetArchivedShellSnapshotError,
        true,
        None
    ),
    WsOrchestrationV2SubscribeThreadRpc => (
        "orchestration.subscribeThread",
        crate::orchestration_v2::OrchestrationV2SubscribeThreadInput,
        crate::orchestration_v2::OrchestrationV2ThreadStreamItem,
        WsOrchestrationV2GetThreadProjectionError,
        true,
        None
    ),
    WsOrchestrationV2SubscribeCommandOutputRpc => (
        "orchestration.subscribeCommandOutput",
        crate::orchestration_v2::OrchestrationV2SubscribeCommandOutputInput,
        crate::orchestration_v2::OrchestrationV2CommandOutputFrame,
        WsOrchestrationV2SubscribeCommandOutputError,
        true,
        None
    ),
    WsOrchestrationV2SubscribePromptSuggestionRpc => (
        "orchestration.subscribePromptSuggestion",
        crate::orchestration_v2::OrchestrationV2SubscribePromptSuggestionInput,
        crate::orchestration_v2::OrchestrationV2PromptSuggestion,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsSubscribeTerminalMetadataRpc => (
        "subscribeTerminalMetadata",
        Empty,
        crate::terminal::TerminalMetadataStreamEvent,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsSubscribeServerConfigRpc => (
        "subscribeServerConfig",
        WsSubscribeServerConfigPayload,
        crate::server::ServerConfigStreamEvent,
        WsServerGetConfigError,
        true,
        None
    ),
    WsSubscribeServerLifecycleRpc => (
        "subscribeServerLifecycle",
        Empty,
        crate::server::ServerLifecycleStreamEvent,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
    WsScheduledTasksSubscribeRpc => (
        "scheduledTasks.subscribe",
        crate::scheduled_task::ScheduledTaskListInput,
        crate::scheduled_task::ScheduledTaskListResult,
        WsScheduledTasksSubscribeError,
        true,
        None
    ),
    WsScheduledTasksUpsertRpc => (
        "scheduledTasks.upsert",
        crate::scheduled_task::ScheduledTaskUpsertInput,
        crate::scheduled_task::ScheduledTaskMutationResult,
        WsScheduledTasksSubscribeError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ScheduledTasksUpsert.scope())
    ),
    WsScheduledTasksSetEnabledRpc => (
        "scheduledTasks.setEnabled",
        crate::scheduled_task::ScheduledTaskSetEnabledInput,
        crate::scheduled_task::ScheduledTaskMutationResult,
        WsScheduledTasksSubscribeError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ScheduledTasksSetEnabled.scope())
    ),
    WsScheduledTasksDeleteRpc => (
        "scheduledTasks.delete",
        crate::scheduled_task::ScheduledTaskDeleteInput,
        crate::scheduled_task::ScheduledTaskDeleteResult,
        WsScheduledTasksSubscribeError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ScheduledTasksDelete.scope())
    ),
    WsScheduledTasksRunNowRpc => (
        "scheduledTasks.runNow",
        crate::scheduled_task::ScheduledTaskRunNowInput,
        crate::scheduled_task::ScheduledTaskRunNowResult,
        WsScheduledTasksSubscribeError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ScheduledTasksRunNow.scope())
    ),
    WsScheduledTasksRotateWebhookTokenRpc => (
        "scheduledTasks.rotateWebhookToken",
        crate::scheduled_task::ScheduledTaskRotateWebhookTokenInput,
        crate::scheduled_task::ScheduledTaskMutationResult,
        WsScheduledTasksSubscribeError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::ScheduledTasksRotateWebhookToken.scope())
    ),
    WsGuestShareCreateRpc => (
        "guestShare.create",
        crate::guest_share::GuestShareCreateInput,
        crate::guest_share::GuestShareCreateResult,
        WsGuestShareCreateError,
        false,
        Some(crate::client_rpc_permissions::ClientGuardedRpcTag::GuestShareCreate.scope())
    ),
    WsSecretsAnswerRequestRpc => (
        "secrets.answerRequest",
        crate::secret_request::SecretRequestAnswerInput,
        (),
        WsSecretsAnswerRequestError,
        false,
        None
    ),
    WsScheduledTasksListWebhookDeliveriesRpc => (
        "scheduledTasks.listWebhookDeliveries",
        crate::scheduled_task::ScheduledTaskListWebhookDeliveriesInput,
        crate::scheduled_task::ScheduledTaskListWebhookDeliveriesResult,
        WsScheduledTasksSubscribeError,
        false,
        None
    ),
    WsScheduledTasksGetWebhookDeliveryRpc => (
        "scheduledTasks.getWebhookDelivery",
        crate::scheduled_task::ScheduledTaskGetWebhookDeliveryInput,
        crate::scheduled_task::ScheduledTaskGetWebhookDeliveryResult,
        WsScheduledTasksSubscribeError,
        false,
        None
    ),
    WsSubscribeAuthAccessRpc => (
        "subscribeAuthAccess",
        Empty,
        crate::auth::AuthAccessStreamEvent,
        WsSubscribeAuthAccessError,
        true,
        None
    ),
    WsSubscribeResourceTelemetryRpc => (
        "subscribeResourceTelemetry",
        Empty,
        crate::resource_telemetry::ResourceTelemetrySnapshot,
        crate::auth::EnvironmentAuthorizationError,
        true,
        None
    ),
}
