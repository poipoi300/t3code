//! Preview tiles, project icon colours and the file tree against the TypeScript
//! (fixtures/misc.json, written by tools/extract-misc.mjs).

use serde::Deserialize;
use t3_theme::color::Color;
use t3_theme::palette::Appearance;
use t3_theme::resolved::{ThemeInput, resolve_theme};
use t3_theme::ui_data::{
    PreviewColors, mix_theme_preview_base, preview_render_spec, project_icon_colors,
    standard_preview_colors, tree_foreground_override, tree_host_variables,
};

#[derive(Deserialize)]
struct Case {
    colors: PreviewColors,
    mode: Appearance,
    out: String,
}

#[derive(Deserialize)]
struct Theme {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Misc {
    #[serde(rename = "themePreview")]
    theme_preview: Theme,
}

#[test]
fn preview_base_mix_matches_typescript() {
    let misc: Misc = serde_json::from_str(include_str!("../fixtures/misc.json")).expect("fixture");
    assert!(misc.theme_preview.cases.len() > 80);
    for case in misc.theme_preview.cases {
        assert_eq!(
            mix_theme_preview_base(&case.colors, case.mode),
            case.out,
            "{:?} {:?}",
            case.mode,
            case.colors
        );
    }
    for mode in Appearance::ALL {
        assert!(standard_preview_colors(mode).is_some() && preview_render_spec(mode).is_some());
    }
}

#[test]
fn project_icon_colours_resolve_to_palette_tokens() {
    let colors = project_icon_colors();
    assert_eq!(colors.len(), 18);
    let gray = &colors[0];
    assert_eq!(
        (
            gray.value.as_str(),
            gray.light_token.as_str(),
            gray.dark_token.as_str()
        ),
        ("gray", "--color-gray-600", "--color-gray-400")
    );
    assert_eq!(gray.swatch_token, "--color-gray-500");
    let light = resolve_theme(&ThemeInput::new(None, Appearance::Light));
    for color in colors {
        for appearance in Appearance::ALL {
            assert!(
                light.color(color.token(appearance)).is_some(),
                "{} {appearance:?}",
                color.value
            );
        }
    }
}

#[test]
fn the_file_tree_tints_mix_from_the_text_colour() {
    let variables = tree_host_variables(Color::srgb(1.0, 0.0, 0.0));
    let selected = variables
        .iter()
        .find(|(name, _)| name == "--trees-selected-bg-override")
        .expect("selected");
    let color = selected.1.color().expect("colour");
    assert!((color.alpha - 0.12).abs() < 1e-9);
    assert_eq!(color.to_srgb_unclamped(), [1.0, 0.0, 0.0]);
    assert_eq!(
        tree_foreground_override(Appearance::Dark),
        Some(("--trees-fg-override", "var(--contrast-foreground)"))
    );
}
