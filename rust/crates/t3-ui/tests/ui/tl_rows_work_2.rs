//! C3 projections rendered through the same work-group entry point the timeline mounts.

use std::cell::RefCell;
use std::rc::Rc;

use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent as Content, MessagesTimelineRowsInput,
    MessagesTimelineRowsProjection, TimelineLatestRun, derive_messages_timeline_rows_with_state,
};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::i18n::t;
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rows::groups::{
    WorkGroupsContext, WorkGroupsState, work_group_timeline_row,
};

use crate::support::{DESKTOP, PHONE};

fn at(second: usize) -> String {
    format!("2026-01-01T00:00:{second:02}Z")
}

fn message(id: &str, second: usize, role: &str, intent: &str) -> Rc<TimelineEntry> {
    Rc::new(serde_json::from_value(json!({
        "id": id, "kind": "message", "createdAt": at(second),
        "message": { "id": id, "role": role, "text": id, "inputIntent": intent,
            "runId": "run", "createdAt": at(second), "updatedAt": at(second), "streaming": false }
    })).expect("canonical message"))
}

fn work(id: &str, second: usize, extra: Value) -> Rc<TimelineEntry> {
    let mut entry = json!({
        "id": id, "runId": "run", "createdAt": at(second), "label": id,
        "tone": "tool", "itemType": "command_execution", "toolLifecycleStatus": "completed"
    });
    for (key, value) in extra.as_object().expect("entry fields") {
        entry[key] = value.clone();
    }
    Rc::new(
        serde_json::from_value(json!({
            "kind": "work", "id": id, "createdAt": at(second), "entry": entry,
        }))
        .expect("canonical work entry"),
    )
}

fn input(scenario: &str) -> MessagesTimelineRowsInput {
    let mut entries = vec![message("prompt", 0, "user", "turn_start")];
    match scenario {
        "steer" => {
            entries.push(work("Before steer", 4, json!({})));
            entries.push(message("steer", 10, "user", "steer"));
            entries.push(work("After steer", 15, json!({})));
        }
        "attempt" => {
            let mut old =
                serde_json::to_value(work("Superseded tool", 2, json!({}))).expect("entry");
            old["attempt"] = json!({
                "id":"attempt-1", "runId":"run", "attemptOrdinal":1, "rootNodeId":"root",
                "providerInstanceId":"codex", "providerThreadId":"provider-thread", "providerTurnId":null,
                "reason":"initial", "status":"superseded", "startedAt":at(0), "completedAt":at(4)
            });
            entries.push(Rc::new(
                serde_json::from_value(old).expect("superseded attempt"),
            ));
            entries.push(work(
                "Current tool",
                5,
                json!({"toolLifecycleStatus":"inProgress"}),
            ));
        }
        "deferred" | "deferred-next" => {
            let anchor = if scenario == "deferred" {
                "anchor-one"
            } else {
                "anchor-two"
            };
            entries.push(work("Deferred tool", 3, json!({ "projectedItem": {
                "position":0, "visibility":"local", "sourceThreadId":"thread", "sourceItemId":anchor,
                "item": {
                    "id":anchor, "threadId":"thread", "runId":"run", "nodeId":null,
                    "providerThreadId":null, "providerTurnId":null, "nativeItemRef":null,
                    "parentItemId":null, "ordinal":1, "title":null, "startedAt":null,
                    "completedAt":null, "updatedAt":at(3), "type":"command_execution", "status":"completed",
                    "input":"vp test run", "output":null, "exitCode":null,
                    "deferredWork":{"cursor":"cursor", "itemCount":2, "lastOrdinal":3}
                }
            }})));
        }
        "live" => {
            entries.push(work("Thought entry", 2, json!({
                "itemType": "reasoning", "tone": "thinking", "detail": "**Checking the parser**\nMore detail"
            })));
            entries.push(work(
                "Test command",
                3,
                json!({"command":"vp test run", "toolLifecycleStatus":"inProgress"}),
            ));
        }
        _ => {
            entries.push(message(
                "Looking around first.",
                2,
                "assistant",
                "turn_start",
            ));
            entries.push(work(
                "Read package.json",
                3,
                json!({"changedFiles":["package.json"], "toolTitle":"read"}),
            ));
            entries.push(work(
                "Test command",
                4,
                json!({"command":"vp test run", "detail":"42 tests passed"}),
            ));
        }
    }
    let live = matches!(scenario, "live" | "attempt");
    if !live {
        entries.push(message("Final answer", 30, "assistant", "turn_start"));
    }
    MessagesTimelineRowsInput {
        timeline_entries: entries,
        is_working: live,
        running_run_id: live.then(|| "run".into()),
        active_turn_started_at: live.then(|| at(0)),
        latest_run: Some(TimelineLatestRun {
            run_id: "run".into(),
            status: if live { "running" } else { "completed" }.into(),
            started_at: Some(at(0)),
            completed_at: (!live).then(|| at(30)),
            turn_ended: None,
        }),
        tool_call_visibility: "showAll".into(),
        ..Default::default()
    }
}

