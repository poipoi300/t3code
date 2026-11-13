# Codex protocol type audit

The authority is `packages/effect-codex-app-server/src/_generated/schema.gen.ts`, plus `src/client.ts` for the three `turn/start` additions and `apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts` for raw sample/background-terminal extensions. Every declaration in `src/types.rs` was compared field by field, including wire spelling, required/optional/nullable fields, annotations and union alternatives. The table lists the audited types and discrepancies found in the original crate.

The generated reference test covers 108 concrete types and 534 cases: minimal objects, all fields, every optional nullable key set to null, and all declared root union alternatives. It fails on unexpected catch-all fields, known shapes decoded as Other, lost keys and changed values. `tests/generate_schema_cases.py` regenerates these cases from TypeScript declarations; `schema_sources.json` pins a normalized source fingerprint so stale cases cannot silently pass after a schema change. This checks serde wire preservation; it does not claim to reproduce every Effect schema refinement.

`scope` default `turn`, `text_elements` default `[]`, and the other generated `default` annotations describe peer defaults. They do not transform omitted fields in the TypeScript decoder. Rust likewise leaves them absent; in particular a default permission response never emits an empty scope. Default constructors are construction conveniences, not decoder defaults. Literal status/effort/phase strings remain open strings, while explicitly modeled structural unions have Other variants.

