//! `vcs.trustRepository`, a Rust-only method (`t3_contracts::rpc::RUST_ONLY_METHODS`). After the
//! server refused a folder as `checkpoint.git.repository-changed` (t3-git H-1: a repository
//! replaced, or created after the folder was first seen without one), the user trusts the
//! repository now there. The folder comes from the project's or thread's own record, never from
//! the client. Agents cannot reach it: it is no MCP tool, and an agent's thread credential is
//! not an RPC session.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::WsVcsTrustRepositoryRpc;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(all(
        feature = "git",
        feature = "workspace",
        feature = "orchestration",
        feature = "local-services"
    ))]
    if let (Some(git), Some(workspaces), Some(orchestration)) = (
        domains.get::<crate::adapters::git::GitAdapter>(),
        domains.get::<crate::workspaces::EnvironmentWorkspaces>(),
        domains.get::<crate::adapters::orchestration::OrchestrationAdapter>(),
    ) && git.vcs().is_some()
    {
        return live::register(
            registry,
            live::Trust {
                git,
                workspaces,
                orchestration,
            },
        );
    }
    let _ = domains;
    registry.unimplemented::<WsVcsTrustRepositoryRpc>()
}

#[cfg(all(
    feature = "git",
    feature = "workspace",
    feature = "orchestration",
    feature = "local-services"
))]
mod live {
    use super::*;
    use crate::{
        adapters::{
            git::{Error, GitAdapter, command_error},
            orchestration::OrchestrationAdapter,
        },
        registry::Failure,
        workspaces::EnvironmentWorkspaces,
    };
    use std::sync::Arc;
    use t3_contracts::{
        git::{GitCommandError, VcsTrustRepositoryInput, VcsTrustRepositoryResult},
        rpc::{RpcMethod, WsVcsPullError},
    };

    pub struct Trust {
        pub git: Arc<GitAdapter>,
        pub workspaces: Arc<EnvironmentWorkspaces>,
        pub orchestration: Arc<OrchestrationAdapter>,
    }

    type Refusal = Failure<WsVcsPullError>;

    fn refused(detail: &str) -> Refusal {
        Failure::Typed(WsVcsPullError::GitCommandError(Box::new(GitCommandError {
            detail: detail.into(),
            ..command_error(WsVcsTrustRepositoryRpc::TAG)
        })))
    }

    fn git_error(error: Error) -> Refusal {
        Failure::Typed(match error {
            Error::Authorization(e) => WsVcsPullError::EnvironmentAuthorizationError(e),
            Error::Git(e) => WsVcsPullError::GitCommandError(e),
            Error::Vcs(_) => WsVcsPullError::GitCommandError(Box::new(command_error(
                WsVcsTrustRepositoryRpc::TAG,
            ))),
        })
    }

    impl Trust {
        async fn binding(
            &self,
            folder: &std::path::Path,
        ) -> Result<Option<t3_git::RepositoryBinding>, Refusal> {
            self.orchestration
                .repository_binding(folder)
                .await
                .map(Option::flatten)
                .map_err(|_| refused("T3 Code cannot read its saved repository bindings."))
        }

        async fn trust(
            &self,
            services: &crate::services::ServiceRegistry,
            input: VcsTrustRepositoryInput,
        ) -> Result<VcsTrustRepositoryResult, Refusal> {
            let (project, thread) = match (&input.project_id, &input.thread_id) {
                (Some(project), None) => (Some(project.0.as_str()), None),
                (None, Some(thread)) => (None, Some(thread.0.as_str())),
                _ => return Err(refused("Name either a project or a thread.")),
            };
            let (folder, project_root) = self
                .workspaces
                .owned_folder(project, thread)
                .await
                .ok_or_else(|| refused("That project or thread does not exist."))?;
            // Not while a restore holds the folder: it runs against the binding being replaced.
            let _admitted =
                crate::writers::admit(services, "a repository trust", &folder.to_string_lossy())
                    .await;
            self.git
                .trust_repository(&folder)
                .await
                .map_err(git_error)?;
            let bound = self.binding(&folder).await?.ok_or_else(|| {
                refused("No Git repository is in this folder; T3 Code now records it as not a repository.")
            })?;
            let worktree_of_project = bound.git_dir != bound.common_dir
                && folder != project_root
                && self
                    .binding(&project_root)
                    .await?
                    .is_some_and(|root| root.common_dir == bound.common_dir);
            Ok(VcsTrustRepositoryResult {
                top_level: bound.top_level.to_string_lossy().into_owned(),
                worktree_of_project,
            })
        }
    }

    pub fn register(registry: &mut HandlerRegistry, trust: Trust) -> crate::Result<()> {
        let trust = Arc::new(trust);
        registry.register::<WsVcsTrustRepositoryRpc, _, _>(move |ctx, input| {
            let trust = trust.clone();
            async move { trust.trust(&ctx.services, input).await }
        })
    }
}
