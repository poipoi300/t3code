//! How close ICU4X alone gets to `Intl`, which is what a locale without a generated pattern
//! table gets. This is the measurement behind the library decision in the crate docs; run with
//! `cargo test -p t3-i18n --test icu4x_alone -- --nocapture` to print it. The assertions are
//! floors, so a data update that makes ICU4X worse is noticed.

#![cfg(feature = "icu-fallback")]

use std::collections::BTreeMap;

use serde::Deserialize;
use t3_i18n::{DateFormat, Formats, TimeZone};

#[derive(Deserialize)]
struct File<T> {
    cases: Vec<T>,
}

#[derive(Deserialize)]
struct DateCase {
    locale: String,
    format: String,
    zone: String,
    hour12: Option<bool>,
    instant: String,
    expected: String,
}

fn parse_instant(iso: &str) -> i64 {
    let n = |range: std::ops::Range<usize>| iso[range].parse::<i64>().expect("digits");
    let (year, month, day) = (n(0..4), n(5..7), n(8..10));
    let (hour, minute, second) = (n(11..13), n(14..16), n(17..19));
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000
}

fn format_of(name: &str) -> DateFormat {
    match name {
        "time_hm" => DateFormat::TimeHm,
        "time_hms" => DateFormat::TimeHms,
        "month_day_numeric" => DateFormat::MonthDayNumeric,
        "month_day_year_numeric" => DateFormat::MonthDayYearNumeric,
        "date_medium" => DateFormat::DateMedium,
        "date_medium_time_short" => DateFormat::DateMediumTimeShort,
        "month_short_day" => DateFormat::MonthShortDay,
        "month_short_day_year" => DateFormat::MonthShortDayYear,
        "month_short_day_time" => DateFormat::MonthShortDayTime,
        "weekday_long" => DateFormat::WeekdayLong,
        "weekday_short" => DateFormat::WeekdayShort,
        "month_short" => DateFormat::MonthShort,
        "month_long" => DateFormat::MonthLong,
        "default_date_time" => DateFormat::DefaultDateTime,
        "default_date" => DateFormat::DefaultDate,
        "default_time" => DateFormat::DefaultTime,
        other => panic!("unknown date format {other}"),
    }
}

/// `(matching, total)` per format and overall for the ICU4X-only path.
fn score(json: &str) -> (usize, usize, BTreeMap<String, (usize, usize)>) {
    let cases: Vec<DateCase> = serde_json::from_str::<File<DateCase>>(json)
        .expect("fixture")
        .cases;
    let mut per_format: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let (mut ok, mut total) = (0, 0);
    for case in &cases {
        let zone = if case.zone == "UTC" {
            TimeZone::Utc
        } else {
            TimeZone::Iana(case.zone.clone())
        };
        let actual = Formats::new(&case.locale).date_icu4x(
            format_of(&case.format),
            parse_instant(&case.instant),
            &zone,
            case.hour12,
        );
        let entry = per_format.entry(case.format.clone()).or_default();
        entry.1 += 1;
        total += 1;
        if actual == case.expected {
            entry.0 += 1;
            ok += 1;
        }
    }
    (ok, total, per_format)
}

#[test]
fn icu4x_alone_dates() {
    for (label, json, floor) in [
        ("shipped locales", include_str!("fixtures/dates.json"), 0.40),
        (
            "other locales",
            include_str!("fixtures/dates-other.json"),
            0.40,
        ),
    ] {
        let (ok, total, per_format) = score(json);
        eprintln!(
            "ICU4X alone, dates, {label}: {ok}/{total} = {:.1}%",
            100.0 * ok as f64 / total as f64
        );
        for (format, (ok, total)) in &per_format {
            eprintln!(
                "    {format:<26} {ok:>5}/{total:<5} {:>5.1}%",
                100.0 * *ok as f64 / *total as f64
            );
        }
        assert!(ok as f64 / total as f64 >= floor, "{label}");
    }
}

#[test]
fn icu4x_alone_compact_numbers() {
    #[derive(Deserialize)]
    struct NumberCase {
        locale: String,
        format: String,
        value: f64,
        expected: String,
    }
    let cases: Vec<NumberCase> =
        serde_json::from_str::<File<NumberCase>>(include_str!("fixtures/numbers.json"))
            .expect("fixture")
            .cases;
    let (mut ok, mut total) = (0, 0);
    for case in cases.iter().filter(|case| case.format == "compact_1") {
        total += 1;
        ok += usize::from(Formats::new(&case.locale).compact_icu4x(case.value) == case.expected);
    }
    eprintln!(
        "ICU4X alone, compact (maximumFractionDigits 1): {ok}/{total} = {:.1}%",
        100.0 * ok as f64 / total as f64
    );
    assert!(ok as f64 / total as f64 >= 0.60);
}
