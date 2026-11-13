use crate::{
    Result, files, invalid,
    watch::{self, WatchHandle},
};
use serde_json::{Map, Value, json};
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{
    provider_instance::ProviderInstanceMutation,
    settings::{ServerSettings, ServerSettingsPatch},
};

pub(crate) fn merge(current: &mut Value, patch: &Value) {
    if let (Some(current), Some(patch)) = (current.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            if let Some(saved) = current.get_mut(key) {
                merge(saved, value);
            } else {
                current.insert(key.clone(), value.clone());
            }
        }
    } else {
        *current = patch.clone();
    }
}
fn entries(current: &Value, patch: &Value) -> Value {
    let mut current = current.as_object().cloned().unwrap_or_default();
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            if value.is_null() {
                current.remove(key);
            } else {
                current.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(current)
}
fn object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}
fn idle_mode(current: &Value) -> &str {
    current
        .get("idleCompaction")
        .and_then(Value::as_str)
        .or_else(|| current.get("compactIdleThreads").and_then(Value::as_str))
        .unwrap_or(if current["compactIdleThreads"] == true {
            "beforeCacheExpires"
        } else {
            "onReturn"
        })
}
fn normalize(value: &mut Value) {
    let mode = idle_mode(value).to_owned();
    value["compactIdleThreads"] = json!(mode == "beforeCacheExpires");
    value["idleCompaction"] = json!(mode);
    if let Some(instances) = value["providerInstances"].as_object_mut() {
        for instance in instances.values_mut() {
            if let Some(enabled) = instance["config"].get("enabled").and_then(Value::as_bool) {
                instance["enabled"] = json!(instance["enabled"] != false && enabled);
                if let Some(config) = instance["config"].as_object_mut() {
                    config.remove("enabled");
                }
            }
        }
    }
    let overrides = value["projectSettingsOverrides"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (legacy, key) in [
        (
            "projectAgentBrowserAccessOverrides",
            "enableAgentBrowserAccess",
        ),
        ("projectAutoPullOverrides", "defaultAutoPull"),
        ("projectScriptOverrides", "defaultProjectScripts"),
    ] {
        value[legacy] = Value::Object(
            overrides
                .iter()
                .filter_map(|(id, entry)| entry.get(key).map(|v| (id.clone(), v.clone())))
                .collect(),
        );
    }
}
pub fn decode_persisted(mut raw: Value) -> Result<ServerSettings> {
    if !raw.is_object() {
        return Err(invalid("settings.json", "expected an object"));
    }
    if raw.get("responseStreamingMode").is_none() || raw["responseStreamingMode"] == "token" {
        raw["responseStreamingMode"] = json!("paragraph");
    }
    if let Some(overrides) = raw
        .get_mut("projectSettingsOverrides")
        .and_then(Value::as_object_mut)
    {
        for entry in overrides.values_mut() {
            if entry["responseStreamingMode"] == "token" {
                entry["responseStreamingMode"] = json!("paragraph");
            }
        }
    }
    let settings: ServerSettings = serde_json::from_value(raw)?;
    let mut value = serde_json::to_value(settings)?;
    normalize(&mut value);
    Ok(serde_json::from_value(value)?)
}

pub fn apply_patch(
    current: &ServerSettings,
    patch: &ServerSettingsPatch,
) -> Result<ServerSettings> {
    let current = serde_json::to_value(current)?;
    let mut patch = serde_json::to_value(patch)?;
    let canonical: Vec<_> = patch["projectSettingsOverrides"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for (legacy, key) in [
        (
            "projectAgentBrowserAccessOverrides",
            "enableAgentBrowserAccess",
        ),
        ("projectAutoPullOverrides", "defaultAutoPull"),
        ("projectScriptOverrides", "defaultProjectScripts"),
    ] {
        if let Some(changes) = object(&mut patch)
            .remove(legacy)
            .and_then(|v| v.as_object().cloned())
        {
            for (id, value) in changes {
                if canonical.contains(&id) {
                    continue;
                }
                let mut entry = patch["projectSettingsOverrides"]
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| current["projectSettingsOverrides"][&id].clone());
                if value.is_null() {
                    object(&mut entry).remove(key);
                } else {
                    object(&mut entry).insert(key.into(), value);
                }
                let entry = if object(&mut entry).is_empty() {
                    Value::Null
                } else {
                    entry
                };
                object(&mut patch["projectSettingsOverrides"]).insert(id, entry);
            }
        }
    }
    let mut next = current.clone();
    merge(&mut next, &patch);
    for key in [
        "usageLimitSources",
        "textGenerators",
        "secretProviders",
        "usagePriceOverrides",
        "usageModelAliases",
    ] {
        if let Some(change) = patch.get(key) {
            next[key] = entries(&current[key], change);
        }
    }
    for key in [
        "providerInstances",
        "defaultModelSelection",
        "fallbackModelSelection",
        "speechPostProcessingModelSelection",
        "sourceControlWriterModelSelection",
        "reviewGuideModelSelection",
        "defaultProjectScripts",
    ] {
        if let Some(change) = patch.get(key) {
            next[key] = change.clone();
        }
    }
    if let Some(changes) = patch["projectSettingsOverrides"].as_object() {
        let mut map = current["projectSettingsOverrides"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (id, entry) in changes {
            if entry.is_null() {
                map.remove(id);
                continue;
            }
            let known: t3_contracts::settings::ProjectSettingsOverrides =
                serde_json::from_value(entry.clone())?;
            let mut accepted = serde_json::to_value(known)?;
            if let Some(stored) = map.get(id) {
                // The contract's flattened extras identify stored fields a downgraded build cannot edit.
                let decoded: t3_contracts::settings::ProjectSettingsOverrides =
                    serde_json::from_value(stored.clone())?;
                let known_stored = serde_json::to_value(decoded)?;
                for (key, value) in object(&mut known_stored.clone()) {
                    if accepted.get(key).is_none() && key == "providerInstances" {
                        accepted[key] = value.clone();
                    }
                }
                let full: t3_contracts::settings::ServerSettingsProjectSettingsOverridesValue =
                    serde_json::from_value(stored.clone())?;
                for (key, value) in full.extra {
                    object(&mut accepted).insert(key, value);
                }
            }
            if accepted.as_object().is_some_and(Map::is_empty) {
                map.remove(id);
            } else {
                map.insert(id.clone(), accepted);
            }
        }
        next["projectSettingsOverrides"] = Value::Object(map);
    }
    if let Some(hosts) = patch["sourceControlHosts"].as_object() {
        next["sourceControlHosts"] = current["sourceControlHosts"].clone();
        for (kind, fields) in hosts {
            let mut host = current["sourceControlHosts"][kind].clone();
            for (key, value) in fields.as_object().into_iter().flatten() {
                object(&mut host).insert(key.clone(), value.clone());
            }
            if fields["tokens"].as_object().is_some_and(|m| !m.is_empty()) {
                let by_host = |v: &Value| {
                    v.as_object()
                        .into_iter()
                        .flatten()
                        .map(|(h, v)| (h.trim().to_lowercase(), v.clone()))
                        .collect::<Map<_, _>>()
                };
                let mut tokens = by_host(&current["sourceControlHosts"][kind]["tokens"]);
                tokens.extend(by_host(&fields["tokens"]));
                host["tokens"] = Value::Object(tokens);
            }
            object(&mut next["sourceControlHosts"]).insert(kind.clone(), host);
        }
    }
    if patch
        .get("worktreesDirectory")
        .is_some_and(|v| *v != current["worktreesDirectory"])
    {
        let mut previous: Vec<_> = current["previousWorktreesDirectories"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|v| *v != patch["worktreesDirectory"])
            .collect();
        if current["worktreesDirectory"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
            && !previous.contains(&current["worktreesDirectory"])
        {
            previous.push(current["worktreesDirectory"].clone());
        }
        next["previousWorktreesDirectories"] = json!(previous);
    }
    let migration = &patch["migrateModelPreferences"];
    let initial_favorites = patch
        .get("favorites")
        .filter(|v| !v.is_null())
        .or_else(|| current.get("favorites").filter(|v| !v.is_null()))
        .or_else(|| migration.get("favorites"))
        .cloned()
        .unwrap_or(Value::Null);
    next["favorites"] = initial_favorites;
    if let Some(changes) = patch["setModelFavorites"].as_array() {
        let mut favorites = next["favorites"].as_array().cloned().unwrap_or_default();
        for change in changes {
            let exists = favorites
                .iter()
                .any(|v| v["provider"] == change["provider"] && v["model"] == change["model"]);
            if change["favorite"] == true && !exists {
                favorites.push(json!({"provider":change["provider"],"model":change["model"]}));
            } else if change["favorite"] == false {
                favorites.retain(|v| {
                    !(v["provider"] == change["provider"] && v["model"] == change["model"])
                });
            }
        }
        next["favorites"] = json!(favorites);
    }
    next["providerModelPreferences"] = patch
        .get("providerModelPreferences")
        .filter(|v| !v.is_null())
        .or_else(|| {
            current
                .get("providerModelPreferences")
                .filter(|v| !v.is_null())
        })
        .or_else(|| migration.get("providerModelPreferences"))
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(changes) = patch["setModelsHidden"].as_array() {
        for change in changes {
            let Some(provider) = change["provider"].as_str() else {
                continue;
            };
            let prefs = object(&mut next["providerModelPreferences"])
                .entry(provider)
                .or_insert_with(|| json!({"hiddenModels":[],"modelOrder":[]}));
            let mut hidden = prefs["hiddenModels"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            hidden.retain(|v| *v != change["model"]);
            if change["hidden"] == true {
                hidden.push(change["model"].clone());
            }
            prefs["hiddenModels"] = json!(hidden);
        }
    }
    if let Some(change) = patch.get("setProviderModelOrder")
        && let Some(provider) = change["provider"].as_str()
    {
        let prefs = object(&mut next["providerModelPreferences"])
            .entry(provider)
            .or_insert_with(|| json!({"hiddenModels":[],"modelOrder":[]}));
        prefs["modelOrder"] = change["modelOrder"].clone();
    }
    if let Some(selection) = patch.get("textGenerationModelSelection") {
        if selection.get("instanceId").is_some() || selection.get("model").is_some() {
            object(&mut next["textGenerationModelSelection"]).remove("options");
            if let Some(options) = selection.get("options")
                && options.as_array().is_some_and(|a| !a.is_empty())
            {
                next["textGenerationModelSelection"]["options"] = options.clone();
            }
        } else if let Some(options) = selection["options"].as_array() {
            let mut merged = current["textGenerationModelSelection"]["options"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for option in options {
                if let Some(stored) = merged.iter_mut().find(|v| v["id"] == option["id"]) {
                    *stored = option.clone();
                } else {
                    merged.push(option.clone());
                }
            }
            if options.is_empty() {
                object(&mut next["textGenerationModelSelection"]).remove("options");
            } else {
                next["textGenerationModelSelection"]["options"] = json!(merged);
            }
        }
    }
    let mode = patch
        .get("idleCompaction")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| match patch.get("compactIdleThreads") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Bool(true)) => "beforeCacheExpires".into(),
            Some(Value::Bool(false)) if idle_mode(&current) != "off" => "onReturn".into(),
            _ => idle_mode(&current).into(),
        });
    next["idleCompaction"] = json!(mode);
    update_background(&current, &patch, &mut next);
    if patch["worktreeCleanup"]["mode"] == "custom" {
        let mut rules = json!({});
        for key in [
            "worktreeKeepWhen",
            "worktreeAfterDays",
            "worktreeSettledAfterDays",
            "worktreeOnMerge",
            "worktreeOnDelete",
            "worktreeUnchanged",
        ] {
            rules[key] = next["storageCleanup"][key].clone();
        }
        if current["worktreeCleanup"]["mode"] == "custom" {
            merge(&mut rules, &current["worktreeCleanup"]["rules"]);
        }
        merge(&mut rules, &patch["worktreeCleanup"]["rules"]);
        next["worktreeCleanup"]["rules"] = rules;
    }
    normalize(&mut next);
    Ok(serde_json::from_value(next)?)
}

fn preset(profile: &str) -> Value {
    let (git, health, active, idle, battery, low) = match profile {
        "performance" => (15000, 60000, 30000, 120000, false, false),
        "battery-saver" => (0, 900000, 60000, 600000, true, true),
        _ => (30000, 300000, 30000, 300000, false, true),
    };
    json!({"automaticGitFetchInterval":git,"providerHealthRefreshInterval":health,"hostPowerMonitorActiveInterval":active,"hostPowerMonitorIdleInterval":idle,"pauseWhenHostLocked":true,"pauseWhenHostLowPower":low,"pauseWhenClientLowPower":low,"pauseWhenOnBattery":battery})
}
fn update_background(current: &Value, patch: &Value, next: &mut Value) {
    let mut background = current["backgroundActivity"].clone();
    let legacy_profile = current["backgroundActivityProfile"]
        .as_str()
        .unwrap_or("balanced");
    if background["profile"] == "balanced"
        && background["overrides"]
            .as_object()
            .is_some_and(Map::is_empty)
        && (legacy_profile != "balanced"
            || current["automaticGitFetchInterval"] != 30000
            || current["providerHealthRefreshInterval"] != 300000)
    {
        background = json!({"schemaVersion":1,"profile":"custom","baseProfile":legacy_profile,"overrides":{"automaticGitFetchInterval":current["automaticGitFetchInterval"],"providerHealthRefreshInterval":current["providerHealthRefreshInterval"]}});
    }
    if let Some(change) = patch.get("backgroundActivity") {
        merge(&mut background, change);
        if let Some(overrides) = change.get("overrides") {
            background["overrides"] = overrides.clone();
        }
    } else if patch.get("backgroundActivityProfile").is_some()
        || patch.get("automaticGitFetchInterval").is_some()
        || patch.get("providerHealthRefreshInterval").is_some()
    {
        let base = patch["backgroundActivityProfile"]
            .as_str()
            .unwrap_or_else(|| {
                if background["profile"] == "custom" {
                    background["baseProfile"].as_str().unwrap_or("balanced")
                } else {
                    background["profile"].as_str().unwrap_or("balanced")
                }
            })
            .to_owned();
        let mut overrides = if patch.get("backgroundActivityProfile").is_none()
            && background["profile"] == "custom"
        {
            background["overrides"].clone()
        } else {
            json!({})
        };
        for key in ["automaticGitFetchInterval", "providerHealthRefreshInterval"] {
            if let Some(value) = patch.get(key) {
                overrides[key] = value.clone();
            }
        }
        background = json!({"schemaVersion":1,"profile":if overrides.as_object().is_some_and(Map::is_empty) { base.as_str() } else { "custom" },"baseProfile":base,"overrides":overrides});
    }
    let base = if background["profile"] == "custom" {
        background["baseProfile"].as_str().unwrap_or("balanced")
    } else {
        background["profile"].as_str().unwrap_or("balanced")
    }
    .to_owned();
    let mut resolved = preset(&base);
    if background["profile"] == "custom" {
        merge(&mut resolved, &background["overrides"]);
    }
    let mut matched = false;
    for profile in [base.as_str(), "balanced", "performance", "battery-saver"] {
        if resolved == preset(profile) {
            background = json!({"schemaVersion":1,"profile":profile,"overrides":{}});
            matched = true;
            break;
        }
    }
    if !matched {
        let defaults = preset(&base);
        let overrides: Map<_, _> = resolved
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, v)| defaults.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        background =
            json!({"schemaVersion":1,"profile":"custom","baseProfile":base,"overrides":overrides});
    }
    next["automaticGitFetchInterval"] = resolved["automaticGitFetchInterval"].clone();
    next["providerHealthRefreshInterval"] = resolved["providerHealthRefreshInterval"].clone();
    next["backgroundActivityProfile"] = json!(base);
    next["backgroundActivity"] = background;
}

fn strip_defaults(current: &Value, defaults: &Value) -> Option<Value> {
    if current == defaults {
        return None;
    }
    if let (Some(c), Some(d)) = (current.as_object(), defaults.as_object()) {
        let mut result = Map::new();
        for (key, value) in c {
            let atomic = matches!(
                key.as_str(),
                "backgroundActivity"
                    | "automaticGitFetchInterval"
                    | "providerHealthRefreshInterval"
                    | "sourceControlWriterModelSelection"
                    | "reviewGuideModelSelection"
                    | "textGenerationModelSelection"
                    | "fallbackModelSelection"
                    | "pullRequestMergeMethod"
            ) || d.get(key).is_some_and(Value::is_null);
            let stripped = if atomic {
                (d.get(key) != Some(value)).then(|| value.clone())
            } else {
                strip_defaults(value, d.get(key).unwrap_or(&Value::Null))
            };
            if let Some(value) = stripped {
                result.insert(key.clone(), value);
            }
        }
        return (!result.is_empty()).then_some(Value::Object(result));
    }
    Some(current.clone())
}

#[derive(Clone)]
pub struct SettingsState {
    pub settings: ServerSettings,
    pub parse_failure: Option<String>,
    stored: ServerSettings,
}
impl std::fmt::Debug for SettingsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsState").finish_non_exhaustive()
    }
}

