//! Port of apps/web/src/textGenerators.ts (`useTextGenerationTargets`, `textGeneratorSnapshot`,
//! `textGeneratorDisplayName`, `textGeneratorBrand`, the models atom) and
//! components/settings/useScopedModelAvailability.ts, for the settings pages' model pickers.
use super::{scoped::SettingsText, service::SettingsScopeView};
use crate::{
    ClientError,
    rpc::{RpcCallError, RpcEndpoint},
    types::EnvironmentId,
    view_logic::shell::models,
};
use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap};
use t3_contracts::{
    model_selection::ModelSelection,
    provider_instance::ProviderInstanceId,
    rpc::WsServerListTextGeneratorModelsRpc,
    server::ServerProvider,
    settings::{ServerSettings, TextGeneratorConfig, TextGeneratorModel},
};

/// One text generator's model list (`textGeneratorModelsAtom`).
#[derive(Debug, Clone, PartialEq)]
pub enum GeneratorModels {
    Loading,
    Ready(Vec<TextGeneratorModel>),
    /// The error to show (server detail: content).
    Failed(String),
}

/// Lists keyed like the web's `ModelsKey`: environment, id, kind, base URL and key.
#[derive(Default)]
pub struct GeneratorLists {
    lists: RefCell<BTreeMap<String, GeneratorModels>>,
}

/// `TEXT_GENERATOR_PRESETS` (label, kind, base URL).
const PRESETS: [(&str, &str, &str); 4] = [
    ("OpenAI", "openaiCompatible", "https://api.openai.com/v1"),
    ("OpenRouter", "openaiCompatible", "https://openrouter.ai/api/v1"),
    ("Anthropic", "anthropic", "https://api.anthropic.com/v1"),
    ("Custom (OpenAI-compatible)", "openaiCompatible", ""),
];

fn config_json(config: &TextGeneratorConfig) -> Value {
    serde_json::to_value(config).unwrap_or_default()
}

/// `textGeneratorDisplayName` (content: a label, a preset or a host).
pub fn text_generator_display_name(config: &TextGeneratorConfig) -> String {
    let json = config_json(config);
    if let Some(label) = json["label"].as_str().filter(|l| !l.is_empty()) {
        return label.to_owned();
    }
    let kind = json["kind"].as_str().unwrap_or_default();
    let base = json["baseUrl"].as_str().unwrap_or_default();
    if let Some((label, _, _)) = PRESETS.iter().find(|(_, k, url)| *url == base && *k == kind) {
        return (*label).to_owned();
    }
    url::Url::parse(base)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| base.to_owned())
}

/// `textGeneratorBrand`: the driver kind that picks the generator's picker icon.
pub fn text_generator_brand(config: &TextGeneratorConfig) -> &'static str {
    let json = config_json(config);
    if json["kind"] == "anthropic" {
        return "anthropic";
    }
    let url = url::Url::parse(json["baseUrl"].as_str().unwrap_or_default()).ok();
    let host = url.as_ref().and_then(|u| u.host_str()).unwrap_or_default().to_owned();
    if host == "openrouter.ai" || host.ends_with(".openrouter.ai") {
        return "openrouter";
    }
    if host == "api.openai.com" {
        return "openai";
    }
    if url.as_ref().and_then(|u| u.port()) == Some(11434) || host.contains("ollama") {
        return "ollama";
    }
    "openaiCompatible"
}

fn list_key(environment: &EnvironmentId, id: &ProviderInstanceId, config: &TextGeneratorConfig) -> String {
    let json = config_json(config);
    json!([environment.0, id.0, json["kind"], json["baseUrl"], json["apiKey"]]).to_string()
}

