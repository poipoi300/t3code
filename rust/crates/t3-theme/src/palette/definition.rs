//! Theme data model: roles, colour sets, theme definitions and the built-in themes.

use std::collections::BTreeMap;
use std::ops::Index;
use std::sync::OnceLock;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

use super::math::to_canonical_theme_color;
use super::roles::{ROLE_COUNT, ThemeColorRole};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 2] = [Appearance::Light, Appearance::Dark];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub const fn other(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }
}

/// One colour string per role. Values are CSS colours (canonical OKLCH once they
/// have passed through [`to_canonical_theme_color`], but literal built-ins such as
/// `#fcfcfc` are kept as written).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeColors([String; ROLE_COUNT]);

impl ThemeColors {
    pub fn from_fn(mut f: impl FnMut(ThemeColorRole) -> String) -> Self {
        Self(std::array::from_fn(|i| f(ThemeColorRole::ALL[i])))
    }

    pub fn get(&self, role: ThemeColorRole) -> &str {
        &self.0[role as usize]
    }

    pub fn set(&mut self, role: ThemeColorRole, value: impl Into<String>) {
        self.0[role as usize] = value.into();
    }

    #[must_use]
    pub fn with(mut self, role: ThemeColorRole, value: impl Into<String>) -> Self {
        self.set(role, value);
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = (ThemeColorRole, &str)> {
        ThemeColorRole::ALL
            .iter()
            .map(|role| (*role, self.get(*role)))
    }

    /// `decodeThemeColors`: every role as a canonical OKLCH string, or the first role
    /// that is not a literal colour.
    pub fn canonicalized(&self) -> Result<Self, ThemeColorRole> {
        let mut out = self.clone();
        for role in ThemeColorRole::ALL {
            match to_canonical_theme_color(self.get(role)) {
                Some(color) => out.set(role, color),
                None => return Err(role),
            }
        }
        Ok(out)
    }
}

impl Index<ThemeColorRole> for ThemeColors {
    type Output = str;

    fn index(&self, role: ThemeColorRole) -> &str {
        self.get(role)
    }
}

impl Serialize for ThemeColors {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(ROLE_COUNT))?;
        for (role, value) in self.iter() {
            map.serialize_entry(role.name(), value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ThemeColors {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColorsVisitor;
        impl<'de> Visitor<'de> for ColorsVisitor {
            type Value = ThemeColors;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object with a colour for every theme role")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ThemeColors, A::Error> {
                let mut found: BTreeMap<String, String> = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    found.insert(key, value);
                }
                let mut missing = None;
                let colors = ThemeColors::from_fn(|role| {
                    found.remove(role.name()).unwrap_or_else(|| {
                        missing.get_or_insert(role.name());
                        String::new()
                    })
                });
                match missing {
                    Some(name) => Err(de::Error::custom(format!("missing theme role {name}"))),
                    None => Ok(colors),
                }
            }
        }
        deserializer.deserialize_map(ColorsVisitor)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeVariants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<ThemeColors>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<ThemeColors>,
}

impl ThemeVariants {
    pub fn get(&self, appearance: Appearance) -> Option<&ThemeColors> {
        match appearance {
            Appearance::Light => self.light.as_ref(),
            Appearance::Dark => self.dark.as_ref(),
        }
    }

