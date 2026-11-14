//! Theme files and stored themes: strict parsing of user-supplied files, lenient
//! decoding of saved ones, ids, and published environment themes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use super::definition::{
    Appearance, ThemeCollection, ThemeColors, ThemeDefinition, ThemeVariants, default_theme_colors,
    is_reserved_theme_id,
};
use super::derive::create_vivid_theme_colors;
use super::math::to_canonical_theme_color;
use super::roles::ThemeColorRole;

pub const THEME_FILE_VERSION: u32 = 1;
pub const THEME_CSS_MAX_LENGTH: usize = 20_000;
/// The `data-theme-id` of an unsaved draft shown while editing a theme.
pub const THEME_PREVIEW_ID: &str = "__preview";
pub const CUSTOM_THEMES_STORAGE_KEY: &str = "t3code:themes:v1";
pub const THEME_FOLLOW_SYSTEM_STORAGE_KEY: &str = "t3code:theme-follow-system";
pub const THEME_APPEARANCE_MODE_STORAGE_KEY: &str = "t3code:theme-appearance-mode";
pub const THEME_HALVES_STORAGE_KEY: &str = "t3code:theme-halves:v1";
pub const LEGACY_T3_CHAT_DARK_THEME_ID: &str = "t3-chat-dark";

/// A theme file or saved theme that cannot be used. Messages are shown to people.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct ThemeError(pub String);

impl ThemeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

fn is_ascii_lower_alnum(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

/// `isThemeId`: `^[a-z0-9](?:[a-z0-9-]{0,47})$`.
pub fn is_theme_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_ascii_lower_alnum(first)
        && value.len() <= 48
        && chars.all(|c| is_ascii_lower_alnum(c) || c == '-')
}

fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// `isThemeLabel`: non-blank, at most 48 UTF-16 units once trimmed.
pub fn is_theme_label(value: &str) -> bool {
    let trimmed = js_trim(value);
    !trimmed.is_empty() && trimmed.encode_utf16().count() <= 48
}

fn theme_css(value: Option<&Value>) -> Option<&str> {
    value?
        .as_str()
        .filter(|css| css.encode_utf16().count() <= THEME_CSS_MAX_LENGTH)
}

fn nonempty_css(value: Option<&Value>) -> Option<String> {
    theme_css(value)
        .filter(|css| !css.is_empty())
        .map(str::to_owned)
}

fn is_collection_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && value.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-'))
}

/// `themeIdFromName`: a slug of the name, or a stable hash for names without ASCII letters.
pub fn theme_id_from_name(name: &str) -> String {
    let label = js_trim(name).to_lowercase();
    let mut normalized = String::new();
    let mut pending_dash = false;
    for c in label.chars() {
        if is_ascii_lower_alnum(c) {
            if pending_dash && !normalized.is_empty() {
                normalized.push('-');
            }
            pending_dash = false;
            normalized.push(c);
        } else {
            pending_dash = true;
        }
    }
    normalized.truncate(48);
    while normalized.ends_with('-') {
        normalized.pop();
    }
    if !normalized.is_empty() || label.is_empty() {
        return if normalized.is_empty() {
            "custom-theme".to_owned()
        } else {
            normalized
        };
    }
    let mut hash: u32 = 2_166_136_261;
    for unit in label.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("theme-{}", to_base36(hash))
}

fn to_base36(mut value: u32) -> String {
    if value == 0 {
        return "0".to_owned();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(char::from_digit(value % 36, 36).unwrap_or('0'));
        value /= 36;
    }
    digits.iter().rev().collect()
}

fn parse_collection(value: Option<&Value>) -> Option<ThemeCollection> {
    let object = value?.as_object()?;
    let id = object.get("id")?.as_str()?;
    let label = object.get("label")?.as_str()?;
    (is_collection_id(id) && is_theme_label(label)).then(|| ThemeCollection {
        id: id.to_owned(),
        label: js_trim(label).to_owned(),
    })
}

fn role_from_name(name: &str) -> Option<ThemeColorRole> {
    ThemeColorRole::ALL
        .iter()
        .copied()
        .find(|role| role.name() == name)
}

