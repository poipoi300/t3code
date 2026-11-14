#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    provider_usage_limits::ServerProviderUsageLimits,
    server::{ServerProvider, ServerProviderState},
};
use t3_provider::{managed_provider::*, sensitive::*, service::ProviderInstanceService};
use tokio::sync::{mpsc, oneshot, watch};
struct Request {
    answer: oneshot::Sender<ServiceResult<ServerProvider>>,
    demanded: bool,
}
struct Source {
    id: ProviderInstanceId,
    requests: mpsc::UnboundedSender<Request>,
    policy: watch::Receiver<BackgroundPolicy>,
    enrichment: Option<mpsc::UnboundedSender<Enrichment>>,
    update_checks: AtomicBool,
}
struct Enrichment {
    snapshot: ServerProvider,
    answer: oneshot::Sender<ServiceResult<ServerProvider>>,
}
#[async_trait]
impl ProviderInstanceService for Source {
    async fn configure_update_checks(&self, enabled: bool) -> ServiceResult<bool> {
        Ok(self.update_checks.swap(enabled, Ordering::SeqCst) != enabled)
    }
    fn has_enrichment(&self) -> bool {
        self.enrichment.is_some()
    }
    async fn enrich_snapshot(&self, snapshot: ServerProvider) -> ServiceResult<ServerProvider> {
        let (answer, response) = oneshot::channel();
        self.enrichment
            .as_ref()
            .unwrap()
            .send(Enrichment { snapshot, answer })
            .unwrap();
        response.await.map_err(|_| ServiceError::Response)?
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.id
    }
    async fn snapshot(&self, _: Option<&str>, _: bool) -> ServiceResult<ServerProvider> {
        let (answer, response) = oneshot::channel();
        self.requests
            .send(Request {
                answer,
                demanded: self.policy.borrow().demanded,
            })
            .unwrap();
        response.await.map_err(|_| ServiceError::Response)?
    }
}
struct Usage(AtomicUsize);
#[async_trait]
impl ManagedUsage for Usage {
    async fn invalidate(&self) -> ServiceResult<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn check(&self) -> ServiceResult<Option<ServerProviderUsageLimits>> {
        panic!("usage recovery is not requested")
    }
}
fn snapshot(status: &str) -> ServerProvider {
    serde_json::from_value(json!({"instanceId":"codex","driver":"codex","enabled":true,"installed":true,"version":"1.0.0","status":status,"auth":{"status":"authenticated","email":"user@example.invalid","workspaceId":"workspace","label":"Original"},"checkedAt":"2026-04-10T00:00:00.000Z","models":[],"skills":[],"slashCommands":[],"usageLimits":{"checkedAt":"2026-04-10T00:00:00.000Z","windows":[{"id":"weekly","kind":"weekly","label":"Weekly","usedPercent":20}]}})).unwrap()
}
fn setup(
    initial: ServerProvider,
    usage: Option<Arc<dyn ManagedUsage>>,
) -> (
    Arc<ManagedProvider>,
    watch::Sender<BackgroundPolicy>,
    mpsc::UnboundedReceiver<Request>,
) {
    setup_all(initial, usage, None)
}
fn setup_all(
    initial: ServerProvider,
    usage: Option<Arc<dyn ManagedUsage>>,
    enrichment: Option<mpsc::UnboundedSender<Enrichment>>,
) -> (
    Arc<ManagedProvider>,
    watch::Sender<BackgroundPolicy>,
    mpsc::UnboundedReceiver<Request>,
) {
    let (policy, background) = watch::channel(BackgroundPolicy::default());
    let (requests, events) = mpsc::unbounded_channel();
    let source = Arc::new(Source {
        id: "codex".into(),
        requests,
        policy: background.clone(),
        enrichment,
        update_checks: AtomicBool::new(false),
    });
    let mut options = ManagedOptions::new(background);
    options.usage = usage;
    (
        ManagedProvider::new(source, initial, options).unwrap(),
        policy,
        events,
    )
}
async fn observed(
    updates: &mut watch::Receiver<Option<ServerProvider>>,
    matches: impl Fn(&ServerProvider) -> bool,
) -> ServerProvider {
    loop {
        let current = updates.borrow_and_update().clone();
        if let Some(current) = current
            && matches(&current)
        {
            return current;
        }
        updates.changed().await.unwrap();
    }
}
#[tokio::test(start_paused = true)]
async fn startup_waits_for_server_readiness_periodic_probes_need_demand_and_one_transient_failure_is_deferred()
 {
    let (provider, policy, mut requests) = setup(snapshot("warning"), None);
    let mut changes = provider.subscribe_snapshots().unwrap();
    assert!(provider.is_awaiting_first_check().await);
    assert!(requests.try_recv().is_err());
    policy.send_modify(|policy| policy.server_ready = true);
    let initial = requests.recv().await.unwrap();
    assert!(!initial.demanded);
    initial.answer.send(Ok(snapshot("ready"))).unwrap();
    observed(&mut changes, |snapshot| {
        snapshot.status == ServerProviderState::Ready
    })
    .await;
    assert!(!provider.is_awaiting_first_check().await);
    tokio::time::advance(Duration::from_secs(120)).await;
    policy.send_modify(|policy| policy.demanded = true);
    let failure = requests.recv().await.unwrap();
    assert!(failure.demanded);
    failure.answer.send(Ok(snapshot("error"))).unwrap();
    let retry = requests.recv().await.unwrap();
    assert!(retry.demanded);
    assert_eq!(provider.current().await.status, ServerProviderState::Ready);
    retry.answer.send(Ok(snapshot("error"))).unwrap();
    observed(&mut changes, |snapshot| {
        snapshot.status == ServerProviderState::Error
    })
    .await;
}
#[tokio::test]
async fn usage_during_a_probe_survives_missing_results_and_stale_enrichment_cannot_revert_limits_or_cleared_labels()
 {
    let (provider, _policy, mut requests) = setup(snapshot("ready"), None);
    let stale = provider.generation().await;
    let refresh = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, false).await }
    });
    let request = requests.recv().await.unwrap();
    provider
        .apply_usage(RuntimeUsageUpdate {
            windows: serde_json::from_value(
                json!([{"id":"weekly","kind":"weekly","label":"Weekly","usedPercent":70}]),
            )
            .unwrap(),
            checked_at: t3_contracts::wire::WireDateTime("2026-04-10T00:00:02.000Z".into()),
            complete: false,
            auth_label: Some("Pro".into()),
        })
        .await;
    let mut probed = snapshot("ready");
    probed.usage_limits = None;
    probed.auth.workspace_id = None;
    request.answer.send(Ok(probed)).unwrap();
    let current = refresh.await.unwrap().unwrap();
    assert_eq!(
        current
            .usage_limits
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .windows[0]
            .used_percent,
        serde_json::from_value(json!(70)).unwrap()
    );
    assert_eq!(current.auth.workspace_id, Some(Some("workspace".into())));
    assert_eq!(current.auth.label, Some(Some("Pro".into())));
    assert!(!provider.publish_enrichment(stale, snapshot("error")).await);
    provider
        .apply_usage(RuntimeUsageUpdate {
            windows: serde_json::from_value(
                json!([{"id":"weekly","kind":"weekly","label":"Weekly","usedPercent":80}]),
            )
            .unwrap(),
            checked_at: t3_contracts::wire::WireDateTime("2026-04-10T00:00:03.000Z".into()),
            complete: true,
            auth_label: None,
        })
        .await;
    let mut enriched = snapshot("ready");
    enriched.version = Some("2.0.0".into());
    assert!(
        provider
            .publish_enrichment(provider.generation().await, enriched)
            .await
    );
    let current = provider.current().await;
    assert!(current.auth.label.is_none());
    assert_eq!(
        current.usage_limits.unwrap().unwrap().windows[0].used_percent,
        serde_json::from_value(json!(80)).unwrap()
    );
}
#[tokio::test]
async fn cancelling_running_usage_refresh_preserves_the_waiting_claim_and_coalesces_later_refreshes()
 {
    let usage = Arc::new(Usage(AtomicUsize::new(0)));
    let (provider, _policy, mut requests) = setup(snapshot("ready"), Some(usage.clone()));
    let running = tokio::spawn({
        let provider = provider.clone();
        async move { provider.refresh_usage().await }
    });
    let first = requests.recv().await.unwrap();
    let queued = provider.refresh_usage();
    tokio::pin!(queued);
    std::future::poll_fn(|cx| {
        assert!(queued.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    drop(first);
    let extra = provider.refresh_usage();
    tokio::pin!(extra);
    std::future::poll_fn(|cx| {
        assert!(extra.as_mut().poll(cx).is_ready());
        std::task::Poll::Ready(())
    })
    .await;
    let (result, ()) = tokio::join!(queued, async {
        requests
            .recv()
            .await
            .unwrap()
            .answer
            .send(Ok(snapshot("ready")))
            .unwrap();
    });
    result.unwrap();
    assert_eq!(usage.0.load(Ordering::SeqCst), 2);
    assert!(requests.try_recv().is_err());
}
#[tokio::test]
async fn settings_recheck_unauthenticated_instances_even_without_config_changes_and_failures_allow_a_later_retry()
 {
    let mut initial = snapshot("error");
    initial.auth.status = t3_contracts::server::ServerProviderAuthStatus::Unauthenticated;
    let (provider, _policy, mut requests) = setup(initial, None);
    for result in [Err(ServiceError::Response), Ok(snapshot("ready"))] {
        let refresh = tokio::spawn({
            let provider = provider.clone();
            async move { provider.settings_applied(false, false).await }
        });
        requests
            .recv()
            .await
            .unwrap()
            .answer
            .send(result.clone())
            .unwrap();
        assert_eq!(refresh.await.unwrap(), result);
    }
    assert_eq!(provider.current().await.status, ServerProviderState::Ready);
}

#[tokio::test]
async fn dropping_a_managed_provider_cancels_its_pending_owned_background_probe() {
    let (provider, policy, mut requests) = setup(snapshot("warning"), None);
    policy.send_modify(|policy| policy.server_ready = true);
    let mut request = requests.recv().await.unwrap();
    let weak = Arc::downgrade(&provider);
    drop(provider);
    request.answer.closed().await;
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn correlated_cache_hydrates_only_the_placeholder_before_the_first_successful_check() {
    let (provider, _policy, mut requests) = setup(snapshot("warning"), None);
    let cached = snapshot("ready");
    provider
        .hydrate_cached_snapshot(cached.clone())
        .await
        .unwrap();
    assert_eq!(provider.current().await.status, ServerProviderState::Ready);
    assert!(provider.is_awaiting_first_check().await);
    let refresh = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, false).await }
    });
    let mut fresh = cached.clone();
    fresh.version = Some("2.0.0".into());
    requests
        .recv()
        .await
        .unwrap()
        .answer
        .send(Ok(fresh.clone()))
        .unwrap();
    refresh.await.unwrap().unwrap();
    assert_eq!(
        provider
            .hydrate_cached_snapshot(cached)
            .await
            .unwrap()
            .version,
        fresh.version
    );
}

