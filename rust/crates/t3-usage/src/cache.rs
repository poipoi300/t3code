use crate::{
    UsageProviderKind as P, UsageRecord, UsageSpeed, UsageTokenTotals,
    aggregation::merge_usage_copies,
    reader::{GUARD_LENGTH, ParsePosition, ScanState},
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, Write},
};

pub const SCAN_CACHE_FILE_NAME: &str = "usage-scan-cache-v16.json";
pub const LEGACY_SCAN_CACHE_FILE_NAMES: [&str; 12] = [
    "usage-scan-cache-v15.json",
    "usage-scan-cache-v14.json",
    "usage-scan-cache-v13.json",
    "usage-scan-cache-v12.json",
    "usage-scan-cache-v11.json",
    "usage-scan-cache-v10.json",
    "usage-scan-cache-v9.json",
    "usage-scan-cache-v8.json",
    "usage-scan-cache-v7.json",
    "usage-scan-cache-v6.json",
    "usage-scan-cache-v5.json",
    "usage-scan-cache.json",
];

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedFile {
    pub size: i64,
    pub mtime_ms: f64,
    pub provider: P,
    pub records: Vec<UsageRecord>,
    pub tail_records: Vec<UsageRecord>,
    pub malformed_records: u64,
    pub position: ParsePosition,
}
pub type ScanCache = IndexMap<String, CachedFile>;
pub const CURSOR_ACCOUNT_CACHE_FILE_NAME: &str = "usage-cursor-account-cache-v2.json";
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorAccountCache {
    pub account_key: String,
    pub since_ms: f64,
    pub until_ms: f64,
    pub fetched_at_ms: f64,
    pub records: Vec<UsageRecord>,
}
pub fn decode_cursor_account_caches(document: &Value) -> IndexMap<String, CursorAccountCache> {
    let mut caches = IndexMap::new();
    if document["version"] != 2 {
        return caches;
    }
    let Some(strings) = document["strings"].as_array().filter(|s| {
        s.len() <= crate::safety::MAX_ENTRIES
            && s.iter()
                .all(|v| v.as_str().is_some_and(|s| s.len() <= 4096))
    }) else {
        return caches;
    };
    let Some(accounts) = document["accounts"].as_object() else {
        return caches;
    };
    let string_at = |v: &Value| {
        strings
            .get(safe_uint(v)? as usize)?
            .as_str()
            .map(str::to_owned)
    };
    for (key, account) in accounts.iter().take(crate::safety::MAX_ENTRIES) {
        let decoded = (|| {
            let account_key = account["accountKey"].as_str()?.to_owned();
            let since_ms = finite(&account["sinceMs"])?;
            let until_ms = finite(&account["untilMs"])?;
            let fetched_at_ms = finite(&account["fetchedAtMs"])?;
            let rows = account["records"]
                .as_array()
                .filter(|r| r.len() <= crate::safety::MAX_RECORDS)?;
            let mut records = Vec::with_capacity(rows.len());
            for row in rows {
                let row = row.as_array().filter(|r| r.len() >= 9)?;
                let model = string_at(&row[1])?;
                let base = model.strip_prefix("cursor-").unwrap_or(&model);
                let base = base.strip_suffix("-fast").unwrap_or(base);
                let base = ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
                    .iter()
                    .find_map(|suffix| base.strip_suffix(&format!("-{suffix}")))
                    .unwrap_or(base);
                let base = base.strip_suffix("-thinking").unwrap_or(base);
                let rate_model = if base.starts_with("grok-") {
                    format!("xai/{base}")
                } else {
                    base.to_owned()
                };
                let reported_cost_usd = if row[7].is_null() {
                    None
                } else {
                    Some(finite(&row[7])?)
                };
                let dedupe_key = if row[8].is_null() {
                    None
                } else {
                    Some(row[8].as_str()?.to_owned())
                };
                records.push(UsageRecord {
                    provider: P::Cursor,
                    timestamp_ms: finite(&row[0])?,
                    model,
                    rate_model: Some(rate_model),
                    session_id: string_at(&row[2])?,
                    totals: UsageTokenTotals {
                        uncached_input_tokens: safe_uint(&row[3])?,
                        cached_input_tokens: safe_uint(&row[4])?,
                        cache_creation_tokens: safe_uint(&row[5])?,
                        output_tokens: safe_uint(&row[6])?,
                        reasoning_tokens: 0,
                    },
                    cache_creation1h_tokens: None,
                    reported_cost_usd,
                    speed: UsageSpeed::Standard,
                    unresolved_service_tier_turn_id: None,
                    dedupe_key,
                });
            }
            Some(CursorAccountCache {
                account_key,
                since_ms,
                until_ms,
                fetched_at_ms,
                records,
            })
        })();
        if let Some(cache) = decoded {
            caches.insert(key.clone(), cache);
        }
    }
    caches
}
pub fn write_cursor_account_caches(
    output: impl Write,
    caches: &IndexMap<String, CursorAccountCache>,
) -> io::Result<()> {
    let mut strings = IndexMap::new();
    let mut accounts = serde_json::Map::new();
    for (key, cache) in caches {
        let records: Vec<_> = cache
            .records
            .iter()
            .map(|r| {
                json!([
                    r.timestamp_ms,
                    intern(&mut strings, &r.model),
                    intern(&mut strings, &r.session_id),
                    r.totals.uncached_input_tokens,
                    r.totals.cached_input_tokens,
                    r.totals.cache_creation_tokens,
                    r.totals.output_tokens,
                    r.reported_cost_usd,
                    r.dedupe_key
                ])
            })
            .collect();
        accounts.insert(key.clone(),json!({"accountKey":cache.account_key,"sinceMs":cache.since_ms,"untilMs":cache.until_ms,"fetchedAtMs":cache.fetched_at_ms,"records":records}));
    }
    serde_json::to_writer(
        output,
        &json!({"version":2,"strings":strings.into_keys().collect::<Vec<_>>(),"accounts":accounts}),
    )
    .map_err(io::Error::other)
}