/// `textGeneratorSnapshot`: a picker stand-in for a text generator.
fn generator_snapshot(
    id: &ProviderInstanceId,
    config: &TextGeneratorConfig,
    models: &GeneratorModels,
    selected: &[String],
) -> Option<ServerProvider> {
    let mut list: Vec<Value> = match models {
        GeneratorModels::Ready(models) => models
            .iter()
            .map(|m| json!({"slug": m.id.0, "name": m.name.0, "isCustom": false, "capabilities": null}))
            .collect(),
        _ => Vec::new(),
    };
    for slug in selected {
        if !list.iter().any(|m| m["slug"] == slug.as_str()) {
            list.push(json!({"slug": slug, "name": slug, "isCustom": false, "capabilities": null}));
        }
    }
    let failure = match models {
        GeneratorModels::Failed(message) => Some(message.clone()),
        _ => None,
    };
    let mut snapshot = json!({
        "instanceId": id.0,
        "driver": text_generator_brand(config),
        "displayName": text_generator_display_name(config),
        "supportsTextGeneration": true,
        "enabled": config.enabled,
        "installed": true,
        "version": null,
        "status": if failure.is_some() { "error" } else { "ready" },
        "auth": {"status": "unknown"},
        "checkedAt": "1970-01-01T00:00:00.000Z",
        "models": list,
        "slashCommands": [],
        "skills": [],
    });
    if let Some(message) = failure {
        snapshot["message"] = json!(message);
    }
    serde_json::from_value(snapshot).ok()
}

/// What the text-generation picker offers (`useTextGenerationTargets`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextGenerationTargets {
    pub environment: Option<EnvironmentId>,
    pub providers: Vec<ServerProvider>,
}

impl TextGenerationTargets {
    /// `hasTextGenerationProvider`: an enabled, available entry.
    pub fn has_provider(&self, settings: &ServerSettings, project: Option<&t3_contracts::base_schemas::ProjectId>) -> bool {
        models::entries_for(&self.providers, settings, project)
            .iter()
            .any(|entry| entry.enabled && entry.is_available)
    }
}

/// `resolveAppModelSelectionState` over the targets' providers.
pub fn text_generation_selection(settings: &ServerSettings, providers: &[ServerProvider]) -> ModelSelection {
    models::text_generation_selection(
        &models::Settings {
            server: settings,
            legacy_preferences: None,
        },
        providers,
    )
}

/// The traits the settings pickers show for `selection` (`TraitsPicker` without prompt effort).
pub fn selection_traits(
    providers: &[ServerProvider],
    selection: &ModelSelection,
) -> crate::thread_views::composer::model_traits::ModelTraitsView {
    use crate::thread_views::composer::model_traits::{ModelTraitsInput, resolve_model_traits};
    let Some(provider) = providers.iter().find(|p| p.instance_id == selection.instance_id) else {
        return Default::default();
    };
    let capabilities = models::model_capabilities(
        provider,
        Some(&selection.model),
        models::selects_plan(selection.options.as_ref()),
    );
    let descriptors = capabilities.option_descriptors.flatten().unwrap_or_default();
    resolve_model_traits(&ModelTraitsInput {
        descriptors: &descriptors,
        selection: Some(selection),
        reported: None,
        prompt: "",
        allow_prompt_injected_effort: false,
        unavailable: false,
    })
}

/// `createModelSelection(instance, model, options)` with one option replaced.
pub fn with_option(
    selection: &ModelSelection,
    option: t3_contracts::model::ProviderOptionSelection,
) -> ModelSelection {
    let mut options = selection.options.clone().map(|o| o.0).unwrap_or_default();
    options.retain(|existing| existing.id != option.id);
    options.push(option);
    ModelSelection {
        instance_id: selection.instance_id.clone(),
        model: selection.model.clone(),
        options: Some(t3_contracts::model::ProviderOptionSelections(options)),
    }
}

/// `DefaultModelSettingsPresentation`.
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultModelPresentation {
    pub selection: Option<ModelSelection>,
    /// Set when the saved model's provider cannot run; effort stays hidden until it can.
    pub provider_note: Option<SettingsText>,
}

