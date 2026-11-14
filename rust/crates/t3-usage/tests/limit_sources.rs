#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::provider_usage_limits::{
    ProviderConsumeResetCreditResult, UsageLimitSourceAccount,
};
use t3_usage::{
    Error,
    limit_sources::{SourceConfig, UsageLimitSources, hub_pool_reset_at},
    limits::LimitSourceProbe,
    sources::ProbeFuture,
};
const AT: &str = "2026-08-01T10:00:00.000Z";

fn iso(ms: f64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn pool(
    windows: serde_json::Value,
) -> t3_contracts::provider_usage_limits::UsageLimitSourceSnapshot {
    serde_json::from_value(json!({"id":"hub","kind":"cliproxy","label":"hub","origin":"http://hub.test","checkedAt":AT,"accounts":windows})).unwrap()
}
fn account(driver: &str, windows: serde_json::Value) -> serde_json::Value {
    json!({"id":"account","driver":driver,"usageLimits":{"checkedAt":AT,"windows":windows}})
}
fn spent(id: &str, hours: u64) -> serde_json::Value {
    json!({"id":id,"kind":"weekly","label":id,"usedPercent":100,"resetsAt":iso(t3_usage::timestamp_ms(AT).unwrap()+hours as f64*3_600_000.0)})
}
#[test]
fn pool_recovers_at_the_earliest_accounts_latest_applicable_reset() {
    let cases = [
        (
            json!([
                account(
                    "claudeAgent",
                    json!([spent("five_hour", 1), spent("seven_day", 3)])
                ),
                account("claudeAgent", json!([spent("five_hour", 2)])),
                account("codex", json!([]))
            ]),
            Some("claude-haiku-5-5"),
            Some(2),
        ),
        (
            json!([
                account("claudeAgent", json!([spent("five_hour", 1)])),
                account("claudeAgent", json!([]))
            ]),
            Some("claude-haiku-5-5"),
            None,
        ),
        (
            json!([{"id":"unread","driver":"claudeAgent","usageLimits":{"checkedAt":AT,"windows":[],"unavailable":{"reason":"probeFailed"}}}]),
            None,
            None,
        ),
        (
            json!([account(
                "claudeAgent",
                json!([{ "id":"five_hour","kind":"session","label":"Session","usedPercent":100}])
            )]),
            None,
            None,
        ),
        (
            json!([account("claudeAgent", json!([spent("five_hour", 0)]))]),
            None,
            None,
        ),
        (
            json!([account("claudeAgent", json!([spent("seven_day_fable", 5)]))]),
            Some("claude-haiku-5-5"),
            None,
        ),
        (
            json!([account("claudeAgent", json!([spent("seven_day_fable", 5)]))]),
            Some("claude-fable-5-2"),
            Some(5),
        ),
        (
            json!([account("claudeAgent", json!([spent("seven_day_fable", 5)]))]),
            None,
            None,
        ),
        (
            json!([account("codex", json!([spent("five_hour", 1)]))]),
            None,
            None,
        ),
    ];
    let now = t3_usage::timestamp_ms(AT).unwrap();
    for (accounts, model, hours) in cases {
        assert_eq!(
            hub_pool_reset_at(&[pool(accounts)], "claudeAgent", model, now),
            hours.map(|h| iso(now + h as f64 * 3_600_000.0))
        );
    }
}

struct PoolProbe {
    accounts: std::sync::Mutex<Vec<UsageLimitSourceAccount>>,
    pending: std::sync::atomic::AtomicBool,
    reads: AtomicUsize,
}
impl LimitSourceProbe for PoolProbe {
    fn accounts<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, Vec<UsageLimitSourceAccount>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            if self.pending.load(Ordering::Relaxed) {
                std::future::pending::<()>().await;
            }
            Ok(self.accounts.lock().unwrap().clone())
        })
    }
    fn consume<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, ProviderConsumeResetCreditResult> {
        Box::pin(std::future::pending())
    }
}
struct Clock;
impl t3_usage::service::Clock for Clock {
    fn now_ms(&self) -> f64 {
        t3_usage::timestamp_ms(AT).unwrap()
    }
}
#[tokio::test(start_paused = true)]
async fn reset_fallback_refreshes_only_a_matching_endpoint_and_bounds_stalled_refreshes() {
    let mut cached = account("claudeAgent", json!([spent("five_hour", 2)]));
    cached["usageLimits"]["windows"][0]["usedPercent"] = json!(99);
    let probe = Arc::new(PoolProbe {
        accounts: std::sync::Mutex::new(vec![serde_json::from_value(cached).unwrap()]),
        pending: std::sync::atomic::AtomicBool::new(false),
        reads: AtomicUsize::new(0),
    });
    let sources = UsageLimitSources::new(probe.clone());
    let configs = [config("hub", "http://hub.test/path")];
    sources.refresh(&configs, AT).await.unwrap();
    assert_eq!(
        sources
            .reset_at(&configs, "claudeAgent", "http://other.test", None, &Clock)
            .await,
        None
    );
    assert_eq!(probe.reads.load(Ordering::Relaxed), 1);
    *probe.accounts.lock().unwrap() = vec![
        serde_json::from_value(account("claudeAgent", json!([spent("five_hour", 2)]))).unwrap(),
    ];
    assert_eq!(
        sources
            .reset_at(&configs, "claudeAgent", "http://hub.test", None, &Clock)
            .await,
        Some(iso(t3_usage::timestamp_ms(AT).unwrap() + 7_200_000.0))
    );
    probe.accounts.lock().unwrap()[0].usage_limits.windows[0].used_percent =
        serde_json::from_value(json!(99)).unwrap();
    sources.refresh(&configs, AT).await.unwrap();
    probe.pending.store(true, Ordering::Relaxed);
    use std::{future::Future, task::Poll};
    let mut stalled =
        Box::pin(sources.reset_at(&configs, "claudeAgent", "http://hub.test", None, &Clock));
    std::future::poll_fn(|cx| {
        assert!(stalled.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    std::future::poll_fn(|cx| {
        assert!(
            matches!(stalled.as_mut().poll(cx), Poll::Ready(None)),
            "refresh exceeded five seconds"
        );
        Poll::Ready(())
    })
    .await;
    sources.refresh(&[], AT).await.unwrap();
    assert!(sources.current().is_empty());
}
struct PendingProbe;
impl LimitSourceProbe for PendingProbe {
    fn accounts<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, Vec<UsageLimitSourceAccount>> {
        Box::pin(std::future::pending())
    }
    fn consume<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, ProviderConsumeResetCreditResult> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test(start_paused = true)]
async fn stalled_hub_requests_time_out_and_release_the_refresh_lock() {
    let sources = UsageLimitSources::new(Arc::new(PendingProbe));
    let configs = [config("one", "https://hub.invalid")];
    sources.refresh(&configs, AT).await.unwrap();
    assert_eq!(
        json!(sources.current())[0]["error"],
        "The hub did not answer within 15 seconds."
    );
    assert!(
        sources
            .consume_reset_credit(&configs, "one", "account", "credit", AT)
            .await
            .is_err()
    );
    sources.refresh(&[], AT).await.unwrap();
    assert!(sources.current().is_empty());
}
struct Probe {
    reads: AtomicUsize,
    consumes: AtomicUsize,
}
impl LimitSourceProbe for Probe {
    fn accounts<'a>(
        &'a self,
        url: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, Vec<UsageLimitSourceAccount>> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if url.contains("fail") {
                return Err(Error::Probe("The hub answered with HTTP 502.".into()));
            }
            Ok(vec![serde_json::from_value(json!({"id":"account-1","driver":"codex","email":"test@example.invalid","usageLimits":{"checkedAt":AT,"windows":[]}})).unwrap()])
        })
    }
    fn consume<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> ProbeFuture<'a, ProviderConsumeResetCreditResult> {
        Box::pin(async move {
            self.consumes.fetch_add(1, Ordering::Relaxed);
            Ok(serde_json::from_value(json!({"outcome":"reset"})).unwrap())
        })
    }
}
fn config(id: &str, url: &str) -> SourceConfig {
    SourceConfig {
        id: id.into(),
        url: url.into(),
        label: String::new(),
        management_key: "fixture-management-key".into(),
        enabled: true,
        key_is_reference: false,
    }
}
#[tokio::test]
async fn refresh_preserves_order_reports_errors_and_publishes_only_changes() {
    let probe = Arc::new(Probe {
        reads: AtomicUsize::new(0),
        consumes: AtomicUsize::new(0),
    });
    let sources = UsageLimitSources::new(probe);
    let mut changes = sources.subscribe();
    let mut disabled = config("disabled", "https://disabled.invalid");
    disabled.enabled = false;
    sources
        .refresh(
            &[
                config("one", "https://hub.invalid:8317/path"),
                config("two", "https://fail.invalid"),
                disabled,
            ],
            AT,
        )
        .await
        .unwrap();
    changes.changed().await.unwrap();
    let current = json!(sources.current());
    assert_eq!(current.as_array().unwrap().len(), 2);
    assert_eq!(current[0]["label"], "hub.invalid:8317");
    assert_eq!(current[0]["origin"], "https://hub.invalid:8317");
    assert_eq!(current[0]["accounts"][0]["id"], "account-1");
    assert!(current[1]["error"] == "The hub answered with HTTP 502.");
    sources
        .refresh(
            &[
                config("one", "https://hub.invalid:8317/path"),
                config("two", "https://fail.invalid"),
            ],
            AT,
        )
        .await
        .unwrap();
    assert!(!changes.has_changed().unwrap());
    sources.refresh(&[], AT).await.unwrap();
    changes.changed().await.unwrap();
    assert!(sources.current().is_empty());
}

