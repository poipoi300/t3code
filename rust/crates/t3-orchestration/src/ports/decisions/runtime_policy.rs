use super::dispatch_mode_limit::{CeilingRefused, RuntimeModeCeiling, is_runtime_mode_above};
use crate::{
    error::{Error, Result},
    provider::RuntimePolicy,
};
use std::path::PathBuf;
use t3_contracts::{orchestration_v2::OrchestrationV2AppThread, provider_policy::RuntimeMode};

#[derive(Debug, Clone, Default)]
pub struct RuntimePolicyOverride {
    pub cwd: Option<PathBuf>,
    pub approval_policy: Option<serde_json::Value>,
    pub sandbox_policy: Option<serde_json::Value>,
}

pub fn provider_runtime_mode(mode: RuntimeMode, supported: Option<&[RuntimeMode]>) -> RuntimeMode {
    if supported.is_none_or(|modes| modes.is_empty() || modes.contains(&mode)) {
        mode
    } else {
        RuntimeMode::ApprovalRequired
    }
}

pub fn resolve(
    thread: &OrchestrationV2AppThread,
    workspace_root: Option<&str>,
    supported_runtime_modes: Option<&[RuntimeMode]>,
    ceiling: Option<&RuntimeModeCeiling>,
    overrides: &RuntimePolicyOverride,
) -> Result<RuntimePolicy> {
    if let Some(ceiling) = ceiling
        .filter(|ceiling| is_runtime_mode_above(thread.runtime_mode, ceiling.max_runtime_mode))
    {
        return Err(Error::Service {
            operation: "runtime-policy.above-ceiling",
            detail: ceiling.message(CeilingRefused::Turn),
        });
    }
    let cwd = thread
        .worktree_path
        .as_ref()
        .map(|path| path.0.as_str())
        .or(workspace_root)
        .ok_or(Error::Service {
            operation: "RuntimePolicy.resolve",
            detail: "Project not found.".into(),
        })?;
    Ok(RuntimePolicy {
        runtime_mode: provider_runtime_mode(thread.runtime_mode, supported_runtime_modes),
        interaction_mode: thread.interaction_mode,
        cwd: Some(overrides.cwd.clone().unwrap_or_else(|| PathBuf::from(cwd))),
        approval_policy: overrides.approval_policy.clone(),
        sandbox_policy: overrides.sandbox_policy.clone(),
        max_runtime_mode: ceiling.map(|ceiling| ceiling.max_runtime_mode),
    })
}

pub fn resolve_base(thread: &OrchestrationV2AppThread) -> RuntimePolicy {
    RuntimePolicy {
        runtime_mode: thread.runtime_mode,
        interaction_mode: thread.interaction_mode,
        cwd: thread
            .worktree_path
            .as_ref()
            .map(|path| PathBuf::from(&path.0)),
        approval_policy: None,
        sandbox_policy: None,
        max_runtime_mode: None,
    }
}

#[async_trait::async_trait]
pub trait ProviderRuntimeModeSource: Send + Sync {
    async fn supported_modes(
        &self,
        instance: t3_contracts::provider_instance::ProviderInstanceId,
    ) -> Result<Option<Vec<RuntimeMode>>>;
}

pub struct ProjectRuntimePolicy {
    pub database: crate::stores::StoreDatabase,
    pub providers: std::sync::Arc<dyn ProviderRuntimeModeSource>,
    pub ceiling: Option<RuntimeModeCeiling>,
}

#[async_trait::async_trait]
impl crate::api::RuntimePolicyResolver for ProjectRuntimePolicy {
    async fn resolve(
        &self,
        thread: OrchestrationV2AppThread,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> Result<crate::provider::ConfiguredRuntimePolicy> {
        if let Some(ceiling) = self
            .ceiling
            .as_ref()
            .filter(|ceiling| is_runtime_mode_above(thread.runtime_mode, ceiling.max_runtime_mode))
        {
            return Err(Error::Service {
                operation: "runtime-policy.above-ceiling",
                detail: ceiling.message(CeilingRefused::Turn),
            });
        }
        let supported = self
            .providers
            .supported_modes(selection.instance_id)
            .await?;
        let workspace = if thread.worktree_path.is_some() {
            None
        } else {
            let project_id =
                crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                    &thread.project_id,
                )?;
            self.database
                .read(move |db| crate::stores::projects::get(db, &project_id, false))
                .await?
                .map(|project| project.workspace_root)
        };
        Ok(resolve(
            &thread,
            workspace.as_deref(),
            supported.as_deref(),
            self.ceiling.as_ref(),
            &RuntimePolicyOverride::default(),
        )?
        .with_reasoning_effort(None))
    }
}

pub struct OverriddenRuntimePolicy {
    pub base: std::sync::Arc<dyn crate::api::RuntimePolicyResolver>,
    pub overrides: RuntimePolicyOverride,
    pub reasoning_effort: Option<String>,
}

#[async_trait::async_trait]
impl crate::api::RuntimePolicyResolver for OverriddenRuntimePolicy {
    async fn resolve(
        &self,
        thread: OrchestrationV2AppThread,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> Result<crate::provider::ConfiguredRuntimePolicy> {
        let mut configured = self.base.resolve(thread, selection).await?;
        if let Some(cwd) = &self.overrides.cwd {
            configured.policy.cwd = Some(cwd.clone());
        }
        if let Some(policy) = &self.overrides.approval_policy {
            configured.policy.approval_policy = Some(policy.clone());
        }
        if let Some(policy) = &self.overrides.sandbox_policy {
            configured.policy.sandbox_policy = Some(policy.clone());
        }
        if let Some(effort) = &self.reasoning_effort {
            configured.options.reasoning_effort = Some(effort.clone());
        }
        Ok(configured)
    }
}

pub struct CeilingRuntimePolicy {
    pub base: std::sync::Arc<dyn crate::api::RuntimePolicyResolver>,
    pub ceiling: RuntimeModeCeiling,
}

#[async_trait::async_trait]
impl crate::api::RuntimePolicyResolver for CeilingRuntimePolicy {
    async fn resolve(
        &self,
        thread: OrchestrationV2AppThread,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> Result<crate::provider::ConfiguredRuntimePolicy> {
        if is_runtime_mode_above(thread.runtime_mode, self.ceiling.max_runtime_mode) {
            return Err(Error::Service {
                operation: "runtime-policy.above-ceiling",
                detail: self.ceiling.message(CeilingRefused::Turn),
            });
        }
        self.base.resolve(thread, selection).await
    }
}
