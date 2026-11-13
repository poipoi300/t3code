#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use t3_usage::{
    UsageRecord,
    aggregation::{
        AggregateOptions, UsageAggregator, canonical_model_filter, resolve_model_aliases,
    },
    cache::{CachedFile, decode_scan_cache, write_scan_cache},
    limits,
    pricing::{
        RateTable, cache_savings_usd, create_override_rate_table, parse_rate_table, price_usage,
    },
    reader::read_transcript,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}
fn fixture(name: &str) -> Value {
    serde_json::from_slice(&fs::read(fixtures().join(name)).unwrap()).unwrap()
}
fn equivalent(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let a = a.as_f64().unwrap();
            let b = b.as_f64().unwrap();
            assert!(
                (a - b).abs() <= 1e-10 * b.abs().max(1.0),
                "{path}: {a} != {b}"
            );
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path} length");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                equivalent(a, b, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.len(), b.len(), "{path} keys: {actual} != {expected}");
            for (k, b) in b {
                equivalent(
                    a.get(k).unwrap_or_else(|| panic!("Missing {path}.{k}")),
                    b,
                    &format!("{path}.{k}"),
                );
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}
#[test]
fn transcript_reader_matches_typescript_records_tails_state_and_guards() {
    for case in fixture("readers.json").as_array().unwrap() {
        let name = case["file"].as_str().unwrap();
        let provider = serde_json::from_value(case["provider"].clone()).unwrap();
        let parsed = read_transcript(&fixtures().join(name), &provider, None).unwrap();
        equivalent(
            &json!({"records":parsed.records,"tailRecords":parsed.tail_records,"malformedRecords":parsed.malformed_records,
            "position":parsed.position,"resumed":parsed.resumed}),
            &case["expected"],
            name,
        );
    }
}
#[test]
fn aggregation_matches_typescript_buckets_filters_aliases_timezones_and_prices() {
    let rates = parse_rate_table(&fixture("rates.json"));
    for case in fixture("aggregation.json").as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let o = &case["options"];
        let mut options = AggregateOptions::daily(
            o["sinceDay"].as_str().unwrap(),
            o["untilDay"].as_str().unwrap(),
            o["timeZone"].as_str().unwrap(),
            rates.clone(),
        );
        options.overrides = create_override_rate_table(&o["overrides"]);
        options.aliases = resolve_model_aliases(
            &serde_json::from_value::<BTreeMap<String, String>>(
                o.get("aliases").cloned().unwrap_or(json!({})),
            )
            .unwrap(),
        );
        options.model_filter = o
            .get("modelFilter")
            .map(|v| serde_json::from_value(v.clone()).unwrap());
        if o["resolution"] == "hour" {
            options.hourly_window = Some((
                o["sinceTimeMs"].as_f64().unwrap(),
                o["untilTimeMs"].as_f64().unwrap(),
            ));
        }
        let mut aggregate = UsageAggregator::new(options);
        let accepted: Vec<_> = case["records"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, r)| {
                aggregate.add(
                    serde_json::from_value(r.clone()).unwrap(),
                    o["sources"][i].as_str(),
                )
            })
            .collect();
        assert_eq!(json!(accepted), case["accepted"], "{name}");
        let result = aggregate.finish().unwrap();
        if name == "category and speed" {
            let bucket = json!(result.buckets)[0].clone();
            assert_eq!(bucket["costUsd"].as_f64().unwrap(), 0.013875000000000002);
            assert_eq!(bucket["fastCostUsd"].as_f64().unwrap(), 0.00925);
            assert_eq!(bucket["speedPremiumUsd"].as_f64().unwrap(), 0.004625);
        }
        equivalent(
            &json!({"buckets":result.buckets,"duplicatesDropped":result.duplicates_dropped,"outOfWindow":result.out_of_window}),
            &case["expected"],
            name,
        );
    }
}
#[test]
fn pricing_matches_typescript_provider_costs_tiers_speeds_and_ambiguous_aliases() {
    for (i, case) in fixture("pricing.json")
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let table = parse_rate_table(&case["document"]);
        let r: UsageRecord = serde_json::from_value(case["record"].clone()).unwrap();
        let priced = price_usage(&table, &r, &RateTable::new());
        let category = priced
            .category_cost_usd
            .map(|[a, b, c, d]| json!({"input":a,"cacheRead":b,"cacheWrite":c,"output":d}));
        equivalent(
            &json!({"costUsd":priced.cost_usd,"costSource":priced.cost_source,"categoryCostUsd":category,"speedPremiumUsd":priced.speed_premium_usd}),
            &case["expected"],
            &format!("pricing {i}"),
        );
        equivalent(
            &json!(cache_savings_usd(&table, &r, &RateTable::new())),
            &case["savings"],
            &format!("savings {i}"),
        );
    }
}
#[test]
fn pricing_hostile_token_totals_cannot_overflow_tier_selection() {
    let cases = fixture("pricing.json");
    let case = &cases[0];
    let table = parse_rate_table(&case["document"]);
    let mut record: UsageRecord = serde_json::from_value(case["record"].clone()).unwrap();
    record.totals.uncached_input_tokens = u64::MAX;
    record.totals.cached_input_tokens = u64::MAX;
    record.totals.cache_creation_tokens = u64::MAX;
    assert!(
        price_usage(&table, &record, &RateTable::new())
            .cost_usd
            .is_finite()
    );
}
#[test]
fn scan_cache_matches_typescript_migrations_validation_and_round_trip() {
    for case in fixture("cache.json").as_array().unwrap() {
        let decoded = decode_scan_cache(&case["document"]);
        let actual: Vec<_> = decoded.iter().map(|(p, e)| json!([p, e])).collect();
        equivalent(
            &json!(actual),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
        let mut encoded = vec![];
        write_scan_cache(&mut encoded, &decoded, &serde_json::Map::new()).unwrap();
        let roundtrip = decode_scan_cache(&serde_json::from_slice::<Value>(&encoded).unwrap());
        assert_eq!(roundtrip, decoded, "roundtrip {}", case["name"]);
        let _: Vec<CachedFile> = decoded.into_values().collect();
    }
}

#[test]
fn legacy_cache_without_reducer_state_keeps_deleted_history() {
    let cases = fixture("cache.json");
    let mut document = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "current")
        .unwrap_or(&cases[0])["document"]
        .clone();
    document["version"] = json!(4);
    for (_, entry) in document["files"].as_object_mut().unwrap() {
        entry.as_object_mut().unwrap().remove("cs");
        entry["p"] = json!("claude");
    }
    let decoded = decode_scan_cache(&document);
    assert!(!decoded.is_empty());
    assert!(
        decoded
            .values()
            .all(|f| f.size == -1 && f.position.resume_offset == 0)
    );
    assert!(decoded.values().any(|f| !f.records.is_empty()));
}