fn intern(values: &mut IndexMap<String, usize>, value: &str) -> usize {
    let next = values.len();
    *values.entry(value.into()).or_insert(next)
}
pub fn write_scan_cache(
    mut output: impl Write,
    cache: &ScanCache,
    extra: &serde_json::Map<String, Value>,
) -> io::Result<()> {
    let mut models = IndexMap::new();
    let mut sessions = IndexMap::new();
    output.write_all(b"{\"files\":{")?;
    for (i, (path, entry)) in cache.iter().enumerate() {
        if i > 0 {
            output.write_all(b",")?;
        }
        serde_json::to_writer(&mut output, path)?;
        output.write_all(b":")?;
        let mut encode = |r: &UsageRecord| {
            let mut row = vec![
                json!(r.timestamp_ms),
                json!(intern(&mut models, &r.model)),
                json!(intern(&mut sessions, &r.session_id)),
                json!(r.totals.uncached_input_tokens),
                json!(r.totals.cached_input_tokens),
                json!(r.totals.cache_creation_tokens),
                json!(r.totals.output_tokens),
                json!(r.totals.reasoning_tokens),
                json!(r.dedupe_key),
                json!(r.reported_cost_usd),
                json!(match r.speed {
                    UsageSpeed::Standard => 0,
                    UsageSpeed::Fast => 1,
                    UsageSpeed::Ultrafast => 2,
                }),
                json!(r.rate_model.as_ref().map(|m| intern(&mut models, m))),
            ];
            if r.cache_creation1h_tokens.is_some() || r.unresolved_service_tier_turn_id.is_some() {
                row.push(json!(r.unresolved_service_tier_turn_id));
            }
            if let Some(n) = r.cache_creation1h_tokens {
                row.push(json!(n));
            }
            row
        };
        let records: Vec<_> = entry.records.iter().map(&mut encode).collect();
        let tails: Vec<_> = entry.tail_records.iter().map(&mut encode).collect();
        serde_json::to_writer(
            &mut output,
            &json!({"s":entry.size,"m":entry.mtime_ms,"p":entry.provider,"r":records,"t":tails,"mr":entry.malformed_records,
            "o":entry.position.resume_offset,"gl":entry.position.guard_length,"gh":entry.position.guard_hash,"cs":entry.position.state}),
        )?;
    }
    output.write_all(b"}")?;
    for (key, value) in extra {
        if ["files", "models", "sessions", "version"].contains(&key.as_str()) {
            continue;
        }
        output.write_all(b",")?;
        serde_json::to_writer(&mut output, key)?;
        output.write_all(b":")?;
        serde_json::to_writer(&mut output, value)?;
    }
    output.write_all(b",\"version\":16,\"models\":")?;
    serde_json::to_writer(&mut output, &models.into_keys().collect::<Vec<_>>())?;
    output.write_all(b",\"sessions\":")?;
    serde_json::to_writer(&mut output, &sessions.into_keys().collect::<Vec<_>>())?;
    output.write_all(b"}")
}

