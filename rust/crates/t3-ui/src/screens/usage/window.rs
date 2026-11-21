use crate::ui::calendar::day_at;
use t3_client::time::{epoch_ms, iso_from_epoch_ms};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, usage::*};
use t3_i18n::TimeZone;
const DAY: i64 = 86_400_000;
fn date(index: i64) -> String {
    let d = day_at(index);
    format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
}
pub(super) fn input(days: u32, now: u64, zone: &str) -> UsageSummaryInput {
    let mut name = zone.to_owned();
    let mut zone = TimeZone::Iana(name.clone());
    if !zone.is_known() {
        zone = TimeZone::Utc;
        name = "UTC".into();
    }
    let index = |ms: i64| (ms + i64::from(zone.offset_seconds(ms)) * 1000).div_euclid(DAY);
    let end = if days == 1 {
        now / 60_000 * 60_000
    } else {
        now
    };
    let start = end.saturating_sub(DAY as u64);
    let end_day = index(end as i64);
    let hourly = days == 1;
    UsageSummaryInput {
        since_day: UsageDay(date(if hourly {
            index(start as i64)
        } else {
            end_day - i64::from(days) + 1
        })),
        until_day: UsageDay(date(end_day)),
        time_zone: TrimmedNonEmptyString(name),
        resolution: Some(Some(if hourly {
            UsageResolution::Hour
        } else {
            UsageResolution::Day
        })),
        since_time: hourly.then(|| Some(TrimmedNonEmptyString(iso_from_epoch_ms(start)))),
        until_time: hourly.then(|| Some(TrimmedNonEmptyString(iso_from_epoch_ms(end)))),
        await_refresh: None,
        provisional: None,
        model_filter: None,
    }
}
pub(super) fn periods(window: &UsageSummaryInput) -> Vec<String> {
    if let Some(Some(start)) = &window.since_time {
        let start = epoch_ms(&start.0).unwrap_or_default();
        return (0..24)
            .map(|n| iso_from_epoch_ms((start + n * 3_600_000).max(0) as u64))
            .collect();
    }
    let index = |day: &UsageDay| {
        epoch_ms(&format!("{}T00:00:00Z", day.0))
            .unwrap_or_default()
            .div_euclid(DAY)
    };
    (index(&window.since_day)..=index(&window.until_day))
        .take(90)
        .map(date)
        .collect()
}

pub(super) fn period_label(key: &str, window: &UsageSummaryInput) -> String {
    if let Some(ms) = epoch_ms(key).filter(|_| window.since_time.is_some()) {
        let zone = TimeZone::Iana(window.time_zone.0.clone());
        let local = ms + i64::from(zone.offset_seconds(ms)) * 1000;
        let minutes = local.div_euclid(60_000).rem_euclid(24 * 60);
        format!(
            "{} {:02}:{:02}",
            date(local.div_euclid(DAY)),
            minutes / 60,
            minutes % 60
        )
    } else {
        key.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hourly_window_follows_elapsed_time_across_dst_and_resolves_unknown_zones() {
        let now = epoch_ms("2026-11-01T07:00:37Z").expect("fall back instant") as u64;
        let local = input(1, now, "America/Toronto");
        let slots = periods(&local);
        assert_eq!(slots.len(), 24);
        assert_eq!(period_label(&slots[22], &local), "2026-11-01 01:00");
        assert_eq!(period_label(&slots[23], &local), "2026-11-01 01:00");
        let late = epoch_ms("2026-11-02T01:00:00Z").expect("before local midnight") as u64;
        assert_eq!(input(7, late, "America/Toronto").until_day.0, "2026-11-01");
        let fallback = input(7, late, "Unknown/Zone");
        assert_eq!(fallback.time_zone.0, "UTC");
        assert_eq!(fallback.until_day.0, "2026-11-02");
    }
}
