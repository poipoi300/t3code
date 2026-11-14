#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
use t3_contracts::usage::UsageSummaryInput;
use t3_usage::{
    Error, UsageProviderKind as P, UsageSpeed,
    cache::SCAN_CACHE_FILE_NAME,
    service::{Clock, RateFetcher, ServiceConfig, UsageService, UsageSettings},
    sources::{
        ExternalSource, HashMapTiers, ProbeFuture, ProviderInstance, ProviderUsageProbe, Source,
        TierRequest,
    },
    timestamp_ms,
};
struct TestClock(AtomicU64);
impl TestClock {
    fn new() -> Self {
        Self(AtomicU64::new(
            timestamp_ms("2026-08-10T12:00:00Z").unwrap() as u64,
        ))
    }
}
impl Clock for TestClock {
    fn now_ms(&self) -> f64 {
        self.0.load(Ordering::Relaxed) as f64
    }
}
struct Rates {
    calls: AtomicUsize,
    document: Value,
    fail: bool,
}
struct PendingRates(AtomicUsize);
impl RateFetcher for PendingRates {
    fn fetch<'a>(&'a self, _: &'a str, _: &'a str) -> ProbeFuture<'a, Value> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Box::pin(std::future::pending())
    }
}
#[tokio::test(start_paused = true)]
async fn stalled_rate_fetches_time_out_and_keep_the_retry_floor() {
    let temp = tempfile::tempdir().unwrap();
    let fetcher = Arc::new(PendingRates(AtomicUsize::new(0)));
    let s = UsageService::with_dependencies(
        config(&temp),
        Arc::new(TestClock::new()),
        fetcher.clone(),
        Arc::new(t3_usage::sources::NoProviderProbe),
    )
    .await
    .unwrap();
    let result = json!(s.refresh_rates().await.unwrap());
    assert_eq!(result["status"], "unavailable");
    assert_eq!(fetcher.0.load(Ordering::Relaxed), 2);
    s.refresh_rates().await.unwrap();
    assert_eq!(fetcher.0.load(Ordering::Relaxed), 2);
}
impl RateFetcher for Rates {
    fn fetch<'a>(&'a self, _: &'a str, _: &'a str) -> ProbeFuture<'a, Value> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(async {
            if self.fail {
                Err(Error::Probe("offline".into()))
            } else {
                Ok(self.document.clone())
            }
        })
    }
}
fn rates() -> Arc<Rates> {
    Arc::new(Rates {
        calls: AtomicUsize::new(0),
        document: serde_json::from_slice(&fs::read(fixture("rates.json")).unwrap()).unwrap(),
        fail: false,
    })
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}
fn config(temp: &tempfile::TempDir) -> ServiceConfig {
    ServiceConfig::new(
        temp.path().join("base"),
        temp.path().join("base/state"),
        temp.path().join("fixture-home"),
        "usage-test-host".into(),
    )
}
fn window() -> UsageSummaryInput {
    serde_json::from_value(
        json!({"timeZone":"UTC","sinceDay":"2026-07-31","untilDay":"2026-08-02"}),
    )
    .unwrap()
}
fn copy_transcript(temp: &tempfile::TempDir, name: &str) -> PathBuf {
    let dir = temp.path().join("transcripts");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::copy(fixture(name), &path).unwrap();
    path
}
fn output(v: &Value) -> u64 {
    v["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["totals"]["outputTokens"].as_u64().unwrap())
        .sum()
}
async fn service(conf: ServiceConfig) -> UsageService {
    UsageService::with_dependencies(
        conf,
        Arc::new(TestClock::new()),
        rates(),
        Arc::new(t3_usage::sources::NoProviderProbe),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn unchanged_reads_do_not_rewrite_cache_or_rates() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let s = service(conf.clone()).await;
    s.read_summary(window()).await.unwrap();
    let sentinel = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    for name in [SCAN_CACHE_FILE_NAME, "usage-model-rates.json"] {
        fs::File::options()
            .write(true)
            .open(conf.state_dir.join(name))
            .unwrap()
            .set_modified(sentinel)
            .unwrap();
    }
    for provisional in [true, false] {
        let mut input = window();
        input.provisional = Some(Some(provisional));
        s.read_summary(input).await.unwrap();
        for name in [SCAN_CACHE_FILE_NAME, "usage-model-rates.json"] {
            assert_eq!(
                fs::metadata(conf.state_dir.join(name))
                    .unwrap()
                    .modified()
                    .unwrap(),
                sentinel,
                "{name}, provisional={provisional}"
            );
        }
    }
}

#[tokio::test]
async fn cache_with_node_file_times_is_reused_against_real_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let cases: Value =
        serde_json::from_slice(&fs::read(fixture("node-mtimes.json")).unwrap()).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["sec"] == 1_788_480_000 && c["nsec"] == 123_456_700)
        .unwrap();
    let modified = std::time::UNIX_EPOCH
        + std::time::Duration::new(
            case["sec"].as_u64().unwrap(),
            case["nsec"].as_u64().unwrap() as u32,
        );
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let warm = service(conf.clone()).await;
    let first = json!(warm.read_summary(window()).await.unwrap());
    drop(warm);
    let cache_path = conf.state_dir.join(SCAN_CACHE_FILE_NAME);
    let mut cache: Value = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
    for entry in cache["files"].as_object_mut().unwrap().values_mut() {
        entry["m"] = case["milliseconds"].clone();
    }
    fs::write(&cache_path, serde_json::to_vec(&cache).unwrap()).unwrap();
    // Same metadata with invalid bytes distinguishes a cache hit from a fresh parse.
    fs::write(
        &path,
        vec![b' '; fs::metadata(&path).unwrap().len() as usize],
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let next = json!(service(conf).await.read_summary(window()).await.unwrap());
    assert_eq!(next["buckets"], first["buckets"]);
}

#[tokio::test]
async fn failed_cache_publication_remains_dirty_and_retries_on_the_next_read() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let s = service(conf.clone()).await;
    let cache_path = conf.state_dir.join(SCAN_CACHE_FILE_NAME);
    fs::create_dir_all(&cache_path).unwrap();
    let first = json!(s.read_summary(window()).await.unwrap());
    assert!(cache_path.is_dir());
    fs::remove_dir(&cache_path).unwrap();
    let second = json!(s.read_summary(window()).await.unwrap());
    assert_eq!(first["buckets"], second["buckets"]);
    assert!(
        !serde_json::from_slice::<Value>(&fs::read(&cache_path).unwrap()).unwrap()["files"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

struct StalledRates {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
    document: Value,
}
impl RateFetcher for StalledRates {
    fn fetch<'a>(&'a self, _: &'a str, _: &'a str) -> ProbeFuture<'a, Value> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            Ok(self.document.clone())
        })
    }
}
#[tokio::test]
async fn provisional_reads_do_not_wait_for_a_stalled_rate_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let warm = service(conf.clone()).await;
    warm.read_summary(window()).await.unwrap();
    drop(warm);
    let clock = Arc::new(TestClock::new());
    clock.0.fetch_add(2 * 86_400_000, Ordering::Relaxed);
    let fetch = Arc::new(StalledRates {
        started: Default::default(),
        release: Default::default(),
        document: rates().document.clone(),
    });
    let s = UsageService::with_dependencies(
        conf,
        clock,
        fetch.clone(),
        Arc::new(t3_usage::sources::NoProviderProbe),
    )
    .await
    .unwrap();
    let refresh = tokio::spawn({
        let s = s.clone();
        async move { s.refresh_rates().await }
    });
    fetch.started.notified().await;
    let mut input = window();
    input.provisional = Some(Some(true));
    let read = s.read_summary(input);
    tokio::pin!(read);
    let result = read.await;
    fetch.release.notify_one();
    refresh.await.unwrap().unwrap();
    result.unwrap();
}

