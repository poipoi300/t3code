//! Port of apps/web/src/components/ui/calendar.tsx Calendar: react-day-picker's single-month,
//! single-selection grid with the web's class names, as CustomSnoozeDialog uses it.
//!
//! `calendar(CalendarOptions { selected, today, on_select, .. })`. Days are
//! `t3_i18n::CalendarDay`s (`calendar_day(y, m, d)`, `local_today()`).
//!
//! DayPicker behaviour carried over: outside days shown by default, the month caption between
//! the previous and next buttons, a roving keyboard focus (arrows by day and week, PageUp and
//! PageDown by month, with Shift by year, Home and End to the week's ends, moving the month with
//! the focus and passing over disabled days), Enter and Space select, `required` keeps a pressed
//! selected day selected, and the day button's label ("Today, …, selected").
//!
//! Departures:
//! - DayPicker formats with date-fns' English locale unless given another, so its copy is
//!   English on the web; here the month, weekday and label words are translation keys with the
//!   same English. The label's ordinal day ("November 14th") is a plain number ("November 14"):
//!   an ordinal has no locale-neutral key.
//! - Only the `mode="single"` caption-label layout is ported; the range classes (`range-*`) and
//!   the dropdown caption are not used by any web caller and are not built.
//! - `--cell-size` is `size-10` below `sm` and `size-9` above, from `layout_class()`.
//! - The `today` marker (`*:after:` dot) is an extra view inside the day button.
//! - DayPicker's roving tabindex (one day in the Tab order) needs a tab-index setting NSH's
//!   Button lacks, so every day button is a Tab stop; the arrow keys behave as on the web.

use std::rc::Rc;

use nsh_core::keymap::{Key, Keystroke, Modifiers};
use nsh_core::prelude::*;
use nsh_core::{KeyCapture, KeyEvent};
use t3_i18n::{CalendarDay, TimeZone};

use super::icon::{Icon, icon_with_class};
use crate::i18n::{Value, t, tr};
use crate::state::layout::layout_class;
use crate::theme::{cn, tw};

const DAY_MS: i64 = 86_400_000;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's days_from_civil).
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The day at `index` days since 1970-01-01.
pub fn day_at(index: i64) -> CalendarDay {
    let z = index + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    CalendarDay {
        year,
        month,
        day,
        index,
        weekday: (index + 4).rem_euclid(7) as u8,
    }
}

/// The day `year`-`month`-`day`, the day clamped to the month's length.
pub fn calendar_day(year: i32, month: u8, day: u8) -> CalendarDay {
    let month = month.clamp(1, 12);
    let day = day.clamp(1, days_in_month(year, month));
    day_at(days_from_civil(year, month, day))
}

fn days_in_month(year: i32, month: u8) -> u8 {
    let next = if month == 12 {
        days_from_civil(year + 1, 1, 1)
    } else {
        days_from_civil(year, month + 1, 1)
    };
    (next - days_from_civil(year, month, 1)) as u8
}

/// `day` moved by `months`, its day of the month clamped (date-fns `addMonths`).
pub fn add_months(day: CalendarDay, months: i32) -> CalendarDay {
    let total = day.year * 12 + i32::from(day.month) - 1 + months;
    calendar_day(
        total.div_euclid(12),
        (total.rem_euclid(12) + 1) as u8,
        day.day,
    )
}

fn start_of_month(day: CalendarDay) -> CalendarDay {
    calendar_day(day.year, day.month, 1)
}

/// The weeks DayPicker shows for the month of `month`: from the week holding its first day to
/// the week holding its last, each seven day indices from `week_starts_on` (0 is Sunday).
pub fn month_weeks(month: CalendarDay, week_starts_on: u8) -> Vec<[i64; 7]> {
    let first = start_of_month(month);
    let last = first.index + i64::from(days_in_month(first.year, first.month)) - 1;
    let back = (i64::from(first.weekday) - i64::from(week_starts_on)).rem_euclid(7);
    let mut start = first.index - back;
    let mut weeks = Vec::new();
    while start <= last {
        weeks.push(std::array::from_fn(|offset| start + offset as i64));
        start += 7;
    }
    weeks
}

