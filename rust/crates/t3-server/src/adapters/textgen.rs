//! Generated text for this environment (`apps/server/src/textGeneration/TextGeneration.ts`): one
//! text runner over the configured model APIs, the routing TypeScript applies before each
//! generation (fallback past an unusable or exhausted selection), and the orchestration
//! runtime's title port. Requests go only to the base URL and key saved in settings; nothing a
//! client sends reaches either.
//!
//! Provider-native generators (Codex, Claude, Cursor, Grok, OpenCode, Antigravity: TypeScript's
//! `ProviderInstance.textGeneration`) are not ported; a selection that routes to one fails with
//! a content-free `Unavailable`.
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use t3_config::settings::SettingsService;
use t3_contracts::{
    base_schemas::ProjectId, settings::ServerSettings, settings_secret_value::SettingsSecretValue,
};
use t3_textgen::{
    CancellationToken, GenerationError, GenerationRequest, TextGeneration, TextRunner,
    api::{ApiTextRunner, GeneratorSecrets},
    generation::{Generated, GenerationContext, GenerationInput, ProviderTarget, RoutingState},
    host_titles::{HostTitleGenerator, TitleRoutingSource},
    orchestration::OrchestrationTitleGenerator,
};

/// Started before orchestration (whose title port it supplies); the provider domain is bound
/// once it has started, before the server accepts requests.
pub struct TextGenAdapter {
    host: Arc<Host>,
    generation: Arc<TextGeneration>,
}

struct Host {
    settings: Arc<SettingsService>,
    #[cfg(feature = "providers")]
    providers: OnceLock<Arc<t3_provider::domain::ProviderDomain>>,
}

impl TextGenAdapter {
    pub fn start(context: &crate::services::DomainContext<'_>) -> Arc<Self> {
        Self::new(context.local.settings.clone())
    }

    pub fn new(settings: Arc<SettingsService>) -> Arc<Self> {
        let host = Arc::new(Host {
            settings,
            #[cfg(feature = "providers")]
            providers: OnceLock::new(),
        });
        let generation = Arc::new(TextGeneration::new(host.clone()));
        Arc::new(Self { host, generation })
    }

    /// Provider snapshots, the managed policy's limits and the secret resolver come from here.
    #[cfg(feature = "providers")]
    pub fn bind_providers(&self, domain: Arc<t3_provider::domain::ProviderDomain>) {
        let _ = self.host.providers.set(domain);
    }

    /// `RuntimePorts::titles`: the runtime calls `generate_with_input` for each title effect.
    pub fn title_generator(&self) -> Arc<dyn t3_orchestration::api::TitleGenerator> {
        Arc::new(OrchestrationTitleGenerator::new(HostTitleGenerator::new(
            self.generation.clone(),
            self.host.clone(),
        )))
    }

    /// One routed generation (`TextGeneration.generate`): the selection in `context` is replaced
    /// by TypeScript's fallback when it is unusable or exhausted.
    pub async fn generate(
        &self,
        context: GenerationContext,
        input: GenerationInput,
        cancellation: CancellationToken,
    ) -> Result<Generated, GenerationError> {
        let routing = self
            .host
            .snapshot(context.project_id.as_ref(), cancellation.clone())
            .await?;
        self.generation
            .generate(context, input, Some(&routing), cancellation)
            .await
    }

    /// The settings generation reads: the managed policy's limits applied, as the provider
    /// domain reads them.
    pub async fn settings(&self) -> Result<ServerSettings, GenerationError> {
        self.host.settings().await
    }

    /// Current provider snapshots in the provider service's order; empty before binding.
    pub async fn providers(&self) -> Vec<t3_contracts::server::ServerProvider> {
        self.host.providers().await
    }
}

