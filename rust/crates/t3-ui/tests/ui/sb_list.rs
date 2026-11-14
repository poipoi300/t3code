//! SB-LIST: drive the production list from canonical store projections, including recycled cells.

use std::{cell::RefCell, rc::Rc};

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::{
    ClientStore,
    connection::{ConnectionFailure, ConnectionState, FailureKind, PairingInput, Phase},
    types::{EnvironmentId, ThreadKey},
};
use t3_ui::{
    runtime::{
        Rt,
        backend::{Backend, EnvironmentEntry, SendError},
        host::Choice,
    },
    screens::sidebar::{sidebar, sidebar_state},
    state::{
        nav::{Navigator, Route},
        panels::Panels,
    },
};

use crate::support::{DESKTOP, PHONE, pair, start};

struct FixtureBackend {
    entries: Rc<RefCell<Vec<EnvironmentEntry>>>,
    receiver: RefCell<Option<LocalBoxStream<'static, ()>>>,
}

impl Backend for FixtureBackend {
    fn pair(
        &self,
        _: PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>> {
        futures::future::ready(Err(ConnectionFailure::new(
            FailureKind::Unsupported,
            "Fixture has no pairing endpoint",
        )))
        .boxed_local()
    }
    fn environments(&self) -> Vec<EnvironmentEntry> {
        self.entries.borrow().clone()
    }
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.receiver
            .borrow_mut()
            .take()
            .expect("one runtime subscriber")
    }
    fn open_thread(&self, _: &ThreadKey) {}
    fn close_thread(&self, _: &ThreadKey) {}
    fn send_message(
        &self,
        _: &ThreadKey,
        _: String,
    ) -> LocalBoxFuture<'static, Result<(), SendError>> {
        futures::future::ready(Err(SendError::Disconnected)).boxed_local()
    }
}

fn project(id: &str) -> Value {
    json!({"id":id,"title":id,"workspaceRoot":format!("/workspace/{id}"),"defaultModelSelection":null,
        "scripts":[],"createdAt":"2026-11-13T10:00:00.000Z","updatedAt":"2026-11-13T10:00:00.000Z"})
}

fn thread(id: &str, title: &str, project: &str) -> Value {
    json!({
        "id":id,"projectId":project,"title":title,
        "providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
        "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,
        "lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},
        "forkedFrom":null,"activeProviderThreadId":null,"latestRunId":null,"activeRunId":null,
        "status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,
        "hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,
        "createdBy":"user","creationSource":"web","createdAt":"2026-11-13T10:00:00.000Z",
        "updatedAt":"2026-11-13T10:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null
    })
}

fn install(store: &ClientStore, environment: &str, threads: Vec<Value>) {
    let environment = EnvironmentId(environment.into());
    let session = store.begin_session(environment.clone());
    store.apply_shell(
        &environment,
        session,
        vec![serde_json::from_value(json!({
        "kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":threads,
        "projects":[project(if environment.0 == "local" { "Alpha" } else { "Beta" })],"archivedThreads":[]}
    })).expect("canonical snapshot")],
    );
}

#[component]
fn ListFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    let rows = (0..1200)
        .map(|i| thread(&format!("thread-{i:04}"), &format!("Row {i:04}"), "Alpha"))
        .collect();
    install(&store, "local", rows);
    install(
        &store,
        "remote",
        vec![thread("thread-0000", "Remote row", "Beta")],
    );
    let entries = Rc::new(RefCell::new(
        ["local", "remote"]
            .map(|id| EnvironmentEntry {
                id: EnvironmentId(id.into()),
                label: id.into(),
                routes: Vec::new(),
                damaged: false,
                state: ConnectionState {
                    phase: Phase::Connected,
                    ..Default::default()
                },
            })
            .to_vec(),
    ));
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    let backend = Rc::new(FixtureBackend {
        entries: entries.clone(),
        receiver: RefCell::new(Some(receiver.boxed_local())),
    });
    provide_context(Rt::new(store.clone(), backend, Choice::Demo));
    let nav = Navigator::new(Route::Home);
    provide_context(nav);
    provide_context(Panels::new());
    let state = sidebar_state();
    let ids = ["local", "remote"].map(|id| EnvironmentId(id.into()));
    view()
        .w_full()
        .h_full()
        .child(view().flex_1().min_h(px(0.0)).child(sidebar()))
        .child(text(move || format!("route:{}", nav.top().path())))
        .child(text(move || {
            format!(
                "new-projects:{}",
                state.with(|state| state
                    .new_thread_projects
                    .iter()
                    .map(|key| format!("{}:{}", key.environment.0, key.project.0))
                    .collect::<Vec<_>>()
                    .join(","))
            )
        }))
        .child(
            nsh_base::button()
                .accessibility_label("Disconnect")
                .child(text("Disconnect"))
                .on_press(move || {
                    for id in &ids {
                        let generation = store
                            .snapshot()
                            .environments
                            .get(id)
                            .expect("environment")
                            .session;
                        store.disconnect(id, generation);
                    }
                    for entry in entries.borrow_mut().iter_mut() {
                        entry.state.phase = Phase::Available;
                    }
                    sender.unbounded_send(()).expect("notify runtime");
                }),
        )
}

