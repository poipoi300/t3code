use crate::{
    Result, invalid,
    versions::{CompatibilityPolicy, compare_versions, validate_compatibility_policy},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString as Text,
    model::*,
    server::{ServerProviderModel, ServerProviderNewBadge, ServerProviderUpdateRequiredModel},
};
use tokio::sync::{Mutex, RwLock, watch};

pub const MODEL_MANIFEST_URL: &str = "https://raw.githubusercontent.com/pingdotgg/t3code/main/apps/server/src/provider/model-manifest.json";

pub fn model(
    slug: &str,
    name: &str,
    capabilities: Option<ModelCapabilities>,
) -> ServerProviderModel {
    ServerProviderModel {
        slug: Text(slug.into()),
        name: Text(name.into()),
        short_name: None,
        sub_provider: None,
        aliases: None,
        badge: None,
        is_custom: false,
        is_default: None,
        is_legacy: None,
        supports_text_generation: None,
        capabilities,
    }
}

pub fn capabilities(descriptors: Vec<ProviderOptionDescriptor>) -> ModelCapabilities {
    ModelCapabilities {
        option_descriptors: Some(Some(descriptors)),
    }
}
pub fn descriptors(caps: &ModelCapabilities) -> &[ProviderOptionDescriptor] {
    caps.option_descriptors
        .as_ref()
        .and_then(Option::as_deref)
        .unwrap_or_default()
}

pub fn select_option(
    id: &str,
    label: &str,
    choices: &[(&str, &str)],
    default: Option<&str>,
) -> ProviderOptionDescriptor {
    ProviderOptionDescriptor::Select(SelectProviderOptionDescriptor {
        id: id.into(),
        label: label.into(),
        description: None,
        r#type: SelectProviderOptionType::Select,
        options: choices
            .iter()
            .map(|(id, label)| ProviderOptionChoice {
                id: (*id).into(),
                label: (*label).into(),
                description: None,
                is_default: if Some(*id) == default {
                    Some(Some(true))
                } else {
                    None
                },
            })
            .collect(),
        current_value: default.map(|v| Some(v.into())),
        prompt_injected_values: None,
        role: None,
    })
}

pub fn boolean_option(id: &str, label: &str, current: Option<bool>) -> ProviderOptionDescriptor {
    ProviderOptionDescriptor::Boolean(BooleanProviderOptionDescriptor {
        id: id.into(),
        label: label.into(),
        description: None,
        r#type: BooleanProviderOptionType::Boolean,
        current_value: current.map(Some),
    })
}
pub fn option_id(option: &ProviderOptionDescriptor) -> &str {
    match option {
        ProviderOptionDescriptor::Select(o) => &o.id,
        ProviderOptionDescriptor::Boolean(o) => &o.id,
    }
}

pub fn option_descriptors(
    caps: &ModelCapabilities,
    selections: &[ProviderOptionSelection],
) -> Vec<ProviderOptionDescriptor> {
    let mut selections = selections.to_vec();
    let has_ultracode = descriptors(caps)
        .iter()
        .any(|d| matches!(d,ProviderOptionDescriptor::Boolean(d) if d.id == "ultracode"));
    if has_ultracode
        && selections
            .iter()
            .find(|s| s.id == "effort")
            .is_some_and(|s| s.value == ProviderOptionSelectionValue::String("ultracode".into()))
    {
        for selection in &mut selections {
            if selection.id == "effort"
                && selection.value == ProviderOptionSelectionValue::String("ultracode".into())
            {
                selection.value = ProviderOptionSelectionValue::String("xhigh".into());
            }
        }
        if !selections.iter().any(|s| s.id == "ultracode") {
            selections.push(ProviderOptionSelection {
                id: "ultracode".into(),
                value: ProviderOptionSelectionValue::Boolean(true),
            });
        }
    }
    descriptors(caps)
        .iter()
        .cloned()
        .map(|mut d| {
            let selection = selections
                .iter()
                .find(|s| s.id == option_id(&d))
                .map(|s| &s.value);
            match &mut d {
                ProviderOptionDescriptor::Boolean(d) => {
                    if let Some(ProviderOptionSelectionValue::Boolean(value)) = selection {
                        d.current_value = Some(Some(*value));
                    }
                }
                ProviderOptionDescriptor::Select(d) => {
                    let fallback = d.current_value.clone().flatten().or_else(|| {
                        d.options
                            .iter()
                            .find(|o| o.is_default == Some(Some(true)))
                            .map(|o| o.id.clone())
                    });
                    let raw = match selection {
                        Some(ProviderOptionSelectionValue::String(v)) => Some(v.trim().to_owned()),
                        _ => d.current_value.clone().flatten(),
                    };
                    let current = match raw.filter(|s| !s.is_empty()) {
                        Some(value) if d.options.is_empty() => Some(value),
                        Some(value) if d.options.iter().any(|o| o.id == value) => {
                            if d.prompt_injected_values
                                .as_ref()
                                .and_then(Option::as_ref)
                                .is_some_and(|v| v.contains(&value))
                            {
                                d.options
                                    .iter()
                                    .find(|o| o.is_default == Some(Some(true)))
                                    .map(|o| o.id.clone())
                            } else {
                                Some(value)
                            }
                        }
                        _ => fallback,
                    };
                    d.current_value = current.map(Some);
                }
            }
            d
        })
        .collect()
}