impl Host {
    async fn settings(&self) -> Result<ServerSettings, GenerationError> {
        let settings = self
            .settings
            .snapshot()
            .await
            .map_err(|_| GenerationError::Unavailable)?;
        #[cfg(feature = "providers")]
        {
            // Unbound means the provider domain did not start: no policy to apply, so refuse.
            let domain = self.providers.get().ok_or(GenerationError::Unavailable)?;
            Ok(domain.policy.apply_limits(&settings))
        }
        #[cfg(not(feature = "providers"))]
        Ok(settings)
    }

    async fn providers(&self) -> Vec<t3_contracts::server::ServerProvider> {
        #[cfg(feature = "providers")]
        if let Some(domain) = self.providers.get() {
            return domain.providers.get_providers().await.0;
        }
        Vec::new()
    }
}

#[async_trait]
impl TextRunner for Host {
    async fn run(
        &self,
        request: &GenerationRequest,
        cancellation: CancellationToken,
    ) -> Result<String, GenerationError> {
        let settings = self.settings().await?;
        let Some(config) = settings
            .text_generators
            .get(&request.model_selection.instance_id)
            .cloned()
        else {
            return Err(GenerationError::Unavailable);
        };
        let secrets: Arc<dyn GeneratorSecrets> = Arc::new(Secrets {
            settings,
            #[cfg(feature = "providers")]
            resolver: self.providers.get().map(|domain| domain.secrets.clone()),
        });
        ApiTextRunner::new(config, secrets)?
            .run(request, cancellation)
            .await
    }
}

/// The saved key of the generator being run: a literal, or a settings secret reference read
/// through the providers' resolver.
struct Secrets {
    settings: ServerSettings,
    #[cfg(feature = "providers")]
    resolver: Option<Arc<t3_provider::secrets::SecretResolver>>,
}
#[async_trait]
impl GeneratorSecrets for Secrets {
    async fn resolve(&self, value: &SettingsSecretValue) -> Result<String, GenerationError> {
        match value {
            SettingsSecretValue::Literal(key) => Ok(key.clone()),
            #[cfg(feature = "providers")]
            SettingsSecretValue::Reference(reference) => {
                let resolver = self.resolver.as_ref().ok_or(GenerationError::Provider)?;
                resolver
                    .read(
                        &self.settings.secret_providers,
                        std::slice::from_ref(reference),
                        false,
                    )
                    .await
                    .into_iter()
                    .next()
                    .flatten()
                    .ok_or(GenerationError::Provider)
            }
            #[cfg(not(feature = "providers"))]
            SettingsSecretValue::Reference(_) => {
                let _ = &self.settings;
                Err(GenerationError::Provider)
            }
        }
    }
}

#[async_trait]
impl TitleRoutingSource for Host {
    async fn snapshot(
        &self,
        project_id: Option<&ProjectId>,
        cancellation: CancellationToken,
    ) -> Result<RoutingState, GenerationError> {
        let read = async {
            let settings = self.settings().await?;
            let providers = self.providers().await;
            routing_state(&settings, &providers, project_id, now_millis())
        };
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(GenerationError::Cancelled),
            state = read => state,
        }
    }
}