#[tokio::test]
async fn only_http_hubs_advertise_an_origin_even_when_credentials_are_missing() {
    let sources = UsageLimitSources::new(Arc::new(PendingProbe));
    let configs: Vec<_> = [
        "",
        "not a URL",
        "file:///tmp/hub",
        "https://hub.test:8317/v1",
        "ssh://host",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, url)| {
        let mut config = config(&format!("source-{i}"), url);
        config.management_key.clear();
        config
    })
    .collect();
    sources.refresh(&configs, AT).await.unwrap();
    let origins: Vec<_> = sources
        .current()
        .iter()
        .map(|s| json!(s)["origin"].clone())
        .collect();
    assert_eq!(
        origins,
        vec![
            json!(null),
            json!(null),
            json!(null),
            json!("https://hub.test:8317"),
            json!(null)
        ]
    );
}
#[tokio::test]
async fn redemption_checks_config_and_refreshes_the_affected_source() {
    let probe = Arc::new(Probe {
        reads: AtomicUsize::new(0),
        consumes: AtomicUsize::new(0),
    });
    let sources = UsageLimitSources::new(probe.clone());
    let config = config("one", "https://hub.invalid");
    assert!(
        sources
            .consume_reset_credit(&[], "one", "account-1", "credit-1", AT)
            .await
            .is_err()
    );
    sources
        .refresh(std::slice::from_ref(&config), AT)
        .await
        .unwrap();
    let result = sources
        .consume_reset_credit(&[config], "one", "account-1", "credit-1", AT)
        .await
        .unwrap();
    assert_eq!(json!(result)["outcome"], "reset");
    assert_eq!(probe.consumes.load(Ordering::Relaxed), 1);
    assert_eq!(probe.reads.load(Ordering::Relaxed), 2);
}
#[tokio::test]
async fn missing_management_key_is_a_snapshot_error_and_blocks_redemption() {
    let probe = Arc::new(Probe {
        reads: AtomicUsize::new(0),
        consumes: AtomicUsize::new(0),
    });
    let sources = UsageLimitSources::new(probe.clone());
    let mut config = config("one", "https://hub.invalid");
    config.management_key.clear();
    sources
        .refresh(std::slice::from_ref(&config), AT)
        .await
        .unwrap();
    assert_eq!(
        json!(sources.current())[0]["error"],
        "No management key configured."
    );
    assert_eq!(probe.reads.load(Ordering::Relaxed), 0);
    assert!(
        sources
            .consume_reset_credit(&[config], "one", "a", "c", AT)
            .await
            .is_err()
    );
}