pub fn implicit_fast_mode(
    caps: &ModelCapabilities,
    selections: &[ProviderOptionSelection],
) -> Vec<ProviderOptionSelection> {
    let mut next = selections.to_vec();
    if !next.iter().any(|s| s.id == "fastMode")
        && descriptors(caps)
            .iter()
            .any(|o| matches!(o,ProviderOptionDescriptor::Boolean(d) if d.id == "fastMode"))
    {
        next.push(ProviderOptionSelection {
            id: "fastMode".into(),
            value: ProviderOptionSelectionValue::Boolean(false),
        });
    }
    next
}

pub fn custom_models(
    built_in: &[ServerProviderModel],
    raw: &[Value],
    fallback: Option<&ModelCapabilities>,
) -> Vec<ServerProviderModel> {
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in raw {
        let slug = entry
            .as_str()
            .or_else(|| entry.get("slug").and_then(Value::as_str))
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let Some(slug) = slug else {
            continue;
        };
        if !seen.insert(slug.to_owned()) {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(slug);
        let caps = entry
            .get("capabilities")
            .and_then(|v| serde_json::from_value::<ModelCapabilities>(v.clone()).ok())
            .map(|c| capabilities(descriptors(&c).to_vec()));
        entries.push((slug.to_owned(), name.to_owned(), caps));
    }
    let mut models = built_in.to_vec();
    for m in &mut models {
        if let Some((_, name, _)) = entries
            .iter()
            .find(|(slug, name, _)| *slug == m.slug.0 && name != slug)
        {
            m.name = Text(name.clone());
            m.short_name = None;
        }
    }
    let mut seen: BTreeSet<_> = models.iter().map(|m| m.slug.0.clone()).collect();
    for (slug, name, caps) in entries {
        if !seen.insert(slug.clone()) {
            continue;
        }
        let mut m = model(&slug, &name, caps.or_else(|| fallback.cloned()));
        m.is_custom = true;
        models.push(m);
    }
    models
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<Vec<CompatibilityPolicy>>,
    pub current_models: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, ManifestCatalog>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCatalog {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<ManifestDefaults>,
    pub profiles: BTreeMap<String, ManifestProfile>,
    pub models: Vec<ManifestModel>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeProfile {
    effort_map: Option<BTreeMap<Text, Option<Text>>>,
    model_suffixes: Option<BTreeMap<Text, BTreeMap<Text, Text>>>,
    context_window_tokens: Option<BTreeMap<Text, f64>>,
    fixed_context_window_tokens: Option<f64>,
}

fn validate_claude_profile(adapter: Option<&Value>) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Adapter {
        claude_code: Option<ClaudeProfile>,
    }
    let decoded: Adapter = serde_json::from_value(
        adapter
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )?;
    if let Some(profile) = decoded.claude_code {
        let _ = (
            profile.effort_map,
            profile.model_suffixes,
            profile.context_window_tokens,
            profile.fixed_context_window_tokens,
        );
    }
    Ok(())
}

fn validate_claude_model(adapter: Option<&Value>) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Bounds {
        min_version: Option<String>,
        max_version_exclusive: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Adapter {
        claude_code: Option<Bounds>,
    }
    let decoded: Adapter = serde_json::from_value(
        adapter
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )?;
    if let Some(bounds) = decoded.claude_code {
        if bounds
            .min_version
            .iter()
            .chain(&bounds.max_version_exclusive)
            .any(|v| crate::versions::parse_version(v).is_none())
        {
            return Err(invalid("Claude model adapter", "invalid semantic version"));
        }
        if bounds
            .min_version
            .as_ref()
            .zip(bounds.max_version_exclusive.as_ref())
            .is_some_and(|(min, max)| !compare_versions(min, max).is_lt())
        {
            return Err(invalid(
                "Claude model adapter",
                "minimum must be below exclusive maximum",
            ));
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelStatus {
    Current,
    Legacy,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestModel {
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    pub status: ModelStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct CatalogModel {
    pub model: ServerProviderModel,
    pub status: ModelStatus,
    pub adapter: Option<Value>,
    pub profile_adapter: Option<Value>,
}
#[derive(Debug, Clone)]
pub struct Catalog {
    pub models: Vec<CatalogModel>,
    pub default_chat_model: Option<String>,
}

pub fn date_millis(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|d| d.timestamp_millis())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|d| d.and_utc().timestamp_millis())
        })
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        let mut manifest: Self = serde_json::from_str(text)?;
        manifest.normalize()?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub fn bundled() -> Result<Self> {
        Self::parse(include_str!("../data/model-manifest.json"))
    }
    fn normalize(&mut self) -> Result<()> {
        fn trim(value: &mut String) -> Result<()> {
            *value = serde_json::from_value::<Text>(Value::String(std::mem::take(value)))?.0;
            Ok(())
        }
        for policy in self.compatibility.iter_mut().flatten() {
            trim(&mut policy.driver)?;
            trim(&mut policy.t3_code_range)?;
            for value in policy
                .recommended_range
                .iter_mut()
                .chain(&mut policy.recommended_version)
            {
                trim(value)?;
            }
            for range in &mut policy.ranges {
                trim(&mut range.range)?;
            }
        }
        for catalog in self.providers.iter_mut().flat_map(|p| p.values_mut()) {
            for value in catalog.defaults.iter_mut().flat_map(|d| d.chat.iter_mut()) {
                trim(value)?;
            }
            for model in &mut catalog.models {
                trim(&mut model.slug)?;
                trim(&mut model.name)?;
                for value in model
                    .short_name
                    .iter_mut()
                    .chain(&mut model.sub_provider)
                    .chain(&mut model.profile)
                    .chain(model.aliases.iter_mut().flatten())
                {
                    trim(value)?;
                }
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(invalid("model manifest", "unsupported version"));
        }
        for policy in self.compatibility.iter().flatten() {
            validate_compatibility_policy(policy)?;
        }
        for catalog in self.providers.iter().flat_map(|p| p.values()) {
            let mut seen = BTreeSet::new();
            for m in &catalog.models {
                if m.slug.trim().is_empty()
                    || m.name.trim().is_empty()
                    || !seen.insert(&m.slug)
                    || m.profile
                        .as_ref()
                        .is_some_and(|p| !catalog.profiles.contains_key(p))
                {
                    return Err(invalid(
                        "model manifest",
                        "duplicate model or missing profile",
                    ));
                }
            }
            if catalog
                .defaults
                .as_ref()
                .and_then(|d| d.chat.as_ref())
                .is_some_and(|s| !seen.contains(s))
            {
                return Err(invalid("model manifest", "missing default model"));
            }
        }
        if let Some(catalog) = self.providers.as_ref().and_then(|p| p.get("claudeAgent")) {
            for profile in catalog.profiles.values() {
                validate_claude_profile(profile.adapter.as_ref())?;
            }
            for model in &catalog.models {
                validate_claude_model(model.adapter.as_ref())?;
            }
        }
        Ok(())
    }
    pub fn updated_at_ms(&self) -> i64 {
        self.updated_at
            .as_deref()
            .and_then(date_millis)
            .unwrap_or(0)
    }
    pub fn catalog(&self, driver: &str, now_ms: i64) -> Option<Catalog> {
        let catalog = self.providers.as_ref()?.get(driver)?;
        let default_chat_model = catalog.defaults.as_ref().and_then(|d| d.chat.clone());
        let models = catalog
            .models
            .iter()
            .map(|m| {
                let profile = m.profile.as_ref().and_then(|p| catalog.profiles.get(p));
                let mut model = model(
                    &m.slug,
                    &m.name,
                    profile.and_then(|p| p.capabilities.clone()),
                );
                model.short_name = m.short_name.clone().map(|s| Some(Text(s)));
                model.sub_provider = m.sub_provider.clone().map(|s| Some(Text(s)));
                model.aliases = m
                    .aliases
                    .clone()
                    .map(|a| Some(a.into_iter().map(Text).collect()));
                model.is_default =
                    (default_chat_model.as_ref() == Some(&m.slug)).then_some(Some(true));
                model.is_legacy = (m.status == ModelStatus::Legacy).then_some(Some(true));
                if m.status == ModelStatus::Current
                    && m.released_at
                        .as_deref()
                        .and_then(date_millis)
                        .is_some_and(|released| {
                            now_ms >= released && now_ms - released < 7 * 24 * 60 * 60 * 1000
                        })
                {
                    model.badge = Some(Some(ServerProviderNewBadge::New));
                }
                CatalogModel {
                    model,
                    status: m.status,
                    adapter: m.adapter.clone(),
                    profile_adapter: profile.and_then(|p| p.adapter.clone()),
                }
            })
            .collect();
        Some(Catalog {
            models,
            default_chat_model,
        })
    }
}

pub fn codex_family(slug: &str) -> &str {
    if slug.starts_with("openai.gpt-") {
        &slug[7..]
    } else {
        slug
    }
}

pub fn apply_catalog(
    models: &[ServerProviderModel],
    catalog: Option<&Catalog>,
    family: fn(&str) -> &str,
) -> Vec<ServerProviderModel> {
    let mut next = models.to_vec();
    for model in &mut next {
        if model.is_custom {
            continue;
        }
        let entry = catalog.and_then(|c| {
            c.models
                .iter()
                .find(|m| m.model.slug == model.slug)
                .or_else(|| {
                    c.models
                        .iter()
                        .find(|m| m.model.slug.0 == family(&model.slug.0))
                })
        });
        model.is_legacy = entry
            .filter(|e| e.status == ModelStatus::Legacy)
            .map(|_| Some(true));
    }
    let Some(requested) = catalog.and_then(|c| c.default_chat_model.as_deref()) else {
        return next;
    };
    let target = next.iter().position(|m| m.slug.0 == requested).or_else(|| {
        next.iter()
            .position(|m| !m.is_custom && family(&m.slug.0) == family(requested))
    });
    let Some(target) = target else {
        return next;
    };
    let Some(previous) = next
        .iter()
        .position(|m| m.is_default == Some(Some(true)) && m.slug != next[target].slug)
    else {
        return next;
    };
    let aliases = next[previous].aliases.take().flatten().unwrap_or_default();
    next[previous].is_default = None;
    next[target].is_default = Some(Some(true));
    let mut moved = next[target].aliases.take().flatten().unwrap_or_default();
    for alias in aliases {
        if !moved.contains(&alias) {
            moved.push(alias);
        }
    }
    if !moved.is_empty() {
        next[target].aliases = Some(Some(moved));
    }
    next
}

pub fn codex_update_required(
    models: &[ServerProviderModel],
    catalog: Option<&Catalog>,
    version: Option<&str>,
) -> Vec<ServerProviderUpdateRequiredModel> {
    let (Some(catalog), Some(version)) = (
        catalog,
        version.filter(|v| crate::versions::parse_version(v).is_some()),
    ) else {
        return vec![];
    };
    catalog
        .models
        .iter()
        .filter_map(|entry| {
            if entry.status != ModelStatus::Current
                || models
                    .iter()
                    .any(|m| codex_family(&m.slug.0) == codex_family(&entry.model.slug.0))
            {
                return None;
            }
            let min = entry
                .adapter
                .as_ref()?
                .get("codex")?
                .get("minVersion")?
                .as_str()?;
            if crate::versions::parse_version(min).is_none()
                || compare_versions(version, min) != std::cmp::Ordering::Less
            {
                return None;
            }
            Some(ServerProviderUpdateRequiredModel {
                slug: entry.model.slug.clone(),
                name: entry.model.name.clone(),
                badge: entry.model.badge,
                min_version: Text(min.into()),
            })
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestCache {
    pub fetched_at_ms: i64,
    pub manifest: Manifest,
}

struct ManifestState {
    manifest: Arc<Manifest>,
    fetched_at_ms: Option<i64>,
    last_attempt_ms: Option<i64>,
}
#[derive(Clone)]
pub struct ManifestSource(watch::Receiver<Arc<Manifest>>);
impl ManifestSource {
    pub fn subscribe(&self) -> watch::Receiver<Arc<Manifest>> {
        self.0.clone()
    }
    pub fn current(&self) -> Arc<Manifest> {
        self.0.borrow().clone()
    }
}
impl From<Arc<Manifest>> for ManifestSource {
    fn from(manifest: Arc<Manifest>) -> Self {
        Self(watch::channel(manifest).1)
    }
}
impl From<Arc<ModelManifest>> for ManifestSource {
    fn from(manifest: Arc<ModelManifest>) -> Self {
        manifest.source()
    }
}
pub struct ModelManifest {
    state: RwLock<ManifestState>,
    gate: Mutex<()>,
    client: reqwest::Client,
    url: String,
    published: watch::Sender<Arc<Manifest>>,
}

impl ModelManifest {
    pub fn new(client: reqwest::Client, cache: Option<ManifestCache>) -> Result<Self> {
        let bundled = Manifest::bundled()?;
        let cache = cache.and_then(|mut cache| {
            cache.manifest.normalize().ok()?;
            Some(cache)
        });
        let (manifest, fetched_at_ms) = match cache {
            Some(cache)
                if cache.manifest.validate().is_ok()
                    && cache.manifest.updated_at_ms() >= bundled.updated_at_ms() =>
            {
                (cache.manifest, Some(cache.fetched_at_ms))
            }
            _ => (bundled, None),
        };
        let manifest = Arc::new(manifest);
        let (published, _) = watch::channel(manifest.clone());
        Ok(Self {
            state: RwLock::new(ManifestState {
                manifest,
                fetched_at_ms,
                last_attempt_ms: None,
            }),
            gate: Mutex::new(()),
            client,
            url: MODEL_MANIFEST_URL.into(),
            published,
        })
    }
    pub fn with_url(mut self, url: String) -> Self {
        self.url = url;
        self
    }
    pub async fn current(&self) -> Arc<Manifest> {
        self.published.borrow().clone()
    }
    pub fn source(&self) -> ManifestSource {
        ManifestSource(self.published.subscribe())
    }
    /// Returns a cache value for the host to persist. Settings and disk ownership stay with the host.
    pub async fn refresh(
        &self,
        now_ms: i64,
        on_demand: bool,
        force: bool,
        update_checks: bool,
    ) -> Result<Option<ManifestCache>> {
        let _gate = self.gate.lock().await;
        let within = |since: Option<i64>, window: i64| {
            since.is_some_and(|s| now_ms >= s && now_ms - s < window)
        };
        {
            let state = self.state.read().await;
            if !update_checks
                || (!force
                    && (within(
                        state.fetched_at_ms,
                        if on_demand { 300_000 } else { 3_600_000 },
                    ) || within(state.last_attempt_ms, 300_000)))
            {
                return Ok(None);
            }
        }
        self.state.write().await.last_attempt_ms = Some(now_ms);
        let fetched = async {
            let response = self
                .client
                .get(&self.url)
                .timeout(Duration::from_secs(10))
                .send()
                .await?
                .error_for_status()?;
            Manifest::parse(&response.text().await?)
        }
        .await;
        let Ok(manifest) = fetched else {
            return Ok(None);
        };
        let mut state = self.state.write().await;
        if manifest.updated_at_ms() < state.manifest.updated_at_ms() {
            return Ok(None);
        }
        state.manifest = Arc::new(manifest.clone());
        self.published.send_replace(state.manifest.clone());
        state.fetched_at_ms = Some(now_ms);
        Ok(Some(ManifestCache {
            fetched_at_ms: now_ms,
            manifest,
        }))
    }
}

pub struct CapabilityModels<'a> {
    pub listed: Vec<&'a ServerProviderModel>,
    pub omitted: usize,
}
pub fn list_capability_models<'a>(
    provider: &'a t3_contracts::server::ServerProvider,
    preferences: Option<&t3_contracts::settings::ProviderModelPreferencesValueValue>,
    search_terms: &[String],
    is_pinned: impl Fn(&ServerProviderModel) -> bool,
) -> CapabilityModels<'a> {
    let rank = preferences
        .into_iter()
        .flat_map(|preferences| preferences.model_order.iter().enumerate())
        .map(|(index, slug)| (slug.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut visible = provider
        .models
        .iter()
        .filter(|model| {
            model.is_custom
                || !preferences
                    .is_some_and(|preferences| preferences.hidden_models.contains(&model.slug.0))
        })
        .collect::<Vec<_>>();
    visible.sort_by_key(|model| {
        rank.get(model.slug.0.as_str())
            .copied()
            .unwrap_or(rank.len())
    });
    let provider_text = format!(
        "{} {} {}",
        provider.instance_id.0,
        provider.driver.0,
        provider
            .display_name
            .as_ref()
            .and_then(Option::as_ref)
            .map_or("", |name| name.0.as_str())
    )
    .to_lowercase();
    let provider_words = provider_text
        .split_whitespace()
        .chain(provider_text.split(|character: char| !character.is_alphanumeric()))
        .collect::<BTreeSet<_>>();
    let model_text = |model: &ServerProviderModel| {
        format!(
            "{} {} {}",
            model.slug.0,
            model.name.0,
            model
                .sub_provider
                .as_ref()
                .and_then(Option::as_ref)
                .map_or("", |name| name.0.as_str())
        )
        .to_lowercase()
    };
    let terms = search_terms
        .iter()
        .filter(|term| {
            !provider_words.contains(term.as_str())
                && (!provider_text.contains(term.as_str())
                    || visible
                        .iter()
                        .any(|model| model_text(model).contains(term.as_str())))
        })
        .collect::<Vec<_>>();
    if !terms.is_empty() {
        let matches = visible
            .into_iter()
            .filter(|model| {
                let text = model_text(model);
                terms.iter().all(|term| text.contains(term.as_str()))
            })
            .collect::<Vec<_>>();
        let omitted = matches.len().saturating_sub(8);
        return CapabilityModels {
            listed: matches.into_iter().take(8).collect(),
            omitted,
        };
    }
    let full = matches!(
        provider.driver.0.as_str(),
        "codex" | "claudeAgent" | "muse" | "grok"
    );
    let mut current = 0;
    let count = visible.len();
    let listed = visible
        .into_iter()
        .filter(|model| {
            let shown = model.is_legacy != Some(Some(true)) && (full || current < 8);
            if model.is_legacy != Some(Some(true)) {
                current += 1;
            }
            is_pinned(model) || shown
        })
        .collect::<Vec<_>>();
    CapabilityModels {
        omitted: count - listed.len(),
        listed,
    }
}

pub fn option_current_value(
    descriptor: &ProviderOptionDescriptor,
) -> Option<ProviderOptionSelectionValue> {
    match descriptor {
        ProviderOptionDescriptor::Boolean(descriptor) => descriptor
            .current_value
            .flatten()
            .map(ProviderOptionSelectionValue::Boolean),
        ProviderOptionDescriptor::Select(descriptor) => descriptor
            .current_value
            .as_ref()
            .and_then(Option::as_ref)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                descriptor
                    .options
                    .iter()
                    .find(|option| option.is_default == Some(Some(true)))
                    .map(|option| &option.id)
            })
            .cloned()
            .map(ProviderOptionSelectionValue::String),
    }
}