/// `lenientThemeColorOverrides`: keeps the entries that are known roles with literal colours.
fn lenient_overrides(object: &Map<String, Value>) -> Vec<(ThemeColorRole, String)> {
    object
        .iter()
        .filter_map(|(key, value)| {
            let role = role_from_name(key)?;
            let color = to_canonical_theme_color(value.as_str()?)?;
            Some((role, color))
        })
        .collect()
}

fn with_overrides(mut base: ThemeColors, overrides: Vec<(ThemeColorRole, String)>) -> ThemeColors {
    for (role, color) in overrides {
        base.set(role, color);
    }
    base
}

fn parse_stored_colors(value: Option<&Value>, appearance: Appearance) -> Option<ThemeColors> {
    let object = value?.as_object()?;
    Some(with_overrides(
        default_theme_colors(appearance).clone(),
        lenient_overrides(object),
    ))
}

/// `Some(None)` when absent, `None` when malformed.
fn parse_stored_variants(value: Option<&Value>, base: Appearance) -> Option<Option<ThemeVariants>> {
    let Some(value) = value else {
        return Some(None);
    };
    let object = value.as_object()?;
    let mut variants = ThemeVariants::default();
    for (key, colors) in object {
        let appearance = Appearance::parse(key)?;
        if appearance == base {
            continue;
        }
        variants.set(appearance, parse_stored_colors(Some(colors), appearance)?);
    }
    Some((!variants.is_empty()).then_some(variants))
}

/// `parseStoredTheme`: a saved theme, filled in from defaults where roles are missing.
pub fn parse_stored_theme(value: &Value) -> Option<ThemeDefinition> {
    let object = value.as_object()?;
    let id = object.get("id")?.as_str()?;
    if !is_theme_id(id) || is_reserved_theme_id(id) {
        return None;
    }
    let label = object.get("label")?.as_str()?;
    let appearance = Appearance::parse(object.get("appearance")?.as_str()?)?;
    if !is_theme_label(label) {
        return None;
    }
    let colors = parse_stored_colors(object.get("colors"), appearance)?;
    let variants = parse_stored_variants(object.get("variants"), appearance)?;
    Some(ThemeDefinition {
        id: id.to_owned(),
        label: js_trim(label).to_owned(),
        appearance,
        colors,
        variants,
        collection: parse_collection(object.get("collection")),
        sidebar_artwork: None,
        managed: (object.get("managed") == Some(&Value::Bool(true))).then_some(true),
        css: nonempty_css(object.get("css")),
    })
}

/// `parseStoredThemes`: drops malformed entries and repeated ids.
pub fn parse_stored_themes(stored: &[Value]) -> Vec<ThemeDefinition> {
    let mut themes: Vec<ThemeDefinition> = Vec::new();
    for value in stored {
        if let Some(theme) = parse_stored_theme(value)
            && !themes.iter().any(|existing| existing.id == theme.id)
        {
            themes.push(theme);
        }
    }
    themes
}

fn parse_color_overrides(value: &Value) -> Result<Vec<(ThemeColorRole, String)>, ThemeError> {
    let Some(object) = value.as_object() else {
        return Err(ThemeError::new("Theme colors must be objects."));
    };
    let mut overrides = Vec::new();
    for (key, color) in object {
        let Some(role) = role_from_name(key) else {
            return Err(ThemeError::new(format!(
                "\"{key}\" is not a supported theme color role."
            )));
        };
        let Some(normalized) = color.as_str().and_then(to_canonical_theme_color) else {
            return Err(ThemeError::new(format!(
                "The color for \"{key}\" must be a literal CSS color such as oklch(0.62 0.2 280)."
            )));
        };
        overrides.push((role, normalized));
    }
    if overrides.is_empty() {
        return Err(ThemeError::new(
            "Add at least one color role to the theme file.",
        ));
    }
    Ok(overrides)
}

