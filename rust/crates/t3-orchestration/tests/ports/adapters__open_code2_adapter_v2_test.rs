#![cfg(feature = "acp-adapter")]
use serde_json::{Value, json};
use t3_orchestration::ports::acp_replay::adapters::open_code2_adapter_v2_testkit::operation_of;

#[tokio::test]
async fn opencode2_http_routes_equal_the_typescript_replay_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let mut requests = Vec::new();
    let paths = [
        "/api/experimental/job",
        "/api/fs/read/a.png",
        "/api/rpc/id/start",
        "/api/shell",
        "/api/shell/id",
        "/api/shell/id/output",
        "/api/event",
        "/api/model",
        "/api/agent",
        "/api/session",
        "/api/session/active",
        "/api/command",
        "/api/experimental/mcp/x",
        "/api/experimental/session/s/instructions/entries/k",
        "/api/skill",
        "/api/unknown",
    ];
    let rest = [
        "",
        "/prompt",
        "/command",
        "/compact",
        "/interrupt",
        "/model",
        "/agent",
        "/move",
        "/synthetic",
        "/fork",
        "/revert/stage",
        "/revert/commit",
        "/revert",
        "/inbox/a%2Fb%20%C3%A9",
        "/message",
        "/permission",
        "/form",
        "/form/f",
        "/form/f/reply",
        "/permission/r/reply",
    ];
    let mut paths: Vec<String> = paths.iter().map(|path| (*path).into()).collect();
    paths.extend(rest.iter().map(|rest| format!("/api/session/s{rest}")));
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        for path in &paths {
            requests.push(json!({"method":method,"path":path,"query":{"kind":"image","directory":"<workspace>","override":"query"},"body":{"optional":null,"enabled":false,"override":"body"}}));
        }
    }
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {stripTypeScriptTypes} from 'node:module';
import {readFileSync} from 'node:fs';
const source=readFileSync('apps/server/src/orchestration-v2/Adapters/OpenCode2AdapterV2.testkit.ts','utf8');
const begin=source.indexOf('const operationOf =');
const end=source.indexOf('const replayHttpClient =',begin);
const code=stripTypeScriptTypes(source.slice(begin,end).replace('const operationOf =','export const operationOf ='));
const {operationOf}=await import('data:text/javascript,'+encodeURIComponent(code));
const rows=JSON.parse(readFileSync(0,'utf8'));
process.stdout.write(JSON.stringify(rows.map(({method,path,query,body})=>operationOf(method,path,query,body))));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&requests)?);
    let output = t3_process::run(options).await?;
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(expected.len(), requests.len());
    for (request, expected) in requests.iter().zip(expected) {
        assert_eq!(
            operation_of(
                request["method"].as_str().unwrap(),
                request["path"].as_str().unwrap(),
                &request["query"],
                &request["body"]
            )?,
            expected,
            "{request}"
        );
    }
    Ok(())
}

mod resumed_calls {
    use super::*;
    use std::{collections::BTreeMap, sync::Arc};
    use t3_orchestration::ports::acp_replay::adapters::open_code2_adapter_v2_testkit::OpenCode2ReplayServer;
    use t3_orchestration::{
        error::{Error, Result},
        provider::{ProviderEventKind, SessionRuntime, TurnInput},
        testing::replay::ReplayEntry,
    };

