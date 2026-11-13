"""Regenerate serde audit cases from the authoritative TypeScript type declarations."""
from pathlib import Path
import json
import re

CRATE = Path(__file__).resolve().parents[1]
REPO = CRATE.parents[2]
SOURCE = REPO / "packages/effect-codex-app-server/src/_generated/schema.gen.ts"
DEFS = dict(re.findall(r"export type (\w+) =\s*(.*?)(?=\nexport )", SOURCE.read_text(), re.S))
ALIASES = dict(line.split("=", 1) for line in """
ClientInfo=V1InitializeParams__ClientInfo
InitializeCapabilities=V1InitializeParams__InitializeCapabilities
ThreadIdParams=V2ThreadGoalGetParams
Thread=V2ThreadReadResponse__Thread
ThreadStatus=V2ThreadReadResponse__ThreadStatus
ThreadResponse=V2ThreadStartResponse
ThreadGoal=V2ThreadGoalGetResponse__ThreadGoal
ThreadGoalResponse=V2ThreadGoalGetResponse
Turn=V2TurnStartResponse__Turn
TurnResponse=V2TurnStartResponse
TurnError=V2ThreadReadResponse__TurnError
UserInput=V2TurnStartParams__UserInput
CollaborationMode=ClientRequest__CollaborationMode
CollaborationSettings=ClientRequest__Settings
AdditionalContextEntry=V2TurnStartParams__AdditionalContextEntry
ReasoningEffortOption=V2ModelListResponse__ReasoningEffortOption
Model=V2ModelListResponse__Model
Account=V2GetAccountResponse__Account
RateLimitWindow=V2GetAccountRateLimitsResponse__RateLimitWindow
RateLimitSnapshot=V2GetAccountRateLimitsResponse__RateLimitSnapshot
ConsumeResetCreditParams=V2ConsumeAccountRateLimitResetCreditParams
ConsumeResetCreditResponse=V2ConsumeAccountRateLimitResetCreditResponse
Skill=V2SkillsListResponse__SkillMetadata
SkillsListEntry=V2SkillsListResponse__SkillsListEntry
McpServerStatus=V2ListMcpServerStatusResponse__McpServerStatus
ItemBody=V2ThreadReadResponse__ThreadItem
ThreadItem=V2ThreadReadResponse__ThreadItem
ToolError=V2ThreadReadResponse__McpToolCallError
FileChange=V2ThreadReadResponse__FileUpdateChange
FileChangeKind=V2ThreadReadResponse__PatchChangeKind
ItemDelta=V2AgentMessageDeltaNotification
ItemNotification=V2ItemCompletedNotification
TurnNotification=V2TurnCompletedNotification
ThreadNotification=V2ThreadClosedNotification
PlanStep=V2TurnPlanUpdatedNotification__TurnPlanStep
TurnPlanUpdated=V2TurnPlanUpdatedNotification
TokenUsageBreakdown=V2ThreadTokenUsageUpdatedNotification__TokenUsageBreakdown
TokenUsage=V2ThreadTokenUsageUpdatedNotification__ThreadTokenUsage
ThreadTokenUsageUpdated=V2ThreadTokenUsageUpdatedNotification
ThreadSettings=V2ThreadSettingsUpdatedNotification__ThreadSettings
ThreadSettingsUpdated=V2ThreadSettingsUpdatedNotification
ModelRerouted=V2ModelReroutedNotification
McpStartupStatusUpdated=V2McpServerStatusUpdatedNotification
HookRun=V2HookCompletedNotification__HookRunSummary
HookCompleted=V2HookCompletedNotification
ThreadGoalUpdated=V2ThreadGoalUpdatedNotification
ErrorNotification=V2ErrorNotification
AccountRateLimitsUpdated=V2AccountRateLimitsUpdatedNotification
CommandApprovalParams=CommandExecutionRequestApprovalParams
FileChangeApprovalParams=FileChangeRequestApprovalParams
PermissionsApprovalParams=PermissionsRequestApprovalParams
ApprovalResponse=CommandExecutionRequestApprovalResponse
PermissionsApprovalResponse=PermissionsRequestApprovalResponse
UserInputOption=ToolRequestUserInputParams__ToolRequestUserInputOption
UserInputQuestion=ToolRequestUserInputParams__ToolRequestUserInputQuestion
UserInputParams=ToolRequestUserInputParams
UserInputAnswer=ToolRequestUserInputResponse__ToolRequestUserInputAnswer
UserInputResponse=ToolRequestUserInputResponse
ElicitationParams=McpServerElicitationRequestParams
ElicitationResponse=McpServerElicitationRequestResponse
LegacyExecApprovalParams=ExecCommandApprovalParams
LegacyPatchApprovalParams=ApplyPatchApprovalParams
FileChangeApprovalResponse=FileChangeRequestApprovalResponse
LegacyExecApprovalResponse=ExecCommandApprovalResponse
LegacyPatchApprovalResponse=ApplyPatchApprovalResponse
RequestResolved=V2ServerRequestResolvedNotification
FunctionCallOutputBody=V2ThreadReadResponse__FunctionCallOutputBody
""".strip().splitlines())


