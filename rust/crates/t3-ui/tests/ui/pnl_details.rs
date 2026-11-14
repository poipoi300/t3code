//! PNL-DETAILS: real client projections in the retained details target.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_ui::runtime::{Rt, demo::DemoBackend, host::Choice};
use t3_ui::screens::panel::details::{
    VersionControlSlots, thread_details_popover, thread_details_target,
};
use t3_ui::screens::panel::{Action, PanelHost, PanelStore};

use crate::support::{DESKTOP, PHONE};

fn sample(source: &str) -> Value {
    serde_json::from_str(source.lines().next().expect("canonical sample")).expect("sample JSON")
}

fn config(environment: &str, label: &str) -> t3_contracts::server::ServerConfig {
    let mut source = sample(include_str!(
        "../../../../fixtures/samples/ServerConfig.jsonl"
    ));
    source["environment"]["environmentId"] = json!(environment);
    source["environment"]["label"] = json!(label);
    source["environment"]["platform"]["machine"] = json!("desktop");
    serde_json::from_value(source).expect("canonical config")
}

fn publish_shell(
    store: &t3_client::ClientStore,
    environment: &EnvironmentId,
    session: t3_client::types::Generation,
    sequence: u64,
) {
    let mut thread = sample(include_str!(
        "../../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl"
    ));
    thread["id"] = json!("same-thread");
    thread["projectId"] = json!("project");
    thread["worktreePath"] = json!(null);
    let mut project = sample(include_str!(
        "../../../../fixtures/samples/OrchestrationProjectShell.jsonl"
    ));
    project["id"] = json!("project");
    let snapshot = json!({
        "kind":"snapshot", "snapshot": {
            "schemaVersion":1,"snapshotSequence":sequence,
            "threads":[thread],"archivedThreads":[],"projects":[project]
        }
    });
    store.apply_shell(
        environment,
        session,
        vec![serde_json::from_value(snapshot).expect("shell snapshot")],
    );
}

fn control(label: &str, on_press: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(SharedString::from(label.to_string()))
        .child(text(SharedString::from(label.to_string())))
        .on_press(on_press)
}

#[component]
fn DetailsPanelFixture() -> impl IntoElement {
    details_fixture(false)
}

#[component]
fn PopoverFixture() -> impl IntoElement {
    details_fixture(true)
}

fn details_fixture(popover: bool) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let store = t3_client::ClientStore::default();
    let first = EnvironmentId("first".into());
    let second = EnvironmentId("second".into());
    let session = store.begin_session(first.clone());
    let second_session = store.begin_session(second.clone());
    publish_shell(&store, &first, session, 1);
    publish_shell(&store, &second, second_session, 1);
    assert!(store.set_config(&first, session, config("first", "Build machine")));
    assert!(store.set_config(&second, second_session, config("second", "Other machine")));
    provide_context(Rt::new(
        store.clone(),
        Rc::new(DemoBackend::new(store.clone())),
        Choice::Demo,
    ));
    let key = ThreadKey {
        environment: first.clone(),
        thread: ThreadId("same-thread".into()),
    };
    let host = PanelHost::new(PanelStore::default(), |_| {});
    let selected = stored(key.clone());
    let inline = signal(true);
    let publishing = store.clone();
    let removing = store.clone();
    let details = if popover {
        thread_details_popover(
            host,
            key,
            nsh_base::button()
                .accessibility_label("Show details")
                .child(text("Show details")),
            600.0.into(),
            VersionControlSlots::default(),
        )
        .into_element()
    } else {
        thread_details_target(
            host,
            key,
            Prop::bind(move || inline.get()),
            600.0.into(),
            VersionControlSlots::default(),
        )
        .into_element()
    };
    view()
        .w_full()
        .h_full()
        .child(control("Toggle details", move || {
            host.toggle_thread_panel(&selected.get(), true)
        }))
        .child(control("Use popover mode", move || inline.set(false)))
        .child(control("Use inline mode", move || inline.set(true)))
        .child(control("Open popover flag", move || {
            host.apply(
                &selected.get(),
                Action::SetThreadPanel {
                    inline: false,
                    open: true,
                },
            );
        }))
        .child(control("Rename selected environment", move || {
            assert!(publishing.set_config(&first, session, config("first", "Renamed machine")));
        }))
        .child(control("Rename other environment", move || {
            assert!(store.set_config(&second, second_session, config("second", "Elsewhere")));
        }))
        .child(control("Remove selected environment", move || {
            removing.remove_environment(&EnvironmentId("first".into()))
        }))
        .child(view().w(px(280.0)).child(details))
}

