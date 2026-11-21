//! Discovery and credential settings for the selected source control environment.
use std::collections::BTreeMap;

use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::{Map, Value, json};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    rpc::{Empty, WsSourceControlSignInStartRpc},
    secret_reference::SecretReference,
    settings_secret_value::SettingsSecretValue,
    source_control::{
        SourceControlDiscoveryResult, SourceControlProviderAuthAccounts, SourceControlProviderKind,
        SourceControlSignInInput, SourceControlSignInPrompt,
    },
    wire::trim_js,
};

use super::{SettingsScopeView, SettingsService};
use crate::{
    platform::Platform, types::EnvironmentId, workspace::project_ops::source_control::SourceControl,
};

const REDACTED: &str = "••••••";

#[derive(Clone, PartialEq)]
pub enum SavedToken {
    Stored,
    Reference(SecretReference),
}

#[derive(Clone, Default, PartialEq)]
pub struct HostCredentials {
    pub tokens: BTreeMap<String, SavedToken>,
    pub hosts: BTreeMap<String, HostChoice>,
    pub signed_in: bool,
    pub sources: Vec<(String, String)>,
}

#[derive(Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct HostChoice {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct HostAccounts {
    pub host: String,
    pub active: Option<String>,
    pub selectable: Vec<String>,
    pub broken: Vec<SourceControlProviderAuthAccounts>,
    pub environment_variable: Option<String>,
}

pub fn group_host_accounts(accounts: &[SourceControlProviderAuthAccounts]) -> Vec<HostAccounts> {
    let mut hosts = Vec::new();
    for entry in accounts {
        if !hosts.contains(&entry.host.0) {
            hosts.push(entry.host.0.clone());
        }
    }
    hosts
        .into_iter()
        .map(|host| {
            let entries = accounts
                .iter()
                .filter(|entry| entry.host.0 == host)
                .collect::<Vec<_>>();
            let stored = entries
                .iter()
                .copied()
                .filter(|entry| entry.environment_variable.is_none())
                .collect::<Vec<_>>();
            let valid = stored
                .iter()
                .copied()
                .filter(|entry| entry.authenticated)
                .collect::<Vec<_>>();
            HostAccounts {
                host,
                active: valid
                    .iter()
                    .find(|entry| entry.active)
                    .or_else(|| valid.first())
                    .map(|entry| entry.account.0.clone()),
                selectable: valid.iter().map(|entry| entry.account.0.clone()).collect(),
                broken: stored
                    .into_iter()
                    .filter(|entry| !entry.authenticated)
                    .cloned()
                    .collect(),
                environment_variable: entries.into_iter().find_map(|entry| {
                    entry
                        .environment_variable
                        .as_ref()
                        .map(|value| value.0.clone())
                }),
            }
        })
        .collect()
}

pub fn normalize_forge_host(host: &str) -> String {
    trim_js(host).to_lowercase()
}

pub fn token_management_url(host: &str, path: &str) -> Option<String> {
    let authority = normalize_forge_host(host);
    let origin = url::Url::parse(&format!("https://{authority}/")).ok()?;
    if origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || authority.contains(['@', '/', '\\', '?', '#'])
    {
        return None;
    }
    origin.join(path).ok().map(Into::into)
}

pub fn credentials(view: &SettingsScopeView, provider: &str, app_host: &str) -> HostCredentials {
    let Some(config) = view
        .selected
        .representative
        .as_ref()
        .and_then(|environment| environment.config.as_ref())
    else {
        return HostCredentials::default();
    };
    let saved = config.settings.source_control_hosts.get(provider);
    let field = |name| {
        saved
            .and_then(|saved| saved.get(name))
            .and_then(Value::as_object)
    };
    let tokens = field("tokens")
        .into_iter()
        .flatten()
        .filter_map(|(host, value)| match value {
            Value::String(value) if !value.is_empty() => Some((host.clone(), SavedToken::Stored)),
            Value::Object(_) => serde_json::from_value::<SecretReference>(value.clone())
                .ok()
                .map(|reference| (host.clone(), SavedToken::Reference(reference))),
            _ => None,
        })
        .collect();
    HostCredentials {
        tokens,
        hosts: field("hosts")
            .into_iter()
            .flatten()
            .filter_map(|(host, value)| {
                serde_json::from_value(value.clone())
                    .ok()
                    .map(|choice| (host.clone(), choice))
            })
            .collect(),
        signed_in: field("signIns")
            .and_then(|sign_ins| sign_ins.get(app_host))
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        sources: if config.secret_references == Some(true) {
            config
                .settings
                .secret_providers
                .iter()
                .filter(|(_, provider)| provider.kind == "one_password")
                .map(|(id, provider)| (id.0.clone(), provider.label.0.clone()))
                .collect()
        } else {
            Vec::new()
        },
    }
}

