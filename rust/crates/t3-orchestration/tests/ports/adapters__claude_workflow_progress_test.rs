use serde_json::{Value, json};
use t3_contracts::orchestration_v2::OrchestrationV2SubagentWorkflow;
use t3_orchestration::ports::claude::adapters::claude_workflow_progress::*;

fn merge(
    previous: Option<&OrchestrationV2SubagentWorkflow>,
    entries: Vec<Value>,
) -> OrchestrationV2SubagentWorkflow {
    merge_claude_workflow_progress(previous, None, None, &json!({"workflow_progress":entries}))
        .expect("workflow progress snapshot")
}
fn agent(state: &str) -> Value {
    json!({"type":"workflow_agent","index":1,"label":"alpha:one","state":state,"phaseIndex":1,"phaseTitle":"Alpha"})
}
fn value(workflow: &OrchestrationV2SubagentWorkflow) -> Value {
    serde_json::to_value(workflow).expect("encode workflow progress")
}

#[test]
fn maps_provider_states_and_member_metrics() {
    let mut done = agent("done");
    for (key,v) in json!({"agentId":"a1","model":"claude-opus-5[1m]","attempt":2,"tokens":71141,"toolCalls":3,"durationMs":1277,
        "startedAt":1700000000000u64,"lastProgressAt":1700000004432u64,"queuedAt":1699999999000u64,"promptPreview":"Reply with exactly: A1","resultPreview":"A1"}).as_object().unwrap(){done[key]=v.clone();}
    let mut queued = agent("queued");
    queued["index"] = json!(2);
    queued["label"] = json!("alpha:two");
    queued["lastProgressAt"] = json!(1000);
    let mut running = agent("progress");
    running["index"] = json!(3);
    running["lastProgressAt"] = json!(2000);
    let result = value(&merge(
        None,
        vec![
            json!({"type":"workflow_phase","index":1,"title":"Alpha"}),
            done,
            queued,
            running,
        ],
    ));
    assert_eq!(result["phases"], json!([{"index":1,"title":"Alpha"}]));
    assert_eq!(
        result["agents"][0],
        json!({"index":1,"label":"alpha:one","agentId":"a1","state":"completed","phaseIndex":1,"phaseTitle":"Alpha",
        "model":"claude-opus-5[1m]","attempt":2,"totalTokens":71141,"toolCalls":3,"durationMs":1277,"queuedAt":1699999999000u64,
        "startedAt":1700000000000u64,"completedAt":1700000004432u64,"prompt":"Reply with exactly: A1","result":"A1"})
    );
    assert_eq!(result["agents"][1]["state"], "queued");
    assert!(result["agents"][1].get("completedAt").is_none());
    assert!(result["agents"][2].get("completedAt").is_none());
}
#[test]
fn scheduling_waits_for_started_at_and_unknown_states_remain_running() {
    let mut scheduled = agent("start");
    scheduled["queuedAt"] = json!(1000);
    let first = merge(None, vec![scheduled.clone()]);
    assert_eq!(value(&first)["agents"][0]["state"], "queued");
    scheduled["startedAt"] = json!(21000);
    let second = merge(Some(&first), vec![scheduled]);
    assert_eq!(value(&second)["agents"][0]["state"], "running");
    for state in ["something-new", "toString", "constructor", "__proto__"] {
        assert_eq!(
            value(&merge(None, vec![agent(state)]))["agents"][0]["state"],
            "running"
        );
    }
}
#[test]
fn usage_only_frames_retain_roster_and_do_not_create_workflows() {
    let first=merge_claude_workflow_progress(None,Some("probe-wf"),None,&json!({"workflow_progress":[{"type":"workflow_phase","index":1,"title":"Alpha"},agent("start")],"usage":{"total_tokens":100,"tool_uses":1,"duration_ms":40}})).unwrap();
    let second=merge_claude_workflow_progress(Some(&first),None,None,&json!({"workflow_progress":[],"usage":{"total_tokens":250,"tool_uses":2,"duration_ms":900}})).unwrap();
    let result = value(&second);
    assert_eq!(result["agents"].as_array().unwrap().len(), 1);
    assert_eq!(result["phases"].as_array().unwrap().len(), 1);
    assert_eq!(result["name"], "probe-wf");
    assert_eq!(result["totalTokens"], 250);
    assert_eq!(result["toolCalls"], 2);
    assert_eq!(result["durationMs"], 900);
    assert!(
        merge_claude_workflow_progress(None, None, None, &json!({"usage":{"total_tokens":123}}))
            .is_none()
    );
}
#[test]
fn sparse_retry_resets_old_metrics_but_keeps_other_members_and_phase_links() {
    let mut failed = agent("error");
    failed["attempt"] = json!(1);
    failed["promptPreview"] = json!("First prompt");
    failed["resultPreview"] = json!("First result");
    failed["tokens"] = json!(100);
    failed["lastProgressAt"] = json!(2010);
    let mut other = agent("done");
    other["index"] = json!(2);
    let mut first = value(&merge(
        None,
        vec![
            json!({"type":"workflow_phase","index":1,"title":"Alpha"}),
            failed,
            other,
        ],
    ));
    for member in first["agents"].as_array_mut().unwrap() {
        member["effort"] = json!("max");
        member["speed"] = json!("standard");
    }
    let first: OrchestrationV2SubagentWorkflow = serde_json::from_value(first).unwrap();
    let second = value(&merge(
        Some(&first),
        vec![
            json!({"type":"workflow_agent","index":1,"label":"alpha:one","state":"start","attempt":2,"startedAt":3000}),
        ],
    ));
    assert_eq!(
        second["agents"][0],
        json!({"index":1,"label":"alpha:one","state":"running","attempt":2,"startedAt":3000,"phaseIndex":1,"phaseTitle":"Alpha"})
    );
    assert_eq!(second["agents"][1], value(&first)["agents"][1]);
    assert_eq!(second["phases"], value(&first)["phases"]);
}
#[test]
fn rejects_older_attempts_and_same_attempt_state_regressions() {
    for (state, stale, old_attempt) in [
        ("done", "error", 1),
        ("done", "start", 2),
        ("done", "queued", 2),
        ("error", "start", 2),
        ("error", "queued", 2),
        ("start", "queued", 2),
    ] {
        let mut first = agent(state);
        first["attempt"] = json!(2);
        first["startedAt"] = json!(1010);
        first["resultPreview"] = json!("Current result");
        let first = merge(None, vec![first]);
        let mut next = agent(stale);
        next["attempt"] = json!(old_attempt);
        next["startedAt"] = json!(1010);
        next["resultPreview"] = json!("Old result");
        assert_eq!(
            value(&merge(Some(&first), vec![next]))["agents"],
            value(&first)["agents"]
        );
    }
}
#[test]
fn omitted_fields_survive_same_attempt_and_malformed_entries_are_ignored() {
    let mut first = agent("start");
    first["attempt"] = json!(2);
    first["model"] = json!("claude-opus-5[1m]");
    first["tokens"] = json!(100);
    first["promptPreview"] = json!("Current prompt");
    let mut first = value(&merge(None, vec![first]));
    first["agents"][0]["effort"] = json!("xhigh");
    first["agents"][0]["speed"] = json!("standard");
    let first = serde_json::from_value(first).unwrap();
    let mut done = agent("done");
    done["resultPreview"] = json!("Current result");
    let result = value(&merge(Some(&first), vec![done]));
    let member = &result["agents"][0];
    for (key, expected) in [
        ("state", json!("completed")),
        ("attempt", json!(2)),
        ("prompt", json!("Current prompt")),
        ("result", json!("Current result")),
        ("totalTokens", json!(100)),
        ("model", json!("claude-opus-5[1m]")),
        ("effort", json!("xhigh")),
        ("speed", json!("standard")),
    ] {
        assert_eq!(member[key], expected);
    }
    let result = value(&merge(
        None,
        vec![
            json!({"type":"workflow_phase","title":"No index"}),
            json!({"type":"workflow_agent","index":4}),
            json!({"type":"something_else","index":1,"title":"Ignored"}),
            json!({"type":"workflow_phase","index":1,"title":"Alpha"}),
        ],
    ));
    assert_eq!(
        result,
        json!({"phases":[{"index":1,"title":"Alpha"}],"agents":[]})
    );
}
#[test]
fn validates_metrics_timestamps_and_merges_run_handles() {
    let handles = parse_claude_workflow_run_handles(
        &json!({"taskType":"local_workflow","runId":"r","transcriptDir":"d","scriptPath":"s"}),
    )
    .unwrap();
    assert!(
        parse_claude_workflow_run_handles(&json!({"taskType":"local_agent","runId":"r"})).is_none()
    );
    let mut entry = agent("done");
    entry["tokens"] = json!(-1);
    entry["durationMs"] = json!(1.5);
    entry["lastProgressAt"] = json!(8640000000000001u64);
    let result = merge_claude_workflow_progress(
        None,
        None,
        Some(&handles),
        &json!({"workflow_progress":[entry]}),
    )
    .unwrap();
    let result = value(&result);
    assert_eq!(
        result["runHandles"],
        json!({"runId":"r","transcriptDir":"d","scriptPath":"s"})
    );
    assert!(result["agents"][0].get("totalTokens").is_none());
    assert!(result["agents"][0].get("durationMs").is_none());
    assert!(result["agents"][0].get("completedAt").is_none());
    let numeric = merge(
        None,
        vec![
            json!({"type":"workflow_agent","index":1.0,"label":"Decimal JSON numbers",
        "state":"done","tokens":100.0,"lastProgressAt":250.0}),
        ],
    );
    assert_eq!(value(&numeric)["agents"][0]["totalTokens"], 100);
    assert_eq!(value(&numeric)["agents"][0]["completedAt"], 250);
}
