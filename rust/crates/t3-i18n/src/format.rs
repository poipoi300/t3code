//! Locale-aware numbers, dates, times, plurals and lists, on ICU4X (CLDR data compiled in).
//!
//! The product formats with `Intl` in a handful of shapes (see [`DateFormat`] and the number
//! methods); each is one method here, checked against the real `Intl` by the fixtures in
//! `tests/fixtures`. The current time, the locale and the time zone are always arguments.

use fixed_decimal::{Decimal, FloatPrecision, SignedRoundingMode, UnsignedRoundingMode};
#[cfg(feature = "icu-fallback")]
use icu::calendar::Date;
use icu::calendar::week::WeekInformation;
#[cfg(feature = "icu-fallback")]
use icu::datetime::DateTimeFormatter;
#[cfg(feature = "icu-fallback")]
use icu::datetime::DateTimeFormatterPreferences;
#[cfg(feature = "icu-fallback")]
use icu::datetime::fieldsets;
#[cfg(feature = "icu-fallback")]
use icu::datetime::options::TimePrecision;
#[cfg(feature = "icu-fallback")]
use icu::decimal::CompactDecimalFormatter;
use icu::decimal::DecimalFormatter;
use icu::decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use icu::experimental::dimension::currency::formatter::CurrencyFormatter;
use icu::experimental::dimension::percent::formatter::PercentFormatter;
use icu::list::ListFormatter;
use icu::list::options::{ListFormatterOptions, ListLength};
use icu::locale::Locale;
#[cfg(feature = "icu-fallback")]
use icu::locale::preferences::extensions::unicode::keywords::HourCycle;
use icu::locale::preferences::extensions::unicode::keywords::currency;
use icu::plurals::{PluralCategory, PluralOperands, PluralRules};
#[cfg(feature = "icu-fallback")]
use icu::time::{DateTime, Time};

use crate::intl_pattern::{self, Clock};
use crate::message::{PluralKind, Runtime};

/// How `Intl.DateTimeFormat` options are named here. Each variant is one option bag the web or
/// mobile app passes today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateFormat {
    /// `{ hour: "numeric", minute: "2-digit" }`
    TimeHm,
    /// `{ hour: "numeric", minute: "2-digit", second: "2-digit" }`
    TimeHms,
    /// `{ month: "numeric", day: "numeric" }`
    MonthDayNumeric,
    /// `{ month: "numeric", day: "numeric", year: "numeric" }`
    MonthDayYearNumeric,
    /// `{ dateStyle: "medium" }`
    DateMedium,
    /// `{ dateStyle: "medium", timeStyle: "short" }`
    DateMediumTimeShort,
    /// `{ month: "short", day: "numeric" }`
    MonthShortDay,
    /// `{ month: "short", day: "numeric", year: "numeric" }`
    MonthShortDayYear,
    /// `{ month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }`
    MonthShortDayTime,
    /// `{ weekday: "long" }`
    WeekdayLong,
    /// `{ weekday: "short" }`
    WeekdayShort,
    /// `{ month: "short" }`
    MonthShort,
    /// `{ month: "long" }`
    MonthLong,
    /// `toLocaleString()`
    DefaultDateTime,
    /// `toLocaleDateString()`
    DefaultDate,
    /// `toLocaleTimeString()`
    DefaultTime,
}

/// The user's time zone: `Intl.DateTimeFormat().resolvedOptions().timeZone`, or a fixed offset.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TimeZone {
    Utc,
    /// Seconds east of UTC.
    Fixed(i32),
    /// An IANA name such as `America/New_York`. An unknown name formats as UTC.
    Iana(String),
}

impl TimeZone {
    /// Seconds east of UTC at `epoch_ms`.
    pub fn offset_seconds(&self, epoch_ms: i64) -> i32 {
        match self {
            Self::Utc => 0,
            Self::Fixed(seconds) => *seconds,
            Self::Iana(name) => iana_offset_seconds(name, epoch_ms).unwrap_or(0),
        }
    }

    /// Whether the IANA name resolves.
    pub fn is_known(&self) -> bool {
        match self {
            Self::Iana(name) => iana_offset_seconds(name, 0).is_some(),
            _ => true,
        }
    }
}

