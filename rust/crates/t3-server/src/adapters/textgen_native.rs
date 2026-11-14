//! Provider-native text generation (`ProviderInstance.textGeneration` for Codex and Claude): the
//! selected instance's binary, environment and model as the native turn adapters resolve them
//! (`native_providers.rs` uses the same `t3-provider` functions), under the provider's
//! installation gate, run once through `t3_textgen::provider_runners`.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::model::{
    ProviderOptionDescriptor, ProviderOptionSelection, ProviderOptionSelectionValue,
};
use t3_provider::{
    domain::ProviderDomain,
    environment::{
        Environment, Platform, claude_home, claude_process_environment, codex_home_layout,
        expand_home_path,
    },
    instances::{InstanceSpec, InstanceStatus},
};
use t3_textgen::{
    CancellationToken, GenerationError, GenerationRequest,
    provider_runners::{
        ClaudeRun, CodexModel, CodexRun, DEFAULT_TEXT_GENERATION_REASONING_EFFORT,
        resolve_codex_model, run_claude, run_codex,
    },
};

/// What every native launch shares (as `native_providers.rs`'s host): the server's environment,
/// account home and cwd, and the attachments directory.
pub struct NativeHost {
    pub environment: Environment,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub attachments: PathBuf,
    pub platform: Platform,
}

impl NativeHost {
    pub fn new(context: &crate::services::DomainContext<'_>) -> Self {
        Self {
            environment: context.environment.clone(),
            home: context.account_home.clone(),
            cwd: context.local.config.cwd.clone(),
            attachments: context.local.config.paths.attachments(),
            platform: Platform::current(),
        }
    }
}

/// Runs `request` on its selected instance; an instance that is absent, unreadable, managed
/// Codex or of another driver answers `Unavailable`.
pub async fn run(
    domain: &Arc<ProviderDomain>,
    host: &NativeHost,
    request: &GenerationRequest,
    cancellation: CancellationToken,
) -> Result<String, GenerationError> {
    let id = &request.model_selection.instance_id;
    let Some(entry) = domain.instances.get(id).await else {
        return Err(GenerationError::Unavailable);
    };
    let InstanceStatus::Prepared(spec) = entry.status else {
        return Err(GenerationError::Unavailable);
    };
    let gate = domain.providers.installation_gate();
    gate.execute(id, async {
        match spec.driver.0.as_str() {
            "codex" => {
                let run = codex(domain, host, &spec, request).await?;
                run_codex(&run, request, cancellation).await
            }
            "claudeAgent" => {
                let run = claude(domain, host, &spec, request).await;
                run_claude(&run, request, cancellation).await
            }
            _ => Err(GenerationError::Unavailable),
        }
    })
    .await
}

fn text<'a>(config: &'a serde_json::Value, key: &str) -> &'a str {
    config[key].as_str().unwrap_or_default()
}

