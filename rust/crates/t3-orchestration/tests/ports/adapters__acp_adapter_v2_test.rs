#![cfg(feature = "acp-adapter")]

use serde_json::json;
use std::sync::Arc;
use t3_contracts::{
    base_schemas::*,
    model_selection::ModelSelection,
    orchestration_v2::*,
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderApprovalDecision, ProviderInteractionMode, RuntimeMode},
};
use t3_orchestration::{
    adapters::acp::*,
    api::Clock,
    ports::acp_replay::adapters::acp_adapter_v2_testkit::make_turn_input,
    provider::*,
    testing::{ManualClock, SequenceIds, TEST_TIME},
};
use tokio_util::sync::CancellationToken;

fn policy() -> RuntimePolicy {
    RuntimePolicy {
        runtime_mode: RuntimeMode::ApprovalRequired,
        interaction_mode: ProviderInteractionMode::Default,
        cwd: Some(std::path::PathBuf::from(".")),
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    }
}
fn selection(model: &str) -> ModelSelection {
    serde_json::from_value(json!({"instanceId":"replay","model":model})).unwrap()
}
async fn runtime(
    model: &str,
) -> (
    Arc<dyn SessionRuntime>,
    EventSubscription,
    OrchestrationV2ProviderThread,
) {
    let adapter = AcpAdapter::replay(
        ProviderInstanceId("replay".into()),
        Arc::new(ManualClock::new(TEST_TIME)),
        Arc::new(SequenceIds::new("native")),
        false,
    );
    open_with(adapter, model).await
}
async fn open_with(
    adapter: AcpAdapter,
    model: &str,
) -> (
    Arc<dyn SessionRuntime>,
    EventSubscription,
    OrchestrationV2ProviderThread,
) {
    open_with_identity(adapter, model, None, None).await
}
async fn open_with_identity(
    adapter: AcpAdapter,
    model: &str,
    native_thread_id: Option<&str>,
    identity_version: Option<u8>,
) -> (
    Arc<dyn SessionRuntime>,
    EventSubscription,
    OrchestrationV2ProviderThread,
) {
    let runtime = adapter
        .open_session(
            OpenSession {
                thread_id: ThreadId("thread:test".into()),
                provider_session_id: ProviderSessionId("session:test".into()),
                model_selection: selection(model),
                runtime_policy: policy(),
                prompt_suggestions: None,
                resume_from_session: None,
                initial_native_thread_id: native_thread_id.map(str::to_owned),
                initial_provider_item_identity_version: identity_version,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let events = runtime.subscribe().await.unwrap();
    let thread = runtime
        .ensure_thread(EnsureThread {
            thread_id: ThreadId("thread:test".into()),
            title: None,
            model_selection: selection(model),
            runtime_policy: policy(),
            provider_session_id: None,
            existing_provider_thread: None,
            force_new_native_thread: false,
        })
        .await
        .unwrap();
    (runtime, events, thread)
}

#[tokio::test(start_paused = true)]
async fn multi_turn_replay_cycles_and_keeps_distinct_message_and_tool_identities() {
    let (runtime, mut events, mut thread) = runtime("multi-turn").await;
    let mut replies = Vec::new();
    for ordinal in 1..=3 {
        runtime
            .start_turn(
                make_turn_input(
                    thread.clone(),
                    selection("multi-turn"),
                    policy(),
                    ManualClock::new(TEST_TIME).now(),
                    ordinal,
                    "prompt",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut reply = String::new();
        loop {
            match events.events.recv().await.unwrap().unwrap().kind {
                ProviderEventKind::Message(message) if !message.streaming => reply = message.text,
                ProviderEventKind::Thread {
                    provider_thread, ..
                } => thread = *provider_thread,
                ProviderEventKind::Terminal { outcome, .. } => {
                    assert!(matches!(outcome, TerminalOutcome::Completed { .. }));
                    break;
                }
                _ => {}
            }
        }
        replies.push(reply);
    }
    assert!(replies[0].contains("first fixture turn complete"));
    assert!(replies[1].contains("second fixture turn complete"));
    assert_eq!(replies[2], replies[0]);
    let snapshot = runtime.snapshot(thread).await.unwrap();
    assert_eq!(snapshot.provider_turns.len(), 3);
    let messages: std::collections::BTreeSet<_> = snapshot
        .messages
        .iter()
        .map(|message| serde_json::to_value(&message.id).unwrap().to_string())
        .collect();
    assert_eq!(messages.len(), 3);
    runtime.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn resumed_identity_version_survives_provider_thread_metadata() {
    let adapter = AcpAdapter::replay(
        ProviderInstanceId("replay".into()),
        Arc::new(ManualClock::new(TEST_TIME)),
        Arc::new(SequenceIds::new("identity")),
        false,
    );
    let (runtime, _events, thread) =
        open_with_identity(adapter, "default", Some("legacy-session"), None).await;
    assert_eq!(
        serde_json::to_value(&thread).unwrap()["nativeMetadata"],
        serde_json::Value::Null
    );
    runtime.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn ordered_id_only_permissions_and_client_effects_cross_the_security_seams() {
    use t3_orchestration::testing::replay::*;
    let records = [
        json!({"type":"transcript_start","provider":"acpRegistry","protocol":"acp.ndjson-jsonrpc","version":"1","scenario":"security"}),
        json!({"type":"expect_outbound","frame":{"kind":"request","method":"initialize","params":"<any>"}}),
        json!({"type":"emit_inbound","frame":{"kind":"response","method":"initialize","result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}}),
        json!({"type":"expect_outbound","frame":{"kind":"request","method":"session/new","params":"<any>"}}),
        json!({"type":"emit_inbound","frame":{"kind":"response","method":"session/new","result":{"sessionId":"security-session"}}}),
        json!({"type":"expect_outbound","frame":{"kind":"request","method":"session/prompt","params":"<any>"}}),
        json!({"type":"emit_inbound","frame":{"kind":"notification","method":"session/update","params":{"sessionId":"security-session","update":{"sessionUpdate":"tool_call","toolCallId":"exec","kind":"execute","status":"pending","title":"Run command"}}}}),
        json!({"type":"emit_inbound","frame":{"kind":"request","method":"session/request_permission","params":{"sessionId":"security-session","toolCall":{"toolCallId":"exec"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},{"optionId":"deny","name":"Deny","kind":"reject_once"}]}}}),
        json!({"type":"expect_outbound","frame":{"kind":"response","method":"session/request_permission","result":{"outcome":{"outcome":"selected","optionId":"deny"}}}}),
        json!({"type":"emit_inbound","frame":{"kind":"request","method":"fs/write_text_file","params":{"sessionId":"security-session","path":"C:/not-created-by-replay.txt","content":"must not write"}}}),
        json!({"type":"expect_outbound","frame":{"kind":"response","method":"fs/write_text_file","error":{"code":-32000,"message":"Client operation fs/write_text_file is not enabled"}}}),
        json!({"type":"emit_inbound","frame":{"kind":"request","method":"terminal/create","params":{"sessionId":"security-session","command":"never-run-this-command","args":[]}}}),
        json!({"type":"expect_outbound","frame":{"kind":"response","method":"terminal/create","error":{"code":-32000,"message":"Client operation terminal/create is not enabled"}}}),
        json!({"type":"emit_inbound","frame":{"kind":"response","method":"session/prompt","result":{"stopReason":"end_turn","usage":{"inputTokens":23,"outputTokens":7,"cachedReadTokens":5,"cachedWriteTokens":2,"thoughtTokens":3}}}}),
        json!({"type":"runtime_exit","status":"success"}),
    ];
    let transcript = decode_ndjson(
        &records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let (status, progress) = tokio::sync::watch::channel(ReplayStatus {
        scenario: "security".into(),
        cursor: 0,
        total: transcript.entries.len(),
        failure: None,
    });
    let mut adapter = AcpAdapter::replay(
        ProviderInstanceId("replay".into()),
        Arc::new(ManualClock::new(TEST_TIME)),
        Arc::new(SequenceIds::new("security")),
        false,
    );
    adapter.transport = Arc::new(StrictReplayTransportFactory {
        transcript,
        status,
        gate: ReplayGate::default(),
    });
    let (runtime, mut events, thread) = open_with(adapter, "default").await;
    runtime
        .start_turn(
            make_turn_input(
                thread,
                selection("default"),
                policy(),
                ManualClock::new(TEST_TIME).now(),
                1,
                "prompt",
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let mut responder = None;
    let mut completed_usage = None;
    loop {
        match events.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::Turn { provider_turn, .. }
                if serde_json::to_value(&provider_turn).unwrap()["status"] == "completed" =>
            {
                completed_usage =
                    Some(serde_json::to_value(&provider_turn).unwrap()["turnTokenUsage"].clone());
            }
            ProviderEventKind::RuntimeRequest { request, .. }
                if request.status == RuntimeRequestStatus::Pending =>
            {
                let request = serde_json::to_value(request).unwrap();
                assert_eq!(request["kind"], "command");
                let id = request["id"].as_str().unwrap().to_owned();
                let runtime = runtime.clone();
                responder = Some(tokio::spawn(async move {
                    runtime
                        .respond(RuntimeResponse {
                            request_id: RuntimeRequestId(id),
                            decision: Some(ProviderApprovalDecision::Decline),
                            feedback: None,
                            answers: None,
                        })
                        .await
                }));
            }
            ProviderEventKind::Terminal { outcome, .. } => {
                assert!(matches!(outcome, TerminalOutcome::Completed { .. }));
                break;
            }
            _ => {}
        }
    }
    responder.unwrap().await.unwrap().unwrap();
    assert_eq!(
        completed_usage,
        Some(
            json!({"usageScope":"main_agent","usageStatus":"complete","hasSubagents":false,"inputTokens":23,"outputTokens":7,"cachedInputTokens":5,"cacheCreationTokens":2,"reasoningTokens":3})
        )
    );
    progress.borrow().assert_complete().unwrap();
    runtime.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn replay_plays_showcase_through_real_acp_and_waits_for_permission() {
    let (runtime, mut events, thread) = runtime("showcase").await;
    let input = make_turn_input(
        thread.clone(),
        selection("showcase"),
        policy(),
        ManualClock::new(TEST_TIME).now(),
        1,
        "showcase",
    )
    .unwrap();
    runtime.start_turn(input).await.unwrap();
    let mut requests = 0;
    let mut plans = 0;
    let mut final_text = String::new();
    loop {
        match events.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::Plan(_) => plans += 1,
            ProviderEventKind::RuntimeRequest { request, .. }
                if request.status == RuntimeRequestStatus::Pending =>
            {
                requests += 1;
                let id = serde_json::to_value(&request).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let runtime = runtime.clone();
                // The response waits for the native write; keep consuming its resolution event.
                let response = tokio::spawn(async move {
                    runtime
                        .respond(RuntimeResponse {
                            request_id: RuntimeRequestId(id),
                            decision: Some(ProviderApprovalDecision::Accept),
                            feedback: None,
                            answers: None,
                        })
                        .await
                });
                while !matches!(events.events.recv().await.unwrap().unwrap().kind, ProviderEventKind::RuntimeRequest { request, .. } if request.status == RuntimeRequestStatus::Resolved)
                {
                }
                response.await.unwrap().unwrap();
            }
            ProviderEventKind::Message(message) if !message.streaming => {
                final_text.push_str(&message.text)
            }
            ProviderEventKind::Terminal { outcome, .. } => {
                assert!(matches!(outcome, TerminalOutcome::Completed { .. }));
                break;
            }
            _ => {}
        }
    }
    assert_eq!(requests, 1);
    assert!(plans >= 3);
    assert!(final_text.contains("clamp"));
    let snapshot = runtime.snapshot(thread).await.unwrap();
    assert_eq!(snapshot.provider_turns.len(), 1);
    assert!(snapshot.messages.iter().all(|message| !message.streaming));
    runtime.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn rejecting_approval_ends_recording_before_later_tools() {
    let (runtime, mut events, thread) = runtime("showcase").await;
    let input = make_turn_input(
        thread,
        selection("showcase"),
        policy(),
        ManualClock::new(TEST_TIME).now(),
        1,
        "showcase",
    )
    .unwrap();
    runtime.start_turn(input).await.unwrap();
    let mut responder = None;
    let mut saw_test_tool = false;
    loop {
        match events.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::RuntimeRequest { request, .. }
                if request.status == RuntimeRequestStatus::Pending =>
            {
                let id = serde_json::to_value(request).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let runtime = runtime.clone();
                responder = Some(tokio::spawn(async move {
                    runtime
                        .respond(RuntimeResponse {
                            request_id: RuntimeRequestId(id),
                            decision: Some(ProviderApprovalDecision::Decline),
                            feedback: None,
                            answers: None,
                        })
                        .await
                }));
            }
            ProviderEventKind::TurnItem { item, .. } => {
                saw_test_tool |= serde_json::to_value(item).unwrap()["input"] == "vp test run"
            }
            ProviderEventKind::Terminal { outcome, .. } => {
                assert!(matches!(outcome, TerminalOutcome::Completed { .. }));
                break;
            }
            _ => {}
        }
    }
    responder.unwrap().await.unwrap().unwrap();
    assert!(!saw_test_tool);
    runtime.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn replay_question_types_answers_and_cancelled_approvals_interrupt() {
    for model in ["question", "showcase"] {
        let (runtime, mut events, thread) = runtime(model).await;
        runtime
            .start_turn(
                make_turn_input(
                    thread,
                    selection(model),
                    policy(),
                    ManualClock::new(TEST_TIME).now(),
                    1,
                    "prompt",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut responder = None;
        let mut question = false;
        loop {
            match events.events.recv().await.unwrap().unwrap().kind {
                ProviderEventKind::RuntimeRequest { request, .. }
                    if request.status == RuntimeRequestStatus::Pending =>
                {
                    let id = serde_json::to_value(request).unwrap()["id"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let runtime = runtime.clone();
                    responder = Some(tokio::spawn(async move {
                        runtime.respond(RuntimeResponse { request_id: RuntimeRequestId(id), decision: (model == "showcase").then_some(ProviderApprovalDecision::Cancel), feedback: None, answers: (model == "question").then(||serde_json::from_value(json!({"schema_vs_ui_flexibility":"Strict schemas (Recommended)"})).unwrap()) }).await
                    }));
                }
                ProviderEventKind::TurnItem { item, .. } => {
                    question |= serde_json::to_value(item).unwrap()["type"] == "user_input_request"
                }
                ProviderEventKind::Terminal { outcome, .. } => {
                    assert_eq!(
                        matches!(outcome, TerminalOutcome::Interrupted { .. }),
                        model == "showcase"
                    );
                    break;
                }
                _ => {}
            }
        }
        responder.unwrap().await.unwrap().unwrap();
        assert_eq!(question, model == "question");
        runtime.close().await.unwrap();
    }
}

#[test]
fn form_keeps_titled_multiselect_companions_and_required_types() {
    let form = ElicitationForm::parse(&json!({"properties":{"areas":{"type":"array","title":"Areas","items":{"anyOf":[{"const":"schemas","title":"Strict schemas","description":"Typed wire shapes"},{"const":"ui","title":"UI flexibility"}]}},"choice":{"type":"string","enum":["a","b"]},"choice__other":{"type":"string"},"count":{"type":"integer"}},"required":["areas","count"]}), "Question").unwrap();
    assert_eq!(form.questions.as_array().unwrap().len(), 3);
    assert_eq!(form.questions[0]["multiSelect"], true);
    assert_eq!(form.questions[0]["options"][0]["value"], "schemas");
    assert!(
        form.content(&json!({"areas":["schemas"],"count":"1.5"}))
            .is_none()
    );
    assert_eq!(
        form.content(
            &json!({"areas":["ui","ui","unknown"],"count":"2","choice":"custom","unknown":"drop"})
        )
        .unwrap(),
        json!({"areas":["ui"],"count":2.0,"choice__other":"custom"})
    );
    assert!(
        ElicitationForm::parse(&json!({"properties":{" x ":{"type":"string"}}}), "Question")
            .is_none()
    );
}

#[test]
fn permission_policy_reads_and_mutations_follow_explicit_overrides() {
    let mut policy = policy();
    assert_eq!(
        permission_disposition(&policy, "read", None),
        PermissionDisposition::Allow
    );
    assert_eq!(
        permission_disposition(&policy, "execute", None),
        PermissionDisposition::Ask
    );
    policy.approval_policy = Some(json!("never"));
    assert_eq!(
        permission_disposition(&policy, "edit", None),
        PermissionDisposition::Deny
    );
    policy.runtime_mode = RuntimeMode::AutoAcceptEdits;
    policy.approval_policy = None;
    assert_eq!(
        permission_disposition(&policy, "edit", None),
        PermissionDisposition::Allow
    );
    assert_eq!(
        permission_disposition(&policy, "execute", None),
        PermissionDisposition::Ask
    );
    policy.runtime_mode = RuntimeMode::FullAccess;
    policy.sandbox_policy = Some(json!({"type":"readOnly"}));
    assert_eq!(
        permission_disposition(&policy, "execute", None),
        PermissionDisposition::Deny
    );
    policy.sandbox_policy = Some(json!({"type":"unknown"}));
    assert_eq!(
        permission_disposition(&policy, "read", None),
        PermissionDisposition::Deny
    );
    for invalid_type in [json!(null), json!(1), json!(true), json!({}), json!([])] {
        policy.sandbox_policy = Some(json!({"type":invalid_type}));
        for kind in ["read", "edit", "execute"] {
            assert_eq!(
                permission_disposition(&policy, kind, None),
                PermissionDisposition::Deny,
                "sandbox type {invalid_type}, kind {kind}"
            );
        }
        policy.runtime_mode = RuntimeMode::ApprovalRequired;
        assert_eq!(
            permission_disposition(&policy, "execute", None),
            PermissionDisposition::Ask
        );
        policy.runtime_mode = RuntimeMode::FullAccess;
    }
    let root = tempfile::tempdir().unwrap();
    policy.cwd = None;
    policy.sandbox_policy = Some(json!({"type":"workspaceWrite","writableRoots":[root.path()]}));
    let locations = json!([{"path":format!("  {}  ",root.path().join("new.txt").display())}]);
    assert_eq!(
        permission_disposition(&policy, "edit", Some(&locations)),
        PermissionDisposition::Allow
    );
}

#[tokio::test]
async fn per_turn_usage_matches_the_source_clamping_and_gemini_counter_rules()
-> Result<(), Box<dyn std::error::Error>> {
    use serde_json::Value;
    let samples = [
        Value::Null,
        json!({"inputTokens":2453,"outputTokens":4,"totalTokens":2457,"cachedReadTokens":512,"cachedWriteTokens":32,"thoughtTokens":3}),
        json!({"inputTokens":42,"outputTokens":3,"totalTokens":54,"cachedReadTokens":24,"cachedWriteTokens":0,"thoughtTokens":9}),
        json!({"inputTokens":100,"outputTokens":10,"totalTokens":110,"cachedReadTokens":150,"cachedWriteTokens":120,"thoughtTokens":30}),
        json!({"inputTokens":100,"outputTokens":10,"totalTokens":110}),
        json!({"inputTokens":100,"outputTokens":10,"cachedReadTokens":80,"cachedWriteTokens":80}),
    ];
    let mut cases = Vec::new();
    for usage in samples {
        for status in ["completed", "interrupted", "failed"] {
            for gemini in [false, true] {
                for children in [false, true] {
                    cases.push(
                        json!({"usage":usage,"status":status,"gemini":gemini,"children":children}),
                    );
                }
            }
        }
    }
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {stripTypeScriptTypes} from 'node:module';
import {readFileSync} from 'node:fs';
const source=readFileSync('packages/provider-acp/src/server/adapter.ts','utf8');
const begin=source.indexOf('export function acpPerTurnPromptUsage(');
const end=source.indexOf('export interface AcpAdapterV2Flavor',begin);
const code=stripTypeScriptTypes(source.slice(begin,end));
const {acpPerTurnPromptUsage}=await import('data:text/javascript,'+encodeURIComponent(code));
process.stdout.write(JSON.stringify(JSON.parse(readFileSync(0,'utf8')).map(row=>acpPerTurnPromptUsage(row.usage,row.children,row.status,row.gemini))));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&cases)?);
    let output = t3_process::run(options).await?;
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let actual = acp_per_turn_prompt_usage(
            Some(&case["usage"]),
            case["children"].as_bool().unwrap(),
            case["status"].as_str().unwrap(),
            case["gemini"].as_bool().unwrap(),
        )?;
        assert_eq!(serde_json::to_value(actual)?, expected, "{case}");
    }
    Ok(())
}

#[tokio::test]
async fn acp_diff_previews_match_source_snippets_headers_and_formatter_bound()
-> Result<(), Box<dyn std::error::Error>> {
    use serde_json::Value;
    let snippet = json!({"type":"diff","path":"/repo/a.ts","oldText":"const target = 10;","newText":"const target = 100;","patch":{"format":"snippet","text":"@@ -1,1 +1,1 @@ snippet, line unknown\n"}});
    let mut omitted = snippet.clone();
    omitted["patch"] = json!({"format":"omitted"});
    let cases = vec![
        json!([snippet]),
        json!([omitted]),
        json!([{"type":"content","content":{"type":"text","text":"Edited."}},snippet]),
        json!([snippet,{"type":"diff","path":"/repo/b.ts","oldText":"b\n","newText":"B\n","patch":{"text":"@@ -1 +1 @@\n-b\n+B\n"}}]),
        json!([{"type":"diff","changes":[{"operation":"add","path":"/repo/a.ts"}],"patch":{"format":"git_patch","text":"@@ -0,0 +1 @@\n+content\n"}}]),
        json!([{"type":"diff","changes":[{"operation":"delete","path":"/repo/a.ts"}],"patch":{"format":"git_patch","text":"@@ -1 +0,0 @@\n-content\n"}}]),
        json!([{"type":"diff","path":"/repo/new.txt","oldText":null,"newText":"hello\n"},{"type":"diff","path":"/repo/a.ts","oldText":"a\nb\nc\n","newText":"a\nB\nc\n"},{"type":"diff","path":"/repo/same.ts","oldText":"x\n","newText":"x\n"}]),
        json!([{"type":"diff","path":"/repo/new.txt","oldText":null,"newText":"hello\n"},{"type":"diff","changes":[{"operation":"modify","path":"/repo/a.ts"}],"patch":{"format":"git_patch","text":"@@ -1 +1 @@\n-a\n+A\n"}}]),
    ];
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {stripTypeScriptTypes,createRequire} from 'node:module';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
const require=createRequire(resolve('packages/provider-acp/package.json'));
const diff=require('diff');
const source=readFileSync('packages/provider-acp/src/server/adapter.ts','utf8');
const begin=source.indexOf('export function acpToolCallSnippetEdit('),end=source.indexOf('function pathFromToolCall(',begin);
let code=stripTypeScriptTypes(source.slice(begin,end));
code='const {formatPatch,structuredPatch,FILE_HEADERS_ONLY}=globalThis.__acpDiff;const ACP_V1_DIFF_MAX_EDITS=1000;const unknownRecord=v=>typeof v==="object"&&v!==null&&!Array.isArray(v)?v:undefined;'+code;
globalThis.__acpDiff=diff;
const functions=await import('data:text/javascript,'+encodeURIComponent(code));
const cases=JSON.parse(readFileSync(0,'utf8'));
const calls=[];
for(const content of cases)for(const entry of content) {
 if(entry.type!=='diff'||typeof entry.patch?.text==='string'||typeof entry.path!=='string'||typeof entry.newText!=='string')continue;
 const old=typeof entry.oldText==='string'?entry.oldText:undefined;
 const args=[old===undefined?'/dev/null':entry.path,entry.path,old??'',entry.newText,3,old&&entry.newText?1000:null];
 const patch=diff.structuredPatch(args[0],args[1],args[2],args[3],undefined,undefined,{context:3,maxEditLength:args[5]??Infinity});
 calls.push({args,text:patch?.hunks.length?diff.formatPatch(patch,diff.FILE_HEADERS_ONLY):null});
}
process.stdout.write(JSON.stringify({calls,results:cases.map(content=>({patch:functions.acpToolCallDiffPatch(content)??null,snippet:functions.acpToolCallSnippetEdit(content)??null}))}));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&cases)?);
    let output = t3_process::run(options).await?;
    let expected: Value = serde_json::from_slice(&output.stdout)?;
    struct Formatter(Value);
    impl AcpDiffFormatter for Formatter {
        fn format(
            &self,
            old: &str,
            new: &str,
            before: &str,
            after: &str,
            context: usize,
            max: Option<usize>,
        ) -> ProviderResult<Option<String>> {
            let args = json!([old, new, before, after, context, max]);
            let call = self
                .0
                .as_array()
                .unwrap()
                .iter()
                .find(|call| call["args"] == args)
                .expect("formatter arguments must match the bounded source call");
            Ok(call["text"].as_str().map(str::to_owned))
        }
    }
    let formatter = Formatter(expected["calls"].clone());
    for (case, expected) in cases.iter().zip(expected["results"].as_array().unwrap()) {
        assert_eq!(
            json!(acp_tool_call_diff_patch(case, &formatter)?),
            expected["patch"]
        );
        assert_eq!(json!(acp_tool_call_snippet_edit(case)), expected["snippet"]);
    }
    Ok(())
}
