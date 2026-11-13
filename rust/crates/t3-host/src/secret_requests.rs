use crate::{
    background::Clock,
    secrets::{Secret, SecretBackend},
};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_config::secrets::SecretStore;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId, TurnItemId},
    secret_request::{
        SecretRequestAnswer, SecretRequestAnswerInput, SecretRequestFailureReason as Reason,
    },
};
use tokio::sync::Mutex;

const TTL_MS: i64 = 86_400_000;
#[derive(Debug, thiserror::Error)]
#[error("secret request failed: {0:?}")]
pub struct RequestError(pub Reason);
type Result<T> = std::result::Result<T, RequestError>;

#[derive(Debug, Clone)]
pub struct RequestCard {
    pub project_id: ProjectId,
    pub thread_id: ThreadId,
    pub item_id: TurnItemId,
    pub status: String,
    pub run_active: bool,
    pub file_delivery: bool,
}
#[async_trait]
pub trait RequestCards: Send + Sync {
    async fn read(&self, thread: &ThreadId, item: &TurnItemId) -> Result<RequestCard>;
    /// Commit a secret_request.record command with id secret-request:<item>:<status>.
    /// Its payload contains status and card metadata only, never the value or ref.
    async fn record(&self, card: &RequestCard, status: &str) -> Result<()>;
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSecret {
    project_id: String,
    value: String,
    saved_at: i64,
}
pub struct SecretRequests {
    directory: PathBuf,
    store: SecretStore,
    key: Vec<u8>,
    clock: Arc<dyn Clock>,
    cards: Arc<dyn RequestCards>,
    resolver: Arc<dyn SecretBackend>,
    gate: Mutex<()>,
}
pub struct SecretExpiryWorker(tokio::task::JoinHandle<()>);
impl Drop for SecretExpiryWorker {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl SecretRequests {
    pub async fn start(self: &Arc<Self>) -> SecretExpiryWorker {
        let _ = self.sweep_expired().await;
        let service = self.clone();
        SecretExpiryWorker(tokio::spawn(async move {
            let hour = std::time::Duration::from_secs(3600);
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + hour, hour);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                let _ = service.sweep_expired().await;
            }
        }))
    }
    pub fn new(
        directory: PathBuf,
        clock: Arc<dyn Clock>,
        cards: Arc<dyn RequestCards>,
        resolver: Arc<dyn SecretBackend>,
    ) -> Result<Self> {
        let store = SecretStore::new(&directory);
        let key = match store
            .get("secret-request-salt")
            .map_err(|_| RequestError(Reason::ReadFailed))?
        {
            Some(key) => key,
            None => {
                let mut bytes = [0_u8; 32];
                getrandom::fill(&mut bytes).map_err(|_| RequestError(Reason::StoreFailed))?;
                if store.create("secret-request-salt", &bytes).is_err() {
                    store
                        .get("secret-request-salt")
                        .map_err(|_| RequestError(Reason::StoreFailed))?
                        .ok_or(RequestError(Reason::StoreFailed))?
                } else {
                    bytes.to_vec()
                }
            }
        };
        Ok(Self {
            directory,
            store,
            key: hex::encode(key).into_bytes(),
            clock,
            cards,
            resolver,
            gate: Mutex::new(()),
        })
    }
    pub fn ref_for(&self, thread: &ThreadId, item: &str) -> Result<String> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key)
            .map_err(|_| RequestError(Reason::StoreFailed))?;
        mac.update(thread.0.as_bytes());
        mac.update(&[0]);
        mac.update(item.as_bytes());
        Ok(format!(
            "secret-ref:{}",
            &hex::encode(mac.finalize().into_bytes())[..32]
        ))
    }
    fn name(reference: &str) -> Result<String> {
        let suffix = reference
            .strip_prefix("secret-ref:")
            .filter(|s| {
                s.len() == 32
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or(RequestError(Reason::InvalidRef))?;
        Ok(format!("secret-request-{suffix}"))
    }
    pub async fn answer(&self, input: SecretRequestAnswerInput) -> Result<()> {
        let _guard = self.gate.lock().await;
        let card = self
            .cards
            .read(&input.thread_id, &input.turn_item_id)
            .await?;
        if card.status != "pending" {
            return Err(RequestError(Reason::AlreadyAnswered));
        }
        if !card.run_active {
            return Err(RequestError(Reason::AgentStopped));
        }
        let value = match &input.answer {
            SecretRequestAnswer::Save { secret } => Some(secret.clone()),
            SecretRequestAnswer::Decline => None,
            SecretRequestAnswer::Reference { reference } => Some(
                self.resolver
                    .read(std::slice::from_ref(reference), true)
                    .await
                    .map_err(|_| RequestError(Reason::ReferenceUnreadable))?
                    .into_iter()
                    .next()
                    .flatten()
                    .filter(|v| !v.expose().trim().is_empty())
                    .ok_or(RequestError(Reason::ReferenceUnreadable))?
                    .expose()
                    .to_owned(),
            ),
        };
        let name = Self::name(&self.ref_for(&input.thread_id, &input.turn_item_id.0)?)?;
        if let Some(value) = &value {
            let encoded = t3_contracts::stored::to_string(&StoredSecret {
                project_id: card.project_id.0.clone(),
                value: value.clone(),
                saved_at: self.clock.now_ms(),
            })
            .map_err(|_| RequestError(Reason::StoreFailed))?;
            if self.store.create(&name, encoded.as_bytes()).is_err()
                && self
                    .store
                    .get(&name)
                    .map_err(|_| RequestError(Reason::StoreFailed))?
                    .is_none()
            {
                return Err(RequestError(Reason::StoreFailed));
            }
        }
        let status = if value.is_some() { "saved" } else { "declined" };
        if self.cards.record(&card, status).await.is_err() {
            if value.is_some() {
                let _ = self.store.remove(&name);
            }
            return Err(RequestError(Reason::RecordFailed));
        }
        if value.is_some()
            && self
                .cards
                .read(&input.thread_id, &input.turn_item_id)
                .await?
                .status
                != "saved"
        {
            let _ = self.store.remove(&name);
            return Err(RequestError(Reason::AgentStopped));
        }
        Ok(())
    }
    pub fn saved_ref(&self, thread: &ThreadId, item: &TurnItemId) -> Result<Option<String>> {
        let reference = self.ref_for(thread, &item.0)?;
        Ok(self
            .store
            .get(&Self::name(&reference)?)
            .ok()
            .flatten()
            .map(|_| reference))
    }
    fn read_ref(&self, reference: &str, project: &ProjectId) -> Result<Secret> {
        let name = Self::name(reference)?;
        let stored = self
            .store
            .get(&name)
            .map_err(|_| RequestError(Reason::ReadFailed))?
            .and_then(|bytes| serde_json::from_slice::<StoredSecret>(&bytes).ok())
            .filter(|s| s.project_id == project.0)
            .ok_or(RequestError(Reason::RefUnavailable))?;
        if self.clock.now_ms().saturating_sub(stored.saved_at) > TTL_MS {
            let _ = self.store.remove(&name);
            return Err(RequestError(Reason::RefExpired));
        }
        Ok(Secret::new(stored.value))
    }
    pub async fn consume(&self, reference: &str, project: &ProjectId) -> Result<Secret> {
        let _guard = self.gate.lock().await;
        let value = self.read_ref(reference, project)?;
        self.store
            .remove(&Self::name(reference)?)
            .map_err(|_| RequestError(Reason::ConsumeFailed))?;
        Ok(value)
    }
    fn write_file(&self, reference: &str, value: &Secret) -> Result<PathBuf> {
        let suffix = reference
            .strip_prefix("secret-ref:")
            .ok_or(RequestError(Reason::InvalidRef))?;
        Self::name(reference)?;
        let directory = self.directory.join("files");
        fs::create_dir_all(&directory).map_err(|_| RequestError(Reason::StoreFailed))?;
        if fs::symlink_metadata(&directory)
            .map_err(|_| RequestError(Reason::StoreFailed))?
            .file_type()
            .is_symlink()
        {
            return Err(RequestError(Reason::StoreFailed));
        }
        private_permissions(&directory, true)?;
        let file = directory.join(suffix);
        if file.exists() {
            if !fs::symlink_metadata(&file)
                .map_err(|_| RequestError(Reason::StoreFailed))?
                .is_file()
            {
                return Err(RequestError(Reason::StoreFailed));
            }
            return Ok(file);
        }
        let mut temp = tempfile::NamedTempFile::new_in(&directory)
            .map_err(|_| RequestError(Reason::StoreFailed))?;
        private_permissions(temp.path(), false)?;
        temp.write_all(value.expose().as_bytes())
            .map_err(|_| RequestError(Reason::StoreFailed))?;
        temp.as_file()
            .sync_all()
            .map_err(|_| RequestError(Reason::StoreFailed))?;
        temp.persist_noclobber(&file)
            .map_err(|_| RequestError(Reason::StoreFailed))?;
        Ok(file)
    }
    pub async fn save_to_file(&self, thread: &ThreadId, item: &TurnItemId) -> Result<PathBuf> {
        let _guard = self.gate.lock().await;
        let card = self.cards.read(thread, item).await?;
        if card.status != "saved" || !card.file_delivery {
            return Err(RequestError(Reason::NotAFileRequest));
        }
        let reference = self.ref_for(thread, &item.0)?;
        let file = self.directory.join("files").join(&reference[11..]);
        let file = if file.exists() {
            self.write_file(&reference, &Secret::new(""))?
        } else {
            self.write_file(&reference, &self.read_ref(&reference, &card.project_id)?)?
        };
        self.store
            .remove(&Self::name(&reference)?)
            .map_err(|_| RequestError(Reason::ConsumeFailed))?;
        Ok(file)
    }
    pub async fn save_pasted(
        &self,
        thread: &ThreadId,
        namespace: &str,
        key: &str,
        value: &Secret,
    ) -> Result<PathBuf> {
        let _guard = self.gate.lock().await;
        self.write_file(
            &self.ref_for(thread, &format!("pasted\0{namespace}\0{key}"))?,
            value,
        )
    }
    pub async fn sweep_expired(&self) -> Result<usize> {
        let _guard = self.gate.lock().await;
        let mut removed = 0;
        let entries = match fs::read_dir(&self.directory) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err(RequestError(Reason::ReadFailed)),
        };
        for entry in entries {
            let entry = entry.map_err(|_| RequestError(Reason::ReadFailed))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(suffix) = name
                .strip_prefix("secret-request-")
                .and_then(|s| s.strip_suffix(".bin"))
            else {
                continue;
            };
            if Self::name(&format!("secret-ref:{suffix}")).is_err() {
                continue;
            }
            let stored = fs::read(entry.path())
                .ok()
                .and_then(|b| serde_json::from_slice::<StoredSecret>(&b).ok());
            if stored.is_none_or(|s| self.clock.now_ms().saturating_sub(s.saved_at) > TTL_MS) {
                self.store
                    .remove(name.trim_end_matches(".bin"))
                    .map_err(|_| RequestError(Reason::ConsumeFailed))?;
                removed += 1;
            }
        }
        let files = self.directory.join("files");
        if let Ok(entries) = fs::read_dir(&files) {
            for entry in entries {
                let entry = entry.map_err(|_| RequestError(Reason::ReadFailed))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if Self::name(&format!("secret-ref:{}", name.trim_end_matches(".partial"))).is_err()
                {
                    continue;
                }
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|_| RequestError(Reason::ReadFailed))?;
                if !metadata.is_file() {
                    continue;
                }
                let expired = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .is_none_or(|d| {
                        i128::from(self.clock.now_ms()) - d.as_millis() as i128 > i128::from(TTL_MS)
                    });
                if expired {
                    fs::remove_file(entry.path())
                        .map_err(|_| RequestError(Reason::ConsumeFailed))?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}
fn private_permissions(path: &Path, directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
        .map_err(|_| RequestError(Reason::StoreFailed))?;
    }
    #[cfg(not(unix))]
    let _ = (path, directory);
    Ok(())
}