#[tokio::test]
async fn enrichment_runs_after_the_base_check_and_is_cancelled_on_refresh_and_teardown() {
    let (enrichment, mut callbacks) = mpsc::unbounded_channel();
    let (provider, _policy, mut requests) = setup_all(snapshot("warning"), None, Some(enrichment));
    let mut changes = provider.subscribe_snapshots().unwrap();
    let first = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, false).await }
    });
    requests
        .recv()
        .await
        .unwrap()
        .answer
        .send(Ok(snapshot("ready")))
        .unwrap();
    first.await.unwrap().unwrap();
    let mut old = callbacks.recv().await.unwrap();
    assert_eq!(old.snapshot.status, ServerProviderState::Ready);
    let second = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, false).await }
    });
    let mut newer = snapshot("ready");
    newer.version = Some("2.0.0".into());
    requests
        .recv()
        .await
        .unwrap()
        .answer
        .send(Ok(newer))
        .unwrap();
    second.await.unwrap().unwrap();
    old.answer.closed().await;
    let mut current = callbacks.recv().await.unwrap();
    current.snapshot.version = Some("3.0.0".into());
    current.answer.send(Ok(current.snapshot)).unwrap();
    observed(&mut changes, |snapshot| {
        snapshot
            .version
            .as_ref()
            .is_some_and(|version| version.0 == "3.0.0")
    })
    .await;
    let third = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, false).await }
    });
    requests
        .recv()
        .await
        .unwrap()
        .answer
        .send(Ok(snapshot("ready")))
        .unwrap();
    third.await.unwrap().unwrap();
    let mut pending = callbacks.recv().await.unwrap();
    drop(provider);
    pending.answer.closed().await;
}

