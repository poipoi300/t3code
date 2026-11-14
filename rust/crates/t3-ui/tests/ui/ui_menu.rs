//! UI-MENU: the shared primitive mounted alone; feature packages own its application entry points.

use nsh_core::{
    Checked,
    element::Root,
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;
use std::time::{Duration, SystemTime};
use t3_ui::{i18n::t, theme, ui::menu::*};

use crate::support::{DESKTOP, PHONE};

fn fixture(window: (f32, f32), root: Root, touch: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: nsh_core::time::Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(root)
    };
    config.environment.os = if touch { Os::Android } else { Os::Web };
    config.environment.idiom = if touch { Idiom::Phone } else { Idiom::Desktop };
    let mut app = Headless::start(config).expect("menu fixture starts");
    app.first_render().expect("fixture renders");
    app
}

fn choice(label: &str, disabled: bool, chosen: Signal<String>) -> impl IntoElement {
    let name = label.to_owned();
    menu_item(
        t(label),
        ItemOptions {
            disabled: disabled.into(),
            ..Default::default()
        },
        menu_item_label(t(label)),
        move || chosen.set(name.clone()),
    )
}

#[component]
fn ActionMenu() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let chosen = signal(String::from("none"));
    let blocked = signal(false);
    let open = signal(false);
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(menu_button(
            t("Actions"),
            move || blocked.get(),
            text(t("Actions")),
            MenuOptions {
                open: Some(open),
                ..Default::default()
            },
            menu_popup(
                view()
                    .child(menu_group_label(t("Thread actions"), false))
                    .child(choice("Unavailable", true, chosen))
                    .child(choice("Rename", false, chosen))
                    .child(choice("Pin", false, chosen))
                    .child(menu_separator())
                    .child(menu_sub(
                        menu_sub_trigger(
                            t("Copy"),
                            ItemOptions::default(),
                            menu_item_label(t("Copy")),
                        ),
                        || false,
                        menu_sub_popup(
                            view()
                                .child(choice("Unavailable copy", true, chosen))
                                .child(choice("Path", false, chosen))
                                .child(choice("Branch", false, chosen)),
                        ),
                    )),
            ),
        ))
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle permission"))
                .on_press(move || blocked.set(!blocked.get_untracked()))
                .child(text(t("Toggle permission"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Close from outside"))
                .on_press(move || chosen.set("outside".to_owned()))
                .child(text(t("Close from outside"))),
        )
        .child(text(move || format!("chosen:{}", chosen.get())))
}

#[test]
fn keyboard_selection_wraps_skips_disabled_rows_and_activates_with_enter_or_space() {
    // contextMenuFallback.keyboard.browser.test.ts: enabled-item traversal and key activation.
    for window in [PHONE, DESKTOP] {
        for (keys, activate, expected) in [
            (&["down"][..], "enter", "Rename"),
            (&["down", "down"][..], "space", "Pin"),
            (&["down", "up", "up"][..], "space", "Pin"),
            (&["end", "down"][..], "enter", "Rename"),
            (&["end", "home"][..], "space", "Rename"),
        ] {
            let mut app = fixture(window, Root::of::<ActionMenu>(), false);
            app.press("role:button[Actions]").expect("open");
            app.wait_for("Rename").expect("menu content");
            for key in keys {
                app.press_key(key).expect("move focus");
            }
            app.press_key(activate).expect("choose");
            app.wait_for(&format!("text:chosen:{expected}"))
                .expect("enabled action selected");
            assert_eq!(app.screen().matches("role:menuitem[Rename]"), Ok(false));
        }
    }
}

#[test]
fn escape_leaves_one_submenu_level_and_restores_the_trigger() {
    // contextMenuFallback.keyboard.browser.test.ts: submenu cancellation and focus return.
    let mut app = fixture(DESKTOP, Root::of::<ActionMenu>(), false);
    app.press("role:button[Actions]").expect("open");
    app.press_key("end").expect("Copy");
    app.press_key("right").expect("submenu");
    app.wait_for("Path").expect("child items");
    app.press_key("escape").expect("leave submenu");
    assert!(!app.shows("Path"));
    assert!(app.shows("Rename"));
    app.press_key("enter").expect("Copy receives focus again");
    app.wait_for("Path").expect("submenu reopened");
    app.press_key("enter").expect("first enabled child");
    app.wait_for("text:chosen:Path")
        .expect("disabled child skipped");
    app.press_key("enter").expect("root trigger regained focus");
    app.wait_for("Rename").expect("menu reopens");
    app.press_key("escape").expect("dismiss");
    assert!(!app.shows("Rename"));
}

#[test]
fn on_a_coarse_pointer_submenus_offer_back_then_the_parent_actions_work() {
    let mut app = fixture(PHONE, Root::of::<ActionMenu>(), true);
    app.press("role:button[Actions]").expect("open sheet");
    app.press("role:menuitem[Copy]").expect("submenu");
    app.wait_for("Path").expect("child items");
    assert!(!app.shows("Rename"));
    app.press("role:button[Back]").expect("back one level");
    app.wait_for("role:menuitem[Rename]").expect("parent items");
    assert!(!app.shows("Path"));
    app.press("role:menuitem[Rename]").expect("parent action");
    app.wait_for("text:chosen:Rename").expect("selected");
}

#[test]
fn disabled_trigger_and_disabled_row_do_not_run_and_outside_press_closes() {
    let mut app = fixture(DESKTOP, Root::of::<ActionMenu>(), false);
    app.press("role:button[Toggle permission]")
        .expect("disable trigger");
    assert!(app.press("role:button[Actions]").is_err());
    assert!(!app.shows("Rename"));
    app.press("role:button[Toggle permission]")
        .expect("enable trigger");
    app.press("role:button[Actions]").expect("open");
    assert!(app.press("role:menuitem[Unavailable]").is_err());
    assert!(app.shows("chosen:none"));
    app.press("role:button[Close from outside]")
        .expect("outside dismiss");
    assert!(!app.shows("Rename"));
}

#[test]
fn tab_dismisses_and_typeahead_matches_enabled_labels() {
    let mut app = fixture(DESKTOP, Root::of::<ActionMenu>(), false);
    app.press("role:button[Actions]").expect("open");
    app.press_key("down").expect("first enabled");
    app.press_key("p").expect("typeahead");
    app.fast_forward(Duration::from_millis(500))
        .expect("typeahead buffer expires");
    app.press_key("r").expect("new prefix after focus moved");
    app.press_key("space").expect("choose new prefix");
    app.wait_for("text:chosen:Rename")
        .expect("buffer expires after typeahead moves focus");
    app.press_key("enter").expect("reopen for another search");
    app.press_key("p").expect("Pin");
    app.press_key("space").expect("choose focused result");
    app.wait_for("text:chosen:Pin")
        .expect("typeahead moved focus");
    app.press_key("enter").expect("reopen");
    app.press_key("tab").expect("leave menu");
    assert!(!app.shows("Rename"));
}

#[component]
fn CheckMenu() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let checked = signal(false);
    let selected = signal(String::from("name"));
    let changes = signal(0);
    let options = ItemOptions {
        close_on_select: false,
        ..Default::default()
    };
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(menu_button(
            t("Preferences"),
            || false,
            text(t("Preferences")),
            MenuOptions::default(),
            menu_popup(
                view()
                    .child(menu_checkbox_item(
                        t("Pinned"),
                        move || checked.get(),
                        CheckboxVariant::Default,
                        options,
                        menu_item_label(t("Pinned")),
                        move |next| checked.set(next),
                    ))
                    .child(menu_checkbox_item(
                        t("Icon check"),
                        move || checked.get(),
                        CheckboxVariant::Icon,
                        options,
                        menu_item_label(t("Icon check")),
                        move |next| checked.set(next),
                    ))
                    .child(menu_checkbox_item(
                        t("Switch"),
                        move || checked.get(),
                        CheckboxVariant::Switch,
                        options,
                        menu_item_label(t("Switch")),
                        move |next| checked.set(next),
                    ))
                    .child(menu_radio_group(
                        t("Sort"),
                        move || selected.get(),
                        view()
                            .child(menu_radio_item(
                                t("Name"),
                                "name".to_owned(),
                                options,
                                menu_item_label(t("Name")),
                                || {},
                            ))
                            .child(menu_radio_item(
                                t("Activity"),
                                "activity".to_owned(),
                                options,
                                menu_item_label(t("Activity")),
                                || {},
                            )),
                        move |value| {
                            changes.update(|n| *n += 1);
                            selected.set(value);
                        },
                    )),
            ),
        ))
        .child(text(move || {
            format!(
                "checked:{} selected:{} changes:{}",
                checked.get(),
                selected.get() == "activity",
                changes.get()
            )
        }))
}