#[component]
fn GroupsFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let scenario = signal("settled");
    let mounted = signal(true);
    let state = WorkGroupsState::default();
    let loads = signal(Vec::<String>::new());
    let mut context = WorkGroupsContext::new(state);
    context.load_deferred_work = Some(Rc::new(move |anchor| {
        loads.update(|loads| loads.push(anchor));
        async { Err("fixture transport failure".into()) }.boxed_local()
    }));
    let previous: Rc<RefCell<Option<MessagesTimelineRowsProjection>>> = Rc::default();
    let rows = computed(move || {
        let mut input = input(scenario.get());
        state.apply_to_input(&mut input);
        let next = derive_messages_timeline_rows_with_state(&input, previous.borrow().as_ref());
        let rows = Shared(next.rows.clone());
        *previous.borrow_mut() = Some(next);
        rows
    });
    let mut root = view().w_full().h_full();
    for (label, value) in [
        ("Settled example", "settled"),
        ("Steer example", "steer"),
        ("Live example", "live"),
        ("Attempt example", "attempt"),
        ("Deferred example", "deferred"),
    ] {
        root = root.child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t(label))))
                .on_press(move || {
                    scenario.set(value);
                    state.expanded_run_ids.set(Default::default());
                })
                .child(text(t(label))),
        );
    }
    root.child(
        nsh_base::button()
            .accessibility_label(Some(SharedString::from(t("Recycle rows"))))
            .on_press(move || mounted.update(|mounted| *mounted = !*mounted))
            .child(text(t("Recycle rows"))),
    )
    .child(
        nsh_base::button()
            .accessibility_label(Some(SharedString::from(t("Next deferred anchor"))))
            .on_press(move || scenario.set("deferred-next"))
            .child(text(t("Next deferred anchor"))),
    )
    .child(text(move || {
        SharedString::from(format!("Loaded anchors: {}", loads.get().join(", ")))
    }))
    .child(when(
        move || mounted.get(),
        move || {
            let keys = computed(move || {
                rows.with(|rows| rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>())
            });
            let context = context.clone();
            view().child(dynamic(move || {
                let keys = keys.get();
                Some(untracked(|| {
                    let mut column = view();
                    for key in keys {
                        let row = computed(move || {
                            rows.with(|rows| {
                                rows.iter().find(|row| row.id == key).cloned().map(Shared)
                            })
                        });
                        column = column
                            .child(work_group_timeline_row(row, context.clone()))
                            .child(dynamic(move || {
                                row.with(|row| match row.as_ref().map(|row| &row.content) {
                                    // Other row packages own messages; the fixture preserves C3's order.
                                    Some(Content::Message { message, .. }) => {
                                        Some(text(SharedString::from(message.text.clone())))
                                    }
                                    _ => None,
                                })
                            }));
                    }
                    column
                }))
            }))
        },
    ))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut registry = Registry::new();
    registry.register::<GroupsFixture>();
    sets.push(registry);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<GroupsFixture>())
    })
    .expect("work groups fixture");
    app.wait_for("Worked for 30s").expect("C3 fold mounted");
    app
}

