use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use t3_forge::{
    FailureReason, Forge, Result, bindings::*, forge::*, model::*, registry::ForgeRegistry,
};
#[derive(Clone)]
struct Driver {
    kind: String,
    caps: PullRequestCapabilities,
    calls: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl Forge for Driver {
    fn kind(&self) -> SourceControlProviderKind {
        self.kind.clone().into()
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.caps
    }
    fn supports(&self, operation: &str) -> bool {
        [
            "get_change_request_summary",
            "checkout_list_change_requests",
            "refine_repository_identity",
        ]
        .contains(&operation)
            || self.kind == "forgejo" && operation == "resolve_remote_web_host"
    }
    async fn refine_repository_identity(
        &self,
        identity: RepositoryIdentity,
    ) -> Result<RepositoryIdentity> {
        Ok(identity)
    }
    async fn resolve_remote_web_host(&self, _: CheckoutInput) -> Result<Option<String>> {
        Ok(Some("git.example:3000".into()))
    }
    async fn with_verified_credential(&self, _: VerifiedCredentialInput) -> Result<VerifiedForge> {
        Ok(VerifiedForge {
            identity: RoutingIdentity {
                account_id: "account-one".into(),
                viewer: "fixture".into(),
                credential_fingerprint: "fingerprint".into(),
            },
            forge: Arc::new(self.clone()),
        })
    }
    async fn checkout_list_change_requests(
        &self,
        input: CheckoutListInput,
    ) -> Result<Vec<ChangeRequest>> {
        assert!(input.state.is_none());
        self.calls.lock().unwrap().push(input.head_selector);
        Ok(vec![serde_json::from_value(json!({"provider":"github","number":7,"title":"Fixture","url":"https://github.com/owner/repo/pull/7","headRefName":"feature","baseRefName":"main","state":"open","updatedAt":{"_tag":"Some","value":"2026-11-13T00:00:00.000Z"}})).unwrap()])
    }
    async fn get_change_request_summary(
        &self,
        input: PullRequestInput,
    ) -> Result<ProviderChangeRequestSummary> {
        self.calls.lock().unwrap().push(input.repo.repository);
        Ok(serde_json::from_value(json!({"number":7,"title":"Merged fixture","url":"https://github.com/owner/repo/pull/7","headBranch":"feature","baseBranch":"main","state":"merged","isDraft":false,"updatedAt":"2026-11-13T01:00:00.000Z","mergedAt":"2026-11-13T01:00:00.000Z"})).unwrap())
    }
}
fn driver(kind: &str) -> Arc<Driver> {
    Arc::new(Driver{kind:kind.into(),caps:serde_json::from_value(json!({"diff":true,"comment":true,"actions":[],"mergeMethods":[],"search":false,"review":{"inlineComment":false,"reply":false,"resolve":false,"verdicts":[]},"reviewers":{"request":false,"listCandidates":false}})).unwrap(),calls:Arc::default()})
}
fn record(dir: &std::path::Path) -> ProjectRecord {
    ProjectRecord{id:"project-one".into(),title:"Fixture".into(),workspace_root:dir.into(),repository_identity:Some(serde_json::from_value(json!({"canonicalKey":"github.com/owner/repo","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"git@github.com:owner/repo.git"},"provider":"github","webUrl":"https://github.com/owner/repo"})).unwrap()),remove_agent_credits_on_merge:true}
}
#[tokio::test]
async fn bindings_refuse_relative_roots_wrong_accounts_and_foreign_private_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let driver = driver("github");
    let bindings = ForgeBindings::new(Arc::new(ForgeRegistry::new([
        driver.clone() as Arc<dyn Forge>
    ])));
    let mut relative = record(std::path::Path::new("relative"));
    assert!(bindings.bind(relative.clone()).await.is_err());
    relative.workspace_root = dir.path().into();
    relative.repository_identity.as_mut().unwrap().root_path = Some("C:/outside-grant".into());
    let binding = bindings.bind(relative).await.unwrap().unwrap();
    assert_eq!(
        binding
            .project
            .repository_identity
            .as_ref()
            .unwrap()
            .root_path
            .as_ref()
            .unwrap()
            .0,
        dir.path().to_string_lossy()
    );
    assert_eq!(
        bindings
            .account(&binding, "wrong")
            .await
            .err()
            .unwrap()
            .reason(),
        Some(FailureReason::Forbidden)
    );
    assert!(
        bindings
            .resolve_watch_target(
                &binding,
                WatchTarget {
                    url: Some("https://token@github.com/owner/repo/pull/7".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert!(
        bindings
            .resolve_watch_target(
                &binding,
                WatchTarget {
                    repository: Some("owner/../escape".into()),
                    number: Some(7),
                    ..Default::default()
                }
            )
            .is_err()
    );
    let link=serde_json::from_value(json!({"host":"private.example","repository":"owner/repo","number":7,"url":"https://private.example/owner/repo/pull/7","source":"manual","linkedAt":"2026-11-13T00:00:00.000Z","snapshot":null,"stack":null})).unwrap();
    assert_eq!(
        bindings
            .refresh_links(
                &binding,
                vec![link],
                t3_contracts::wire::WireDateTime("2026-11-13T01:00:00.000Z".into())
            )
            .await
            .unwrap_err()
            .reason(),
        Some(FailureReason::Forbidden)
    );
    assert!(driver.calls.lock().unwrap().is_empty());
    let verified = bindings.account(&binding, "account-one").await.unwrap();
    assert_eq!(verified.identity.viewer, "fixture");
}
#[tokio::test]
async fn discovery_and_refresh_preserve_link_policy_while_updating_native_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let driver = driver("github");
    let bindings = ForgeBindings::new(Arc::new(ForgeRegistry::new([
        driver.clone() as Arc<dyn Forge>
    ])));
    let binding = bindings.bind(record(dir.path())).await.unwrap().unwrap();
    let now = t3_contracts::wire::WireDateTime("2026-11-13T00:00:00.000Z".into());
    let mut links = bindings
        .discover_links(
            &binding,
            "feature".into(),
            now,
            t3_contracts::thread_pull_request::ThreadPullRequestLinkSource::Agent,
        )
        .await
        .unwrap();
    let snapshot = serde_json::to_value(&links[0].snapshot).unwrap();
    assert_eq!(snapshot["updatedAt"], "2026-11-13T00:00:00.000Z");
    assert!(snapshot.get("closedAt").is_none());
    links[0].watch = Some(None);
    let before = links[0].clone();
    let links = bindings
        .refresh_links(
            &binding,
            links,
            t3_contracts::wire::WireDateTime("2026-11-13T01:00:00.000Z".into()),
        )
        .await
        .unwrap();
    assert_eq!(links[0].linked_at, before.linked_at);
    assert_eq!(links[0].source, before.source);
    assert_eq!(links[0].watch, before.watch);
    assert_eq!(
        links[0].snapshot.as_ref().unwrap().state,
        PullRequestState::Merged
    );
    assert_eq!(
        driver.calls.lock().unwrap().as_slice(),
        ["feature", "owner/repo"]
    );
}
#[tokio::test]
async fn azure_project_binding_joins_ssh_web_keys_and_decodes_the_cli_selector() {
    let dir = tempfile::tempdir().unwrap();
    let driver = driver("azure-devops");
    let bindings = ForgeBindings::new(Arc::new(ForgeRegistry::new([driver as Arc<dyn Forge>])));
    let mut project = record(dir.path());
    project.repository_identity=Some(serde_json::from_value(json!({"canonicalKey":"ssh.dev.azure.com/v3/Org/Project/Repo%20One","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"git@ssh.dev.azure.com:v3/Org/Project/Repo%20One"},"provider":"azure-devops","name":"Repo%20One"})).unwrap());
    let binding = bindings.bind(project).await.unwrap().unwrap();
    assert_eq!(binding.host, "dev.azure.com");
    assert_eq!(binding.repository, "Repo One");
    assert_eq!(
        binding.canonical_repository_key,
        "dev.azure.com/org/project/_git/repo%20one"
    );
    let target = bindings
        .resolve_watch_target(
            &binding,
            WatchTarget {
                url: Some(
                    "https://org.visualstudio.com/Project/_git/Repo%20One/pullrequest/7".into(),
                ),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(target.key.host.0, "dev.azure.com");
    assert_eq!(target.key.repository.0, "org/project/_git/repo%20one");
}
#[tokio::test]
async fn mounted_ssh_alias_bindings_use_the_serving_web_key_for_watch_targets() {
    let dir = tempfile::tempdir().unwrap();
    let driver = driver("forgejo");
    let bindings = ForgeBindings::new(Arc::new(ForgeRegistry::new([driver as Arc<dyn Forge>])));
    let mut project = record(dir.path());
    project.repository_identity=Some(serde_json::from_value(json!({"canonicalKey":"alias/owner/repo","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"git@alias:owner/repo.git"},"provider":"forgejo","webUrl":"http://git.example:3000/mount/owner/repo"})).unwrap());
    let binding = bindings.bind(project).await.unwrap().unwrap();
    assert_eq!(binding.repository, "owner/repo");
    assert_eq!(
        binding.canonical_repository_key,
        "git.example:3000/mount/owner/repo"
    );
    let target = bindings
        .resolve_watch_target(
            &binding,
            WatchTarget {
                repository: Some("owner/repo".into()),
                number: Some(7),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        target.url,
        "http://git.example:3000/mount/owner/repo/pulls/7"
    );
    assert_eq!(target.key.repository.0, "mount/owner/repo");
}
