use crate::{Error, Result};
use async_trait::async_trait;
use std::sync::Arc;

/// `create` must publish complete bytes atomically and return false only for AlreadyExists.
#[async_trait]
pub trait Secrets: Send + Sync {
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>>;
    async fn set(&self, name: &str, value: &[u8]) -> Result<()>;
    async fn create(&self, name: &str, value: &[u8]) -> Result<bool>;
    async fn remove(&self, name: &str) -> Result<()>;
}
pub type SecretStore = Arc<dyn Secrets>;

pub async fn read_text(store: &dyn Secrets, name: &str) -> Result<Option<String>> {
    store
        .get(name)
        .await?
        .map(|b| String::from_utf8(b).map_err(|_| Error::Storage))
        .transpose()
}

#[cfg(feature = "auth")]
#[async_trait]
impl Secrets for t3_auth::ServerSecretStore {
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.get(name).await.map_err(|_| Error::Storage)
    }
    async fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        self.set(name, value).await.map_err(|_| Error::Storage)
    }
    async fn create(&self, name: &str, value: &[u8]) -> Result<bool> {
        match self.create(name, value).await {
            Ok(()) => Ok(true),
            Err(e) if e.is_already_exists() => Ok(false),
            Err(_) => Err(Error::Storage),
        }
    }
    async fn remove(&self, name: &str) -> Result<()> {
        self.remove(name).await.map_err(|_| Error::Storage)
    }
}

/// Replay markers deliberately outlive the proof; the host prunes them with auth's replay job.
pub async fn consume_replay(
    store: &dyn Secrets,
    kind: &str,
    jti: &str,
    nonce: &str,
    now: &str,
) -> Result<()> {
    let valid = |s: &str| {
        !s.is_empty()
            && s.len() <= 512
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    };
    if !matches!(kind, "mint" | "health") || !valid(jti) || !valid(nonce) {
        return Err(Error::Proof);
    }
    // Spend both markers even if one was already used, matching consumeCloudReplayGuards.
    let jti_name = format!("cloud-{kind}-jti-{jti}");
    let nonce_name = format!("cloud-{kind}-nonce-{nonce}");
    let (a, b) = tokio::join!(
        store.create(&jti_name, now.as_bytes()),
        store.create(&nonce_name, now.as_bytes())
    );
    if a? && b? { Ok(()) } else { Err(Error::Replay) }
}
