use super::app::{AppSession, AppSessionStore};
use crate::{FailureReason, ForgeError, Result};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use t3_config::{secrets::SecretStore, settings::SettingsService};

pub struct SettingsAppSessions {
    settings: Arc<SettingsService>,
    secrets: SecretStore,
    revision: AtomicU64,
}
fn failed() -> ForgeError {
    ForgeError::provider(
        "github",
        "appSession",
        FailureReason::Failed,
        "The server could not read or save the GitHub sign-in.",
    )
}
fn host(host: &str) -> Result<String> {
    let host = host.trim().to_ascii_lowercase();
    if host != "github.com" {
        return Err(failed());
    }
    Ok(host)
}
fn slot(host: &str) -> String {
    format!(
        "source-control-{}-{}-{}",
        URL_SAFE_NO_PAD.encode("github"),
        URL_SAFE_NO_PAD.encode("signIns"),
        URL_SAFE_NO_PAD.encode(host)
    )
}
fn read(
    snapshot: &t3_contracts::settings::ServerSettings,
    secrets: &SecretStore,
    host: &str,
) -> t3_config::Result<Option<AppSession>> {
    let encoded = snapshot
        .source_control_hosts
        .get("github")
        .and_then(|v| v.get("signIns"))
        .and_then(|v| v.get(host))
        .and_then(serde_json::Value::as_str);
    if encoded.is_none() {
        return Ok(None);
    }
    let Some(bytes) = secrets.get(&slot(host))? else {
        return Ok(None);
    };
    if bytes.len() > 64 * 1024 {
        return Ok(None);
    }
    Ok(serde_json::from_slice::<AppSession>(&bytes)
        .ok()
        .filter(|v| !v.access_token.trim().is_empty()))
}
impl SettingsAppSessions {
    pub fn new(settings: Arc<SettingsService>, secrets: SecretStore) -> Self {
        Self {
            settings,
            secrets,
            revision: AtomicU64::new(0),
        }
    }
}
#[async_trait]
impl AppSessionStore for SettingsAppSessions {
    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    async fn read(&self, raw_host: &str) -> Result<Option<AppSession>> {
        let host = host(raw_host)?;
        self.settings
            .with_snapshot(async |snapshot| read(snapshot, &self.secrets, &host))
            .await
            .map_err(|_| failed())
    }
    async fn write(&self, raw_host: &str, session: AppSession) -> Result<()> {
        let host = host(raw_host)?;
        if session.access_token.trim().is_empty() {
            return Err(failed());
        }
        let encoded = serde_json::to_string(&session).map_err(|_| failed())?;
        if encoded.len() > 64 * 1024 {
            return Err(failed());
        }
        let snapshot = self.settings.snapshot().await.map_err(|_| failed())?;
        let mut sign_ins = snapshot
            .source_control_hosts
            .get("github")
            .and_then(|v| v.get("signIns"))
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        sign_ins.insert(host, serde_json::Value::String(encoded));
        let patch = serde_json::from_value(
            serde_json::json!({"sourceControlHosts":{"github":{"signIns":sign_ins}}}),
        )
        .map_err(|_| failed())?;
        self.settings.update(&patch).await.map_err(|_| failed())?;
        self.revision.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    async fn replace(
        &self,
        raw_host: &str,
        previous_token: &str,
        session: Option<AppSession>,
    ) -> Result<bool> {
        let host = host(raw_host)?;
        if session
            .as_ref()
            .is_some_and(|s| s.access_token.trim().is_empty())
        {
            return Err(failed());
        }
        let bytes = session
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| failed())?;
        if bytes.as_ref().is_some_and(|b| b.len() > 64 * 1024) {
            return Err(failed());
        }
        let replaced = self
            .settings
            .with_snapshot(async |snapshot| {
                if read(snapshot, &self.secrets, &host)?
                    .is_none_or(|s| s.access_token != previous_token)
                {
                    return Ok(false);
                }
                if let Some(bytes) = bytes {
                    self.secrets.set(&slot(&host), &bytes).await?;
                } else {
                    self.secrets.remove(&slot(&host))?;
                }
                Ok(true)
            })
            .await
            .map_err(|_| failed())?;
        if replaced {
            self.revision.fetch_add(1, Ordering::AcqRel);
            self.settings.reload().await.map_err(|_| failed())?;
        }
        Ok(replaced)
    }
}
