use super::{
    provider_adapter_registry::{AdapterMetadata, AdapterRegistry},
    provider_session_transition_policy::*,
};
use crate::{
    api::ProviderRegistry,
    error::{Error, Result},
    provider::SelectionTransitionInput,
};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::ProviderSessionId, model_selection::ModelSelection, orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex, provider_instance::ProviderInstanceId,
};

pub struct ProviderSwitchPlan {
    pub instance_changed: bool,
    pub release_provider_session_ids: Vec<ProviderSessionId>,
    pub transition: ProviderSessionTransition,
}

pub struct ProviderSwitchService {
    registry: Arc<dyn ProviderRegistry>,
    metadata_registry: Option<Arc<AdapterRegistry>>,
}

impl ProviderSwitchService {
    pub fn new(registry: Arc<dyn ProviderRegistry>) -> Self {
        Self {
            registry,
            metadata_registry: None,
        }
    }
    pub fn with_metadata(registry: Arc<AdapterRegistry>) -> Self {
        Self {
            registry: registry.clone(),
            metadata_registry: Some(registry),
        }
    }
    async fn metadata(&self, id: &ProviderInstanceId) -> Result<AdapterMetadata> {
        if let Some(registry) = &self.metadata_registry {
            return registry.metadata(id).await;
        }
        match self.registry.metadata(id.clone()).await {
            Ok(metadata) => return Ok(metadata),
            Err(Error::Unsupported(_)) => {}
            Err(error) => return Err(error),
        }
        let adapter = self.registry.adapter(id.clone()).await?;
        Ok(AdapterMetadata {
            driver: adapter.driver(),
            continuation_key: format!("{}:instance:{}", adapter.driver().0, id.0),
            enabled: true,
            capabilities: adapter.capabilities().await?,
        })
    }
    pub async fn plan(
        &self,
        projection: &OrchestrationV2ThreadProjection,
        target: ModelSelection,
    ) -> Result<ProviderSwitchPlan> {
        let current = &projection.thread.model_selection;
        let current_instance = self.metadata(&current.instance_id).await.ok();
        let target_instance = self.metadata(&target.instance_id).await.ok();
        let target_adapter = self.registry.adapter(target.instance_id.clone()).await.ok();
        let mut sessions = projection
            .provider_sessions
            .iter()
            .filter(|s| s.provider_instance_id == IdOrIndex::Id(current.instance_id.clone()))
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| {
            std::cmp::Reverse(super::usage_limit_recovery_worker::epoch_millis(
                &session.updated_at.0,
            ))
        });
        let live = sessions.iter().copied().find(|s| is_live(s));
        let negotiated = live
            .or_else(|| sessions.first().copied())
            .map(|s| s.capabilities.clone());
        let native_thread = projection.provider_threads.iter().find(|thread| {
            Some(&thread.id) == projection.thread.active_provider_thread_id.as_ref()
                && thread.provider_instance_id == IdOrIndex::Id(current.instance_id.clone())
                && thread.native_thread_ref.is_some()
        });
        let selection_plan = if current.instance_id == target.instance_id
            && !model_selections_equal(current, &target)
            && current_instance.is_some()
            && (live.is_some() || native_thread.is_some())
        {
            if let Some(adapter) = &target_adapter {
                Some(match negotiated {
                    Some(capabilities) => {
                        adapter
                            .plan_selection_transition(SelectionTransitionInput {
                                current: current.clone(),
                                target: target.clone(),
                                session_capabilities: capabilities,
                            })
                            .await?
                    }
                    None => crate::provider::SelectionTransitionPlan::ApplyOnNextTurn,
                })
            } else {
                None
            }
        } else {
            None
        };
        let state = |metadata: &AdapterMetadata, selection: ModelSelection, workspace: String| {
            SessionTransitionState {
                driver: metadata.driver.clone(),
                continuation_identity: ContinuationIdentity {
                    driver: metadata.driver.clone(),
                    key: metadata.continuation_key.clone(),
                },
                model_selection: selection,
                runtime_mode: projection.thread.runtime_mode,
                interaction_mode: projection.thread.interaction_mode,
                workspace,
            }
        };
        let current_workspace = live
            .map(|s| s.cwd.0.clone())
            .or_else(|| {
                projection
                    .thread
                    .worktree_path
                    .as_ref()
                    .map(|v| v.0.clone())
            })
            .unwrap_or_else(|| "<unresolved-workspace>".into());
        let target_workspace = projection
            .thread
            .worktree_path
            .as_ref()
            .map(|v| v.0.clone())
            .or_else(|| live.map(|s| s.cwd.0.clone()))
            .unwrap_or_else(|| "<unresolved-workspace>".into());
        let current_state = current_instance
            .as_ref()
            .filter(|_| live.is_some() || native_thread.is_some())
            .map(|m| state(m, current.clone(), current_workspace));
        let transition = match target_instance
            .as_ref()
            .filter(|_| target_adapter.is_some())
        {
            Some(metadata) => decide_provider_session_transition(
                current_state.as_ref(),
                &state(metadata, target.clone(), target_workspace),
                metadata.enabled,
                selection_plan.as_ref(),
            ),
            None => ProviderSessionTransition::Reject {
                reason: "The target provider instance is unavailable.".into(),
            },
        };
        if let ProviderSessionTransition::Reject { reason } = &transition {
            return Err(Error::Service {
                operation: "ProviderSwitchService.plan",
                detail: reason.clone(),
            });
        }
        let release_provider_session_ids = projection
            .provider_sessions
            .iter()
            .filter(|s| is_live(s))
            .filter(|s| match transition {
                ProviderSessionTransition::RestartAndResume => {
                    live.is_some_and(|current| current.id == s.id)
                }
                ProviderSessionTransition::CreateWithHandoff => {
                    s.provider_instance_id != IdOrIndex::Id(target.instance_id.clone())
                }
                _ => false,
            })
            .map(|s| match &s.id {
                IdOrIndex::Id(id) => Ok(id.clone()),
                _ => Err(Error::Decode {
                    entity: "provider session",
                    detail: "service requires expanded IDs".into(),
                }),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ProviderSwitchPlan {
            instance_changed: current.instance_id != target.instance_id,
            release_provider_session_ids,
            transition,
        })
    }
}

fn is_live(session: &OrchestrationV2ProviderSession) -> bool {
    !matches!(
        session.status,
        ProviderSessionStatus::Stopped | ProviderSessionStatus::Error
    )
}
