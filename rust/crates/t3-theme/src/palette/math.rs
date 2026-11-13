//! Colour maths of `themePalette.ts`, ported expression for expression.
//!
//! The TypeScript module carries its own OKLab constants (ten digits), its own
//! gamut mapping and its own rounding, and the stored theme files depend on
//! their exact output, so this module deliberately does not reuse the
//! browser-accurate conversions in [`crate::color`] except for parsing.

use crate::color::{Color, Space, normalize_hue};

/// An sRGB colour with channels on a 0..255 scale, not necessarily integral.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }
}

pub const LIGHT_FOREGROUND: Rgb = Rgb::new(255.0, 250.0, 255.0);
pub const DARK_FOREGROUND: Rgb = Rgb::new(36.0, 21.0, 35.0);
pub const WHITE: Rgb = Rgb::new(255.0, 255.0, 255.0);
pub const BLACK: Rgb = Rgb::new(0.0, 0.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParsedColor {
    pub color: Oklch,
    pub alpha: f64,
}

/// `parseThemeColor`: any literal CSS colour as clamped OKLCH plus alpha.
pub fn parse_theme_color(value: &str) -> Option<ParsedColor> {
    let input = value.trim();
    let parsed = Color::parse(input)?;
    let [l, c, h] = parsed.to_space(Space::Oklch).c;
    let alpha = parsed.alpha;
    if ![l, c, h, alpha].iter().all(|v| v.is_finite()) {
        return None;
    }
    Some(ParsedColor {
        color: Oklch {
            l: l.clamp(0.0, 1.0),
            c: c.max(0.0),
            h,
        },
        alpha: alpha.clamp(0.0, 1.0),
    })
}

/// JavaScript's `Number.prototype.toFixed` for the magnitudes theme colours reach.
fn js_to_fixed(value: f64, precision: usize) -> String {
    if value.abs() >= 1e21 {
        let exponential = format!("{value:e}");
        return match exponential.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => exponential,
        };
    }
    format!("{value:.precision$}")
}

/// `formatThemeColorNumber`.
fn format_number(value: f64, precision: usize) -> String {
    let rounded = if value.abs() < 10f64.powi(-(precision as i32)) / 2.0 {
        0.0
    } else {
        value
    };
    let text = js_to_fixed(rounded, precision);
    if text.contains('e') {
        return text;
    }
    trim_number_text(&text)
}

