//! The token table's typed surface, the resolved default themes, and drift checks.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use t3_theme::color::Color;
use t3_theme::fonts::{AppearanceFontPreferences, font_variables};
use t3_theme::motion::{Repaint, looping_animations};
use t3_theme::palette::{Appearance, ThemeColorRole, built_in_theme, built_in_themes};
use t3_theme::resolved::{ThemeInput, resolve_theme};
use t3_theme::tokens::{TokenCategory, TokenTable, Value};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn sources_have_not_drifted() {
    let sources: BTreeMap<String, String> =
        serde_json::from_str(include_str!("../fixtures/sources.json")).expect("fixture parses");
    if !repo_root().join("apps/web/src/index.css").exists() {
        eprintln!("skipping: the TypeScript sources are not next to this crate");
        return;
    }
    let mut stale = Vec::new();
    for (path, expected) in &sources {
        let bytes = std::fs::read(repo_root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let digest = Sha256::digest(&bytes);
        let mut actual = String::new();
        for byte in digest {
            let _ = write!(actual, "{byte:02x}");
        }
        if &actual != expected {
            stale.push(path.as_str());
        }
    }
    assert!(
        stale.is_empty(),
        "upstream changed; run `node tools/regenerate.mjs` in rust/crates/t3-theme and review the diff:\n{}",
        stale.join("\n")
    );
}

#[test]
fn the_token_table_covers_every_category() {
    let table = TokenTable::builtin();
    let counts = table.counts();
    for category in [
        TokenCategory::ColorPalette,
        TokenCategory::Color,
        TokenCategory::ColorContrast,
        TokenCategory::Border,
        TokenCategory::StageArt,
        TokenCategory::Spacing,
        TokenCategory::Radius,
        TokenCategory::TypographyFamily,
        TokenCategory::TypographySize,
        TokenCategory::TypographyLineHeight,
        TokenCategory::TypographyWeight,
        TokenCategory::TypographyLetterSpacing,
        TokenCategory::Shadow,
        TokenCategory::Blur,
        TokenCategory::Breakpoint,
        TokenCategory::Motion,
        TokenCategory::ZIndex,
        TokenCategory::Effect,
    ] {
        assert!(
            counts.get(&category).copied().unwrap_or(0) > 0,
            "no tokens in {category:?}"
        );
    }
    assert_eq!(table.names().count(), counts.values().sum::<usize>());
    assert!(table.keyframes.len() >= 15);
    assert!(table.utilities.iter().any(|u| u.name == "surface-glass"));
}

fn standard(appearance: Appearance) -> t3_theme::resolved::ResolvedTheme {
    resolve_theme(&ThemeInput::new(None, appearance))
}

#[test]
fn stock_tokens_read_as_index_css_declares_them() {
    let light = standard(Appearance::Light);
    let dark = standard(Appearance::Dark);
    assert_eq!(light.px("--radius"), Some(8.0));
    assert_eq!(light.px("--radius-sm"), Some(4.0));
    assert_eq!(light.px("--radius-3xl"), Some(20.0));
    assert_eq!(light.px("--spacing"), Some(4.0));
    assert_eq!(light.px("--control-radius"), Some(8.0));
    assert_eq!(light.px("--workspace-gutter"), Some(12.0));
    assert_eq!(light.px("--text-2xs"), Some(11.0));
    assert_eq!(light.px("--breakpoint-sm"), Some(640.0));
    assert_eq!(light.px("--glass-blur"), Some(12.0));
    assert_eq!(dark.px("--glass-blur"), Some(16.0));
    // Appearance-specific colours: the dark canvas is true black, the light one zinc-25.
    assert_eq!(
        dark.color("--background").map(|c| c.to_hex()).as_deref(),
        Some("#000000")
    );
    assert_eq!(
        light.color("--background").map(|c| c.to_hex()).as_deref(),
        Some("#fcfcfc")
    );
    assert_eq!(
        light.color("--primary").map(|c| c.to_hex()).as_deref(),
        Some("#1b4ed8")
    );
    assert_ne!(light.color("--foreground"), dark.color("--foreground"));
    // Contrast tokens equal their source at 100% contrast.
    assert_eq!(
        light.color("--contrast-foreground").map(|c| c.to_hex()),
        light.color("--foreground").map(|c| c.to_hex())
    );
    // The sidebar subtree re-binds the surface colours.
    assert_eq!(
        dark.sidebar_color("--background")
            .map(|c| c.to_hex())
            .as_deref(),
        Some("#000000")
    );
    assert_eq!(
        dark.sidebar_color("--card").map(|c| c.to_hex()).as_deref(),
        Some("#000000")
    );
    assert_ne!(dark.color("--card"), dark.sidebar_color("--card"));
}

#[test]
fn themed_tokens_follow_the_app_theme_roles_not_the_first_definition() {
    // `index.css` declares `--background` in an early `:root` block and again, later and with
    // higher specificity, from `--app-theme-canvas`. A theme must win over the stock block.
    let grove = built_in_theme("grove").expect("grove");
    let themed = resolve_theme(&ThemeInput::new(Some(grove), Appearance::Light));
    let stock = standard(Appearance::Light);
    assert_eq!(themed.theme_id.as_deref(), Some("grove"));
    let canvas = Color::parse(grove.colors.get(ThemeColorRole::Canvas)).expect("canvas");
    assert_eq!(
        themed.color("--background").map(|c| c.to_hex()),
        Some(canvas.to_hex())
    );
    assert_ne!(themed.color("--background"), stock.color("--background"));
    let action = Color::parse(grove.colors.get(ThemeColorRole::MessageAction)).expect("action");
    assert_eq!(
        themed.color("--primary").map(|c| c.to_hex()),
        Some(action.to_hex())
    );
    assert_eq!(
        themed.color("--ring").map(|c| c.to_hex()),
        Color::parse(grove.colors.get(ThemeColorRole::Focus)).map(|c| c.to_hex())
    );
}

#[test]
fn contrast_boost_moves_foregrounds_toward_the_target() {
    let normal = standard(Appearance::Dark);
    let mut input = ThemeInput::new(None, Appearance::Dark);
    input.contrast = 150;
    let boosted = resolve_theme(&input);
    let luminance = |c: Color| c.to_oklab()[0];
    let before = luminance(normal.color("--contrast-muted-foreground").expect("muted"));
    let after = luminance(boosted.color("--contrast-muted-foreground").expect("muted"));
    assert!(
        after > before,
        "dark mode pushes foregrounds toward white: {before} -> {after}"
    );
}

#[test]
fn font_settings_scale_the_root_and_the_composer() {
    let preferences = AppearanceFontPreferences {
        sans: String::new(),
        code: String::new(),
        composer: String::new(),
        size_interface: 20.0,
        size_prompt: 14.0,
        size_code: 13.0,
        smoothing: true,
        separate: false,
    };
    let fonts = font_variables(&preferences, None);
    let mut input = ThemeInput::new(None, Appearance::Light);
    input.fonts = Some(&fonts);
    let theme = resolve_theme(&input);
    // `text-sm` is 0.875rem, so a 20px root gives the composer 17.5px; the touch size keeps
    // the same value because it is only floored at 16px.
    assert_eq!(theme.px("--font-size-prompt"), Some(17.5));
    assert_eq!(theme.px("--font-size-prompt-touch"), Some(17.5));
    assert_eq!(
        theme.px("--radius"),
        Some(10.0),
        "rem tokens follow the interface size"
    );
    let separate = font_variables(
        &AppearanceFontPreferences {
            separate: true,
            ..preferences
        },
        None,
    );
    let mut input = ThemeInput::new(None, Appearance::Light);
    input.fonts = Some(&separate);
    let theme = resolve_theme(&input);
    assert_eq!(theme.px("--font-size-prompt"), Some(14.0));
    assert_eq!(theme.px("--font-size-prompt-touch"), Some(16.0));
}

#[test]
fn nothing_the_app_declares_repaints_continuously() {
    let table = TokenTable::builtin();
    let looping = looping_animations(table);
    assert!(!looping.is_empty());
    for animation in looping.iter().filter(|a| a.origin == "index.css") {
        assert_ne!(
            animation.repaint,
            Repaint::Continuous,
            "{} loops smoothly",
            animation.token
        );
    }
    // Tailwind's own spinner-style tokens are the only smooth loops among the tokens.
    let smooth: Vec<&str> = looping
        .iter()
        .filter(|a| a.repaint == Repaint::Continuous)
        .map(|a| a.token.as_str())
        .collect();
    for token in &smooth {
        assert!(token.starts_with("--animate-"), "{token}");
    }
    assert!(
        looping
            .iter()
            .any(|a| a.token == "--animate-status-pulse" && a.repaint == Repaint::Stepped)
    );
    assert!(
        looping
            .iter()
            .any(|a| a.token == "--animate-skeleton" && a.repaint == Repaint::Stepped)
    );
}

fn export_of(appearance: Appearance) -> serde_json::Value {
    serde_json::to_value(standard(appearance).export()).expect("serialises")
}

#[test]
fn the_default_themes_export_matches_the_checked_in_fixture() {
    let current = serde_json::json!({ "light": export_of(Appearance::Light), "dark": export_of(Appearance::Dark) });
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/resolved-default-themes.json");
    if std::env::var_os("T3_THEME_UPDATE_FIXTURES").is_some() {
        // Refreshing: write and stop; the next run compares.
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&current).expect("json")),
        )
        .expect("write fixture");
    }
    let checked_in: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/resolved-default-themes.json"))
            .expect("fixture parses");
    assert_eq!(
        current, checked_in,
        "run with T3_THEME_UPDATE_FIXTURES=1 to refresh the fixture"
    );
}

