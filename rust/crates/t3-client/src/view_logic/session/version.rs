use crate::{
    platform::{Platform, PlatformError, StorageArea, StorageKey, StorageWrite},
    types::EnvironmentId,
    view_logic::JsTrim,
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    rc::{Rc, Weak},
};
use t3_contracts::{
    environment::{ServerInstallation, ServerInstallationMember1Kind},
    server::ServerConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMismatch {
    pub client_version: String,
    pub server_version: String,
    pub hint: &'static str,
}
#[derive(PartialEq, Eq)]
struct Semver {
    core: [String; 3],
    prerelease: Vec<String>,
}
fn numeric_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}
fn semver(value: &str) -> Option<Semver> {
    let value = value.js_trim().split('+').next()?;
    let (main, pre) = value.split_once('-').unwrap_or((value, ""));
    let mut parts: Vec<_> = main
        .split('.')
        .map(str::js_trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    while !parts.is_empty() && parts.len() < 3 {
        parts.push("0".into());
    }
    if let Some(first) = parts.first_mut()
        && let Some(raw) = first.strip_prefix('v')
    {
        *first = raw.into();
    }
    if parts.len() != 3
        || parts
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(Semver {
        core: parts.try_into().ok()?,
        prerelease: pre
            .split('.')
            .map(str::js_trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    })
}
fn compare(a: &Semver, b: &Semver, nightly: bool) -> Ordering {
    for (a, b) in a.core.iter().zip(&b.core) {
        let order = numeric_cmp(a, b);
        if !order.is_eq() {
            return order;
        }
    }
    if !nightly {
        return Ordering::Equal;
    }
    for (a, b) in a.prerelease.iter().zip(&b.prerelease) {
        let numeric = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        let order = match (numeric(a), numeric(b)) {
            (true, true) => numeric_cmp(a, b),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a.cmp(b),
        };
        if !order.is_eq() {
            return order;
        }
    }
    a.prerelease.len().cmp(&b.prerelease.len())
}
pub fn resolve_version_mismatch(server: Option<&str>, client: &str) -> Option<VersionMismatch> {
    let server = server?.js_trim();
    let client = client.js_trim();
    if server.is_empty() || client.is_empty() {
        return None;
    }
    let behind = match (semver(server), semver(client)) {
        (Some(server), Some(client)) => compare(
            &server,
            &client,
            server.prerelease.first().is_some_and(|s| s == "nightly")
                && client.prerelease.first().is_some_and(|s| s == "nightly"),
        )
        .is_lt(),
        _ => server != client,
    };
    behind.then(|| VersionMismatch {
        client_version: client.into(),
        server_version: server.into(),
        hint: "Version mismatch. Try syncing the client and server to the same T3 Code version.",
    })
}
pub fn resolve_server_config_version_mismatch(
    config: Option<&ServerConfig>,
    client: &str,
) -> Option<VersionMismatch> {
    config.and_then(|c| resolve_version_mismatch(Some(&c.environment.server_version.0), client))
}
pub fn resolve_server_self_update_capability(
    config: Option<&ServerConfig>,
) -> Option<&t3_contracts::environment::ServerSelfUpdateCapability> {
    config.and_then(|c| c.environment.capabilities.server_self_update.as_ref())
}
pub fn supports_desktop_app_update(config: Option<&ServerConfig>) -> bool {
    config.is_some_and(|c| c.environment.capabilities.desktop_app_update == Some(true))
}
pub fn manual_server_update_command(
    version: &str,
    installation: Option<&ServerInstallation>,
) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    match installation {
        Some(ServerInstallation::NpmGlobal(i)) => format!(
            "npm install --global --prefix {} t3@{version}",
            quote(&i.prefix.0)
        ),
        Some(ServerInstallation::T3Launcher(i)) => format!(
            "{} update --base-dir {} {version}",
            quote(&i.launcher),
            quote(&i.base_dir)
        ),
        _ => {
            let runner = match installation {
                Some(ServerInstallation::Ephemeral(i)) => match i.kind {
                    ServerInstallationMember1Kind::PnpmDlx => "pnpm dlx",
                    ServerInstallationMember1Kind::Bunx => "bunx",
                    _ => "npx",
                },
                _ => "npx",
            };
            format!("{runner} t3@{version}")
        }
    }
}
pub fn version_mismatch_dismissal_key(
    environment: &EnvironmentId,
    mismatch: &VersionMismatch,
) -> String {
    format!(
        "{}:{}:{}",
        environment.0, mismatch.client_version, mismatch.server_version
    )
}
#[derive(Default, Serialize, Deserialize)]
pub struct VersionMismatchDismissals {
    pub keys: Vec<String>,
}
fn dismissal_storage_key() -> StorageKey {
    StorageKey::new("preferences", None, "t3code:version-mismatch-dismissals:v1")
}
impl VersionMismatchDismissals {
    pub async fn load(platform: &impl Platform) -> Result<Self, PlatformError> {
        platform
            .get(StorageArea::Ordinary, dismissal_storage_key())
            .await?
            .map_or_else(
                || Ok(Self::default()),
                |bytes| {
                    serde_json::from_slice(&bytes).map_err(|_| {
                        PlatformError::InvalidData("version-mismatch dismissals".into())
                    })
                },
            )
    }
    pub fn contains(&self, key: Option<&str>) -> bool {
        key.is_some_and(|key| !key.is_empty() && self.keys.iter().any(|k| k == key))
    }
    pub async fn dismiss(
        &mut self,
        key: Option<&str>,
        platform: &impl Platform,
    ) -> Result<(), PlatformError> {
        let Some(key) = key.filter(|key| !key.is_empty() && !self.contains(Some(key))) else {
            return Ok(());
        };
        let next = Self {
            keys: self
                .keys
                .iter()
                .cloned()
                .chain(std::iter::once(key.into()))
                .collect(),
        };
        let bytes = serde_json::to_vec(&next)
            .map_err(|_| PlatformError::InvalidData("version-mismatch dismissals".into()))?;
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: dismissal_storage_key(),
                    value: Some(bytes),
                }],
            )
            .await?;
        *self = next;
        Ok(())
    }
}
#[derive(Default)]
pub struct UpdateFailureDismissals<T>(Vec<Weak<T>>);
impl<T> UpdateFailureDismissals<T> {
    pub fn dismiss(&mut self, state: &Rc<T>, failed: bool) {
        if failed {
            self.0.retain(|entry| entry.strong_count() > 0);
            self.0.push(Rc::downgrade(state));
        }
    }
    pub fn contains(&self, state: &Rc<T>, failed: bool) -> bool {
        failed
            && self
                .0
                .iter()
                .any(|entry| entry.ptr_eq(&Rc::downgrade(state)))
    }
}
