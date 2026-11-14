use super::super::{Registry, register};
use t3_contracts::orchestration_v2::*;
pub fn register_module(registry: &mut Registry) {
    register::<OrchestrationV2ThreadVisitJson>(registry, "OrchestrationV2ThreadVisitJson");
    register::<OrchestrationV2ThreadVisit>(registry, "OrchestrationV2ThreadVisit");
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.visit-recorded",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.visit-recorded",
    );
    register::<OrchestrationGetWorkflowScriptError>(
        registry,
        "OrchestrationGetWorkflowScriptError",
    );
    register::<OrchestrationV2Actor>(registry, "OrchestrationV2Actor");
    register::<OrchestrationV2AppThread>(registry, "OrchestrationV2AppThread");
    register::<OrchestrationV2AppThreadJson>(registry, "OrchestrationV2AppThreadJson");
    register::<OrchestrationV2AppThreadLineage>(registry, "OrchestrationV2AppThreadLineage");
    register::<OrchestrationV2ApprovalCapabilities>(
        registry,
        "OrchestrationV2ApprovalCapabilities",
    );
    register::<OrchestrationV2ArchivedShellSnapshot>(
        registry,
        "OrchestrationV2ArchivedShellSnapshot",
    );
    register::<OrchestrationV2Checkpoint>(registry, "OrchestrationV2Checkpoint");
    register::<OrchestrationV2CheckpointCapabilities>(
        registry,
        "OrchestrationV2CheckpointCapabilities",
    );
    register::<OrchestrationV2CheckpointFileSummary>(
        registry,
        "OrchestrationV2CheckpointFileSummary",
    );
    register::<OrchestrationV2CheckpointFileTotals>(
        registry,
        "OrchestrationV2CheckpointFileTotals",
    );
    register::<OrchestrationV2CheckpointJson>(registry, "OrchestrationV2CheckpointJson");
    register::<OrchestrationV2CheckpointRollbackRequest>(
        registry,
        "OrchestrationV2CheckpointRollbackRequest",
    );
    register::<OrchestrationV2CheckpointRollbackRequestJson>(
        registry,
        "OrchestrationV2CheckpointRollbackRequestJson",
    );
    register::<OrchestrationV2CheckpointScope>(registry, "OrchestrationV2CheckpointScope");
    register::<OrchestrationV2CheckpointScopeJson>(registry, "OrchestrationV2CheckpointScopeJson");
    register::<OrchestrationV2CheckpointUnavailableError>(
        registry,
        "OrchestrationV2CheckpointUnavailableError",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command");
    register::<OrchestrationV2CommandOutputError>(registry, "OrchestrationV2CommandOutputError");
    register::<OrchestrationV2CommandOutputFrame>(registry, "OrchestrationV2CommandOutputFrame");
    register::<OrchestrationV2ContextCapabilities>(registry, "OrchestrationV2ContextCapabilities");
    register::<OrchestrationV2ContextHandoff>(registry, "OrchestrationV2ContextHandoff");
    register::<OrchestrationV2ContextHandoffJson>(registry, "OrchestrationV2ContextHandoffJson");
    register::<OrchestrationV2ContextSourcePoint>(registry, "OrchestrationV2ContextSourcePoint");
    register::<OrchestrationV2ContextTransfer>(registry, "OrchestrationV2ContextTransfer");
    register::<OrchestrationV2ContextTransferJson>(registry, "OrchestrationV2ContextTransferJson");
    register::<OrchestrationV2ContextTransferResolution>(
        registry,
        "OrchestrationV2ContextTransferResolution",
    );
    register::<OrchestrationV2ContextTransferType>(registry, "OrchestrationV2ContextTransferType");
    register::<OrchestrationV2ConversationMessage>(registry, "OrchestrationV2ConversationMessage");
    register::<OrchestrationV2ConversationMessageJson>(
        registry,
        "OrchestrationV2ConversationMessageJson",
    );
    register::<OrchestrationV2CreationSource>(registry, "OrchestrationV2CreationSource");
    register::<OrchestrationV2DelegatedCompletionCohort>(
        registry,
        "OrchestrationV2DelegatedCompletionCohort",
    );
    register::<OrchestrationV2DelegatedCompletionDelivery>(
        registry,
        "OrchestrationV2DelegatedCompletionDelivery",
    );
    register::<OrchestrationV2DelegatedCompletionTaskDelivery>(
        registry,
        "OrchestrationV2DelegatedCompletionTaskDelivery",
    );
    register::<OrchestrationV2DelegatedCompletionTaskDeliveryState>(
        registry,
        "OrchestrationV2DelegatedCompletionTaskDeliveryState",
    );
    register::<OrchestrationV2DelegatedFrom>(registry, "OrchestrationV2DelegatedFrom");
    register::<OrchestrationV2DispatchCommandError>(
        registry,
        "OrchestrationV2DispatchCommandError",
    );
    register::<OrchestrationV2DispatchCommandResult>(
        registry,
        "OrchestrationV2DispatchCommandResult",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent");
    register::<OrchestrationV2DomainEventJson>(registry, "OrchestrationV2DomainEventJson");
    register::<OrchestrationV2ExecutionNode>(registry, "OrchestrationV2ExecutionNode");
    register::<OrchestrationV2ExecutionNodeJson>(registry, "OrchestrationV2ExecutionNodeJson");
    register::<OrchestrationV2FileChangeDetail>(registry, "OrchestrationV2FileChangeDetail");
    register::<OrchestrationV2FileSearchResult>(registry, "OrchestrationV2FileSearchResult");
    register::<OrchestrationV2ForceStopThreadError>(
        registry,
        "OrchestrationV2ForceStopThreadError",
    );
    register::<OrchestrationV2ForceStopThreadInput>(
        registry,
        "OrchestrationV2ForceStopThreadInput",
    );
    register::<OrchestrationV2ForceStopThreadResult>(
        registry,
        "OrchestrationV2ForceStopThreadResult",
    );
    register::<OrchestrationV2GetNativeChildWorkError>(
        registry,
        "OrchestrationV2GetNativeChildWorkError",
    );
    register::<OrchestrationV2GetNativeChildWorkInput>(
        registry,
        "OrchestrationV2GetNativeChildWorkInput",
    );
    register::<OrchestrationV2GetNativeChildWorkResult>(
        registry,
        "OrchestrationV2GetNativeChildWorkResult",
    );
    register::<OrchestrationV2GetOlderSettledThreadsInput>(
        registry,
        "OrchestrationV2GetOlderSettledThreadsInput",
    );
    register::<OrchestrationV2GetShellSnapshotError>(
        registry,
        "OrchestrationV2GetShellSnapshotError",
    );
    register::<OrchestrationV2GetThreadFamilyInput>(
        registry,
        "OrchestrationV2GetThreadFamilyInput",
    );
    register::<OrchestrationV2GetThreadProjectionError>(
        registry,
        "OrchestrationV2GetThreadProjectionError",
    );
    register::<OrchestrationV2GetThreadProjectionInput>(
        registry,
        "OrchestrationV2GetThreadProjectionInput",
    );
    register::<OrchestrationV2GetThreadTitlesInput>(
        registry,
        "OrchestrationV2GetThreadTitlesInput",
    );
    register::<OrchestrationV2GetTurnItemInput>(registry, "OrchestrationV2GetTurnItemInput");
    register::<OrchestrationV2GetTurnItemResult>(registry, "OrchestrationV2GetTurnItemResult");
    register::<OrchestrationV2GetWorkflowScriptInput>(
        registry,
        "OrchestrationV2GetWorkflowScriptInput",
    );
    register::<OrchestrationV2GetWorkflowScriptResult>(
        registry,
        "OrchestrationV2GetWorkflowScriptResult",
    );
    register::<OrchestrationV2GetWorktreeThreadIdsInput>(
        registry,
        "OrchestrationV2GetWorktreeThreadIdsInput",
    );
    register::<OrchestrationV2HistoricalMessage>(registry, "OrchestrationV2HistoricalMessage");
    register::<OrchestrationV2IdentityCapabilities>(
        registry,
        "OrchestrationV2IdentityCapabilities",
    );
    register::<OrchestrationV2IdleCompaction>(registry, "OrchestrationV2IdleCompaction");
    register::<OrchestrationV2LatestVisibleMessageSummary>(
        registry,
        "OrchestrationV2LatestVisibleMessageSummary",
    );
    register::<OrchestrationV2LatestVisibleMessageSummaryJson>(
        registry,
        "OrchestrationV2LatestVisibleMessageSummaryJson",
    );
    register::<OrchestrationV2LimitRecovery>(registry, "OrchestrationV2LimitRecovery");
    register::<OrchestrationV2LimitRecoveryUpdate>(registry, "OrchestrationV2LimitRecoveryUpdate");
    register::<OrchestrationV2LinkOrigin>(registry, "OrchestrationV2LinkOrigin");
    register::<OrchestrationV2LinkRequestStatus>(registry, "OrchestrationV2LinkRequestStatus");
    register::<OrchestrationV2LinkedSender>(registry, "OrchestrationV2LinkedSender");
    register::<OrchestrationV2ModelSwitch>(registry, "OrchestrationV2ModelSwitch");
    register::<OrchestrationV2NativeChildStep>(registry, "OrchestrationV2NativeChildStep");
    register::<OrchestrationV2NativeChildWork>(registry, "OrchestrationV2NativeChildWork");
    register::<OrchestrationV2NativeRefStrength>(registry, "OrchestrationV2NativeRefStrength");
    register::<OrchestrationV2Notification>(registry, "OrchestrationV2Notification");
    register::<OrchestrationV2NotificationSource>(registry, "OrchestrationV2NotificationSource");
    register::<OrchestrationV2OlderSettledCount>(registry, "OrchestrationV2OlderSettledCount");
    register::<OrchestrationV2OlderSettledCursor>(registry, "OrchestrationV2OlderSettledCursor");
    register::<OrchestrationV2OlderSettledThreads>(registry, "OrchestrationV2OlderSettledThreads");
    register::<OrchestrationV2OlderSettledThreadsPage>(
        registry,
        "OrchestrationV2OlderSettledThreadsPage",
    );
    register::<OrchestrationV2PendingBackgroundTask>(
        registry,
        "OrchestrationV2PendingBackgroundTask",
    );
    register::<OrchestrationV2PendingRuntimeRequestSummary>(
        registry,
        "OrchestrationV2PendingRuntimeRequestSummary",
    );
    register::<OrchestrationV2PendingRuntimeRequestSummaryJson>(
        registry,
        "OrchestrationV2PendingRuntimeRequestSummaryJson",
    );
    register::<OrchestrationV2PlanArtifact>(registry, "OrchestrationV2PlanArtifact");
    register::<OrchestrationV2PlanStep>(registry, "OrchestrationV2PlanStep");
    register::<OrchestrationV2PlanningCapabilities>(
        registry,
        "OrchestrationV2PlanningCapabilities",
    );
    register::<OrchestrationV2ProjectedTurnItem>(registry, "OrchestrationV2ProjectedTurnItem");
    register::<OrchestrationV2ProjectedTurnItemJson>(
        registry,
        "OrchestrationV2ProjectedTurnItemJson",
    );
    register::<OrchestrationV2PromptSuggestion>(registry, "OrchestrationV2PromptSuggestion");
    register::<OrchestrationV2ProviderCapabilities>(
        registry,
        "OrchestrationV2ProviderCapabilities",
    );
    register::<OrchestrationV2ProviderFailure>(registry, "OrchestrationV2ProviderFailure");
    register::<OrchestrationV2ProviderFailureClass>(
        registry,
        "OrchestrationV2ProviderFailureClass",
    );
    register::<OrchestrationV2ProviderGoal>(registry, "OrchestrationV2ProviderGoal");
    register::<OrchestrationV2ProviderGoalStatus>(registry, "OrchestrationV2ProviderGoalStatus");
    register::<OrchestrationV2ProviderRef>(registry, "OrchestrationV2ProviderRef");
    register::<OrchestrationV2ProviderRetry>(registry, "OrchestrationV2ProviderRetry");
    register::<OrchestrationV2ProviderSession>(registry, "OrchestrationV2ProviderSession");
    register::<OrchestrationV2ProviderSessionDetached>(
        registry,
        "OrchestrationV2ProviderSessionDetached",
    );
    register::<OrchestrationV2ProviderSessionDetachedJson>(
        registry,
        "OrchestrationV2ProviderSessionDetachedJson",
    );
    register::<OrchestrationV2ProviderSessionJson>(registry, "OrchestrationV2ProviderSessionJson");
    register::<OrchestrationV2ProviderThread>(registry, "OrchestrationV2ProviderThread");
    register::<OrchestrationV2ProviderThreadJson>(registry, "OrchestrationV2ProviderThreadJson");
    register::<OrchestrationV2ProviderThreadNativeMetadata>(
        registry,
        "OrchestrationV2ProviderThreadNativeMetadata",
    );
    register::<OrchestrationV2ProviderTurn>(registry, "OrchestrationV2ProviderTurn");
    register::<OrchestrationV2ProviderTurnJson>(registry, "OrchestrationV2ProviderTurnJson");
    register::<OrchestrationV2ProviderTurnTokenUsage>(
        registry,
        "OrchestrationV2ProviderTurnTokenUsage",
    );
    register::<OrchestrationV2RemoteTaskChild>(registry, "OrchestrationV2RemoteTaskChild");
    register::<OrchestrationV2RestartCancelledBackgroundWork>(
        registry,
        "OrchestrationV2RestartCancelledBackgroundWork",
    );
    register::<OrchestrationV2Run>(registry, "OrchestrationV2Run");
    register::<OrchestrationV2RunAttempt>(registry, "OrchestrationV2RunAttempt");
    register::<OrchestrationV2RunAttemptJson>(registry, "OrchestrationV2RunAttemptJson");
    register::<OrchestrationV2RunBackgroundWorkCancelled>(
        registry,
        "OrchestrationV2RunBackgroundWorkCancelled",
    );
    register::<OrchestrationV2RunJson>(registry, "OrchestrationV2RunJson");
    register::<OrchestrationV2RunStatus>(registry, "OrchestrationV2RunStatus");
    register::<OrchestrationV2RuntimePolicyCapabilities>(
        registry,
        "OrchestrationV2RuntimePolicyCapabilities",
    );
    register::<OrchestrationV2RuntimeRequest>(registry, "OrchestrationV2RuntimeRequest");
    register::<OrchestrationV2RuntimeRequestJson>(registry, "OrchestrationV2RuntimeRequestJson");
    register::<OrchestrationV2SearchThreadError>(registry, "OrchestrationV2SearchThreadError");
    register::<OrchestrationV2SearchThreadInput>(registry, "OrchestrationV2SearchThreadInput");
    register::<OrchestrationV2SearchThreadResult>(registry, "OrchestrationV2SearchThreadResult");
    register::<OrchestrationV2SearchThreadsInput>(registry, "OrchestrationV2SearchThreadsInput");
    register::<OrchestrationV2SearchThreadsResult>(registry, "OrchestrationV2SearchThreadsResult");
    register::<OrchestrationV2SecretRequestDelivery>(
        registry,
        "OrchestrationV2SecretRequestDelivery",
    );
    register::<OrchestrationV2SecretRequestStatus>(registry, "OrchestrationV2SecretRequestStatus");
    register::<OrchestrationV2SessionCapabilities>(registry, "OrchestrationV2SessionCapabilities");
    register::<OrchestrationV2ShellHttpSnapshot>(registry, "OrchestrationV2ShellHttpSnapshot");
    register::<OrchestrationV2ShellResumable>(registry, "OrchestrationV2ShellResumable");
    register::<OrchestrationV2ShellSnapshot>(registry, "OrchestrationV2ShellSnapshot");
    register::<OrchestrationV2ShellSnapshotJson>(registry, "OrchestrationV2ShellSnapshotJson");
    register::<OrchestrationV2ShellStreamItem>(registry, "OrchestrationV2ShellStreamItem");
    register::<OrchestrationV2ShellThreadStatus>(registry, "OrchestrationV2ShellThreadStatus");
    register::<OrchestrationV2StopWorkflowError>(registry, "OrchestrationV2StopWorkflowError");
    register::<OrchestrationV2StopWorkflowInput>(registry, "OrchestrationV2StopWorkflowInput");
    register::<OrchestrationV2StoredEvent>(registry, "OrchestrationV2StoredEvent");
    register::<OrchestrationV2StreamingCapabilities>(
        registry,
        "OrchestrationV2StreamingCapabilities",
    );
    register::<OrchestrationV2Subagent>(registry, "OrchestrationV2Subagent");
    register::<OrchestrationV2SubagentCapabilities>(
        registry,
        "OrchestrationV2SubagentCapabilities",
    );
    register::<OrchestrationV2SubagentJson>(registry, "OrchestrationV2SubagentJson");
    register::<OrchestrationV2SubagentWorkflow>(registry, "OrchestrationV2SubagentWorkflow");
    register::<OrchestrationV2SubscribeCommandOutputInput>(
        registry,
        "OrchestrationV2SubscribeCommandOutputInput",
    );
    register::<OrchestrationV2SubscribePromptSuggestionInput>(
        registry,
        "OrchestrationV2SubscribePromptSuggestionInput",
    );
    register::<OrchestrationV2SubscribeShellInput>(registry, "OrchestrationV2SubscribeShellInput");
    register::<OrchestrationV2SubscribeThreadInput>(
        registry,
        "OrchestrationV2SubscribeThreadInput",
    );
    register::<OrchestrationV2ThreadBoundedSnapshot>(
        registry,
        "OrchestrationV2ThreadBoundedSnapshot",
    );
    register::<OrchestrationV2ThreadCapabilities>(registry, "OrchestrationV2ThreadCapabilities");
    register::<OrchestrationV2ThreadDetailSnapshot>(
        registry,
        "OrchestrationV2ThreadDetailSnapshot",
    );
    register::<OrchestrationV2ThreadFamily>(registry, "OrchestrationV2ThreadFamily");
    register::<OrchestrationV2ThreadFindMatch>(registry, "OrchestrationV2ThreadFindMatch");
    register::<OrchestrationV2ThreadForkSourcePoint>(
        registry,
        "OrchestrationV2ThreadForkSourcePoint",
    );
    register::<OrchestrationV2ThreadHistoryOrigin>(registry, "OrchestrationV2ThreadHistoryOrigin");
    register::<OrchestrationV2ThreadHistoryPage>(registry, "OrchestrationV2ThreadHistoryPage");
    register::<OrchestrationV2ThreadLaunchError>(registry, "OrchestrationV2ThreadLaunchError");
    register::<OrchestrationV2ThreadLaunchInput>(registry, "OrchestrationV2ThreadLaunchInput");
    register::<OrchestrationV2ThreadLaunchResult>(registry, "OrchestrationV2ThreadLaunchResult");
    register::<OrchestrationV2ThreadLaunchWorkspaceStrategy>(
        registry,
        "OrchestrationV2ThreadLaunchWorkspaceStrategy",
    );
    register::<OrchestrationV2ThreadLinkMetadata>(registry, "OrchestrationV2ThreadLinkMetadata");
    register::<OrchestrationV2ThreadOpens>(registry, "OrchestrationV2ThreadOpens");
    register::<OrchestrationV2ThreadProjection>(registry, "OrchestrationV2ThreadProjection");
    register::<OrchestrationV2ThreadProjectionJson>(
        registry,
        "OrchestrationV2ThreadProjectionJson",
    );
    register::<OrchestrationV2ThreadShell>(registry, "OrchestrationV2ThreadShell");
    register::<OrchestrationV2ThreadShellJson>(registry, "OrchestrationV2ThreadShellJson");
    register::<OrchestrationV2ThreadShellSnapshot>(registry, "OrchestrationV2ThreadShellSnapshot");
    register::<OrchestrationV2ThreadStreamItem>(registry, "OrchestrationV2ThreadStreamItem");
    register::<OrchestrationV2ThreadTitles>(registry, "OrchestrationV2ThreadTitles");
    register::<OrchestrationV2ThreadTranscript>(registry, "OrchestrationV2ThreadTranscript");
    register::<OrchestrationV2ToolCapabilities>(registry, "OrchestrationV2ToolCapabilities");
    register::<OrchestrationV2TurnCapabilities>(registry, "OrchestrationV2TurnCapabilities");
    register::<OrchestrationV2TurnItem>(registry, "OrchestrationV2TurnItem");
    register::<OrchestrationV2TurnItemJson>(registry, "OrchestrationV2TurnItemJson");
    register::<OrchestrationV2TurnItemStatus>(registry, "OrchestrationV2TurnItemStatus");
    register::<OrchestrationV2UserInputQuestion>(registry, "OrchestrationV2UserInputQuestion");
    register::<OrchestrationV2UserMessageInputIntent>(
        registry,
        "OrchestrationV2UserMessageInputIntent",
    );
    register::<OrchestrationV2WebSearchResult>(registry, "OrchestrationV2WebSearchResult");
    register::<OrchestrationV2WorkflowAgent>(registry, "OrchestrationV2WorkflowAgent");
    register::<OrchestrationV2WorkflowPhase>(registry, "OrchestrationV2WorkflowPhase");
    register::<OrchestrationV2WorkflowRunHandles>(registry, "OrchestrationV2WorkflowRunHandles");
    register::<OrchestrationV2WorktreeThreadIds>(registry, "OrchestrationV2WorktreeThreadIds");
    register::<OrchestrationWorkflowFileError>(registry, "OrchestrationWorkflowFileError");
    register::<ProviderReplayEntry>(registry, "ProviderReplayEntry");
    register::<ProviderReplayNdjsonRecord>(registry, "ProviderReplayNdjsonRecord");
    register::<ProviderReplayTranscript>(registry, "ProviderReplayTranscript");
    register::<ProviderReplayTranscriptHeader>(registry, "ProviderReplayTranscriptHeader");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.checkpoint.rollback");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.delegated_task.completion-delivery.acknowledge",
    );
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.delegated_task.completion-delivery.dispose",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.delegated_task.request");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.delegated_task.wake-policy",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.message.dispatch");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.notification.delivery.accept",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.prepared-run.fail");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.prepared-run.progress");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.prepared-run.release");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.prepared-run.retry");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.provider-session.detach");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.provider.switch");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.queue.resume");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.queued-message.promote-to-steer",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.queued-run.cancel");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.queued-run.edit");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.queued-run.reorder");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.queued-run.start");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.run.interrupt");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.runtime-request.respond");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.subagent.stop");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.active.reorder");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.archive");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.auto-settle");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.auto-settle.set");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.create");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.created.record");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.delete");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.fork");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.guest-message.dismiss",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.guest-share.close");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.interaction-mode.set",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.mark-unread");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.merge_back");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.metadata.update");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.model-selection.set",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.pin");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.pin.reorder");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.pull-request-link.sync",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.pull-request.link");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.pull-request.sync");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.pull-request.unlink",
    );
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.pull-request.watch",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.runtime-mode.set");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.settle");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.snooze");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.stop");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.tag.set");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.title.regeneration.complete",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.unarchive");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.unpin");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.unsettle");
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.unsnooze");
    register::<OrchestrationV2Command>(
        registry,
        "OrchestrationV2Command.thread.user-input.dismiss",
    );
    register::<OrchestrationV2Command>(registry, "OrchestrationV2Command.thread.visit");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.checkpoint-scope.created",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.checkpoint.captured",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.checkpoint.rollback-requested",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.context-handoff.updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.context-transfer.created",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.context-transfer.updated",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.message.updated");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.node.updated");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.plan.updated");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.provider-session.attached",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.provider-session.detached",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.provider-session.updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.provider-thread.updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.provider-turn.updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.run-attempt.created",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.run-attempt.updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.run.background-work-cancelled",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.run.created");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.run.updated");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.runtime-request.updated",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.subagent.updated");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.active-reordered",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.archived");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.auto-settle-set",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.created");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.deleted");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.guest-share-updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.interaction-mode-updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.marked-unread",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.metadata-updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.model-selection-updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.pin-reordered",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.pinned");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.provider-switched",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.pull-request-link-updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.pull-request-synced",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.runtime-mode-updated",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.settled");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.snoozed");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.tags-updated",
    );
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.thread.unarchived",
    );
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.unpinned");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.unsettled");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.unsnoozed");
    register::<OrchestrationV2DomainEvent>(registry, "OrchestrationV2DomainEvent.thread.visited");
    register::<OrchestrationV2DomainEvent>(
        registry,
        "OrchestrationV2DomainEvent.turn-item.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.checkpoint-scope.created",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.checkpoint.captured",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.checkpoint.rollback-requested",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.context-handoff.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.context-transfer.created",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.context-transfer.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.message.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.node.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.plan.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.provider-session.attached",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.provider-session.detached",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.provider-session.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.provider-thread.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.provider-turn.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.run-attempt.created",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.run-attempt.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.run.background-work-cancelled",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.run.created",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.run.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.runtime-request.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.subagent.updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.active-reordered",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.archived",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.auto-settle-set",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.created",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.deleted",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.guest-share-updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.interaction-mode-updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.marked-unread",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.metadata-updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.model-selection-updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.pin-reordered",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.pinned",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.provider-switched",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.pull-request-link-updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.pull-request-synced",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.runtime-mode-updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.settled",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.snoozed",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.tags-updated",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.unarchived",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.unpinned",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.unsettled",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.unsnoozed",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.thread.visited",
    );
    register::<OrchestrationV2DomainEventJson>(
        registry,
        "OrchestrationV2DomainEventJson.turn-item.updated",
    );
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("orchestration_v2", register_module)
}

