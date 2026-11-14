//! The environment's forge registry and the source-control services over it: TypeScript's
//! `SourceControlProviderRegistry` over `builtInDrivers.ts`, with `SourceControlRepositoryService`,
//! `SourceControlSignIn` and `SourceControlDiscovery`. Built once at start-up; every consumer
//! (source control, project clone, pull requests, the git PR actions) derives from
//! [`ForgeAdapter::registry`] and never builds a second adapter set.
//!
//! A client never chooses where a credential goes. GitLab sends glab only to its default host and
//! the server's `GITLAB_HOST` (or an authorised checkout's remote). GitHub's environment token goes
//! only to configured hosts, whatever named the host ([`ConfiguredHostCredentials`]), and requests
//! a client addresses by host (app access, stars) go only to configured hosts. Paths a client names
//! are authorised here before the forge sees them: a cwd through the workspace authority, a clone
//! destination by [`ForgeAdapter::authorize_destination`].
use crate::adapters::git::{Error as GitAdapterError, GitAdapter};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};
use t3_contracts::{
    auth::{AuthEnvironmentScope, EnvironmentAuthorizationError, EnvironmentAuthorizationErrorTag},
    base_schemas::{AuthSessionId, TrimmedNonEmptyString as Text},
    git::{
        GitManagerError, GitManagerErrorTag, GitManagerServiceError, GitPullRequestRefInput,
        GitResolvePullRequestResult,
    },
    settings::{ServerSettings, SourceControlHostSettings},
    source_control::*,
};
use t3_forge::{
    FailureReason, Forge, ForgeError,
    azure_devops::AzureDevOps,
    bitbucket::Bitbucket,
    cursor_origin::{CursorOrigin, CursorOriginConfig},
    discovery::SourceControlDiscovery,
    forgejo::{
        Forgejo,
        transport::{ForgejoConfig, keys_paths},
    },
    gitcafe::{
        GitCafe,
        credentials::{GitCafeCredentials, GitCafeSettings},
    },
    github::{
        GitHub,
        app::AppSignIn,
        session_store::SettingsAppSessions,
        transport::{Credential, CredentialSettings, GitHubCredentials, GitHubHttp, valid_host},
    },
    gitlab::GitLab,
    http::{ApiCredential, ApiHttp},
    rate_limit::RateLimits,
    review_host::{ForgeReviewHost, ReviewSetup},
    services::{CloneOptions, SourceControlRepositoryService, SourceControlSignIn},
};

use t3_workspace::paths::{validate_host_path, within};

pub use t3_forge::registry::ForgeRegistry;

/// Why a source-control call was refused, in the contract's failure types.
#[derive(Debug)]
pub enum Refusal {
    Authorization(Box<EnvironmentAuthorizationError>),
    Repository(Box<SourceControlRepositoryError>),
}

pub struct ForgeAdapter {
    registry: Arc<ForgeRegistry>,
    repositories: SourceControlRepositoryService,
    sign_in: SourceControlSignIn,
    discovery: SourceControlDiscovery,
    /// The last discovery and when it ran; its lock makes concurrent calls share one run.
    discovered: Arc<tokio::sync::Mutex<Option<(std::time::Instant, SourceControlDiscoveryResult)>>>,
    /// The environment's shared Git handle; its caches are the ones a clone must invalidate.
    git: t3_git::Git,
    /// The workflow every per-checkout workflow is a clone of, so they share Git and request
    /// caches across calls.
    workflows: t3_git::GitWorkflowService,
    workspaces: Arc<GitAdapter>,
    cwd: PathBuf,
    home: PathBuf,
    destinations: Destinations,
    github_hosts: Arc<RwLock<BTreeSet<String>>>,
    credentials: Credentials,
    stop: tokio_util::sync::CancellationToken,
    follower: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// The live credential handles settings changes update.
struct Credentials {
    github: Arc<GitHubCredentials>,
    bitbucket: ApiHttp,
    gitcafe: Arc<GitCafeCredentials>,
    environment: BTreeMap<String, String>,
    patch: t3_process::Environment,
    #[cfg(feature = "providers")]
    resolver: Option<Arc<t3_provider::secrets::SecretResolver>>,
}

/// Process environment variables are case-insensitive on Windows.
fn variable<'a>(environment: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    environment
        .iter()
        .find(|(name, _)| {
            if cfg!(windows) {
                name.eq_ignore_ascii_case(key)
            } else {
                name.as_str() == key
            }
        })
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn service(error: ForgeError) -> crate::Error {
    crate::Error::Service(format!("forge registry: {error}"))
}

/// GitHub hosts a client may address by name: github.com, `GH_HOST`, and hosts the settings hold
/// a token, account or app sign-in for.
fn github_hosts(
    raw: Option<&SourceControlHostSettings>,
    environment: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut hosts = BTreeSet::from(["github.com".to_owned()]);
    hosts.extend(variable(environment, "GH_HOST").map(str::to_ascii_lowercase));
    for field in ["tokens", "accounts", "signIns"] {
        if let Some(entries) = raw.and_then(|raw| raw.get(field)?.as_object()) {
            hosts.extend(entries.keys().map(|host| host.trim().to_ascii_lowercase()));
        }
    }
    hosts.retain(|host| valid_host(host));
    hosts
}

impl Credentials {
    /// `resolveSourceControlHostSecrets`: secret references in source-control settings become
    /// literals; one that cannot be read stays a reference, which the forge settings refuse.
    async fn resolve(
        &self,
        settings: &ServerSettings,
    ) -> BTreeMap<String, SourceControlHostSettings> {
        let mut hosts = settings.source_control_hosts.clone();
        #[cfg(feature = "providers")]
        if let Some(resolver) = &self.resolver {
            let mut slots = Vec::new();
            for (kind, fields) in &hosts {
                for (field, value) in fields {
                    if let Some(found) = reference(value) {
                        slots.push((kind.clone(), field.clone(), None, found));
                    } else if let Some(entries) = value.as_object() {
                        for (host, entry) in entries {
                            if let Some(found) = reference(entry) {
                                slots.push((
                                    kind.clone(),
                                    field.clone(),
                                    Some(host.clone()),
                                    found,
                                ));
                            }
                        }
                    }
                }
            }
            if !slots.is_empty() {
                let references: Vec<_> = slots.iter().map(|slot| slot.3.clone()).collect();
                let answers = resolver
                    .read(&settings.secret_providers, &references, false)
                    .await;
                for ((kind, field, host, _), answer) in slots.into_iter().zip(answers) {
                    let Some(secret) = answer.filter(|secret| !secret.trim().is_empty()) else {
                        continue;
                    };
                    let Some(value) = hosts
                        .get_mut(&kind)
                        .and_then(|fields| fields.get_mut(&field))
                    else {
                        continue;
                    };
                    match host {
                        None => *value = serde_json::Value::String(secret),
                        Some(host) => {
                            if let Some(entry) = value.get_mut(&host) {
                                *entry = serde_json::Value::String(secret);
                            }
                        }
                    }
                }
            }
        }
        hosts
    }

