//! SB-ACTIONS-1: interaction tests of the actual action menu, rename input and undo notice.

use nsh_core::{
    element::Root,
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;
use std::time::SystemTime;
use t3_contracts::ipc::ContextMenuItemSchema;
use t3_ui::{i18n::t, screens::sidebar::actions::*, theme};

use crate::support::{DESKTOP, PHONE};

fn start(window: (f32, f32), root: Root) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(root)
    };
    config.environment.os = if window == PHONE {
        Os::Android
    } else {
        Os::Web
    };
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("action component fixture starts");
    app.first_render().expect("fixture renders");
    app
}

fn item(value: serde_json::Value) -> ContextMenuItemSchema {
    serde_json::from_value(value).expect("canonical context-menu fixture")
}

#[component]
fn MenuFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let unavailable = signal(false);
    let read_only = signal(false);
    let chosen = signal(String::from("none"));
    let calls = signal(0);
    let model = computed(move || {
        (!unavailable.get()).then(|| ThreadActionMenu {
            unavailable_reason: None,
            // Values are supplied by the client; this fixture does not simulate a mutation.
            items: vec![
                item(serde_json::json!({"id":"rename","label":"Rename thread","disabled":read_only.get()})),
                item(serde_json::json!({"id":"archive","label":"Archive thread","disabled":true})),
                item(serde_json::json!({"id":"delete","label":"Delete","destructive":true,"disabled":read_only.get()})),
                item(serde_json::json!({"id":"copy","label":"Copy","disabled":true,"children":[{"id":"copy-path","label":"Path"}]})),
            ],
        })
    });
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(sidebar_thread_context_menu(
            nsh_base::button()
                .test_id("thread")
                .child(text(t("Thread"))),
            model,
            move |action| {
                calls.update(|n| *n += 1);
                chosen.set(action);
            },
        ))
        .child(
            nsh_base::button()
                .accessibility_label(t("Read only"))
                .on_press(move || read_only.set(true))
                .child(text(t("Read only"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Disconnect"))
                .on_press(move || unavailable.set(true))
                .child(text(t("Disconnect"))),
        )
        .child(text(move || {
            format!("selected:{} calls:{}", chosen.get(), calls.get())
        }))
}

#[test]
fn context_menu_uses_client_availability_and_never_activates_a_disabled_action() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window, Root::of::<MenuFixture>());
        if window == PHONE {
            app.long_press("id:thread").expect("open touch menu");
        } else {
            app.right_click("id:thread").expect("open context menu");
        }
        app.wait_for("Rename thread").expect("menu mounted");
        if window == PHONE {
            app.press("role:menuitem[Delete]")
                .expect("choose from touch sheet");
        } else {
            app.press_key("home").expect("first enabled item");
            app.press_key("down").expect("skip disabled archive");
            app.press_key("enter").expect("activate delete");
        }
        app.wait_for("text:selected:delete calls:1")
            .expect("actual menu dispatch");
        app.press("role:button[Read only]")
            .expect("permission revoked");
        if window == PHONE {
            app.long_press("id:thread").expect("open touch menu");
        } else {
            app.right_click("id:thread").expect("open read-only menu");
        }
        app.press_key("home").expect("no enabled item");
        app.press_key("enter")
            .expect("disabled action does not dispatch");
        assert!(app.shows("selected:delete calls:1"));
    }
}

#[test]
fn missing_client_resource_renders_the_reason_and_no_enabled_operations() {
    let mut app = start(DESKTOP, Root::of::<MenuFixture>());
    app.press("role:button[Disconnect]")
        .expect("resource removed");
    app.right_click("id:thread").expect("open unsupported menu");
    app.wait_for("Thread actions are not available on this client.")
        .expect("honest unsupported state");
    app.press_key("home").expect("no available action");
    app.press_key("enter").expect("no action dispatched");
    assert!(app.shows("selected:none calls:0"));
}

