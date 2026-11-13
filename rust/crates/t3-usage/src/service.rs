use crate::{
    Error, UsageProviderKind as P, UsageRecord,
    aggregation::{
        AggregateOptions, UsageAggregator, canonical_model_filter, resolve_model_aliases,
    },
    cache::{
        CachedFile, LEGACY_SCAN_CACHE_FILE_NAMES, SCAN_CACHE_FILE_NAME, ScanCache,
        decode_scan_cache, dedupe_within_file, prune_scan_cache, write_scan_cache,
    },
    database::{AntigravityCache, read_antigravity_bounded, read_opencode_bounded},
    iso,
    pricing::{RateTable, create_override_rate_table, parse_rate_table},
    provider_name,
    reader::{list_transcripts, read_transcript_bounded},
    sources::{
        ExternalSource, NoProviderProbe, ProbeFuture, ProviderInstance, ProviderUsageProbe, Source,
        SourceKind, TierRequest, resolve_sources,
    },
    timestamp_ms,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use t3_contracts::usage::{USAGE_CONTRACT_VERSION, UsagePricing, UsageSummary, UsageSummaryInput};
use tokio::sync::Mutex;

pub const RATES_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
pub const RATES_API_URL: &str = "https://api.github.com/repos/BerriAI/litellm/contents/model_prices_and_context_window.json?ref=main";
const DAY: f64 = 86_400_000.0;
const SLACK: f64 = 36.0 * 3_600_000.0;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> f64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }
}
pub trait RateFetcher: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a str, accept: &'a str) -> ProbeFuture<'a, Value>;
}
pub struct NoRateFetcher;
impl RateFetcher for NoRateFetcher {
    fn fetch<'a>(&'a self, _: &'a str, _: &'a str) -> ProbeFuture<'a, Value> {
        Box::pin(async { Err(Error::Probe("No rate fetcher configured".into())) })
    }
}

