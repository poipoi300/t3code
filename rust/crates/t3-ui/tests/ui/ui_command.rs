//! UI-COMMAND slot harness. SH-PALETTE owns mounting the primitive in the application.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::theme::{self, ThemeChoice};
use t3_ui::ui::command::*;

use crate::support::{DESKTOP, PHONE};

#[component]
fn CommandFixture() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let open = signal(false);
    let virtualized = signal(false);
    let grouped = signal(false);
    let long_list = signal(false);
    let chosen = signal(String::from("<none>"));
    let generation = signal(0u32);
    let outside_presses = signal(0u32);
    on_key("ctrl+r", move |_| open.set(true));
    view()
        .w_full()
        .h_full()
        .child(command_dialog_trigger(
            open,
            "Open palette",
            text(t("Open palette")),
        ))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Open virtualized"))))
                .on_press(move || {
                    virtualized.set(true);
                    open.set(true);
                })
                .child(text(t("Open virtualized"))),
        )
        .child(text(move || format!("chosen: {}", chosen.get())))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Open grouped"))))
                .on_press(move || {
                    grouped.set(true);
                    open.set(true);
                })
                .child(text(t("Open grouped"))),
        )
        .child(text(move || format!("mounts: {}", generation.get())))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Open long list"))))
                .on_press(move || {
                    long_list.set(true);
                    open.set(true);
                })
                .child(text(t("Open long list"))),
        )
        .child(text(move || {
            format!("outside presses: {}", outside_presses.get())
        }))
        .child(command_dialog(open, "Command palette", move || {
            generation.update(|n| *n += 1);
            let changed = signal(false);
            let close_next = signal(false);
            let state = CommandState::new(
                move || {
                    let mut disabled = Item::new("disabled", "Unavailable");
                    disabled.disabled = true;
                    let mut items = if long_list.get() {
                        (0..50)
                            .map(|index| {
                                Item::new(format!("long-{index}"), format!("Command {index:02}"))
                            })
                            .collect()
                    } else if changed.get() {
                        vec![Item::new("b", "Project B"), disabled]
                    } else {
                        vec![
                            disabled,
                            Item::new("a", "Project A"),
                            Item::new("b", "Project B"),
                        ]
                    };
                    if grouped.get() {
                        for item in &mut items {
                            item.group =
                                Some(if item.disabled { "Actions" } else { "Projects" }.into());
                        }
                    }
                    items
                },
                CommandOptions::default(),
                move |value| {
                    chosen.set(value.to_owned());
                    if close_next.get_untracked() {
                        open.set(false);
                    }
                },
            );
            let back = nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Back"))))
                .on_press(move || open.set(false))
                .child(text(t("Back")))
                .into_element();
            let mut input = InputOptions::new("Search projects", "Search projects");
            input.auto_focus = false;
            input.start_addon = Some(back);
            command_dialog_popup(
                command(state, move |state| {
                    view()
                        .child(command_input(state, input))
                        .child(command_panel(
                            true,
                            view().h(px(180.0)).child(if virtualized.get_untracked() {
                                command_list_virtualized(state, "Projects", move |item| {
                                    row(state, item)
                                })
                                .into_element()
                            } else {
                                command_list(
                                    state,
                                    "Projects",
                                    if grouped.get_untracked() {
                                        view()
                                            .child(command_group(
                                                state,
                                                "Actions",
                                                false,
                                                move || {
                                                    view()
                                                        .child(command_group_label("Actions"))
                                                        .child(command_collection(
                                                            state,
                                                            move |item| row(state, item),
                                                        ))
                                                },
                                            ))
                                            .child(command_group(
                                                state,
                                                "Projects",
                                                move || {
                                                    state.items.with(|items| {
                                                        items.iter().any(|i| {
                                                            i.group.as_deref() == Some("Actions")
                                                        })
                                                    })
                                                },
                                                move || {
                                                    view()
                                                        .child(command_group_label("Projects"))
                                                        .child(command_collection(
                                                            state,
                                                            move |item| row(state, item),
                                                        ))
                                                },
                                            ))
                                            .into_element()
                                    } else {
                                        command_collection(state, move |item| row(state, item))
                                            .into_element()
                                    },
                                )
                                .into_element()
                            }),
                        ))
                        .child(when(
                            move || state.items.with(|i| i.is_empty()),
                            || text(t("No results")),
                        ))
                        .child(text(move || {
                            format!("highlight: {}", state.highlighted.get().unwrap_or_default())
                        }))
                        .child(command_footer(
                            view()
                                .child(command_footer_action(
                                    "Choose and close",
                                    || false,
                                    move || {
                                        close_next.set(true);
                                        state.choose("a");
                                    },
                                ))
                                .child(command_footer_action(
                                    "Replace results",
                                    || false,
                                    move || changed.set(true),
                                ))
                                .child(command_footer_action(
                                    "Disable commands",
                                    || false,
                                    move || state.disabled.set(true),
                                ))
                                .child(command_footer_action(
                                    "Attempt disabled",
                                    || false,
                                    move || state.choose("disabled"),
                                ))
                                .child(command_footer_action(
                                    "Attempt unknown",
                                    || false,
                                    move || state.choose("unknown"),
                                ))
                                .child(command_footer_action(
                                    "Attempt current",
                                    || false,
                                    move || state.choose("b"),
                                )),
                        ))
                }),
                move || outside_presses.update(|count| *count += 1),
            )
        }))
}

