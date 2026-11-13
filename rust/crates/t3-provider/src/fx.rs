use crate::{
    Result,
    catalog::{capabilities, custom_models, model, select_option},
    discovery::{Driver, is_windows_command_not_found},
    environment::{Environment, Platform, probe_environment, process_options},
    versions::probe_timeout_ms,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use t3_contracts::{
    model::{ModelCapabilities, ProviderOptionDescriptor},
    provider_instance::ProviderInstanceId,
    provider_policy::RuntimeMode,
    server::{ServerProvider, ServerProviderModel},
};

/// Packaged settings have not yet been added to t3-contracts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FxSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_binary")]
    pub binary_path: String,
    #[serde(default)]
    pub custom_models: Vec<t3_contracts::model::CustomModelSetting>,
}
fn default_binary() -> String {
    "fx".into()
}

pub fn effort_descriptor() -> ProviderOptionDescriptor {
    let mut descriptor = select_option(
        "effort",
        "Effort",
        &[
            ("auto", "Auto"),
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
            ("xhigh", "Extra High"),
            ("max", "Max"),
        ],
        Some("medium"),
    );
    // fx publishes the default on its choice, without a currentValue in the fallback catalog.
    if let ProviderOptionDescriptor::Select(descriptor) = &mut descriptor {
        descriptor.current_value = None;
    }
    descriptor
}
pub fn model_capabilities() -> ModelCapabilities {
    capabilities(vec![effort_descriptor()])
}

pub fn models(
    settings: &FxSettings,
    default: Option<&str>,
    listed: &[String],
) -> Vec<ServerProviderModel> {
    let caps = model_capabilities();
    let mut first = model(
        "default",
        &default.map_or_else(|| "Default".into(), |d| format!("Default ({d})")),
        Some(caps.clone()),
    );
    first.is_default = Some(Some(true));
    first.aliases = default.map(|d| {
        Some(vec![t3_contracts::base_schemas::TrimmedNonEmptyString(
            d.into(),
        )])
    });
    let mut built = vec![first];
    for id in listed {
        if id != "default" && !built.iter().any(|m| m.slug.0 == *id) {
            built.push(model(id, id, Some(caps.clone())));
        }
    }
    let custom = settings
        .custom_models
        .iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .collect::<Vec<_>>();
    custom_models(&built, &custom, Some(&caps))
}

pub fn session_mode(
    mode: RuntimeMode,
    approval_override: bool,
    sandbox_override: bool,
) -> &'static str {
    if mode == RuntimeMode::Auto && !approval_override && !sandbox_override {
        "auto"
    } else {
        "ask"
    }
}
pub fn advertised_mode<'a>(requested: &str, advertised: &'a [String]) -> Option<&'a str> {
    let candidates: &[&str] = match requested {
        "ask" => &["ask"],
        "auto" | "code" => &["auto", "code"],
        _ => &[],
    };
    candidates.iter().find_map(|candidate| {
        advertised
            .iter()
            .find(|a| a.as_str() == *candidate)
            .map(String::as_str)
    })
}

pub fn default_model_id(models: &[ServerProviderModel]) -> Option<&str> {
    models
        .iter()
        .find(|m| m.slug.0 == "default")?
        .aliases
        .as_ref()?
        .as_ref()?
        .first()
        .map(|s| s.0.as_str())
}

#[derive(Deserialize)]
struct FxStatus {
    #[serde(default, with = "t3_contracts::wire::optional")]
    auth: Option<String>,
    #[serde(default, with = "t3_contracts::wire::optional")]
    model: Option<String>,
}
#[derive(Deserialize)]
struct FxModels {
    #[serde(default, with = "t3_contracts::wire::optional")]
    ids: Option<Vec<String>>,
}

struct FxProbe<'a> {
    installed: bool,
    version: Option<String>,
    status: &'a str,
    auth: Value,
    message: Option<&'a str>,
}

pub struct FxProvider {
    pub instance_id: ProviderInstanceId,
    pub settings: FxSettings,
    pub environment: Environment,
}

impl FxProvider {
    async fn run(
        &self,
        args: &[&str],
        timeout: u64,
    ) -> std::result::Result<t3_process::ProcessOutput, t3_process::ProcessError> {
        let env = probe_environment(&self.environment);
        let binary = if self.settings.binary_path.is_empty() {
            "fx"
        } else {
            &self.settings.binary_path
        };
        let args: Vec<_> = args.iter().map(|s| (*s).into()).collect();
        let mut options = process_options(binary, &args, &env);
        options.check_exit = false;
        options.timeout = Some(Duration::from_millis(probe_timeout_ms(
            "fx",
            timeout,
            &self.environment,
        )));
        t3_process::run(options).await
    }

