use crate::{
    sensitive::{ServiceError, ServiceResult},
    usage::clamp_percent,
};
use serde_json::{Value, json};
use t3_contracts::provider_usage_limits::{ProviderUsageLimitsUpdate, ServerProviderUsageLimits};

fn epoch_iso(value: Option<f64>) -> Option<String> {
    let value = value.filter(|v| v.is_finite() && *v > 0.)? * 1000.;
    chrono::DateTime::from_timestamp_millis(value as i64)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}
fn string_iso(value: Option<&str>) -> Option<String> {
    epoch_iso(
        value
            .and_then(crate::catalog::date_millis)
            .map(|n| n as f64 / 1000.),
    )
}
fn limits(
    checked: &str,
    mut windows: Vec<Value>,
    unavailable: Option<&str>,
) -> ServiceResult<ServerProviderUsageLimits> {
    let rank = |w: &Value| match w["kind"].as_str() {
        Some("session") => 0,
        Some("weekly") => 1,
        Some("monthly") => 2,
        _ => 3,
    };
    windows.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    let mut value = json!({"checkedAt":checked,"windows":windows});
    if let Some(reason) = unavailable {
        value["unavailable"] = json!({"reason":reason});
    }
    serde_json::from_value(value).map_err(|_| ServiceError::Response)
}
pub fn unavailable(checked: &str, reason: &str) -> ServiceResult<ServerProviderUsageLimits> {
    limits(checked, vec![], Some(reason))
}
fn codex_windows(snapshot: &Value) -> Vec<Value> {
    if snapshot
        .get("limitId")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty() && s != "codex")
    {
        return vec![];
    }
    let monthly = matches!(snapshot["planType"].as_str(), Some("free" | "go"));
    let mut output = vec![];
    for (id, fallback) in [
        ("primary", if monthly { 43200 } else { 300 }),
        ("secondary", 10080),
    ] {
        let window = &snapshot[id];
        let Some(used) = window["usedPercent"].as_f64().filter(|v| v.is_finite()) else {
            continue;
        };
        let duration = window["windowDurationMins"].as_u64().unwrap_or(fallback);
        let (kind, label) = if duration >= 43200 {
            ("monthly", "Monthly")
        } else if duration >= 10080 {
            ("weekly", "Weekly")
        } else {
            ("session", "Session")
        };
        let mut next = json!({"id":id,"kind":kind,"label":label,"usedPercent":clamp_percent(used),"windowDurationMins":duration});
        if let Some(reset) = epoch_iso(window["resetsAt"].as_f64()) {
            next["resetsAt"] = json!(reset);
        }
        output.push(next);
    }
    if let Some(remaining) = snapshot["individualLimit"]["remainingPercent"]
        .as_f64()
        .filter(|v| v.is_finite())
    {
        let mut next = json!({"id":"individual","kind":"monthly","label":"Monthly credit limit","usedPercent":clamp_percent(100.-remaining)});
        if let Some(reset) = epoch_iso(snapshot["individualLimit"]["resetsAt"].as_f64()) {
            next["resetsAt"] = json!(reset);
        }
        output.push(next);
    }
    output
}
pub fn codex_limits(response: &Value, checked: &str) -> ServiceResult<ServerProviderUsageLimits> {
    let snapshot = response
        .get("rateLimitsByLimitId")
        .and_then(|v| v.get("codex"))
        .filter(|v| !v.is_null())
        .or_else(|| response.get("rateLimits"))
        .unwrap_or(response);
    let mut value = serde_json::to_value(limits(checked, codex_windows(snapshot), None)?)
        .map_err(|_| ServiceError::Response)?;
    if let Some(credits) = response
        .get("rateLimitResetCredits")
        .filter(|v| v.is_object())
    {
        let count = credits["availableCount"].as_i64().unwrap_or(0).max(0);
        let mut summary = json!({"availableCount":count});
        let reset = credits
            .get("credits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c["status"] == "available")
            .filter_map(|c| c["expiresAt"].as_f64())
            .min_by(f64::total_cmp);
        if let Some(reset) = epoch_iso(reset) {
            summary["nextExpiresAt"] = json!(reset);
        }
        value["resetCredits"] = summary;
    }
    serde_json::from_value(value).map_err(|_| ServiceError::Response)
}
pub fn codex_update(
    snapshot: &Value,
    previous: Option<&Value>,
) -> ServiceResult<Option<ProviderUsageLimitsUpdate>> {
    let previous = previous.map(codex_windows).unwrap_or_default();
    let windows: Vec<_> = codex_windows(snapshot)
        .into_iter()
        .filter(|next| {
            !previous
                .iter()
                .any(|old| old["id"] == next["id"] && old == next)
        })
        .collect();
    if windows.is_empty() {
        Ok(None)
    } else {
        serde_json::from_value(json!({"windows":windows}))
            .map(Some)
            .map_err(|_| ServiceError::Response)
    }
}
pub fn merge_codex(previous: Option<&Value>, update: &Value) -> Option<Value> {
    if update
        .get("limitId")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty() && s != "codex")
    {
        return previous.cloned();
    }
    let mut next = previous.cloned().unwrap_or_else(|| json!({}));
    for key in [
        "limitId",
        "planType",
        "rateLimitReachedType",
        "primary",
        "secondary",
        "individualLimit",
    ] {
        if let Some(value) = update.get(key) {
            next[key] = value.clone();
        }
    }
    Some(next)
}
pub fn codex_reset_at(snapshot: &Value) -> Option<String> {
    let exhausted: Vec<_> = codex_windows(snapshot)
        .into_iter()
        .filter(|w| w["usedPercent"].as_f64().is_some_and(|n| n >= 100.))
        .collect();
    if exhausted.is_empty() || exhausted.iter().any(|w| w["resetsAt"].as_str().is_none()) {
        return None;
    }
    exhausted
        .iter()
        .filter_map(|w| w["resetsAt"].as_str())
        .max_by_key(|s| crate::catalog::date_millis(s))
        .map(str::to_owned)
}
#[derive(Debug, Clone, Default)]
pub struct ClaudeScopedLimitNames {
    pub overage_included: Option<String>,
}
fn scoped_id(name: &str) -> String {
    let mut id = String::from("seven_day_");
    let mut separator = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c);
            separator = false;
        } else if !separator {
            id.push('_');
            separator = true;
        }
    }
    id
}
fn scoped_window(name: &str, used: f64, reset: Option<String>) -> Value {
    let mut window = json!({"id":scoped_id(name),"kind":"weekly","label":format!("Weekly · {name}"),"windowDurationMins":10080,"usedPercent":clamp_percent(used)});
    if let Some(reset) = reset {
        window["resetsAt"] = json!(reset);
    }
    window
}
fn ratio(used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let (used, limit) = (used?, limit?);
    (used.is_finite() && limit.is_finite() && used >= 0. && limit > 0.)
        .then_some(used / limit * 100.)
        .filter(|v| v.is_finite())
        .map(clamp_percent)
}
pub fn claude_limits(
    response: &Value,
    checked: &str,
) -> ServiceResult<(ServerProviderUsageLimits, ClaudeScopedLimitNames)> {
    let available = response["rate_limits_available"].as_bool() == Some(true);
    let Some(rates) = response
        .get("rate_limits")
        .filter(|v| !v.is_null() && available)
    else {
        return Ok((
            unavailable(
                checked,
                if available {
                    "probeFailed"
                } else {
                    "unsupported"
                },
            )?,
            ClaudeScopedLimitNames::default(),
        ));
    };
    let mut windows = vec![];
    for (id, kind, label, duration) in [
        ("five_hour", "session", "Session", 300),
        ("seven_day", "weekly", "Weekly", 10080),
    ] {
        if let Some(used) = rates[id]["utilization"].as_f64() {
            let mut window = json!({"id":id,"kind":kind,"label":label,"windowDurationMins":duration,"usedPercent":clamp_percent(used)});
            if let Some(reset) = string_iso(rates[id]["resets_at"].as_str()) {
                window["resetsAt"] = json!(reset);
            }
            windows.push(window);
        }
    }
    let mut names = ClaudeScopedLimitNames::default();
    for entry in rates
        .get("model_scoped")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(name), Some(used)) = (
            entry["display_name"].as_str(),
            entry["utilization"].as_f64(),
        ) {
            windows.push(scoped_window(
                name,
                used,
                string_iso(entry["resets_at"].as_str()),
            ));
            names.overage_included.get_or_insert(name.into());
        }
    }
    let spend = &rates["spend"];
    let used = &spend["used"];
    let limit = &spend["limit"];
    let mut percent = if spend["enabled"].as_bool() == Some(true)
        && used["currency"].as_str().is_some_and(|s| {
            !s.trim().is_empty() && Some(s.trim()) == limit["currency"].as_str().map(str::trim)
        })
        && used["exponent"].as_u64().is_some()
        && used["exponent"] == limit["exponent"]
        && used["amount_minor"].as_u64().is_some()
        && limit["amount_minor"].as_u64().is_some()
    {
        ratio(
            used["amount_minor"].as_f64(),
            limit["amount_minor"].as_f64(),
        )
    } else {
        None
    };
    let extra = &rates["extra_usage"];
    if percent.is_none() && extra["is_enabled"].as_bool() == Some(true) {
        percent = ratio(
            extra["used_credits"].as_f64(),
            extra["monthly_limit"].as_f64(),
        )
        .or_else(|| {
            extra["utilization"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.)
                .map(clamp_percent)
        });
    }
    if let Some(percent) = percent {
        windows.push(json!({"id":"monthly_spend","kind":"monthly","label":"Monthly spend","usedPercent":percent}));
    }
    Ok((limits(checked, windows, None)?, names))
}
pub fn claude_update(
    info: &Value,
    names: &ClaudeScopedLimitNames,
) -> ServiceResult<Option<ProviderUsageLimitsUpdate>> {
    let (Some(kind), Some(used)) = (info["rateLimitType"].as_str(), info["utilization"].as_f64())
    else {
        return Ok(None);
    };
    let reset = epoch_iso(info["resetsAt"].as_f64());
    let window = match kind {
        "five_hour" | "seven_day" => {
            let mut window = json!({"id":kind,"kind":if kind == "five_hour" {"session"} else {"weekly"},"label":if kind == "five_hour" {"Session"} else {"Weekly"},"windowDurationMins":if kind == "five_hour" {300} else {10080},"usedPercent":clamp_percent(used*100.)});
            if let Some(reset) = reset {
                window["resetsAt"] = json!(reset);
            }
            window
        }
        "seven_day_overage_included" => {
            let Some(name) = names.overage_included.as_ref() else {
                return Ok(None);
            };
            scoped_window(name, used * 100., reset)
        }
        _ => return Ok(None),
    };
    serde_json::from_value(json!({"windows":[window]}))
        .map(Some)
        .map_err(|_| ServiceError::Response)
}
