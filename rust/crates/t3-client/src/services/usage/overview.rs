//! Aggregation for UsagePage and UsageModelDialog (usageMerge.ts and usageBreakdown.ts).
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{speech::SpeechNumber, usage::*};

pub fn number(value: &SpeechNumber) -> f64 {
    match value {
        SpeechNumber::Finite(v) => v.value(),
        SpeechNumber::NonFinite(_) => 0.0,
    }
}
pub const PROVIDERS: [&str; 8] = [
    "Codex",
    "Claude Code",
    "Grok Build",
    "Cursor",
    "OpenCode",
    "Antigravity",
    "Pi",
    "Muse Code",
];
pub fn provider_label(provider: &UsageProviderKind) -> &'static str {
    match provider {
        UsageProviderKind::Codex => "Codex",
        UsageProviderKind::Claude => "Claude Code",
        UsageProviderKind::Grok => "Grok Build",
        UsageProviderKind::Cursor => "Cursor",
        UsageProviderKind::Opencode => "OpenCode",
        UsageProviderKind::Antigravity => "Antigravity",
        UsageProviderKind::Pi => "Pi",
        UsageProviderKind::Muse => "Muse Code",
    }
}
pub fn driver_label(driver: &str) -> &str {
    match driver {
        "codex" => "Codex",
        "claudeAgent" => "Claude Code",
        "grok" => "Grok Build",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        "antigravity" => "Antigravity",
        "pi" => "Pi",
        "muse" => "Muse Code",
        other => other,
    }
}
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Figures {
    pub cost: f64,
    pub tokens: [f64; 4],
    pub costs: [f64; 4],
    pub savings: f64,
    pub fast: f64,
    pub ultrafast: f64,
    pub premium: f64,
    pub records: f64,
    pub unpriced_records: f64,
    pub unpriced_tokens: f64,
}
impl Figures {
    pub fn total_tokens(&self) -> f64 {
        self.tokens.iter().sum()
    }
    pub fn cost_unknown(&self) -> bool {
        self.records > 0.0 && self.unpriced_records >= self.records
    }
    pub fn cache_hit(&self) -> Option<f64> {
        let input = self.tokens[..3].iter().sum::<f64>();
        (input > 0.0).then(|| self.tokens[1] / input)
    }
    pub fn per_million(&self) -> Option<f64> {
        let priced = self.total_tokens() - self.unpriced_tokens;
        (priced > 0.0 && !self.cost_unknown()).then(|| self.cost / priced * 1e6)
    }
    fn add(&mut self, b: &UsageBucket) {
        let t = &b.totals;
        let counts = [
            t.uncached_input_tokens,
            t.cached_input_tokens,
            t.cache_creation_tokens,
            t.output_tokens,
        ]
        .map(|n| n as f64);
        for (v, n) in self.tokens.iter_mut().zip(counts) {
            *v += n
        }
        self.cost += number(&b.cost_usd);
        self.savings += number(&b.cache_savings_usd);
        self.fast += optional(&b.fast_cost_usd);
        self.ultrafast += optional(&b.ultrafast_cost_usd);
        self.premium += optional(&b.speed_premium_usd);
        self.records += b.records as f64;
        self.unpriced_records += b.unpriced_records as f64;
        self.unpriced_tokens += b
            .unpriced_tokens
            .flatten()
            .map(|n| n as f64)
            .unwrap_or_else(|| {
                if b.records == 0 {
                    0.0
                } else {
                    counts.iter().sum::<f64>() * b.unpriced_records as f64 / b.records as f64
                }
            });
        if let Some(Some(c)) = &b.category_cost_usd {
            for (v, n) in
                self.costs
                    .iter_mut()
                    .zip([&c.input, &c.cache_read, &c.cache_write, &c.output])
            {
                *v += number(n)
            }
        }
    }
}
fn optional(v: &Option<Option<SpeechNumber>>) -> f64 {
    v.as_ref().and_then(Option::as_ref).map_or(0.0, number)
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Overview {
    pub totals: Figures,
    pub models: BTreeMap<(&'static str, String), Figures>,
    pub providers: BTreeMap<&'static str, Figures>,
    pub sessions: BTreeMap<&'static str, u64>,
    pub periods: BTreeMap<String, BTreeMap<&'static str, Figures>>,
    pub notices: Vec<String>,
}
impl Overview {
    fn add(&mut self, b: &UsageBucket, hourly: bool) {
        let provider = provider_label(&b.provider);
        self.totals.add(b);
        self.models
            .entry((provider, b.model.0.clone()))
            .or_default()
            .add(b);
        self.providers.entry(provider).or_default().add(b);
        let period = if hourly {
            b.hour_start
                .as_ref()
                .and_then(Option::as_ref)
                .map(|s| s.0.as_str())
        } else {
            Some(b.day.0.as_str())
        };
        if let Some(period) = period {
            self.periods
                .entry(period.to_owned())
                .or_default()
                .entry(provider)
                .or_default()
                .add(b);
        }
    }
}
pub struct Report<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub summary: &'a UsageSummary,
}
pub fn merge(
    reports: &[Report<'_>],
    hidden: &BTreeSet<String>,
    model: Option<(&str, &str)>,
    hourly: bool,
) -> Overview {
    let mut out = Overview::default();
    let mut claims = BTreeMap::new();
    let mut malformed = BTreeMap::new();
    let mut messages = BTreeSet::new();
    for report in reports {
        let summary = report.summary;
        let v = number(&summary.contract_version);
        if !(USAGE_MERGE_COMPATIBLE_SINCE as f64..=USAGE_CONTRACT_VERSION as f64).contains(&v) {
            let message = if v < USAGE_MERGE_COMPATIBLE_SINCE as f64 {
                format!(
                    "{} runs an older server version and is excluded from totals.",
                    report.label
                )
            } else {
                format!(
                    "This client is older than the server on {}; its usage is excluded from totals.",
                    report.label
                )
            };
            out.notices.push(message);
            continue;
        }
        for s in &summary.sources {
            let f = &s.fingerprint;
            let provider = provider_label(&f.provider);
            if hidden.contains(provider) {
                continue;
            }
            let shared = (
                f.host_id.0.as_str(),
                provider,
                f.resolved_home_path.0.as_str(),
                f.volume_id.as_str(),
            );
            if matches!(
                s.status,
                UsageSourceStatus::Partial | UsageSourceStatus::Failed
            ) || provider == "Cursor"
                || (s.status == UsageSourceStatus::Missing && s.malformed_records > 0)
            {
                let mut message = s.message.as_ref().map_or("", |m| m.0.as_str());
                if s.malformed_records > 0 {
                    let n = malformed.entry(shared).or_insert(0);
                    *n = (*n).max(s.malformed_records);
                    let warning = malformed_warning(s.malformed_records);
                    message = message.strip_suffix(&warning).unwrap_or(message).trim();
                }
                if !message.is_empty() {
                    messages.insert(message.to_owned());
                }
            }
            if s.status == UsageSourceStatus::Missing {
                continue;
            }
            claims
                .entry(shared)
                .or_insert_with(Vec::new)
                .push((report, s));
        }
    }
    let mut included_buckets = BTreeSet::new();
    for ((_, provider, path, _), mut scans) in claims {
        scans.sort_by_key(|(r, s)| {
            let status = match s.status {
                UsageSourceStatus::Ok => 0,
                UsageSourceStatus::Partial => 1,
                _ => 2,
            };
            (
                status,
                std::cmp::Reverse(crate::time::epoch_ms(&r.summary.read_at)),
                r.id,
            )
        });
        let (owner, source) = scans[0];
        let mut sessions = source.distinct_sessions;
        let mut cells = BTreeSet::new();
        for (i, (report, s)) in scans.into_iter().enumerate() {
            if i > 0 {
                out.notices.push(format!(
                    "Duplicate source counted once: {} · {}",
                    report.label, path
                ));
                if s.status != UsageSourceStatus::Partial
                    || source.status != UsageSourceStatus::Ok
                    || report.summary.model_filter != owner.summary.model_filter
                    || crate::time::epoch_ms(&report.summary.read_at)
                        <= crate::time::epoch_ms(&owner.summary.read_at)
                {
                    continue;
                }
            }
            let sole = report
                .summary
                .sources
                .iter()
                .filter(|s| provider_label(&s.fingerprint.provider) == provider)
                .count()
                == 1;
            let mut contributed = false;
            for (index, b) in report.summary.buckets.iter().enumerate() {
                if provider_label(&b.provider) != provider
                    || !b
                        .source_path
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map_or(i == 0 || sole, |p| p.0 == path)
                    || model.is_some_and(|m| m != (provider, b.model.0.as_str()))
                {
                    continue;
                }
                let cell = (
                    &b.day.0,
                    b.hour_start.as_ref().and_then(Option::as_ref).map(|h| &h.0),
                    &b.model.0,
                );
                let attributed = b.source_path.as_ref().is_some_and(Option::is_some) || sole;
                if attributed && !cells.insert(cell) && i > 0 {
                    continue;
                }
                if included_buckets.insert((report.id, index)) {
                    out.add(b, hourly);
                }
                contributed = true;
            }
            if contributed {
                sessions = sessions.max(s.distinct_sessions)
            }
        }
        let count = out.sessions.entry(provider).or_default();
        *count = count.saturating_add(sessions);
    }
    let skipped = malformed.values().copied().fold(0_u64, u64::saturating_add);
    if skipped > 0 {
        messages.insert(malformed_warning(skipped));
    }
    out.notices.extend(messages);
    out
}
fn malformed_warning(count: u64) -> String {
    format!(
        "{count} malformed or inconsistent usage record(s) were skipped; usage may be incomplete."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pathless_legacy_bucket_counts_once_across_owned_directories() {
        let source = |path: &str| {
            json!({
                "fingerprint": {"hostId":"host","provider":"codex","resolvedHomePath":path,"volumeId":"disk"},
                "status":"ok", "scannedFiles":1, "skippedFiles":0, "malformedRecords":0, "distinctSessions":1, "message":null
            })
        };
        let summary: UsageSummary = serde_json::from_value(json!({
            "contractVersion":6, "readAt":"2026-06-20T00:00:00Z", "timeZone":"UTC", "sinceDay":"2026-06-01", "untilDay":"2026-06-20",
            "sources":[source("/a"), source("/b")],
            "buckets":[{"provider":"codex","day":"2026-06-20","model":"local","costUsd":3.5,"cacheSavingsUsd":0,"costSource":"modelPriced","records":1,"unpricedRecords":0,"sessions":1,"totals":{"uncachedInputTokens":50,"cachedInputTokens":30,"cacheCreationTokens":0,"outputTokens":20,"reasoningTokens":10}}],
            "pricing":{"status":"fresh","source":"fixture","knownModels":1,"fetchedAt":null},"scanDurationMs":1
        })).unwrap();
        let reports = [
            Report {
                id: "a",
                label: "First",
                summary: &summary,
            },
            Report {
                id: "b",
                label: "Duplicate",
                summary: &summary,
            },
        ];
        let data = merge(&reports, &Default::default(), None, false);
        assert_eq!(data.totals.total_tokens(), 100.0);
        assert_eq!(data.totals.cost, 3.5);
        assert_eq!(data.sessions["Codex"], 2);
    }
}
