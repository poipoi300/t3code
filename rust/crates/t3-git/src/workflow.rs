use crate::{
    CommitOptions, ExecuteOptions, Git, GitError, WorktreeOptions,
    core::{Cache, branch_from_ref, physical_path},
    parse_commit_message, remote_repository_key,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString as Text,
    git::*,
    source_control::{ChangeRequest, ChangeRequestState, SourceControlRepositoryCloneUrls},
};
use tokio::sync::Mutex;
use unicode_normalization::UnicodeNormalization;

pub type ActionProgress = Arc<dyn Fn(GitActionProgressEvent) + Send + Sync>;

/// Forge and project setup are external services; their adapters retain their own CLI/HTTP policy.
#[async_trait]
pub trait ReviewHost: Send + Sync {
    async fn list(
        &self,
        cwd: &Path,
        head: &str,
        state: Option<ChangeRequestState>,
        limit: usize,
    ) -> Result<Vec<ChangeRequest>, GitError>;
    async fn get(&self, cwd: &Path, reference: &str) -> Result<ChangeRequest, GitError>;
    async fn checkout(&self, cwd: &Path, reference: &str) -> Result<(), GitError>;
    async fn create(
        &self,
        cwd: &Path,
        base: &str,
        head: &str,
        title: &str,
        body_file: &Path,
    ) -> Result<(), GitError>;
    async fn clone_urls(
        &self,
        cwd: &Path,
        repository: &str,
        request_url: &str,
    ) -> Result<SourceControlRepositoryCloneUrls, GitError>;
    async fn default_branch(&self, cwd: &Path) -> Result<Option<String>, GitError>;
    fn head_ref(&self, number: u64) -> Option<String>;
    fn short_label(&self) -> &str {
        "PR"
    }
    fn head_branch_probe(
        &self,
        selectors: &[String],
        state: Option<ChangeRequestState>,
        default_branch_selector: Option<&str>,
    ) -> (Vec<String>, usize) {
        let _ = default_branch_selector;
        (
            selectors.to_vec(),
            if state == Some(ChangeRequestState::Open) {
                1
            } else {
                20
            },
        )
    }
    async fn targets_remote(
        &self,
        _cwd: &Path,
        _remote: &str,
        _repository: Option<&str>,
    ) -> Result<bool, GitError> {
        Ok(true)
    }
    async fn setup(&self, _cwd: &Path, _thread_id: &str, _worktree: &Path) -> Result<(), GitError> {
        Ok(())
    }
}

