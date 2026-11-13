use crate::{Result, secrets::SecretStore};
use serde_json::{Value, json};

pub(crate) async fn migrate(
    value: &mut Value,
    raw: &Value,
    db: Option<&t3_persistence::Database>,
    store: &SecretStore,
    trusted: bool,
) -> Result<bool> {
    let mut changed = false;
    if let Some(legacy) = raw["providers"].as_object() {
        let history = if let Some(db) = db {
            db.reader(|connection| {
                let mut stmt = connection.prepare("SELECT DISTINCT provider_name, provider_instance_id FROM projection_thread_sessions WHERE provider_name IN ('cursor', 'grok', 'opencode') UNION SELECT DISTINCT provider_name, provider_instance_id FROM provider_session_runtime WHERE provider_name IN ('cursor', 'grok', 'opencode')")?;
                let rows = stmt.query_map([],|r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?)))?;
                Ok(rows.collect::<std::result::Result<Vec<_>,_>>()?)
            }).await?
        } else {
            Vec::new()
        };
        let mut legacy = legacy.clone();
        for driver in ["cursor", "grok", "opencode"] {
            legacy.entry(driver).or_insert_with(|| json!({}));
        }
        for (driver, blob) in legacy {
            let valid = !driver.is_empty()
                && driver.len() <= 64
                && driver.as_bytes()[0].is_ascii_alphabetic()
                && driver
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'));
            if !valid || !blob.is_object() {
                continue;
            }
            let restored = matches!(driver.as_str(), "cursor" | "grok" | "opencode");
            if value["providerInstances"].get(&driver).is_some() {
                continue;
            }
            let mut config = blob.as_object().cloned().unwrap_or_default();
            let explicit = config.remove("enabled").and_then(|v| v.as_bool());
            let enabled = explicit.or_else(|| {
                (restored && history.iter().any(|(name, _)| *name == driver)).then_some(true)
            });
            let default = !matches!(
                driver.as_str(),
                "cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity"
            );
            if config.is_empty() && enabled.is_none_or(|v| v == default) {
                continue;
            }
            let mut instance = json!({"driver":driver,"config":config});
            if let Some(enabled) = enabled {
                instance["enabled"] = json!(enabled);
            }
            value["providerInstances"][&driver] = instance;
        }
        for (id, instance) in value["providerInstances"]
            .as_object_mut()
            .into_iter()
            .flat_map(|m| m.iter_mut())
        {
            if instance.get("enabled").is_none()
                && matches!(
                    instance["driver"].as_str(),
                    Some("cursor" | "grok" | "opencode")
                )
                && history
                    .iter()
                    .any(|(name, instance_id)| instance_id.as_deref().unwrap_or(name) == id)
            {
                instance["enabled"] = json!(true);
            }
        }
        changed = trusted;
    }
    if !trusted {
        return Ok(false);
    }
    for kind in ["bitbucket", "github"] {
        let Some(legacy) = raw.get(kind).filter(|v| v.is_object()) else {
            continue;
        };
        if value["sourceControlHosts"].get(kind).is_some() {
            changed = true;
            continue;
        }
        let mut copied = true;
        if kind == "bitbucket" {
            for (field, name) in [
                ("accessToken", "bitbucket-access-token"),
                ("apiToken", "bitbucket-api-token"),
            ] {
                if legacy[field] == crate::secrets::REDACTED {
                    copied &= copy_secret(store, name, &source_name(kind, field, None)).await;
                }
            }
        } else {
            for (host, token) in legacy["tokens"].as_object().into_iter().flatten() {
                if token == crate::secrets::REDACTED {
                    use base64::Engine;
                    let host = host.trim().to_lowercase();
                    let old = format!(
                        "github-token-{}",
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(host.as_bytes())
                    );
                    copied &=
                        copy_secret(store, &old, &source_name(kind, "tokens", Some(&host))).await;
                }
            }
        }
        if copied {
            value["sourceControlHosts"][kind] = legacy.clone();
            changed = true;
        }
    }
    if value["projectSettingsFolded"] != true {
        let mut entries = value["projectSettingsOverrides"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut any = false;
        for (legacy, key) in [
            (
                "projectAgentBrowserAccessOverrides",
                "enableAgentBrowserAccess",
            ),
            ("projectAutoPullOverrides", "defaultAutoPull"),
            ("projectScriptOverrides", "defaultProjectScripts"),
        ] {
            for (id, value) in raw[legacy].as_object().into_iter().flatten() {
                any = true;
                if value.is_null() {
                    continue;
                }
                let entry = entries.entry(id).or_insert_with(|| json!({}));
                if entry.get(key).is_none() {
                    entry[key] = value.clone();
                }
            }
        }
        if let Some(db) = db {
            let projects = db.reader(|connection| {
                let mut stmt = connection.prepare("SELECT project_id, default_model_selection_json, default_thread_env_mode, auto_pull, scripts_json FROM projection_projects WHERE deleted_at IS NULL")?;
                let rows = stmt.query_map([],|r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?)))?;
                Ok(rows.collect::<std::result::Result<Vec<_>,_>>()?)
            }).await?;
            any |= !projects.is_empty();
            for (id, model, mode, auto_pull, scripts) in projects {
                let entry = entries.entry(&id).or_insert_with(|| json!({}));
                let mut set = |key: &str, value: Value| {
                    if entry.get(key).is_none() {
                        entry[key] = value;
                    }
                };
                if let Some(model) = model.and_then(|s| {
                    serde_json::from_str::<t3_contracts::model_selection::ModelSelection>(&s).ok()
                }) {
                    set("defaultModelSelection", serde_json::to_value(model)?);
                }
                if let Some(mode) = mode.filter(|s| matches!(s.as_str(), "local" | "worktree")) {
                    set("defaultThreadEnvMode", json!(mode));
                }
                if auto_pull == 1 {
                    set("defaultAutoPull", json!(true));
                }
                let reset = raw["projectScriptOverrides"]
                    .get(&id)
                    .is_some_and(Value::is_null);
                if let Ok(scripts) =
                    serde_json::from_str::<Vec<t3_contracts::project::ProjectScript>>(&scripts)
                    && !scripts.is_empty()
                    && !reset
                {
                    set("defaultProjectScripts", serde_json::to_value(scripts)?);
                }
            }
        }
        if any {
            entries.retain(|_, v| v.as_object().is_some_and(|m| !m.is_empty()));
            value["projectSettingsOverrides"] = Value::Object(entries);
            value["projectSettingsFolded"] = json!(true);
            changed = true;
        }
    }
    Ok(changed)
}
pub(crate) fn cleanup_legacy_secrets(store: &SecretStore, raw: &Value, next: &Value) {
    for kind in ["bitbucket", "github"] {
        if raw["sourceControlHosts"].get(kind).is_some()
            || next["sourceControlHosts"].get(kind).is_none()
        {
            continue;
        }
        if kind == "bitbucket" {
            for (field, name) in [
                ("accessToken", "bitbucket-access-token"),
                ("apiToken", "bitbucket-api-token"),
            ] {
                if raw[kind][field] == crate::secrets::REDACTED {
                    let _ = store.remove(name);
                }
            }
        } else {
            use base64::Engine;
            for (host, value) in raw[kind]["tokens"].as_object().into_iter().flatten() {
                if value == crate::secrets::REDACTED {
                    let name = format!(
                        "github-token-{}",
                        base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(host.trim().to_lowercase())
                    );
                    let _ = store.remove(&name);
                }
            }
        }
    }
}
fn source_name(kind: &str, field: &str, host: Option<&str>) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    format!(
        "source-control-{}-{}{}",
        B64.encode(kind),
        B64.encode(field),
        host.map(|h| format!("-{}", B64.encode(h)))
            .unwrap_or_default()
    )
}
async fn copy_secret(store: &SecretStore, old: &str, new: &str) -> bool {
    match store.get(old) {
        Ok(Some(bytes)) => store.set(new, &bytes).await.is_ok(),
        Ok(None) => true,
        Err(_) => false,
    }
}

