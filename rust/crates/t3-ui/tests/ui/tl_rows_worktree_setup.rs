//! The worktree-setup row mounted as the timeline mounts it: a client row derived by
//! MessagesTimeline's logic from the setup snapshot, with the thread-ops binding's actions.

use std::rc::Rc;

use futures::FutureExt;
use futures::future::LocalBoxFuture;
use futures::stream::LocalBoxStream;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientError;
use t3_client::ClientStore;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, TimelineLatestRun, derive_messages_timeline_rows,
};
use t3_client::thread_views::worktree_setup::WorktreeSetupView;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::worktree_setup::WorktreeSetupSnapshot;
use t3_ui::i18n::t;
use t3_ui::runtime::services::thread_ops::{WorktreeSetupBinding, WorktreeSetupClient};
use t3_ui::runtime::{Rt, Shared, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::rows::worktree_setup::{
    WorktreeSetupContext, worktree_setup_timeline_row,
};

use crate::support::{DESKTOP, PHONE};

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("thread".into()),
    }
}

fn snapshot(phase: &str, thread: &str) -> Value {
    let running = phase == "running";
    json!({"threadId":thread,"phase":phase,"startedAt":"2026-10-26T12:00:00.000Z",
        "endedAt":if running {Value::Null} else {json!("2026-10-26T12:00:09.000Z")},
        "branch":"t3/feature","baseRef":"main","worktreePath":"/repo/.t3/worktrees/feature",
        "setupScript":{"name":"Install deps","command":"bun install","terminalId":"setup-1"},
        "stages":[
            {"id":"fetch","status":"done","startedAt":"2026-10-26T12:00:00.000Z","endedAt":"2026-10-26T12:00:03.000Z","percent":null,"detail":null,"tail":[]},
            {"id":"checkout","status":"done","startedAt":"2026-10-26T12:00:03.000Z","endedAt":"2026-10-26T12:00:04.000Z","percent":null,"detail":null,"tail":[]},
            {"id":"setup-script","status":if running {"running"} else {"failed"},"startedAt":"2026-10-26T12:00:04.000Z","endedAt":null,"percent":null,"detail":null,
                "tail":["resolving packages","error: lockfile is stale"]},
            {"id":"agent","status":"pending","startedAt":null,"endedAt":null,"percent":null,"detail":null,"tail":[]}],
        "error":if running {Value::Null} else {json!("Setup script exited with code 1")},"sequence":1})
}

/// Records what the card asked the client for, as a label the test reads on screen.
struct Recorder(Signal<Vec<String>>);
impl WorktreeSetupClient for Recorder {
    fn watch(&self, _: ThreadKey) -> LocalBoxStream<'static, WorktreeSetupView> {
        futures::stream::empty().boxed_local()
    }
    fn cancel(
        &self,
        key: &ThreadKey,
        setup: &WorktreeSetupSnapshot,
    ) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        let call = format!("cancel {} {}", key.thread.0, setup.thread_id.0);
        self.0.update(|calls| calls.push(call));
        async { Ok(true) }.boxed_local()
    }
    fn open_terminal(&self, key: &ThreadKey, _: &WorktreeSetupSnapshot, terminal: &str) -> bool {
        let call = format!("terminal {} {terminal}", key.thread.0);
        self.0.update(|calls| calls.push(call));
        true
    }
}

fn control(label: &'static str, id: &str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(t(label).into()))
        .test_id(id)
        .on_press(action)
        .child(text(t(label)))
}