fn option<'a>(
    options: &'a [ProviderOptionSelection],
    id: &str,
) -> Option<&'a ProviderOptionSelectionValue> {
    options
        .iter()
        .find(|option| option.id == id)
        .map(|option| &option.value)
}
fn string_option(options: &[ProviderOptionSelection], id: &str) -> Option<String> {
    match option(options, id) {
        Some(ProviderOptionSelectionValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}
fn boolean_option(options: &[ProviderOptionSelection], id: &str) -> Option<bool> {
    match option(options, id) {
        Some(ProviderOptionSelectionValue::Boolean(value)) => Some(*value),
        _ => None,
    }
}

/// `makeCodexTextGeneration` for a direct (`existing`) instance.
async fn codex(
    domain: &Arc<ProviderDomain>,
    host: &NativeHost,
    spec: &Arc<InstanceSpec>,
    request: &GenerationRequest,
) -> Result<CodexRun, GenerationError> {
    let config = &spec.config;
    // The managed runtime resolves its own config and environment under its access controller.
    if config["setupMode"] == "managed" {
        return Err(GenerationError::Unavailable);
    }
    let mut environment = spec.environment(&host.environment, &host.home, host.platform);
    let configured = text(config, "homePath");
    let home_path = if t3_contracts::wire::trim_js(configured).is_empty() {
        environment
            .get("CODEX_HOME")
            .cloned()
            .unwrap_or_else(|| configured.to_owned())
    } else {
        configured.to_owned()
    };
    let layout = codex_home_layout(
        &home_path,
        text(config, "shadowHomePath"),
        &host.home,
        &host.cwd,
    );
    t3_provider::homes::materialize_codex_shadow(&layout).map_err(|_| GenerationError::Provider)?;
    if let Some(home) = &layout.effective_home {
        if host.platform == Platform::Windows {
            environment.retain(|key, _| !key.eq_ignore_ascii_case("CODEX_HOME"));
        }
        environment.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
    }
    // `resolveCodexLaunchArgs`: the environment's T3CODE_CODEX_LAUNCH_ARGS wins over the setting.
    let launch_args = environment
        .get("T3CODE_CODEX_LAUNCH_ARGS")
        .map(|value| t3_contracts::wire::trim_js(value).to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| t3_contracts::wire::trim_js(text(config, "launchArgs")).to_owned());
    let launch_argv = t3_codex_app_server::launch::tokenize_cli_args(&launch_args);
    let snapshot = domain
        .providers
        .get_providers()
        .await
        .0
        .into_iter()
        .find(|provider| provider.instance_id == spec.instance_id);
    let models: Vec<CodexModel> = snapshot
        .map(|provider| serde_json::to_value(provider).unwrap_or_default())
        .and_then(|provider| provider["models"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|model| {
            Some(CodexModel {
                slug: model["slug"].as_str()?.to_owned(),
                is_custom: model["isCustom"] == true,
                is_default: model["isDefault"] == true,
                is_legacy: model["isLegacy"] == true,
            })
        })
        .collect();
    let custom = t3_shared::project_settings::custom_model_slugs(&config["customModels"]);
    let (model, fell_back) = resolve_codex_model(&request.model_selection.model, &models, &custom);
    let options = request
        .model_selection
        .options
        .as_ref()
        .map(|options| options.0.clone())
        .unwrap_or_default();
    let reasoning_effort = (!fell_back)
        .then(|| string_option(&options, "reasoningEffort"))
        .flatten()
        .unwrap_or_else(|| DEFAULT_TEXT_GENERATION_REASONING_EFFORT.to_owned());
    let service_tier = (!fell_back)
        .then(|| {
            string_option(&options, "serviceTier").or_else(|| {
                (boolean_option(&options, "fastMode") == Some(true)).then(|| "fast".into())
            })
        })
        .flatten();
    let image_paths = request
        .attachments
        .iter()
        .filter(|attachment| {
            matches!(
                attachment,
                t3_contracts::chat_attachment::ChatAttachment::Image(_)
            )
        })
        .filter_map(|attachment| {
            t3_orchestration::ports::runs_ingestion::attachment_claims::attachment_path(
                &host.attachments,
                attachment,
            )
        })
        .filter(|path| path.is_absolute() && path.is_file())
        .collect();
    let program = match text(config, "binaryPath") {
        "" => "codex".to_owned(),
        configured => expand_home_path(configured, &host.home),
    };
    Ok(CodexRun {
        program,
        launch_argv,
        environment,
        model,
        reasoning_effort,
        service_tier,
        image_paths,
    })
}

/// `makeClaudeTextGeneration`: the catalog resolves the model, effort and option settings.
async fn claude(
    domain: &Arc<ProviderDomain>,
    host: &NativeHost,
    spec: &Arc<InstanceSpec>,
    request: &GenerationRequest,
) -> ClaudeRun {
    use t3_provider::{catalog, claude_catalog};
    let config = &spec.config;
    let mut environment = claude_process_environment(
        &host.environment,
        &spec.variables,
        &host.home,
        host.platform,
        config["entrypoint"].as_str(),
    );
    let home_path = text(config, "homePath");
    if !t3_contracts::wire::trim_js(home_path).is_empty() {
        let home = claude_home(home_path, &environment, &host.home, &host.cwd);
        environment.insert(
            "CLAUDE_CONFIG_DIR".into(),
            home.to_string_lossy().into_owned(),
        );
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    let catalog = domain
        .manifest
        .current()
        .await
        .catalog("claudeAgent", now_ms)
        .unwrap_or(catalog::Catalog {
            models: Vec::new(),
            default_chat_model: None,
        });
    let mut selection = request.model_selection.clone();
    selection.model = claude_catalog::resolve_slug(&catalog, &selection.model);
    let options = selection
        .options
        .as_ref()
        .map(|options| options.0.clone())
        .unwrap_or_default();
    let caps = claude_catalog::resolve(&catalog, &selection.model)
        .and_then(|entry| claude_catalog::ultracode(entry.model.capabilities.clone()))
        .unwrap_or_else(|| catalog::capabilities(vec![]));
    let descriptors = catalog::option_descriptors(&caps, &options);
    let boolean = |id: &str| {
        descriptors.iter().find_map(|descriptor| match descriptor {
            ProviderOptionDescriptor::Boolean(descriptor) if descriptor.id == id => {
                Some(descriptor)
            }
            _ => None,
        })
    };
    let raw_effort = string_option(&options, "effort");
    let resolved =
        claude_catalog::resolve_effort(&catalog, Some(&selection.model), raw_effort.as_deref());
    let effort = claude_catalog::normalize_effort(&catalog, &selection.model, resolved.as_deref());
    let program = match text(config, "binaryPath") {
        "" => "claude".to_owned(),
        configured => expand_home_path(configured, &host.home),
    };
    ClaudeRun {
        program,
        api_model: claude_catalog::api_model_id(&catalog, &selection),
        effort,
        thinking: boolean("thinking").and_then(|_| boolean_option(&options, "thinking")),
        fast_mode: boolean("fastMode").and_then(|d| d.current_value.flatten()) == Some(true),
        ultracode: boolean("ultracode").and_then(|d| d.current_value.flatten()),
        managed_mcp_config: managed_mcp_config(host.platform),
        environment,
    }
}

/// `hasClaudeManagedMcpConfig`.
fn managed_mcp_config(platform: Platform) -> bool {
    let path = match platform {
        Platform::Macos => "/Library/Application Support/ClaudeCode/managed-mcp.json",
        Platform::Windows => "C:\\Program Files\\ClaudeCode\\managed-mcp.json",
        Platform::Linux => "/etc/claude-code/managed-mcp.json",
    };
    Path::new(path).exists()
}
