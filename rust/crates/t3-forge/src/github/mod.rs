mod adapter;
pub mod app;
mod batch;
mod checks_cache;
pub mod cli;
pub mod decode;
pub mod discovery;
mod events;
mod media;
pub mod merge_message;
mod queries;
mod repository;
pub mod session_store;
pub mod stack;
pub mod transport;
mod workflows;
use crate::{Result, model::*};
use std::sync::Arc;
type KeyedGates = Arc<
    tokio::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
>;

#[derive(Clone)]
pub struct GitHub {
    pub(crate) http: Arc<transport::GitHubHttp>,
    capabilities: PullRequestCapabilities,
    pub(crate) git_environment: t3_process::Environment,
    pub(crate) git_driver: t3_git::Git,
    default_host: String,
    repository_override: Option<String>,
    pub(crate) app_sign_in: Option<Arc<app::AppSignIn>>,
    coverage: Arc<tokio::sync::Mutex<std::collections::HashMap<String, app::Coverage>>>,
    coverage_gates: KeyedGates,
    signed_urls: Arc<tokio::sync::Mutex<std::collections::HashMap<String, media::SignedUrl>>>,
    event_pauses: Arc<tokio::sync::Mutex<std::collections::HashMap<String, u64>>>,
    check_entries: checks_cache::Entries,
    routing_identities: Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, (u64, crate::forge::RoutingIdentity)>>,
    >,
    routing_identity_gates: KeyedGates,
    node_ids: Arc<tokio::sync::Mutex<std::collections::HashMap<String, (u64, String)>>>,
    search_coverage: Arc<tokio::sync::Mutex<std::collections::HashMap<String, (u64, bool)>>>,
    cli: Arc<dyn cli::GitHubCli>,
    pub(crate) event_stop: t3_process::CancellationToken,
    pub(crate) event_tasks: tokio_util::task::TaskTracker,
}
impl GitHub {
    pub fn with_cli(mut self, cli: Arc<dyn cli::GitHubCli>) -> Self {
        self.cli = cli;
        self
    }
    pub fn with_git_environment(mut self, environment: t3_process::Environment) -> Self {
        self.git_environment = environment;
        self
    }
    pub fn with_repository_environment(
        mut self,
        host: Option<String>,
        repository: Option<String>,
    ) -> Self {
        self.default_host = host
            .unwrap_or_else(|| "github.com".into())
            .trim()
            .to_lowercase();
        self.repository_override = repository;
        self
    }
    pub fn new(http: transport::GitHubHttp) -> Result<Self> {
        let capabilities = serde_json::from_value(serde_json::json!({
            "diff":true,"comment":true,"stacks":true,"stackActions":true,
            "actions":["merge","ready","draft","close","reopen","delete-branch","update-branch","enable-auto-merge","disable-auto-merge","revert","approve-workflows"],
            "mergeMethods":["merge","squash","rebase"],"deleteBranch":true,"updateMethods":["merge","rebase"],"search":true,"reactions":true,"mentions":true,"referenceSigils":["#"],"viewedFiles":"host",
            "review":{"inlineComment":true,"rangeComment":true,"reply":true,"resolve":true,"verdicts":["comment","approve","request-changes"]},
            "reviewers":{"request":true,"listCandidates":true,"searchCandidates":true},"edit":{"changeRequest":true,"comment":true,"maintainerEdits":true},"labels":true,"checksAlwaysReport":true
        })).map_err(|_| transport::error("capabilities", crate::FailureReason::Failed, "Invalid capabilities."))?;
        Ok(Self {
            http: Arc::new(http),
            capabilities,
            git_environment: Default::default(),
            git_driver: t3_git::Git::new("."),
            default_host: "github.com".into(),
            repository_override: None,
            app_sign_in: None,
            coverage: Default::default(),
            coverage_gates: Default::default(),
            signed_urls: Default::default(),
            event_pauses: Default::default(),
            check_entries: Default::default(),
            routing_identities: Default::default(),
            routing_identity_gates: Default::default(),
            node_ids: Default::default(),
            search_coverage: Default::default(),
            cli: Arc::new(cli::ProcessGitHubCli),
            event_stop: t3_process::CancellationToken::new(),
            event_tasks: {
                let tasks = tokio_util::task::TaskTracker::new();
                tasks.close();
                tasks
            },
        })
    }
}