#[component]
fn SetupFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    let calls = signal(Vec::<String>::new());
    let with_client = signal(true);
    let phase = signal("running");
    let other_thread = signal(false);
    let handed_off = signal(false);
    let mounted = signal(true);
    let rows = computed(move || {
        let setup = snapshot(
            phase.get(),
            if other_thread.get() { "other" } else { "thread" },
        );
        let mut input = MessagesTimelineRowsInput {
            worktree_setup: Some(if handed_off.get() {
                let mut setup = setup;
                setup["stages"][3]["status"] = json!("done");
                setup
            } else {
                setup
            }),
            ..Default::default()
        };
        if handed_off.get() {
            input.latest_run = Some(TimelineLatestRun {
                run_id: "run".into(),
                status: "running".into(),
                started_at: Some("2026-10-26T12:00:10.000Z".into()),
                ..Default::default()
            });
        }
        derive_messages_timeline_rows(&input)
    });
    let row = computed(move || {
        rows.with(|rows| {
            rows.iter()
                .find(|row| row.id == "worktree-setup-row")
                .cloned()
                .map(Shared)
        })
    });
    let recorder = WorktreeSetupBinding(Rc::new(Recorder(calls)));
    let with = WorktreeSetupContext::with_client(key(), Some(recorder));
    let without = WorktreeSetupContext::with_client(key(), None);
    view()
        .w_full()
        .h_full()
        .child(control("Recycle", "recycle", move || {
            mounted.update(|m| *m = !*m)
        }))
        .child(control("Fail", "fail", move || phase.set("failed")))
        .child(control("Other thread", "other", move || {
            other_thread.set(true)
        }))
        .child(control("Hand off", "handoff", move || handed_off.set(true)))
        .child(control("No client", "no-client", move || {
            with_client.set(false)
        }))
        .child(text(move || {
            SharedString::from(format!("Calls: {}", calls.get().join(", ")))
        }))
        .child(dynamic(move || {
            let client = with_client.get();
            mounted.get().then(|| {
                let context = if client {
                    with.clone()
                } else {
                    without.clone()
                };
                untracked(|| worktree_setup_timeline_row(row, context).into_element())
            })
        }))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<SetupFixture>();
    sets.push(fixtures);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<SetupFixture>())
    })
    .expect("fixture");
    app.wait_for("Fetch base branch").expect("stages from the client row");
    app
}

#[test]
fn a_running_setup_lists_its_stages_and_tail_and_cancels_through_the_client() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for("Install deps").expect("the script's name labels its stage");
        app.wait_for("3s").expect("a finished stage keeps its duration");
        app.wait_for("error: lockfile is stale")
            .expect("the running script's output tail");
        assert!(!app.screen().shows("Setting up worktree…"), "no header while running");
        app.press("role:button[Cancel]").expect("cancel offered while running");
        app.wait_for("Calls: cancel thread thread")
            .expect("the client cancels the shown snapshot's setup");
        app.press("role:button[Open terminal]")
            .expect("the setup script started, so its terminal opens");
        app.wait_for("text:/terminal thread setup-1/")
            .expect("the snapshot's terminal");
        assert!(app.press("role:button[Work locally]").is_err(), "server threads have no local switch");
    }
}

#[test]
fn details_open_survives_recycling_and_list_the_worktree() {
    let mut app = fixture(DESKTOP);
    assert_eq!(app.screen().count("label:t3/feature"), Ok(0));
    app.press("role:button[Details]").expect("details");
    app.wait_for("label:t3/feature").expect("branch");
    app.wait_for("bun install").expect("setup command");
    app.press("id:recycle").expect("unmount");
    app.press("id:recycle").expect("remount");
    app.wait_for("label:t3/feature").expect("keyed by row id");
}

#[test]
fn a_failed_setup_shows_its_header_and_error_and_offers_no_cancel() {
    let mut app = fixture(DESKTOP);
    app.press("id:fail").expect("fail");
    app.wait_for("Worktree setup failed").expect("header");
    app.wait_for("Setup script exited with code 1")
        .expect("server error is content");
    assert!(app.press("role:button[Cancel]").is_err());
    app.wait_for("role:button[Open terminal]")
        .expect("terminal of the failed script");
}

#[test]
fn actions_follow_the_web_when_the_capability_or_thread_is_absent() {
    let mut app = fixture(DESKTOP);
    app.press("id:other").expect("another thread's setup");
    app.wait_for("Fetch base branch").expect("still drawn");
    assert!(
        app.press("role:button[Open terminal]").is_err(),
        "terminals of another thread are not exposed"
    );
    app.press("id:no-client").expect("no client");
    app.wait_for("Details").expect("details stay");
    assert!(app.press("role:button[Cancel]").is_err());
    assert!(app.press("role:button[Open terminal]").is_err());
}

#[test]
fn a_handed_off_finished_setup_collapses_to_its_summary() {
    let mut app = fixture(DESKTOP);
    app.press("id:fail").expect("finish");
    app.press("id:handoff").expect("agent started");
    app.wait_for("Worktree setup failed").expect("summary label");
    assert!(!app.screen().shows("Fetch base branch"), "stages fold into the summary");
    assert!(app.press("role:button[Cancel]").is_err(), "embedded rows never cancel");
}