    /// A GitHub setting that cannot be resolved disables GitHub credentials for every host the
    /// server knows, rather than falling back to another account.
    fn github(&self, raw: &SourceControlHostSettings) -> CredentialSettings {
        t3_forge::settings::github(raw, &self.patch).unwrap_or_else(|_| {
            tracing::warn!(
                "a GitHub credential in settings cannot be resolved; GitHub credentials are off until it is fixed"
            );
            CredentialSettings {
                tokens: BTreeMap::new(),
                accounts: BTreeMap::new(),
                disabled_hosts: github_hosts(Some(raw), &self.environment)
                    .into_iter()
                    .collect::<HashSet<_>>(),
                environment: BTreeMap::new(),
            }
        })
    }

    /// Bitbucket without a usable credential is anonymous, as upstream.
    fn bitbucket(&self, raw: &SourceControlHostSettings) -> ApiCredential {
        match t3_forge::settings::bitbucket(raw, &self.patch) {
            Ok(credential) => credential.unwrap_or(ApiCredential::Anonymous),
            Err(_) => {
                tracing::warn!(
                    "a Bitbucket credential in settings cannot be resolved; Bitbucket is anonymous until it is fixed"
                );
                ApiCredential::Anonymous
            }
        }
    }

    fn gitcafe(&self, raw: &SourceControlHostSettings) -> GitCafeSettings {
        t3_forge::settings::gitcafe(raw, self.patch.clone()).unwrap_or_else(|_| {
            tracing::warn!(
                "a GitCafe credential in settings cannot be resolved; GitCafe has no token until it is fixed"
            );
            GitCafeSettings {
                token: None,
                environment: Default::default(),
            }
        })
    }
}

/// A settings value that is a secret provider reference.
#[cfg(feature = "providers")]
fn reference(value: &serde_json::Value) -> Option<t3_contracts::secret_reference::SecretReference> {
    if value.is_object() {
        serde_json::from_value(value.clone()).ok()
    } else {
        None
    }
}

fn host_settings<'a>(
    hosts: &'a BTreeMap<String, SourceControlHostSettings>,
    kind: &str,
) -> &'a SourceControlHostSettings {
    static EMPTY: std::sync::LazyLock<SourceControlHostSettings> =
        std::sync::LazyLock::new(SourceControlHostSettings::new);
    hosts.get(kind).unwrap_or(&EMPTY)
}

/// Project setup in a worktree belongs to the setup script runner, which is not ported; PR
/// resolution never reaches it, and preparing a PR thread is not served without it.
struct NoProjectSetup;
#[async_trait::async_trait]
impl ReviewSetup for NoProjectSetup {
    async fn setup(&self, _: &Path, _: &str, _: &Path) -> Result<(), t3_git::GitError> {
        Err(t3_git::GitError::Unsupported(
            "project setup is not available on this server".into(),
        ))
    }
}

