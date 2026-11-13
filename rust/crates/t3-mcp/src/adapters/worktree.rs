use super::{decoded, not_available, orchestration::OrchestrationAdapter};
use crate::{services::WorktreeService, *};
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::git::VcsListRefsResult;

pub struct WorktreeAdapter {
    pub orchestration: Arc<OrchestrationAdapter>,
}
#[async_trait]
impl WorktreeService for WorktreeAdapter {
    async fn t3_worktree_list(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<VcsListRefsResult> {
        let mut input = input.into_value();
        let thread = input["threadId"]
            .as_str()
            .map(|id| t3_contracts::base_schemas::ThreadId(id.into()))
            .or_else(|| context.scope.thread.as_ref().map(|t| t.thread_id.clone()))
            .ok_or_else(|| {
                McpError::failure(
                    "target_required",
                    "Pass threadId: this MCP client is not running inside a T3 thread.",
                )
            })?;
        let shell = self.orchestration.shell(thread).await?;
        let project_id = decoded(shell["projectId"].clone())?;
        let project = self
            .orchestration
            .services
            .database
            .read(move |db| t3_orchestration::stores::projects::get(db, &project_id, false))
            .await
            .map_err(|_| McpError::unavailable())?
            .ok_or_else(|| McpError::failure("invalid_request", "The project was not found."))?;
        let mut cwd = project.workspace_root;
        if let Some(path) = shell["worktreePath"].as_str()
            && tokio::fs::try_exists(path).await.unwrap_or(false)
        {
            cwd = path.into();
        }
        input
            .as_object_mut()
            .ok_or(McpError::Internal)?
            .remove("threadId");
        input["cwd"] = serde_json::json!(cwd);
        let git = t3_git::Git::new(&cwd);
        git.list_branches(&decoded(input)?)
            .await
            .map_err(|error| match error {
                t3_git::GitError::Unsupported(_) => {
                    not_available("t3_worktree_list", "Git::list_branches")
                }
                _ => McpError::unavailable(),
            })
    }
}
