//! Cleanup for accepted deletions, including side chats removed by the server.

use super::ThreadOperations;
use crate::{
    ClientError,
    platform::Platform,
    rpc::{RpcEndpoint, SessionEndpoint},
    services::settings::scoped::{resolve_project_settings, resolve_worktree_cleanup},
    types::ThreadKey,
    workspace::{
        ProjectKey,
        project_ops::paths::{
            format_worktree_path_for_display, get_orphaned_worktree_path_for_thread,
        },
        projects::normalize_project_path_for_comparison,
    },
};
use futures::{FutureExt, StreamExt, channel::mpsc, stream::LocalBoxStream};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};
use t3_contracts::{
    base_schemas::{ThreadId, TrimmedNonEmptyString},
    git::VcsRemoveWorktreeInput,
    orchestration_v2::{
        OrchestrationV2GetThreadFamilyInput, OrchestrationV2GetWorktreeThreadIdsInput,
        OrchestrationV2ThreadShell,
    },
    rpc::{
        WsOrchestrationV2GetThreadFamilyRpc, WsOrchestrationV2GetWorktreeThreadIdsRpc,
        WsVcsRefreshStatusPayload, WsVcsRefreshStatusRpc, WsVcsRemoveWorktreeRpc,
    },
};

pub(super) fn is_side_chat_of(row: &OrchestrationV2ThreadShell, parent: &ThreadId) -> bool {
    row.lineage.side_chat == Some(Some(true))
        && row.lineage.parent_thread_id.as_ref() == Some(parent)
}

pub(super) struct Deletion {
    threads: Rc<[ThreadKey]>,
    worktree: Option<OrphanedWorktree>,
}

struct OrphanedWorktree {
    input: VcsRemoveWorktreeInput,
    check_server: bool,
}

#[derive(Clone)]
pub enum DeletionEvent {
    Accepted(Rc<[ThreadKey]>),
    CleanupFailed {
        title: &'static str,
        message: String,
    },
}

#[derive(Default)]
pub(super) struct DeletionState {
    listeners: RefCell<Vec<mpsc::UnboundedSender<DeletionEvent>>>,
}