#[component]
fn RenameFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let title = signal(String::from("Old title"));
    let open = signal(false);
    let commits = signal(0);
    let cancellations = signal(0);
    let saved = signal(String::new());
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(
            nsh_base::button()
                .accessibility_label(t("Rename thread"))
                .on_press(move || open.set(true))
                .child(text(t("Rename thread"))),
        )
        .child(dynamic(move || {
            open.get().then(|| {
                // Mount the component itself so its settled guard is exercised on real blur/unmount.
                untracked(|| {
                    thread_rename_input(
                        title,
                        move || {
                            commits.update(|n| *n += 1);
                            saved.set(title.get_untracked());
                            open.set(false);
                        },
                        move || {
                            cancellations.update(|n| *n += 1);
                            open.set(false);
                        },
                    )
                })
            })
        }))
        .child(
            nsh_base::button()
                .accessibility_label(t("Elsewhere"))
                .child(text(t("Elsewhere"))),
        )
        .child(nsh_base::input().label(t("Another field")))
        .child(text(move || {
            format!(
                "commits:{} cancellations:{} saved:{}",
                commits.get(),
                cancellations.get(),
                saved.get()
            )
        }))
}

#[test]
fn rename_commits_once_on_enter_or_blur_and_escape_never_commits() {
    for window in [PHONE, DESKTOP] {
        for end in ["enter", "escape", "blur"] {
            let mut app = start(window, Root::of::<RenameFixture>());
            app.press("role:button[Rename thread]")
                .expect("begin rename");
            app.wait_for("label:Thread title").expect("input mounted");
            // Headless::type_text appends, ignoring the native selection; clear is an explicit edit.
            app.clear("label:Thread title").expect("replace title");
            app.type_text("label:Thread title", "New title")
                .expect("edit title");
            if end == "blur" {
                app.type_text("label:Another field", "x")
                    .expect("focus another input");
            } else {
                app.press_key(end).expect("finish editing");
            }
            app.wait_for(if end == "escape" {
                "text:commits:0 cancellations:1 saved:"
            } else {
                "text:commits:1 cancellations:0 saved:New title"
            })
            .unwrap_or_else(|error| panic!("rename at {window:?}, finishing with {end}: {error}"));
            app.press("role:button[Elsewhere]")
                .expect("later focus move");
            assert!(app.screen().matches("label:Thread title") == Ok(false));
        }
    }
}

#[test]
fn composing_escape_does_not_cancel_the_rename_session() {
    let mut app = start(DESKTOP, Root::of::<RenameFixture>());
    app.press("role:button[Rename thread]")
        .expect("begin rename");
    app.compose("label:Thread title", "候補")
        .expect("start IME composition");
    app.press_key("escape").expect("IME Escape");
    assert!(app.screen().matches("label:Thread title") == Ok(true));
    assert!(app.shows("commits:0 cancellations:0 saved:"));
    app.compose("label:Thread title", "")
        .expect("end composition");
    app.press_key("escape").expect("cancel rename");
    app.wait_for("text:commits:0 cancellations:1 saved:")
        .expect("normal Escape cancels");
}

#[component]
fn PendingRenameFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let title = signal(String::from("Title"));
    let commits = signal(0);
    let cancellations = signal(0);
    view()
        .w_full()
        .h_full()
        // Retain the real input until focus moves, to exercise its own key-then-blur guard.
        .child(thread_rename_input(
            title,
            move || commits.update(|n| *n += 1),
            move || cancellations.update(|n| *n += 1),
        ))
        .child(nsh_base::input().label(t("Another field")))
        .child(text(move || {
            format!(
                "commits:{} cancellations:{}",
                commits.get(),
                cancellations.get()
            )
        }))
}

#[test]
fn the_inputs_own_settled_guard_suppresses_blur_after_either_finishing_key() {
    for key in ["enter", "escape"] {
        let mut app = start(DESKTOP, Root::of::<PendingRenameFixture>());
        app.wait_for("label:Thread title").expect("input mounted");
        app.type_text("label:Thread title", "x")
            .expect("focus the input");
        app.press_key(key).expect("finish with keyboard");
        app.type_text("label:Another field", "x")
            .expect("blur the retained input");
        app.wait_for(if key == "enter" {
            "text:commits:1 cancellations:0"
        } else {
            "text:commits:0 cancellations:1"
        })
        .expect("guard lives in the component, not the fixture");
    }
}

