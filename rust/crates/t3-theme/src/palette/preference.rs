//! Stored theme preferences: which theme is selected, in which appearance, and how
//! "follow the system" interacts with themes that only have one appearance.

use serde_json::Value;

use super::definition::{Appearance, ThemeDefinition, built_in_theme};
use super::files::LEGACY_T3_CHAT_DARK_THEME_ID;

/// Where a theme preference can be looked up besides the built-ins.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThemeCatalog<'a> {
    /// Themes the person installed.
    pub custom: &'a [ThemeDefinition],
    /// Palettes published by the connected environment.
    pub environment: &'a [ThemeDefinition],
}

/// `ThemePreferenceMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearanceMode {
    Light,
    Dark,
    System,
}

impl AppearanceMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    fn appearance(self) -> Option<Appearance> {
        match self {
            Self::Light => Some(Appearance::Light),
            Self::Dark => Some(Appearance::Dark),
            Self::System => None,
        }
    }
}

/// What the desktop shell is told to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopTheme {
    Light,
    Dark,
    System,
}

impl From<Appearance> for DesktopTheme {
    fn from(value: Appearance) -> Self {
        match value {
            Appearance::Light => Self::Light,
            Appearance::Dark => Self::Dark,
        }
    }
}

/// Per-appearance theme selections ("mix and match"), as stored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeHalves {
    pub light: Option<String>,
    pub dark: Option<String>,
}

impl ThemeHalves {
    pub fn get(&self, appearance: Appearance) -> Option<&str> {
        match appearance {
            Appearance::Light => self.light.as_deref(),
            Appearance::Dark => self.dark.as_deref(),
        }
    }
}

const LEGACY_THEME_ID_ALIASES: [(&str, &str); 5] = [
    (LEGACY_T3_CHAT_DARK_THEME_ID, "t3-chat"),
    ("t3-grove", "grove"),
    ("t3-ocean", "ocean"),
    ("t3-ember", "ember"),
    ("t3-iris", "iris"),
];

fn normalize_theme_id(theme_id: &str) -> &str {
    LEGACY_THEME_ID_ALIASES
        .iter()
        .find(|(legacy, _)| *legacy == theme_id)
        .map_or(theme_id, |(_, id)| id)
}

/// `canonicalThemePreference`.
pub fn canonical_theme_preference(theme: &str) -> &str {
    if theme == LEGACY_T3_CHAT_DARK_THEME_ID {
        theme
    } else {
        normalize_theme_id(theme)
    }
}

fn legacy_theme_mode(theme: &str) -> Option<Appearance> {
    (theme == LEGACY_T3_CHAT_DARK_THEME_ID).then_some(Appearance::Dark)
}