impl ForgeAdapter {
    /// After the providers (their secret resolver reads settings references) and Git (the
    /// workspace authority and the shared Git handle); `None` without Git.
    pub async fn start(
        context: &crate::services::DomainContext<'_>,
        domains: &crate::services::Domains,
    ) -> crate::Result<Option<Arc<Self>>> {
        let Some(workspaces) = domains.get::<GitAdapter>().filter(|git| git.is_available()) else {
            return Ok(None);
        };
        #[cfg(feature = "providers")]
        let resolver = domains.get::<t3_provider::secrets::SecretResolver>();
        let config = &context.local.config;
        let settings = context.local.settings.clone();
        let environment = context.environment.clone();
        let patch: t3_process::Environment = environment
            .iter()
            .map(|(key, value)| (key.into(), Some(value.into())))
            .collect();
        let cwd = config.cwd.clone();
        let home = context.account_home.clone();
        let git = workspaces.at(&cwd);
        // Subscribed before the first snapshot, so no change between them is lost.
        let changes = settings.subscribe_changes();
        let snapshot = settings
            .snapshot()
            .await
            .map_err(|e| crate::Error::Service(format!("forge settings: {e}")))?;
        let limits = Arc::new(RateLimits::default());
        let probe = Credentials {
            github: Arc::new(GitHubCredentials::new(CredentialSettings {
                tokens: BTreeMap::new(),
                accounts: BTreeMap::new(),
                disabled_hosts: HashSet::new(),
                environment: BTreeMap::new(),
            })),
            bitbucket: ApiHttp::new(
                "bitbucket",
                "https://api.bitbucket.org/2.0"
                    .parse()
                    .map_err(|_| crate::Error::Service("forge: Bitbucket API origin".into()))?,
                ApiCredential::Anonymous,
                limits.clone(),
            )
            .map_err(service)?,
            gitcafe: Arc::new(GitCafeCredentials::new(
                cwd.clone(),
                GitCafeSettings {
                    token: None,
                    environment: Default::default(),
                },
            )),
            environment: environment.clone(),
            patch: patch.clone(),
            #[cfg(feature = "providers")]
            resolver,
        };
        let hosts = probe.resolve(&snapshot).await;
        probe
            .github
            .update_settings(probe.github(host_settings(&hosts, "github")))
            .await;
        probe
            .bitbucket
            .update_credential(probe.bitbucket(host_settings(&hosts, "bitbucket")))
            .map_err(service)?;
        probe
            .gitcafe
            .update_settings(probe.gitcafe(host_settings(&hosts, "gitcafe")))
            .await;
        let credentials = probe;
        let github_hosts = Arc::new(RwLock::new(github_hosts(hosts.get("github"), &environment)));

        let sessions = Arc::new(SettingsAppSessions::new(
            settings.clone(),
            t3_config::secrets::SecretStore::new(config.paths.secrets()),
        ));
        // An unset client id turns the GitHub app sign-in off (`CLIENT_ID_VARIABLE`).
        let client_id = variable(&environment, "T3CODE_GITHUB_APP_CLIENT_ID")
            .unwrap_or_default()
            .to_owned();
        let mut app = AppSignIn::new(client_id, sessions).map_err(service)?;
        let mut http = GitHubHttp::new(
            Arc::new(ConfiguredHostCredentials {
                inner: credentials.github.clone(),
                hosts: github_hosts.clone(),
            }),
            limits.clone(),
        )
        .map_err(service)?;
        // Debug builds only: a loopback stand-in for github.com, for recorded-response tests.
        #[cfg(debug_assertions)]
        if let Some(endpoint) = variable(&environment, "T3CODE_DEV_GITHUB_ENDPOINT") {
            app = app.with_loopback_endpoint(endpoint).map_err(service)?;
            http = http.with_loopback_endpoint(endpoint).map_err(service)?;
        }
        let app = Arc::new(app);
        let mut github = GitHub::new(http)
            .map_err(service)?
            .with_git_environment(patch.clone())
            .with_repository_environment(
                variable(&environment, "GH_HOST").map(str::to_owned),
                variable(&environment, "GH_REPO").map(str::to_owned),
            );
        if app.enabled() {
            github = github.with_app_sign_in(app);
        }
        let forgejo = ForgejoConfig {
            authentication_cwd: cwd.clone(),
            keys_paths: keys_paths(
                &home,
                variable(&environment, "XDG_DATA_HOME").map(Path::new),
                variable(&environment, "APPDATA").map(Path::new),
            ),
            environment: patch.clone(),
        };
        let forges: Vec<Arc<dyn Forge>> = vec![
            Arc::new(github),
            Arc::new(
                GitLab::new(git.clone())
                    .map_err(service)?
                    .with_environment(patch.clone()),
            ),
            Arc::new(
                AzureDevOps::new()
                    .map_err(service)?
                    .with_environment(patch.clone()),
            ),
            Arc::new(Bitbucket::new(credentials.bitbucket.clone(), git.clone()).map_err(service)?),
            Arc::new(
                GitCafe::new(git.clone(), credentials.gitcafe.clone(), limits).map_err(service)?,
            ),
            Arc::new(Forgejo::new(git.clone(), forgejo).map_err(service)?),
            Arc::new(
                CursorOrigin::new(
                    git.clone(),
                    CursorOriginConfig {
                        authentication_cwd: cwd.clone(),
                        environment: patch.clone(),
                    },
                )
                .map_err(service)?,
            ),
        ];
        let registry = Arc::new(ForgeRegistry::new(forges).with_git_environment(patch.clone()));
        let paths = &config.paths;
        let adapter = Arc::new(Self {
            repositories: SourceControlRepositoryService::new(
                registry.clone(),
                git.clone(),
                cwd.clone(),
            )
            .with_home(home.clone()),
            sign_in: SourceControlSignIn::new(registry.clone()),
            discovery: SourceControlDiscovery::new(registry.clone(), cwd.clone())
                .with_environment(patch),
            discovered: Default::default(),
            registry,
            workflows: t3_git::GitWorkflowService::new(git.clone()),
            git,
            workspaces,
            destinations: Destinations::new(
                paths,
                &home,
                &environment,
                domains.get::<crate::workspaces::EnvironmentWorkspaces>(),
            ),
            cwd,
            home,
            github_hosts,
            credentials,
            stop: context.shutdown.child_token(),
            follower: Default::default(),
        });
        // Status pull-request enrichment for the VCS group (`git.rs`, `vcs`).
        if let Some(vcs) = adapter.workspaces.vcs() {
            let registry = adapter.registry.clone();
            vcs.bind_workflow_factory(|template| {
                Arc::new(ForgeWorkflows {
                    registry,
                    template,
                    lenient_lookups: true,
                })
            });
        }
        let follower = adapter.clone().follow_settings(changes);
        *adapter
            .follower
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(follower);
        Ok(Some(adapter))
    }

    /// The one forge registry of this environment.
    pub fn registry(&self) -> Arc<ForgeRegistry> {
        self.registry.clone()
    }

