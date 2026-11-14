//! Port of CommandPalette.browser.test.tsx and commandPaletteCommands.browser.test.tsx shell cases.

use nsh_core::platform::{Environment, Idiom, Os};
use nsh_core::{keymap::When, prelude::*};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::view_logic::shell::shortcuts::DispatchEvent;
use t3_ui::i18n::t;
use t3_ui::screens::palette::{
    Intent, OpenDetail, OpenPage, OverlayMode, UiAction, command_palette, open_command_palette,
    palette_controller,
};
use t3_ui::theme::{self, ThemeChoice};

use crate::support::{DESKTOP, PHONE};

#[component]
fn PaletteFixture() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let palette = palette_controller();
    let mounted = signal(true);
    let workspace_value = signal(String::new());
    let command_claimed = signal(false);
    for (stroke, mode) in [("f3", OverlayMode::Files), ("f4", OverlayMode::Content)] {
        on_key_with(
            stroke,
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| {
                palette.open_child_search(
                    mode,
                    t3_ui::screens::palette::ReturnContext {
                        query: "https://github.com/acme/web/pull/7".into(),
                        linked_search: Some(Intent::Search {
                            query: "https://github.com/acme/web/pull/7".into(),
                            linked_threads: Some(vec![]),
                        }),
                    },
                );
            },
        );
    }
    on_key_with(
        "f5",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            palette.dispatch(UiAction::OpenIntent(Intent::Search {
                query: "replacement".into(),
                linked_threads: None,
            }));
        },
    );
    on_key_with(
        "f6",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| mounted.update(|mounted| *mounted = !*mounted),
    );
    let file_toggle = palette
        .command_handlers()
        .remove("filePicker.toggle")
        .expect("file toggle handler");
    on_key_with(
        "f7",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            let mut event = DispatchEvent {
                key: Default::default(),
                default_prevented: false,
                propagation_stopped: false,
                inside_confirmation: false,
            };
            let consumed = file_toggle(&mut event);
            command_claimed.set(consumed && event.default_prevented && event.propagation_stopped);
        },
    );
    let theme_select = palette
        .command_handlers()
        .remove("theme.select")
        .expect("theme picker handler");
    on_key_with(
        "f9",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| {
            let mut event = DispatchEvent {
                key: Default::default(),
                default_prevented: false,
                propagation_stopped: false,
                inside_confirmation: false,
            };
            event.key.repeat = true;
            let consumed = theme_select(&mut event);
            command_claimed.set(consumed && event.default_prevented && event.propagation_stopped);
        },
    );
    on_key_with(
        "f8",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| palette.dispatch(UiAction::SetOpen(true)),
    );
    view()
        .w_full()
        .h_full()
        .child(text(move || format!("mounted: {}", palette.is_mounted())))
        .child(text(move || format!("presented: {}", palette.is_open())))
        .child(text(move || format!("mode: {:?}", palette.state().mode)))
        .child(text(move || format!("claimed: {}", command_claimed.get())))
        .child(when(mounted, move || {
            untracked(move || {
                command_palette(
                    view()
                        .w_full()
                        .h_full()
                        .child(
                            nsh_base::input()
                                .label(move || SharedString::from(t("Workspace")))
                                .value(move || workspace_value.get())
                                .on_change(move |value: &str| {
                                    workspace_value.set(value.to_owned())
                                }),
                        )
                        .child(trigger("Open", OpenDetail::default))
                        .child(trigger("New thread in", || OpenDetail {
                            open: Some(OpenPage::NewThreadIn),
                            ..Default::default()
                        }))
                        .child(trigger("Search", || OpenDetail {
                            query: Some("acme".into()),
                            ..Default::default()
                        })),
                )
            })
        }))
}

fn trigger(label: &'static str, detail: impl Fn() -> OpenDetail + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(move || Some(SharedString::from(t(label))))
        .on_press(move || open_command_palette(detail()))
        .child(text(move || SharedString::from(t(label))))
}

fn fixture(size_: (f32, f32)) -> Headless {
    // A viewport alone leaves NSH's headless environment on iOS, without desktop modal focus.
    fixture_on(size_, Os::Web)
}

fn fixture_on(size_: (f32, f32), os: Os) -> Headless {
    let mut registry = Registry::new();
    registry.register::<PaletteFixture>();
    let mut app = Headless::start(EngineConfig {
        ui_sets: vec![nsh_base::registry(), registry],
        viewport: size(px(size_.0), px(size_.1)),
        environment: Environment {
            os,
            idiom: if size_ == PHONE {
                Idiom::Phone
            } else {
                Idiom::Desktop
            },
            ..Default::default()
        },
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<PaletteFixture>())
    })
    .expect("palette fixture starts");
    app.wait_for("role:button[Open]")
        .expect("palette fixture mounts");
    app
}

fn input_value(app: &Headless) -> String {
    app.screen()
        .input_text("role:combobox[Command palette]")
        .expect("palette query")
        .expect("input text")
        .to_owned()
}