/// The inputs of TypeScript's `generate`: text generators first (a generator of that id wins),
/// then every provider instance with its probe, models and usage windows.
pub fn routing_state(
    settings: &ServerSettings,
    providers: &[t3_contracts::server::ServerProvider],
    project_id: Option<&ProjectId>,
    now_ms: f64,
) -> Result<RoutingState, GenerationError> {
    use t3_shared::project_settings::{custom_model_slugs, project_provider_instance_enabled};
    let json = serde_json::to_value(settings).map_err(|_| GenerationError::Provider)?;
    let project = project_id.map(|id| id.0.as_str());
    let mut targets: Vec<ProviderTarget> = settings
        .text_generators
        .iter()
        .map(|(id, config)| ProviderTarget {
            instance_id: id.0.to_string(),
            driver: String::new(),
            enabled: config.enabled,
            project_enabled: true,
            installed: true,
            unauthenticated: false,
            probe_error: false,
            available: true,
            text_capable: true,
            text_generator: true,
            unsupported_models: Vec::new(),
            exhausted_models: Vec::new(),
            exhausted: false,
            custom_models: Vec::new(),
        })
        .collect();
    for provider in providers {
        let snapshot = serde_json::to_value(provider).map_err(|_| GenerationError::Provider)?;
        let id = snapshot["instanceId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let driver = snapshot["driver"].as_str().unwrap_or_default().to_owned();
        let models = snapshot["models"].as_array().cloned().unwrap_or_default();
        let custom_models =
            custom_model_slugs(&json["providerInstances"][&id]["config"]["customModels"]);
        let default_model = t3_contracts::model::DEFAULT_TEXT_GENERATION_MODEL_BY_PROVIDER
            .iter()
            .chain(t3_contracts::model::DEFAULT_MODEL_BY_PROVIDER.iter())
            .find(|(candidate, _)| *candidate == driver)
            .map_or(
                t3_contracts::model::DEFAULT_TEXT_GENERATION_MODEL,
                |(_, model)| *model,
            );
        let mut candidates: Vec<String> = models
            .iter()
            .filter_map(|model| model["slug"].as_str().map(str::to_owned))
            .chain(custom_models.iter().cloned())
            .chain(std::iter::once(default_model.to_owned()))
            .collect();
        if let Some(selection) = &settings.default_model_selection {
            candidates.push(selection.model.clone());
        }
        candidates.push(settings.text_generation_model_selection.model.clone());
        candidates.sort();
        candidates.dedup();
        targets.push(ProviderTarget {
            enabled: snapshot["enabled"] == true,
            project_enabled: project_provider_instance_enabled(&json, project, &id),
            installed: snapshot["installed"] == true,
            unauthenticated: snapshot["auth"]["status"] == "unauthenticated",
            probe_error: snapshot["status"] == "error",
            available: snapshot["availability"] != "unavailable",
            text_capable: snapshot["supportsTextGeneration"] != false,
            text_generator: false,
            unsupported_models: models
                .iter()
                .filter(|model| model["supportsTextGeneration"] == false)
                .filter_map(|model| model["slug"].as_str().map(str::to_owned))
                .collect(),
            exhausted_models: candidates
                .into_iter()
                .filter(|model| exhausted_usage_window(&snapshot, now_ms, model))
                .collect(),
            exhausted: false,
            custom_models,
            instance_id: id,
            driver,
        });
    }
    Ok(RoutingState {
        providers: targets,
        default_selection: settings.default_model_selection.clone(),
    })
}

/// `exhaustedUsageWindow(provider, now, model)` (`packages/shared/src/usageLimits.ts`).
pub fn exhausted_usage_window(provider: &Value, now_ms: f64, model: &str) -> bool {
    let windows = provider["usageLimits"]["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let listed = |ids: &Value, value: &str| {
        ids.as_array()
            .is_none_or(|ids| ids.iter().any(|id| id.as_str() == Some(value)))
    };
    let provider_prefix = model.split('/').next().unwrap_or_default();
    let exhausted: Vec<&Value> = windows
        .iter()
        .filter(|window| {
            window["usedPercent"].as_f64().unwrap_or(0.0) >= 100.0
                && listed(&window["modelIds"], model)
                && listed(&window["providerIds"], provider_prefix)
                && match window["resetsAt"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                {
                    None => true,
                    Some(resets) => parse_iso_millis(resets).is_none_or(|at| at > now_ms),
                }
        })
        .collect();
    let id = |window: &&Value| window["id"].as_str().unwrap_or_default().to_owned();
    match provider["driver"].as_str() {
        Some("cursor") => {
            let api = exhausted.iter().any(|w| id(w) == "apiPercentUsed");
            let auto = exhausted.iter().any(|w| id(w) == "autoPercentUsed");
            let pooled = ["auto", "default", "grok", "composer"]
                .iter()
                .any(|prefix| model.starts_with(prefix));
            if pooled { auto && api } else { api }
        }
        Some("antigravity") => {
            let family = ["gemini-", "claude-", "gpt-"]
                .iter()
                .find(|prefix| model.starts_with(**prefix))
                .map(|prefix| prefix.trim_end_matches('-'));
            exhausted.iter().any(|window| {
                let window_id = id(window);
                let pool = ["gemini-", "claude-", "gpt-", "3p-"]
                    .iter()
                    .find(|prefix| window_id.starts_with(**prefix))
                    .map(|prefix| prefix.trim_end_matches('-'));
                match (pool, family) {
                    (None, _) | (_, None) => true,
                    (Some(pool), Some(family)) => {
                        pool == family || (pool == "3p" && matches!(family, "claude" | "gpt"))
                    }
                }
            })
        }
        Some("codex") => {
            let plan = windows
                .iter()
                .any(|w| matches!(w["id"].as_str(), Some("primary" | "secondary")));
            exhausted
                .iter()
                .any(|window| id(window) != "individual" || !plan)
        }
        Some("opencode") => exhausted
            .iter()
            .any(|window| !id(window).starts_with("go_") || model.starts_with("opencode-go/")),
        _ => !exhausted.is_empty(),
    }
}

/// `Date.parse` for the ISO 8601 forms usage windows carry (`YYYY-MM-DDTHH:MM[:SS[.fff]]` with
/// `Z` or an offset); anything else is "not finite", which TypeScript treats as still exhausted.
fn parse_iso_millis(value: &str) -> Option<f64> {
    let (date, time) = value.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let (clock, offset_minutes) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else {
        let split = time.rfind(['+', '-'])?;
        let (clock, offset) = time.split_at(split);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (hours, minutes) = offset[1..].split_once(':')?;
        (
            clock,
            sign * (hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?),
        )
    };
    let mut clock_parts = clock.splitn(3, ':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let seconds: f64 = clock_parts.next().map_or(Some(0.0), |s| s.parse().ok())?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 24 || minute > 59 {
        return None;
    }
    // Days from civil (H. Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let minutes = days * 1_440 + hour * 60 + minute - offset_minutes;
    Some(minutes as f64 * 60_000.0 + seconds * 1_000.0)
}

fn now_millis() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_windows_follow_typescript_per_driver() {
        let now = parse_iso_millis("2026-11-14T12:00:00.000Z").expect("now");
        let window = |id: &str, resets: &str| json!({"id":id,"usedPercent":100,"resetsAt":resets});
        let codex = json!({"driver":"codex","usageLimits":{"windows":[window("individual","2026-11-15T00:00:00Z"), {"id":"primary","usedPercent":10}]}});
        assert!(!exhausted_usage_window(&codex, now, "gpt-5"));
        let reset = json!({"driver":"codex","usageLimits":{"windows":[window("primary","2026-11-14T11:59:59Z")]}});
        assert!(!exhausted_usage_window(&reset, now, "gpt-5"));
        let unparsed =
            json!({"driver":"codex","usageLimits":{"windows":[window("primary","soon")]}});
        assert!(exhausted_usage_window(&unparsed, now, "gpt-5"));
        let cursor =
            json!({"driver":"cursor","usageLimits":{"windows":[window("apiPercentUsed","")]}});
        assert!(exhausted_usage_window(&cursor, now, "claude-4"));
        assert!(!exhausted_usage_window(&cursor, now, "auto"));
        let gravity =
            json!({"driver":"antigravity","usageLimits":{"windows":[window("3p-pool","")]}});
        assert!(exhausted_usage_window(&gravity, now, "claude-sonnet"));
        assert!(!exhausted_usage_window(&gravity, now, "gemini-pro"));
        let scoped = json!({"driver":"opencode","usageLimits":{"windows":[{"id":"go_x","usedPercent":100,"modelIds":["opencode-go/a"]}]}});
        assert!(exhausted_usage_window(&scoped, now, "opencode-go/a"));
        assert!(!exhausted_usage_window(&scoped, now, "opencode-go/b"));
        assert_eq!(parse_iso_millis("2026-11-14T13:00:00+01:00"), Some(now));
    }
}
