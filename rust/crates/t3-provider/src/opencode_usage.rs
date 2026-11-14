use crate::{
    environment::Environment,
    opencode_http::OpenCodeHttp,
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport},
    usage::clamp_percent,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};
use t3_contracts::{provider_usage_limits::ServerProviderUsageLimits, wire::JsNumber};

pub fn unavailable(
    checked: &str,
    reason: &str,
    message: Option<&str>,
) -> ServerProviderUsageLimits {
    let mut value = json!({"checkedAt":checked,"windows":[],"unavailable":{"reason":reason}});
    if let Some(message) = message {
        value["unavailable"]["message"] = json!(message);
    }
    serde_json::from_value(value).expect("fixed usage unavailable shape")
}
fn reset(value: &JsNumber) -> ServiceResult<String> {
    let ms = value.value();
    if ms.abs() > 8_640_000_000_000_000. {
        return Err(ServiceError::Response);
    }
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(ServiceError::Response)
}
fn finish(checked: &str, mut windows: Vec<Value>) -> ServiceResult<ServerProviderUsageLimits> {
    let rank = |value: &Value| match value["kind"].as_str() {
        Some("session") => 0,
        Some("weekly") => 1,
        Some("monthly") => 2,
        _ => 3,
    };
    windows.sort_by(|left, right| {
        rank(left)
            .cmp(&rank(right))
            .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
    });
    serde_json::from_value(json!({"checkedAt":checked,"windows":windows})).map_err(
        crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meter {
    percent: JsNumber,
    limited: bool,
    resets_at: JsNumber,
}
#[derive(Deserialize)]
struct Go {
    rolling: Meter,
    weekly: Meter,
    monthly: Meter,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BudgetModel {
    #[serde(rename = "providerID")]
    provider_id: String,
    #[serde(rename = "modelID")]
    model_id: String,
    name: String,
    #[serde(default, with = "t3_contracts::wire::optional")]
    spent: Option<JsNumber>,
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Scope {
    Each,
    Combined,
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Per {
    Set,
    Model,
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Coverage {
    All,
    Models,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Budget {
    id: String,
    limit: JsNumber,
    spent: JsNumber,
    scope: Scope,
    per: Per,
    coverage: Coverage,
    models: Vec<BudgetModel>,
    shared: bool,
    resets_at: JsNumber,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Workspace {
    id: String,
    #[serde(default, with = "t3_contracts::wire::optional")]
    name: Option<String>,
    #[serde(rename = "usageURL")]
    usage_url: String,
}
#[derive(Deserialize)]
struct Data {
    workspace: Workspace,
    #[serde(default, with = "t3_contracts::wire::optional")]
    go: Option<Go>,
    budgets: Vec<Budget>,
}

pub async fn server(
    connection: &OpenCodeHttp,
    directory: &str,
    checked: &str,
) -> Option<ServerProviderUsageLimits> {
    let work = async {
        let response = connection
            .raw(
                "/api/experimental/usage/limits",
                Some(directory),
                2,
                Duration::from_secs(5),
            )
            .await
            .map_err(ServiceError::from)?;
        if matches!(response.status, 404 | 405)
            || (response.status == 200
                && response
                    .content_type
                    .as_deref()
                    .and_then(|value| value.split(';').next())
                    .is_some_and(|value| value.trim() == "text/html"))
        {
            return Ok(None);
        }
        if !(200..300).contains(&response.status) {
            return Err(ServiceError::Response);
        }
        let value: Value = serde_json::from_slice(response.body.expose()).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
        )?;
        if !value["location"]["directory"].is_string() {
            return Err(ServiceError::Response);
        }
        let value = value.get("data").ok_or(ServiceError::Response)?;
        if value.is_null() {
            return Ok(Some(unavailable(
                checked,
                "unsupported",
                Some("No OpenCode Console workspace is signed in."),
            )));
        }
        let data: Data = serde_json::from_value(value.clone()).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
        )?;
        let mut windows = vec![];
        if let Some(go) = data.go {
            for (key, kind, label, duration, meter) in [
                ("rolling", "session", "Session", Some(300_u64), go.rolling),
                ("weekly", "weekly", "Weekly", Some(10080), go.weekly),
                ("monthly", "monthly", "Monthly", None, go.monthly),
            ] {
                let mut window = json!({"id":format!("go_{key}"),"kind":kind,"label":format!("Go · {label}"),"usedPercent":if meter.limited{100.}else{clamp_percent(meter.percent.value())},"resetsAt":reset(&meter.resets_at)?});
                if let Some(duration) = duration {
                    window["windowDurationMins"] = json!(duration);
                }
                windows.push(window);
            }
        }
        let providers = if data
            .budgets
            .iter()
            .any(|budget| budget.coverage == Coverage::All && budget.per == Per::Set)
        {
            connection
                .get("/api/provider", Some(directory), 2, Duration::from_secs(3))
                .await
                .ok()
                .and_then(|value| value["data"].as_array().cloned())
                .filter(|providers| {
                    providers.iter().all(|provider| {
                        provider["id"].is_string()
                            && provider.get("integrationID").is_none_or(Value::is_string)
                    })
                })
                .unwrap_or_default()
                .into_iter()
                .filter(|provider| {
                    provider["integrationID"] == "opencode" && provider["id"] != "opencode-go"
                })
                .filter_map(|provider| provider["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        for budget in data.budgets {
            let scope = if budget.shared || budget.scope == Scope::Combined {
                "Shared"
            } else {
                "Individual"
            };
            let percent = |spent: f64| {
                if budget.limit.value() <= 0. {
                    100.
                } else {
                    clamp_percent(spent / budget.limit.value() * 100.)
                }
            };
            let reset = reset(&budget.resets_at)?;
            if budget.per == Per::Model
                && budget.coverage == Coverage::Models
                && !budget.models.is_empty()
                && budget.models.iter().all(|model| model.spent.is_some())
            {
                for model in &budget.models {
                    windows.push(json!({"id":format!("budget_{}_{}_{}",budget.id,model.provider_id,model.model_id),"kind":"other","label":format!("Console · {scope} · {}",model.name),"modelIds":[format!("{}/{}",model.provider_id,model.model_id)],"usedPercent":percent(model.spent.as_ref().map_or(budget.spent.value(),JsNumber::value)),"resetsAt":reset}));
                }
            } else {
                let covered = if budget.coverage == Coverage::All {
                    "All spend".into()
                } else {
                    let names = budget
                        .models
                        .iter()
                        .map(|model| model.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    if names.is_empty() {
                        "Models".into()
                    } else {
                        names
                    }
                };
                let mut window = json!({"id":format!("budget_{}",budget.id),"kind":"other","label":format!("Console · {scope} · {covered}{}",if budget.per==Per::Model{" · Most used model"}else{""}),"usedPercent":percent(budget.spent.value()),"resetsAt":reset});
                if budget.per == Per::Model {
                    window["modelIds"] = json!([]);
                } else if budget.coverage == Coverage::Models {
                    window["modelIds"] = json!(
                        budget
                            .models
                            .iter()
                            .map(|model| format!("{}/{}", model.provider_id, model.model_id))
                            .collect::<Vec<_>>()
                    );
                } else {
                    window["providerIds"] = json!(providers);
                }
                windows.push(window);
            }
        }
        let mut limits = serde_json::to_value(finish(checked, windows)?).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
        )?;
        if let Ok(url) = url::Url::parse(&data.workspace.usage_url)
            && matches!(url.scheme(), "http" | "https")
        {
            limits["externalUsage"] = json!({"label":data.workspace.name.as_deref().map(str::trim).filter(|name|!name.is_empty()).unwrap_or(&data.workspace.id),"url":url.as_str()});
        }
        serde_json::from_value(limits)
            .map(Some)
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))
    };
    match tokio::time::timeout(Duration::from_secs(5), work).await {
        Ok(Ok(limits)) => limits,
        _ => Some(unavailable(
            checked,
            "probeFailed",
            Some("OpenCode could not read workspace usage."),
        )),
    }
}

pub fn credential_paths(environment: &Environment, home: &Path) -> (PathBuf, PathBuf) {
    let data = environment
        .get("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            environment
                .get("HOME")
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    environment
                        .get("USERPROFILE")
                        .filter(|value| !value.is_empty())
                })
                .map(PathBuf::from)
                .unwrap_or_else(|| home.into())
                .join(".local/share")
        });
    let root = data.join("opencode");
    let database = environment
        .get("OPENCODE_DB")
        .map_or("opencode.db", String::as_str);
    (root.join("auth.json"), root.join(database))
}
async fn stored_key(path: PathBuf, integration: &'static str) -> Option<SecretString> {
    tokio::task::spawn_blocking(move||{
        use t3_persistence::rusqlite::{Connection,OpenFlags,OptionalExtension};
        let connection=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
        connection.busy_timeout(Duration::from_millis(100)).ok()?;
        let text:Option<String>=connection.query_row("SELECT value FROM credential WHERE integration_id = ? AND active IS NOT 0 ORDER BY active DESC, time_created DESC, id DESC LIMIT 1",[integration],|row|row.get(0)).optional().ok()?;
        let value:Value=serde_json::from_str(text.as_deref()?).ok()?;
        if value["type"]!="key"||!value["key"].is_string()||value.get("metadata").is_some_and(|metadata|!metadata.is_null()&&!metadata.is_object()){return None;}
        if let Some(server)=value["metadata"]["server"].as_str(){let url=url::Url::parse(server).ok()?;if format!("{}{}",url.origin().ascii_serialization(),url.path().trim_end_matches('/'))!="https://opencode.ai/console"{return None;}}
        Some(Sensitive::new(value["key"].as_str()?.trim().to_owned()))
    }).await.ok().flatten()
}
pub async fn keys(environment: &Environment, home: &Path) -> ServiceResult<Vec<SecretString>> {
    let (auth, database) = credential_paths(environment, home);
    let inline = environment
        .get("OPENCODE_AUTH_CONTENT")
        .filter(|value| !value.is_empty());
    let contents = if let Some(inline) = inline {
        inline.clone()
    } else {
        match tokio::fs::read_to_string(auth).await {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}".into(),
            Err(_) => return Err(ServiceError::Storage),
        }
    };
    let value: Value = serde_json::from_str(&contents).map_err(crate::diagnostics::mapped(
        concat!(module_path!(), "::error"),
        ServiceError::Response,
    ))?;
    if !value.is_object() {
        return Err(ServiceError::Response);
    }
    let key = &value["opencode-go"];
    let go_key = if key["type"] == "api" && key["key"].is_string() {
        key["key"].as_str()
    } else {
        environment.get("OPENCODE_API_KEY").map(String::as_str)
    };
    let mut keys = vec![];
    if inline.is_none()
        && environment
            .get("OPENCODE_DB")
            .is_none_or(|path| path != ":memory:")
    {
        if let Some(key) = stored_key(database.clone(), "opencode").await {
            keys.push(key);
        }
        if let Some(key) = stored_key(database, "opencode-go").await {
            keys.push(key);
        }
    }
    if let Some(key) = go_key {
        keys.push(Sensitive::new(key.trim().into()));
    }
    let mut seen = BTreeSet::new();
    keys.retain(|key| !key.expose().is_empty() && seen.insert(key.expose().clone()));
    Ok(keys)
}
pub async fn legacy(
    http: &dyn HttpTransport,
    environment: &Environment,
    home: &Path,
    enabled: bool,
    external: bool,
    checked: &str,
) -> ServerProviderUsageLimits {
    if !enabled || external {
        return unavailable(checked, "unsupported", None);
    }
    let work = async {
        let keys = keys(environment, home).await?;
        let mut selected = None;
        for key in keys {
            let response = http
                .raw(HttpRequest {
                    method: "GET",
                    url: "https://opencode.ai/zen/go/v1/usage".into(),
                    headers: BTreeMap::from([(
                        "authorization".into(),
                        Sensitive::new(format!("Bearer {}", key.expose())),
                    )]),
                    body: None,
                })
                .await?;
            let retry = matches!(response.status, 401 | 403);
            selected = Some((key, response));
            if !retry {
                break;
            }
        }
        let Some((key, response)) = selected else {
            return Ok(unavailable(checked, "unsupported", None));
        };
        if response.status == 403 {
            return Ok(unavailable(checked, "unsupported", None));
        }
        if !(200..300).contains(&response.status) {
            return Err(ServiceError::Network);
        }
        let body: Value = serde_json::from_slice(response.body.expose()).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
        )?;
        let usage = &body["usage"];
        let mut windows = vec![];
        for (key, kind, label, duration) in [
            ("rolling", "session", "Session", Some(300_u64)),
            ("weekly", "weekly", "Weekly", Some(10080)),
            ("monthly", "monthly", "Monthly", None),
        ] {
            let meter = &usage[key];
            let percent = meter["percent"].as_f64().ok_or(ServiceError::Response)?;
            let resets = meter["resetsAt"]
                .as_str()
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                .ok_or(ServiceError::Response)?;
            let mut window = json!({"id":format!("go_{key}"),"kind":kind,"label":format!("Go · {label}"),"usedPercent":clamp_percent(percent),"resetsAt":resets.to_utc().to_rfc3339_opts(chrono::SecondsFormat::Millis,true)});
            if let Some(duration) = duration {
                window["windowDurationMins"] = json!(duration);
            }
            windows.push(window);
        }
        let mut result = finish(checked, windows)?;
        result.credential_fingerprint = Some(Some(
            format!(
                "{:x}",
                Sha256::digest(format!("opencode-go\0{}", key.expose()).as_bytes())
            )
            .into(),
        ));
        Ok(result)
    };
    match tokio::time::timeout(Duration::from_secs(5), work).await {
        Ok(Ok(limits)) => limits,
        _ => unavailable(
            checked,
            "probeFailed",
            Some("OpenCode Go could not read usage."),
        ),
    }
}