impl DeletionState {
    fn publish(&self, event: DeletionEvent) {
        self.listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(event.clone()).is_ok());
    }

    fn failed(&self, title: &'static str, error: impl std::fmt::Display) {
        self.publish(DeletionEvent::CleanupFailed {
            title,
            message: error.to_string(),
        });
    }
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn deletion_events(&self) -> LocalBoxStream<'static, DeletionEvent> {
        let (send, receive) = mpsc::unbounded();
        self.state.deletion.listeners.borrow_mut().push(send);
        receive.boxed_local()
    }

    pub(super) async fn prepare_deletion(
        &self,
        key: &ThreadKey,
        row: &OrchestrationV2ThreadShell,
        endpoint: &SessionEndpoint,
    ) -> Result<Deletion, ClientError> {
        let snapshot = self.runtime.store().snapshot();
        let config = snapshot
            .environments
            .get(&key.environment)
            .and_then(|env| env.config.clone());
        let families = config
            .as_ref()
            .is_some_and(|config| config.shell_thread_families == Some(true));
        let worktree = self
            .confirm_orphaned_worktree(key, row, endpoint, config.as_deref())
            .await?;
        // The confirmation can sit open while the server adds more side chats.
        let mut deleted = BTreeSet::from([key.clone()]);
        let side_chats = |rows: &[OrchestrationV2ThreadShell]| {
            rows.iter()
                .filter(|row| is_side_chat_of(row, &key.thread))
                .map(|row| ThreadKey {
                    environment: key.environment.clone(),
                    thread: row.id.clone(),
                })
                .collect::<Vec<_>>()
        };
        if families
            && let Ok(family) = endpoint
                .call::<WsOrchestrationV2GetThreadFamilyRpc>(OrchestrationV2GetThreadFamilyInput {
                    thread_id: key.thread.clone(),
                })
                .await
        {
            deleted.extend(side_chats(&family.threads));
        }
        if let Some(shell) = self.shell(&key.environment) {
            deleted.extend(side_chats(&shell.threads));
            deleted.extend(side_chats(&shell.archived_threads));
        }
        Ok(Deletion {
            threads: deleted.into_iter().collect(),
            worktree,
        })
    }

    async fn confirm_orphaned_worktree(
        &self,
        key: &ThreadKey,
        row: &OrchestrationV2ThreadShell,
        endpoint: &SessionEndpoint,
        config: Option<&t3_contracts::server::ServerConfig>,
    ) -> Result<Option<OrphanedWorktree>, ClientError> {
        let project = ProjectKey {
            environment: key.environment.clone(),
            project: row.project_id.clone(),
        };
        let confirmation = self.state.confirmation.borrow().clone();
        let (Some(shell), Some(project), Some(config), Some(confirm)) = (
            self.shell(&key.environment),
            self.project(&project).filter(|project| !project.is_scratch),
            config,
            confirmation,
        ) else {
            return Ok(None);
        };
        let paths = shell
            .threads
            .iter()
            .chain(&shell.archived_threads)
            .map(|row| {
                (
                    ThreadKey {
                        environment: key.environment.clone(),
                        thread: row.id.clone(),
                    },
                    row.worktree_path.as_ref().map(|path| path.0.clone()),
                )
            })
            .collect::<Vec<_>>();
        let Some(path) = get_orphaned_worktree_path_for_thread(&paths, key) else {
            return Ok(None);
        };
        let settings = Rc::new(config.settings.clone());
        let effective = resolve_project_settings(&settings, Some(&row.project_id));
        let settings = serde_json::to_value(settings.as_ref())?;
        if resolve_worktree_cleanup(&settings, &effective.json)["worktreeOnDelete"] == true {
            return Ok(None);
        }
        let input = VcsRemoveWorktreeInput {
            cwd: project.project.workspace_root.clone(),
            path: TrimmedNonEmptyString(path.to_owned()),
            force: Some(Some(true)),
        };
        let check_server = config.shell_thread_families == Some(true);
        if !endpoint.available::<WsVcsRemoveWorktreeRpc>(&input)
            || (check_server && !only_worktree_thread(endpoint, key, &input.path).await)
        {
            return Ok(None);
        }
        let approved = confirm(
            format!("This thread is the only one linked to this worktree:\n{}\n\nDelete the worktree too?",
                format_worktree_path_for_display(path)),
            true,
        ).await?;
        Ok(approved.then_some(OrphanedWorktree {
            input,
            check_server,
        }))
    }

    pub(super) fn accept_deletion(
        &self,
        key: ThreadKey,
        deletion: Deletion,
        endpoint: SessionEndpoint,
        sequence: u64,
        persistence_error: Option<ClientError>,
    ) {
        for thread in deletion.threads.iter() {
            self.runtime.terminals().clear_ui(thread);
        }
        self.state
            .deletion
            .publish(DeletionEvent::Accepted(deletion.threads.clone()));
        if let Some(error) = persistence_error {
            self.state
                .deletion
                .failed("Could not save the accepted deletion locally", error);
        }
        let service = self.clone();
        self.runtime.spawn(
            async move {
                let drafts = async {
                    if let Err(error) = service
                        .runtime
                        .discard_deleted_thread_drafts(&deletion.threads)
                        .await
                    {
                        service.state.deletion.failed(
                            "Could not finish cleaning up the deleted thread’s drafts and uploads",
                            error,
                        );
                    }
                };
                let worktree = async {
                    if let Some(worktree) = deletion.worktree {
                        service
                            .remove_deleted_thread_worktree(&key, &endpoint, worktree)
                            .await;
                    }
                };
                let refresh = service.refresh_archived_shell(&key.environment, sequence);
                futures::join!(drafts, worktree, refresh);
            }
            .boxed_local(),
        );
    }

    async fn remove_deleted_thread_worktree(
        &self,
        key: &ThreadKey,
        endpoint: &SessionEndpoint,
        worktree: OrphanedWorktree,
    ) {
        let OrphanedWorktree {
            input,
            check_server,
        } = worktree;
        let path = normalize_project_path_for_comparison(&input.path.0);
        let occupied = self.shell(&key.environment).is_none_or(|shell| {
            shell
                .threads
                .iter()
                .chain(&shell.archived_threads)
                .any(|row| {
                    row.id != key.thread
                        && row.worktree_path.as_ref().is_some_and(|other| {
                            normalize_project_path_for_comparison(&other.0) == path
                        })
                })
        });
        let title = "Could not remove the deleted thread’s worktree";
        if occupied || (check_server && !only_worktree_thread(endpoint, key, &input.path).await) {
            self.state
                .deletion
                .failed(title, "The worktree may still be in use.");
            return;
        }
        let cwd = input.cwd.clone();
        if let Err(error) = endpoint.call::<WsVcsRemoveWorktreeRpc>(input).await {
            self.state.deletion.failed(title, error);
            return;
        }
        if let Err(error) = endpoint
            .call::<WsVcsRefreshStatusRpc>(WsVcsRefreshStatusPayload {
                cwd,
                local_only: None,
            })
            .await
        {
            self.state.deletion.failed(
                "Worktree removed. Could not refresh source control status.",
                error,
            );
        }
    }
}

async fn only_worktree_thread(
    endpoint: &SessionEndpoint,
    target: &ThreadKey,
    path: &TrimmedNonEmptyString,
) -> bool {
    endpoint
        .call::<WsOrchestrationV2GetWorktreeThreadIdsRpc>(
            OrchestrationV2GetWorktreeThreadIdsInput {
                worktree_path: path.clone(),
            },
        )
        .await
        .is_ok_and(|occupants| occupants.thread_ids.iter().all(|id| id == &target.thread))
}