#[test]
fn every_builtin_theme_resolves_in_every_mode() {
    for theme in built_in_themes() {
        for mode in theme.modes() {
            let resolved = resolve_theme(&ThemeInput::new(Some(theme), mode));
            assert_eq!(resolved.appearance, mode);
            assert!(
                resolved.color("--background").is_some(),
                "{} {mode:?}",
                theme.id
            );
            assert!(
                resolved
                    .tokens
                    .root
                    .values()
                    .all(|v| !matches!(v, Value::Invalid))
            );
        }
    }
}

#[test]
fn shadows_and_glass_effects_resolve_to_typed_values() {
    use t3_theme::tokens::effects::{resolve_shadow, resolve_utility};
    let table = TokenTable::builtin();
    let light = standard(Appearance::Light);
    let dark = standard(Appearance::Dark);

    let composer = resolve_shadow(table, "--shadow-composer", &light.tokens.root, false, 16.0)
        .expect("composer");
    assert_eq!(composer.len(), 1);
    assert_eq!(
        (
            composer[0].x,
            composer[0].y,
            composer[0].blur,
            composer[0].spread
        ),
        (0.0, 1.0, 2.0, 0.0)
    );
    assert!((composer[0].color.expect("colour").alpha - 0.05).abs() < 1e-9);
    let medium = resolve_shadow(table, "--shadow-md", &light.tokens.root, false, 16.0).expect("md");
    assert_eq!(medium.len(), 2);
    assert_eq!(
        (medium[0].y, medium[0].blur, medium[0].spread),
        (4.0, 6.0, -1.0)
    );
    let inset = resolve_shadow(
        table,
        "--inset-shadow-control-highlight",
        &light.tokens.root,
        false,
        16.0,
    )
    .expect("inset");
    assert!(inset[0].inset);

    let glass = |theme: &t3_theme::resolved::ResolvedTheme, dark: bool| {
        resolve_utility(table, "surface-glass", &theme.tokens.root, dark, 16.0)
            .expect("surface-glass")
    };
    let light_glass = glass(&light, false);
    let backdrop = light_glass
        .iter()
        .find(|d| d.prop == "backdrop-filter")
        .expect("backdrop filter");
    let Value::List { items, .. } = &backdrop.value else {
        panic!("filters: {:?}", backdrop.value)
    };
    assert_eq!(items.len(), 2);
    assert!(
        matches!(&items[0], Value::Filter { name, arg } if name == "blur" && arg.px(16.0) == Some(12.0))
    );
    assert!(
        matches!(&items[1], Value::Filter { name, arg } if name == "saturate" && arg.number() == Some(1.14))
    );
    let dark_glass = glass(&dark, true);
    let dark_backdrop = dark_glass
        .iter()
        .find(|d| d.prop == "backdrop-filter")
        .expect("backdrop filter");
    assert!(
        matches!(&dark_backdrop.value, Value::List { items, .. } if matches!(&items[0], Value::Filter { arg, .. } if arg.px(16.0) == Some(16.0)))
    );
    // The glass background is the canvas at the glass opacity.
    let background = light_glass
        .iter()
        .find(|d| d.prop == "background")
        .and_then(|d| d.value.color())
        .expect("background");
    assert!((background.alpha - 0.8).abs() < 1e-9);
    // Dark dialogs add an inset highlight on top of the drop shadow.
    let dialog = resolve_utility(table, "dialog-glass", &dark.tokens.root, true, 16.0)
        .expect("dialog-glass");
    assert!(
        dialog
            .iter()
            .any(|d| d.prop == "box-shadow" && d.variant.as_deref() == Some("dark"))
    );
}

#[test]
fn every_looping_animation_in_the_stylesheet_is_stepped_or_visibility_gated() {
    use t3_theme::motion::looping_animation_uses;
    let uses = looping_animation_uses(TokenTable::builtin());
    let names: Vec<&str> = uses.iter().map(|u| u.source.as_str()).collect();
    for expected in [
        "@utility live-tool-shine",
        "@utility live-activity-focus",
        "@utility live-activity-focus-counter",
        "@utility visible-animate-spin",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} not found in {names:?}"
        );
    }
    for animation in &uses {
        assert!(
            animation.repaint != Repaint::Continuous || animation.visibility_gated,
            "{} repaints continuously and is not gated by visibility",
            animation.source
        );
    }
    let shine = uses
        .iter()
        .find(|u| u.source == "@utility live-tool-shine")
        .expect("shine");
    assert_eq!(shine.repaint, Repaint::Stepped);
    let spin = uses
        .iter()
        .find(|u| u.source == "@utility visible-animate-spin")
        .expect("spin");
    assert_eq!(
        (spin.repaint, spin.visibility_gated),
        (Repaint::Continuous, true)
    );
}
