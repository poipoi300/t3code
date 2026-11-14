use crate::types::{EnvironmentId, ProjectId};
pub mod chat_canvas;
#[path = "workspace/branch.rs"]
pub mod branch;
#[path = "workspace/git_actions.rs"]
pub mod git_actions;
#[path = "workspace/pr_assignment.rs"]
pub mod pr_assignment;
#[path = "workspace/pr_list.rs"]
pub mod pr_list;
#[path = "workspace/pr_views.rs"]
pub mod pr_views;
#[path = "workspace/project_ops.rs"]
pub mod project_ops;
#[path = "workspace/projects.rs"]
pub mod projects;
#[path = "workspace/pull_requests.rs"]
pub mod pull_requests;
#[path = "workspace/review.rs"]
pub mod review;
#[path = "workspace/sidebar.rs"]
pub mod sidebar;
#[path = "workspace/vcs.rs"]
pub mod vcs;

#[cfg(test)]
#[path = "workspace/tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectKey {
    pub environment: EnvironmentId,
    pub project: ProjectId,
}