    /// Settings changes update the live credential handles and the configured hosts, and drop
    /// cached provider detection (TypeScript reads settings on each request).
    fn follow_settings(
        self: Arc<Self>,
        mut changes: tokio::sync::mpsc::UnboundedReceiver<ServerSettings>,
    ) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(&self);
        let stop = self.stop.clone();
        drop(self);
        tokio::spawn(async move {
            loop {
                let settings = tokio::select! {
                    () = stop.cancelled() => break,
                    changed = changes.recv() => match changed {
                        Some(settings) => settings,
                        None => break,
                    },
                };
                let Some(adapter) = weak.upgrade() else { break };
                adapter.apply_settings(&settings).await;
            }
        })
    }

    async fn apply_settings(&self, settings: &ServerSettings) {
        let credentials = &self.credentials;
        let hosts = credentials.resolve(settings).await;
        credentials
            .github
            .update_settings(credentials.github(host_settings(&hosts, "github")))
            .await;
        if let Err(error) = credentials
            .bitbucket
            .update_credential(credentials.bitbucket(host_settings(&hosts, "bitbucket")))
        {
            tracing::warn!(%error, "cannot apply the Bitbucket credential");
        }
        credentials
            .gitcafe
            .update_settings(credentials.gitcafe(host_settings(&hosts, "gitcafe")))
            .await;
        *self
            .github_hosts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            github_hosts(hosts.get("github"), &credentials.environment);
        self.registry.invalidate_detection();
    }

    fn configured_host(&self, provider: &SourceControlProviderKind, host: &str) -> bool {
        provider.0 != "github"
            || self
                .github_hosts
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&host.trim().to_ascii_lowercase())
    }

    /// `sourceControl.lookupRepository`; a cwd, when given, must be an authorised workspace.
    pub async fn lookup_repository(
        &self,
        session: &AuthSessionId,
        mut input: SourceControlRepositoryLookupInput,
    ) -> Result<SourceControlRepositoryInfo, Refusal> {
        if let Some(Some(cwd)) = &input.cwd {
            let (cwd, _) = self
                .workspaces
                .directory(session, &cwd.0)
                .await
                .map_err(|_| denied("The requested directory is outside this session's authorized project workspaces."))?;
            input.cwd = Some(Some(text(&cwd)));
        }
        self.repositories
            .lookup_repository(input)
            .await
            .map_err(|error| Refusal::Repository(error.wire))
    }

    /// `sourceControl.cloneRepository` into an authorised destination.
    pub async fn clone_repository(
        &self,
        mut input: SourceControlCloneRepositoryInput,
    ) -> Result<SourceControlCloneRepositoryResult, Refusal> {
        let provider = input.provider.clone().flatten();
        let destination = self
            .authorize_destination(provider.as_ref(), &input.destination_path.0)
            .await?;
        input.destination_path = text(&destination.path);
        match self
            .repositories
            .clone_repository(input, CloneOptions::default())
            .await
        {
            Ok(cloned) => {
                self.git.at(&destination.path).invalidate_caches().await;
                Ok(cloned)
            }
            Err(error) => {
                destination.discard().await;
                Err(Refusal::Repository(error.wire))
            }
        }
    }

    /// `sourceControl.publishRepository` from an authorised checkout.
    pub async fn publish_repository(
        &self,
        session: &AuthSessionId,
        mut input: SourceControlPublishRepositoryInput,
    ) -> Result<SourceControlPublishRepositoryResult, Refusal> {
        let git = self
            .workspaces
            .checkout(session, &input.cwd.0, "sourceControl.publishRepository")
            .await
            .map_err(|error| match error {
                GitAdapterError::Authorization(error) => Refusal::Authorization(error),
                _ => Refusal::Repository(Box::new(SourceControlRepositoryError {
                    _tag: SourceControlRepositoryErrorTag::SourceControlRepositoryError,
                    provider: input.provider.clone(),
                    operation: "publishRepository".into(),
                    detail: "The source control operation could not be completed.".into(),
                    cause: None,
                })),
            })?;
        input.cwd = text(git.cwd());
        let cwd = git.cwd().to_owned();
        let published = self
            .repositories
            .publish_repository(input)
            .await
            .map_err(|error| Refusal::Repository(error.wire));
        // A failed push still leaves the new remote added.
        self.git.at(&cwd).invalidate_caches().await;
        published
    }

    pub async fn sign_in_start(
        &self,
        input: SourceControlSignInInput,
    ) -> Result<SourceControlSignInPrompt, Box<SourceControlSignInError>> {
        self.sign_in.start(input).await.map_err(|error| error.wire)
    }

    pub async fn sign_in_wait(
        &self,
        input: SourceControlSignInInput,
    ) -> Result<SourceControlSignInResult, Box<SourceControlSignInError>> {
        self.sign_in.wait(input).await.map_err(|error| error.wire)
    }

    pub async fn sign_in_cancel(
        &self,
        input: SourceControlSignInInput,
    ) -> Result<(), Box<SourceControlSignInError>> {
        self.sign_in.cancel(input).await.map_err(|error| error.wire)
    }

    /// `sourceControl.appAccess`; a host the server is not configured for is signed out.
    pub async fn app_access(&self, input: SourceControlAppAccessInput) -> SourceControlAppAccess {
        if !self.configured_host(&input.provider, &input.host.0) {
            return SourceControlAppAccess {
                status: SourceControlAppAccessStatus::SignedOut,
            };
        }
        self.sign_in.access(input).await
    }

    /// `sourceControl.starred`; false for a host the server is not configured for.
    pub async fn starred(&self, input: SourceControlStarredInput) -> bool {
        self.configured_host(&input.provider, &input.host.0) && self.sign_in.starred(input).await
    }

    /// `sourceControl.star`; false for a host the server is not configured for.
    pub async fn star(&self, input: SourceControlStarredInput) -> bool {
        self.configured_host(&input.provider, &input.host.0) && self.sign_in.star(input).await
    }

    /// `server.discoverSourceControl`; its only failure is an invalid probe result. The forge's
    /// discovery future is not provably `Send` (its buffered probe stream holds a higher-ranked
    /// closure), so it runs on a blocking thread through the runtime handle.
    /// One run at a time, its result reused briefly: each run starts about nine `--version`
    /// probes, and the run holds its lock until it finishes even if its caller has gone.
    pub async fn discover(self: Arc<Self>) -> Option<SourceControlDiscoveryResult> {
        const REUSE: std::time::Duration = std::time::Duration::from_secs(10);
        let mut last = self.discovered.clone().lock_owned().await;
        if let Some((at, result)) = last.as_ref()
            && at.elapsed() < REUSE
        {
            return Some(result.clone());
        }
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let found = handle.block_on(self.discovery.discover()).ok();
            if let Some(found) = &found {
                *last = Some((std::time::Instant::now(), found.clone()));
            }
            found
        })
        .await
        .ok()
        .flatten()
    }

    /// `git.resolvePullRequest` for an authorised checkout, through its own review host.
    pub async fn resolve_pull_request(
        &self,
        session: &AuthSessionId,
        input: GitPullRequestRefInput,
    ) -> Result<GitResolvePullRequestResult, PullRequestRefusal> {
        let git = self
            .workspaces
            .checkout(session, &input.cwd.0, "git.resolvePullRequest")
            .await
            .map_err(|error| match error {
                GitAdapterError::Authorization(error) => PullRequestRefusal::Authorization(error),
                _ => PullRequestRefusal::Git(manager_error(&input.cwd.0)),
            })?;
        self.workflow_factory(self.workflows.clone())
            .for_checkout(git.cwd())
            .await
            .map_err(|_| PullRequestRefusal::Git(manager_error(&input.cwd.0)))?
            .resolve_pull_request(&input.reference.0)
            .await
            .map_err(|error| {
                PullRequestRefusal::Git(Box::new(match error {
                    t3_git::GitError::Command(failure) => match failure.to_contract() {
                        Ok(command) => GitManagerServiceError::Command(command),
                        Err(_) => *manager_error(&input.cwd.0),
                    },
                    _ => *manager_error(&input.cwd.0),
                }))
            })
    }

    /// A `GitWorkflowFactory` giving each checkout a clone of `template` with that checkout's own
    /// forge review host, for the VCS status broadcaster and facade (`GitApplicationService`).
    pub fn workflow_factory(
        &self,
        template: t3_git::GitWorkflowService,
    ) -> Arc<dyn t3_git::GitWorkflowFactory> {
        Arc::new(ForgeWorkflows {
            registry: self.registry.clone(),
            template,
            lenient_lookups: false,
        })
    }

    /// Clone destinations. `~` and relative paths are read as the repository service reads them;
    /// the text must name a local disk path without device names, `:` or trailing dots or spaces
    /// (Windows would create another name). The nearest existing ancestor is resolved by the
    /// workspace resolver, which refuses remote drives and links to network paths before it
    /// follows them; then [`Destinations`] decides. The folder is created here and resolved again,
    /// so a link planted after the check (a concurrent clone can check one out) cannot redirect
    /// it; Git then clones into that existing empty folder.
    pub async fn authorize_destination(
        &self,
        provider: Option<&SourceControlProviderKind>,
        raw: &str,
    ) -> Result<Destination, Refusal> {
        let refused = || {
            Refusal::Repository(Box::new(SourceControlRepositoryError {
                _tag: SourceControlRepositoryErrorTag::SourceControlRepositoryError,
                provider: provider
                    .cloned()
                    .unwrap_or_else(|| SourceControlProviderKind("unknown".into())),
                operation: "cloneRepository".into(),
                detail: "This folder cannot be a clone destination. Choose a folder on a local disk, outside hidden or application data folders, existing projects, configuration and program folders, and T3 Code's own folders.".into(),
                cause: None,
            }))
        };
        let raw = raw.trim();
        let expanded = if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
            self.home.join(raw.get(2..).unwrap_or(""))
        } else {
            PathBuf::from(raw)
        };
        let joined = if expanded.is_absolute() {
            expanded
        } else {
            self.cwd.join(expanded)
        };
        let mut lexical = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    lexical.pop();
                }
                other => lexical.push(other),
            }
        }
        if !lexical.is_absolute()
            || lexical.file_name().is_none()
            || !local_disk(&lexical)
            || validate_host_path(&lexical).is_err()
        {
            return Err(refused());
        }
        let walked = tokio::task::spawn_blocking(move || {
            let mut existing = lexical;
            let mut missing = Vec::new();
            loop {
                match t3_workspace::paths::canonicalize(&existing) {
                    Ok(real) => return Some((real, missing)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        missing.push(existing.file_name()?.to_owned());
                        existing = existing.parent()?.to_path_buf();
                    }
                    Err(_) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten();
        let Some((real, missing)) = walked else {
            return Err(refused());
        };
        let mut resolved = plain(&real);
        resolved.extend(missing.iter().rev());
        if !local_disk(&resolved)
            || resolved.file_name().is_none()
            || self.destinations.refuses(&resolved).await
        {
            return Err(refused());
        }
        let target = resolved.clone();
        let created = tokio::task::spawn_blocking(move || create_verified(&target))
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(refused)?;
        Ok(Destination {
            path: resolved,
            created,
        })
    }

    /// The project clone backend over this registry and shared Git handle.
    #[cfg(feature = "project")]
    pub fn clone_backend(&self) -> Arc<dyn t3_project::clones::CloneRepository> {
        Arc::new(ProjectClones {
            inner: t3_project::clones::ForgeCloneRepository {
                registry: self.registry.clone(),
                git: self.git.clone(),
                cwd: self.cwd.clone(),
                home: self.home.clone(),
            },
            git: self.git.clone(),
        })
    }

    /// `projectClone.start` into an authorised destination; the tracker keeps the resolved path
    /// for retry and discard.
    #[cfg(feature = "project")]
    pub async fn start_project_clone(
        &self,
        projects: &t3_project::ProjectService,
        mut input: t3_contracts::project_clone::ProjectCloneStartInput,
    ) -> Result<t3_contracts::project_clone::ProjectCloneStartResult, ProjectCloneRefusal> {
        let requested = input.provider.clone().flatten();
        let destination = self
            .authorize_destination(requested.as_ref(), &input.destination_path.0)
            .await
            .map_err(ProjectCloneRefusal::Refused)?;
        input.destination_path = text(&destination.path);
        let provider = requested.unwrap_or_else(|| SourceControlProviderKind("unknown".into()));
        match projects.start_clone(input).await {
            Ok(started) => Ok(started),
            Err(error) => {
                destination.discard().await;
                Err(ProjectCloneRefusal::Project(provider, Box::new(error)))
            }
        }
    }
}

/// T3's own entries at the top of its base directory. The base may be a user's work folder, so
/// only these (and dot entries) are refused there; the managed `projects`, `source` and `scratch`
/// folders and any other folder take clones, and managed worktrees are workspaces.
const T3_BASE_ENTRIES: &[&str] = &[
    "userdata",
    "dev",
    "caches",
    "runtime",
    "tools",
    "service.env",
    "usage-host-id",
    "source-staging",
];

/// A destination the rules accepted, created by the server.
pub struct Destination {
    pub path: PathBuf,
    created: bool,
}
impl Destination {
    /// After a failed clone: removes the folder if this request created it and it is still empty.
    async fn discard(&self) {
        if self.created {
            let _ = tokio::fs::remove_dir(&self.path).await;
        }
    }
}

/// Folders a clone must never create, because something later runs or trusts what is in them.
/// A deny rule: TypeScript accepts any folder. Every path here is resolved; comparisons ignore
/// case on Windows.
struct Destinations {
    /// The T3 base: [`T3_BASE_ENTRIES`] and dot entries at its top are refused.
    base: PathBuf,
    /// The account home, `HOME` (Git for Windows reads it when it differs) and `USERPROFILE`: the
    /// first folder below each may not start with `.`, nor be `AppData` (Windows) or `Library`
    /// (macOS). Deeper dot folders are left to the workspace rule.
    homes: Vec<PathBuf>,
    /// The state directory and secrets, wherever they live.
    internal: Vec<PathBuf>,
    /// Refused themselves, inside, and as an ancestor the clone would create: PowerShell profile
    /// folders, Git configuration folders, application data, autostart and shell start-up
    /// folders, `~/bin` and `~/.local/bin`.
    trees: Vec<PathBuf>,
    /// Refused themselves and as an ancestor the clone would create: PATH folders (existing or
    /// not), the folders of `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM`, the homes.
    entries: Vec<PathBuf>,
    /// Any known workspace (project roots, thread worktrees, managed worktree folders): a clone
    /// into an existing project's tree would plant its `.claude`, `.git/hooks`, `.husky` or
    /// `.vscode`.
    workspaces: Option<Arc<crate::workspaces::EnvironmentWorkspaces>>,
}
impl Destinations {
    fn new(
        paths: &t3_config::home::HomeLayout,
        home: &Path,
        environment: &BTreeMap<String, String>,
        workspaces: Option<Arc<crate::workspaces::EnvironmentWorkspaces>>,
    ) -> Self {
        let absolute = |key: &str| {
            variable(environment, key)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let mut homes = vec![home.to_owned()];
        homes.extend(absolute("HOME"));
        homes.extend(absolute("USERPROFILE"));
        let mut trees: Vec<PathBuf> = absolute("XDG_CONFIG_HOME").into_iter().collect();
        let mut entries = homes.clone();
        for home in &homes {
            trees.push(home.join(".config").join("git"));
            if cfg!(windows) {
                let documents = home.join("Documents");
                trees.push(documents.join("PowerShell"));
                trees.push(documents.join("WindowsPowerShell"));
            } else {
                trees.push(home.join("bin"));
                trees.push(home.join(".local").join("bin"));
            }
        }
        #[cfg(windows)]
        {
            // The Documents known folder, which OneDrive or policy may redirect.
            if let Some(documents) = dirs::document_dir() {
                trees.push(documents.join("PowerShell"));
                trees.push(documents.join("WindowsPowerShell"));
            }
            trees.extend(absolute("APPDATA"));
            trees.extend(absolute("LOCALAPPDATA"));
            let program_data = absolute("PROGRAMDATA")
                .or_else(|| absolute("ALLUSERSPROFILE"))
                .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
            trees.push(program_data.join("Git"));
            trees.push(
                program_data
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu"),
            );
        }
        #[cfg(unix)]
        trees.extend(["/etc/profile.d", "/etc/xdg/autostart"].map(PathBuf::from));
        for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
            if let Some(file) = absolute(key) {
                entries.extend(file.parent().map(Path::to_path_buf));
            }
        }
        if let Some(path) = variable(environment, "PATH") {
            entries.extend(std::env::split_paths(path).filter(|entry| entry.is_absolute()));
        }
        let resolve = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
            paths.iter().map(|path| resolved_or_lexical(path)).collect()
        };
        Self {
            base: resolved_or_lexical(&paths.base_dir),
            homes: resolve(homes),
            internal: resolve(vec![paths.state_dir.clone(), paths.secrets()]),
            trees: resolve(trees),
            entries: resolve(entries),
            workspaces,
        }
    }

    /// `path` is resolved: its existing part canonical, the rest as the client wrote it.
    async fn refuses(&self, path: &Path) -> bool {
        if self.internal.iter().any(|internal| within(path, internal))
            || self
                .trees
                .iter()
                .any(|tree| within(path, tree) || within(tree, path))
            || self.entries.iter().any(|entry| within(entry, path))
        {
            return true;
        }
        let first_below = |root: &Path| {
            within(path, root).then(|| {
                path.components()
                    .nth(root.components().count())
                    .map(|first| first.as_os_str().to_string_lossy().into_owned())
            })
        };
        if let Some(first) = first_below(&self.base) {
            let Some(first) = first else {
                return true;
            };
            if first.starts_with('.')
                || T3_BASE_ENTRIES.iter().any(|entry| same_name(entry, &first))
            {
                return true;
            }
        } else if let Some(first) = self.homes.iter().find_map(|home| first_below(home)) {
            let Some(first) = first else {
                return true;
            };
            if first.starts_with('.')
                || (cfg!(windows) && first.eq_ignore_ascii_case("AppData"))
                || (cfg!(target_os = "macos") && first == "Library")
            {
                return true;
            }
        }
        match &self.workspaces {
            Some(workspaces) => !workspaces.roots_for(Some(path)).await.is_empty(),
            None => true,
        }
    }
}

/// File names compare without case where the file system does (Windows, macOS by default).
fn same_name(a: &str, b: &str) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Every GitHub request takes its credential here, whatever named the host: a URL,
/// `host/owner/repo`, a remote or a pull-request link. gh's rule sends `GH_TOKEN`/`GITHUB_TOKEN`
/// to github.com and every `*.ghe.com`; from the server's environment they go only to configured
/// hosts. Credentials bound to the host itself (its settings token, gh's login for it, the app's
/// session) are unchanged.
struct ConfiguredHostCredentials {
    inner: Arc<GitHubCredentials>,
    hosts: Arc<RwLock<BTreeSet<String>>>,
}
#[async_trait::async_trait]
impl t3_forge::github::transport::Credentials for ConfiguredHostCredentials {
    fn revision(&self) -> u64 {
        self.inner.revision()
    }
    async fn get(&self, host: &str, cwd: &Path) -> t3_forge::Result<Credential> {
        let credential = self.inner.get(host, cwd).await?;
        let configured = self
            .hosts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&host.trim().to_ascii_lowercase());
        if credential.source == "env" && !configured {
            return Err(ForgeError::provider(
                "github",
                "credential",
                FailureReason::Unauthenticated,
                "This GitHub host is not configured on this server.",
            ));
        }
        Ok(credential)
    }
    async fn saved(&self, host: &str) -> t3_forge::Result<Option<Credential>> {
        self.inner.saved(host).await
    }
    async fn invalidate(&self, host: &str) {
        self.inner.invalidate(host).await;
    }
    async fn settings(&self) -> Option<CredentialSettings> {
        self.inner.settings().await
    }
}

