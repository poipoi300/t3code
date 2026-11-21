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

#[component]
pub(super) fn LiveSidebar() -> impl IntoElement {
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
        state::nav::Route,
    };

    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("sidebar-remote", spawner.clone());
    server.set_shell(shell(&[("alpha", "Alpha thread"), ("beta", "Beta thread")]));
    server.edit_config(|config| {
        config["settings"]["snoozePresets"] = json!([{ "kind": "after", "minutes": 17 }]);
        for capability in ["threadPinning", "threadSettlement", "threadSnooze"] {
            config["environment"]["capabilities"][capability] = json!(true);
        }
    });
    for sequence in 2..12 {
        server.reply(
            "orchestration.dispatchCommand",
            Ok(json!({"sequence":sequence})),
        );
    }
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
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
                })),
        )
        .child(text(move || format!("RPC {}", commands.get())))
}

pub(super) fn start_live_sidebar(window: (f32, f32)) -> Headless {
    let mut fixtures = Registry::new();
    fixtures.register::<LiveSidebar>();
    let mut sets = t3_ui::registries();
    sets.push(fixtures);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<LiveSidebar>())
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