#[component]
fn NoticeFixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let current = signal(Some(ThreadUndoNotice {
        action: "Archived".into(),
        count: 2,
        shortcut: None,
    }));
    let calls = signal(0);
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(sidebar_thread_undo_notice(
            computed(move || current.get()),
            move || {
                calls.update(|n| *n += 1);
                current.set(None);
            },
        ))
        .child(text(move || format!("undos:{}", calls.get())))
}

#[test]
fn the_notice_invokes_client_undo_then_disappears_when_the_resource_clears() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window, Root::of::<NoticeFixture>());
        app.press("role:button[Undo]")
            .expect("undo latest client group");
        app.wait_for("text:undos:1").expect("one undo dispatched");
        assert_eq!(app.screen().matches("role:button[Undo]"), Ok(false));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SidebarScenario {
    Normal,
    ConfirmUnpin,
    DeferredBatch,
    RejectedBatch,
}

#[component]
fn LiveSidebar() -> impl IntoElement {
    live_sidebar_fixture().scenario(SidebarScenario::Normal as usize)
}

#[component]
fn ConfirmUnpinSidebar() -> impl IntoElement {
    live_sidebar_fixture().scenario(SidebarScenario::ConfirmUnpin as usize)
}

#[component]
fn DeferredBatchSidebar() -> impl IntoElement {
    live_sidebar_fixture().scenario(SidebarScenario::DeferredBatch as usize)
}

#[component]
fn RejectedBatchSidebar() -> impl IntoElement {
    live_sidebar_fixture().scenario(SidebarScenario::RejectedBatch as usize)
}

#[derive(Clone, PartialEq, Props)]
struct LiveSidebarFixtureProps {
    scenario: usize,
}

