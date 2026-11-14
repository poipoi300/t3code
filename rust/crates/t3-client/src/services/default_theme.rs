//! Environment defaults apply once per generation; subsequent local choices remain authoritative.
use crate::{
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
    types::EnvironmentId,
    view_logic::shell::ui_state::PREFERENCES_NAMESPACE,
};
use t3_contracts::settings::{DefaultAppearancePreference, ServerSettings};

pub const APPLIED_DEFAULT_THEME_STORAGE_PREFIX: &str = "t3code:default-theme-applied:v2:";
pub const APPLIED_DEFAULT_APPEARANCE_STORAGE_PREFIX: &str = "t3code:default-appearance-applied:v1:";

pub fn default_theme_generation(value: &str, set_at: &str) -> String {
    if set_at.is_empty() {
        value.into()
    } else {
        format!("{value}@{set_at}")
    }
}
pub fn default_theme_to_apply(
    environment: Option<&EnvironmentId>,
    value: &str,
    set_at: &str,
    applied: Option<&str>,
    resolves: bool,
) -> Option<String> {
    if environment.is_none() || value.is_empty() || !resolves {
        return None;
    }
    let generation = default_theme_generation(value, set_at);
    (applied != Some(generation.as_str())).then_some(generation)
}
pub fn theme_sets_own_mode(settings: &ServerSettings) -> bool {
    settings.default_appearance == DefaultAppearancePreference::Unset
        || settings.default_appearance_set_at < settings.default_theme_set_at
}
#[derive(Debug, Clone, Copy)]
pub struct ResolvedTheme {
    pub single_appearance: Option<DefaultAppearancePreference>,
}
fn key(environment: &EnvironmentId, prefix: &str) -> StorageKey {
    StorageKey::new(PREFERENCES_NAMESPACE, Some(environment.clone()), prefix)
}
async fn applied(storage: &(impl Storage + ?Sized), key: StorageKey) -> Option<String> {
    storage
        .get(StorageArea::Ordinary, key)
        .await
        .ok()
        .flatten()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}
async fn record(storage: &(impl Storage + ?Sized), key: StorageKey, generation: String) {
    let _ = storage
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key,
                value: Some(generation.into_bytes()),
            }],
        )
        .await;
}

/// Re-run on destination/settings/palette changes. Callbacks report whether preference writes succeeded.
pub async fn adopt_defaults(
    storage: &(impl Storage + ?Sized),
    environment: Option<&EnvironmentId>,
    settings: &ServerSettings,
    resolve: impl Fn(&str) -> Option<ResolvedTheme>,
    mut set_theme: impl FnMut(&str) -> bool,
    mut set_appearance: impl FnMut(DefaultAppearancePreference) -> bool,
) {
    let Some(environment) = environment else {
        return;
    };
    let theme_key = key(environment, APPLIED_DEFAULT_THEME_STORAGE_PREFIX);
    let definition = resolve(&settings.default_theme);
    let applied_theme = applied(storage, theme_key.clone()).await;
    if let Some(generation) = default_theme_to_apply(
        Some(environment),
        &settings.default_theme,
        &settings.default_theme_set_at,
        applied_theme.as_deref(),
        definition.is_some(),
    ) && let Some(definition) = definition
        && set_theme(&settings.default_theme)
    {
        let mode_saved = definition
            .single_appearance
            .is_none_or(|mode| !theme_sets_own_mode(settings) || set_appearance(mode));
        if mode_saved {
            record(storage, theme_key, generation).await;
        }
    }
    let appearance = match settings.default_appearance {
        DefaultAppearancePreference::Unset => "",
        DefaultAppearancePreference::Light => "light",
        DefaultAppearancePreference::Dark => "dark",
        DefaultAppearancePreference::System => "system",
    };
    let appearance_key = key(environment, APPLIED_DEFAULT_APPEARANCE_STORAGE_PREFIX);
    let applied_appearance = applied(storage, appearance_key.clone()).await;
    if let Some(generation) = default_theme_to_apply(
        Some(environment),
        appearance,
        &settings.default_appearance_set_at,
        applied_appearance.as_deref(),
        true,
    ) && set_appearance(settings.default_appearance)
    {
        record(storage, appearance_key, generation).await;
    }
}
