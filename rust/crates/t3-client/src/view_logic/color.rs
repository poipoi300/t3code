//! Port of apps/web/src/lib/color.ts: the colour pickers' HSV model and its hex conversions.

/// A picker colour: hue in degrees (0 to 360), saturation and value from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HsvColor {
    pub h: f64,
    pub s: f64,
    pub v: f64,
}

/// `hexToHsv`: `#rrggbb` (anything after the six digits, such as an alpha pair, is not read).
pub fn hex_to_hsv(hex: &str) -> HsvColor {
    let numeric = parse_int_prefix_hex(hex.get(1..).unwrap_or(""));
    let red = ((numeric >> 16) & 255) as f64 / 255.0;
    let green = ((numeric >> 8) & 255) as f64 / 255.0;
    let blue = (numeric & 255) as f64 / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;

    let mut hue = 0.0;
    if delta != 0.0 {
        hue = if max == red {
            ((green - blue) / delta) % 6.0
        } else if max == green {
            (blue - red) / delta + 2.0
        } else {
            (red - green) / delta + 4.0
        };
        hue *= 60.0;
        if hue < 0.0 {
            hue += 360.0;
        }
    }

    HsvColor {
        h: hue,
        s: if max == 0.0 { 0.0 } else { delta / max },
        v: max,
    }
}

/// `hsvToHex`: lower-case `#rrggbb`.
pub fn hsv_to_hex(hue: f64, saturation: f64, value: f64) -> String {
    let normalized_hue = ((hue % 360.0) + 360.0) % 360.0;
    let chroma = value * saturation;
    let x = chroma * (1.0 - (((normalized_hue / 60.0) % 2.0) - 1.0).abs());
    let matched = value - chroma;
    let (red, green, blue) = if normalized_hue < 60.0 {
        (chroma, x, 0.0)
    } else if normalized_hue < 120.0 {
        (x, chroma, 0.0)
    } else if normalized_hue < 180.0 {
        (0.0, chroma, x)
    } else if normalized_hue < 240.0 {
        (0.0, x, chroma)
    } else if normalized_hue < 300.0 {
        (x, 0.0, chroma)
    } else {
        (chroma, 0.0, x)
    };
    let channel = |channel: f64| js_round((channel + matched) * 255.0) as i64;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(red),
        channel(green),
        channel(blue)
    )
}

/// `Number.parseInt(text, 16)` on the leading hex digits; `NaN` (no digit) reads as 0, as the
/// bit operations in `hexToHsv` turn it into 0.
fn parse_int_prefix_hex(text: &str) -> u64 {
    let digits = text.trim_start();
    let (negative, digits) = match digits.as_bytes().first() {
        Some(b'-') => (true, &digits[1..]),
        Some(b'+') => (false, &digits[1..]),
        _ => (false, digits),
    };
    let mut value: u64 = 0;
    for ch in digits.chars() {
        let Some(digit) = ch.to_digit(16) else { break };
        value = value.wrapping_mul(16).wrapping_add(u64::from(digit));
    }
    // `>>` and `&` work on the 32-bit integer JavaScript makes of the number.
    let value = value as u32;
    u64::from(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

/// JavaScript's `Math.round`: halves go up.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_hsv_without_changing_the_persisted_color() {
        for hex in [
            "#000000", "#ffffff", "#808080", "#ff0000", "#00ff00", "#0000ff", "#2563eb",
        ] {
            let HsvColor { h, s, v } = hex_to_hsv(hex);
            assert_eq!(hsv_to_hex(h, s, v), hex);
        }
    }

    #[test]
    fn represents_black_and_greys_without_undefined_saturation() {
        assert_eq!(
            hex_to_hsv("#000000"),
            HsvColor {
                h: 0.0,
                s: 0.0,
                v: 0.0
            }
        );
        assert_eq!(
            hex_to_hsv("#ffffff"),
            HsvColor {
                h: 0.0,
                s: 0.0,
                v: 1.0
            }
        );
    }

    #[test]
    fn reads_only_the_colour_of_a_translucent_value_and_wraps_hue() {
        // parseInt reads all eight digits; the bit masks keep the low 24, as on the web.
        assert_eq!(hex_to_hsv("#ff000080"), hex_to_hsv("#000080"));
        assert_eq!(hsv_to_hex(360.0, 0.5, 0.5), hsv_to_hex(0.0, 0.5, 0.5));
        // colorPickers.browser.test.tsx: #804040 with its hue moved to 10 degrees.
        let HsvColor { s, v, .. } = hex_to_hsv("#804040");
        assert_eq!(hsv_to_hex(10.0, s, v), "#804b40");
        assert_eq!(hsv_to_hex(0.0, s, v), "#804040");
    }
}
