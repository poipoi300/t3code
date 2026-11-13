#![cfg(feature = "acp-adapter")]

use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};
use t3_orchestration::{
    adapters::acp::AcpAdapter,
    api::Clock,
    error::Result,
    ports::acp_replay::{
        adapters::acp_adapter_v2_testkit::make_turn_input,
        testkit::provider_replay_harness::{AcpFlavorAdapterFactory, ProviderReplayHarness},
    },
    provider::*,
    testing::{ManualClock, SequenceIds, TEST_TIME, replay::ReplayGate},
};
use tokio_util::sync::CancellationToken;

const SESSION: &str = "fx-session-01";
const DEFAULT_MODEL: &str = "anthropic/claude-opus-5.5";
const NAMED: &str = "anthropic/claude-sonnet-5.5";
const CUSTOM: &str = "openai/gpt-6.1-sol";
const CODEX: &str = "gpt-6.1-sol";
const OLD_MODES: &[&str] = &["code", "ask"];
const NEW_MODES: &[&str] = &["auto", "ask", "full-access"];
fn outbound(method: &str, params: Value) -> Value {
    json!({"type":"expect_outbound","label":method,"frame":{"kind":"request","method":method,"params":params}})
}
fn answer(method: &str, result: Value) -> Value {
    json!({"type":"emit_inbound","label":format!("{method}.result"),"frame":{"kind":"response","method":method,"result":result}})
}
fn select(id: &str, category: &str, current: &str, values: &[&str]) -> Value {
    json!({"id":id,"name":id,"category":category,"type":"select","currentValue":current,"options":values.iter().map(|value|json!({"value":value,"name":value})).collect::<Vec<_>>()})
}
fn config(model: &str, modes: &[&str], mode: &str, models: Option<&[&str]>) -> Value {
    json!([
        select(
            "provider",
            "model",
            "gateway",
            &["gateway", "codex", "grok"]
        ),
        select(
            "model",
            "model",
            model,
            models.unwrap_or(&[NAMED, DEFAULT_MODEL, "openai/gpt-5.6-sol"])
        ),
        select("mode", "mode", mode, modes),
        select(
            "effort",
            "thought_level",
            "xhigh",
            &["auto", "low", "medium", "high", "xhigh", "max"]
        ),
    ])
}
fn session(model: &str, modes: &[&str], models: Option<&[&str]>) -> Value {
    json!({"sessionId":SESSION,"configOptions":config(model,modes,"ask",models),"modes":{"currentModeId":"ask","availableModes":modes.iter().map(|id|json!({"id":id,"name":id})).collect::<Vec<_>>()}})
}
fn initialize() -> Vec<Value> {
    vec![
        outbound("initialize", json!("<any>")),
        answer(
            "initialize",
            json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true,"audio":false,"embeddedContext":true}},"agentInfo":{"name":"fx","title":"fx","version":"0.0.13"},"authMethods":[]}),
        ),
    ]
}
fn set_config(frames: &mut Vec<Value>, id: &str, value: &str, after: Value) {
    frames.push(outbound(
        "session/set_config_option",
        json!({"sessionId":SESSION,"configId":id,"value":value}),
    ));
    frames.push(answer(
        "session/set_config_option",
        json!({"configOptions":after}),
    ));
}
fn fx_cases() -> Vec<Value> {
    let mut cases = Vec::new();
    for (name, modes, written) in [
        ("auto-0-0-13", OLD_MODES, "code"),
        ("auto-main", NEW_MODES, "auto"),
    ] {
        let mut frames = initialize();
        frames.push(outbound(
            "session/new",
            json!({"cwd":"<workspace>","mcpServers":"<any>"}),
        ));
        frames.push(answer("session/new", session(DEFAULT_MODEL, modes, None)));
        set_config(
            &mut frames,
            "mode",
            written,
            config(DEFAULT_MODEL, modes, written, None),
        );
        cases.push(json!({"scenario":name,"runtimeMode":"auto","frames":frames}));
    }
    let mut frames = initialize();
    frames.push(outbound("session/load", json!("<any>")));
    frames.push(answer("session/load", session(NAMED, OLD_MODES, None)));
    set_config(
        &mut frames,
        "model",
        DEFAULT_MODEL,
        config(DEFAULT_MODEL, OLD_MODES, "ask", None),
    );
    set_config(
        &mut frames,
        "mode",
        "ask",
        config(DEFAULT_MODEL, OLD_MODES, "ask", None),
    );
    cases.push(json!({"scenario":"default-model","runtimeMode":"approval-required","model":"default","loadSessionId":SESSION,"frames":frames}));
    let mut frames = initialize();
    frames.push(outbound("session/load", json!("<any>")));
    frames.push(answer(
        "session/load",
        session(CODEX, OLD_MODES, Some(&[CODEX])),
    ));
    set_config(
        &mut frames,
        "mode",
        "ask",
        config(CODEX, OLD_MODES, "ask", Some(&[CODEX])),
    );
    cases.push(json!({"scenario":"default-other-backend","runtimeMode":"approval-required","model":"default","loadSessionId":SESSION,"frames":frames}));
    let mut frames = initialize();
    frames.push(outbound("session/new", json!("<any>")));
    frames.push(answer(
        "session/new",
        session(DEFAULT_MODEL, OLD_MODES, None),
    ));
    set_config(
        &mut frames,
        "model",
        CUSTOM,
        config(CUSTOM, OLD_MODES, "ask", None),
    );
    set_config(
        &mut frames,
        "mode",
        "ask",
        config(CUSTOM, OLD_MODES, "ask", None),
    );
    set_config(
        &mut frames,
        "model",
        DEFAULT_MODEL,
        config(DEFAULT_MODEL, OLD_MODES, "ask", None),
    );
    frames.push(outbound("session/prompt", json!("<any>")));
    frames.push(answer("session/prompt", json!({"stopReason":"end_turn"})));
    cases.push(json!({"scenario":"custom-model","runtimeMode":"approval-required","model":CUSTOM,"turnModel":"default","frames":frames}));
    cases
}

