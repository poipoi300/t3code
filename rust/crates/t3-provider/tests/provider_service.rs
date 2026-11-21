#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use t3_contracts::{provider_instance::ProviderInstanceId, server::*};
use t3_provider::{
    auth::AuthService, catalog::Manifest, maintenance::*, sensitive::*, service::*,
    transport::ProviderClock,
};
use tokio::sync::{Mutex, mpsc, oneshot};
struct Clock;
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
struct Catalog;
#[async_trait]
impl CatalogRefresh for Catalog {
    async fn refresh(&self) -> ServiceResult<()> {
        Ok(())
    }
}
struct Instance {
    id: ProviderInstanceId,
    value: Mutex<ServerProvider>,
    events: mpsc::UnboundedSender<String>,
    ownership: Mutex<String>,
    model_error: Option<ServiceError>,
}
#[async_trait]
impl ProviderInstanceService for Instance {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.id
    }
    fn catalogs_per_cwd(&self) -> bool {
        true
    }
    async fn snapshot(&self, cwd: Option<&str>, _: bool) -> ServiceResult<ServerProvider> {
        self.events
            .send(format!("snapshot:{}", cwd.unwrap_or("machine")))
            .unwrap();
        Ok(self.value.lock().await.clone())
    }
    async fn reload_secrets(&self) -> ServiceResult<()> {
        self.events.send("secrets".into()).unwrap();
        Ok(())
    }
    async fn invalidate_models(&self, _: Option<&str>, _: bool) -> ServiceResult<()> {
        self.events.send("invalidate".into()).unwrap();
        Ok(())
    }
    async fn refresh_models(&self) -> ServiceResult<()> {
        self.events.send("models".into()).unwrap();
        self.model_error.map_or(Ok(()), Err)
    }
    async fn maintenance(&self, _: bool) -> ServiceResult<MaintenanceCapabilities> {
        Ok(MaintenanceCapabilities {
            driver: "codex".into(),
            package: Some("@openai/codex".into()),
            update: Some(action(
                "fixture",
                vec![],
                &self.ownership.lock().await,
                Default::default(),
                t3_provider::environment::Platform::current(),
            )),
            install_path: None,
            manual_command: None,
        })
    }
    async fn latest_version(&self, _: bool) -> ServiceResult<Option<String>> {
        Ok(Some("0.166.0".into()))
    }
}
struct Executor {
    started: mpsc::UnboundedSender<()>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
#[async_trait]
impl MaintenanceExecutor for Executor {
    async fn execute(&self, _: MaintenanceAction) -> ServiceResult<()> {
        self.started.send(()).unwrap();
        if let Some(release) = self.release.lock().await.take() {
            release.await.unwrap();
        }
        Ok(())
    }
}
fn instance(enabled: bool) -> (Arc<Instance>, mpsc::UnboundedReceiver<String>) {
    instance_at("work", enabled)
}
fn instance_at(id: &str, enabled: bool) -> (Arc<Instance>, mpsc::UnboundedReceiver<String>) {
    let (events, receiver) = mpsc::unbounded_channel();
    let value=serde_json::from_value(json!({"instanceId":id,"driver":"codex","displayName":"Work","enabled":enabled,"installed":true,"version":"0.166.0","status":"ready","checkedAt":"2027-01-15T08:00:00.000Z","auth":{"status":"authenticated"},"models":[{"slug":"model","name":"Model","isCustom":false,"capabilities":null}],"slashCommands":[{"name":"command","description":"full description"}],"skills":[{"name":"skill","path":"/skill","enabled":true,"scope":"user","description":"full description"}]})).unwrap();
    (
        Arc::new(Instance {
            id: id.into(),
            value: Mutex::new(value),
            events,
            ownership: Mutex::new("shared".into()),
            model_error: None,
        }),
        receiver,
    )
}
fn service(executor: Arc<dyn MaintenanceExecutor>) -> Arc<ProviderService> {
    ProviderService::new(
        Arc::new(AuthService::default()),
        executor,
        Arc::new(Catalog),
        Arc::new(Clock),
        Arc::new(Manifest::bundled().unwrap()),
        "0.1.0".into(),
    )
}
fn idle_executor() -> Arc<Executor> {
    let (started, _) = mpsc::unbounded_channel();
    Arc::new(Executor {
        started,
        release: Mutex::new(None),
    })
}
struct LiveInstance {
    base: Arc<Instance>,
    updates: tokio::sync::watch::Sender<Option<ServerProvider>>,
}
#[async_trait]
impl ProviderInstanceService for LiveInstance {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.base.id
    }
    async fn snapshot(&self, _: Option<&str>, _: bool) -> ServiceResult<ServerProvider> {
        Ok(self.base.value.lock().await.clone())
    }
    fn subscribe_snapshots(&self) -> Option<tokio::sync::watch::Receiver<Option<ServerProvider>>> {
        Some(self.updates.subscribe())
    }
}
#[tokio::test]
async fn live_snapshots_remain_authoritative_during_refresh_and_replaced_instances_cannot_publish_late()
 {
    let (base, _events) = instance(true);
    let mut live = base.value.lock().await.clone();
    live.version = Some("live".into());
    let (updates, _) = tokio::sync::watch::channel(Some(live.clone()));
    let service = service(idle_executor());
    let mut notifications = service.subscribe();
    service
        .register(Arc::new(LiveInstance {
            base: base.clone(),
            updates: updates.clone(),
        }))
        .await;
    notifications.changed().await.unwrap();
    let response = service
        .refresh_providers(serde_json::from_value(json!({})).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.providers.0[0]
            .version
            .as_ref()
            .map(|version| version.0.as_str()),
        Some("live")
    );
    notifications.borrow_and_update();
    let mut next = live.clone();
    next.version = Some("replacement".into());
    let (replacement, _) = tokio::sync::watch::channel(Some(next));
    service
        .register(Arc::new(LiveInstance {
            base,
            updates: replacement.clone(),
        }))
        .await;
    notifications.changed().await.unwrap();
    updates.send_replace(Some(live));
    assert_eq!(
        service.get_providers().await.0[0]
            .version
            .as_ref()
            .map(|version| version.0.as_str()),
        Some("replacement")
    );
    service.unregister(&"work".into()).await;
    replacement.send_replace(None);
    assert!(service.get_providers().await.0.is_empty());
}
#[tokio::test]
async fn global_model_refresh_attributes_the_failure_to_its_instance() {
    let (mut instance, _events) = instance_at("model-error", true);
    Arc::get_mut(&mut instance).unwrap().model_error = Some(ServiceError::Response);
    let service = service(idle_executor());
    service.register(instance).await;
    let input = serde_json::from_value(json!({"refreshModels":true})).unwrap();
    assert!(matches!(service.refresh_providers_at(input, None).await,
        Err(RefreshProvidersError::Models { instance_id, error:ServiceError::Response }) if instance_id.0 == "model-error"));
}

