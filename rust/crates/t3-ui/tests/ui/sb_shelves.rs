//! SB-SHELVES: production markers driven by client projections and controlled shelf state.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use nsh_core::prelude::*;
use nsh_engine::{Clock, EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::{
    ClientStore,
    types::EnvironmentId,
    view_logic::shell::sidebar::{
        Marker, Section,
        presentation::{ActiveOrder, Options, sidebar_rows},
    },
};
use t3_ui::{
    i18n::t,
    runtime::StoreSignals,
    screens::sidebar::shelves::{
        OlderSettledState, SidebarSectionHeaderProps, sidebar_drag_boundary,
        sidebar_section_header, sidebar_section_placeholder, sidebar_settled_more,
    },
    theme,
    ui::collapsible::CollapsibleHandle,
};

use crate::support::{DESKTOP, PHONE};

fn start<C: Component<Props = ()> + 'static>(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        viewport: size(px(window.0), px(window.1)),
        ui_sets: t3_ui::registries(),
        clock: Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<C>())
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    let mut app = Headless::start(config).expect("marker fixture starts");
    app.first_render().expect("first render");
    app
}

fn thread(id: &str, extra: Value) -> Value {
    let mut value = json!({
        "id":id,"projectId":"p","title":id,
        "providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
        "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,
        "lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},
        "forkedFrom":null,"activeProviderThreadId":null,"latestRunId":null,"activeRunId":null,
        "status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,
        "hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,
        "createdBy":"user","creationSource":"web","createdAt":"2026-11-13T10:00:00.000Z",
        "updatedAt":"2026-11-13T10:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null
    });
    value
        .as_object_mut()
        .expect("thread")
        .extend(extra.as_object().expect("fields").clone());
    value
}

fn install(store: &ClientStore, environment: &EnvironmentId, extra_working: bool) {
    let mut threads = vec![
        thread(
            "working",
            json!({"latestRunId":"run","status":"running","latestRunStartedAt":"2026-11-13T11:00:00.000Z"}),
        ),
        thread(
            "nap",
            json!({"snoozedAt":"2026-11-13T10:30:00.000Z","snoozedUntil":"2026-11-14T10:00:00.000Z"}),
        ),
        thread(
            "done",
            json!({"settledOverride":"settled","settledAt":"2026-11-13T11:00:00.000Z"}),
        ),
    ];
    if extra_working {
        threads.push(thread("another", json!({"latestRunId":"other","status":"running","latestRunStartedAt":"2026-11-13T11:30:00.000Z"})));
    }
    let session = store.begin_session(environment.clone());
    store.apply_shell(environment, session, vec![serde_json::from_value(json!({
        "kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":threads,
        "projects":[{"id":"p","title":"Project","workspaceRoot":"/repo","defaultModelSelection":null,
        "scripts":[],"createdAt":"2026-11-13T10:00:00.000Z","updatedAt":"2026-11-13T10:00:00.000Z"}],"archivedThreads":[]}
    })).expect("canonical shell")]);
}

#[component]
fn Headers() -> impl IntoElement {
    theme::install(&Default::default());
    let store = ClientStore::default();
    let environment = EnvironmentId("host".into());
    install(&store, &environment, false);
    let shell = StoreSignals::attach(store.clone()).shell(environment.clone());
    let rows = computed(move || {
        shell.with(|shell| {
            let shell = shell.as_ref().expect("shell");
            sidebar_rows(
                &EnvironmentId("host".into()),
                shell,
                &[],
                &Options {
                    now: t3_client::time::epoch_ms("2026-11-13T12:00:00.000Z").expect("now"),
                    scope: None,
                    delegated_under_parent: &BTreeSet::new(),
                    selected: &BTreeSet::new(),
                    route: None,
                    working_shelf: true,
                    active_order: ActiveOrder::InboxReturn,
                    inbox: None,
                },
                None,
            )
            .rows
        })
    });
    let mut result = view().w_full().h_full();
    for (marker, section, label) in [
        (Marker::WorkingHeader, Section::Working, "Working"),
        (Marker::SnoozedHeader, Section::Snoozed, "Snoozed"),
        (Marker::SettledHeader, Section::Settled, "Settled"),
    ] {
        let open = signal(false);
        let requested = signal(false);
        let toggle = CollapsibleHandle::controlled(
            move || open.get(),
            || false,
            move |next| requested.set(next),
        );
        result = result
            .child(sidebar_section_header(SidebarSectionHeaderProps {
                marker,
                label: label.into(),
                count: Prop::bind(move || {
                    rows.with(|rows| rows.iter().filter(|row| row.section == section).count())
                }),
                class_name: None,
                dragging: false.into(),
                is_drop_target: false.into(),
                toggle,
            }))
            .child(text(move || {
                format!("{label} requested:{}", requested.get())
            }))
            .child(
                nsh_base::button()
                    .accessibility_label(format!("Accept {label}"))
                    .on_press(move || open.set(requested.get()))
                    .child(text(t("Accept"))),
            );
    }
    result.child(
        nsh_base::button()
            .accessibility_label(t("Publish another working thread"))
            .on_press(move || install(&store, &environment, true))
            .child(text(t("Publish another working thread"))),
    )
}

