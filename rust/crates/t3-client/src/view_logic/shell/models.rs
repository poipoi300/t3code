//! Provider-instance and model picker policy, shared by all clients. Catalogs/settings stay scoped
//! to their destination environment. Missing dynamic models can remain explicit unavailable choices;
//! the no-provider placeholder must never be persisted or dispatched.
use crate::view_logic::JsTrim;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::{EnvironmentId, ProjectId},
    model::*,
    model_selection::ModelSelection,
    provider_instance::{ProviderInstanceId, default_instance_id_for_driver},
    server::{
        ServerProvider, ServerProviderAvailability, ServerProviderModel, ServerProviderState,
    },
    settings::{ProviderModelPreferences, ProviderModelPreferencesValueValue, ServerSettings},
};
#[path = "model_composer.rs"]
mod composer;
pub use composer::*;
pub const MAX_CUSTOM_MODEL_LENGTH: usize = 256;
pub fn no_provider_selection() -> ModelSelection {
    ModelSelection {
        instance_id: ProviderInstanceId("t3code_no_provider".into()),
        model: String::new(),
        options: None,
    }
}
#[derive(Debug, Clone)]
pub struct ProviderEntry<'a> {
    pub snapshot: &'a ServerProvider,
    pub enabled: bool,
    pub is_available: bool,
    pub is_default: bool,
    pub display_name: String,
    pub accent: Option<String>,
    pub acp_agent: Option<String>,
    pub acp_icon: Option<String>,
    pub continuation_group_key: Option<&'a str>,
}
impl ProviderEntry<'_> {
    pub fn ready(&self) -> bool {
        self.enabled && self.is_available && self.snapshot.status == ServerProviderState::Ready
    }
    pub fn visible(&self) -> bool {
        self.enabled
    }
}
pub fn derive_entries(providers: &[ServerProvider]) -> Vec<ProviderEntry<'_>> {
    providers
        .iter()
        .map(|p| ProviderEntry {
            snapshot: p,
            enabled: p.enabled,
            is_available: p.availability != Some(Some(ServerProviderAvailability::Unavailable)),
            is_default: p.instance_id == default_instance_id_for_driver(&p.driver),
            display_name: crate::services::providers::provider_instance_display_name(p),
            accent: crate::services::providers::normalize_provider_accent(
                p.accent_color
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|s| s.0.as_str()),
            )
            .map(str::to_owned),
            acp_agent: None,
            acp_icon: (p.driver.0 == "acpRegistry")
                .then(|| {
                    p.icon_url
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(|s| s.0.clone())
                })
                .flatten(),
            continuation_group_key: p
                .continuation
                .as_ref()
                .and_then(Option::as_ref)
                .map(|c| c.group_key.0.as_str()),
        })
        .collect()
}
fn brand(entry: &mut ProviderEntry<'_>, settings: &ServerSettings) {
    if entry.snapshot.driver.0 != "acpRegistry" {
        return;
    }
    let Some(config) = settings
        .provider_instances
        .get(&entry.snapshot.instance_id)
        .and_then(|c| c.config.as_ref())
    else {
        return;
    };
    if config["source"] == "local" {
        entry.acp_agent = None;
        entry.acp_icon = None;
        return;
    }
    if let Some(agent) = config["agentId"]
        .as_str()
        .map(JsTrim::js_trim)
        .filter(|s| !s.is_empty())
    {
        entry.acp_agent = Some(agent.into());
    }
    if let Some(icon) = config["registryIconUrl"]
        .as_str()
        .map(JsTrim::js_trim)
        .filter(|s| !s.is_empty())
    {
        entry.acp_icon = Some(icon.into());
    }
}
pub fn project_instance_enabled(
    settings: &ServerSettings,
    project: Option<&ProjectId>,
    id: &ProviderInstanceId,
) -> bool {
    if let Some(enabled) = project
        .and_then(|p| settings.project_settings_overrides.get(p))
        .and_then(|p| p.provider_instances.as_ref())
        .and_then(|p| p.get(id))
        .and_then(|p| p.enabled)
    {
        return enabled;
    }
    let Some(config) = settings.provider_instances.get(id) else {
        return matches!(id.0.as_str(), "codex" | "claudeAgent");
    };
    let enabled = config
        .config
        .as_ref()
        .and_then(|c| c.get("enabled"))
        .and_then(serde_json::Value::as_bool);
    if config.enabled == Some(false) || enabled == Some(false) {
        return false;
    }
    config.enabled.or(enabled).unwrap_or(!matches!(
        config.driver.0.as_str(),
        "cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity"
    ))
}
pub fn entries_for<'a>(
    providers: &'a [ServerProvider],
    settings: &ServerSettings,
    project: Option<&ProjectId>,
) -> Vec<ProviderEntry<'a>> {
    let mut entries = derive_entries(providers);
    for entry in &mut entries {
        entry.enabled = project_instance_enabled(settings, project, &entry.snapshot.instance_id);
        brand(entry, settings);
    }
    let mut drivers = vec![];
    for e in &entries {
        if !drivers.contains(&e.snapshot.driver) {
            drivers.push(e.snapshot.driver.clone());
        }
    }
    entries.sort_by_key(|e| {
        (
            drivers
                .iter()
                .position(|d| d == &e.snapshot.driver)
                .unwrap_or(usize::MAX),
            !e.is_default,
        )
    });
    entries
}
pub fn entries_by_environment<'a>(
    environments: impl IntoIterator<
        Item = (
            &'a EnvironmentId,
            &'a [ServerProvider],
            Option<&'a ServerSettings>,
        ),
    >,
) -> BTreeMap<EnvironmentId, BTreeMap<ProviderInstanceId, ProviderEntry<'a>>> {
    environments
        .into_iter()
        .map(|(id, providers, settings)| {
            let mut entries = derive_entries(providers);
            if let Some(settings) = settings {
                for e in &mut entries {
                    brand(e, settings);
                }
            }
            (
                id.clone(),
                entries
                    .into_iter()
                    .map(|e| (e.snapshot.instance_id.clone(), e))
                    .collect(),
            )
        })
        .collect()
}
pub fn selectable_entry<'a, 'b>(
    entries: &'b [ProviderEntry<'a>],
    id: Option<&ProviderInstanceId>,
) -> Option<&'b ProviderEntry<'a>> {
    entries
        .iter()
        .find(|e| Some(&e.snapshot.instance_id) == id && e.enabled && e.is_available)
        .or_else(|| entries.iter().find(|e| e.ready()))
        .or_else(|| {
            entries.iter().find(|e| {
                e.enabled && e.is_available && e.snapshot.status != ServerProviderState::Error
            })
        })
}
fn default_for(driver: &str, text: bool) -> Option<&'static str> {
    (if text {
        DEFAULT_TEXT_GENERATION_MODEL_BY_PROVIDER
    } else {
        DEFAULT_MODEL_BY_PROVIDER
    })
    .iter()
    .find_map(|(d, m)| (*d == driver).then_some(*m))
}
pub fn default_instance_model(
    providers: &[ServerProvider],
    id: &ProviderInstanceId,
) -> Option<String> {
    let p = providers.iter().find(|p| &p.instance_id == id)?;
    p.models
        .iter()
        .find(|m| m.is_default == Some(Some(true)) && !m.is_custom)
        .or_else(|| p.models.iter().find(|m| !m.is_custom))
        .or_else(|| p.models.first())
        .map(|m| m.slug.0.clone())
        .or_else(|| default_for(&p.driver.0, false).map(str::to_owned))
}
pub fn default_selection(
    providers: &[ServerProvider],
    selection: Option<&ModelSelection>,
) -> Option<ModelSelection> {
    let entries = derive_entries(providers);
    let selected = selectable_entry(&entries, selection.map(|s| &s.instance_id))?;
    if let Some(selection) = selection.filter(|s| s.instance_id == selected.snapshot.instance_id) {
        return Some(selection.clone());
    }
    Some(ModelSelection {
        instance_id: selected.snapshot.instance_id.clone(),
        model: default_instance_model(providers, &selected.snapshot.instance_id)?,
        options: None,
    })
}
pub fn project_default(
    providers: &[ServerProvider],
    settings: &ServerSettings,
    project: Option<&ProjectId>,
) -> Option<ModelSelection> {
    let entries = entries_for(providers, settings, project);
    let first = entries.first().map(|e| &e.snapshot.driver);
    let same = entries
        .iter()
        .filter(|e| Some(&e.snapshot.driver) == first)
        .cloned()
        .collect::<Vec<_>>();
    let selected = selectable_entry(&same, None).or_else(|| selectable_entry(&entries, None))?;
    Some(ModelSelection {
        instance_id: selected.snapshot.instance_id.clone(),
        model: default_instance_model(providers, &selected.snapshot.instance_id)?,
        options: None,
    })
}
pub fn selectable_driver(providers: &[ServerProvider], requested: Option<&str>) -> String {
    providers
        .iter()
        .find(|p| Some(p.instance_id.0.as_str()) == requested && p.enabled)
        .or_else(|| providers.iter().find(|p| p.enabled))
        .map_or_else(|| "codex".into(), |p| p.driver.0.clone())
}
pub fn default_server_model(providers: &[ServerProvider], driver: &str) -> String {
    let id = default_instance_id_for_driver(&t3_contracts::provider_instance::ProviderDriverKind(
        driver.into(),
    ));
    default_instance_model(providers, &id)
        .or_else(|| default_for(driver, false).map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_MODEL.into())
}
pub fn models_for_driver<'a>(
    providers: &'a [ServerProvider],
    driver: &str,
) -> &'a [ServerProviderModel] {
    let id = default_instance_id_for_driver(&t3_contracts::provider_instance::ProviderDriverKind(
        driver.into(),
    ));
    providers
        .iter()
        .find(|p| p.instance_id == id)
        .map_or(&[], |p| p.models.as_slice())
}
pub use crate::services::providers::{normalize_provider_accent, should_show_instance_badge};
pub fn driver_label(driver: &str) -> String {
    let mut text = String::new();
    let mut previous_lower = false;
    let mut word_start = true;
    for c in driver.js_trim().chars() {
        if c.is_ascii_uppercase() && previous_lower {
            text.push(' ');
            word_start = true;
        }
        if matches!(c, '_' | '-') {
            if !text.ends_with(' ') {
                text.push(' ');
            }
            word_start = true;
            previous_lower = false;
            continue;
        }
        let word = c.is_ascii_alphanumeric();
        if word_start && word {
            text.push(c.to_ascii_uppercase());
        } else {
            text.push(c);
        }
        word_start = !word;
        previous_lower = c.is_ascii_lowercase();
    }
    text
}
pub struct Settings<'a> {
    pub server: &'a ServerSettings,
    pub legacy_preferences: Option<&'a ProviderModelPreferences>,
}
impl Settings<'_> {
    fn preferences(&self, id: &ProviderInstanceId) -> Option<&ProviderModelPreferencesValueValue> {
        self.server
            .provider_model_preferences
            .as_ref()
            .or(self.legacy_preferences)
            .and_then(|p| p.get(id))
    }
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelOption {
    pub slug: String,
    pub name: String,
    pub short_name: Option<String>,
    pub sub_provider: Option<String>,
    pub aliases: Vec<String>,
    pub badge_new: bool,
    pub is_custom: bool,
    pub is_default: bool,
    pub is_legacy: bool,
    pub is_unavailable: bool,
}
pub fn read_custom_models(
    settings: &ServerSettings,
    id: &ProviderInstanceId,
    driver: &str,
) -> Vec<ModelOption> {
    if driver == "antigravity" {
        return vec![];
    }
    let raw = settings
        .provider_instances
        .get(id)
        .and_then(|p| p.config.as_ref())
        .and_then(|c| c.get("customModels"))
        .and_then(serde_json::Value::as_array);
    let mut seen = BTreeSet::new();
    raw.into_iter()
        .flatten()
        .filter_map(|v| {
            let slug = v
                .as_str()
                .or_else(|| v.get("slug").and_then(serde_json::Value::as_str))?
                .js_trim();
            if slug.is_empty() || !seen.insert(slug) {
                return None;
            }
            let name = v
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(JsTrim::js_trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(slug);
            Some(ModelOption {
                slug: slug.into(),
                name: name.into(),
                short_name: short_name(name, None, driver),
                is_custom: true,
                ..Default::default()
            })
        })
        .collect()
}
fn to_option(m: &ServerProviderModel, driver: &str) -> ModelOption {
    ModelOption {
        slug: m.slug.0.clone(),
        name: m.name.0.clone(),
        short_name: short_name(
            &m.name.0,
            m.short_name
                .as_ref()
                .and_then(Option::as_ref)
                .map(|s| s.0.as_str()),
            driver,
        ),
        sub_provider: m
            .sub_provider
            .as_ref()
            .and_then(Option::as_ref)
            .map(|s| s.0.clone()),
        aliases: m
            .aliases
            .as_ref()
            .and_then(Option::as_ref)
            .into_iter()
            .flatten()
            .map(|s| s.0.clone())
            .collect(),
        badge_new: m.badge.as_ref().is_some_and(Option::is_some),
        is_custom: m.is_custom,
        is_default: m.is_default == Some(Some(true)),
        is_legacy: m.is_legacy == Some(Some(true)),
        is_unavailable: false,
    }
}
pub fn resolve_model<'a>(
    driver: &str,
    value: Option<&str>,
    options: &'a [ModelOption],
) -> Option<&'a str> {
    let value = value?.js_trim();
    if value.is_empty() {
        return None;
    }
    options
        .iter()
        .find(|o| o.slug == value)
        .or_else(|| {
            options
                .iter()
                .find(|o| o.name.to_lowercase() == value.to_lowercase())
        })
        .or_else(|| {
            options.iter().find(|o| {
                o.aliases
                    .iter()
                    .any(|a| a.to_lowercase() == value.to_lowercase())
            })
        })
        .or_else(|| {
            let normalized = MODEL_SLUG_ALIASES_BY_PROVIDER
                .iter()
                .find(|(p, _)| *p == driver)
                .and_then(|(_, aliases)| {
                    aliases
                        .iter()
                        .find_map(|(alias, slug)| (*alias == value).then_some(*slug))
                })
                .unwrap_or(value);
            options.iter().find(|o| o.slug == normalized)
        })
        .map(|o| o.slug.as_str())
}
pub fn sort_models<T>(
    models: &mut [T],
    slug: impl Fn(&T) -> &str,
    order: &[String],
    favorites: &BTreeSet<String>,
    group: bool,
) {
    let ranks = order
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect::<BTreeMap<_, _>>();
    let original = models
        .iter()
        .enumerate()
        .map(|(i, m)| (slug(m).to_string(), i))
        .collect::<BTreeMap<_, _>>();
    models.sort_by_key(|m| {
        (
            !(!group || favorites.contains(slug(m))),
            ranks.get(slug(m)).copied().unwrap_or(usize::MAX),
            original.get(slug(m)).copied().unwrap_or(usize::MAX),
        )
    });
}
pub fn provider_model_key(id: &ProviderInstanceId, slug: &str) -> String {
    format!("{}:{slug}", id.0)
}
pub fn sort_provider_models<T>(
    items: &mut [T],
    identity: impl Fn(&T) -> (&ProviderInstanceId, &str),
    order: &[ProviderInstanceId],
    favorites: &BTreeSet<String>,
    group: bool,
) {
    let ranks = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect::<BTreeMap<_, _>>();
    let original = items
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (id, slug) = identity(m);
            ((id.clone(), slug.to_string()), i)
        })
        .collect::<BTreeMap<_, _>>();
    items.sort_by_key(|m| {
        let (id, slug) = identity(m);
        (
            !(!group || favorites.contains(&provider_model_key(id, slug))),
            ranks.get(id).copied().unwrap_or(usize::MAX),
            original
                .get(&(id.clone(), slug.to_string()))
                .copied()
                .unwrap_or(usize::MAX),
        )
    });
}
pub fn options_for(
    settings: &Settings<'_>,
    provider: &ServerProvider,
    selected: Option<&str>,
) -> Vec<ModelOption> {
    let mut options = provider
        .models
        .iter()
        .filter(|m| !m.is_custom)
        .map(|m| to_option(m, &provider.driver.0))
        .collect::<Vec<_>>();
    let mut seen = options
        .iter()
        .map(|m| m.slug.clone())
        .collect::<BTreeSet<_>>();
    for custom in read_custom_models(settings.server, &provider.instance_id, &provider.driver.0) {
        if custom.slug.encode_utf16().count() <= MAX_CUSTOM_MODEL_LENGTH
            && seen.insert(custom.slug.clone())
        {
            options.push(custom);
        }
    }
    let preferences = settings.preferences(&provider.instance_id);
    let hidden = preferences
        .map(|p| p.hidden_models.as_slice())
        .unwrap_or(&[]);
    options.retain(|o| o.is_custom || !hidden.contains(&o.slug));
    sort_models(
        &mut options,
        |o| &o.slug,
        preferences.map(|p| p.model_order.as_slice()).unwrap_or(&[]),
        &BTreeSet::new(),
        false,
    );
    if matches!(
        provider.driver.0.as_str(),
        "opencode" | "antigravity" | "cursor"
    ) && let Some(slug) = selected.map(JsTrim::js_trim).filter(|s| !s.is_empty())
        && !(provider.driver.0 == "antigravity" && slug == ANTIGRAVITY_DEFAULT_MODEL)
        && crate::view_logic::chat_view::resolve_selectable_model(
            &provider.driver.0,
            Some(slug),
            &provider.models,
        )
        .is_none()
        && !hidden.iter().any(|s| s == slug)
        && !options.iter().any(|o| o.slug == slug)
    {
        options.push(ModelOption {
            slug: slug.into(),
            name: slug.into(),
            is_unavailable: true,
            ..Default::default()
        });
    }
    options
}
pub fn resolve_for_instance(
    settings: &Settings<'_>,
    providers: &[ServerProvider],
    id: &ProviderInstanceId,
    selected: Option<&str>,
    preserve: bool,
) -> Option<String> {
    let p = providers.iter().find(|p| &p.instance_id == id)?;
    let options = options_for(settings, p, if preserve { selected } else { None });
    resolve_model(&p.driver.0, selected, &options)
        .map(str::to_owned)
        .or_else(|| {
            options
                .iter()
                .find(|o| o.is_default)
                .or_else(|| options.first())
                .map(|o| o.slug.clone())
        })
}
pub fn resolve_app_selection(
    driver: &str,
    settings: &Settings<'_>,
    providers: &[ServerProvider],
    selected: Option<&str>,
) -> String {
    let driver = selectable_driver(providers, Some(driver));
    let id = default_instance_id_for_driver(&t3_contracts::provider_instance::ProviderDriverKind(
        driver.clone(),
    ));
    let options = providers
        .iter()
        .find(|p| p.instance_id == id)
        .map(|p| options_for(settings, p, selected))
        .unwrap_or_default();
    resolve_model(&driver, selected, &options)
        .map(str::to_owned)
        .unwrap_or_else(|| default_server_model(providers, &driver))
}
pub fn effective_default(
    settings: &Settings<'_>,
    providers: &[ServerProvider],
    selection: &ModelSelection,
) -> ModelSelection {
    let mut result = selection.clone();
    if let Some(model) = resolve_for_instance(
        settings,
        providers,
        &selection.instance_id,
        Some(&selection.model),
        true,
    ) {
        result.model = model;
    }
    result
}
pub struct DefaultTarget<'a> {
    pub project: Option<&'a ProjectId>,
    pub settings: &'a ServerSettings,
    pub providers: &'a [ServerProvider],
}
pub fn defaults_mixed(
    legacy: Option<&ProviderModelPreferences>,
    targets: &[DefaultTarget<'_>],
) -> bool {
    let selections = targets
        .iter()
        .map(|t| {
            let stored = t.settings.default_model_selection.clone();
            let resolved = if let Some(stored) = &stored {
                default_selection(t.providers, Some(stored))
            } else {
                project_default(t.providers, t.settings, t.project)
            };
            let effective = resolved.as_ref().map(|s| {
                effective_default(
                    &Settings {
                        server: t.settings,
                        legacy_preferences: legacy,
                    },
                    t.providers,
                    s,
                )
            });
            (stored, effective)
        })
        .collect::<Vec<_>>();
    selections.iter().any(|s| selections.first() != Some(s))
}
pub fn options_by_instance(
    settings: &Settings<'_>,
    providers: &[ServerProvider],
    selected: Option<&ModelSelection>,
    text: bool,
) -> BTreeMap<ProviderInstanceId, Vec<ModelOption>> {
    providers
        .iter()
        .filter(|p| !text || p.supports_text_generation != Some(Some(false)))
        .map(|p| {
            let mut options = options_for(
                settings,
                p,
                selected
                    .filter(|s| s.instance_id == p.instance_id)
                    .map(|s| s.model.as_str()),
            );
            if text {
                let unsupported = p
                    .models
                    .iter()
                    .filter(|m| m.supports_text_generation == Some(Some(false)))
                    .map(|m| m.slug.0.as_str())
                    .collect::<BTreeSet<_>>();
                options.retain(|o| !unsupported.contains(o.slug.as_str()));
            }
            (p.instance_id.clone(), options)
        })
        .collect()
}
pub fn selects_plan(options: Option<&ProviderOptionSelections>) -> bool {
    options.is_some_and(|o| {
        o.0.iter().any(|o| {
            o.id == "agent" && o.value == ProviderOptionSelectionValue::String("plan".into())
        })
    })
}
pub fn text_generation_selection(
    settings: &Settings<'_>,
    providers: &[ServerProvider],
) -> ModelSelection {
    let selection = &settings.server.text_generation_model_selection;
    let candidates = providers
        .iter()
        .filter(|p| p.supports_text_generation != Some(Some(false)))
        .collect::<Vec<_>>();
    let selected = candidates.iter().copied().find(|p| {
        p.instance_id == selection.instance_id
            && p.enabled
            && p.availability != Some(Some(ServerProviderAvailability::Unavailable))
    });
    let entry = selected.or_else(|| {
        candidates.iter().copied().find(|p| {
            p.enabled && p.availability != Some(Some(ServerProviderAvailability::Unavailable))
        })
    });
    let Some(entry) = entry else {
        return no_provider_selection();
    };
    let mut options = options_for(settings, entry, None);
    let unsupported = entry
        .models
        .iter()
        .filter(|m| m.supports_text_generation == Some(Some(false)))
        .map(|m| m.slug.0.as_str())
        .collect::<BTreeSet<_>>();
    options.retain(|o| !unsupported.contains(o.slug.as_str()));
    let model = resolve_model(
        &entry.driver.0,
        selected.map(|_| selection.model.as_str()),
        &options,
    )
    .or_else(|| {
        resolve_model(
            &entry.driver.0,
            default_for(&entry.driver.0, true),
            &options,
        )
    })
    .map(str::to_owned)
    .or_else(|| {
        options
            .iter()
            .find(|o| o.is_default)
            .or_else(|| options.first())
            .map(|o| o.slug.clone())
    })
    .or_else(|| {
        entry
            .models
            .iter()
            .find(|m| m.supports_text_generation != Some(Some(false)))
            .map(|m| m.slug.0.clone())
    })
    .or_else(|| default_for(&entry.driver.0, true).map(str::to_owned))
    .unwrap_or_default();
    let options = if model.is_empty() {
        None
    } else {
        dispatch_options(
            entry,
            &model,
            selected.and(selection.options.as_ref()),
            selects_plan(selection.options.as_ref()),
        )
    };
    ModelSelection {
        instance_id: entry.instance_id.clone(),
        model,
        options,
    }
}
#[path = "model_options.rs"]
mod normalization;
pub use normalization::{dispatch_options, model_capabilities, short_name};
#[derive(Clone, Default)]
pub struct PickerVisibility(std::rc::Rc<PickerMounts>);
#[derive(Default)]
struct PickerMounts {
    count: std::cell::Cell<usize>,
    revision: std::cell::Cell<u64>,
}
pub struct PickerMount(PickerVisibility);
impl PickerVisibility {
    pub fn is_open(&self) -> bool {
        self.0.count.get() > 0
    }
    pub fn revision(&self) -> u64 {
        self.0.revision.get()
    }
    pub fn register(&self) -> PickerMount {
        if !self.is_open() {
            self.0.revision.set(self.revision() + 1);
        }
        self.0.count.set(self.0.count.get() + 1);
        PickerMount(self.clone())
    }
}
impl Drop for PickerMount {
    fn drop(&mut self) {
        self.0.0.count.set(self.0.0.count.get().saturating_sub(1));
        if !self.0.is_open() {
            self.0.0.revision.set(self.0.revision() + 1);
        }
    }
}