#[tokio::test]
async fn parked_workspace_reads_the_project_catalog_under_the_requested_key() {
    let (instance, mut events) = instance(true);
    let service = service(idle_executor());
    let scan = "/project";
    let requested = "/worktrees/parked";
    let row = serde_json::from_value(json!({"cwd":scan,"checkedAt":"2027-01-15T08:00:00.000Z","slashCommands":[{"name":"project-command","description":"Project command"}],"skills":[]})).unwrap();
    instance.value.lock().await.workspace_snapshots = Some(vec![row]);
    service.register(instance).await;
    let input = serde_json::from_value(
        json!({"instanceId":"work","cwd":requested,"workspaceSnapshotsOnDemand":true}),
    )
    .unwrap();
    let result = service
        .refresh_providers_at(input, Some(scan))
        .await
        .unwrap();
    assert_eq!(events.recv().await.as_deref(), Some("snapshot:/project"));
    let scoped = &result.providers.0[0].workspace_snapshots.as_ref().unwrap()[0];
    assert_eq!(scoped.cwd.0, requested);
    assert_eq!(scoped.slash_commands[0].name.0, "project-command");
    assert!(
        service.get_providers().await.0[0]
            .workspace_snapshots
            .as_ref()
            .unwrap()
            .iter()
            .any(|s| s.cwd.0 == requested)
    );
    let (other, _events) = instance_at("other", true);
    service.register(other).await;
    let fresh =
        serde_json::from_value(json!({"instanceId":"other","cwd":requested,"fresh":true})).unwrap();
    service
        .refresh_providers_at(fresh, Some(scan))
        .await
        .unwrap();
    let providers = service.get_providers().await;
    let previous = providers
        .0
        .iter()
        .find(|p| p.instance_id.0 == "work")
        .unwrap();
    assert!(
        !previous
            .workspace_snapshots
            .iter()
            .flatten()
            .any(|s| s.cwd.0 == requested),
        "fresh scans evict the requested cwd from the previous provider"
    );
}

