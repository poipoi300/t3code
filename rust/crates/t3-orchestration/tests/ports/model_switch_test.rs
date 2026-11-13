#![allow(clippy::unwrap_used)]

use super::command_policy_test::{change, run};
use serde_json::json;
use t3_contracts::{model_selection::ModelSelection, orchestration_v2::*};
use t3_orchestration::{ports::decisions::model_switch::*, testing::TEST_TIME};

fn selection(model: &str) -> ModelSelection {
    serde_json::from_value(json!({"instanceId":"codex","model":model})).unwrap()
}
fn with_model(message: &str, ordinal: u64, model: &str, status: &str) -> OrchestrationV2Run {
    change(
        &run(message, ordinal, status),
        json!({"userMessageId":message,"modelSelection":selection(model)}),
    )
}
pub(super) fn user_item(message: &str) -> OrchestrationV2TurnItem {
    serde_json::from_value(json!({"id":format!("item:{message}"),"threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":TEST_TIME,"createdBy":"user","creationSource":"web","type":"user_message","messageId":message,"inputIntent":"turn_start","text":message,"attachments":[]})).unwrap()
}

#[test]
fn model_switch_chains_from_latest_visible_run_even_when_it_failed_before_starting() {
    let runs = vec![
        with_model("first", 1, "a", "completed"),
        with_model("failed", 2, "b", "failed"),
        with_model("queued", 3, "c", "queued"),
        with_model("undone", 4, "d", "rolled_back"),
    ];
    let items: Vec<_> = ["first", "failed", "queued", "undone"]
        .into_iter()
        .map(user_item)
        .collect();
    let swap = model_switch_since(&runs, &items, &selection("a"), None).unwrap();
    assert_eq!(swap.from, selection("b"));
    assert_eq!(swap.to, selection("a"));
}

#[test]
fn model_switch_skips_messages_clients_hide() {
    let runs = vec![
        with_model("first", 1, "a", "completed"),
        with_model("pending", 2, "b", "cancelled"),
        with_model("/compact", 3, "c", "completed"),
        with_model("async-answer:question", 4, "d", "completed"),
    ];
    let items = vec![
        user_item("first"),
        change(
            &user_item("pending"),
            json!({"inputIntent":"queued_turn","status":"pending"}),
        ),
        user_item("/compact"),
        user_item("async-answer:question"),
    ];
    let mut items = items;
    items[2] = change(&items[2], json!({"text":"\u{feff}/compact\u{feff}"}));
    assert_eq!(
        model_switch_since(&runs, &items, &selection("c"), None)
            .unwrap()
            .from,
        selection("a")
    );
}

#[test]
fn model_switch_counts_wake_handoffs_and_uses_fork_baseline_only_without_visible_runs() {
    let runs = vec![
        with_model("first", 1, "a", "completed"),
        change(
            &with_model("wake", 2, "other", "completed"),
            json!({"contextHandoffId":"handoff"}),
        ),
    ];
    assert!(model_switch_since(&runs, &[user_item("first")], &selection("other"), None).is_none());
    assert!(model_switch_since(&[], &[], &selection("a"), None).is_none());
    assert_eq!(
        model_switch_since(&[], &[], &selection("a"), Some(&selection("b")))
            .unwrap()
            .from,
        selection("b")
    );
    assert!(model_switch_since(&runs[..1], &[user_item("first")], &selection("a"), None).is_none());
}

#[test]
fn execution_order_uses_completion_time_and_option_order_does_not_create_switches() {
    let older = change(
        &run("older", 1, "completed"),
        json!({"completedAt":"2026-09-04T00:00:00.000Z"}),
    );
    let newer = change(
        &run("newer", 2, "completed"),
        json!({"completedAt":TEST_TIME}),
    );
    assert!(run_ran_after(&older, &newer));
    let left = change(
        &selection("a"),
        json!({"options":[{"id":"effort","value":"high"},{"id":"fastMode","value":false}]}),
    );
    let right = change(
        &selection("a"),
        json!({"options":[{"id":"fastMode","value":false},{"id":"effort","value":"high"}]}),
    );
    assert!(model_selections_equal(&left, &right));
    assert!(model_selections_equal(
        &selection("a"),
        &change(&selection("a"), json!({"options":[]}))
    ));
    for (left_value, right_value) in [
        (json!(true), json!("true")),
        (json!("é"), json!("e\u{301}")),
    ] {
        let left = change(
            &selection("a"),
            json!({"options":[{"id":"option","value":left_value},{"id":"option","value":right_value}]}),
        );
        let right = change(
            &selection("a"),
            json!({"options":[{"id":"option","value":right_value},{"id":"option","value":left_value}]}),
        );
        assert!(!model_selections_equal(&left, &right));
    }
}
