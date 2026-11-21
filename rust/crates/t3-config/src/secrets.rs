use crate::{Result, files, invalid};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub const REDACTED: &str = "••••••";
fn encode(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(s.as_bytes())
}
#[derive(Clone)]
pub struct SecretStore {
    directory: PathBuf,
}
impl SecretStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
    fn path(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty()
            || name.chars().any(|c| c == '/' || c == '\\' || c == '\0')
            || matches!(name, "." | "..")
        {
            return Err(invalid("secret name", "must be a file name"));
        }
        Ok(self.directory.join(format!("{name}.bin")))
    }
    fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)
            .map_err(|e| files::io("prepare secrets", &self.directory, e))?;
        permissions(&self.directory, 0o700)
    }
    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path(name)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(files::io("read secret", &path, e)),
        }
    }
    pub async fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        self.prepare()?;
        let path = self.path(name)?;
        // Temp files inherit owner-only access before their name is published.
        let temp = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|e| files::io("create secret", &path, e))?;
        permissions(temp.path(), 0o600)?;
        fs::write(temp.path(), value).map_err(|e| files::io("write secret", &path, e))?;
        temp.persist(&path)
            .map_err(|e| files::io("publish secret", &path, e.error))?;
        permissions(&path, 0o600)
    }
    pub fn create(&self, name: &str, value: &[u8]) -> Result<()> {
        self.prepare()?;
        let path = self.path(name)?;
        let temp = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|e| files::io("create secret", &path, e))?;
        permissions(temp.path(), 0o600)?;
        fs::write(temp.path(), value).map_err(|e| files::io("write secret", &path, e))?;
        temp.as_file()
            .sync_all()
            .map_err(|e| files::io("sync secret", &path, e))?;
        fs::hard_link(temp.path(), &path).map_err(|e| files::io("create secret", &path, e))
    }
    pub fn remove(&self, name: &str) -> Result<()> {
        let path = self.path(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(files::io("remove secret", &path, e)),
        }
    }
}
fn permissions(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|e| files::io("restrict permissions", path, e))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}
#[derive(Clone)]
struct Slot {
    path: String,
    name: String,
    environment: bool,
    sensitive: bool,
    redacted: bool,
}
fn source_control_secret_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "github" | "gitlab" => &["tokens", "signIns"],
        "bitbucket" => &["accessToken", "apiToken"],
        "gitcafe" => &["token"],
        _ => &[],
    }
}

/// External credentials re-read by a global provider refresh.
pub fn settings_refresh_references(
    settings: &t3_contracts::settings::ServerSettings,
) -> Vec<t3_contracts::secret_reference::SecretReference> {
    use serde::Deserialize;
    use t3_contracts::{
        secret_reference::SecretReference, settings_secret_value::SettingsSecretValue,
    };
    let speech = &settings.cloud_speech_credentials;
    let mut refs: Vec<_> = settings
        .usage_limit_sources
        .values()
        .map(|s| &s.management_key)
        .chain(settings.text_generators.values().map(|s| &s.api_key))
        .chain(settings.eas_device_hosts.iter().map(|h| &h.access_token))
        .chain(settings.simlock_device_hosts.iter().map(|h| &h.agent_token))
        .chain([&speech.openai, &speech.xai, &speech.openrouter])
        .filter_map(|s| match s {
            SettingsSecretValue::Reference(reference) => Some(reference.clone()),
            SettingsSecretValue::Literal(_) => None,
        })
        .collect();
    for (kind, host) in &settings.source_control_hosts {
        for field in source_control_secret_fields(kind) {
            let Some(value) = host.get(*field) else {
                continue;
            };
            if let Ok(reference) = SecretReference::deserialize(value) {
                refs.push(reference);
            } else if let Some(entries) = value.as_object() {
                refs.extend(
                    entries
                        .values()
                        .filter_map(|v| SecretReference::deserialize(v).ok()),
                );
            }
        }
    }
    refs
}

