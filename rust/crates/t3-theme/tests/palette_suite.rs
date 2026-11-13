//! Port of `apps/web/src/themePalette.test.ts` (the tests that do not need a DOM).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::{Value, json};
use t3_theme::color::{Color, Space};
use t3_theme::palette::{
    Appearance, AppearanceMode, CUSTOM_THEMES_STORAGE_KEY, LibraryError, StorageFailure,
    ThemeCatalog, ThemeColorRole as R, ThemeColors, ThemeDefinition, ThemeHalves, ThemeLibrary,
    ThemeStorage, built_in_themes, context_chip_accents, context_chip_variables,
    create_vivid_theme_colors, default_theme_colors, parse_theme_file, serialize_theme_file,
    standard_painted_theme_colors, standard_theme_colors, t3_chat_theme, theme_color_to_hex,
    to_canonical_theme_color, update_theme_color_family,
};

fn as_hex(value: &str) -> String {
    theme_color_to_hex(value).unwrap_or_else(|| panic!("Expected a theme color, received {value}"))
        [..7]
        .to_owned()
}

fn canonical(value: &str) -> String {
    to_canonical_theme_color(value)
        .unwrap_or_else(|| panic!("Expected a theme color, received {value}"))
}

fn channels(hex: &str) -> [f64; 3] {
    [1, 3, 5].map(|i| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")) / 255.0)
}

fn luminance(channels: [f64; 3]) -> f64 {
    let lin = |c: f64| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(channels[0]) + 0.7152 * lin(channels[1]) + 0.0722 * lin(channels[2])
}

fn contrast_of(first: f64, second: f64) -> f64 {
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

fn contrast_ratio(first: &str, second: &str) -> f64 {
    contrast_of(
        luminance(channels(&as_hex(first))),
        luminance(channels(&as_hex(second))),
    )
}

fn oklab_distance(first: &str, second: &str) -> f64 {
    let a = Color::parse(first).expect("colour").to_oklab();
    let b = Color::parse(second).expect("colour").to_oklab();
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn file(extra: Value) -> Value {
    let mut base = json!({ "version": 1 });
    for (key, value) in extra.as_object().expect("object") {
        base[key] = value.clone();
    }
    base
}

fn parse(extra: Value) -> ThemeDefinition {
    parse_theme_file(&file(extra)).expect("valid theme file")
}

fn all_builtin_palettes() -> Vec<ThemeColors> {
    let mut palettes = vec![
        standard_theme_colors(Appearance::Light).clone(),
        standard_theme_colors(Appearance::Dark).clone(),
    ];
    for theme in built_in_themes() {
        for mode in theme.modes() {
            palettes.push(theme.colors_for(mode).expect("mode").clone());
        }
    }
    palettes
}

#[test]
fn gives_non_latin_theme_names_distinct_stable_ids() {
    let base = json!({ "appearance": "dark", "colors": { "canvas": "#101820" } });
    let with_name = |name: &str| {
        let mut value = base.clone();
        value["name"] = json!(name);
        parse(value)
    };
    let night = with_name("夜");
    assert_ne!(with_name("朝").id, night.id);
    assert_eq!(with_name(" 夜 ").id, night.id);
}

#[test]
fn built_in_and_standard_search_pairs_stay_readable_and_distinct() {
    let mut palettes = all_builtin_palettes();
    // Built-ins include every variant, as the TypeScript test iterates them.
    palettes.extend(built_in_themes().iter().flat_map(|t| {
        t.variants
            .iter()
            .flat_map(|v| [v.light.clone(), v.dark.clone()])
            .flatten()
    }));
    for colors in palettes {
        assert!(
            contrast_ratio(
                colors.get(R::SearchMatchForeground),
                colors.get(R::SearchMatchBackground)
            ) >= 4.5
        );
        assert!(
            contrast_ratio(
                colors.get(R::SearchMatchActiveForeground),
                colors.get(R::SearchMatchActiveBackground)
            ) >= 4.5
        );
        assert!(
            oklab_distance(
                colors.get(R::SearchMatchBackground),
                colors.get(R::SearchMatchActiveBackground)
            ) > 0.14
        );
        for background in [R::Muted, R::MessageSurface, R::CodeBackground] {
            assert!(
                oklab_distance(colors.get(R::SearchMatchBackground), colors.get(background)) > 0.08
            );
            assert!(
                oklab_distance(
                    colors.get(R::SearchMatchActiveBackground),
                    colors.get(background)
                ) > 0.08
            );
        }
    }
}

#[test]
fn round_trips_custom_search_colours_and_fills_in_older_files() {
    let legacy =
        parse(json!({ "name": "Legacy", "appearance": "dark", "colors": { "canvas": "#101010" } }));
    assert_eq!(
        legacy.colors.get(R::SearchMatchBackground),
        default_theme_colors(Appearance::Dark).get(R::SearchMatchBackground)
    );
    let theme = parse(json!({
        "name": "Search",
        "appearance": "dark",
        "colors": {
            "searchMatchBackground": "#224466",
            "searchMatchForeground": "#ffffff",
            "searchMatchActiveBackground": "#aaddff",
            "searchMatchActiveForeground": "#112233"
        },
        "variants": { "light": { "searchMatchBackground": "#ddeeff", "searchMatchForeground": "#112233" } }
    }));
    let text = serialize_theme_file(&theme).expect("serialises");
    let restored = parse_theme_file(&serde_json::from_str(&text).expect("json")).expect("parses");
    assert_eq!(restored.colors, theme.colors);
    assert_eq!(restored.variants, theme.variants);
}

#[test]
fn editing_search_roles_derives_readable_text_without_touching_other_roles() {
    for (role, foreground) in [
        (R::SearchMatchBackground, R::SearchMatchForeground),
        (
            R::SearchMatchActiveBackground,
            R::SearchMatchActiveForeground,
        ),
    ] {
        let original = default_theme_colors(Appearance::Dark);
        for value in ["#ffffff", "#000000", "#33669980"] {
            let updated = update_theme_color_family(Appearance::Dark, original, role, value);
            assert!(
                contrast_ratio(updated.get(foreground), updated.get(role)) >= 4.5,
                "{role:?} {value}"
            );
            for key in R::ALL {
                if key != role && key != foreground {
                    assert_eq!(updated.get(key), original.get(key), "{key:?}");
                }
            }
        }
    }
}

#[test]
fn derives_readable_distinctive_vivid_palettes_from_exact_seeds() {
    let seeds = [
        (Appearance::Light, "#f4f9f2", "#1d8a4e"),
        (Appearance::Dark, "#101a2c", "#4f8fe8"),
        (Appearance::Dark, "#211a23", "#df5398"),
        (Appearance::Light, "#fdf6ec", "#c2571b"),
        (Appearance::Light, "#111827", "#8ab4f8"),
        (Appearance::Dark, "#f5ecf5", "#a84370"),
    ];
    for (appearance, canvas, accent) in seeds {
        let c = create_vivid_theme_colors(appearance, canvas, accent);
        assert!(c.get(R::Canvas).starts_with("oklch("));
        assert!(c.get(R::Accent).starts_with("oklch("));
        assert_eq!(as_hex(c.get(R::Canvas)), canvas);
        assert_eq!(as_hex(c.get(R::Accent)), accent);
        let ratio = |fg: R, bg: R| contrast_ratio(c.get(fg), c.get(bg));
        assert!(ratio(R::Text, R::Canvas) >= 7.0);
        assert!(ratio(R::TextMuted, R::Canvas) >= 4.5 && ratio(R::TextMuted, R::Canvas) < 5.5);
        assert!(ratio(R::MutedForeground, R::Muted) >= 4.5);
        assert!(ratio(R::MutedForeground, R::Muted) < ratio(R::Text, R::Muted));
        assert!(ratio(R::Placeholder, R::SurfaceRaised) >= 4.5);
        assert!(ratio(R::Placeholder, R::SurfaceRaised) < ratio(R::Text, R::SurfaceRaised));
        assert_eq!(c.get(R::SecondaryLabel), c.get(R::TextMuted));
        assert!(ratio(R::AccentForeground, R::Accent) >= 4.5);
        assert!(ratio(R::SearchMatchForeground, R::SearchMatchBackground) >= 4.5);
        assert!(
            ratio(
                R::SearchMatchActiveForeground,
                R::SearchMatchActiveBackground
            ) >= 4.5
        );
        assert!(ratio(R::MessageActionForeground, R::MessageAction) >= 4.5);
        assert!(ratio(R::MessageForeground, R::MessageSurface) >= 4.5);
        assert!(ratio(R::SecondaryForeground, R::Secondary) >= 4.5);
        assert!(ratio(R::SidebarForeground, R::Sidebar) >= 4.5);
        assert_ne!(c.get(R::MessageAction), c.get(R::Accent));
        assert_eq!(as_hex(c.get(R::Update)), accent);
        let [er, eg, eb] = channels(&as_hex(c.get(R::Error)));
        assert!(er > eg * 2.0 && er > eb * 2.0);
        assert!(contrast_ratio(c.get(R::Error), "#ffffff") >= 2.5);
        assert!(ratio(R::ErrorForeground, R::ErrorSurface) >= 4.5);
        let [wr, wg, wb] = channels(&as_hex(c.get(R::Warning)));
        assert!(wr > wb && wg > wb);
    }
}

#[test]
fn decodes_literal_css_colour_formats_into_oklch_without_dropping_alpha() {
    let theme = parse(json!({
        "name": "Translucent",
        "appearance": "light",
        "colors": {
            "canvas": "oklch(62% 0.2 280deg / 50%)",
            "accent": "#abcd",
            "focus": "rgb(10 20 30 / 50%)",
            "error": "hsl(350 80% 50%)",
            "warning": "hwb(45 10% 20%)",
            "update": "lab(60% 40 30)",
            "messageAction": "lch(60% 50 120)",
            "sidebar": "oklab(0.6 0.1 -0.1)",
            "terminalCursor": "color(display-p3 0.8 0.2 0.3)",
            "terminalSelection": "rebeccapurple",
            "terminalScrollbar": "transparent",
            "terminalScrollbarHover": "rgb(10 20 30 / none)"
        }
    }));
    let c = &theme.colors;
    assert_eq!(c.get(R::Canvas), "oklch(0.62 0.2 280 / 0.5)");
    assert_eq!(c.get(R::Accent), canonical("#abcd"));
    assert_eq!(
        theme_color_to_hex(c.get(R::Accent)).as_deref(),
        Some("#aabbccdd")
    );
    assert_eq!(
        theme_color_to_hex(c.get(R::Focus)).as_deref(),
        Some("#0a141e80")
    );
    assert_eq!(
        theme_color_to_hex(c.get(R::TerminalSelection)).as_deref(),
        Some("#663399")
    );
    assert_eq!(c.get(R::TerminalScrollbar), "oklch(0 0 0 / 0)");
    assert_eq!(
        theme_color_to_hex(c.get(R::TerminalScrollbarHover)).as_deref(),
        Some("#0a141e00")
    );
    for role in [
        R::Error,
        R::Warning,
        R::Update,
        R::MessageAction,
        R::Sidebar,
        R::TerminalCursor,
    ] {
        assert!(c.get(role).starts_with("oklch("), "{role:?}");
    }
}

#[test]
fn gamut_maps_extreme_finite_oklch_chroma_from_theme_files() {
    let theme = parse(
        json!({ "name": "Extreme chroma", "appearance": "light", "colors": { "accent": "oklch(0.5 1e303 0)" } }),
    );
    assert_eq!(theme.colors.get(R::Accent), "oklch(0.5 1e+303 0)");
    assert_eq!(
        theme_color_to_hex(theme.colors.get(R::Accent)).as_deref(),
        Some("#b5005e")
    );
}

#[test]
fn keeps_optional_light_and_dark_palettes_under_one_theme_id() {
    let theme = parse(json!({
        "id": "aurora",
        "name": "Aurora",
        "appearance": "light",
        "colors": { "canvas": "#f8fbff", "text": "#10243d" },
        "variants": { "dark": { "canvas": "#101827", "text": "#eef5ff" } }
    }));
    assert_eq!(theme.modes(), vec![Appearance::Light, Appearance::Dark]);
    let dark = theme.colors_for(Appearance::Dark).expect("dark");
    assert_eq!(dark.get(R::Canvas), canonical("#101827"));
    assert_eq!(dark.get(R::Text), canonical("#eef5ff"));
    assert_eq!(
        t3_chat_theme().modes(),
        vec![Appearance::Light, Appearance::Dark]
    );
    let catalog = ThemeCatalog::default();
    assert_eq!(
        catalog.resolve_appearance("t3-chat", true, Some(true), None, None),
        Appearance::Dark
    );
    assert_eq!(
        catalog.resolve_desktop_theme("t3-chat", Some(true), None, None),
        t3_theme::palette::DesktopTheme::System
    );
    assert_eq!(
        catalog.resolve_appearance(
            "t3-chat",
            false,
            Some(false),
            Some(AppearanceMode::Dark),
            None
        ),
        Appearance::Dark
    );
    assert_eq!(
        catalog.resolve_desktop_theme("t3-chat", Some(false), Some(AppearanceMode::Dark), None),
        t3_theme::palette::DesktopTheme::Dark
    );
    let serialised: Value =
        serde_json::from_str(&serialize_theme_file(&theme).expect("serialises")).expect("json");
    assert_eq!(
        serialised["variants"]["dark"]["canvas"],
        json!(canonical("#101827"))
    );
    assert_eq!(
        serialised["variants"]["dark"]["text"],
        json!(canonical("#eef5ff"))
    );
}

/// In-memory storage with the failure modes the browser's local storage has.
#[derive(Clone, Default)]
struct MemoryStorage {
    value: Rc<RefCell<Option<String>>>,
    reads: Rc<Cell<usize>>,
    writes: Rc<Cell<usize>>,
    fail_reads_after: Rc<Cell<Option<usize>>>,
    fail_all_reads: Rc<Cell<bool>>,
}

impl MemoryStorage {
    fn with(value: &Value) -> Self {
        let storage = Self::default();
        *storage.value.borrow_mut() = Some(value.to_string());
        storage
    }

    fn json(&self) -> Value {
        serde_json::from_str(self.value.borrow().as_deref().unwrap_or("[]")).expect("json")
    }
}

impl ThemeStorage for MemoryStorage {
    fn read(&self) -> Result<Option<String>, StorageFailure> {
        self.reads.set(self.reads.get() + 1);
        if self.fail_all_reads.get()
            || self
                .fail_reads_after
                .get()
                .is_some_and(|limit| self.reads.get() > limit)
        {
            return Err(StorageFailure);
        }
        Ok(self.value.borrow().clone())
    }

    fn write(&mut self, raw: &str) -> Result<(), StorageFailure> {
        self.writes.set(self.writes.get() + 1);
        *self.value.borrow_mut() = Some(raw.to_owned());
        Ok(())
    }
}

#[test]
fn preserves_valid_imported_collections_and_drops_malformed_metadata() {
    let storage = MemoryStorage::with(&json!([
        {
            "id": "github-dark", "label": "GitHub Dark", "appearance": "dark", "colors": { "canvas": "#0d1117" },
            "collection": { "id": "open-vsx:github.github-vscode-theme", "label": "GitHub Theme" }
        },
        {
            "id": "github-light", "label": "GitHub Light", "appearance": "light", "colors": { "canvas": "#ffffff" },
            "collection": { "id": "bad collection id", "label": "GitHub Theme" }
        }
    ]));
    let mut library = ThemeLibrary::new(storage);
    let themes = library.custom_themes();
    assert_eq!(themes.len(), 2);
    let collection = themes[0].collection.as_ref().expect("collection");
    assert_eq!(collection.id, "open-vsx:github.github-vscode-theme");
    assert_eq!(collection.label, "GitHub Theme");
    assert_eq!(themes[1].id, "github-light");
    assert!(themes[1].collection.is_none());
}

#[test]
fn atomically_replaces_an_imported_collection_and_removes_stale_variants() {
    let collection = json!({ "id": "open-vsx:demo.theme", "label": "Demo Theme" });
    let personal = json!({
        "id": "personal", "label": "Personal", "appearance": "dark",
        "colors": { "canvas": "#111111", "futureRole": "hsl(10 20% 30%)" }, "futureMetadata": { "version": 2 }
    });
    let storage = MemoryStorage::with(&json!([
        personal,
        { "id": "old-light", "label": "Old Light", "appearance": "light", "colors": { "canvas": "#ffffff" }, "collection": collection },
        { "id": "removed-dark", "label": "Removed Dark", "appearance": "dark", "colors": { "canvas": "#000000" }, "collection": collection }
    ]));
    let mut library = ThemeLibrary::new(storage.clone());
    let expected = library
        .stored_collection("open-vsx:demo.theme")
        .expect("collection");
    library.custom_themes();
    // Another tab adds a theme while the package downloads.
    let mut stored = storage.json();
    stored.as_array_mut().expect("array").push(json!({
        "id": "other-tab", "label": "Other Tab", "appearance": "dark", "colors": { "canvas": "#222222" }
    }));
    *storage.value.borrow_mut() = Some(stored.to_string());
    let with_collection = |theme: ThemeDefinition| ThemeDefinition {
        collection: Some(t3_theme::palette::ThemeCollection {
            id: "open-vsx:demo.theme".into(),
            label: "Demo Theme".into(),
        }),
        ..theme
    };
    let replacement = vec![
        with_collection(parse(
            json!({ "id": "old-light", "name": "New Light", "appearance": "light", "colors": { "canvas": "#fafafa" } }),
        )),
        with_collection(parse(
            json!({ "id": "new-dark", "name": "New Dark", "appearance": "dark", "colors": { "canvas": "#101010" } }),
        )),
    ];
    let returned = library
        .replace_collection("open-vsx:demo.theme", &replacement, Some(&expected))
        .expect("replaced");
    assert_eq!(returned, replacement);
    let ids: Vec<&str> = library
        .custom_themes()
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(ids, ["personal", "old-light", "new-dark", "other-tab"]);
    assert_eq!(storage.writes.get(), 1);
    assert_eq!(storage.json()[0], personal);
}

#[test]
fn canonicalises_explicit_writes_without_migrating_untouched_themes() {
    let untouched = json!({
        "id": "legacy", "label": "Legacy", "appearance": "dark",
        "colors": { "accent": "#5b6cff", "futureRole": "hsl(10 20% 30%)" }, "futureMetadata": { "version": 2 }
    });
    let storage = MemoryStorage::with(&json!([untouched]));
    let mut library = ThemeLibrary::new(storage.clone());
    let created = library
        .install(&parse(json!({
            "id": "aurora", "name": "Aurora", "appearance": "light",
            "colors": { "canvas": "#f8fbff", "accent": "#5b6cff" }, "sidebarArtwork": true
        })))
        .expect("installed");
    let mut edited = created.clone();
    edited.label = "Aurora Night".into();
    edited.colors.set(R::Accent, "hsl(263 70% 58%)");
    let updated = library.update(&edited).expect("updated");
    assert_eq!(updated.id, "aurora");
    assert_eq!(updated.label, "Aurora Night");
    assert_eq!(updated.colors.get(R::Accent), canonical("hsl(263 70% 58%)"));
    assert!(updated.sidebar_artwork.is_none());
    let stored = storage.json();
    assert_eq!(stored[0], untouched);
    assert_eq!(stored[1]["id"], "aurora");
    assert_eq!(stored[1]["label"], "Aurora Night");
    assert_eq!(
        stored[1]["colors"]["accent"],
        json!(canonical("hsl(263 70% 58%)"))
    );
    assert!(stored[1].get("sidebarArtwork").is_none());
    library.invalidate();
    let reread = library
        .custom_themes()
        .iter()
        .find(|t| t.id == "aurora")
        .cloned()
        .expect("aurora");
    assert_eq!(reread.colors.get(R::Accent), canonical("hsl(263 70% 58%)"));
    library.remove(&["aurora"]).expect("removed");
    assert_eq!(storage.json(), json!([untouched]));
}

#[test]
fn writes_from_the_cached_snapshot_without_risking_a_destructive_reread() {
    let legacy = json!({ "id": "legacy", "label": "Legacy", "appearance": "dark", "colors": { "accent": "#5b6cff" }, "futureMetadata": true });
    let storage = MemoryStorage::with(&json!([legacy]));
    storage.fail_reads_after.set(Some(1));
    let mut library = ThemeLibrary::new(storage.clone());
    assert_eq!(library.custom_themes().len(), 1);
    library
        .install(&parse(json!({ "id": "aurora", "name": "Aurora", "appearance": "light", "colors": { "accent": "hsl(263 70% 58%)" } })))
        .expect("installed from the cached snapshot");
    assert_eq!(storage.reads.get(), 1);
    assert_eq!(storage.writes.get(), 1);
    assert_eq!(storage.json()[0], legacy);
}

#[test]
fn refuses_to_overwrite_a_library_that_could_not_be_read() {
    let storage = MemoryStorage::default();
    storage.fail_all_reads.set(true);
    let mut library = ThemeLibrary::new(storage.clone());
    assert!(library.custom_themes().is_empty());
    let error = library
        .install(&parse(json!({ "id": "aurora", "name": "Aurora", "appearance": "light", "colors": { "accent": "#5b6cff" } })))
        .expect_err("refused");
    assert_eq!(
        error.to_string(),
        format!("Failed to read the theme library from {CUSTOM_THEMES_STORAGE_KEY}.")
    );
    assert!(matches!(error, LibraryError::Storage { .. }));
    assert_eq!(storage.writes.get(), 0);
}

fn chip_surface_contrast(chip: &str, surface: &str, canvas: &str) -> f64 {
    let clamp = |c: [f64; 3]| c.map(|v| v.clamp(0.0, 1.0));
    let parse_rgb = |value: &str| Color::parse(value).expect("colour");
    let back = clamp(parse_rgb(canvas).to_srgb_unclamped());
    let surface_color = parse_rgb(surface);
    let alpha = surface_color.alpha;
    let rgb = clamp(surface_color.to_srgb_unclamped());
    let painted = [0, 1, 2].map(|i| back[i] * (1.0 - alpha) + rgb[i] * alpha);
    contrast_of(luminance(channels(&as_hex(chip))), luminance(painted))
}

fn all_palettes_by_mode() -> Vec<ThemeColors> {
    all_builtin_palettes()
}

#[test]
fn chooses_readable_chip_lightness_when_the_text_is_medium_gray() {
    for (surface, minimum) in [("#ffffff", 5.0), ("#777777", 4.5)] {
        let colors = standard_theme_colors(Appearance::Light)
            .clone()
            .with(R::Canvas, surface)
            .with(R::Surface, surface)
            .with(R::SurfaceRaised, surface)
            .with(R::MessageSurface, surface)
            .with(R::Text, "#767676")
            .with(R::MessageForeground, "#767676");
        for (_, color) in context_chip_variables(&colors) {
            assert!(
                contrast_ratio(&color, surface) >= minimum,
                "{color} on {surface}"
            );
        }
    }
}

#[test]
fn chips_read_at_five_to_one_on_every_chip_surface_and_keep_each_hue() {
    for colors in all_palettes_by_mode() {
        let variables: BTreeMap<String, String> =
            context_chip_variables(&colors).into_iter().collect();
        assert!(!variables.contains_key("--context-chip-message"));
        for (_, accent, variable) in context_chip_accents() {
            let chip = &variables[variable];
            for surface in [R::Canvas, R::Surface, R::SurfaceRaised, R::MessageSurface] {
                assert!(
                    chip_surface_contrast(chip, colors.get(surface), colors.get(R::Canvas)) >= 5.0,
                    "{variable} on {surface:?}"
                );
            }
            let solved = Color::parse(chip).expect("chip").to_space(Space::Oklch).c;
            if solved[1] > 0.04 {
                let wanted = Color::parse(accent)
                    .expect("accent")
                    .to_space(Space::Oklch)
                    .c[2];
                let drift = ((solved[2] - wanted + 540.0).rem_euclid(360.0) - 180.0).abs();
                assert!(drift < 4.0, "{variable} drifted {drift}");
            }
        }
    }
}

#[test]
fn chips_read_at_five_to_one_on_the_standard_surfaces_as_index_css_paints_them() {
    let painted: [(Appearance, &[&str]); 2] = [
        (Appearance::Light, &["#fcfcfc", "#ffffff", "#ebebed"]),
        (
            Appearance::Dark,
            &["#000000", "#0a0a0a", "#111111", "#141414", "#1e1e1e"],
        ),
    ];
    for (appearance, surfaces) in painted {
        let variables = context_chip_variables(&standard_painted_theme_colors(appearance));
        for (_, _, variable) in context_chip_accents() {
            let chip = &variables
                .iter()
                .find(|(name, _)| name == variable)
                .expect("chip")
                .1;
            for surface in surfaces {
                assert!(
                    contrast_ratio(chip, surface) >= 5.0,
                    "{variable} on {surface}"
                );
            }
        }
    }
}

#[test]
fn invalid_state_error_colour_reads_at_five_to_one_on_every_chip_surface() {
    for colors in all_palettes_by_mode() {
        let variables: BTreeMap<String, String> =
            context_chip_variables(&colors).into_iter().collect();
        let invalid = &variables["--context-chip-invalid"];
        for surface in [R::Canvas, R::Surface, R::SurfaceRaised, R::MessageSurface] {
            assert!(
                chip_surface_contrast(invalid, colors.get(surface), colors.get(R::Canvas)) >= 5.0
            );
        }
    }
}

#[test]
fn leaves_a_bubble_with_opposite_shade_text_out_of_the_shared_solve() {
    let colors = standard_theme_colors(Appearance::Light)
        .clone()
        .with(R::MessageSurface, "#1b4ed8")
        .with(R::MessageForeground, "#ffffff");
    let variables: BTreeMap<String, String> = context_chip_variables(&colors).into_iter().collect();
    assert_eq!(variables["--context-chip-message"], "#ffffff");
    for (_, _, variable) in context_chip_accents() {
        assert!(contrast_ratio(&variables[variable], colors.get(R::Canvas)) >= 5.0);
    }
    assert!(
        Color::parse(&variables["--context-chip-file"])
            .expect("file")
            .to_space(Space::Oklch)
            .c[1]
            > 0.08
    );
}

#[test]
fn lets_a_configured_half_unlock_an_appearance_the_base_theme_lacks() {
    let paper = parse(
        json!({ "id": "paper", "name": "Paper", "appearance": "light", "colors": { "canvas": "#f8fbff" } }),
    );
    let custom = [paper];
    let catalog = ThemeCatalog {
        custom: &custom,
        environment: &[],
    };
    assert_eq!(
        catalog.resolve_appearance("paper", true, Some(true), None, None),
        Appearance::Light
    );
    let halves = ThemeHalves {
        light: None,
        dark: Some("grove".into()),
    };
    assert_eq!(
        catalog.resolve_appearance("paper", true, Some(true), None, Some(&halves)),
        Appearance::Dark
    );
    assert_eq!(
        catalog.resolve_appearance(
            "paper",
            false,
            Some(false),
            Some(AppearanceMode::Dark),
            Some(&halves)
        ),
        Appearance::Dark
    );
    assert_eq!(
        catalog.resolve_desktop_theme("paper", Some(true), None, Some(&halves)),
        t3_theme::palette::DesktopTheme::System
    );
}

#[test]
fn decodes_stored_colours_in_memory_without_writing_during_reads() {
    let stored = json!([
        {
            "id": "aurora", "label": "Aurora", "appearance": "light",
            "colors": { "canvas": "#f8fbff", "futureRole": "#123456", "accent": "not-a-color" },
            "variants": { "dark": { "canvas": "rgb(16 24 39)" } }
        },
        { "id": "light", "label": "Reserved", "appearance": "light", "colors": {} },
        { "id": "aurora", "label": "Duplicate", "appearance": "dark", "colors": {} }
    ]);
    let storage = MemoryStorage::with(&stored);
    let mut library = ThemeLibrary::new(storage.clone());
    let themes = library.custom_themes().to_vec();
    assert_eq!(themes.len(), 1);
    assert_eq!(themes[0].id, "aurora");
    assert_eq!(themes[0].colors.get(R::Canvas), canonical("#f8fbff"));
    assert_eq!(
        themes[0].colors.get(R::Accent),
        default_theme_colors(Appearance::Light).get(R::Accent)
    );
    assert_eq!(themes[0].modes(), vec![Appearance::Light, Appearance::Dark]);
    assert_eq!(
        themes[0]
            .colors_for(Appearance::Dark)
            .map(|c| c.get(R::Canvas).to_owned()),
        Some(canonical("rgb(16 24 39)"))
    );
    assert_eq!(storage.writes.get(), 0);
    assert_eq!(storage.json(), stored);
}
