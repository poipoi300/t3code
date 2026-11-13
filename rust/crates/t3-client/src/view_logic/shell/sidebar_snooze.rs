//! Local calendar conversion and locale formatting are host operations; preset policy is shared.
use std::collections::BTreeSet;
use t3_contracts::settings::{SnoozePresetDay as Day, SnoozePresetSetting as Setting};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub id: String,
    pub label: String,
    pub when_label: String,
    pub snoozed_until: String,
    pub wake_ms: i64,
}
pub struct Calendar<'a> {
    pub now_ms: i64,
    pub weekday: u8,
    /// Set a local calendar day/time, preserving DST rules instead of adding 24-hour durations.
    pub local_wake: &'a dyn Fn(i64, u32, u32) -> Option<i64>,
    pub same_day: &'a dyn Fn(i64, i64) -> bool,
    pub format_time: &'a dyn Fn(i64) -> String,
    pub short_weekday: &'a dyn Fn(i64) -> String,
    pub to_iso: &'a dyn Fn(i64) -> String,
}
fn day_name(day: &Day) -> &'static str {
    match day {
        Day::Today => "today",
        Day::Tomorrow => "tomorrow",
        Day::Monday => "monday",
        Day::Tuesday => "tuesday",
        Day::Wednesday => "wednesday",
        Day::Thursday => "thursday",
        Day::Friday => "friday",
        Day::Saturday => "saturday",
        Day::Sunday => "sunday",
    }
}
pub fn preset_id(setting: &Setting) -> String {
    match setting {
        Setting::After1(s) => format!("after:{}", s.minutes),
        Setting::At2(s) => format!("at:{}:{}", day_name(&s.day), s.time),
    }
}
pub fn resolve_preset(setting: &Setting, calendar: &Calendar<'_>) -> Option<Preset> {
    let (wake, label, names_day) = match setting {
        Setting::After1(s) => {
            let (count, unit) = if s.minutes % 1440 == 0 {
                (s.minutes / 1440, "day")
            } else if s.minutes % 60 == 0 {
                (s.minutes / 60, "hour")
            } else {
                (s.minutes, "minute")
            };
            (
                calendar
                    .now_ms
                    .checked_add(s.minutes.checked_mul(60_000)?)?,
                format!("In {count} {unit}{}", if count == 1 { "" } else { "s" }),
                false,
            )
        }
        Setting::At2(s) => {
            let days = match s.day {
                Day::Today => 0,
                Day::Tomorrow => 1,
                ref day => {
                    let target = match day {
                        Day::Sunday => 0,
                        Day::Monday => 1,
                        Day::Tuesday => 2,
                        Day::Wednesday => 3,
                        Day::Thursday => 4,
                        Day::Friday => 5,
                        Day::Saturday => 6,
                        _ => 0,
                    };
                    let diff = (target - i64::from(calendar.weekday) + 7) % 7;
                    if diff == 0 { 7 } else { diff }
                }
            };
            let hour = s.time.get(..2)?.parse().ok()?;
            let minute = s.time.get(3..)?.parse().ok()?;
            let wake = (calendar.local_wake)(days, hour, minute)?;
            if wake <= calendar.now_ms {
                return None;
            }
            let label = if days == 0 {
                if hour < 12 {
                    "This morning".into()
                } else if hour < 17 {
                    "This afternoon".into()
                } else {
                    "This evening".into()
                }
            } else if days == 1 {
                "Tomorrow".into()
            } else if s.day == Day::Monday {
                "Next week".into()
            } else {
                let name = day_name(&s.day);
                let title = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
                if days == 7 {
                    format!("Next {title}")
                } else {
                    title
                }
            };
            let names_day = label != "Next week";
            (wake, label, names_day)
        }
    };
    let time = (calendar.format_time)(wake);
    let when_label = if names_day || (calendar.same_day)(wake, calendar.now_ms) {
        time
    } else {
        format!("{} {time}", (calendar.short_weekday)(wake))
    };
    Some(Preset {
        id: preset_id(setting),
        label,
        when_label,
        snoozed_until: (calendar.to_iso)(wake),
        wake_ms: wake,
    })
}
pub fn resolve_presets(settings: &[Setting], calendar: &Calendar<'_>) -> Vec<Preset> {
    let mut presets = settings
        .iter()
        .filter_map(|s| resolve_preset(s, calendar))
        .collect::<Vec<_>>();
    presets.sort_by_key(|p| p.wake_ms.div_euclid(60_000));
    let mut seen = BTreeSet::new();
    presets.retain(|p| seen.insert(p.wake_ms.div_euclid(60_000)));
    presets
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickDecision {
    Picked(Preset),
    Expired,
}
pub fn pick_preset(
    shown: &[Preset],
    id: &str,
    settings: &[Setting],
    calendar: &Calendar<'_>,
) -> PickDecision {
    let duration = settings
        .iter()
        .find(|s| matches!(s, Setting::After1(_)) && preset_id(s) == id)
        .and_then(|s| resolve_preset(s, calendar));
    duration
        .or_else(|| {
            shown
                .iter()
                .find(|p| p.id == id && p.wake_ms > calendar.now_ms)
                .cloned()
        })
        .map_or(PickDecision::Expired, PickDecision::Picked)
}
pub fn wake_label(wake: Option<i64>, now: Option<i64>) -> String {
    let Some(remaining) = wake
        .zip(now)
        .map(|(w, n)| i128::from(w) - i128::from(n))
        .filter(|r| *r > 0)
    else {
        return "now".into();
    };
    let (unit, suffix) = if remaining < 3_600_000 {
        (60_000, "m")
    } else if remaining < 86_400_000 {
        (3_600_000, "h")
    } else {
        (86_400_000, "d")
    };
    format!("{}{suffix}", (remaining + unit - 1) / unit)
}