    pub fn set(&mut self, appearance: Appearance, colors: ThemeColors) {
        match appearance {
            Appearance::Light => self.light = Some(colors),
            Appearance::Dark => self.dark = Some(colors),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.light.is_none() && self.dark.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeCollection {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeDefinition {
    pub id: String,
    pub label: String,
    pub appearance: Appearance,
    pub colors: ThemeColors,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<ThemeVariants>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<ThemeCollection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_artwork: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
}

impl ThemeDefinition {
    /// `getThemeColorsForMode`.
    pub fn colors_for(&self, mode: Appearance) -> Option<&ThemeColors> {
        if mode == self.appearance {
            return Some(&self.colors);
        }
        self.variants.as_ref().and_then(|v| v.get(mode))
    }

    /// `getThemeModes`.
    pub fn modes(&self) -> Vec<Appearance> {
        Appearance::ALL
            .into_iter()
            .filter(|mode| self.colors_for(*mode).is_some())
            .collect()
    }

    /// `singleAppearanceOf`.
    pub fn single_appearance(&self) -> Option<Appearance> {
        match self.modes().as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    pub fn is_managed(&self) -> bool {
        self.managed == Some(true)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChipAccentRow {
    kind: String,
    accent: String,
    css_variable: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaletteFixture {
    built_in_theme_ids: Vec<String>,
    mobile_default_theme_id: String,
    mobile_theme_ids: Vec<String>,
    reserved_theme_ids: Vec<String>,
    unpublishable_theme_ids: Vec<String>,
    built_in_themes: Vec<ThemeDefinition>,
    t3_code_light_colors: ThemeColors,
    t3_code_dark_colors: ThemeColors,
    context_chip_accents: Vec<ChipAccentRow>,
}

struct BuiltIns {
    ids: Vec<String>,
    mobile_default: String,
    mobile_ids: Vec<String>,
    reserved: Vec<String>,
    unpublishable: Vec<String>,
    themes: Vec<ThemeDefinition>,
    light: ThemeColors,
    dark: ThemeColors,
    chips: Vec<(String, String, String)>,
}

fn built_ins() -> &'static BuiltIns {
    static CELL: OnceLock<BuiltIns> = OnceLock::new();
    CELL.get_or_init(|| {
        let fixture: PaletteFixture =
            serde_json::from_str(include_str!("../../fixtures/palettes.json"))
                .unwrap_or_else(|error| unreachable_fixture(&error));
        BuiltIns {
            ids: fixture.built_in_theme_ids,
            mobile_default: fixture.mobile_default_theme_id,
            mobile_ids: fixture.mobile_theme_ids,
            reserved: fixture.reserved_theme_ids,
            unpublishable: fixture.unpublishable_theme_ids,
            themes: fixture.built_in_themes,
            light: fixture.t3_code_light_colors,
            dark: fixture.t3_code_dark_colors,
            chips: fixture
                .context_chip_accents
                .into_iter()
                .map(|row| (row.kind, row.accent, row.css_variable))
                .collect(),
        }
    })
}

#[cold]
fn unreachable_fixture(error: &serde_json::Error) -> ! {
    // The fixture is embedded at build time and checked by the drift test.
    panic!("fixtures/palettes.json is malformed: {error}")
}

pub fn built_in_theme_ids() -> &'static [String] {
    &built_ins().ids
}

pub fn built_in_themes() -> &'static [ThemeDefinition] {
    &built_ins().themes
}

pub fn mobile_default_theme_id() -> &'static str {
    &built_ins().mobile_default
}

pub fn mobile_theme_ids() -> &'static [String] {
    &built_ins().mobile_ids
}

pub fn is_reserved_theme_id(id: &str) -> bool {
    built_ins().reserved.iter().any(|reserved| reserved == id)
}

/// Ids a published theme may not use: the reserved ones plus the mobile default.
pub fn is_unpublishable_theme_id(id: &str) -> bool {
    built_ins()
        .unpublishable
        .iter()
        .any(|reserved| reserved == id)
}

pub fn built_in_theme(id: &str) -> Option<&'static ThemeDefinition> {
    built_in_themes().iter().find(|theme| theme.id == id)
}

/// The `t3-chat` theme, which supplies the defaults every other theme is filled from.
pub fn t3_chat_theme() -> &'static ThemeDefinition {
    &built_ins().themes[0]
}

/// `getDefaultThemeColors`.
pub fn default_theme_colors(appearance: Appearance) -> &'static ThemeColors {
    let theme = t3_chat_theme();
    match appearance {
        Appearance::Light => &theme.colors,
        Appearance::Dark => theme
            .variants
            .as_ref()
            .and_then(|v| v.dark.as_ref())
            .unwrap_or(&theme.colors),
    }
}

/// `T3_CODE_LIGHT_THEME_COLORS` / `T3_CODE_DARK_THEME_COLORS` as written (hex literals).
pub fn t3_code_theme_colors(appearance: Appearance) -> &'static ThemeColors {
    match appearance {
        Appearance::Light => &built_ins().light,
        Appearance::Dark => &built_ins().dark,
    }
}

/// `getStandardThemeColors`: the stock T3 Code palette, canonicalised.
pub fn standard_theme_colors(appearance: Appearance) -> &'static ThemeColors {
    static LIGHT: OnceLock<ThemeColors> = OnceLock::new();
    static DARK: OnceLock<ThemeColors> = OnceLock::new();
    let canonical = |colors: ThemeColors| colors.canonicalized().unwrap_or(colors);
    match appearance {
        Appearance::Light => LIGHT.get_or_init(|| canonical(built_ins().light.clone())),
        Appearance::Dark => DARK.get_or_init(|| {
            canonical(
                built_ins()
                    .dark
                    .clone()
                    .with(ThemeColorRole::Canvas, "#000000"),
            )
        }),
    }
}

/// `getStandardPaintedThemeColors`: the standard theme as `index.css` paints it.
pub fn standard_painted_theme_colors(appearance: Appearance) -> ThemeColors {
    let base = standard_theme_colors(appearance).clone();
    match appearance {
        Appearance::Light => base.with(ThemeColorRole::MessageSurface, "#ebebed"),
        Appearance::Dark => base
            .with(ThemeColorRole::Canvas, "#000000")
            .with(ThemeColorRole::MessageSurface, "#1e1e1e"),
    }
}

/// Context chip kinds as `(kind, accent colour, css variable)` in declaration order.
pub fn context_chip_accents() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    built_ins()
        .chips
        .iter()
        .map(|(kind, accent, variable)| (kind.as_str(), accent.as_str(), variable.as_str()))
}
