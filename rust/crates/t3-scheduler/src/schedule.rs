use chrono::{DateTime, Datelike, Days, NaiveDateTime, TimeDelta, Timelike, Utc};
use chrono_tz::Tz;
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, LazyLock, Mutex},
};
use t3_contracts::scheduled_task::{
    MIN_SCHEDULED_TASK_INTERVAL_MS, ScheduledTaskSchedule as Schedule,
};

pub(crate) fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    let with_seconds;
    let value = if value
        .as_bytes()
        .get(16)
        .is_some_and(|b| matches!(b, b'Z' | b'+' | b'-'))
    {
        with_seconds = format!("{}:00{}", &value[..16], &value[16..]);
        with_seconds.as_str()
    } else {
        value
    };
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f%#z"))
        .ok()
        .and_then(|d| DateTime::from_timestamp_millis(d.timestamp_millis()))
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc())
        })
}

pub(crate) fn iso(value: DateTime<Utc>) -> String {
    if (0..=9999).contains(&value.year()) {
        return value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    }
    format!(
        "{}{:06}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        if value.year() < 0 { '-' } else { '+' },
        value.year().unsigned_abs(),
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.timestamp_subsec_millis()
    )
}

fn time_of_day(value: &str) -> Option<(u32, u32)> {
    let (h, m) = value.trim().split_once(':')?;
    if !(1..=2).contains(&h.len())
        || m.len() != 2
        || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let h = h.parse().ok()?;
    let m = m.parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

static ZONES: LazyLock<Mutex<HashMap<Tz, Arc<tz::TimeZone>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn zone_rules(zone: Tz) -> Option<Arc<tz::TimeZone>> {
    let mut zones = ZONES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(zone) = zones.get(&zone) {
        return Some(zone.clone());
    }
    let (_, data) = jiff_tzdb::get(zone.name())?;
    let rules = Arc::new(tz::TimeZone::from_tz_data(data).ok()?);
    zones.insert(zone, rules.clone());
    Some(rules)
}

fn local_time(zone: &tz::TimeZone, value: DateTime<Utc>) -> Option<NaiveDateTime> {
    let offset = zone
        .find_local_time_type(value.timestamp())
        .ok()?
        .ut_offset();
    value
        .naive_utc()
        .checked_add_signed(TimeDelta::seconds(offset.into()))
}

// Effect DateTime's compatible disambiguation chooses the earlier fold and shifts gaps forward.
fn compatible(zone: &tz::TimeZone, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    let found = tz::DateTime::find(
        local.year(),
        local.month() as u8,
        local.day() as u8,
        local.hour() as u8,
        local.minute() as u8,
        local.second() as u8,
        local.nanosecond(),
        zone.as_ref(),
    )
    .ok()?;
    let offset = found.earliest()?.local_time_type().ut_offset();
    local
        .checked_sub_signed(TimeDelta::seconds(offset.into()))
        .map(|d| d.and_utc())
}

pub fn next_scheduled_run_at(
    schedule: &Schedule,
    from: DateTime<Utc>,
    zone: Tz,
) -> Option<DateTime<Utc>> {
    let from = DateTime::from_timestamp_millis(from.timestamp_millis())?;
    match schedule {
        Schedule::Webhook(_) => None,
        Schedule::Once(s) => parse_date(&s.run_at.0).filter(|d| *d > from),
        Schedule::Interval(s) => from.checked_add_signed(TimeDelta::milliseconds(
            s.every_ms.max(MIN_SCHEDULED_TASK_INTERVAL_MS),
        )),
        Schedule::FixedTime(s) => {
            let zone = zone_rules(zone)?;
            let (hour, minute) = time_of_day(&s.time_of_day.0)?;
            let weekdays = s.weekdays.as_ref().and_then(Option::as_ref);
            let local = local_time(&zone, from)?;
            for offset in 0..=7 {
                // DateTime.add resolves a calendar day before DateTime.setParts resolves its time.
                let added = compatible(&zone, local.checked_add_days(Days::new(offset))?)?;
                let date = local_time(&zone, added)?.date();
                let candidate = compatible(&zone, date.and_hms_opt(hour, minute, 0)?)?;
                if candidate <= from {
                    continue;
                }
                let weekday = i64::from(
                    local_time(&zone, candidate)?
                        .weekday()
                        .num_days_from_sunday(),
                );
                if weekdays.is_some_and(|days| !days.is_empty() && !days.contains(&weekday)) {
                    continue;
                }
                return Some(candidate);
            }
            None
        }
    }
}

fn weekday_key(days: &Option<Option<Vec<i64>>>) -> BTreeSet<i64> {
    let days: BTreeSet<_> = days
        .as_ref()
        .and_then(Option::as_ref)
        .into_iter()
        .flatten()
        .copied()
        .collect();
    if days.is_empty() || days.len() == 7 {
        BTreeSet::new()
    } else {
        days
    }
}

pub fn is_same_schedule(a: &Schedule, b: &Schedule) -> bool {
    match (a, b) {
        (Schedule::Interval(a), Schedule::Interval(b)) => a.every_ms == b.every_ms,
        (Schedule::Webhook(_), Schedule::Webhook(_)) => true,
        (Schedule::Once(a), Schedule::Once(b)) => parse_date(&a.run_at.0)
            .zip(parse_date(&b.run_at.0))
            .is_some_and(|(a, b)| a == b),
        (Schedule::FixedTime(a), Schedule::FixedTime(b)) => {
            time_of_day(&a.time_of_day.0)
                .zip(time_of_day(&b.time_of_day.0))
                .is_some_and(|(a, b)| a == b)
                && weekday_key(&a.weekdays) == weekday_key(&b.weekdays)
        }
        _ => false,
    }
}

pub fn is_missed_fixed_time_run(
    schedule: &Schedule,
    due: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    matches!(schedule, Schedule::FixedTime(_))
        && now.signed_duration_since(due) > TimeDelta::minutes(10)
}
