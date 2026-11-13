//! Small design-system data the UI layer reads: project icon colours, the theme picker's
//! preview tiles, and the file tree's colour variables.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::palette::Appearance;
use crate::tokens::expr::{Env, Expr, Lookup, Value, eval};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IconRow {
    value: String,
    label: String,
    class_name: String,
    swatch_class_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewFixture {
    standard_colors: BTreeMap<String, PreviewColors>,
    render_specs: BTreeMap<String, PreviewRenderSpec>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeFixture {
    host_variables: Vec<TreeVariable>,
    styles: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct TreeVariable {
    name: String,
    value: Expr,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Misc {
    project_icon_colors: Vec<IconRow>,
    theme_preview: PreviewFixture,
    file_tree: TreeFixture,
}

fn misc() -> &'static Misc {
    static CELL: OnceLock<Misc> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(include_str!("../fixtures/misc.json"))
            .unwrap_or_else(|error| panic!("fixtures/misc.json is malformed: {error}"))
    })
}

// ------------------------------------------------------------------ project icon colours

/// A colour a project's icon can take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProjectIconColor {
    pub value: String,
    pub label: String,
    /// Token for the icon on light surfaces (for example `--color-gray-600`).
    pub light_token: String,
    /// Token for the icon on dark surfaces (`--color-gray-400`).
    pub dark_token: String,
    /// Token for the colour picker swatch (`--color-gray-500`).
    pub swatch_token: String,
}

fn class_token(class: &str) -> Option<String> {
    let colour = class.rsplit(':').next()?;
    let (_, name) = colour.split_once('-')?;
    Some(format!("--color-{name}"))
}

/// `PROJECT_ICON_COLORS`, with the Tailwind classes turned into palette tokens.
pub fn project_icon_colors() -> &'static [ProjectIconColor] {
    static CELL: OnceLock<Vec<ProjectIconColor>> = OnceLock::new();
    CELL.get_or_init(|| {
        misc()
            .project_icon_colors
            .iter()
            .filter_map(|row| {
                let mut light = None;
                let mut dark = None;
                for class in row.class_name.split_whitespace() {
                    if class.starts_with("dark:") {
                        dark = class_token(class);
                    } else {
                        light = class_token(class);
                    }
                }
                Some(ProjectIconColor {
                    value: row.value.clone(),
                    label: row.label.clone(),
                    light_token: light?,
                    dark_token: dark?,
                    swatch_token: class_token(&row.swatch_class_name)?,
                })
            })
            .collect()
    })
}

impl ProjectIconColor {
    /// The token to paint the icon with in `appearance`.
    pub fn token(&self, appearance: Appearance) -> &str {
        match appearance {
            Appearance::Light => &self.light_token,
            Appearance::Dark => &self.dark_token,
        }
    }
}

// ------------------------------------------------------------------ theme preview tiles

/// The three colours a preview tile is drawn from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewColors {
    pub canvas: String,
    pub accent: String,
    pub message_action: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccentGlow {
    pub center: [f64; 2],
    pub middle_offset: f64,
    pub middle_opacity: f64,
    pub end_offset: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionGlow {
    pub center: [f64; 2],
    pub start_opacity: f64,
    pub end_offset: f64,
}

/// How a preview tile is composed: a canvas mixed toward `base_target`, an accent glow and an
/// action glow, scaled and blurred.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRenderSpec {
    pub base_target: String,
    pub base_weight: f64,
    pub accent: AccentGlow,
    pub action: ActionGlow,
    pub scale: f64,
    pub blur_at56_px: f64,
}

fn appearance_key(appearance: Appearance) -> &'static str {
    appearance.as_str()
}

/// `STANDARD_THEME_PREVIEW_COLORS`.
pub fn standard_preview_colors(appearance: Appearance) -> Option<&'static PreviewColors> {
    misc()
        .theme_preview
        .standard_colors
        .get(appearance_key(appearance))
}

/// `THEME_PREVIEW_RENDER_SPECS`.
pub fn preview_render_spec(appearance: Appearance) -> Option<&'static PreviewRenderSpec> {
    misc()
        .theme_preview
        .render_specs
        .get(appearance_key(appearance))
}

#[derive(Clone, Copy)]
struct Oklab {
    l: f64,
    a: f64,
    b: f64,
}

fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f64) -> u8 {
    let converted = if value <= 0.0031308 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (converted.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

/// `parseOklab` of the preview module: `oklch(l c h ...)` or `#rrggbb`.
fn parse_oklab(value: &str) -> Option<Oklab> {
    if let Some(body) = value.strip_prefix("oklch(") {
        let mut parts = body
            .split(|c: char| c.is_whitespace() || c == ')' || c == '/')
            .filter(|p| !p.is_empty());
        let l: f64 = parts.next()?.parse().ok()?;
        let c: f64 = parts.next()?.parse().ok()?;
        let h = parts.next()?.parse::<f64>().ok()?.to_radians();
        return Some(Oklab {
            l,
            a: c * h.cos(),
            b: c * h.sin(),
        });
    }
    let hex = value
        .strip_prefix('#')
        .filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let channel = |at: usize| {
        u8::from_str_radix(&hex[at..at + 2], 16).map(|v| srgb_to_linear(f64::from(v) / 255.0))
    };
    let (red, green, blue) = (channel(0).ok()?, channel(2).ok()?, channel(4).ok()?);
    let l = (0.4122214708 * red + 0.5363325363 * green + 0.0514459929 * blue).cbrt();
    let m = (0.2119034982 * red + 0.6806995451 * green + 0.1073969566 * blue).cbrt();
    let s = (0.0883024619 * red + 0.2817188376 * green + 0.6299787005 * blue).cbrt();
    Some(Oklab {
        l: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        a: 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        b: 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    })
}

fn oklab_to_hex(color: Oklab) -> String {
    let l = (color.l + 0.3963377774 * color.a + 0.2158037573 * color.b).powi(3);
    let m = (color.l - 0.1055613458 * color.a - 0.0638541728 * color.b).powi(3);
    let s = (color.l - 0.0894841775 * color.a - 1.291485548 * color.b).powi(3);
    let r = linear_to_srgb(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s);
    let g = linear_to_srgb(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s);
    let b = linear_to_srgb(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `mixThemePreviewBase`: the tile's base colour, the canvas mixed toward the mode's target.
pub fn mix_theme_preview_base(colors: &PreviewColors, mode: Appearance) -> String {
    let Some(spec) = preview_render_spec(mode) else {
        return colors.canvas.clone();
    };
    let (Some(canvas), Some(target)) =
        (parse_oklab(&colors.canvas), parse_oklab(&spec.base_target))
    else {
        return colors.canvas.clone();
    };
    let target_weight = 1.0 - spec.base_weight;
    oklab_to_hex(Oklab {
        l: canvas.l * spec.base_weight + target.l * target_weight,
        a: canvas.a * spec.base_weight + target.a * target_weight,
        b: canvas.b * spec.base_weight + target.b * target_weight,
    })
}

// ------------------------------------------------------------------ file tree

/// The custom property the file tree takes its text colour from (`pierreTreeStyle`).
pub fn tree_foreground_override(appearance: Appearance) -> Option<(&'static str, &'static str)> {
    misc()
        .file_tree
        .styles
        .get(appearance_key(appearance))
        .and_then(|style| {
            style
                .get("--trees-fg-override")
                .map(|value| ("--trees-fg-override", value.as_str()))
        })
}

fn substitute_current_color(expr: &Expr, color: Color) -> Expr {
    let again = |e: &Expr| substitute_current_color(e, color);
    match expr {
        Expr::CurrentColor => Expr::Color {
            c: color,
            name: None,
        },
        Expr::Mix {
            space,
            a,
            pa,
            b,
            pb,
        } => Expr::Mix {
            space: *space,
            a: Box::new(again(a)),
            pa: pa.clone(),
            b: Box::new(again(b)),
            pb: pb.clone(),
        },
        other => other.clone(),
    }
}

struct NoVariables;

impl Env for NoVariables {
    fn lookup(&self, _name: &str) -> Lookup {
        Lookup::Missing
    }

    fn dark(&self) -> bool {
        false
    }

    fn root_px(&self) -> f64 {
        16.0
    }
}

/// The `:host` variables of the file tree's stylesheet with `currentColor` bound to
/// `foreground`: selected, hover and border tints mixed from the text colour.
pub fn tree_host_variables(foreground: Color) -> Vec<(String, Value)> {
    misc()
        .file_tree
        .host_variables
        .iter()
        .map(|variable| {
            (
                variable.name.clone(),
                eval(
                    &substitute_current_color(&variable.value, foreground),
                    &NoVariables,
                ),
            )
        })
        .collect()
}