#[test]
fn command_and_event_variants_cover_the_manifest() -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::BTreeSet;
    use t3_contracts::wire::TaggedUnion;
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/orchestration.json"))?;
    for (name, tags) in [
        ("OrchestrationV2Command", OrchestrationV2Command::KNOWN_TAGS),
        (
            "OrchestrationV2DomainEvent",
            OrchestrationV2DomainEvent::KNOWN_TAGS,
        ),
        (
            "OrchestrationV2DomainEventJson",
            OrchestrationV2DomainEventJson::KNOWN_TAGS,
        ),
    ] {
        let entries = manifest[name]
            .as_array()
            .ok_or("missing variant manifest")?;
        let expected = entries
            .iter()
            .map(|entry| entry["tag"].as_str().ok_or("missing variant tag"))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let actual = tags.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(actual.len(), tags.len(), "duplicate Rust tags for {name}");
        assert_eq!(actual, expected, "Rust variants differ from {name}");
        let mut decoded_tags = BTreeSet::new();
        for raw in fixture_samples(name)? {
            let wire = match name {
                "OrchestrationV2Command" => {
                    serde_json::to_value(serde_json::from_value::<OrchestrationV2Command>(raw)?)?
                }
                "OrchestrationV2DomainEvent" => serde_json::to_value(serde_json::from_value::<
                    OrchestrationV2DomainEvent,
                >(raw)?)?,
                _ => serde_json::to_value(
                    serde_json::from_value::<OrchestrationV2DomainEventJson>(raw)?,
                )?,
            };
            decoded_tags.insert(
                wire["type"]
                    .as_str()
                    .ok_or("missing encoded tag")?
                    .to_owned(),
            );
        }
        assert_eq!(
            decoded_tags
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            expected,
            "no valid parent sample for a variant in {name}"
        );
    }
    Ok(())
}

