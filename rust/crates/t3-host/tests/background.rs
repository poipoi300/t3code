use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use t3_contracts::{background::*, base_schemas::*, settings::ServerSettings};
use t3_host::background::*;

struct Time(AtomicI64);
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn activity(id: &str, ttl: i64) -> ClientActivityReportInput {
    serde_json::from_value(serde_json::json!({"clientId":id,"clientKind":"web","visible":true,"focused":true,"recentlyInteracted":false,"scopes":[{"type":"server-config"}],"ttlMs":ttl,"observedAt":"2099-01-01T00:00:00.000Z"})).unwrap()
}
fn fixture() -> (
    Arc<Time>,
    Arc<HostPowerMonitor>,
    BackgroundPolicy,
    ActivitySettings,
) {
    let time = Arc::new(Time(AtomicI64::new(1_000_000)));
    let power = Arc::new(HostPowerMonitor::unknown(time.now_ms()).unwrap());
    let service = BackgroundPolicy::new(time.clone(), power.clone());
    (
        time,
        power,
        service,
        ActivitySettings::resolve(&ServerSettings::default()),
    )
}
#[tokio::test]
async fn ttl_is_server_based_clamped_and_expires_at_boundary() {
    let (time, _, service, settings) = fixture();
    service
        .report_client_activity(AuthSessionId("s".into()), RpcClientId(1), activity("c", 1))
        .await
        .unwrap();
    assert!(
        service
            .should_run_scope_work(&BackgroundScope::ServerConfig, &settings)
            .await
            .unwrap()
    );
    assert!(
        !service
            .should_run_scope_work(&BackgroundScope::Diagnostics, &settings)
            .await
            .unwrap()
    );
    time.0.fetch_add(999, Ordering::SeqCst);
    assert_eq!(service.snapshot(&settings).await.unwrap().leases.len(), 1);
    time.0.fetch_add(1, Ordering::SeqCst);
    assert!(service.snapshot(&settings).await.unwrap().leases.is_empty());
    service
        .report_client_activity(
            AuthSessionId("s".into()),
            RpcClientId(1),
            activity("c", 999999),
        )
        .await
        .unwrap();
    time.0.fetch_add(120000, Ordering::SeqCst);
    assert!(service.snapshot(&settings).await.unwrap().leases.is_empty());
}
#[tokio::test]
async fn lease_limit_eviction_is_per_connection_and_upserts_do_not_evict() {
    let (_, _, service, settings) = fixture();
    for id in 0..17 {
        service
            .report_client_activity(
                AuthSessionId("s".into()),
                RpcClientId(1),
                activity(&id.to_string(), 45000),
            )
            .await
            .unwrap();
    }
    service
        .report_client_activity(
            AuthSessionId("other".into()),
            RpcClientId(1),
            activity("0", 45000),
        )
        .await
        .unwrap();
    let snapshot = service.snapshot(&settings).await.unwrap();
    assert_eq!(snapshot.leases.len(), 17);
    assert_eq!(snapshot.leases[0].client_id, "1");
    service
        .report_client_activity(
            AuthSessionId("s".into()),
            RpcClientId(1),
            activity("1", 45000),
        )
        .await
        .unwrap();
    assert_eq!(service.snapshot(&settings).await.unwrap().leases.len(), 17);
    service
        .remove_rpc_client(&AuthSessionId("s".into()), &RpcClientId(1))
        .await;
    assert_eq!(
        service.snapshot(&settings).await.unwrap().leases[0]
            .session_id
            .0,
        "other"
    );
}
#[tokio::test]
async fn foreground_power_and_profile_gates_are_independent() {
    let (_, power, service, mut settings) = fixture();
    let mut input = activity("c", 45000);
    input.focused = false;
    service
        .report_client_activity(AuthSessionId("s".into()), RpcClientId(1), input.clone())
        .await
        .unwrap();
    assert!(
        !service
            .should_run_scope_work(&BackgroundScope::ServerConfig, &settings)
            .await
            .unwrap()
    );
    settings.performance = true;
    assert!(
        service
            .should_run_scope_work(&BackgroundScope::ServerConfig, &settings)
            .await
            .unwrap()
    );
    assert!(
        !service
            .should_run_opportunistic_work(&settings)
            .await
            .unwrap()
    );
    input.recently_interacted = true;
    input.low_power_mode = Some(BackgroundBooleanState::True);
    service
        .report_client_activity(AuthSessionId("s".into()), RpcClientId(1), input)
        .await
        .unwrap();
    assert!(
        !service
            .should_run_scope_work(&BackgroundScope::ServerConfig, &settings)
            .await
            .unwrap()
    );
    settings.pause_when_client_low_power = false;
    let mut report = power.snapshot();
    report.suspended = true;
    report.stale = false;
    power.report(report.clone()).await.unwrap();
    assert!(
        !service
            .should_run_scope_work(&BackgroundScope::ServerConfig, &settings)
            .await
            .unwrap()
    );
    report.stale = true;
    power.report(report).await.unwrap();
    assert!(
        service
            .should_run_opportunistic_work(&settings)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn power_discards_older_reports_and_does_not_publish_idle_only_changes() {
    let (_, power, _, _) = fixture();
    let mut events = power.stream_changes();
    let mut report = power.snapshot();
    report.updated_at.0 = "1970-01-01T00:16:41.000Z".into();
    report.idle_seconds = Some(t3_contracts::speech::SpeechNumber::Finite(5_u32.into()));
    power.report(report.clone()).await.unwrap();
    assert!(!events.has_changed().unwrap());
    assert_eq!(power.snapshot(), report);
    report.suspended = true;
    power.report(report.clone()).await.unwrap();
    events.changed().await.unwrap();
    assert_eq!(*events.borrow_and_update(), report);
    report.updated_at.0 = "1970-01-01T00:16:40.000Z".into();
    report.suspended = false;
    power.report(report).await.unwrap();
    assert!(power.snapshot().suspended);
}