fn trim_number_text(text: &str) -> String {
    if !text.contains('.') {
        return text.to_owned();
    }
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `formatOklchThemeColor`.
pub fn format_oklch(color: Oklch, alpha: f64) -> String {
    let hue = if color.c < 0.0000005 {
        0.0
    } else {
        ((color.h % 360.0) + 360.0) % 360.0
    };
    let body = format!(
        "{} {} {}",
        format_number(color.l, 6),
        format_number(color.c, 6),
        format_number(hue, 3)
    );
    if alpha < 1.0 {
        format!("oklch({body} / {})", format_number(alpha, 4))
    } else {
        format!("oklch({body})")
    }
}

/// `toCanonicalThemeColor`.
pub fn to_canonical_theme_color(value: &str) -> Option<String> {
    parse_theme_color(value).map(|p| format_oklch(p.color, p.alpha))
}

/// `themeColorToHex`: `#rrggbb`, or `#rrggbbaa` when translucent.
pub fn theme_color_to_hex(value: &str) -> Option<String> {
    let parsed = parse_theme_color(value)?;
    let opaque = rgb_to_hex(oklch_to_rgb(parsed.color, true));
    if parsed.alpha >= 1.0 {
        return Some(opaque);
    }
    let alpha = (parsed.alpha * 255.0 + 0.5).floor() as u8;
    Some(format!("{opaque}{alpha:02x}"))
}

pub fn parse_rgb(value: &str, fallback: Rgb) -> Rgb {
    parse_theme_color(value).map_or(fallback, |p| oklch_to_rgb(p.color, true))
}

pub fn rgb_to_hex(color: Rgb) -> String {
    let channel = |v: f64| (v.clamp(0.0, 255.0) + 0.5).floor() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

pub fn rgb_to_theme_color(color: Rgb) -> String {
    format_oklch(rgb_to_oklch(color), 1.0)
}

pub fn mix(base: Rgb, overlay: Rgb, amount: f64) -> Rgb {
    Rgb {
        r: base.r + (overlay.r - base.r) * amount,
        g: base.g + (overlay.g - base.g) * amount,
        b: base.b + (overlay.b - base.b) * amount,
    }
}

pub fn relative_luminance(color: Rgb) -> f64 {
    let linearize = |channel: f64| {
        let n = channel / 255.0;
        if n <= 0.03928 {
            n / 12.92
        } else {
            ((n + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linearize(color.r) + 0.7152 * linearize(color.g) + 0.0722 * linearize(color.b)
}

fn srgb_channel_to_linear(channel: f64) -> f64 {
    let c = channel / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_channel_to_srgb(channel: f64, round: bool) -> f64 {
    let c = if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    let srgb = c.clamp(0.0, 1.0) * 255.0;
    if round { (srgb + 0.5).floor() } else { srgb }
}

pub fn rgb_to_oklch(color: Rgb) -> Oklch {
    let r = srgb_channel_to_linear(color.r);
    let g = srgb_channel_to_linear(color.g);
    let b = srgb_channel_to_linear(color.b);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let lightness = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    let bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    Oklch {
        l: lightness,
        c: a.hypot(bb),
        h: bb.atan2(a).to_degrees(),
    }
}

fn oklch_to_rgb_unclamped(color: Oklch) -> [f64; 3] {
    let hr = color.h.to_radians();
    let a = color.c * hr.cos();
    let bb = color.c * hr.sin();
    let l = (color.l + 0.3963377774 * a + 0.2158037573 * bb).powi(3);
    let m = (color.l - 0.1055613458 * a - 0.0638541728 * bb).powi(3);
    let s = (color.l - 0.0894841775 * a - 1.291485548 * bb).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ]
}

/// `mapThemeOklchToSrgbGamut`: reduces chroma by bisection until the colour fits sRGB.
pub fn map_oklch_to_srgb_gamut(color: Oklch) -> Oklch {
    let is_in_gamut = |c: f64| {
        oklch_to_rgb_unclamped(Oklch { c, ..color })
            .iter()
            .all(|channel| (-0.0001..=1.0001).contains(channel))
    };
    if is_in_gamut(color.c) {
        return color;
    }
    let mut low = 0.0;
    let mut high = color.c;
    let resolution = 0.000001_f64;
    let steps = ((color.c.max(resolution)).log2() - resolution.log2())
        .ceil()
        .max(1.0) as usize;
    for _ in 0..steps {
        let mid = (low + high) / 2.0;
        if is_in_gamut(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    Oklch { c: low, ..color }
}

pub fn oklch_to_rgb(color: Oklch, round: bool) -> Rgb {
    let linear = oklch_to_rgb_unclamped(map_oklch_to_srgb_gamut(color));
    Rgb {
        r: linear_channel_to_srgb(linear[0], round),
        g: linear_channel_to_srgb(linear[1], round),
        b: linear_channel_to_srgb(linear[2], round),
    }
}

pub fn oklch_to_theme_color(color: Oklch) -> String {
    format_oklch(map_oklch_to_srgb_gamut(color), 1.0)
}

pub fn contrast_ratio(first: Rgb, second: Rgb) -> f64 {
    let a = relative_luminance(first);
    let b = relative_luminance(second);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Lighter,
    Darker,
}

/// `solveOklchLightness`: bisection on lightness only, so hue and chroma survive.
pub fn solve_oklch_lightness(
    base: Oklch,
    against: &[Rgb],
    min_contrast: f64,
    direction: Direction,
) -> Oklch {
    let contrast_of = |color: Oklch| {
        against
            .iter()
            .map(|surface| contrast_ratio(oklch_to_rgb(color, true), *surface))
            .fold(f64::INFINITY, f64::min)
    };
    let (mut low, mut high) = match direction {
        Direction::Lighter => (base.l, 1.0),
        Direction::Darker => (0.0, base.l),
    };
    let mut candidate = base;
    if contrast_of(candidate) >= min_contrast {
        return candidate;
    }
    for _ in 0..18 {
        let mid = (low + high) / 2.0;
        candidate = Oklch { l: mid, ..base };
        let meets = contrast_of(candidate) >= min_contrast;
        match (meets, direction) {
            (true, Direction::Lighter) => high = mid,
            (true, Direction::Darker) => low = mid,
            (false, Direction::Lighter) => low = mid,
            (false, Direction::Darker) => high = mid,
        }
    }
    Oklch {
        l: match direction {
            Direction::Lighter => high,
            Direction::Darker => low,
        },
        ..base
    }
}

/// `readableThemeColorOn`: moves only lightness until `color` reads on `surface`.
pub fn readable_theme_color_on(color: &str, surface: &str, min_contrast: f64) -> String {
    let (Some(parsed_color), Some(parsed_surface)) =
        (parse_theme_color(color), parse_theme_color(surface))
    else {
        return color.to_owned();
    };
    let surface_rgb = oklch_to_rgb(parsed_surface.color, true);
    if contrast_ratio(oklch_to_rgb(parsed_color.color, true), surface_rgb) >= min_contrast {
        return color.to_owned();
    }
    let direction = if relative_luminance(surface_rgb) < 0.179 {
        Direction::Lighter
    } else {
        Direction::Darker
    };
    let solved = solve_oklch_lightness(parsed_color.color, &[surface_rgb], min_contrast, direction);
    rgb_to_hex(oklch_to_rgb(solved, true))
}

pub fn readable_foreground(background: Rgb) -> Rgb {
    let light = contrast_ratio(background, LIGHT_FOREGROUND);
    let dark = contrast_ratio(background, DARK_FOREGROUND);
    if light.max(dark) >= 4.5 {
        return if light >= dark {
            LIGHT_FOREGROUND
        } else {
            DARK_FOREGROUND
        };
    }
    if contrast_ratio(background, WHITE) >= contrast_ratio(background, BLACK) {
        WHITE
    } else {
        BLACK
    }
}

pub fn readable_text(background: Rgb, foreground: Rgb, amount: f64, minimum_ratio: f64) -> Rgb {
    let softened = mix(foreground, background, amount);
    if contrast_ratio(softened, background) >= minimum_ratio {
        return softened;
    }
    let mut readable = foreground;
    let mut lower = 0.0;
    let mut upper = amount;
    for _ in 0..12 {
        let candidate_amount = (lower + upper) / 2.0;
        let candidate = mix(foreground, background, candidate_amount);
        if contrast_ratio(candidate, background) >= minimum_ratio {
            readable = candidate;
            lower = candidate_amount;
        } else {
            upper = candidate_amount;
        }
    }
    readable
}

const STANDARD_LIGHT_MUTED_CONTRAST: f64 = 4.705;
const STANDARD_DARK_MUTED_CONTRAST: f64 = 5.082;

pub fn standard_muted_text(background: Rgb, foreground: Rgb) -> Rgb {
    let target = if relative_luminance(background) < 0.179 {
        STANDARD_DARK_MUTED_CONTRAST
    } else {
        STANDARD_LIGHT_MUTED_CONTRAST
    };
    readable_text(background, foreground, 1.0, target)
}

/// JavaScript's `%` followed by the `+ 360` wrap the theme code uses for hues.
pub fn wrap_hue(hue: f64) -> f64 {
    normalize_hue(hue)
}

pub fn number_text(value: f64, precision: usize) -> String {
    format_number(value, precision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::trim_number;

    #[test]
    fn extreme_chroma_serialises_like_javascript() {
        assert_eq!(
            to_canonical_theme_color("oklch(0.5 1e303 0)").as_deref(),
            Some("oklch(0.5 1e+303 0)")
        );
        assert_eq!(
            theme_color_to_hex("oklch(0.5 1e303 0)").as_deref(),
            Some("#b5005e")
        );
    }

    #[test]
    fn canonical_form_drops_default_alpha_and_trailing_zeros() {
        assert_eq!(
            to_canonical_theme_color("oklch(62% 0.2 280deg / 50%)").as_deref(),
            Some("oklch(0.62 0.2 280 / 0.5)")
        );
        assert_eq!(
            to_canonical_theme_color("transparent").as_deref(),
            Some("oklch(0 0 0 / 0)")
        );
        assert_eq!(trim_number(1.0, 3), "1");
    }
}