#[cfg(feature = "iana")]
fn iana_offset_seconds(name: &str, epoch_ms: i64) -> Option<i32> {
    let zone = jiff::tz::TimeZone::get(name).ok()?;
    let timestamp = jiff::Timestamp::from_millisecond(epoch_ms).ok()?;
    Some(zone.to_offset(timestamp).seconds())
}

#[cfg(not(feature = "iana"))]
fn iana_offset_seconds(_name: &str, _epoch_ms: i64) -> Option<i32> {
    None
}

/// A calendar day in a time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CalendarDay {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    /// Days since 1970-01-01, so differences are whole days across DST shifts.
    pub index: i64,
    /// 0 is Sunday, as `Date#getDay`.
    pub weekday: u8,
}

/// The local civil fields of an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub day: CalendarDay,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub fn civil(epoch_ms: i64, zone: &TimeZone) -> Civil {
    let local_ms = epoch_ms + i64::from(zone.offset_seconds(epoch_ms)) * 1000;
    let index = local_ms.div_euclid(86_400_000);
    let ms_of_day = local_ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(index);
    Civil {
        day: CalendarDay {
            year,
            month,
            day,
            index,
            weekday: (index + 4).rem_euclid(7) as u8,
        },
        hour: (ms_of_day / 3_600_000) as u8,
        minute: (ms_of_day / 60_000 % 60) as u8,
        second: (ms_of_day / 1000 % 60) as u8,
    }
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    ((year + i64::from(month <= 2)) as i32, month, day)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListKind {
    Conjunction,
    Disjunction,
}

/// Formatters for one locale. Build one per active locale and share it.
pub struct Formats {
    tag: String,
    locale: Locale,
}

fn intl_decimal(value: f64) -> Option<Decimal> {
    Decimal::try_from_f64(value, FloatPrecision::RoundTrip).ok()
}

/// `Intl.NumberFormat` rounds half away from zero on the shortest decimal form of the double.
fn round_half_expand(decimal: &mut Decimal, max_fraction: i16) {
    decimal.round_with_mode(
        -max_fraction,
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    );
}

impl Formats {
    /// `tag` is a BCP 47 locale such as `en-US`. An invalid tag falls back to `en-US`, as the
    /// web app falls back to the host default.
    pub fn new(tag: &str) -> Self {
        let locale = tag
            .parse::<Locale>()
            .unwrap_or(icu::locale::locale!("en-US"));
        Self {
            tag: locale.to_string(),
            locale,
        }
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }

    // ---- numbers --------------------------------------------------------------------------

    fn decimal_formatter(&self, grouping: GroupingStrategy) -> Option<DecimalFormatter> {
        let mut options = DecimalFormatterOptions::default();
        options.grouping_strategy = Some(grouping);
        DecimalFormatter::try_new((&self.locale).into(), options).ok()
    }

    /// `new Intl.NumberFormat(locale, { minimumFractionDigits, maximumFractionDigits,
    /// useGrouping }).format(value)`.
    pub fn number(&self, value: f64, options: NumberOptions) -> String {
        let Some(mut decimal) = intl_decimal(value) else {
            return non_finite(value);
        };
        round_half_expand(&mut decimal, options.max_fraction as i16);
        decimal.trim_end();
        decimal.pad_end(-(options.min_fraction as i16));
        match self.decimal_formatter(if options.grouping {
            GroupingStrategy::Auto
        } else {
            GroupingStrategy::Never
        }) {
            Some(formatter) => formatter.format(&decimal).to_string(),
            None => decimal.to_string(),
        }
    }

    /// `new Intl.NumberFormat(locale).format(value)`.
    pub fn decimal(&self, value: f64) -> String {
        self.number(value, NumberOptions::default())
    }

