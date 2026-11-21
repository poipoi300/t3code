use futures::{StreamExt, stream::LocalBoxStream};
use serde_json::{Value, json};
use t3_contracts::{
    acp_registry::{
        AcpRegistryDeleteSessionInput, AcpRegistryImportSessionInput, AcpRegistryListSessionsInput,
        AcpRegistryListSessionsResult, AcpRegistryManagedBinaryUninstallInput,
    },
    auth::AuthEnvironmentScope as Scope,
    base_schemas::ProjectId,
    device::DeviceToolVersions,
    model::PROVIDER_DISPLAY_NAMES,
    provider_instance::{
        ProviderDriverKind, ProviderInstanceConfig, ProviderInstanceId, ProviderInstanceMutation,
        default_instance_id_for_driver,
    },
    provider_setup::{
        ProviderAuthCancelInput, ProviderAuthRespondInput, ProviderAuthStartInput,
        ProviderAuthState, ProviderInstallCancelInput, ProviderInstallState, ProviderSetupInput,
    },
    rpc::{
        WsProviderAuthCancelRpc, WsProviderAuthLogoutRpc, WsProviderAuthRespondRpc,
        WsProviderAuthStartRpc, WsProviderAuthSubscribeRpc, WsProviderInstallCancelRpc,
        WsProviderInstallRemoveRpc, WsProviderInstallStartRpc, WsProviderInstallSubscribeRpc,
        WsServerDeleteAcpRegistrySessionRpc, WsServerImportAcpRegistrySessionRpc,
        WsServerListAcpRegistrySessionsRpc, WsServerRefreshProvidersRpc,
        WsServerUninstallAcpRegistryManagedBinaryRpc, WsServerUpdateSettingsPayload,
        WsServerUpdateSettingsRpc,
    },
    server::{ServerConfig, ServerProvider, ServerProviderAuthStatus, ServerProviderState},
};

use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::{RpcEndpoint, SessionEndpoint},
    services::settings::scoped::{SettingsPatch, required_scopes_for_server_settings_patch},
    types::EnvironmentId,
};

pub const DEVICE_TOOL_UPDATE_OWNERSHIP: &str = "This environment's T3 server chooses device tool versions for itself and its SSH hosts. Update that server to receive newer tool versions; updating only your browser or mobile app does not update a remote server.";