fn row(state: CommandState, item: Memo<Option<Item>>) -> impl IntoElement {
    command_item(
        state,
        item,
        || None,
        text(move || item.get().map(|i| t(&i.label)).unwrap_or_default()),
    )
}

fn fixture(size: (f32, f32)) -> Headless {
    fixture_with_motion(size, false)
}

fn fixture_with_motion(size: (f32, f32), reduce_motion: bool) -> Headless {
    // The application palette is a different work package; mount this primitive directly.
    let mut commands = Registry::new();
    commands.register::<CommandFixture>();
    let mut app = Headless::start(EngineConfig {
        ui_sets: vec![nsh_base::registry(), commands],
        viewport: nsh_core::geometry::size(px(size.0), px(size.1)),
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
        environment: nsh_core::platform::Environment {
            reduce_motion,
            ..Default::default()
        },
        ..EngineConfig::new(Root::of::<CommandFixture>())
    })
    .expect("command fixture starts");
    app.wait_for("Open palette").expect("fixture mounts");
    app
}

#[test]
fn submenu_search_takes_focus_before_back_at_phone_and_desktop_sizes() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("role:button[Open palette]").expect("open");
        // Port of CommandPaletteContent.browser.test.tsx: typing immediately goes to search.
        app.press_key("b")
            .expect("type into initially focused input");
        assert_eq!(
            app.screen()
                .input_text("label:Search projects")
                .expect("input"),
            Some("b")
        );
        assert!(!app.screen().shows("Project A"));
        app.press_key("down")
            .expect("move past matching disabled option");
        app.wait_for("text:highlight: b").expect("filtered result");
        app.submit("label:Search projects")
            .expect("select using touch keyboard");
        app.wait_for("text:chosen: b")
            .expect("correct stable value");
        app.press_key("escape").expect("close");
        assert_eq!(app.screen().count("role:dialog"), Ok(0));
        app.press("role:button[Open palette]").expect("reopen");
        assert_eq!(
            app.screen()
                .input_text("label:Search projects")
                .expect("input"),
            Some("")
        );
        app.wait_for("text:mounts: 2")
            .expect("typing did not remount the modal");
        app.press("role:button[Choose and close]")
            .expect("selection closes its owner");
        app.wait_for("text:chosen: a")
            .expect("selection callback completed while unmounting");
        assert_eq!(app.screen().count("role:dialog"), Ok(0));
    }
}