pub(crate) fn resolve_text_generation(value: &mut Value) {
    let usable = |id: &str| {
        if let Some(generator) = value["textGenerators"].get(id) {
            return generator["enabled"] == true;
        }
        match value["providerInstances"].get(id) {
            Some(instance) => {
                instance["driver"] != "acpRegistry"
                    && instance["enabled"].as_bool().unwrap_or(!matches!(
                        instance["driver"].as_str(),
                        Some(
                            "cursor"
                                | "grok"
                                | "fx"
                                | "kiro"
                                | "muse"
                                | "pi"
                                | "opencode"
                                | "antigravity"
                        )
                    ))
            }
            None => matches!(id, "codex" | "claudeAgent"),
        }
    };
    if value["textGenerationModelSelection"]["instanceId"]
        .as_str()
        .is_some_and(usable)
    {
        return;
    }
    let defaults = [
        "codex",
        "claudeAgent",
        "cursor",
        "grok",
        "muse",
        "pi",
        "opencode",
        "antigravity",
    ];
    let candidates: Vec<_> = defaults
        .iter()
        .map(|d| ((*d).to_owned(), (*d).to_owned()))
        .chain(
            value["providerInstances"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(id, i)| {
                    i["driver"]
                        .as_str()
                        .filter(|d| defaults.contains(d))
                        .map(|d| (id.clone(), d.to_owned()))
                }),
        )
        .collect();
    for (id, driver) in candidates {
        if !usable(&id) {
            continue;
        }
        let thread_default = &value["defaultModelSelection"];
        let custom = value["providerInstances"][&id]["config"]["customModels"]
            .as_array()
            .is_some_and(|a| {
                a.iter()
                    .any(|m| m == &thread_default["model"] || m["slug"] == thread_default["model"])
            });
        if thread_default["instanceId"] == id && custom {
            value["textGenerationModelSelection"] = thread_default.clone();
            return;
        }
        let model = t3_contracts::model::DEFAULT_TEXT_GENERATION_MODEL_BY_PROVIDER
            .iter()
            .chain(t3_contracts::model::DEFAULT_MODEL_BY_PROVIDER)
            .find(|(d, _)| *d == driver)
            .map(|(_, m)| *m)
            .unwrap_or(t3_contracts::model::DEFAULT_TEXT_GENERATION_MODEL);
        value["textGenerationModelSelection"] = json!({"instanceId":id,"model":model});
        return;
    }
}
