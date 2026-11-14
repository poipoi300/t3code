use t3_theme::css::{theme_css_loads_remote_files, theme_css_value_fetches};

#[test]
fn declaration_fetch_guards_match_typescript() {
    for (value, fetches) in [
        (r#"url("https://example.com/a")"#, true),
        (r#"url("/api/thing")"#, true),
        (r#"image-set("https://example.com/a" 1x)"#, true),
        (r"u\72l(https://example.com/a)", true),
        (r"im\61ge-set(var(--leak) 1x)", true),
        (r"v\61r(--none, u\72l(https://example.com/a))", true),
        ("url(\"\u{a0}data:/../../probe\")", true),
        (r#"url("data:image/png;base64,AA")"#, false),
        (r##"url("#blur")"##, false),
        ("var(--pattern)", false),
        ("red", false),
        ("URL( \t\n\r\u{c}'DATA:x')", false),
        ("src('remote')", true),
        ("notsrc('remote')", false),
    ] {
        assert_eq!(theme_css_value_fetches(value), fetches, "{value}");
    }
}

#[test]
fn publication_text_check_is_early_feedback_not_the_parsed_guard() {
    assert!(!theme_css_loads_remote_files(
        "/* @import url(remote); */ a {color:red}"
    ));
    assert!(theme_css_loads_remote_files("@IMPORT 'remote';"));
    assert!(theme_css_loads_remote_files(
        "a { background: url(remote) }"
    ));
    assert!(!theme_css_loads_remote_files(
        r"a { background: u\72l(remote) }"
    ));
    assert!(!theme_css_loads_remote_files("a {filter: url(#local)}"));
}

#[test]
fn parsed_rules_drop_fetches_recursively_and_keep_safe_rules() {
    use t3_theme::css::{CssDeclaration, CssRule, strip_fetches};
    let declaration = |property: &str, value: &str| CssDeclaration {
        property: property.into(),
        value: value.into(),
        important: false,
    };
    let mut rules = vec![
        CssRule::Unsupported,
        CssRule::Namespace,
        CssRule::LayerStatement,
        CssRule::Property {
            initial_value: "url(https://example.com/d)".into(),
        },
        CssRule::Keyframes(vec![vec![declaration(
            "background-image",
            "url(https://example.com/g)",
        )]]),
        CssRule::Style {
            declarations: vec![declaration("--leak", r"u\72l(https://example.com/a)")],
            children: vec![],
        },
        CssRule::Style {
            declarations: vec![
                declaration("color", "rgb(1, 2, 3)"),
                declaration("background-image", "url(https://example.com/b)"),
                declaration("list-style-image", "var(--leak)"),
                declaration("border-image-source", "var(--registered)"),
                declaration("mask-image", r"v\61r(--none, u\72l(https://example.com/e))"),
            ],
            children: vec![CssRule::Group(vec![CssRule::Style {
                declarations: vec![declaration("cursor", "url(https://example.com/c), auto")],
                children: vec![],
            }])],
        },
        CssRule::Property {
            initial_value: "red".into(),
        },
    ];
    strip_fetches(&mut rules);
    assert!(!format!("{rules:?}").contains("example.com"));
    assert_eq!(rules.len(), 6);
    let CssRule::Style { declarations, .. } = &rules[4] else {
        panic!("style")
    };
    assert_eq!(
        declarations
            .iter()
            .map(|d| d.property.as_str())
            .collect::<Vec<_>>(),
        ["color", "list-style-image", "border-image-source"]
    );
}

#[test]
fn guide_and_hooks_match_the_upstream_sources() {
    use t3_theme::css::{THEME_CSS_HOOKS, THEME_GUIDE};
    assert_eq!(THEME_CSS_HOOKS.len(), 29);
    for (selector, description) in THEME_CSS_HOOKS {
        assert!(THEME_GUIDE.contains(&format!("- {selector}: {description}")));
    }
    for role in t3_theme::palette::ThemeColorRole::ALL {
        assert!(THEME_GUIDE.contains(role.name()));
    }
    assert!(THEME_GUIDE.contains("20,000"));
}

#[test]
fn environment_and_library_preserve_css_when_the_palette_changes() {
    use serde_json::json;
    use t3_theme::palette::{EnvironmentTheme, parse_stored_theme, published_theme_definitions};
    let published: EnvironmentTheme = serde_json::from_value(json!({"id": "css", "name": "CSS", "appearance": "dark", "canvas": "#000000", "accent": "#123456", "css": ":root { --radius: 0px }"})).expect("theme");
    let theme = published_theme_definitions(&[published]).remove(0);
    assert_eq!(theme.css.as_deref(), Some(":root { --radius: 0px }"));
    let stored =
        parse_stored_theme(&serde_json::to_value(&theme).expect("serialize")).expect("stored");
    assert_eq!(stored.css, theme.css);
}
