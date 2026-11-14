//! The context-compaction divider mounted from the client's rows: MessagesTimeline's logic turns a
//! compaction work entry into the row this package draws.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::thread_views::messages_timeline::MessagesTimelineRow;
use t3_ui::i18n::t;
use t3_ui::runtime::{Rt, Shared, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::rows::compaction::context_compaction_timeline_row;

use crate::support::{DESKTOP, PHONE};

fn row(value: Value) -> MessagesTimelineRow {
    let mut row = json!({"id":"compaction","createdAt":"2026-11-13T12:00:00Z","kind":"context-compaction"});
    row.as_object_mut()
        .unwrap()
        .extend(value.as_object().unwrap().clone());
    serde_json::from_value(row).unwrap()
}

fn control(label: &'static str, id: &str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(t(label).into()))
        .test_id(id)
        .on_press(action)
        .child(text(t(label)))
}

#[component]
fn CompactionFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    let stage = signal("live");
    let current = computed(move || {
        let value = match stage.get() {
            "live" => json!({"label":"Compacting before the cache expires…","tokenCounts":null,"active":true,
                "title":"Context automatically compacted before cache expiry"}),
            "done" => json!({"label":"Context compacted 899K → 19K tokens","tokenCounts":{"before":"899K","after":"19K"},
                "active":false,"title":"Context automatically compacted before cache expiry"}),
            _ => json!({"label":"Context compaction","tokenCounts":null,"active":false,"failed":true,
                "failure":"The provider refused to compact"}),
        };
        Some(Shared(Rc::new(row(value))))
    });
    view()
        .w_full()
        .h_full()
        .child(control("Finish", "done", move || stage.set("done")))
        .child(control("Fail", "fail", move || stage.set("failed")))
        .child(context_compaction_timeline_row(current))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<CompactionFixture>();
    sets.push(fixtures);
    Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<CompactionFixture>())
    })
    .expect("fixture")
}

/// MessagesTimeline.browser.test.tsx: "renders automatic compaction as one live divider that
/// finishes with its reason and token delta".
#[test]
fn a_live_divider_finishes_with_its_reason_and_token_delta() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("label:Compacting before the cache expires…")
            .expect("live divider named by its label");
        assert_eq!(app.screen().count("id:context-compaction"), Ok(1));
        app.press("id:done").expect("finish");
        app.wait_for(
            "label:Context automatically compacted before cache expiry 899K → 19K tokens",
        )
        .expect("the reason replaces the compacted prefix");
        app.wait_for("Context automatically compacted before cache expiry 899K")
            .expect("title and the count before");
        app.wait_for("19K tokens").expect("the count after");
        assert_eq!(app.screen().count("id:context-compaction"), Ok(1), "still one divider");
        assert!(!app.screen().shows("Compacting before the cache expires…"));
    }
}

#[test]
fn a_failed_compaction_names_its_failure() {
    let mut app = fixture(DESKTOP);
    app.press("id:fail").expect("fail");
    app.wait_for("label:Context compaction: The provider refused to compact")
        .expect("label and failure");
    app.wait_for("· The provider refused to compact")
        .expect("server detail is shown as content");
}