fn fixture(window: (f32, f32)) -> Headless {
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<DetailsPanelFixture>())
    })
    .expect("details fixture starts")
}

#[test]
fn environment_publications_update_the_row_and_do_not_leak_across_environments() {
    for size in [DESKTOP, PHONE] {
        let mut app = fixture(size);
        app.wait_for("Build machine")
            .expect("client environment shown");
        app.press("role:button[Rename other environment]")
            .expect("publish unrelated config");
        app.press("role:button[Rename selected environment]")
            .expect("publish selected config");
        app.wait_for("Renamed machine")
            .expect("updated client name shown");
        assert!(!app.screen().shows("Build machine"));
        assert!(!app.screen().shows("Elsewhere"));
    }
}

#[test]
fn hiding_and_reopening_details_reads_the_latest_client_projection() {
    for size in [DESKTOP, PHONE] {
        let mut app = fixture(size);
        app.wait_for("Build machine")
            .expect("details default open inline");
        app.press("role:button[Toggle details]").expect("hide");
        assert!(!app.screen().shows("Build machine"));
        app.press("role:button[Rename selected environment]")
            .expect("publish while hidden");
        app.press("role:button[Toggle details]").expect("reopen");
        app.wait_for("Renamed machine")
            .expect("fresh projection after reopening");
    }
}

#[test]
fn removing_an_environment_removes_its_details_without_an_invented_fallback() {
    let mut app = fixture(DESKTOP);
    app.wait_for("Build machine").expect("details");
    app.press("role:button[Remove selected environment]")
        .expect("remove environment");
    app.wait_for("Thread details")
        .expect("target remains mounted");
    assert!(!app.screen().shows("Build machine"));
    assert!(!app.screen().shows("this machine"));
    assert!(!app.screen().shows("Open in editor"));
    assert!(!app.screen().shows("Add project script"));
}

#[test]
fn a_popover_is_closed_by_default_and_reopens_after_escape() {
    for window in [DESKTOP, PHONE] {
        let mut app = Headless::start(EngineConfig {
            ui_sets: t3_ui::registries(),
            viewport: size(px(window.0), px(window.1)),
            ..EngineConfig::new(Root::of::<PopoverFixture>())
        })
        .expect("popover fixture");
        app.wait_for("Show details").expect("anchor");
        assert!(!app.screen().shows("Build machine"));
        app.press("role:button[Show details]").expect("open");
        app.wait_for("Build machine").expect("details open");
        app.press_key("escape").expect("dismiss");
        assert!(!app.screen().shows("Build machine"));
        app.press("role:button[Show details]").expect("reopen");
        app.wait_for("Build machine").expect("details reopen");
    }
}

#[test]
fn returning_from_inline_does_not_reopen_a_stale_popover() {
    let mut app = fixture(DESKTOP);
    app.wait_for("Build machine").expect("inline initially");
    app.press("role:button[Use popover mode]")
        .expect("popover mode");
    assert!(!app.screen().shows("Build machine"));
    app.press("role:button[Open popover flag]")
        .expect("open popup");
    app.wait_for("Build machine").expect("popup content");
    app.press("role:button[Use inline mode]")
        .expect("switch inline");
    app.press("role:button[Use popover mode]")
        .expect("return to popup mode");
    assert!(!app.screen().shows("Build machine"));
}