/// `resolveDefaultModelSettingsPresentation` (ProjectDefaultsSettings.logic.ts).
pub fn default_model_presentation(
    providers: &[ServerProvider],
    settings: &ServerSettings,
    project: Option<&t3_contracts::base_schemas::ProjectId>,
) -> DefaultModelPresentation {
    let model_settings = models::Settings {
        server: settings,
        legacy_preferences: None,
    };
    let Some(saved) = settings.default_model_selection.clone() else {
        return DefaultModelPresentation {
            selection: models::project_default(providers, settings, project)
                .map(|automatic| models::effective_default(&model_settings, providers, &automatic)),
            provider_note: None,
        };
    };
    let entries = models::entries_for(providers, settings, project);
    let entry = entries.iter().find(|e| e.snapshot.instance_id == saved.instance_id);
    let resolved = models::default_selection(providers, Some(&saved));
    let usable = entry.is_some_and(|e| e.enabled && e.is_available)
        && resolved.as_ref().is_some_and(|r| r.instance_id == saved.instance_id);
    if !usable {
        let note = match entry {
            None => SettingsText::key("This provider is not available on this environment."),
            Some(entry) if !entry.enabled => {
                SettingsText::with("{name} is turned off.", "name", entry.display_name.clone())
            }
            Some(entry) => SettingsText::with("{name} is unavailable.", "name", entry.display_name.clone()),
        };
        return DefaultModelPresentation {
            selection: Some(saved),
            provider_note: Some(note),
        };
    }
    DefaultModelPresentation {
        selection: resolved.map(|r| models::effective_default(&model_settings, providers, &r)),
        provider_note: None,
    }
}

/// `defaultModelSelectionsAreMixed` over the scope's targets.
pub fn default_models_mixed(view: &SettingsScopeView) -> bool {
    let providers: Vec<(usize, Vec<ServerProvider>)> = view
        .targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let providers = view
                .all_environments
                .iter()
                .find(|e| e.environment_id == target.environment_id)
                .and_then(|e| e.config.as_ref())
                .map(|c| c.providers.0.clone())
                .unwrap_or_default();
            (index, providers)
        })
        .collect();
    let targets: Vec<models::DefaultTarget<'_>> = view
        .targets
        .iter()
        .zip(&providers)
        .map(|(target, (_, providers))| models::DefaultTarget {
            project: target.project_id.as_ref(),
            settings: &target.settings,
            providers,
        })
        .collect();
    models::defaults_mixed(None, &targets)
}

/// `managedRuntimeModeDenial` over the connected targets (`firstManagedDenial`).
pub fn managed_runtime_mode_denial(
    view: &SettingsScopeView,
    mode: t3_contracts::provider_policy::RuntimeMode,
) -> Option<SettingsText> {
    use t3_contracts::provider_policy::RuntimeMode;
    let rank = |mode: &RuntimeMode| match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    };
    view.selected.connected.iter().find_map(|environment| {
        let policy = environment.config.as_ref()?.managed_policy.as_ref()?;
        let ceiling = policy.limits.as_ref()?.max_runtime_mode.clone()?;
        (rank(&mode) > rank(&ceiling)).then(|| SettingsText {
            key: "{organization} limits threads to {mode}.",
            args: vec![
                ("organization", policy.organization.0.clone()),
                ("mode", runtime_mode_label(&ceiling).to_owned()),
            ],
        })
    })
}

/// `runtimeModeConfig` labels and descriptions (English keys).
pub fn runtime_mode_label(mode: &t3_contracts::provider_policy::RuntimeMode) -> &'static str {
    use t3_contracts::provider_policy::RuntimeMode;
    match mode {
        RuntimeMode::ApprovalRequired => "Supervised",
        RuntimeMode::AutoAcceptEdits => "Auto-accept edits",
        RuntimeMode::Auto => "Auto",
        RuntimeMode::FullAccess => "Full access",
    }
}

pub fn runtime_mode_description(mode: &t3_contracts::provider_policy::RuntimeMode) -> &'static str {
    use t3_contracts::provider_policy::RuntimeMode;
    match mode {
        RuntimeMode::ApprovalRequired => "Ask before commands and file changes.",
        RuntimeMode::AutoAcceptEdits => "Auto-approve edits, ask before other actions.",
        RuntimeMode::Auto => "Supported providers approve routine actions; others still ask.",
        RuntimeMode::FullAccess => "Allow commands and edits without prompts.",
    }
}