#[test]
fn timezone_names_ignore_ascii_case_and_offset_inputs_never_panic() {
    let cases = fixture("aggregation.json");
    let mut r: UsageRecord = serde_json::from_value(cases[0]["records"][0].clone()).unwrap();
    r.timestamp_ms = t3_usage::timestamp_ms("2026-08-01T02:00:00Z").unwrap();
    let aggregate = |zone: &str| {
        let mut a = UsageAggregator::new(AggregateOptions::daily(
            "2026-01-01",
            "2026-12-31",
            zone,
            RateTable::new(),
        ));
        a.add(r.clone(), None);
        json!(a.finish().unwrap().buckets)
    };
    assert_eq!(aggregate("America/New_York"), aggregate("america/new_york"));
    for zone in ["+éabc", "+aébc", "-éabc"] {
        aggregate(zone);
    }
}

#[test]
fn bucket_sort_uses_locale_collation_and_preserves_source_insertion_for_ties() {
    let cases = fixture("aggregation.json");
    let r: UsageRecord = serde_json::from_value(cases[0]["records"][0].clone()).unwrap();
    let mut a = UsageAggregator::new(AggregateOptions::daily(
        "2026-01-01",
        "2026-12-31",
        "UTC",
        RateTable::new(),
    ));
    for (model, source) in [
        ("z", "same"),
        ("a", "z-source"),
        ("a", "a-source"),
        ("A", "same"),
        ("é", "same"),
    ] {
        let mut next = r.clone();
        next.model = model.into();
        a.add(next, Some(source));
    }
    let buckets = json!(a.finish().unwrap().buckets);
    assert_eq!(
        buckets
            .as_array()
            .unwrap()
            .iter()
            .map(|b| (
                b["model"].as_str().unwrap(),
                b["sourcePath"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            ("a", "z-source"),
            ("a", "a-source"),
            ("A", "same"),
            ("é", "same"),
            ("z", "same")
        ]
    );
}
#[test]
fn canonical_filters_match_typescript() {
    for case in fixture("filters.json").as_array().unwrap() {
        let actual = canonical_model_filter(serde_json::from_value(case["input"].clone()).unwrap());
        assert_eq!(json!(actual), case["expected"]);
    }
}
#[test]
fn provider_limit_windows_match_typescript() {
    for case in fixture("limits.json").as_array().unwrap() {
        let at = case["checkedAt"].as_str().unwrap();
        let input = &case["input"];
        let result = match case["provider"].as_str().unwrap() {
            "codex" => limits::codex_limits(input, None, None, at),
            "claude" => limits::claude_limits(input, at),
            "grok" => limits::grok_limits(input, at),
            "cursor" => limits::cursor_limits(input, at),
            _ => unreachable!(),
        }
        .unwrap();
        equivalent(
            &json!(result),
            &case["expected"],
            case["provider"].as_str().unwrap(),
        );
    }
}