fn humanize_slug(slug: &str) -> String {
    let mut spaced = String::new();
    let mut lower = false;
    let mut separated = false;
    for character in slug.chars() {
        if character.is_ascii_uppercase() && lower {
            spaced.push(' ');
        }
        if matches!(character, '_' | '-') {
            if !separated {
                spaced.push(' ');
            }
            separated = true;
        } else {
            spaced.push(character);
            separated = false;
        }
        lower = character.is_ascii_lowercase();
    }
    let mut word_start = true;
    spaced
        .trim()
        .chars()
        .flat_map(|character| {
            let upper = word_start && (character.is_ascii_alphanumeric() || character == '_');
            word_start = !(character.is_ascii_alphanumeric() || character == '_');
            if upper {
                character.to_uppercase().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}
pub fn provider_instance_display_name(provider: &ServerProvider) -> String {
    let name = provider
        .display_name
        .as_ref()
        .and_then(Option::as_ref)
        .map(|name| name.0.trim())
        .filter(|name| !name.is_empty());
    let kind = PROVIDER_DISPLAY_NAMES
        .iter()
        .find(|(driver, _)| *driver == provider.driver.0)
        .map_or_else(
            || humanize_slug(&provider.driver.0),
            |(_, label)| label.to_string(),
        );
    if let Some(name) = name
        && name != kind
    {
        return name.into();
    }
    if provider.instance_id != default_instance_id_for_driver(&provider.driver) {
        let custom = humanize_slug(&provider.instance_id.0);
        if !custom.is_empty() {
            return custom;
        }
    }
    name.map(str::to_owned).unwrap_or(kind)
}
pub fn provider_instance_initials(label: &str) -> String {
    let label: String = label
        .chars()
        .map(|character| {
            use unicode_general_category::{GeneralCategory, get_general_category};
            if !matches!(character, '\'' | '’')
                && matches!(
                    get_general_category(character),
                    GeneralCategory::ConnectorPunctuation
                        | GeneralCategory::DashPunctuation
                        | GeneralCategory::OpenPunctuation
                        | GeneralCategory::ClosePunctuation
                        | GeneralCategory::InitialPunctuation
                        | GeneralCategory::FinalPunctuation
                        | GeneralCategory::OtherPunctuation
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect();
    let words: Vec<_> = label
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '’']))
        .filter(|word| !word.is_empty())
        .collect();
    match words.as_slice() {
        [] => String::new(),
        [word] => word.chars().take(2).collect::<String>().to_uppercase(),
        _ => words
            .iter()
            .take(2)
            .filter_map(|word| word.chars().next())
            .collect::<String>()
            .to_uppercase(),
    }
}
pub fn normalize_provider_accent(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    (value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit))
    .then_some(value)
}
pub fn provider_accent_foreground(accent: &str) -> &'static str {
    let channel = |offset| {
        accent
            .get(offset..offset + 2)
            .and_then(|value| u8::from_str_radix(value, 16).ok())
            .map_or(0.0, |value| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            })
    };
    let luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
    if (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05) {
        "#000000"
    } else {
        "#ffffff"
    }
}
pub struct InstanceBadge<'a> {
    pub driver: &'a str,
    pub accent: Option<&'a str>,
    pub acp_agent: Option<&'a str>,
    pub enabled: bool,
}
pub fn should_show_instance_badge(
    entry: &InstanceBadge<'_>,
    entries: &[InstanceBadge<'_>],
) -> bool {
    normalize_provider_accent(entry.accent).is_some()
        || entries
            .iter()
            .filter(|candidate| {
                candidate.driver == entry.driver
                    && (!entry.enabled || candidate.enabled)
                    && (entry.driver != "acpRegistry" || candidate.acp_agent == entry.acp_agent)
            })
            .take(2)
            .count()
            > 1
}
pub fn device_tool_version_labels(tools: Option<&DeviceToolVersions>) -> Vec<String> {
    let Some(tools) = tools else {
        return vec!["Device tool versions have not been checked.".into()];
    };
    [("Device hub", &tools.hub), ("Agent tools", &tools.agent)]
        .map(|(name, tool)| {
            let installed = if tool.installed_versions.is_empty() {
                "none".into()
            } else {
                tool.installed_versions.join(", ")
            };
            let running = tool
                .running_version
                .as_ref()
                .filter(|version| !version.is_empty())
                .map_or(String::new(), |version| format!("; running {version}"));
            format!(
                "{name}: installed {installed}; required {}{running}.",
                tool.required_version
            )
        })
        .into()
}
pub fn device_tool_update_policy(tools: Option<&DeviceToolVersions>) -> &'static str {
    let Some(tools) = tools else {
        return "Versions have not been checked. Reconnect the host and check versions.";
    };
    if [&tools.hub, &tools.agent].iter().any(|tool| {
        !tool.installed_versions.is_empty()
            && !tool.installed_versions.contains(&tool.required_version)
    }) {
        "Update pending. Required tools will install automatically when next used. The host needs network access; an older install is not used as a fallback."
    } else {
        "Required tools are installed automatically when needed. Checking versions does not install or start anything."
    }
}

/// Provider settings operations target one environment. The live config subscription supplies
/// the rows and observes committed settings changes.
#[derive(Clone)]
pub struct ProviderSettings<P: Platform + Clone + 'static>(ClientRuntime<P>);

