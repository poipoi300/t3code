use super::*;
use crate::rpc::RpcEndpoint;
use futures::future::LocalBoxFuture;
use serde_json::{Value, json};
use t3_contracts::orchestration_project::OrchestrationProjectShell;

#[derive(Clone, Debug)]
pub struct WorkspaceContext {
    pub project: Rc<OrchestrationProjectShell>,
    pub thread: Option<Rc<OrchestrationV2ThreadShell>>,
    pub draft_id: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub env_mode: super::super::branch::EnvMode,
    pub start_from_origin: bool,
    pub manual: bool,
}
impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub fn clear_workspace_context_override(
        &self,
        key: &ThreadKey,
        mode: super::super::branch::EnvMode,
        start_from_origin: bool,
    ) -> bool {
        let mut overrides = self.state.context_overrides.borrow_mut();
        if overrides.get(key) != Some(&(mode, start_from_origin)) {
            return false;
        }
        overrides.remove(key);
        drop(overrides);
        self.state.context_changes.notify();
        true
    }
    pub fn workspace_context(
        &self,
        key: &ThreadKey,
        draft_id: Option<&str>,
    ) -> Option<WorkspaceContext> {
        let thread = self.thread(key);
        let drafts = self.runtime.composer_drafts().document();
        let document = drafts.borrow();
        let sessions = document["draftThreadsByThreadKey"].as_object();
        let draft = sessions.and_then(|sessions| {
            draft_id
                .and_then(|id| sessions.get_key_value(id))
                .filter(|(_, d)| {
                    d["environmentId"] == key.environment.0 && d["threadId"] == key.thread.0
                })
                .or_else(|| {
                    sessions.iter().find(|(_, d)| {
                        d["environmentId"] == key.environment.0 && d["threadId"] == key.thread.0
                    })
                })
        });
        let project_id = thread
            .as_ref()
            .map(|t| t.project_id.0.as_str())
            .or_else(|| draft.and_then(|(_, d)| d["projectId"].as_str()))?;
        let project = self
            .runtime
            .store()
            .shared_shell(&key.environment)?
            .projects
            .iter()
            .find(|p| p.id.0 == project_id)?
            .clone();
        let branch = thread
            .as_ref()
            .map(|t| t.branch.as_ref().map(|b| b.0.clone()))
            .unwrap_or_else(|| draft.and_then(|(_, d)| d["branch"].as_str().map(str::to_owned)));
        let worktree_path = thread
            .as_ref()
            .map(|t| t.worktree_path.as_ref().map(|p| p.0.clone()))
            .unwrap_or_else(|| {
                draft.and_then(|(_, d)| d["worktreePath"].as_str().map(str::to_owned))
            });
        let draft_mode = draft.map(|(_, d)| {
            if d["envMode"] == "worktree" {
                super::super::branch::EnvMode::Worktree
            } else {
                super::super::branch::EnvMode::Local
            }
        });
        let pending = self
            .state
            .context_overrides
            .borrow()
            .get(key)
            .copied()
            .filter(|_| {
                thread.as_ref().is_some_and(|t| {
                    t.active_provider_thread_id.is_none() && t.worktree_path.is_none()
                })
            });
        Some(WorkspaceContext {
            env_mode: pending.map(|p| p.0).unwrap_or_else(|| {
                super::super::branch::effective_env_mode(
                    worktree_path.as_deref(),
                    thread.is_some(),
                    draft_mode,
                    false,
                )
            }),
            start_from_origin: pending
                .map(|p| p.1)
                .unwrap_or_else(|| draft.is_some_and(|(_, d)| d["startFromOrigin"] == true)),
            manual: draft.is_some_and(|(_, d)| d["environmentSelection"] == "manual"),
            project,
            thread,
            draft_id: draft.map(|(id, _)| id.clone()),
            branch,
            worktree_path,
        })
    }
    pub(crate) fn update_draft_context(
        &self,
        key: &ThreadKey,
        draft_id: Option<&str>,
        patch: Value,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        let context = self.workspace_context(key, draft_id);
        async move {
            let id = context
                .and_then(|c| c.draft_id)
                .ok_or(ClientError::Unsupported("draft context is not loaded"))?;
            let drafts = service.runtime.composer_drafts();
            let before = drafts.document().borrow()["draftThreadsByThreadKey"][&id].clone();
            drafts.update(|document| {
                let session = document["draftThreadsByThreadKey"][&id]
                    .as_object_mut()
                    .ok_or(ClientError::Cancelled)?;
                for (k, v) in patch.as_object().into_iter().flatten() {
                    session.insert(k.clone(), v.clone());
                }
                Ok(())
            })?;
            if let Err(error) = drafts.flush().await {
                drafts.update(|document| {
                    if let Some(session) = document["draftThreadsByThreadKey"][&id].as_object_mut()
                    {
                        for (k, v) in patch.as_object().into_iter().flatten() {
                            if session.get(k) == Some(v) {
                                match before.get(k) {
                                    Some(v) => {
                                        session.insert(k.clone(), v.clone());
                                    }
                                    None => {
                                        session.remove(k);
                                    }
                                }
                            }
                        }
                    }
                    Ok(())
                })?;
                return Err(error);
            }
            Ok(())
        }
        .boxed_local()
    }
    pub(crate) async fn set_branch_context(
        &self,
        key: &ThreadKey,
        branch: Option<String>,
        path: Option<String>,
        automatic: bool,
    ) -> Result<(), ClientError> {
        let context = self
            .workspace_context(key, None)
            .ok_or(ClientError::Cancelled)?;
        if automatic && (context.branch.is_some() || context.worktree_path.is_some()) {
            return Err(ClientError::Cancelled);
        }
        if let Some(thread) = context.thread {
            if automatic
                && (thread.active_provider_thread_id.is_some() || thread.worktree_path.is_some())
            {
                return Err(ClientError::Cancelled);
            }
            if context.worktree_path != path && thread.active_run_id.is_some() {
                return Err(ClientError::Rejected {
                    message: "Stop the current turn to switch worktrees.".into(),
                    detail: None,
                });
            }
            if context.worktree_path != path && thread.active_provider_thread_id.is_some() {
                self.runtime.interrupt_thread(key).await?;
            }
            let command = serde_json::from_value(
                json!({"type":"thread.metadata.update","commandId":crate::operations::allocate_command_id(&self.platform,None)?,"threadId":key.thread,"branch":branch,"worktreePath":path,"expectedEmpty":automatic}),
            )?;
            self.session(&key.environment)?
                .call::<t3_contracts::rpc::WsOrchestrationV2DispatchCommandRpc>(command)
                .await?;
            return Ok(());
        }
        let mode = super::super::branch::draft_env_mode_after_branch_change(
            path.as_deref(),
            context.worktree_path.as_deref(),
            context.env_mode,
        );
        self.update_draft_context(key, context.draft_id.as_deref(), json!({"branch":branch,"worktreePath":path,"envMode":if mode == super::super::branch::EnvMode::Worktree {"worktree"} else {"local"},"environmentSelection":if automatic && !context.manual {"auto"} else {"manual"}})).await
    }
}