#[test]
fn checkbox_variants_follow_the_controlled_value_and_radio_selection_is_exclusive() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window, Root::of::<CheckMenu>(), false);
        app.press("role:button[Preferences]").expect("open");
        for name in ["Pinned", "Icon check", "Switch"] {
            assert_eq!(
                app.screen()
                    .accessibility_state(&format!("role:menuitemcheckbox[{name}]"))
                    .expect("checkbox role")
                    .checked,
                Some(Checked::False)
            );
        }
        app.press("role:menuitemcheckbox[Switch]").expect("toggle");
        app.wait_for("text:checked:true selected:false changes:0")
            .expect("value changed");
        for name in ["Pinned", "Icon check", "Switch"] {
            assert_eq!(
                app.screen()
                    .accessibility_state(&format!("role:menuitemcheckbox[{name}]"))
                    .expect("checkbox state")
                    .checked,
                Some(Checked::True)
            );
        }
        app.press("role:menuitemradio[Activity]")
            .expect("change selection");
        app.wait_for("text:checked:true selected:true changes:1")
            .expect("selection changed");
        app.press("role:menuitemradio[Activity]").expect("reselect");
        app.wait_for("text:checked:true selected:true changes:1")
            .expect("reselect does not report a changed group value");
        assert_eq!(
            app.screen()
                .accessibility_state("role:menuitemradio[Name]")
                .expect("radio state")
                .checked,
            Some(Checked::False)
        );
        assert_eq!(
            app.screen()
                .accessibility_state("role:menuitemradio[Activity]")
                .expect("radio state")
                .checked,
            Some(Checked::True)
        );
        app.press("role:menuitemcheckbox[Pinned]")
            .expect("reverse toggle");
        app.wait_for("text:checked:false selected:true changes:1")
            .expect("reverse value");
        for name in ["Pinned", "Icon check", "Switch"] {
            assert_eq!(
                app.screen()
                    .accessibility_state(&format!("role:menuitemcheckbox[{name}]"))
                    .expect("reversed checkbox state")
                    .checked,
                Some(Checked::False)
            );
        }
        app.press_key("escape").expect("close");
        app.press("role:button[Preferences]").expect("reopen");
        app.wait_for("Activity").expect("new opening");
        assert_eq!(
            app.screen()
                .accessibility_state("role:menuitemradio[Activity]")
                .expect("retained value")
                .checked,
            Some(Checked::True)
        );
    }
}