fn fixture_samples(name: &str) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        super::super::fixtures().join("manifest.json"),
    )?)?;
    let file = manifest["samples"]
        .as_array()
        .ok_or("missing samples")?
        .iter()
        .find(|entry| entry["schema"] == name)
        .and_then(|entry| entry["file"].as_str())
        .ok_or("missing fixture")?;
    std::fs::read_to_string(super::super::fixtures().join(file))?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

#[test]
fn notification_and_background_kinds_follow_legacy_and_future_rules()
-> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    for (raw, expected) in [
        (
            json!({"kind":"command"}),
            json!({"kind":"background_command"}),
        ),
        (
            json!({"kind":"subagent", "childThreadId":null}),
            json!({"kind":"background_task", "work":"subagent", "childThreadId":null}),
        ),
        (
            json!({"kind":"future", "other":"ignored"}),
            json!({"kind":"background_task"}),
        ),
        (json!({}), json!({"kind":"background_task"})),
    ] {
        let decoded: OrchestrationV2NotificationSource = serde_json::from_value(raw)?;
        assert_eq!(serde_json::to_value(decoded)?, expected);
    }
    let future = json!({"kind":"future", "taskId":"task:1", "description":"waiting", "parentTaskId":null, "command":"discard", "itemId":"discard", "awaited":true});
    let task: OrchestrationV2PendingBackgroundTask = serde_json::from_value(future)?;
    assert_eq!(
        serde_json::to_value(task)?,
        json!({"kind":"background_task", "taskId":"task:1", "description":"waiting", "parentTaskId":null})
    );
    for raw in [
        json!({"kind":"subagent"}),
        json!({"kind":"command", "taskId":"t", "awaited":false}),
        json!({"kind":4, "taskId":"t"}),
    ] {
        assert!(serde_json::from_value::<OrchestrationV2PendingBackgroundTask>(raw).is_err());
    }
    assert!(
        serde_json::from_value::<OrchestrationV2NotificationSource>(
            json!({"kind":"delegated_task"})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn future_stream_events_are_decode_only_and_known_events_stay_strict()
-> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    for event in [
        json!({"type":"future", "payload":{"anything":true}}),
        json!({"type":"turn-item.updated", "payload":{"type":"future"}}),
    ] {
        let item: OrchestrationV2ThreadStreamItem = serde_json::from_value(
            json!({"kind":"event", "sequence":9007199254740991_u64, "event":event}),
        )?;
        assert!(matches!(
            &item,
            OrchestrationV2ThreadStreamItem::UnknownEvent {
                sequence: 9007199254740991,
                ..
            }
        ));
        assert!(serde_json::to_value(item).is_err());
    }
    for event in [
        json!({"type":"run.created"}),
        json!({"type":"turn-item.updated", "payload":{"type":"reasoning"}}),
        json!({"type":4}),
    ] {
        assert!(
            serde_json::from_value::<OrchestrationV2ThreadStreamItem>(
                json!({"kind":"event", "sequence":1, "event":event})
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn projection_arrays_drop_future_items_and_reject_malformed_known_items()
-> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    let mut raw = fixture_samples("OrchestrationV2ThreadProjection")?
        .into_iter()
        .next()
        .ok_or("empty fixtures")?;
    raw["turnItems"] = json!([{"type":"future"}]);
    raw["visibleTurnItems"] = json!([{"position":0, "visibility":"local", "sourceThreadId":"t", "sourceItemId":"i", "item":{"type":"future"}}]);
    let projection: OrchestrationV2ThreadProjection = serde_json::from_value(raw.clone())?;
    assert!(projection.turn_items.is_empty());
    assert!(projection.visible_turn_items.is_empty());
    let mut malformed_future_row = raw.clone();
    malformed_future_row["visibleTurnItems"][0]
        .as_object_mut()
        .ok_or("missing row")?
        .remove("position");
    assert!(
        serde_json::from_value::<OrchestrationV2ThreadProjection>(malformed_future_row).is_err()
    );
    raw["turnItems"] = json!([{"type":"reasoning"}]);
    assert!(serde_json::from_value::<OrchestrationV2ThreadProjection>(raw.clone()).is_err());
    raw["turnItems"] = json!([]);
    raw["visibleTurnItems"][0]["item"]["type"] = json!("reasoning");
    assert!(serde_json::from_value::<OrchestrationV2ThreadProjection>(raw).is_err());
    Ok(())
}

#[test]
fn compact_envelopes_preserve_indices_and_validate_before_dropping_rows()
-> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    let samples = fixture_samples("OrchestrationV2ThreadHistoryPage")?;
    let mut compact = samples
        .iter()
        .find(|raw| {
            raw["ids"].as_array().is_some_and(|ids| !ids.is_empty())
                && raw["items"].as_array().is_some_and(|rows| !rows.is_empty())
        })
        .ok_or("missing compact fixture")?
        .clone();
    let decoded: OrchestrationV2ThreadHistoryPage = serde_json::from_value(compact.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, compact);
    let invalid = compact["ids"].as_array().ok_or("missing id table")?.len();
    compact["items"][0]["sourceThreadId"] = json!(invalid);
    assert!(serde_json::from_value::<OrchestrationV2ThreadHistoryPage>(compact.clone()).is_err());
    compact["items"][0]["item"]["type"] = json!("future");
    assert!(serde_json::from_value::<OrchestrationV2ThreadHistoryPage>(compact).is_err());
    let mut plain = samples
        .into_iter()
        .find(|raw| {
            raw.get("ids").is_none() && raw["items"].as_array().is_some_and(|rows| !rows.is_empty())
        })
        .ok_or("missing plain fixture")?;
    plain["items"][0]["sourceThreadId"] = json!("source:parent");
    plain["items"][0]["sourceItemId"] = json!("source:item");
    plain["items"][0]["item"]["id"] = json!("local:item");
    plain["items"][0]["item"]["threadId"] = json!("local:thread");
    plain["idTable"] = json!(true);
    let page: OrchestrationV2ThreadHistoryPage = serde_json::from_value(plain)?;
    let wire = serde_json::to_value(page)?;
    assert!(wire.get("idTable").is_none());
    assert_eq!(
        &wire["ids"].as_array().ok_or("missing table")?[..4],
        &[
            json!("source:parent"),
            json!("source:item"),
            json!("local:item"),
            json!("local:thread")
        ]
    );
    assert_eq!(wire["items"][0]["item"]["id"], json!(2));
    Ok(())
}

#[test]
fn replay_defaults_and_lenient_opens_preserve_presence() -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    let mut raw = fixture_samples("OrchestrationV2AppThread")?
        .into_iter()
        .next()
        .ok_or("empty fixtures")?;
    let object = raw.as_object_mut().ok_or("missing thread object")?;
    for key in ["settledOverride", "settledAt", "lastVisitedAt"] {
        object.remove(key);
    }
    object.insert("opens".into(), json!({"score":"future-shape"}));
    let thread: OrchestrationV2AppThread = serde_json::from_value(raw)?;
    let wire = serde_json::to_value(thread)?;
    for key in ["settledOverride", "settledAt", "lastVisitedAt", "opens"] {
        assert_eq!(wire.get(key), Some(&serde_json::Value::Null));
    }
    let mut capabilities = fixture_samples("OrchestrationV2ProviderCapabilities")?
        .into_iter()
        .next()
        .ok_or("empty fixtures")?;
    capabilities
        .as_object_mut()
        .ok_or("missing capabilities")?
        .remove("runtimePolicy");
    let decoded: OrchestrationV2ProviderCapabilities = serde_json::from_value(capabilities)?;
    assert_eq!(
        serde_json::to_value(decoded)?["runtimePolicy"],
        json!({"enforcement":"client-boundary"})
    );
    assert!(
        serde_json::from_value::<OrchestrationV2SubscribeThreadInput>(
            json!({"threadId":"t", "acceptCompactIds":null})
        )
        .is_err()
    );
    Ok(())
}