#[test]
fn shelf_headers_request_both_states_and_react_to_real_client_counts() {
    for window in [DESKTOP, PHONE] {
        let mut app = start::<Headers>(window);
        for label in ["Working", "Snoozed", "Settled"] {
            let selector = format!("role:button[{label}]");
            assert_eq!(
                app.screen()
                    .accessibility_state(&selector)
                    .expect("header")
                    .expanded,
                Some(false)
            );
            app.press(&selector).expect("request expansion");
            app.wait_for(&format!("text:{label} requested:true"))
                .expect("request");
            assert_eq!(
                app.screen()
                    .accessibility_state(&selector)
                    .expect("still controlled")
                    .expanded,
                Some(false)
            );
            app.press(&format!("role:button[Accept {label}]"))
                .expect("controller accepts");
            assert_eq!(
                app.screen()
                    .accessibility_state(&selector)
                    .expect("expanded")
                    .expanded,
                Some(true)
            );
            app.press(&selector).expect("request collapse");
            app.press(&format!("role:button[Accept {label}]"))
                .expect("accept collapse");
            assert_eq!(
                app.screen()
                    .accessibility_state(&selector)
                    .expect("collapsed")
                    .expanded,
                Some(false)
            );
        }
        app.wait_for("id:sidebar-working-header >> text:1")
            .expect("client count");
        let header = app
            .screen()
            .id("role:button[Working]")
            .expect("header identity");
        app.press("role:button[Publish another working thread]")
            .expect("store update");
        app.wait_for("id:sidebar-working-header >> text:2")
            .expect("new client count");
        assert_eq!(
            app.screen()
                .id("role:button[Working]")
                .expect("same header"),
            header
        );
        assert!(
            app.screen()
                .matches("id:sidebar-snoozed-header >> text:1")
                .expect("snooze unchanged")
        );
    }
}

