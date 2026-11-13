pub mod eas;
pub mod expo;
pub mod simlock;

use crate::{
    Error, Result,
    host::{DeviceHost, HostFactory},
    io::{Commands, Http, Secret, Spawner},
    local::Ports,
    service::{Accounts, Settings},
};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    path::PathBuf,
    sync::Arc,
};
use t3_contracts::{device::*, settings_secret_value::SettingsSecretValue};

#[async_trait]
pub trait Secrets: Send + Sync {
    async fn resolve(&self, value: &SettingsSecretValue) -> Result<Secret>;
}
pub struct LiteralSecrets;
#[async_trait]
impl Secrets for LiteralSecrets {
    async fn resolve(&self, value: &SettingsSecretValue) -> Result<Secret> {
        match value {
            SettingsSecretValue::Literal(s) => Ok(Secret::new(s)),
            SettingsSecretValue::Reference(r) => Err(Error::Unavailable(format!(
                "Could not read secret provider \"{}\". Unlock or reconnect it, then retry.",
                r.provider.0
            ))),
        }
    }
}

pub struct AccountService {
    pub settings: Arc<dyn Settings>,
    pub secrets: Arc<dyn Secrets>,
    pub http: Arc<dyn Http>,
    pub environment: BTreeMap<String, String>,
    pub expo_home: PathBuf,
}
#[async_trait]
impl Accounts for AccountService {
    async fn eas_account(&self, input: DeviceEasAccountInput) -> Result<DeviceEasAccount> {
        let settings = self.settings.read().await?;
        let saved = input
            .host_id
            .as_ref()
            .and_then(|v| v.as_ref())
            .and_then(|id| {
                settings.hosts.iter().find_map(|h| match h {
                    DeviceHostConfig::EasDeviceHostConfig(c) if c.id == *id => {
                        Some(c.access_token.clone())
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| SettingsSecretValue::Literal(String::new()));
        let token = self
            .secrets
            .resolve(
                input
                    .access_token
                    .as_ref()
                    .and_then(|v| v.as_ref())
                    .unwrap_or(&saved),
            )
            .await?;
        let credential = expo::resolve_credential(
            token.expose(),
            self.environment.get("EXPO_TOKEN").map(String::as_str),
            &self.expo_home,
        )
        .await?;
        let source = credential.source.clone();
        let api = expo::ExpoHttp::new(self.http.clone(), credential);
        let viewer = expo::ExpoApi::viewer(&api).await.map_err(|e| {
            Error::Unavailable(if e.status == Some(401) || e.status == Some(403) {
                format!(
                    "Expo rejected the credential (HTTP {}).",
                    e.status.unwrap_or(401)
                )
            } else if e.rejected {
                "Expo refused the account check.".into()
            } else {
                "Could not complete the Expo account check.".into()
            })
        })?;
        Ok(DeviceEasAccount {
            signed_in_as: viewer.0,
            credential_source: source,
            apps: viewer.1,
        })
    }
    async fn simlock_identity(
        &self,
        input: DeviceSimlockIdentityInput,
    ) -> Result<DeviceSimlockIdentity> {
        let settings = self.settings.read().await?;
        let machine = simlock::machine_token(&self.environment);
        let gateway = input
            .gateway_url
            .as_ref()
            .and_then(|v| v.as_ref())
            .map(|s| s.0.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| machine.as_ref().map(|(g, _)| g.clone()))
            .ok_or_else(|| {
                Error::Unavailable(
                    "This machine's agent-device does not point at a Simlock gateway.".into(),
                )
            })?;
        let gateway = simlock::gateway(&gateway)?;
        let saved = input
            .host_id
            .as_ref()
            .and_then(|v| v.as_ref())
            .and_then(|id| {
                settings.hosts.iter().find_map(|h| match h {
                    DeviceHostConfig::SimlockDeviceHostConfig(c) if c.id == *id => Some(c),
                    _ => None,
                })
            });
        let (token, source) =
            if let Some(token) = input.agent_token.as_ref().and_then(|v| v.as_ref()) {
                (
                    self.secrets.resolve(token).await?,
                    DeviceSimlockTokenSource::Entered,
                )
            } else {
                simlock::resolve_token(self.secrets.as_ref(), &gateway, saved, &self.environment)
                    .await?
                    .ok_or_else(|| {
                        Error::Unavailable(
                        if saved.is_some() {
                            "The saved token is for another gateway. Enter one for this gateway."
                        } else {
                            "No agent token for this gateway on this machine."
                        }
                        .into(),
                    )
                    })?
            };
        let value = simlock::gateway_json(
            self.http.as_ref(),
            &gateway,
            &token,
            http::Method::GET,
            "/v1/identity",
            None,
        )
        .await?;
        let role = value["role"]
            .as_str()
            .ok_or_else(|| crate::error::invalid("Simlock identity"))?;
        if role != "agent" {
            return Err(Error::Unavailable(format!(
                "That is a {} token. Create one with --role agent.",
                crate::error::bounded(&role.replace(token.expose(), "[redacted]"), 64)
            )));
        }
        let name = value["name"]
            .as_str()
            .ok_or_else(|| crate::error::invalid("Simlock identity"))?;
        Ok(DeviceSimlockIdentity {
            gateway_url: gateway,
            signed_in_as: name.replace(token.expose(), "[redacted]"),
            token_source: source,
        })
    }
}

pub struct NativeHostFactory {
    pub state: PathBuf,
    pub environment_id: String,
    pub node: String,
    pub agent_entry: PathBuf,
    pub tools: Arc<crate::tools::Toolchain>,
    pub http: Arc<dyn Http>,
    pub commands: Arc<dyn Commands>,
    pub spawner: Arc<dyn Spawner>,
    pub ports: Arc<dyn Ports>,
    pub secrets: Arc<dyn Secrets>,
    pub environment: BTreeMap<String, String>,
    pub expo_home: PathBuf,
    pub dns: Arc<dyn crate::ssh::Dns>,
    pub local_addresses: BTreeSet<IpAddr>,
}
impl NativeHostFactory {
    fn ssh(&self, config: SshDeviceHostConfig) -> crate::ssh::SshOptions {
        crate::ssh::SshOptions {
            owner: crate::ssh::owner(
                &self.environment_id,
                &self.state.to_string_lossy(),
                config.id.as_ref(),
            ),
            config,
            commands: self.commands.clone(),
            http: self.http.clone(),
            spawner: self.spawner.clone(),
            ports: self.ports.clone(),
        }
    }
}
#[async_trait]
impl HostFactory for NativeHostFactory {
    async fn create(&self, config: &DeviceHostConfig) -> Result<Arc<dyn DeviceHost>> {
        validate_config(config)?;
        Ok(match config {
            DeviceHostConfig::SshDeviceHostConfig(c) => {
                Arc::new(crate::ssh::SshDeviceHost::new(self.ssh(c.clone()))?)
            }
            DeviceHostConfig::EasDeviceHostConfig(c) => Arc::new(eas::EasHost::new(
                c.clone(),
                self.state.clone(),
                Arc::new(expo::SavedExpo {
                    http: self.http.clone(),
                    secrets: self.secrets.clone(),
                    saved: c.access_token.clone(),
                    environment: self.environment.clone(),
                    home: self.expo_home.clone(),
                }),
                self.http.clone(),
            )?),
            DeviceHostConfig::SimlockDeviceHostConfig(c) => {
                Arc::new(simlock::SimlockHost::new(simlock::SimlockOptions {
                    config: c.clone(),
                    state: self.state.clone(),
                    node: self.node.clone(),
                    agent_entry: self.agent_entry.clone(),
                    cli_provider: Some(Arc::new(crate::tools::ToolchainAgentCli {
                        node: self.node.clone(),
                        tools: self.tools.clone(),
                    })),
                    commands: self.commands.clone(),
                    http: self.http.clone(),
                    secrets: self.secrets.clone(),
                    environment: self.environment.clone(),
                })?)
            }
        })
    }
    async fn test(&self, config: &SshDeviceHostConfig) -> Result<DeviceHostSummary> {
        if self.is_local(config).await {
            return Err(Error::Unavailable(
                "This SSH target is this machine; use the local device host.".into(),
            ));
        }
        self.ssh(config.clone()).probe().await
    }
    async fn is_local(&self, config: &SshDeviceHostConfig) -> bool {
        crate::ssh::is_local(
            config,
            self.commands.as_ref(),
            self.dns.as_ref(),
            &self.local_addresses,
        )
        .await
    }
}
pub fn validate_config(config: &DeviceHostConfig) -> Result<()> {
    let id = match config {
        DeviceHostConfig::SshDeviceHostConfig(c) => c.id.as_ref(),
        DeviceHostConfig::EasDeviceHostConfig(c) => c.id.as_ref(),
        DeviceHostConfig::SimlockDeviceHostConfig(c) => c.id.as_ref(),
    };
    if id == "local"
        || id.len() > 128
        || !id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(crate::error::invalid("host id"));
    }
    match config {
        DeviceHostConfig::SshDeviceHostConfig(c) => {
            crate::ssh::base_args(c)?;
            if c.target.as_ref().chars().any(char::is_whitespace) {
                return Err(crate::error::invalid("SSH target"));
            }
        }
        DeviceHostConfig::EasDeviceHostConfig(c) => {
            if !(1..=115).contains(&c.max_session_minutes)
                || !(1..=60).contains(&c.idle_minutes)
                || c.idle_minutes >= c.max_session_minutes
                || !(1..=20).contains(&c.max_concurrent_sessions)
            {
                return Err(crate::error::invalid("EAS limits"));
            }
        }
        DeviceHostConfig::SimlockDeviceHostConfig(c) => {
            simlock::gateway(c.gateway_url.as_ref())?;
            if !(1..=20).contains(&c.max_leases) || !(5..=240).contains(&c.max_lease_minutes) {
                return Err(crate::error::invalid("Simlock limits"));
            }
        }
    }
    Ok(())
}
