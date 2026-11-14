use crate::{
    environment::Environment,
    sensitive::{Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use t3_contracts::provider_usage_limits::ServerProviderUsageLimits;
pub const DEFAULT_API_ENDPOINT: &str = "https://api2.cursor.sh";
pub(crate) fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| {
            value.as_str().and_then(|value| {
                let value = value.trim();
                if value.is_empty() {
                    return Some(0.);
                }
                for (prefix, radix) in [
                    ("0x", 16),
                    ("0X", 16),
                    ("0b", 2),
                    ("0B", 2),
                    ("0o", 8),
                    ("0O", 8),
                ] {
                    if let Some(value) = value.strip_prefix(prefix) {
                        return u64::from_str_radix(value, radix)
                            .ok()
                            .map(|value| value as f64);
                    }
                }
                value.parse::<f64>().ok()
            })
        })
        .filter(|value| value.is_finite())
}
pub fn limits(response: &Value, checked_at: &str) -> ServiceResult<ServerProviderUsageLimits> {
    for field in ["billingCycleStart", "billingCycleEnd"] {
        if response
            .get(field)
            .is_some_and(|value| !value.is_string() && !value.is_number())
        {
            return Err(ServiceError::Response);
        }
    }
    if let Some(plan) = response.get("planUsage")
        && (!plan.is_object()
            || ["totalPercentUsed", "autoPercentUsed", "apiPercentUsed"]
                .iter()
                .any(|field| plan.get(field).is_some_and(|value| !value.is_number())))
    {
        return Err(ServiceError::Response);
    }
    let end = response
        .get("billingCycleEnd")
        .and_then(number)
        .filter(|end| *end > 0.);
    let reset = end
        .and_then(|end| chrono::DateTime::from_timestamp_millis(end as i64))
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let start = response
        .get("billingCycleStart")
        .and_then(number)
        .filter(|start| *start > 0.);
    let duration = start
        .zip(end)
        .filter(|_| reset.is_some())
        .map(|(start, end)| ((end - start) / 60_000.).round())
        .filter(|duration| *duration > 0.);
    let mut windows = vec![];
    for (id, label) in [
        ("totalPercentUsed", "Overall"),
        ("autoPercentUsed", "Cursor Models"),
        ("apiPercentUsed", "Other Models"),
    ] {
        if let Some(used) = response["planUsage"][id]
            .as_f64()
            .filter(|used| used.is_finite())
        {
            let mut window = json!({"id":id,"kind":"monthly","label":label,"usedPercent":crate::usage::clamp_percent(used)});
            if let Some(reset) = reset.as_ref() {
                window["resetsAt"] = json!(reset);
            }
            if let Some(duration) = duration {
                window["windowDurationMins"] = json!(duration as u64);
            }
            windows.push(window);
        }
    }
    windows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    if windows.is_empty() {
        return crate::account_usage::unavailable(checked_at, "unsupported");
    }
    serde_json::from_value(json!({"checkedAt":checked_at,"windows":windows})).map_err(
        crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
    )
}
async fn post(
    http: &dyn HttpTransport,
    endpoint: String,
    token: &str,
    dashboard: bool,
) -> ServiceResult<Value> {
    let mut headers = BTreeMap::from([
        (
            "authorization".into(),
            Sensitive::new(format!("Bearer {token}")),
        ),
        (
            "content-type".into(),
            Sensitive::new("application/json".into()),
        ),
    ]);
    if dashboard {
        headers.insert(
            "connect-protocol-version".into(),
            Sensitive::new("1".into()),
        );
        headers.insert("x-cursor-client-type".into(), Sensitive::new("cli".into()));
    }
    let response = http
        .send(HttpRequest {
            method: "POST",
            url: endpoint,
            headers,
            body: Some(Sensitive::new("{}".into())),
        })
        .await?;
    if !(200..300).contains(&response.status) {
        return Err(ServiceError::Response);
    }
    Ok(response.body.into_inner())
}
pub async fn read(
    http: &dyn HttpTransport,
    endpoint: Option<&str>,
    environment: &Environment,
    checked_at: &str,
) -> ServiceResult<ServerProviderUsageLimits> {
    let work = async {
        let nonempty = |value: &str| !value.trim().is_empty();
        let endpoint = endpoint
            .filter(|value| nonempty(value))
            .map(str::trim)
            .or_else(|| {
                environment
                    .get("CURSOR_API_ENDPOINT")
                    .map(|value| value.trim())
                    .filter(|value| nonempty(value))
            })
            .unwrap_or(DEFAULT_API_ENDPOINT)
            .trim_end_matches('/');
        let mut token = environment
            .get("CURSOR_AUTH_TOKEN")
            .map(|value| value.trim().to_owned())
            .filter(|value| nonempty(value));
        if token.is_none()
            && let Some(key) = environment
                .get("CURSOR_API_KEY")
                .map(|key| key.trim())
                .filter(|key| nonempty(key))
        {
            if endpoint != DEFAULT_API_ENDPOINT {
                return crate::account_usage::unavailable(checked_at, "unsupported");
            }
            let response = post(
                http,
                format!("{DEFAULT_API_ENDPOINT}/auth/exchange_user_api_key"),
                key,
                false,
            )
            .await?;
            token = Some(
                response["accessToken"]
                    .as_str()
                    .filter(|token| !token.is_empty())
                    .ok_or(ServiceError::Response)?
                    .to_owned(),
            );
        }
        let Some(token) = token else {
            return crate::account_usage::unavailable(checked_at, "unsupported");
        };
        limits(
            &post(
                http,
                format!("{endpoint}/aiserver.v1.DashboardService/GetCurrentPeriodUsage"),
                &token,
                true,
            )
            .await?,
            checked_at,
        )
    };
    match tokio::time::timeout(Duration::from_secs(10), work).await {
        Ok(Ok(limits)) => Ok(limits),
        _ => {
            let mut unavailable = serde_json::to_value(crate::account_usage::unavailable(
                checked_at,
                "probeFailed",
            )?)
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
            unavailable["unavailable"]["message"] = json!("Cursor could not read usage limits.");
            serde_json::from_value(unavailable).map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))
        }
    }
}
