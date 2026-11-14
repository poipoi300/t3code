use crate::sensitive::{ServiceError, ServiceResult};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{provider_instance::ProviderInstanceId, server::ServerProvider};
use tokio::{io::AsyncReadExt, sync::Mutex};
const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub fn correlated(cached: &ServerProvider, fallback: &ServerProvider) -> bool {
    cached.instance_id == fallback.instance_id && cached.driver == fallback.driver
}
pub fn hydrate(cached: &ServerProvider, mut fallback: ServerProvider) -> ServerProvider {
    if !correlated(cached, &fallback) || !fallback.enabled || cached.enabled != fallback.enabled {
        return fallback;
    }
    let slugs = fallback
        .models
        .iter()
        .map(|model| model.slug.clone())
        .collect::<BTreeSet<_>>();
    fallback.models.extend(
        cached
            .models
            .iter()
            .filter(|model| !model.is_custom && !slugs.contains(&model.slug))
            .cloned(),
    );
    fallback.installed = cached.installed;
    fallback.version = cached.version.clone();
    fallback.status = cached.status;
    fallback.auth = cached.auth.clone();
    fallback.checked_at = cached.checked_at.clone();
    fallback.slash_commands = cached.slash_commands.clone();
    fallback.skills = cached.skills.clone();
    fallback.message = cached
        .message
        .as_ref()
        .filter(|message| {
            message
                .as_ref()
                .is_some_and(|message| !message.0.is_empty())
        })
        .cloned();
    fallback
}
pub fn order(providers: &mut [ServerProvider]) {
    const DRIVERS: &[&str] = &[
        "codex",
        "claudeAgent",
        "cursor",
        "grok",
        "opencode",
        "antigravity",
        "fx",
        "kiro",
    ];
    let rank = |driver: &str| {
        DRIVERS
            .iter()
            .position(|entry| *entry == driver)
            .unwrap_or(DRIVERS.len())
    };
    let name = |provider: &ServerProvider| {
        provider
            .display_name
            .as_ref()
            .and_then(Option::as_ref)
            .map(|name| name.0.clone())
            .unwrap_or_default()
    };
    providers.sort_by(|left, right| {
        rank(&left.driver.0)
            .cmp(&rank(&right.driver.0))
            .then_with(|| left.driver.cmp(&right.driver))
            .then_with(|| name(left).cmp(&name(right)))
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
}
pub fn path(cache: &Path, id: &ProviderInstanceId) -> ServiceResult<PathBuf> {
    if id.0.is_empty()
        || id
            .0
            .chars()
            .any(|character| character.is_control() || "/\\<>:\"|?*".contains(character))
        || matches!(id.0.as_str(), "." | "..")
        || id.0.ends_with(['.', ' '])
    {
        return Err(ServiceError::Storage);
    }
    Ok(cache.join(format!("{}.json", id.0)))
}
pub struct ProviderStatusCache {
    directory: PathBuf,
    writes: Arc<Mutex<()>>,
}
impl ProviderStatusCache {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            writes: Arc::new(Mutex::new(())),
        }
    }
    pub async fn read(&self, id: &ProviderInstanceId) -> Option<ServerProvider> {
        let path = path(&self.directory, id).ok()?;
        let file = tokio::fs::File::open(path).await.ok()?;
        if file.metadata().await.ok()?.len() > MAX_BYTES {
            return None;
        }
        let mut bytes = vec![];
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() as u64 > MAX_BYTES {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    }
    pub async fn write(&self, provider: &ServerProvider) -> ServiceResult<()> {
        let path = path(&self.directory, &provider.instance_id)?;
        let guard = self.writes.clone().lock_owned().await;
        let mut cached = provider.clone();
        cached.update_state = None;
        cached.workspace_snapshots = None;
        let bytes = serde_json::to_vec_pretty(&cached).map_err(|_| ServiceError::Response)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(ServiceError::Response);
        }
        tokio::task::spawn_blocking(move || -> ServiceResult<()> {
            use std::io::Write;
            let _guard = guard;
            let parent = path.parent().ok_or(ServiceError::Storage)?;
            std::fs::create_dir_all(parent).map_err(|_| ServiceError::Storage)?;
            let mut file =
                tempfile::NamedTempFile::new_in(parent).map_err(|_| ServiceError::Storage)?;
            file.write_all(&bytes)
                .and_then(|_| file.write_all(b"\n"))
                .map_err(|_| ServiceError::Storage)?;
            file.persist(path).map_err(|_| ServiceError::Storage)?;
            Ok(())
        })
        .await
        .map_err(|_| ServiceError::Storage)?
    }
}