#[tokio::test]
async fn summary_counts_partial_copies_once_and_survives_deletion_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let first = service(conf.clone()).await;
    let result = json!(first.read_summary(window()).await.unwrap());
    assert_eq!(output(&result), 271);
    assert_eq!(result["sources"][0]["distinctSessions"], 1);
    assert_eq!(result["sources"][0]["scannedFiles"], 1);
    let buckets = result["buckets"].clone();
    fs::remove_file(path).unwrap();
    drop(first);
    let recreated = service(conf).await;
    let saved = json!(recreated.read_summary(window()).await.unwrap());
    assert_eq!(saved["buckets"], buckets);
    assert_eq!(saved["sources"][0]["distinctSessions"], 1);
}

#[tokio::test]
async fn upgrades_legacy_codex_caches_reparses_live_files_and_keeps_deleted_history() {
    for version in [4, 14, 15] {
        let temp = tempfile::tempdir().unwrap();
        let live = dunce::canonicalize(copy_transcript(&temp, "codex-cumulative.jsonl")).unwrap();
        let deleted = live.with_file_name("deleted.jsonl");
        let mut conf = config(&temp);
        conf.sources
            .push(Source::transcripts(P::Codex, live.parent().unwrap()));
        let parsed = t3_usage::reader::read_transcript(&live, &P::Codex, None).unwrap();
        let entry = t3_usage::cache::CachedFile {
            size: fs::metadata(&live).unwrap().len() as i64,
            mtime_ms: fs::metadata(&live)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
                * 1000.0,
            provider: P::Codex,
            records: parsed.records,
            tail_records: parsed.tail_records,
            malformed_records: parsed.malformed_records,
            position: parsed.position,
        };
        let expected: u64 = entry
            .records
            .iter()
            .chain(&entry.tail_records)
            .map(|r| r.totals.output_tokens)
            .sum();
        let mut deleted_entry = entry.clone();
        for record in deleted_entry
            .records
            .iter_mut()
            .chain(&mut deleted_entry.tail_records)
        {
            record.session_id = "deleted-session".into();
        }
        let cache = [
            (live.to_string_lossy().to_string(), entry.clone()),
            (deleted.to_string_lossy().to_string(), deleted_entry),
        ]
        .into_iter()
        .collect();
        let mut encoded = vec![];
        t3_usage::cache::write_scan_cache(&mut encoded, &cache, &serde_json::Map::new()).unwrap();
        let mut document: Value = serde_json::from_slice(&encoded).unwrap();
        document["version"] = json!(version);
        document["files"][live.to_string_lossy().as_ref()]["r"][0][6] = json!(99999);
        assert_eq!(
            t3_usage::cache::decode_scan_cache(&document).len(),
            2,
            "v{version} decoded"
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&live)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        let old = if version == 4 {
            "usage-scan-cache.json".to_string()
        } else {
            format!("usage-scan-cache-v{version}.json")
        };
        fs::create_dir_all(&conf.state_dir).unwrap();
        fs::write(
            conf.state_dir.join(&old),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let summary = json!(
            service(conf.clone())
                .await
                .read_summary(window())
                .await
                .unwrap()
        );
        assert_eq!(output(&summary), expected * 2, "v{version}: {summary}");
        assert_eq!(SCAN_CACHE_FILE_NAME, "usage-scan-cache-v16.json");
        assert!(conf.state_dir.join(SCAN_CACHE_FILE_NAME).is_file());
        assert!(!conf.state_dir.join(old).exists());
    }
}
#[tokio::test]
async fn malformed_codex_records_report_partial_source_with_saved_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "codex-legacy.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Codex, path.parent().unwrap()));
    let s = service(conf.clone()).await;
    let summary = json!(s.read_summary(window()).await.unwrap());
    assert_eq!(summary["sources"][0]["status"], "partial");
    assert_eq!(summary["sources"][0]["malformedRecords"], 1);
    assert_eq!(output(&summary), 30);
    fs::remove_file(path).unwrap();
    let s = service(conf).await;
    let saved = json!(s.read_summary(window()).await.unwrap());
    assert_eq!(saved["sources"][0]["malformedRecords"], 1);
    assert_eq!(saved["buckets"], summary["buckets"]);
}
#[tokio::test]
async fn grows_then_rewrites_without_retaining_stale_records() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let s = service(conf).await;
    assert_eq!(output(&json!(s.read_summary(window()).await.unwrap())), 271);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert_eq!(output(&json!(s.read_summary(window()).await.unwrap())), 271);
    fs::write(&path, fs::read(fixture("claude-advisor.jsonl")).unwrap()).unwrap();
    assert_eq!(output(&json!(s.read_summary(window()).await.unwrap())), 47);
}
#[tokio::test]
async fn compressed_archive_replaces_deleted_original_cache() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "codex-ledger.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Codex, path.parent().unwrap()));
    let s = service(conf.clone()).await;
    let first = json!(s.read_summary(window()).await.unwrap());
    let compressed = path.with_file_name("codex-ledger.jsonl.zst");
    fs::write(
        &compressed,
        zstd::encode_all(fs::File::open(&path).unwrap(), 0).unwrap(),
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    let second = json!(s.read_summary(window()).await.unwrap());
    assert_eq!(first["buckets"], second["buckets"]);
    let cache: Value =
        serde_json::from_slice(&fs::read(conf.state_dir.join(SCAN_CACHE_FILE_NAME)).unwrap())
            .unwrap();
    assert_eq!(cache["files"].as_object().unwrap().len(), 1);
}
#[tokio::test]
async fn moved_codex_copy_preserves_occurrences_and_counts_one_session() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "codex-ledger.jsonl");
    fs::copy(&path, path.with_file_name("copy.jsonl")).unwrap();
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Codex, path.parent().unwrap()));
    let summary = json!(service(conf).await.read_summary(window()).await.unwrap());
    assert_eq!(output(&summary), 60);
    assert_eq!(summary["sources"][0]["distinctSessions"], 1);
}
#[tokio::test]
async fn reprices_unchanged_files_on_alias_override_and_filter_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let s = service(conf).await;
    let first = json!(s.read_summary(window()).await.unwrap());
    let settings = UsageSettings {
        model_aliases: BTreeMap::new(),
        price_overrides: json!({"claude-fable-5":{"inputCostPerMillionTokens":1,"outputCostPerMillionTokens":1,"cacheReadCostPerMillionTokens":1,"cacheWriteCostPerMillionTokens":1}}),
    };
    let repriced = json!(
        s.read_summary_with_settings(window(), settings)
            .await
            .unwrap()
    );
    assert_eq!(
        first["buckets"][0]["totals"],
        repriced["buckets"][0]["totals"]
    );
    assert_ne!(
        first["buckets"][0]["costUsd"],
        repriced["buckets"][0]["costUsd"]
    );
    let restored = json!(s.read_summary(window()).await.unwrap());
    assert_eq!(first["buckets"], restored["buckets"]);
    let settings = UsageSettings {
        model_aliases: BTreeMap::from([("claude-fable-5".into(), "unknown-target".into())]),
        price_overrides: Value::Null,
    };
    let mut input = json!(window());
    input["modelFilter"] = json!({"include":[{"provider":"claude","model":"claude-fable-5"}]});
    let aliased = json!(
        s.read_summary_with_settings(serde_json::from_value(input).unwrap(), settings)
            .await
            .unwrap()
    );
    assert_eq!(aliased["buckets"][0]["model"], "unknown-target");
    assert_eq!(aliased["buckets"][0]["costSource"], "unpriced");
}
#[tokio::test]
async fn invalid_windows_are_typed_errors() {
    let temp = tempfile::tempdir().unwrap();
    let s = service(config(&temp)).await;
    for extra in [
        json!({"sinceDay":"2026-09-01"}),
        json!({"resolution":"hour"}),
        json!({"resolution":"hour","sinceTime":"2026-08-01T00:00:00Z","untilTime":"2026-08-03T00:00:00Z"}),
        json!({"resolution":"hour","sinceTime":"2026-08-01T00:00:00Z","untilTime":"2026-08-01T00:00:00Z"}),
    ] {
        let mut input = json!(window());
        for (k, v) in extra.as_object().unwrap() {
            input[k] = v.clone();
        }
        let input = serde_json::from_value(input).unwrap();
        assert!(matches!(
            s.read_summary(input).await,
            Err(Error::InvalidWindow(_))
        ));
    }
}
#[tokio::test]
async fn provisional_uses_only_covered_windows_and_marks_retained_sources_refreshing() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let s = service(conf).await;
    let exact = json!(s.read_summary(window()).await.unwrap());
    fs::write(path, "").unwrap();
    let mut provisional = window();
    provisional.provisional = Some(Some(true));
    let saved = json!(s.read_summary(provisional.clone()).await.unwrap());
    assert_eq!(saved["buckets"], exact["buckets"]);
    assert_eq!(saved["sources"][0]["refreshing"], true);
    provisional.await_refresh = Some(Some(true));
    assert!(
        s.read_summary(provisional)
            .await
            .unwrap()
            .buckets
            .is_empty()
    );
}
#[tokio::test]
async fn provider_identity_is_part_of_file_cache_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    conf.sources
        .push(Source::transcripts(P::Codex, path.parent().unwrap()));
    let summary = json!(service(conf).await.read_summary(window()).await.unwrap());
    assert_eq!(output(&summary), 271);
    assert_eq!(summary["sources"][1]["scannedFiles"], 0);
}
#[tokio::test]
async fn stable_host_identity_and_explicit_override_survive_service_recreation() {
    let temp = tempfile::tempdir().unwrap();
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, temp.path().join("missing")));
    let first = json!(
        service(conf.clone())
            .await
            .read_summary(window())
            .await
            .unwrap()
    );
    conf.hostname = "renamed".into();
    let second = json!(
        service(conf.clone())
            .await
            .read_summary(window())
            .await
            .unwrap()
    );
    assert_eq!(
        first["sources"][0]["fingerprint"]["hostId"],
        second["sources"][0]["fingerprint"]["hostId"]
    );
    conf.host_id = Some("explicit".into());
    let explicit = json!(
        service(conf.clone())
            .await
            .read_summary(window())
            .await
            .unwrap()
    );
    assert_eq!(explicit["sources"][0]["fingerprint"]["hostId"], "explicit");
    conf.host_id = None;
    let restored = json!(service(conf).await.read_summary(window()).await.unwrap());
    assert_eq!(
        first["sources"][0]["fingerprint"]["hostId"],
        restored["sources"][0]["fingerprint"]["hostId"]
    );
}
#[tokio::test]
async fn concurrent_host_initialization_publishes_one_complete_identity() {
    let temp = tempfile::tempdir().unwrap();
    let mut a = config(&temp);
    let mut b = a.clone();
    a.hostname = "host-a".into();
    b.hostname = "host-b".into();
    let (_a, _b) = tokio::join!(service(a.clone()), service(b));
    let identity = fs::read_to_string(a.base_dir.join("usage-host-id")).unwrap();
    assert!(["host-a\n", "host-b\n"].contains(&identity.as_str()));
}
#[tokio::test]
async fn rate_cache_ttl_refresh_floor_and_offline_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let conf = config(&temp);
    let clock = Arc::new(TestClock::new());
    let rates = rates();
    let s = UsageService::with_dependencies(
        conf.clone(),
        clock.clone(),
        rates.clone(),
        Arc::new(t3_usage::sources::NoProviderProbe),
    )
    .await
    .unwrap();
    s.read_summary(window()).await.unwrap();
    assert_eq!(rates.calls.load(Ordering::Relaxed), 1);
    s.refresh_rates().await.unwrap();
    assert_eq!(rates.calls.load(Ordering::Relaxed), 1);
    clock.0.fetch_add(60_001, Ordering::Relaxed);
    s.refresh_rates().await.unwrap();
    assert_eq!(rates.calls.load(Ordering::Relaxed), 2);
    clock.0.fetch_add(86_400_001, Ordering::Relaxed);
    drop(s);
    let failing = Arc::new(Rates {
        calls: AtomicUsize::new(0),
        document: Value::Null,
        fail: true,
    });
    let s = UsageService::with_dependencies(
        conf,
        clock.clone(),
        failing.clone(),
        Arc::new(t3_usage::sources::NoProviderProbe),
    )
    .await
    .unwrap();
    let summary = json!(s.read_summary(window()).await.unwrap());
    assert_eq!(summary["pricing"]["status"], "cached");
    assert_eq!(failing.calls.load(Ordering::Relaxed), 2);
    s.refresh_rates().await.unwrap();
    assert_eq!(failing.calls.load(Ordering::Relaxed), 2);
}
struct Tiers;
impl ProviderUsageProbe for Tiers {
    fn codex_tiers<'a>(&'a self, requests: &'a [TierRequest]) -> ProbeFuture<'a, HashMapTiers> {
        Box::pin(async move {
            Ok(requests
                .iter()
                .cloned()
                .map(|r| (r, UsageSpeed::Fast))
                .collect())
        })
    }
    fn scan<'a>(
        &'a self,
        _: &'a [ProviderInstance],
        _: f64,
        _: f64,
        _: bool,
    ) -> ProbeFuture<'a, Vec<ExternalSource>> {
        Box::pin(async { Ok(vec![]) })
    }
}
#[tokio::test]
async fn corrected_codex_tiers_persist_after_reparse_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "codex-ledger.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Codex, path.parent().unwrap()));
    let s = UsageService::with_dependencies(
        conf.clone(),
        Arc::new(TestClock::new()),
        rates(),
        Arc::new(Tiers),
    )
    .await
    .unwrap();
    let first = json!(s.read_summary(window()).await.unwrap());
    assert!(first["buckets"][0].get("fastCostUsd").is_none());
    let cache: Value =
        serde_json::from_slice(&fs::read(conf.state_dir.join(SCAN_CACHE_FILE_NAME)).unwrap())
            .unwrap();
    let entry = cache["files"].as_object().unwrap().values().next().unwrap();
    assert!(entry["r"].as_array().unwrap().iter().all(|r| r[10] == 1));
    let content = fs::read(&path).unwrap();
    fs::write(&path, content).unwrap();
    drop(s);
    let s = service(conf).await;
    s.read_summary(window()).await.unwrap();
}

