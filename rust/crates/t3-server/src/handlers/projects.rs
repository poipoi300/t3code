//! Package A: `projects.ensureScratch` and `projects.mutate` over `t3-project`.
#[cfg(feature = "project")]
use crate::registry::{Failure, ServerDefect};
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "project")]
    if let Some(projects) = domains.get::<crate::adapters::project::ProjectAdapter>() {
        let p = projects.clone();
        // TypeScript maps every failure to OrchestrationDispatchCommandError with its message.
        registry.register::<WsProjectsEnsureScratchRpc, _, _>(move |_, _| {
            let p = p.clone();
            async move {
                p.ensure_scratch().await.map_err(|error| {
                    typed(serde_json::json!({
                        "_tag": "OrchestrationDispatchCommandError",
                        "message": error.to_string(),
                    }))
                })
            }
        })?;
        let p = projects;
        // Mutations pass the command gate, as TypeScript's `startup.enqueueCommand` does.
        registry.register::<WsProjectsMutateRpc, _, _>(move |ctx, mutation| {
            let p = p.clone();
            async move {
                let command_id = serde_json::to_value(&mutation)
                    .ok()
                    .and_then(|value| value.get("commandId").cloned())
                    .unwrap_or_default();
                let refuse = |message: String| {
                    typed(serde_json::json!({
                        "_tag": "ProjectMutationError",
                        "commandId": command_id,
                        "message": message,
                    }))
                };
                if let Err(refused) = ctx.services.lifecycle.admit_command() {
                    return Err(refuse(refused.to_string()));
                }
                p.mutate(mutation).await.map_err(|error| {
                    refuse(match &error {
                        crate::adapters::project::ProjectError::NotEmpty(_) => error.to_string(),
                        _ => "Failed to mutate project.".into(),
                    })
                })
            }
        })?;
        return Ok(());
    }
    let _ = domains;
    registry.unimplemented::<WsProjectsEnsureScratchRpc>()?;
    registry.unimplemented::<WsProjectsMutateRpc>()
}

/// A typed failure built from its wire form; a shape the contract rejects becomes a defect.
#[cfg(feature = "project")]
fn typed<E: serde::de::DeserializeOwned>(value: serde_json::Value) -> Failure<E> {
    match serde_json::from_value(value) {
        Ok(error) => Failure::Typed(error),
        Err(error) => Failure::Defect(ServerDefect::Internal {
            message: error.to_string(),
        }),
    }
}
