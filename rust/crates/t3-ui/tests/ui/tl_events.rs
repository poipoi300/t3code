//! Event rows mounted exactly as the timeline mounts C3 projections.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent, MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_ui::i18n::t;
use t3_ui::runtime::{Rt, Shared, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::rows::events::{EventRowContext, v2_event_timeline_row};
use t3_ui::screens::thread::rows::work::WorkRowState;
use t3_ui::state::nav::{Navigator, Route};

use crate::support::{DESKTOP, PHONE};

const AT: &str = "2026-11-13T12:34:00Z";

fn event(id: &str, kind: &str, extra: Value) -> Rc<TimelineEntry> {
    let mut item = json!({"id":id,"type":kind,"threadId":"thread","runId":null,"nodeId":null,
        "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
        "ordinal":1,"status":"completed","title":null,"startedAt":AT,"completedAt":AT,"updatedAt":AT});
    item.as_object_mut()
        .expect("item")
        .extend(extra.as_object().expect("extra").clone());
    Rc::new(serde_json::from_value(json!({"id":id,"kind":"event","createdAt":AT,
        "projectedItem":{"position":0,"visibility":"local","sourceThreadId":"thread","sourceItemId":id,"item":item}})).expect("canonical event"))
}

fn agent(id: &str, title: &str, failed: bool) -> Rc<TimelineEntry> {
    event(
        id,
        "subagent",
        json!({"subagentId":id,"origin":"app_owned","driver":"codex","providerInstanceId":"codex",
        "childThreadId":format!("child-{id}"),"prompt":"Inspect changes","title":title,
        "status":if failed {"failed"} else {"running"},"result":"- `Settings` [file](/repo)","progress":"Reviewing changes",
        "completedAt":if failed {json!(AT)} else {Value::Null}}),
    )
}

fn message(id: &str, body: &str) -> Rc<TimelineEntry> {
    Rc::new(serde_json::from_value(json!({"id":id,"kind":"message","createdAt":AT,
        "message":{"id":id,"role":"assistant","text":body,"runId":"resource-run","streaming":false,"createdAt":AT,"updatedAt":AT}})).expect("canonical assistant entry"))
}

fn control(label: &'static str, id: &str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(t(label).into()))
        .test_id(id)
        .on_press(action)
        .child(text(t(label)))
}

#[component]
fn EventFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    provide_context(Rt::new(
        store,
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    let nav = Navigator::new(Route::Home);
    provide_context(nav);
    let context = EventRowContext::new(
        ThreadKey {
            environment: EnvironmentId("environment".into()),
            thread: ThreadId("thread".into()),
        },
        WorkRowState::default(),
    );
    let mode = signal("group");
    let failed = signal(false);
    let mounted = signal(true);
    let rows = computed(move || {
        let entries = match mode.get() {
            "group" => vec![
                agent("a", "First reviewer", failed.get()),
                agent("b", "Second reviewer", false),
            ],
            "created" => vec![
                message("intro", "Creating the chat"),
                event(
                    "created",
                    "thread_created",
                    json!({"runId":"resource-run","targetThreadId":"created","targetRunId":null,"targetProviderInstanceId":"codex","targetModel":"gpt","title":"Settings"}),
                ),
                message("final", "Chat is ready"),
            ],
            "interrupt" => vec![event(
                "interrupt",
                "run_interrupt_result",
                json!({"message":"Settings"}),
            )],
            "fork" => vec![event(
                "fork",
                "fork",
                json!({"source":{"type":"run","threadId":"original","runId":"old"},"targetThreadId":"fork"}),
            )],
            "remote" => vec![event(
                "remote",
                "subagent",
                json!({"subagentId":"remote","origin":"app_owned","driver":"remote","providerInstanceId":"remote","childThreadId":null,"prompt":"Review", "title":"Remote reviewer", "result":null,"remoteChild":{"environmentId":"other","threadId":"remote-child","label":"Workstation"}}),
            )],
            "handoff" => vec![event(
                "handoff",
                "handoff",
                json!({"contextHandoffId":"context","fromProviderThreadIds":[],"toProviderThreadId":"destination","fromProviderInstanceIds":["codex"],"toProviderInstanceId":"claudeAgent","fromModelSelections":[{"instanceId":"codex","model":"Settings"}],"toModel":"Copy","strategy":"full_thread_summary"}),
            )],
            _ => Vec::new(),
        };
        derive_messages_timeline_rows(&MessagesTimelineRowsInput {
            timeline_entries: entries,
            ..Default::default()
        })
    });
    let row = computed(move || {
        rows.with(|rows| {
            rows.iter()
                .find(|r| {
                    matches!(
                        r.content,
                        MessagesTimelineRowContent::Event {
                            resource_summary: Some(true),
                            ..
                        }
                    )
                })
                .or_else(|| {
                    rows.iter()
                        .find(|r| matches!(r.content, MessagesTimelineRowContent::Event { .. }))
                })
                .cloned()
                .map(Shared)
        })
    });
    let calls = signal(0usize);
    let collapsed = signal(false);
    let mut context = context;
    context.on_toggle_entry = Some(Rc::new(move |_, was_expanded| {
        calls.update(|n| *n += 1);
        collapsed.set(was_expanded);
    }));
    let body_context = context.clone();
    view()
        .w_full()
        .h_full()
        .child(control("Toggle", "recycle", move || {
            mounted.update(|m| *m = !*m)
        }))
        .child(control("Update", "fail", move || failed.set(true)))
        .child(control("French", "fr", move || {
            t3_ui::i18n::set_language("fr", &[])
        }))
        .child(
            [
                ("Created thread", "created"),
                ("Interrupt", "interrupt"),
                ("Fork", "fork"),
                ("Remote", "remote"),
                ("Group", "group"),
                ("Handoff", "handoff"),
            ]
            .into_iter()
            .fold(view(), |parent, (label, id)| {
                parent.child(control(label, id, move || mode.set(id)))
            }),
        )
        .child(text(move || {
            SharedString::from(format!("Route: {}", nav.top().path()))
        }))
        .child(text(move || {
            SharedString::from(format!(
                "Disclosures: {}, collapsed: {}",
                calls.get(),
                collapsed.get()
            ))
        }))
        .child(dynamic(move || {
            mounted.get().then(|| {
                untracked(|| v2_event_timeline_row(row, body_context.clone()).into_element())
            })
        }))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<EventFixture>();
    sets.push(fixtures);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<EventFixture>())
    })
    .expect("fixture");
    app.wait_for("2 subagents").expect("actual C3 group");
    app
}