#[test]
fn disabled_items_are_navigable_but_activation_and_unknown_values_fail_closed() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Open palette]").expect("open");
    app.wait_for("text:highlight: disabled")
        .expect("first disabled result remains readable");
    app.submit("label:Search projects")
        .expect("submit disabled option");
    assert!(app.screen().shows("chosen: <none>"));
    assert!(
        !app.screen()
            .enabled("role:option[Unavailable]")
            .expect("disabled option")
    );
    app.press_key("down").expect("next");
    app.wait_for("text:highlight: a").expect("next option");
    app.press_key("down").expect("next");
    app.wait_for("text:highlight: b")
        .expect("highlight follows keyboard");
    app.press_key("down").expect("wrap");
    app.wait_for("text:highlight: disabled")
        .expect("auto-highlight wraps across list boundary");
    app.press_key("up").expect("wrap back");
    app.wait_for("text:highlight: b")
        .expect("reverse wrap reaches last option");
    app.submit("label:Search projects").expect("choose");
    app.wait_for("text:chosen: b").expect("correct selection");
    app.press("role:button[Attempt disabled]")
        .expect("stale callback");
    app.press("role:button[Attempt unknown]")
        .expect("unknown callback");
    assert!(app.screen().shows("chosen: b"));
    app.press("role:option[Project A]")
        .expect("touch selection");
    app.wait_for("text:chosen: a")
        .expect("pointer selected its own value");
    app.press("role:button[Disable commands]")
        .expect("deny permission");
    app.press("role:button[Attempt current]")
        .expect("stale handler after denial");
    assert!(app.screen().shows("chosen: a"));
}

#[test]
fn async_results_keep_the_highlight_position_and_empty_results_do_not_dispatch() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Open palette]").expect("open");
    app.wait_for("text:highlight: disabled").expect("initial");
    app.press("role:button[Replace results]")
        .expect("async result replacement");
    app.wait_for("text:highlight: b")
        .expect("new result occupies the same highlighted position");
    assert!(!app.screen().shows("Project A"));
    app.type_text("label:Search projects", "missing")
        .expect("query with no matches");
    app.wait_for("No results").expect("empty state slot");
    app.submit("label:Search projects")
        .expect("submit empty result set");
    assert!(app.screen().shows("chosen: <none>"));
}

#[test]
fn virtualized_results_use_the_same_query_and_selection_contract() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("role:button[Open virtualized]").expect("open");
        app.wait_for("Project A").expect("virtual rows");
        app.type_text("label:Search projects", "Project B")
            .expect("filter");
        app.wait_for("text:highlight: b")
            .expect("new query resets highlight");
        app.press("role:option[Project B]")
            .expect("press recycled row");
        app.wait_for("text:chosen: b")
            .expect("row dispatch uses its current memo");
        assert!(!app.screen().shows("Project A"));
    }
}

#[test]
fn filtering_hides_empty_groups_and_keeps_collections_in_their_group() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Open grouped]")
        .expect("open grouped list");
    app.wait_for("Project A").expect("grouped results");
    assert!(app.screen().enabled("role:group[Actions]").is_ok());
    app.type_text("label:Search projects", "Project A")
        .expect("filter group");
    app.wait_for("Project A").expect("matching group stays");
    assert!(app.screen().enabled("role:group[Actions]").is_err());
    assert!(app.screen().enabled("role:group[Projects]").is_ok());
    app.press("role:option[Project A]")
        .expect("exactly one matching option");
    app.wait_for("text:chosen: a")
        .expect("group selection keeps stable value");
}