fn fixture(window: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<ListFixture>())
    })
    .expect("list fixture")
}

#[test]
fn long_lists_recycle_rows_and_search_opens_the_correct_environment() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("id:sidebar-threads").expect("production list");
        let mounted = app
            .screen()
            .count("id:sidebar-row-card")
            .expect("row count");
        assert!(
            mounted > 0 && mounted < 80,
            "mount only a viewport, got {mounted}"
        );
        app.scroll("id:sidebar-threads", px(24_000.0))
            .expect("scroll long list");
        assert!(
            app.screen()
                .count("id:sidebar-row-card")
                .expect("row count")
                < 80
        );
        app.type_text("label:Search threads", "Row 1199")
            .expect("search offscreen row");
        app.wait_for("role:button[Row 1199, Alpha]")
            .expect("recycled result reads its own data");
        app.press("role:button[Row 1199, Alpha]")
            .expect("open local row");
        app.wait_for("text:route:/local/thread-1199")
            .expect("scoped local route");
        assert!(
            app.screen()
                .selected("role:button[Row 1199, Alpha]")
                .expect("selected row")
        );
        app.press("role:button[Clear thread search]")
            .expect("clear");
        app.type_text("label:Search threads", "Remote row")
            .expect("remote search");
        app.wait_for("role:button[Remote row, Beta]")
            .expect("remote match");
        app.press("role:button[Remote row, Beta]")
            .expect("open remote");
        app.wait_for("text:route:/remote/thread-0000")
            .expect("same raw id stays scoped");
        app.press("role:button[Disconnect]").expect("disconnect");
        assert!(
            app.screen().shows("Remote row"),
            "cached rows remain available offline"
        );
    }
}

#[test]
fn project_filter_controls_membership_and_the_new_thread_context_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("id:sidebar-thread-header")
            .expect("mounted chrome");
        app.press("role:button[Filter threads by project]")
            .expect("scope menu");
        app.wait_for("role:menuitem[Beta]").expect("project option");
        app.press("role:menuitem[Beta]").expect("select project");
        app.wait_for("role:button[Remote row, Beta]")
            .expect("filtered remote member");
        assert!(!app.screen().shows("Row 0000"));
        app.press("role:button[New thread]").expect("new thread");
        app.wait_for("text:new-projects:remote:Beta")
            .expect("physical scope is retained for destination selection");
        app.press("role:button[Filter threads by project: Beta]")
            .expect("reopen");
        app.press("role:menuitem[All projects]")
            .expect("clear scope");
        assert!(
            app.screen()
                .count("id:sidebar-row-card")
                .expect("visible rows")
                > 1
        );
    }
}

#[test]
fn the_integrated_app_mounts_chrome_and_retains_sidebar_navigation() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        pair(&mut app);
        app.wait_for("id:sidebar-chrome-header")
            .expect("header mounted once");
        assert_eq!(
            app.screen().count("label:Search threads").expect("search"),
            1
        );
        assert!(app.screen().shows("Rich composer: chips, IME and paste"));
        app.press("role:button[Build the shared Rust UI slice, t3code]")
            .expect("thread");
        app.wait_for("I added clamp next to sum and the tests pass.")
            .expect("real demo pipeline");
        if window == PHONE {
            app.press("role:button[Back]").expect("back");
            app.wait_for("id:sidebar-threads").expect("list restored");
        } else {
            assert!(
                app.screen()
                    .selected("role:button[Build the shared Rust UI slice, t3code]")
                    .expect("active row")
            );
        }
    }
}
