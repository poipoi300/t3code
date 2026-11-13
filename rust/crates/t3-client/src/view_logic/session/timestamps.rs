//! Locale and time-zone conversion are supplied by the host; all label policy is shared.
use t3_contracts::settings::TimestampFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarTimestamp {
    pub epoch_ms: i64,
    pub day_index: i64,
    pub year: i32,
    pub month: u8,
    pub day: u8,
}
pub trait TimestampHost {
    fn parse_epoch_ms(&self, iso: &str) -> Option<i64>;
    fn calendar(&self, epoch_ms: i64, zone: Option<&str>) -> CalendarTimestamp;
    fn short_time(&self, epoch_ms: i64, format: &TimestampFormat, zone: Option<&str>) -> String;
    fn numeric_date(&self, epoch_ms: i64, include_year: bool, zone: Option<&str>) -> String;
    fn full_timestamp(&self, iso: &str, format: &TimestampFormat) -> String;
    fn locale_hour12(&self) -> bool;
}
pub fn resolve_hour12(format: &TimestampFormat, locale_hour12: bool) -> bool {
    match format {
        TimestampFormat::Locale => locale_hour12,
        TimestampFormat::V12Hour => true,
        _ => false,
    }
}
pub fn resolve_week_starts_on(first_day: Option<u8>) -> Option<u8> {
    first_day.map(|d| d % 7)
}
pub fn ordinal_suffix(day: u8) -> &'static str {
    if (11..=13).contains(&(day % 100)) {
        return "th";
    }
    match day % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}
