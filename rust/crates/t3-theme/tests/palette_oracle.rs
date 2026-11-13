//! The palette code against the real `themePalette.ts`: fixtures/palette-oracle.json
//! holds what the TypeScript returned for the same inputs (tools/palette-oracle.mjs).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use t3_theme::palette::{
    Appearance, AppearanceMode, EnvironmentTheme, ROLE_COUNT, ThemeCatalog, ThemeColorRole,
    ThemeColors, context_chip_variables, create_vivid_theme_colors, default_theme_colors,
    parse_theme_file, published_theme_definitions, readable_theme_color_on, serialize_theme_file,
    standard_painted_theme_colors, standard_theme_colors, theme_color_to_hex, theme_id_from_name,
    to_canonical_theme_color, update_theme_color_family,
};

#[derive(Deserialize)]
struct ColorCase {
    css: String,
    canonical: Option<String>,
    hex: Option<String>,
}

#[derive(Deserialize)]
struct VividCase {
    appearance: Appearance,
    canvas: String,
    accent: String,
    colors: ThemeColors,
}

#[derive(Deserialize)]
struct EditCase {
    appearance: Appearance,
    base: String,
    role: String,
    value: String,
    changed: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct ReadableCase {
    color: String,
    surface: String,
    min: f64,
    out: String,
}

#[derive(Deserialize)]
struct ChipCase {
    name: String,
    colors: ThemeColors,
    variables: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct FileCase {
    input: Value,
    parsed: Value,
    serialized: Option<Value>,
}

#[derive(Deserialize)]
struct IdCase {
    name: String,
    id: String,
}

#[derive(Deserialize)]
struct ResolutionCase {
    theme: String,
    #[serde(rename = "systemDark")]
    system_dark: bool,
    #[serde(rename = "followSystem")]
    follow_system: Option<bool>,
    mode: Option<String>,
    halves: Option<BTreeMap<String, String>>,
    appearance: Appearance,
    desktop: String,
}

#[derive(Deserialize)]
struct PreferenceCase {
    theme: String,
    known: bool,
    mode: Option<Appearance>,
    canonical: String,
    artwork: bool,
}

#[derive(Deserialize)]
struct HalvesCase {
    raw: Option<String>,
    parsed: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct Standard {
    light: ThemeColors,
    dark: ThemeColors,
    #[serde(rename = "paintedLight")]
    painted_light: ThemeColors,
    #[serde(rename = "paintedDark")]
    painted_dark: ThemeColors,
    #[serde(rename = "defaultLight")]
    default_light: ThemeColors,
    #[serde(rename = "defaultDark")]
    default_dark: ThemeColors,
}

#[derive(Deserialize)]
struct Oracle {
    colors: Vec<ColorCase>,
    vivid: Vec<VividCase>,
    edits: Vec<EditCase>,
    readable: Vec<ReadableCase>,
    chips: Vec<ChipCase>,
    files: Vec<FileCase>,
    ids: Vec<IdCase>,
    resolution: Vec<ResolutionCase>,
    preferences: Vec<PreferenceCase>,
    halves: Vec<HalvesCase>,
    published: Vec<Value>,
    #[serde(rename = "publishedOut")]
    published_out: Value,
    standard: Standard,
}

fn oracle() -> Oracle {
    serde_json::from_str(include_str!("../fixtures/palette-oracle.json")).expect("fixture parses")
}

fn role(name: &str) -> ThemeColorRole {
    ThemeColorRole::ALL
        .iter()
        .copied()
        .find(|r| r.name() == name)
        .expect("known role")
}

fn first_difference(actual: &ThemeColors, expected: &ThemeColors) -> Option<String> {
    ThemeColorRole::ALL.iter().find_map(|r| {
        (actual.get(*r) != expected.get(*r)).then(|| {
            format!(
                "{}: rust {} ts {}",
                r.name(),
                actual.get(*r),
                expected.get(*r)
            )
        })
    })
}

#[test]
fn colour_strings_canonicalise_like_typescript() {
    for case in oracle().colors {
        assert_eq!(
            to_canonical_theme_color(&case.css),
            case.canonical,
            "canonical {:?}",
            case.css
        );
        assert_eq!(
            theme_color_to_hex(&case.css),
            case.hex,
            "hex {:?}",
            case.css
        );
    }
}

#[test]
fn standard_and_default_palettes_match() {
    let o = oracle().standard;
    assert_eq!(
        first_difference(standard_theme_colors(Appearance::Light), &o.light),
        None
    );
    assert_eq!(
        first_difference(standard_theme_colors(Appearance::Dark), &o.dark),
        None
    );
    assert_eq!(
        first_difference(
            &standard_painted_theme_colors(Appearance::Light),
            &o.painted_light
        ),
        None
    );
    assert_eq!(
        first_difference(
            &standard_painted_theme_colors(Appearance::Dark),
            &o.painted_dark
        ),
        None
    );
    assert_eq!(
        first_difference(default_theme_colors(Appearance::Light), &o.default_light),
        None
    );
    assert_eq!(
        first_difference(default_theme_colors(Appearance::Dark), &o.default_dark),
        None
    );
}

#[test]
fn guided_generator_matches_typescript() {
    let cases = oracle().vivid;
    assert!(cases.len() > 100);
    for case in cases {
        let rust = create_vivid_theme_colors(case.appearance, &case.canvas, &case.accent);
        assert_eq!(
            first_difference(&rust, &case.colors),
            None,
            "{:?} {} {}",
            case.appearance,
            case.canvas,
            case.accent
        );
    }
}

#[test]
fn single_role_edits_match_typescript() {
    let cases = oracle().edits;
    assert!(cases.len() > 1900);
    for case in cases {
        let base = match case.base.as_str() {
            "default" => default_theme_colors(case.appearance).clone(),
            _ => {
                let (canvas, accent) = match case.appearance {
                    Appearance::Dark => ("#14202e", "#3a7bd5"),
                    Appearance::Light => ("#f2f6ee", "#3a7bd5"),
                };
                let canvas = if case.appearance == Appearance::Dark {
                    canvas
                } else {
                    "#f2f6ee"
                };
                create_vivid_theme_colors(case.appearance, canvas, accent)
            }
        };
        let updated =
            update_theme_color_family(case.appearance, &base, role(&case.role), &case.value);
        let expected = ThemeColors::from_fn(|r| {
            case.changed
                .get(r.name())
                .cloned()
                .unwrap_or_else(|| base.get(r).to_owned())
        });
        assert_eq!(
            first_difference(&updated, &expected),
            None,
            "{:?} {} {} {}",
            case.appearance,
            case.base,
            case.role,
            case.value
        );
    }
}

#[test]
fn readable_colour_solve_matches_typescript() {
    for case in oracle().readable {
        assert_eq!(
            readable_theme_color_on(&case.color, &case.surface, case.min),
            case.out,
            "{} on {} at {}",
            case.color,
            case.surface,
            case.min
        );
    }
}

#[test]
fn context_chip_variables_match_typescript() {
    let cases = oracle().chips;
    assert!(cases.len() > 50);
    for case in cases {
        assert_eq!(
            context_chip_variables(&case.colors),
            case.variables,
            "{}",
            case.name
        );
    }
}

#[test]
fn theme_files_parse_and_serialise_like_typescript() {
    for case in oracle().files {
        let rust = parse_theme_file(&case.input);
        match (&rust, case.parsed.get("ok"), case.parsed.get("error")) {
            (Ok(theme), Some(expected), None) => {
                assert_eq!(
                    &serde_json::to_value(theme).expect("serialises"),
                    expected,
                    "{}",
                    case.input
                );
                if let Some(serialized) = &case.serialized {
                    let text = serialize_theme_file(theme).expect("serialises");
                    assert_eq!(
                        serialized.get("ok").and_then(Value::as_str),
                        Some(text.as_str()),
                        "{}",
                        case.input
                    );
                }
            }
            (Err(error), None, Some(expected)) => {
                assert_eq!(Some(error.0.as_str()), expected.as_str(), "{}", case.input);
            }
            _ => panic!(
                "outcome differs for {}: rust {rust:?} ts {}",
                case.input, case.parsed
            ),
        }
    }
}

#[test]
fn theme_ids_match_typescript() {
    for case in oracle().ids {
        assert_eq!(theme_id_from_name(&case.name), case.id, "{:?}", case.name);
    }
}

fn halves_of(map: &Option<BTreeMap<String, String>>) -> Option<t3_theme::palette::ThemeHalves> {
    map.as_ref().map(|m| t3_theme::palette::ThemeHalves {
        light: m.get("light").cloned(),
        dark: m.get("dark").cloned(),
    })
}

#[test]
fn preference_resolution_matches_typescript() {
    let catalog = ThemeCatalog::default();
    let o = oracle();
    assert!(o.resolution.len() > 1000);
    for case in o.resolution {
        let halves = halves_of(&case.halves);
        let mode = case.mode.as_deref().and_then(AppearanceMode::parse);
        let appearance = catalog.resolve_appearance(
            &case.theme,
            case.system_dark,
            case.follow_system,
            mode,
            halves.as_ref(),
        );
        assert_eq!(
            appearance,
            case.appearance,
            "appearance {}",
            serde_json::json!({
                "theme": case.theme, "systemDark": case.system_dark, "follow": case.follow_system, "mode": case.mode, "halves": case.halves
            })
        );
        let desktop =
            catalog.resolve_desktop_theme(&case.theme, case.follow_system, mode, halves.as_ref());
        let name = match desktop {
            t3_theme::palette::DesktopTheme::Light => "light",
            t3_theme::palette::DesktopTheme::Dark => "dark",
            t3_theme::palette::DesktopTheme::System => "system",
        };
        assert_eq!(
            name, case.desktop,
            "desktop {} {:?} {:?} {:?}",
            case.theme, case.follow_system, case.mode, case.halves
        );
    }
    for case in o.preferences {
        assert_eq!(
            catalog.is_known_preference(&case.theme),
            case.known,
            "{}",
            case.theme
        );
        assert_eq!(
            catalog.preference_mode(&case.theme),
            case.mode,
            "{}",
            case.theme
        );
        assert_eq!(
            t3_theme::palette::canonical_theme_preference(&case.theme),
            case.canonical
        );
        assert_eq!(
            catalog.allows_sidebar_artwork(&case.theme),
            case.artwork,
            "{}",
            case.theme
        );
    }
    for case in o.halves {
        let parsed = catalog.parse_halves(case.raw.as_deref());
        let expected = halves_of(&case.parsed);
        assert_eq!(parsed, expected, "{:?}", case.raw);
    }
}

#[test]
fn published_themes_match_typescript() {
    let o = oracle();
    let themes: Vec<EnvironmentTheme> = o
        .published
        .iter()
        .map(|v| serde_json::from_value(v.clone()).expect("environment theme"))
        .collect();
    let rust = serde_json::to_value(published_theme_definitions(&themes)).expect("serialises");
    assert_eq!(rust, o.published_out);
}

#[test]
fn every_role_is_covered() {
    assert_eq!(ThemeColorRole::ALL.len(), ROLE_COUNT);
}