#[tokio::test]
async fn update_policy_changes_cancel_enrichment_without_probing_or_starting_before_readiness() {
    let (tx, mut callbacks) = mpsc::unbounded_channel();
    let (provider, _policy, mut requests) = setup_all(snapshot("warning"), None, Some(tx));
    assert!(provider.configure_update_checks(true).await.unwrap());
    assert!(requests.try_recv().is_err());
    assert!(callbacks.try_recv().is_err());
    let check = tokio::spawn({
        let provider = provider.clone();
        async move { provider.snapshot(None, false).await.unwrap() }
    });
    requests
        .recv()
        .await
        .unwrap()
        .answer
        .send(Ok(snapshot("ready")))
        .unwrap();
    check.await.unwrap();
    let mut old = callbacks.recv().await.unwrap();
    assert!(provider.configure_update_checks(false).await.unwrap());
    old.answer.closed().await;
    let mut next = callbacks.recv().await.unwrap();
    next.snapshot.version = Some("2.0.0".into());
    next.answer.send(Ok(next.snapshot)).unwrap();
    let mut updates = provider.subscribe_snapshots().unwrap();
    observed(&mut updates, |snapshot| {
        snapshot
            .version
            .as_ref()
            .is_some_and(|version| version.0 == "2.0.0")
    })
    .await;
    assert!(requests.try_recv().is_err());
}
