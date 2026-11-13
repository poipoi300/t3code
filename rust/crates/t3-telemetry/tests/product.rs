use serde_json::{Value, json};
use t3_contracts::{
    model_selection::ModelSelection, orchestration_v2::OrchestrationV2ProviderTurn,
};
use t3_telemetry::{datetime, product::*, provider_usage::*};

fn context(run: &str, segment: &str) -> TurnContext {
    TurnContext {run_id:Some(run.into()),model_selection:serde_json::from_value::<ModelSelection>(json!({"instanceId":"codex","model":"gpt-6-sol","options":[{"id":"reasoningEffort","value":"xhigh"},{"id":"serviceTier","value":"priority"}]})).expect("selection"),runtime_mode:Some("full-access".into()),interaction_mode:Some("default".into()),origin:Some(TurnOrigin{created_by:"user".into(),creation_source:"web".into(),scheduled:false,subagent:false}),segment:Some(segment.into()),restart_continuation:None}
}
fn turn(id: &str, usage: Value) -> OrchestrationV2ProviderTurn {
    serde_json::from_value(json!({"id":id,"providerThreadId":"provider-thread","nodeId":format!("node:{id}"),"runAttemptId":null,"nativeTurnRef":null,"ordinal":1,"status":"completed","startedAt":datetime(1000),"completedAt":datetime(4000),"turnTokenUsage":usage})).expect("turn")
}
fn complete(input: u64, output: u64) -> Value {
    json!({"usageStatus":"complete","usageScope":"main_agent","hasSubagents":false,"inputTokens":input,"outputTokens":output})
}
fn label(model: &str) -> Option<String> {
    catalog_model_label(&[], model)
}

#[test]
fn counts_requests_once_across_replay_continuation_and_late_child() {
    let mut ledger = RequestUsageLedger::default();
    ledger.start("run-a", "codex", context("run-a", "initial"));
    let first = TurnReport {
        driver: "codex".into(),
        provider_turn: turn("root", complete(100, 10)),
        context: Some(context("run-a", "initial")),
    };
    ledger.add(&first);
    ledger.add(&first);
    ledger.add(&TurnReport {
        driver: "codex".into(),
        provider_turn: turn("goal", complete(50, 5)),
        context: Some(context("run-a", "continuation")),
    });
    ledger.add(&TurnReport {
        driver: "codex".into(),
        provider_turn: turn("child", complete(20, 2)),
        context: Some(context("run-a", "native_child")),
    });
    let settle = SettledReport {
        run_id: "run-a".into(),
        status: "completed".into(),
        completed_at: datetime(9000),
        children_running: false,
    };
    let properties = ledger.settle(&settle, label).expect("report");
    assert_eq!(
        serde_json::to_value(properties).expect("json"),
        json!({"reportVersion":2,"provider":"codex","requestKind":"user","terminalStatus":"completed","lifecycleObserved":true,"model":"gpt-6-sol","effort":"xhigh","serviceTier":"fast","runtimeMode":"full-access","interactionMode":"default","createdBy":"user","creationSource":"web","scheduled":false,"subagent":false,"providerTurns":2,"continuations":1,"steeringRestarts":0,"usageStatus":"complete","inputTokens":150,"outputTokens":15,"nativeChildTurns":1,"childUsageStatus":"complete","childInputTokens":20,"childOutputTokens":2,"durationMs":8000})
    );
    ledger.add(&TurnReport {
        driver: "codex".into(),
        provider_turn: turn("late", complete(7, 1)),
        context: Some(context("run-a", "native_child")),
    });
    assert!(ledger.settle(&settle, label).is_none());
}

#[test]
fn missing_start_missing_counter_and_running_children_do_not_claim_complete() {
    let mut ledger = RequestUsageLedger::default();
    let mut usage = complete(0, 0);
    usage["cachedInputTokens"] = json!(0);
    usage["models"] = json!(["gpt-6-sol"]);
    ledger.add(&TurnReport {
        driver: "codex".into(),
        provider_turn: turn("first", usage),
        context: Some(context("r", "initial")),
    });
    let report = ledger
        .settle(
            &SettledReport {
                run_id: "r".into(),
                status: "completed".into(),
                completed_at: datetime(9000),
                children_running: true,
            },
            label,
        )
        .expect("report");
    assert_eq!(report["usageStatus"], "partial");
    assert_eq!(report["inputTokens"], 0);
    assert_eq!(report["cachedInputTokens"], 0);
    assert!(!report.contains_key("reasoningTokens"));
    assert_eq!(report["childUsageStatus"], "unavailable");
    assert_eq!(report["mixedModels"], false);
}

#[test]
fn reports_only_catalog_models_closed_efforts_and_confirmed_observations() {
    assert_eq!(
        label("anthropic/claude-haiku-4-5-20251001"),
        Some("claude-haiku-4-5".into())
    );
    assert!(label("ollama/private-project").is_none());
    assert!(label("acme-azure-deployment").is_none());
    let mut c = context("private-run", "initial");
    c.model_selection.model = "acme-azure-deployment".into();
    c.model_selection.options =
        serde_json::from_value(json!([{"id":"variant","value":"PRIVATE-EFFORT"}]))
            .expect("options");
    let mut usage = complete(10, 2);
    usage["models"] = json!(["private-model"]);
    let properties = turn_properties(
        &TurnReport {
            driver: "codex".into(),
            provider_turn: turn("private-turn", usage),
            context: Some(c),
        },
        label,
    );
    assert!(!properties.contains_key("model"));
    assert!(!properties.contains_key("effort"));
    assert!(!properties.contains_key("observedModel"));
    assert_eq!(properties["mixedModels"], false);
    let text = serde_json::to_string(&properties).expect("json");
    assert!(!text.contains("private"));
    assert!(!text.contains("PRIVATE"));
}

#[test]
fn feature_and_handoff_events_exclude_prompts_paths_and_unknown_variants() {
    let input = json!({"commandId":"private-command","threadId":"private-thread","type":"thread.pull-request.watch","host":"github.com","repository":"private/repo","number":7,"watching":false});
    let command = serde_json::from_value(input).expect("command");
    assert_eq!(
        command_feature_usage(&command),
        Some(FeatureUsage {
            feature: "thread.pull_request.watch".into(),
            variant: Some("off".into())
        })
    );
    assert!(
        feature_usage(
            "orchestration.launchThread",
            &json!({"title":"private title"})
        )
        .is_none()
    );
    assert!(feature_usage("terminal.write", &json!({"data":"private prompt"})).is_none());
    assert_eq!(
        feature_usage("terminal.open", &json!({"cwd":"C:/private"})),
        Some(FeatureUsage {
            feature: "terminal.open".into(),
            variant: None
        })
    );
    assert_eq!(
        handoff_settings(
            "delegate_task",
            &json!({"mode":"/private/path"}),
            &Value::Null
        ),
        compact_test(json!({"targetChosen":false,"mode":"async"}))
    );
    assert_eq!(
        handoff_settings(
            "t3_thread_launch",
            &json!({"workspaceStrategy":{"type":"/private"}}),
            &Value::Null
        ),
        compact_test(json!({"targetChosen":false,"workspace":"root"}))
    );
}
fn compact_test(value: Value) -> Properties {
    value
        .as_object()
        .expect("object")
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
