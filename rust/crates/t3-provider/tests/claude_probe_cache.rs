#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_provider::{
    claude_probe_cache::ClaudeProbeCache, native_probe::*, sensitive::*, transport::ProviderClock,
};
use tokio::sync::Mutex;
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Probe {
    replies: Mutex<VecDeque<ServiceResult<Sensitive<Value>>>>,
    calls: AtomicUsize,
}
#[async_trait]
impl NativeProbe for Probe {
    async fn codex(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        unreachable!()
    }
    async fn claude(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.replies.lock().await.pop_front().unwrap()
    }
}
fn launch(home: &std::path::Path) -> ProbeLaunch {
    ProbeLaunch {
        executable: "fixture".into(),
        environment: Default::default(),
        cwd: home.into(),
        launch_args: String::new(),
        managed_mcp: false,
        include_status: true,
        gateway_discovery_grace: Duration::ZERO,
    }
}
#[tokio::test]
async fn only_fresh_usage_earns_the_ttl_and_transient_account_fallback_is_consumed_once() {
    let temp = tempfile::tempdir().unwrap();
    let clock = Arc::new(Clock(AtomicI64::new(1000)));
    let probe=Arc::new(Probe {replies:Mutex::new(vec![Ok(Sensitive::new(json!({"initialize":{"account":{"tokenSource":"oauth"}},"usage":{"five_hour":{"utilization":42}}}))),Err(ServiceError::Response),Err(ServiceError::Response)].into()),calls:AtomicUsize::new(0)});
    let cache = ClaudeProbeCache::new(
        probe.clone(),
        clock.clone(),
        temp.path().join(".claude.json"),
    );
    let first = cache.claude(launch(temp.path())).await.unwrap();
    clock.0.store(2000, Ordering::SeqCst);
    let cached = cache.claude(launch(temp.path())).await.unwrap();
    assert_eq!(first, cached);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    clock.0.store(302000, Ordering::SeqCst);
    let fallback = cache.claude(launch(temp.path())).await.unwrap();
    assert!(fallback.expose().get("usage").is_none());
    assert!(fallback.expose().get("usageCheckedAt").is_none());
    assert_eq!(
        cache.claude(launch(temp.path())).await.unwrap_err(),
        ServiceError::Response
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn switching_the_local_organization_never_reuses_another_accounts_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let account = temp.path().join(".claude.json");
    std::fs::write(
        &account,
        json!({"oauthAccount":{"organizationUuid":"first"}}).to_string(),
    )
    .unwrap();
    let clock = Arc::new(Clock(AtomicI64::new(1000)));
    let probe = Arc::new(Probe {
        replies: Mutex::new(
            vec![
                Ok(Sensitive::new(
                    json!({"initialize":{"account":{"tokenSource":"oauth"}},"usage":{}}),
                )),
                Err(ServiceError::Response),
            ]
            .into(),
        ),
        calls: AtomicUsize::new(0),
    });
    let cache = ClaudeProbeCache::new(probe, clock.clone(), account.clone());
    assert_eq!(
        cache.claude(launch(temp.path())).await.unwrap().expose()["workspaceId"],
        "first"
    );
    std::fs::write(
        account,
        json!({"oauthAccount":{"organizationUuid":"second"}}).to_string(),
    )
    .unwrap();
    clock.0.store(302000, Ordering::SeqCst);
    assert_eq!(
        cache.claude(launch(temp.path())).await.unwrap_err(),
        ServiceError::Response
    );
}
