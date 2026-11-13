use crate::pricing::{RateTable, cache_savings_usd, price_usage};
use crate::{
    Error, UsageProviderKind, UsageRecord, UsageSpeed, UsageTokenTotals, empty_totals, iso,
    provider_name, total_tokens,
};
use chrono::{DateTime, FixedOffset, Utc};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use t3_contracts::usage::{UsageBucket, UsageModelFilter, UsageModelSelector};

pub fn resolve_model_aliases(aliases: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (model, target) in aliases {
        let mut seen = HashSet::from([model.as_str()]);
        let mut target = target.as_str();
        while let Some(next) = aliases.get(target) {
            if !seen.insert(target) {
                break;
            }
            target = next;
        }
        if !aliases.contains_key(target) {
            out.insert(model.clone(), target.into());
        }
    }
    out
}
fn selector_key(s: &UsageModelSelector) -> String {
    json!([
        s.provider
            .as_ref()
            .and_then(|v| v.as_ref())
            .map(|v| v.0.as_str()),
        s.model.0.as_str()
    ])
    .to_string()
}
pub fn canonical_model_filter(filter: Option<UsageModelFilter>) -> Option<UsageModelFilter> {
    let mut f = filter?;
    let canonical = |list: Vec<UsageModelSelector>| -> Vec<_> {
        let mut entries: Vec<_> = list
            .into_iter()
            .map(|s| (selector_key(&s), s))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect();
        entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
        entries.into_iter().map(|(_, s)| s).collect()
    };
    f.include = f.include.flatten().map(|l| Some(canonical(l)));
    f.exclude = f
        .exclude
        .flatten()
        .filter(|v| !v.is_empty())
        .map(|l| Some(canonical(l)));
    if f.include.is_none() && f.exclude.is_none() {
        None
    } else {
        Some(f)
    }
}
fn keeps(filter: Option<&UsageModelFilter>, r: &UsageRecord) -> bool {
    let Some(f) = filter else {
        return true;
    };
    let matches = |s: &UsageModelSelector| {
        s.model.0.as_str() == r.model.trim()
            && s.provider
                .as_ref()
                .and_then(|v| v.as_ref())
                .is_none_or(|p| p.0.as_str() == provider_name(&r.provider))
    };
    f.include
        .as_ref()
        .and_then(|v| v.as_ref())
        .is_none_or(|v| v.iter().any(matches))
        && !f
            .exclude
            .as_ref()
            .and_then(|v| v.as_ref())
            .is_some_and(|v| v.iter().any(matches))
}

pub fn merge_usage_copies(a: &mut UsageRecord, b: &UsageRecord) {
    let x = &mut a.totals;
    let y = &b.totals;
    x.uncached_input_tokens = x.uncached_input_tokens.max(y.uncached_input_tokens);
    x.cached_input_tokens = x.cached_input_tokens.max(y.cached_input_tokens);
    x.cache_creation_tokens = x.cache_creation_tokens.max(y.cache_creation_tokens);
    x.output_tokens = x.output_tokens.max(y.output_tokens);
    x.reasoning_tokens = x.reasoning_tokens.max(y.reasoning_tokens);
    let one = a
        .cache_creation1h_tokens
        .unwrap_or(0)
        .max(b.cache_creation1h_tokens.unwrap_or(0));
    if one > 0 {
        a.cache_creation1h_tokens = Some(one);
    }
    a.reported_cost_usd = match (a.reported_cost_usd, b.reported_cost_usd) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
}

