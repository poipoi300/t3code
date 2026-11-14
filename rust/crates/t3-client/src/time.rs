//! Wire timestamps as JavaScript reads them.

/// Epoch milliseconds of an ISO-8601 wire timestamp, as `Date.parse` computes them for the ISO
/// format, which is what every TypeScript caller uses. Wire strings themselves are never
/// rewritten; this is for ordering, expiry and elapsed-time math only.
///
/// Like V8: a day past the month's end rolls over (`02-31` is March 3), `24:00` is the next
/// midnight, fractions beyond milliseconds are truncated, and a date alone is UTC midnight. A
/// date-time without a zone is host-local time in JavaScript; there is no host zone here, so it
/// returns `None` rather than guessing.
pub fn epoch_ms(value: &str) -> Option<i64> {
    let mut cursor = Cursor(value.as_bytes());
    let year = match cursor.peek()? {
        sign @ (b'+' | b'-') => {
            cursor.0 = &cursor.0[1..];
            let year = cursor.digits(6)?;
            if sign == b'-' {
                if year == 0 {
                    return None;
                }
                -year
            } else {
                year
            }
        }
        _ => cursor.digits(4)?,
    };
    cursor.expect(b'-')?;
    let month = cursor.digits(2)?;
    cursor.expect(b'-')?;
    let day = cursor.digits(2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if cursor.0.is_empty() {
        return Some(days * 86_400_000);
    }
    if !matches!(cursor.peek()?, b'T' | b't' | b' ') {
        return None;
    }
    cursor.0 = &cursor.0[1..];
    let hour = cursor.digits(2)?;
    cursor.expect(b':')?;
    let minute = cursor.digits(2)?;
    let (mut second, mut millis) = (0, 0);
    if cursor.peek() == Some(b':') {
        cursor.0 = &cursor.0[1..];
        second = cursor.digits(2)?;
        if cursor.peek() == Some(b'.') {
            cursor.0 = &cursor.0[1..];
            let fraction = cursor.0.iter().take_while(|b| b.is_ascii_digit()).count();
            if fraction == 0 {
                return None;
            }
            millis = cursor.0[..fraction]
                .iter()
                .chain(b"00")
                .take(3)
                .fold(0, |n, digit| n * 10 + i64::from(digit - b'0'));
            cursor.0 = &cursor.0[fraction..];
        }
    }
    let midnight = hour == 24 && minute == 0 && second == 0 && millis == 0;
    if (hour > 23 && !midnight) || minute > 59 || second > 59 {
        return None;
    }
    let offset = match cursor.peek()? {
        b'Z' | b'z' => {
            cursor.0 = &cursor.0[1..];
            0
        }
        sign @ (b'+' | b'-') => {
            cursor.0 = &cursor.0[1..];
            let hours = cursor.digits(2)?;
            if cursor.peek() == Some(b':') {
                cursor.0 = &cursor.0[1..];
            }
            let minutes = cursor.digits(2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            (hours * 60 + minutes) * if sign == b'-' { -1 } else { 1 }
        }
        _ => return None,
    };
    if !cursor.0.is_empty() {
        return None;
    }
    Some(((days * 24 + hour) * 60 + minute - offset) * 60_000 + second * 1_000 + millis)
}

/// `new Date(epoch_ms).toISOString()`: always UTC, milliseconds, six-digit years past 9999.
pub fn iso_from_epoch_ms(epoch_ms: u64) -> String {
    let z = (epoch_ms / 86_400_000) as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let year = if year <= 9999 {
        format!("{year:04}")
    } else {
        format!("+{year:06}")
    };
    let seconds = epoch_ms / 1000;
    format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        epoch_ms % 1000
    )
}

/// Days since 1970-01-01 of a proleptic Gregorian date; days past the month's end roll over.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }
    fn expect(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.0 = &self.0[1..])
    }
    fn digits(&mut self, count: usize) -> Option<i64> {
        let digits = self.0.get(..count)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.0 = &self.0[count..];
        Some(
            digits
                .iter()
                .fold(0, |n, digit| n * 10 + i64::from(digit - b'0')),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::epoch_ms;

    #[test]
    fn matches_javascript_date_parse() {
        // Expected values are `Date.parse` in Node 22 (V8); `None` is `NaN`, or a zoneless
        // date-time, which JavaScript reads in the host's zone.
        for (input, expected) in [
            ("1970-01-01T00:00:00Z", Some(0)),
            ("1970-01-01T01:00:00.123+01:00", Some(123)),
            ("2026-02-31T00:00:00Z", Some(1_772_496_000_000)),
            ("2026-02-29T00:00:00Z", Some(1_772_323_200_000)),
            ("2024-02-29T00:00:00Z", Some(1_709_164_800_000)),
            ("2026-1-5T00:00:00Z", None),
            ("2026-01-05T00:00:00.1234Z", Some(1_767_571_200_123)),
            ("2026-01-05T00:00:00.1Z", Some(1_767_571_200_100)),
            ("2026-01-05T00:00:00.Z", None),
            ("2026-01-05T00:00Z", Some(1_767_571_200_000)),
            ("2026-01-05T00:00:00+05:30", Some(1_767_551_400_000)),
            ("2026-01-05T00:00:00+0530", Some(1_767_551_400_000)),
            ("2026-01-05T24:00:00Z", Some(1_767_657_600_000)),
            ("2026-01-05T23:59:60Z", None),
            ("+012026-01-05T00:00:00Z", Some(317_337_091_200_000)),
            ("0000-01-01T00:00:00Z", Some(-62_167_219_200_000)),
            ("2026-01-05", Some(1_767_571_200_000)),
            ("2026-01-05T00:00:00z", Some(1_767_571_200_000)),
            ("2026-01-05 00:00:00Z", Some(1_767_571_200_000)),
            ("2026-01-05T00:00:00+24:00", None),
            ("2026-01-05T00:00:00.000-00:00", Some(1_767_571_200_000)),
            ("2026-01-05T00:00:00", None),
            ("26-01-05T00:00:00Z", None),
            ("not a date", None),
            ("", None),
        ] {
            assert_eq!(epoch_ms(input), expected, "{input}");
        }
    }

    #[test]
    fn formats_like_javascript_to_iso_string() {
        use super::iso_from_epoch_ms;
        // `new Date(ms).toISOString()` in Node 22.
        assert_eq!(iso_from_epoch_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso_from_epoch_ms(1_772_496_000_123),
            "2026-03-03T00:00:00.123Z"
        );
        assert_eq!(
            iso_from_epoch_ms(317_337_091_200_000),
            "+012026-01-05T00:00:00.000Z"
        );
        for ms in [0, 1_709_164_800_000, 1_767_571_200_123, 253_402_300_799_999] {
            assert_eq!(epoch_ms(&iso_from_epoch_ms(ms)), Some(ms as i64));
        }
    }
}