impl<P: Platform + Clone + 'static> ProviderSettings<P> {
    pub fn new(client: &ClientRuntime<P>) -> Self {
        Self(client.clone())
    }

    fn session(&self, environment: &EnvironmentId) -> Result<SessionEndpoint, ClientError> {
        self.0
            .client(environment)
            .and_then(|client| client.session())
            .ok_or(ClientError::Disconnected)
    }

    pub fn grants(&self, environment: &EnvironmentId, scope: Scope) -> bool {
        self.session(environment)
            .ok()
            .and_then(|session| session.grants_scope(scope))
            == Some(true)
    }

    async fn save(
        &self,
        environment: &EnvironmentId,
        patch: SettingsPatch,
        mutation: Option<ProviderInstanceMutation>,
    ) -> Result<(), ClientError> {
        let session = self.session(environment)?;
        let mut scopes = if patch.is_empty() && mutation.is_some() {
            Vec::new()
        } else {
            required_scopes_for_server_settings_patch(&patch)
        };
        if mutation.is_some() {
            scopes.push(Scope::ProvidersManage);
        }
        if scopes
            .into_iter()
            .any(|scope| session.grants_scope(scope) != Some(true))
        {
            return Err(ClientError::PermissionDenied {
                method: "server.updateSettings".into(),
            });
        }
        let input = WsServerUpdateSettingsPayload {
            patch: serde_json::from_value(serde_json::to_value(patch)?)?,
            provider_instance_mutation: mutation,
        };
        session
            .command::<WsServerUpdateSettingsRpc>(input)
            .await
            .map_err(crate::rpc::RpcCallError::into_client_error)?;
        Ok(())
    }

    pub async fn update(
        &self,
        environment: &EnvironmentId,
        patch: SettingsPatch,
    ) -> Result<(), ClientError> {
        self.save(environment, patch, None).await
    }

    /// Seed server model preferences from this device before the first edit, as in the composer.
    pub async fn update_model_preferences(
        &self,
        environment: &EnvironmentId,
        mut patch: SettingsPatch,
    ) -> Result<(), ClientError> {
        let snapshot = self.0.store().snapshot();
        let settings = &snapshot
            .environments
            .get(environment)
            .and_then(|env| env.config.as_ref())
            .ok_or(ClientError::Disconnected)?
            .settings;
        if settings.favorites.is_none() || settings.provider_model_preferences.is_none() {
            let preferences = self.0.composer_preferences();
            preferences.load().await?;
            let legacy = preferences.snapshot();
            let mut migration = SettingsPatch::default();
            if settings.favorites.is_none() && !legacy.favorites.is_empty() {
                migration.insert("favorites".into(), serde_json::to_value(&legacy.favorites)?);
            }
            if settings.provider_model_preferences.is_none()
                && !legacy.provider_model_preferences.is_empty()
            {
                migration.insert(
                    "providerModelPreferences".into(),
                    serde_json::to_value(&legacy.provider_model_preferences)?,
                );
            }
            if !migration.is_empty() {
                patch.insert("migrateModelPreferences".into(), Value::Object(migration));
            }
        }
        self.update(environment, patch).await
    }

    pub async fn mutate_instance(
        &self,
        environment: &EnvironmentId,
        mutation: ProviderInstanceMutation,
    ) -> Result<(), ClientError> {
        self.save(environment, SettingsPatch::default(), Some(mutation))
            .await
    }

    pub async fn set_enabled(
        &self,
        environment: &EnvironmentId,
        id: ProviderInstanceId,
        enabled: bool,
    ) -> Result<(), ClientError> {
        let snapshot = self.0.store().snapshot();
        let config = snapshot
            .environments
            .get(environment)
            .and_then(|env| env.config.as_ref())
            .ok_or(ClientError::Disconnected)?;
        let provider = config.providers.0.iter().find(|p| p.instance_id == id);
        let builtin = ["codex", "claudeAgent"].contains(&id.0.as_str());
        let Some(mut instance) = config
            .settings
            .provider_instances
            .get(&id)
            .cloned()
            .or_else(|| provider.map(|p| implicit_provider_instance(&p.driver)))
            .or_else(|| {
                builtin.then(|| implicit_provider_instance(&ProviderDriverKind(id.0.clone())))
            })
        else {
            return Err(ClientError::InvalidPayload(
                "This provider instance is no longer available on this device.".into(),
            ));
        };
        if enabled && !provider_driver_allowed(config, &instance.driver.0) {
            return Err(ClientError::PermissionDenied {
                method: "server.updateSettings".into(),
            });
        }
        // A driver with a legacy config.enabled=false must be enabled at both levels.
        if enabled {
            instance = crate::view_logic::provider_readiness::turned_on_instance(&instance);
        } else {
            instance.enabled = Some(false);
        }
        let mut patch = SettingsPatch::default();
        if !enabled
            && self.grants(environment, Scope::SettingsWrite)
            && crate::services::settings::models::text_generation_selection(
                &config.settings,
                &config.providers.0,
            )
            .instance_id
                == id
        {
            let default =
                t3_contracts::settings::ServerSettings::default().text_generation_model_selection;
            patch.insert(
                "textGenerationModelSelection".into(),
                serde_json::to_value(default)?,
            );
        }
        self.save(
            environment,
            patch,
            Some(ProviderInstanceMutation::Upsert {
                instance_id: id,
                instance,
            }),
        )
        .await
    }
}

fn implicit_provider_instance(
    driver: &t3_contracts::provider_instance::ProviderDriverKind,
) -> ProviderInstanceConfig {
    ProviderInstanceConfig {
        driver: driver.clone(),
        display_name: None,
        accent_color: None,
        environment: None,
        enabled: None,
        config: None,
    }
}

