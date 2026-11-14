#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use t3_contracts::{
    provider_instance::ProviderInstanceId, server::ServerProvider, settings::ServerSettings,
};
use t3_provider::{
    auth::AuthService,
    catalog::Manifest,
    host::*,
    instances::{InstanceRegistry, InstanceSpec, core_driver_definitions},
    secrets::{OnePassword, SecretResolver},
    sensitive::*,
    service::*,
    transport::ProviderClock,
};
use tokio::sync::mpsc;
struct Clock;
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
struct Effects {
    events: mpsc::UnboundedSender<String>,
    reject_retirement: AtomicBool,
    update_checks: Arc<AtomicBool>,
}
#[async_trait]
impl ProviderSessions for Effects {
    async fn retire(&self, id: &ProviderInstanceId) -> ServiceResult<()> {
        self.events.send(format!("retire:{}", id.0)).unwrap();
        if id.0 == "work" && self.reject_retirement.swap(false, Ordering::SeqCst) {
            return Err(ServiceError::StopSessions);
        }
        Ok(())
    }
}
#[async_trait]
impl CatalogRefresh for Effects {
    async fn refresh(&self) -> ServiceResult<()> {
        Ok(())
    }
}
#[async_trait]
impl MaintenanceExecutor for Effects {
    async fn execute(&self, _: t3_provider::maintenance::MaintenanceAction) -> ServiceResult<()> {
        panic!("host reconciliation must not run maintenance")
    }
}
struct Source {
    spec: Arc<InstanceSpec>,
    value: ServerProvider,
    events: mpsc::UnboundedSender<String>,
    update_checks: Arc<AtomicBool>,
}
impl Drop for Source {
    fn drop(&mut self) {
        let _ = self
            .events
            .send(format!("drop:{}", self.spec.instance_id.0));
    }
}
#[async_trait]
impl ProviderInstanceService for Source {
    async fn configure_update_checks(&self, enabled: bool) -> ServiceResult<bool> {
        Ok(self.update_checks.swap(enabled, Ordering::SeqCst) != enabled)
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    async fn snapshot(&self, _: Option<&str>, _: bool) -> ServiceResult<ServerProvider> {
        Ok(self.value.clone())
    }
}
#[async_trait]
impl ProviderFactory for Effects {
    async fn create(&self, spec: Arc<InstanceSpec>) -> ServiceResult<ProviderRuntime> {
        self.events
            .send(format!(
                "create:{}:{}",
                spec.instance_id.0,
                spec.config["launchArgs"].as_str().unwrap_or_default()
            ))
            .unwrap();
        let value:ServerProvider=serde_json::from_value(json!({"instanceId":spec.instance_id,"driver":spec.driver,"enabled":spec.enabled,"installed":true,"version":spec.config["launchArgs"].as_str().filter(|value|!value.is_empty()).unwrap_or("default"),"status":"ready","auth":{"status":"unknown"},"checkedAt":"2026-04-10T00:00:00.000Z","models":[],"skills":[],"slashCommands":[]})).unwrap();
        Ok(ProviderRuntime {
            source: Arc::new(Source {
                spec,
                value: value.clone(),
                events: self.events.clone(),
                update_checks: self.update_checks.clone(),
            }),
            initial: value,
            auth: None,
        })
    }
}
#[tokio::test]
async fn replacement_waits_for_retirement_retries_failures_and_parks_unavailable_instances_without_losing_the_old_runtime()
 {
    let fixture = common::Fixture::new(json!({}));
    let (events, mut received) = mpsc::unbounded_channel();
    let effects = Arc::new(Effects {
        events,
        reject_retirement: AtomicBool::new(false),
        update_checks: Arc::new(AtomicBool::new(false)),
    });
    let service = ProviderService::new(
        Arc::new(AuthService::default()),
        effects.clone(),
        effects.clone(),
        Arc::new(Clock),
        Arc::new(Manifest::bundled().unwrap()),
        "0.1.0".into(),
    );
    let host = ProviderHost::new(
        Arc::new(InstanceRegistry::new(core_driver_definitions())),
        service.clone(),
        BTreeMap::from([("codex".into(), effects.clone() as Arc<dyn ProviderFactory>)]),
        effects.clone(),
        Arc::new(SecretResolver::new(OnePassword::with_executable(
            fixture.binary(),
            fixture.environment.clone(),
        ))),
        Arc::new(Clock),
    );
    let mut settings:ServerSettings=serde_json::from_value(json!({"providerInstances":{"work":{"driver":"codex","enabled":true,"config":{"launchArgs":"one"}},"invalid":{"driver":"pi","config":{"binaryPath":2}}}})).unwrap();
    let first = host
        .reconcile(&settings, &BTreeSet::new(), false, &BTreeMap::new())
        .await
        .unwrap();
    assert!(first.activated.contains(&"work".into()));
    let snapshots = service.get_providers().await;
    let parked = snapshots
        .0
        .iter()
        .find(|snapshot| snapshot.instance_id.0 == "invalid")
        .unwrap();
    let parked = serde_json::to_value(parked).unwrap();
    assert_eq!(parked["availability"], "unavailable");
    assert!(parked.get("setup").is_none());
    while received.try_recv().is_ok() {}
    assert!(
        host.reconcile(&settings, &BTreeSet::new(), false, &BTreeMap::new())
            .await
            .unwrap()
            .activated
            .is_empty()
    );
    assert!(received.try_recv().is_err());
    settings
        .provider_instances
        .get_mut(&"work".into())
        .unwrap()
        .config = Some(json!({"launchArgs":"two"}));
    effects.reject_retirement.store(true, Ordering::SeqCst);
    assert_eq!(
        host.reconcile(&settings, &BTreeSet::new(), false, &BTreeMap::new())
            .await
            .unwrap_err(),
        ServiceError::StopSessions
    );
    assert_eq!(received.recv().await.unwrap(), "retire:work");
    assert!(received.try_recv().is_err());
    assert_eq!(
        service
            .get_providers()
            .await
            .0
            .iter()
            .find(|snapshot| snapshot.instance_id.0 == "work")
            .unwrap()
            .version
            .as_ref()
            .unwrap()
            .0,
        "one"
    );
    let retry = host
        .reconcile(&settings, &BTreeSet::new(), false, &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(retry.retired, [ProviderInstanceId("work".into())]);
    assert_eq!(retry.activated, [ProviderInstanceId("work".into())]);
    let mut events = vec![];
    while let Ok(event) = received.try_recv() {
        events.push(event);
    }
    assert_eq!(events, ["retire:work", "drop:work", "create:work:two"]);
    assert!(effects.update_checks.load(Ordering::SeqCst));
    settings.enable_provider_update_checks = false;
    let policy = host
        .reconcile(&settings, &BTreeSet::new(), false, &BTreeMap::new())
        .await
        .unwrap();
    assert!(policy.activated.is_empty() && policy.retired.is_empty());
    assert!(!effects.update_checks.load(Ordering::SeqCst));
    assert!(received.try_recv().is_err());
    host.close().await.unwrap();
    assert!(service.get_providers().await.0.is_empty());
    assert!(fixture.calls().is_empty());
}
