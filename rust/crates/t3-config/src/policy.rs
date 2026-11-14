use crate::{Result, files, invalid, runtime::LoadedPolicy};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use t3_contracts::managed_policy::ManagedPolicyDocument;

pub fn managed_policy_path(platform: &str) -> PathBuf {
    match platform {
        "darwin" => "/Library/Application Support/T3Code/policy.json",
        "win32" | "windows" => "C:\\Program Files\\T3Code\\policy.json",
        _ => "/etc/t3code/policy.json",
    }
    .into()
}
pub fn selected_policy_path(
    platform: &str,
    dev_build: bool,
    env: &crate::config::Environment,
) -> PathBuf {
    if dev_build
        && crate::policy_sources::DEV_OVERRIDES
        && let Some(path) = env.get("T3CODE_DEV_MANAGED_POLICY_PATH")
    {
        return path.into();
    }
    managed_policy_path(platform)
}

#[derive(Debug, Clone)]
pub struct ManagedPolicy {
    pub path: PathBuf,
    pub document: Option<ManagedPolicyDocument>,
    source_json: Option<Value>,
}
impl ManagedPolicy {
    pub fn load_file(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let source_json = files::read(&path)?
            .map(|text| serde_json::from_str::<Value>(&text))
            .transpose()?;
        let document = source_json
            .as_ref()
            .map(|value| decode(&path, &value.to_string()))
            .transpose()?;
        Ok(Self {
            path,
            document,
            source_json,
        })
    }
    /// A document a platform source supplied, with the JSON it was decoded from.
    pub(crate) fn from_source(
        path: impl Into<PathBuf>,
        document: ManagedPolicyDocument,
        source_json: Value,
    ) -> Self {
        Self {
            path: path.into(),
            document: Some(document),
            source_json: Some(source_json),
        }
    }
    pub fn loaded_policy(&self) -> Result<Option<LoadedPolicy>> {
        let Some(document) = &self.document else {
            return Ok(None);
        };
        let mut encoded = serde_json::to_value(document)?;
        if let Some(raw) = &self.source_json {
            for pointer in ["/settings", "/limits/providers/instances"] {
                if let Some(Value::Object(map)) = encoded.pointer_mut(pointer) {
                    let mut ordered = serde_json::Map::new();
                    for key in raw
                        .pointer(pointer)
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|m| m.keys())
                    {
                        if let Some(value) = map.shift_remove(key) {
                            ordered.insert(key.clone(), value);
                        }
                    }
                    ordered.append(map);
                    *map = ordered;
                }
            }
        }
        let bytes = serde_json::to_vec(&encoded)?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        Ok(Some(LoadedPolicy {
            source: self.path.display().to_string(),
            organization: document.organization.0.clone(),
            fingerprint: digest[..12].into(),
        }))
    }
}
pub fn decode(path: &Path, raw: &str) -> Result<ManagedPolicyDocument> {
    let value: Value = serde_json::from_str(raw)
        .map(javascript_numbers)
        .map_err(|e| invalid(path.display().to_string(), e.to_string()))?;
    let document: ManagedPolicyDocument = serde_json::from_value(value.clone())
        .map_err(|e| invalid(path.display().to_string(), e.to_string()))?;
    let encoded = serde_json::to_value(&document)?;
    reject_excess(&value, &encoded, "")?;
    if value
        .get("limits")
        .and_then(|v| v.get("clients"))
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        return Err(invalid("managed policy", "clients must be nonempty"));
    }
    if value
        .pointer("/limits/providers/allowedDrivers")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        return Err(invalid("managed policy", "allowedDrivers must be nonempty"));
    }
    if value
        .pointer("/limits/updates/feedUrl")
        .and_then(Value::as_str)
        .is_some_and(|s| {
            !s.to_lowercase().starts_with("https://")
                || s.split_once("://").is_none_or(|(_, host)| {
                    host.is_empty()
                        || host.starts_with('/')
                        || host.chars().any(char::is_whitespace)
                })
        })
    {
        return Err(invalid("managed policy", "feedUrl must be HTTPS"));
    }
    Ok(document)
}
/// A JSON number as `JSON.parse` reads it: there is one number type, so `1.0` is `1`. macOS
/// writes a profile's `<real>` that way, and TypeScript accepts it wherever an integer goes.
fn javascript_numbers(value: Value) -> Value {
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    match value {
        Value::Number(number) if !number.is_i64() && !number.is_u64() => match number.as_f64() {
            Some(float) if float.fract() == 0.0 && float.abs() <= MAX_SAFE_INTEGER => {
                Value::from(float as i64)
            }
            _ => Value::Number(number),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(javascript_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, javascript_numbers(value)))
                .collect(),
        ),
        other => other,
    }
}
fn reject_excess(raw: &Value, encoded: &Value, path: &str) -> Result<()> {
    if path.starts_with("settings.") && path.ends_with(".value") {
        return Ok(());
    }
    if let (Some(raw), Some(encoded)) = (raw.as_object(), encoded.as_object()) {
        for (key, value) in raw {
            let next = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            let expected = encoded
                .get(key)
                .ok_or_else(|| invalid("managed policy", format!("unknown property {next}")))?;
            reject_excess(value, expected, &next)?;
        }
    }
    if let (Some(raw), Some(encoded)) = (raw.as_array(), encoded.as_array()) {
        for (raw, encoded) in raw.iter().zip(encoded) {
            reject_excess(raw, encoded, path)?;
        }
    }
    Ok(())
}
