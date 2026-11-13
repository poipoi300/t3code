//! T3 design tokens for NSH.
//!
//! `t3-theme` resolves the product's token table for the light and the dark appearance; this
//! module pairs every token into an NSH [`AdaptiveColor`], so a light/dark switch repaints
//! natively without a render or a re-resolve. Components read [`palette`] and [`metrics`].

mod classes;
mod metrics;
mod palette;

use std::sync::Arc;

use nsh_core::prelude::*;
use t3_theme::color::Color;
use t3_theme::palette::{
    Appearance as ThemeAppearance, AppearanceMode, DesktopTheme, ThemeCatalog, ThemeHalves,
};
use t3_theme::resolved::{ResolvedTheme, ThemeInput, resolve_theme};
use t3_theme::tokens::Value;

pub use classes::{LIGHT_COLOR_PREFIX, SIDEBAR_COLOR_PREFIX, Scope, cn, tw, tw_with};
pub use metrics::{Density, Metrics, metrics};
pub use palette::{Palette, palette};

/// What the user chose. The resolver does the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeChoice {
    /// A catalogue theme id; empty is the stock look.
    pub theme: String,
    pub mode: AppearanceMode,
    pub halves: Option<ThemeHalves>,
    pub contrast: u32,
}

impl Default for ThemeChoice {
    fn default() -> Self {
        Self {
            theme: String::new(),
            mode: AppearanceMode::System,
            halves: None,
            contrast: t3_theme::resolved::DEFAULT_APPEARANCE_CONTRAST,
        }
    }
}

/// Installs `choice`: the paired tokens, and the appearance NSH shows.
pub fn install(choice: &ThemeChoice) {
    set_tokens(tokens_for(choice));
    set_color_scheme(scheme_for(choice));
}

/// The appearance NSH shows: the system's while the theme has both halves and the user follows
/// the system, else the one the theme or the user forces.
pub fn scheme_for(choice: &ThemeChoice) -> Appearance {
    let catalog = ThemeCatalog {
        custom: &[],
        environment: &[],
    };
    match catalog.resolve_desktop_theme(
        &choice.theme,
        Some(choice.mode == AppearanceMode::System),
        Some(choice.mode),
        choice.halves.as_ref(),
    ) {
        DesktopTheme::System => Appearance::System,
        DesktopTheme::Light => Appearance::Light,
        DesktopTheme::Dark => Appearance::Dark,
    }
}

/// The NSH tokens of `choice`, both appearances paired.
pub fn tokens_for(choice: &ThemeChoice) -> Tokens {
    let catalog = ThemeCatalog {
        custom: &[],
        environment: &[],
    };
    let resolve = |appearance: ThemeAppearance| {
        let id = choice
            .halves
            .as_ref()
            .and_then(|halves| halves.get(appearance))
            .unwrap_or(&choice.theme);
        let mut input = ThemeInput::new(catalog.get(id), appearance);
        input.contrast = choice.contrast;
        resolve_theme(&input)
    };
    pair(
        &resolve(ThemeAppearance::Light),
        &resolve(ThemeAppearance::Dark),
    )
}

/// NSH tokens named as the web's classes name them: `bg-X` paints `--color-X` (which carries the
/// contrast setting), a sidebar-redeclared colour is also `sb-X`, every colour is also `light-X`
/// (clear in dark, for `light:`), and lengths are points.
fn pair(light: &ResolvedTheme, dark: &ResolvedTheme) -> Tokens {
    let mut tokens = Tokens::default();
    for (scope, light_values, dark_values) in [
        ("", &light.tokens.root, &dark.tokens.root),
        (
            SIDEBAR_COLOR_PREFIX,
            &light.tokens.sidebar,
            &dark.tokens.sidebar,
        ),
    ] {
        // Plain names first, so a `--color-X` alias overrides `--X` under the class name `X`.
        let mut names: Vec<_> = light_values.keys().collect();
        names.sort_by_key(|name| name.starts_with("--color-"));
        for name in names {
            let Some(key) = name.strip_prefix("--") else {
                continue;
            };
            match (&light_values[name], dark_values.get(name)) {
                (Value::Color { color: l }, Some(Value::Color { color: d })) => {
                    let color = AdaptiveColor::new(hsla_of(l), hsla_of(d));
                    if scope.is_empty() {
                        tokens.set_color(key, color);
                    }
                    let class = key.strip_prefix("color-").unwrap_or(key);
                    tokens.set_color(&format!("{scope}{class}"), color);
                }
                (value, _) if scope.is_empty() => {
                    if let Some(px) = value.px(light.root_font_px) {
                        tokens.set_number(key, px as f32);
                    }
                }
                _ => {}
            }
        }
    }
    let light_only: Vec<_> = tokens
        .colors()
        .filter(|(name, _)| !name.starts_with("color-"))
        .map(|(name, color)| {
            let clear = AdaptiveColor::TRANSPARENT.dark;
            (
                format!("{LIGHT_COLOR_PREFIX}{name}"),
                AdaptiveColor::new(color.light, clear),
            )
        })
        .collect();
    for (name, color) in light_only {
        tokens.set_color(&name, color);
    }
    if let Some(radius) = light.px("--control-radius") {
        tokens.set_number("radius", radius as f32);
    }
    tokens.set_font(nsh_core::element::FONT_SANS, INTER.family);
    tokens
}