#[component]
fn LiveSidebarFixture(props: LiveSidebarFixtureProps) -> impl IntoElement {
    use std::rc::Rc;

    use futures::FutureExt;
    use nsh_core::keymap::When;
    use serde_json::json;
    use t3_client::{
        ClientStore,
        composition::{ClientRuntime, Spawner},
        testing::environment::{FakeEnvironment, shell},
    };
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_ui::{
        app::{app_tree, provide_client_services},
        runtime::{Rt, host::Choice},
        screens::sidebar::row::RowThreadActionsRuntime,
        state::nav::{Route, navigator},
    };

    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("sidebar-remote", spawner.clone());
    let mut initial = shell(&[("alpha", "Alpha thread"), ("beta", "Beta thread")]);
    if props.scenario == SidebarScenario::ConfirmUnpin as usize {
        for thread in initial["threads"].as_array_mut().expect("thread rows") {
            thread["pinnedAt"] = json!("2026-11-21T00:00:00.000Z");
        }
    }
    server.set_shell(initial);
    server.edit_config(|config| {
        config["settings"]["snoozePresets"] = json!([{ "kind": "after", "minutes": 17 }]);
        for capability in ["threadPinning", "threadSettlement", "threadSnooze"] {
            config["environment"]["capabilities"][capability] = json!(true);
        }
    });
    let dispatch = "orchestration.dispatchCommand";
    let first_reply = (props.scenario == SidebarScenario::DeferredBatch as usize)
        .then(|| server.defer_reply(dispatch));
    let first_reply = stored(first_reply);
    on_key_with(
        "f5",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            if let Some(reply) = first_reply.with(Option::take) {
                reply
                    .send(Ok(json!({"sequence":101})))
                    .expect("complete first mutation");
            }
        },
    );
    if props.scenario == SidebarScenario::RejectedBatch as usize {
        for _ in 0..2 {
            server.reply(dispatch, Err(json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":"fixture-command","commandType":"thread.delete","message":"Fixture rejected the action"})));
        }
    }
    for sequence in 2..12 {
        server.reply(
            "orchestration.dispatchCommand",
            Ok(json!({"sequence":sequence})),
        );
    }
    let store = ClientStore::default();
    let platform = server.platform();
    let clock = platform.fake.clone();
    on_key_with(
        "f7",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| clock.advance(5_000),
    );
    let remote = store.clone();
    on_key_with(
        "f8",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            let id = t3_client::types::EnvironmentId("sidebar-remote".into());
            let session = remote.snapshot().environments[&id].session;
            remote.apply_shell(
                &id,
                session,
                vec![
                    serde_json::from_value(json!({
                        "kind": "thread.removed", "threadId":"beta", "location":"active", "sequence":100
                    }))
                    .expect("server shell delta"),
                ],
            );
        },
    );
    let client = ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    if props.scenario == SidebarScenario::ConfirmUnpin as usize {
        let client = client.clone();
        on_key_with(
            "f4",
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| {
                client
                    .client(&t3_client::types::EnvironmentId("sidebar-remote".into()))
                    .and_then(|client| client.session())
                    .expect("connected fixture")
                    .update_grants(t3_client::permissions::SessionGrants {
                        scopes: ["orchestration:read".into()].into(),
                        valid: true,
                    });
            },
        );
    }
    let pairing = client.pair(server.pairing());
    let prefs = client.composer_preferences();
    spawner(
        async move {
            if props.scenario == SidebarScenario::ConfirmUnpin as usize {
                prefs
                    .update(|prefs| prefs.confirm_thread_unpin = true)
                    .await
                    .expect("enable unpin confirmation");
            }
            pairing.await.expect("pair sidebar environment");
        }
        .boxed_local(),
    );
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    let commands = signal(String::new());
    on_key_with(
        "f9",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            commands.set(
                server
                    .calls("orchestration.dispatchCommand")
                    .into_iter()
                    .filter(|payload| payload["type"] != "thread.visit")
                    .map(|payload| {
                        format!(
                            "{}:{}",
                            payload["type"].as_str().unwrap_or_default(),
                            payload["threadId"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        },
    );
    let route = stored(None);
    let panel_state = stored(None::<Memo<t3_client::view_logic::shell::panels::Panel>>);
    let panel_host = stored(None::<t3_ui::screens::panel::PanelHost>);
    let alpha = t3_client::types::ThreadKey {
        environment: t3_client::types::EnvironmentId("sidebar-remote".into()),
        thread: t3_contracts::base_schemas::ThreadId("alpha".into()),
    };
    let terminal_client = client.clone();
    let test_alpha = alpha.clone();
    on_key_with(
        "f6",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            terminal_client.terminals().set_open(&test_alpha, true);
            panel_host.get().expect("panel host mounted").apply(
                &test_alpha,
                t3_client::view_logic::shell::panels::Action::Open("files".into()),
            );
        },
    );
    let panels_open = move || {
        panel_state
            .get()
            .is_some_and(|panel| panel.with(|panel| !panel.surfaces.is_empty()))
    };
    let selection = stored(None);
    view()
        .w_full()
        .h_full()
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .child(app_tree(runtime, Route::Home, move || {
                    provide_client_services(&client);
                    provide_context(RowThreadActionsRuntime(Rc::new(client)));
                    route.replace(Some(navigator()));
                    let host = t3_ui::screens::panel::workspace_panel_host();
                    panel_host.replace(Some(host));
                    panel_state.replace(Some(host.panel(alpha)));
                    selection.replace(Some(
                        t3_ui::screens::sidebar::selection::use_thread_selection_store(),
                    ));
                })),
        )
        .child(text(move || format!("RPC {}", commands.get())))
        .child(text(move || {
            route.get().map_or_else(String::new, |nav| match nav.top() {
                Route::Draft(_) => "Draft route".into(),
                route => format!("Route {}", route.path()),
            })
        }))
        .child(text(move || {
            if panels_open() {
                "Panel open"
            } else {
                "Panel closed"
            }
        }))
        .child(text(move || {
            format!(
                "Selection {}",
                selection
                    .get()
                    .map_or(0, |s| s.selected_thread_keys().len())
            )
        }))
}

pub(super) fn start_live_sidebar(window: (f32, f32)) -> Headless {
    start_sidebar(window, Root::of::<LiveSidebar>())
}

fn start_sidebar(window: (f32, f32), root: Root) -> Headless {
    let mut fixtures = Registry::new();
    fixtures.register::<LiveSidebar>();
    fixtures.register::<LiveSidebarFixture>();
    fixtures.register::<ConfirmUnpinSidebar>();
    fixtures.register::<DeferredBatchSidebar>();
    fixtures.register::<RejectedBatchSidebar>();
    let mut sets = t3_ui::registries();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(root)
    };
    config.environment.os = if window == PHONE {
        Os::Android
    } else {
        Os::Linux
    };
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("start live sidebar");
    app.wait_for("role:button[Alpha thread, Project]")
        .expect("paired sidebar");
    app
}

pub(super) fn open_row_menu(app: &mut Headless, window: (f32, f32), title: &str) {
    let row = format!("role:button[{title}, Project]");
    if window == PHONE {
        app.long_press(&row).expect("open touch menu");
    } else {
        app.right_click(&row).expect("open context menu");
    }
}

pub(super) fn assert_commands(app: &mut Headless, expected: &str) {
    app.press_key("f9").expect("read RPC trace");
    app.wait_for(&format!("text:RPC {expected}"))
        .expect("expected thread commands");
}

#[test]
fn live_sidebar_archives_and_undoes_from_the_footer_desktop() {
    archives_and_undoes_from_the_footer(DESKTOP);
}

#[test]
fn live_sidebar_archives_and_undoes_from_the_footer_phone() {
    archives_and_undoes_from_the_footer(PHONE);
}

fn archives_and_undoes_from_the_footer(window: (f32, f32)) {
    let mut app = start_live_sidebar(window);
    open_row_menu(&mut app, window, "Alpha thread");
    app.press("role:menuitem[Archive thread]")
        .expect("archive from row");
    app.wait_for("Archived,")
        .expect("footer announces accepted archive");
    app.press("role:button[Ctrl+Z to undo]")
        .expect("undo from footer");
    assert_commands(&mut app, "thread.archive:alpha thread.unarchive:alpha");
}

#[test]
fn live_sidebar_renames_pins_and_confirms_permanent_deletion_desktop() {
    renames_pins_and_confirms_permanent_deletion(DESKTOP);
}

#[test]
fn live_sidebar_renames_pins_and_confirms_permanent_deletion_phone() {
    renames_pins_and_confirms_permanent_deletion(PHONE);
}

fn renames_pins_and_confirms_permanent_deletion(window: (f32, f32)) {
    let mut app = start_live_sidebar(window);
    open_row_menu(&mut app, window, "Alpha thread");
    app.press("role:menuitem[Rename thread]")
        .expect("rename from row");
    app.clear("label:Thread title").expect("replace title");
    app.type_text("label:Thread title", "Renamed thread")
        .expect("enter new title");
    app.press_key("enter").expect("save title");
    assert_commands(&mut app, "thread.metadata.update:alpha");

    open_row_menu(&mut app, window, "Beta thread");
    app.press("role:menuitem[Pin thread]")
        .expect("pin from row");
    assert_commands(&mut app, "thread.metadata.update:alpha thread.pin:beta");

    open_row_menu(&mut app, window, "Beta thread");
    app.press("role:menuitem[Delete]").expect("delete from row");
    app.wait_for("This permanently clears conversation history for this thread.")
        .expect("confirmation before sending delete");
    assert_commands(&mut app, "thread.metadata.update:alpha thread.pin:beta");
    app.press("role:button[Confirm]")
        .expect("confirm permanent delete");
    assert_commands(
        &mut app,
        "thread.metadata.update:alpha thread.pin:beta thread.delete:beta",
    );
}

#[test]
fn live_sidebar_undo_shortcut_uses_the_client_notice() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_live_sidebar(window);
        open_row_menu(&mut app, window, "Alpha thread");
        app.press("role:menuitem[Archive thread]")
            .expect("archive from row");
        app.wait_for("Ctrl+Z to undo")
            .expect("configured shortcut in footer");
        app.press_key("ctrl+z")
            .expect("undo with the configured keys");
        assert_commands(&mut app, "thread.archive:alpha thread.unarchive:alpha");
    }
}

