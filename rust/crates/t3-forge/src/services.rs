//! Application operations for source-control transports. Callers authorize paths before dispatch.
use crate::{ForgeError, forge::*, registry::ForgeRegistry};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString as Text, source_control::*};

#[derive(Debug, thiserror::Error)]
#[error("{detail}", detail = .wire.detail)]
pub struct RepositoryError {
    pub wire: Box<SourceControlRepositoryError>,
    pub git_reason: Option<t3_contracts::git::GitCommandFailureReason>,
}
type RepositoryResult<T> = std::result::Result<T, RepositoryError>;
fn repository_error(provider: &str, operation: &str, detail: &str) -> RepositoryError {
    RepositoryError {
        wire: Box::new(SourceControlRepositoryError {
            _tag: SourceControlRepositoryErrorTag::SourceControlRepositoryError,
            provider: SourceControlProviderKind(provider.into()),
            operation: operation.into(),
            detail: detail.into(),
            cause: None,
        }),
        git_reason: None,
    }
}
fn git_error(provider: &str, operation: &str, error: t3_git::GitError) -> RepositoryError {
    if matches!(&error,t3_git::GitError::Command(error) if error.detail == "Git command timed out.")
    {
        return repository_error(
            provider,
            operation,
            "The source control operation timed out.",
        );
    }
    let reason = match error {
        t3_git::GitError::Command(error) => error.reason,
        t3_git::GitError::Exit { stderr, .. } => t3_git::classify_git_failure(&stderr),
        _ => None,
    };
    RepositoryError {
        git_reason: reason,
        ..repository_error(
            provider,
            operation,
            "The source control operation could not be completed.",
        )
    }
}