/// Typed styles for every `TextView` (markdown) under `App`: root text colour, which every part
/// inherits, and the code surface. Without the root colour, code text takes the platform default.
pub fn text_view_parts() -> nsh_base::text_view::TextViewParts {
    use nsh_base::text_view::{TextViewPart, TextViewParts};
    TextViewParts::new()
        .part(TextViewPart::Root, |s| s.text_color(palette().foreground))
        .part(TextViewPart::CodeBlock, |s| {
            s.bg(palette().code_background)
                .border_color(palette().border)
        })
        .part(TextViewPart::CodeBlockCode, |s| {
            s.text_color(palette().code_foreground)
        })
}

/// The bundled Inter, byte-identical to the web app's `@fontsource-variable/inter` file.
pub const INTER: FontAsset = asset!("fonts/inter-latin-wght-normal.woff2");

pub fn hsla_of(color: &Color) -> Hsla {
    let [r, g, b, a] = color.to_rgba8();
    rgba(u32::from_be_bytes([r, g, b, a])).into()
}

/// The shared token handle the palette caches on.
pub(crate) fn current_tokens() -> Arc<Tokens> {
    tokens()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(color: AdaptiveColor) -> (u32, u32) {
        let pack = |c: Hsla| {
            let Rgba { r, g, b, a } = c.into();
            let byte = |v: f32| (v * 255.0).round() as u32;
            byte(r) << 24 | byte(g) << 16 | byte(b) << 8 | byte(a)
        };
        (pack(color.light), pack(color.dark))
    }

    #[test]
    fn stock_tokens_pair_each_appearance_from_its_own_resolution() {
        let tokens = tokens_for(&ThemeChoice::default());
        let background = tokens.color("background").expect("background");
        assert_eq!(hex(background), (0xfcfcfcff, 0x000000ff));
        let primary = tokens.color("primary").expect("primary");
        assert_eq!(hex(primary), (0x1b4ed8ff, 0x346bf1ff));
    }

    #[test]
    fn sidebar_scoped_values_differ_from_root_where_the_stylesheet_redeclares_them() {
        let tokens = tokens_for(&ThemeChoice::default());
        let root = tokens.color("sidebar").expect("root sidebar");
        let scoped = tokens.color("sb-sidebar").expect("sidebar-scoped sidebar");
        assert_eq!(hex(root).1, 0x111111ff);
        assert_eq!(hex(scoped).1, 0x000000ff);
        assert_eq!(hex(scoped).0, hex(root).0);
    }

    #[test]
    fn lengths_become_points_at_the_root_size() {
        let tokens = tokens_for(&ThemeChoice::default());
        assert_eq!(tokens.number("control-radius"), Some(8.0));
        assert_eq!(tokens.number("radius"), Some(8.0));
        assert_eq!(tokens.number("sidebar-row-content-inset"), Some(10.0));
    }

    #[test]
    fn scheme_follows_the_system_only_when_asked_and_forces_otherwise() {
        assert_eq!(scheme_for(&ThemeChoice::default()), Appearance::System);
        let dark = ThemeChoice {
            mode: AppearanceMode::Dark,
            ..ThemeChoice::default()
        };
        assert_eq!(scheme_for(&dark), Appearance::Dark);
        let light = ThemeChoice {
            mode: AppearanceMode::Light,
            ..ThemeChoice::default()
        };
        assert_eq!(scheme_for(&light), Appearance::Light);
    }

    #[test]
    fn contrast_reaches_the_contrast_tokens_and_leaves_roles_alone() {
        let stock = tokens_for(&ThemeChoice::default());
        let high = tokens_for(&ThemeChoice {
            contrast: 200,
            ..ThemeChoice::default()
        });
        let pick = |t: &Tokens, name: &str| hex(t.color(name).expect(name));
        assert_ne!(
            pick(&stock, "contrast-muted-foreground"),
            pick(&high, "contrast-muted-foreground")
        );
        assert_eq!(
            pick(&high, "contrast-muted-foreground"),
            (0x000000ff, 0xffffffff)
        );
        // What `text-muted-foreground` paints: Tailwind's `--color-muted-foreground`, which
        // follows contrast (so `(--muted-foreground)` reads it too; equal at the default).
        assert_eq!(
            pick(&stock, "muted-foreground"),
            pick(&stock, "color-muted-foreground")
        );
        assert_ne!(
            pick(&stock, "muted-foreground"),
            pick(&high, "muted-foreground")
        );
    }
}
