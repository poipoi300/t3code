//! Subagent rows consume C3 projections; inspectors consume canonical client subagents.

use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::view_logic::session::TimelineEntry;
use t3_client::{
    ClientStore,
    types::{EnvironmentId, ThreadId, ThreadKey},
};
use t3_ui::runtime::{Rt, Shared, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::{
    rows::{events::EventRowContext, work::WorkRowState},
    subagents::{SubagentRow, subagent_timeline_row, subagent_tooltip_content},
};
use t3_ui::state::nav::{Navigator, Route};

const AT: &str = "2026-11-13T12:34:00Z";

fn parent() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("parent".into()),
    }
}

fn entries() -> Vec<Rc<TimelineEntry>> {
    ["first", "second"].into_iter().enumerate().map(|(at, id)| Rc::new(serde_json::from_value(json!({
        "id":id,"kind":"event","createdAt":AT,"projectedItem":{"position":at,"visibility":"local","sourceThreadId":"parent","sourceItemId":id,"item":{
            "type":"subagent","id":id,"threadId":"parent","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":at+1,"status":"completed","title":id,"startedAt":AT,"completedAt":AT,"updatedAt":AT,
            "subagentId":id,"origin":"app_owned","driver":"codex","providerInstanceId":"codex","childThreadId":format!("child-{id}"),"prompt":"Review","result":"done"
        }}
    })).expect("canonical event"))).collect()
}

#[component]
fn SubagentFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("en", &[]);
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    let nav = Navigator::new(Route::Home);
    provide_context(nav);
    let state = WorkRowState::default();
    let context = EventRowContext::new(parent(), state);
    let rows = derive_messages_timeline_rows(&MessagesTimelineRowsInput {
        timeline_entries: entries(),
        ..Default::default()
    });
    let row = rows
        .into_iter()
        .find(|row| SubagentRow::from_row(row).is_some())
        .expect("C3 group");
    let row = computed(move || Some(Shared(row.clone())));
    let mounted = signal(true);
    let inspected = signal(false);
    let running = signal(true);
    let model = signal("Copy");
    view().w_full().h_full()
        .child(nsh_base::button().test_id("recycle").child(text("Recycle")).on_press(move || mounted.update(|v| *v = !*v)))
        .child(nsh_base::button().test_id("inspect").child(text("Inspect")).on_press(move || { t3_ui::i18n::set_language("fr", &[]); inspected.set(true); }))
        .child(nsh_base::button().test_id("settle").child(text("Settle")).on_press(move || running.set(false)))
        .child(nsh_base::button().test_id("slug").child(text("Unknown model")).on_press(move || model.set("gpt-6.1-sol")))
        .child(text(move || SharedString::from(nav.top().path())))
        .child(dynamic(move || mounted.get().then(|| untracked(|| subagent_timeline_row(row, context.clone()).into_element()))))
        .child(dynamic(move || inspected.get().then(|| {
            let agent = Rc::new(serde_json::from_value(json!({
                "id":"agent","threadId":"parent","runId":null,"parentNodeId":"root","origin":"app_owned","createdBy":"agent","driver":"codex","providerInstanceId":"codex","providerThreadId":null,"childThreadId":null,"nativeTaskRef":null,"prompt":"Review","title":"Settings","model":model.get(),"status":if running.get() {"running"} else {"completed"},"progress":"Current progress","result":"Final result","startedAt":"2026-11-13T12:33:00Z","completedAt":AT,"updatedAt":AT
            })).expect("canonical agent"));
            untracked(|| subagent_tooltip_content(parent(), agent).into_element())
        })))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixture = Registry::new();
    fixture.register::<SubagentFixture>();
    sets.push(fixture);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<SubagentFixture>())
    })
    .expect("fixture");
    app.wait_for("2 subagents").expect("C3 group is mounted");
    app
}

#[test]
fn group_projection_retains_members_and_navigation_survives_recycling() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.wait_for("2 subagents").expect("group");
        app.press("role:button[2 subagents]").expect("expand");
        app.wait_for("first").expect("members");
        app.press("id:recycle").expect("unmount");
        app.press("id:recycle").expect("remount");
        app.wait_for("second").expect("keyed expansion");
        app.press("role:button[Open second]").expect("navigate");
        app.wait_for("/environment/child-second")
            .expect("scoped child");
    }
}

#[test]
fn inspector_uses_progress_while_running_result_when_settled_and_literal_names() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("id:inspect").expect("inspect");
        app.wait_for("Current progress").expect("live preview");
        assert!(app.screen().shows("Settings"));
        assert!(app.screen().shows("Copy"));
        assert!(!app.screen().shows("Final result"));
        app.press("id:settle").expect("settle");
        app.wait_for("Final result").expect("settled preview");
        app.wait_for("1m 00s").expect("client elapsed format");
        assert!(!app.screen().shows("Current progress"));
        app.press("id:slug").expect("uncatalogued model");
        app.wait_for("GPT-6.1-Sol")
            .expect("client-owned slug formatting");
    }
}

#[test]
fn expanded_production_members_mount_their_inspector_without_a_caller_renderer() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[2 subagents]").expect("expand group");
    app.wait_for("first").expect("member");
    app.hover("role:button[Open first]")
        .expect("inspect member");
    app.wait_for("Not reported")
        .expect("fallback inspector resolves its own model metadata");
}