/// The local calendar day now, following the app clock (`state::clock`).
pub fn local_today() -> CalendarDay {
    let now = crate::state::clock::now_ms();
    let settings = locale_settings();
    let zone = match settings.time_zone.as_str() {
        "UTC" | "Etc/UTC" | "GMT" => TimeZone::Utc,
        name => TimeZone::Iana(name.into()),
    };
    day_at((now + i64::from(zone.offset_seconds(now)) * 1000).div_euclid(DAY_MS))
}

/// Where the roving focus goes for a key (DayPicker's `getFocusableDate`), before skipping
/// disabled days; `None` for a key the grid ignores.
pub fn focus_target(
    from: CalendarDay,
    key: &str,
    shift: bool,
    week_starts_on: u8,
) -> Option<CalendarDay> {
    let back = (i64::from(from.weekday) - i64::from(week_starts_on)).rem_euclid(7);
    Some(match key {
        "ArrowLeft" => day_at(from.index - 1),
        "ArrowRight" => day_at(from.index + 1),
        "ArrowUp" => day_at(from.index - 7),
        "ArrowDown" => day_at(from.index + 7),
        "PageUp" => add_months(from, if shift { -12 } else { -1 }),
        "PageDown" => add_months(from, if shift { 12 } else { 1 }),
        "Home" => day_at(from.index - back),
        "End" => day_at(from.index - back + 6),
        _ => return None,
    })
}

/// DayPicker's focus move: from `target` onward in the key's direction until a day that is not
/// disabled, giving up after a year (DayPicker's own limit).
fn next_enabled(
    from: CalendarDay,
    key: &str,
    shift: bool,
    week_starts_on: u8,
    disabled: &dyn Fn(CalendarDay) -> bool,
) -> Option<CalendarDay> {
    let mut at = focus_target(from, key, shift, week_starts_on)?;
    let forward = at.index >= from.index;
    for _ in 0..366 {
        if !disabled(at) {
            return Some(at);
        }
        at = day_at(at.index + if forward { 1 } else { -1 });
    }
    None
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const WEEKDAYS_SHORT: [&str; 7] = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];

fn month_name(day: CalendarDay) -> String {
    t(MONTHS[usize::from(day.month - 1)])
}

fn caption(month: CalendarDay) -> String {
    tr(
        "{month} {year}",
        &[
            ("month", Value::Text(month_name(month))),
            ("year", Value::Int(i64::from(month.year))),
        ],
    )
}

fn day_label(day: CalendarDay, today: bool, selected: bool) -> String {
    let date = tr(
        "{weekday}, {month} {day}, {year}",
        &[
            (
                "weekday",
                Value::Text(t(WEEKDAYS[usize::from(day.weekday)])),
            ),
            ("month", Value::Text(month_name(day))),
            ("day", Value::Int(i64::from(day.day))),
            ("year", Value::Int(i64::from(day.year))),
        ],
    );
    let date = if today {
        tr("Today, {date}", &[("date", Value::Text(date))])
    } else {
        date
    };
    if selected {
        tr("{date}, selected", &[("date", Value::Text(date))])
    } else {
        date
    }
}

/// The DayPicker props the web passes.
#[derive(Clone)]
pub struct CalendarOptions {
    pub selected: Reactive<Option<CalendarDay>>,
    /// The month shown first: `defaultMonth`, else the selected day's, else today's.
    pub default_month: Option<CalendarDay>,
    pub today: Reactive<CalendarDay>,
    pub week_starts_on: u8,
    /// `disabled={{ before }}`.
    pub disabled_before: Option<CalendarDay>,
    pub show_outside_days: bool,
    pub required: bool,
    /// Called with the day pressed, or `None` when a selected day is pressed without `required`.
    pub on_select: Rc<dyn Fn(Option<CalendarDay>)>,
}

impl CalendarOptions {
    pub fn new(
        selected: impl IntoReactive<Option<CalendarDay>>,
        today: impl IntoReactive<CalendarDay>,
        on_select: impl Fn(Option<CalendarDay>) + 'static,
    ) -> Self {
        Self {
            selected: selected.into_reactive(),
            default_month: None,
            today: today.into_reactive(),
            week_starts_on: 0,
            disabled_before: None,
            show_outside_days: true,
            required: false,
            on_select: Rc::new(on_select),
        }
    }
}