#[test]
fn live_sidebar_archiving_the_active_thread_opens_a_draft_and_undo_returns() {
    let mut app = start_live_sidebar(DESKTOP);
    app.press("role:button[Alpha thread, Project]")
        .expect("open Alpha");
    open_row_menu(&mut app, DESKTOP, "Alpha thread");
    app.press("role:menuitem[Archive thread]")
        .expect("archive active Alpha");
    app.wait_for("Draft route")
        .expect("new draft in the same project");
    app.press("role:button[Ctrl+Z to undo]")
        .expect("restore Alpha");
    app.wait_for("Route /sidebar-remote/alpha")
        .expect("return to restored Alpha");
    assert_commands(&mut app, "thread.archive:alpha thread.unarchive:alpha");
}

#[test]
fn live_sidebar_cannot_undo_after_five_seconds() {
    let mut app = start_live_sidebar(DESKTOP);
    open_row_menu(&mut app, DESKTOP, "Alpha thread");
    app.press("role:menuitem[Archive thread]")
        .expect("archive Alpha");
    app.wait_for("Archived,").expect("undo initially available");
    app.press_key("f7")
        .expect("advance the client clock by five seconds");
    assert!(!app.screen().shows("Ctrl+Z to undo"));
    app.press_key("ctrl+z").expect("press expired shortcut");
    assert_commands(&mut app, "thread.archive:alpha");
}

