//! Subscription limits from provider snapshots and pooled usage sources.
use super::overview::{driver_label, number};
use crate::composition::usage::UsageTarget;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString, provider_usage_limits::*, server::ServerProviderAuthStatus,
};

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Limits {
    pub windows: Vec<Window>,
    pub notices: Vec<String>,
    pub links: Vec<ExternalLink>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalLink {
    pub url: String,
    pub label: String,
    pub message: Option<String>,
    pub accounts: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub driver: String,
    pub label: String,
    pub remaining: f64,
    pub accounts: Vec<Member>,
    pub zero_caps: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub remaining: f64,
    pub capacity: f64,
    pub reset: Option<String>,
    pub restores_percent: f64,
    pub time_remaining: Option<f64>,
    pub plan: Option<String>,
    pub environments: Vec<String>,
    pub source_label: Option<String>,
    pub credits: u64,
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
    plan_at: Option<i64>,
    environments: Vec<(&'a crate::types::EnvironmentId, &'a str)>,
    source_label: Option<String>,
    credit_source: Option<&'a ServerProviderUsageLimits>,
}
fn identity(
    driver: &str,
    email: Option<&str>,
    limits: Option<&ServerProviderUsageLimits>,
) -> Option<(String, String)> {
    let id = email
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("email:{}", s.to_lowercase()))
        .or_else(|| field(&limits?.credential_fingerprint).map(|s| format!("credential:{s}")))?;
    Some((driver.into(), id))
}
impl Account<'_> {
    fn identity(&self) -> Option<(String, String)> {
        identity(self.driver, self.email, Some(self.limits))
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
fn reset_at(w: &ServerProviderUsageWindow) -> Option<i64> {
    crate::time::epoch_ms(&w.resets_at.as_ref()?.as_ref()?.0)
}
struct PoolMember {
    remaining: f64,
    reset: Option<i64>,
    duration: Option<u64>,
    refill_percent: f64,
}
fn pool_member(account: &Account<'_>, window: &ServerProviderUsageWindow, now: i64) -> PoolMember {
    use ServerProviderUsageWindowKind::{Session, Weekly};
    let used = number(&window.used_percent);
    let session = reset_at(window);
    let spent = account.limits.windows.iter().find(|other| {
        window.kind == Session
            && number(&other.used_percent) >= 100.0
            && ((account.driver == "codex" && other.kind == Weekly)
                || (account.driver == "claudeAgent" && other.id.0 == "seven_day"))
    });
    let Some(weekly) = spent else {
        return PoolMember {
            remaining: 100.0 - used.clamp(0.0, 100.0),
            reset: session,
            duration: window.window_duration_mins.flatten(),
            refill_percent: used,
        };
    };
    let clocks = session.zip(reset_at(weekly)).filter(|(s, w)| {
        crate::time::epoch_ms(&account.limits.checked_at.0).is_some_and(|checked| {
            let cutoff = now.max(checked);
            *s > cutoff && *w > cutoff
        })
    });
    let (reset, refill_percent) = match clocks {
        Some((s, w)) if s <= w => (Some(w), 100.0),
        Some((s, _)) if used >= 100.0 => (Some(s), 100.0),
        Some((_, w)) => (Some(w), 100.0 - used.clamp(0.0, 100.0)),
        None => (None, 0.0),
    };
    PoolMember {
        remaining: 0.0,
        reset,
        duration: None,
        refill_percent,
    }
}

fn hub_name(id: &str) -> &str {
    id.get(..id.len().saturating_sub(5))
        .filter(|prefix| id[prefix.len()..].eq_ignore_ascii_case(".json"))
        .unwrap_or(id)
}

pub fn limits(targets: &[UsageTarget], hidden: &BTreeSet<String>, now: u64) -> Limits {
    let mut out = Limits::default();
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    let mut accounts = Vec::new();
    let mut hubs = Vec::new();
    let mut seen = BTreeSet::new();
    let mut workspaces = BTreeMap::<_, BTreeSet<_>>::new();
    for target in targets {
        let Some(config) = &target.config else {
            continue;
        };
        let mut notice = |subject: &str, message: &str| {
            let line = if targets.len() > 1 {
                format!("{} · {subject}: {message}", target.label)
            } else {
                format!("{subject}: {message}")
            };
            if seen.insert(line.clone()) {
                out.notices.push(line);
            }
        };
        for p in &config.providers.0 {
            if hidden.contains(driver_label(&p.driver.0)) {
                continue;
            }
            if p.enabled
                && let Some(workspace) = field(&p.auth.workspace_id)
            {
                let identity = identity(
                    &p.driver.0,
                    field(&p.auth.email),
                    p.usage_limits.as_ref().and_then(Option::as_ref),
                );
                if let Some(key) = identity {
                    workspaces.entry(key).or_default().insert(workspace);
                }
            }
            if !p.enabled
                || !p.installed
                || !crate::view_logic::provider_readiness::is_provider_available(p)
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
                let account = format!("{name} on {}", target.label);
                let index = out
                    .links
                    .iter()
                    .position(|previous| previous.url == link.url.0);
                let entry = if let Some(index) = index {
                    &mut out.links[index]
                } else {
                    out.links.push(ExternalLink {
                        url: link.url.0.clone(),
                        label: String::new(),
                        message: None,
                        accounts: Vec::new(),
                    });
                    out.links.last_mut().expect("just inserted external link")
                };
                entry.label = link.label.0.clone();
                entry.message = l
                    .unavailable
                    .as_ref()
                    .and_then(Option::as_ref)
                    .and_then(|u| field(&u.message))
                    .map(str::to_owned);
                if !entry.accounts.contains(&account) {
                    entry.accounts.push(account);
                }
            }
            if let Some(Some(u)) = &l.unavailable {
                if u.reason == ServerProviderUsageUnavailableReason::ProbeFailed {
                    notice(
                        field(&p.display_name).unwrap_or(&p.driver.0),
                        field(&u.message).unwrap_or("Could not read limits."),
                    );
                }
                continue;
            }
            if l.windows.is_empty() {
                notice(
                    field(&p.display_name).unwrap_or(&p.driver.0),
                    "No limits reported.",
                );
                continue;
            }
            accounts.push(Account {
                key: format!("{}:{}", target.id.0, p.instance_id.0),
                driver: &p.driver.0,
                name: field(&p.display_name)
                    .or(field(&p.auth.email))
                    .unwrap_or(name),
                email: field(&p.auth.email),
                workspace: field(&p.auth.workspace_id),
                plan: field(&p.auth.label),
                limits: l,
                plan_at: field(&p.auth.label).and_then(|_| crate::time::epoch_ms(&l.checked_at.0)),
                environments: vec![(&target.id, &target.label)],
                source_label: None,
                credit_source: l.reset_credits.as_ref().and_then(Option::as_ref).map(|_| l),
            });
        }
        if let Some(Some(sources)) = &config.usage_limit_sources {
            for source in &sources.0 {
                if !source.accounts.is_empty()
                    && source
                        .accounts
                        .iter()
                        .all(|a| hidden.contains(driver_label(&a.driver.0)))
                {
                    continue;
                }
                if let Some(error) = field(&source.error) {
                    notice(&source.label.0, error);
                } else if source.accounts.is_empty() {
                    notice(&source.label.0, "No accounts reported.");
                }
                for a in &source.accounts {
                    if hidden.contains(driver_label(&a.driver.0)) {
                        continue;
                    }
                    if let (Some(key), Some(ws)) = (
                        identity(&a.driver.0, field(&a.email), Some(&a.usage_limits)),
                        field(&a.workspace_id),
                    ) {
                        workspaces.entry(key).or_default().insert(ws);
                    }
                    if a.usage_limits.windows.is_empty()
                        || a.usage_limits
                            .unavailable
                            .as_ref()
                            .is_some_and(Option::is_some)
                    {
                        continue;
                    }
                    hubs.push(Account {
                        key: format!(
                            "{}:{}:{}",
                            source.id.0,
                            a.id.0,
                            field(&a.workspace_id).unwrap_or_default()
                        ),
                        driver: &a.driver.0,
                        name: field(&a.email).unwrap_or_else(|| hub_name(&a.id.0)),
                        email: field(&a.email),
                        workspace: field(&a.workspace_id),
                        plan: field(&a.plan),
                        limits: &a.usage_limits,
                        plan_at: field(&a.plan)
                            .and_then(|_| crate::time::epoch_ms(&a.usage_limits.checked_at.0)),
                        environments: Vec::new(),
                        source_label: Some(if targets.len() > 1 {
                            format!("{} · {}", target.label, source.label.0)
                        } else {
                            source.label.0.clone()
                        }),
                        credit_source: a
                            .usage_limits
                            .reset_credits
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|_| &a.usage_limits),
                    });
                }
            }
        }
    }
    let mut unique = BTreeMap::new();
    for a in accounts.into_iter().chain(hubs) {
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
        use std::collections::btree_map::Entry;
        match unique.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(a);
            }
            Entry::Occupied(mut entry) => {
                let prev = entry.get_mut();
                if a.plan.is_some() && (prev.plan.is_none() || a.plan_at > prev.plan_at) {
                    prev.plan = a.plan;
                    prev.plan_at = a.plan_at;
                }
                if a.environments.is_empty()
                    && prev.name.eq_ignore_ascii_case(driver_label(a.driver))
                    && let Some(email) = a.email
                {
                    prev.name = email;
                }
                for env in a.environments {
                    if !prev.environments.iter().any(|(id, _)| *id == env.0) {
                        prev.environments.push(env);
                    }
                }
                if !prev.environments.is_empty() {
                    prev.source_label = None;
                }
                if let Some(source) = a.credit_source
                    && prev.credit_source.is_none_or(|p| {
                        crate::time::epoch_ms(&source.checked_at.0)
                            > crate::time::epoch_ms(&p.checked_at.0)
                    })
                {
                    prev.credit_source = Some(source);
                }
                if crate::time::epoch_ms(&a.limits.checked_at.0)
                    > crate::time::epoch_ms(&prev.limits.checked_at.0)
                {
                    prev.limits = a.limits;
                }
            }
        }
    }
    let mut pools = BTreeMap::<_, Vec<_>>::new();
    for a in unique.values() {
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
            pools.entry((a.driver, rank, id)).or_default().push((
                a,
                w.label.0.as_str(),
                a.capacity(&w.kind),
                pool_member(a, w, now),
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
            remaining: (members
                .iter()
                .map(|m| weight(m.2) * m.3.remaining)
                .sum::<f64>()
                / capacity)
                .round(),
            accounts: members
                .iter()
                .map(|(account, _, units, member)| {
                    let reset = member.reset.map(|at| at.saturating_sub(now));
                    let time_remaining = reset
                        .zip(member.duration)
                        .filter(|(_, mins)| *mins > 0)
                        .map(|(ms, mins)| {
                            (100.0 * ms as f64 / (mins as f64 * 60_000.0))
                                .clamp(0.0, 100.0)
                                .round()
                        });
                    Member {
                        name: display_name(account.name),
                        plan: account.plan.map(str::to_owned),
                        environments: account
                            .environments
                            .iter()
                            .map(|(_, label)| (*label).to_owned())
                            .collect(),
                        source_label: account.source_label.clone(),
                        credits: account
                            .credit_source
                            .and_then(|l| l.reset_credits.as_ref()?.as_ref())
                            .map_or(0, |c| c.available_count),
                        remaining: member.remaining,
                        restores_percent: (member.refill_percent * weight(*units) / capacity)
                            .round(),
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

    fn target(label: &str, accounts: serde_json::Value) -> UsageTarget {
        let raw = include_str!("../../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .expect("sample config");
        let mut config: serde_json::Value = serde_json::from_str(raw).expect("valid sample");
        config["providers"] = json!([]);
        config["usageLimitSources"] = json!([{"id":"hub","kind":"cliproxy","label":"Hub","checkedAt":"2026-06-20T00:00:00Z","accounts":accounts}]);
        UsageTarget {
            id: crate::types::EnvironmentId(label.into()),
            label: label.into(),
            generation: crate::types::Generation(1),
            connected: true,
            diagnostics: true,
            settings_write: false,
            config: Some(std::rc::Rc::new(
                serde_json::from_value(config).expect("valid test target"),
            )),
        }
    }

    #[test]
    fn blocked_sessions_use_weekly_reset_and_hide_pace() {
        let machine = target(
            "Home",
            json!([{"id":"work","driver":"codex","usageLimits":{"checkedAt":"2026-06-20T01:00:00Z","windows":[
                {"id":"short","kind":"session","label":"Session","usedPercent":20,"resetsAt":"2026-06-20T02:00:00Z","windowDurationMins":300},
                {"id":"long","kind":"weekly","label":"Week","usedPercent":100,"resetsAt":"2026-06-21T00:00:00Z","windowDurationMins":10080}
            ]}}]),
        );
        let now = crate::time::epoch_ms("2026-06-20T00:00:00Z").expect("fixture clock") as u64;
        let data = limits(std::slice::from_ref(&machine), &BTreeSet::new(), now);
        let session = &data.windows[0].accounts[0];
        assert_eq!(session.remaining, 0.0);
        assert_eq!(session.reset.as_deref(), Some("resets in 1d 0h"));
        assert_eq!(session.time_remaining, None);
        assert_eq!(session.restores_percent, 100.);
        let stale = limits(&[machine], &BTreeSet::new(), now + 2 * 3_600_000);
        assert_eq!(stale.windows[0].accounts[0].reset, None);
    }

    #[test]
    fn notices_include_environment_and_deduplicate_in_order() {
        let targets = [
            target("Home", json!([])),
            target("Home", json!([])),
            target("Work", json!([])),
        ];
        assert_eq!(
            limits(&targets, &BTreeSet::new(), 0).notices,
            [
                "Home · Hub: No accounts reported.",
                "Work · Hub: No accounts reported."
            ]
        );
    }

    #[test]
    fn deduplicated_workspaces_use_fresh_usage_and_pooled_plan_capacity() {
        let report = |id: &str,
                      workspace: Option<&str>,
                      plan: Option<&str>,
                      used: u32,
                      checked: &str| json!({"id":id,"driver":"codex","email":"name@example.com","workspaceId":workspace,"plan":plan,"usageLimits":{"checkedAt":checked,"windows":[{"id":"slot","kind":"session","label":"Session","usedPercent":used}]}});
        let old = "2026-06-19T00:00:00Z";
        let new = "2026-06-20T00:00:00Z";
        let first = target(
            "Home",
            json!([report(
                "one",
                Some("org"),
                Some("ChatGPT Pro 100 Subscription"),
                100,
                old
            )]),
        );
        let mut guest = report("guest", None, Some("ChatGPT Plus Subscription"), 100, new);
        guest["email"] = json!("guest@example.com");
        let second = target("Work", json!([report("two", None, None, 40, new), guest]));
        let result = limits(&[first, second], &BTreeSet::new(), 0);
        assert_eq!(result.windows.len(), 1);
        assert_eq!(result.windows[0].accounts.len(), 2);
        assert_eq!(result.windows[0].remaining, 50.0);
    }
    #[test]
    fn suppressed_session_only_restores_unused_share_when_weekly_resets_first() {
        let machine = target(
            "Home",
            json!([{"id":"one","driver":"claudeAgent","usageLimits":{"checkedAt":"2026-06-20T01:00:00Z","windows":[
            {"id":"five_hour","kind":"session","label":"Session","usedPercent":45,"resetsAt":"2026-06-20T03:00:00Z"},
            {"id":"seven_day","kind":"weekly","label":"Weekly","usedPercent":100,"resetsAt":"2026-06-20T02:00:00Z"}]}}]),
        );
        let now = crate::time::epoch_ms("2026-06-20T00:00:00Z").expect("clock") as u64;
        let pooled = limits(std::slice::from_ref(&machine), &BTreeSet::new(), now);
        assert_eq!(pooled.windows[0].accounts[0].restores_percent, 55.0);
        let mut stale = machine;
        let config = std::rc::Rc::make_mut(stale.config.as_mut().expect("fixture config"));
        config
            .usage_limit_sources
            .as_mut()
            .and_then(Option::as_mut)
            .expect("source")
            .0[0]
            .accounts[0]
            .usage_limits
            .checked_at
            .0 = "2026-06-20T02:00:00Z".into();
        let expired = limits(&[stale], &BTreeSet::new(), now);
        assert_eq!(expired.windows[0].accounts[0].reset, None);
        assert_eq!(expired.windows[0].accounts[0].restores_percent, 0.);
    }
    #[test]
    fn external_links_preserve_latest_label_message_and_distinct_accounts() {
        let mut home = target("Home", json!([]));
        let config = std::rc::Rc::make_mut(home.config.as_mut().expect("fixture config"));
        let p = json!({"instanceId":"codex","driver":"codex","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-06-20T00:00:00Z","models":[],"skills":[],"slashCommands":[],"usageLimits":{"windows":[],"checkedAt":"2026-06-20T00:00:00Z","unavailable":{"reason":"unsupported","message":"Visit website"},"externalUsage":{"url":"https://example.com","label":"Old label"}}});
        let mut latest = p.clone();
        latest["usageLimits"]["externalUsage"]["label"] = json!("Latest");
        let mut zero = p.clone();
        zero["instanceId"] = json!("zero");
        zero["usageLimits"] =
            json!({"windows":[],"zeroSpendCap":true,"checkedAt":"2026-06-20T00:00:00Z"});
        config.providers = serde_json::from_value(json!([p, latest, zero])).expect("providers");
        let mut work = home.clone();
        work.label = "Work".into();
        let data = limits(&[home, work], &BTreeSet::new(), 0);
        assert_eq!(data.links.len(), 1);
        assert_eq!(data.links[0].accounts, ["codex on Home", "codex on Work"]);
        assert_eq!(data.links[0].message.as_deref(), Some("Visit website"));
        assert_eq!(data.links[0].label, "Latest");
        assert!(data.windows.is_empty());
        assert!(
            data.notices
                .contains(&"Work · codex: No limits reported.".into())
        );
    }
    #[test]
    fn empty_hub_accounts_are_omitted_silently() {
        let machine = target(
            "Machine",
            json!([
                {"id":"one.JSON","driver":"claudeAgent","usageLimits":{"checkedAt":"2026-06-20T00:00:00Z","windows":[{"id":"extra","kind":"monthly","label":"Monthly","usedPercent":45}]}},
                {"id":"two","driver":"claudeAgent","usageLimits":{"checkedAt":"2026-06-20T00:00:00Z","zeroSpendCap":true,"windows":[]}}
            ]),
        );
        let result = limits(&[machine], &BTreeSet::new(), 0);
        assert_eq!(result.windows.len(), 1);
        assert_eq!(result.windows[0].accounts[0].name, "one");
        assert_eq!(result.windows[0].remaining, 55.0);
        assert!(result.windows[0].zero_caps.is_empty());
        assert!(result.notices.is_empty());
    }
}