/// `parseThemeFile`: validates a user-supplied theme file and fills in missing roles.
pub fn parse_theme_file(value: &Value) -> Result<ThemeDefinition, ThemeError> {
    let Some(object) = value.as_object() else {
        return Err(ThemeError::new("Theme files must contain a JSON object."));
    };
    if object.get("version").and_then(Value::as_f64) != Some(f64::from(THEME_FILE_VERSION)) {
        return Err(ThemeError::new(format!(
            "This theme file uses an unsupported version. Expected {THEME_FILE_VERSION}."
        )));
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| is_theme_label(n));
    let Some(name) = name else {
        return Err(ThemeError::new(
            "Theme files need a name (48 characters or fewer).",
        ));
    };
    let Some(appearance) = object
        .get("appearance")
        .and_then(Value::as_str)
        .and_then(Appearance::parse)
    else {
        return Err(ThemeError::new(
            "Theme files need an appearance of \"light\" or \"dark\".",
        ));
    };
    let Some(raw_colors) = object.get("colors").filter(|c| c.is_object()) else {
        return Err(ThemeError::new("Theme files need a colors object."));
    };

    let id = match object.get("id") {
        None => theme_id_from_name(name),
        Some(Value::String(id)) if is_theme_id(id) => id.clone(),
        Some(_) => {
            return Err(ThemeError::new(
                "Theme ids may only contain lowercase letters, numbers, and hyphens.",
            ));
        }
    };
    if is_reserved_theme_id(&id) {
        return Err(ThemeError::new(format!(
            "The theme id \"{id}\" is reserved."
        )));
    }

    let overrides = parse_color_overrides(raw_colors)?;
    let collection = parse_collection(object.get("collection"));
    if object.get("collection").is_some() && collection.is_none() {
        return Err(ThemeError::new(
            "Theme collections need a valid id and label.",
        ));
    }

    if object.contains_key("css") && theme_css(object.get("css")).is_none() {
        return Err(ThemeError::new(format!(
            "Theme CSS must be text of {THEME_CSS_MAX_LENGTH} characters or fewer."
        )));
    }
    let mut variants = ThemeVariants::default();
    if let Some(raw_variants) = object.get("variants") {
        let Some(raw_variants) = raw_variants.as_object() else {
            return Err(ThemeError::new("Theme variants must be an object."));
        };
        for (key, colors) in raw_variants {
            let Some(variant) = Appearance::parse(key) else {
                return Err(ThemeError::new(
                    "Theme variants may only be named \"light\" or \"dark\".",
                ));
            };
            if variant == appearance {
                return Err(ThemeError::new(format!(
                    "Theme variants must not repeat the base appearance \"{}\".",
                    appearance.as_str()
                )));
            }
            let overrides = parse_color_overrides(colors)?;
            variants.set(
                variant,
                with_overrides(default_theme_colors(variant).clone(), overrides),
            );
        }
    }

    Ok(ThemeDefinition {
        id,
        label: js_trim(name).to_owned(),
        appearance,
        colors: with_overrides(default_theme_colors(appearance).clone(), overrides),
        variants: (!variants.is_empty()).then_some(variants),
        collection,
        sidebar_artwork: None,
        managed: (object.get("managed") == Some(&Value::Bool(true))).then_some(true),
        css: nonempty_css(object.get("css")),
    })
}

/// A theme in the shape of a `.json` theme file.
#[derive(Serialize)]
struct ThemeFile<'a> {
    version: u32,
    id: &'a str,
    name: &'a str,
    appearance: Appearance,
    colors: &'a ThemeColors,
    #[serde(skip_serializing_if = "Option::is_none")]
    variants: Option<&'a ThemeVariants>,
    #[serde(skip_serializing_if = "Option::is_none")]
    collection: Option<&'a ThemeCollection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    css: Option<&'a str>,
}

/// `canonicalizeThemeDefinition`: every colour as canonical OKLCH.
pub fn canonicalize_theme_definition(
    theme: &ThemeDefinition,
) -> Result<ThemeDefinition, ThemeError> {
    let decode = |colors: &ThemeColors| {
        colors.canonicalized().map_err(|role| {
            ThemeError::new(format!(
                "The color for \"{}\" must be a literal CSS color such as oklch(0.62 0.2 280).",
                role.name()
            ))
        })
    };
    let mut out = theme.clone();
    out.colors = decode(&theme.colors)?;
    if let Some(variants) = &theme.variants {
        let mut decoded = ThemeVariants::default();
        for appearance in Appearance::ALL {
            if let Some(colors) = variants.get(appearance) {
                decoded.set(appearance, decode(colors)?);
            }
        }
        out.variants = Some(decoded);
    }
    Ok(out)
}