impl<'a> ThemeCatalog<'a> {
    /// `getThemeDefinition`.
    pub fn get(&self, theme: &str) -> Option<&'a ThemeDefinition> {
        let id = normalize_theme_id(theme);
        built_in_theme(id)
            .or_else(|| self.custom.iter().find(|definition| definition.id == id))
            .or_else(|| {
                self.environment
                    .iter()
                    .find(|definition| definition.id == id)
            })
    }

    /// `themeAllowsSidebarArtwork`: only built-in themes ship artwork.
    pub fn allows_sidebar_artwork(&self, theme: &str) -> bool {
        built_in_theme(normalize_theme_id(theme)).and_then(|t| t.sidebar_artwork) == Some(true)
    }

    /// `getThemePreferenceMode`.
    pub fn preference_mode(&self, theme: &str) -> Option<Appearance> {
        match theme {
            "system" => None,
            "light" => Some(Appearance::Light),
            "dark" => Some(Appearance::Dark),
            _ => legacy_theme_mode(theme)
                .or_else(|| self.get(theme).map(|definition| definition.appearance)),
        }
    }

    /// `isKnownThemePreference`.
    pub fn is_known_preference(&self, theme: &str) -> bool {
        matches!(theme, "light" | "dark" | "system") || self.get(theme).is_some()
    }

    fn requested_mode(
        theme: &str,
        follow_system: Option<bool>,
        appearance_mode: Option<AppearanceMode>,
    ) -> Option<AppearanceMode> {
        appearance_mode.or_else(|| {
            follow_system
                .unwrap_or(theme == "system")
                .then_some(AppearanceMode::System)
        })
    }

    /// `resolveThemeAppearance`: the appearance to paint.
    pub fn resolve_appearance(
        &self,
        theme: &str,
        system_dark: bool,
        follow_system: Option<bool>,
        appearance_mode: Option<AppearanceMode>,
        halves: Option<&ThemeHalves>,
    ) -> Appearance {
        let system = if system_dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        let fallback_to_theme = |wanted: Appearance| match self.get(theme) {
            Some(definition) if definition.colors_for(wanted).is_none() => definition.appearance,
            _ => wanted,
        };
        match Self::requested_mode(theme, follow_system, appearance_mode) {
            Some(AppearanceMode::System) => {
                if halves.and_then(|h| h.get(system)).is_some() {
                    return system;
                }
                fallback_to_theme(system)
            }
            Some(mode) => {
                let wanted = mode.appearance().unwrap_or(system);
                if halves.and_then(|h| h.get(wanted)).is_some() {
                    return wanted;
                }
                fallback_to_theme(wanted)
            }
            None => self.preference_mode(theme).unwrap_or(Appearance::Light),
        }
    }

    /// `resolveDesktopTheme`: what the desktop window chrome should follow.
    pub fn resolve_desktop_theme(
        &self,
        theme: &str,
        follow_system: Option<bool>,
        appearance_mode: Option<AppearanceMode>,
        halves: Option<&ThemeHalves>,
    ) -> DesktopTheme {
        match Self::requested_mode(theme, follow_system, appearance_mode) {
            Some(AppearanceMode::System) => {
                let definition = self.get(theme);
                let has = |appearance: Appearance| {
                    halves.and_then(|h| h.get(appearance)).is_some()
                        || definition.is_some_and(|d| d.colors_for(appearance).is_some())
                };
                match definition {
                    Some(d) if !has(Appearance::Light) || !has(Appearance::Dark) => {
                        d.appearance.into()
                    }
                    _ => DesktopTheme::System,
                }
            }
            Some(mode) => {
                let wanted = mode.appearance().unwrap_or(Appearance::Light);
                if halves.and_then(|h| h.get(wanted)).is_some() {
                    return wanted.into();
                }
                match self.get(theme) {
                    Some(d) if d.colors_for(wanted).is_none() => d.appearance.into(),
                    _ => wanted.into(),
                }
            }
            None => self
                .preference_mode(theme)
                .map_or(DesktopTheme::System, Into::into),
        }
    }

    /// `parseThemeHalves`: keeps only the halves that name a theme with that appearance.
    pub fn parse_halves(&self, raw: Option<&str>) -> Option<ThemeHalves> {
        let value: Value = serde_json::from_str(raw.filter(|r| !r.is_empty())?).ok()?;
        let object = value.as_object()?;
        let mut halves = ThemeHalves::default();
        for appearance in Appearance::ALL {
            let Some(theme_id) = object.get(appearance.as_str()).and_then(Value::as_str) else {
                continue;
            };
            if let Some(definition) = self.get(theme_id)
                && definition.colors_for(appearance).is_some()
            {
                let slot = match appearance {
                    Appearance::Light => &mut halves.light,
                    Appearance::Dark => &mut halves.dark,
                };
                *slot = Some(definition.id.clone());
            }
        }
        (halves.light.is_some() || halves.dark.is_some()).then_some(halves)
    }
}

/// `resolveThemeHalf`.
pub fn resolve_theme_half<'a>(
    theme: &'a str,
    halves: Option<&'a ThemeHalves>,
    appearance: Appearance,
) -> &'a str {
    halves.and_then(|h| h.get(appearance)).unwrap_or(theme)
}