    const SESSION: &str = "ses_f148ca2deffeJcwCnRQtb0YFNX";
    const CHILD: &str = "ses_f1485c529ffea4URrYruwEg0Ja";
    const WORK: &str = "/work/opencode2";
    #[derive(serde::Serialize)]
    pub struct ResumedCallCase {
        pub name: &'static str,
        pub replay: Vec<ReplayEntry>,
        pub expected: BTreeMap<String, Value>,
    }
    fn out(kind: &str, input: Value) -> Value {
        json!({"type":"expect_outbound","frame":{"type":kind,"input":input}})
    }
    fn reply(operation: &str, data: Value) -> Value {
        json!({"type":"emit_inbound","frame":{"type":"sdk.response","operation":operation,"data":data}})
    }
    fn event_version(kind: &str, data: Value, version: u64) -> Value {
        json!({"type":"emit_inbound","frame":{"type":"sdk.event","event":{"id":format!("evt_{}0000",kind.replace('.',"")),"created":1,"type":kind,"data":data,"durable":{"aggregateID":SESSION,"seq":1,"version":version}}}})
    }
    fn event(kind: &str, data: Value) -> Value {
        event_version(kind, data, 1)
    }
    fn succeeded(session: &str) -> Value {
        event("session.execution.succeeded", json!({"sessionID":session}))
    }
    fn started() -> Value {
        event("session.execution.started", json!({"sessionID":CHILD}))
    }
    fn returned_running(id: &str) -> Value {
        let mut entry = event_version(
            "session.tool.success",
            json!({"sessionID":SESSION,"assistantMessageID":"msg_assistant","id":id,"content":[{"type":"text","text":"The subagent is working in the background."}],"metadata":{"sessionID":CHILD,"status":"running"},"executed":true}),
            2,
        );
        entry["frame"]["event"]["id"] = json!(format!("evt_toolsuccess_{id}"));
        entry
    }
    fn call_again(id: &str, writes_rules: bool, description: &str) -> Vec<Value> {
        let tool = json!({"sessionID":SESSION,"assistantMessageID":"msg_assistant","id":id});
        let mut started = tool.clone();
        started["name"] = json!("subagent");
        let mut called = started.clone();
        called["input"] = json!({"description":description,"prompt":"also this","sessionID":CHILD,"background":true});
        called["executed"] = json!(false);
        let mut progress = tool;
        progress["metadata"] = json!({"sessionID":CHILD,"status":"running"});
        let mut entries = vec![
            event("session.tool.input.started", started),
            event("session.tool.called", called),
            event("session.tool.progress", progress),
        ];
        if writes_rules {
            entries.extend([
                out(
                    "session.update",
                    json!({"sessionID":CHILD,"permissions":"<any>"}),
                ),
                reply("session.update", Value::Null),
            ]);
        }
        entries.push(returned_running(id));
        entries
    }
    fn child_report(inbox: &str, text: &str, description: &str) -> Value {
        event(
            "session.inbox.enqueued",
            json!({"inboxID":inbox,"sessionID":SESSION,"item":{"type":"synthetic","payload":{"text":format!("<subagent sessionID=\"{CHILD}\" state=\"completed\" description=\"{description}\">\n{text}\n</subagent>"),"description":description,"metadata":{"source":"subagent","childID":CHILD,"agent":"General","state":"completed"}},"delivery":"steer"}}),
        )
    }
    fn child_said(text: &str, run: u64) -> Value {
        event(
            "session.text.ended",
            json!({"sessionID":CHILD,"assistantMessageID":format!("msg_child_{run}"),"ordinal":0,"text":text}),
        )
    }
    fn background_launch() -> Vec<Value> {
        let tool = json!({"sessionID":SESSION,"assistantMessageID":"msg_assistant","id":"call-background"});
        let mut input_started = tool.clone();
        input_started["name"] = json!("subagent");
        let mut called = input_started.clone();
        called["input"] = json!({"description":"Sleep","prompt":"sleep","background":true});
        called["executed"] = json!(false);
        let mut progress = tool;
        progress["metadata"] = json!({"sessionID":CHILD,"status":"running"});
        vec![
            out(
                "session.prompt",
                json!({"sessionID":SESSION,"text":"<any>"}),
            ),
            reply(
                "session.prompt",
                json!({"data":{"id":"msg_0eb735d41001NJee1EvVePJAK5","sessionID":SESSION,"time":{"created":1790656601410u64},"type":"user","payload":{"text":"hi"},"delivery":"steer"}}),
            ),
            event("session.execution.started", json!({"sessionID":SESSION})),
            event("session.tool.input.started", input_started),
            event("session.tool.called", called),
            event(
                "session.created",
                json!({"sessionID":CHILD,"slug":"proud-canyon","version":"2.0.18","projectID":"global","parentID":SESSION,"location":{"directory":WORK},"subpath":"","title":"Sleep","agent":"general","model":{"id":"big-pickle","providerID":"opencode","variant":"default"}}),
            ),
            event("session.tool.progress", progress),
        ]
    }
    pub fn cases() -> Result<Vec<ResumedCallCase>> {
        let mut cases = Vec::new();
        for (name, variant, expected) in [
            (
                "an earlier run's report never arrived",
                0,
                vec![
                    ("call-background", "FIRST_OK"),
                    ("call-again", "FIRST_OK"),
                    ("call-later", "SECOND_OK"),
                ],
            ),
            (
                "both runs' reports arrive after the runs ended",
                1,
                vec![("call-background", "FIRST_OK"), ("call-again", "SECOND_OK")],
            ),
            (
                "two runs with the same answer report late",
                2,
                vec![
                    ("call-background", "DONE"),
                    ("call-again", "DONE"),
                    ("call-later", "THIRD_OK"),
                ],
            ),
            (
                "the call it joined already returned",
                3,
                vec![("call-background", "FIRST_OK"), ("call-again", "CHILD_OK")],
            ),
        ] {
            let mut replay = background_launch();
            if variant != 3 {
                replay.extend([returned_running("call-background"), started()]);
            }
            match variant {
                0 => {
                    replay.extend(call_again("call-again", true, "Again"));
                    replay.extend([child_said("FIRST_OK", 1), succeeded(CHILD)]);
                    replay.extend(call_again("call-later", false, "Again"));
                    replay.extend([
                        started(),
                        child_said("SECOND_OK", 2),
                        succeeded(CHILD),
                        succeeded(SESSION),
                        child_report("msg_report_2", "SECOND_OK", "Again"),
                    ]);
                }
                1 => {
                    replay.push(succeeded(CHILD));
                    replay.extend(call_again("call-again", true, "Sleep"));
                    replay.extend([
                        started(),
                        succeeded(CHILD),
                        child_report("msg_report_1", "FIRST_OK", "Sleep"),
                        child_report("msg_report_2", "SECOND_OK", "Sleep"),
                        succeeded(SESSION),
                    ]);
                }
                2 => {
                    replay.extend([child_said("DONE", 1), succeeded(CHILD)]);
                    replay.extend(call_again("call-again", true, "Again"));
                    replay.extend([
                        started(),
                        child_said("DONE", 2),
                        succeeded(CHILD),
                        child_report("msg_report_1", "DONE", "Sleep"),
                    ]);
                    replay.extend(call_again("call-later", false, "Again"));
                    replay.extend([
                        started(),
                        child_report("msg_report_2", "DONE", "Again"),
                        child_said("THIRD_OK", 3),
                        succeeded(CHILD),
                        succeeded(SESSION),
                        child_report("msg_report_3", "THIRD_OK", "Again"),
                    ]);
                }
                _ => {
                    replay.push(started());
                    replay.extend(call_again("call-again", true, "Again"));
                    replay.extend([succeeded(CHILD),event_version("session.tool.success",json!({"sessionID":SESSION,"assistantMessageID":"msg_assistant","id":"call-background","content":[{"type":"text","text":"FIRST_OK"}],"metadata":{"sessionID":CHILD,"status":"completed"},"executed":true}),2),succeeded(SESSION),child_report("msg_report","CHILD_OK","Again")]);
                }
            }
            cases.push(ResumedCallCase {
                name,
                replay: serde_json::from_value(json!(replay))?,
                expected: expected
                    .into_iter()
                    .map(|(id, result)| (id.into(), json!({"status":"completed","result":result})))
                    .collect(),
            });
        }
        Ok(cases)
    }

