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
    let chosen = signal(String::from("<none>"));
    let generation = signal(0u32);
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
        .child(command_dialog(open, "Command palette", move || {
            generation.update(|n| *n += 1);
            let changed = signal(false);
            let state = CommandState::new(
                move || {
                    let mut disabled = Item::new("disabled", "Unavailable");
                    disabled.disabled = true;
                    let mut items = if changed.get() {
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
                move |value| chosen.set(value.to_owned()),
            );
            let back = nsh_base::button()
                .accessibility_label(Some(SharedString::from(t("Back"))))
                .on_press(move || open.set(false))
                .child(text(t("Back")))
                .into_element();
            let mut input = InputOptions::new("Search projects", "Search projects");
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
                                                true,
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
                move || open.set(false),
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
    // The application palette is a different work package; mount this primitive directly.
    let mut commands = Registry::new("t3/ui-command-test");
    commands.register::<CommandFixture>();
    Headless::start(EngineConfig {
        ui_sets: vec![nsh_base::registry(), commands],
        viewport: nsh_core::geometry::size(px(size.0), px(size.1)),
        ..EngineConfig::new(Root::of::<CommandFixture>())
    })
    .expect("command fixture starts")
}

#[test]
fn submenu_search_takes_focus_before_back_at_phone_and_desktop_sizes() {
    for size in [PHONE, DESKTOP] {
        let mut app = fixture(size);
        app.press("role:button[Open palette]").expect("open");
        // Port of CommandPaletteContent.browser.test.tsx: typing immediately goes to search.
        app.press_key("b")
            .expect("type into initially focused input");
        app.wait_for("highlight: b").expect("filtered result");
        assert_eq!(
            app.screen()
                .input_text("label:Search projects")
                .expect("input"),
            Some("b")
        );
        assert!(!app.screen().shows("Project A"));
        app.submit("label:Search projects")
            .expect("select using touch keyboard");
        app.wait_for("chosen: b").expect("correct stable value");
        app.press_key("escape").expect("close");
        assert!(!app.screen().shows("Search projects"));
        app.press("role:button[Open palette]").expect("reopen");
        assert_eq!(
            app.screen()
                .input_text("label:Search projects")
                .expect("input"),
            Some("")
        );
        app.wait_for("mounts: 2")
            .expect("typing did not remount the modal");
    }
}

#[test]
fn keyboard_and_touch_skip_disabled_items_and_refuse_unknown_values() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Open palette]").expect("open");
    app.wait_for("highlight: a")
        .expect("always highlights first enabled result");
    assert!(
        !app.screen()
            .enabled("role:option[Unavailable]")
            .expect("disabled option")
    );
    app.press_key("down").expect("next");
    app.wait_for("highlight: b")
        .expect("highlight follows keyboard");
    app.submit("label:Search projects").expect("choose");
    app.wait_for("chosen: b").expect("correct selection");
    app.press("role:button[Attempt disabled]")
        .expect("stale callback");
    app.press("role:button[Attempt unknown]")
        .expect("unknown callback");
    assert!(app.screen().shows("chosen: b"));
    app.press("role:option[Project A]")
        .expect("touch selection");
    app.wait_for("chosen: a")
        .expect("pointer selected its own value");
    app.press("role:button[Disable commands]")
        .expect("deny permission");
    app.press("role:button[Attempt current]")
        .expect("stale handler after denial");
    assert!(app.screen().shows("chosen: a"));
}

#[test]
fn async_results_reconcile_selection_without_reviving_removed_items() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[Open palette]").expect("open");
    app.wait_for("highlight: a").expect("initial");
    app.press("role:button[Replace results]")
        .expect("async result replacement");
    app.wait_for("highlight: b")
        .expect("removed highlight falls back to enabled result");
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
        app.wait_for("highlight: b")
            .expect("new query resets highlight");
        app.press("role:option[Project B]")
            .expect("press recycled row");
        app.wait_for("chosen: b")
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
    app.wait_for("chosen: a")
        .expect("group selection keeps stable value");
}
