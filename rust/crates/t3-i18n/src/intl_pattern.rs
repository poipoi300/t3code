//! Interpreter for the date patterns and compact-number units in `intl_tables.rs`, which
//! `tools/generate-intl-tables.mjs` reads from the runtime's `Intl` for the shipped locales.

use crate::format::{Civil, DateFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// Follow the locale (`hour12` unset).
    Locale,
    H12,
    H23,
}

/// ECMA-402 hour cycles: `H12` is 1-12, `H11` 0-11, `H23` 0-23, `H24` 1-24. The generated table
/// only uses the ones the shipped locales need, so some variants can be unused.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cycle {
    H11,
    H12,
    H23,
    H24,
}

#[derive(Debug)]
pub enum Token {
    Lit(&'static str),
    Year { two_digit: bool },
    Month { pad: bool },
    MonthName(&'static [&'static str; 12]),
    Day { pad: bool },
    Weekday(&'static [&'static str; 7]),
    Hour { pad: bool, cycle: Cycle },
    Minute { pad: bool },
    Second { pad: bool },
    DayPeriod { am: &'static str, pm: &'static str },
}

#[derive(Debug)]
pub struct Pattern {
    pub locale: &'static str,
    pub format: DateFormat,
    pub clock: Clock,
    pub tokens: &'static [Token],
}

#[derive(Debug)]
pub struct Compact {
    pub locale: &'static str,
    /// `(magnitude k, power of ten to divide by, suffix with its separator)` for each magnitude
    /// from which Intl shows a suffix, ascending.
    pub units: &'static [(i16, i16, &'static str)],
}

/// The table locale whose `Intl` output a BCP 47 tag shares, if it is one the product ships.
pub fn table_locale(tag: &str) -> Option<&'static str> {
    let lower = tag.to_ascii_lowercase();
    Some(match lower.as_str() {
        "en" | "en-us" => "en-US",
        "en-gb" => "en-GB",
        "de" | "de-de" => "de",
        "es" | "es-es" => "es",
        "fr" | "fr-fr" => "fr",
        "ja" | "ja-jp" => "ja",
        "ko" | "ko-kr" => "ko",
        "pt-br" => "pt-BR",
        "ru" | "ru-ru" => "ru",
        "zh" | "zh-cn" => "zh",
        _ => return None,
    })
}

pub fn pattern(locale: &str, format: DateFormat, clock: Clock) -> Option<&'static Pattern> {
    let has_clock = |p: &Pattern| p.locale == locale && p.format == format;
    crate::intl_tables::PATTERNS
        .iter()
        .find(|p| has_clock(p) && p.clock == clock)
        .or_else(|| {
            // Formats without an hour carry only the `Locale` variant.
            crate::intl_tables::PATTERNS
                .iter()
                .find(|p| has_clock(p) && p.clock == Clock::Locale)
        })
}

pub fn compact(locale: &str) -> Option<&'static Compact> {
    crate::intl_tables::COMPACT
        .iter()
        .find(|c| c.locale == locale)
}

fn push_padded(out: &mut String, value: u32, pad: bool) {
    if pad && value < 10 {
        out.push('0');
    }
    out.push_str(&value.to_string());
}

pub fn render(tokens: &[Token], civil: &Civil) -> String {
    let mut out = String::new();
    for token in tokens {
        match token {
            Token::Lit(text) => out.push_str(text),
            Token::Year { two_digit } => {
                let year = civil.day.year;
                if *two_digit {
                    push_padded(&mut out, year.rem_euclid(100) as u32, true);
                } else {
                    out.push_str(&year.to_string());
                }
            }
            Token::Month { pad } => push_padded(&mut out, u32::from(civil.day.month), *pad),
            Token::MonthName(names) => out.push_str(names[usize::from(civil.day.month) - 1]),
            Token::Day { pad } => push_padded(&mut out, u32::from(civil.day.day), *pad),
            Token::Weekday(names) => out.push_str(names[usize::from(civil.day.weekday)]),
            Token::Hour { pad, cycle } => {
                let hour = u32::from(civil.hour);
                let shown = match cycle {
                    Cycle::H23 => hour,
                    Cycle::H11 => hour % 12,
                    Cycle::H12 => {
                        if hour % 12 == 0 {
                            12
                        } else {
                            hour % 12
                        }
                    }
                    Cycle::H24 => {
                        if hour == 0 {
                            24
                        } else {
                            hour
                        }
                    }
                };
                push_padded(&mut out, shown, *pad);
            }
            Token::Minute { pad } => push_padded(&mut out, u32::from(civil.minute), *pad),
            Token::Second { pad } => push_padded(&mut out, u32::from(civil.second), *pad),
            Token::DayPeriod { am, pm } => out.push_str(if civil.hour < 12 { am } else { pm }),
        }
    }
    out
}
