use super::thread_stream_test::{command, projection, projection_value};
use serde_json::json;
use std::cell::Cell;
use t3_contracts::orchestration_v2::*;
use t3_orchestration::ports::transport::thread_history_paging::*;
fn rows(count: usize) -> Vec<OrchestrationV2ProjectedTurnItem> {
    projection(count).visible_turn_items
}
fn user_row(index: usize, human: bool) -> OrchestrationV2ProjectedTurnItem {
    let mut item = command(&format!("item-{index}"), index as u64 + 1);
    item["type"] = "user_message".into();
    item["createdBy"] = if human { "user" } else { "agent" }.into();
    item["creationSource"] = "web".into();
    item["inputIntent"] = "turn_start".into();
    item["messageId"] = format!("message-{index}").into();
    item["text"] = "Continue".into();
    item["attachments"] = json!([]);
    serde_json::from_value(json!({"position":index,"visibility":"local","sourceThreadId":"thread","sourceItemId":format!("item-{index}"),"item":item})).expect("test value")
}
fn ids(rows: &[OrchestrationV2ProjectedTurnItem]) -> Vec<String> {
    rows.iter()
        .map(|row| id_text(&row.source_item_id))
        .collect()
}
#[test]
fn cursor_codec_matches_node_base64url_and_rejects_invalid_payloads() {
    let cursor = ThreadHistoryCursorPayload {
        v: 1,
        seq: 44,
        st: "thread".into(),
        si: "item-5".into(),
        p: 5,
        inclusive: None,
    };
    let encoded = encode_thread_history_cursor(&cursor).unwrap();
    assert_eq!(
        encoded,
        "eyJ2IjoxLCJzZXEiOjQ0LCJzdCI6InRocmVhZCIsInNpIjoiaXRlbS01IiwicCI6NX0"
    );
    assert_eq!(decode_thread_history_cursor(&encoded).unwrap(), cursor);
    for value in [
        json!({"v":2,"seq":0,"st":"t","si":"i","p":0}),
        json!({"v":1,"seq":-1,"st":"t","si":"i","p":0}),
        json!({"v":1,"seq":0,"st":"","si":"i","p":0}),
        json!({"v":1,"seq":0,"st":"t","si":"i","p":0.5}),
        json!({"v":1,"seq":0,"st":"t","si":"i","p":0,"inclusive":"true"}),
    ] {
        assert!(
            decode_thread_history_cursor(&base64url_encode(value.to_string().as_bytes())).is_err()
        );
    }
    for value in [String::new(), "x".repeat(4097), "not-json".into()] {
        assert!(decode_thread_history_cursor(&value).is_err());
    }
}
#[test]
fn whole_human_turns_stay_together_but_long_turns_page_from_inside() {
    for count in [180, 600] {
        let mut items = rows(count);
        items[0] = user_row(0, true);
        let recent = select_recent_timeline_window(&items, 1, Default::default()).unwrap();
        if count == 180 {
            assert_eq!(recent.items.len(), 180);
            assert!(!recent.has_more_history);
        } else {
            assert_eq!(recent.items.len(), 75);
            let mut loaded = recent.items;
            let mut cursor = recent.next_cursor;
            let mut pages = 0;
            while let Some(current) = cursor {
                pages += 1;
                assert!(pages < count);
                let page = select_history_page_from_cursor(&items, &current, 1, None).unwrap();
                assert!(!page.items.is_empty());
                assert!(
                    page.items.len()
                        <= if matches!(page.items[0].item, OrchestrationV2TurnItem::UserMessage(_))
                        {
                            500
                        } else {
                            75
                        }
                );
                let mut next = page.items;
                next.extend(loaded);
                loaded = next;
                cursor = page.next_cursor;
            }
            assert_eq!(ids(&loaded), ids(&items));
        }
    }
}
#[test]
fn agent_only_child_transcripts_keep_earlier_activity_pageable() {
    let mut items = rows(91);
    items[0] = user_row(0, false);
    let recent = select_recent_timeline_window(&items, 1, Default::default()).unwrap();
    assert_eq!(recent.items.len(), 75);
    assert!(recent.has_more_history);
    let older =
        select_history_page_from_cursor(&items, recent.next_cursor.as_ref().unwrap(), 1, None)
            .unwrap();
    assert!(matches!(
        older.items[0].item,
        OrchestrationV2TurnItem::UserMessage(_)
    ));
    assert_eq!(older.items.len() + recent.items.len(), items.len());
}
#[test]
fn raw_background_turn_fanout_stops_at_150_and_retains_every_page() {
    let mut items = rows(322);
    for turn in 0..161 {
        items[turn * 2] = user_row(turn * 2, turn == 0);
    }
    let policy = ThreadHistoryPagePolicy {
        max_items: 1000,
        max_encoded_bytes: 10_000_000,
        ..Default::default()
    };
    let recent = select_recent_timeline_window(&items, 1, policy).unwrap();
    assert_eq!(recent.items.len(), 300);
    assert_eq!(id_text(&recent.items[0].source_item_id), "item-22");
    let end = find_cursor_end_index(
        &items,
        &decode_thread_history_cursor(recent.next_cursor.as_ref().unwrap()).unwrap(),
    );
    let older =
        select_older_timeline_page(&items, end, 1, policy, projected_row_encoded_bytes, None)
            .unwrap();
    assert_eq!(older.items.len(), 22);
    assert!(!older.has_more_history);
}
#[test]
fn budgets_measure_only_rows_needed_to_stop_and_admit_exact_fits() {
    for (policy, count, expected, measured_count) in [
        (
            ThreadHistoryPagePolicy {
                max_items: 3,
                max_turn_items: 3,
                max_user_turns: None,
                max_encoded_bytes: 1000,
            },
            5,
            3,
            3,
        ),
        (
            ThreadHistoryPagePolicy {
                max_items: 100,
                max_turn_items: 100,
                max_user_turns: None,
                max_encoded_bytes: 20,
            },
            5,
            2,
            3,
        ),
        (
            ThreadHistoryPagePolicy {
                max_items: 100,
                max_turn_items: 100,
                max_user_turns: None,
                max_encoded_bytes: 5,
            },
            5,
            1,
            2,
        ),
    ] {
        let items = rows(count);
        let measured = Cell::new(0);
        let page = select_older_timeline_page(
            &items,
            count,
            1,
            policy,
            |_| {
                measured.set(measured.get() + 1);
                Ok(10)
            },
            None,
        )
        .unwrap();
        assert_eq!(page.items.len(), expected);
        assert_eq!(measured.get(), measured_count);
        assert_eq!(ids(&page.items), ids(&items[count - expected..]));
        assert!(page.has_more_history);
    }
    let items = vec![rows(1).remove(0), user_row(1, true), rows(3).remove(2)];
    let measured = Cell::new(0);
    let page = select_older_timeline_page(
        &items,
        3,
        1,
        ThreadHistoryPagePolicy {
            max_user_turns: Some(1),
            max_items: 100,
            max_turn_items: 100,
            max_encoded_bytes: 1000,
        },
        |_| {
            measured.set(measured.get() + 1);
            Ok(10)
        },
        None,
    )
    .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(measured.get(), 2);
}
#[test]
fn oversized_first_rows_cannot_deadlock_paging() {
    for human in [false, true] {
        let mut items = rows(2);
        if human {
            items[0] = user_row(0, true);
            if let OrchestrationV2TurnItem::UserMessage(v) = &mut items[0].item {
                v.text = "x".repeat(2_000_000);
            }
        } else if let OrchestrationV2TurnItem::CommandExecution(v) = &mut items[0].item {
            v.output = Some(Some("x".repeat(2_000_000)));
        }
        let policy = ThreadHistoryPagePolicy {
            max_items: 50,
            max_encoded_bytes: 1024,
            ..Default::default()
        };
        let page = select_recent_timeline_window(&items, 1, policy).unwrap();
        assert_eq!(ids(&page.items), vec!["item-1"]);
        let cursor = decode_thread_history_cursor(page.next_cursor.as_ref().unwrap()).unwrap();
        let older = select_older_timeline_page(
            &items,
            find_cursor_end_index(&items, &cursor),
            1,
            policy,
            projected_row_encoded_bytes,
            None,
        )
        .unwrap();
        assert_eq!(ids(&older.items), vec!["item-0"]);
        assert!(!older.has_more_history);
        assert!(older.next_cursor.is_none());
    }
}
#[test]
fn cursor_identity_survives_new_appends_and_missing_identity_uses_positions() {
    let initial = rows(20);
    let policy = ThreadHistoryPagePolicy {
        max_items: 5,
        max_encoded_bytes: 10_000_000,
        ..Default::default()
    };
    let recent = select_recent_timeline_window(&initial, 2, policy).unwrap();
    let grown = rows(22);
    let cursor = decode_thread_history_cursor(recent.next_cursor.as_ref().unwrap()).unwrap();
    let older = select_older_timeline_page(
        &grown,
        find_cursor_end_index(&grown, &cursor),
        5,
        policy,
        projected_row_encoded_bytes,
        None,
    )
    .unwrap();
    assert_eq!(
        ids(&older.items),
        vec!["item-10", "item-11", "item-12", "item-13", "item-14"]
    );
    let missing = ThreadHistoryCursorPayload {
        v: 1,
        seq: 3,
        st: "thread".into(),
        si: "deleted".into(),
        p: 40,
        inclusive: None,
    };
    assert_eq!(find_cursor_end_index(&rows(5), &missing), 5);
    let sparse: Vec<_> = [0, 10, 20]
        .into_iter()
        .map(|index| {
            let mut row = rows(21).remove(index);
            row.position = index as u64;
            row
        })
        .collect();
    assert_eq!(
        find_cursor_end_index(&sparse, &ThreadHistoryCursorPayload { p: 15, ..missing }),
        2
    );
}
#[test]
fn bounded_snapshots_charge_local_item_duplication_and_preserve_inherited_only_watermarks() {
    let row = rows(1).remove(0);
    let thread = projection(0).thread.id;
    let local = projected_row_bounded_snapshot_encoded_bytes(&row, &thread).unwrap();
    let plain = projected_row_encoded_bytes(&row).unwrap();
    assert!(local > plain);
    let mut inherited = row.clone();
    inherited.visibility = ProjectedTurnItemVisibility::Inherited;
    inherited.source_thread_id = serde_json::from_value(json!("parent")).unwrap();
    assert_eq!(
        projected_row_bounded_snapshot_encoded_bytes(&inherited, &thread).unwrap(),
        projected_row_encoded_bytes(&inherited).unwrap()
    );
    let mut value = projection_value(100);
    for row in value["visibleTurnItems"].as_array_mut().unwrap() {
        row["visibility"] = "inherited".into();
        row["sourceThreadId"] = "parent".into();
    }
    let bounded = build_bounded_thread_projection(
        serde_json::from_value(value).unwrap(),
        1,
        Default::default(),
    )
    .unwrap();
    assert_eq!(bounded.latest_local_turn_ordinal, Some(100));
    assert_eq!(bounded.projection.turn_items.len(), 0);
    assert_eq!(bounded.projection.visible_turn_items.len(), 75);
}
#[test]
fn deferred_work_keeps_persistent_rows_and_separates_attempts_and_blocked_runs() {
    let make = |count| {
        rows(count)
            .into_iter()
            .map(|mut row| {
                if let OrchestrationV2TurnItem::CommandExecution(v) = &mut row.item {
                    v.run_id = Some(serde_json::from_value(json!("run")).unwrap());
                }
                WorkHistoryRow {
                    row,
                    run_status: Some("completed".into()),
                    attempt_id: None,
                    attempt_status: None,
                    persistent_output: false,
                }
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(defer_work_history_rows(make(150)).unwrap().len(), 150);
    let deferred = defer_work_history_rows(make(151)).unwrap();
    assert_eq!(deferred.len(), 1);
    let OrchestrationV2TurnItem::CommandExecution(item) = &deferred[0].row.item else {
        panic!()
    };
    let work = item.deferred_work.as_ref().unwrap();
    assert_eq!(work.item_count, 151);
    assert_eq!(decode_thread_history_cursor(&work.cursor.0).unwrap().p, 151);
    for status in ["running", "failed", "interrupted", "cancelled"] {
        let mut rows = make(151);
        for row in &mut rows {
            row.run_status = Some(status.into());
        }
        assert_eq!(defer_work_history_rows(rows).unwrap().len(), 151);
    }
    let mut superseded = make(151);
    for row in &mut superseded {
        row.run_status = Some("running".into());
        row.attempt_id = Some("old".into());
        row.attempt_status = Some("superseded".into());
    }
    let mut active = make(151);
    for row in &mut active {
        row.run_status = Some("running".into());
        row.attempt_id = Some("active".into());
        row.attempt_status = Some("running".into());
    }
    superseded.extend(active);
    assert_eq!(defer_work_history_rows(superseded).unwrap().len(), 152);
    let mut persistent = make(152);
    persistent[20].persistent_output = true;
    let deferred = defer_work_history_rows(persistent).unwrap();
    assert_eq!(deferred.len(), 2);
    assert_eq!(id_text(&deferred[1].row.source_item_id), "item-20");
}