pub fn provider_driver_allowed(config: &ServerConfig, driver: &str) -> bool {
    config
        .managed_policy
        .as_ref()
        .and_then(|policy| policy.limits.as_ref())
        .and_then(|limits| limits.providers.as_ref())
        .and_then(|providers| providers.allowed_drivers.as_ref())
        .is_none_or(|drivers| drivers.iter().any(|allowed| allowed.0 == driver))
}

pub fn provider_settings_enabled(instance: &ProviderInstanceConfig) -> bool {
    t3_shared::project_settings::provider_instance_enabled(
        &json!({"driver": instance.driver.0, "enabled": instance.enabled, "config": instance.config}),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatusTone {
    Normal,
    Warning,
    Error,
}

pub struct ProviderStatus<'a> {
    pub headline: &'static str,
    pub auth_label: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub fallback_detail: Option<&'static str>,
    pub tone: ProviderStatusTone,
}

pub fn provider_settings_status(
    provider: Option<&ServerProvider>,
    enabled: bool,
) -> ProviderStatus<'_> {
    use ProviderStatusTone::*;
    if !enabled {
        return ProviderStatus {
            headline: "Disabled",
            detail: None,
            fallback_detail: None,
            auth_label: None,
            tone: Normal,
        };
    }
    let Some(provider) = provider else {
        return ProviderStatus {
            headline: "Checking provider status",
            detail: None,
            fallback_detail: enabled.then_some(
                "Waiting for the server to report installation and authentication details.",
            ),
            auth_label: None,
            tone: if enabled { Warning } else { Normal },
        };
    };
    let auth_type = provider.auth.r#type.as_ref().and_then(Option::as_ref);
    let sign_in_unchecked = provider.driver.0 == "antigravity"
        && provider.installed
        && provider.status == ServerProviderState::Warning
        && provider.auth.status == ServerProviderAuthStatus::Unknown
        && auth_type
            .is_none_or(|kind| matches!(kind.0.as_str(), "oauth-personal" | "oauth-business"));
    let tone = match (enabled, sign_in_unchecked, provider.status) {
        (true, false, ServerProviderState::Warning) => Warning,
        (true, false, ServerProviderState::Error) => Error,
        _ => Normal,
    };
    let (headline, fallback_detail, show_auth) =
        if !enabled || provider.status == ServerProviderState::Disabled {
            (
                "Disabled",
                Some("This provider is installed but disabled for new sessions in T3 Code."),
                false,
            )
        } else if !provider.installed {
            ("Not found", Some("CLI not detected on PATH."), false)
        } else if provider.auth.status == ServerProviderAuthStatus::Unauthenticated {
            ("Not authenticated", None, true)
        } else if sign_in_unchecked {
            ("Installed · Sign-in not checked", None, false)
        } else if tone == Warning {
            (
                "Needs attention",
                Some("The provider is installed, but the server could not fully verify it."),
                false,
            )
        } else if tone == Error {
            (
                "Unavailable",
                Some("The provider failed its startup checks."),
                false,
            )
        } else if provider.auth.status == ServerProviderAuthStatus::Authenticated {
            ("Authenticated", None, true)
        } else {
            ("Available", None, false)
        };
    ProviderStatus {
        headline,
        detail: provider
            .message
            .as_ref()
            .and_then(Option::as_ref)
            .map(|text| text.0.as_str()),
        fallback_detail,
        auth_label: show_auth
            .then(|| {
                provider
                    .auth
                    .label
                    .as_ref()
                    .and_then(Option::as_ref)
                    .or(auth_type)
                    .map(|label| label.0.as_str())
            })
            .flatten(),
        tone,
    }
}

/// Port of buildProviderInstanceOverridePatch. An empty provider map clears the stored map.
pub fn provider_override_patch(
    settings: &t3_contracts::settings::ServerSettings,
    projects: &[t3_contracts::base_schemas::ProjectId],
    id: &ProviderInstanceId,
    enabled: Option<bool>,
) -> SettingsPatch {
    let stored = json!(settings.project_settings_overrides);
    let entries = projects
        .iter()
        .map(|project| {
            let mut entry = stored[&project.0].as_object().cloned().unwrap_or_default();
            let mut instances = entry
                .remove("providerInstances")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            let mut instance = instances
                .remove(&id.0)
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            if let Some(enabled) = enabled {
                instance.insert("enabled".into(), json!(enabled));
            } else {
                instance.remove("enabled");
            }
            if !instance.is_empty() {
                instances.insert(id.0.clone(), Value::Object(instance));
            }
            let value = if entry.is_empty() && instances.is_empty() {
                Value::Null
            } else {
                entry.insert("providerInstances".into(), Value::Object(instances));
                Value::Object(entry)
            };
            (project.0.clone(), value)
        })
        .collect::<serde_json::Map<_, _>>();
    SettingsPatch::from_iter([("projectSettingsOverrides".into(), Value::Object(entries))])
}