#[test]
fn settled_work_expands_and_collapses_through_c3_and_survives_cell_recycling() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        assert!(!app.screen().shows("Looking around first."));
        assert!(!app.screen().shows("Read package.json"));
        app.press("id:turn-fold:run").expect("open settled turn");
        app.wait_for("Looking around first.")
            .expect("folded commentary restored");
        app.press("id:work-toggle:Read package.json")
            .expect("expand C3 tool summary");
        app.wait_for("vp test run")
            .expect("work entry renderer mounted");
        app.press("id:work-entry-Test command")
            .expect("open individual entry");
        app.wait_for("42 tests passed").expect("entry detail");
        app.press("role:button[Recycle rows]")
            .expect("unmount cells");
        assert!(!app.screen().shows("42 tests passed"));
        app.press("role:button[Recycle rows]")
            .expect("remount cells");
        app.wait_for("42 tests passed")
            .expect("both expansion levels survived");
        app.press("id:turn-fold:run").expect("close settled turn");
        assert!(!app.screen().shows("Looking around first."));
        assert!(!app.screen().shows("42 tests passed"));
        assert!(app.screen().shows("Final answer"));
    }
}

#[test]
fn opening_a_steer_segment_toggles_its_returned_key_independently_of_the_run() {
    let mut app = fixture(PHONE);
    app.press("role:button[Steer example]")
        .expect("steer example");
    app.wait_for("Continued for 20s")
        .expect("independent segment");
    app.press("id:turn-fold:run:steer:steer")
        .expect("open segment");
    app.wait_for("After steer").expect("segment restored");
    assert!(!app.screen().shows("Before steer"));
    app.press("id:turn-fold:run").expect("open initial work");
    app.wait_for("Before steer")
        .expect("initial segment restored");
    app.press("id:turn-fold:run:steer:steer")
        .expect("close only segment");
    assert!(!app.screen().shows("After steer"));
    assert!(app.screen().shows("Before steer"));
}

#[test]
fn the_live_activity_uses_the_client_label_and_expands_the_ordered_work_group() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.press("role:button[Live example]")
            .expect("live example");
        app.wait_for("Running vp").expect("C3 live label");
        app.wait_for("Checking the parser")
            .expect("latest thought preview");
        assert!(!app.screen().shows("vp test run"));
        app.press("role:button[Running vp]")
            .expect("expand live group");
        app.wait_for("vp test run").expect("ordered group details");
        app.press("role:button[Running vp]")
            .expect("collapse live group");
        assert!(!app.screen().shows("vp test run"));
    }
}

#[test]
fn superseded_attempts_toggle_the_attempt_set_without_opening_the_turn() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Attempt example]")
        .expect("superseded attempt");
    app.wait_for("Cut off by a steer").expect("attempt header");
    assert!(!app.screen().shows("Superseded tool"));
    app.press("id:attempt-fold:attempt-1")
        .expect("expand attempt");
    app.wait_for("Superseded tool")
        .expect("attempt's work revealed");
    app.press("id:attempt-fold:attempt-1")
        .expect("collapse attempt");
    assert!(!app.screen().shows("Superseded tool"));
}

#[test]
fn deferred_folds_retry_the_service_and_reload_changed_anchors_while_open() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Deferred example]")
        .expect("deferred history");
    app.press("id:turn-fold:run").expect("expand deferred fold");
    app.wait_for("Worked for 30s · Couldn't load work. Click to retry.")
        .expect("source's retry state");
    app.wait_for("Loaded anchors: anchor-one")
        .expect("exact anchor sent");
    app.press("id:turn-fold:run")
        .expect("retry without collapsing");
    app.wait_for("Loaded anchors: anchor-one, anchor-one")
        .expect("retried same anchor");
    app.press("role:button[Next deferred anchor]")
        .expect("new deferred slice arrives");
    app.wait_for("Loaded anchors: anchor-one, anchor-one, anchor-two")
        .expect("new anchor loaded while expanded");
}