impl<P: crate::platform::Platform + Clone + 'static> super::service::SettingsService<P> {
    /// `useTextGenerationTargets` for the representative environment of `view`. Missing
    /// generator lists start loading; `changes()` fires when they arrive.
    pub fn text_generation_targets(&self, view: &SettingsScopeView) -> TextGenerationTargets {
        let Some(environment) = view.selected.representative.as_ref() else {
            return TextGenerationTargets::default();
        };
        let Some(config) = environment.config.as_ref() else {
            return TextGenerationTargets::default();
        };
        let settings = view.server_settings();
        let selection = &settings.text_generation_model_selection;
        let mut providers: Vec<ServerProvider> = config
            .providers
            .0
            .iter()
            .filter(|p| p.supports_text_generation != Some(Some(false)))
            .filter(|p| !settings.text_generators.contains_key(&p.instance_id))
            .cloned()
            .collect();
        for (id, generator) in &settings.text_generators {
            if !generator.enabled {
                continue;
            }
            let key = list_key(&environment.environment_id, id, generator);
            let models = self.generator_models(&environment.environment_id, id, generator, key);
            let selected: Vec<String> = (selection.instance_id == *id && !selection.model.is_empty())
                .then(|| selection.model.clone())
                .into_iter()
                .collect();
            if let Some(snapshot) = generator_snapshot(id, generator, &models, &selected) {
                providers.push(snapshot);
            }
        }
        TextGenerationTargets {
            environment: Some(environment.environment_id.clone()),
            providers,
        }
    }

    fn generator_models(
        &self,
        environment: &EnvironmentId,
        id: &ProviderInstanceId,
        config: &TextGeneratorConfig,
        key: String,
    ) -> GeneratorModels {
        let lists = self.generator_lists();
        if let Some(models) = lists.lists.borrow().get(&key) {
            return models.clone();
        }
        lists.lists.borrow_mut().insert(key.clone(), GeneratorModels::Loading);
        let fetch = self.list_generator_models(environment, id);
        let notify = self.notifier();
        let _ = config;
        self.spawn(
            async move {
                let result = fetch.await;
                lists.lists.borrow_mut().insert(
                    key,
                    match result {
                        Ok(models) => GeneratorModels::Ready(models),
                        Err(message) => GeneratorModels::Failed(message),
                    },
                );
                notify();
            }
            .boxed_local(),
        );
        GeneratorModels::Loading
    }

    fn list_generator_models(
        &self,
        environment: &EnvironmentId,
        id: &ProviderInstanceId,
    ) -> LocalBoxFuture<'static, Result<Vec<TextGeneratorModel>, String>> {
        let session = self.session_for(environment);
        let input = serde_json::from_value(json!({ "id": id.0 }));
        async move {
            let session = session.ok_or_else(|| ClientError::Disconnected.to_string())?;
            let input = input.map_err(|e| ClientError::from(e).to_string())?;
            if !session.available::<WsServerListTextGeneratorModelsRpc>(&input) {
                return Err(ClientError::PermissionDenied {
                    method: "server.listTextGeneratorModels".into(),
                }
                .to_string());
            }
            session
                .command::<WsServerListTextGeneratorModelsRpc>(input)
                .await
                .map(|result| result.models)
                .map_err(|error| match RpcCallError::into_client_error(error) {
                    ClientError::Rejected { message, detail } => {
                        crate::error::rejection_text(&message, detail.as_deref())
                    }
                    other => other.to_string(),
                })
        }
        .boxed_local()
    }

    /// `useScopedModelDisabledReason(settings, entries, textGeneration)(instanceId, model)`.
    pub fn model_disabled_reason(
        &self,
        view: &SettingsScopeView,
        instance: &ProviderInstanceId,
        model: &str,
        text_generation: bool,
    ) -> Option<SettingsText> {
        let shown = view.server_settings();
        let shown_generator = shown.text_generators.get(instance);
        let source_driver = view
            .selected
            .representative
            .as_ref()
            .and_then(|e| e.config.as_ref())
            .and_then(|c| c.providers.0.iter().find(|p| &p.instance_id == instance))
            .map(|p| p.driver.clone());
        for target in &view.targets {
            let environment = view
                .all_environments
                .iter()
                .find(|e| e.environment_id == target.environment_id);
            let unavailable = || {
                SettingsText::with(
                    "This model is unavailable on {label}. Select that environment to choose its model separately.",
                    "label",
                    environment.map_or_else(|| "a selected environment".to_owned(), |e| e.label.clone()),
                )
            };
            let generator = target.settings.text_generators.get(instance);
            if shown_generator.is_some() || generator.is_some() {
                let same = match (generator, shown_generator) {
                    (Some(generator), Some(shown)) => {
                        generator.enabled
                            && config_json(generator)["kind"] == config_json(shown)["kind"]
                            && config_json(generator)["baseUrl"] == config_json(shown)["baseUrl"]
                    }
                    _ => false,
                };
                if !same {
                    return Some(unavailable());
                }
                continue;
            }
            let Some(providers) = environment.and_then(|e| e.config.as_ref()).map(|c| c.providers.0.clone()) else {
                continue;
            };
            let entries = models::entries_for(&providers, &target.settings, target.project_id.as_ref());
            let entry = entries.iter().find(|e| &e.snapshot.instance_id == instance);
            let options = models::options_by_instance(
                &models::Settings {
                    server: &target.settings,
                    legacy_preferences: None,
                },
                &providers,
                None,
                text_generation,
            );
            let usable = entry.is_some_and(|entry| {
                entry.enabled
                    && entry.is_available
                    && Some(&entry.snapshot.driver) == source_driver.as_ref()
            }) && options
                .get(instance)
                .is_some_and(|options| options.iter().any(|o| o.slug == model && !o.is_unavailable));
            if !usable {
                return Some(unavailable());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(value: Value) -> TextGeneratorConfig {
        serde_json::from_value(value).expect("generator config")
    }

    #[test]
    fn generators_are_named_and_branded_by_their_service() {
        let openrouter = config(json!({"kind": "openaiCompatible", "baseUrl": "https://openrouter.ai/api/v1", "apiKey": "", "enabled": true}));
        assert_eq!(text_generator_display_name(&openrouter), "OpenRouter");
        assert_eq!(text_generator_brand(&openrouter), "openrouter");
        let local = config(json!({"kind": "openaiCompatible", "baseUrl": "http://localhost:11434/v1", "apiKey": "", "enabled": true}));
        assert_eq!(text_generator_display_name(&local), "localhost:11434");
        assert_eq!(text_generator_brand(&local), "ollama");
        let named = config(json!({"kind": "anthropic", "label": "Work", "baseUrl": "https://api.anthropic.com/v1", "apiKey": "", "enabled": true}));
        assert_eq!(text_generator_display_name(&named), "Work");
        assert_eq!(text_generator_brand(&named), "anthropic");
    }

    #[test]
    fn a_generator_snapshot_keeps_the_selected_model_while_its_list_loads() {
        let generator = config(json!({"kind": "anthropic", "baseUrl": "https://api.anthropic.com/v1", "apiKey": "", "enabled": true}));
        let id = ProviderInstanceId("work".into());
        let loading = generator_snapshot(&id, &generator, &GeneratorModels::Loading, &["claude-x".into()]).expect("snapshot");
        assert_eq!(loading.models.len(), 1);
        assert_eq!(loading.models[0].slug.0, "claude-x");
        let failed = generator_snapshot(&id, &generator, &GeneratorModels::Failed("401".into()), &[]).expect("snapshot");
        assert!(failed.models.is_empty());
        assert_eq!(serde_json::to_value(&failed).unwrap_or_default()["status"], "error");
    }
}