| Rust type | TypeScript counterpart | Discrepancies corrected |
| --- | --- | --- |
| `Empty` | `Empty RPC result object` | Open empty result object; unknown result fields remain in `extra`. |
| `ClientInfo` | `V1InitializeParams__ClientInfo` | preserve missing/null `title` |
| `InitializeCapabilities` | `V1InitializeParams__InitializeCapabilities` | add `explicitGatewayOauth`; preserve missing/null `extensions`; add `mcpServerOpenaiFormElicitation`; preserve missing/null `optOutNotificationMethods`; add `requestAttestation` |
| `InitializeParams` | `V1InitializeParams` | preserve missing/null `capabilities` |
| `InitializeResponse` | `V1InitializeResponse` | No field discrepancy. |
| `ThreadStartParams` | `V2ThreadStartParams` | preserve missing/null `approvalPolicy`; preserve missing/null `approvalsReviewer`; preserve missing/null `baseInstructions`; preserve missing/null `config`; preserve missing/null `cwd`; preserve missing/null `developerInstructions`; preserve missing/null `ephemeral`; preserve missing/null `model`; preserve missing/null `modelProvider`; add `personality`; preserve missing/null `sandbox`; add `serviceName`; preserve missing/null `serviceTier`; add `sessionStartSource`; add `threadSource` |
| `ThreadResumeParams` | `V2ThreadResumeParams` | preserve missing/null `approvalPolicy`; add `approvalsReviewer`; add `baseInstructions`; preserve missing/null `config`; preserve missing/null `cwd`; add `developerInstructions`; preserve missing/null `model`; preserve missing/null `modelProvider`; add `personality`; preserve missing/null `sandbox`; preserve missing/null `serviceTier` |
| `ThreadForkParams` | `V2ThreadForkParams` | add `approvalPolicy`; add `approvalsReviewer`; add `baseInstructions`; preserve missing/null `config`; preserve missing/null `cwd`; add `developerInstructions`; preserve missing/null `lastTurnId`; preserve missing/null `model`; add `modelProvider`; add `sandbox`; add `serviceTier`; add `threadSource` |
| `ThreadIdParams` | `V2ThreadGoalGetParams` | No field discrepancy. |
| `ThreadReadParams` | `V2ThreadReadParams` | No field discrepancy. |
| `ThreadRevertParams` | `V2ThreadRevertParams` | No field discrepancy. |
| `ThreadTurnsListParams` | `V2ThreadTurnsListParams` | preserve missing/null `cursor`; preserve missing/null `itemsView`; preserve missing/null `limit`; preserve missing/null `sortDirection` |
| `ThreadInjectItemsParams` | `V2ThreadInjectItemsParams` | No field discrepancy; `items` remains an explicit raw JSON array. |
| `ThreadGoalSetParams` | `V2ThreadGoalSetParams` | preserve missing/null `objective`; preserve missing/null `status`; preserve missing/null `tokenBudget` |
| `Thread` | `V2ThreadReadResponse__Thread` | add `agentNickname`; add `agentRole`; add `cliVersion`; correct optionality `createdAt`; add `cwd`; add `ephemeral`; add `forkedFromId`; add `gitInfo`; preserve missing/null `model`; add `modelProvider`; add `name`; add `originator`; add `parentThreadId`; add `path`; add `preview`; add `projectId`; preserve missing/null `reasoningEffort`; add `recencyAt`; add `section`; add `sectionEnteredAt`; add `sessionId`; add `source`; correct optionality `status`; add `threadSource`; correct optionality `turns`; correct optionality `updatedAt` |
| `ThreadResponse` | `V2ThreadStartResponse` | Add `approvalPolicy`, `approvalsReviewer`, `cwd`, `disabledPluginIds`, `instructionSources`, `modelProvider`, `sandbox`, `collaborationMode`, `itemsBackwardsCursor`, `turnsBackwardsCursor`; preserve missing/null `reasoningEffort` and `serviceTier`. Keep `thread` required. Other fields vary by RPC and are optional in this shared envelope. |
| `ThreadTurnsListResponse` | `V2ThreadTurnsListResponse` | add `backwardsCursor`; preserve missing/null `nextCursor` |
| `ThreadGoal` | `V2ThreadGoalGetResponse__ThreadGoal` | add `createdAt`; add `threadId`; preserve missing/null `tokenBudget`; add `updatedAt` |
| `ThreadGoalResponse` | `V2ThreadGoalGetResponse` | preserve missing/null `goal` |
| `ThreadGoalClearResponse` | `V2ThreadGoalClearResponse` | No field discrepancy. |
| `Turn` | `V2TurnStartResponse__Turn` | preserve missing/null `completedAt`; add `durationMs`; preserve missing/null `error`; correct optionality `items`; add `itemsView`; preserve missing/null `startedAt`; correct optionality `status` |
| `TurnError` | `V2ThreadReadResponse__TurnError` | preserve missing/null `additionalDetails`; preserve missing/null `codexErrorInfo`; add `misalignment` |
| `TurnResponse` | `V2TurnStartResponse` | No field discrepancy. |
| `CollaborationMode` | `ClientRequest__CollaborationMode` | No field discrepancy. |
| `AdditionalContextEntry` | `V2TurnStartParams__AdditionalContextEntry` | No field discrepancy; `value` stays a string. |
| `TurnStartParams` | `V2TurnStartParams` | preserve missing/null `approvalPolicy`; preserve missing/null `approvalsReviewer`; add `clientUserMessageId`; preserve missing/null `cwd`; add `disabledPluginIds`; preserve missing/null `effort`; preserve missing/null `model`; add `personality`; preserve missing/null `sandboxPolicy`; preserve missing/null `serviceTier`; add `serviceTierForTurn`; preserve missing/null `summary`; add `toolOutput`; add `turnTrigger` |
| `TurnSteerParams` | `V2TurnSteerParams` | add `clientUserMessageId` |
| `TurnSteerResponse` | `V2TurnSteerResponse` | No field discrepancy. |
| `TurnInterruptParams` | `V2TurnInterruptParams` | No field discrepancy. |
| `ConfigReadParams` | `V2ConfigReadParams` | Preserve missing/null `cwd`; `includeLayers` remains optional as declared. |
| `ConfigReadResponse` | `V2ConfigReadResponse` | preserve missing/null `layers`; correct optionality `origins` |
| `FeedbackUploadParams` | `V2FeedbackUploadParams` | Add `extraLogFiles`, `tags` (a string map); make `includeLogs` optional; preserve missing/null `reason` and `threadId`. |
| `FeedbackUploadResponse` | `V2FeedbackUploadResponse` | add `promptHash` |
| `ModelListParams` | `V2ModelListParams` | preserve missing/null `cursor`; preserve missing/null `includeHidden`; preserve missing/null `limit` |
| `ReasoningEffortOption` | `V2ModelListResponse__ReasoningEffortOption` | No field discrepancy. |
| `Model` | `V2ModelListResponse__Model` | add `additionalSpeedTiers`; add `availabilityNux`; preserve missing/null `availableAccessPrograms`; correct optionality `defaultReasoningEffort`; preserve missing/null `defaultServiceTier`; correct optionality `description`; add `hidden`; add `inputModalities`; correct optionality `isDefault`; add `modelSpecialty`; add `multiAgentVersion`; correct optionality `supportedReasoningEfforts`; add `supportsPersonality`; add `upgrade`; add `upgradeInfo` |
| `ModelListResponse` | `V2ModelListResponse` | preserve missing/null `nextCursor` |
| `GetAccountParams` | `V2GetAccountParams` | No field discrepancy; `refreshToken` remains optional. |
| `GetAccountResponse` | `V2GetAccountResponse` | preserve missing/null `account` |
| `RateLimitWindow` | `V2GetAccountRateLimitsResponse__RateLimitWindow` | preserve missing/null `resetsAt`; preserve missing/null `windowDurationMins` |
| `GetAccountRateLimitsResponse` | `V2GetAccountRateLimitsResponse` | Add `ordinaryUsageAllowed`, `rateLimitUpsell`; preserve missing/null `accountId`, `rateLimitResetCredits`, `rateLimitsByLimitId`. |
| `ConsumeResetCreditParams` | `V2ConsumeAccountRateLimitResetCreditParams` | add `creditId` |
| `ConsumeResetCreditResponse` | `V2ConsumeAccountRateLimitResetCreditResponse` | No field discrepancy. |
| `SkillsListParams` | `V2SkillsListParams` | No field discrepancy. |
| `Skill` | `V2SkillsListResponse__SkillMetadata` | add `dependencies`; correct optionality `description`; preserve missing/null `interface`; add `pluginId`; correct optionality `scope`; preserve missing/null `shortDescription` |
| `SkillsListEntry` | `V2SkillsListResponse__SkillsListEntry` | No field discrepancy. |
| `SkillsListResponse` | `V2SkillsListResponse` | No field discrepancy. |
| `ListMcpServerStatusParams` | `V2ListMcpServerStatusParams` | preserve missing/null `cursor`; preserve missing/null `detail`; preserve missing/null `limit`; preserve missing/null `serverName`; preserve missing/null `threadId` |
| `McpServerStatus` | `V2ListMcpServerStatusResponse__McpServerStatus` | add `authStatus`; add `httpOrigin`; add `pluginId`; add `resourceTemplates`; add `resources`; add `runtimeStatus`; add `serverCapabilities`; add `serverInfo`; add `toolsError` |
| `ListMcpServerStatusResponse` | `V2ListMcpServerStatusResponse` | preserve missing/null `nextCursor` |
| `McpServerToolCallParams` | `V2McpServerToolCallParams` | add `_meta`; correct optionality `arguments` |
| `McpServerToolCallResponse` | `V2McpServerToolCallResponse` | add `_meta`; preserve missing/null `isError` |
| `McpResourceReadParams` | `V2McpResourceReadParams` | add `connectorId`; add `originCallId`; add `target`; preserve missing/null `threadId` |
| `McpResourceReadResponse` | `V2McpResourceReadResponse` | add `originCallId` |
| `BackgroundTerminalsListParams` | `CodexAdapterV2.ts (raw extension)` | No field discrepancy against adapter raw requests / local response schemas. |
| `BackgroundTerminal` | `CodexAdapterV2.ts (raw extension)` | No field discrepancy against adapter raw requests / local response schemas. |
| `BackgroundTerminalsListResponse` | `CodexAdapterV2.ts (raw extension)` | No field discrepancy against adapter raw requests / local response schemas. |
| `BackgroundTerminalsTerminateParams` | `CodexAdapterV2.ts (raw extension)` | No field discrepancy against adapter raw requests / local response schemas. |
| `BackgroundTerminalsTerminateResponse` | `CodexAdapterV2.ts (raw extension)` | No field discrepancy against adapter raw requests / local response schemas. |
| `ThreadSampleParams` | `CodexAdapterV2.ts (raw extension)` | No field discrepancy against adapter raw requests / local response schemas. |
| `ThreadSampleResponse` | `CodexAdapterV2.ts (raw extension)` | No field discrepancy against adapter raw requests / local response schemas. |
| `ToolError` | `V2ThreadReadResponse__McpToolCallError` | No field discrepancy. |
| `FileChange` | `V2ThreadReadResponse__FileUpdateChange` | No field discrepancy. |
| `ItemDelta` | `V2AgentMessageDeltaNotification` | No field discrepancy. |
| `ItemNotification` | `V2ItemCompletedNotification` | No field discrepancy. |
| `TurnNotification` | `V2TurnCompletedNotification` | No field discrepancy. |
| `ThreadNotification` | `V2ThreadClosedNotification` | No field discrepancy. |
| `PlanStep` | `V2TurnPlanUpdatedNotification__TurnPlanStep` | No field discrepancy. |
| `TurnPlanUpdated` | `V2TurnPlanUpdatedNotification` | preserve missing/null `explanation` |
| `TokenUsageBreakdown` | `V2ThreadTokenUsageUpdatedNotification__TokenUsageBreakdown` | No field discrepancy. |
| `TokenUsage` | `V2ThreadTokenUsageUpdatedNotification__ThreadTokenUsage` | preserve missing/null `modelContextWindow` |
| `ThreadTokenUsageUpdated` | `V2ThreadTokenUsageUpdatedNotification` | No field discrepancy. |
| `ThreadSettings` | `V2ThreadSettingsUpdatedNotification__ThreadSettings` | add `activePermissionProfile`; add `approvalPolicy`; add `approvalsReviewer`; add `collaborationMode`; add `cwd`; add `disabledPluginIds`; preserve missing/null `effort`; add `modelProvider`; add `personality`; add `sandboxPolicy`; preserve missing/null `serviceTier`; add `summary` |
| `ThreadSettingsUpdated` | `V2ThreadSettingsUpdatedNotification` | No field discrepancy. |
| `ModelRerouted` | `V2ModelReroutedNotification` | correct optionality `fromModel`; correct optionality `reason` |
| `McpStartupStatusUpdated` | `V2McpServerStatusUpdatedNotification` | preserve missing/null `error`; add `failureReason`; preserve missing/null `threadId` |
| `HookRun` | `V2HookCompletedNotification__HookRunSummary` | add `completedAt`; add `displayOrder`; add `durationMs`; correct optionality `eventName`; add `executionMode`; add `handlerType`; add `id`; add `scope`; add `source`; add `sourcePath`; add `startedAt`; add `statusMessage` |
| `HookCompleted` | `V2HookCompletedNotification` | preserve missing/null `turnId` |
| `ThreadGoalUpdated` | `V2ThreadGoalUpdatedNotification` | add `turnId` |
| `ErrorNotification` | `V2ErrorNotification` | correct optionality `threadId`; correct optionality `turnId` |
| `AccountRateLimitsUpdated` | `V2AccountRateLimitsUpdatedNotification` | No field discrepancy. |
| `CommandApprovalParams` | `CommandExecutionRequestApprovalParams` | preserve missing/null `approvalId`; preserve missing/null `command`; add `commandActions`; add `cwd`; add `environmentId`; add `kind`; add `networkApprovalContext`; add `proposedExecpolicyAmendment`; add `proposedNetworkPolicyAmendments`; preserve missing/null `reason`; add `startedAtMs` |
| `FileChangeApprovalParams` | `FileChangeRequestApprovalParams` | preserve missing/null `grantRoot`; preserve missing/null `reason`; add `startedAtMs` |
| `PermissionsApprovalParams` | `PermissionsRequestApprovalParams` | add `cwd`; add `environmentId`; preserve missing/null `reason`; add `startedAtMs` |
| `ApprovalResponse` | `CommandExecutionRequestApprovalResponse` | No field discrepancy. |
| `PermissionsApprovalResponse` | `PermissionsRequestApprovalResponse` | correct optionality `scope`; add `strictAutoReview` |
| `UserInputOption` | `ToolRequestUserInputParams__ToolRequestUserInputOption` | No field discrepancy. |
| `UserInputQuestion` | `ToolRequestUserInputParams__ToolRequestUserInputQuestion` | preserve missing/null `options` |
| `UserInputParams` | `ToolRequestUserInputParams` | add `autoResolutionMs`; add `isBlocking` |
| `UserInputAnswer` | `ToolRequestUserInputResponse__ToolRequestUserInputAnswer` | No field discrepancy. |
| `UserInputResponse` | `ToolRequestUserInputResponse` | No field discrepancy. |
| `ElicitationResponse` | `McpServerElicitationRequestResponse` | add `_meta` |
| `LegacyExecApprovalParams` | `ExecCommandApprovalParams` | preserve missing/null `approvalId`; add `cwd`; add `parsedCmd`; preserve missing/null `reason` |
| `LegacyPatchApprovalParams` | `ApplyPatchApprovalParams` | preserve missing/null `grantRoot`; preserve missing/null `reason` |
| `UserInput` | `V2TurnStartParams__UserInput` | Correct `text_elements` wire name; add image `fileId` alternative, image/local-image `detail`, `audio` and `localAudio`; retain missing/null detail, known-variant extension fields, and unknown / changed known shapes. |
| `ThreadItem` | `V2ThreadReadResponse__ThreadItem` | Keep `id` and flattened body; changed known item shapes fall back to `ItemBody::Other` instead of failing the enclosing thread or notification. |
| `ItemBody` | `V2ThreadReadResponse__ThreadItem` | Add `hookPrompt`, `functionCallOutput`, `sleep`, `enteredReviewMode`, `exitedReviewMode`. Add user `clientId`; agent `memoryCitation`; command `commandActions`, `cwd`, `durationMs`, `pluginId`, `scriptPath`, `source`; MCP `durationMs`, `pluginId`, `readOnlyHint`; dynamic-tool `durationMs`; collab `senderThreadId`, `status`; web-search `results`. Correct every optional nullable item field to preserve null; image-generation `result` is an optional string. Retain all 19 known discriminants and unknown / changed known shapes. |
| `FileChangeKind` | `V2ThreadReadResponse__PatchChangeKind` | Correct the update field to snake_case `move_path`, preserve missing/null; model add/delete/update explicitly, plus unknown / changed known shapes. |
| `ThreadStatus` | `V2ThreadReadResponse__ThreadStatus` | Model notLoaded/idle/systemError/active explicitly; active has required `activeFlags`; retain unknown / changed known shapes. |
| `Account` | `V2GetAccountResponse__Account` | Model apiKey/chatgpt/amazonBedrock explicitly; chatgpt email is nullable, planType required; include `usesCodexManagedCredentials`; retain unknown / changed known shapes. |
| `ElicitationParams` | `McpServerElicitationRequestParams` | Model form/openai/form/openaiForm/url modes explicitly; add `_meta` and URL-mode `url`; require `serverName`, `threadId`, `message`, and mode-specific schema or URL/id; preserve missing/null `turnId` and JSON `_meta`; retain unknown / changed known modes. |
| `CollaborationSettings` | `ClientRequest__Settings` | Keep snake_case `reasoning_effort`; preserve missing/null; add optional nullable snake_case `developer_instructions`. |
| `RateLimitSnapshot` | `V2GetAccountRateLimitsResponse__RateLimitSnapshot` | Add optional nullable `limitName`, `normalModelSlug`, `spendControlReached`; existing missing/null/value semantics retained for all other fields. |
| `RequestResolved` | `V2ServerRequestResolvedNotification` | Add required `threadId` alongside `requestId`; preserve extra fields; the original local helper discarded threadId. |
| `FunctionCallOutputBody` | `V2ThreadReadResponse__FunctionCallOutputBody` | Explicit string / content-array / unknown representation; known function-call output items no longer rely on catch-all JSON for this union. |
| `FileChangeApprovalResponse` | `FileChangeRequestApprovalResponse` | Distinct response type for the file-change responder; `decision` remains raw protocol JSON. |
| `LegacyExecApprovalResponse` | `ExecCommandApprovalResponse` | Distinct legacy exec response type; `decision` remains raw protocol JSON. |
| `LegacyPatchApprovalResponse` | `ApplyPatchApprovalResponse` | Distinct legacy patch response type; `decision` remains raw protocol JSON. |
| `Field` | `Internal method/presence union` | Internal presence union: Missing / Null / Value. No protocol tag; no discrepancy. |
| `Object` | `Internal method/presence union` | Open JSON object alias; no fixed schema fields. |
| `ServerNotification` | `Internal method/presence union` | Method-dispatched union remains open through Unknown. RequestResolved now contains its complete typed DTO. Item/turn notification nesting uses the open item/input unions. |
| `ServerRequestParams` | `Internal method/presence union` | Method-dispatched union remains open through Unknown. Known params use their audited DTOs; malformed known request objects are rejected without approval. |