fn finite(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}
fn safe_uint(v: &Value) -> Option<u64> {
    finite(v)
        .filter(|n| *n >= 0.0 && *n <= 9_007_199_254_740_991.0 && n.fract() == 0.0)
        .map(|n| n as u64)
}
pub fn decode_scan_cache(document: &Value) -> ScanCache {
    let mut out = ScanCache::new();
    let Some(version) = document["version"]
        .as_u64()
        .filter(|v| (4..=16).contains(v))
    else {
        return out;
    };
    let Some(models) = document["models"].as_array().filter(|v| {
        v.len() <= crate::safety::MAX_ENTRIES
            && v.iter()
                .all(|s| s.as_str().is_some_and(|s| s.len() <= 4096))
    }) else {
        return out;
    };
    let Some(sessions) = document["sessions"].as_array().filter(|v| {
        v.len() <= crate::safety::MAX_ENTRIES
            && v.iter()
                .all(|s| s.as_str().is_some_and(|s| s.len() <= 4096))
    }) else {
        return out;
    };
    let Some(files) = document["files"].as_object() else {
        return out;
    };
    let mut remaining = crate::safety::MAX_SCAN_RETAINED_BYTES as usize;
    for (path, v) in files.iter().take(crate::safety::MAX_ENTRIES) {
        let previous_remaining = remaining;
        let entry = (|| {
            let size = finite(&v["s"])? as i64;
            let mtime = finite(&v["m"])?;
            let provider: P = serde_json::from_value(v["p"].clone()).ok()?;
            if !matches!(provider, P::Claude | P::Codex | P::Grok | P::Pi | P::Muse) {
                return None;
            }
            let malformed = v.get("mr").map(safe_uint).unwrap_or(Some(0))?;
            let offset = safe_uint(&v["o"])?;
            let length = safe_uint(&v["gl"])? as usize;
            let hash = finite(&v["gh"])? as u32;
            if length > GUARD_LENGTH || length as u64 > offset {
                return None;
            }
            let legacy = (provider == P::Claude && version < 13)
                || (matches!(provider, P::Codex | P::Pi)
                    && (version < 6
                        || (provider == P::Codex
                            && (version < 14 || (version < 15 && malformed > 0)))));
            let state = if legacy { &Value::Null } else { v.get("cs")? };
            let state_size = serde_json::to_vec(state).ok()?.len();
            if state_size > crate::safety::MAX_STATE_BYTES {
                return None;
            }
            remaining = remaining.checked_sub(state_size + path.len())?;
            if !legacy && !state.is_null() && ScanState::decode(&provider, state).is_none() {
                return None;
            }
            let mut decode = |rows: &Value| -> Option<Vec<UsageRecord>> {
                let mut records = vec![];
                let rows = rows.as_array()?;
                if rows.len() > crate::safety::MAX_RECORDS {
                    return None;
                }
                for row in rows {
                    let row = row.as_array()?;
                    if row.len() < 11 {
                        return None;
                    }
                    let at = finite(&row[0])?;
                    let model = models.get(safe_uint(&row[1])? as usize)?.as_str()?;
                    let rate = if row.get(11).is_none_or(Value::is_null) {
                        None
                    } else {
                        Some(
                            models
                                .get(safe_uint(&row[11])? as usize)?
                                .as_str()?
                                .to_string(),
                        )
                    };
                    let unresolved = match row.get(12) {
                        None | Some(Value::Null) => None,
                        Some(Value::String(s)) => Some(s.clone()),
                        _ => return None,
                    };
                    let one_hour = row.get(13).map(safe_uint).unwrap_or(Some(0))?;
                    let speed = match safe_uint(&row[10])? {
                        0 => UsageSpeed::Standard,
                        1 => UsageSpeed::Fast,
                        2 => UsageSpeed::Ultrafast,
                        _ => return None,
                    };
                    let totals = UsageTokenTotals {
                        uncached_input_tokens: safe_uint(&row[3])?,
                        cached_input_tokens: safe_uint(&row[4])?,
                        cache_creation_tokens: safe_uint(&row[5])?,
                        output_tokens: safe_uint(&row[6])?,
                        reasoning_tokens: safe_uint(&row[7])?,
                    };
                    if provider == P::Claude && model == "<synthetic>" {
                        continue;
                    }
                    let record = UsageRecord {
                        provider: provider.clone(),
                        timestamp_ms: at,
                        model: model.into(),
                        rate_model: rate,
                        session_id: row[2]
                            .as_u64()
                            .and_then(|i| sessions.get(i as usize))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        totals,
                        cache_creation1h_tokens: (one_hour > 0).then_some(one_hour),
                        reported_cost_usd: finite(&row[9]),
                        speed,
                        unresolved_service_tier_turn_id: if version >= 7 {
                            unresolved
                        } else {
                            None
                        },
                        dedupe_key: row[8].as_str().map(str::to_string),
                    };
                    remaining = remaining.checked_sub(record.retained_size())?;
                    records.push(record);
                }
                Some(records)
            };
            Some(CachedFile {
                size: if legacy { -1 } else { size },
                mtime_ms: mtime,
                provider: provider.clone(),
                records: decode(&v["r"])?,
                tail_records: decode(&v["t"])?,
                malformed_records: malformed,
                position: if legacy || (provider == P::Codex && version < 16) {
                    ParsePosition::default()
                } else {
                    ParsePosition {
                        resume_offset: offset,
                        guard_length: length,
                        guard_hash: hash,
                        state: state.clone(),
                    }
                },
            })
        })();
        if let Some(entry) = entry {
            out.insert(path.clone(), entry);
        } else {
            remaining = previous_remaining;
        }
    }
    out
}
pub fn prune_scan_cache(cache: &mut ScanCache, cutoff: f64) -> usize {
    let before = cache.len();
    cache.retain(|_, f| f.mtime_ms >= cutoff);
    before - cache.len()
}
pub fn dedupe_within_file(records: impl IntoIterator<Item = UsageRecord>) -> Vec<UsageRecord> {
    let mut kept: Vec<UsageRecord> = vec![];
    let mut indexes = HashMap::new();
    for r in records {
        if let Some(key) = &r.dedupe_key {
            if let Some(i) = indexes.get(key) {
                merge_usage_copies(&mut kept[*i], &r);
                continue;
            }
            indexes.insert(key.clone(), kept.len());
        }
        kept.push(r);
    }
    kept
}
