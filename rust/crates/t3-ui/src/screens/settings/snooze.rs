//! Port of apps/web/src/components/settings/SnoozePresetsSettings.tsx: the options of every
//! snooze menu. The editor's logic is `t3_client::services::settings::panels` (draft
//! reconciliation, row order, candidates, kind changes); wake times come from the client's
//! snooze resolver over the local calendar below.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use nsh_core::prelude::*;
use serde_json::json;
use t3_client::services::settings::panels::{
    MAX_SNOOZE_PRESET_MINUTES, MAX_SNOOZE_PRESETS, next_snooze_candidate, reconcile_snooze_draft,
    same_snooze_presets, snooze_unit_minutes, snooze_with_when, sort_snooze_rows,
    split_snooze_minutes,
};
use t3_client::thread_views::shelves::{
    SnoozeCalendar, resolve_snooze_preset, resolve_snooze_presets,
};
use t3_contracts::settings::{
    ServerSettings, SnoozePresetSetting, SnoozePresetSettingMember1,
    SnoozePresetSettingMember1Kind, SnoozePresetSettingMember2, TimestampFormat,
};

use super::shell::{
    RowReset, SettingsContext, SettingsRowProps, region, settings_row, settings_select,
};
use crate::i18n::{Text, Value, t, tr};
use crate::theme::{palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;

/// The local calendar snooze presets resolve against: the system time zone (DST-aware through
/// `t3-i18n`), the user's clock format, and `now_ms`.
pub fn with_snooze_calendar<R>(
    now_ms: i64,
    format: &TimestampFormat,
    body: impl FnOnce(&SnoozeCalendar<'_>) -> R,
) -> R {
    use t3_client::view_logic::session::timestamps::resolve_hour12;
    use t3_i18n::format::civil;
    use t3_i18n::{DateFormat, TimeZone};
    const DAY_MS: i64 = 86_400_000;
    let settings = locale_settings();
    let zone = match settings.time_zone.as_str() {
        "UTC" | "Etc/UTC" | "GMT" => TimeZone::Utc,
        name => TimeZone::Iana(name.into()),
    };
    let hour12 = resolve_hour12(format, settings.hour_cycle.twelve_hour());
    let offset = |ms: i64| i64::from(zone.offset_seconds(ms)) * 1000;
    let today = civil(now_ms, &zone).day;
    // `Date#setDate` + `setHours`: a wall time `days` ahead, through the zone's offset at that
    // instant (resolved twice, so a DST change between now and then is honoured).
    let local_wake = |days: i64, hour: u32, minute: u32| {
        let local = (today.index + days) * DAY_MS
            + i64::from(hour) * 3_600_000
            + i64::from(minute) * 60_000;
        Some(local - offset(local - offset(local)))
    };
    let same_day = |a: i64, b: i64| civil(a, &zone).day.index == civil(b, &zone).day.index;
    let formats = crate::i18n::formats();
    let format_time = |ms: i64| formats.date(DateFormat::TimeHm, ms, &zone, Some(hour12));
    let short_weekday = |ms: i64| formats.date(DateFormat::WeekdayShort, ms, &zone, None);
    let to_iso = |ms: i64| t3_client::time::iso_from_epoch_ms(ms.max(0) as u64);
    body(&SnoozeCalendar {
        now_ms,
        weekday: today.weekday,
        local_wake: &local_wake,
        same_day: &same_day,
        format_time: &format_time,
        short_weekday: &short_weekday,
        to_iso: &to_iso,
    })
}

const WHEN_LABELS: [(&str, &str); 10] = [
    ("after", "In"),
    ("today", "Today"),
    ("tomorrow", "Tomorrow"),
    ("monday", "Monday"),
    ("tuesday", "Tuesday"),
    ("wednesday", "Wednesday"),
    ("thursday", "Thursday"),
    ("friday", "Friday"),
    ("saturday", "Saturday"),
    ("sunday", "Sunday"),
];

/// Saves the rows: the presets, and their keys when they change with them.
type SaveRows = Rc<dyn Fn(Vec<SnoozePresetSetting>, Option<Vec<u64>>)>;

/// One state for presets and their keys, so a sort queued with an edit applies on top of it.
/// Keys survive removals and sorts, so a row's in-progress field state stays with it.
#[derive(Clone, PartialEq, Default)]
struct SnoozeRows {
    presets: Vec<SnoozePresetSetting>,
    keys: Vec<u64>,
}

impl SnoozeRows {
    fn sorted(
        presets: &[SnoozePresetSetting],
        keys: &[u64],
        now: i64,
        format: &TimestampFormat,
    ) -> Self {
        let (presets, keys) = with_snooze_calendar(now, format, |calendar| {
            sort_snooze_rows(presets, keys, calendar)
        });
        Self { presets, keys }
    }
}

#[derive(Clone, PartialEq)]
enum Preview {
    Menu { label: String, when: String },
    Muted(&'static str),
}

/// `SnoozePresetsRow`.
pub fn snooze_presets_row(context: SettingsContext) -> impl IntoElement {
    let saved = computed(move || context.server_settings().snooze_presets.clone());
    let format = computed(move || context.client_settings().timestamp_format.clone());
    // `useNowMinute`: the app clock ticks twice a minute.
    let now = || crate::state::clock::now_ms();
    let initial = untracked(|| saved.get());
    let next_key = Rc::new(Cell::new(initial.len() as u64));
    let rows = signal(untracked(|| {
        SnoozeRows::sorted(
            &initial,
            &(0..initial.len() as u64).collect::<Vec<_>>(),
            now(),
            &format.get(),
        )
    }));
    let written: Rc<RefCell<Vec<Vec<SnoozePresetSetting>>>> = Rc::default();
    // `None` adopts the saved value, as after a failed write; `retry` re-runs the adoption.
    let source = Rc::new(RefCell::new(Some(initial)));
    let retry = signal(0_u32);
    let fresh = {
        let next_key = next_key.clone();
        move |count: usize| {
            let first = next_key.get();
            next_key.set(first + count as u64);
            (first..first + count as u64).collect::<Vec<_>>()
        }
    };
    {
        let written = written.clone();
        let source = source.clone();
        let fresh = fresh.clone();
        // The draft adopts saved values it did not write; echoes of its own writes keep it.
        effect(move || {
            retry.get();
            let current = saved.get();
            if source.borrow().as_ref() == Some(&current) {
                return;
            }
            *source.borrow_mut() = Some(current.clone());
            let pending = written.borrow().clone();
            match reconcile_snooze_draft(&pending, &current) {
                None => {
                    written.borrow_mut().clear();
                    let keys = fresh(current.len());
                    rows.set(SnoozeRows::sorted(
                        &current,
                        &keys,
                        untracked(now),
                        &untracked(|| format.get()),
                    ));
                }
                Some(remaining) => *written.borrow_mut() = remaining,
            }
        });
    }
    // Environments disagree, so equal-looking edits and Reset must still write to all of them.
    let mixed = move || context.mixed(&["snoozePresets"]);
    let save: SaveRows = Rc::new(move |presets, keys| {
        let current = rows.get_untracked();
        if !mixed() && presets == current.presets {
            return;
        }
        let keys = keys.unwrap_or(current.keys);
        rows.set(SnoozeRows {
            presets: presets.clone(),
            keys,
        });
        written.borrow_mut().push(presets.clone());
        let written = written.clone();
        let source = source.clone();
        context.update_with(json!({ "snoozePresets": presets }), move || {
            written.borrow_mut().clear();
            *source.borrow_mut() = None;
            retry.update(|n| *n += 1);
        });
    });
    let defaults = || ServerSettings::default().snooze_presets;
    let reset_save = save.clone();
    let reset_fresh = fresh.clone();
    settings_row(
        SettingsRowProps::searchable("snooze-options")
            .first()
            .server(&["snoozePresets"])
            .description(|| {
                Text::from("Choices in every snooze menu, listed soonest first. Weekdays mean the next one after today.")
            })
            .reset(RowReset::new(
                "snooze options",
                move || mixed() || !rows.with(|r| same_snooze_presets(&r.presets, &defaults())),
                move || {
                    let defaults = defaults();
                    let keys = reset_fresh(defaults.len());
                    let sorted = SnoozeRows::sorted(&defaults, &keys, untracked(now), &untracked(|| format.get()));
                    reset_save(sorted.presets, Some(sorted.keys));
                },
            ))
            .children(move || snooze_preset_list(rows, save.clone(), fresh.clone(), format, now)),
    )
}

/// `SnoozePresetList`.
fn snooze_preset_list(
    rows: Signal<SnoozeRows>,
    save: SaveRows,
    fresh: impl Fn(usize) -> Vec<u64> + 'static,
    format: Memo<TimestampFormat>,
    now: impl Fn() -> i64 + Copy + 'static,
) -> impl IntoElement {
    let previews = computed(move || {
        let presets = rows.with(|r| r.presets.clone());
        with_snooze_calendar(now(), &format.get(), |calendar| {
            let mut ids = std::collections::BTreeSet::new();
            let mut wakes = std::collections::BTreeSet::new();
            let previews: Vec<Preview> = presets
                .iter()
                .map(|preset| match resolve_snooze_preset(preset, calendar) {
                    None => Preview::Muted("Passed for today"),
                    Some(resolved) if ids.contains(&resolved.id) => {
                        Preview::Muted("Duplicate of an option above")
                    }
                    Some(resolved) if wakes.contains(&resolved.snoozed_until) => {
                        Preview::Muted("Same time as an option above")
                    }
                    Some(resolved) => {
                        ids.insert(resolved.id);
                        wakes.insert(resolved.snoozed_until);
                        Preview::Menu {
                            label: resolved.label,
                            when: resolved.when_label,
                        }
                    }
                })
                .collect();
            let menu = resolve_snooze_presets(&presets, calendar);
            let order: Vec<String> = menu
                .iter()
                .map(|preset| {
                    if menu
                        .iter()
                        .filter(|other| other.label == preset.label)
                        .count()
                        > 1
                    {
                        format!("{} {}", preset.label, preset.when_label)
                    } else {
                        preset.label.clone()
                    }
                })
                .collect();
            (previews, order)
        })
    });
    let full = move || rows.with(|r| r.presets.len() >= MAX_SNOOZE_PRESETS);
    let list = element_ref();
    // The web settles when focus leaves the list; a select's popup counts as inside it.
    list.on_focus_out(move || {
        let current = rows.get_untracked();
        let sorted = SnoozeRows::sorted(
            &current.presets,
            &current.keys,
            untracked(now),
            &untracked(|| format.get()),
        );
        if sorted != current {
            rows.set(sorted);
        }
    });
    let row_save = save.clone();
    let add_save = save;
    view()
        .element_ref(&list)
        .app_class(tw("flex flex-col gap-1.5 pt-3 pb-2"))
        .child(each(
            move || {
                rows.with(|r| {
                    r.keys
                        .iter()
                        .copied()
                        .zip(r.presets.iter().cloned())
                        .collect::<Vec<_>>()
                })
            },
            |(key, _)| *key,
            move |item, index| snooze_preset_row(rows, item, index, previews, row_save.clone()),
        ))
        .child(
            text(move || {
                previews.with(|(_, order)| {
                    if order.is_empty() {
                        t("Menus offer only Custom…")
                    } else {
                        tr(
                            "Menu now: {options}",
                            &[("options", Value::Text(order.join(" · ")))],
                        )
                    }
                })
            })
            .app_class(tw("text-xs text-muted-foreground")),
        )
        .child(
            view()
                .app_class(tw("flex flex-row items-center"))
                .child(button(
                    ButtonContent::IconLabel {
                        icon: Icon::Plus,
                        label: t("Add option"),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::Compact,
                    full,
                    move || {
                        let current = rows.get_untracked();
                        let mut presets = current.presets.clone();
                        presets.push(next_snooze_candidate(&current.presets));
                        let mut keys = current.keys;
                        keys.extend(fresh(1));
                        add_save(presets, Some(keys));
                    },
                ))
                .child(when(full, || {
                    text(tr(
                        "Up to {count} options",
                        &[("count", Value::Text(MAX_SNOOZE_PRESETS.to_string()))],
                    ))
                    .app_class(tw("ps-2 text-xs text-muted-foreground"))
                })),
        )
}

fn snooze_preset_row(
    rows: Signal<SnoozeRows>,
    item: ReadSignal<(u64, SnoozePresetSetting)>,
    index: ReadSignal<usize>,
    previews: Memo<(Vec<Preview>, Vec<String>)>,
    save: SaveRows,
) -> impl IntoElement {
    let preset = computed(move || item.with(|(_, preset)| preset.clone()));
    let number = move || Value::Text((index.get() + 1).to_string());
    let change_save = save.clone();
    let on_change = move |next: SnoozePresetSetting| {
        let mut presets = rows.get_untracked().presets;
        if let Some(slot) = presets.get_mut(index.get_untracked()) {
            *slot = next;
        }
        change_save(presets, None);
    };
    view()
        .role(AccessibilityRole::Group)
        .label(move || tr("Snooze option {index}", &[("index", number())]))
        .app_class(tw("flex flex-row flex-wrap items-center gap-2"))
        .child(snooze_preset_fields(preset, on_change))
        .child(
            view()
                .app_class(tw("ml-auto flex min-w-0 flex-row items-center gap-2"))
                .child(region(
                    move || previews.with(|(previews, _)| previews.get(index.get()).cloned()),
                    move |preview| {
                        // Client labels with counts and formatted times: content.
                        match preview {
                            Some(Preview::Menu { label, when }) => view()
                                .app_class(tw("flex min-w-0 flex-row truncate text-xs"))
                                .child(text(label).app_class(tw("text-xs text-foreground")))
                                .child(text(when).app_class(tw(
                                    "ps-2 text-xs tabular-nums text-muted-foreground",
                                )))
                                .into_element(),
                            Some(Preview::Muted(key)) => text(t(key))
                                .app_class(tw("truncate text-xs text-muted-foreground"))
                                .into_element(),
                            None => view().into_element(),
                        }
                    },
                ))
                .child(button(
                    ButtonContent::Icon {
                        icon: Icon::X,
                        label: untracked(|| {
                            tr("Remove snooze option {index}", &[("index", number())])
                        }),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::IconSm,
                    || false,
                    move || {
                        let current = rows.get_untracked();
                        let index = index.get_untracked();
                        let mut presets = current.presets;
                        let mut keys = current.keys;
                        if index < presets.len() {
                            presets.remove(index);
                            keys.remove(index);
                        }
                        save(presets, Some(keys));
                    },
                )),
        )
}

/// `SnoozePresetFields`: When, then a duration or a time of day.
fn snooze_preset_fields(
    preset: Memo<SnoozePresetSetting>,
    on_change: impl Fn(SnoozePresetSetting) + 'static,
) -> impl IntoElement {
    let on_change = Rc::new(on_change);
    // Switching between a duration and a set time keeps the other kind's last value, so a
    // misclick on "When" can be undone without retyping it.
    let memory = signal((60_i64, "09:00".to_owned()));
    let after = computed(move || matches!(preset.get(), SnoozePresetSetting::After1(_)));
    let when_change = on_change.clone();
    view()
        .app_class(tw("flex min-w-0 flex-row flex-wrap items-center gap-2"))
        .child(settings_select(
            "When",
            "w-32",
            WHEN_LABELS
                .iter()
                .map(|(value, label)| (*value, Text::from(*label)))
                .collect(),
            move || match preset.get() {
                SnoozePresetSetting::After1(_) => "after".to_owned(),
                SnoozePresetSetting::At2(at) => serde_json::to_value(&at.day)
                    .ok()
                    .and_then(|day| day.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            },
            || false,
            move |next| {
                let current = preset.get_untracked();
                let (minutes, time) = memory.get_untracked();
                let Some(changed) = snooze_with_when(&current, &next, minutes, &time) else {
                    return;
                };
                match (&current, &changed) {
                    (SnoozePresetSetting::After1(after), SnoozePresetSetting::At2(_)) => {
                        memory.update(|m| m.0 = after.minutes);
                    }
                    (SnoozePresetSetting::At2(at), SnoozePresetSetting::After1(_)) => {
                        memory.update(|m| m.1 = at.time.clone());
                    }
                    _ => {}
                }
                when_change(changed);
            },
        ))
        .child(dynamic(move || {
            let on_change = on_change.clone();
            if after.get() {
                let minutes = computed(move || match preset.get() {
                    SnoozePresetSetting::After1(after) => after.minutes,
                    SnoozePresetSetting::At2(_) => 60,
                });
                untracked(|| {
                    snooze_duration_fields(minutes, move |minutes| {
                        on_change(SnoozePresetSetting::After1(Box::new(
                            SnoozePresetSettingMember1 {
                                kind: SnoozePresetSettingMember1Kind::After,
                                minutes,
                            },
                        )))
                    })
                    .into_element()
                })
            } else {
                untracked(|| snooze_time_field(preset, move |next| on_change(next)).into_element())
            }
        }))
}

/// The time of day. The web's `<input type="time">` has no NSH counterpart; this field takes
/// `HH:MM` and saves each complete, valid value, as the web's `onChange` does.
fn snooze_time_field(
    preset: Memo<SnoozePresetSetting>,
    on_change: impl Fn(SnoozePresetSetting) + 'static,
) -> impl IntoElement {
    let at = computed(move || match preset.get() {
        SnoozePresetSetting::At2(at) => Some(*at),
        SnoozePresetSetting::After1(_) => None,
    });
    let saved = computed(move || at.get().map(|at| at.time).unwrap_or_default());
    let draft = signal(untracked(|| saved.get()));
    effect(move || draft.set(saved.get()));
    view()
        .app_class(tw("flex min-w-0 flex-row flex-wrap items-center gap-2"))
        .child(text(t("at")).app_class(tw("text-xs text-muted-foreground")))
        .child(
            nsh_base::input()
                .value(move || draft.get())
                .on_change(move |value: &str| {
                    draft.set(value.to_owned());
                    if !valid_time(value) {
                        return;
                    }
                    if let Some(current) = at.get_untracked()
                        && current.time != value
                    {
                        on_change(SnoozePresetSetting::At2(Box::new(SnoozePresetSettingMember2 {
                            time: value.to_owned(),
                            ..current
                        })));
                    }
                })
                .on_blur(move || draft.set(saved.get_untracked()))
                .label(t("Time"))
                .placeholder(|| "09:00".to_owned())
                .text_color(move || palette().foreground)
                .app_class(tw("h-8 w-32 rounded-lg border border-input bg-background px-[11px] text-sm shadow-xs/5 dark:bg-input/32")),
        )
}

/// `/^\d{2}:\d{2}$/` and a real time of day (the time input only produces those).
fn valid_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[2] == b':'
        && [0, 1, 3, 4].iter().all(|&i| bytes[i].is_ascii_digit())
        && value[..2].parse::<u8>().is_ok_and(|hour| hour < 24)
        && value[3..].parse::<u8>().is_ok_and(|minute| minute < 60)
}

/// `DurationFields`: an amount committed on blur or Enter, and its unit.
fn snooze_duration_fields(
    minutes: Memo<i64>,
    on_change: impl Fn(i64) + 'static,
) -> impl IntoElement {
    let on_change = Rc::new(on_change);
    // The chosen unit stays put while stepping, so 59 → 60 minutes does not jump to hours.
    let chosen = signal(untracked(|| split_snooze_minutes(minutes.get()).1));
    let unit = computed(move || {
        let minutes = minutes.get();
        let chosen = chosen.get();
        if minutes % snooze_unit_minutes(chosen) == 0 {
            chosen
        } else {
            split_snooze_minutes(minutes).1
        }
    });
    let amount = computed(move || minutes.get() / snooze_unit_minutes(unit.get()));
    // Typing stays local until the field commits, so half-typed values never reach the server.
    let draft = signal(untracked(|| amount.get().to_string()));
    effect(move || draft.set(amount.get().to_string()));
    let commit = {
        let on_change = on_change.clone();
        move || {
            let amount = amount.get_untracked();
            let max = MAX_SNOOZE_PRESET_MINUTES / snooze_unit_minutes(unit.get_untracked());
            let Ok(value) = draft.get_untracked().trim().parse::<f64>() else {
                draft.set(amount.to_string());
                return;
            };
            let next = (value.round() as i64).clamp(1, max);
            draft.set(next.to_string());
            if next != amount {
                on_change(next * snooze_unit_minutes(unit.get_untracked()));
            }
        }
    };
    let submit = commit.clone();
    let units: [(&str, &str, &str); 3] = [
        ("minutes", "minutes", "minute"),
        ("hours", "hours", "hour"),
        ("days", "days", "day"),
    ];
    view()
        .app_class(tw("flex min-w-0 flex-row flex-wrap items-center gap-2"))
        .child(
            nsh_base::input()
                .value(move || draft.get())
                .keyboard_type(KeyboardType::Number)
                .on_change(move |text: &str| draft.set(text.to_owned()))
                .on_blur(commit)
                .on_submit(move |_: &str| submit())
                .submit_behavior(SubmitBehavior::Submit)
                .label(t("Amount"))
                .text_color(move || palette().foreground)
                .app_class(tw("h-8 w-32 rounded-lg border border-input bg-background px-[11px] text-sm shadow-xs/5 dark:bg-input/32")),
        )
        .child(dynamic(move || {
            let singular = amount.get() == 1;
            let on_change = on_change.clone();
            untracked(|| {
                settings_select(
                    "Unit",
                    "w-28",
                    units
                        .iter()
                        .map(|(value, plural, one)| (*value, Text::from(if singular { *one } else { *plural })))
                        .collect(),
                    move || unit.get().to_owned(),
                    || false,
                    move |next| {
                        let Some(next) = ["minutes", "hours", "days"].into_iter().find(|u| *u == next) else {
                            return;
                        };
                        chosen.set(next);
                        on_change((amount.get_untracked() * snooze_unit_minutes(next)).min(MAX_SNOOZE_PRESET_MINUTES));
                    },
                )
                .into_element()
            })
        }))
}
