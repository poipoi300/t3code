//! Syntax highlighting palettes for code blocks and diffs.
//!
//! The app highlights with the Highlights engine and one of three Highlights ("zed")
//! palettes: `pierre-dark` for dark appearances and `t3-light`, `pierre-light` with every
//! colour darkened until it reads on the darkest light code surface. Surfaces take their
//! background, foreground and diff tints from the resolved theme; see
//! [`crate::tokens::surface`].

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::palette::Appearance;

pub const READABLE_LIGHT_THEME_NAME: &str = "t3-light";
pub const DARK_SYNTAX_THEME_NAME: &str = "pierre-dark";

const MIN_CONTRAST: f64 = 4.5;
const FURTHER_MIN_CONTRAST: f64 = 6.0;
/// The darkest light code surface is a changed word on a deleted line (`#f8d3d4`, luminance
/// 0.71), rounded down for margin.
const DARKEST_SURFACE_LUMINANCE: f64 = 0.69;
/// Pairs that differ only in lightness would merge at one floor, so the darker of each goes
/// further, keeping pierre-light's order.
const FURTHER_COLORS: [&str; 3] = ["#1ca1c7", "#636363", "#1a85d4"];

/// The style of one syntax scope.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub cursor: String,
    pub selection: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeStyle {
    pub background: String,
    pub text: String,
    #[serde(rename = "editor.background")]
    pub editor_background: String,
    #[serde(rename = "editor.foreground")]
    pub editor_foreground: String,
    #[serde(rename = "editor.active_line.background")]
    pub editor_active_line_background: String,
    pub players: Vec<Player>,
    pub created: String,
    pub deleted: String,
    pub modified: String,
    #[serde(flatten)]
    terminal: BTreeMap<String, Value>,
    #[serde(with = "ordered_syntax")]
    pub syntax: Vec<(String, ScopeStyle)>,
}

mod ordered_syntax {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::{Map, Value};

    use super::ScopeStyle;

    pub fn serialize<S: Serializer>(
        value: &[(String, ScopeStyle)],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = Map::new();
        for (scope, style) in value {
            map.insert(
                scope.clone(),
                serde_json::to_value(style).map_err(serde::ser::Error::custom)?,
            );
        }
        map.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<(String, ScopeStyle)>, D::Error> {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        map.into_iter()
            .map(|(scope, value)| {
                let style = match value {
                    Value::String(color) => ScopeStyle {
                        color: Some(color),
                        ..ScopeStyle::default()
                    },
                    other => serde_json::from_value(other).map_err(D::Error::custom)?,
                };
                Ok((scope, style))
            })
            .collect()
    }
}

impl ThemeStyle {
    /// The 16 terminal ANSI colours in `black, red, ... white, bright_black, ... bright_white` order.
    pub fn terminal_ansi(&self) -> Vec<&str> {
        const NAMES: [&str; 8] = [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ];
        let get = |key: String| {
            self.terminal
                .get(&key)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        NAMES
            .iter()
            .map(|name| get(format!("terminal.ansi.{name}")))
            .chain(
                NAMES
                    .iter()
                    .map(|name| get(format!("terminal.ansi.bright_{name}"))),
            )
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxTheme {
    pub name: String,
    pub appearance: Appearance,
    pub style: ThemeStyle,
}

impl SyntaxTheme {
    pub fn scope(&self, scope: &str) -> Option<&ScopeStyle> {
        self.style
            .syntax
            .iter()
            .find(|(name, _)| name == scope)
            .map(|(_, style)| style)
    }
}

#[derive(Deserialize)]
struct DiffThemeNames {
    light: String,
    dark: String,
}

#[derive(Deserialize)]
struct SyntaxFixture {
    #[serde(rename = "diffThemeNames")]
    diff_theme_names: DiffThemeNames,
    themes: BTreeMap<String, SyntaxTheme>,
}

fn fixture() -> &'static SyntaxFixture {
    static CELL: OnceLock<SyntaxFixture> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(include_str!("../fixtures/syntax.json"))
            .unwrap_or_else(|error| panic!("fixtures/syntax.json is malformed: {error}"))
    })
}

/// The palettes as the highlighter resolves them: `pierre-light`, `pierre-dark`, `t3-light`.
pub fn syntax_theme(name: &str) -> Option<&'static SyntaxTheme> {
    fixture().themes.get(name)
}

/// `DIFF_THEME_NAMES`: the palette each appearance highlights with.
pub fn syntax_theme_name(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Light => &fixture().diff_theme_names.light,
        Appearance::Dark => &fixture().diff_theme_names.dark,
    }
}

/// The palette code is highlighted with in `appearance`.
pub fn syntax_theme_for(appearance: Appearance) -> &'static SyntaxTheme {
    let name = syntax_theme_name(appearance);
    syntax_theme(name).unwrap_or_else(|| panic!("palette {name} is missing from the fixture"))
}

fn channel_luminance(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn contrast_on_light_code(rgb: [u8; 3]) -> f64 {
    let [r, g, b] = rgb.map(channel_luminance);
    (DARKEST_SURFACE_LUMINANCE + 0.05) / (0.2126 * r + 0.7152 * g + 0.0722 * b + 0.05)
}

/// `readableOnLightCode`: darkens a `#rrggbb(aa)` colour in steps that keep its hue until it
/// reads on light code. Anything that is not 6 or 8 hex digits is returned unchanged.
pub fn readable_on_light_code(color: &str, min_contrast: f64) -> String {
    let Some(hex) = color.strip_prefix('#') else {
        return color.to_owned();
    };
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return color.to_owned();
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
    let mut rgb = [channel(0), channel(2), channel(4)];
    if contrast_on_light_code(rgb) >= min_contrast {
        return color.to_owned();
    }
    while contrast_on_light_code(rgb) < min_contrast {
        rgb = rgb.map(|c| (f64::from(c) * 0.96).floor() as u8);
    }
    format!(
        "#{:02x}{:02x}{:02x}{}",
        rgb[0],
        rgb[1],
        rgb[2],
        hex.get(6..8).unwrap_or("")
    )
}

/// `readableSyntaxColor`: the `t3-light` colour for a `pierre-light` colour.
pub fn readable_syntax_color(color: &str) -> String {
    let further = FURTHER_COLORS.contains(&color.to_lowercase().as_str());
    readable_on_light_code(
        color,
        if further {
            FURTHER_MIN_CONTRAST
        } else {
            MIN_CONTRAST
        },
    )
}

/// `withReadableSyntax`: a copy of a light palette whose syntax colours read on every light surface.
pub fn with_readable_syntax(theme: &SyntaxTheme) -> SyntaxTheme {
    let mut out = theme.clone();
    out.name = READABLE_LIGHT_THEME_NAME.to_owned();
    for (_, style) in &mut out.style.syntax {
        if let Some(color) = &style.color {
            style.color = Some(readable_syntax_color(color));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_colours_are_left_alone_and_light_ones_darkened() {
        assert_eq!(readable_on_light_code("#000000", 4.5), "#000000");
        assert_eq!(readable_on_light_code("not a colour", 4.5), "not a colour");
        assert_ne!(readable_on_light_code("#ffffff", 4.5), "#ffffff");
        assert_eq!(readable_on_light_code("#ffffff80", 4.5).len(), 9);
    }
}