#[test]
fn a_new_thread_intent_focuses_the_search_input_and_back_restores_root() {
    for size_ in [PHONE, DESKTOP] {
        let mut app = fixture(size_);
        app.press("role:button[New thread in]")
            .expect("open submenu intent");
        app.wait_for("Search...").expect("submenu input");
        app.press_key("r")
            .expect("type without explicitly focusing input");
        assert_eq!(input_value(&app), "r");
        app.type_text("role:combobox[Command palette]", "emote project")
            .expect("continue typing");
        assert_eq!(input_value(&app), "remote project");
        app.press_key("backspace").expect("delete within submenu");
        assert_eq!(input_value(&app), "remote projec");
        assert!(
            app.screen().shows("Backspace"),
            "typing does not pop the submenu"
        );
        app.clear("role:combobox[Command palette]")
            .expect("clear query");
        app.press_key("backspace").expect("empty submenu goes back");
        app.wait_for("Search commands, projects, and threads...")
            .expect("root");
        app.press_key("escape").expect("root closes");
        assert_eq!(app.screen().count("role:dialog"), Ok(0));
    }
}

#[test]
fn a_search_intent_seeds_the_input_and_another_intent_replaces_the_open_session() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Search]").expect("search intent");
    assert_eq!(input_value(&app), "acme");
    app.press_key("f5").expect("new intent while open");
    app.wait_for("replacement").expect("new query");
    assert_eq!(input_value(&app), "replacement");
    app.press_key("escape").expect("close");
    app.press("role:button[Open]").expect("plain reopen");
    assert_eq!(input_value(&app), "");
}

#[test]
fn child_search_escape_restores_the_parent_and_the_replacement_input_owns_typing() {
    for size_ in [PHONE, DESKTOP] {
        for key in ["f3", "f4"] {
            let mut app = fixture(size_);
            app.press("role:button[Search]").expect("open parent");
            let dialog = app.screen().id("role:dialog").expect("modal");
            app.press_key(key).expect("open child search");
            assert_eq!(app.screen().count("role:combobox"), Ok(0));
            app.press_key("escape").expect("return from child");
            assert_eq!(
                app.screen().id("role:dialog"),
                Ok(dialog),
                "mode changes reuse modal"
            );
            assert_eq!(input_value(&app), "https://github.com/acme/web/pull/7");
            app.press_key("x").expect("type into remounted input");
            assert!(input_value(&app).contains('x'));
            assert_eq!(app.screen().input_text("label:Workspace"), Ok(Some("")));
            app.press_key("escape").expect("then close root");
            assert_eq!(app.screen().count("role:dialog"), Ok(0));
        }
    }
}

#[test]
fn outside_closes_a_child_and_discards_the_return_context() {
    for size_ in [PHONE, DESKTOP] {
        let mut app = fixture(size_);
        app.press("role:button[Search]").expect("open");
        app.press_key("f3").expect("child");
        let backdrop = app
            .screen()
            .id("id:command-dialog-backdrop")
            .expect("backdrop");
        app.engine()
            .control()
            .dispatch_event(backdrop, EventKind::PressDown, EventPayload::None);
        app.wait_for("text:presented: false")
            .expect("outside dismissal completes");
        assert_eq!(app.screen().count("role:dialog"), Ok(0));
        app.press("role:button[Open]").expect("fresh root");
        assert_eq!(input_value(&app), "");
    }
}

#[test]
fn requests_without_a_mounted_palette_are_dropped_and_unmount_clears_open_state() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Search]").expect("open");
    app.press_key("f6").expect("unmount shell while open");
    app.wait_for("text:mounted: false")
        .expect("mount counter reset");
    assert_eq!(app.screen().count("role:dialog"), Ok(0));
    app.press_key("f5")
        .expect("ignored intent while no palette mounted");
    app.press_key("f8")
        .expect("ignored plain open while absent");
    app.press_key("f7")
        .expect("registered overlay handler also stays inert");
    assert!(
        app.screen().shows("claimed: false"),
        "an absent palette does not claim the shortcut"
    );
    app.press_key("f6").expect("remount shell");
    app.wait_for("text:mounted: true").expect("mounted again");
    assert_eq!(
        app.screen().count("role:dialog"),
        Ok(0),
        "no stale request pops later"
    );
    app.press("role:button[Open]")
        .expect("open usable fresh shell");
    assert_eq!(input_value(&app), "");
}

#[test]
fn toggling_an_open_child_mode_closes_it_and_the_next_toggle_opens_cleanly() {
    let mut app = fixture(PHONE);
    app.press_key("f7").expect("open files directly");
    assert!(app.screen().shows("claimed: true"));
    app.wait_for("text:mode: Files").expect("files mode");
    app.press_key("f7").expect("same mode closes");
    assert_eq!(app.screen().count("role:dialog"), Ok(0));
    app.press_key("f7").expect("reopen files");
    app.press_key("escape")
        .expect("direct child escapes to root");
    assert_eq!(input_value(&app), "");
    app.type_text("role:combobox[Command palette]", "retained")
        .expect("query before repeat");
    app.press_key("f9")
        .expect("repeat of theme shortcut is consumed without replacing root");
    assert_eq!(input_value(&app), "retained");
    assert!(app.screen().shows("claimed: true"));
}

