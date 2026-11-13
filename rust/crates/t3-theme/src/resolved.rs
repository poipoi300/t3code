//! A resolved theme: every token for the active appearance, ready for a UI layer to read.
//!
//! This is what `applyThemePalette`, `applyAppearanceContrast` and
//! `applyAppearanceFontVariables` do to `<html>`, followed by the browser's cascade, as data.
//! A UI layer applies a resolved theme by reading [`ResolvedTheme::tokens`] (and the code
//! surface and syntax palette) instead of setting CSS variables.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::color::Color;
use crate::fonts::{FontVariables, PromptFontSize};
use crate::palette::{
    Appearance, ThemeColorRole, ThemeColors, ThemeDefinition, context_chip_variables,
    standard_painted_theme_colors,
};
use crate::syntax::{SyntaxTheme, syntax_theme_for};
use crate::tokens::cascade::{Computed, StyleContext, compute};
use crate::tokens::expr::Value;
use crate::tokens::surface::{CodeSurface, code_surface};
use crate::tokens::table::TokenTable;

pub const MIN_APPEARANCE_CONTRAST: u32 = 50;
pub const MAX_APPEARANCE_CONTRAST: u32 = 200;
pub const DEFAULT_APPEARANCE_CONTRAST: u32 = 100;

/// `applyAppearanceContrast`: the three `--appearance-contrast-*` percentages.
pub fn appearance_contrast_values(contrast: u32) -> [(&'static str, f64); 3] {
    let contrast = f64::from(contrast);
    let boost = (contrast - 100.0).max(0.0);
    [
        ("--appearance-contrast-base", contrast.min(100.0)),
        ("--appearance-contrast-boost", boost),
        ("--appearance-contrast-border-boost", boost / 4.0),
    ]
}

/// What to resolve.
#[derive(Clone, Debug)]
pub struct ThemeInput<'a> {
    /// The selected palette, or `None` for the stock T3 Code look.
    pub theme: Option<&'a ThemeDefinition>,
    pub appearance: Appearance,
    pub contrast: u32,
    pub fonts: Option<&'a FontVariables>,
    pub diff_color_scheme: Option<&'a str>,
    pub chat_width: Option<&'a str>,
    /// Window Controls Overlay is active (desktop with native controls).
    pub wco: bool,
    pub viewport_rem: Option<f64>,
}

