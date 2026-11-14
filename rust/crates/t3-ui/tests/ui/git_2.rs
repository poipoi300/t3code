//! GIT-2: branch chrome driven by the real scoped client shell projection.

use std::rc::Rc;

use nsh_core::keymap::When;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::types::{EnvironmentId, Generation, ThreadId, ThreadKey};
use t3_ui::runtime::{Rt, demo::DemoBackend, host::Choice};
use t3_ui::screens::panel::branch::branch_toolbar_branch_selector;

use crate::support::{DESKTOP, PHONE};

fn sample(source: &str) -> Value {
    serde_json::from_str(source.lines().next().expect("canonical sample")).expect("sample JSON")
}

fn publish(
    store: &ClientStore,
    environment: &EnvironmentId,
    session: Generation,
    sequence: u64,
    branch: Option<&str>,
    has_project: bool,
) {
    let mut thread = sample(include_str!(
        "../../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl"
    ));
    thread["id"] = json!("same-thread");
    thread["projectId"] = json!("project");
    thread["branch"] = json!(branch);
    thread["worktreePath"] = json!(null);
    let mut project = sample(include_str!(
        "../../../../fixtures/samples/OrchestrationProjectShell.jsonl"
    ));
    project["id"] = json!("project");
    store.apply_shell(
        environment,
        session,
        vec![
            serde_json::from_value(json!({
                "kind":"snapshot", "snapshot": {
                    "schemaVersion":1, "snapshotSequence":sequence,
                    "threads":[thread], "archivedThreads":[],
                    "projects": if has_project { vec![project] } else { vec![] }
                }
            }))
            .expect("canonical shell snapshot"),
        ],
    );
}

fn control(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(label)
        .on_press(action)
        .child(text(label))
}

#[component]
fn BranchFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = ClientStore::default();
    let environment = EnvironmentId("selected".into());
    let other = EnvironmentId("other".into());
    let session = store.begin_session(environment.clone());
    let other_session = store.begin_session(other.clone());
    publish(&store, &environment, session, 1, Some("main"), true);
    publish(&store, &other, other_session, 1, Some("elsewhere"), true);
    provide_context(Rt::new(
        store.clone(),
        Rc::new(DemoBackend::new(store.clone())),
        Choice::Demo,
    ));
    let sequence = stored(1_u64);
    let key = ThreadKey {
        environment: environment.clone(),
        thread: ThreadId("same-thread".into()),
    };
    let write = stored(store.clone());
    let selected = stored(environment);
    let change = move |branch: Option<&str>, has_project: bool| {
        let sequence = sequence.with(|sequence| {
            *sequence += 1;
            *sequence
        });
        publish(
            &write.get(),
            &selected.get(),
            session,
            sequence,
            branch,
            has_project,
        );
    };
    on_key_with(
        "alt+shift+r",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| change(Some("main"), false),
    );
    view()
        .w_full()
        .h_full()
        .child(control("Update other environment", move || {
            publish(
                &store,
                &other,
                other_session,
                2,
                Some("unrelated-update"),
                true,
            )
        }))
        .child(control("Replace branch", move || {
            change(Some("feature/next"), true)
        }))
        .child(control("Clear branch", move || change(None, true)))
        .child(control("Remove project", move || {
            change(Some("main"), false)
        }))
        .child(control("Restore project", move || {
            change(Some("restored"), true)
        }))
        .child(
            view()
                .w(px(264.0))
                .child(branch_toolbar_branch_selector(key)),
        )
}

fn fixture(window: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<BranchFixture>())
    })
    .expect("branch fixture")
}

#[test]
fn current_branch_updates_the_combobox_name_without_leaking_the_other_environments_branch() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("role:combobox[Branch: main]")
            .expect("initial branch");
        app.press("role:button[Update other environment]")
            .expect("unrelated shell update");
        app.wait_for("role:combobox[Branch: main]")
            .expect("scope retained");
        assert!(!app.screen().shows("unrelated-update"));
        app.press("role:button[Replace branch]")
            .expect("selected shell update");
        app.wait_for("role:combobox[Branch: feature/next]")
            .expect("browse-mode name refreshed");
        assert_eq!(
            app.screen()
                .count("role:combobox[Branch: main]")
                .expect("selector"),
            0
        );
        app.press("role:button[Clear branch]")
            .expect("server clears branch");
        app.wait_for("role:combobox[Branch: Select ref]")
            .expect("no stale branch fallback");
    }
}

#[test]
fn empty_picker_dismisses_resets_its_query_and_cannot_create_a_fake_branch() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("role:combobox[Branch: main]").expect("branch");
        app.press("role:combobox[Branch: main]")
            .expect("open empty picker");
        app.wait_for("No refs found.")
            .expect("source failed-query state");
        app.type_text("label:Search refs...", "new branch")
            .expect("search");
        app.press_key("enter").expect("no selectable ref");
        assert!(app.screen().shows("No refs found."));
        assert_eq!(
            app.screen()
                .count("role:combobox[Branch: main]")
                .expect("selector"),
            1
        );
        assert!(!app.screen().shows("Loading refs..."));
        app.press_key("escape").expect("dismiss");
        assert!(!app.screen().shows("No refs found."));
        app.press("role:combobox[Branch: main]").expect("reopen");
        app.wait_for("No refs found.").expect("reopened");
        assert_eq!(
            app.screen()
                .input_text("label:Search refs...")
                .expect("search input"),
            Some("")
        );
    }
}

#[test]
fn losing_the_project_removes_the_open_picker_and_restoring_it_reads_fresh_client_state() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        app.wait_for("role:combobox[Branch: main]").expect("branch");
        app.press("role:combobox[Branch: main]").expect("open");
        app.wait_for("No refs found.").expect("picker");
        app.press_key("alt+shift+r")
            .expect("publish removal while picker is open");
        assert_eq!(
            app.screen()
                .count("role:combobox[Branch: main]")
                .expect("selector"),
            0
        );
        assert!(!app.screen().shows("No refs found."));
        app.press("role:button[Restore project]").expect("restore");
        app.wait_for("role:combobox[Branch: restored]")
            .expect("fresh scoped branch");
        assert!(!app.screen().shows("No refs found."));
    }
}
