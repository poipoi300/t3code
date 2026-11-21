//! `orchestration.launchThread` (`ws.ts`): the client's workspace strategy is checked by the
//! launch host, then the runtime checks the worktree base and launches through message intake
//! as the user. Served only when the runtime was composed with the launch host.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::WsOrchestrationV2LaunchThreadRpc;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(all(feature = "git-actions", feature = "local-services"))]
    if let (Some(orchestration), Some(launch)) = (
        domains.get::<crate::adapters::orchestration::OrchestrationAdapter>(),
        domains.get::<crate::adapters::launch::LaunchAdapter>(),
    ) {
        return live::register(registry, orchestration, launch);
    }
    let _ = domains;
    registry.unimplemented::<WsOrchestrationV2LaunchThreadRpc>()
}

#[cfg(all(feature = "git-actions", feature = "local-services"))]
mod live {
    use crate::{
        adapters::{
            launch::LaunchAdapter,
            orchestration::{OrchestrationAdapter, OrchestrationError},
        },
        registry::{Failure, HandlerRegistry},
    };
    use serde_json::json;
    use std::sync::Arc;
    use t3_contracts::{
        base_schemas::{CommandId, ProjectId},
        orchestration_v2::*,
        rpc::*,
    };

    fn refused(
        command_id: CommandId,
        project_id: ProjectId,
        message: String,
        cause: Option<serde_json::Value>,
    ) -> Failure<WsOrchestrationV2LaunchThreadError> {
        Failure::Typed(
            WsOrchestrationV2LaunchThreadError::OrchestrationV2ThreadLaunchError(Box::new(
                OrchestrationV2ThreadLaunchError {
                    _tag: ThreadLaunchErrorTag::OrchestrationV2ThreadLaunchError,
                    command_id,
                    project_id,
                    message,
                    cause: cause.map(Some),
                },
            )),
        )
    }

    /// TS `catchTags`: the base-ref, branch-conflict, not-a-repository and disabled-provider
    /// failures keep their own message; every other launch failure is "Failed to launch thread".
    fn message(error: &OrchestrationError) -> String {
        match error {
            OrchestrationError::Service { operation, detail }
                if matches!(
                    *operation,
                    "thread-launch.base-ref"
                        | "thread-launch.branch-conflict"
                        | "thread-launch.not-repository"
                        | "ThreadManagementProviderInstanceDisabledError"
                ) =>
            {
                detail.clone()
            }
            _ => "Failed to launch thread".into(),
        }
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        orchestration: Arc<OrchestrationAdapter>,
        launch: Arc<LaunchAdapter>,
    ) -> crate::Result<()> {
        registry.register::<WsOrchestrationV2LaunchThreadRpc, _, _>(move |ctx, mut input| {
            let (orchestration, launch) = (orchestration.clone(), launch.clone());
            async move {
                let (command_id, project_id) = (input.command_id.clone(), input.project_id.clone());
                if let Err(refusal) = ctx.services.lifecycle.admit_command() {
                    return Err(refused(
                        command_id,
                        project_id,
                        "Failed to launch thread".into(),
                        Some(json!({"message": refusal.to_string()})),
                    ));
                }
                if let Err(reason) = launch
                    .check_client_strategy(&input.project_id, &mut input.workspace_strategy)
                    .await
                {
                    return Err(refused(
                        command_id,
                        project_id,
                        "Failed to launch thread".into(),
                        Some(json!({"message": reason})),
                    ));
                }
                let runtime = orchestration.composed().ok_or_else(|| {
                    refused(
                        command_id.clone(),
                        project_id.clone(),
                        "Failed to launch thread".into(),
                        None,
                    )
                })?;
                let intake = crate::adapters::attachments::session_intake(&ctx, &runtime.intake);
                let mut result = runtime.launch_client_thread_with_intake(input, &intake).await.map_err(|error| {
                    refused(
                        command_id.clone(),
                        project_id.clone(),
                        message(&error),
                        Some(json!({"message": error.to_string()})),
                    )
                })?;
                t3_orchestration::ports::transport::wire_projection::project_thread_projection_for_wire(
                    &mut result.projection,
                )
                .map_err(|error| {
                    refused(
                        command_id,
                        project_id,
                        "Failed to launch thread".into(),
                        Some(json!({"message": error.to_string()})),
                    )
                })?;
                Ok(result)
            }
        })
    }
}