impl<'a> ThemeInput<'a> {
    pub fn new(theme: Option<&'a ThemeDefinition>, appearance: Appearance) -> Self {
        Self {
            theme,
            appearance,
            contrast: DEFAULT_APPEARANCE_CONTRAST,
            fonts: None,
            diff_color_scheme: None,
            chat_width: None,
            wco: false,
            viewport_rem: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedTheme {
    pub appearance: Appearance,
    /// `data-theme-id`; `None` for the stock look.
    pub theme_id: Option<String>,
    /// The role colours in effect (the stock palette as `index.css` paints it, or the theme's).
    pub roles: ThemeColors,
    /// Computed custom properties for `<html>` and for `[data-app-sidebar]`.
    pub tokens: Computed,
    /// The palette code is highlighted with.
    pub syntax: &'static SyntaxTheme,
    /// The `--diffs-*` values on diff and file containers.
    pub code_surface: CodeSurface,
    /// `--context-chip-*`: lightness-solved chip colours (`#rrggbb`).
    pub chips: Vec<(String, String)>,
    pub root_font_px: f64,
}

fn parse_color_value(text: &str) -> Option<Value> {
    Color::parse(text).map(|color| Value::Color { color })
}

/// Resolves a theme with the built-in token table.
pub fn resolve_theme(input: &ThemeInput<'_>) -> ResolvedTheme {
    resolve_theme_with(TokenTable::builtin(), input)
}

pub fn resolve_theme_with(table: &TokenTable, input: &ThemeInput<'_>) -> ResolvedTheme {
    let mut inline: BTreeMap<String, Value> = BTreeMap::new();
    let (theme_id, roles) = match input.theme {
        Some(theme) => {
            let colors = theme
                .colors_for(input.appearance)
                .unwrap_or(&theme.colors)
                .clone();
            (Some(theme.id.clone()), colors)
        }
        None => (None, standard_painted_theme_colors(input.appearance)),
    };
    if theme_id.is_some() {
        for role in ThemeColorRole::ALL {
            if let Some(value) = parse_color_value(roles.get(role)) {
                inline.insert(role.css_variable().to_owned(), value);
            }
        }
    }
    let chips = context_chip_variables(&roles);
    for (name, hex) in &chips {
        if let Some(value) = parse_color_value(hex) {
            inline.insert(name.clone(), value);
        }
    }
    for (name, percent) in appearance_contrast_values(input.contrast) {
        inline.insert(name.to_owned(), Value::Percent { v: percent });
    }
    let mut root_font_px = 16.0;
    if let Some(fonts) = input.fonts {
        root_font_px = f64::from(fonts.root_font_size_px);
        let mut text = |name: &str, value: &Option<String>| {
            if let Some(value) = value {
                inline.insert(
                    name.to_owned(),
                    Value::Raw {
                        text: value.clone(),
                    },
                );
            }
        };
        text("--font-sans", &fonts.font_sans);
        text("--font-mono", &fonts.font_mono);
        text("--font-composer", &fonts.font_composer);
        text("--list-marker-sans", &fonts.list_marker_sans_em);
        let px = |px: u32| Value::Length {
            px: f64::from(px),
            rem: 0.0,
            vw: 0.0,
        };
        inline.insert(
            "--font-size-prompt".to_owned(),
            match fonts.font_size_prompt {
                PromptFontSize::FollowsInterface => Value::Length {
                    px: 0.0,
                    rem: 0.875,
                    vw: 0.0,
                },
                PromptFontSize::Px(size) => px(size),
            },
        );
        inline.insert("--font-size-code".to_owned(), px(fonts.code_font_size_px));
        inline.insert("--diffs-font-size".to_owned(), px(fonts.code_font_size_px));
    }
    let mut ctx = StyleContext::new(input.appearance == Appearance::Dark, theme_id.as_deref());
    ctx.diff_color_scheme = input.diff_color_scheme.map(str::to_owned);
    ctx.chat_width = input.chat_width.map(str::to_owned);
    ctx.wco = input.wco;
    ctx.viewport_rem = input.viewport_rem;
    ctx.root_px = root_font_px;
    let tokens = compute(table, &ctx, &inline);
    let surface = code_surface(table, input.appearance, &tokens.root, root_font_px);
    ResolvedTheme {
        appearance: input.appearance,
        theme_id,
        roles,
        tokens,
        syntax: syntax_theme_for(input.appearance),
        code_surface: surface,
        chips,
        root_font_px,
    }
}

impl ResolvedTheme {
    /// A computed colour token on `<html>` (`--background`, `--primary`, ...).
    pub fn color(&self, name: &str) -> Option<Color> {
        self.tokens.root.get(name).and_then(Value::color)
    }

    /// A computed colour token inside the sidebar.
    pub fn sidebar_color(&self, name: &str) -> Option<Color> {
        self.tokens.sidebar.get(name).and_then(Value::color)
    }

    /// A computed length token in pixels at this theme's root font size.
    pub fn px(&self, name: &str) -> Option<f64> {
        self.tokens
            .root
            .get(name)
            .and_then(|v| v.px(self.root_font_px))
    }
}

/// A value in the machine-readable export.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ExportValue {
    Color(ExportColor),
    Other(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExportColor {
    pub t: &'static str,
    pub css: String,
    pub hex: String,
    pub rgba: [u8; 4],
}

fn export_value(value: &Value) -> ExportValue {
    match value {
        Value::Color { color } => ExportValue::Color(ExportColor {
            t: "color",
            css: color.to_css(),
            hex: color.to_hex(),
            rgba: color.to_rgba8(),
        }),
        other => ExportValue::Other(other.clone()),
    }
}

/// The machine-readable form of a resolved theme, for visual tests of a UI layer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeExport {
    pub appearance: Appearance,
    pub theme_id: Option<String>,
    pub root_font_px: f64,
    pub roles: BTreeMap<&'static str, String>,
    pub tokens: BTreeMap<String, ExportValue>,
    /// Only the tokens whose value differs inside `[data-app-sidebar]`.
    pub sidebar_overrides: BTreeMap<String, ExportValue>,
    pub syntax_theme: String,
    pub code_surface: BTreeMap<String, ExportValue>,
    pub chips: BTreeMap<String, String>,
}

impl ResolvedTheme {
    pub fn export(&self) -> ThemeExport {
        let tokens = self
            .tokens
            .root
            .iter()
            .map(|(k, v)| (k.clone(), export_value(v)))
            .collect();
        let sidebar_overrides = self
            .tokens
            .sidebar
            .iter()
            .filter(|(k, v)| self.tokens.root.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), export_value(v)))
            .collect();
        ThemeExport {
            appearance: self.appearance,
            theme_id: self.theme_id.clone(),
            root_font_px: self.root_font_px,
            roles: ThemeColorRole::ALL
                .iter()
                .map(|r| (r.name(), self.roles.get(*r).to_owned()))
                .collect(),
            tokens,
            sidebar_overrides,
            syntax_theme: self.syntax.name.clone(),
            code_surface: self
                .code_surface
                .iter()
                .map(|(k, v)| (k.to_owned(), export_value(v)))
                .collect(),
            chips: self.chips.iter().cloned().collect(),
        }
    }
}
