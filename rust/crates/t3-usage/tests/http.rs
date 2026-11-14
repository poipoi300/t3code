#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_usage::{
    Error,
    http::*,
    service::{Clock, RATES_API_URL, RATES_URL, RateFetcher, ServiceConfig, UsageService},
    sources::{NoProviderProbe, ProbeFuture},
};

struct Body {
    chunks: VecDeque<Vec<u8>>,
    reads: Arc<AtomicUsize>,
}
impl RateHttpBody for Body {
    fn next_chunk(&mut self) -> ProbeFuture<'_, Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(self.chunks.pop_front()) })
    }
}
type HttpFixture = (u16, Option<u64>, Vec<Vec<u8>>);
struct Http {
    responses: Mutex<VecDeque<HttpFixture>>,
    calls: Mutex<Vec<(String, String, Duration)>>,
    reads: Arc<AtomicUsize>,
}
impl RateHttpClient for Http {
    fn get<'a>(
        &'a self,
        url: &'a str,
        accept: &'a str,
        timeout: Duration,
    ) -> ProbeFuture<'a, RateHttpResponse> {
        self.calls
            .lock()
            .unwrap()
            .push((url.into(), accept.into(), timeout));
        Box::pin(async move {
            let (status, content_length, chunks) =
                self.responses.lock().unwrap().pop_front().unwrap();
            Ok(RateHttpResponse {
                status,
                content_length,
                body: Box::new(Body {
                    chunks: chunks.into(),
                    reads: self.reads.clone(),
                }),
            })
        })
    }
}
fn http(responses: Vec<HttpFixture>) -> Arc<Http> {
    Arc::new(Http {
        responses: Mutex::new(responses.into()),
        calls: Mutex::default(),
        reads: Arc::default(),
    })
}
struct TestClock;
impl Clock for TestClock {
    fn now_ms(&self) -> f64 {
        1785578400000.0
    }
}
#[tokio::test]
async fn real_rate_fetcher_uses_bounded_injected_http_and_the_service_api_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let http = http(vec![
        (503, None, vec![]),
        (
            200,
            None,
            vec![
                br#"{"model":{"input_cost_per_token":0.001,"output_cost_per_token":0.002}}"#
                    .to_vec(),
            ],
        ),
    ]);
    let service = UsageService::with_dependencies(
        ServiceConfig::new(
            temp.path().join("base"),
            temp.path().join("cache"),
            temp.path().join("home"),
            "fixture".into(),
        ),
        Arc::new(TestClock),
        Arc::new(BoundedRateFetcher::new(http.clone())),
        Arc::new(NoProviderProbe),
    )
    .await
    .unwrap();
    let pricing = json!(service.refresh_rates().await.unwrap());
    assert_eq!(pricing["status"], "fresh");
    assert_eq!(pricing["knownModels"], 1);
    assert_eq!(
        *http.calls.lock().unwrap(),
        vec![
            (
                RATES_URL.into(),
                "application/json".into(),
                RATE_FETCH_TIMEOUT
            ),
            (
                RATES_API_URL.into(),
                "application/vnd.github.raw+json".into(),
                RATE_FETCH_TIMEOUT
            )
        ]
    );
    assert!(temp.path().join("cache/usage-model-rates.json").is_file());
}
#[tokio::test]
async fn bounds_streamed_and_declared_bodies_and_rejects_invalid_json_and_statuses() {
    for (status, length, chunks, expected_reads) in [
        (200, Some(MAX_RATE_BODY_BYTES as u64 + 1), vec![], 0),
        (
            200,
            None,
            vec![vec![b' '; MAX_RATE_BODY_BYTES], vec![b' ']],
            2,
        ),
        (200, None, vec![b"invalid".to_vec()], 2),
        (302, None, vec![], 0),
        (
            200,
            None,
            vec![format!("{}0{}", "[".repeat(65), "]".repeat(65)).into_bytes()],
            2,
        ),
    ] {
        let http = http(vec![(status, length, chunks)]);
        let fetcher = BoundedRateFetcher::new(http.clone());
        assert!(fetcher.fetch(RATES_URL, "application/json").await.is_err());
        assert_eq!(http.reads.load(Ordering::Relaxed), expected_reads);
    }
}
struct Never(bool);
impl RateHttpBody for Never {
    fn next_chunk(&mut self) -> ProbeFuture<'_, Option<Vec<u8>>> {
        Box::pin(std::future::pending())
    }
}
impl RateHttpClient for Never {
    fn get<'a>(&'a self, _: &'a str, _: &'a str, _: Duration) -> ProbeFuture<'a, RateHttpResponse> {
        Box::pin(async move {
            if self.0 {
                std::future::pending::<()>().await;
            }
            Ok(RateHttpResponse {
                status: 200,
                content_length: None,
                body: Box::new(Never(false)),
            })
        })
    }
}
#[tokio::test(start_paused = true)]
async fn bounds_the_complete_http_exchange_to_ten_seconds() {
    use std::{future::Future, task::Poll};
    for stalled_headers in [true, false] {
        let fetcher = BoundedRateFetcher::new(Arc::new(Never(stalled_headers)));
        let mut request = Box::pin(fetcher.fetch(RATES_URL, "application/json"));
        std::future::poll_fn(|cx| {
            assert!(request.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        tokio::time::advance(RATE_FETCH_TIMEOUT).await;
        std::future::poll_fn(|cx| {
            assert!(matches!(
                request.as_mut().poll(cx),
                Poll::Ready(Err(Error::Probe(_)))
            ));
            Poll::Ready(())
        })
        .await;
    }
}

use t3_contracts::{
    provider_instance::ProviderInstanceId,
    provider_usage_limits::{ProviderConsumeResetCreditResult, UsageLimitSourceAccount},
    server::ServerProviders,
};
use t3_usage::{
    limit_sources::SourceConfig, limits::LimitSourceProbe, runtime::*, sources::ProviderInstance,
};
use tokio::sync::watch;
struct Providers(watch::Sender<ProviderUsageSnapshot>);
impl t3_usage::sources::ProviderUsageProbe for Providers {}
impl ProviderUsageDiscovery for Providers {
    fn subscribe_usage(&self) -> watch::Receiver<ProviderUsageSnapshot> {
        self.0.subscribe()
    }
}
struct Hub;
impl LimitSourceProbe for Hub {
    fn accounts<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, Vec<UsageLimitSourceAccount>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn consume<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, ProviderConsumeResetCreditResult> {
        Box::pin(async { Ok(serde_json::from_value(json!({"outcome":"reset"})).unwrap()) })
    }
}
fn provider_limits(percent: u64) -> ServerProviders {
    serde_json::from_value(json!([{"instanceId":"claude","driver":"claudeAgent","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated","endpoint":"https://hub.test/v1"},"checkedAt":"2026-08-01T10:00:00.000Z","models":[],"slashCommands":[],"skills":[],"usageLimits":{"checkedAt":"2026-08-01T10:00:00.000Z","windows":[{"id":"session","kind":"session","label":"Session","usedPercent":percent}]}}])).unwrap()
}
#[tokio::test]
async fn one_constructor_supplies_live_history_config_changes_and_captureable_endpoints() {
    let temp = tempfile::tempdir().unwrap();
    let instance = |name: &str, output| {
        let home = temp.path().join(name);
        std::fs::create_dir_all(home.join("projects")).unwrap();
        std::fs::write(home.join("projects/session.jsonl"),format!("{}\n",json!({"type":"assistant","timestamp":"2026-08-01T10:00:00Z","requestId":name,"sessionId":name,"message":{"id":name,"model":"model","usage":{"input_tokens":2,"output_tokens":output}}}))).unwrap();
        let mut instance = ProviderInstance::new(t3_usage::UsageProviderKind::Claude);
        instance.home_path = Some(home);
        instance
            .environment
            .insert("PRIVATE_FIXTURE".into(), "must-not-publish".into());
        instance
    };
    let first = instance("first", 3);
    let second = instance("second", 7);
    let providers = Arc::new(Providers(
        watch::channel(ProviderUsageSnapshot {
            providers: provider_limits(10),
            instances: vec![first],
        })
        .0,
    ));
    let runtime = UsageRuntime::new(UsageRuntimeInputs {
        config: ServiceConfig::new(
            temp.path().join("base"),
            temp.path().join("cache"),
            temp.path().join("home"),
            "fixture".into(),
        ),
        clock: Arc::new(TestClock),
        http: http(vec![(
            200,
            None,
            vec![
                br#"{"model":{"input_cost_per_token":0.001,"output_cost_per_token":0.002}}"#
                    .to_vec(),
            ],
        )]),
        providers: providers.clone(),
        hub: Arc::new(Hub),
    })
    .await
    .unwrap();
    let mut subscription = runtime.subscribe_config();
    let Some(UsageConfigChange::Snapshot(initial)) = subscription.next().await else {
        panic!("initial snapshot")
    };
    assert_eq!(
        json!(initial.providers)[0]["usageLimits"]["windows"][0]["usedPercent"],
        10
    );
    assert!(
        !json!(initial.providers)
            .to_string()
            .contains("must-not-publish")
    );
    let captured = runtime
        .endpoint(&ProviderInstanceId("claude".into()))
        .unwrap();
    assert_eq!(captured.origin, "https://hub.test");
    let input = || {
        serde_json::from_value(
            json!({"timeZone":"UTC","sinceDay":"2026-08-01","untilDay":"2026-08-02"}),
        )
        .unwrap()
    };
    let first = json!(runtime.service().read_summary(input()).await.unwrap());
    assert_eq!(first["buckets"][0]["totals"]["outputTokens"], 3);
    providers.0.send_replace(ProviderUsageSnapshot {
        providers: provider_limits(90),
        instances: vec![second],
    });
    let Some(UsageConfigChange::Providers(update)) = subscription.next().await else {
        panic!("provider update")
    };
    assert_eq!(
        json!(update.providers)[0]["usageLimits"]["windows"][0]["usedPercent"],
        90
    );
    let second = json!(runtime.service().read_summary(input()).await.unwrap());
    assert_eq!(second["buckets"][0]["totals"]["outputTokens"], 7);
    runtime
        .refresh_sources(&[SourceConfig {
            id: "hub".into(),
            label: "hub".into(),
            url: "https://hub.test/path".into(),
            management_key: "fixture-only".into(),
            enabled: true,
        }])
        .await
        .unwrap();
    let Some(UsageConfigChange::Sources(update)) = subscription.next().await else {
        panic!("source update")
    };
    assert_eq!(json!(update.sources)[0]["origin"], "https://hub.test");
    assert_eq!(runtime.current_config().sources.len(), 1);
    runtime.refresh_sources(&[]).await.unwrap();
    let Some(UsageConfigChange::Sources(update)) = subscription.next().await else {
        panic!("clear")
    };
    assert!(update.sources.0.is_empty());
}
