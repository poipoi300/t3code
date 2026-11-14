#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use t3_contracts::{
    acp_registry::*,
    base_schemas::{ProjectId, ThreadId},
    provider_instance::ProviderInstanceId,
    server::ServerProvider,
};
use t3_provider::{
    acp_coordinator::AcpRuntimeCoordinator, acp_service::*, auth::AuthService,
    sensitive::Sensitive, transport::SystemClock,
};
use tokio::sync::{Mutex, mpsc, oneshot};
type LookupGate = mpsc::UnboundedSender<(ThreadId, oneshot::Sender<()>)>;
fn failure() -> AcpError {
    AcpError::new(
        AcpRegistryOperationErrorReason::SessionImportFailed,
        "Recorded dependency failure.",
    )
}
struct Host {
    snapshot: Mutex<ServerProvider>,
    threads: Mutex<BTreeMap<ThreadId, ImportedThread>>,
    calls: Mutex<Vec<String>>,
    launches: Mutex<Vec<ImportLaunch>>,
    stop_fails: AtomicBool,
    launch_fails: AtomicBool,
    history_fails: AtomicBool,
    listed_count: AtomicUsize,
    lookup: Mutex<Option<LookupGate>>,
}
#[derive(Clone)]
struct Shared(Arc<Host>);
impl std::ops::Deref for Shared {
    type Target = Host;
    fn deref(&self) -> &Host {
        &self.0
    }
}
#[async_trait]
impl AcpSessionManager for Shared {
    async fn list_sessions(
        &self,
        _: &Path,
        _: Option<&str>,
    ) -> AcpResult<AcpRegistryListSessionsResult> {
        let count = self.listed_count.load(Ordering::SeqCst);
        let sessions=(0..count).map(|index|json!({"sessionId":if count==1{"native:id".into()}else{format!("native:{index}")},"cwd":"/workspace","additionalDirectories":[],"title":null,"updatedAt":null,"importedThreadId":null})).collect::<Vec<_>>();
        Ok(serde_json::from_value(json!({"sessions":sessions,"nextCursor":null,"canLoad":true,"canResume":true,"canDelete":true})).unwrap())
    }
    async fn delete_session(&self, _: &Path, _: &str) -> AcpResult<()> {
        self.calls.lock().await.push("delete".into());
        Ok(())
    }
    async fn list_providers(&self, _: &Path) -> AcpResult<AcpRegistryListProvidersResult> {
        self.calls.lock().await.push("list-providers".into());
        Ok(serde_json::from_value(json!({"providers":[{"providerId":"required","supported":["openai"],"required":true,"current":null},{"providerId":"optional","supported":["openai"],"required":false,"current":null}]})).unwrap())
    }
    async fn set_provider(
        &self,
        _: &Path,
        input: Sensitive<AcpRegistrySetProviderInput>,
    ) -> AcpResult<()> {
        assert!(!format!("{input:?}").contains("never-log"));
        self.calls.lock().await.push("set".into());
        Ok(())
    }
    async fn disable_provider(&self, _: &Path, _: &str) -> AcpResult<()> {
        self.calls.lock().await.push("disable".into());
        Ok(())
    }
    async fn logout(&self, _: &Path) -> AcpResult<()> {
        self.calls.lock().await.push("logout".into());
        Ok(())
    }
}
#[async_trait]
impl AcpSessionHost for Shared {
    async fn project_workspace(&self, _: &ProjectId) -> AcpResult<PathBuf> {
        Ok("/workspace".into())
    }
    async fn instance(&self, _: &ProviderInstanceId) -> AcpResult<AcpInstance> {
        Ok(AcpInstance {
            driver: "acpRegistry".into(),
            snapshot: self.snapshot.lock().await.clone(),
            manager: Arc::new(self.clone()),
            managed_auth: false,
        })
    }
    async fn thread(&self, id: &ThreadId) -> AcpResult<Option<ImportedThread>> {
        let gate = self.lookup.lock().await.clone();
        if let Some(gate) = gate {
            let (answer, wait) = oneshot::channel();
            gate.send((id.clone(), answer)).unwrap();
            wait.await.unwrap();
        }
        Ok(self.threads.lock().await.get(id).cloned())
    }
    async fn launch_import(&self, launch: ImportLaunch) -> AcpResult<()> {
        if self.launch_fails.load(Ordering::SeqCst) {
            return Err(failure());
        }
        let id = launch
            .launch
            .thread_id
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone();
        self.threads.lock().await.insert(
            id,
            ImportedThread {
                native_history_pending: true,
            },
        );
        self.launches.lock().await.push(launch);
        Ok(())
    }
    async fn import_history(&self, id: &ThreadId, _: &Path) -> AcpResult<()> {
        self.calls.lock().await.push("history".into());
        if self.history_fails.load(Ordering::SeqCst) {
            return Err(failure());
        }
        self.threads
            .lock()
            .await
            .get_mut(id)
            .unwrap()
            .native_history_pending = false;
        Ok(())
    }
    async fn close_sessions(&self, _: &ProviderInstanceId) -> AcpResult<()> {
        self.calls.lock().await.push("close".into());
        if self.stop_fails.load(Ordering::SeqCst) {
            Err(failure())
        } else {
            Ok(())
        }
    }
    async fn refresh(&self, _: &ProviderInstanceId) -> AcpResult<()> {
        self.calls.lock().await.push("refresh".into());
        Ok(())
    }
}
struct Catalog;
#[async_trait]
impl AcpCatalogOperations for Catalog {
    async fn search(&self, _: AcpRegistrySearchInput) -> AcpResult<AcpRegistrySearchResult> {
        Ok(AcpRegistrySearchResult { agents: vec![] })
    }
    async fn prepare(&self, _: AcpRegistryPrepareInput) -> AcpResult<AcpRegistryPrepareResult> {
        Err(failure())
    }
    async fn uninstall(
        &self,
        _: AcpRegistryManagedBinaryUninstallInput,
    ) -> AcpResult<AcpRegistryManagedBinaryUninstallResult> {
        Err(failure())
    }
}
fn setup() -> (AcpRegistryService, Arc<Host>) {
    let snapshot=serde_json::from_value(json!({"instanceId":"acp","driver":"acpRegistry","displayName":"ACP","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated","canLogout":true},"models":[{"slug":"default-model","name":"Default","isDefault":true,"isCustom":false,"capabilities":null}],"checkedAt":"2026-11-13T00:00:00.000Z","configurableProviders":true,"nativeSessions":{"canList":true,"canLoad":true,"canResume":true,"canDelete":true}})).unwrap();
    let host = Arc::new(Host {
        snapshot: Mutex::new(snapshot),
        threads: Mutex::new(BTreeMap::new()),
        calls: Mutex::new(vec![]),
        launches: Mutex::new(vec![]),
        stop_fails: AtomicBool::new(false),
        launch_fails: AtomicBool::new(false),
        history_fails: AtomicBool::new(false),
        listed_count: AtomicUsize::new(1),
        lookup: Mutex::new(None),
    });
    let service = AcpRegistryService::new(
        Arc::new(Catalog),
        Arc::new(Shared(host.clone())),
        Arc::new(AcpRuntimeCoordinator::new(Arc::new(SystemClock))),
        Arc::new(AuthService::default()),
        "/server".into(),
    );
    (service, host)
}
fn set_input() -> Sensitive<AcpRegistrySetProviderInput> {
    Sensitive::new(serde_json::from_value(json!({"instanceId":"acp","projectId":"project","providerId":"optional","apiType":"openai","baseUrl":"https://api.invalid/?never-log","headers":{"Authorization":"never-log"}})).unwrap())
}
#[tokio::test]
async fn provider_mutations_validate_before_stopping_and_stop_before_writing_or_logout() {
    let (service, host) = setup();
    let mut invalid = set_input().into_inner();
    invalid.headers = Some(
        (0..33)
            .map(|index| (format!("header{index}").into(), "never-log".into()))
            .collect(),
    );
    assert_eq!(
        service
            .set_provider(Sensitive::new(invalid))
            .await
            .unwrap_err()
            .reason,
        AcpRegistryOperationErrorReason::ProviderConfigurationFailed
    );
    assert!(host.calls.lock().await.is_empty());
    let mut invalid = set_input().into_inner();
    invalid.api_type = "unsupported".into();
    assert!(service.set_provider(Sensitive::new(invalid)).await.is_err());
    assert_eq!(*host.calls.lock().await, ["list-providers"]);
    host.calls.lock().await.clear();
    host.stop_fails.store(true, Ordering::SeqCst);
    let error = service.set_provider(set_input()).await.unwrap_err();
    assert!(!format!("{error:?} {error} {:?}", error.clone().wire()).contains("never-log"));
    assert_eq!(*host.calls.lock().await, ["list-providers", "close"]);
    host.calls.lock().await.clear();
    host.stop_fails.store(false, Ordering::SeqCst);
    service.set_provider(set_input()).await.unwrap();
    assert_eq!(
        *host.calls.lock().await,
        ["list-providers", "close", "set", "refresh"]
    );
    host.calls.lock().await.clear();
    let input = |provider: &str| AcpRegistryDisableProviderInput {
        instance_id: "acp".into(),
        project_id: "project".into(),
        provider_id: provider.into(),
    };
    assert!(service.disable_provider(input("required")).await.is_err());
    assert_eq!(*host.calls.lock().await, ["list-providers"]);
    host.calls.lock().await.clear();
    service.disable_provider(input("optional")).await.unwrap();
    assert_eq!(
        *host.calls.lock().await,
        ["list-providers", "close", "disable", "refresh"]
    );
    host.calls.lock().await.clear();
    service
        .logout(AcpRegistryLogoutInput {
            instance_id: "acp".into(),
        })
        .await
        .unwrap();
    assert_eq!(*host.calls.lock().await, ["close", "logout", "refresh"]);
}
#[tokio::test]
async fn session_import_repairs_pending_history_and_native_delete_refuses_an_existing_mapping() {
    let (service, host) = setup();
    let input = AcpRegistryImportSessionInput {
        instance_id: "acp".into(),
        project_id: "project".into(),
        session_id: "native:id".into(),
        title: Some(Some("Title".into())),
        updated_at: None,
    };
    host.history_fails.store(true, Ordering::SeqCst);
    let imported = service.import_session(input.clone()).await.unwrap();
    assert!(imported.imported);
    let launch = host.launches.lock().await;
    assert_eq!(launch.len(), 1);
    assert_eq!(launch[0].launch.model_selection.model, "default-model");
    assert_eq!(
        launch[0].created_by,
        t3_contracts::orchestration_v2::OrchestrationV2Actor::User
    );
    assert_eq!(
        serde_json::to_value(&launch[0].native).unwrap()["metadata"]["itemIdentityVersion"],
        2
    );
    drop(launch);
    let list = AcpRegistryListSessionsInput {
        instance_id: "acp".into(),
        project_id: "project".into(),
        cursor: None,
    };
    let listed = service.list_sessions(list.clone()).await.unwrap();
    assert_eq!(
        listed.sessions[0].imported_thread_id,
        Some(imported.thread_id.clone())
    );
    assert_eq!(listed.sessions[0].native_history_pending, Some(true));
    assert!(service.import_session(input.clone()).await.is_err());
    host.history_fails.store(false, Ordering::SeqCst);
    assert!(
        service
            .import_session(input.clone())
            .await
            .unwrap()
            .imported
    );
    assert!(!service.import_session(input).await.unwrap().imported);
    assert_eq!(host.launches.lock().await.len(), 1);
    assert!(
        service.list_sessions(list).await.unwrap().sessions[0]
            .native_history_pending
            .is_none()
    );
    let delete = AcpRegistryDeleteSessionInput {
        instance_id: "acp".into(),
        project_id: "project".into(),
        session_id: "native:id".into(),
    };
    assert!(service.delete_session(delete.clone()).await.is_err());
    assert!(!host.calls.lock().await.iter().any(|call| call == "delete"));
    host.threads.lock().await.clear();
    service.delete_session(delete).await.unwrap();
    assert_eq!(host.calls.lock().await.last().unwrap(), "delete");
    assert_eq!(
        imported_thread_id(&"acpRegistry".into(), &"one:two".into(), "é/a!").0,
        "thread:provider:acpRegistry:provider-instance:one%3Atwo:native-thread:%C3%A9%2Fa!"
    );
}