#[derive(Clone)]
pub struct ServiceConfig {
    pub base_dir: PathBuf,
    pub state_dir: PathBuf,
    pub home_dir: PathBuf,
    pub hostname: String,
    pub host_id: Option<String>,
    pub sources: Vec<Source>,
    pub instances: Vec<ProviderInstance>,
}
impl ServiceConfig {
    pub fn new(base_dir: PathBuf, state_dir: PathBuf, home_dir: PathBuf, hostname: String) -> Self {
        Self {
            base_dir,
            state_dir,
            home_dir,
            hostname,
            host_id: None,
            sources: vec![],
            instances: vec![],
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct UsageSettings {
    pub model_aliases: BTreeMap<String, String>,
    pub price_overrides: Value,
}
#[derive(Default, Clone)]
struct State {
    cache: ScanCache,
    source_cache: indexmap::IndexMap<String, Value>,
    covered: BTreeMap<String, f64>,
    antigravity: AntigravityCache,
    rates: RateTable,
    rates_document: Value,
    rates_fetched_at: Option<f64>,
    rates_last_failed: Option<f64>,
    rates_status: String,
    cache_dirty: bool,
}
type SharedScan = tokio::sync::watch::Receiver<Option<Result<UsageSummary, String>>>;
#[derive(Clone)]
pub struct UsageService {
    config: Arc<ServiceConfig>,
    state: Arc<Mutex<State>>,
    host_id: Arc<String>,
    clock: Arc<dyn Clock>,
    rate_fetcher: Arc<dyn RateFetcher>,
    provider_probe: Arc<dyn ProviderUsageProbe>,
    inflight: Arc<Mutex<HashMap<String, SharedScan>>>,
    scan_lock: Arc<Mutex<()>>,
    rates_lock: Arc<Mutex<()>>,
    persistence_lock: Arc<Mutex<()>>,
    cleaned: Arc<std::sync::atomic::AtomicBool>,
}
impl UsageService {
    pub async fn new(config: ServiceConfig) -> Result<Self, Error> {
        Self::with_dependencies(
            config,
            Arc::new(SystemClock),
            Arc::new(NoRateFetcher),
            Arc::new(NoProviderProbe),
        )
        .await
    }
    pub async fn with_dependencies(
        config: ServiceConfig,
        clock: Arc<dyn Clock>,
        rate_fetcher: Arc<dyn RateFetcher>,
        provider_probe: Arc<dyn ProviderUsageProbe>,
    ) -> Result<Self, Error> {
        let conf = config.clone();
        let (state, host_id) = tokio::task::spawn_blocking(move || {
            let host_id = host_identity(&conf);
            let mut state = State {
                rates_status: "unavailable".into(),
                ..Default::default()
            };
            for name in std::iter::once(SCAN_CACHE_FILE_NAME).chain(LEGACY_SCAN_CACHE_FILE_NAMES) {
                let value = match crate::safety::read_document(&conf.state_dir.join(name)) {
                    Ok(value) => value,
                    Err(error) => {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            tracing::warn!("Usage scan cache could not be read");
                        }
                        continue;
                    }
                };
                state.cache = decode_scan_cache(&value);
                if let Some(sources) = value["sources"].as_object() {
                    for (k, v) in sources {
                        if v["dir"].is_string() && v["volumeId"].is_string() {
                            state.source_cache.insert(
                                k.clone(),
                                json!({"dir":v["dir"],"volumeId":v["volumeId"]}),
                            );
                        }
                    }
                }
                if name == SCAN_CACHE_FILE_NAME
                    && !state.cache.is_empty()
                    && value["files"]
                        .as_object()
                        .is_some_and(|files| files.len() == state.cache.len())
                    && let Some(covered) = value["coveredSince"].as_object()
                {
                    for (k, v) in covered {
                        if let Some(t) = v.as_f64() {
                            state.covered.insert(k.clone(), t);
                        }
                    }
                }
                break;
            }
            if let Ok(v) =
                crate::safety::read_document(&conf.state_dir.join("usage-model-rates.json"))
            {
                let table = parse_rate_table(&v["document"]);
                if !table.is_empty()
                    && let Some(at) = v["fetchedAtMs"].as_f64()
                {
                    state.rates = table;
                    state.rates_document = v["document"].clone();
                    state.rates_fetched_at = Some(at);
                    state.rates_status = "cached".into();
                }
            }
            (state, host_id)
        })
        .await
        .map_err(|e| Error::Worker(e.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
            state: Arc::new(Mutex::new(state)),
            host_id: Arc::new(host_id),
            clock,
            rate_fetcher,
            provider_probe,
            inflight: Arc::new(Mutex::new(HashMap::new())),
            scan_lock: Arc::new(Mutex::new(())),
            rates_lock: Arc::new(Mutex::new(())),
            persistence_lock: Arc::new(Mutex::new(())),
            cleaned: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    async fn ensure_rates(&self, force: bool) -> Result<(), Error> {
        let _rates = self.rates_lock.lock().await;
        let now = self.clock.now_ms();
        {
            let state = self.state.lock().await;
            let age = if force { 60_000.0 } else { DAY };
            if state.rates_fetched_at.is_some_and(|at| now - at < age)
                || state
                    .rates_last_failed
                    .is_some_and(|at| now - at < 60_000.0)
            {
                return Ok(());
            }
        }
        for (url, accept) in [
            (RATES_URL, "application/json"),
            (RATES_API_URL, "application/vnd.github.raw+json"),
        ] {
            let Ok(Ok(document)) = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.rate_fetcher.fetch(url, accept),
            )
            .await
            else {
                continue;
            };
            let table = parse_rate_table(&document);
            if table.is_empty() {
                continue;
            }
            {
                let mut state = self.state.lock().await;
                state.rates = table;
                state.rates_document = document.clone();
                state.rates_fetched_at = Some(now);
                state.rates_last_failed = None;
                state.rates_status = "fresh".into();
            }
            let _persistence = self.persistence_lock.lock().await;
            let config = self.config.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(error) =
                    atomic_write(&config.state_dir.join("usage-model-rates.json"), |w| {
                        serde_json::to_writer(w, &json!({"fetchedAtMs":now,"document":document}))?;
                        Ok(())
                    })
                {
                    tracing::warn!(kind = ?error.kind(), "Usage rates cache write failed");
                }
            })
            .await
            .map_err(|_| Error::Worker("Rates persistence worker failed".into()))?;
            return Ok(());
        }
        let mut state = self.state.lock().await;
        state.rates_last_failed = Some(self.clock.now_ms());
        if !state.rates.is_empty() {
            state.rates_status = "cached".into();
        }
        tracing::warn!("Usage rate fetch failed");
        Ok(())
    }
    pub async fn refresh_rates(&self) -> Result<UsagePricing, Error> {
        self.ensure_rates(true).await?;
        pricing(&*self.state.lock().await)
    }
    pub async fn read_summary(&self, input: UsageSummaryInput) -> Result<UsageSummary, Error> {
        self.read_summary_with_settings(input, UsageSettings::default())
            .await
    }
    pub async fn read_summary_with_settings(
        &self,
        mut input: UsageSummaryInput,
        settings: UsageSettings,
    ) -> Result<UsageSummary, Error> {
        input.model_filter = canonical_model_filter(input.model_filter.flatten()).map(Some);
        validate_window(&input)?;
        let key = json!([input, settings.model_aliases, settings.price_overrides]).to_string();
        let mut inflight = self.inflight.lock().await;
        let mut receiver = if let Some(existing) = inflight.get(&key) {
            existing.clone()
        } else {
            let (sender, receiver) = tokio::sync::watch::channel(None);
            inflight.insert(key.clone(), receiver.clone());
            let service = self.clone();
            tokio::spawn(async move {
                let result = service
                    .perform_read(input, settings)
                    .await
                    .map_err(|e| e.to_string());
                let mut inflight = service.inflight.lock().await;
                let _ = sender.send(Some(result));
                inflight.remove(&key);
            });
            receiver
        };
        drop(inflight);
        let result = receiver
            .wait_for(|value| value.is_some())
            .await
            .map_err(|e| Error::Worker(e.to_string()))?
            .clone();
        result
            .ok_or_else(|| Error::Worker("Scan completed without a result".into()))?
            .map_err(Error::ScanFailed)
    }
    async fn perform_read(
        &self,
        input: UsageSummaryInput,
        settings: UsageSettings,
    ) -> Result<UsageSummary, Error> {
        let service = self.clone();
        // The scan owns its state after spawning, so a disconnected caller cannot cancel cache warming.
        tokio::spawn(async move {
            let started = service.clock.now_ms();
            let provisional =
                input.provisional == Some(Some(true)) && input.await_refresh != Some(Some(true));
            let mut state = service.state.lock().await.clone();
            let mut sources = service.config.sources.clone();
            let await_refresh = input.await_refresh == Some(Some(true));
            for instance in &service.config.instances {
                let muse_home = if instance.provider == P::Muse {
                    match service
                        .provider_probe
                        .muse_home(instance, await_refresh)
                        .await
                    {
                        Ok(crate::sources::MuseHome::Resolved(path)) => Some(path),
                        Ok(crate::sources::MuseHome::Pending) => None,
                        Ok(crate::sources::MuseHome::Unavailable) => continue,
                        Err(_) => {
                            tracing::warn!("Muse usage home discovery failed");
                            continue;
                        }
                    }
                } else {
                    None
                };
                sources.extend(resolve_sources(
                    &service.config.home_dir,
                    instance,
                    muse_home.as_deref(),
                ));
            }
            canonical_sources(&mut sources, &mut state, started - 92.0 * DAY);
            let (window, window_start) = validate_window(&input)?;
            let from_memory = provisional
                && !sources.is_empty()
                && sources
                    .iter()
                    .map(|s| &s.directory)
                    .collect::<HashSet<_>>()
                    .len()
                    == sources.len()
                && sources.iter().all(|s| {
                    state
                        .covered
                        .get(&source_key(s))
                        .is_some_and(|t| *t <= window_start)
                });
            if !from_memory {
                service.ensure_rates(false).await?;
            }
            let _scan = service.scan_lock.clone().lock_owned().await;
            {
                let shared = service.state.lock().await;
                state.cache = shared.cache.clone();
                state.covered = shared.covered.clone();
                state.antigravity = shared.antigravity.clone();
                state.cache_dirty |= shared.cache_dirty;
                state.rates = shared.rates.clone();
                state.rates_document = shared.rates_document.clone();
                state.rates_fetched_at = shared.rates_fetched_at;
                state.rates_last_failed = shared.rates_last_failed;
                state.rates_status = shared.rates_status.clone();
            }
            let external = service
                .provider_probe
                .scan(
                    &service.config.instances,
                    window_start,
                    started,
                    await_refresh,
                )
                .await
                .map_err(|_| Error::Probe("Provider usage scan failed".into()))?;
            let config = service.config.clone();
            let (mut state, mut scanned) = tokio::task::spawn_blocking(move || {
                let scanned = scan(
                    &config,
                    &mut state,
                    sources,
                    window_start,
                    started,
                    from_memory,
                    external,
                );
                (state, scanned)
            })
            .await
            .map_err(|e| Error::Worker(e.to_string()))?;
            if !from_memory {
                let mut requests = vec![];
                let end = window.map(|(_, until)| until).unwrap_or(
                    timestamp_ms(&format!("{}T00:00:00Z", input.until_day.0)).unwrap_or(started)
                        + DAY
                        + SLACK,
                );
                for dir in &scanned {
                    for (_, records, _) in &dir.files {
                        for r in records {
                            if r.provider == P::Codex
                                && r.timestamp_ms >= window_start
                                && r.timestamp_ms < end
                                && let Some(turn) = &r.unresolved_service_tier_turn_id
                            {
                                requests.push(TierRequest {
                                    session_id: r.session_id.clone(),
                                    native_turn_id: turn.clone(),
                                    model: r.model.clone(),
                                });
                            }
                        }
                    }
                }
                if !requests.is_empty()
                    && let Ok(tiers) = service
                        .provider_probe
                        .codex_tiers(&requests)
                        .await
                        .inspect_err(|_| tracing::warn!("Codex usage tier recovery failed"))
                {
                    let correct = |r: &mut UsageRecord| {
                        if let Some(turn) = &r.unresolved_service_tier_turn_id
                            && let Some(speed) = tiers.get(&TierRequest {
                                session_id: r.session_id.clone(),
                                native_turn_id: turn.clone(),
                                model: r.model.clone(),
                            })
                        {
                            r.speed = *speed;
                            r.unresolved_service_tier_turn_id = None;
                        }
                    };
                    for dir in &mut scanned {
                        for (_, records, _) in &mut dir.files {
                            for r in records {
                                correct(r);
                            }
                        }
                    }
                    for entry in state.cache.values_mut() {
                        for r in entry.records.iter_mut().chain(&mut entry.tail_records) {
                            let previous = r.clone();
                            correct(r);
                            state.cache_dirty |= previous != *r;
                        }
                    }
                }
            }
            {
                let mut shared = service.state.lock().await;
                shared.cache = state.cache.clone();
                shared.source_cache = state.source_cache.clone();
                shared.covered = state.covered.clone();
                shared.antigravity = state.antigravity.clone();
                shared.cache_dirty = state.cache_dirty;
            }
            let _persistence = service.persistence_lock.clone().lock_owned().await;
            let persist_service = service.clone();
            let (summary, persisted) = tokio::task::spawn_blocking(move || {
                let summary = finish(
                    &service,
                    &state,
                    scanned,
                    input,
                    settings,
                    window,
                    started,
                    from_memory,
                )?;
                let persisted = !state.cache_dirty
                    || persist(
                        &service.config,
                        &state,
                        service.clock.now_ms(),
                        &service.cleaned,
                    );
                Ok::<_, Error>((summary, persisted))
            })
            .await
            .map_err(|e| Error::Worker(e.to_string()))??;
            if persisted {
                persist_service.state.lock().await.cache_dirty = false;
            }
            Ok(summary)
        })
        .await
        .map_err(|e| Error::Worker(e.to_string()))?
    }
}

fn validate_window(input: &UsageSummaryInput) -> Result<(Option<(f64, f64)>, f64), Error> {
    if input.since_day.0 > input.until_day.0 {
        return Err(Error::InvalidWindow("sinceDay is after untilDay".into()));
    }
    let since = timestamp_ms(&format!("{}T00:00:00Z", input.since_day.0))
        .ok_or_else(|| Error::InvalidWindow("sinceDay is not a valid date".into()))?;
    let hourly = matches!(
        input.resolution,
        Some(Some(t3_contracts::usage::UsageResolution::Hour))
    );
    let window = if hourly {
        let since = input
            .since_time
            .as_ref()
            .and_then(|s| s.as_ref())
            .and_then(|s| timestamp_ms(&s.0));
        let until = input
            .until_time
            .as_ref()
            .and_then(|s| s.as_ref())
            .and_then(|s| timestamp_ms(&s.0));
        let (Some(s), Some(u)) = (since, until) else {
            return Err(Error::InvalidWindow(
                "Hourly usage requires valid sinceTime and untilTime instants".into(),
            ));
        };
        if u <= s || u - s > DAY {
            return Err(Error::InvalidWindow(
                "Hourly usage window must be greater than zero and at most 24 hours".into(),
            ));
        }
        Some((s, u))
    } else {
        None
    };
    Ok((window, window.map(|(s, _)| s).unwrap_or(since) - SLACK))
}
fn pricing(state: &State) -> Result<UsagePricing, Error> {
    Ok(serde_json::from_value(
        json!({"status":state.rates_status,"source":RATES_URL,"fetchedAt":state.rates_fetched_at.map(iso),"knownModels":state.rates.len()}),
    )?)
}
fn source_key(s: &Source) -> String {
    format!("{}\0{}", provider_name(&s.provider), s.directory.display())
}
fn canonical_sources(sources: &mut Vec<Source>, state: &mut State, cutoff: f64) {
    let mut seen = HashSet::new();
    sources.retain_mut(|s| {
        let original = source_key(s);
        let previous = state.source_cache.get(&original).cloned();
        s.directory = dunce::canonicalize(&s.directory).unwrap_or_else(|_| {
            previous
                .as_ref()
                .and_then(|v| v["dir"].as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| crate::sources::canonical(s.directory.clone()))
        });
        let retained = state.cache.iter().any(|(p, e)| {
            e.provider == s.provider
                && e.mtime_ms >= cutoff
                && (!e.records.is_empty() || !e.tail_records.is_empty() || e.malformed_records > 0)
                && Path::new(p).starts_with(&s.directory)
        });
        let current = volume_id(&s.directory).unwrap_or_default();
        let held = previous
            .as_ref()
            .filter(|v| v["dir"].as_str() == s.directory.to_str())
            .and_then(|v| v["volumeId"].as_str())
            .filter(|s| !s.is_empty());
        let volume = if retained || current.is_empty() {
            held.unwrap_or(&current).to_string()
        } else {
            current
        };
        let value = json!({"dir":s.directory,"volumeId":volume});
        state.cache_dirty |= state.source_cache.get(&original) != Some(&value);
        state.source_cache.insert(original, value);
        seen.insert(source_key(s))
    });
}

struct Scanned {
    provider: P,
    dir: String,
    volume: String,
    host: Option<String>,
    missing: bool,
    status: Option<String>,
    message: Option<String>,
    refreshing: bool,
    files: Vec<(String, Vec<UsageRecord>, u64)>,
}
fn scan(
    config: &ServiceConfig,
    state: &mut State,
    sources: Vec<Source>,
    window_start: f64,
    now: f64,
    from_memory: bool,
    external: Vec<ExternalSource>,
) -> Vec<Scanned> {
    let cutoff = now - 92.0 * DAY;
    let retained = state
        .cache
        .values()
        .map(|entry| {
            entry
                .records
                .iter()
                .chain(&entry.tail_records)
                .map(UsageRecord::retained_size)
                .sum::<usize>()
                + serde_json::to_vec(&entry.position.state)
                    .map_or(crate::safety::MAX_STATE_BYTES, |v| v.len())
        })
        .sum::<usize>()
        + state.antigravity.retained_size();
    let budget = crate::safety::ScanBudget::new(
        crate::safety::MAX_SCAN_BYTES,
        crate::safety::MAX_SCAN_RETAINED_BYTES.saturating_sub(retained as u64),
    );
    let mut scanned = vec![];
    let antigravity_roots: Vec<_> = sources
        .iter()
        .filter(|s| s.kind == SourceKind::Antigravity && !s.refreshing)
        .map(|s| s.directory.clone())
        .collect();
    let antigravity = read_antigravity_bounded(
        &antigravity_roots,
        window_start,
        &mut state.antigravity,
        &budget,
    );
    for source in sources {
        let key = source_key(&source);
        let dir = source.directory.to_string_lossy().to_string();
        let current_volume = volume_id(&source.directory).unwrap_or_default();
        let held = state
            .source_cache
            .get(&key)
            .and_then(|v| v["volumeId"].as_str())
            .filter(|s| !s.is_empty());
        let retained = state.cache.iter().any(|(p, e)| {
            e.provider == source.provider
                && e.mtime_ms >= cutoff
                && (!e.records.is_empty() || !e.tail_records.is_empty() || e.malformed_records > 0)
                && Path::new(p).starts_with(&source.directory)
        });
        let volume = if retained || current_volume.is_empty() {
            held.unwrap_or(&current_volume).to_string()
        } else {
            current_volume
        };
        let value = json!({"dir":dir,"volumeId":volume});
        state.cache_dirty |= state.source_cache.get(&key) != Some(&value);
        state.source_cache.insert(key.clone(), value);
        let mut result = Scanned {
            provider: source.provider.clone(),
            dir: dir.clone(),
            volume,
            host: None,
            missing: !source.refreshing && !source.directory.exists(),
            status: None,
            message: None,
            refreshing: source.refreshing,
            files: vec![],
        };
        let mut failed = 0;
        if !source.refreshing {
            match &source.kind {
                SourceKind::Transcripts { file_name } => {
                    if !from_memory && !result.missing {
                        let listed =
                            list_transcripts(&source.directory, window_start, file_name.as_deref());
                        failed += listed.failed_paths;
                        let files = listed.files;
                        let mut updates = vec![];
                        for batch in files.chunks(4) {
                            std::thread::scope(|scope| {
                                let workers: Vec<_> = batch
                                    .iter()
                                    .map(|f| {
                                        let cached =
                                            state.cache.get(&f.path.to_string_lossy().to_string());
                                        let provider = &source.provider;
                                        let budget = &budget;
                                        scope.spawn(move || {
                                            let path = f.path.to_string_lossy().to_string();
                                            if let Some(cached) = cached.filter(|c| {
                                                c.provider == *provider
                                                    && c.size == f.size as i64
                                                    && c.mtime_ms == f.mtime_ms
                                            }) {
                                                return (path, Some(cached.clone()), false);
                                            }
                                            let resume = cached
                                                .filter(|c| {
                                                    c.provider == *provider
                                                        && f.size as i64 > c.size
                                                })
                                                .map(|c| &c.position);
                                            let parsed = read_transcript_bounded(
                                                &f.path, provider, resume, budget,
                                            );
                                            let Some(mut parsed) = parsed else {
                                                return (
                                                    path,
                                                    cached
                                                        .filter(|c| c.provider == *provider)
                                                        .cloned(),
                                                    true,
                                                );
                                            };
                                            preserve_corrections(
                                                cached.filter(|c| c.provider == *provider),
                                                &mut parsed.records,
                                                &mut parsed.tail_records,
                                            );
                                            let base = cached
                                                .filter(|_| parsed.resumed)
                                                .map(|c| c.records.clone())
                                                .unwrap_or_default();
                                            let entry = CachedFile {
                                                size: f.size as i64,
                                                mtime_ms: f.mtime_ms,
                                                provider: provider.clone(),
                                                records: dedupe_within_file(
                                                    base.into_iter().chain(parsed.records),
                                                ),
                                                tail_records: parsed.tail_records,
                                                malformed_records: parsed.malformed_records,
                                                position: parsed.position,
                                            };
                                            (path, Some(entry), false)
                                        })
                                    })
                                    .collect();
                                for worker in workers {
                                    match worker.join() {
                                        Ok(update) => updates.push(update),
                                        Err(_) => failed += 1,
                                    }
                                }
                            });
                        }
                        let live: HashSet<_> = files
                            .iter()
                            .map(|f| f.path.to_string_lossy().to_string())
                            .collect();
                        for (path, entry, read_failed) in updates {
                            if read_failed {
                                failed += 1;
                            }
                            if let Some(entry) = entry {
                                let records = entry
                                    .records
                                    .iter()
                                    .chain(&entry.tail_records)
                                    .cloned()
                                    .collect();
                                result
                                    .files
                                    .push((path.clone(), records, entry.malformed_records));
                                if !read_failed {
                                    state.cache_dirty |= state.cache.get(&path) != Some(&entry);
                                    state.cache.insert(path.clone(), entry);
                                }
                                if !read_failed
                                    && let Some(original) = path
                                        .strip_suffix(".zst")
                                        .filter(|_| path.ends_with(".jsonl.zst"))
                                    && !live.contains(original)
                                {
                                    state.cache_dirty |=
                                        state.cache.shift_remove(original).is_some();
                                }
                            }
                        }
                        let saved: Vec<_> = state.cache.keys().cloned().collect();
                        let members: HashSet<_> = saved
                            .iter()
                            .filter(|p| {
                                state.cache[*p].provider == source.provider
                                    && Path::new(p).starts_with(&source.directory)
                            })
                            .cloned()
                            .collect();
                        let walked: Vec<_> = listed
                            .walked
                            .iter()
                            .map(|p| p.to_string_lossy().to_string())
                            .collect();
                        let listed: HashSet<_> = walked.iter().cloned().collect();
                        let mut ordered = walked.into_iter().filter(|p| members.contains(p)).chain(
                            saved
                                .iter()
                                .filter(|p| members.contains(*p) && !listed.contains(*p))
                                .cloned(),
                        );
                        let keys: Vec<_> = saved
                            .iter()
                            .map(|p| {
                                if members.contains(p) {
                                    ordered.next().unwrap_or_else(|| p.clone())
                                } else {
                                    p.clone()
                                }
                            })
                            .collect();
                        state.cache_dirty |= !keys.iter().eq(state.cache.keys());
                        let mut cache = std::mem::take(&mut state.cache);
                        for p in keys {
                            if let Some(entry) = cache.shift_remove(&p) {
                                state.cache.insert(p, entry);
                            }
                        }
                    }
                    if failed == 0 {
                        state.cache_dirty |=
                            state.covered.get(&key).is_none_or(|t| window_start < *t);
                        state
                            .covered
                            .entry(key)
                            .and_modify(|t| *t = t.min(window_start))
                            .or_insert(window_start);
                    }
                    let live: HashSet<_> = result.files.iter().map(|(p, _, _)| p.clone()).collect();
                    for (path, entry) in &state.cache {
                        if entry.provider == source.provider
                            && entry.mtime_ms >= window_start.max(cutoff)
                            && !live.contains(path)
                            && Path::new(path).starts_with(&source.directory)
                        {
                            result.files.push((
                                path.clone(),
                                entry
                                    .records
                                    .iter()
                                    .chain(&entry.tail_records)
                                    .cloned()
                                    .collect(),
                                entry.malformed_records,
                            ));
                        }
                    }
                }
                SourceKind::OpenCode => {
                    let read = read_opencode_bounded(&source.directory, window_start, &budget);
                    result.missing = read.missing;
                    failed += read.errors.len() as u64;
                    result.files = read
                        .files
                        .into_iter()
                        .map(|(p, r)| (p.to_string_lossy().to_string(), r, 0))
                        .collect();
                }
                SourceKind::Antigravity => {
                    failed += antigravity
                        .errors
                        .iter()
                        .filter(|p| p.starts_with(&source.directory))
                        .count() as u64;
                    result.files = antigravity
                        .files
                        .iter()
                        .filter(|(p, _)| p.starts_with(&source.directory))
                        .map(|(p, r)| (p.to_string_lossy().to_string(), r.clone(), 0))
                        .collect();
                }
            }
        }
        if failed > 0 {
            tracing::warn!(paths = failed, "Usage input paths could not be read");
            result.status = Some("partial".into());
            result.message = Some(format!(
                "{failed} transcript path(s) could not be read; usage may be incomplete."
            ));
        }
        scanned.push(result);
    }
    for external in external {
        scanned.push(Scanned {
            provider: external.provider,
            dir: external.directory,
            volume: external.volume_id,
            host: external.host_id,
            missing: external.files.is_none(),
            status: external.status,
            message: external.message,
            refreshing: external.refreshing,
            files: external
                .files
                .unwrap_or_default()
                .into_iter()
                .map(|(p, r)| (p, r, 0))
                .collect(),
        });
    }
    if prune_scan_cache(&mut state.cache, cutoff) > 0 {
        state.cache_dirty = true;
        for since in state.covered.values_mut() {
            *since = since.max(cutoff);
        }
    }
    let _ = config;
    scanned
}
fn preserve_corrections(
    cached: Option<&CachedFile>,
    records: &mut [UsageRecord],
    tail: &mut [UsageRecord],
) {
    fn identity(r: &UsageRecord) -> String {
        json!([
            r.provider,
            r.session_id,
            r.timestamp_ms,
            r.model,
            r.rate_model,
            r.totals,
            r.reported_cost_usd,
            r.dedupe_key
        ])
        .to_string()
    }
    let Some(cached) = cached else {
        return;
    };
    let mut saved: HashMap<String, std::collections::VecDeque<&UsageRecord>> = HashMap::new();
    for r in cached.records.iter().chain(&cached.tail_records) {
        saved.entry(identity(r)).or_default().push_back(r);
    }
    for r in records.iter_mut().chain(tail) {
        if let Some(previous) = saved.get_mut(&identity(r)).and_then(|q| q.pop_front())
            && r.unresolved_service_tier_turn_id.is_some()
            && previous.unresolved_service_tier_turn_id.is_none()
        {
            r.speed = previous.speed;
            r.unresolved_service_tier_turn_id = None;
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn finish(
    service: &UsageService,
    state: &State,
    scanned: Vec<Scanned>,
    input: UsageSummaryInput,
    settings: UsageSettings,
    window: Option<(f64, f64)>,
    started: f64,
    from_memory: bool,
) -> Result<UsageSummary, Error> {
    let mut options = AggregateOptions::daily(
        &input.since_day.0,
        &input.until_day.0,
        &input.time_zone.0,
        state.rates.clone(),
    );
    options.overrides = create_override_rate_table(&settings.price_overrides);
    options.aliases = resolve_model_aliases(&settings.model_aliases);
    options.model_filter = input.model_filter.clone().flatten();
    options.hourly_window = window;
    let mut aggregator = UsageAggregator::new(options);
    let mut sources = vec![];
    let mut first_files = HashMap::new();
    let mut shared = HashSet::new();
    let mut file_index = 0;
    for dir in &scanned {
        for (_, records, _) in &dir.files {
            for r in records {
                if r.provider == P::Codex && !r.session_id.is_empty() {
                    if let Some(first) = first_files.get(&r.session_id) {
                        if *first != file_index {
                            shared.insert(r.session_id.clone());
                        }
                    } else {
                        first_files.insert(r.session_id.clone(), file_index);
                    }
                }
            }
            file_index += 1;
        }
    }
    for dir in scanned {
        let mut sessions = HashSet::new();
        let mut scanned_files = 0;
        let mut skipped = 0;
        let mut malformed = 0;
        let mut counted = false;
        for (_, records, mr) in dir.files {
            malformed += mr;
            if records.is_empty() {
                skipped += 1;
                continue;
            }
            scanned_files += 1;
            let mut occurrences = HashMap::new();
            for mut r in records {
                if r.provider == P::Codex && shared.contains(&r.session_id) {
                    let key = json!([r.provider, r.session_id, r.timestamp_ms, r.model, r.totals])
                        .to_string();
                    let occurrence = occurrences.entry(key.clone()).or_insert(0);
                    *occurrence += 1;
                    r.dedupe_key = Some(format!("{key}:{occurrence}"));
                }
                let session = r.session_id.clone();
                if aggregator.add(r, Some(&dir.dir)) {
                    counted = true;
                    if !session.is_empty() {
                        sessions.insert(session);
                    }
                }
            }
        }
        let mut message = dir.message.or_else(|| {
            dir.missing
                .then(|| "No transcript directory on this environment.".into())
        });
        if malformed > 0 {
            let warning = format!(
                "{malformed} malformed or inconsistent usage record(s) were skipped; usage may be incomplete."
            );
            message = Some(message.map(|m| format!("{m} {warning}")).unwrap_or(warning));
        }
        let status = if dir.missing && scanned_files == 0 {
            "missing"
        } else if malformed > 0 && dir.status.as_deref().is_none_or(|s| s == "ok") {
            "partial"
        } else {
            dir.status.as_deref().unwrap_or("ok")
        };
        let mut source = json!({"fingerprint":{"hostId":dir.host.as_deref().unwrap_or(&service.host_id),"provider":dir.provider,"resolvedHomePath":dir.dir,"volumeId":dir.volume},
            "status":status,"scannedFiles":scanned_files,"skippedFiles":skipped,"malformedRecords":malformed,"distinctSessions":sessions.len(),"message":message});
        if dir.refreshing
            || (from_memory
                && counted
                && matches!(
                    dir.provider,
                    P::Claude | P::Codex | P::Grok | P::Pi | P::Muse
                ))
        {
            source["refreshing"] = json!(true);
        }
        sources.push(source);
    }
    let result = aggregator.finish()?;
    let now = service.clock.now_ms();
    let mut summary = json!({"contractVersion":USAGE_CONTRACT_VERSION,"readAt":iso(now),"timeZone":input.time_zone,"sinceDay":input.since_day,"untilDay":input.until_day,
        "buckets":result.buckets,"sources":sources,"pricing":pricing(state)?,"scanDurationMs":(now-started).max(0.0) as u64});
    if let Some(filter) = input.model_filter.flatten() {
        summary["modelFilter"] = serde_json::to_value(filter)?;
    }
    Ok(serde_json::from_value(summary)?)
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<fs::File>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::ErrorKind::InvalidInput.into());
    };
    fs::create_dir_all(parent)?;
    let temp_dir = parent.join(format!(
        "{}.{}-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&temp_dir)?;
    let temp = temp_dir.join("contents.tmp");
    let result = (|| {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let mut writer = BufWriter::new(file);
        write(&mut writer)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        for attempt in 0..3 {
            match fs::rename(&temp, path) {
                Ok(()) => {
                    #[cfg(unix)]
                    fs::File::open(parent)?.sync_all()?;
                    return Ok(());
                }
                Err(e) if attempt == 2 => return Err(e),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        Err(std::io::Error::other("Usage cache rename failed"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    let _ = fs::remove_dir(&temp_dir);
    result
}
fn persist(
    config: &ServiceConfig,
    state: &State,
    now: f64,
    cleaned: &std::sync::atomic::AtomicBool,
) -> bool {
    let extra = json!({"sources":state.source_cache,"coveredSince":state.covered});
    let Some(extra) = extra.as_object() else {
        return false;
    };
    if let Err(error) = atomic_write(&config.state_dir.join(SCAN_CACHE_FILE_NAME), |w| {
        write_scan_cache(w, &state.cache, extra)
    }) {
        tracing::warn!(kind = ?error.kind(), "Usage scan cache write failed");
        return false;
    }
    if !cleaned.swap(true, Ordering::Relaxed) {
        for name in LEGACY_SCAN_CACHE_FILE_NAMES {
            let _ = fs::remove_file(config.state_dir.join(name));
        }
        if let Ok(entries) = fs::read_dir(&config.state_dir) {
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !std::iter::once(SCAN_CACHE_FILE_NAME)
                    .chain(LEGACY_SCAN_CACHE_FILE_NAMES)
                    .chain(["usage-model-rates.json"])
                    .any(|prefix| name.starts_with(&format!("{prefix}.")))
                {
                    continue;
                }
                let path = entry.path();
                if fs::symlink_metadata(&path)
                    .is_ok_and(|m| m.is_dir() && crate::reader::mtime_ms(&m) <= now - 3_600_000.0)
                    && let (Ok(root), Ok(target)) =
                        (fs::canonicalize(&config.state_dir), fs::canonicalize(&path))
                    && target.starts_with(root)
                {
                    let _ = fs::remove_dir_all(&path);
                }
            }
        }
    }
    true
}
fn host_identity(config: &ServiceConfig) -> String {
    if let Some(id) = config
        .host_id
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return id.into();
    }
    let path = config.base_dir.join("usage-host-id");
    if let Ok(id) = fs::read_to_string(&path) {
        return if id.trim().is_empty() {
            config.hostname.clone()
        } else {
            id.trim().into()
        };
    }
    let publish = (|| -> std::io::Result<()> {
        fs::create_dir_all(&config.base_dir)?;
        let temp = config.base_dir.join(format!(
            "usage-host-id.{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&temp)?;
        let file = temp.join("identity");
        let result = (|| {
            fs::write(&file, format!("{}\n", config.hostname))?;
            fs::hard_link(&file, &path)
        })();
        let _ = fs::remove_file(file);
        let _ = fs::remove_dir(temp);
        if result
            .as_ref()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists)
        {
            Ok(())
        } else {
            result
        }
    })();
    if let Err(error) = publish {
        tracing::warn!(kind = ?error.kind(), "Usage host identity could not be saved");
    }
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(config.hostname.clone())
}
fn volume_id(path: &Path) -> Option<String> {
    #[cfg(windows)]
    let result = file_id::get_low_res_file_id(path);
    #[cfg(not(windows))]
    let result = file_id::get_file_id(path);
    let id = result
        .inspect_err(|_| {
            if path.exists() {
                tracing::warn!("Usage source volume identity unavailable");
            }
        })
        .ok()?;
    match id {
        file_id::FileId::Inode {
            device_id,
            inode_number,
        } => Some(format!("{device_id}:{inode_number}")),
        file_id::FileId::LowRes {
            volume_serial_number,
            file_index,
        } => Some(format!("{volume_serial_number}:{file_index}")),
        file_id::FileId::HighRes {
            volume_serial_number,
            file_id,
        } => Some(format!("{volume_serial_number}:{file_id}")),
    }
}
