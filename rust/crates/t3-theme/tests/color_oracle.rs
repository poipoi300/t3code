//! Colour maths against values computed by culori and 8-bit pixels painted by
//! Chrome (fixtures/color-corpus.json, written by tools/color-oracle.mjs).

use serde::Deserialize;
use t3_theme::color::{Color, MixSpace, color_mix};

#[derive(Deserialize)]
struct Painted {
    #[serde(rename = "onBlack")]
    on_black: [u8; 4],
    #[serde(rename = "onWhite")]
    on_white: [u8; 4],
}

#[derive(Deserialize)]
struct Culori {
    oklch: [f64; 3],
    oklab: [f64; 3],
    lab: [f64; 3],
    srgb: [f64; 3],
    alpha: f64,
}

#[derive(Deserialize)]
struct Entry {
    css: String,
    chrome: Painted,
    culori: Option<Option<Culori>>,
}

#[derive(Deserialize)]
struct Corpus {
    entries: Vec<Entry>,
}

fn corpus() -> Corpus {
    serde_json::from_str(include_str!("../fixtures/color-corpus.json")).expect("fixture parses")
}

fn split_top_level(text: &str) -> Vec<&str> {
    let mut depth = 0;
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

fn parse_mix_operand(text: &str) -> (Color, Option<f64>) {
    let text = text.trim();
    if let Some((color, pct)) = text.rsplit_once(' ')
        && let Some(p) = pct.strip_suffix('%')
        && let Ok(p) = p.parse::<f64>()
        && !color.ends_with(',')
    {
        return (Color::parse(color).expect("operand"), Some(p));
    }
    (Color::parse(text).expect("operand"), None)
}

fn parse_mix(css: &str) -> Color {
    let inner = css
        .strip_prefix("color-mix(in ")
        .and_then(|s| s.strip_suffix(')'))
        .expect("mix");
    let (space, rest) = inner.split_once(',').expect("space");
    let space = match space.trim() {
        "srgb" => MixSpace::Srgb,
        "srgb-linear" => MixSpace::SrgbLinear,
        "oklab" => MixSpace::Oklab,
        "oklch" => MixSpace::Oklch,
        other => panic!("{other}"),
    };
    let operands = split_top_level(rest);
    let (a, pa) = parse_mix_operand(operands[0]);
    let (b, pb) = parse_mix_operand(operands[1]);
    color_mix(space, a, pa, b, pb)
}

fn composite(color: &Color, backdrop: f64) -> [f64; 3] {
    // Skia floors channels at zero, blends, and clamps the result when storing.
    let rgb = color.to_srgb_unclamped();
    let a = color.alpha.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (rgb[i].max(0.0) * a + backdrop * (1.0 - a)).clamp(0.0, 1.0) * 255.0)
}

#[test]
fn conversions_match_culori() {
    let mut checked = 0;
    for entry in corpus().entries {
        let Some(Some(reference)) = entry.culori else {
            continue;
        };
        let color = Color::parse(&entry.css).unwrap_or_else(|| panic!("parse {}", entry.css));
        assert!(
            (color.alpha - reference.alpha).abs() < 1e-9,
            "alpha {}",
            entry.css
        );
        let oklab = color.to_oklab();
        for i in 0..3 {
            assert!(
                (oklab[i] - reference.oklab[i]).abs() < 2e-6,
                "oklab {} {oklab:?} {:?}",
                entry.css,
                reference.oklab
            );
        }
        let oklch = color.to_oklch();
        assert!(
            (oklch[0] - reference.oklch[0]).abs() < 2e-6,
            "oklch L {}",
            entry.css
        );
        assert!(
            (oklch[1] - reference.oklch[1]).abs() < 2e-6,
            "oklch C {}",
            entry.css
        );
        if reference.oklch[1] > 1e-3 {
            let dh = (oklch[2] - reference.oklch[2] + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                dh.abs() < 1e-3,
                "oklch h {} {oklch:?} {:?}",
                entry.css,
                reference.oklch
            );
        }
        let lab = color.to_space(t3_theme::color::Space::Lab).c;
        for i in 0..3 {
            assert!(
                (lab[i] - reference.lab[i]).abs() < 1e-4 * (1.0 + reference.lab[i].abs()),
                "lab {} {lab:?} {:?}",
                entry.css,
                reference.lab
            );
        }
        let srgb = color.to_srgb_unclamped();
        for i in 0..3 {
            assert!(
                (srgb[i] - reference.srgb[i]).abs() < 2e-5,
                "srgb {} {srgb:?} {:?}",
                entry.css,
                reference.srgb
            );
        }
        checked += 1;
    }
    assert!(checked > 1500, "{checked}");
}

fn within(actual: [f64; 3], expected: [u8; 4], tolerance: f64, css: &str) -> Option<String> {
    (0..3)
        .any(|i| (actual[i] - f64::from(expected[i])).abs() > tolerance)
        .then(|| format!("{css}: rust {actual:?} chrome {expected:?}"))
}

#[test]
fn painted_pixels_match_chrome() {
    let mut mismatches = Vec::new();
    let mut opaque = 0;
    let mut exact = 0;
    let mut translucent = 0;
    for entry in corpus().entries {
        let color = if entry.css.starts_with("color-mix(") {
            parse_mix(&entry.css)
        } else {
            Color::parse(&entry.css).expect("parse")
        };
        if color.alpha >= 1.0 {
            opaque += 1;
            let [r, g, b, _] = color.to_rgba8();
            let chrome = entry.chrome.on_black;
            let worst = [r, g, b]
                .iter()
                .zip(chrome)
                .map(|(a, c)| (i32::from(*a) - i32::from(c)).abs())
                .max()
                .unwrap_or(0);
            if worst == 0 {
                exact += 1;
            }
            // Chrome's raster path differs from exact arithmetic by up to one level.
            if worst > 1 {
                mismatches.push(format!(
                    "{}: rust {:?} chrome {:?}",
                    entry.css,
                    [r, g, b],
                    chrome
                ));
            }
        } else {
            translucent += 1;
            // Chrome stores 8-bit premultiplied pixels, so allow two levels.
            for (backdrop, painted) in [(0.0, entry.chrome.on_black), (1.0, entry.chrome.on_white)]
            {
                if let Some(m) = within(composite(&color, backdrop), painted, 2.0, &entry.css) {
                    mismatches.push(m);
                }
            }
        }
    }
    assert!(opaque > 1500 && translucent > 100, "{opaque} {translucent}");
    assert!(
        exact * 100 > opaque * 70,
        "only {exact} of {opaque} opaque colours match Chrome exactly"
    );
    assert!(
        mismatches.is_empty(),
        "{} mismatches; first: {:#?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(15)]
    );
}