impl<P: Platform + Clone + 'static> ProviderSettings<P> {
    fn manage(&self, environment: &EnvironmentId) -> Result<SessionEndpoint, ClientError> {
        let session = self.session(environment)?;
        if session.grants_scope(Scope::ProvidersManage) != Some(true) {
            return Err(ClientError::PermissionDenied {
                method: "providers:manage".into(),
            });
        }
        Ok(session)
    }

    pub fn auth_state(
        &self,
        environment: &EnvironmentId,
        input: ProviderSetupInput,
    ) -> LocalBoxStream<'static, Result<ProviderAuthState, ClientError>> {
        match self.manage(environment) {
            Ok(session) => session.subscribe::<WsProviderAuthSubscribeRpc>(input),
            Err(error) => futures::stream::once(async move { Err(error) }).boxed_local(),
        }
    }

    pub fn install_state(
        &self,
        environment: &EnvironmentId,
        input: ProviderSetupInput,
    ) -> LocalBoxStream<'static, Result<ProviderInstallState, ClientError>> {
        match self.manage(environment) {
            Ok(session) => session.subscribe::<WsProviderInstallSubscribeRpc>(input),
            Err(error) => futures::stream::once(async move { Err(error) }).boxed_local(),
        }
    }

    pub async fn start_auth(
        &self,
        environment: &EnvironmentId,
        input: ProviderAuthStartInput,
    ) -> Result<ProviderAuthState, ClientError> {
        self.manage(environment)?
            .call::<WsProviderAuthStartRpc>(input)
            .await
    }

    pub async fn respond_auth(
        &self,
        environment: &EnvironmentId,
        input: ProviderAuthRespondInput,
    ) -> Result<ProviderAuthState, ClientError> {
        self.manage(environment)?
            .call::<WsProviderAuthRespondRpc>(input)
            .await
    }

    pub async fn cancel_auth(
        &self,
        environment: &EnvironmentId,
        input: ProviderAuthCancelInput,
    ) -> Result<ProviderAuthState, ClientError> {
        self.manage(environment)?
            .call::<WsProviderAuthCancelRpc>(input)
            .await
    }

    pub async fn logout(
        &self,
        environment: &EnvironmentId,
        input: ProviderSetupInput,
    ) -> Result<ProviderAuthState, ClientError> {
        self.manage(environment)?
            .call::<WsProviderAuthLogoutRpc>(input)
            .await
    }

    pub async fn start_install(
        &self,
        environment: &EnvironmentId,
        input: ProviderSetupInput,
    ) -> Result<ProviderInstallState, ClientError> {
        self.manage(environment)?
            .call::<WsProviderInstallStartRpc>(input)
            .await
    }

    pub async fn cancel_install(
        &self,
        environment: &EnvironmentId,
        input: ProviderInstallCancelInput,
    ) -> Result<ProviderInstallState, ClientError> {
        self.manage(environment)?
            .call::<WsProviderInstallCancelRpc>(input)
            .await
    }

    pub async fn remove_install(
        &self,
        environment: &EnvironmentId,
        input: ProviderSetupInput,
    ) -> Result<ProviderInstallState, ClientError> {
        self.manage(environment)?
            .call::<WsProviderInstallRemoveRpc>(input)
            .await
    }
}

/// Port of AddUsageLimitSourceDialog.logic.ts.
pub fn usage_source_id(
    url: &str,
    sources: &std::collections::BTreeMap<
        t3_contracts::usage_limit_source_id::UsageLimitSourceId,
        t3_contracts::settings::UsageLimitSourceConfig,
    >,
) -> t3_contracts::usage_limit_source_id::UsageLimitSourceId {
    use t3_contracts::usage_limit_source_id::UsageLimitSourceId;
    if let Some((id, _)) = sources.iter().find(|(_, source)| source.url.0 == url) {
        return id.clone();
    }
    let host = url::Url::parse(url).ok().and_then(|parsed| {
        let host = parsed.host_str()?;
        Some(match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        })
    });
    let mut slug = String::new();
    let mut separator = false;
    for character in host.as_deref().unwrap_or(url).to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '-') {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    let slug = slug.trim_matches('-');
    let base = format!("cliproxy-{}", if slug.is_empty() { "hub" } else { slug });
    let mut id = UsageLimitSourceId(base.clone());
    let mut suffix = 2;
    while sources.contains_key(&id) {
        id = UsageLimitSourceId(format!("{base}-{suffix}"));
        suffix += 1;
    }
    id
}

