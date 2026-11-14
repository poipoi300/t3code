//! UI-TABS interaction fixtures: the primitives have no application route until their consumers
//! integrate them, so these roots use the shared support sizes without changing the app seam.

use crate::support::{DESKTOP, PHONE};
use nsh_core::{
    Checked,
    platform::{Idiom, Os},
    prelude::*,
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::i18n::t;
use t3_ui::theme;
use t3_ui::ui::tabs::{panel_tab_close_button, tab, tabs};
use t3_ui::ui::toggle::{
    Content, GroupChild, GroupItem, GroupOptions, Orientation, Size, Variant, toggle, toggle_group,
};

fn start<C: Component<Props = ()>>(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<C>())
    };
    config.environment.os = Os::MacOs;
    if window == PHONE {
        config.environment.idiom = Idiom::Phone;
    }
    let mut app = Headless::start(config).expect("fixture starts");
    app.first_render().expect("fixture renders");
    app
}

fn pressed(app: &Headless, label: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("role:button[{label}]"))
        .expect("toggle accessibility")
        .pressed
        == Some(Checked::True)
}

#[component]
fn Controlled() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let pressed = signal(false);
    let changes = signal(0);
    let requested = signal(false);
    let disabled = signal(false);
    view()
        .child(toggle(
            Content::Label(t("Bold")),
            Variant::Outline,
            Size::Default,
            move || pressed.get(),
            move || disabled.get(),
            move |next| {
                requested.set(next);
                changes.update(|n| *n += 1);
            },
        ))
        .child(
            nsh_base::button()
                .accessibility_label(t("Accept value"))
                .on_press(move || pressed.set(requested.get()))
                .child(text(t("Accept value"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Disable"))
                .on_press(move || disabled.set(true))
                .child(text(t("Disable"))),
        )
        .child(text(move || {
            format!("changes:{};requested:{}", changes.get(), requested.get())
        }))
}

#[test]
fn controlled_toggle_only_displays_the_value_passed_back_and_disabled_is_inert() {
    for window in [PHONE, DESKTOP] {
        let mut app = start::<Controlled>(window);
        assert!(!pressed(&app, "Bold"));
        app.press("role:button[Bold]").expect("request on");
        app.wait_for("changes:1;requested:true")
            .expect("requested value");
        assert!(!pressed(&app, "Bold"), "no uncontrolled local mutation");
        app.press("role:button[Accept value]")
            .expect("echo controlled value");
        assert!(pressed(&app, "Bold"));
        app.press("role:button[Bold]").expect("request off");
        app.wait_for("changes:2;requested:false")
            .expect("off requested");
        assert!(pressed(&app, "Bold"));
        app.press("role:button[Disable]").expect("disable");
        assert!(app.press("role:button[Bold]").is_err());
        assert!(pressed(&app, "Bold"));
        assert!(app.screen().shows("changes:2;requested:false"));
    }
}

fn item(value: &str, label: &str, disabled: bool) -> GroupChild {
    let mut item = GroupItem::new(value, Content::Label(t(label)));
    item.disabled = disabled.into();
    item.into()
}

#[component]
fn Groups() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let single = signal(vec!["a".to_owned()]);
    let multiple = signal(vec!["a".to_owned()]);
    let disabled = signal(false);
    let changes = signal(0);
    view()
        .child(toggle_group(
            t("Single"),
            vec![
                item("a", "First", false),
                item("disabled", "Unavailable", true),
                item("b", "Last", false),
            ],
            GroupOptions::default(),
            move || single.get(),
            move || disabled.get(),
            move |next| {
                single.set(next);
                changes.update(|n| *n += 1);
            },
        ))
        .child(toggle_group(
            t("Multiple"),
            vec![
                item("a", "Bold", false),
                GroupChild::Separator,
                item("b", "Italic", false),
            ],
            GroupOptions {
                multiple: true,
                variant: Variant::Outline,
                orientation: Orientation::Vertical,
                ..Default::default()
            },
            move || multiple.get(),
            || false,
            move |next| multiple.set(next),
        ))
        .child(
            nsh_base::button()
                .accessibility_label(t("Lock group"))
                .on_press(move || disabled.set(true))
                .child(text(t("Lock group"))),
        )
        .child(text(move || {
            format!(
                "single:{};multiple:{};changes:{}",
                single.get().join(","),
                multiple.get().join(","),
                changes.get()
            )
        }))
}

#[test]
fn single_selection_replaces_and_clears_while_multiple_keeps_other_values() {
    for window in [PHONE, DESKTOP] {
        let mut app = start::<Groups>(window);
        app.press("role:button[Last]").expect("select last");
        assert!(!pressed(&app, "First"));
        assert!(pressed(&app, "Last"));
        app.press("role:button[Last]").expect("clear selected item");
        assert!(!pressed(&app, "Last"));
        app.press("role:button[Italic]").expect("add second value");
        assert!(pressed(&app, "Bold"));
        assert!(pressed(&app, "Italic"));
        app.press("role:button[Bold]").expect("remove one value");
        assert!(!pressed(&app, "Bold"));
        assert!(pressed(&app, "Italic"));
        app.wait_for("single:;multiple:b;changes:2")
            .expect("selection stored by caller");
    }
}

#[test]
fn disabled_item_and_disabled_group_never_request_selection() {
    let mut app = start::<Groups>(DESKTOP);
    assert!(app.press("role:button[Unavailable]").is_err());
    app.press("role:button[Lock group]").expect("lock");
    assert!(app.press("role:button[First]").is_err());
    assert!(app.press("role:button[Last]").is_err());
    assert!(pressed(&app, "First"));
    assert!(!pressed(&app, "Last"));
    assert!(app.screen().shows("single:a;multiple:a;changes:0"));
}

#[test]
fn arrows_skip_disabled_items_without_selecting_and_space_activates() {
    let mut app = start::<Groups>(DESKTOP);
    app.press_key("tab")
        .expect("enter first group at selected item");
    app.press_key("right").expect("skip unavailable item");
    assert!(pressed(&app, "First"), "arrows do not activate toggles");
    assert!(!pressed(&app, "Last"));
    app.press_key("space").expect("activate focused last item");
    assert!(!pressed(&app, "First"));
    assert!(pressed(&app, "Last"));
    app.press_key("right").expect("wrap to first");
    app.press_key("enter").expect("activate first");
    assert!(pressed(&app, "First"));
    assert!(!pressed(&app, "Last"));
    app.wait_for("single:a;multiple:a;changes:2")
        .expect("only activation changed state");
}

#[component]
fn CloseButton() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let closed = signal(0);
    view()
        .child(
            view()
                .app_class(theme::tw("group/tab"))
                .test_id("surface-tab")
                .child(panel_tab_close_button(
                    text(t("Surface icon")),
                    t("Close preview"),
                    move || closed.update(|n| *n += 1),
                    Some(t("Close the preview")),
                )),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Elsewhere"))
                .on_press(|| {})
                .child(text(t("Elsewhere"))),
        )
        .child(text(move || format!("closed:{}", closed.get())))
}