#[test]
fn grouped_events_expand_survive_recycling_and_navigate_to_their_own_children() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        assert!(!app.screen().shows("First reviewer"));
        app.press("role:button[2 subagents]").expect("expand");
        app.wait_for("First reviewer").expect("members mounted");
        app.wait_for("Disclosures: 1, collapsed: false")
            .expect("source callback polarity");
        app.press("id:recycle").expect("unmount");
        assert!(!app.screen().shows("First reviewer"));
        app.press("id:recycle").expect("remount");
        app.wait_for("First reviewer")
            .expect("keyed expansion survives");
        app.press("role:button[Open Second reviewer]")
            .expect("open second child");
        app.wait_for("Route: /environment/child-b")
            .expect("correct environment and child");
        app.press("role:button[2 subagents]").expect("collapse");
        assert!(!app.screen().shows("First reviewer"));
        app.wait_for("Disclosures: 2, collapsed: true")
            .expect("collapse callback polarity");
    }
}

#[test]
fn projected_status_updates_and_literal_details_are_not_translated() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[2 subagents]").expect("expand");
    app.wait_for("Reviewing changes").expect("active progress");
    app.press("id:fail").expect("settle first agent");
    app.wait_for("Settings file")
        .expect("settled result and plain markup");
    app.press("id:interrupt").expect("interrupt");
    app.press("id:fr").expect("switch language");
    app.wait_for("Settings")
        .expect("server detail remains content");
    assert!(!app.screen().shows("Paramètres"));
    app.press("id:handoff").expect("handoff endpoints");
    app.wait_for("Settings").expect("source model is literal");
    app.wait_for("Copy").expect("destination model is literal");
    assert!(!app.screen().shows("Copier"));
}

#[test]
fn created_resources_and_forks_own_their_actions_and_unconnected_remote_agents_are_static() {
    let mut app = fixture(DESKTOP);
    app.press("id:created").expect("resource");
    app.wait_for("Settings").expect("title from projection");
    app.press("role:button[Open chat]")
        .expect("row supplies action");
    app.wait_for("Route: /environment/created")
        .expect("created target");
    app.press("id:fork").expect("fork");
    app.press("role:button[Open source conversation]")
        .expect("source action");
    app.wait_for("Route: /environment/original")
        .expect("fork source target");
    app.press("id:remote").expect("remote");
    app.wait_for("Remote reviewer").expect("remote title");
    app.wait_for("on Workstation").expect("remote location");
    assert!(app.press("role:button[Open Remote reviewer]").is_err());
    assert!(!app.screen().shows("unavailable"));
}
