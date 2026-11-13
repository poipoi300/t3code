//! Output parity with the real `Intl`: every fixture case is a call the runtime's `Intl`
//! answered (see `tools/generate-fixtures.mjs`). Cases that differ are grouped; the ones the
//! crate knowingly does not reproduce are listed with their reason in `KNOWN_DIFFERENCES`, and
//! any other difference fails the test.

use std::collections::BTreeMap;

use serde::Deserialize;
use t3_i18n::format::{DateFormat, Formats, ListKind, NumberOptions, TimeZone};
use t3_i18n::message::PluralKind;

#[derive(Deserialize)]
struct File<T> {
    cases: Vec<T>,
}

fn load<T: for<'de> Deserialize<'de>>(json: &str) -> Vec<T> {
    serde_json::from_str::<File<T>>(json)
        .expect("fixture")
        .cases
}

/// One mismatch group: (category, format, locale) and the reason it is accepted. `*` matches any.
const KNOWN_DIFFERENCES: &[(&str, &str, &str, &str)] = &[
    (
        "date",
        "default_date_time/hour12=true",
        "ja",
        "V8 answers toLocaleString({ hour12: true }) with a 0-11 hour (午前0:05) only while no          Intl.DateTimeFormat for ja has resolved a 1-12 hour in the process; afterwards it gives          午前12:05, as Intl.DateTimeFormat with explicit fields always does. Rust follows the          latter, so the fixture's midnight and noon rows differ.",
    ),
    (
        "date",
        "default_time/hour12=true",
        "ja",
        "Same V8 call-history quirk as default_date_time/hour12=true.",
    ),
];

fn known(category: &str, format: &str, locale: &str) -> bool {
    KNOWN_DIFFERENCES.iter().any(|(c, f, l, _)| {
        *c == category && (*f == "*" || *f == format) && (*l == "*" || *l == locale)
    })
}

/// (category, format, locale)
type Group = (String, String, String);
/// (input, expected, actual)
type Mismatch = (String, String, String);

#[derive(Default)]
struct Report {
    total: usize,
    failures: BTreeMap<Group, Vec<Mismatch>>,
}

impl Report {
    fn check(
        &mut self,
        category: &str,
        format: &str,
        locale: &str,
        input: String,
        expected: &str,
        actual: String,
    ) {
        self.total += 1;
        if expected != actual {
            self.failures
                .entry((category.to_owned(), format.to_owned(), locale.to_owned()))
                .or_default()
                .push((input, expected.to_owned(), actual));
        }
    }

    fn assert_clean(self, category: &str) {
        let unexpected: Vec<_> = self
            .failures
            .iter()
            .filter(|((c, f, l), _)| !known(c, f, l))
            .collect();
        if std::env::var_os("T3_I18N_REPORT").is_some() {
            for ((c, f, l), cases) in &self.failures {
                eprintln!("[{c}] {f} {l}: {} differ of {}", cases.len(), self.total);
                for (input, expected, actual) in cases.iter().take(3) {
                    eprintln!("    {input}: expected {expected:?}, got {actual:?}");
                }
            }
        }
        assert!(
            unexpected.is_empty(),
            "{category}: {} groups differ from Intl, e.g. {:?}",
            unexpected.len(),
            unexpected.first().map(|(key, cases)| (key, cases.first()))
        );
    }
}

#[derive(Deserialize)]
struct NumberCase {
    locale: String,
    format: String,
    value: f64,
    expected: String,
}

#[test]
fn numbers_match_intl() {
    let mut report = Report::default();
    for case in load::<NumberCase>(include_str!("fixtures/numbers.json")) {
        let formats = Formats::new(&case.locale);
        let actual = match case.format.as_str() {
            "decimal" => formats.decimal(case.value),
            "no_grouping" => formats.number(
                case.value,
                NumberOptions {
                    grouping: false,
                    ..NumberOptions::default()
                },
            ),
            "max_fraction_1" => formats.number(
                case.value,
                NumberOptions {
                    max_fraction: 1,
                    ..NumberOptions::default()
                },
            ),
            "integer" => formats.number(
                case.value,
                NumberOptions {
                    max_fraction: 0,
                    ..NumberOptions::default()
                },
            ),
            "compact_1" => formats.compact(case.value, 1),
            "usd_2" => formats.currency(case.value, "USD"),
            "percent_0" => formats.percent(case.value, 0),
            other => panic!("unknown number format {other}"),
        };
        report.check(
            "number",
            &case.format,
            &case.locale,
            case.value.to_string(),
            &case.expected,
            actual,
        );
    }
    report.assert_clean("numbers");
}

#[derive(Deserialize)]
struct PluralCase {
    locale: String,
    r#type: String,
    value: f64,
    expected: String,
}

#[test]
fn plural_categories_match_intl() {
    let mut report = Report::default();
    for case in load::<PluralCase>(include_str!("fixtures/plurals.json")) {
        let kind = if case.r#type == "cardinal" {
            PluralKind::Cardinal
        } else {
            PluralKind::Ordinal
        };
        let actual = Formats::new(&case.locale).plural_category(kind, case.value);
        report.check(
            "plural",
            &case.r#type,
            &case.locale,
            case.value.to_string(),
            &case.expected,
            actual.to_owned(),
        );
    }
    report.assert_clean("plurals");
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
    // YYYY-MM-DDTHH:MM:SSZ
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

fn date_format(name: &str) -> DateFormat {
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

#[test]
#[cfg(feature = "iana")]
fn dates_match_intl() {
    let mut report = Report::default();
    for case in load::<DateCase>(include_str!("fixtures/dates.json")) {
        let formats = Formats::new(&case.locale);
        let zone = if case.zone == "UTC" {
            TimeZone::Utc
        } else {
            TimeZone::Iana(case.zone.clone())
        };
        let actual = formats.date(
            date_format(&case.format),
            parse_instant(&case.instant),
            &zone,
            case.hour12,
        );
        let format = match case.hour12 {
            None => case.format.clone(),
            Some(twelve) => format!("{}/hour12={twelve}", case.format),
        };
        report.check(
            "date",
            &format,
            &case.locale,
            format!("{} {}", case.instant, case.zone),
            &case.expected,
            actual,
        );
    }
    report.assert_clean("dates");
}

#[derive(Deserialize)]
struct WeekCase {
    locale: String,
    #[serde(rename = "firstDay")]
    first_day: u8,
}

#[test]
fn week_start_matches_intl() {
    let mut report = Report::default();
    for case in load::<WeekCase>(include_str!("fixtures/weeks.json")) {
        let actual = Formats::new(&case.locale).week_starts_on();
        report.check(
            "week",
            "first_day",
            &case.locale,
            case.locale.clone(),
            &(case.first_day % 7).to_string(),
            actual.to_string(),
        );
    }
    report.assert_clean("weeks");
}

#[derive(Deserialize)]
struct ListCase {
    locale: String,
    r#type: String,
    items: Vec<String>,
    expected: String,
}

#[test]
fn lists_match_intl() {
    let mut report = Report::default();
    for case in load::<ListCase>(include_str!("fixtures/lists.json")) {
        let kind = if case.r#type == "conjunction" {
            ListKind::Conjunction
        } else {
            ListKind::Disjunction
        };
        let items: Vec<&str> = case.items.iter().map(String::as_str).collect();
        let actual = Formats::new(&case.locale).list(&items, kind);
        report.check(
            "list",
            &case.r#type,
            &case.locale,
            case.items.join("|"),
            &case.expected,
            actual,
        );
    }
    report.assert_clean("lists");
}