    /// `{ notation: "compact", maximumFractionDigits }`, short form. The shipped locales use the
    /// units `Intl` reports; other locales use ICU4X's compact formatter, which rounds to a whole
    /// number from two integer digits up regardless of `max_fraction`.
    pub fn compact(&self, value: f64, max_fraction: u8) -> String {
        let Some(decimal) = intl_decimal(value) else {
            return non_finite(value);
        };
        if let Some(table) = intl_pattern::table_locale(&self.tag).and_then(intl_pattern::compact) {
            return self.compact_from_units(decimal, max_fraction, table.units);
        }
        #[cfg(feature = "icu-fallback")]
        {
            self.compact_icu4x_decimal(decimal)
        }
        #[cfg(not(feature = "icu-fallback"))]
        {
            match intl_pattern::compact("en-US") {
                Some(table) => self.compact_from_units(decimal, max_fraction, table.units),
                None => decimal.to_string(),
            }
        }
    }

    /// [`Formats::compact`] through ICU4X alone: it rounds to a whole number from two integer
    /// digits up and to one decimal below, whatever `maximumFractionDigits` says.
    #[cfg(feature = "icu-fallback")]
    pub fn compact_icu4x(&self, value: f64) -> String {
        match intl_decimal(value) {
            Some(decimal) => self.compact_icu4x_decimal(decimal),
            None => non_finite(value),
        }
    }

    #[cfg(feature = "icu-fallback")]
    fn compact_icu4x_decimal(&self, mut decimal: Decimal) -> String {
        decimal.trim_end();
        match CompactDecimalFormatter::try_new_short((&self.locale).into(), Default::default()) {
            Ok(formatter) => {
                // ICU 2.1 rounded subunit amounts to a whole number in the compact fallback.
                if decimal.absolute.nonzero_magnitude_start() < 0 {
                    decimal.round(0);
                }
                formatter.format(&decimal).to_string()
            }
            Err(_) => decimal.to_string(),
        }
    }

    fn compact_from_units(
        &self,
        decimal: Decimal,
        max_fraction: u8,
        units: &[(i16, i16, &str)],
    ) -> String {
        // `units` holds, per magnitude `k` that has a suffix, the power of ten the value is
        // divided by and the suffix: Spanish shows 1e9 as "1000 M", not "1 mil M".
        let mut magnitude = if decimal.is_zero() {
            0
        } else {
            decimal.absolute.nonzero_magnitude_start()
        };
        loop {
            let (exponent, suffix) = units
                .iter()
                .rev()
                .find(|(k, _, _)| *k <= magnitude)
                .map_or((0, ""), |(_, exponent, suffix)| (*exponent, *suffix));
            let mut scaled = decimal.clone();
            scaled.multiply_pow10(-exponent);
            round_half_expand(&mut scaled, i16::from(max_fraction));
            let rounded = if scaled.is_zero() {
                0
            } else {
                scaled.absolute.nonzero_magnitude_start() + exponent
            };
            if rounded > magnitude {
                magnitude = rounded;
                continue;
            }
            scaled.trim_start();
            scaled.trim_end();
            let digits = match self.decimal_formatter(GroupingStrategy::Min2) {
                Some(formatter) => formatter.format(&scaled).to_string(),
                None => scaled.to_string(),
            };
            return format!("{digits}{suffix}");
        }
    }

    /// `{ style: "currency", currency, minimumFractionDigits: 2, maximumFractionDigits: 2 }`.
    pub fn currency(&self, value: f64, code: &str) -> String {
        let Some(mut decimal) = intl_decimal(value) else {
            return non_finite(value);
        };
        round_half_expand(&mut decimal, 2);
        decimal.trim_end();
        decimal.pad_end(-2);
        let code = match code.to_ascii_uppercase().as_str() {
            "USD" => currency!("usd"),
            "EUR" => currency!("eur"),
            _ => return decimal.to_string(),
        };
        // ICU4X puts the minus after the symbol (`$-1.00`); every shipped locale writes it first.
        let negative = decimal.sign() == fixed_decimal::Sign::Negative;
        let mut magnitude = decimal.clone();
        magnitude.set_sign(fixed_decimal::Sign::None);
        match CurrencyFormatter::try_new_symbol((&self.locale).into(), code, Default::default()) {
            Ok(formatter) => {
                let text = formatter.format_fixed_decimal(&magnitude).to_string();
                if negative { format!("-{text}") } else { text }
            }
            Err(_) => decimal.to_string(),
        }
    }