pub fn load_preference(weight: Option<i64>) -> &'static str {
    match weight {
        None | Some(50) => "50",
        Some(0) => "0",
        Some(weight) if weight < 50 => "25",
        Some(_) => "100",
    }
}

impl<P: Platform + Clone + 'static> ProviderSettings<P> {
    pub async fn refresh(&self, environment: &EnvironmentId) -> Result<(), ClientError> {
        let input = serde_json::from_value(json!({"refreshModels":true,"reloadSecrets":true}))?;
        self.session(environment)?
            .call::<WsServerRefreshProvidersRpc>(input)
            .await?;
        Ok(())
    }

    pub async fn list_sessions(
        &self,
        environment: &EnvironmentId,
        input: AcpRegistryListSessionsInput,
    ) -> Result<AcpRegistryListSessionsResult, ClientError> {
        self.session(environment)?
            .call::<WsServerListAcpRegistrySessionsRpc>(input)
            .await
    }

    pub async fn import_session(
        &self,
        environment: &EnvironmentId,
        input: AcpRegistryImportSessionInput,
    ) -> Result<crate::types::ThreadId, ClientError> {
        let result = self
            .session(environment)?
            .call::<WsServerImportAcpRegistrySessionRpc>(input)
            .await?;
        Ok(result.thread_id)
    }

    pub async fn uninstall_registry_binary(
        &self,
        environment: &EnvironmentId,
        input: AcpRegistryManagedBinaryUninstallInput,
    ) -> Result<(), ClientError> {
        self.manage(environment)?
            .call::<WsServerUninstallAcpRegistryManagedBinaryRpc>(input)
            .await
            .map(|_| ())
    }

    pub async fn delete_session(
        &self,
        environment: &EnvironmentId,
        input: AcpRegistryDeleteSessionInput,
    ) -> Result<(), ClientError> {
        self.session(environment)?
            .call::<WsServerDeleteAcpRegistrySessionRpc>(input)
            .await?;
        Ok(())
    }
}

pub fn provider_instance_id_error(id: &str, used: bool) -> Option<&'static str> {
    if id.is_empty() {
        return Some("Instance ID is required.");
    }
    if id.len() > 64 {
        return Some("Instance ID must be 64 characters or fewer.");
    }
    let mut bytes = id.bytes();
    if !bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        || bytes.any(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
    {
        return Some(
            "Instance ID must start with a letter and use only letters, digits, '-', or '_'.",
        );
    }
    used.then_some("An instance named '{id}' already exists.")
}

#[derive(Clone)]
pub struct ProviderSettingsRow {
    pub id: ProviderInstanceId,
    pub driver: String,
    pub name: String,
    pub enabled: bool,
    pub project_override: Option<bool>,
    pub provider: Option<ServerProvider>,
}

impl PartialEq for ProviderSettingsRow {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.driver == other.driver
            && self.name == other.name
            && self.enabled == other.enabled
            && self.project_override == other.project_override
            && match (&self.provider, &other.provider) {
                (Some(previous), Some(current)) if previous.checked_at != current.checked_at => {
                    // A timestamp-only status check does not change the settings view.
                    let mut previous = previous.clone();
                    previous.checked_at = current.checked_at.clone();
                    &previous == current
                }
                (previous, current) => previous == current,
            }
    }
}