#[test]
fn close_swaps_the_surface_slot_on_tab_hover_or_keyboard_focus_and_closes_once() {
    let mut app = start::<CloseButton>(DESKTOP);
    assert!(app.screen().shows("Surface icon"));
    app.hover("id:surface-tab").expect("hover whole tab");
    assert!(!app.screen().shows("Surface icon"));
    app.hover("role:button[Elsewhere]").expect("leave tab");
    assert!(app.screen().shows("Surface icon"));
    app.press_key("tab").expect("focus close");
    assert!(!app.screen().shows("Surface icon"));
    app.wait_for("Close the preview").expect("focus tooltip");
    app.press_key("enter").expect("close");
    app.wait_for("closed:1").expect("one close callback");
}

#[component]
fn TabList() -> impl IntoElement {
    let selected = signal(0);
    view()
        .child(tabs(
            t("Sections"),
            view()
                .flex_row()
                .child(tab(
                    t("First tab"),
                    move || selected.get() == 0,
                    || false,
                    move || selected.set(0),
                    text(t("First")),
                ))
                .child(tab(
                    t("Disabled tab"),
                    || false,
                    || true,
                    move || selected.set(1),
                    text(t("Disabled")),
                ))
                .child(tab(
                    t("Last tab"),
                    move || selected.get() == 2,
                    || false,
                    move || selected.set(2),
                    text(t("Last")),
                )),
        ))
        .child(text(move || format!("section:{}", selected.get())))
}

#[test]
fn tab_keyboard_navigation_selects_automatically_and_skips_disabled_tabs() {
    let mut app = start::<TabList>(DESKTOP);
    app.press_key("tab").expect("enter tab list");
    app.press_key("right").expect("next enabled tab");
    app.wait_for("section:2").expect("automatic tab activation");
    assert!(
        app.screen()
            .accessibility_state("role:tab[Last tab]")
            .expect("tab state")
            .selected
    );
    assert!(app.press("role:tab[Disabled tab]").is_err());
    app.press_key("home").expect("first tab");
    app.wait_for("section:0").expect("home activates first tab");
}
