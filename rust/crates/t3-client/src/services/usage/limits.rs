//! Subscription limits from provider snapshots and pooled usage sources.
use super::overview::{driver_label, number};
use crate::composition::usage::UsageTarget;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    provider_usage_limits::*,
    server::{ServerProviderAuthStatus, ServerProviderAvailability},
};

#[derive(Default, Debug, PartialEq)]
pub struct Limits {
    pub windows: Vec<Window>,
    pub notices: Vec<String>,
    pub links: BTreeMap<String, (String, Vec<String>)>,
}
#[derive(Debug, PartialEq)]
pub struct Window {
    pub driver: String,
    pub label: String,
    pub remaining: f64,
    pub accounts: Vec<Member>,
    pub zero_caps: Vec<String>,
}
#[derive(Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub remaining: f64,
    pub capacity: f64,
    pub reset: Option<String>,
    pub time_remaining: Option<f64>,
}
fn display_name(name: &str) -> String {
    if let Some((local, domain)) = name.split_once('@') {
        let initials: String = local
            .chars()
            .take(1)
            .chain(domain.chars().take(1))
            .collect();
        format!("Account {}", initials.to_uppercase())
    } else {
        name.into()
    }
}
fn reset_label(left: i64) -> String {
    let minutes = left / 60_000;
    if left <= 0 {
        "resets now".into()
    } else if minutes == 0 {
        "Resets momentarily".into()
    } else if minutes >= 1440 {
        format!("resets in {}d {}h", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!("resets in {}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("resets in {minutes}m")
    }
}
fn field(v: &Option<Option<TrimmedNonEmptyString>>) -> Option<&str> {
    v.as_ref()?.as_ref().map(|s| s.0.as_str())
}
struct Account<'a> {
    key: String,
    driver: &'a str,
    name: &'a str,
    email: Option<&'a str>,
    workspace: Option<&'a str>,
    plan: Option<&'a str>,
    limits: &'a ServerProviderUsageLimits,
}
impl Account<'_> {
    fn identity(&self) -> Option<(String, String)> {
        let id = self
            .email
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("email:{}", s.to_lowercase()))
            .or_else(|| {
                field(&self.limits.credential_fingerprint).map(|s| format!("credential:{s}"))
            })?;
        Some((self.driver.into(), id))
    }
    fn capacity(&self, kind: &ServerProviderUsageWindowKind) -> Option<f64> {
        use ServerProviderUsageWindowKind::*;
        match (self.driver, kind, self.plan?) {
            ("codex", Session | Weekly, "ChatGPT Plus Subscription")
            | ("claudeAgent", Session, "Pro") => Some(1.0),
            ("codex", Session | Weekly, "ChatGPT Pro 100 Subscription")
            | ("claudeAgent", Session, "Max") => Some(5.0),
            ("codex", Session | Weekly, "ChatGPT Pro 200 Subscription")
            | ("claudeAgent", Session, "Ultra") => Some(20.0),
            _ => None,
        }
    }
}
pub fn limits(targets: &[UsageTarget], hidden: &BTreeSet<String>, now: u64) -> Limits {
    let mut out = Limits::default();
    let mut accounts = Vec::new();
    for target in targets {
        let Some(config) = &target.config else {
            continue;
        };
        for p in &config.providers.0 {
            if !p.enabled
                || !p.installed
                || matches!(
                    p.availability,
                    Some(Some(ServerProviderAvailability::Unavailable))
                )
                || hidden.contains(driver_label(&p.driver.0))
            {
                continue;
            }
            let Some(Some(l)) = &p.usage_limits else {
                continue;
            };
            let name = field(&p.display_name).unwrap_or(&p.instance_id.0);
            if matches!(p.auth.status, ServerProviderAuthStatus::Authenticated)
                && let Some(Some(link)) = &l.external_usage
            {
                out.links
                    .entry(link.url.0.clone())
                    .or_insert_with(|| (link.label.0.clone(), Vec::new()))
                    .1
                    .push(format!("{name} on {}", target.label));
            }
            accounts.push(Account {
                key: format!("{}:{}", target.id.0, p.instance_id.0),
                driver: &p.driver.0,
                name,
                email: field(&p.auth.email),
                workspace: field(&p.auth.workspace_id),
                plan: field(&p.auth.label),
                limits: l,
            });
        }
        if let Some(Some(sources)) = &config.usage_limit_sources {
            for source in &sources.0 {
                if let Some(error) = field(&source.error) {
                    out.notices.push(format!("{}: {error}", source.label.0));
                } else if source.accounts.is_empty() {
                    out.notices
                        .push(format!("{}: No accounts reported.", source.label.0));
                }
                for a in &source.accounts {
                    if hidden.contains(driver_label(&a.driver.0)) {
                        continue;
                    }
                    accounts.push(Account {
                        key: format!(
                            "{}:{}:{}",
                            source.id.0,
                            a.id.0,
                            field(&a.workspace_id).unwrap_or_default()
                        ),
                        driver: &a.driver.0,
                        name: field(&a.email).unwrap_or(&a.id.0),
                        email: field(&a.email),
                        workspace: field(&a.workspace_id),
                        plan: field(&a.plan),
                        limits: &a.usage_limits,
                    });
                }
            }
        }
    }
    let mut workspaces = BTreeMap::<_, BTreeSet<_>>::new();
    for a in &accounts {
        if let (Some(key), Some(ws)) = (a.identity(), a.workspace) {
            workspaces.entry(key).or_default().insert(ws);
        }
    }
    let mut unique = BTreeMap::new();
    for a in accounts {
        if let Some(Some(u)) = &a.limits.unavailable {
            if u.reason == ServerProviderUsageUnavailableReason::ProbeFailed {
                out.notices.push(format!(
                    "{}: {}",
                    display_name(a.name),
                    field(&u.message).unwrap_or("Could not read limits.")
                ));
            }
            continue;
        }
        if a.limits.windows.is_empty() && a.limits.zero_spend_cap != Some(Some(true)) {
            out.notices
                .push(format!("{}: No limits reported.", display_name(a.name)));
            continue;
        }
        let mut key = (a.driver.to_owned(), a.key.clone(), None);
        if let Some(id) = a.identity() {
            let known = workspaces.get(&id);
            if a.workspace.is_some() || known.is_none_or(|w| w.len() < 2) {
                key = (
                    id.0,
                    id.1,
                    a.workspace
                        .or_else(|| known.and_then(|w| w.first().copied())),
                );
            }
        }
        let current = unique.entry(key).or_insert_with(|| None);
        if current.as_ref().is_none_or(|prev: &Account| {
            crate::time::epoch_ms(&prev.limits.checked_at.0)
                < crate::time::epoch_ms(&a.limits.checked_at.0)
        }) {
            *current = Some(a)
        }
    }
    let mut pools = BTreeMap::<_, Vec<_>>::new();
    for a in unique.values().flatten() {
        let mut seen = BTreeSet::new();
        for w in &a.limits.windows {
            use ServerProviderUsageWindowKind::*;
            let rank = match w.kind {
                Session => 0,
                Weekly => 1,
                Monthly => 2,
                Other => 3,
            };
            let id = if a.driver == "codex" && seen.insert(rank) {
                ""
            } else {
                &w.id.0
            };
            let blocked = w.kind == Session
                && a.limits.windows.iter().any(|other| {
                    number(&other.used_percent) >= 100.0
                        && ((a.driver == "codex" && other.kind == Weekly)
                            || (a.driver == "claudeAgent" && other.id.0 == "seven_day"))
                });
            let left = if blocked {
                0.0
            } else {
                100.0 - number(&w.used_percent).clamp(0.0, 100.0)
            };
            pools.entry((a.driver, rank, id)).or_default().push((
                a.name,
                w.label.0.as_str(),
                a.capacity(&w.kind),
                left,
                w,
            ));
        }
    }
    for ((driver, kind, _), members) in pools {
        let weighted = members.iter().all(|m| m.2.is_some());
        let weight = |n: Option<f64>| if weighted { n.unwrap_or(1.0) } else { 1.0 };
        let capacity: f64 = members.iter().map(|m| weight(m.2)).sum();
        out.windows.push(Window {
            driver: driver_label(driver).into(),
            label: members[0].1.into(),
            zero_caps: unique
                .values()
                .flatten()
                .filter(|a| {
                    kind == 2
                        && a.driver == driver
                        && a.limits.zero_spend_cap == Some(Some(true))
                        && !a
                            .limits
                            .windows
                            .iter()
                            .any(|w| w.kind == ServerProviderUsageWindowKind::Monthly)
                })
                .map(|a| display_name(a.name))
                .collect(),
            remaining: (members.iter().map(|m| weight(m.2) * m.3).sum::<f64>() / capacity).round(),
            accounts: members
                .iter()
                .map(|(name, _, units, left, window)| {
                    let reset = window
                        .resets_at
                        .as_ref()
                        .and_then(Option::as_ref)
                        .and_then(|at| crate::time::epoch_ms(&at.0))
                        .map(|at| at.saturating_sub(i64::try_from(now).unwrap_or(i64::MAX)));
                    let time_remaining = reset
                        .zip(window.window_duration_mins.flatten())
                        .filter(|(_, mins)| *mins > 0)
                        .map(|(ms, mins)| {
                            (100.0 * ms as f64 / (mins as f64 * 60_000.0))
                                .clamp(0.0, 100.0)
                                .round()
                        });
                    Member {
                        name: display_name(name),
                        remaining: *left,
                        capacity: weight(*units) / capacity,
                        reset: reset.map(reset_label),
                        time_remaining,
                    }
                })
                .collect(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zero_monthly_cap_has_a_cell_and_does_not_dilute_the_pool() {
        let raw = include_str!("../../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap();
        let mut config: serde_json::Value = serde_json::from_str(raw).unwrap();
        let checked = "2026-06-20T00:00:00Z";
        config["providers"] = json!([]);
        config["usageLimitSources"] = json!([{
            "id":"hub", "kind":"cliproxy", "label":"Hub", "checkedAt":checked,
            "accounts":[
                {"id":"one", "driver":"claudeAgent", "usageLimits":{"checkedAt":checked,
                    "windows":[{"id":"extra", "kind":"monthly", "label":"Monthly", "usedPercent":45}]}},
                {"id":"two", "driver":"claudeAgent", "usageLimits":{"checkedAt":checked,
                    "zeroSpendCap":true, "windows":[]}}
            ]
        }]);
        let target = UsageTarget {
            id: crate::types::EnvironmentId("machine".into()),
            label: "Machine".into(),
            generation: crate::types::Generation(1),
            connected: true,
            diagnostics: true,
            settings_write: false,
            config: Some(std::rc::Rc::new(serde_json::from_value(config).unwrap())),
        };
        let result = limits(&[target], &BTreeSet::new(), 0);
        assert_eq!(result.windows.len(), 1);
        assert_eq!(result.windows[0].remaining, 55.0);
        assert_eq!(result.windows[0].zero_caps, ["two"]);
    }
}
