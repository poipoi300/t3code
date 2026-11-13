use crate::{Error, Result};
use async_trait::async_trait;
use std::{fmt, sync::Arc};
use t3_contracts::{
    secret_reference::SecretReference,
    settings_secret_value::{SecretSourceCheckResult, SettingsSecretValue},
    wire::Redacted,
};

#[derive(Clone)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Redacted::of(&self.0).fmt(f)
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", Redacted::of(&self.0))
    }
}

/// Adapter to the owning provider resolver and registry. Fresh reads must bypass
/// failed cached reads; refresh consumes that same read rather than resolving again.
#[async_trait]
pub trait SecretBackend: Send + Sync {
    async fn read(
        &self,
        references: &[SecretReference],
        fresh: bool,
    ) -> Result<Vec<Option<Secret>>>;
    async fn label(&self, reference: &SecretReference) -> Result<String>;
    async fn apply_secret_and_refresh(
        &self,
        reference: &SecretReference,
        value: &Secret,
    ) -> Result<()>;
}

#[async_trait]
pub trait SecretRefresh: Send + Sync {
    async fn apply_secret_and_refresh(
        &self,
        reference: &SecretReference,
        value: &Secret,
    ) -> Result<()>;
}
pub struct ProviderSecrets {
    settings: Arc<t3_config::settings::SettingsService>,
    resolver: Arc<t3_provider::secrets::SecretResolver>,
    refresh: Arc<dyn SecretRefresh>,
}
impl ProviderSecrets {
    pub fn new(
        settings: Arc<t3_config::settings::SettingsService>,
        resolver: Arc<t3_provider::secrets::SecretResolver>,
        refresh: Arc<dyn SecretRefresh>,
    ) -> Self {
        Self {
            settings,
            resolver,
            refresh,
        }
    }
}
#[async_trait]
impl SecretBackend for ProviderSecrets {
    async fn read(
        &self,
        references: &[SecretReference],
        fresh: bool,
    ) -> Result<Vec<Option<Secret>>> {
        let providers = self
            .settings
            .snapshot()
            .await
            .map(|s| s.secret_providers)
            .unwrap_or_default();
        Ok(self
            .resolver
            .read(&providers, references, fresh)
            .await
            .into_iter()
            .map(|v| v.map(Secret::new))
            .collect())
    }
    async fn label(&self, reference: &SecretReference) -> Result<String> {
        let providers = self
            .settings
            .snapshot()
            .await
            .map(|s| s.secret_providers)
            .unwrap_or_default();
        Ok(t3_provider::secrets::SecretResolver::label(
            &providers,
            &reference.provider,
        ))
    }
    async fn apply_secret_and_refresh(
        &self,
        reference: &SecretReference,
        value: &Secret,
    ) -> Result<()> {
        self.refresh
            .apply_secret_and_refresh(reference, value)
            .await
    }
}
pub struct SecretCheck {
    backend: Arc<dyn SecretBackend>,
}
impl SecretCheck {
    pub fn new(backend: Arc<dyn SecretBackend>) -> Self {
        Self { backend }
    }
    pub async fn check(&self, reference: &SecretReference) -> Result<SecretSourceCheckResult> {
        let values = self
            .backend
            .read(std::slice::from_ref(reference), true)
            .await?;
        let Some(value) = values
            .first()
            .and_then(Option::as_ref)
            .filter(|v| !v.expose().trim().is_empty())
        else {
            let label = self.backend.label(reference).await?;
            return Ok(SecretSourceCheckResult::Failed {
                detail: format!(
                    "Could not read this secret from \"{label}\". Check the provider's settings and that its store is unlocked."
                ),
            });
        };
        self.backend
            .apply_secret_and_refresh(reference, value)
            .await?;
        Ok(SecretSourceCheckResult::Resolved)
    }
    pub async fn require_settings_secret(&self, value: &SettingsSecretValue) -> Result<Secret> {
        match value {
            SettingsSecretValue::Literal(value) => Ok(Secret::new(value)),
            SettingsSecretValue::Reference(reference) => self
                .backend
                .read(std::slice::from_ref(reference), false)
                .await?
                .into_iter()
                .next()
                .flatten()
                .filter(|v| !v.expose().trim().is_empty())
                .ok_or(Error::Refused("settings secret could not be resolved")),
        }
    }
    pub async fn resolve_settings_secrets(
        &self,
        values: &[SettingsSecretValue],
    ) -> Result<Vec<SettingsSecretValue>> {
        let references: Vec<_> = values
            .iter()
            .filter_map(|v| match v {
                SettingsSecretValue::Reference(r) => Some(r.clone()),
                _ => None,
            })
            .collect();
        let mut resolved = self.backend.read(&references, false).await?.into_iter();
        Ok(values
            .iter()
            .map(|v| match v {
                SettingsSecretValue::Reference(_) => resolved
                    .next()
                    .flatten()
                    .filter(|s| !s.expose().trim().is_empty())
                    .map_or_else(|| v.clone(), |s| SettingsSecretValue::Literal(s.0)),
                _ => v.clone(),
            })
            .collect())
    }
}