    fn snapshot(
        &self,
        checked_at: &str,
        probe: FxProbe<'_>,
        default: Option<&str>,
        listed: &[String],
    ) -> Result<ServerProvider> {
        let FxProbe {
            installed,
            version,
            status,
            auth,
            message,
        } = probe;
        let mut value = json!({
            "instanceId":self.instance_id,"driver":"fx","displayName":"fx","continuation":{"groupKey":format!("fx:instance:{}",self.instance_id.0)},
            "showInteractionModeToggle":false,"supportedRuntimeModes":["approval-required","auto"],"supportsTextGeneration":false,
            "enabled":self.settings.enabled,"installed":installed,"version":version,"status":if self.settings.enabled {status}else{"disabled"},"auth":auth,"checkedAt":checked_at,
            "setup":{"canAuthenticate":false,"canInstall":false},"models":models(&self.settings,default,listed),"slashCommands":[],"skills":[],
            "versionAdvisory":{"status":"unknown","currentVersion":version,"latestVersion":null,"updateCommand":null,"canUpdate":false,"canInstallVersion":false,"checkedAt":checked_at,"message":null}
        });
        if let Some(message) = message {
            value["message"] = json!(message);
        }
        Ok(serde_json::from_value(value)?)
    }

    pub fn initial(&self, checked_at: &str) -> Result<ServerProvider> {
        self.snapshot(
            checked_at,
            FxProbe {
                installed: self.settings.enabled,
                version: None,
                status: "warning",
                auth: json!({"status":"unknown"}),
                message: Some(if self.settings.enabled {
                    "Checking fx CLI availability..."
                } else {
                    "fx is disabled in T3 Code settings."
                }),
            },
            None,
            &[],
        )
    }

    pub async fn check(&self, checked_at: &str) -> Result<ServerProvider> {
        let unknown = || json!({"status":"unknown"});
        if !self.settings.enabled {
            return self.snapshot(
                checked_at,
                FxProbe {
                    installed: false,
                    version: None,
                    status: "warning",
                    auth: unknown(),
                    message: Some("fx is disabled in T3 Code settings."),
                },
                None,
                &[],
            );
        }
        let output = match self.run(&["--version"], 10_000).await {
            Ok(output)
                if !is_windows_command_not_found(
                    Platform::current(),
                    output.code,
                    &output.stderr_text(),
                ) =>
            {
                output
            }
            result => {
                let missing = matches!(result, Err(t3_process::ProcessError::NotFound { .. }))
                    || result.is_ok();
                return self.snapshot(
                    checked_at,
                    FxProbe {
                        installed: !missing,
                        version: None,
                        status: "error",
                        auth: unknown(),
                        message: Some(if missing {
                            "fx is not installed or not on PATH."
                        } else {
                            "Failed to run `fx --version`."
                        }),
                    },
                    None,
                    &[],
                );
            }
        };
        let version = if output.code == Some(0) {
            Driver::Fx.parse_version(&format!(
                "{}\n{}",
                output.stdout_text(),
                output.stderr_text()
            ))
        } else {
            None
        };
        if output.code != Some(0) {
            return self.snapshot(
                checked_at,
                FxProbe {
                    installed: true,
                    version,
                    status: "error",
                    auth: unknown(),
                    message: Some("fx is installed but failed to run."),
                },
                None,
                &[],
            );
        }
        let status = self
            .run(&["status", "--json"], 10_000)
            .await
            .ok()
            .filter(|o| o.code == Some(0))
            .and_then(|o| serde_json::from_slice::<FxStatus>(&o.stdout).ok());
        let listed = self
            .run(&["models", "--json"], 10_000)
            .await
            .ok()
            .filter(|o| o.code == Some(0))
            .and_then(|o| serde_json::from_slice::<FxModels>(&o.stdout).ok())
            .map(|m| {
                m.ids
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            });
        let method = status
            .as_ref()
            .and_then(|s| s.auth.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let auth = match method {
            None => unknown(),
            Some("missing") => json!({"status":"unauthenticated"}),
            Some(method) => json!({"status":"authenticated","label":method}),
        };
        let (state, message) = match method {
            Some("missing") => ("error", Some("Run `fx login` to sign in.")),
            None => (
                "warning",
                Some("fx is installed but login state could not be determined."),
            ),
            _ if listed.is_none() => (
                "warning",
                Some("fx is installed but model listing failed. Model options may be incomplete."),
            ),
            _ => ("ready", None),
        };
        let default = status
            .as_ref()
            .and_then(|s| s.model.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        self.snapshot(
            checked_at,
            FxProbe {
                installed: true,
                version,
                status: state,
                auth,
                message,
            },
            default,
            &listed.unwrap_or_default(),
        )
    }
}