#[tokio::test]
async fn native_session_mapping_uses_at_most_sixteen_reads_and_preserves_list_order() {
    let (service, host) = setup();
    host.listed_count.store(20, Ordering::SeqCst);
    let (gate, mut entered) = mpsc::unbounded_channel();
    *host.lookup.lock().await = Some(gate);
    let listing = tokio::spawn(async move {
        service
            .list_sessions(AcpRegistryListSessionsInput {
                instance_id: "acp".into(),
                project_id: "project".into(),
                cursor: None,
            })
            .await
            .unwrap()
    });
    let mut first = vec![];
    for _ in 0..16 {
        first.push(entered.recv().await.unwrap());
    }
    assert!(entered.try_recv().is_err());
    let (_, release) = first.pop().unwrap();
    release.send(()).unwrap();
    let replacement = entered.recv().await.unwrap();
    assert!(entered.try_recv().is_err());
    for (_, release) in first.into_iter().rev() {
        release.send(()).unwrap();
    }
    replacement.1.send(()).unwrap();
    for _ in 0..3 {
        entered.recv().await.unwrap().1.send(()).unwrap();
    }
    let listed = listing.await.unwrap();
    assert_eq!(
        listed
            .sessions
            .iter()
            .map(|session| session.session_id.0.clone())
            .collect::<Vec<_>>(),
        (0..20)
            .map(|index| format!("native:{index}"))
            .collect::<Vec<_>>()
    );
}
