use super::provider_runtime_recovery_service_test::projection;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use t3_contracts::{
    model_selection::ModelSelection, orchestration_v2::*, provider_instance::*, wire::WireDateTime,
};
use t3_orchestration::{error::Result, provider::*, runtime::sessions::*};
use tokio_util::sync::CancellationToken;

struct SelectionAdapter {
    id: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
    plan: SelectionTransitionPlan,
}
#[async_trait]
impl ProviderAdapter for SelectionAdapter {
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
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
        Ok(self.plan.clone())
    }
    async fn open_session(
        &self,
        _: OpenSession,
        _: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        Err(ProviderError::Unsupported("test does not open sessions"))
    }
}
fn selection(instance: &str, model: &str) -> ModelSelection {
    serde_json::from_value(json!({"instanceId":instance,"model":model})).expect("test value")
}
fn service(plan: SelectionTransitionPlan) -> Result<ProviderSwitchService> {
    let capabilities = projection()?.provider_sessions[0].capabilities.clone();
    Ok(ProviderSwitchService::new(Arc::new(AdapterRegistry::new(
        [
            ("codex", "shared"),
            ("compatible", "shared"),
            ("other", "other"),
        ]
        .into_iter()
        .map(|(id, key)| AdapterRegistration {
            adapter: Arc::new(SelectionAdapter {
                id: ProviderInstanceId(id.into()),
                capabilities: capabilities.clone(),
                plan: plan.clone(),
            }),
            continuation_key: key.into(),
            enabled: true,
            credential_access: vec![],
        })
        .collect(),
    ))))
}

#[tokio::test]
async fn releases_newest_live_session_not_newest_record() -> Result<()> {
    let mut projection = projection()?;
    let mut newer = projection.provider_sessions[0].clone();
    newer.id = serde_json::from_value(json!("newer"))?;
    newer.updated_at = WireDateTime("2026-09-03T00:00:01.000Z".into());
    let mut dead = newer.clone();
    dead.id = serde_json::from_value(json!("dead"))?;
    dead.status = ProviderSessionStatus::Stopped;
    dead.updated_at = WireDateTime("2026-09-03T00:00:02.000Z".into());
    projection.provider_sessions.extend([newer, dead]);
    let plan = service(SelectionTransitionPlan::RestartSession)?
        .plan(&projection, selection("codex", "changed"))
        .await?;
    assert_eq!(plan.transition, ProviderSessionTransition::RestartAndResume);
    assert_eq!(
        plan.release_provider_session_ids
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>(),
        ["newer"]
    );
    let mut offset = projection.provider_sessions[0].clone();
    offset.id = serde_json::from_value(json!("offset"))?;
    offset.updated_at = WireDateTime("2026-09-02T20:00:03.000-04:00".into());
    projection.provider_sessions.push(offset);
    let plan = service(SelectionTransitionPlan::RestartSession)?
        .plan(&projection, selection("codex", "changed"))
        .await?;
    assert_eq!(plan.release_provider_session_ids[0].0, "offset");
    Ok(())
}

#[tokio::test]
async fn dead_session_without_native_thread_hands_off_but_detached_native_thread_resumes()
-> Result<()> {
    let mut projection = projection()?;
    projection.provider_sessions[0].status = ProviderSessionStatus::Stopped;
    let service = service(SelectionTransitionPlan::RestartSession)?;
    let mut no_native = projection.clone();
    no_native.provider_threads.clear();
    let fresh = service
        .plan(&no_native, selection("codex", "changed"))
        .await?;
    assert_eq!(
        fresh.transition,
        ProviderSessionTransition::CreateWithHandoff
    );
    assert!(fresh.release_provider_session_ids.is_empty());
    let mut native = serde_json::to_value(&projection.provider_threads[0])?;
    native["nativeThreadRef"] =
        json!({"driver":"codex","nativeId":"native:test","strength":"strong"});
    projection.provider_threads[0] = serde_json::from_value(native)?;
    let resume = service
        .plan(&projection, selection("codex", "changed"))
        .await?;
    assert_eq!(
        resume.transition,
        ProviderSessionTransition::RestartAndResume
    );
    assert!(resume.release_provider_session_ids.is_empty());
    projection.provider_sessions.clear();
    assert_eq!(
        service
            .plan(&projection, selection("codex", "changed"))
            .await?
            .transition,
        ProviderSessionTransition::SwitchModelInSession
    );
    assert_eq!(
        service
            .plan(&projection, selection("other", "changed"))
            .await?
            .transition,
        ProviderSessionTransition::CreateWithHandoff
    );
    Ok(())
}

#[tokio::test]
async fn adapter_classification_from_dead_negotiated_record_controls_model_switch() -> Result<()> {
    for status in [ProviderSessionStatus::Stopped, ProviderSessionStatus::Error] {
        let mut projection = projection()?;
        projection.provider_sessions[0].status = status;
        let mut native = serde_json::to_value(&projection.provider_threads[0])?;
        native["nativeThreadRef"] =
            json!({"driver":"codex","nativeId":"native:test","strength":"strong"});
        projection.provider_threads[0] = serde_json::from_value(native)?;
        assert_eq!(
            service(SelectionTransitionPlan::ApplyOnNextTurn)?
                .plan(&projection, selection("codex", "changed"))
                .await?
                .transition,
            ProviderSessionTransition::SwitchModelInSession
        );
        assert!(
            service(SelectionTransitionPlan::Reject {
                reason: "not negotiated".into()
            })?
            .plan(&projection, selection("codex", "changed"))
            .await
            .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn same_driver_instances_require_matching_continuation_identity() -> Result<()> {
    let projection = projection()?;
    let service = service(SelectionTransitionPlan::RestartSession)?;
    let compatible = service
        .plan(&projection, selection("compatible", "test"))
        .await?;
    let incompatible = service
        .plan(&projection, selection("other", "test"))
        .await?;
    assert!(compatible.instance_changed);
    assert_eq!(
        compatible.transition,
        ProviderSessionTransition::RestartAndResume
    );
    assert_eq!(
        incompatible.transition,
        ProviderSessionTransition::CreateWithHandoff
    );
    assert!(
        service
            .plan(&projection, selection("missing", "test"))
            .await
            .is_err()
    );
    Ok(())
}