async fn cancellable<T>(
    token: &crate::CancellationToken,
    operation: impl std::future::Future<Output = Result<T, GitError>>,
) -> Result<T, GitError> {
    tokio::select! { biased; _ = token.cancelled() => Err(t3_process::ProcessError::Cancelled.into()), result = operation => result }
}
struct ReviewSession<'a> {
    host: &'a dyn ReviewHost,
    token: &'a crate::CancellationToken,
}
#[async_trait]
impl ReviewHost for ReviewSession<'_> {
    async fn list(
        &self,
        cwd: &Path,
        head: &str,
        state: Option<ChangeRequestState>,
        limit: usize,
    ) -> Result<Vec<ChangeRequest>, GitError> {
        cancellable(self.token, self.host.list(cwd, head, state, limit)).await
    }
    async fn get(&self, cwd: &Path, reference: &str) -> Result<ChangeRequest, GitError> {
        cancellable(self.token, self.host.get(cwd, reference)).await
    }
    async fn checkout(&self, cwd: &Path, reference: &str) -> Result<(), GitError> {
        cancellable(self.token, self.host.checkout(cwd, reference)).await
    }
    async fn create(
        &self,
        cwd: &Path,
        base: &str,
        head: &str,
        title: &str,
        body_file: &Path,
    ) -> Result<(), GitError> {
        cancellable(
            self.token,
            self.host.create(cwd, base, head, title, body_file),
        )
        .await
    }
    async fn clone_urls(
        &self,
        cwd: &Path,
        repository: &str,
        request_url: &str,
    ) -> Result<SourceControlRepositoryCloneUrls, GitError> {
        cancellable(
            self.token,
            self.host.clone_urls(cwd, repository, request_url),
        )
        .await
    }
    async fn default_branch(&self, cwd: &Path) -> Result<Option<String>, GitError> {
        cancellable(self.token, self.host.default_branch(cwd)).await
    }
    async fn targets_remote(
        &self,
        cwd: &Path,
        remote: &str,
        repository: Option<&str>,
    ) -> Result<bool, GitError> {
        cancellable(
            self.token,
            self.host.targets_remote(cwd, remote, repository),
        )
        .await
    }
    async fn setup(&self, cwd: &Path, thread: &str, worktree: &Path) -> Result<(), GitError> {
        cancellable(self.token, self.host.setup(cwd, thread, worktree)).await
    }
    fn head_ref(&self, number: u64) -> Option<String> {
        self.host.head_ref(number)
    }
    fn short_label(&self) -> &str {
        self.host.short_label()
    }
    fn head_branch_probe(
        &self,
        selectors: &[String],
        state: Option<ChangeRequestState>,
        default: Option<&str>,
    ) -> (Vec<String>, usize) {
        self.host.head_branch_probe(selectors, state, default)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestContent {
    pub title: String,
    pub body: String,
}
#[async_trait]
pub trait WorkflowTextGenerator: Send + Sync {
    async fn commit_message(
        &self,
        git: &Git,
        context: &crate::PreparedCommit,
    ) -> Result<String, GitError>;
    async fn pull_request(
        &self,
        git: &Git,
        base: &str,
        head: &str,
    ) -> Result<PullRequestContent, GitError>;
}
#[derive(Clone, Default)]
pub struct WorktreeCreationSettings {
    pub directory: Option<PathBuf>,
    pub submodules: Option<crate::WorktreeSubmodules>,
}
#[async_trait]
pub trait WorktreeHost: Send + Sync {
    async fn creation_settings(&self, cwd: &Path) -> WorktreeCreationSettings;
    async fn setup(&self, cwd: &Path, thread_id: &str, worktree: &Path) -> Result<(), GitError>;
}
#[derive(Debug, Clone)]
pub struct BranchHeadContext {
    pub local_branch: String,
    pub head_branch: String,
    pub head_selectors: Vec<String>,
    pub preferred_head_selector: String,
    pub remote_name: Option<String>,
    pub head_repository: Option<String>,
    pub head_owner: Option<String>,
    pub cross_repository: bool,
}

pub fn matches_branch_head(request: &ChangeRequest, context: &BranchHeadContext) -> bool {
    if request.head_ref_name.0 != context.head_branch {
        return false;
    }
    let request_repository = request
        .head_repository_name_with_owner
        .as_ref()
        .and_then(Option::as_ref)
        .map(|value| value.0.trim().to_lowercase());
    let request_owner = request
        .head_repository_owner_login
        .as_ref()
        .and_then(Option::as_ref)
        .map(|value| value.0.trim().to_lowercase())
        .or_else(|| {
            request_repository
                .as_ref()
                .and_then(|repository| repository.split('/').next().map(str::to_owned))
        });
    let repository = context
        .head_repository
        .as_ref()
        .map(|value| value.trim().to_lowercase());
    let owner = context
        .head_owner
        .as_ref()
        .map(|value| value.trim().to_lowercase())
        .or_else(|| {
            repository
                .as_ref()
                .and_then(|repository| repository.split('/').next().map(str::to_owned))
        });
    if repository
        .as_ref()
        .zip(request_repository.as_ref())
        .is_some_and(|(left, right)| left != right)
        || owner
            .as_ref()
            .zip(request_owner.as_ref())
            .is_some_and(|(left, right)| left != right)
    {
        return false;
    }
    if context.cross_repository {
        if request.is_cross_repository.flatten() == Some(false) {
            return false;
        }
        if (repository.is_some() || owner.is_some())
            && request_repository.is_none()
            && request_owner.is_none()
        {
            return false;
        }
    } else if request.is_cross_repository.flatten() == Some(true)
        && (repository.is_none() && owner.is_none()
            || request_repository.is_none() && request_owner.is_none())
    {
        return false;
    }
    true
}

pub fn sanitize_feature_branch(raw: &str) -> String {
    let normalized: String = raw
        .nfkd()
        .filter(|character| !('\u{0300}'..='\u{036f}').contains(character))
        .collect::<String>()
        .trim()
        .to_lowercase()
        .chars()
        .filter(|character| !['\'', '"', '`'].contains(character))
        .collect();
    let normalized = normalized
        .trim_matches(|character: char| "./_-".contains(character) || character.is_whitespace());
    let mut output = String::new();
    for character in normalized.chars() {
        let character = if character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || "/_-".contains(character)
        {
            character
        } else {
            '-'
        };
        if (character == '/' || character == '-') && output.ends_with(character) {
            continue;
        }
        output.push(character);
    }
    let output: String = output
        .trim_matches(['.', '/', '_', '-'])
        .chars()
        .take(64)
        .collect();
    let output = output.trim_end_matches(['.', '/', '_', '-']);
    let output = if output.is_empty() { "update" } else { output };
    if output.contains('/') {
        output.into()
    } else {
        format!("feature/{output}")
    }
}
pub fn available_feature_branch(existing: &[String], preferred: &str) -> String {
    let base = sanitize_feature_branch(preferred);
    let existing: BTreeSet<_> = existing.iter().map(|value| value.to_lowercase()).collect();
    let parts: Vec<_> = base.split('/').collect();
    let mut resolved = String::new();
    for (index, part) in parts.iter().enumerate() {
        let base = if resolved.is_empty() {
            (*part).into()
        } else {
            format!("{resolved}/{part}")
        };
        let mut candidate = base.clone();
        let mut suffix = 2;
        while existing.contains(&candidate)
            || index == parts.len() - 1
                && existing
                    .iter()
                    .any(|reference| reference.starts_with(&format!("{candidate}/")))
        {
            candidate = format!("{base}-{suffix}");
            suffix += 1;
        }
        resolved = candidate;
    }
    resolved
}
fn repository_name(url: &str) -> Option<String> {
    let key = remote_repository_key(url);
    let (_, repository) = key.split_once('/')?;
    (repository.split('/').count() >= 2 && !key.contains(['\\', ':'])).then(|| repository.into())
}

type RequestCache = BTreeMap<String, (tokio::time::Instant, Option<ChangeRequest>)>;

#[async_trait]
pub trait GitWorkflowFactory: Send + Sync {
    async fn for_checkout(&self, cwd: &Path) -> Result<GitWorkflowService, GitError>;

    async fn can_reuse_checkout(
        &self,
        _cwd: &Path,
        _workflow: &GitWorkflowService,
    ) -> Result<bool, GitError> {
        Ok(true)
    }
}

#[derive(Clone)]
pub struct GitWorkflowService {
    git: Git,
    host: Option<Arc<dyn ReviewHost>>,
    worktrees_dir: Option<PathBuf>,
    text_generator: Option<Arc<dyn WorkflowTextGenerator>>,
    worktree_host: Option<Arc<dyn WorktreeHost>>,
    remote_status: Arc<Cache<Option<VcsStatusRemoteResult>>>,
    requests: Arc<Mutex<RequestCache>>,
}
pub type GitManager = GitWorkflowService;
impl GitWorkflowService {
    pub fn new(git: Git) -> Self {
        Self {
            git,
            host: None,
            worktrees_dir: None,
            text_generator: None,
            worktree_host: None,
            remote_status: Arc::new(Cache::new(2048)),
            requests: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    pub(crate) fn without_fsmonitor(mut self) -> Self {
        self.git = self.git.without_fsmonitor();
        self
    }
    pub fn with_review_host(mut self, host: Arc<dyn ReviewHost>) -> Self {
        self.host = Some(host);
        self
    }
    pub fn with_worktrees_directory(mut self, directory: PathBuf) -> Self {
        self.worktrees_dir = Some(directory);
        self
    }
    pub fn with_text_generator(mut self, generator: Arc<dyn WorkflowTextGenerator>) -> Self {
        self.text_generator = Some(generator);
        self
    }
    pub fn with_worktree_host(mut self, host: Arc<dyn WorktreeHost>) -> Self {
        self.worktree_host = Some(host);
        self
    }
    async fn setup_worktree(&self, thread: &str, path: &Path) -> Result<(), GitError> {
        if let Some(host) = &self.worktree_host {
            cancellable(
                &self.git.cancellation,
                host.setup(self.git.cwd(), thread, path),
            )
            .await
        } else {
            self.host()?.setup(self.git.cwd(), thread, path).await
        }
    }
    pub async fn create_worktree(
        &self,
        input: &VcsCreateWorktreeInput,
    ) -> Result<VcsCreateWorktreeResult, GitError> {
        self.create_worktree_with_options(input, crate::WorktreeCreationOptions::default())
            .await
    }
    pub async fn create_worktree_with_options(
        &self,
        input: &VcsCreateWorktreeInput,
        options: crate::WorktreeCreationOptions,
    ) -> Result<VcsCreateWorktreeResult, GitError> {
        let mut workflow = self.clone();
        let input = input.clone();
        self.git
            .owned_operation(move |git| async move {
                workflow.git = git;
                workflow.create_worktree_inner(&input, options).await
            })
            .await
    }
    async fn create_worktree_inner(
        &self,
        input: &VcsCreateWorktreeInput,
        options: crate::WorktreeCreationOptions,
    ) -> Result<VcsCreateWorktreeResult, GitError> {
        let git = self.git.at(&input.cwd.0);
        let settings = match &self.worktree_host {
            Some(host) => {
                cancellable(&git.cancellation, async {
                    Ok(host.creation_settings(git.cwd()).await)
                })
                .await?
            }
            None => WorktreeCreationSettings::default(),
        };
        let branch = input.new_ref_name.as_ref().and_then(Option::as_ref);
        let base = if branch.is_some() {
            if let Some(base) = input.base_ref_name.as_ref().and_then(Option::as_ref) {
                let resolved = git
                    .core_text(
                        "GitManager.createWorktree.base",
                        &[
                            "rev-parse",
                            "--symbolic-full-name",
                            "--verify",
                            "--end-of-options",
                            &base.0,
                        ],
                        true,
                    )
                    .await?;
                let resolved = resolved.trim();
                if let Some(name) = resolved.strip_prefix("refs/heads/") {
                    Some((name.to_owned(), resolved.to_owned()))
                } else if let Some(remote_ref) = resolved.strip_prefix("refs/remotes/") {
                    crate::parse_remote_ref(remote_ref, &git.list_remote_names().await?)
                        .filter(|(_, name)| name != "HEAD")
                        .map(|(_, name)| (name, resolved.to_owned()))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let path = git
            .add_worktree_with_progress(
                WorktreeOptions {
                    reference: input.ref_name.0.clone(),
                    new_branch: branch.map(|name| name.0.clone()),
                    path: input.path.as_ref().map(|path| PathBuf::from(&path.0)),
                    directory: settings.directory.or_else(|| self.worktrees_dir.clone()),
                    submodules: options
                        .submodules
                        .or(settings.submodules)
                        .unwrap_or(crate::WorktreeSubmodules::Auto),
                },
                options.progress,
            )
            .await?;
        if let (Some(branch), Some((name, reference))) = (branch, base) {
            let _ = git
                .write_config_value(&format!("branch.{}.t3-merge-base", branch.0), &name)
                .await;
            if reference.starts_with("refs/remotes/") {
                let _ = git
                    .write_config_value(
                        &format!("branch.{}.t3-merge-base-ref", branch.0),
                        &reference,
                    )
                    .await;
            }
        }
        // Refresh before returning: an index refresh in the background races setup and agent Git commands.
        let _ = git
            .at(&path)
            .execute(
                &["update-index", "--refresh"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await;
        self.invalidate_status().await;
        Ok(VcsCreateWorktreeResult {
            worktree: VcsWorktree {
                path: Text(path.to_string_lossy().into()),
                ref_name: branch.cloned().unwrap_or_else(|| input.ref_name.clone()),
            },
        })
    }
    pub fn git(&self) -> &Git {
        &self.git
    }
    pub fn at(&self, cwd: impl Into<PathBuf>) -> Self {
        Self {
            git: self.git.at(cwd),
            ..self.clone()
        }
    }
    /// This workflow with its Git handle pinned to a saved repository binding
    /// ([`Git::with_repository_binding`]): every command it runs refuses a changed repository.
    pub async fn with_repository_binding(
        mut self,
        binding: crate::RepositoryBinding,
    ) -> Result<Self, GitError> {
        self.git = self.git.with_repository_binding(binding).await?;
        Ok(self)
    }
    pub fn with_git(mut self, git: Git) -> Self {
        self.git = git;
        self
    }
    pub fn with_cancellation(mut self, cancellation: crate::CancellationToken) -> Self {
        self.git.operation_scope = Some(Arc::new(crate::actions::OperationScope::default()));
        self.git.status_fetch_scope = Some(cancellation.clone());
        self.git = self.git.with_cancellation(cancellation);
        self
    }
    pub async fn invalidate_local_status(&self) {
        self.git
            .core
            .local_status
            .invalidate(&physical_path(self.git.cwd()).to_string_lossy())
            .await;
    }
    pub async fn invalidate_remote_status(&self) {
        self.remote_status
            .invalidate(&self.git.cwd().to_string_lossy())
            .await;
    }
    fn host(&self) -> Result<ReviewSession<'_>, GitError> {
        self.host
            .as_deref()
            .map(|host| ReviewSession {
                host,
                token: &self.git.cancellation,
            })
            .ok_or_else(|| {
                self.git.command_error(
                    "GitManager.sourceControlProvider",
                    "No source control provider is configured for this workflow.",
                )
            })
    }
    pub async fn repository_root(&self) -> Result<Option<PathBuf>, GitError> {
        Ok(self
            .git
            .repository_paths_uncached()
            .await?
            .map(|paths| paths.repository.root))
    }
    pub async fn current_branch(&self) -> Result<Option<String>, GitError> {
        let reader = self.git.without_fsmonitor();
        if reader.repository_paths_uncached().await?.is_none() {
            return Ok(None);
        }
        let output = reader
            .execute_command(
                "GitWorkflowService.currentBranch",
                &["symbolic-ref", "--quiet", "HEAD"],
                ExecuteOptions {
                    allow_nonzero: true,
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
            )
            .await?;
        match output.code {
            Some(0) => Ok(branch_from_ref(&output.stdout_text())),
            Some(1) => Ok(None),
            _ => Err(reader.command_error(
                "GitWorkflowService.currentBranch",
                "Failed to read the checked-out branch.",
            )),
        }
    }
    pub async fn local_status(&self) -> Result<VcsStatusLocalResult, GitError> {
        let details = self.git.cached_local_status().await?;
        Ok(VcsStatusLocalResult {
            is_repo: details.is_repo,
            source_control_provider: None,
            has_primary_remote: details.has_origin_remote,
            is_default_ref: details.is_default_branch,
            ref_name: details.branch.map(Text),
            has_working_tree_changes: details.has_working_tree_changes,
            working_tree: details.working_tree,
            branch_changes: details.branch_changes.map(Some),
        })
    }
    pub async fn invalidate_status(&self) {
        self.git.invalidate_caches().await;
        self.remote_status.clear().await;
        self.requests.lock().await.clear();
    }
    pub async fn branch_head_context(
        &self,
        branch: &str,
        upstream: Option<&str>,
        remote_override: Option<&str>,
    ) -> Result<BranchHeadContext, GitError> {
        let configured = match remote_override {
            Some(remote) => Some(remote.into()),
            None => {
                self.git
                    .read_config_value(&format!("branch.{branch}.remote"))
                    .await?
            }
        };
        let remote = configured.as_deref().filter(|name| *name != ".");
        let head = if configured.as_deref() == Some(".") {
            branch
        } else {
            upstream
                .map(|upstream| {
                    remote
                        .and_then(|remote| upstream.strip_prefix(&format!("{remote}/")))
                        .or_else(|| upstream.split_once('/').map(|(_, branch)| branch))
                        .unwrap_or(upstream)
                })
                .filter(|branch| !branch.is_empty())
                .unwrap_or(branch)
        };
        let origin_url = self.git.read_config_value("remote.origin.url").await?;
        let remote_url = if let Some(remote) = remote {
            self.git
                .read_config_value(&format!("remote.{remote}.url"))
                .await?
        } else {
            None
        };
        let origin_repository = origin_url.as_deref().and_then(repository_name);
        let head_repository = remote_url.as_deref().and_then(repository_name);
        let owner = head_repository
            .as_ref()
            .and_then(|repository| repository.split('/').next().map(str::to_owned));
        let only_remote = remote.is_some_and(|remote| remote != "origin")
            && origin_url.is_none()
            && self.git.list_remote_names().await?.len() == 1
            && match self.host.as_deref() {
                Some(host) => {
                    cancellable(
                        &self.git.cancellation,
                        host.targets_remote(
                            self.git.cwd(),
                            remote.unwrap_or(""),
                            head_repository.as_deref(),
                        ),
                    )
                    .await?
                }
                None => true,
            };
        let foreign_remote = remote.is_some_and(|remote| remote != "origin") && !only_remote;
        let targets = if let (Some(host), Some(remote)) = (self.host.as_deref(), remote) {
            cancellable(
                &self.git.cancellation,
                host.targets_remote(self.git.cwd(), remote, head_repository.as_deref()),
            )
            .await?
        } else {
            true
        };
        let cross = match (&head_repository, &origin_repository) {
            (Some(head), Some(origin)) => head.to_lowercase() != origin.to_lowercase(),
            _ => foreign_remote && head_repository.is_some(),
        } || remote.is_some() && !only_remote && head_repository.is_some() && !targets;
        let owner_selector = owner.as_ref().map(|owner| format!("{owner}:{head}"));
        let alias = remote.map(|remote| format!("{remote}:{head}"));
        let mut selectors = Vec::new();
        let mut append = |value: Option<String>| {
            if let Some(value) = value
                && !value.trim().is_empty()
                && !selectors.contains(&value)
            {
                selectors.push(value);
            }
        };
        if cross {
            append(owner_selector.clone());
            append(alias.clone());
        }
        if upstream.is_none() || head == branch {
            append(Some(branch.into()));
        }
        if head != branch {
            append(Some(head.into()));
        }
        if !cross && foreign_remote {
            append(owner_selector.clone());
            append(alias);
        }
        Ok(BranchHeadContext {
            local_branch: branch.into(),
            head_branch: head.into(),
            head_selectors: selectors,
            preferred_head_selector: if cross {
                owner_selector.unwrap_or_else(|| head.into())
            } else {
                head.into()
            },
            remote_name: remote.map(str::to_owned),
            head_repository,
            head_owner: owner,
            cross_repository: cross,
        })
    }
    async fn find_request(
        &self,
        context: &BranchHeadContext,
        open_only: bool,
    ) -> Result<Option<ChangeRequest>, GitError> {
        let host = self.host()?;
        let mut requests = BTreeMap::new();
        let state = open_only.then_some(ChangeRequestState::Open);
        let (selectors, limit) =
            host.head_branch_probe(&context.head_selectors, state.clone(), None);
        for selector in &selectors {
            let state = open_only.then_some(ChangeRequestState::Open);
            for request in host.list(self.git.cwd(), selector, state, limit).await? {
                if matches_branch_head(&request, context) {
                    if open_only {
                        return Ok(Some(request));
                    }
                    requests.insert(request.number, request);
                }
            }
        }
        let mut requests: Vec<_> = requests.into_values().collect();
        requests.sort_by(|left, right| {
            let updated = |request: &ChangeRequest| match &request.updated_at {
                t3_contracts::source_control::WireOption::Some { value } => value.0.clone(),
                _ => String::new(),
            };
            updated(right).cmp(&updated(left))
        });
        Ok(requests
            .iter()
            .find(|request| request.state == ChangeRequestState::Open)
            .cloned()
            .or_else(|| requests.into_iter().next()))
    }
    async fn find_tracking_remote(
        &self,
        branch: &str,
        preferred: Option<&str>,
    ) -> Result<Option<String>, GitError> {
        let names = self.git.list_remote_names().await?;
        let mut matching = Vec::new();
        for name in names {
            if self.git.remote_branch_exists(&name, branch).await? {
                matching.push(name);
            }
        }
        if let Some(preferred) = preferred
            && matching.iter().any(|name| name == preferred)
        {
            return Ok(Some(preferred.into()));
        }
        if matching.iter().any(|name| name == "origin") {
            return Ok(Some("origin".into()));
        }
        Ok(matching.into_iter().next())
    }
    async fn find_published_remote(&self, branch: &str) -> Result<Option<String>, GitError> {
        let names = self.git.list_remote_names().await?;
        let local_ref = format!("refs/heads/{branch}");
        let mut args = vec![
            "for-each-ref".into(),
            "--format=%(refname)%00%(objectname)".into(),
            local_ref.clone(),
        ];
        args.extend(
            names
                .iter()
                .map(|name| format!("refs/remotes/{name}/{branch}")),
        );
        let output = self
            .git
            .execute_command(
                "GitManager.findPublishedBranchRemote",
                &args,
                ExecuteOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
            )
            .await?;
        let refs: BTreeMap<_, _> = output
            .stdout_text()
            .lines()
            .filter_map(|line| {
                line.trim()
                    .split_once('\0')
                    .map(|(name, oid)| (name.to_owned(), oid.to_owned()))
            })
            .collect();
        let Some(local) = refs.get(&local_ref) else {
            return Ok(None);
        };
        let remotes: Vec<_> = names
            .iter()
            .filter_map(|name| {
                refs.get(&format!("refs/remotes/{name}/{branch}"))
                    .map(|oid| (name, oid))
            })
            .collect();
        let exact: Vec<_> = remotes.iter().filter(|(_, oid)| *oid == local).collect();
        let name = if exact.len() == 1 {
            Some(exact[0].0.clone())
        } else if exact.len() > 1 {
            None
        } else {
            let mut ancestors = Vec::new();
            for (name, oid) in remotes {
                let result = self
                    .git
                    .execute_command(
                        "GitManager.findPublishedBranchRemote.ancestor",
                        &["merge-base", "--is-ancestor", oid, local],
                        ExecuteOptions {
                            allow_nonzero: true,
                            timeout: Some(Duration::from_secs(5)),
                            ..Default::default()
                        },
                    )
                    .await?;
                match result.code {
                    Some(0) => ancestors.push(name.clone()),
                    Some(1) => {}
                    _ => {
                        return Err(self.git.command_error(
                            "GitManager.findPublishedBranchRemote.ancestor",
                            "Ancestry probe failed; rejecting ancestry inference.",
                        ));
                    }
                }
            }
            if ancestors.len() == 1 {
                ancestors.into_iter().next()
            } else {
                None
            }
        };
        if let Some(name) = name
            && self
                .git
                .read_config_value(&format!("remote.{name}.url"))
                .await?
                .is_some()
        {
            return Ok(Some(name));
        }
        Ok(None)
    }
    async fn lookup_head_context(
        &self,
        branch: &str,
        upstream: Option<&str>,
        default_branch: Option<&str>,
    ) -> Result<(BranchHeadContext, bool), GitError> {
        let context = self.branch_head_context(branch, upstream, None).await?;
        if upstream.is_none()
            && let Some(remote) = self.find_published_remote(branch).await.ok().flatten()
            && Some(remote.as_str()) != context.remote_name.as_deref()
        {
            return Ok((
                self.branch_head_context(branch, upstream, Some(&remote))
                    .await?,
                true,
            ));
        }
        let explicit_pr = regex::Regex::new(r"^t3code[/-]pr-\d+[/-]")
            .is_ok_and(|pattern| pattern.is_match(branch));
        let tracks_fork_head = context.cross_repository
            && (context.remote_name.as_deref() != Some("origin") || explicit_pr);
        if context.head_branch != branch && !tracks_fork_head {
            let base = Some(context.head_branch.as_str()) == default_branch
                || default_branch.is_none()
                    && ["main", "master"].contains(&context.head_branch.as_str())
                || self
                    .git
                    .read_branch_base(branch)
                    .await
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some(context.head_branch.as_str());
            if base {
                let remote = self
                    .find_tracking_remote(branch, context.remote_name.as_deref())
                    .await
                    .ok()
                    .flatten();
                return match remote {
                    Some(remote) => Ok((
                        self.branch_head_context(branch, None, Some(&remote))
                            .await?,
                        true,
                    )),
                    None => Ok((context, false)),
                };
            }
        }
        let remote = self
            .git
            .read_config_value(&format!("branch.{branch}.remote"))
            .await?;
        let merge = self
            .git
            .read_config_value(&format!("branch.{branch}.merge"))
            .await?;
        if (upstream.is_none() || context.remote_name.is_none())
            && !(remote.as_deref().is_some_and(|remote| remote != ".") && merge.is_some())
        {
            let any = self
                .git
                .core_text(
                    "GitManager.isUnpublishedBranch",
                    &[
                        "for-each-ref",
                        "--count=1",
                        "--format=%(refname)",
                        "refs/remotes",
                    ],
                    false,
                )
                .await?;
            if !any.trim().is_empty()
                && self
                    .find_tracking_remote(&context.head_branch, None)
                    .await?
                    .is_none()
            {
                return Ok((context, false));
            }
        }
        Ok((context, true))
    }
    pub async fn remote_status(
        &self,
        refresh_upstream: bool,
    ) -> Result<Option<VcsStatusRemoteResult>, GitError> {
        let read = || async {
            let details = self.git.status_details_remote(refresh_upstream).await?;
            if !details.is_repo {
                return Ok(None);
            }
            let mut result = details.to_contract();
            if let (Some(branch), Some(_host)) = (&details.branch, &self.host) {
                let (context, lookup) = self
                    .lookup_head_context(
                        branch,
                        details.upstream_ref.as_deref(),
                        details.default_branch.as_deref(),
                    )
                    .await?;
                let key = format!(
                    "{}\0{branch}\0{}\0{}",
                    self.git.cwd().display(),
                    details.upstream_ref.as_deref().unwrap_or(""),
                    context.remote_name.as_deref().unwrap_or("")
                );
                let remembered = self.requests.lock().await.get(&key).cloned();
                let request = if let Some((expires, request)) =
                    remembered.filter(|(expires, _)| tokio::time::Instant::now() < *expires)
                {
                    let _ = expires;
                    request
                } else {
                    let request = if lookup {
                        self.find_request(&context, false).await?
                    } else {
                        None
                    };
                    let mut requests = self.requests.lock().await;
                    if requests.len() >= 2048 && !requests.contains_key(&key) {
                        requests.pop_first();
                    }
                    let ttl = if request
                        .as_ref()
                        .is_some_and(|request| request.state == ChangeRequestState::Open)
                    {
                        60
                    } else {
                        300
                    };
                    requests.insert(
                        key,
                        (
                            tokio::time::Instant::now() + Duration::from_secs(ttl),
                            request.clone(),
                        ),
                    );
                    request
                };
                if let Some(request) = request
                    && (!details.is_default_branch || request.state == ChangeRequestState::Open)
                {
                    let shipped = request.state == ChangeRequestState::Merged
                        && request
                            .head_sha
                            .as_ref()
                            .and_then(Option::as_ref)
                            .is_some_and(|sha| !sha.0.is_empty())
                        && (Some(request.base_ref_name.0.as_str())
                            == details.default_branch.as_deref()
                            || details.default_branch.is_none()
                                && ["main", "master"].contains(&request.base_ref_name.0.as_str()))
                        && self.git.resolve_commit("HEAD").await.ok().as_deref()
                            == request
                                .head_sha
                                .as_ref()
                                .and_then(Option::as_ref)
                                .map(|sha| sha.0.as_str());
                    if shipped {
                        result.ahead_of_default_count = Some(Some(0));
                        if !details.has_upstream {
                            result.ahead_count = 0;
                        }
                    }
                    result.pr = Some(status_pr(&request));
                }
            }
            Ok(Some(result))
        };
        if !refresh_upstream {
            return read().await;
        }
        self.remote_status
            .get(
                &self.git.cwd().to_string_lossy(),
                Duration::from_secs(1),
                Duration::ZERO,
                read,
            )
            .await
    }
    pub async fn status(&self) -> Result<VcsStatusResult, GitError> {
        let (local, remote) = tokio::try_join!(self.local_status(), self.remote_status(true))?;
        let remote = remote.unwrap_or(VcsStatusRemoteResult {
            has_upstream: false,
            ahead_count: 0,
            behind_count: 0,
            ahead_of_default_count: Some(Some(0)),
            pr: None,
        });
        Ok(VcsStatusResult {
            is_repo: local.is_repo,
            source_control_provider: local.source_control_provider,
            has_primary_remote: local.has_primary_remote,
            is_default_ref: local.is_default_ref,
            ref_name: local.ref_name,
            has_working_tree_changes: local.has_working_tree_changes,
            working_tree: VcsStatusResultWorkingTree {
                root_path: local.working_tree.root_path,
                files: local
                    .working_tree
                    .files
                    .into_iter()
                    .map(|file| VcsStatusResultWorkingTreeFiles {
                        path: file.path,
                        insertions: file.insertions,
                        deletions: file.deletions,
                    })
                    .collect(),
                insertions: local.working_tree.insertions,
                deletions: local.working_tree.deletions,
            },
            branch_changes: local.branch_changes.flatten().map(|value| {
                Some(VcsStatusResultBranchChanges {
                    base_ref: value.base_ref,
                    insertions: value.insertions,
                    deletions: value.deletions,
                })
            }),
            has_upstream: remote.has_upstream,
            ahead_count: remote.ahead_count,
            behind_count: remote.behind_count,
            ahead_of_default_count: remote.ahead_of_default_count,
            pr: remote.pr,
        })
    }
    pub async fn resolve_pull_request(
        &self,
        reference: &str,
    ) -> Result<GitResolvePullRequestResult, GitError> {
        let reference = normalize_reference(reference);
        Ok(GitResolvePullRequestResult {
            pull_request: resolved_request(&self.host()?.get(self.git.cwd(), &reference).await?),
        })
    }
    async fn pr_base(
        &self,
        branch: &str,
        upstream: Option<&str>,
        context: &BranchHeadContext,
    ) -> Result<(String, String), GitError> {
        if let Some(reference) = self
            .git
            .read_config_value(&format!("branch.{branch}.t3-merge-base-ref"))
            .await?
            && (reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/"))
            && let Some(base) = self.git.resolve_branch_name(&reference).await?
        {
            return Ok((base, reference));
        }
        if let Some(configured) = self.git.read_branch_base(branch).await? {
            let local_ref = if configured.starts_with("refs/") {
                configured.clone()
            } else {
                format!("refs/heads/{configured}")
            };
            if let Some(base) = self.git.resolve_branch_name(&local_ref).await? {
                let shadow = configured.contains('/')
                    && !configured.starts_with("refs/")
                    && self
                        .git
                        .resolve_branch_name(&format!("refs/remotes/{configured}"))
                        .await?
                        .is_some();
                return Ok((
                    base.clone(),
                    if configured.starts_with("refs/") || shadow {
                        local_ref
                    } else {
                        base
                    },
                ));
            }
            if configured.contains('/') && !configured.starts_with("refs/") {
                let reference = format!("refs/remotes/{configured}");
                if let Some(base) = self.git.resolve_branch_name(&reference).await? {
                    return Ok((base, reference));
                }
            }
            if let Some(base) = self.git.resolve_branch_name(&configured).await? {
                return Ok((base.clone(), base));
            }
        }
        if !context.cross_repository
            && let Some(upstream) = upstream
        {
            let base = context
                .remote_name
                .as_ref()
                .and_then(|remote| upstream.strip_prefix(&format!("{remote}/")))
                .or_else(|| upstream.split_once('/').map(|(_, base)| base))
                .unwrap_or(upstream);
            if !base.is_empty() && base != branch {
                return Ok((base.into(), base.into()));
            }
        }
        if let Some(base) = self
            .host()?
            .default_branch(self.git.cwd())
            .await
            .ok()
            .flatten()
        {
            return Ok((base.clone(), base));
        }
        if let Some(remote) = self.git.primary_remote_name().await.ok().flatten()
            && let Some(base) = self.git.default_branch_name(&remote).await.ok().flatten()
        {
            return Ok((base.clone(), base));
        }
        Ok(("main".into(), "main".into()))
    }
    async fn run_pr_step(
        &self,
        fallback: Option<&str>,
        content: Option<&PullRequestContent>,
    ) -> Result<Value, GitError> {
        let host = self.host()?;
        let details = self.git.status_details().await?;
        let branch = details.branch.as_deref().or(fallback).ok_or_else(|| {
            self.git.command_error(
                "runPrStep",
                "Cannot create a pull request from detached HEAD.",
            )
        })?;
        if !details.has_upstream {
            return Err(self.git.command_error(
                "runPrStep",
                "Current branch has not been pushed. Push before creating a PR.",
            ));
        }
        let context = self
            .branch_head_context(branch, details.upstream_ref.as_deref(), None)
            .await?;
        if let Some(request) = self.find_request(&context, true).await? {
            return Ok(pr_step("opened_existing", &request));
        }
        let (base, _) = self
            .pr_base(branch, details.upstream_ref.as_deref(), &context)
            .await?;
        let generated;
        let content = if let Some(content) = content {
            content
        } else {
            let generator = self.text_generator.as_ref().ok_or_else(|| {
                self.git
                    .command_error("runPrStep", "Pull request title and body are required.")
            })?;
            generated = cancellable(
                &self.git.cancellation,
                generator.pull_request(&self.git, &base, &context.head_branch),
            )
            .await?;
            &generated
        };
        let directory = tempfile::Builder::new()
            .prefix("t3code-pr-body-")
            .tempdir()?;
        let path = directory.path().join("body.md");
        tokio::fs::write(&path, &content.body).await?;
        host.create(
            self.git.cwd(),
            &base,
            &context.preferred_head_selector,
            &content.title,
            &path,
        )
        .await?;
        let request = self.find_request(&context, true).await.ok().flatten();
        Ok(request.map_or_else(|| json!({"status":"created","baseBranch":base,"headBranch":context.head_branch,"title":content.title}), |request| pr_step("created", &request)))
    }
    pub async fn run_stacked_action(
        &self,
        input: &GitRunStackedActionInput,
        pr_content: Option<&PullRequestContent>,
        progress: Option<ActionProgress>,
    ) -> Result<GitRunStackedActionResult, GitError> {
        if Path::new(&input.cwd.0) != self.git.cwd() {
            return Err(self.git.command_error(
                "runStackedAction",
                "Action cwd does not match this workflow's checkout.",
            ));
        }
        let _lease = self.git.checkout_lock().await?;
        let mut phase: Option<&str> = None;
        let emit = |payload: Value| {
            if let Some(reporter) = &progress {
                let mut value =
                    json!({"actionId":input.action_id,"cwd":input.cwd,"action":input.action});
                if let (Some(value), Some(payload)) = (value.as_object_mut(), payload.as_object()) {
                    value.extend(payload.clone());
                }
                if let Ok(event) = serde_json::from_value(value) {
                    reporter(event);
                }
            }
        };
        let result = async {
            let details = self.git.status_details().await?;
            let commit_action = matches!(input.action, GitStackedAction::Commit | GitStackedAction::CommitPush | GitStackedAction::CommitPushPr);
            let push_action = matches!(input.action, GitStackedAction::Push | GitStackedAction::CommitPush | GitStackedAction::CommitPushPr) || input.action == GitStackedAction::CreatePr && (!details.has_upstream || details.ahead_count > 0);
            let pr_action = matches!(input.action, GitStackedAction::CreatePr | GitStackedAction::CommitPushPr);
            let feature = input.feature_branch.flatten() == Some(true);
            if feature && !commit_action { return Err(self.git.command_error("runStackedAction", "Feature-branch checkout is only supported for commit actions.")); }
            let phases: Vec<_> = [(feature,"branch"),(commit_action,"commit"),(push_action,"push"),(pr_action,"pr")].into_iter().filter_map(|(enabled, phase)| enabled.then_some(phase)).collect();
            emit(json!({"kind":"action_started","phases":phases}));
            if !feature && push_action && details.branch.is_none() { return Err(self.git.command_error("runStackedAction", "Cannot push from detached HEAD.")); }
            if !feature && pr_action && details.branch.is_none() { return Err(self.git.command_error("runStackedAction", "Cannot create a pull request from detached HEAD.")); }
            let paths = input.file_paths.as_ref().and_then(Option::as_ref).map(Vec::as_slice);
            let supplied = input.commit_message.as_ref().and_then(Option::as_ref).map(|value| value.0.as_str());
            let generated = if commit_action && supplied.is_none() {
                match self.git.prepare_commit_context(paths, true).await? {
                    Some(context) => Some(cancellable(&self.git.cancellation, self.text_generator.as_ref().ok_or_else(|| self.git.command_error("runCommitStep", "A commit message or text generator is required."))?.commit_message(&self.git, &context)).await?),
                    None => None,
                }
            } else { None };
            let message = supplied.or(generated.as_deref());
            let mut branch_step = json!({"status":"skipped_not_requested"}); let mut branch = details.branch;
            let mut prepared = false;
            if feature {
                phase = Some("branch"); emit(json!({"kind":"phase_started","phase":"branch","label":"Preparing feature branch..."}));
                let message = message.and_then(parse_commit_message).ok_or_else(|| self.git.command_error("runFeatureBranchStep", "A commit message is required."))?;
                if self.git.prepare_commit_context(paths, false).await?.is_none() { return Err(self.git.command_error("runFeatureBranchStep", "Cannot create a feature branch because there are no changes to commit.")); }
                prepared = true;
                let name = available_feature_branch(&self.git.list_local_branch_names().await?, &message.0);
                self.git.create_ref(&name, false).await?; self.git.switch_ref_unlocked(&name).await?;
                branch_step = json!({"status":"created","name":name}); branch = Some(name);
            }
            if pr_action && !feature && let Some(branch) = branch.as_deref() && let Some(target) = self.git.resolve_push_target(branch, details.has_upstream).await? {
                let context = self.branch_head_context(branch, Some(&format!("{}/{}", target.remote_name, target.branch_name)), Some(&target.remote_name)).await?;
                if !context.cross_repository {
                    let (base, _) = self.pr_base(branch, details.upstream_ref.as_deref(), &context).await?;
                    if base == target.branch_name && self.find_request(&context, true).await?.is_none() { return Err(self.git.command_error("runStackedAction", format!("{base} is the base branch. Check out a feature branch to create a {}.", self.host()?.short_label()))); }
                }
            }
            let commit = if commit_action {
                phase = Some("commit");
                if !prepared && self.git.prepare_commit_context(paths, false).await?.is_none() { json!({"status":"skipped_no_changes"}) } else {
                    let (subject, body) = message.and_then(parse_commit_message).ok_or_else(|| self.git.command_error("runCommitStep", "A commit message is required."))?;
                    emit(json!({"kind":"phase_started","phase":"commit","label":"Committing..."}));
                    let commit_progress = progress.as_ref().map(|reporter| {
                        let reporter = reporter.clone();
                        let input = input.clone();
                        let current_hook = std::sync::Mutex::new(None::<String>);
                        Arc::new(move |event: crate::CommitProgress| {
                            let payload = match event {
                                crate::CommitProgress::HookStarted { hook_name } => {
                                    if let Ok(mut current) = current_hook.lock() { *current = Some(hook_name.clone()); }
                                    json!({"kind":"hook_started","hookName":hook_name})
                                }
                                crate::CommitProgress::HookFinished { hook_name, exit_code, duration_ms } => {
                                    if let Ok(mut current) = current_hook.lock() && current.as_deref() == Some(&hook_name) { *current = None; }
                                    json!({"kind":"hook_finished","hookName":hook_name,"exitCode":exit_code,"durationMs":duration_ms})
                                }
                                crate::CommitProgress::Output { stream, text } => {
                                    let text: String = text.trim().chars().take(500).collect();
                                    if text.is_empty() { return; }
                                    let current = current_hook.lock().ok().and_then(|current| current.clone());
                                    json!({"kind":"hook_output","hookName":current,"stream":stream,"text":text.trim_end()})
                                }
                            };
                            let mut value = json!({"actionId":input.action_id,"cwd":input.cwd,"action":input.action});
                            if let (Some(value),Some(payload)) = (value.as_object_mut(),payload.as_object()) { value.extend(payload.clone()); }
                            if let Ok(event) = serde_json::from_value(value) { reporter(event); }
                        }) as crate::CommitReporter
                    });
                    let sha = self.git.commit(&subject, &body, CommitOptions { stage: Some(paths.unwrap_or_default().to_vec()), timeout: Some(Duration::from_secs(600)), progress: commit_progress }).await?;
                    json!({"status":"created","commitSha":sha,"subject":subject})
                }
            } else { json!({"status":"skipped_not_requested"}) };
            let push = if push_action { phase = Some("push"); emit(json!({"kind":"phase_started","phase":"push","label":"Pushing..."})); serde_json::to_value(self.git.push_current_branch(branch.as_deref(), None).await?).map_err(|_| self.git.command_error("runStackedAction", "Could not encode push result."))? } else { json!({"status":"skipped_not_requested"}) };
            let pr = if pr_action { phase = Some("pr"); emit(json!({"kind":"phase_started","phase":"pr","label":format!("Preparing {}...",self.host()?.short_label())})); self.run_pr_step(branch.as_deref(), pr_content).await? } else { json!({"status":"skipped_not_requested"}) };
            let label = self.host.as_ref().map_or("PR", |host| host.short_label());
            let title = if pr["status"] == "created" || pr["status"] == "opened_existing" { format!("{} {label}{}", if pr["status"] == "created" { "Created" } else { "Opened" }, pr["number"].as_u64().map_or(String::new(), |number| format!(" #{number}"))) } else if push["status"] == "pushed" { format!("Pushed{}{}", commit["commitSha"].as_str().map_or(String::new(), |sha| format!(" {}", sha.chars().take(7).collect::<String>())), push["upstreamBranch"].as_str().or_else(|| push["branch"].as_str()).map_or(String::new(), |branch| format!(" to {branch}"))) } else if commit["status"] == "created" { format!("Committed {}", commit["commitSha"].as_str().unwrap_or("").chars().take(7).collect::<String>()) } else { "Done".into() };
            let cta = if input.action == GitStackedAction::Commit && commit["status"] == "created" { json!({"kind":"run_action","label":"Push","action":{"kind":"push"}}) } else if pr["url"].is_string() { json!({"kind":"open_pr","label":format!("View {label}"),"url":pr["url"]}) } else { json!({"kind":"none"}) };
            let mut toast = json!({"title":title,"cta":cta});
            if let Some(description) = pr["title"].as_str().or_else(|| commit["subject"].as_str()).filter(|text| !text.is_empty()) { toast["description"] = description.chars().take(72).collect::<String>().into(); }
            let result = json!({"action":input.action,"branch":branch_step,"commit":commit,"push":push,"pr":pr,"toast":toast});
            emit(json!({"kind":"action_finished","result":result}));
            serde_json::from_value(result).map_err(|_| self.git.command_error("runStackedAction", "Could not encode Git action result."))
        }.await;
        self.invalidate_status().await;
        if let Err(error) = &result {
            emit(json!({"kind":"action_failed","phase":phase,"message":error.to_string()}));
        }
        result
    }
    async fn configure_pr_upstream(
        &self,
        git: &Git,
        request: &ChangeRequest,
        branch: &str,
    ) -> Result<(), GitError> {
        let repository = request
            .head_repository_name_with_owner
            .as_ref()
            .and_then(Option::as_ref)
            .map(|value| value.0.as_str());
        let remote = if let Some(repository) = repository {
            let urls = self
                .host()?
                .clone_urls(git.cwd(), repository, &request.url)
                .await?;
            let origin = git.read_config_value("remote.origin.url").await?;
            let ssh = origin
                .as_ref()
                .is_some_and(|url| url.starts_with("git@") || url.starts_with("ssh://"));
            let url = if ssh { &urls.ssh_url.0 } else { &urls.url.0 };
            let owner = request
                .head_repository_owner_login
                .as_ref()
                .and_then(Option::as_ref)
                .map(|value| value.0.as_str())
                .or_else(|| repository.split('/').next())
                .unwrap_or("fork");
            git.ensure_remote(owner, url).await?
        } else {
            if request.is_cross_repository.flatten() == Some(true) {
                return Ok(());
            }
            git.primary_remote_name().await?.ok_or_else(|| {
                git.command_error(
                    "configurePullRequestHeadUpstream",
                    "No git remote is configured for this repository.",
                )
            })?
        };
        git.fetch_remote_tracking_branch(&remote, &request.head_ref_name.0)
            .await?;
        git.set_branch_upstream(branch, &remote, &request.head_ref_name.0)
            .await
    }
    async fn materialize_pr(&self, request: &ChangeRequest, local: &str) -> Result<(), GitError> {
        let result = async {
            if let Some(repository) = request
                .head_repository_name_with_owner
                .as_ref()
                .and_then(Option::as_ref)
            {
                let urls = self
                    .host()?
                    .clone_urls(self.git.cwd(), &repository.0, &request.url)
                    .await?;
                let origin = self.git.read_config_value("remote.origin.url").await?;
                let ssh = origin
                    .as_ref()
                    .is_some_and(|url| url.starts_with("git@") || url.starts_with("ssh://"));
                let owner = request
                    .head_repository_owner_login
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|owner| owner.0.as_str())
                    .or_else(|| repository.0.split('/').next())
                    .unwrap_or("fork");
                let remote = self
                    .git
                    .ensure_remote(owner, if ssh { &urls.ssh_url.0 } else { &urls.url.0 })
                    .await?;
                self.git
                    .fetch_remote_branch(&remote, &request.head_ref_name.0, local)
                    .await?;
                self.git
                    .set_branch_upstream(local, &remote, &request.head_ref_name.0)
                    .await
            } else {
                let head_ref = self.host()?.head_ref(request.number).ok_or_else(|| {
                    self.git.command_error(
                        "changeRequestHeadRef",
                        "This host publishes no ref for a change request's head.",
                    )
                })?;
                self.git.fetch_ref_into_branch(&head_ref, local).await
            }
        }
        .await;
        if result.is_err()
            && let Some(reference) = self.host()?.head_ref(request.number)
        {
            return self.git.fetch_ref_into_branch(&reference, local).await;
        }
        result
    }
    async fn refresh_pr_worktree(
        &self,
        git: &Git,
        request: &ChangeRequest,
        local: &str,
    ) -> Result<crate::CheckoutRefresh, GitError> {
        let before = git.resolve_commit("@{upstream}").await.ok();
        let _ = self.configure_pr_upstream(git, request, local).await;
        let target = if let Some(reference) = self.host()?.head_ref(request.number) {
            git.fetch_ref_commit(&reference).await.ok()
        } else {
            None
        };
        let target = if let Some(target) = target {
            target
        } else {
            let details = git.status_details().await?;
            let upstream = details
                .upstream_ref
                .filter(|reference| reference.ends_with(&format!("/{}", request.head_ref_name.0)))
                .ok_or_else(|| {
                    git.command_error(
                        "preparePullRequestThread",
                        "The pull request head could not be resolved for this checkout.",
                    )
                })?;
            git.resolve_commit(&upstream).await?
        };
        git.refresh_checked_out_branch(&target, before.as_deref())
            .await
    }
    pub async fn prepare_pull_request_thread(
        &self,
        input: &GitPreparePullRequestThreadInput,
    ) -> Result<GitPreparePullRequestThreadResult, GitError> {
        let result = self.prepare_pull_request_thread_inner(input).await;
        if let Ok(result) = &result {
            let _ = self
                .git
                .write_config_value(
                    &format!("branch.{}.t3-pull-request", result.branch.0),
                    &result.pull_request.url,
                )
                .await;
        }
        self.invalidate_status().await;
        result
    }
    async fn prepare_pull_request_thread_inner(
        &self,
        input: &GitPreparePullRequestThreadInput,
    ) -> Result<GitPreparePullRequestThreadResult, GitError> {
        let host = self.host()?;
        let reference = normalize_reference(&input.reference.0);
        let request = host.get(self.git.cwd(), &reference).await?;
        let resolved = resolved_request(&request);
        let thread_id = input
            .thread_id
            .as_ref()
            .and_then(Option::as_ref)
            .map(|id| id.0.as_str());
        if input.mode == GitPreparePullRequestThreadMode::Local {
            let _lease = self.git.checkout_lock().await?;
            let tracked = self
                .git
                .core_text(
                    "GitManager.preparePullRequestThread.trackedChanges",
                    &["status", "--porcelain", "--untracked-files=no"],
                    false,
                )
                .await?;
            if !tracked.trim().is_empty() {
                return Err(self.git.command_error("preparePullRequestThread", format!("This repository has uncommitted changes. Commit or stash them before checking out pull request #{} here, or check it out in its own folder.", request.number)));
            }
            host.checkout(self.git.cwd(), &reference).await?;
            let branch = self
                .current_branch()
                .await?
                .unwrap_or_else(|| request.head_ref_name.0.clone());
            let _ = self
                .configure_pr_upstream(&self.git, &request, &branch)
                .await;
            let on_target = if let Some(sha) = request.head_sha.as_ref().and_then(Option::as_ref) {
                match self.git.resolve_commit(&sha.0).await {
                    Ok(sha) => self
                        .git
                        .refresh_checked_out_branch(&sha, None)
                        .await
                        .is_ok_and(|result| result.on_target),
                    Err(_) => false,
                }
            } else {
                false
            };
            return Ok(GitPreparePullRequestThreadResult {
                pull_request: resolved,
                branch: Text(branch),
                worktree_path: None,
                is_on_pull_request_head: on_target,
            });
        }
        let local = pr_local_branch(&request, &self.git.list_local_branch_names().await?);
        let root = physical_path(self.git.cwd());
        let worktrees = self.git.list_worktrees().await?;
        let existing = worktrees
            .iter()
            .find(|tree| {
                tree.branch.as_deref() == Some(&format!("refs/heads/{local}"))
                    && tree.prunable.is_none()
            })
            .or_else(|| {
                worktrees.iter().find(|tree| {
                    tree.branch.as_deref()
                        == Some(&format!("refs/heads/{}", request.head_ref_name.0))
                        && physical_path(&tree.path) != root
                        && tree.prunable.is_none()
                })
            });
        if let Some(existing) = existing {
            if physical_path(&existing.path) == root {
                return Err(self.git.command_error("preparePullRequestThread", "This PR branch is already checked out in the main repo. Use Local, or switch the main repo off that branch before creating a worktree thread."));
            }
            let matching = existing.branch.as_deref() == Some(&format!("refs/heads/{local}"));
            let refreshed = if matching {
                self.refresh_pr_worktree(&self.git.at(&existing.path), &request, &local)
                    .await
                    .ok()
            } else {
                None
            };
            if refreshed.as_ref().is_some_and(|result| result.moved)
                && let Some(thread) = thread_id
            {
                let _ = self.setup_worktree(thread, &existing.path).await;
            }
            return Ok(GitPreparePullRequestThreadResult {
                pull_request: resolved,
                branch: Text(local),
                worktree_path: Some(Text(existing.path.to_string_lossy().into_owned())),
                is_on_pull_request_head: refreshed.is_some_and(|result| result.on_target),
            });
        }
        let existed = self.git.ref_exists(&format!("refs/heads/{local}")).await?;
        if !existed {
            self.materialize_pr(&request, &local).await?;
        }
        let tree = self
            .create_worktree(&VcsCreateWorktreeInput {
                cwd: Text(self.git.cwd().to_string_lossy().into()),
                ref_name: Text(local.clone()),
                new_ref_name: None,
                base_ref_name: None,
                path: None,
            })
            .await?
            .worktree;
        let path = PathBuf::from(tree.path.0);
        let on_target = if existed {
            self.refresh_pr_worktree(&self.git.at(&path), &request, &local)
                .await
                .is_ok_and(|result| result.on_target)
        } else {
            let _ = self
                .configure_pr_upstream(&self.git.at(&path), &request, &local)
                .await;
            true
        };
        if let Some(thread) = thread_id {
            let _ = self.setup_worktree(thread, &path).await;
        }
        Ok(GitPreparePullRequestThreadResult {
            pull_request: resolved,
            branch: Text(local),
            worktree_path: Some(Text(path.to_string_lossy().into_owned())),
            is_on_pull_request_head: on_target,
        })
    }
}
fn normalize_reference(reference: &str) -> String {
    let value = reference.trim();
    value
        .strip_prefix('#')
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or(value)
        .into()
}
fn resolved_request(request: &ChangeRequest) -> GitResolvedPullRequest {
    GitResolvedPullRequest {
        number: request.number,
        title: request.title.clone(),
        url: request.url.clone(),
        base_branch: request.base_ref_name.clone(),
        head_branch: request.head_ref_name.clone(),
        state: match request.state {
            ChangeRequestState::Open => GitPullRequestState::Open,
            ChangeRequestState::Closed => GitPullRequestState::Closed,
            ChangeRequestState::Merged => GitPullRequestState::Merged,
        },
    }
}
fn status_pr(request: &ChangeRequest) -> VcsStatusChangeRequest {
    VcsStatusChangeRequest {
        number: request.number,
        title: request.title.clone(),
        url: request.url.clone(),
        base_ref: request.base_ref_name.clone(),
        head_ref: request.head_ref_name.clone(),
        state: match request.state {
            ChangeRequestState::Open => VcsStatusChangeRequestState::Open,
            ChangeRequestState::Closed => VcsStatusChangeRequestState::Closed,
            ChangeRequestState::Merged => VcsStatusChangeRequestState::Merged,
        },
        is_draft: request.is_draft,
        updated_at: match &request.updated_at {
            t3_contracts::source_control::WireOption::Some { value } => Some(Some(value.0.clone())),
            _ => None,
        },
    }
}
fn pr_step(status: &str, request: &ChangeRequest) -> Value {
    json!({"status":status,"url":request.url,"number":request.number,"baseBranch":request.base_ref_name,"headBranch":request.head_ref_name,"title":request.title})
}
fn pr_local_branch(request: &ChangeRequest, branches: &[String]) -> String {
    if request.is_cross_repository.flatten() != Some(true) {
        return request.head_ref_name.0.clone();
    }
    let head = sanitize_feature_branch(&request.head_ref_name.0);
    let head = head.strip_prefix("feature/").unwrap_or(&head);
    let creatable = |candidate: &str| {
        let parts: Vec<_> = candidate.split('/').collect();
        !(1..parts.len()).any(|index| branches.contains(&parts[..index].join("/")))
            && !branches
                .iter()
                .any(|branch| branch.starts_with(&format!("{candidate}/")))
    };
    let preferred = format!("t3code/pr-{}/{head}", request.number);
    if creatable(&preferred) {
        return preferred;
    }
    let flat = format!("t3code-pr-{}-{}", request.number, head.replace('/', "-"));
    for attempt in 1.. {
        let candidate = if attempt == 1 {
            flat.clone()
        } else {
            format!("{flat}-{attempt}")
        };
        if creatable(&candidate) {
            return candidate;
        }
    }
    flat
}
