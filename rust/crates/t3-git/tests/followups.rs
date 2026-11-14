use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    environment::RepositoryIdentity,
    git::*,
    orchestration_project::OrchestrationProjectShell,
};
use t3_git::{
    CreatedPullRequestKey, GitActionEffects, GitActionFollowups, GitActionHost, GitError,
};

struct Host {
    refreshed: Mutex<Vec<String>>,
    linked: Mutex<Vec<CreatedPullRequestKey>>,
}
#[async_trait]
impl GitActionHost for Host {
    async fn thread_project(&self, id: &ThreadId) -> Result<Option<ProjectId>, GitError> {
        Ok((id.0 == "thread").then(|| ProjectId("thread-project".into())))
    }
    async fn project(&self, _: &ProjectId) -> Result<Option<OrchestrationProjectShell>, GitError> {
        Ok(None)
    }
    async fn projects(&self) -> Result<Vec<OrchestrationProjectShell>, GitError> {
        Ok([("matching-a", "repo"), ("matching-b", "repo"), ("other", "elsewhere")].into_iter().map(|(id, root)| serde_json::from_value(json!({"id":id,"title":"Project","workspaceRoot":root,"defaultModelSelection":null,"scripts":[],"createdAt":"2026-11-13T00:00:00Z","updatedAt":"2026-11-13T00:00:00Z"})).expect("project")).collect())
    }
    async fn link_created(&self, _: &ThreadId, key: CreatedPullRequestKey) -> Result<(), GitError> {
        self.linked.lock().expect("links").push(key);
        Err(GitError::Unavailable("link failed".into()))
    }
    async fn refresh_pushed(&self, project: &ProjectId) -> Result<(), GitError> {
        self.refreshed
            .lock()
            .expect("refreshes")
            .push(project.0.clone());
        Ok(())
    }
    fn parse_request_url(&self, url: &str) -> Option<CreatedPullRequestKey> {
        Some(CreatedPullRequestKey {
            host: "github.com".into(),
            repository: "owner/repo".into(),
            number: 17,
            url: url.into(),
        })
    }
    fn repository_request_key(
        &self,
        _: &RepositoryIdentity,
        _: u64,
        _: &str,
    ) -> Option<CreatedPullRequestKey> {
        None
    }
}

#[tokio::test]
async fn created_links_are_best_effort_and_push_refresh_routes_thread_then_project_then_workspace()
{
    let host = Arc::new(Host {
        refreshed: Mutex::new(vec![]),
        linked: Mutex::new(vec![]),
    });
    let service = GitActionFollowups::new(host.clone());
    let result: GitRunStackedActionResult = serde_json::from_value(json!({"action":"commit_push_pr","branch":{"status":"skipped_not_requested"},"commit":{"status":"skipped_no_changes"},"push":{"status":"pushed"},"pr":{"status":"created","number":17,"url":"https://github.com/owner/repo/pull/17"},"toast":{"title":"Done","cta":{"kind":"none"}}})).expect("result");
    for (thread, project) in [
        (Some("thread"), Some("explicit")),
        (Some("missing"), Some("explicit")),
        (None, None),
    ] {
        let input = serde_json::from_value(json!({"cwd":"repo","actionId":"action","action":"commit_push_pr","threadId":thread,"projectId":project})).expect("input");
        service.after_action(&input, &result).await;
    }
    assert_eq!(
        *host.refreshed.lock().expect("refreshes"),
        ["thread-project", "explicit", "matching-a", "matching-b"]
    );
    let linked = host.linked.lock().expect("links");
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].number, 17);
    assert_eq!(linked[0].repository, "owner/repo");
}
