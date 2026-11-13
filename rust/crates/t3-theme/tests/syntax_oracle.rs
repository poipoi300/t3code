//! The syntax palettes against `@pierre/diffs` and `readableSyntaxTheme.ts`
//! (fixtures/syntax.json, written by tools/extract-syntax.mjs).

use serde::Deserialize;
use t3_theme::palette::Appearance;
use t3_theme::syntax::{
    READABLE_LIGHT_THEME_NAME, readable_syntax_color, syntax_theme, syntax_theme_for,
    syntax_theme_name, with_readable_syntax,
};

#[derive(Deserialize)]
struct Case {
    scope: Option<String>,
    input: String,
    output: String,
}

#[derive(Deserialize)]
struct Oracle {
    #[serde(rename = "readableCases")]
    readable_cases: Vec<Case>,
}

#[test]
fn readable_colours_match_typescript() {
    let oracle: Oracle =
        serde_json::from_str(include_str!("../fixtures/syntax.json")).expect("fixture");
    assert!(oracle.readable_cases.len() > 100);
    for case in oracle.readable_cases {
        assert_eq!(
            readable_syntax_color(&case.input),
            case.output,
            "{:?} {}",
            case.scope,
            case.input
        );
    }
}

#[test]
fn t3_light_is_pierre_light_made_readable() {
    let pierre = syntax_theme("pierre-light").expect("pierre-light");
    let derived = with_readable_syntax(pierre);
    let expected = syntax_theme(READABLE_LIGHT_THEME_NAME).expect("t3-light");
    assert_eq!(derived.name, expected.name);
    assert_eq!(derived.style.syntax, expected.style.syntax);
    assert_ne!(derived.style.syntax, pierre.style.syntax);
}

#[test]
fn each_appearance_has_its_palette() {
    assert_eq!(syntax_theme_name(Appearance::Light), "t3-light");
    assert_eq!(syntax_theme_name(Appearance::Dark), "pierre-dark");
    for appearance in Appearance::ALL {
        let theme = syntax_theme_for(appearance);
        assert_eq!(theme.appearance, appearance);
        assert_eq!(theme.style.terminal_ansi().len(), 16);
        assert!(
            theme
                .style
                .terminal_ansi()
                .iter()
                .all(|c| c.starts_with('#'))
        );
        assert!(
            theme
                .scope("keyword")
                .and_then(|s| s.color.as_deref())
                .is_some()
        );
        assert!(theme.style.syntax.len() > 100);
    }
}