#[tokio::test]
async fn antigravity_learns_models_across_instance_roots() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::copy(
        fixture("databases/antigravity/a-usage.db"),
        a.join("a-usage.db"),
    )
    .unwrap();
    fs::copy(
        fixture("databases/antigravity/z-evidence.db"),
        b.join("z-evidence.db"),
    )
    .unwrap();
    let mut conf = config(&temp);
    for directory in [a, b] {
        conf.sources.push(Source {
            provider: P::Antigravity,
            directory,
            kind: t3_usage::sources::SourceKind::Antigravity,
            refreshing: false,
        });
    }
    let input = serde_json::from_value(
        json!({"sinceDay":"2026-05-01","untilDay":"2026-06-01","timeZone":"UTC"}),
    )
    .unwrap();
    let summary = json!(service(conf).await.read_summary(input).await.unwrap());
    let models: Vec<_> = summary["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["model"].as_str().unwrap())
        .collect();
    assert!(models.contains(&"gemini-9.1-flash-high"), "{models:?}");
}

struct GatedRates {
    started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: MutexReceiver,
    calls: AtomicUsize,
}
type MutexReceiver = tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>;
impl RateFetcher for GatedRates {
    fn fetch<'a>(&'a self, _: &'a str, _: &'a str) -> ProbeFuture<'a, Value> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            if let Some(release) = self.release.lock().await.take() {
                let _ = release.await;
            }
            Ok(json!({"claude-fable-5":{"input_cost_per_token":1e-5,"output_cost_per_token":5e-5}}))
        })
    }
}
#[tokio::test]
async fn cancelled_first_caller_does_not_orphan_shared_scan() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let rates = Arc::new(GatedRates {
        started: std::sync::Mutex::new(Some(started_tx)),
        release: tokio::sync::Mutex::new(Some(release_rx)),
        calls: AtomicUsize::new(0),
    });
    let s = UsageService::with_dependencies(
        conf,
        Arc::new(TestClock::new()),
        rates.clone(),
        Arc::new(t3_usage::sources::NoProviderProbe),
    )
    .await
    .unwrap();
    let first = tokio::spawn({
        let s = s.clone();
        async move { s.read_summary(window()).await }
    });
    started_rx.await.unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let second = tokio::spawn({
        let s = s.clone();
        async move { s.read_summary(window()).await }
    });
    release_tx.send(()).unwrap();
    assert_eq!(output(&json!(second.await.unwrap().unwrap())), 271);
    assert_eq!(rates.calls.load(Ordering::Relaxed), 1);
}