/// Per-checkout workflows: the review host is bound to the checkout's own forge and remote, never
/// reused for another repository. Project setup in a new worktree is the template's worktree
/// host's; the review host has none. For status (`lenient_lookups`) a checkout whose forge cannot
/// be resolved gets no review host, and a failed pull-request lookup does not fail the read.
struct ForgeWorkflows {
    registry: Arc<ForgeRegistry>,
    template: t3_git::GitWorkflowService,
    lenient_lookups: bool,
}
#[async_trait::async_trait]
impl t3_git::GitWorkflowFactory for ForgeWorkflows {
    async fn for_checkout(
        &self,
        cwd: &Path,
    ) -> Result<t3_git::GitWorkflowService, t3_git::GitError> {
        eprintln!("DEBUG for_checkout start {cwd:?}");
        let handle = match self.registry.resolve(cwd, None).await {
            Ok(handle) => handle,
            Err(_) if self.lenient_lookups => return Ok(self.template.at(cwd)),
            Err(_) => {
                return Err(t3_git::GitError::Unavailable(
                    "the source control provider could not be detected".into(),
                ));
            }
        };
        eprintln!("DEBUG resolved kind {}", handle.forge.kind().0);
        let host = ForgeReviewHost::new(handle, Arc::new(NoProjectSetup));
        Ok(self
            .template
            .at(cwd)
            .with_review_host(if self.lenient_lookups {
                Arc::new(StatusReviewHost {
                    inner: host,
                    last: Default::default(),
                })
            } else {
                Arc::new(host)
            }))
    }
}