def split(text, delimiter):
    result, start, depth, quote, escape = [], 0, 0, None, False
    for i, c in enumerate(text):
        if quote:
            if escape:
                escape = False
            elif c == "\\":
                escape = True
            elif c == quote:
                quote = None
        elif c in ["\"", "'"]:
            quote = c
        elif c in "<{[(":
            depth += 1
        elif c in ">}])":
            depth -= 1
        elif c == delimiter and depth == 0:
            result.append(text[start:i].strip())
            start = i + 1
    result.append(text[start:].strip())
    return [x for x in result if x]


def fields(body):
    body = body.strip().removesuffix(";")
    result = []
    for part in split(body[1:-1], ";"):
        match = re.fullmatch(r"\s*readonly ([\w$]+)(\?)?:\s*(.*?)\s*", part, re.S)
        assert match, repr(part)
        result.append(match.groups())
    return result


def sample(text, full=False, depth=0):
    assert depth < 40, text
    text = text.strip().removesuffix(";")
    intersections = split(text, "&")
    if len(intersections) > 1:
        value = {}
        for part in intersections:
            value.update(sample(part, full, depth + 1))
        return value
    branches = split(text, "|")
    if len(branches) > 1:
        return sample(branches[0], full, depth + 1)
    if text in DEFS:
        return sample(DEFS[text], full, depth + 1)
    if text == "string":
        return "probe"
    if text == "boolean":
        return True
    if text == "number":
        return 7
    if text == "null":
        return None
    if text == "Schema.Json":
        return {"probe": True}
    if text.startswith('"'):
        return json.loads(text)
    if text.startswith("ReadonlyArray<"):
        return []
    if text.startswith("{"):
        record = re.fullmatch(r"\{\s*readonly \[\w+: string\]:\s*(.*?)\s*\}", text, re.S)
        if record:
            return {"probe": sample(record[1], full, depth + 1)}
        return {n: sample(t, full, depth + 1) for n, opt, t in fields(text) if full or not opt}
    raise ValueError(text)


ADAPTER_CASES = {
    "Empty": [{}],
    "BackgroundTerminalsListParams": [{"threadId": "t"}, {"threadId": "t", "cursor": "next"}],
    "BackgroundTerminal": [{"processId": "p"}],
    "BackgroundTerminalsListResponse": [{"data": [{"processId": "p"}], "nextCursor": None}, {"data": [], "nextCursor": "next"}],
    "BackgroundTerminalsTerminateParams": [{"threadId": "t", "processId": "p"}],
    "BackgroundTerminalsTerminateResponse": [{"terminated": True}, {"terminated": False}],
    "ThreadSampleParams": [{"threadId": "t", "expectedTurnId": "u", "prompt": "sample", "effort": "low"}],
    "ThreadSampleResponse": [{"text": None}, {"text": "sample"}],
}


