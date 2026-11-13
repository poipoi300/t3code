#![allow(clippy::unwrap_used)]
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeSet;
use t3_codex_app_server::*;
#[path = "support/audit.rs"]
mod audit;
use audit::{Audit, numbers};

fn check<T: DeserializeOwned + Serialize + Audit>(name: &str, cases: &Value) {
    for (i, original) in cases[name].as_array().unwrap().iter().enumerate() {
        let typed: T = serde_json::from_value(original.clone())
            .unwrap_or_else(|e| panic!("{name} sample {i}: {e}"));
        let mut extra = BTreeSet::new();
        typed.audit(&mut extra);
        assert!(extra.is_empty(), "{name} sample {i}: {extra:?}");
        assert_eq!(
            numbers(serde_json::to_value(typed).unwrap()),
            numbers(original.clone()),
            "{name} sample {i}"
        );
    }
}

#[test]
fn schema_fields_nullability_and_all_union_variants_round_trip() {
    let cases: Value = serde_json::from_str(include_str!("schema_cases.json")).unwrap();
    let sources: Value = serde_json::from_str(include_str!("schema_sources.json")).unwrap();
    let schema =
        include_str!("../../../../packages/effect-codex-app-server/src/_generated/schema.gen.ts")
            .replace("\r\n", "\n");
    let fingerprint = schema.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    assert_eq!(
        format!("{fingerprint:016x}"),
        sources["schema_fnv1a"],
        "Regenerate schema cases after reviewing schema drift"
    );
    check::<Empty>("Empty", &cases);
    check::<ClientInfo>("ClientInfo", &cases);
    check::<InitializeCapabilities>("InitializeCapabilities", &cases);
    check::<InitializeParams>("InitializeParams", &cases);
    check::<InitializeResponse>("InitializeResponse", &cases);
    check::<ThreadStartParams>("ThreadStartParams", &cases);
    check::<ThreadResumeParams>("ThreadResumeParams", &cases);
    check::<ThreadForkParams>("ThreadForkParams", &cases);
    check::<ThreadIdParams>("ThreadIdParams", &cases);
    check::<ThreadReadParams>("ThreadReadParams", &cases);
    check::<ThreadRevertParams>("ThreadRevertParams", &cases);
    check::<ThreadTurnsListParams>("ThreadTurnsListParams", &cases);
    check::<ThreadInjectItemsParams>("ThreadInjectItemsParams", &cases);
    check::<ThreadGoalSetParams>("ThreadGoalSetParams", &cases);
    check::<Thread>("Thread", &cases);
    check::<ThreadResponse>("ThreadResponse", &cases);
    check::<ThreadTurnsListResponse>("ThreadTurnsListResponse", &cases);
    check::<ThreadGoal>("ThreadGoal", &cases);
    check::<ThreadGoalResponse>("ThreadGoalResponse", &cases);
    check::<ThreadGoalClearResponse>("ThreadGoalClearResponse", &cases);
    check::<Turn>("Turn", &cases);
    check::<TurnError>("TurnError", &cases);
    check::<TurnResponse>("TurnResponse", &cases);
    check::<CollaborationMode>("CollaborationMode", &cases);
    check::<AdditionalContextEntry>("AdditionalContextEntry", &cases);
    check::<TurnStartParams>("TurnStartParams", &cases);
    check::<TurnSteerParams>("TurnSteerParams", &cases);
    check::<TurnSteerResponse>("TurnSteerResponse", &cases);
    check::<TurnInterruptParams>("TurnInterruptParams", &cases);
    check::<ConfigReadParams>("ConfigReadParams", &cases);
    check::<ConfigReadResponse>("ConfigReadResponse", &cases);
    check::<FeedbackUploadParams>("FeedbackUploadParams", &cases);
    check::<FeedbackUploadResponse>("FeedbackUploadResponse", &cases);
    check::<ModelListParams>("ModelListParams", &cases);
    check::<ReasoningEffortOption>("ReasoningEffortOption", &cases);
    check::<Model>("Model", &cases);
    check::<ModelListResponse>("ModelListResponse", &cases);
    check::<GetAccountParams>("GetAccountParams", &cases);
    check::<GetAccountResponse>("GetAccountResponse", &cases);
    check::<RateLimitWindow>("RateLimitWindow", &cases);
    check::<GetAccountRateLimitsResponse>("GetAccountRateLimitsResponse", &cases);
    check::<ConsumeResetCreditParams>("ConsumeResetCreditParams", &cases);
    check::<ConsumeResetCreditResponse>("ConsumeResetCreditResponse", &cases);
    check::<SkillsListParams>("SkillsListParams", &cases);
    check::<Skill>("Skill", &cases);
    check::<SkillsListEntry>("SkillsListEntry", &cases);
    check::<SkillsListResponse>("SkillsListResponse", &cases);
    check::<ListMcpServerStatusParams>("ListMcpServerStatusParams", &cases);
    check::<McpServerStatus>("McpServerStatus", &cases);
    check::<ListMcpServerStatusResponse>("ListMcpServerStatusResponse", &cases);
    check::<McpServerToolCallParams>("McpServerToolCallParams", &cases);
    check::<McpServerToolCallResponse>("McpServerToolCallResponse", &cases);
    check::<McpResourceReadParams>("McpResourceReadParams", &cases);
    check::<McpResourceReadResponse>("McpResourceReadResponse", &cases);
    check::<BackgroundTerminalsListParams>("BackgroundTerminalsListParams", &cases);
    check::<BackgroundTerminal>("BackgroundTerminal", &cases);
    check::<BackgroundTerminalsListResponse>("BackgroundTerminalsListResponse", &cases);
    check::<BackgroundTerminalsTerminateParams>("BackgroundTerminalsTerminateParams", &cases);
    check::<BackgroundTerminalsTerminateResponse>("BackgroundTerminalsTerminateResponse", &cases);
    check::<ThreadSampleParams>("ThreadSampleParams", &cases);
    check::<ThreadSampleResponse>("ThreadSampleResponse", &cases);
    check::<ToolError>("ToolError", &cases);
    check::<FileChange>("FileChange", &cases);
    check::<ItemDelta>("ItemDelta", &cases);
    check::<ItemNotification>("ItemNotification", &cases);
    check::<TurnNotification>("TurnNotification", &cases);
    check::<ThreadNotification>("ThreadNotification", &cases);
    check::<PlanStep>("PlanStep", &cases);
    check::<TurnPlanUpdated>("TurnPlanUpdated", &cases);
    check::<TokenUsageBreakdown>("TokenUsageBreakdown", &cases);
    check::<TokenUsage>("TokenUsage", &cases);
    check::<ThreadTokenUsageUpdated>("ThreadTokenUsageUpdated", &cases);
    check::<ThreadSettings>("ThreadSettings", &cases);
    check::<ThreadSettingsUpdated>("ThreadSettingsUpdated", &cases);
    check::<ModelRerouted>("ModelRerouted", &cases);
    check::<McpStartupStatusUpdated>("McpStartupStatusUpdated", &cases);
    check::<HookRun>("HookRun", &cases);
    check::<HookCompleted>("HookCompleted", &cases);
    check::<ThreadGoalUpdated>("ThreadGoalUpdated", &cases);
    check::<ErrorNotification>("ErrorNotification", &cases);
    check::<AccountRateLimitsUpdated>("AccountRateLimitsUpdated", &cases);
    check::<CommandApprovalParams>("CommandApprovalParams", &cases);
    check::<FileChangeApprovalParams>("FileChangeApprovalParams", &cases);
    check::<PermissionsApprovalParams>("PermissionsApprovalParams", &cases);
    check::<ApprovalResponse>("ApprovalResponse", &cases);
    check::<PermissionsApprovalResponse>("PermissionsApprovalResponse", &cases);
    check::<UserInputOption>("UserInputOption", &cases);
    check::<UserInputQuestion>("UserInputQuestion", &cases);
    check::<UserInputParams>("UserInputParams", &cases);
    check::<UserInputAnswer>("UserInputAnswer", &cases);
    check::<UserInputResponse>("UserInputResponse", &cases);
    check::<ElicitationResponse>("ElicitationResponse", &cases);
    check::<LegacyExecApprovalParams>("LegacyExecApprovalParams", &cases);
    check::<LegacyPatchApprovalParams>("LegacyPatchApprovalParams", &cases);
    check::<FileChangeApprovalResponse>("FileChangeApprovalResponse", &cases);
    check::<LegacyExecApprovalResponse>("LegacyExecApprovalResponse", &cases);
    check::<LegacyPatchApprovalResponse>("LegacyPatchApprovalResponse", &cases);
    check::<UserInput>("UserInput", &cases);
    check::<ItemBody>("ItemBody", &cases);
    check::<FileChangeKind>("FileChangeKind", &cases);
    check::<ElicitationParams>("ElicitationParams", &cases);
    check::<Account>("Account", &cases);
    check::<ThreadStatus>("ThreadStatus", &cases);
    check::<CollaborationSettings>("CollaborationSettings", &cases);
    check::<RateLimitSnapshot>("RateLimitSnapshot", &cases);
    check::<ThreadItem>("ThreadItem", &cases);
    check::<RequestResolved>("RequestResolved", &cases);
    check::<FunctionCallOutputBody>("FunctionCallOutputBody", &cases);
}
