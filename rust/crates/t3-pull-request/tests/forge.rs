#![cfg(feature = "forge")]
mod support;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::*;
use t3_forge::{Forge, ForgeError, forge as f, model::*};
use t3_pull_request::{
    PullRequestService,
    cache::{MemoryStore, PullRequestReadCache},
    context::Binding,
    forge::{ForgeHost, parse_diff_paths},
};
struct ScriptedForge {
    caps: PullRequestCapabilities,
    requests: Mutex<Vec<Value>>,
    failure: Mutex<Option<t3_forge::FailureReason>>,
}
impl ScriptedForge {
    fn new() -> Self {
        Self {
            caps: capabilities(),
            requests: Default::default(),
            failure: Default::default(),
        }
    }
}
#[async_trait]
impl Forge for ScriptedForge {
    fn kind(&self) -> SourceControlProviderKind {
        SourceControlProviderKind("github".into())
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.caps
    }
    fn supports(&self, op: &str) -> bool {
        !matches!(op, "get_diff_file_paths" | "change_request_events")
    }
    fn batches_reads(&self) -> bool {
        true
    }
    fn reads_public_host_without_checkout(&self) -> bool {
        true
    }
    fn unavailable_requirement(&self) -> Option<f::UnavailableRequirement> {
        Some(f::UnavailableRequirement {
            missing: "Install the test forge tool.".into(),
            unauthenticated: "Authenticate the test forge account.".into(),
        })
    }
    async fn get_viewer(&self, _: f::ViewerInput) -> t3_forge::Result<String> {
        Ok("alice".into())
    }
    async fn get_change_request_summary(
        &self,
        input: f::PullRequestInput,
    ) -> t3_forge::Result<ProviderChangeRequestSummary> {
        if let Some(reason) = *self.failure.lock().expect("failure") {
            return Err(ForgeError::provider(
                "github",
                "summary",
                reason,
                "Raw provider failure.",
            ));
        }
        self.requests.lock().expect("requests").push(json!({"op":"summary","cwd":input.repo.cwd,"repository":input.repo.repository,"number":input.number}));
        Ok(decode(json!(summary("open"))))
    }
    async fn get_viewer_permissions(
        &self,
        _: f::PermissionsInput,
    ) -> t3_forge::Result<PullRequestViewerPermissions> {
        Ok(permissions())
    }
    async fn comment(&self, input: f::CommentInput) -> t3_forge::Result<()> {
        self.requests
            .lock()
            .expect("requests")
            .push(json!({"op":"comment","body":input.body}));
        Ok(())
    }
    async fn list_change_requests_across(
        &self,
        input: f::ListAcrossInput,
    ) -> t3_forge::Result<ProviderBatchedChangeRequestPage> {
        self.requests
            .lock()
            .expect("requests")
            .push(json!({"op":"across","repositories":input.repositories}));
        let item = |repository: &str, number: u64| {
            let mut row = json!(row(number, "2026-01-01T00:00:00.000Z"));
            row["repository"] = json!(repository);
            row["reviewRequestLogins"] = json!(["alice"]);
            decode(row)
        };
        Ok(ProviderBatchedChangeRequestPage {
            items: vec![item("owner/repo", 42), item("owner/second", 43)],
            truncated: false,
            unresolved: None,
        })
    }
    async fn get_diff(&self, _: f::DiffInput) -> t3_forge::Result<ProviderDiffSlice> {
        Ok(ProviderDiffSlice {
            patch: "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n"
                .into(),
            truncated: false,
            next_cursor: None,
            omitted_file_stats: None,
        })
    }
    async fn get_diff_file_contents(
        &self,
        input: f::FileContentsInput,
    ) -> t3_forge::Result<ProviderDiffFileContents> {
        self.requests
            .lock()
            .expect("requests")
            .push(json!({"op":"contents","path":input.new_path}));
        Ok(ProviderDiffFileContents {
            old_contents: "old".into(),
            new_contents: "new".into(),
        })
    }
}
fn setup_forge() -> (PullRequestService, Arc<ScriptedForge>, Arc<Source>) {
    let forge = Arc::new(ScriptedForge::new());
    let source = Arc::new(Source(Mutex::new(vec![Binding {
        project: project(),
        host: Arc::new(ForgeHost::new(forge.clone())),
    }])));
    let clock = Arc::new(TestClock::default());
    let cache = Arc::new(PullRequestReadCache::new(
        Arc::new(MemoryStore::default()),
        clock.clone(),
    ));
    (
        PullRequestService::new(source.clone(), clock, cache),
        forge,
        source,
    )
}

