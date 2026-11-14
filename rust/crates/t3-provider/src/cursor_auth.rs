use crate::{
    auth::{AuthBackend, AuthContext},
    cursor_catalog::CursorCatalog,
    secrets::CredentialStore,
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use t3_contracts::{provider_setup::*, wire::JsNumber};
use tokio::sync::mpsc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorCredentials {
    pub version: u8,
    pub backend_url: String,
    pub api_key: SecretString,
    pub created_at_ms: JsNumber,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub api_key_expires_at_ms: Option<JsNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub email: Option<String>,
}
impl std::fmt::Debug for CursorCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CursorCredentials([REDACTED])")
    }
}
#[async_trait]
pub trait CursorCredentialStore: Send + Sync {
    async fn load(&self) -> ServiceResult<Option<CursorCredentials>>;
    async fn save(&self, credentials: &CursorCredentials) -> ServiceResult<()>;
    async fn clear(&self) -> ServiceResult<()>;
}
pub struct ConfigCursorCredentials(CredentialStore);
impl ConfigCursorCredentials {
    pub fn new(directory: impl Into<std::path::PathBuf>, instance: &str) -> Self {
        Self(CredentialStore::new(
            t3_config::secrets::SecretStore::new(directory),
            "cursor",
            instance,
        ))
    }
    pub fn binding_key(&self) -> &str {
        self.0.key()
    }
    pub async fn migrate_legacy(&self, path: &std::path::Path) -> ServiceResult<()> {
        let legacy = match std::fs::read(path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(ServiceError::Storage),
        };
        if self.load().await?.is_none()
            && let Ok(credentials) = decode(&legacy)
        {
            self.save(&credentials).await?;
        }
        std::fs::remove_file(path).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Storage,
        ))
    }
}
fn decode(bytes: &[u8]) -> ServiceResult<CursorCredentials> {
    let credentials: CursorCredentials = serde_json::from_slice(bytes).map_err(
        crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
    )?;
    if credentials.version != 1 {
        return Err(ServiceError::Response);
    }
    Ok(credentials)
}
#[async_trait]
impl CursorCredentialStore for ConfigCursorCredentials {
    async fn load(&self) -> ServiceResult<Option<CursorCredentials>> {
        Ok(self
            .0
            .get()
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Storage,
            ))?
            .and_then(|bytes| decode(&bytes).ok()))
    }
    async fn save(&self, credentials: &CursorCredentials) -> ServiceResult<()> {
        if credentials.version != 1 {
            return Err(ServiceError::Response);
        }
        self.0
            .set(
                &serde_json::to_vec(credentials).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?,
            )
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Storage,
            ))
    }
    async fn clear(&self) -> ServiceResult<()> {
        self.0.remove().map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Storage,
        ))
    }
}
#[async_trait]
pub trait CursorLogin: Send + Sync {
    async fn login(
        &self,
        api_key_name: String,
        urls: mpsc::UnboundedSender<SecretString>,
        cancellation: t3_process::CancellationToken,
    ) -> ServiceResult<CursorCredentials>;
}
pub struct CursorAuth {
    pub enabled: bool,
    pub display_name: String,
    pub configured_key: Option<SecretString>,
    pub store: Arc<dyn CursorCredentialStore>,
    pub login: Arc<dyn CursorLogin>,
    pub catalog: Arc<CursorCatalog>,
    pub clock: Arc<dyn ProviderClock>,
}
impl CursorAuth {
    pub fn uses_api_key(&self) -> bool {
        self.configured_key
            .as_ref()
            .is_some_and(|key| !key.expose().trim().is_empty())
    }
    pub async fn read_api_key(&self) -> ServiceResult<Option<SecretString>> {
        if let Some(key) = self
            .configured_key
            .as_ref()
            .filter(|key| !key.expose().trim().is_empty())
        {
            return Ok(Some(Sensitive::new(key.expose().trim().into())));
        }
        Ok(self
            .store
            .load()
            .await?
            .filter(|credentials| {
                credentials
                    .api_key_expires_at_ms
                    .as_ref()
                    .is_none_or(|expiry| expiry.value() > self.clock.now_ms() as f64)
            })
            .map(|credentials| credentials.api_key))
    }
}
#[async_trait]
impl AuthBackend for CursorAuth {
    fn check_start(&self) -> ServiceResult<()> {
        if !self.enabled {
            return Err(ServiceError::Disabled);
        }
        if self.uses_api_key() {
            return Err(ServiceError::Unsupported);
        }
        Ok(())
    }
    fn check_logout(&self) -> ServiceResult<()> {
        if self.uses_api_key() {
            Err(ServiceError::Unsupported)
        } else {
            Ok(())
        }
    }
    fn is_logout_prompt(&self, text: &str, has_attachments: bool) -> bool {
        !has_attachments && text.trim() == "/logout"
    }
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>> {
        serde_json::from_value(serde_json::json!([{"id":"browser","name":"Sign in with Cursor","description":null,"type":"agent"}])).map_err(crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response))
    }
    async fn authenticate(&self, _: &str, context: AuthContext) -> ServiceResult<()> {
        let (urls, mut received) = mpsc::unbounded_channel();
        let login = self.login.login(
            format!("T3 Code - {}", self.display_name),
            urls,
            context.cancellation.clone(),
        );
        tokio::pin!(login);
        let credentials = loop {
            tokio::select! {
                result = &mut login => break result?,
                Some(url) = received.recv() => {
                    context.interaction(ProviderAuthInteraction::Browser {
                        id: context.flow_id.clone().into(), url: url.into_inner().into(), requires_consent: false,
                        accepts_callback: None,
                    }, false, false).await?;
                }
            }
        };
        let store = self.store.clone();
        let catalog = self.catalog.clone();
        context
            .commit(async move {
                store.save(&credentials).await?;
                let verified = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    catalog.read(&credentials.api_key),
                )
                .await;
                if !matches!(verified, Ok(Ok(_))) {
                    store.clear().await?;
                    return Err(ServiceError::Response);
                }
                Ok(())
            })
            .await
    }
    async fn logout(&self) -> ServiceResult<Option<String>> {
        self.store.clear().await?;
        Ok(Some("Signed out of Cursor.".into()))
    }
}