/// Status looks the branch's pull request up through `list`. A failed lookup (the forge CLI
/// missing or signed out, the forge unreachable) answers that branch's last listing, else none,
/// with a warning: TypeScript's `lookupStatusPr` does the same, so status never fails on it.
struct StatusReviewHost {
    inner: ForgeReviewHost,
    last: std::sync::Mutex<BTreeMap<String, Vec<t3_contracts::source_control::ChangeRequest>>>,
}
#[async_trait::async_trait]
impl t3_git::ReviewHost for StatusReviewHost {
    async fn list(
        &self,
        cwd: &Path,
        head: &str,
        state: Option<t3_contracts::source_control::ChangeRequestState>,
        limit: usize,
    ) -> Result<Vec<t3_contracts::source_control::ChangeRequest>, t3_git::GitError> {
        let key = format!("{}\0{head}\0{state:?}\0{limit}", cwd.display());
        eprintln!("DEBUG list start {head}");
        let listed = self.inner.list(cwd, head, state, limit).await;
        eprintln!("DEBUG list end {}", listed.is_ok());
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match listed {
            Ok(requests) => {
                if last.len() >= 512 && !last.contains_key(&key) {
                    last.pop_first();
                }
                last.insert(key, requests.clone());
                Ok(requests)
            }
            Err(error) => {
                tracing::warn!(%error, "PR lookup failed.");
                Ok(last.get(&key).cloned().unwrap_or_default())
            }
        }
    }
    async fn get(
        &self,
        cwd: &Path,
        reference: &str,
    ) -> Result<t3_contracts::source_control::ChangeRequest, t3_git::GitError> {
        self.inner.get(cwd, reference).await
    }
    async fn checkout(&self, cwd: &Path, reference: &str) -> Result<(), t3_git::GitError> {
        self.inner.checkout(cwd, reference).await
    }
    async fn create(
        &self,
        cwd: &Path,
        base: &str,
        head: &str,
        title: &str,
        body_file: &Path,
    ) -> Result<(), t3_git::GitError> {
        self.inner.create(cwd, base, head, title, body_file).await
    }
    async fn clone_urls(
        &self,
        cwd: &Path,
        repository: &str,
        request_url: &str,
    ) -> Result<SourceControlRepositoryCloneUrls, t3_git::GitError> {
        self.inner.clone_urls(cwd, repository, request_url).await
    }
    async fn default_branch(&self, cwd: &Path) -> Result<Option<String>, t3_git::GitError> {
        self.inner.default_branch(cwd).await
    }
    fn head_ref(&self, number: u64) -> Option<String> {
        self.inner.head_ref(number)
    }
    fn short_label(&self) -> &str {
        self.inner.short_label()
    }
    fn head_branch_probe(
        &self,
        selectors: &[String],
        state: Option<t3_contracts::source_control::ChangeRequestState>,
        default_branch_selector: Option<&str>,
    ) -> (Vec<String>, usize) {
        self.inner
            .head_branch_probe(selectors, state, default_branch_selector)
    }
    async fn targets_remote(
        &self,
        cwd: &Path,
        remote: &str,
        repository: Option<&str>,
    ) -> Result<bool, t3_git::GitError> {
        self.inner.targets_remote(cwd, remote, repository).await
    }
    async fn setup(
        &self,
        cwd: &Path,
        thread_id: &str,
        worktree: &Path,
    ) -> Result<(), t3_git::GitError> {
        self.inner.setup(cwd, thread_id, worktree).await
    }
}