#[test]
fn outside_press_dismisses_without_requiring_the_observer_to_close() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("role:button[Open palette]").expect("open");
        let backdrop = app
            .screen()
            .id("id:command-dialog-backdrop")
            .expect("backdrop");
        app.engine()
            .control()
            .dispatch_event(backdrop, EventKind::PressDown, EventPayload::None);
        app.wait_for("text:outside presses: 1")
            .expect("observer runs");
        app.fast_forward(std::time::Duration::from_millis(250))
            .expect("finish exit");
        assert_eq!(
            app.screen().count("role:dialog"),
            Ok(0),
            "outside press closes palette"
        );
        app.press("role:button[Open palette]").expect("reopen");
        app.wait_for("Search projects")
            .expect("popup remains usable");
    }
}

#[test]
fn exit_retains_an_inert_popup_and_interrupted_exit_keeps_the_same_mount() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Open palette]").expect("open");
    let popup = app.screen().id("role:dialog").expect("dialog");
    let backdrop = app
        .screen()
        .id("id:command-dialog-backdrop")
        .expect("backdrop");
    // Dispatch before Headless::press settles animations, then advance only the app's clock.
    app.engine()
        .control()
        .dispatch_event(backdrop, EventKind::PressDown, EventPayload::None);
    app.wait_for("text:outside presses: 1")
        .expect("exit begins");
    assert_eq!(
        app.screen().id("role:dialog"),
        Ok(popup),
        "retain exit content"
    );
    assert!(
        !app.screen()
            .enabled("role:option[Project A]")
            .expect("closing row")
    );
    assert!(
        !app.screen()
            .enabled("role:button[Choose and close]")
            .expect("closing footer")
    );
    app.press_key("ctrl+r").expect("reopen during exit");
    app.fast_forward(std::time::Duration::from_millis(250))
        .expect("past cancelled exit");
    assert_eq!(
        app.screen().id("role:dialog"),
        Ok(popup),
        "reuse interrupted popup"
    );
    assert!(app.screen().shows("mounts: 1"));
    app.press_key("escape").expect("close again");
    assert_eq!(
        app.screen().count("role:dialog"),
        Ok(0),
        "unmount after exit"
    );
}

#[test]
fn reduced_motion_dismisses_without_retaining_exit_content() {
    let mut app = fixture_with_motion(PHONE, true);
    app.press("role:button[Open palette]").expect("open");
    let backdrop = app
        .screen()
        .id("id:command-dialog-backdrop")
        .expect("backdrop");
    app.engine()
        .control()
        .dispatch_event(backdrop, EventKind::PressDown, EventPayload::None);
    app.wait_for("text:outside presses: 1").expect("dismissed");
    assert_eq!(
        app.screen().count("role:dialog"),
        Ok(0),
        "instant reduced-motion exit"
    );
}

#[test]
fn keyboard_reveal_keeps_the_active_row_inside_the_lists_scroll_padding() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("role:button[Open long list]").expect("open");
        for _ in 0..15 {
            app.press_key("down").expect("navigate beyond viewport");
        }
        app.wait_for("text:highlight: long-15").expect("active row");
        let list = app.screen().id("role:listbox[Projects]").expect("list");
        let row = app.screen().id("role:option[Command 15]").expect("row");
        let nodes = app.screen().layout_nodes();
        let viewport = nodes
            .iter()
            .find(|node| node.id == list)
            .expect("viewport")
            .bounds;
        let active = nodes
            .iter()
            .find(|node| node.id == row)
            .expect("active bounds")
            .bounds;
        assert!(active.origin.y.0 >= viewport.origin.y.0 + 7.5);
        assert!(
            active.origin.y.0 + active.size.height.0
                <= viewport.origin.y.0 + viewport.size.height.0 - 7.5
        );
        let bottom_gap =
            viewport.origin.y.0 + viewport.size.height.0 - active.origin.y.0 - active.size.height.0;
        assert!(
            (bottom_gap - 8.0).abs() < 0.5,
            "nearest reveal leaves 8px at the lower edge, got {bottom_gap}"
        );
    }
}