#[component]
fn SearchMenu() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let query = signal(String::new());
    let chosen = signal(String::from("none"));
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(menu_button(
            t("Search actions"),
            || false,
            text(t("Search actions")),
            MenuOptions::default(),
            menu_popup_with(move || {
                view()
                    .child(menu_search_input(
                        query,
                        t("Filter"),
                        t("Filter actions"),
                        || false,
                    ))
                    .child(choice("Unavailable", true, chosen))
                    .child(when(
                        move || "beta".contains(&query.get().to_lowercase()),
                        move || choice("Beta", false, chosen),
                    ))
            }),
        ))
        .child(text(move || {
            format!("query:{} chosen:{}", query.get(), chosen.get())
        }))
}

#[test]
fn search_keeps_typed_letters_and_arrow_down_reaches_an_enabled_result() {
    // MenuSearchInput: letters stay in the input, ArrowDown reaches items, Escape dismisses.
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window, Root::of::<SearchMenu>(), false);
        app.press("role:button[Search actions]").expect("open");
        app.type_text("label:Filter actions", "Be").expect("filter");
        app.wait_for("text:query:Be chosen:none")
            .expect("letters belong to input");
        app.press_key("down").expect("leave search for result");
        app.press_key("enter").expect("choose result");
        app.wait_for("text:query:Be chosen:Beta")
            .expect("selected enabled result");
        app.press_key("enter").expect("open again");
        app.clear("label:Filter actions").expect("clear search");
        app.type_text("label:Filter actions", "z")
            .expect("empty results");
        assert!(!app.shows("Beta"));
        app.press_key("down").expect("no enabled result");
        app.wait_for("label:Filter actions")
            .expect("empty search stays open");
        app.clear("label:Filter actions")
            .expect("clear no-match query");
        app.type_text("label:Filter actions", "Be")
            .expect("restore result");
        app.wait_for("Beta").expect("conditional row remounted");
        app.press_key("escape").expect("dismiss from input");
        assert_eq!(app.screen().matches("label:Filter actions"), Ok(false));
    }
}

#[component]
fn RowContext() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let presses = signal(0);
    let chosen = signal(String::from("none"));
    view()
        .w_full()
        .h_full()
        .p(px(24.0))
        .child(context_menu(
            view()
                .role(AccessibilityRole::Button)
                .label(t("Thread row"))
                .w(px(200.0))
                .h(px(44.0))
                .on_press(move || presses.update(|n| *n += 1))
                .child(text(t("Thread row"))),
            MenuOptions::default(),
            || false,
            menu_popup(choice("Rename", false, chosen)),
        ))
        .child(text(move || {
            format!("presses:{} chosen:{}", presses.get(), chosen.get())
        }))
}

#[test]
fn context_menu_supports_secondary_click_and_long_press_without_pressing_the_row() {
    for touch in [false, true] {
        let mut app = fixture(
            if touch { PHONE } else { DESKTOP },
            Root::of::<RowContext>(),
            touch,
        );
        if touch {
            app.long_press("role:button[Thread row]")
                .expect("held finger");
        } else {
            app.right_click("role:button[Thread row]")
                .expect("secondary click");
        }
        app.wait_for("Rename").expect("context menu opens");
        assert!(app.shows("presses:0 chosen:none"));
        app.press("role:menuitem[Rename]").expect("choose");
        app.wait_for("text:presses:0 chosen:Rename")
            .expect("action without row press");
        app.press("role:button[Thread row]")
            .expect("normal tap still works");
        app.wait_for("text:presses:1 chosen:Rename")
            .expect("reverse to normal interaction");
        assert_eq!(app.screen().matches("role:menuitem[Rename]"), Ok(false));
    }
}
