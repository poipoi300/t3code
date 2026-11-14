use serde_json::{Value, json};
use t3_contracts::orchestration_v2::*;
use t3_orchestration::ports::transport::{thread_stream::*, wire_projection::*};

pub fn command(id: &str, ordinal: u64) -> Value {
    json!({"id":id,"type":"command_execution","threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":"Command","input":"command","output":"private output","exitCode":0,"startedAt":"2026-09-08T00:00:00.000Z","completedAt":"2026-09-08T00:00:00.000Z","updatedAt":"2026-09-08T00:00:00.000Z"})
}
pub fn projection_value(count: usize) -> Value {
    let items: Vec<_> = (0..count)
        .map(|i| command(&format!("item-{i}"), i as u64 + 1))
        .collect();
    let rows: Vec<_> = items.iter().enumerate().map(|(i,item)|json!({"position":i,"visibility":"local","sourceThreadId":"thread","sourceItemId":item["id"],"item":item})).collect();
    json!({"thread":{"id":"thread","projectId":"project","title":"Test","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":"thread"},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":"2026-09-08T00:00:00.000Z","updatedAt":"2026-09-08T00:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null},"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":items,"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":rows,"updatedAt":"2026-09-08T00:00:00.000Z"})
}
pub fn projection(count: usize) -> OrchestrationV2ThreadProjection {
    serde_json::from_value(projection_value(count)).expect("test value")
}

#[test]
fn replay_uses_filtered_count_and_inclusive_byte_bounds() {
    assert_eq!(
        decide_thread_resume(10, 20_000, 128, 1_048_576),
        ThreadResumePlan::Replay {
            after_sequence: 10,
            through_sequence: 20_000
        }
    );
    for (after, head, count, bytes) in [(10, 11, 1, 1_048_577), (50, 40, 0, 0), (0, 200, 129, 0)] {
        assert_eq!(
            decide_thread_resume(after, head, count, bytes),
            ThreadResumePlan::Snapshot
        );
    }
    assert!(is_thread_replay_raw_payload_safe(1_048_576));
    assert!(!is_thread_replay_raw_payload_safe(1_048_577));
}
#[test]
fn fallback_snapshot_is_bounded_with_cursor_and_full_watermark() {
    let snapshot =
        build_bounded_thread_stream_snapshot(44, projection(80), SnapshotOptions::default())
            .unwrap();
    let OrchestrationV2ThreadStreamItem::Snapshot(snapshot) = snapshot else {
        panic!()
    };
    assert_eq!(snapshot.snapshot_sequence, 44);
    assert_eq!(snapshot.projection.turn_items.len(), 75);
    assert_eq!(snapshot.projection.visible_turn_items.len(), 75);
    assert!(snapshot.history_cursor.unwrap().is_some());
    assert_eq!(snapshot.has_more_history, Some(true));
    assert_eq!(snapshot.latest_local_turn_ordinal, Some(Some(80)));
}
#[test]
fn raw_replay_limit_applies_before_tool_output_projection() {
    let mut tool = command("large", 1);
    tool["type"] = "dynamic_tool".into();
    tool["toolName"] = "large".into();
    tool["input"] = json!({"query":"small"});
    tool["output"] = json!({"text":"x".repeat(10 * 1_048_576)});
    let mut item: OrchestrationV2TurnItem = serde_json::from_value(tool).unwrap();
    assert!(!is_thread_replay_raw_payload_safe(
        encoded_bytes(&item).unwrap()
    ));
    project_turn_item_for_wire(&mut item).unwrap();
    assert!(thread_replay_encoded_bytes(&[item], 1_048_576).unwrap() <= 2048);
}
#[test]
fn compact_snapshots_restore_the_exact_local_prefix_for_linear_and_fork_threads() {
    for fork in [false, true] {
        let mut base = projection_value(if fork { 6 } else { 80 });
        if fork {
            for row in base["visibleTurnItems"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .take(3)
            {
                row["visibility"] = "inherited".into();
                row["sourceThreadId"] = "parent".into();
                row["item"]["threadId"] = "parent".into();
            }
            let mut request = command("interrupt-request", 0);
            request["type"] = "run_interrupt_request".into();
            request["message"] = "Stop".into();
            base["turnItems"].as_array_mut().unwrap().insert(0, request);
        }
        let base: OrchestrationV2ThreadProjection = serde_json::from_value(base).unwrap();
        let full =
            build_bounded_thread_stream_snapshot(44, base.clone(), SnapshotOptions::default())
                .unwrap();
        let compact = build_bounded_thread_stream_snapshot(
            44,
            base,
            SnapshotOptions {
                compact_turn_items: true,
                ..Default::default()
            },
        )
        .unwrap();
        let (
            OrchestrationV2ThreadStreamItem::Snapshot(full),
            OrchestrationV2ThreadStreamItem::Snapshot(mut compact),
        ) = (full, compact)
        else {
            panic!()
        };
        assert!(full.turn_items_omit_local_visible.is_none());
        assert!(compact.turn_items_omit_local_visible.is_some());
        if fork {
            assert_eq!(
                compact
                    .projection
                    .visible_turn_items
                    .iter()
                    .filter(|row| row.visibility == ProjectedTurnItemVisibility::Inherited)
                    .count(),
                3
            );
            assert_eq!(compact.projection.turn_items.len(), 1);
            assert!(matches!(
                compact.projection.turn_items[0],
                OrchestrationV2TurnItem::RunInterruptRequest(_)
            ));
        }
        let mut restored: Vec<_> = compact
            .projection
            .visible_turn_items
            .iter()
            .filter(|row| {
                row.visibility == ProjectedTurnItemVisibility::Local
                    || row.source_thread_id == compact.projection.thread.id
            })
            .map(|row| row.item.clone())
            .collect();
        restored.append(&mut compact.projection.turn_items);
        compact.projection.turn_items = restored;
        assert_eq!(compact.projection, full.projection);
        let encoded =
            serde_json::to_string(&OrchestrationV2ThreadStreamItem::Snapshot(compact)).unwrap();
        assert!(serde_json::from_str::<OrchestrationV2ThreadStreamItem>(&encoded).is_ok());
    }
}

#[test]
fn full_state_coalescing_preserves_identity_switches_and_control_boundaries() {
    fn event(sequence: u64, id: &str, terminal: bool) -> OrchestrationV2ThreadStreamItem {
        let mut item = command(id, sequence);
        item["status"] = if terminal { "completed" } else { "running" }.into();
        serde_json::from_value(json!({"kind":"event","sequence":sequence,"event":{"id":format!("event-{sequence}"),"threadId":"thread","type":"turn-item.updated","occurredAt":"2026-09-08T00:00:00.000Z","payload":item}})).unwrap()
    }
    for (input, expected) in [
        (vec![event(1, "a", false), event(2, "a", false)], vec![2]),
        (
            vec![
                event(1, "a", false),
                event(2, "b", false),
                event(3, "a", false),
            ],
            vec![1, 2, 3],
        ),
        (
            vec![
                event(1, "a", false),
                event(2, "a", true),
                event(3, "a", false),
            ],
            vec![1, 2, 3],
        ),
    ] {
        let output = coalesce_live_updated_events(input);
        let sequences: Vec<_> = output
            .into_iter()
            .map(|item| match item {
                OrchestrationV2ThreadStreamItem::Event(item) => item.sequence,
                _ => 0,
            })
            .collect();
        assert_eq!(sequences, expected);
    }
}