const BUTTON_CLASSES: &str = "relative flex size-(--cell-size) text-base sm:text-sm items-center justify-center rounded-lg text-foreground disabled:pointer-events-none disabled:opacity-64";

fn cell_size() -> &'static str {
    if layout_class().is_compact() {
        "size-10"
    } else {
        "size-9"
    }
}

fn grid_keys() -> KeyCapture {
    let keys = [
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
    ];
    KeyCapture::keys(keys.into_iter().flat_map(|key| {
        [false, true].map(|shift| Keystroke {
            key,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
        })
    }))
}

/// `Calendar`.
pub fn calendar(options: CalendarOptions) -> impl IntoElement {
    let CalendarOptions {
        selected,
        default_month,
        today,
        week_starts_on,
        disabled_before,
        show_outside_days,
        required,
        on_select,
    } = options;
    let selected = computed(move || selected.get());
    let today = computed(move || today.get());
    let first_month = default_month
        .or_else(|| selected.get_untracked())
        .unwrap_or_else(|| today.get_untracked());
    let month = signal(start_of_month(first_month));
    let focused = signal(None::<CalendarDay>);
    let disabled: Rc<dyn Fn(CalendarDay) -> bool> = Rc::new(move |day: CalendarDay| {
        disabled_before.is_some_and(|before| day.index < before.index)
    });
    let weeks = computed(move || month_weeks(month.get(), week_starts_on));

    let shift_month = move |by: i32| month.update(|month| *month = add_months(*month, by));
    let nav_button = move |label: &'static str, icon: Icon, by: i32| {
        nsh_base::button()
            .accessibility_label(move || Some(SharedString::from(t(label))))
            .on_press(move || shift_month(by))
            .app_class(move || {
                tw(&cn([
                    Some(BUTTON_CLASSES),
                    Some(cell_size()),
                    Some("hover:bg-accent"),
                ]))
            })
            .child(icon_with_class(
                icon,
                "size-4.5 sm:size-4 shrink-0 opacity-80",
            ))
    };

    let select = on_select.clone();
    let press_day = Rc::new(move |day: CalendarDay| {
        focused.set(Some(day));
        if selected.get_untracked() == Some(day) && !required {
            select(None);
        } else {
            select(Some(day));
        }
    });

    let roving_disabled = disabled.clone();
    // The day that takes Tab: the focused one, else the selected one in this month, else
    // today in this month, else the month's first enabled day.
    let tab_stop = computed(move || {
        let shown = month.get();
        let in_month = |day: &CalendarDay| day.year == shown.year && day.month == shown.month;
        focused
            .get()
            .filter(in_month)
            .or_else(|| selected.get().filter(in_month))
            .or_else(|| {
                Some(today.get())
                    .filter(in_month)
                    .filter(|day| !roving_disabled(*day))
            })
            .or_else(|| {
                let first = start_of_month(shown).index;
                (first..first + i64::from(days_in_month(shown.year, shown.month)))
                    .map(day_at)
                    .find(|day| !roving_disabled(*day))
            })
    });

    let key_disabled = disabled.clone();
    let key_press = press_day.clone();
    view()
        .test_id("calendar")
        .app_class(tw("relative w-fit"))
        .child(
            view()
                .app_class(tw("absolute top-0 flex w-full justify-between z-1"))
                .child(nav_button("Go to the Previous Month", Icon::ChevronLeft, -1))
                .child(nav_button("Go to the Next Month", Icon::ChevronRight, 1)),
        )
        .child(
            view()
                .app_class(tw("w-full"))
                .child(
                    view()
                        .app_class(move || tw(&cn([
                            Some("relative px-1 mb-1 flex items-center justify-center z-2"),
                            Some(if layout_class().is_compact() { "mx-10 h-10" } else { "mx-9 h-9" }),
                        ])))
                        .child(
                            text(move || caption(month.get()))
                                .role(AccessibilityRole::Header)
                                .app_class(tw("text-base sm:text-sm font-medium flex items-center gap-2 h-full")),
                        ),
                )
                .child(
                    view()
                        .role(AccessibilityRole::Grid)
                        .label(move || SharedString::from(caption(month.get())))
                        .capture_keys(grid_keys())
                        .on_key_down(move |event: &KeyEvent| {
                            let Some(from) = focused.get_untracked().or_else(|| tab_stop.get_untracked()) else {
                                return;
                            };
                            if let Some(next) = next_enabled(from, event.key.as_str(), event.modifiers.shift, week_starts_on, &*key_disabled) {
                                focused.set(Some(next));
                                let shown = month.get_untracked();
                                if (next.year, next.month) != (shown.year, shown.month) {
                                    month.set(start_of_month(next));
                                }
                            }
                        })
                        .child(
                            view().app_class(tw("flex")).child(
                                (0..7u8)
                                    .map(|offset| {
                                        let weekday = usize::from((week_starts_on + offset) % 7);
                                        text(t(WEEKDAYS_SHORT[weekday]))
                                            .label(t(WEEKDAYS[weekday]))
                                            .app_class(move || tw(&cn([
                                                Some("flex items-center justify-center p-0 text-xs font-medium text-muted-foreground/72"),
                                                Some(cell_size()),
                                            ])))
                                            .into_element()
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                        )
                        .child(dynamic(move || {
                            let weeks = weeks.get();
                            let disabled = disabled.clone();
                            let press_day = key_press.clone();
                            untracked(move || {
                                weeks
                                    .into_iter()
                                    .map(|week| {
                                        view()
                                            .role(AccessibilityRole::Row)
                                            .app_class(tw("flex"))
                                            .child(
                                                week.into_iter()
                                                    .map(|index| {
                                                        calendar_day_cell(
                                                            day_at(index),
                                                            month,
                                                            selected,
                                                            today,
                                                            focused,
                                                            disabled.clone(),
                                                            show_outside_days,
                                                            press_day.clone(),
                                                        )
                                                    })
                                                    .collect::<Vec<_>>(),
                                            )
                                            .into_element()
                                    })
                                    .collect::<Vec<_>>()
                            })
                        })),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn calendar_day_cell(
    day: CalendarDay,
    month: Signal<CalendarDay>,
    selected: Memo<Option<CalendarDay>>,
    today: Memo<CalendarDay>,
    focused: Signal<Option<CalendarDay>>,
    disabled: Rc<dyn Fn(CalendarDay) -> bool>,
    show_outside_days: bool,
    press: Rc<dyn Fn(CalendarDay)>,
) -> AnyElement {
    let shown = month.get_untracked();
    let outside = (day.year, day.month) != (shown.year, shown.month);
    let is_disabled = disabled(day);
    let is_selected = move || selected.get() == Some(day);
    let is_today = move || today.get() == day;
    let cell = view().role(AccessibilityRole::Cell).app_class(move || {
        tw(&cn([
            Some("text-sm py-px"),
            Some(cell_size()),
            (outside && is_selected()).then_some("bg-accent/50"),
            (outside && !show_outside_days).then_some("invisible"),
        ]))
    });
    if outside && !show_outside_days {
        return cell.into_element();
    }
    let target = element_ref();
    effect(move || {
        if focused.get() == Some(day) {
            target.focus();
        }
    });
    cell.child(
        nsh_base::button()
            .element_ref(&target)
            // A cell built by a month change is not mounted when its effect first runs: it takes
            // the roving focus once it is laid out.
            .on_layout(move |_: &Bounds<Points>| {
                if focused.get_untracked() == Some(day) {
                    target.focus();
                }
            })
            .accessibility_label(move || Some(SharedString::from(day_label(day, is_today(), is_selected()))))
            .selected(is_selected)
            .disabled(is_disabled)
            .on_press(move || {
                if !is_disabled {
                    press(day);
                }
            })
            .app_class(move || {
                let chosen = is_selected();
                tw(&cn([
                    Some(BUTTON_CLASSES),
                    Some(cell_size()),
                    (!chosen).then_some("hover:bg-accent"),
                    Some("outline-none focus-visible:z-1 focus-visible:ring-[3px] focus-visible:ring-inset focus-visible:ring-ring/50"),
                    chosen.then_some("bg-primary text-primary-foreground"),
                    is_disabled.then_some("pointer-events-none text-muted-foreground/72 line-through"),
                    (outside && !chosen).then_some("text-muted-foreground/72"),
                    (outside && chosen).then_some("text-primary-foreground"),
                ]))
            })
            .child(text(day.day.to_string()))
            .child(when(is_today, move || {
                    view().accessible(false).app_class(move || tw(&cn([
                        Some("pointer-events-none absolute bottom-1 start-1/2 z-1 size-[3px] -translate-x-1/2 rounded-full"),
                        Some(if is_disabled {
                            "bg-foreground/30"
                        } else if is_selected() {
                            "bg-background"
                        } else {
                            "bg-primary"
                        }),
                    ])))
            })),
    )
    .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip_and_know_their_weekday() {
        let day = calendar_day(2026, 11, 14);
        assert_eq!(day.weekday, 6, "2026-11-14 is a Saturday");
        assert_eq!(day_at(day.index), day);
        assert_eq!(day_at(0), calendar_day(1970, 1, 1));
        assert_eq!(
            calendar_day(2024, 2, 31).day,
            29,
            "clamped to a leap February"
        );
        assert_eq!(calendar_day(2023, 2, 31).day, 28);
    }

    #[test]
    fn months_add_with_the_day_clamped() {
        assert_eq!(
            add_months(calendar_day(2026, 1, 31), 1),
            calendar_day(2026, 2, 28)
        );
        assert_eq!(
            add_months(calendar_day(2026, 12, 15), 1),
            calendar_day(2027, 1, 15)
        );
        assert_eq!(
            add_months(calendar_day(2026, 1, 15), -13),
            calendar_day(2024, 12, 15)
        );
    }

    #[test]
    fn weeks_cover_the_month_from_the_chosen_first_weekday() {
        // November 2026 starts on a Sunday and ends on a Monday.
        let sunday = month_weeks(calendar_day(2026, 11, 20), 0);
        assert_eq!(sunday.len(), 5);
        assert_eq!(day_at(sunday[0][0]), calendar_day(2026, 11, 1));
        assert_eq!(day_at(sunday[4][6]), calendar_day(2026, 12, 5));
        let monday = month_weeks(calendar_day(2026, 11, 20), 1);
        assert_eq!(monday.len(), 6);
        assert_eq!(day_at(monday[0][0]), calendar_day(2026, 10, 26));
        assert_eq!(day_at(monday[5][0]), calendar_day(2026, 11, 30));
        // February 2026 starts on a Sunday and fills exactly four weeks.
        assert_eq!(month_weeks(calendar_day(2026, 2, 1), 0).len(), 4);
    }

    #[test]
    fn keys_move_the_focus_and_pass_over_disabled_days() {
        let from = calendar_day(2026, 11, 14);
        assert_eq!(
            focus_target(from, "ArrowRight", false, 0),
            Some(calendar_day(2026, 11, 15))
        );
        assert_eq!(
            focus_target(from, "ArrowUp", false, 0),
            Some(calendar_day(2026, 11, 7))
        );
        assert_eq!(
            focus_target(from, "PageDown", false, 0),
            Some(calendar_day(2026, 12, 14))
        );
        assert_eq!(
            focus_target(from, "PageUp", true, 0),
            Some(calendar_day(2025, 11, 14))
        );
        assert_eq!(
            focus_target(from, "Home", false, 1),
            Some(calendar_day(2026, 11, 9))
        );
        assert_eq!(
            focus_target(from, "End", false, 0),
            Some(calendar_day(2026, 11, 14))
        );
        assert_eq!(focus_target(from, "Enter", false, 0), None);
        let before = calendar_day(2026, 11, 12);
        let disabled = move |day: CalendarDay| day.index < before.index;
        assert_eq!(
            next_enabled(from, "ArrowUp", false, 0, &disabled),
            None,
            "nothing earlier is enabled"
        );
        assert_eq!(
            next_enabled(calendar_day(2026, 11, 13), "ArrowLeft", false, 0, &disabled),
            Some(calendar_day(2026, 11, 12))
        );
    }
}
