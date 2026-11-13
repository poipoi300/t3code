use crate::{Error, UsageProviderKind as P, iso, sources::ProbeFuture, text, timestamp_ms};
use serde_json::{Value, json};
use t3_contracts::provider_usage_limits::{
    ProviderConsumeResetCreditResult, ServerProviderUsageLimits as Limits,
    ServerProviderUsageWindow as Window, UsageLimitSourceAccount,
};

pub fn clamp_percent(n: f64) -> f64 {
    if n.is_finite() {
        n.clamp(0.0, 100.0)
    } else {
        0.0
    }
}
fn rank(v: &Value) -> u8 {
    match text(&v["kind"]) {
        "session" => 0,
        "weekly" => 1,
        "monthly" => 2,
        _ => 3,
    }
}
pub fn make_limits(checked_at: &str, mut windows: Vec<Value>) -> Result<Limits, Error> {
    windows.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| crate::collation::compare(text(&a["id"]), text(&b["id"])))
    });
    Ok(serde_json::from_value(
        json!({"checkedAt":checked_at,"windows":windows}),
    )?)
}
pub fn unavailable(checked_at: &str, reason: &str, message: Option<&str>) -> Result<Limits, Error> {
    let mut v = json!({"checkedAt":checked_at,"windows":[],"unavailable":{"reason":reason}});
    if let Some(message) = message {
        v["unavailable"]["message"] = json!(message);
    }
    Ok(serde_json::from_value(v)?)
}
fn window(
    id: &str,
    kind: &str,
    label: &str,
    percent: f64,
    reset: Option<String>,
    duration: Option<u64>,
) -> Value {
    let mut v = json!({"id":id,"kind":kind,"label":label,"usedPercent":clamp_percent(percent)});
    if let Some(reset) = reset.filter(|s| !s.is_empty()) {
        v["resetsAt"] = json!(reset);
    }
    if let Some(duration) = duration {
        v["windowDurationMins"] = json!(duration);
    }
    v
}
fn finite(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}
fn reset(v: &Value) -> Option<String> {
    v.as_str().and_then(timestamp_ms).map(iso)
}
fn epoch(v: &Value) -> Option<String> {
    finite(v)
        .filter(|n| *n > 0.0)
        .map(|n| iso(n * 1000.0))
        .filter(|s| !s.is_empty())
}