#[tokio::test]
async fn forge_bridge_receives_authoritative_context_and_unmodified_comment_data() {
    let (service, forge, _) = setup_forge();
    assert!(service.batches_reads(&reference()).await);
    let summary = service
        .summary(reference(), Default::default())
        .await
        .expect("summary");
    assert_eq!(summary.repository.as_str(), "owner/repo");
    assert_eq!(summary.closed_at, Some(None));
    assert_eq!(
        forge.requests.lock().expect("requests")[0]["cwd"],
        "C:/sandbox"
    );
    let body = "'; $(cmd)\n--flag";
    service
        .comment(decode(input(json!({"body":body}))))
        .await
        .expect("comment");
    assert_eq!(forge.requests.lock().expect("requests")[1]["body"], body);
}

#[tokio::test]
async fn host_search_batches_repositories_and_restores_each_project_context() {
    let (service, forge, source) = setup_forge();
    let mut second = project();
    second.id = t3_contracts::base_schemas::ProjectId("p2".into());
    second.repository = "owner/second".into();
    second.canonical_repository_key = Some("github.com/owner/second".into());
    source.0.lock().expect("source").push(Binding {
        project: second,
        host: Arc::new(ForgeHost::new(forge.clone())),
    });
    let result = service
        .list(decode(json!({"state":"open"})))
        .await
        .expect("list");
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[1].project_id.0, "p2");
    let requests = forge.requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["repositories"]
            .as_array()
            .expect("repositories")
            .len(),
        2
    );
}

#[tokio::test]
async fn file_membership_falls_back_to_paged_patch_when_driver_has_no_path_operation() {
    let (service, forge, _) = setup_forge();
    let input = decode(input(
        json!({"changeType":"change","oldPath":"a.rs","newPath":"a.rs"}),
    ));
    assert_eq!(
        service
            .diff_file_contents(input)
            .await
            .expect("contents")
            .new_contents,
        "new"
    );
    assert_eq!(forge.requests.lock().expect("requests")[0]["path"], "a.rs");
}

#[test]
fn diff_membership_includes_binary_files_and_pure_renames_without_side_headers() {
    let patch = "diff --git a/image.png b/image.png\nindex aaaa..bbbb 100644\nBinary files a/image.png and b/image.png differ\ndiff --git a/old name b/new name\nsimilarity index 100%\nrename from old name\nrename to new name\n";
    assert_eq!(
        parse_diff_paths(patch),
        [
            ("image.png".into(), "image.png".into()),
            ("old name".into(), "new name".into())
        ]
    );
}

#[test]
fn forge_error_mapping_preserves_classification_retry_and_removes_url_secrets() {
    let error = ForgeError::Provider {
        provider: SourceControlProviderKind("github".into()),
        operation: "summary".into(),
        reason: t3_forge::FailureReason::RateLimited,
        detail: "failed https://user:secret@example.com/x?token=hidden".into(),
        retry_at: Some(99),
        access_hint: None,
        status: Some(429),
        diagnostics: Box::default(),
    };
    let error = t3_pull_request::forge::map_error(error);
    assert_eq!(error.retry_at, Some(99));
    assert_eq!(error.reason, t3_pull_request::FailureReason::RateLimited);
    assert!(!error.detail.contains("secret"));
    assert!(!error.detail.contains("hidden"));
}

#[tokio::test]
async fn unavailable_errors_keep_the_provider_and_its_actionable_requirement() {
    for (reason, requirement) in [
        (
            t3_forge::FailureReason::MissingTool,
            "Install the test forge tool.",
        ),
        (
            t3_forge::FailureReason::Unauthenticated,
            "Authenticate the test forge account.",
        ),
    ] {
        let (service, forge, _) = setup_forge();
        *forge.failure.lock().expect("failure") = Some(reason);
        let error = service
            .summary(reference(), Default::default())
            .await
            .expect_err("unavailable")
            .unavailable_error()
            .expect("wire error");
        assert_eq!(error.provider.flatten().expect("provider").0, "github");
        assert_eq!(
            error.requirement.flatten().expect("guidance").as_str(),
            requirement
        );
        service.shutdown().await;
    }
}

#[tokio::test]
async fn same_host_cross_repository_reads_use_that_hosts_binding_and_unbound_private_hosts_are_refused()
 {
    let (service, forge, _) = setup_forge();
    let mut reference = reference();
    reference.repository = "owner/another".into();
    service
        .summary(reference.clone(), Default::default())
        .await
        .expect("same-host cross-repository read");
    reference.host = Some(Some("github.attacker.invalid".into()));
    assert!(
        service
            .summary(reference, Default::default())
            .await
            .is_err()
    );
    assert_eq!(forge.requests.lock().expect("requests").len(), 1);
    service.shutdown().await;
}