fn slots(value: &Value) -> Vec<Slot> {
    let mut slots = Vec::new();
    let mut field = |path: String, name: String| {
        let Some(value) = value.pointer(&path) else {
            return;
        };
        if let Some(text) = value.as_str() {
            slots.push(Slot {
                path,
                name,
                environment: false,
                sensitive: true,
                redacted: text == REDACTED,
            });
        } else if value.get("reference").is_none()
            && let Some(map) = value.as_object()
        {
            for (host, value) in map {
                if let Some(text) = value.as_str() {
                    slots.push(Slot {
                        path: format!("{path}/{}", pointer_escape(host)),
                        name: format!("{name}-{}", encode(&host.trim().to_lowercase())),
                        environment: false,
                        sensitive: true,
                        redacted: text == REDACTED,
                    });
                }
            }
        }
    };
    for (id, instance) in value["providerInstances"].as_object().into_iter().flatten() {
        let driver = instance["driver"].as_str().unwrap_or("");
        let fields: &[&str] = match driver {
            "antigravity" => &["apiKey"],
            "opencode" => &["serverPassword"],
            _ => &[],
        };
        for f in fields {
            field(
                format!("/providerInstances/{}/config/{f}", pointer_escape(id)),
                format!(
                    "provider-config-{}-{}-{}",
                    encode(id),
                    encode(driver),
                    encode(f)
                ),
            );
        }
    }
    for (kind, fields) in value["sourceControlHosts"]
        .as_object()
        .into_iter()
        .flatten()
    {
        for f in source_control_secret_fields(kind) {
            if fields.get(*f).is_some() {
                field(
                    format!("/sourceControlHosts/{}/{f}", pointer_escape(kind)),
                    format!("source-control-{}-{}", encode(kind), encode(f)),
                );
            }
        }
    }
    for (group, f, prefix) in [
        ("usageLimitSources", "managementKey", "usage-limit-source"),
        ("textGenerators", "apiKey", "text-generator"),
    ] {
        for (id, _) in value[group].as_object().into_iter().flatten() {
            field(
                format!("/{group}/{}/{f}", pointer_escape(id)),
                format!(
                    "{prefix}-{}{}",
                    encode(id),
                    if group == "usageLimitSources" {
                        String::new()
                    } else {
                        format!("-{}", encode(f))
                    }
                ),
            );
        }
    }
    for provider in ["openai", "xai", "openrouter"] {
        field(
            format!("/cloudSpeechCredentials/{provider}"),
            format!("voice-cloud-{provider}"),
        );
    }
    for (group, f, prefix) in [
        ("easDeviceHosts", "accessToken", "device-host"),
        ("simlockDeviceHosts", "agentToken", "simlock-device-host"),
    ] {
        for (index, host) in value[group].as_array().into_iter().flatten().enumerate() {
            if let Some(id) = host["id"].as_str() {
                field(
                    format!("/{group}/{index}/{f}"),
                    format!("{prefix}-{}-{}", encode(id), encode(f)),
                );
            }
        }
    }
    for (id, instance) in value["providerInstances"].as_object().into_iter().flatten() {
        for (i, var) in instance["environment"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if !var["value"].is_string() || var.get("secret").is_some() {
                continue;
            }
            if let Some(name) = var["name"].as_str() {
                slots.push(Slot {
                    path: format!(
                        "/providerInstances/{}/environment/{i}/value",
                        pointer_escape(id)
                    ),
                    name: format!("provider-env-{}-{}", encode(id), encode(name)),
                    environment: true,
                    sensitive: var["sensitive"] == true,
                    redacted: var["valueRedacted"] == true,
                });
            }
        }
    }
    slots
}
fn pointer_escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
fn set_slot(value: &mut Value, slot: &Slot, text: &str, redacted: bool) {
    if let Some(target) = value.pointer_mut(&slot.path) {
        *target = json!(if slot.environment || !redacted {
            text
        } else {
            REDACTED
        });
    }
    if slot.environment
        && let Some((parent, _)) = slot.path.rsplit_once('/')
        && let Some(Value::Object(var)) = value.pointer_mut(parent)
    {
        if redacted {
            var.insert("valueRedacted".into(), json!(true));
        } else {
            var.remove("valueRedacted");
        }
    }
}
pub(crate) async fn migrate_inline(store: &SecretStore, value: &mut Value) -> bool {
    let mut changed = false;
    for slot in slots(value) {
        if slot.environment || slot.redacted {
            continue;
        }
        let text = value
            .pointer(&slot.path)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if text.is_empty() {
            continue;
        }
        if store.set(&slot.name, text.as_bytes()).await.is_ok() {
            set_slot(value, &slot, "", true);
            changed = true;
        } else {
            tracing::warn!("failed to move a settings secret into the secret store");
        }
    }
    changed
}
pub(crate) fn materialize(store: &SecretStore, mut value: Value) -> Result<Value> {
    let mut cached = BTreeMap::new();
    for slot in slots(&value) {
        if !slot.sensitive || !slot.redacted {
            continue;
        }
        let text = if let Some(text) = cached.get(&slot.name) {
            text
        } else {
            let bytes = store.get(&slot.name)?.unwrap_or_default();
            cached
                .entry(slot.name.clone())
                .or_insert_with(|| String::from_utf8_lossy(&bytes).into_owned())
        };
        set_slot(&mut value, &slot, text, slot.environment);
    }
    Ok(value)
}
pub fn redact_for_client(value: &mut Value, secret_references: bool) {
    for slot in slots(value) {
        if slot.sensitive {
            let has = value
                .pointer(&slot.path)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty());
            set_slot(value, &slot, "", has || slot.redacted);
        }
    }
    if !secret_references {
        for slot in reference_slots(value) {
            if let Some(target) = value.pointer_mut(&slot.path) {
                *target = json!(REDACTED);
            }
        }
    }
    for entry in value["secretProviders"]
        .as_object_mut()
        .into_iter()
        .flat_map(|m| m.values_mut())
    {
        if entry["kind"] != "one_password"
            && let Some(map) = entry.as_object_mut()
        {
            map.remove("config");
        }
    }
}
fn replace_references(value: &mut Value, change: &mut impl FnMut(&Value) -> Option<Value>) {
    if value.get("provider").is_some_and(Value::is_string)
        && value.get("reference").is_some_and(Value::is_string)
    {
        if let Some(next) = change(value) {
            *value = next;
        }
        return;
    }
    match value {
        Value::Object(map) => {
            for value in map.values_mut() {
                replace_references(value, change);
            }
        }
        Value::Array(items) => {
            for value in items {
                replace_references(value, change);
            }
        }
        _ => (),
    }
}
pub(crate) fn restore_echoed_references(current: &Value, next: &mut Value) {
    for slot in reference_slots(current) {
        if next.pointer(&slot.path).is_some_and(|v| v == REDACTED)
            && let (Some(old), Some(target)) =
                (current.pointer(&slot.path), next.pointer_mut(&slot.path))
        {
            *target = old.clone();
        }
    }
}
fn reference_slots(value: &Value) -> Vec<Slot> {
    let mut mask = value.clone();
    replace_references(&mut mask, &mut |_| Some(json!(REDACTED)));
    slots(&mask)
        .into_iter()
        .filter(|slot| {
            value.pointer(&slot.path).is_some_and(|v| {
                v.get("provider").is_some_and(Value::is_string)
                    && v.get("reference").is_some_and(Value::is_string)
            })
        })
        .collect()
}
pub(crate) fn remove_provider_references(current: &Value, next: &mut Value) {
    let removed: BTreeSet<_> = current["secretProviders"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.keys())
        .filter(|id| next["secretProviders"].get(*id).is_none())
        .cloned()
        .collect();
    if removed.is_empty() {
        return;
    }
    for instance in next["providerInstances"]
        .as_object_mut()
        .into_iter()
        .flat_map(|m| m.values_mut())
    {
        if let Some(vars) = instance
            .get_mut("environment")
            .and_then(Value::as_array_mut)
        {
            vars.retain(|v| {
                !v["secret"]["provider"]
                    .as_str()
                    .is_some_and(|id| removed.contains(id))
            });
        }
    }
    for slot in reference_slots(next) {
        let removed = next
            .pointer(&slot.path)
            .and_then(|v| v["provider"].as_str())
            .is_some_and(|id| removed.contains(id));
        if removed && let Some(target) = next.pointer_mut(&slot.path) {
            *target = json!("");
        }
    }
}
pub(crate) struct SecretTransaction {
    store: SecretStore,
    before: Vec<(String, Option<Vec<u8>>)>,
    committed: bool,
}
impl SecretTransaction {
    pub(crate) async fn apply(
        store: &SecretStore,
        current: &Value,
        next: &mut Value,
    ) -> Result<Self> {
        let previous = slots(current);
        let mut kept = BTreeSet::new();
        let mut claimed = BTreeSet::new();
        let mut inline_writes = BTreeMap::new();
        let mut written = BTreeSet::new();
        let mut changes = Vec::new();
        for slot in slots(next) {
            let text = next
                .pointer(&slot.path)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if !slot.sensitive {
                changes.push((slot.name.clone(), None));
                set_slot(next, &slot, &text, false);
            } else if slot.redacted {
                kept.insert(slot.name.clone());
                claimed.insert(slot.name.clone());
                if let Some(old) = previous
                    .iter()
                    .find(|s| s.name == slot.name && s.sensitive && !s.redacted)
                    && let Some(text) = current
                        .pointer(&old.path)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                {
                    inline_writes.insert(slot.name.clone(), text.as_bytes().to_vec());
                }
                set_slot(next, &slot, "", true);
            } else if text.is_empty() {
                changes.push((slot.name.clone(), None));
                set_slot(next, &slot, "", false);
            } else {
                claimed.insert(slot.name.clone());
                written.insert(slot.name.clone());
                changes.push((slot.name.clone(), Some(text.into_bytes())));
                set_slot(next, &slot, "", true);
            }
        }
        for (name, value) in inline_writes {
            if !written.contains(&name) {
                changes.push((name, Some(value)));
            }
        }
        for old in previous.iter().filter(|s| s.sensitive) {
            if !claimed.contains(&old.name) && !changes.iter().any(|(name, _)| *name == old.name) {
                changes.push((old.name.clone(), None));
            }
        }
        changes.retain(|(name, value)| value.is_some() || !kept.contains(name));
        let mut transaction = Self {
            store: store.clone(),
            before: Vec::new(),
            committed: false,
        };
        for (name, value) in changes {
            let before = store.get(&name)?;
            transaction.before.push((name.clone(), before));
            if let Some(value) = value {
                store.set(&name, &value).await?;
            } else {
                store.remove(&name)?;
            }
        }
        Ok(transaction)
    }
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for SecretTransaction {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for (name, value) in self.before.iter().rev() {
            let result = match value {
                None => self.store.remove(name),
                Some(bytes) => self.store.path(name).and_then(|path| {
                    let temp = tempfile::NamedTempFile::new_in(&self.store.directory)
                        .map_err(|e| files::io("rollback secret", &path, e))?;
                    fs::write(temp.path(), bytes)
                        .map_err(|e| files::io("rollback secret", &path, e))?;
                    permissions(temp.path(), 0o600)?;
                    temp.persist(&path)
                        .map_err(|e| files::io("rollback secret", &path, e.error))?;
                    Ok(())
                }),
            };
            if let Err(error) = result {
                tracing::warn!(%error,"failed to roll back settings secret");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{home::HomeLayout, settings::SettingsService};

    #[tokio::test]
    async fn interrupted_sign_in_transaction_requires_both_marker_and_secret() {
        for removing in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = HomeLayout::derive(temp.path(), false, true);
            let service = SettingsService::new(home.settings());
            let session = r#"{"accessToken":"token"}"#;
            let signed_in =
                json!({"sourceControlHosts":{"github":{"signIns":{"github.com":session}}}});
            let signed_out = json!({"sourceControlHosts":{"github":{"signIns":{}}}});
            let initial = if removing { &signed_in } else { &signed_out };
            service
                .update(&serde_json::from_value(initial.clone()).unwrap())
                .await
                .unwrap();
            let before: Value =
                serde_json::from_slice(&fs::read(home.settings()).unwrap()).unwrap();
            let store = SecretStore::new(home.secrets());
            let mut next = if removing { signed_out } else { signed_in };
            let transaction = SecretTransaction::apply(&store, &before, &mut next)
                .await
                .unwrap();
            // Process termination skips rollback before settings.json is published.
            std::mem::forget(transaction);
            drop(service);
            let disk: Value = serde_json::from_slice(&fs::read(home.settings()).unwrap()).unwrap();
            let slot = "source-control-Z2l0aHVi-c2lnbklucw-Z2l0aHViLmNvbQ";
            assert_eq!(
                disk["sourceControlHosts"]["github"]["signIns"]
                    .get("github.com")
                    .is_some(),
                removing
            );
            assert_eq!(store.get(slot).unwrap().is_some(), !removing);
            let restarted = SettingsService::new(home.settings());
            let snapshot = restarted.snapshot().await.unwrap();
            assert!(
                snapshot.source_control_hosts["github"]["signIns"]
                    .get("github.com")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            );
        }
    }
}
