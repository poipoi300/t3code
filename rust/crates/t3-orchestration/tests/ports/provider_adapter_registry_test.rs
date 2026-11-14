use super::provider_session_manager_test::fake;
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::{orchestration_v2::*, provider_instance::*};
use t3_orchestration::{
    api::{ProviderRegistry, SessionDirectory},
    error::Result,
    provider::*,
    runtime::sessions::*,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

struct LiveInstances(std::sync::Mutex<Vec<AdapterRegistration>>);

#[async_trait]
impl t3_orchestration::ports::sessions_recovery::provider_adapter_registry::ProviderInstanceSource
    for LiveInstances
{
    async fn instances(&self) -> Result<Vec<AdapterRegistration>> {
        Ok(self.0.lock().expect("instance source").clone())
    }
}

#[tokio::test]
async fn trait_metadata_and_list_follow_live_instance_changes() -> Result<()> {
    let (adapter, _, input) = fake()?;
    let source = Arc::new(LiveInstances(std::sync::Mutex::new(vec![])));
    let registry = AdapterRegistry::from_instance_source(source.clone());
    assert!(ProviderRegistry::list(&registry).await?.is_empty());
    source
        .0
        .lock()
        .expect("instances")
        .push(AdapterRegistration {
            adapter: adapter.clone(),
            enabled: false,
            continuation_key: "codex:account".into(),
            credential_access: vec![],
        });
    assert_eq!(
        ProviderRegistry::list(&registry).await?,
        vec![input.model_selection.instance_id.clone()]
    );
    let metadata =
        ProviderRegistry::metadata(&registry, input.model_selection.instance_id.clone()).await?;
    assert!(!metadata.enabled);
    assert_eq!(metadata.continuation_key, "codex:account");
    assert_eq!(
        metadata.capabilities,
        adapter.runtime.session().capabilities
    );
    source.0.lock().expect("instances").clear();
    assert!(
        registry
            .adapter(input.model_selection.instance_id)
            .await
            .is_err()
    );
    assert!(ProviderRegistry::list(&registry).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn live_credentials_are_rechecked_at_open_and_policy_is_forwarded() -> Result<()> {
    let (adapter, _, input) = fake()?;
    let peer = Arc::new(CredentialAccess::default());
    let source = Arc::new(LiveInstances(std::sync::Mutex::new(vec![
        AdapterRegistration {
            adapter: adapter.clone(),
            enabled: true,
            continuation_key: "codex:account".into(),
            credential_access: vec![],
        },
    ])));
    let registry = AdapterRegistry::from_instance_source(source.clone());
    let guarded = registry
        .adapter(input.model_selection.instance_id.clone())
        .await?;
    source.0.lock().expect("instances")[0].credential_access = vec![peer.clone()];
    peer.begin_change();
    let error = guarded
        .open_session(input.clone(), CancellationToken::new())
        .await
        .err()
        .expect("blocked");
    let ProviderError::Setup(setup) = error else {
        panic!("expected structured setup failure")
    };
    assert_eq!(setup.instance_id, input.model_selection.instance_id);
    assert_eq!(setup.operation, "session");
    peer.end_change();
    adapter
        .open_session_with_policy
        .push(Ok(adapter.runtime.clone()));
    guarded
        .open_session_with_policy(
            input,
            RuntimePolicyOptions {
                reasoning_effort: Some("high".into()),
            },
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(
        adapter
            .open_session_with_policy
            .next_call()
            .await
            .expect("policy open")
            .1
            .reasoning_effort
            .as_deref(),
        Some("high")
    );
    Ok(())
}

struct BlockingAdapter {
    id: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
    milestones: mpsc::UnboundedSender<&'static str>,
    modes: Option<Vec<t3_contracts::provider_policy::RuntimeMode>>,
}
#[async_trait]
impl ProviderAdapter for BlockingAdapter {
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        self.modes.clone()
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.id
    }
    fn driver(&self) -> ProviderDriverKind {
        ProviderDriverKind("codex".into())
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        Ok(self.capabilities.clone())
    }
    async fn plan_selection_transition(
        &self,
        _: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        Ok(SelectionTransitionPlan::ApplyOnNextTurn)
    }
    async fn open_session(
        &self,
        _: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.milestones.send("entered").expect("test receiver");
        cancellation.cancelled().await;
        self.milestones.send("stopped").expect("test receiver");
        Err(ProviderError::Cancelled)
    }
}

#[tokio::test]
async fn routes_same_driver_instances_independently_and_preserves_list_order() -> Result<()> {
    let (fake, _, _) = fake()?;
    let (send, _) = mpsc::unbounded_channel();
    let a: Arc<dyn ProviderAdapter> = Arc::new(BlockingAdapter {
        id: ProviderInstanceId("personal".into()),
        capabilities: fake.runtime.session().capabilities.clone(),
        milestones: send.clone(),
        modes: None,
    });
    let b: Arc<dyn ProviderAdapter> = Arc::new(BlockingAdapter {
        id: ProviderInstanceId("work".into()),
        capabilities: fake.runtime.session().capabilities.clone(),
        milestones: send,
        modes: None,
    });
    let registry = AdapterRegistry::from_adapters(vec![a.clone(), b.clone()]);
    assert!(Arc::ptr_eq(
        &registry.adapter(a.instance_id().clone()).await?,
        &a
    ));
    assert!(Arc::ptr_eq(
        &registry.adapter(b.instance_id().clone()).await?,
        &b
    ));
    assert_eq!(
        registry.list().await?,
        vec![a.instance_id().clone(), b.instance_id().clone()]
    );
    assert!(
        registry
            .adapter(ProviderInstanceId("missing".into()))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn shared_credential_changes_block_new_startup_and_cancel_admitted_startup() -> Result<()> {
    let (fake, _, input) = fake()?;
    let (send, mut milestones) = mpsc::unbounded_channel();
    let peer = Arc::new(CredentialAccess::default());
    let own = Arc::new(CredentialAccess::default());
    // Muse's offered modes: the credential guard must not drop them (review R2).
    let muse = vec![
        t3_contracts::provider_policy::RuntimeMode::ApprovalRequired,
        t3_contracts::provider_policy::RuntimeMode::FullAccess,
    ];
    let adapter: Arc<dyn ProviderAdapter> = Arc::new(BlockingAdapter {
        id: input.model_selection.instance_id.clone(),
        capabilities: fake.runtime.session().capabilities.clone(),
        milestones: send,
        modes: Some(muse.clone()),
    });
    let registry = AdapterRegistry::new(vec![AdapterRegistration {
        adapter,
        enabled: true,
        continuation_key: "codex:account:shared".into(),
        credential_access: vec![own, peer.clone()],
    }]);
    let guarded = registry
        .adapter(input.model_selection.instance_id.clone())
        .await?;
    assert_eq!(guarded.supported_runtime_modes(), Some(muse));
    peer.begin_change();
    assert!(
        guarded
            .open_session(input.clone(), CancellationToken::new())
            .await
            .is_err()
    );
    assert!(milestones.try_recv().is_err());
    peer.end_change();
    let startup: JoinHandle<_> =
        tokio::spawn(async move { guarded.open_session(input, CancellationToken::new()).await });
    assert_eq!(milestones.recv().await, Some("entered"));
    peer.begin_change();
    assert_eq!(milestones.recv().await, Some("stopped"));
    assert!(matches!(
        startup.await.expect("startup task"),
        Err(ProviderError::Cancelled)
    ));
    Ok(())
}

#[tokio::test]
async fn dropping_attach_cancels_and_awaited_shutdown_collects_its_handshake() -> Result<()> {
    let (fake, _, input) = fake()?;
    let (send, mut milestones) = mpsc::unbounded_channel();
    let adapter = Arc::new(BlockingAdapter {
        id: input.model_selection.instance_id.clone(),
        capabilities: fake.runtime.session().capabilities.clone(),
        milestones: send,
        modes: None,
    });
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
    );
    let worker = manager.clone();
    let id = input.provider_session_id.clone();
    let attach = tokio::spawn(async move {
        worker
            .attach(input.model_selection.instance_id.clone(), input)
            .await
    });
    assert_eq!(milestones.recv().await, Some("entered"));
    attach.abort();
    assert!(matches!(attach.await,Err(error) if error.is_cancelled()));
    assert_eq!(milestones.recv().await, Some("stopped"));
    manager.shutdown().await?;
    assert!(!manager.is_live(&id));
    Ok(())
}

struct HandshakeChangingAdapter {
    inner: Arc<dyn ProviderAdapter>,
    gate: Arc<CredentialAccess>,
}
#[async_trait]
impl ProviderAdapter for HandshakeChangingAdapter {
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        self.inner.instance_id()
    }
    fn driver(&self) -> ProviderDriverKind {
        self.inner.driver()
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        self.inner.capabilities().await
    }
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        self.inner.plan_selection_transition(input).await
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        let runtime = self.inner.open_session(input, cancellation).await?;
        self.gate.begin_change();
        Ok(runtime)
    }
}

#[tokio::test]
async fn credential_change_at_handshake_completion_closes_the_new_runtime() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let gate = Arc::new(CredentialAccess::default());
    let registry = AdapterRegistry::new(vec![AdapterRegistration {
        adapter: Arc::new(HandshakeChangingAdapter {
            inner: adapter,
            gate: gate.clone(),
        }),
        enabled: true,
        continuation_key: "account".into(),
        credential_access: vec![gate],
    }]);
    let guarded = registry
        .adapter(input.model_selection.instance_id.clone())
        .await?;
    assert!(matches!(
        guarded.open_session(input, CancellationToken::new()).await,
        Err(ProviderError::Cancelled)
    ));
    assert!(matches!(
        calls.recv().await,
        Some(t3_orchestration::testing::FakeCall::Open(_))
    ));
    assert!(matches!(
        calls.recv().await,
        Some(t3_orchestration::testing::FakeCall::Close)
    ));
    Ok(())
}