/// Why `projectClone.start` was refused.
#[cfg(feature = "project")]
pub enum ProjectCloneRefusal {
    Refused(Refusal),
    Project(SourceControlProviderKind, Box<t3_project::Error>),
}

/// Why `git.resolvePullRequest` was refused.
pub enum PullRequestRefusal {
    Authorization(Box<EnvironmentAuthorizationError>),
    Git(Box<GitManagerServiceError>),
}

fn manager_error(cwd: &str) -> Box<GitManagerServiceError> {
    Box::new(GitManagerServiceError::Manager(GitManagerError {
        _tag: GitManagerErrorTag::GitManagerError,
        operation: "resolvePullRequest".into(),
        cwd: cwd.into(),
        detail: "The pull request could not be resolved.".into(),
        cause: None,
    }))
}

/// `ForgeCloneRepository`, plus invalidating the shared Git handle's caches for the new checkout
/// (its negative discovery and unborn refs predate the clone).
#[cfg(feature = "project")]
struct ProjectClones {
    inner: t3_project::clones::ForgeCloneRepository,
    git: t3_git::Git,
}
#[cfg(feature = "project")]
#[async_trait::async_trait]
impl t3_project::clones::CloneRepository for ProjectClones {
    async fn prepare(
        &self,
        input: &t3_contracts::project_clone::ProjectCloneStartInput,
    ) -> t3_project::Result<t3_project::clones::PreparedClone> {
        self.inner.prepare(input).await
    }
    async fn clone_repository(
        &self,
        input: &t3_contracts::project_clone::ProjectCloneStartInput,
        prepared: &t3_project::clones::PreparedClone,
        progress: Arc<dyn Fn(t3_project::clones::CloneProgress) + Send + Sync>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> t3_project::Result<()> {
        self.inner
            .clone_repository(input, prepared, progress, cancel)
            .await?;
        self.git
            .at(&prepared.destination_path)
            .invalidate_caches()
            .await;
        Ok(())
    }
    async fn discard(&self, destination: &str) -> t3_project::Result<()> {
        self.inner.discard(destination).await
    }
}

impl crate::services::DomainRuntime for ForgeAdapter {
    fn name(&self) -> &'static str {
        "forge"
    }
    /// Stops following settings, then shuts the adapters down (device-flow polling, event
    /// feeds, CLI cleanup) before their HTTP clients and credentials drop.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            self.stop.cancel();
            let follower = self
                .follower
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(follower) = follower {
                let _ = follower.await;
            }
            self.registry.shutdown().await;
            Ok(())
        })
    }
}