#[test]
fn dismissal_returns_keyboard_focus_to_the_unchanged_workspace() {
    for os in [Os::Windows, Os::Web] {
        let mut app = fixture_on(DESKTOP, os);
        app.type_text("label:Workspace", "draft")
            .expect("workspace input takes focus");
        app.press_key("f8")
            .expect("open without changing opener focus");
        app.press_key("p").expect("palette input takes typing");
        assert_eq!(input_value(&app), "p");
        app.press_key("escape").expect("close palette");
        app.press_key("x")
            .expect("type after returning to workspace");
        assert_eq!(
            app.screen().input_text("label:Workspace"),
            Ok(Some("draftx"))
        );
    }
}

#[test]
fn outside_dismissal_restores_the_workspace_input_on_desktop_and_web() {
    for os in [Os::Windows, Os::Web] {
        for size_ in [PHONE, DESKTOP] {
            let mut app = fixture_on(size_, os);
            app.type_text("label:Workspace", "draft")
                .expect("focus workspace");
            app.press_key("f8").expect("open palette");
            app.press_key("p").expect("palette owns typing");
            assert_eq!(input_value(&app), "p");
            let backdrop = app
                .screen()
                .id("id:command-dialog-backdrop")
                .expect("backdrop");
            app.engine().control().dispatch_event(
                backdrop,
                EventKind::PressDown,
                EventPayload::None,
            );
            app.wait_for("text:presented: false").expect("dismissed");
            app.press_key("x").expect("workspace owns typing again");
            assert_eq!(
                app.screen().input_text("label:Workspace"),
                Ok(Some("draftx"))
            );
        }
    }
}

#[component]
fn CommandFocusFixture() -> impl IntoElement {
    use t3_ui::screens::palette::{ContentOptions, command_palette_content};
    use t3_ui::ui::command::{self, CommandOptions, CommandState, Item};
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let workspace = signal(String::new());
    let destination = signal(String::new());
    let destination_ref = element_ref();
    let executed = signal(0u32);
    on_key_with(
        "f8",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| open.set(true),
    );
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::input()
                .label("Workspace")
                .value(move || workspace.get())
                .on_change(move |value: &str| workspace.set(value.into())),
        )
        .child(
            nsh_base::input()
                .label("Destination")
                .element_ref(&destination_ref)
                .value(move || destination.get())
                .on_change(move |value: &str| destination.set(value.into())),
        )
        .child(text(move || format!("executed: {}", executed.get())))
        .child(command::command_dialog(
            open,
            "Command palette",
            move || {
                let state = CommandState::new(
                    || {
                        vec![
                            Item::new("stay", "Run command"),
                            Item::new("move", "Focus destination"),
                        ]
                    },
                    CommandOptions {
                        filter: false,
                        ..Default::default()
                    },
                    move |value| {
                        executed.update(|count| *count += 1);
                        open.set(false);
                        if value == "move" {
                            destination_ref.focus();
                        }
                    },
                );
                command::command_dialog_popup(
                    command_palette_content(
                        state,
                        ContentOptions::default(),
                        command::command_list(
                            state,
                            "Commands",
                            command::command_collection(state, move |item| {
                                command::command_item(
                                    state,
                                    item,
                                    || None,
                                    text(move || {
                                        item.with(|item| {
                                            item.as_ref()
                                                .map(|item| item.label.clone())
                                                .unwrap_or_default()
                                        })
                                    }),
                                )
                            }),
                        ),
                    ),
                    || {},
                )
            },
        ))
}

#[test]
fn choosing_a_command_restores_the_opener_unless_the_action_moves_focus() {
    for os in [Os::Windows, Os::Web] {
        for moves_focus in [false, true] {
            let mut registry = Registry::new();
            registry.register::<CommandFocusFixture>();
            let mut app = Headless::start(EngineConfig {
                ui_sets: vec![nsh_base::registry(), registry],
                environment: Environment {
                    os,
                    idiom: Idiom::Desktop,
                    ..Default::default()
                },
                viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
                clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
                ..EngineConfig::new(Root::of::<CommandFocusFixture>())
            })
            .expect("command fixture starts");
            app.wait_for("label:Workspace").expect("workspace mounted");
            app.type_text("label:Workspace", "draft")
                .expect("focus workspace");
            app.press_key("f8").expect("open command dialog");
            app.wait_for("Run command").expect("real command row");
            if moves_focus {
                app.press_key("down").expect("select focus action");
            }
            app.press_key("enter").expect("execute selected command");
            app.wait_for("text:executed: 1").expect("one action ran");
            assert_eq!(app.screen().count("role:dialog"), Ok(0));
            app.press_key("x").expect("type after command");
            assert_eq!(
                app.screen().input_text("label:Workspace"),
                Ok(Some(if moves_focus { "draft" } else { "draftx" }))
            );
            assert_eq!(
                app.screen().input_text("label:Destination"),
                Ok(Some(if moves_focus { "x" } else { "" }))
            );
        }
    }
}