struct ExternalProbe;
impl ProviderUsageProbe for ExternalProbe {
    fn muse_home<'a>(
        &'a self,
        _: &'a ProviderInstance,
        await_refresh: bool,
    ) -> ProbeFuture<'a, t3_usage::sources::MuseHome> {
        Box::pin(async move {
            assert!(!await_refresh);
            Ok(t3_usage::sources::MuseHome::Pending)
        })
    }
    fn scan<'a>(
        &'a self,
        _: &'a [ProviderInstance],
        _: f64,
        _: f64,
        _: bool,
    ) -> ProbeFuture<'a, Vec<ExternalSource>> {
        Box::pin(async {
            let r = t3_usage::UsageRecord {
                provider: P::Cursor,
                timestamp_ms: timestamp_ms("2026-08-01T10:00:00Z").unwrap(),
                model: "cursor-fixture".into(),
                rate_model: None,
                session_id: "conversation-1".into(),
                totals: t3_usage::UsageTokenTotals {
                    uncached_input_tokens: 100,
                    cached_input_tokens: 10,
                    cache_creation_tokens: 0,
                    output_tokens: 20,
                    reasoning_tokens: 0,
                },
                cache_creation1h_tokens: None,
                reported_cost_usd: Some(0.25),
                speed: UsageSpeed::Standard,
                unresolved_service_tier_turn_id: None,
                dedupe_key: Some("cursor-account:fixture:event:0".into()),
            };
            Ok(vec![ExternalSource {
                provider: P::Cursor,
                directory: "cursor-account:fixture".into(),
                host_id: Some("cursor.com".into()),
                volume_id: "fixture".into(),
                files: Some(vec![("cursor-account:fixture".into(), vec![r])]),
                status: Some("ok".into()),
                message: None,
                refreshing: false,
            }])
        })
    }
}
#[tokio::test]
async fn fake_cursor_account_and_pending_muse_discovery_leave_local_usage_available() {
    let temp = tempfile::tempdir().unwrap();
    let path = copy_transcript(&temp, "claude-copies.jsonl");
    let mut conf = config(&temp);
    conf.sources
        .push(Source::transcripts(P::Claude, path.parent().unwrap()));
    conf.instances.push(ProviderInstance::new(P::Muse));
    let s = UsageService::with_dependencies(
        conf,
        Arc::new(TestClock::new()),
        rates(),
        Arc::new(ExternalProbe),
    )
    .await
    .unwrap();
    let result = json!(s.read_summary(window()).await.unwrap());
    assert_eq!(output(&result), 291);
    assert_eq!(result["sources"][1]["status"], "ok");
    assert_eq!(result["sources"][1]["refreshing"], true);
    assert_eq!(result["sources"][2]["fingerprint"]["hostId"], "cursor.com");
    assert_eq!(result["buckets"][1]["costSource"], "providerReported");
}
