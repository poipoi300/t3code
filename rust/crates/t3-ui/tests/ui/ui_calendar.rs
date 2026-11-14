//! UI-CALENDAR: the calendar as CustomSnoozeDialog mounts it: single, required, past days
//! disabled, the selected day's month shown first.
use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::{
    theme,
    ui::calendar::{CalendarOptions, calendar, calendar_day},
};

#[component]
fn Fixture() -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let today = calendar_day(2026, 11, 14);
    let date = signal(Some(calendar_day(2026, 11, 20)));
    let picks = signal(0u32);
    view()
        .child(text(move || {
            date.get().map_or_else(
                || String::from("date none"),
                |d| format!("date {}-{}-{}", d.year, d.month, d.day),
            )
        }))
        .child(text(move || format!("picks {}", picks.get())))
        .child(calendar(CalendarOptions {
            required: true,
            disabled_before: Some(today),
            default_month: date.get_untracked(),
            ..CalendarOptions::new(
                move || date.get(),
                move || today,
                move |next| {
                    picks.update(|n| *n += 1);
                    date.set(next);
                },
            )
        }))
}

fn start(window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<Fixture>())
    };
    // A press moves the keyboard focus to the day on the web, macOS and Linux.
    config.environment.os = nsh_core::platform::Os::Linux;
    let mut app = Headless::start(config).expect("the engine starts");
    app.wait_for("November 2026")
        .expect("the selected day's month");
    app
}

#[test]
fn pressing_days_selects_them_but_never_a_past_one_or_nothing() {
    for window in [PHONE, DESKTOP] {
        let mut app = start(window);
        app.wait_for("label:Today, Saturday, November 14, 2026")
            .expect("today is labelled");
        app.wait_for("label:Friday, November 20, 2026, selected")
            .expect("selected day");
        app.press("label:Monday, November 23, 2026").expect("pick");
        app.wait_for("date 2026-11-23").expect("selected");
        // required: pressing the selected day keeps it.
        app.press("label:Monday, November 23, 2026, selected")
            .expect("press again");
        app.wait_for("picks 2").expect("reported");
        assert!(app.shows("date 2026-11-23"));
        // Days before today are disabled.
        assert!(
            !app.screen()
                .enabled("label:Tuesday, November 10, 2026")
                .expect("enabled")
        );
        let _ = app.press("label:Tuesday, November 10, 2026");
        app.settle();
        assert!(app.shows("picks 2"), "a disabled day does not select");
    }
}

#[test]
fn the_month_buttons_and_page_keys_change_the_month() {
    let mut app = start(DESKTOP);
    app.press("role:button[Go to the Next Month]")
        .expect("next");
    app.wait_for("December 2026").expect("december");
    // Outside days of the next month are shown (DayPicker's showOutsideDays).
    app.wait_for("label:Friday, January 1, 2027")
        .expect("outside day");
    app.press("role:button[Go to the Previous Month]")
        .expect("previous");
    app.wait_for("November 2026").expect("back");
    app.press("label:Monday, November 23, 2026")
        .expect("focus a day");
    app.press_key("ArrowDown").expect("a week on");
    app.press_key("Enter").expect("select");
    app.wait_for("date 2026-11-30")
        .expect("the focused day selected");
    app.press_key("ArrowDown").expect("into december");
    app.wait_for("December 2026")
        .expect("the month follows the focus");
    app.press_key("Enter").expect("select");
    app.wait_for("date 2026-12-7")
        .expect("selected across the month");
}