#[tokio::test]
async fn refresh_models_obeys_installation_enablement_and_returns_only_the_requested_catalog_shape()
{
    let (instance, mut events) = instance(true);
    let service = service(idle_executor());
    service.register(instance.clone()).await;
    let input = json!({"instanceId":"work","cwd":"/workspace","refreshModels":true,"reloadSecrets":true,"workspaceSnapshotsOnDemand":true,"providerCatalogsOnDemand":true,"machineCatalog":true});
    let response = service
        .refresh_providers(serde_json::from_value(input).unwrap())
        .await
        .unwrap();
    let mut calls = vec![];
    while let Ok(event) = events.try_recv() {
        calls.push(event);
    }
    assert_eq!(
        calls,
        [
            "secrets",
            "invalidate",
            "snapshot:/workspace",
            "models",
            "snapshot:machine"
        ]
    );
    let response = serde_json::to_value(response).unwrap();
    let provider = &response["providers"][0];
    assert_eq!(provider["models"], json!([]));
    assert_eq!(provider["slashCommands"], json!([{"name":"command"}]));
    assert!(provider["skills"][0].get("description").is_none());
    assert_eq!(provider["catalogDigest"].as_str().unwrap().len(), 16);
    assert_eq!(
        provider["workspaceSnapshots"][0]["slashCommandsPending"],
        true
    );
    instance.value.lock().await.enabled = false;
    service
        .refresh_providers(
            serde_json::from_value(
                json!({"instanceId":"work","refreshModels":true,"omitProviders":true}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let mut calls = vec![];
    while let Ok(event) = events.try_recv() {
        calls.push(event);
    }
    assert_eq!(calls, ["invalidate", "snapshot:machine"]);
    assert_eq!(service.get_providers().await.0.len(), 1);
    assert!(
        service
            .refresh_providers(serde_json::from_value(json!({"modelCatalogOnly":true})).unwrap())
            .await
            .unwrap()
            .providers
            .0
            .is_empty()
    );
}
#[tokio::test]
async fn an_update_survives_rpc_disconnect_rejects_duplicates_and_publishes_its_terminal_state() {
    let (started, mut received) = mpsc::unbounded_channel();
    let (release, wait) = oneshot::channel();
    let service = service(Arc::new(Executor {
        started,
        release: Mutex::new(Some(wait)),
    }));
    let cache_root = tempfile::tempdir().unwrap();
    let cache = Arc::new(t3_provider::status_cache::ProviderStatusCache::new(
        cache_root.path().into(),
    ));
    service.set_status_cache(cache.clone()).await;
    let (instance, _events) = instance(true);
    instance.value.lock().await.version = Some("0.165.0".into());
    service.register(instance.clone()).await;
    let (peer, mut peer_events) = instance_at("other", true);
    peer.value.lock().await.version = Some("0.165.0".into());
    service.register(peer.clone()).await;

    service
        .refresh_providers(serde_json::from_value(json!({})).unwrap())
        .await
        .unwrap();
    let input: ServerProviderUpdateInput =
        serde_json::from_value(json!({"provider":"codex","instanceId":"work"})).unwrap();
    let request = tokio::spawn({
        let service = service.clone();
        let input = input.clone();
        async move { service.update_provider(input).await }
    });
    received.recv().await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert_eq!(
        service.update_provider(input).await.unwrap_err(),
        ServiceError::Busy
    );
    instance.value.lock().await.version = Some("0.166.0".into());
    peer.value.lock().await.version = Some("0.166.0".into());
    release.send(()).unwrap();
    service.await_updates().await;
    let persisted = cache.read(&"work".into()).await.unwrap();
    assert_eq!(persisted.version.as_ref().unwrap().0, "0.166.0");
    assert!(persisted.update_state.is_none());
    let peer_snapshots: Vec<_> = std::iter::from_fn(|| peer_events.try_recv().ok()).collect();
    assert_eq!(
        peer_snapshots,
        ["snapshot:machine", "invalidate", "snapshot:machine"]
    );
    assert!(
        service
            .get_providers()
            .await
            .0
            .iter()
            .all(|provider| provider.version.as_ref().unwrap().0 == "0.166.0")
    );

    assert_eq!(
        service
            .get_providers()
            .await
            .0
            .iter()
            .find(|provider| provider.instance_id.0 == "work")
            .unwrap()
            .update_state
            .as_ref()
            .unwrap()
            .status,
        ServerProviderUpdateStatus::Succeeded
    );
}
#[tokio::test]
async fn an_already_current_installation_is_verified_without_running_its_updater() {
    let (started, mut received) = mpsc::unbounded_channel();
    let service = service(Arc::new(Executor {
        started,
        release: Mutex::new(None),
    }));
    let (instance, _events) = instance(true);
    service.register(instance).await;
    service
        .refresh_providers(serde_json::from_value(json!({})).unwrap())
        .await
        .unwrap();
    let input = serde_json::from_value(json!({"provider":"codex","instanceId":"work"})).unwrap();
    let response = service.update_provider(input).await.unwrap();
    assert_eq!(
        response.providers.0[0]
            .update_state
            .as_ref()
            .unwrap()
            .status,
        ServerProviderUpdateStatus::Succeeded
    );
    assert!(received.try_recv().is_err());
}