#[component]
fn More() -> impl IntoElement {
    theme::install(&Default::default());
    let expanded = signal(true);
    let count = signal(60usize);
    let state = signal(OlderSettledState::Ready);
    let requests = signal(0);
    on_key("ctrl+f", move |_| state.set(OlderSettledState::Failed));
    view()
        .w_full()
        .h_full()
        .child(sidebar_settled_more(
            Prop::bind(move || expanded.get()),
            Prop::bind(move || count.get()),
            Prop::bind(move || state.get()),
            move || {
                requests.update(|n| *n += 1);
                state.set(OlderSettledState::Pending);
            },
        ))
        .child(text(move || format!("requests:{}", requests.get())))
        .child(
            nsh_base::button()
                .accessibility_label(t("Fail read"))
                .on_press(move || state.set(OlderSettledState::Failed))
                .child(text(t("Fail read"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Complete read"))
                .on_press(move || {
                    count.set(7);
                    state.set(OlderSettledState::Ready);
                })
                .child(text(t("Complete read"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Exhaust read"))
                .on_press(move || count.set(0))
                .child(text(t("Exhaust read"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle shelf"))
                .on_press(move || expanded.update(|v| *v = !*v))
                .child(text(t("Toggle shelf"))),
        )
}

#[test]
fn older_settled_more_keeps_its_host_and_keyboard_focus_and_blocks_duplicate_reads() {
    for window in [DESKTOP, PHONE] {
        let mut app = start::<More>(window);
        let id = app.screen().id("role:button[Show 25 more]").expect("more");
        app.press_key("tab").expect("focus more");
        app.press_key("enter").expect("start read");
        app.wait_for("role:button[Loading]").expect("pending read");
        assert_eq!(
            app.screen().id("role:button[Loading]").expect("same host"),
            id
        );
        assert!(
            app.screen()
                .accessibility_state("role:button[Loading]")
                .expect("pending read announced")
                .busy
        );
        assert!(
            app.screen()
                .enabled("role:button[Loading]")
                .expect("focusable host retained")
        );
        app.press_key("enter")
            .expect("duplicate keyboard activation");
        app.press("role:button[Loading]")
            .expect("duplicate pointer activation");
        assert!(app.screen().shows("requests:1"));
        app.press_key("ctrl+f").expect("out-of-band read rejection");
        app.wait_for("role:button[Could not load older threads. Retry]")
            .expect("retry state");
        assert_eq!(
            app.screen()
                .id("role:button[Could not load older threads. Retry]")
                .expect("same retry host"),
            id
        );
        app.press_key("enter")
            .expect("focus survived loading and rejection");
        app.wait_for("text:requests:2")
            .expect("retry actually runs");
        app.press("role:button[Complete read]")
            .expect("page arrived");
        app.wait_for("role:button[Show 7 more]")
            .expect("partial final page");
        app.press("role:button[Toggle shelf]").expect("collapse");
        assert!(
            !app.screen()
                .matches("id:sidebar-settled-more")
                .expect("absent collapsed")
        );
        app.press("role:button[Toggle shelf]").expect("reopen");
        app.wait_for("role:button[Show 7 more]").expect("restored");
        app.press("role:button[Exhaust read]").expect("exhausted");
        assert!(
            !app.screen()
                .matches("id:sidebar-settled-more")
                .expect("absent exhausted")
        );
    }
}

#[component]
fn Markers() -> impl IntoElement {
    theme::install(&Default::default());
    let dragging = signal(false);
    let target = signal(false);
    view()
        .w_full()
        .h_full()
        .child(sidebar_drag_boundary(
            Marker::PinnedHeader,
            "Pinned".into(),
            Prop::bind(move || dragging.get()),
            Prop::bind(move || target.get()),
        ))
        .child(sidebar_section_placeholder(
            Marker::SettledPlaceholder,
            "Settled".into(),
            Prop::bind(move || dragging.get()),
            Prop::bind(move || target.get()),
        ))
        .child(view().h(px(64.0)))
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle drag"))
                .on_press(move || dragging.update(|v| *v = !*v))
                .child(text(t("Toggle drag"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle target"))
                .on_press(move || target.update(|v| *v = !*v))
                .child(text(t("Toggle target"))),
        )
}

#[test]
fn drag_hints_clear_and_cancel_the_pending_boundary_reveal() {
    for window in [DESKTOP, PHONE] {
        let mut app = start::<Markers>(window);
        assert!(!app.screen().shows("Pinned"));
        assert!(!app.screen().shows("Settled"));
        app.press("role:button[Toggle drag]")
            .expect("drag observation");
        app.wait_for("text:Settled").expect("drop hint");
        app.press("role:button[Toggle drag]")
            .expect("cancel before reveal");
        app.run_for(Duration::from_millis(200))
            .expect("pass the cancelled manual deadline");
        assert!(!app.screen().shows("Pinned"));
        app.press("role:button[Toggle drag]").expect("restart drag");
        app.run_for(Duration::from_millis(200))
            .expect("step-end reveal on manual clock");
        app.wait_for("text:Pinned").expect("boundary");
        app.press("role:button[Toggle target]")
            .expect("target observation");
        assert!(app.screen().shows("Pinned"));
        app.press("role:button[Toggle drag]").expect("drag ends");
        assert!(!app.screen().shows("Pinned"));
        assert!(!app.screen().shows("Settled"));
    }
}
