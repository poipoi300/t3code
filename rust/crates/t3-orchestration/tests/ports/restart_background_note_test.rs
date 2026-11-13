use super::provider_event_ingestor_test::{decode, fixture};
use serde_json::{Value, json};
use std::collections::HashSet;
use t3_orchestration::{api::*, ports::runs_ingestion::restart_background_note::*};
fn run(ordinal: u64, provider: &str, extra: Value) -> OrchestrationV2Run {
    let mut run = fixture("OrchestrationV2Run");
    for (key,value) in json!({"id":format!("run:{ordinal}"),"ordinal":ordinal,"providerThreadId":provider,"userMessageId":format!("message:{ordinal}"),"activeAttemptId":format!("attempt:{ordinal}"),"status":"completed","completedAt":null}).as_object().unwrap(){run[key]=value.clone();}
    for (key, value) in extra.as_object().unwrap() {
        run[key] = value.clone();
    }
    decode(run)
}
fn turn(run: &OrchestrationV2Run, status: &str) -> OrchestrationV2ProviderTurn {
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["runAttemptId"] = t3_contracts::stored::to_value(&run.active_attempt_id).unwrap();
    turn["providerThreadId"] = t3_contracts::stored::to_value(&run.provider_thread_id).unwrap();
    turn["status"] = status.into();
    decode(turn)
}
fn attempt(run: &OrchestrationV2Run, id: &str) -> OrchestrationV2RunAttempt {
    let mut attempt = fixture("OrchestrationV2RunAttempt");
    attempt["runId"] = t3_contracts::stored::to_value(&run.id).unwrap();
    attempt["id"] = id.into();
    decode(attempt)
}
fn lost() -> Value {
    json!([{"kind":"subagent","label":"Background subagent test"}])
}
fn pending(
    target: &OrchestrationV2Run,
    runs: &[OrchestrationV2Run],
) -> Vec<OrchestrationV2RestartCancelledBackgroundWork> {
    let turns = runs
        .iter()
        .filter(|run| run.id != target.id)
        .map(|run| turn(run, "completed"))
        .collect::<Vec<_>>();
    pending_restart_cancelled_background_work(target, runs, &turns, &[], &HashSet::new())
}
#[test]
fn notes_follow_the_lost_session_and_execution_order_instead_of_ordinal() {
    let root = run(
        1,
        "claude",
        json!({"restartCancelledBackgroundWork":lost()}),
    );
    let codex = run(2, "codex", json!({}));
    let back = run(3, "claude", json!({}));
    let later = run(4, "claude", json!({}));
    assert!(pending(&codex, &[root.clone(), codex.clone()]).is_empty());
    assert_eq!(
        t3_contracts::stored::to_value(&pending(
            &back,
            &[root.clone(), codex.clone(), back.clone()]
        ))
        .unwrap(),
        lost()
    );
    assert!(pending(&later, &[root, codex, back, later.clone()]).is_empty());
    let first = run(
        3,
        "claude",
        json!({"completedAt":"2026-10-03T10:00:00.000Z"}),
    );
    let resumed = run(
        2,
        "claude",
        json!({"completedAt":"2026-10-03T10:05:00.000Z","restartCancelledBackgroundWork":lost()}),
    );
    assert_eq!(
        t3_contracts::stored::to_value(&pending(&later, &[resumed, first, later.clone()])).unwrap(),
        lost()
    );
}
#[test]
fn labels_keep_native_resume_ids_and_unicode_and_distinguish_awaited_monitors() {
    let roster = |extra: Value| {
        let mut task = json!({"kind":"command","taskId":"shell-1"});
        for (key, value) in extra.as_object().unwrap() {
            task[key] = value.clone();
        }
        cancelled_roster_task_work(&decode(task)).unwrap()
    };
    let work = roster(json!({"description":"sleep 150","parentTaskId":"agent-1"}));
    assert_eq!(work.label.as_str(), "sleep 150 (subagent agent-1)");
    assert!(is_awaited_work(&work));
    let long = roster(json!({"description":"d".repeat(400),"parentTaskId":"agent-1"}));
    assert_eq!(long.label.as_str().encode_utf16().count(), 160);
    assert!(long.label.as_str().ends_with("(subagent agent-1)"));
    assert_eq!(
        roster(json!({"parentTaskId":"a".repeat(400)}))
            .label
            .as_str()
            .encode_utf16()
            .count(),
        160
    );
    assert!(
        roster(json!({"description":format!("{}🚀 deploy","d".repeat(158))}))
            .label
            .as_str()
            .ends_with('…')
    );
    for kind in ["command", "monitor"] {
        assert!(!is_awaited_work(&roster(json!({"kind":kind}))));
        assert!(is_awaited_work(&roster(
            json!({"kind":kind,"awaited":true})
        )));
    }
    let mut command = super::provider_event_ingestor_test::item("command_execution", "item-2");
    command["input"] = "sleep 180".into();
    command["output"] = "".into();
    command["commandActions"] = json!([]);
    command["exitCode"] = Value::Null;
    command["durationMs"] = Value::Null;
    command["workspaceMutation"] = Value::Null;
    assert!(!is_awaited_work(
        &cancelled_turn_item_work(&decode(command.clone()), None)
            .unwrap()
            .unwrap()
    ));
    command["completionWake"] = "always".into();
    assert!(is_awaited_work(
        &cancelled_turn_item_work(&decode(command), None)
            .unwrap()
            .unwrap()
    ));
    let mut native = super::provider_event_ingestor_test::item("subagent", "item-1");
    native["origin"] = "provider_native".into();
    native["driver"] = "claudeAgent".into();
    native["providerInstanceId"] = "claudeAgent".into();
    native["title"] = "Review".into();
    native["nativeItemRef"] =
        json!({"driver":"claudeAgent","nativeId":"agent-1","strength":"strong"});
    native["subagentId"] = "agent-1".into();
    native["prompt"] = "Review".into();
    native["childThreadId"] = Value::Null;
    native["result"] = Value::Null;
    native["modelSelection"] = Value::Null;
    native["checkpointId"] = Value::Null;
    native["contextHandoffId"] = Value::Null;
    native["failure"] = Value::Null;
    native["completionWake"] = "always".into();
    assert_eq!(
        cancelled_turn_item_work(&decode(native), None)
            .unwrap()
            .unwrap()
            .label
            .as_str(),
        "Review (id agent-1)"
    );
}
#[test]
fn identical_labels_keep_distinct_ids_and_bound_the_note() {
    let first: OrchestrationV2RestartCancelledBackgroundWork =
        decode(json!({"kind":"shell","label":"sleep 20","id":"item-1"}));
    let second: OrchestrationV2RestartCancelledBackgroundWork =
        decode(json!({"kind":"shell","label":"sleep 20","id":"item-2"}));
    assert_eq!(
        merge_restart_cancelled_background_work(
            std::slice::from_ref(&first),
            &[second.clone(), first.clone()]
        ),
        vec![first, second]
    );
    let legacy: OrchestrationV2RestartCancelledBackgroundWork =
        decode(json!({"kind":"shell","label":"sleep 20"}));
    assert_eq!(
        merge_restart_cancelled_background_work(
            std::slice::from_ref(&legacy),
            std::slice::from_ref(&legacy)
        ),
        vec![legacy.clone()]
    );
    let note = restart_cancelled_background_work_note(&vec![legacy; 12]);
    assert!(note.ends_with("- and 2 more"));
    assert_eq!(note.matches("- shell:").count(), 10);
}
#[test]
fn completed_attempts_deliver_notes_but_running_or_cancelled_continuations_do_not() {
    let cut = run(
        1,
        "claude",
        json!({"status":"cancelled","restartCancelledBackgroundWork":lost()}),
    );
    let continuation = run(2, "claude", json!({"restartContinuationOfRunId":"run:1"}));
    let later = run(3, "claude", json!({}));
    let runs = vec![cut.clone(), continuation.clone(), later.clone()];
    let cut_turn = turn(&cut, "cancelled");
    assert!(is_restart_note_continuation(
        &continuation,
        &runs,
        std::slice::from_ref(&cut_turn),
        &[]
    ));
    let mut without_work = cut.clone();
    without_work.restart_cancelled_background_work = Some(Some(vec![]));
    assert!(!is_restart_note_continuation(
        &continuation,
        &[without_work, continuation.clone()],
        std::slice::from_ref(&cut_turn),
        &[]
    ));
    for status in ["running", "cancelled", "completed"] {
        let turns = vec![cut_turn.clone(), turn(&continuation, status)];
        let work =
            pending_restart_cancelled_background_work(&later, &runs, &turns, &[], &HashSet::new());
        assert_eq!(work.is_empty(), status == "completed");
    }
    let chained = run(3, "claude", json!({"restartContinuationOfRunId":"run:2"}));
    let note =
        restart_continuation_note(&continuation, &runs, std::slice::from_ref(&cut_turn), &[]);
    assert_eq!(t3_contracts::stored::to_value(&note.work).unwrap(), lost());
    assert!(!note.settled);
    assert!(is_restart_note_continuation(
        &chained,
        &runs,
        std::slice::from_ref(&cut_turn),
        &[]
    ));
    let steered = run(
        2,
        "claude",
        json!({"restartContinuationOfRunId":"run:1","activeAttemptId":"attempt:replacement"}),
    );
    let turns = vec![
        cut_turn,
        turn(&continuation, "completed"),
        turn(&steered, "cancelled"),
    ];
    let attempts = vec![attempt(&steered, "attempt:2")];
    assert!(
        restart_continuation_note(&steered, &[cut.clone(), steered.clone()], &turns, &attempts)
            .work
            .is_empty()
    );
    assert!(
        pending_restart_cancelled_background_work(
            &steered,
            &[cut, steered.clone()],
            &turns,
            &attempts,
            &HashSet::new()
        )
        .is_empty()
    );
}