## Deliberate compatibility boundaries

- `Thread` accepts absent `projectId` and `sessionId` because seven recorded older thread payloads omit them; the current generated Thread requires them. Missing values are preserved as missing. This does not invent metadata or change an approval decision.
- `ThreadResponse` merges start/resume/fork/read/revert/unarchive results. Only `thread` is common and required. `ItemNotification` shares started/completed payloads, and `ItemDelta` shares ordinary/reasoning delta payloads; method-specific timestamp/index fields are optional in the common DTO. The typed replay validates each recorded method against its actual frame.
- Config bodies/origins/layers, raw Responses items, MCP content/resources/tool/schema bodies, permissions, citations, hook entries and nested tool state remain explicit `Value` or maps/arrays of `Value`. Their keys are user/server-defined; they are not catch-all DTO `extra` fields. This is the existing subset boundary, not a claim that every transitive Codex schema has a Rust struct. Numeric DTOs use the schema's annotated integer widths; serde does not duplicate all Effect refinements. Raw RPC IDs and error codes accept finite JavaScript numbers, including fractional values and values above i64::MAX, with JavaScript ID correlation.
- The exact recorded extensions allowed in known DTO `extra` maps, with reasons, are maintained in `tests/replay.rs::assert_extras`. No wildcard exceptions are used. Fields declared in the generated schemas, including `text_elements`, cannot hide there. Unknown methods and intentionally unknown variants retain their JSON.
- Malformed/unroutable frames are reported and skipped. A malformed response whose id can be decoded fails only its matching waiter. An oversized frame is discarded through its line boundary. This is the requested recovery policy; the TypeScript transport currently terminates on malformed/unroutable wire frames. Transport I/O failure and EOF still terminate the connection.
- Events acknowledge completed handling on the next stream poll, or explicitly through `Events::acknowledge()`. Responses wait for that watermark; a bounded queue pauses the reader. This provides the TypeScript inline-handler ordering through a pull-based API. Callers must finish handling an event before acknowledging it.
- The client owns no process. Windows escaping was compared directly with `packages/shared/src/shell.ts::escapeWindowsShellArg`, including embedded quotes, adjacent/trailing backslashes, spaces and CMD metacharacters. The regression vectors from `shell.test.ts` pass unchanged; no launch implementation defect was found. Actual batch execution and POSIX execution were not probed.

## Review regressions

C1 and C4 failed first on lost text spans / omitted scope; typed corpus equality also failed on omitted nulls. C2 failed first on a response overtaking an earlier event and on unbounded admission, then on acknowledging delivery before processing finished. C5 failed first on duplicate active ids and reply-serialization termination. Numeric-id/code, malformed-reply, oversized-frame and initialization-gate regressions failed before their fixes. Permission tests explicitly try affirmative replies after cancellation, replacement, malformed replacement, shutdown and last-client drop; they verify rejection and an error-only or empty wire. Task cancellation and unanswered-handle drop are also pinned.

All DTOs are Codex-specific local equivalents. If shared across the rewrite, the types in this table belong in a Codex protocol module in `t3-contracts`; no sibling crate was edited. Transport handles, client state and pure launch descriptions stay in this crate.