pub struct AggregateOptions {
    pub time_zone: String,
    pub since_day: String,
    pub until_day: String,
    pub rates: RateTable,
    pub overrides: RateTable,
    pub aliases: BTreeMap<String, String>,
    pub model_filter: Option<UsageModelFilter>,
    pub hourly_window: Option<(f64, f64)>,
}
impl AggregateOptions {
    pub fn daily(since: &str, until: &str, zone: &str, rates: RateTable) -> Self {
        Self {
            time_zone: zone.into(),
            since_day: since.into(),
            until_day: until.into(),
            rates,
            overrides: RateTable::new(),
            aliases: BTreeMap::new(),
            model_filter: None,
            hourly_window: None,
        }
    }
}
#[derive(Debug)]
pub struct AggregateResult {
    pub buckets: Vec<UsageBucket>,
    pub duplicates_dropped: u64,
    pub out_of_window: u64,
}
struct Contribution {
    record: UsageRecord,
    key: Option<String>,
}
struct Bucket {
    document: BucketDocument,
    sessions: HashSet<String>,
    reported: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryCost {
    input: f64,
    cache_read: f64,
    cache_write: f64,
    output: f64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BucketDocument {
    day: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hour_start: Option<String>,
    provider: UsageProviderKind,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
    totals: UsageTokenTotals,
    cost_usd: f64,
    cache_savings_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    category_cost_usd: Option<CategoryCost>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ultrafast_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed_premium_usd: Option<f64>,
    cost_source: &'static str,
    records: u64,
    unpriced_records: u64,
    unpriced_tokens: u64,
    sessions: usize,
}
pub struct UsageAggregator {
    options: AggregateOptions,
    zone: Option<chrono_tz::Tz>,
    offset: Option<FixedOffset>,
    buckets: IndexMap<String, Bucket>,
    seen: HashMap<String, usize>,
    contributions: Vec<Contribution>,
    duplicates_dropped: u64,
    out_of_window: u64,
}
impl UsageAggregator {
    pub fn new(options: AggregateOptions) -> Self {
        let name = options.time_zone.as_str();
        let zone = name.parse::<chrono_tz::Tz>().ok().or_else(|| {
            chrono_tz::TZ_VARIANTS
                .iter()
                .copied()
                .find(|z| z.name().eq_ignore_ascii_case(name))
        });
        let offset = if name.len() == 6
            && name.is_ascii()
            && b"+-".contains(&name.as_bytes()[0])
            && name.as_bytes()[3] == b':'
            && let (Ok(h), Ok(m)) = (name[1..3].parse::<u32>(), name[4..6].parse::<u32>())
            && h < 24
            && m < 60
        {
            FixedOffset::east_opt(
                (h * 3600 + m * 60) as i32 * if name.starts_with('-') { -1 } else { 1 },
            )
        } else {
            None
        };
        if zone.is_none() && offset.is_none() {
            tracing::warn!("Unknown usage time zone; using UTC");
        }
        Self {
            options,
            zone,
            offset,
            buckets: IndexMap::new(),
            seen: HashMap::new(),
            contributions: vec![],
            duplicates_dropped: 0,
            out_of_window: 0,
        }
    }
    fn day(&self, at: f64) -> String {
        let Some(d) = DateTime::<Utc>::from_timestamp_millis(at as i64) else {
            return String::new();
        };
        if let Some(zone) = self.zone {
            return d.with_timezone(&zone).format("%Y-%m-%d").to_string();
        }
        if let Some(offset) = self.offset {
            return d.with_timezone(&offset).format("%Y-%m-%d").to_string();
        }
        d.format("%Y-%m-%d").to_string()
    }
    pub fn add(&mut self, input: UsageRecord, source: Option<&str>) -> bool {
        if !keeps(self.options.model_filter.as_ref(), &input) {
            return false;
        }
        let mut r = input;
        if let Some(model) = self.options.aliases.get(&r.model) {
            r.model = model.clone();
            r.rate_model = None;
        }
        if let Some(key) = &r.dedupe_key
            && let Some(index) = self.seen.get(key)
        {
            self.duplicates_dropped += 1;
            let held = &mut self.contributions[*index];
            if held.key.is_some() {
                merge_usage_copies(&mut held.record, &r);
            }
            return false;
        }
        let day = self.day(r.timestamp_ms);
        let inside = if let Some((since, until)) = self.options.hourly_window {
            r.timestamp_ms >= since && r.timestamp_ms < until
        } else {
            day >= self.options.since_day && day <= self.options.until_day
        };
        let key = if inside {
            let hour = self.options.hourly_window.map(|(since, _)| {
                iso(since + ((r.timestamp_ms - since) / 3_600_000.0).floor() * 3_600_000.0)
            });
            let source = source.unwrap_or("");
            let key = format!(
                "{day}\0{}\0{}\0{}\0{source}",
                hour.as_deref().unwrap_or(""),
                provider_name(&r.provider),
                r.model
            );
            let bucket = self.buckets.entry(key.clone()).or_insert_with(|| Bucket {
                document: BucketDocument {
                    day,
                    hour_start: hour,
                    provider: r.provider.clone(),
                    model: r.model.clone(),
                    source_path: (!source.is_empty()).then(|| source.to_owned()),
                    totals: empty_totals(),
                    cost_usd: 0.0,
                    cache_savings_usd: 0.0,
                    category_cost_usd: None,
                    fast_cost_usd: None,
                    ultrafast_cost_usd: None,
                    speed_premium_usd: None,
                    cost_source: "unpriced",
                    records: 0,
                    unpriced_records: 0,
                    unpriced_tokens: 0,
                    sessions: 0,
                },
                sessions: HashSet::new(),
                reported: 0,
            });
            if !r.session_id.is_empty() {
                bucket.sessions.insert(r.session_id.clone());
            }
            Some(key)
        } else {
            self.out_of_window += 1;
            None
        };
        if let Some(dedupe) = &r.dedupe_key {
            self.seen.insert(dedupe.clone(), self.contributions.len());
        }
        self.contributions.push(Contribution { record: r, key });
        inside
    }
    pub fn finish(mut self) -> Result<AggregateResult, Error> {
        for Contribution { record: r, key } in self.contributions {
            let Some(bucket) = key.and_then(|k| self.buckets.get_mut(&k)) else {
                continue;
            };
            let priced = price_usage(&self.options.rates, &r, &self.options.overrides);
            let v = &mut bucket.document;
            for (field, n) in [
                (
                    &mut v.totals.uncached_input_tokens,
                    r.totals.uncached_input_tokens,
                ),
                (
                    &mut v.totals.cached_input_tokens,
                    r.totals.cached_input_tokens,
                ),
                (
                    &mut v.totals.cache_creation_tokens,
                    r.totals.cache_creation_tokens,
                ),
                (&mut v.totals.output_tokens, r.totals.output_tokens),
                (&mut v.totals.reasoning_tokens, r.totals.reasoning_tokens),
            ] {
                *field = field.saturating_add(n);
            }
            v.cost_usd += priced.cost_usd;
            v.cache_savings_usd +=
                cache_savings_usd(&self.options.rates, &r, &self.options.overrides);
            if let Some([input, read, write, output]) = priced.category_cost_usd {
                let c = v.category_cost_usd.get_or_insert(CategoryCost {
                    input: 0.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                    output: 0.0,
                });
                c.input += input;
                c.cache_read += read;
                c.cache_write += write;
                c.output += output;
            }
            if r.speed == UsageSpeed::Fast {
                *v.fast_cost_usd.get_or_insert(0.0) += priced.cost_usd;
            }
            if r.speed == UsageSpeed::Ultrafast {
                *v.ultrafast_cost_usd.get_or_insert(0.0) += priced.cost_usd;
            }
            *v.speed_premium_usd.get_or_insert(0.0) += priced.speed_premium_usd;
            v.records += 1;
            if priced.cost_source == "unpriced" {
                v.unpriced_records += 1;
                v.unpriced_tokens = v.unpriced_tokens.saturating_add(total_tokens(&r.totals));
            }
            if priced.cost_source == "providerReported" {
                bucket.reported += 1;
            }
        }
        let mut buckets = vec![];
        for (_, b) in self.buckets {
            let mut v = b.document;
            v.sessions = b.sessions.len();
            v.cost_source = if v.unpriced_records == v.records {
                "unpriced"
            } else if v.records == b.reported {
                "providerReported"
            } else {
                "modelPriced"
            };
            for n in [
                &mut v.fast_cost_usd,
                &mut v.ultrafast_cost_usd,
                &mut v.speed_premium_usd,
            ] {
                *n = n.map(round_usd).filter(|n| *n != 0.0);
            }
            if let Some(c) = v.category_cost_usd.as_mut() {
                for n in [
                    &mut c.input,
                    &mut c.cache_read,
                    &mut c.cache_write,
                    &mut c.output,
                ] {
                    *n = round_usd(*n);
                }
            }
            buckets.push(v);
        }
        buckets.sort_by(|a, b| {
            crate::collation::compare(&a.day, &b.day)
                .then_with(|| {
                    crate::collation::compare(
                        a.hour_start.as_deref().unwrap_or(""),
                        b.hour_start.as_deref().unwrap_or(""),
                    )
                })
                .then_with(|| {
                    crate::collation::compare(
                        provider_name(&a.provider),
                        provider_name(&b.provider),
                    )
                })
                .then_with(|| crate::collation::compare(&a.model, &b.model))
        });
        Ok(AggregateResult {
            buckets: buckets
                .into_iter()
                .map(|b| serde_json::from_value(serde_json::to_value(b)?))
                .collect::<Result<_, _>>()?,
            duplicates_dropped: self.duplicates_dropped,
            out_of_window: self.out_of_window,
        })
    }
}

fn round_usd(n: f64) -> f64 {
    (n * 1e6).round() / 1e6
}
