//! One case per `context::resolve` routing branch (`requireProject`, `PullRequestService.ts`).
mod support;
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, atomic::Ordering};
use support::*;
use t3_contracts::{base_schemas::ProjectId, pull_request::*};
use t3_pull_request::{
    FailureReason, PullRequestService, Result,
    cache::{MemoryStore, PullRequestReadCache},
    context::{Binding, Project, ProjectSource, SelectedProject},
    host::Host,
};

/// A forge that serves only its own private host and never reads public github.com.
struct PrivateHost(PullRequestCapabilities);
#[async_trait]
impl Host for PrivateHost {
    fn kind(&self) -> &str {
        "gitlab"
    }
    fn capabilities(&self) -> &PullRequestCapabilities {
        &self.0
    }
}

struct Routed {
    bindings: Vec<Binding>,
    /// Authoritative projects with no binding (no recognised remote).
    unbound: Vec<SelectedProject>,
    public: Option<Arc<dyn Host>>,
}
#[async_trait]
impl ProjectSource for Routed {
    async fn projects(&self) -> Result<Vec<Binding>> {
        Ok(self.bindings.clone())
    }
    async fn selected_project(&self, id: &ProjectId) -> Result<Option<SelectedProject>> {
        let bound = self.bindings.iter().find(|b| b.project.id == *id);
        Ok(bound
            .map(|b| SelectedProject::of(&b.project))
            .or_else(|| self.unbound.iter().find(|p| p.id == *id).cloned()))
    }
    fn public_host(&self) -> Option<Arc<dyn Host>> {
        self.public.clone()
    }
}

fn service(source: Routed) -> PullRequestService {
    let clock = Arc::new(TestClock::default());
    let cache = Arc::new(PullRequestReadCache::new(
        Arc::new(MemoryStore::default()),
        clock.clone(),
    ));
    PullRequestService::new(Arc::new(source), clock, cache)
}
fn gitlab_project() -> Project {
    decode(
        json!({"id":"g","title":"GitLab project","workspace_root":"C:/gitlab","host":"gitlab.com","repository":"team/app","canonical_repository_key":"gitlab.com/team/app","remove_agent_credits_on_merge":false}),
    )
}
fn scratch() -> SelectedProject {
    SelectedProject {
        id: decode(json!("s")),
        title: decode(json!("No project")),
        workspace_root: "C:/scratch".into(),
        remove_agent_credits_on_merge: false,
    }
}
fn public_reference(project: &str) -> PullRequestRef {
    decode(json!({"projectId":project,"host":"github.com","repository":"org/lib","number":5}))
}

#[tokio::test]
async fn same_host_foreign_repository_routes_through_that_hosts_checkout() {
    let host = Arc::new(FakeHost::default());
    let mut project = project();
    project.canonical_repository_key = None;
    let service = service(Routed {
        bindings: vec![Binding {
            project,
            host: host.clone(),
        }],
        unbound: Vec::new(),
        public: None,
    });
    let mut reference = reference();
    reference.repository = "owner/another".into();
    let ctx = service.context(&reference).await.expect("same-host route");
    assert_eq!(ctx.project.id.0.as_str(), "p");
    assert_eq!(ctx.project.repository, "owner/another");
    assert_eq!(ctx.project.workspace_root, "C:/sandbox");
}

#[tokio::test]
async fn public_github_reads_from_a_project_on_another_forge_use_the_public_host() {
    let public = Arc::new(FakeHost::default());
    let service = service(Routed {
        bindings: vec![Binding {
            project: gitlab_project(),
            host: Arc::new(PrivateHost(capabilities())),
        }],
        unbound: Vec::new(),
        public: Some(public.clone()),
    });
    let ctx = service
        .context(&public_reference("g"))
        .await
        .expect("public github.com read from a GitLab project");
    assert_eq!(ctx.host.kind(), "github");
    assert_eq!(ctx.project.id.0.as_str(), "g");
    assert_eq!(ctx.project.host, "github.com");
    assert_eq!(ctx.project.repository, "org/lib");
    assert_eq!(ctx.project.canonical_repository_key, None);
    service
        .summary(public_reference("g"), Default::default())
        .await
        .expect("summary through the public host");
    assert_eq!(public.reads.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn public_github_reads_from_a_project_with_no_remote_use_the_public_host() {
    let service = service(Routed {
        bindings: Vec::new(),
        unbound: vec![scratch()],
        public: Some(Arc::new(FakeHost::default())),
    });
    let ctx = service
        .context(&public_reference("s"))
        .await
        .expect("public read from the scratch project");
    assert_eq!(ctx.project.workspace_root, "C:/scratch");
    assert_eq!(ctx.project.title.as_str(), "No project");
}

#[tokio::test]
async fn public_route_needs_an_authoritative_selected_project_and_a_public_host() {
    let public = Arc::new(FakeHost::default());
    let with_public = service(Routed {
        bindings: Vec::new(),
        unbound: vec![scratch()],
        public: Some(public.clone()),
    });
    // A project id the environment does not know is refused, public host or not.
    let unknown = with_public.context(&public_reference("ghost")).await;
    assert_eq!(
        unknown.err().map(|e| e.reason),
        Some(FailureReason::Unsupported)
    );
    // A recognised private host with no binding is refused, not read publicly.
    let mut private = public_reference("s");
    private.host = Some(Some("github.attacker.invalid".into()));
    let refused = with_public.context(&private).await;
    assert_eq!(
        refused.err().map(|e| e.reason),
        Some(FailureReason::Unsupported)
    );
    assert_eq!(public.reads.load(Ordering::Acquire), 0);
    // Without a public host, a project on another forge cannot read github.com.
    let without_public = service(Routed {
        bindings: vec![Binding {
            project: gitlab_project(),
            host: Arc::new(PrivateHost(capabilities())),
        }],
        unbound: Vec::new(),
        public: None,
    });
    let refused = without_public.context(&public_reference("g")).await;
    assert_eq!(
        refused.err().map(|e| e.reason),
        Some(FailureReason::Unsupported)
    );
}

#[tokio::test]
async fn malformed_host_data_fails_the_request_instead_of_panicking() {
    let mut project = project();
    project.host = String::new();
    project.canonical_repository_key = None;
    let service = service(Routed {
        bindings: vec![Binding {
            project,
            host: Arc::new(FakeHost::default()),
        }],
        unbound: Vec::new(),
        public: None,
    });
    let mut reference = reference();
    reference.host = None;
    let ctx = service.context(&reference).await.expect("own route");
    assert!(ctx.key().is_err());
    let report: PullRequestReportStateInput =
        decode(json!({"reference": reference, "state": "open"}));
    let error = service
        .report_state(report)
        .await
        .expect_err("refused, not a panic");
    assert_eq!(error.operation, "resolveRepository");
}

#[tokio::test]
async fn reported_states_are_capped_and_the_oldest_report_is_evicted() {
    let (service, _, _, _) = setup();
    let report = |number: u64| -> PullRequestReportStateInput {
        let mut reference = reference();
        reference.number = number;
        decode(json!({"reference": reference, "state": "open"}))
    };
    for number in 1..=2_049 {
        service.report_state(report(number)).await.expect("report");
    }
    let mut changes = service.subscribe_state_changes();
    // With TypeScript's 2048-entry cap the first report was evicted, so it publishes again.
    service.report_state(report(1)).await.expect("again");
    assert_eq!(changes.try_recv().expect("published").number, 1);
    // The newest one is still remembered and deduplicated.
    service.report_state(report(2_049)).await.expect("dup");
    assert!(changes.try_recv().is_err());
}