    pub struct ResumedCallRuntime {
        pub runtime: Arc<dyn SessionRuntime>,
        pub turn: TurnInput,
        pub server: Arc<OpenCode2ReplayServer>,
    }
    // Local shim: the OpenCode2 owner supplies its native resumed/opening/instruction harness.
    #[async_trait::async_trait]
    pub trait ResumedCallFactory: Send + Sync {
        async fn open(&self, case: &ResumedCallCase) -> Result<ResumedCallRuntime>;
    }
    #[allow(
        dead_code,
        reason = "Native OpenCode2 resumed-session factory has not landed."
    )]
    pub async fn assert_native_resumed_calls(factory: &dyn ResumedCallFactory) -> Result<()> {
        for case in cases()? {
            let opened = factory.open(&case).await?;
            let result = async {
                let mut events = opened.runtime.subscribe().await?;
                let root = opened.turn.provider_thread.id.clone();
                let mut calls = BTreeMap::new();
                let mut roster = None;
                opened.runtime.start_turn(opened.turn).await?;
                loop {
                    match events
                        .events
                        .recv()
                        .await
                        .ok_or(t3_orchestration::provider::ProviderError::SubscriptionClosed)??
                        .kind
                    {
                        ProviderEventKind::Subagent(subagent) => {
                            let subagent = serde_json::to_value(subagent)?;
                            let id = subagent["nativeTaskRef"]["nativeId"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned();
                            calls.insert(
                                id,
                                json!({"status":subagent["status"],"result":subagent["result"]}),
                            );
                        }
                        ProviderEventKind::Thread {
                            provider_thread, ..
                        } if provider_thread.id == root => {
                            roster = provider_thread
                                .pending_background_tasks
                                .as_ref()
                                .and_then(Option::as_ref)
                                .map(Vec::len);
                        }
                        _ => {}
                    }
                    if roster == Some(0)
                        && case.expected.keys().all(|id| {
                            calls
                                .get(id)
                                .is_some_and(|call| call["status"] == "completed")
                        })
                    {
                        break;
                    }
                }
                assert_eq!(calls, case.expected, "{}", case.name);
                Ok::<_, Error>(())
            }
            .await;
            let close = opened.runtime.close().await;
            result?;
            close?;
            opened
                .server
                .controller
                .assert_complete()
                .map_err(|error| Error::Service {
                    operation: "OpenCode2 resumed replay",
                    detail: error.to_string(),
                })?;
        }
        Ok(())
    }
}

