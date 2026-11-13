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
    limit_sources::{SourceConfig, UsageLimitSources},
    limits::LimitSourceProbe,
    sources::ProbeFuture,
};
const AT: &str = "2026-08-01T10:00:00.000Z";
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
        "The hub management request failed."
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
                return Err(Error::Probe(
                    "unreachable fixture hub; secret-key=private; prompt=private".into(),
                ));
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
    assert_eq!(current[0]["accounts"][0]["id"], "account-1");
    assert!(current[1]["error"] == "The hub management request failed.");
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