fn denied(message: &str) -> Refusal {
    Refusal::Authorization(Box::new(EnvironmentAuthorizationError {
        _tag: EnvironmentAuthorizationErrorTag::EnvironmentAuthorizationError,
        message: message.into(),
        required_scope: AuthEnvironmentScope::OrchestrationRead,
        required_permission: None,
    }))
}

fn text(path: &Path) -> Text {
    Text(path.to_string_lossy().into_owned())
}

/// A drive path on Windows (never UNC, device or verbatim); any absolute path elsewhere.
fn local_disk(path: &Path) -> bool {
    crate::adapters::git::local_disk_path(path)
}

/// `canonicalize` answers `\\?\C:\...` on Windows; drive paths are compared in their plain form.
fn plain(path: &Path) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_owned(),
    }
}

/// The nearest existing ancestor resolved, with the missing rest appended.
fn resolved_or_lexical(path: &Path) -> PathBuf {
    if !local_disk(path) {
        return path.to_owned();
    }
    let mut existing = path;
    let mut missing = Vec::new();
    while std::fs::symlink_metadata(existing).is_err() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
    let Ok(real) = std::fs::canonicalize(existing) else {
        return path.to_owned();
    };
    let mut resolved = plain(&real);
    resolved.extend(missing.iter().rev());
    resolved
}

/// Creates `resolved`, the destination the rules accepted, then resolves it again: a link that
/// appeared inside it after the check makes it another folder, which is refused (and removed if
/// this call created it, while empty). Answers whether this call created it.
fn create_verified(resolved: &Path) -> std::io::Result<bool> {
    let existed = std::fs::symlink_metadata(resolved).is_ok();
    std::fs::create_dir_all(resolved)?;
    let again = t3_workspace::paths::canonicalize(resolved)?;
    if within(&again, resolved) && within(resolved, &again) {
        return Ok(!existed);
    }
    if !existed {
        let _ = std::fs::remove_dir(&again);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "the destination changed while it was created",
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Re-review finding 1: a link that appears inside the accepted path after the check (a
    /// concurrent clone can check one out) must not redirect the new folder.
    #[test]
    fn a_link_planted_after_the_check_does_not_redirect_the_destination() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config");
        std::fs::create_dir(&target).unwrap();
        let work = temp.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let destination = resolved_or_lexical(&work).join("link").join("git");
        let link = work.join("link");
        #[cfg(windows)]
        assert!(
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .output()
                .unwrap()
                .status
                .success()
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(create_verified(&destination).is_err());
        assert!(!target.join("git").exists());
    }
}