#[test]
fn live_sidebar_deleting_the_active_thread_opens_the_survivor_and_forgets_its_panel() {
    let mut app = start_live_sidebar(DESKTOP);
    app.press("role:button[Alpha thread, Project]")
        .expect("open Alpha");
    app.press_key("f6")
        .expect("open Alpha's panel and terminal");
    app.wait_for("Panel open").expect("panel state present");
    open_row_menu(&mut app, DESKTOP, "Alpha thread");
    app.press("role:menuitem[Delete]")
        .expect("delete active Alpha");
    app.press("role:button[Confirm]").expect("confirm deletion");
    app.wait_for("Route /sidebar-remote/beta")
        .expect("open the survivor");
    app.wait_for("Panel closed")
        .expect("forget the deleted thread's panel");
}

#[component]
fn DesktopThemeLaunch() -> impl IntoElement {
    use serde_json::json;
    use std::rc::Rc;
    use t3_client::{
        ClientStore,
        composition::{ClientRuntime, Spawner},
        testing::environment::FakeEnvironment,
        types::EnvironmentId,
    };
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_ui::{
        app::{app_tree, launch_tree, provide_client_services},
        runtime::{Rt, host::Choice},
        state::nav::{Route, SettingsPage},
    };

    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let host = FakeEnvironment::new("desktop-host", spawner.clone());
    let primary =
        FakeEnvironment::at_origin("actual-primary", "https://primary.test", spawner.clone());
    for (environment, name) in [(&host, "Desktop palette"), (&primary, "Primary palette")] {
        environment.edit_config(|config| {
            config["environmentThemes"] = json!([{"id":"dark-test","name":name,"appearance":"dark","canvas":"#222233","accent":"#dd66aa"}]);
        });
    }
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        host.platform().with_environment(&primary),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner,
    );
    let primary_id = EnvironmentId("actual-primary".into());
    let route_client = client.clone();
    on_key("f9", move |_| {
        route_client.supply_test_primary_route(&primary_id)
    });
    let ready = stored(None::<t3_ui::theme::ThemeState>);
    view()
        .w_full()
        .h_full()
        .child(launch_tree(
            || async { Some(EnvironmentId("desktop-host".into())) },
            async move {
                client
                    .pair(host.pairing())
                    .await
                    .expect("pair desktop host");
                client
                    .pair(primary.pairing())
                    .await
                    .expect("pair future primary");
                assert!(client.settings().primary_environment().is_none());
                move || {
                    app_tree(
                        Rt::new(store, Rc::new(client.clone()), Choice::Client),
                        Route::Settings(SettingsPage::Appearance),
                        move || {
                            provide_client_services(&client);
                            ready.replace(use_context::<t3_ui::theme::ThemeState>());
                        },
                    )
                }
            },
        ))
        .child(text(move || {
            ready.get().map_or_else(String::new, |theme| {
                theme.choice.with(|choice| {
                    choice
                        .environment
                        .iter()
                        .map(|theme| theme.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
            })
        }))
}

#[test]
fn desktop_launch_uses_the_host_theme_without_a_primary_and_prefers_the_primary_when_known() {
    let mut fixtures = Registry::new();
    fixtures.register::<DesktopThemeLaunch>();
    let mut sets = t3_ui::registries();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<DesktopThemeLaunch>())
    };
    config.environment.os = Os::Linux;
    let mut app = Headless::start(config).expect("start desktop theme launch");
    app.wait_for("text:Desktop palette")
        .expect("read and bind host theme without a primary");
    app.press_key("f9").expect("discover a primary environment");
    app.wait_for("text:Primary palette")
        .expect("use the primary palette");
    assert!(!app.screen().shows("text:Desktop palette"));
}