#[derive(Clone)]
pub enum ForgeCredentialEdit {
    Token {
        host: String,
        value: SettingsSecretValue,
    },
    Account {
        host: String,
        enabled: Option<bool>,
        account: Option<Option<String>>,
    },
    SignOut {
        host: String,
    },
}

#[derive(Clone, Copy)]
pub enum SignInAction {
    Wait,
    Cancel,
}

pub type ForgeResult<T> = Result<T, ()>;

impl<P: Platform + Clone + 'static> SettingsService<P> {
    pub fn discover_forges(
        &self,
        environment: EnvironmentId,
    ) -> LocalBoxFuture<'static, ForgeResult<SourceControlDiscoveryResult>> {
        let session = self.session(&environment);
        async move {
            SourceControl::default()
                .discovery(&environment, &session.ok_or(())?, Empty {})
                .await
                .map_err(|_| ())
        }
        .boxed_local()
    }

    pub fn can_sign_in_to_forge(
        &self,
        environment: &EnvironmentId,
        input: &SourceControlSignInInput,
    ) -> bool {
        self.session(environment)
            .is_some_and(|session| session.available::<WsSourceControlSignInStartRpc>(input))
    }

    pub fn start_forge_sign_in(
        &self,
        environment: EnvironmentId,
        input: SourceControlSignInInput,
    ) -> LocalBoxFuture<'static, ForgeResult<SourceControlSignInPrompt>> {
        let session = self.session(&environment);
        async move {
            SourceControl::default()
                .start_sign_in(&environment, &session.ok_or(())?, input)
                .await
                .map_err(|_| ())
        }
        .boxed_local()
    }

    pub fn finish_forge_sign_in(
        &self,
        environment: EnvironmentId,
        input: SourceControlSignInInput,
        action: SignInAction,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        let session = self.session(&environment);
        async move {
            let session = session.ok_or(())?;
            let source = SourceControl::default();
            match action {
                SignInAction::Wait => source
                    .wait_for_sign_in(&environment, &session, input)
                    .await
                    .map(|_| ()),
                SignInAction::Cancel => source.cancel_sign_in(&environment, &session, input).await,
            }
            .map_err(|_| ())
        }
        .boxed_local()
    }

    /// Build the map from the current configuration inside the client. Literal siblings are
    /// represented by preservation markers, and no raw reply or error crosses this boundary.
    pub fn edit_forge_credentials(
        &self,
        environment: EnvironmentId,
        provider: SourceControlProviderKind,
        edit: ForgeCredentialEdit,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        let settings = self.clone();
        async move {
            if !settings.can_write_settings(&environment) {
                return Err(());
            }
            let config = settings
                .environments()
                .into_iter()
                .find(|entry| entry.environment_id == environment)
                .and_then(|entry| entry.config)
                .ok_or(())?;
            let current = config.settings.source_control_hosts.get(&provider.0);
            let field = |key| {
                current
                    .and_then(|saved| saved.get(key))
                    .and_then(Value::as_object)
            };
            let patch = match edit {
                ForgeCredentialEdit::Token { host, value } => {
                    let mut tokens = preserve_secrets(field("tokens"));
                    tokens.insert(host, serde_json::to_value(value).map_err(|_| ())?);
                    json!({"tokens":tokens})
                }
                ForgeCredentialEdit::SignOut { host } => {
                    let mut sign_ins = preserve_secrets(field("signIns"));
                    sign_ins.insert(host, json!(""));
                    json!({"signIns":sign_ins})
                }
                ForgeCredentialEdit::Account {
                    host,
                    enabled,
                    account,
                } => {
                    let mut hosts = field("hosts").cloned().unwrap_or_default();
                    let previous = hosts
                        .get(&host)
                        .and_then(|value| serde_json::from_value::<HostChoice>(value.clone()).ok())
                        .unwrap_or(HostChoice {
                            enabled: true,
                            account: None,
                        });
                    let choice = HostChoice {
                        enabled: enabled.unwrap_or(previous.enabled),
                        account: account.unwrap_or(previous.account),
                    };
                    if choice.enabled && choice.account.is_none() {
                        hosts.remove(&host);
                    } else {
                        hosts.insert(host, serde_json::to_value(choice).map_err(|_| ())?);
                    }
                    json!({"hosts":hosts})
                }
            };
            let patch =
                Map::from_iter([("sourceControlHosts".to_owned(), json!({provider.0:patch}))]);
            settings
                .save(&environment, patch)
                .await
                .map(|_| ())
                .map_err(|_| ())
        }
        .boxed_local()
    }
}