fn info(provider: &str, urls: &SourceControlRepositoryCloneUrls) -> SourceControlRepositoryInfo {
    SourceControlRepositoryInfo {
        provider: SourceControlProviderKind(provider.into()),
        name_with_owner: urls.name_with_owner.clone(),
        url: urls.url.clone(),
        ssh_url: urls.ssh_url.clone(),
    }
}
fn protocol_url(
    urls: &SourceControlRepositoryCloneUrls,
    protocol: Option<&SourceControlCloneProtocol>,
) -> String {
    if protocol == Some(&SourceControlCloneProtocol::Https) {
        urls.url.0.clone()
    } else {
        urls.ssh_url.0.clone()
    }
}
fn redacted_url(value: &str) -> String {
    if let Ok(mut url) = url::Url::parse(value) {
        if url.username().is_empty()
            && url.password().is_none_or(str::is_empty)
            && url.query().is_none_or(str::is_empty)
        {
            return value.into();
        }
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.to_string()
    } else {
        value.into()
    }
}
fn clone_url(value: &str) -> String {
    static PATHLESS_QUERY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^([a-zA-Z][a-zA-Z0-9+.-]*://[^/?#]+)\?").unwrap()
    });
    PATHLESS_QUERY.replace(value, "$1/?").into_owned()
}
fn https_url(value: &str) -> Option<String> {
    let value = value.trim();
    let (host, path, scp) = if value.starts_with("ssh://") {
        static SSH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^ssh://(?:[^/\s]*@)?(\[[^\]@/\s]+\]|[^@:/\s]+)(:\d+)?/(.+)$")
                .expect("SSH clone expression")
        });
        let parsed = SSH.captures(value)?;
        if parsed.get(2).is_some() {
            return None;
        }
        (parsed[1].to_string(), parsed[3].to_string(), false)
    } else {
        if value.contains(['\\', '\0']) || value.contains("://") {
            return None;
        }
        static SCP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^(?:[^@/\s]+@)?(\[[^\]@/\s]+\]|[^:/\s\[\]]+):(.+)$")
                .expect("SCP clone expression")
        });
        let parsed = SCP.captures(value)?;
        if parsed[2].starts_with('/') {
            return None;
        }
        (parsed[1].into(), parsed[2].into(), true)
    };
    if host.eq_ignore_ascii_case("ssh.dev.azure.com") {
        let parts: Vec<_> = path.split('/').collect();
        return match parts.as_slice() {
            ["v3", org, project, repo]
                if parts[1..]
                    .iter()
                    .all(|p| !p.is_empty() && !p.contains(['?', '#'])) =>
            {
                Some(format!("https://dev.azure.com/{org}/{project}/_git/{repo}"))
            }
            _ => None,
        };
    }
    let path = if scp {
        path.split('/')
            .map(|p| {
                url::form_urlencoded::byte_serialize(p.as_bytes())
                    .collect::<String>()
                    .replace('+', "%20")
            })
            .collect::<Vec<_>>()
            .join("/")
    } else {
        path
    };
    Some(format!("https://{host}/{path}"))
}
fn ssh_rejected(error: &RepositoryError) -> bool {
    matches!(
        error.git_reason,
        Some(
            t3_contracts::git::GitCommandFailureReason::AuthenticationFailed
                | t3_contracts::git::GitCommandFailureReason::HostKeyUnverified
        )
    )
}
fn known_provider(provider: &str, operation: &str) -> RepositoryResult<()> {
    if provider == "unknown" {
        Err(repository_error(
            provider,
            operation,
            "Choose a source control provider before continuing.",
        ))
    } else {
        Ok(())
    }
}
fn validate_remote(remote: &str, provider: &str, operation: &str) -> RepositoryResult<()> {
    if remote.trim().is_empty() || remote.contains(['\0', '\r', '\n']) {
        return Err(repository_error(
            provider,
            operation,
            "Enter a valid repository path or clone URL before cloning.",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct PreparedClone {
    pub destination_path: PathBuf,
    pub remote_url: String,
    pub clone_url: String,
    pub repository: Option<SourceControlRepositoryInfo>,
}
impl std::fmt::Debug for PreparedClone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedClone")
            .field("destination_path", &self.destination_path)
            .field("remote_url", &self.remote_url)
            .field("repository", &self.repository)
            .finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub struct CloneOptions {
    pub timeout: Option<Duration>,
    pub on_progress: Option<Arc<dyn Fn(crate::clone_progress::GitCloneProgressLine) + Send + Sync>>,
}
impl Default for CloneOptions {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(120)),
            on_progress: None,
        }
    }
}

pub struct SourceControlRepositoryService {
    registry: Arc<ForgeRegistry>,
    git: t3_git::Git,
    cwd: PathBuf,
    home: Option<PathBuf>,
}
impl SourceControlRepositoryService {
    pub fn new(registry: Arc<ForgeRegistry>, git: t3_git::Git, cwd: PathBuf) -> Self {
        Self {
            registry,
            git,
            cwd,
            home: None,
        }
    }
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
        self
    }
    fn destination(&self, path: &Path) -> RepositoryResult<PathBuf> {
        let text = path.to_string_lossy();
        let text = text.trim();
        if text.is_empty() || text.contains('\0') {
            return Err(repository_error(
                "unknown",
                "cloneRepository",
                "Choose a destination path before cloning.",
            ));
        }
        let path = if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
            let home = self.home.as_ref().ok_or_else(|| {
                repository_error(
                    "unknown",
                    "cloneRepository",
                    "The server home directory is unavailable.",
                )
            })?;
            home.join(text.get(2..).unwrap_or(""))
        } else {
            PathBuf::from(text)
        };
        let path = if path.is_absolute() {
            path
        } else {
            self.cwd.join(path)
        };
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                other => normalized.push(other),
            }
        }
        if !normalized.is_absolute() || normalized.file_name().is_none() {
            return Err(repository_error(
                "unknown",
                "cloneRepository",
                "Choose a repository directory as the destination.",
            ));
        }
        Ok(normalized)
    }
    pub async fn lookup_repository(
        &self,
        input: SourceControlRepositoryLookupInput,
    ) -> RepositoryResult<SourceControlRepositoryInfo> {
        known_provider(&input.provider.0, "lookupRepository")?;
        let provider = &input.provider.0;
        let forge = self.registry.get(provider).map_err(|_| {
            repository_error(
                provider,
                "lookupRepository",
                "The source control provider is unavailable.",
            )
        })?;
        let urls = crate::github::transport::with_interactive(
            forge.get_repository_clone_urls(CloneUrlsInput {
                checkout: CheckoutInput {
                    cwd: input
                        .cwd
                        .flatten()
                        .map(|s| s.0.into())
                        .unwrap_or_else(|| self.cwd.clone()),
                    context: None,
                },
                repository: input.repository.0.trim().into(),
                host: None,
            }),
        )
        .await
        .map_err(|error| {
            repository_error(
                provider,
                "lookupRepository",
                error
                    .repository_hint()
                    .unwrap_or("The source control operation could not be completed."),
            )
        })?;
        Ok(info(provider, &urls))
    }
    pub async fn prepare_clone(
        &self,
        input: SourceControlCloneRepositoryInput,
    ) -> RepositoryResult<PreparedClone> {
        let destination = self.destination(Path::new(&input.destination_path.0))?;
        match tokio::fs::read_dir(&destination).await {
            Ok(mut entries) => {
                if entries
                    .next_entry()
                    .await
                    .map_err(|_| {
                        repository_error(
                            "unknown",
                            "cloneRepository",
                            "The destination could not be inspected.",
                        )
                    })?
                    .is_some()
                {
                    return Err(repository_error(
                        "unknown",
                        "cloneRepository",
                        "Destination path already exists and is not empty.",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir_all(destination.parent().ok_or_else(|| {
                    repository_error("unknown", "cloneRepository", "Invalid clone destination.")
                })?)
                .await
                .map_err(|_| {
                    repository_error(
                        "unknown",
                        "cloneRepository",
                        "The destination parent could not be created.",
                    )
                })?;
            }
            Err(_) => {
                return Err(repository_error(
                    "unknown",
                    "cloneRepository",
                    "Destination path already exists and is not a directory.",
                ));
            }
        }
        let provider = input.provider.flatten();
        let mut remote = input.remote_url.flatten().map(|s| s.0.trim().to_string());
        let mut repository = None;
        if let (Some(provider), Some(name)) = (provider.as_ref(), input.repository.flatten()) {
            let found = self
                .lookup_repository(SourceControlRepositoryLookupInput {
                    provider: provider.clone(),
                    repository: name,
                    cwd: Some(Some(Text(
                        destination
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    ))),
                })
                .await?;
            let urls = SourceControlRepositoryCloneUrls {
                name_with_owner: found.name_with_owner.clone(),
                url: found.url.clone(),
                ssh_url: found.ssh_url.clone(),
            };
            remote = Some(protocol_url(
                &urls,
                input.protocol.as_ref().and_then(|p| p.as_ref()),
            ));
            repository = Some(found);
        }
        let remote = remote.ok_or_else(|| {
            repository_error(
                provider.as_ref().map_or("unknown", |p| &p.0),
                "cloneRepository",
                "Enter a repository path or clone URL before cloning.",
            )
        })?;
        validate_remote(
            &remote,
            provider.as_ref().map_or("unknown", |p| &p.0),
            "cloneRepository",
        )?;
        Ok(PreparedClone {
            destination_path: destination,
            remote_url: redacted_url(&remote),
            clone_url: remote,
            repository,
        })
    }
    async fn run_clone(
        &self,
        prepared: &PreparedClone,
        url: &str,
        options: CloneOptions,
        provider: &str,
    ) -> RepositoryResult<()> {
        let destination = prepared
            .destination_path
            .file_name()
            .ok_or_else(|| {
                repository_error(provider, "cloneRepository", "Invalid clone destination.")
            })?
            .to_string_lossy();
        let parent = prepared.destination_path.parent().ok_or_else(|| {
            repository_error(provider, "cloneRepository", "Invalid clone destination.")
        })?;
        let url = clone_url(url);
        let lines = Arc::new(std::sync::Mutex::new(
            crate::clone_progress::Lines::default(),
        ));
        let callback = options.on_progress.clone();
        let on_stderr_chunk = options
            .on_progress
            .map(|callback| -> t3_process::OutputCallback {
                let lines = lines.clone();
                Arc::new(move |bytes| {
                    let progress = lines.lock().unwrap_or_else(|e| e.into_inner()).push(bytes);
                    for line in progress {
                        callback(line);
                    }
                })
            });
        let result = self
            .git
            .at(parent)
            .execute_command(
                "SourceControlRepositoryService.cloneRepository",
                &["clone", "--progress", "--", &url, &destination],
                t3_git::ExecuteOptions {
                    timeout: options.timeout,
                    on_stderr_chunk,
                    max_output_bytes: 256 * 1024,
                    append_truncation_marker: true,
                    env: [
                        ("GIT_PROGRESS_DELAY".into(), Some("0".into())),
                        ("GIT_TERMINAL_PROMPT".into(), Some("0".into())),
                        ("LANGUAGE".into(), Some("C".into())),
                    ]
                    .into(),
                    ..Default::default()
                },
            )
            .await;
        if let Some(callback) = callback {
            let progress = lines.lock().unwrap_or_else(|e| e.into_inner()).push(b"\n");
            for line in progress {
                callback(line);
            }
        }
        result.map_err(|e| git_error(provider, "cloneRepository", e))?;
        Ok(())
    }
    pub async fn clone_repository(
        &self,
        input: SourceControlCloneRepositoryInput,
        options: CloneOptions,
    ) -> RepositoryResult<SourceControlCloneRepositoryResult> {
        let prepared = self.prepare_clone(input.clone()).await?;
        let provider = input
            .provider
            .flatten()
            .map_or_else(|| "unknown".into(), |p| p.0);
        let remote = match self
            .run_clone(&prepared, &prepared.clone_url, options.clone(), &provider)
            .await
        {
            Ok(()) => prepared.remote_url.clone(),
            Err(error) => {
                let fallback = if input.protocol.flatten() == Some(SourceControlCloneProtocol::Ssh)
                {
                    None
                } else if prepared
                    .repository
                    .as_ref()
                    .is_some_and(|r| r.ssh_url.0 == prepared.clone_url)
                {
                    prepared.repository.as_ref().map(|r| r.url.0.clone())
                } else {
                    https_url(&prepared.clone_url)
                };
                let Some(fallback) = fallback.filter(|_| ssh_rejected(&error)) else {
                    return Err(error);
                };
                self.run_clone(&prepared, &fallback, options, &provider)
                    .await?;
                redacted_url(&fallback)
            }
        };
        self.registry.invalidate_detection();
        Ok(SourceControlCloneRepositoryResult {
            cwd: Text(prepared.destination_path.to_string_lossy().into()),
            remote_url: Text(remote),
            repository: prepared.repository,
        })
    }
    pub async fn discard_clone(&self, destination: &Path) -> RepositoryResult<()> {
        let destination = self.destination(destination)?;
        let mut entries = match tokio::fs::read_dir(&destination).await {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                return Err(repository_error(
                    "unknown",
                    "discardClone",
                    "The clone destination could not be inspected.",
                ));
            }
        };
        let mut nonempty = false;
        let mut has_git = false;
        if let Some(entries) = &mut entries {
            while let Some(entry) = entries.next_entry().await.map_err(|_| {
                repository_error(
                    "unknown",
                    "discardClone",
                    "The clone destination could not be inspected.",
                )
            })? {
                nonempty = true;
                has_git |= entry.file_name() == ".git";
            }
        }
        if nonempty && !has_git {
            return Err(repository_error(
                "unknown",
                "discardClone",
                "Destination path contains files that are not from the clone.",
            ));
        }
        drop(entries);
        match tokio::fs::symlink_metadata(&destination).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(repository_error(
                    "unknown",
                    "discardClone",
                    "The clone destination cannot be a directory link.",
                ));
            }
            Ok(_) => tokio::fs::remove_dir_all(&destination).await.map_err(|_| {
                repository_error(
                    "unknown",
                    "discardClone",
                    "The partial clone could not be removed.",
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(repository_error(
                    "unknown",
                    "discardClone",
                    "The clone destination could not be inspected.",
                ));
            }
        }
        tokio::fs::create_dir_all(&destination).await.map_err(|_| {
            repository_error(
                "unknown",
                "discardClone",
                "The clone destination could not be recreated.",
            )
        })
    }
    pub async fn publish_repository(
        &self,
        input: SourceControlPublishRepositoryInput,
    ) -> RepositoryResult<SourceControlPublishRepositoryResult> {
        let provider = &input.provider.0;
        known_provider(provider, "publishRepository")?;
        let forge = self.registry.get(provider).map_err(|_| {
            repository_error(
                provider,
                "publishRepository",
                "The source control provider is unavailable.",
            )
        })?;
        let urls = forge
            .create_repository(CreateRepositoryInput {
                cwd: input.cwd.0.clone().into(),
                repository: input.repository.0.trim().into(),
                visibility: input.visibility,
            })
            .await
            .map_err(|error| {
                if matches!(&error,ForgeError::Provider{provider,operation,..}if provider.0=="cursor-origin"&&operation=="makeRepositoryPrivate"){
                    return repository_error(provider,"publishRepository",&format!("Created {}, but could not make it private, so your Cursor team can still see it. Set it to private on cursor.com.",crate::remote::transport_safe_error_value(&input.repository.0)));
                }
                repository_error(
                    provider,
                    "publishRepository",
                    error.repository_hint().unwrap_or("The repository could not be created."),
                )
            })?;
        let mut remote_url = protocol_url(&urls, input.protocol.as_ref().and_then(|p| p.as_ref()));
        validate_remote(&remote_url, provider, "publishRepository")?;
        let git = self.git.at(&input.cwd.0);
        let remote_name = git
            .ensure_remote(
                input
                    .remote_name
                    .as_ref()
                    .and_then(|n| n.as_ref())
                    .map_or("origin", |n| n.0.trim()),
                &remote_url,
            )
            .await
            .map_err(|e| git_error(provider, "publishRepository", e))?;
        let head = git
            .execute(
                &["rev-parse", "--verify", "HEAD"],
                t3_git::ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| git_error(provider, "publishRepository", e))?;
        let mut status = SourceControlPublishStatus::RemoteAdded;
        let branch = if head.code == Some(0) {
            let pushed = match git.push_current_branch(None, Some(&remote_name)).await {
                Ok(result) => result,
                Err(cause) => {
                    let error = git_error(provider, "publishRepository", cause);
                    if input.protocol.flatten() != Some(SourceControlCloneProtocol::Ssh)
                        && remote_url == urls.ssh_url.0
                        && ssh_rejected(&error)
                    {
                        git.execute(
                            &["remote", "set-url", "--", &remote_name, &urls.url.0],
                            Default::default(),
                        )
                        .await
                        .map_err(|_| error)?;
                        remote_url = urls.url.0.clone();
                        git.push_current_branch(None, Some(&remote_name))
                            .await
                            .map_err(|e| {
                                publish_error(
                                    provider,
                                    &urls,
                                    &remote_name,
                                    git_error(provider, "publishRepository", e),
                                )
                            })?
                    } else {
                        return Err(publish_error(provider, &urls, &remote_name, error));
                    }
                }
            };
            status = SourceControlPublishStatus::Pushed;
            pushed
                .branch
                .flatten()
                .map(|b| b.0)
                .unwrap_or_else(|| "main".into())
        } else {
            git.status_details()
                .await
                .ok()
                .and_then(|s| s.branch)
                .unwrap_or_else(|| "main".into())
        };
        self.registry.invalidate_detection();
        Ok(SourceControlPublishRepositoryResult {
            repository: info(provider, &urls),
            remote_name: Text(remote_name),
            remote_url: Text(redacted_url(&remote_url)),
            branch: Text(branch),
            status,
        })
    }
}
fn publish_error(
    provider: &str,
    urls: &SourceControlRepositoryCloneUrls,
    remote: &str,
    mut error: RepositoryError,
) -> RepositoryError {
    error.wire = repository_error(provider, "publishRepository", &format!("Created {} and added it as remote \"{}\", but the push failed. Check Git credentials and remote access, then push again.", crate::remote::transport_safe_error_value(&urls.name_with_owner.0), remote)).wire;
    error
}

pub struct SourceControlSignIn {
    registry: Arc<ForgeRegistry>,
}
#[derive(Debug, thiserror::Error)]
#[error("{detail}", detail = .wire.detail)]
pub struct SignInError {
    pub wire: Box<SourceControlSignInError>,
}
impl SourceControlSignIn {
    pub fn new(registry: Arc<ForgeRegistry>) -> Self {
        Self { registry }
    }
    fn error(input: &SourceControlSignInInput, error: ForgeError) -> SignInError {
        let (reason, detail) = match error {
            ForgeError::SignIn { reason, detail, .. } => (reason, detail),
            ForgeError::Unsupported { .. } => (
                SourceControlSignInFailure::Unsupported,
                "This server cannot sign in to this provider. Use a token or the host's CLI."
                    .into(),
            ),
            _ => (
                SourceControlSignInFailure::Failed,
                "The source control sign-in could not be completed.".into(),
            ),
        };
        SignInError {
            wire: Box::new(SourceControlSignInError {
                _tag: SourceControlSignInErrorTag::SourceControlSignInError,
                provider: input.provider.clone(),
                host: input.host.0.clone(),
                reason,
                detail,
                cause: None,
            }),
        }
    }
    pub async fn start(
        &self,
        input: SourceControlSignInInput,
    ) -> std::result::Result<SourceControlSignInPrompt, SignInError> {
        let forge = self
            .registry
            .get(&input.provider.0)
            .map_err(|e| Self::error(&input, e))?;
        forge
            .sign_in_start(input.host.0.clone())
            .await
            .map_err(|e| Self::error(&input, e))
    }
    pub async fn wait(
        &self,
        input: SourceControlSignInInput,
    ) -> std::result::Result<SourceControlSignInResult, SignInError> {
        let forge = self
            .registry
            .get(&input.provider.0)
            .map_err(|e| Self::error(&input, e))?;
        let result = forge
            .sign_in_wait(input.host.0.clone())
            .await
            .map_err(|e| Self::error(&input, e))?;
        self.registry.invalidate_detection();
        Ok(result)
    }
    pub async fn cancel(
        &self,
        input: SourceControlSignInInput,
    ) -> std::result::Result<(), SignInError> {
        let forge = self
            .registry
            .get(&input.provider.0)
            .map_err(|e| Self::error(&input, e))?;
        if !forge.supports("sign_in_cancel") {
            return Err(Self::error(
                &input,
                ForgeError::unsupported(&input.provider, "sign_in_cancel"),
            ));
        }
        forge
            .sign_in_cancel(input.host.0.clone())
            .await
            .map_err(|e| Self::error(&input, e))
    }
    pub async fn access(&self, input: SourceControlAppAccessInput) -> SourceControlAppAccess {
        let status = match self.registry.get(&input.provider.0) {
            Ok(forge) => forge
                .sign_in_access(input.host.0, input.repository.0)
                .await
                .unwrap_or(SourceControlAppAccessStatus::SignedOut),
            Err(_) => SourceControlAppAccessStatus::SignedOut,
        };
        SourceControlAppAccess { status }
    }
    pub async fn starred(&self, input: SourceControlStarredInput) -> bool {
        match self.registry.get(&input.provider.0) {
            Ok(forge) => forge
                .is_repository_starred(input.host.0, input.repository.0)
                .await
                .unwrap_or(false),
            Err(_) => false,
        }
    }
    pub async fn star(&self, input: SourceControlStarredInput) -> bool {
        match self.registry.get(&input.provider.0) {
            Ok(forge) => forge
                .star_repository(input.host.0, input.repository.0)
                .await
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::https_url;
    #[test]
    fn clone_fallback_preserves_paths_ports_and_azure_transport_mapping() {
        for (input, expected) in [
            (
                "git@github.com:acme/web.git",
                Some("https://github.com/acme/web.git"),
            ),
            (
                "git@github.com:acme/a b.git",
                Some("https://github.com/acme/a%20b.git"),
            ),
            (
                "ssh://git@github.com/acme/a%20b.git",
                Some("https://github.com/acme/a%20b.git"),
            ),
            ("ssh://git@github.com:2222/acme/web.git", None),
            (
                "git@ssh.dev.azure.com:v3/Fabrikam/Project1/fab_repo",
                Some("https://dev.azure.com/Fabrikam/Project1/_git/fab_repo"),
            ),
            (
                "ssh://user:pa@rt@ssh.dev.azure.com/v3/Fabrikam/Project1/fab_repo",
                Some("https://dev.azure.com/Fabrikam/Project1/_git/fab_repo"),
            ),
            ("C:\\work\\repo", None),
            ("git@[::1]:acme/web.git", Some("https://[::1]/acme/web.git")),
        ] {
            assert_eq!(https_url(input).as_deref(), expected, "{input}");
        }
    }
}

#[cfg(test)]
mod url_tests {
    use super::*;
    #[test]
    fn clone_operands_are_preserved_except_pathless_queries() {
        for value in [
            r"C:\repo",
            "local folder/repo",
            "https://host/a/../repo",
            "https://host/a/%2e%2e/repo",
            "ssh://host/a b.git",
            "https://host/repo#fragment",
            "https://host/repo?",
        ] {
            assert_eq!(clone_url(value), value);
            assert_eq!(redacted_url(value), value);
        }
        assert_eq!(
            clone_url("ssh://git@host/a b.git"),
            "ssh://git@host/a b.git"
        );
        assert_eq!(
            clone_url("https://host?token=value"),
            "https://host/?token=value"
        );
        assert_eq!(
            clone_url("https://host/a?token=value"),
            "https://host/a?token=value"
        );
        assert_eq!(
            redacted_url("https://user:private@host/repo?token=private#branch"),
            "https://host/repo#branch"
        );
    }
}