#[tokio::test]
async fn resumed_subagent_cases_equal_the_four_upstream_replays()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {stripTypeScriptTypes} from 'node:module';
import {readFileSync} from 'node:fs';
const source=readFileSync('apps/server/src/orchestration-v2/Adapters/OpenCode2AdapterV2.test.ts','utf8');
const between=(a,b)=>{const start=source.indexOf(a);const end=source.indexOf(b,start);if(start<0||end<0)throw new Error(`Missing ${a}`);return source.slice(start,end);};
const constant=name=>{const start=source.indexOf(`const ${name} =`);const end=source.indexOf(';',start);if(start<0||end<0)throw new Error(`Missing ${name}`);return source.slice(start,end+1);};
const start=source.indexOf('it.effect.each([',source.indexOf('const childSaid ='));
const end=source.indexOf('])("settles a resumed subagent',start);
if(start<0||end<0)throw new Error('Missing resumed subagent cases');
const code=[constant('SESSION'),constant('WORK'),constant('CHILD'),between('const out =','const mcpRules ='),between('const PROMPT_ID =','const emptyShellInventory:'),between('const childCreated =','const withLineage ='),between('const returnedRunning =','const watchCalls ='),between('const launchedAndRunning =','it.effect.each([false, true])'),between('const childSaid =','it.effect.each(['),`export const cases = ${source.slice(start+'it.effect.each('.length,end+1)};`].join('\n');
const {cases}=await import('data:text/javascript,'+encodeURIComponent(stripTypeScriptTypes(code)));
process.stdout.write(JSON.stringify(cases));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let output = t3_process::run(options).await?;
    let expected: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(serde_json::to_value(resumed_calls::cases()?)?, expected);
    Ok(())
}
