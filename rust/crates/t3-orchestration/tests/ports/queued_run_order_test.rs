use super::command_policy_test::{change, message, projection, run};
use serde_json::json;
use t3_orchestration::ports::decisions::queued_run_order::*;

#[test]
fn automatic_completions_precede_visible_queue_and_ordinals_break_position_ties() {
    let mut projection = projection();
    projection.messages = vec![
        message("first"),
        change(
            &message("automatic"),
            json!({"delegatedCompletion":{"generation":1,"parentRunId":"parent","taskIds":["task"]}}),
        ),
        message("second"),
    ];
    projection.runs = vec![
        change(&run("first", 2, "queued"), json!({"queuePosition":1})),
        change(&run("automatic", 4, "queued"), json!({"queuePosition":3})),
        change(&run("second", 3, "queued"), json!({"queuePosition":2})),
    ];
    let order = || {
        queued_runs_in_delivery_order(&projection)
            .iter()
            .map(|run| serde_json::to_value(&run.id).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        order(),
        vec![json!("automatic"), json!("first"), json!("second")]
    );
    projection.runs[2] = change(&projection.runs[2], json!({"queuePosition":1}));
    let order: Vec<_> = queued_runs_in_delivery_order(&projection)
        .iter()
        .map(|run| run.ordinal)
        .collect();
    assert_eq!(order, vec![4, 2, 3]);
}