pub fn provider_settings_rows(
    config: &ServerConfig,
    projects: &[ProjectId],
    target: Option<&ProviderInstanceId>,
) -> Vec<ProviderSettingsRow> {
    let settings = &config.settings;
    let overrides = projects
        .first()
        .and_then(|id| settings.project_settings_overrides.get(id))
        .and_then(|overrides| overrides.provider_instances.as_ref());
    let rows = |id: &ProviderInstanceId,
                explicit: Option<&ProviderInstanceConfig>,
                provider: Option<&ServerProvider>| {
        let driver = explicit
            .map(|instance| &instance.driver)
            .or(provider.map(|provider| &provider.driver))?;
        let enabled = explicit.map_or_else(
            || provider.is_some_and(|p| p.enabled),
            provider_settings_enabled,
        );
        if !enabled
            && explicit.is_none()
            && target != Some(id)
            && !matches!(driver.0.as_str(), "codex" | "claudeAgent")
        {
            return None;
        }
        let project_override = overrides
            .and_then(|instances| instances.get(id))
            .and_then(|instance| instance.enabled);
        let name = explicit
            .and_then(|instance| instance.display_name.as_ref()?.as_deref())
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| provider.map(provider_instance_display_name))
            .unwrap_or_else(|| id.0.clone());
        Some(ProviderSettingsRow {
            id: id.clone(),
            driver: driver.0.clone(),
            name,
            enabled: project_override.unwrap_or(enabled),
            project_override,
            provider: provider.cloned(),
        })
    };
    let mut result: Vec<_> = config
        .providers
        .0
        .iter()
        .filter_map(|p| {
            rows(
                &p.instance_id,
                settings.provider_instances.get(&p.instance_id),
                Some(p),
            )
        })
        .chain(
            settings
                .provider_instances
                .iter()
                .filter(|(id, _)| !config.providers.0.iter().any(|p| &p.instance_id == *id))
                .filter_map(|(id, instance)| rows(id, Some(instance), None)),
        )
        .collect();
    for (driver, label) in [("codex", "Codex"), ("claudeAgent", "Claude")] {
        if !result.iter().any(|row| row.id.0 == driver) {
            let id = ProviderInstanceId(driver.into());
            let implicit = implicit_provider_instance(&ProviderDriverKind(driver.into()));
            if let Some(mut row) = rows(&id, Some(&implicit), None) {
                row.name = label.into();
                result.push(row);
            }
        }
    }
    result.sort_by_key(|row| match row.id.0.as_str() {
        "codex" => 0,
        "claudeAgent" => 1,
        _ => 2,
    });
    result
}

impl<P: Platform + Clone + 'static> ProviderSettings<P> {
    pub async fn set_load_weight(
        &self,
        environment: t3_contracts::base_schemas::TrimmedNonEmptyString,
        weight: i64,
    ) -> Result<(), ClientError> {
        let preferences = self.0.composer_preferences();
        preferences.load().await?;
        preferences
            .update(|settings| {
                settings.load_balancing_weights.insert(environment, weight);
            })
            .await
    }
}

#[cfg(test)]
mod provider_settings_tests {
    use super::*;

    fn config() -> ServerConfig {
        serde_json::from_str(
            include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
                .lines()
                .next()
                .expect("config sample"),
        )
        .expect("server config")
    }