pub fn codex_limits(
    snapshot: &Value,
    by_id: Option<&Value>,
    credits: Option<&Value>,
    checked_at: &str,
) -> Result<Limits, Error> {
    let s = by_id.and_then(|v| v.get("codex")).unwrap_or(snapshot);
    let mut windows = vec![];
    if text(&s["limitId"]).is_empty() || s["limitId"] == "codex" {
        let monthly = ["free", "go"].contains(&text(&s["planType"]));
        for (id, default) in [
            ("primary", if monthly { 43200 } else { 300 }),
            ("secondary", 10080),
        ] {
            let w = &s[id];
            let Some(percent) = finite(&w["usedPercent"]) else {
                continue;
            };
            let duration = w["windowDurationMins"].as_u64().unwrap_or(default);
            let (kind, label) = if duration >= 43200 {
                ("monthly", "Monthly")
            } else if duration >= 10080 {
                ("weekly", "Weekly")
            } else {
                ("session", "Session")
            };
            windows.push(window(
                id,
                kind,
                label,
                percent,
                epoch(&w["resetsAt"]),
                Some(duration),
            ));
        }
        if let Some(remaining) = finite(&s["individualLimit"]["remainingPercent"]) {
            windows.push(window(
                "individual",
                "monthly",
                "Monthly credit limit",
                100.0 - remaining,
                epoch(&s["individualLimit"]["resetsAt"]),
                None,
            ));
        }
    }
    let mut limits = serde_json::to_value(make_limits(checked_at, windows)?)?;
    if let Some(credits) = credits.filter(|v| !v.is_null()) {
        let mut summary = json!({"availableCount":finite(&credits["availableCount"]).unwrap_or(0.0).max(0.0) as u64});
        let earliest = credits["credits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["status"] == "available")
            .filter_map(|c| finite(&c["expiresAt"]))
            .min_by(f64::total_cmp);
        if let Some(at) = earliest.and_then(|at| epoch(&json!(at))) {
            summary["nextExpiresAt"] = json!(at);
        }
        limits["resetCredits"] = summary;
    }
    Ok(serde_json::from_value(limits)?)
}
pub fn codex_limit_reset_at(snapshot: &Value) -> Option<String> {
    let limits = codex_limits(snapshot, None, None, "2000-01-01T00:00:00.000Z").ok()?;
    let windows: Vec<_> = limits
        .windows
        .iter()
        .filter(|w| {
            serde_json::to_value(&w.used_percent)
                .ok()
                .and_then(|n| n.as_f64())
                .unwrap_or(0.0)
                >= 100.0
        })
        .collect();
    if windows.is_empty() {
        return None;
    }
    windows
        .into_iter()
        .map(|w| {
            serde_json::to_value(w)
                .ok()
                .and_then(|v| v["resetsAt"].as_str().map(str::to_string))
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()
}

pub fn merge_codex_limits(previous: Option<Value>, update: &Value) -> Option<Value> {
    if !text(&update["limitId"]).is_empty() && update["limitId"] != "codex" {
        return previous;
    }
    let Some(mut previous) = previous.filter(Value::is_object) else {
        return Some(update.clone());
    };
    for key in [
        "limitId",
        "planType",
        "rateLimitReachedType",
        "primary",
        "secondary",
        "individualLimit",
    ] {
        if let Some(value) = update.get(key) {
            previous[key] = value.clone();
        }
    }
    Some(previous)
}
pub fn codex_limits_update(
    snapshot: &Value,
    previous: Option<&Value>,
) -> Result<Option<t3_contracts::provider_usage_limits::ProviderUsageLimitsUpdate>, Error> {
    let at = "2000-01-01T00:00:00.000Z";
    let previous = previous
        .map(|p| codex_limits(p, None, None, at))
        .transpose()?
        .map(|l| l.windows)
        .unwrap_or_default();
    let windows = codex_limits(snapshot, None, None, at)?
        .windows
        .into_iter()
        .filter(|w| !previous.contains(w))
        .collect::<Vec<_>>();
    Ok((!windows.is_empty())
        .then_some(t3_contracts::provider_usage_limits::ProviderUsageLimitsUpdate { windows }))
}
pub fn claude_limits_update(
    info: &Value,
    overage_name: Option<&str>,
) -> Result<Option<t3_contracts::provider_usage_limits::ProviderUsageLimitsUpdate>, Error> {
    let kind = text(&info["rateLimitType"]);
    let Some(utilization) = info["utilization"].as_f64() else {
        return Ok(None);
    };
    let mut rates = serde_json::Map::new();
    match kind {
        "five_hour" | "seven_day" => {
            rates.insert(
                kind.into(),
                json!({"utilization":utilization*100.0,"resets_at":epoch(&info["resetsAt"])}),
            );
        }
        "seven_day_overage_included" => {
            let Some(name) = overage_name else {
                return Ok(None);
            };
            rates.insert("model_scoped".into(),json!([{"display_name":name,"utilization":utilization*100.0,"resets_at":epoch(&info["resetsAt"])}]));
        }
        _ => return Ok(None),
    }
    let limits = claude_limits(
        &json!({"rate_limits_available":true,"rate_limits":rates}),
        "2000-01-01T00:00:00.000Z",
    )?;
    Ok(Some(
        t3_contracts::provider_usage_limits::ProviderUsageLimitsUpdate {
            windows: limits.windows,
        },
    ))
}
pub fn claude_limits(response: &Value, checked_at: &str) -> Result<Limits, Error> {
    if response["rate_limits_available"] != true || !response["rate_limits"].is_object() {
        return unavailable(
            checked_at,
            if response["rate_limits_available"] == true {
                "probeFailed"
            } else {
                "unsupported"
            },
            None,
        );
    }
    let r = &response["rate_limits"];
    let mut windows = vec![];
    for (id, kind, label, duration) in [
        ("five_hour", "session", "Session", 300),
        ("seven_day", "weekly", "Weekly", 10080),
    ] {
        if let Some(n) = finite(&r[id]["utilization"]) {
            windows.push(window(
                id,
                kind,
                label,
                n,
                reset(&r[id]["resets_at"]),
                Some(duration),
            ));
        }
    }
    for w in r["model_scoped"].as_array().into_iter().flatten() {
        let Some(display) = w["display_name"].as_str() else {
            continue;
        };
        let Some(n) = finite(&w["utilization"]) else {
            continue;
        };
        let mut id = String::from("seven_day_");
        let mut separator = false;
        for c in display.to_lowercase().chars() {
            if c.is_ascii_alphanumeric() {
                id.push(c);
                separator = false;
            } else if !separator {
                id.push('_');
                separator = true;
            }
        }
        windows.push(window(
            &id,
            "weekly",
            &format!("Weekly · {display}"),
            n,
            reset(&w["resets_at"]),
            Some(10080),
        ));
    }
    let ratio = |used: &Value, limit: &Value| {
        finite(used)
            .filter(|n| *n >= 0.0)
            .zip(finite(limit).filter(|n| *n > 0.0))
            .map(|(u, l)| u / l * 100.0)
            .filter(|n| n.is_finite())
    };
    let spend = &r["spend"];
    let percent = if spend["enabled"] == true
        && !text(&spend["used"]["currency"]).trim().is_empty()
        && text(&spend["used"]["currency"]).trim() == text(&spend["limit"]["currency"]).trim()
        && spend["used"]["exponent"] == spend["limit"]["exponent"]
    {
        ratio(
            &spend["used"]["amount_minor"],
            &spend["limit"]["amount_minor"],
        )
    } else {
        None
    };
    let extra = &r["extra_usage"];
    let percent = percent.or_else(|| {
        (extra["is_enabled"] == true)
            .then(|| {
                ratio(&extra["used_credits"], &extra["monthly_limit"])
                    .or_else(|| finite(&extra["utilization"]).filter(|n| *n >= 0.0))
            })
            .flatten()
    });
    if let Some(n) = percent {
        windows.push(window(
            "monthly_spend",
            "monthly",
            "Monthly spend",
            n,
            None,
            None,
        ));
    }
    make_limits(checked_at, windows)
}
pub fn grok_limits(response: &Value, checked_at: &str) -> Result<Limits, Error> {
    let r = &response["config"];
    let period = &r["currentPeriod"];
    let kind = match text(&period["type"]).trim_start_matches("USAGE_PERIOD_TYPE_") {
        "WEEKLY" => "weekly",
        "MONTHLY" => "monthly",
        _ => "other",
    };
    let boundary = |v: &Value| {
        v.as_str()
            .filter(|s| s.trim() == *s)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis() as f64)
    };
    let start = boundary(&period["start"]);
    let end = boundary(&period["end"]);
    let duration = start
        .zip(end)
        .map(|(s, e)| ((e - s) / 60000.0).round())
        .filter(|n| *n > 0.0);
    let percent = finite(&r["creditUsagePercent"]).or_else(|| {
        (!r.as_object()
            .is_some_and(|r| r.contains_key("creditUsagePercent"))
            && ["weekly", "monthly"].contains(&kind)
            && start.zip(end).is_some_and(|(s, e)| s < e))
        .then_some(0.0)
    });
    let windows = percent
        .map(|n| {
            vec![window(
                "subscription",
                kind,
                match kind {
                    "weekly" => "Weekly",
                    "monthly" => "Monthly",
                    _ => "Subscription",
                },
                n,
                reset(&period["end"]),
                duration.map(|n| n as u64),
            )]
        })
        .unwrap_or_default();
    make_limits(checked_at, windows)
}
pub fn cursor_limits(response: &Value, checked_at: &str) -> Result<Limits, Error> {
    let number = |v: &Value| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    };
    let end = number(&response["billingCycleEnd"]).filter(|n| n.is_finite() && *n > 0.0);
    let start = number(&response["billingCycleStart"]).filter(|n| *n > 0.0);
    let duration = start
        .zip(end)
        .map(|(s, e)| ((e - s) / 60000.0).round())
        .filter(|n| *n > 0.0)
        .map(|n| n as u64);
    let mut windows = vec![];
    for (id, label) in [
        ("totalPercentUsed", "Overall"),
        ("autoPercentUsed", "Cursor Models"),
        ("apiPercentUsed", "Other Models"),
    ] {
        if let Some(n) = finite(&response["planUsage"][id]) {
            windows.push(window(id, "monthly", label, n, end.map(iso), duration));
        }
    }
    if windows.is_empty() {
        unavailable(checked_at, "unsupported", None)
    } else {
        make_limits(checked_at, windows)
    }
}
pub fn muse_limits(usage: &Value) -> Result<Limits, Error> {
    let at = finite(&usage["observedAtMs"])
        .ok_or_else(|| Error::Probe("Missing Muse observation time".into()))?;
    let mut windows = vec![];
    for (key, id, kind, label, duration) in [
        (
            "window",
            "session",
            "session",
            "Session",
            usage["window"]["windowDurationMins"].as_u64(),
        ),
        ("weekly", "weekly", "weekly", "Weekly", Some(10080)),
    ] {
        let w = &usage[key];
        let n = finite(&w["usedPercent"])
            .ok_or_else(|| Error::Probe("Missing Muse usage window".into()))?;
        windows.push(window(
            id,
            kind,
            label,
            n,
            finite(&w["resetsAtMs"]).map(iso),
            duration,
        ));
    }
    make_limits(&iso(at), windows)
}
pub fn muse_limit_reset_at(
    usage: Option<&Value>,
    now: f64,
    refusal: Option<&Value>,
) -> Option<String> {
    let mut resets = vec![];
    if let Some(usage) = usage {
        for key in ["window", "weekly"] {
            if finite(&usage[key]["usedPercent"]).is_some_and(|n| n >= 100.0) {
                resets.push(finite(&usage[key]["resetsAtMs"]));
            }
        }
    }
    if let Some(refusal) = refusal {
        resets.push(finite(&refusal["quota"]["resetsAtMs"]));
        let message = text(&refusal["message"]);
        if let Some(i) = message
            .as_bytes()
            .windows(10)
            .position(|s| s.eq_ignore_ascii_case(b"resets at "))
        {
            let rest = &message[i + 10..];
            let end = rest.char_indices().find_map(|(i, c)| {
                if c == 'Z' || c == 'z' {
                    Some(i + 1)
                } else if i >= 19 && (c == '+' || c == '-') {
                    Some(i + 6)
                } else {
                    None
                }
            });
            if let Some(value) = end.and_then(|end| rest.get(..end)) {
                resets.push(timestamp_ms(value));
            }
        }
    }
    resets
        .into_iter()
        .flatten()
        .filter(|n| n.is_finite() && *n > now && *n <= 8_640_000_000_000_000.0)
        .max_by(f64::total_cmp)
        .map(iso)
        .filter(|s| !s.is_empty())
}
pub fn opencode_limits(response: &Value, checked_at: &str) -> Result<Limits, Error> {
    let mut windows = vec![];
    for (key, id, kind, label, duration) in [
        (
            "rolling",
            "go_rolling",
            "session",
            "Go · Session",
            Some(300),
        ),
        ("weekly", "go_weekly", "weekly", "Go · Weekly", Some(10080)),
        ("monthly", "go_monthly", "monthly", "Go · Monthly", None),
    ] {
        let w = &response["usage"][key];
        let n = finite(&w["percent"])
            .ok_or_else(|| Error::Probe("Invalid OpenCode Go usage window".into()))?;
        windows.push(window(id, kind, label, n, reset(&w["resetsAt"]), duration));
    }
    make_limits(checked_at, windows)
}
pub fn antigravity_limits(
    response: &Value,
    checked_at: &str,
    requested_at: f64,
) -> Result<Limits, Error> {
    let groups = response["groups"].as_array();
    let groups: Vec<_> = if groups.is_some_and(|g| {
        g.iter()
            .any(|g| g["buckets"].as_array().is_some_and(|b| !b.is_empty()))
    }) {
        groups.into_iter().flatten().collect()
    } else {
        vec![response]
    };
    let mut windows = indexmap::IndexMap::new();
    for group in groups {
        for bucket in group["buckets"].as_array().into_iter().flatten() {
            if bucket["disabled"] == true {
                continue;
            }
            let id = text(&bucket["bucketId"]);
            let Some(remaining) = finite(&bucket["remainingFraction"]) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            let group_name = text(&group["displayName"]).trim();
            let family = if id.starts_with("gemini-argon-") {
                if group_name.is_empty() {
                    "Gemini Argon"
                } else {
                    group_name
                }
            } else if id.starts_with("gemini-carbon-") {
                if group_name.is_empty() {
                    "Gemini Carbon"
                } else {
                    group_name
                }
            } else if id.starts_with("gemini-") {
                "Gemini"
            } else if id.starts_with("3p-") {
                "Claude & GPT (shared)"
            } else if group_name.is_empty() {
                id
            } else {
                group_name
            };
            let (kind, label, duration) = match text(&bucket["window"]) {
                "weekly" => ("weekly", "Weekly", Some(10080)),
                "5h" => ("session", "5-hour", Some(300)),
                _ => (
                    "other",
                    bucket["displayName"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Quota"),
                    None,
                ),
            };
            let mut at = reset(&bucket["resetTime"]);
            if remaining == 1.0
                && bucket.get("description").is_none()
                && duration
                    .zip(at.as_ref().and_then(|s| timestamp_ms(s)))
                    .is_some_and(|(d, t)| (t - requested_at - d as f64 * 60000.0).abs() <= 60000.0)
            {
                at = None;
            }
            windows.insert(
                id.to_string(),
                window(
                    id,
                    kind,
                    &format!("{family} · {label}"),
                    (1.0 - remaining.clamp(0.0, 1.0)) * 100.0,
                    at,
                    duration,
                ),
            );
        }
    }
    make_limits(checked_at, windows.into_values().collect())
}
pub fn antigravity_model_limits(response: &Value, checked_at: &str) -> Result<Limits, Error> {
    let mut windows: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
    for (id, model) in response["models"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
    {
        let family = if model["modelProvider"] == "MODEL_PROVIDER_GOOGLE"
            || id.starts_with("gemini-")
        {
            "Gemini"
        } else if model["modelProvider"] == "MODEL_PROVIDER_ANTHROPIC" || id.starts_with("claude-")
        {
            "Claude"
        } else if model["modelProvider"] == "MODEL_PROVIDER_OPENAI" || id.starts_with("gpt-") {
            "GPT"
        } else {
            continue;
        };
        for (suffix, key) in [("quota", "quotaInfo"), ("weekly", "weeklyQuotaInfo")] {
            let quota = &model[key];
            let Some(remaining) = finite(&quota["remainingFraction"]) else {
                continue;
            };
            let id = format!("{}-{suffix}", family.to_lowercase());
            let weekly = suffix == "weekly";
            let label = format!(
                "{family} · {}",
                if weekly { "Weekly" } else { "Reported quota" }
            );
            let next = window(
                &id,
                if weekly { "weekly" } else { "other" },
                &label,
                (1.0 - remaining.clamp(0.0, 1.0)) * 100.0,
                reset(&quota["resetTime"]),
                weekly.then_some(10080),
            );
            if windows
                .get(&id)
                .is_none_or(|old| finite(&next["usedPercent"]) > finite(&old["usedPercent"]))
            {
                windows.insert(id, next);
            }
        }
    }
    make_limits(checked_at, windows.into_values().collect())
}

pub fn apply_limits_update(
    previous: Option<Limits>,
    update: Vec<Window>,
    checked_at: &str,
) -> Result<Option<Limits>, Error> {
    let p = previous
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    if update.is_empty() || p["unavailable"]["reason"] == "unsupported" {
        return Ok(previous);
    }
    let mut merged: indexmap::IndexMap<String, Value> = p["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| (text(&w["id"]).to_string(), w.clone()))
        .collect();
    let mut changed = false;
    for w in update {
        let mut next = serde_json::to_value(w)?;
        let id = text(&next["id"]).to_string();
        next["usedPercent"] = json!(clamp_percent(finite(&next["usedPercent"]).unwrap_or(0.0)));
        if let Some(old) = merged.get(&id) {
            for key in ["resetsAt", "windowDurationMins"] {
                if next.get(key).is_none()
                    && let Some(v) = old.get(key)
                {
                    next[key] = v.clone();
                }
            }
        }
        if merged.get(&id) != Some(&next) {
            merged.insert(id, next);
            changed = true;
        }
    }
    if !changed && previous.is_some() {
        return Ok(previous);
    }
    let mut v = serde_json::to_value(make_limits(checked_at, merged.into_values().collect())?)?;
    if previous.is_none() {
        v["unavailable"] = json!({"reason":"probeFailed"});
    }
    for key in ["resetCredits", "unavailable"] {
        if let Some(value) = p.get(key) {
            v[key] = value.clone();
        }
    }
    Ok(Some(serde_json::from_value(v)?))
}
pub fn resolve_after_probe(
    published: Option<Limits>,
    probed: Option<Limits>,
) -> Result<Option<Limits>, Error> {
    let p = published
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    let mut r = probed
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or(Value::Null);
    if r["unavailable"]["reason"] == "probeFailed"
        && published.is_some()
        && p.get("unavailable").is_none()
    {
        return Ok(published);
    }
    if published.is_some()
        && probed.is_some()
        && p.get("unavailable").is_none()
        && r.get("unavailable").is_none()
        && timestamp_ms(text(&p["checkedAt"])) > timestamp_ms(text(&r["checkedAt"]))
    {
        r["checkedAt"] = p["checkedAt"].clone();
        r["windows"] = p["windows"].clone();
        return Ok(Some(serde_json::from_value(r)?));
    }
    Ok(probed)
}

/// Implementations own authentication, endpoint selection, timeouts and provider CLI/SDK calls.
pub trait UsageLimitProbe: Send + Sync {
    fn read<'a>(&'a self, provider: &'a P, checked_at: &'a str) -> ProbeFuture<'a, Limits>;
}
pub async fn read_provider_limits(
    probe: &dyn UsageLimitProbe,
    provider: &P,
    checked_at: &str,
) -> Result<Limits, Error> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        probe.read(provider, checked_at),
    )
    .await
    {
        Ok(Ok(l)) => Ok(l),
        _ => {
            tracing::warn!("Provider usage limit probe failed");
            unavailable(
                checked_at,
                "probeFailed",
                Some("The provider did not report usage limits."),
            )
        }
    }
}
pub trait LimitSourceProbe: Send + Sync {
    fn accounts<'a>(
        &'a self,
        url: &'a str,
        key: &'a str,
    ) -> ProbeFuture<'a, Vec<UsageLimitSourceAccount>>;
    fn consume<'a>(
        &'a self,
        url: &'a str,
        key: &'a str,
        account: &'a str,
        credit: &'a str,
    ) -> ProbeFuture<'a, ProviderConsumeResetCreditResult>;
}