pub struct TimestampPresenter<H>(pub H);
impl<H: TimestampHost> TimestampPresenter<H> {
    pub fn short(&self, iso: &str, format: &TimestampFormat) -> String {
        self.0
            .parse_epoch_ms(iso)
            .map_or_else(String::new, |ms| self.0.short_time(ms, format, None))
    }
    pub fn tooltip(&self, iso: &str, format: &TimestampFormat) -> String {
        let Some(ms) = self.0.parse_epoch_ms(iso) else {
            return String::new();
        };
        let date = self.0.calendar(ms, None);
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
        let month = MONTHS
            .get(date.month.saturating_sub(1) as usize)
            .copied()
            .unwrap_or_default();
        format!(
            "{}, {}{} {} {}",
            self.0.short_time(ms, format, None),
            date.day,
            ordinal_suffix(date.day),
            month,
            date.year
        )
    }
    pub fn full(&self, iso: &str, format: &TimestampFormat) -> String {
        self.0
            .parse_epoch_ms(iso)
            .map_or_else(String::new, |_| self.0.full_timestamp(iso, format))
    }
    pub fn day_aware(
        &self,
        iso: &str,
        format: &TimestampFormat,
        now_ms: i64,
        zone: Option<&str>,
        upcoming: bool,
    ) -> String {
        let Some(ms) = self.0.parse_epoch_ms(iso) else {
            return String::new();
        };
        let today = self.0.calendar(now_ms, zone);
        let date = self.0.calendar(ms, zone);
        let difference = date.day_index - today.day_index;
        let time = self.0.short_time(ms, format, zone);
        if upcoming && difference == 1 {
            return format!("tomorrow at {time}");
        }
        if difference == 0 || (!upcoming && difference > 0) {
            return time;
        }
        if difference == -1 {
            return format!("yesterday at {time}");
        }
        format!(
            "{} {time}",
            self.0.numeric_date(ms, date.year != today.year, zone)
        )
    }
    pub fn relative(&self, iso: &str, now_ms: i64) -> Option<RelativeTimeParts> {
        self.0
            .parse_epoch_ms(iso)
            .map(|ms| format_relative_elapsed(relative_time_elapsed_ms(ms, now_ms)))
    }
    pub fn relative_label(&self, iso: &str, now_ms: i64) -> String {
        self.relative(iso, now_ms).map_or_else(String::new, |p| {
            p.suffix
                .map_or(p.value.clone(), |s| format!("{} {s}", p.value))
        })
    }
    pub fn elapsed_label(&self, iso: &str, now_ms: i64) -> String {
        self.0
            .parse_epoch_ms(iso)
            .map_or_else(String::new, |ms| format_elapsed_duration_label(ms, now_ms))
    }
    pub fn expires_label(&self, iso: &str, now_ms: i64) -> String {
        self.0
            .parse_epoch_ms(iso)
            .map_or_else(String::new, |ms| format_expires_in_label(ms, now_ms))
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeTimeParts {
    pub value: String,
    pub suffix: Option<&'static str>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelativeTimeState {
    Missing,
    Invalid,
    Relative(RelativeTimeParts),
}
fn unit_ms(elapsed: i64) -> i64 {
    if elapsed < 3_600_000 {
        60_000
    } else if elapsed < 86_400_000 {
        3_600_000
    } else {
        86_400_000
    }
}
pub fn relative_time_elapsed_ms(date_ms: i64, now_ms: i64) -> i64 {
    let elapsed = now_ms.saturating_sub(date_ms).max(0);
    let unit = unit_ms(elapsed);
    elapsed / unit * unit
}
pub fn ms_until_relative_time_change(date_ms: i64, now_ms: i64) -> i64 {
    let elapsed = now_ms.saturating_sub(date_ms).max(0);
    let unit = unit_ms(elapsed);
    date_ms
        .saturating_add((elapsed / unit + 1).saturating_mul(unit))
        .saturating_sub(now_ms)
        .min(86_400_000)
}
pub fn format_relative_elapsed(elapsed_ms: i64) -> RelativeTimeParts {
    let seconds = elapsed_ms / 1000;
    let value = if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    };
    RelativeTimeParts {
        value,
        suffix: (seconds >= 60).then_some("ago"),
    }
}
pub fn format_elapsed_duration_label(date_ms: i64, now_ms: i64) -> String {
    let seconds = now_ms.saturating_sub(date_ms) / 1000;
    if seconds < 5 {
        "just now".into()
    } else if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}
pub fn format_expires_in_label(date_ms: i64, now_ms: i64) -> String {
    let diff = date_ms.saturating_sub(now_ms);
    if diff <= 0 {
        return "Expired".into();
    }
    let seconds = diff / 1000;
    if seconds < 5 {
        return "Expires in a moment".into();
    }
    let mut parts = vec![];
    for (unit, label) in [(86400, "d"), (3600, "h"), (60, "m"), (1, "s")] {
        let count = seconds / unit
            % if unit == 86400 {
                i64::MAX
            } else if unit == 3600 {
                24
            } else {
                60
            };
        if count > 0 {
            parts.push(format!("{count}{label}"));
        }
    }
    format!("Expires in {}", parts.join(" "))
}
pub fn format_duration(ms: f64) -> String {
    if !ms.is_finite() || ms < 0. {
        return "0ms".into();
    }
    if ms < 1000. {
        return format!("{}ms", ms.round().max(1.));
    }
    if ms < 10000. {
        return format!("{}s", (ms / 100.).round() / 10.);
    }
    let seconds = (ms / 1000.).round() as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let (h, m, s) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    let mut parts = vec![];
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if m > 0 {
        parts.push(format!("{m}m"));
    }
    if s > 0 || m > 0 {
        parts.push(format!("{s}s"));
    }
    parts.join(" ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_boundaries_and_future_timer_cap() {
        for (ms, label) in [
            (0, "just now"),
            (59000, "just now"),
            (60000, "1m"),
            (3600000, "1h"),
            (86400000, "1d"),
        ] {
            assert_eq!(format_relative_elapsed(ms).value, label);
        }
        assert_eq!(relative_time_elapsed_ms(0, 119999), 60000);
        assert_eq!(ms_until_relative_time_change(0, 119999), 1);
        assert_eq!(ms_until_relative_time_change(1_000_000_000, 0), 86_400_000);
        assert_eq!(format_expires_in_label(3_661_000, 0), "Expires in 1h 1m 1s");
        assert_eq!(format_expires_in_label(86_400_000, 0), "Expires in 1d");
        assert_eq!(format_elapsed_duration_label(5, 0), "just now");
        assert_eq!(format_duration(59_500.), "1m 0s");
        assert_eq!(format_duration(f64::NAN), "0ms");
    }
    #[test]
    fn timezone_day_policy_uses_host_calendar() {
        struct Host;
        impl TimestampHost for Host {
            fn parse_epoch_ms(&self, iso: &str) -> Option<i64> {
                match iso {
                    "2026-07-02T00:30:00.000Z" => Some(1),
                    "2026-07-02T05:30:00.000Z" => Some(2),
                    "2026-07-01T03:30:00.000Z" => Some(3),
                    _ => None,
                }
            }
            fn calendar(&self, ms: i64, zone: Option<&str>) -> CalendarTimestamp {
                assert_eq!(zone, Some("America/New_York"));
                CalendarTimestamp {
                    epoch_ms: ms,
                    day_index: match ms {
                        2 => 2,
                        3 => 0,
                        _ => 1,
                    },
                    year: 2026,
                    month: 7,
                    day: 1,
                }
            }
            fn short_time(&self, _: i64, _: &TimestampFormat, _: Option<&str>) -> String {
                "8:30 PM".into()
            }
            fn numeric_date(&self, _: i64, _: bool, _: Option<&str>) -> String {
                "7/3".into()
            }
            fn full_timestamp(&self, _: &str, _: &TimestampFormat) -> String {
                String::new()
            }
            fn locale_hour12(&self) -> bool {
                true
            }
        }
        let p = TimestampPresenter(Host);
        let zone = Some("America/New_York");
        let format = TimestampFormat::Locale;
        assert!(
            !p.day_aware("2026-07-02T00:30:00.000Z", &format, 0, zone, true)
                .contains("tomorrow")
        );
        assert!(
            p.day_aware("2026-07-02T05:30:00.000Z", &format, 0, zone, true)
                .starts_with("tomorrow at ")
        );
        assert!(
            p.day_aware("2026-07-01T03:30:00.000Z", &format, 0, zone, false)
                .starts_with("yesterday at ")
        );
    }
}