fn preserve_secrets(values: Option<&Map<String, Value>>) -> Map<String, Value> {
    values
        .into_iter()
        .flatten()
        .filter_map(|(host, value)| match value {
            Value::String(text) if !text.is_empty() => Some((host.clone(), json!(REDACTED))),
            Value::Object(_)
                if serde_json::from_value::<SecretReference>(value.clone()).is_ok() =>
            {
                Some((host.clone(), value.clone()))
            }
            _ => None,
        })
        .collect()
}

pub fn sign_in_target(provider: SourceControlProviderKind, host: &str) -> SourceControlSignInInput {
    SourceControlSignInInput {
        provider,
        host: TrimmedNonEmptyString(host.to_owned()),
    }
}

#[derive(Clone, PartialEq)]
pub struct RoutingChoice {
    pub id: EnvironmentId,
    pub label: String,
    pub permission: crate::persistence::RoutingAccess,
    pub available: bool,
}

impl<P: Platform + Clone + 'static> SettingsService<P> {
    pub fn routing_choices(&self) -> LocalBoxFuture<'static, Vec<RoutingChoice>> {
        let entries = self.runtime.environments();
        let permissions =
            crate::connection::PullRequestRoutingPermissions::new(self.platform.clone());
        async move {
            let reads = entries.into_iter().map(|entry| {
                let permissions = &permissions;
                async move {
                    let available = !entry.routes.is_empty() && !entry.damaged;
                    let registration = route_registration(entry);
                    let permission = permissions.get(&registration).await.unwrap_or_default();
                    RoutingChoice {
                        id: registration.id,
                        label: registration.label,
                        permission,
                        available,
                    }
                }
            });
            futures::future::join_all(reads).await
        }
        .boxed_local()
    }

    pub fn set_routing_choice(
        &self,
        environment: EnvironmentId,
        permission: crate::persistence::RoutingAccess,
    ) -> LocalBoxFuture<'static, ForgeResult<()>> {
        let entry = self
            .runtime
            .environments()
            .into_iter()
            .find(|entry| entry.id == environment && !entry.damaged);
        let permissions =
            crate::connection::PullRequestRoutingPermissions::new(self.platform.clone());
        async move {
            let entry = route_registration(entry.ok_or(())?);
            permissions.set(&entry, permission).await.map_err(|_| ())
        }
        .boxed_local()
    }

    pub fn forge_app_access(
        &self,
        environment: EnvironmentId,
        project: t3_contracts::base_schemas::ProjectId,
    ) -> LocalBoxFuture<'static, Option<t3_contracts::source_control::SourceControlAppAccessStatus>>
    {
        use t3_contracts::source_control::SourceControlAppAccessInput;
        let project = self
            .runtime
            .project_actions()
            .project(&crate::workspace::ProjectKey {
                environment: environment.clone(),
                project,
            });
        let session = self.session(&environment);
        async move {
            let project = project?;
            let identity = project.project.repository_identity.as_ref()?.as_ref()?;
            let repository = identity.canonical_key.0.strip_prefix("github.com/")?;
            let (owner, name) = repository.split_once('/')?;
            if owner.is_empty() || name.is_empty() || name.contains('/') {
                return None;
            }
            SourceControl::default()
                .app_access(
                    &environment,
                    &session?,
                    SourceControlAppAccessInput {
                        provider: SourceControlProviderKind("github".into()),
                        host: TrimmedNonEmptyString("github.com".into()),
                        repository: TrimmedNonEmptyString(repository.into()),
                    },
                )
                .await
                .ok()
                .map(|access| access.status)
        }
        .boxed_local()
    }
}

fn route_registration(
    entry: crate::composition::EnvironmentEntry,
) -> crate::environment::EnvironmentRegistration {
    crate::environment::EnvironmentRegistration {
        id: entry.id,
        label: entry.label,
        routes: entry.routes,
        enabled: entry.state.desired,
    }
}
