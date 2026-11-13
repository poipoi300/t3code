//! The product's own number-to-text helpers: byte sizes (`apps/web/src/lib/formatBytes.ts`) and
//! the usage figures of `packages/shared/src/usageFormat.ts`.
//!
//! They are not localised today: `toFixed` always writes `.`, units are English, and the grouped
//! figures use `Intl.NumberFormat("en-US")`. They are ported exactly so the Rust UI shows the
//! same text; localising them is a product decision that belongs in the UI rewrite (the
//! locale-aware pieces are [`crate::Formats::number`] and [`crate::Formats::compact`]).

use crate::format::{Formats, NumberOptions};

/// JavaScript's `Number#toFixed`: round-half-up on the exact value of the double, sign kept
/// even when the result is zero (`(-0.001).toFixed(2)` is `"-0.00"`).
pub fn to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return if value.is_nan() {
            "NaN".into()
        } else if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let negative = value < 0.0;
    // A double has at most 1074 fractional digits, but a tie or near-tie within 40 is decisive
    // for the magnitudes a UI shows.
    let exact = format!("{:.40}", value.abs());
    let (integer, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits_vec: Vec<u8> = integer
        .bytes()
        .chain(fraction.bytes().take(digits))
        .collect();
    let round_up = fraction
        .as_bytes()
        .get(digits)
        .is_some_and(|next| *next >= b'5');
    if round_up {
        let mut index = digits_vec.len();
        loop {
            if index == 0 {
                digits_vec.insert(0, b'1');
                break;
            }
            index -= 1;
            if digits_vec[index] == b'9' {
                digits_vec[index] = b'0';
            } else {
                digits_vec[index] += 1;
                break;
            }
        }
    }
    let split = digits_vec.len() - digits;
    let (int_part, frac_part) = digits_vec.split_at(split);
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&String::from_utf8_lossy(int_part));
    if digits > 0 {
        out.push('.');
        out.push_str(&String::from_utf8_lossy(frac_part));
    }
    out
}

/// JavaScript's `Math.round`: ties go toward +infinity.
pub fn js_round(value: f64) -> f64 {
    if value.fract() == -0.5 {
        value.ceil()
    } else {
        value.round()
    }
}

fn format_scaled(value: f64) -> String {
    to_fixed(
        value,
        if value >= 100.0 {
            0
        } else if value >= 10.0 {
            1
        } else {
            2
        },
    )
}

fn number(text: &str) -> f64 {
    text.parse().unwrap_or(f64::NAN)
}

/// `formatBytes`: `1.46 KB`, `512 B`.
pub fn format_bytes(value: f64) -> String {
    if js_round(value) < 1024.0 {
        return format!("{} B", js_integer(js_round(value)));
    }
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut next = value / 1024.0;
    let mut unit = 0;
    while number(&format_scaled(next)) >= 1024.0 && unit < UNITS.len() - 1 {
        next /= 1024.0;
        unit += 1;
    }
    format!("{} {}", format_scaled(next), UNITS[unit])
}

/// `String(Math.round(x))` for the integers a size can be.
fn js_integer(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        format!("{value}")
    }
}

/// `INTEGER.format(Math.round(value))`, en-US.
pub fn format_count(value: f64) -> String {
    Formats::new("en-US").number(
        js_round(value),
        NumberOptions {
            max_fraction: 0,
            ..NumberOptions::default()
        },
    )
}

/// `CURRENCY.format(value)`: US dollars with two decimals, en-US.
pub fn format_usd(value: f64) -> String {
    Formats::new("en-US").currency(value, "USD")
}

fn trim(value: f64) -> String {
    let digits = if value.abs() >= 100.0 {
        0
    } else if value.abs() >= 10.0 {
        1
    } else {
        2
    };
    let fixed = to_fixed(value, digits);
    // `.replace(/\.0+$/, "")`
    match fixed.split_once('.') {
        Some((integer, fraction)) if fraction.bytes().all(|b| b == b'0') => integer.to_owned(),
        _ => fixed,
    }
}

/// `formatTokens`: `999`, `1.5K`, `1M`.
pub fn format_tokens(value: f64) -> String {
    if js_round(value).abs() < 1e3 {
        return format_count(value);
    }
    const SUFFIXES: [&str; 4] = ["K", "M", "B", "T"];
    let mut scaled = value / 1e3;
    let mut unit = 0;
    while number(&trim(scaled)).abs() >= 1e3 && unit < SUFFIXES.len() - 1 {
        scaled /= 1e3;
        unit += 1;
    }
    format!("{}{}", trim(scaled), SUFFIXES[unit])
}

/// `formatPercent`: a share (0.5 is 50%) with `digits` decimals, `<0.1%` below the smallest.
pub fn format_percent(share: f64, digits: usize) -> String {
    let percent = share * 100.0;
    let smallest = 10f64.powi(-(digits as i32));
    if percent > 0.0 && percent < smallest {
        return format!("<{}%", to_fixed(smallest, digits));
    }
    format!("{}%", to_fixed(percent, digits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_rounds_ties_up_on_the_exact_value() {
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(0.5, 0), "1");
        assert_eq!(to_fixed(-2.5, 0), "-3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(1.255, 2), "1.25");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(9.995, 2), "9.99");
        assert_eq!(to_fixed(99.5, 0), "100");
    }

    #[test]
    fn js_round_ties_go_up() {
        assert_eq!(
            [js_round(2.5), js_round(-2.5), js_round(-0.4), js_round(1.5)],
            [3.0, -2.0, -0.0, 2.0]
        );
    }
}