/// `serializeThemeFile`: pretty JSON with a trailing newline.
pub fn serialize_theme_file(theme: &ThemeDefinition) -> Result<String, ThemeError> {
    let canonical = canonicalize_theme_definition(theme)?;
    let file = ThemeFile {
        version: THEME_FILE_VERSION,
        id: &canonical.id,
        name: &canonical.label,
        appearance: canonical.appearance,
        colors: &canonical.colors,
        variants: canonical.variants.as_ref(),
        collection: canonical.collection.as_ref(),
        managed: canonical.is_managed().then_some(true),
        css: canonical.css.as_deref().filter(|css| !css.is_empty()),
    };
    serde_json::to_string_pretty(&file)
        .map(|json| format!("{json}\n"))
        .map_err(|error| ThemeError::new(error.to_string()))
}

/// A palette published by the connected environment (`EnvironmentTheme` on the wire).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EnvironmentTheme {
    pub id: String,
    pub name: String,
    pub appearance: Appearance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<EnvironmentThemeVariants>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct EnvironmentThemeVariants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<Map<String, Value>>,
}

impl EnvironmentThemeVariants {
    fn entries(&self) -> impl Iterator<Item = (Appearance, &Map<String, Value>)> {
        [
            (Appearance::Light, self.light.as_ref()),
            (Appearance::Dark, self.dark.as_ref()),
        ]
        .into_iter()
        .filter_map(|(appearance, colors)| colors.map(|c| (appearance, c)))
    }
}

fn published_colors(
    theme: &EnvironmentTheme,
    appearance: Appearance,
    colors: Option<&Map<String, Value>>,
) -> ThemeColors {
    // A variant builds on that appearance's stock defaults: the generator follows the
    // seed canvas's luminance, so a dark seed would give a light variant unreadable colours.
    let base = match (&theme.canvas, &theme.accent) {
        (Some(canvas), Some(accent)) if appearance == theme.appearance => {
            create_vivid_theme_colors(appearance, canvas, accent)
        }
        _ => default_theme_colors(appearance).clone(),
    };
    with_overrides(base, colors.map(lenient_overrides).unwrap_or_default())
}

fn environment_definition(theme: &EnvironmentTheme) -> ThemeDefinition {
    let mut variants = ThemeVariants::default();
    if let Some(published) = &theme.variants {
        for (appearance, colors) in published.entries() {
            if appearance != theme.appearance {
                variants.set(
                    appearance,
                    published_colors(theme, appearance, Some(colors)),
                );
            }
        }
    }
    let pure_seed = theme.canvas.is_some()
        && theme.accent.is_some()
        && theme.colors.is_none()
        && theme.variants.is_none();
    ThemeDefinition {
        id: theme.id.clone(),
        label: theme.name.clone(),
        appearance: theme.appearance,
        colors: published_colors(theme, theme.appearance, theme.colors.as_ref()),
        variants: (!variants.is_empty()).then_some(variants),
        collection: None,
        sidebar_artwork: None,
        managed: pure_seed.then_some(true),
        css: theme.css.clone().filter(|css| !css.is_empty()),
    }
}

/// `publishedThemeDefinitions`: palettes this client can render. Reserved ids are
/// dropped so a published `dark.json` cannot capture the stock `"dark"` preference.
pub fn published_theme_definitions(themes: &[EnvironmentTheme]) -> Vec<ThemeDefinition> {
    themes
        .iter()
        .filter(|theme| {
            if is_reserved_theme_id(&theme.id) {
                return false;
            }
            if theme.canvas.is_some() && theme.accent.is_some() {
                return true;
            }
            let other = theme.appearance.other();
            let other_variant = theme.variants.as_ref().and_then(|v| match other {
                Appearance::Light => v.light.as_ref(),
                Appearance::Dark => v.dark.as_ref(),
            });
            [theme.colors.as_ref(), other_variant]
                .into_iter()
                .flatten()
                .any(|colors| !lenient_overrides(colors).is_empty())
        })
        .map(environment_definition)
        .collect()
}