pub async fn run_fx_adapter_model_cases(factory: &dyn AcpFlavorAdapterFactory) -> Result<()> {
    for case in fx_cases() {
        let name = case["scenario"].as_str().expect("case name");
        let workspace = tempfile::tempdir()?;
        let instance = ProviderInstanceId(format!("fx-{name}"));
        let mut base = AcpAdapter::replay(
            instance.clone(),
            Arc::new(ManualClock::new(TEST_TIME)),
            Arc::new(SequenceIds::new(name)),
            false,
        );
        base.driver = ProviderDriverKind("fx".into());
        let transcript = serde_json::from_value(
            json!({"provider":"fx","protocol":"acp.ndjson-jsonrpc","version":"1","scenario":name,"entries":case["frames"]}),
        )?;
        let harness = ProviderReplayHarness::with_flavor_factory(
            transcript,
            base,
            ReplayGate::default(),
            factory,
        )
        .await?;
        let selection = |model: &str| {
            serde_json::from_value::<t3_contracts::model_selection::ModelSelection>(
                json!({"instanceId":instance,"model":model,"options":[{"id":"effort","value":"xhigh"}]}),
            )
        };
        let model = selection(case["model"].as_str().unwrap_or("default"))?;
        let policy = RuntimePolicy {
            runtime_mode: serde_json::from_value(case["runtimeMode"].clone())?,
            interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode::Default,
            cwd: Some(workspace.path().into()),
            approval_policy: None,
            sandbox_policy: None,
            max_runtime_mode: None,
        };
        let thread_id = ThreadId(format!("thread-fx-{name}"));
        let runtime = harness
            .adapter
            .open_session(
                OpenSession {
                    thread_id: thread_id.clone(),
                    provider_session_id: ProviderSessionId(format!("provider-session-fx-{name}")),
                    model_selection: model.clone(),
                    runtime_policy: policy.clone(),
                    prompt_suggestions: None,
                    resume_from_session: None,
                    initial_native_thread_id: case["loadSessionId"].as_str().map(str::to_owned),
                    initial_provider_item_identity_version: None,
                },
                CancellationToken::new(),
            )
            .await?;
        let result = async {
            if let Some(turn_model) = case["turnModel"].as_str() {
                let mut events = runtime.subscribe().await?;
                let thread = runtime
                    .ensure_thread(EnsureThread {
                        thread_id,
                        title: None,
                        model_selection: model,
                        runtime_policy: policy.clone(),
                        provider_session_id: None,
                        existing_provider_thread: None,
                        force_new_native_thread: false,
                    })
                    .await?;
                let input = make_turn_input(
                    thread,
                    selection(turn_model)?,
                    policy,
                    ManualClock::new(TEST_TIME).now(),
                    1,
                    "Say hello",
                )?;
                runtime.start_turn(input).await?;
                loop {
                    let event = events
                        .events
                        .recv()
                        .await
                        .ok_or(ProviderError::SubscriptionClosed)??;
                    if matches!(event.kind, ProviderEventKind::Terminal { .. }) {
                        break;
                    }
                }
            }
            Ok::<_, t3_orchestration::error::Error>(())
        }
        .await;
        let close = runtime.close().await;
        result?;
        close?;
        harness.assert_complete()?;
    }
    Ok(())
}

#[tokio::test]
async fn fx_model_scripts_equal_the_typescript_adapter_cases()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut options = t3_process::ProcessOptions::new("node");
    options.args=vec!["--input-type=module".into(),"-e".into(),r#"
import {readFileSync} from 'node:fs'; import {stripTypeScriptTypes} from 'node:module'; import {resolve} from 'node:path';
const source=readFileSync(resolve(process.argv[1],'apps/server/src/orchestration-v2/Adapters/FxAdapterV2.test.ts'),'utf8');
const builders=source.slice(source.indexOf('const SESSION_ID'),source.indexOf('const openFxSession'));
const cases=source.slice(source.indexOf('describe("FxAdapterV2"'));
const result=[];const chain={pipe(){return chain}};
new Function('Schema','FxSettings','openFxSession','describe','it','Effect','testLayer',stripTypeScriptTypes(builders+'\n'+cases))(
 {decodeSync:()=>()=>({})},{},input=>{result.push(input);return chain},(_name,body)=>body(),{effect:(_name,body)=>body()},
 {forEach:(values,body)=>{values.forEach(body);return chain},provide:()=>undefined},{});
process.stdout.write(JSON.stringify(result));
"#.into(),std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").into_os_string()];
    let output = t3_process::run(options).await?;
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(expected.len(), 5);
    assert_eq!(fx_cases(), expected);
    Ok(())
}