    fn with_paired_provider(
        permissions: &[&str],
        check: impl FnOnce(
            ProviderSettings<crate::testing::environment::EnvironmentPlatform>,
            EnvironmentId,
            crate::testing::environment::FakeEnvironment,
        ) -> futures::future::LocalBoxFuture<'static, ()>,
    ) {
        use futures::task::LocalSpawnExt;
        use std::rc::Rc;
        use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

        let mut tasks = futures::executor::LocalPool::new();
        let local = tasks.spawner();
        let spawn: crate::composition::Spawner = Rc::new(move |future| {
            local.spawn_local(future).expect("schedule the fixture");
        });
        let server = crate::testing::environment::FakeEnvironment::new("fixture", spawn.clone());
        let mut scopes = vec!["orchestration:read"];
        scopes.extend_from_slice(permissions);
        server.set_permissions(&scopes);
        server.edit_config(|config| {
            config["settings"]["textGenerationModelSelection"] =
                json!({"instanceId":"fixture-codex","model":"fixture-model"});
            config["settings"]["providerInstances"]["fixture-codex"] = json!({"driver":"codex"});
            config["providers"] = json!([{
                "instanceId":"fixture-codex", "driver":"codex", "enabled":true, "installed":true,
                "version":"1.0.0","status":"ready","auth":{"status":"authenticated"},
                "checkedAt":"2026-11-21T00:00:00.000Z",
                "models":[{"slug":"fixture-model","name":"Fixture Model","capabilities":null,"isCustom":false}],
                "slashCommands":[],"skills":[]
            }]);
        });
        server.reply("server.updateSettings", Ok(json!(config().settings)));
        let store = crate::ClientStore::default();
        let client = ClientRuntime::new(
            server.platform(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
            spawn,
        );
        let environment = EnvironmentId("fixture".into());
        tasks.run_until(async {
            let mut ready = store.subscribe_environment(&environment);
            client
                .pair(server.pairing())
                .await
                .expect("pair the fixture");
            while store
                .snapshot()
                .environments
                .get(&environment)
                .and_then(|entry| entry.config.as_ref())
                .is_none()
            {
                ready.next().await.expect("config publication");
            }
            check(ProviderSettings::new(&client), environment, server).await;
        });
    }

    #[test]
    fn save_checks_the_scopes_required_by_patch_and_mutation() {
        use futures::FutureExt;
        for (scopes, patch, mutation) in [
            (
                vec!["providers:manage"],
                json!({"defaultModelSelection":null}),
                None,
            ),
            (vec!["providers:manage"], json!({}), None),
            (
                vec!["settings:write"],
                json!({}),
                Some(ProviderInstanceMutation::Remove {
                    instance_id: ProviderInstanceId("fixture-codex".into()),
                }),
            ),
            (
                vec!["settings:write"],
                json!({"defaultModelSelection":null}),
                Some(ProviderInstanceMutation::Remove {
                    instance_id: ProviderInstanceId("fixture-codex".into()),
                }),
            ),
        ] {
            with_paired_provider(&scopes, move |settings, env, server| {
                async move {
                let error = settings.save(&env, serde_json::from_value(patch).unwrap(), mutation).await;
                assert!(matches!(error, Err(ClientError::PermissionDenied { method }) if method == "server.updateSettings"));
                assert!(server.calls("server.updateSettings").is_empty());
            }.boxed_local()
            });
        }
    }

    #[test]
    fn disabling_the_text_generation_provider_resets_selection_with_settings_access() {
        use futures::FutureExt;
        for can_write in [true, false] {
            let scopes = if can_write {
                vec!["providers:manage", "settings:write"]
            } else {
                vec!["providers:manage"]
            };
            with_paired_provider(&scopes, move |settings, env, server| {
                async move {
                    settings
                        .set_enabled(&env, ProviderInstanceId("fixture-codex".into()), false)
                        .await
                        .expect("disable provider");
                    let calls = server.calls("server.updateSettings");
                    assert_eq!(calls.len(), 1);
                    assert_eq!(
                        calls[0]["providerInstanceMutation"]["instance"]["enabled"],
                        false
                    );
                    assert_eq!(
                        calls[0]["providerInstanceMutation"]["instanceId"],
                        "fixture-codex"
                    );
                    if can_write {
                        assert_eq!(
                            calls[0]["patch"]["textGenerationModelSelection"],
                            json!(
                                t3_contracts::settings::ServerSettings::default()
                                    .text_generation_model_selection
                            )
                        );
                    } else {
                        assert_eq!(calls[0]["patch"], json!({}));
                    }
                }
                .boxed_local()
            });
        }
    }

    #[test]
    fn setup_lists_implicit_defaults_and_honors_a_project_override() {
        let mut config = config();
        config.providers.0.clear();
        config.settings.provider_instances.clear();
        let project = ProjectId("project".into());
        let id = ProviderInstanceId("codex".into());
        config.settings.project_settings_overrides = serde_json::from_value(json!({
            "project": {"providerInstances":{"codex":{"enabled":false}}, "defaultAutoPull":true}
        }))
        .expect("project override");
        let rows = provider_settings_rows(&config, std::slice::from_ref(&project), None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, id);
        assert!(!rows[0].enabled);
        assert_eq!(rows[0].project_override, Some(false));
        assert!(rows[1].enabled);
        let reset = provider_override_patch(&config.settings, &[project], &id, None);
        assert_eq!(
            reset["projectSettingsOverrides"]["project"],
            json!({"providerInstances":{}, "defaultAutoPull":true})
        );
    }

    #[test]
    fn usage_hub_ids_reuse_the_same_url_and_suffix_different_hubs_on_the_same_host() {
        let mut settings = config().settings;
        let url = "https://hub.test:8318";
        let tag = usage_source_id(url, &settings.usage_limit_sources);
        assert_eq!(tag.0, "cliproxy-hub.test-8318");
        settings.usage_limit_sources.insert(
            tag.clone(),
            serde_json::from_value(json!({"kind":"cliproxy","url":url})).expect("usage source"),
        );
        assert_eq!(usage_source_id(url, &settings.usage_limit_sources), tag);
        assert_eq!(
            usage_source_id("https://hub.test:8318/other", &settings.usage_limit_sources).0,
            "cliproxy-hub.test-8318-2"
        );
    }
}

/// Wire frames for the Providers UI's scripted transport fixture.
#[cfg(feature = "testing")]
#[doc(hidden)]
pub use t3_rpc::{CallKind as TestCallKind, Frame as TestFrame};
