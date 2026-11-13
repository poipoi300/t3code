//! The declaration table extracted from `index.css`, Tailwind's default theme and the
//! diff-surface CSS (`fixtures/tokens.json`, written by `tools/extract-tokens.mjs`).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::expr::Expr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// The `<html>` element.
    Root,
    /// An element with `data-app-sidebar`, inside `<html>`.
    Sidebar,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttrOp {
    Present,
    Absent,
    Equals,
    NotEquals,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttrTest {
    pub name: String,
    pub op: AttrOp,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlMatch {
    pub attrs: Vec<AttrTest>,
    pub classes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct When {
    #[serde(default)]
    pub dark: Option<bool>,
    #[serde(default)]
    pub min_width_rem: Option<f64>,
    #[serde(default)]
    pub wco: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Declaration {
    pub name: String,
    pub value: Expr,
    pub important: bool,
    pub layer: Option<String>,
    pub order: u32,
    pub specificity: [u32; 3],
    pub target: Target,
    pub html: HtmlMatch,
    pub when: When,
    pub origin: String,
    pub inline: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyframeStop {
    pub stops: Vec<String>,
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframes {
    pub name: String,
    pub origin: String,
    pub frames: Vec<KeyframeStop>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UtilityDeclaration {
    pub prop: String,
    pub value: Expr,
    pub variant: Option<String>,
    pub important: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Utility {
    pub name: String,
    pub declarations: Vec<UtilityDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceDeclaration {
    pub prop: String,
    pub value: Expr,
    pub important: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub selectors: Vec<String>,
    pub declarations: Vec<SurfaceDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionUse {
    pub selector: String,
    pub property: String,
    pub value: String,
    pub layer: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub nested: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZIndexUse {
    pub selector: String,
    pub value: String,
    pub layer: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub nested: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShadowLayerExpr {
    pub inset: bool,
    pub x: Expr,
    pub y: Expr,
    pub blur: Expr,
    pub spread: Expr,
    pub color: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    #[serde(rename = "indexCss")]
    pub index_css: String,
    #[serde(rename = "tailwindTheme")]
    pub tailwind_theme: String,
    #[serde(rename = "tailwindVersion")]
    pub tailwind_version: String,
}

/// A token's place in the design system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenCategory {
    TypographyFamily,
    TypographySize,
    TypographyLineHeight,
    TypographyWeight,
    TypographyLetterSpacing,
    Shadow,
    Motion,
    ZIndex,
    Breakpoint,
    Blur,
    Effect,
    Radius,
    Spacing,
    Layout,
    Appearance,
    Texture,
    StageArt,
    /// Tailwind's colour scale plus the app's `zinc-25`, `white` and `black`.
    ColorPalette,
    /// `--color-*` utility aliases of semantic colours.
    ColorAlias,
    /// `--contrast-*`: semantic colours after the contrast setting.
    ColorContrast,
    /// Semantic colours (`--background`, `--primary`, ...).
    Color,
    Border,
    Image,
    Other,
}

impl TokenCategory {
    fn parse(text: &str) -> Self {
        match text {
            "typography.family" => Self::TypographyFamily,
            "typography.size" => Self::TypographySize,
            "typography.line-height" => Self::TypographyLineHeight,
            "typography.weight" => Self::TypographyWeight,
            "typography.letter-spacing" => Self::TypographyLetterSpacing,
            "shadow" => Self::Shadow,
            "motion" => Self::Motion,
            "z-index" => Self::ZIndex,
            "breakpoint" => Self::Breakpoint,
            "blur" => Self::Blur,
            "effect" => Self::Effect,
            "radius" => Self::Radius,
            "spacing" => Self::Spacing,
            "layout" => Self::Layout,
            "appearance" => Self::Appearance,
            "texture" => Self::Texture,
            "stage-art" => Self::StageArt,
            "color.palette" => Self::ColorPalette,
            "color.alias" => Self::ColorAlias,
            "color.contrast" => Self::ColorContrast,
            "color" => Self::Color,
            "border" => Self::Border,
            "image" => Self::Image,
            _ => Self::Other,
        }
    }

    /// Whether tokens of this category are colours.
    pub const fn is_color(self) -> bool {
        matches!(
            self,
            Self::ColorPalette
                | Self::ColorAlias
                | Self::ColorContrast
                | Self::Color
                | Self::Border
                | Self::StageArt
        )
    }
}

#[derive(Deserialize)]
struct Ignored {
    rules: usize,
    selectors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTable {
    source: Source,
    layers: Vec<String>,
    declarations: Vec<Declaration>,
    categories: BTreeMap<String, String>,
    shadows: BTreeMap<String, Vec<ShadowLayerExpr>>,
    keyframes: Vec<Keyframes>,
    utilities: Vec<Utility>,
    surfaces: Vec<Surface>,
    motion: Vec<MotionUse>,
    z_index: Vec<ZIndexUse>,
    breakpoints_used_px: Vec<u32>,
    ignored: Ignored,
}

pub struct TokenTable {
    pub source: Source,
    pub layers: Vec<String>,
    pub declarations: Vec<Declaration>,
    categories: BTreeMap<String, TokenCategory>,
    pub shadows: BTreeMap<String, Vec<ShadowLayerExpr>>,
    pub keyframes: Vec<Keyframes>,
    pub utilities: Vec<Utility>,
    pub surfaces: Vec<Surface>,
    pub motion: Vec<MotionUse>,
    pub z_index: Vec<ZIndexUse>,
    pub breakpoints_used_px: Vec<u32>,
    /// Rules with custom properties whose selectors are component-scoped, not global tokens.
    pub ignored_rules: usize,
    pub ignored_selectors: Vec<String>,
}

impl TokenTable {
    /// The table embedded at build time.
    pub fn builtin() -> &'static TokenTable {
        static TABLE: OnceLock<TokenTable> = OnceLock::new();
        TABLE.get_or_init(|| {
            let raw: RawTable = serde_json::from_str(include_str!("../../fixtures/tokens.json"))
                .unwrap_or_else(|error| panic!("fixtures/tokens.json is malformed: {error}"));
            TokenTable {
                source: raw.source,
                layers: raw.layers,
                declarations: raw.declarations,
                categories: raw
                    .categories
                    .iter()
                    .map(|(n, c)| (n.clone(), TokenCategory::parse(c)))
                    .collect(),
                shadows: raw.shadows,
                keyframes: raw.keyframes,
                utilities: raw.utilities,
                surfaces: raw.surfaces,
                motion: raw.motion,
                z_index: raw.z_index,
                breakpoints_used_px: raw.breakpoints_used_px,
                ignored_rules: raw.ignored.rules,
                ignored_selectors: raw.ignored.selectors,
            }
        })
    }

    /// Every token name with at least one declaration, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.categories.keys().map(String::as_str)
    }

    pub fn category(&self, name: &str) -> Option<TokenCategory> {
        self.categories.get(name).copied()
    }

    /// Token names per category.
    pub fn counts(&self) -> BTreeMap<TokenCategory, usize> {
        let mut counts = BTreeMap::new();
        for category in self.categories.values() {
            *counts.entry(*category).or_insert(0) += 1;
        }
        counts
    }

    pub fn names_in(&self, category: TokenCategory) -> impl Iterator<Item = &str> {
        self.categories
            .iter()
            .filter(move |(_, c)| **c == category)
            .map(|(n, _)| n.as_str())
    }

    pub(crate) fn layer_rank(&self, layer: &Option<String>) -> usize {
        match layer {
            // Unlayered declarations beat every layer.
            None => self.layers.len() + 1,
            Some(name) => self.layers.iter().position(|l| l == name).unwrap_or(0),
        }
    }
}