#[test]
fn live_sidebar_preserves_selection_when_batch_unpin_is_cancelled() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_sidebar(window, Root::of::<ConfirmUnpinSidebar>());
        super::sb_select::select_two(&mut app);
        open_row_menu(&mut app, window, "Alpha thread");
        app.press("role:menuitem[Unpin (2)]")
            .expect("unpin both selected threads");
        app.wait_for("Unpin 2 threads?")
            .expect("confirm batch unpin");
        app.press("role:button[Cancel]").expect("cancel unpin");
        app.wait_for("Selection 2").expect("keep both selected");
        assert_commands(&mut app, "");
    }
}

#[test]
fn live_sidebar_rechecks_batch_eligibility_after_the_first_rpc() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_sidebar(window, Root::of::<DeferredBatchSidebar>());
        super::sb_select::select_two(&mut app);
        open_row_menu(&mut app, window, "Alpha thread");
        app.press("role:menuitem[Delete (2)]")
            .expect("delete selection");
        app.press("role:button[Confirm]")
            .expect("approve batch deletion");
        assert_commands(&mut app, "thread.delete:alpha");
        app.press_key("f8")
            .expect("remove Beta on the server while Alpha is in flight");
        app.wait_for("Selection 1").expect("prune the removed row");
        app.press_key("f5").expect("finish deleting Alpha");
        app.wait_for("Selection 0")
            .expect("clear the accepted selection");
        assert_commands(&mut app, "thread.delete:alpha");
    }
}

#[test]
fn live_sidebar_reports_the_batch_failure_count() {
    let mut app = start_sidebar(DESKTOP, Root::of::<RejectedBatchSidebar>());
    super::sb_select::select_two(&mut app);
    open_row_menu(&mut app, DESKTOP, "Alpha thread");
    app.press("role:menuitem[Delete (2)]")
        .expect("delete selection");
    app.press("role:button[Confirm]")
        .expect("approve batch deletion");
    app.wait_for("Failed to delete 2 threads")
        .expect("report both failures");
    app.wait_for("Selection 2")
        .expect("leave failed rows selected");
}

#[test]
fn live_sidebar_keeps_a_new_selection_made_while_a_batch_is_pending() {
    let mut app = start_sidebar(DESKTOP, Root::of::<DeferredBatchSidebar>());
    super::sb_select::select_two(&mut app);
    open_row_menu(&mut app, DESKTOP, "Alpha thread");
    app.press("role:menuitem[Settle (2)]")
        .expect("settle both threads");
    assert_commands(&mut app, "thread.settle:alpha");
    app.press_key("escape").expect("clear the old selection");
    app.wait_for("Selection 0").expect("old selection is gone");
    super::sb_select::select_two(&mut app);
    app.wait_for("Selection 2")
        .expect("new selection made during request");
    app.press_key("f5").expect("finish first RPC");
    app.wait_for("Settled 2,").expect("both mutations accepted");
    app.wait_for("Selection 2")
        .expect("late response leaves the new selection intact");
}

#[test]
fn live_sidebar_preserves_selection_if_permission_is_revoked_during_batch_confirmation() {
    let mut app = start_sidebar(DESKTOP, Root::of::<ConfirmUnpinSidebar>());
    super::sb_select::select_two(&mut app);
    open_row_menu(&mut app, DESKTOP, "Alpha thread");
    app.press("role:menuitem[Unpin (2)]").expect("unpin batch");
    app.wait_for("Unpin 2 threads?").expect("confirm unpin");
    app.press_key("f4").expect("revoke operation permission");
    app.press("role:button[Confirm]")
        .expect("submit confirmation");
    app.wait_for("Failed to unpin thread")
        .expect("report permission failure");
    app.wait_for("Selection 2")
        .expect("preserve selection when the operation is denied");
    assert_commands(&mut app, "");
}
