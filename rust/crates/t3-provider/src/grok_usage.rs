use crate::{
    environment::Environment,
    sensitive::{Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use t3_contracts::provider_usage_limits::ServerProviderUsageLimits;
use tokio::io::AsyncReadExt;
pub const USAGE_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
pub struct GrokAccount {
    pub email: Option<String>,
    pub usage_limits: ServerProviderUsageLimits,
}
async fn text(path: &Path) -> ServiceResult<Option<String>> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ServiceError::Storage),
    };
    let mut bytes = vec![];
    file.take(1_048_577)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ServiceError::Storage)?;
    if bytes.len() > 1_048_576 {
        return Err(ServiceError::Response);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| ServiceError::Response)
}
fn nonempty<'a>(environment: &'a Environment, key: &str) -> Option<&'a str> {
    environment
        .get(key)
        .map(|value| t3_contracts::wire::trim_js(value))
        .filter(|value| !value.is_empty())
}
async fn credential(
    environment: &Environment,
    home: &Path,
    system_config: &Path,
) -> ServiceResult<Option<Sensitive<Value>>> {
    if nonempty(environment, "XAI_API_KEY").is_some()
        || [
            "GROK_OIDC_ISSUER",
            "GROK_OIDC_CLIENT_ID",
            "GROK_OAUTH2_ISSUER",
            "GROK_OAUTH2_CLIENT_ID",
            "GROK_OAUTH2_PRINCIPAL_TYPE",
            "GROK_OAUTH2_PRINCIPAL_ID",
            "GROK_AUTH_PROVIDER_COMMAND",
            "GROK_LOCAL_AUTH",
            "GROK_CLI_CHAT_PROXY_BASE_URL",
            "GROK_MODELS_BASE_URL",
            "GROK_CONFIG",
            "GROK_CONFIG_PATH",
        ]
        .iter()
        .any(|key| nonempty(environment, key).is_some())
    {
        return Ok(None);
    }
    let directory = nonempty(environment, "GROK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            environment
                .get("HOME")
                .or_else(|| environment.get("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_else(|| home.into())
                .join(".grok")
        });
    let custom = regex::Regex::new(
        r#"(?m)^\s*(?:\[\[?\s*)?["']?(?:auth|grok_com_config|endpoints)["']?\s*[.\]=]"#,
    )
    .expect("Grok custom config");
    for path in [
        directory.join("config.toml"),
        directory.join("managed_config.toml"),
        directory.join("requirements.toml"),
        system_config.join("managed_config.toml"),
        system_config.join("requirements.toml"),
    ] {
        if text(&path)
            .await?
            .as_deref()
            .is_some_and(|text| custom.is_match(text))
        {
            return Ok(None);
        }
    }
    let contents = match nonempty(environment, "GROK_AUTH") {
        Some(contents) => contents.to_owned(),
        None => text(&directory.join("auth.json"))
            .await?
            .unwrap_or_else(|| "{}".into()),
    };
    let credentials: Value = serde_json::from_str(&contents).map_err(|_| ServiceError::Response)?;
    let credentials = credentials.as_object().ok_or(ServiceError::Response)?;
    if credentials.values().any(|credential| {
        credential.as_object().is_none()
            || ["key", "auth_mode", "email"]
                .iter()
                .any(|key| credential.get(*key).is_some_and(|value| !value.is_string()))
    }) {
        return Err(ServiceError::Response);
    }
    let credential = credentials
        .get("https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828")
        .or_else(|| credentials.get("https://accounts.x.ai/sign-in"));
    Ok(credential
        .filter(|credential| credential["auth_mode"] != "api_key")
        .cloned()
        .map(Sensitive::new))
}
fn unavailable(
    checked: &str,
    reason: &str,
    message: Option<&str>,
) -> ServiceResult<ServerProviderUsageLimits> {
    let mut value = json!({"checkedAt":checked,"windows":[],"unavailable":{"reason":reason}});
    if let Some(message) = message {
        value["unavailable"]["message"] = json!(message);
    }
    serde_json::from_value(value).map_err(|_| ServiceError::Response)
}
pub fn limits(response: &Value, checked: &str) -> ServiceResult<ServerProviderUsageLimits> {
    if !response.is_object()
        || response.get("config").is_some_and(|config| {
            !config.is_object()
                || config
                    .get("creditUsagePercent")
                    .is_some_and(|percent| !percent.is_number())
                || config.get("currentPeriod").is_some_and(|period| {
                    !period.is_object()
                        || ["type", "end"]
                            .iter()
                            .any(|key| period.get(*key).is_some_and(|value| !value.is_string()))
                })
        })
    {
        return Err(ServiceError::Response);
    }
    let period = &response["config"]["currentPeriod"];
    let kind = match period["type"]
        .as_str()
        .unwrap_or_default()
        .strip_prefix("USAGE_PERIOD_TYPE_")
        .unwrap_or(period["type"].as_str().unwrap_or_default())
    {
        "WEEKLY" => "weekly",
        "MONTHLY" => "monthly",
        _ => "other",
    };
    let iso=regex::Regex::new(r"^(?:\d{4}|[+-]\d{6})-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12]\d|3[01])T(?:(?:[01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d+)?)?|24:00(?::00(?:\.0+)?)?)(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$").expect("usage period timestamp");
    let boundary = |value: &Value| {
        value
            .as_str()
            .filter(|value| iso.is_match(value))
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    };
    let start = boundary(&period["start"]);
    let end = boundary(&period["end"]);
    let used = response["config"]["creditUsagePercent"]
        .as_f64()
        .or_else(|| {
            (matches!(kind, "weekly" | "monthly")
                && start.zip(end).is_some_and(|(start, end)| start < end))
            .then_some(0.)
        });
    let mut value = json!({"checkedAt":checked,"windows":[]});
    if let Some(used) = used.filter(|used| used.is_finite()) {
        let mut window = json!({"id":"subscription","kind":kind,"label":match kind{"weekly"=>"Weekly","monthly"=>"Monthly",_=>"Subscription"},"usedPercent":crate::usage::clamp_percent(used)});
        if let Some(reset) = period["end"]
            .as_str()
            .and_then(crate::catalog::date_millis)
            .and_then(chrono::DateTime::from_timestamp_millis)
        {
            window["resetsAt"] = json!(reset.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        }
        if let Some(duration) = start
            .zip(end)
            .map(|(start, end)| {
                ((end.timestamp_millis() - start.timestamp_millis()) as f64 / 60_000.).round()
            })
            .filter(|duration| *duration > 0.)
        {
            window["windowDurationMins"] = json!(duration as u64);
        }
        value["windows"] = json!([window]);
    }
    serde_json::from_value(value).map_err(|_| ServiceError::Response)
}
pub async fn read(
    http: &dyn HttpTransport,
    environment: &Environment,
    home: &Path,
    system_config: &Path,
    checked: &str,
) -> ServiceResult<GrokAccount> {
    let failed = || {
        unavailable(
            checked,
            "probeFailed",
            Some("Grok could not read usage limits."),
        )
    };
    let credential = match tokio::time::timeout(
        Duration::from_secs(10),
        credential(environment, home, system_config),
    )
    .await
    {
        Ok(Ok(credential)) => credential,
        _ => {
            return Ok(GrokAccount {
                email: None,
                usage_limits: failed()?,
            });
        }
    };
    let email = credential
        .as_ref()
        .and_then(|credential| credential.expose()["email"].as_str())
        .map(t3_contracts::wire::trim_js)
        .filter(|email| !email.is_empty())
        .map(str::to_owned);
    let Some(token) = credential
        .as_ref()
        .and_then(|credential| credential.expose()["key"].as_str())
        .map(t3_contracts::wire::trim_js)
        .filter(|token| !token.is_empty())
    else {
        return Ok(GrokAccount {
            email,
            usage_limits: unavailable(checked, "unsupported", None)?,
        });
    };
    let work = async {
        let response = http
            .send(HttpRequest {
                method: "GET",
                url: USAGE_URL.into(),
                headers: BTreeMap::from([(
                    "authorization".into(),
                    Sensitive::new(format!("Bearer {token}")),
                )]),
                body: None,
            })
            .await?;
        if response.status == 403
            && response.body.expose()["error"]
                .as_str()
                .is_some_and(|error| error.contains("WKE=privacy-mode:"))
        {
            return unavailable(
                checked,
                "probeFailed",
                Some("Switch your xAI account to Privacy Mode to show Grok usage limits."),
            );
        }
        if !(200..300).contains(&response.status) {
            return Err(ServiceError::Network);
        }
        limits(response.body.expose(), checked)
    };
    let usage_limits = match tokio::time::timeout(Duration::from_secs(10), work).await {
        Ok(Ok(limits)) => limits,
        _ => failed()?,
    };
    Ok(GrokAccount {
        email,
        usage_limits,
    })
}
