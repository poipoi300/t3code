//! Package A: `projectClone.{start, cancel, retry}` and `subscribeProjectClones` over the project
//! service's clone tracker, whose backend is the forge registry's (`adapters/forge.rs`).
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

#[cfg(all(feature = "project", feature = "forge"))]
mod live {
    use super::*;
    use crate::{
        adapters::{
            forge::{ForgeAdapter, ProjectCloneRefusal, Refusal},
            project::{ProjectAdapter, ProjectError},
        },
        registry::{Failure, ServerDefect},
    };
    use std::sync::Arc;
    use t3_contracts::{
        project_clone::ProjectCloneActionResult,
        source_control::{
            SourceControlProviderKind, SourceControlRepositoryError,
            SourceControlRepositoryErrorTag,
        },
    };

    fn internal<E>(error: ProjectError) -> Failure<E> {
        Failure::Defect(ServerDefect::Internal {
            message: error.to_string(),
        })
    }

    /// The tracker reports a failed preparation or discard as the repository service's detail.
    fn repository(
        provider: SourceControlProviderKind,
        error: ProjectError,
    ) -> Result<Box<SourceControlRepositoryError>, ProjectError> {
        match error {
            ProjectError::Operation {
                operation: "cloneRepository",
                detail,
            } => Ok(Box::new(SourceControlRepositoryError {
                _tag: SourceControlRepositoryErrorTag::SourceControlRepositoryError,
                provider,
                operation: "cloneRepository".into(),
                detail,
                cause: None,
            })),
            other => Err(other),
        }
    }

    /// TypeScript maps a failed project creation to `OrchestrationDispatchCommandError`
    /// ("Failed to create clone project."); refusals the tracker words itself keep their text.
    fn start_error(refusal: ProjectCloneRefusal) -> Failure<WsProjectCloneStartError> {
        use t3_contracts::orchestration_dispatch::*;
        let dispatch = |message: String| {
            Failure::Typed(WsProjectCloneStartError::OrchestrationDispatchCommandError(
                Box::new(OrchestrationDispatchCommandError {
                    _tag: OrchestrationDispatchCommandErrorTag::OrchestrationDispatchCommandError,
                    message: message.into(),
                    cause: None,
                    bootstrap_thread_disposition: None,
                }),
            ))
        };
        match refusal {
            ProjectCloneRefusal::Refused(Refusal::Authorization(e)) => {
                Failure::Typed(WsProjectCloneStartError::EnvironmentAuthorizationError(e))
            }
            ProjectCloneRefusal::Refused(Refusal::Repository(e)) => {
                Failure::Typed(WsProjectCloneStartError::SourceControlRepositoryError(e))
            }
            ProjectCloneRefusal::Project(provider, error) => match repository(provider, *error) {
                Ok(e) => Failure::Typed(WsProjectCloneStartError::SourceControlRepositoryError(e)),
                Err(error @ (ProjectError::Invalid(_) | ProjectError::Conflict { .. })) => {
                    dispatch(error.to_string())
                }
                Err(_) => dispatch("Failed to create clone project.".into()),
            },
        }
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        projects: Arc<ProjectAdapter>,
        forge: Arc<ForgeAdapter>,
    ) -> crate::Result<()> {
        let (p, f) = (projects.clone(), forge);
        registry.register::<WsProjectCloneStartRpc, _, _>(move |_, input| {
            let (p, f) = (p.clone(), f.clone());
            async move {
                f.start_project_clone(p.service(), input)
                    .await
                    .map_err(start_error)
            }
        })?;
        let p = projects.clone();
        registry.register::<WsProjectCloneCancelRpc, _, _>(move |_, input| {
            let p = p.clone();
            async move {
                p.service()
                    .clones()
                    .cancel(&input.project_id)
                    .await
                    .map(|applied| ProjectCloneActionResult { applied })
                    .map_err(internal)
            }
        })?;
        let p = projects.clone();
        registry.register::<WsProjectCloneRetryRpc, _, _>(move |_, input| {
            let p = p.clone();
            async move {
                p.service()
                    .clones()
                    .retry(&input.project_id)
                    .await
                    .map(|applied| ProjectCloneActionResult { applied })
                    .map_err(|error| {
                        match repository(SourceControlProviderKind("unknown".into()), error) {
                            Ok(e) => Failure::Typed(
                                WsSourceControlLookupRepositoryError::SourceControlRepositoryError(
                                    e,
                                ),
                            ),
                            Err(other) => internal(other),
                        }
                    })
            }
        })?;
        // The current list first, then each change; it ends with the socket, or at shutdown.
        let p = projects;
        registry.register_stream::<WsSubscribeProjectClonesRpc, _, _, _>(move |ctx, _| {
            let mut changes = p.service().clones().subscribe_project_clones();
            let shutdown = ctx.services.shutdown.clone();
            async move {
                Ok(async_stream::stream! {
                    let current = changes.borrow_and_update().clone();
                    yield Ok(current);
                    loop {
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            changed = changes.changed() => {
                                if changed.is_err() {
                                    break;
                                }
                                let next = changes.borrow_and_update().clone();
                                yield Ok(next);
                            }
                        }
                    }
                })
            }
        })?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Project errors carry Git stderr, argv and URLs; a client sees fixed text only.
        #[test]
        fn internal_failures_reach_the_client_as_fixed_text() {
            let error = ProjectError::Operation {
                operation: "cloneRepository",
                detail: "fatal: https://user:secret@example.invalid/repo.git".into(),
            };
            let Failure::<()>::Defect(ServerDefect::Internal { message }) = internal(error) else {
                panic!("expected an internal defect");
            };
            assert_eq!(message, "The project clone could not be updated.");
        }
    }
}

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(all(feature = "project", feature = "forge"))]
    if let (Some(projects), Some(forge)) = (
        domains.get::<crate::adapters::project::ProjectAdapter>(),
        domains.get::<crate::adapters::forge::ForgeAdapter>(),
    ) {
        return live::register(registry, projects, forge);
    }
    let _ = domains;
    registry.unimplemented::<WsProjectCloneStartRpc>()?;
    registry.unimplemented::<WsProjectCloneCancelRpc>()?;
    registry.unimplemented::<WsProjectCloneRetryRpc>()?;
    registry.unimplemented::<WsSubscribeProjectClonesRpc>()
}