def main():
    rust = (CRATE / "src/types.rs").read_text()
    names = re.findall(r"dto!\((\w+)", rust) + re.findall(r"open_union! \{ (\w+)", rust)
    names += ["CollaborationSettings", "RateLimitSnapshot", "ThreadItem", "RequestResolved", "FunctionCallOutputBody"]
    cases, registry, omitted = {}, {}, []
    for name in names:
        schema = ALIASES.get(name) or next((n for n in ["V2" + name, "V1" + name] if n in DEFS), None)
        if not schema:
            assert name in ADAPTER_CASES, f"Unregistered ported type: {name}"
            omitted.append(name)
            cases[name] = ADAPTER_CASES[name]
            registry[name] = "CodexAdapterV2.ts (raw extension)" if name != "Empty" else "Empty RPC result object"
            continue
        records = []
        for branch in split(DEFS[schema].removesuffix(";"), "|"):
            try:
                for full in [False, True]:
                    value = sample(branch, full)
                    if name == "ItemBody":
                        value.pop("id", None)
                    if value not in records:
                        records.append(value)
                if branch.startswith("{") and not re.match(r"^\{\s*readonly \[", branch):
                    for field, optional, ty in fields(branch):
                        if optional and ("null" in split(ty, "|") or ty == "Schema.Json"):
                            value = sample(branch, False)
                            value[field] = None
                            if name == "ItemBody":
                                value.pop("id", None)
                            if value not in records:
                                records.append(value)
            except Exception as error:
                raise ValueError(f"{name} ({schema}): {error}") from error
        cases[name], registry[name] = records, schema
    (CRATE / "tests/schema_cases.json").write_text(json.dumps(cases, indent=2) + "\n")
    fingerprint = 0xcbf29ce484222325
    for byte in SOURCE.read_text().encode():
        fingerprint = ((fingerprint ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    (CRATE / "tests/schema_sources.json").write_text(json.dumps({"schema_fnv1a": f"{fingerprint:016x}", "types": registry}, indent=2) + "\n")
    prelude = '''#![allow(clippy::unwrap_used)]
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeSet;
use t3_codex_app_server::*;
#[path = "support/audit.rs"]
mod audit;
use audit::{Audit, numbers};

fn check<T: DeserializeOwned + Serialize + Audit>(name: &str, cases: &Value) {
    for (i, original) in cases[name].as_array().unwrap().iter().enumerate() {
        let typed: T = serde_json::from_value(original.clone()).unwrap_or_else(|e| panic!("{name} sample {i}: {e}"));
        let mut extra = BTreeSet::new();
        typed.audit(&mut extra);
        assert!(extra.is_empty(), "{name} sample {i}: {extra:?}");
        assert_eq!(numbers(serde_json::to_value(typed).unwrap()), numbers(original.clone()), "{name} sample {i}");
    }
}

#[test]
fn schema_fields_nullability_and_all_union_variants_round_trip() {
    let cases: Value = serde_json::from_str(include_str!("schema_cases.json")).unwrap();
    let sources: Value = serde_json::from_str(include_str!("schema_sources.json")).unwrap();
    let schema = include_str!("../../../../packages/effect-codex-app-server/src/_generated/schema.gen.ts").replace("\\r\\n", "\\n");
    let fingerprint = schema.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
    assert_eq!(format!("{fingerprint:016x}"), sources["schema_fnv1a"], "Regenerate schema cases after reviewing schema drift");
'''
    calls = "".join(f'    check::<{name}>("{name}", &cases);\n' for name in registry)
    (CRATE / "tests/schema_parity.rs").write_text(prelude + calls + "}\n")
    print(f"{len(registry)} types, {sum(map(len, cases.values()))} samples; adapter-only: {omitted}")


if __name__ == "__main__":
    main()
