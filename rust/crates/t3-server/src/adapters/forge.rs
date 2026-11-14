//! The environment's forge registry and the source-control services over it: TypeScript's
//! `SourceControlProviderRegistry` over `builtInDrivers.ts`, with `SourceControlRepositoryService`,
//! `SourceControlSignIn` and `SourceControlDiscovery`. Built once at start-up; every consumer
//! (source control, project clone, pull requests, the git PR actions) derives from
//! [`ForgeAdapter::registry`] and never builds a second adapter set.
//!
//! A client never chooses where a credential goes. GitLab sends glab only to its default host and
//! the server's `GITLAB_HOST` (or an authorised checkout's remote); GitHub requests a client
//! addresses by host (app access, stars) go only to hosts the server is configured for. Paths a
//! client names are authorised here before the forge sees them: a cwd through the workspace
//! authority, a clone destination by resolving links and refusing network, device and internal
//! T3 paths.
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
    Forge, ForgeError,
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
        transport::{CredentialSettings, GitHubCredentials, GitHubHttp, valid_host},
    },
    gitlab::GitLab,
    http::{ApiCredential, ApiHttp},
    rate_limit::RateLimits,
    review_host::{ForgeReviewHost, ReviewSetup},
    services::{CloneOptions, SourceControlRepositoryService, SourceControlSignIn},
};

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
    /// The environment's shared Git handle; its caches are the ones a clone must invalidate.
    git: t3_git::Git,
    workspaces: Arc<GitAdapter>,
    cwd: PathBuf,
    home: PathBuf,
    internal: Vec<PathBuf>,
    github_hosts: RwLock<BTreeSet<String>>,
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
            use t3_contracts::secret_reference::SecretReference;
            let reference = |value: &serde_json::Value| {
                value
                    .is_object()
                    .then(|| serde_json::from_value::<SecretReference>(value.clone()).ok())
                    .flatten()
            };
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

        let sessions = Arc::new(SettingsAppSessions::new(
            settings.clone(),
            t3_config::secrets::SecretStore::new(config.paths.secrets()),
        ));
        // An unset client id turns the GitHub app sign-in off (`CLIENT_ID_VARIABLE`).
        let client_id = variable(&environment, "T3CODE_GITHUB_APP_CLIENT_ID")
            .unwrap_or_default()
            .to_owned();
        let mut app = AppSignIn::new(client_id, sessions).map_err(service)?;
        let mut http =
            GitHubHttp::new(credentials.github.clone(), limits.clone()).map_err(service)?;
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
            registry,
            git,
            workspaces,
            internal: [paths.state_dir.clone(), paths.secrets()]
                .into_iter()
                .map(|path| resolved_or_lexical(&path))
                .collect(),
            cwd,
            home,
            github_hosts: RwLock::new(github_hosts(hosts.get("github"), &environment)),
            credentials,
            stop: context.shutdown.child_token(),
            follower: Default::default(),
        });
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
        let destination = self
            .authorize_destination(&input.destination_path.0)
            .await?;
        input.destination_path = text(&destination);
        let cloned = self
            .repositories
            .clone_repository(input, CloneOptions::default())
            .await
            .map_err(|error| Refusal::Repository(error.wire))?;
        self.git.at(&destination).invalidate_caches().await;
        Ok(cloned)
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

    /// `server.discoverSourceControl`; its only failure is an invalid probe result.
    pub async fn discover(&self) -> Option<SourceControlDiscoveryResult> {
        self.discovery.discover().await.ok()
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
        let handle = self
            .registry
            .resolve(git.cwd(), None)
            .await
            .map_err(|_| PullRequestRefusal::Git(manager_error(&input.cwd.0)))?;
        let host = Arc::new(ForgeReviewHost::new(handle, Arc::new(NoProjectSetup)));
        t3_git::GitWorkflowService::new(git)
            .with_review_host(host)
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

    /// Clone destinations: `~` and relative paths as the repository service reads them, then the
    /// nearest existing ancestor resolved through links. Network shares, device paths and T3's
    /// own state are refused (a UNC destination would make a Windows server authenticate over
    /// SMB); the forge clones into the resolved path.
    pub async fn authorize_destination(&self, raw: &str) -> Result<PathBuf, Refusal> {
        let refused = || {
            denied(
                "The clone destination must be a folder on this server's local disks, outside T3 Code's own state.",
            )
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
        if !lexical.is_absolute() || lexical.file_name().is_none() || !local_disk(&lexical) {
            return Err(refused());
        }
        let mut existing = lexical.as_path();
        let mut missing = Vec::new();
        while tokio::fs::symlink_metadata(existing).await.is_err() {
            let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
                return Err(refused());
            };
            missing.push(name.to_owned());
            existing = parent;
        }
        let mut resolved = tokio::fs::canonicalize(existing)
            .await
            .map(|real| plain(&real))
            .map_err(|_| refused())?;
        resolved.extend(missing.iter().rev());
        if !local_disk(&resolved)
            || resolved.file_name().is_none()
            || self
                .internal
                .iter()
                .any(|internal| resolved.starts_with(internal))
        {
            return Err(refused());
        }
        Ok(resolved)
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
        let destination = self
            .authorize_destination(&input.destination_path.0)
            .await
            .map_err(ProjectCloneRefusal::Refused)?;
        input.destination_path = text(&destination);
        let provider = input
            .provider
            .clone()
            .flatten()
            .unwrap_or_else(|| SourceControlProviderKind("unknown".into()));
        projects
            .start_clone(input)
            .await
            .map_err(|error| ProjectCloneRefusal::Project(provider, error))
    }
}

/// Why `projectClone.start` was refused.
#[cfg(feature = "project")]
pub enum ProjectCloneRefusal {
    Refused(Refusal),
    Project(SourceControlProviderKind, t3_project::Error),
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
        required_scope: AuthEnvironmentScope::SourceControlWrite,
        required_permission: None,
    }))
}

fn text(path: &Path) -> Text {
    Text(path.to_string_lossy().into_owned())
}

/// A drive path on Windows (never UNC, device or verbatim); any absolute path elsewhere.
fn local_disk(path: &Path) -> bool {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => matches!(prefix.kind(), std::path::Prefix::Disk(_)),
        Some(Component::RootDir) => !cfg!(windows),
        _ => false,
    }
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
