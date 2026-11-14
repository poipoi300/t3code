use crate::{GitActionEffects, GitError};
use async_trait::async_trait;
use std::sync::Arc;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    environment::RepositoryIdentity,
    git::*,
    orchestration_project::OrchestrationProjectShell,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedPullRequestKey {
    pub host: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
}

#[async_trait]
pub trait GitActionHost: Send + Sync {
    async fn thread_project(&self, thread: &ThreadId) -> Result<Option<ProjectId>, GitError>;
    async fn project(
        &self,
        project: &ProjectId,
    ) -> Result<Option<OrchestrationProjectShell>, GitError>;
    async fn projects(&self) -> Result<Vec<OrchestrationProjectShell>, GitError>;
    async fn link_created(
        &self,
        thread: &ThreadId,
        key: CreatedPullRequestKey,
    ) -> Result<(), GitError>;
    async fn refresh_pushed(&self, project: &ProjectId) -> Result<(), GitError>;
    fn parse_request_url(&self, url: &str) -> Option<CreatedPullRequestKey>;
    fn repository_request_key(
        &self,
        identity: &RepositoryIdentity,
        number: u64,
        url: &str,
    ) -> Option<CreatedPullRequestKey>;
}

pub struct GitActionFollowups {
    host: Arc<dyn GitActionHost>,
}
impl GitActionFollowups {
    pub fn new(host: Arc<dyn GitActionHost>) -> Self {
        Self { host }
    }
    async fn link_created(
        &self,
        thread: &ThreadId,
        result: &GitRunStackedActionResult,
    ) -> Result<(), GitError> {
        let Some(project_id) = self.host.thread_project(thread).await? else {
            return Ok(());
        };
        let Some(number) = result.pr.number.flatten() else {
            return Ok(());
        };
        let Some(url) = result
            .pr
            .url
            .as_ref()
            .and_then(Option::as_ref)
            .filter(|url| !url.is_empty())
        else {
            return Ok(());
        };
        if !matches!(
            result.pr.status,
            GitPrStepStatus::Created | GitPrStepStatus::OpenedExisting
        ) {
            return Ok(());
        }
        let project = self.host.project(&project_id).await?;
        let key = self.host.parse_request_url(url).or_else(|| {
            project
                .as_ref()
                .and_then(|p| p.repository_identity.as_ref())
                .and_then(Option::as_ref)
                .and_then(|identity| self.host.repository_request_key(identity, number, url))
        });
        if let Some(key) = key {
            self.host.link_created(thread, key).await?;
        }
        Ok(())
    }
    async fn refresh_pushed(
        &self,
        input: &GitRunStackedActionInput,
        result: &GitRunStackedActionResult,
    ) -> Result<(), GitError> {
        if result.push.status != GitPushStepStatus::Pushed {
            return Ok(());
        }
        if let Some(thread) = input.thread_id.as_ref().and_then(Option::as_ref)
            && let Some(project) = self.host.thread_project(thread).await?
        {
            return self.host.refresh_pushed(&project).await;
        }
        if let Some(project) = input.project_id.as_ref().and_then(Option::as_ref) {
            return self.host.refresh_pushed(project).await;
        }
        for project in self
            .host
            .projects()
            .await?
            .into_iter()
            .filter(|p| p.workspace_root == input.cwd)
        {
            self.host.refresh_pushed(&project.id).await?;
        }
        Ok(())
    }
}
#[async_trait]
impl GitActionEffects for GitActionFollowups {
    async fn after_action(
        &self,
        input: &GitRunStackedActionInput,
        result: &GitRunStackedActionResult,
    ) {
        if let Some(thread) = input.thread_id.as_ref().and_then(Option::as_ref) {
            let _ = self.link_created(thread, result).await;
        }
        let _ = self.refresh_pushed(input, result).await;
    }
}