pub struct SettingsService {
    path: PathBuf,
    write_lock: tokio::sync::Mutex<()>,
    cache: tokio::sync::OnceCell<tokio::sync::watch::Sender<SettingsState>>,
    changes: crate::events::Changes<ServerSettings>,
    secrets: crate::secrets::SecretStore,
    database: Option<t3_persistence::Database>,
}
impl SettingsService {
    pub fn new(path: impl Into<PathBuf>) -> Arc<Self> {
        Self::with_database(path, None)
    }
    pub fn with_database(
        path: impl Into<PathBuf>,
        database: Option<t3_persistence::Database>,
    ) -> Arc<Self> {
        let path = path.into();
        let secrets = crate::secrets::SecretStore::new(path.with_file_name("secrets"));
        Arc::new(Self {
            path,
            write_lock: tokio::sync::Mutex::new(()),
            cache: tokio::sync::OnceCell::new(),
            changes: crate::events::Changes::new(),
            secrets,
            database,
        })
    }
    pub fn subscribe_changes(&self) -> tokio::sync::mpsc::UnboundedReceiver<ServerSettings> {
        self.changes.subscribe()
    }
    async fn load(&self) -> Result<SettingsState> {
        let raw = files::read(&self.path)?;
        let parsed = raw.as_deref().map(files::lenient_json).transpose();
        let parsed_value = parsed
            .as_ref()
            .ok()
            .and_then(|v| v.clone())
            .unwrap_or_else(|| json!({}));
        let decoded = parsed.and_then(|v| {
            let value = v.unwrap_or_else(|| json!({}));
            for key in ["providers", "bitbucket", "github"] {
                if value.get(key).is_some_and(|v| !v.is_object()) {
                    return Err(invalid("settings.json", format!("{key} must be an object")));
                }
            }
            decode_persisted(value)
        });
        let (settings, parse_failure) = match decoded {
            Ok(settings) => (settings, None),
            Err(error) => {
                tracing::warn!(%error,"failed to parse settings.json, using defaults");
                let mut value = serde_json::to_value(ServerSettings::default())?;
                value["continueThreadsAfterServerUpdate"] = json!(false);
                (decode_persisted(value)?, Some(error.to_string()))
            }
        };
        let mut value = serde_json::to_value(settings)?;
        let changed = crate::settings_migrations::migrate(
            &mut value,
            &parsed_value,
            self.database.as_ref(),
            &self.secrets,
            parse_failure.is_none(),
        )
        .await?;
        normalize(&mut value);
        let secret_changed = if parse_failure.is_none() {
            crate::secrets::migrate_inline(&self.secrets, &mut value).await
        } else {
            false
        };
        if changed || secret_changed {
            self.write(&value).await?;
            crate::settings_migrations::cleanup_legacy_secrets(
                &self.secrets,
                &parsed_value,
                &value,
            );
        }
        let stored = serde_json::from_value(value.clone())?;
        let settings = self.effective(value)?;
        Ok(SettingsState {
            settings,
            parse_failure,
            stored,
        })
    }
    fn effective(&self, value: Value) -> Result<ServerSettings> {
        let mut value = crate::secrets::materialize(&self.secrets, value)?;
        crate::settings_migrations::resolve_text_generation(&mut value);
        Ok(serde_json::from_value(value)?)
    }
    async fn write(&self, value: &Value) -> Result<()> {
        let defaults = serde_json::to_value(ServerSettings::default())?;
        let sparse = strip_defaults(value, &defaults).unwrap_or_else(|| json!({}));
        files::write_json(&self.path, &sparse, true).await
    }
    async fn cache(&self) -> Result<&tokio::sync::watch::Sender<SettingsState>> {
        self.cache
            .get_or_try_init(async || Ok(tokio::sync::watch::channel(self.load().await?).0))
            .await
    }
    pub async fn snapshot(&self) -> Result<ServerSettings> {
        Ok(self.cache().await?.borrow().settings.clone())
    }
    pub async fn reload(&self) -> Result<SettingsState> {
        let _write = self.write_lock.lock().await;
        let cache = self.cache().await?;
        let next = self.load().await?;
        cache.send_replace(next.clone());
        self.changes.send(next.settings.clone());
        Ok(next)
    }
    pub async fn start(self: &Arc<Self>) -> Result<WatchHandle> {
        let weak = Arc::downgrade(self);
        let handle = watch::watch(&self.path, true, move || {
            let weak = weak.clone();
            async move {
                if let Some(service) = weak.upgrade() {
                    service.reload().await?;
                }
                Ok(())
            }
        })?;
        self.reload().await?;
        Ok(handle)
    }
    pub async fn with_snapshot<T>(
        &self,
        use_snapshot: impl AsyncFnOnce(&ServerSettings) -> Result<T>,
    ) -> Result<T> {
        let _write = self.write_lock.lock().await;
        let snapshot = self.cache().await?.borrow().settings.clone();
        use_snapshot(&snapshot).await
    }
    pub async fn update(&self, patch: &ServerSettingsPatch) -> Result<ServerSettings> {
        self.update_inner(patch, None).await
    }
    pub async fn update_provider_instance(
        &self,
        mutation: &ProviderInstanceMutation,
        patch: &ServerSettingsPatch,
    ) -> Result<ServerSettings> {
        self.update_inner(patch, Some(mutation)).await
    }
    async fn update_inner(
        &self,
        patch: &ServerSettingsPatch,
        mutation: Option<&ProviderInstanceMutation>,
    ) -> Result<ServerSettings> {
        let _write = self.write_lock.lock().await;
        let cache = self.cache().await?;
        let current = self.load().await?;
        if let Some(error) = current.parse_failure {
            return Err(invalid("read-file", error));
        }
        let patch_value = serde_json::to_value(patch)?;
        for (id, entry) in patch_value["secretProviders"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if entry.is_null() {
                continue;
            }
            let valid = entry["kind"] == "one_password"
                && entry["config"]["account"].as_str().is_some_and(|s| {
                    !s.is_empty()
                        && s.len() <= 253
                        && s.as_bytes()[0].is_ascii_alphanumeric()
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
                });
            if !valid {
                return Err(invalid(
                    "write-secret-provider",
                    format!("Unsupported or invalid secret provider {id}"),
                ));
            }
        }
        let next = apply_patch(&current.stored, patch)?;
        let mut value = serde_json::to_value(next)?;
        if let Some(mutation) = mutation {
            match mutation {
                ProviderInstanceMutation::Create {
                    instance_id,
                    instance,
                } => {
                    if value["providerInstances"].get(&instance_id.0).is_some() {
                        return Err(invalid("create-provider-instance", &instance_id.0));
                    }
                    value["providerInstances"][&instance_id.0] = serde_json::to_value(instance)?;
                }
                ProviderInstanceMutation::Upsert {
                    instance_id,
                    instance,
                } => value["providerInstances"][&instance_id.0] = serde_json::to_value(instance)?,
                ProviderInstanceMutation::Remove { instance_id } => {
                    object(&mut value["providerInstances"]).remove(&instance_id.0);
                }
            }
        }
        normalize(&mut value);
        let before = serde_json::to_value(&current.stored)?;
        crate::secrets::restore_echoed_references(&before, &mut value);
        crate::secrets::remove_provider_references(&before, &mut value);
        let transaction =
            crate::secrets::SecretTransaction::apply(&self.secrets, &before, &mut value).await?;
        let stored = serde_json::from_value(value.clone())?;
        let settings = self.effective(value.clone())?;
        self.write(&value).await?;
        transaction.commit();
        cache.send_replace(SettingsState {
            settings: settings.clone(),
            parse_failure: None,
            stored,
        });
        self.changes.send(settings.clone());
        Ok(settings)
    }
}