    /// `{ style: "percent", maximumFractionDigits }` of a fraction (0.5 is 50%).
    pub fn percent(&self, fraction: f64, max_fraction: u8) -> String {
        let Some(mut decimal) = intl_decimal(fraction) else {
            return non_finite(fraction);
        };
        decimal.multiply_pow10(2);
        round_half_expand(&mut decimal, max_fraction as i16);
        decimal.trim_start();
        decimal.trim_end();
        match PercentFormatter::try_new((&self.locale).into(), Default::default()) {
            Ok(formatter) => formatter.format(&decimal).to_string(),
            Err(_) => decimal.to_string(),
        }
    }

    // ---- plurals --------------------------------------------------------------------------

    /// `new Intl.PluralRules(locale, { type }).select(value)`.
    pub fn plural_category(&self, kind: PluralKind, value: f64) -> &'static str {
        // The reference runtime uses CLDR 48, in which every Spanish ordinal is "other".
        // CLDR 49 added an ordinal "one" rule for Spanish.
        if kind == PluralKind::Ordinal && self.locale.id.language.as_str() == "es" {
            return "other";
        }
        let prefs = (&self.locale).into();
        let rules = match kind {
            PluralKind::Cardinal => PluralRules::try_new_cardinal(prefs),
            PluralKind::Ordinal => PluralRules::try_new_ordinal(prefs),
        };
        let Ok(rules) = rules else { return "other" };
        let operands = if value.is_finite() {
            intl_decimal(value).map(|decimal| PluralOperands::from(&decimal))
        } else {
            None
        };
        match operands.map(|operands| rules.category_for(operands)) {
            Some(PluralCategory::Zero) => "zero",
            Some(PluralCategory::One) => "one",
            Some(PluralCategory::Two) => "two",
            Some(PluralCategory::Few) => "few",
            Some(PluralCategory::Many) => "many",
            Some(PluralCategory::Other) | None => "other",
        }
    }

    // ---- dates ----------------------------------------------------------------------------

    /// Format an instant in `zone`. `hour12` forces a 12 or 24 hour clock for the formats that
    /// show an hour; `None` follows the locale.
    pub fn date(
        &self,
        format: DateFormat,
        epoch_ms: i64,
        zone: &TimeZone,
        hour12: Option<bool>,
    ) -> String {
        if let Some(table) = intl_pattern::table_locale(&self.tag) {
            let clock = match hour12 {
                None => Clock::Locale,
                Some(true) => Clock::H12,
                Some(false) => Clock::H23,
            };
            if let Some(pattern) = intl_pattern::pattern(table, format, clock) {
                return intl_pattern::render(pattern.tokens, &civil(epoch_ms, zone));
            }
        }
        #[cfg(feature = "icu-fallback")]
        {
            self.date_icu4x(format, epoch_ms, zone, hour12)
        }
        #[cfg(not(feature = "icu-fallback"))]
        {
            let clock = match hour12 {
                None => Clock::Locale,
                Some(true) => Clock::H12,
                Some(false) => Clock::H23,
            };
            intl_pattern::pattern("en-US", format, clock)
                .map(|pattern| intl_pattern::render(pattern.tokens, &civil(epoch_ms, zone)))
                .unwrap_or_default()
        }
    }

    /// [`Formats::date`] through ICU4X's semantic field sets alone: what other locales get. It
    /// follows CLDR's standard patterns, so it differs from `Intl` where `Intl` honours the
    /// requested field widths (`7:30` against `07:30`); `tests/parity.rs` measures how often.
    #[cfg(feature = "icu-fallback")]
    pub fn date_icu4x(
        &self,
        format: DateFormat,
        epoch_ms: i64,
        zone: &TimeZone,
        hour12: Option<bool>,
    ) -> String {
        let civil = civil(epoch_ms, zone);
        let (Ok(date), Ok(time)) = (
            Date::try_new_iso(civil.day.year, civil.day.month, civil.day.day),
            Time::try_new(civil.hour, civil.minute, civil.second, 0),
        ) else {
            return String::new();
        };
        let datetime = DateTime { date, time };
        let mut prefs = DateTimeFormatterPreferences::from(&self.locale);
        prefs.hour_cycle = hour12.map(|twelve| {
            if twelve {
                HourCycle::H12
            } else {
                HourCycle::H23
            }
        });
        macro_rules! render {
            ($field_set:expr) => {
                DateTimeFormatter::try_new(prefs, $field_set)
                    // V8 replaces CLDR's narrow no-break spaces in date output with plain spaces.
                    .map(|formatter| {
                        formatter
                            .format(&datetime)
                            .to_string()
                            .replace('\u{202f}', " ")
                    })
                    .unwrap_or_default()
            };
        }
        use fieldsets::{E, M, MD, T, YMD};
        match format {
            DateFormat::TimeHm => render!(T::medium().with_time_precision(TimePrecision::Minute)),
            DateFormat::TimeHms => render!(T::medium().with_time_precision(TimePrecision::Second)),
            DateFormat::MonthDayNumeric => render!(MD::short()),
            DateFormat::MonthDayYearNumeric => render!(YMD::short()),
            DateFormat::DateMedium => render!(YMD::medium()),
            DateFormat::DateMediumTimeShort => render!(YMD::medium().with_time_hm()),
            DateFormat::MonthShortDay => render!(MD::medium()),
            DateFormat::MonthShortDayYear => render!(YMD::medium()),
            DateFormat::MonthShortDayTime => render!(MD::medium().with_time_hm()),
            DateFormat::WeekdayLong => render!(E::long()),
            DateFormat::WeekdayShort => render!(E::short()),
            DateFormat::MonthShort => render!(M::medium()),
            DateFormat::MonthLong => render!(M::long()),
            DateFormat::DefaultDateTime => render!(YMD::short().with_time_hms()),
            DateFormat::DefaultDate => render!(YMD::short()),
            DateFormat::DefaultTime => {
                render!(T::medium().with_time_precision(TimePrecision::Second))
            }
        }
    }

    // ---- calendar -------------------------------------------------------------------------

    /// The first day of the week as a `Date#getDay` index (0 is Sunday), the locale's own, as
    /// `Intl.Locale#getWeekInfo().firstDay % 7`.
    pub fn week_starts_on(&self) -> u8 {
        WeekInformation::try_new((&self.locale).into())
            .map(|info| match info.first_weekday {
                icu::calendar::types::Weekday::Sunday => 0,
                icu::calendar::types::Weekday::Monday => 1,
                icu::calendar::types::Weekday::Tuesday => 2,
                icu::calendar::types::Weekday::Wednesday => 3,
                icu::calendar::types::Weekday::Thursday => 4,
                icu::calendar::types::Weekday::Friday => 5,
                icu::calendar::types::Weekday::Saturday => 6,
            })
            .unwrap_or(0)
    }

    // ---- lists ----------------------------------------------------------------------------

    /// `new Intl.ListFormat(locale, { type }).format(items)`.
    pub fn list(&self, items: &[&str], kind: ListKind) -> String {
        let options = ListFormatterOptions::default().with_length(ListLength::Wide);
        let formatter = match kind {
            ListKind::Conjunction => ListFormatter::try_new_and((&self.locale).into(), options),
            ListKind::Disjunction => ListFormatter::try_new_or((&self.locale).into(), options),
        };
        match formatter {
            Ok(formatter) => formatter.format_to_string(items.iter().copied()),
            Err(_) => items.join(", "),
        }
    }
}

/// Number options the product passes to `Intl.NumberFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumberOptions {
    pub min_fraction: u8,
    pub max_fraction: u8,
    pub grouping: bool,
}

impl Default for NumberOptions {
    fn default() -> Self {
        Self {
            min_fraction: 0,
            max_fraction: 3,
            grouping: true,
        }
    }
}

fn non_finite(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value > 0.0 {
        "∞".into()
    } else {
        "-∞".into()
    }
}

impl Runtime for Formats {
    fn plural_category(&self, kind: PluralKind, number: f64) -> &'static str {
        Formats::plural_category(self, kind, number)
    }

    fn format_number(&self, number: f64) -> String {
        self.decimal(number)
    }

    fn format_integer(&self, number: f64) -> String {
        self.number(
            number,
            NumberOptions {
                max_fraction: 0,
                ..NumberOptions::default()
            },
        )
    }
}
