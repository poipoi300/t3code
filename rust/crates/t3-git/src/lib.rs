//! Native installed-Git driver and compatible checkpoint plumbing.
//! This crate is server-only and does not target wasm32-unknown-unknown.

mod actions;
mod application;
mod broadcaster;
mod checkpoint;
mod core;
mod diagnostics;
mod diff;
mod followups;
mod progress;
mod registry;
mod remotes;
mod review;
mod safety;
mod setup_tracker;
mod workflow;
mod worktree;
mod worktree_progress;
pub use actions::{CheckoutRefresh, parse_commit_message};
pub use actions::{CommitOptions, PreparedCommit, PushTarget};
pub use application::{GitActionEffects, GitActionSubscription, GitApplicationService};
pub use broadcaster::{
    DEFAULT_VCS_FETCH_INTERVAL, DisabledVcsPolling, VcsBranchSubscription, VcsStatusBroadcaster,
    VcsStatusPolicy, VcsStatusSubscription, refresh_failure_delay,
};
pub use core::parse_remote_ref;
pub use core::{GitStatusDetails, LocalStatusOptions, RemoteStatusDetails};
pub use diagnostics::{
    CommandError, INDEX_LOCKED_DETAIL, VcsFailure, classify_git_failure, classify_process_failure,
};
pub use followups::{CreatedPullRequestKey, GitActionFollowups, GitActionHost};
pub use progress::{CommitProgress, CommitReporter};
pub use registry::{ProjectConfigWarning, VcsDriverRegistry, VcsSelection, resolve_vcs_kind};
pub use remotes::remote_repository_key;
pub use review::RangeContext;
pub use setup_tracker::{SetupTask, WorktreeSetupSubscription, WorktreeSetupTracker};
pub use workflow::{
    ActionProgress, BranchHeadContext, GitManager, GitWorkflowService, PullRequestContent,
    ReviewHost, WorkflowTextGenerator, WorktreeCreationSettings, WorktreeHost,
    available_feature_branch, matches_branch_head, sanitize_feature_branch,
};

pub use checkpoint::{CHECKPOINT_REF_PREFIX, checkpoint_ref};
pub use diff::{
    CheckpointContext, CheckpointRecord, CheckpointRun, CheckpointScope, DIFF_MAX_BYTES, DiffFile,
    DiffFormat, DiffOptions, TRUNCATED_MARKER, TurnDiff, parse_numstat, truncate_patch,
};
pub use safety::{RestoreParticipant, RestoreSafety, RestoreSession, is_restore_isolated};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};
pub use t3_process::{CancellationToken, OutputMode};
use t3_process::{Environment, ProcessError, ProcessOptions, ProcessOutput, run};
use tokio::sync::Semaphore;
pub use worktree::{Worktree, WorktreeOptions, WorktreeSubmodules};
pub use worktree_progress::{WorktreeCreationOptions, WorktreeProgress, WorktreeReporter};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error(transparent)]
    Vcs(#[from] Box<VcsFailure>),
    #[error(transparent)]
    Command(#[from] Box<CommandError>),
    #[error(transparent)]
    Process(#[from] ProcessError),
    #[error("Git exited with code {code:?}: {stderr}")]
    Exit { code: Option<i32>, stderr: String },
    #[error("Git filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Git output: {0}")]
    InvalidOutput(String),
    #[error("unsupported checkpoint: {0}")]
    Unsupported(String),
    #[error("unsafe restore: {0}")]
    UnsafeRestore(String),
    #[error("invalid Git argument: {0}")]
    InvalidArgument(String),
    #[error("checkpoint range is unavailable: {0}")]
    Unavailable(String),
}
impl From<CommandError> for GitError {
    fn from(error: CommandError) -> Self {
        Self::Command(Box::new(error))
    }
}
impl From<VcsFailure> for GitError {
    fn from(error: VcsFailure) -> Self {
        Self::Vcs(Box::new(error))
    }
}

#[derive(Clone)]
pub struct ExecuteOptions {
    pub unbounded_concurrency: bool,
    pub fallback_error_detail: Option<&'static str>,
    pub long_paths: bool,
    pub stdin: Option<Vec<u8>>,
    pub env: Environment,
    pub allow_nonzero: bool,
    pub timeout: Option<Duration>,
    pub max_output_bytes: usize,
    pub output_mode: OutputMode,
    pub append_truncation_marker: bool,
    pub capture: bool,
    pub on_stdout_chunk: Option<t3_process::OutputCallback>,
    pub on_stderr_chunk: Option<t3_process::OutputCallback>,
}
impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            unbounded_concurrency: false,
            fallback_error_detail: None,
            long_paths: false,
            stdin: None,
            env: Environment::new(),
            allow_nonzero: false,
            timeout: Some(Duration::from_secs(30)),
            max_output_bytes: 1_000_000,
            output_mode: OutputMode::Truncate,
            append_truncation_marker: false,
            capture: false,
            on_stdout_chunk: None,
            on_stderr_chunk: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub root: PathBuf,
    pub common_dir: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub index_status: char,
    pub worktree_status: char,
    pub conflicted: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub files: Vec<ChangedFile>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRef {
    pub name: String,
    pub object_id: String,
    pub upstream: Option<String>,
    pub current: bool,
}

#[derive(Clone)]
pub struct Git {
    cwd: PathBuf,
    program: OsString,
    environment: Environment,
    cancellation: CancellationToken,
    status_fetch_scope: Option<CancellationToken>,
    permits: Arc<Semaphore>,
    core: Arc<core::CoreState>,
    internal_paths: Option<(PathBuf, PathBuf)>,
    git_dir: Option<PathBuf>,
}

impl Git {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
        Self {
            cwd: cwd.into(),
            program: "git".into(),
            environment: Environment::new(),
            cancellation: CancellationToken::new(),
            status_fetch_scope: None,
            permits: PERMITS.get_or_init(|| Arc::new(Semaphore::new(8))).clone(),
            core: Arc::new(core::CoreState::default()),
            internal_paths: None,
            git_dir: None,
        }
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    pub(crate) fn with_git_dir(mut self, git_dir: PathBuf) -> Self {
        self.git_dir = Some(git_dir);
        self
    }
    pub fn with_internal_paths(mut self, state_dir: PathBuf, secrets_dir: PathBuf) -> Self {
        self.internal_paths = Some((state_dir, secrets_dir));
        self
    }
    pub(crate) fn is_internal_path(&self, path: &Path) -> bool {
        if path.starts_with("/dev/fd") {
            return true;
        }
        self.internal_paths
            .as_ref()
            .is_some_and(|(state, secrets)| {
                let state = std::fs::canonicalize(state).unwrap_or_else(|_| state.clone());
                let secrets = std::fs::canonicalize(secrets).unwrap_or_else(|_| secrets.clone());
                path.starts_with(secrets)
                    || path.parent() == Some(state.as_path())
                        && path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().contains(".sqlite"))
            })
    }
    pub fn at(&self, cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            ..self.clone()
        }
    }
    pub fn with_environment(mut self, environment: Environment) -> Self {
        merge_environment(&mut self.environment, &environment);
        self
    }
    pub fn with_program(mut self, program: impl Into<OsString>) -> Self {
        self.program = program.into();
        self
    }
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub async fn execute(
        &self,
        args: &[impl AsRef<std::ffi::OsStr>],
        options: ExecuteOptions,
    ) -> Result<ProcessOutput, GitError> {
        let _permit = if options.unbounded_concurrency
            || options
                .timeout
                .is_none_or(|timeout| timeout > Duration::from_secs(30))
        {
            None
        } else {
            Some(tokio::select! {
                _ = self.cancellation.cancelled() => return Err(ProcessError::Cancelled.into()),
                permit = self.permits.acquire() => permit.map_err(|_| GitError::InvalidOutput("Git process semaphore closed".into()))?,
            })
        };
        #[allow(unused_mut)]
        let mut args: Vec<OsString> = args.iter().map(|arg| arg.as_ref().to_owned()).collect();
        let mut retries = 0;
        #[cfg(target_os = "macos")]
        let mut fsync_fallback = false;
        loop {
            let mut process = ProcessOptions::new(self.program.clone());
            process.args = vec!["-C".into(), self.cwd.as_os_str().to_owned()];
            if let Some(directory) = &self.git_dir {
                process
                    .args
                    .extend(["--git-dir".into(), directory.as_os_str().to_owned()]);
            }
            process.args.extend(args.iter().cloned());
            // -C lets Git diagnose a deleted workspace without a spawn-cwd failure.
            process.env = git_environment(&self.environment, &options.env);
            if options.capture
                || options
                    .env
                    .get(std::ffi::OsStr::new("GIT_INDEX_FILE"))
                    .is_some_and(Option::is_some)
            {
                // Scratch-index and checkpoint writes must not invoke user index/ref hooks.
                append_config_env(&mut process.env, "core.hooksPath", "/dev/null");
            }
            if cfg!(windows) || options.long_paths {
                append_config_env(&mut process.env, "core.longpaths", "true");
            }
            process.termination_grace = Duration::from_secs(1);
            process.stdin = options.stdin.clone();
            process.timeout = options.timeout;
            process.max_output_bytes = options.max_output_bytes;
            process.output_mode = options.output_mode;
            process.truncated_marker = if options.append_truncation_marker {
                TRUNCATED_MARKER.into()
            } else {
                String::new()
            };
            process.check_exit = false;
            process.cancellation = self.cancellation.clone();
            process.on_stdout_chunk = options.on_stdout_chunk.clone();
            process.on_stderr_chunk = options.on_stderr_chunk.clone();
            let output = run(process).await?;
            if !options.allow_nonzero && output.code != Some(0) {
                let stderr = output.stderr_text();
                #[cfg(target_os = "macos")]
                let unsupported_fsync = stderr.to_lowercase().lines().any(|line| {
                    line.contains("fsync error on ") && line.ends_with(": operation not supported")
                        || line.contains("cannot update ref ")
                            && line.contains(": couldn't write '")
                            && line.ends_with(".lock'")
                });
                #[cfg(target_os = "macos")]
                if options.capture && !fsync_fallback && unsupported_fsync {
                    let mut changed = false;
                    for arg in &mut args {
                        if arg.as_os_str() == "core.fsyncMethod=fsync"
                            || arg.as_os_str() == "core.fsyncMethod=batch"
                        {
                            *arg = "core.fsyncMethod=writeout-only".into();
                            changed = true;
                        }
                    }
                    if changed {
                        fsync_fallback = true;
                        continue;
                    }
                }
                if options.capture
                    && options.on_stdout_chunk.is_none()
                    && retries < 2
                    && transient_exit(&stderr)
                {
                    retries += 1;
                    tokio::select! { _ = self.cancellation.cancelled() => return Err(ProcessError::Cancelled.into()), _ = tokio::time::sleep(Duration::from_millis(75)) => {} }
                    continue;
                }
                return Err(self.vcs_exit(
                    if options.capture {
                        "GitVcsDriver.checkpoints.captureCheckpoint"
                    } else {
                        "Git.execute"
                    },
                    args.len(),
                    &output,
                ));
            }
            return Ok(output);
        }
    }

    pub(crate) async fn text(&self, args: &[&str]) -> Result<String, GitError> {
        Ok(self
            .execute(args, ExecuteOptions::default())
            .await?
            .stdout_text())
    }

    pub async fn detect_repository(&self) -> Result<Option<Repository>, GitError> {
        let result = match self
            .execute(
                &[
                    "rev-parse",
                    "--is-inside-work-tree",
                    "--show-toplevel",
                    "--git-common-dir",
                ],
                ExecuteOptions {
                    env: env(&[("LC_ALL", "C")]),
                    allow_nonzero: true,
                    timeout: Some(Duration::from_secs(5)),
                    max_output_bytes: 16_384,
                    ..Default::default()
                },
            )
            .await
        {
            Err(GitError::Process(ProcessError::NotFound { .. })) => return Ok(None),
            result => result?,
        };
        let stdout = result.stdout_text();
        if result.code != Some(0) {
            if stdout.starts_with("false")
                || result.stderr_text().lines().any(|line| {
                    line.starts_with("fatal: not a git repository")
                        || line.starts_with("fatal: cannot use bare repository")
                })
                || !self.cwd.exists()
            {
                return Ok(None);
            }
            return Err(self.vcs_exit("GitVcsDriver.detectRepository", 4, &result));
        }
        let lines: Vec<_> = stdout.trim_end_matches(['\r', '\n']).split('\n').collect();
        if lines.first().map(|line| line.trim()) != Some("true") {
            return Ok(None);
        }
        let (root, common) = if lines.len() == 3 {
            (lines[1].trim().to_owned(), lines[2].trim().to_owned())
        } else {
            (
                self.text(&["rev-parse", "--show-toplevel"])
                    .await?
                    .trim_end_matches('\n')
                    .to_owned(),
                self.text(&["rev-parse", "--git-common-dir"])
                    .await?
                    .trim_end_matches('\n')
                    .to_owned(),
            )
        };
        Ok(Some(Repository {
            root: root.into(),
            common_dir: self.absolute_git_path(&common)?,
        }))
    }
    pub async fn is_repository(&self) -> Result<bool, GitError> {
        Ok(self.detect_repository().await?.is_some())
    }
    pub(crate) fn absolute_git_path(&self, path: &str) -> Result<PathBuf, GitError> {
        let path = Path::new(path);
        if path.is_absolute() {
            Ok(path.to_owned())
        } else {
            Ok(std::fs::canonicalize(&self.cwd)?.join(path))
        }
    }

    pub async fn status(&self) -> Result<Status, GitError> {
        if !self.is_repository().await? {
            return Ok(Status::default());
        }
        let index = self.text(&["rev-parse", "--git-path", "index"]).await?;
        let index = self.absolute_git_path(index.trim())?;
        let mut lock = index.into_os_string();
        lock.push(".lock");
        if Path::new(&lock).try_exists()? {
            return Err(GitError::Unavailable(
                "Git index is locked. Status will resume when the index lock is removed.".into(),
            ));
        }
        let output = self
            .execute(
                &["status", "--porcelain=2", "--branch", "-z"],
                ExecuteOptions {
                    max_output_bytes: 16 * 1024 * 1024,
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?;
        parse_status(&output.stdout_text())
    }
    pub async fn current_branch(&self) -> Result<Option<String>, GitError> {
        let result = self
            .execute(
                &["symbolic-ref", "--quiet", "HEAD"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        match result.code {
            Some(0) => Ok(core::branch_from_ref(&result.stdout_text())),
            Some(1) => Ok(None),
            _ => Err(self.vcs_exit("GitWorkflowService.currentBranch", 3, &result)),
        }
    }
    pub async fn list_refs(&self, prefixes: &[&str]) -> Result<Vec<GitRef>, GitError> {
        let mut args = vec![
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)%00%(HEAD)",
            "--",
        ];
        args.extend_from_slice(prefixes);
        let output = self
            .execute(
                &args,
                ExecuteOptions {
                    max_output_bytes: 16 * 1024 * 1024,
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?;
        let mut refs = Vec::new();
        for line in output.stdout_text().lines() {
            let fields: Vec<_> = line.split('\0').collect();
            if fields.len() != 4 {
                return Err(GitError::InvalidOutput("for-each-ref framing".into()));
            }
            refs.push(GitRef {
                name: fields[0].into(),
                object_id: fields[1].into(),
                upstream: (!fields[2].is_empty()).then(|| fields[2].into()),
                current: fields[3] == "*",
            });
        }
        Ok(refs)
    }
}

pub(crate) fn env(values: &[(&str, &str)]) -> Environment {
    values
        .iter()
        .map(|(key, value)| ((*key).into(), Some((*value).into())))
        .collect()
}

fn merge_environment(environment: &mut Environment, patch: &Environment) {
    for (key, value) in patch {
        #[cfg(windows)]
        environment.retain(|name, _| {
            !name
                .to_string_lossy()
                .eq_ignore_ascii_case(&key.to_string_lossy())
        });
        environment.insert(key.clone(), value.clone());
    }
}

fn git_environment(base: &Environment, command: &Environment) -> Environment {
    git_environment_from(std::env::vars_os().collect(), base, command)
}
fn git_environment_from(
    mut inherited: BTreeMap<OsString, OsString>,
    base: &Environment,
    command: &Environment,
) -> Environment {
    let selectors = [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
        "GIT_REPLACE_REF_BASE",
        "GIT_LITERAL_PATHSPECS",
        "GIT_GLOB_PATHSPECS",
        "GIT_NOGLOB_PATHSPECS",
        "GIT_ICASE_PATHSPECS",
        "GIT_CURL_VERBOSE",
    ];
    let key_matches = |a: &std::ffi::OsStr, b: &str| {
        if cfg!(windows) {
            a.to_string_lossy().eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let mut result: Environment = inherited
        .iter()
        .filter(|(key, _)| {
            selectors.iter().any(|selector| key_matches(key, selector)) || {
                let key = key.to_string_lossy();
                if cfg!(windows) {
                    key.to_ascii_uppercase().starts_with("GIT_TRACE")
                } else {
                    key.starts_with("GIT_TRACE")
                }
            }
        })
        .map(|(key, _)| (key.clone(), None))
        .collect();
    result.extend(env(&[
        ("LANGUAGE", "C"),
        ("LC_ALL", "C"),
        ("GIT_PAGER", "cat"),
        ("GIT_EDITOR", "true"),
        ("GIT_SEQUENCE_EDITOR", "true"),
        ("GIT_SSH_COMMAND", "ssh -oBatchMode=yes"),
    ]));
    result.extend(env(core::NONINTERACTIVE));
    merge_environment(&mut result, base);
    merge_environment(&mut result, command);
    for (key, value) in &result {
        inherited.retain(|name, _| {
            !if cfg!(windows) {
                name.to_string_lossy()
                    .eq_ignore_ascii_case(&key.to_string_lossy())
            } else {
                name == key
            }
        });
        if let Some(value) = value {
            inherited.insert(key.clone(), value.clone());
        }
    }
    let counts: Vec<_> = inherited
        .iter()
        .filter(|(key, _)| key_matches(key, "GIT_CONFIG_COUNT"))
        .collect();
    let inherited_count = counts
        .last()
        .map(|(_, value)| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(count) = parse_config_count(&inherited_count) else {
        return result;
    };
    if let Some(next) = count.checked_add(1) {
        for (key, _) in counts {
            result.insert(key.clone(), Some(next.to_string().into()));
        }
        result.insert("GIT_CONFIG_COUNT".into(), Some(next.to_string().into()));
        result.insert(
            format!("GIT_CONFIG_KEY_{count}").into(),
            Some("safe.bareRepository".into()),
        );
        result.insert(
            format!("GIT_CONFIG_VALUE_{count}").into(),
            Some("explicit".into()),
        );
    }
    result
}
fn transient_exit(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.lines().any(|line| {
        (line.contains("unable to create ")
            && line.contains(".lock")
            && line.contains("file exists"))
            || ((line.contains("unable to stat")
                || line.contains("lstat(")
                || line.contains("error: open("))
                && line.contains(": no such file or directory"))
    })
}

fn append_config_env(environment: &mut Environment, key: &str, value: &str) {
    let count = environment
        .get(std::ffi::OsStr::new("GIT_CONFIG_COUNT"))
        .and_then(Option::as_ref)
        .map(|value| value.to_string_lossy());
    if let Some(count) = count.as_deref().and_then(parse_config_count)
        && let Some(next) = count.checked_add(1)
    {
        for (name, value) in environment.iter_mut() {
            if name
                .to_string_lossy()
                .eq_ignore_ascii_case("GIT_CONFIG_COUNT")
            {
                *value = Some(next.to_string().into());
            }
        }
        environment.insert("GIT_CONFIG_COUNT".into(), Some(next.to_string().into()));
        environment.insert(format!("GIT_CONFIG_KEY_{count}").into(), Some(key.into()));
        environment.insert(
            format!("GIT_CONFIG_VALUE_{count}").into(),
            Some(value.into()),
        );
    }
}
fn parse_status(output: &str) -> Result<Status, GitError> {
    let mut status = Status {
        is_repo: true,
        ..Default::default()
    };
    let mut records = output.split('\0');
    while let Some(record) = records.next() {
        if let Some(value) = record.strip_prefix("# branch.head ") {
            status.branch = (value != "(detached)").then(|| value.into());
        } else if let Some(value) = record.strip_prefix("# branch.oid ") {
            status.head = (value != "(initial)").then(|| value.into());
        } else if let Some(value) = record.strip_prefix("# branch.upstream ") {
            status.upstream = Some(value.into());
        } else if let Some(value) = record.strip_prefix("# branch.ab ") {
            if let Some((ahead, behind)) = value.split_once(' ') {
                status.ahead = ahead.trim_start_matches('+').parse().unwrap_or(0);
                status.behind = behind.trim_start_matches('-').parse().unwrap_or(0);
            }
        } else if let Some(kind) = record.chars().next().filter(|kind| "12u?!".contains(*kind)) {
            let fields = match kind {
                '1' => 8,
                '2' => 9,
                'u' => 10,
                _ => 1,
            };
            let parts: Vec<_> = record.splitn(fields + 1, ' ').collect();
            if parts.len() != fields + 1 {
                return Err(GitError::InvalidOutput("porcelain path".into()));
            }
            let xy = if kind == '?' || kind == '!' {
                "??"
            } else {
                parts[1]
            };
            let previous_path = if kind == '2' {
                Some(
                    records
                        .next()
                        .ok_or_else(|| GitError::InvalidOutput("rename source".into()))?
                        .into(),
                )
            } else {
                None
            };
            status.files.push(ChangedFile {
                path: parts[fields].into(),
                previous_path,
                index_status: xy.chars().next().unwrap_or('.'),
                worktree_status: xy.chars().nth(1).unwrap_or('.'),
                conflicted: kind == 'u',
            });
        }
    }
    Ok(status)
}

fn parse_config_count(value: &str) -> Option<usize> {
    if value.is_empty() {
        return Some(0);
    }
    let value = value.trim_start_matches([' ', '\t', '\r', '\n', '\u{000b}', '\u{000c}']);
    if let Some(zero) = value.strip_prefix('-') {
        return (!zero.is_empty() && zero.bytes().all(|byte| byte == b'0')).then_some(0);
    }
    let value = value.strip_prefix('+').unwrap_or(value);
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

#[cfg(test)]
mod execution_tests {
    use super::*;

    #[test]
    fn inherited_namespace_pathspec_and_trace_overrides_do_not_escape_the_service() {
        for key in [
            "GIT_NAMESPACE",
            "GIT_LITERAL_PATHSPECS",
            "GIT_GLOB_PATHSPECS",
            "GIT_TRACE_CURL",
            "GIT_TRACE2_EVENT",
            "GIT_TRACE_PERFORMANCE",
            "GIT_TRACE_PACKFILE",
            "GIT_SHALLOW_FILE",
        ] {
            let inherited = [(OsString::from(key), OsString::from("untrusted"))]
                .into_iter()
                .collect();
            let environment =
                git_environment_from(inherited, &Environment::new(), &Environment::new());
            assert_eq!(
                environment.get(std::ffi::OsStr::new(key)),
                Some(&None),
                "{key}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn environment_patch_precedence_uses_windows_key_equality() {
        let base = env(&[("git_editor", "base"), ("GIT_CONFIG_COUNT", "0")]);
        let command = env(&[("GIT_EDITOR", "command")]);
        let environment = git_environment_from(BTreeMap::new(), &base, &command);
        let editors: Vec<_> = environment
            .iter()
            .filter(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("GIT_EDITOR"))
            .collect();
        assert_eq!(editors.len(), 1);
        assert_eq!(editors[0].1, &Some(OsString::from("command")));
    }
    #[tokio::test]
    async fn untimed_and_long_commands_bypass_a_closed_global_lane() {
        for timeout in [None, Some(Duration::from_secs(31))] {
            let mut git = Git::new(std::env::temp_dir());
            git.permits = Arc::new(Semaphore::new(0));
            git.permits.close();
            let output = git
                .execute(
                    &["--version"],
                    ExecuteOptions {
                        timeout,
                        ..Default::default()
                    },
                )
                .await;
            assert!(output.is_ok(), "{output:?}");
        }
        let mut git = Git::new(std::env::temp_dir());
        git.permits = Arc::new(Semaphore::new(0));
        git.permits.close();
        let output = git
            .execute_command(
                "test.explicit-unbounded",
                &["--version"],
                ExecuteOptions {
                    unbounded_concurrency: true,
                    ..Default::default()
                },
            )
            .await;
        assert!(output.is_ok(), "{output:?}");
    }
}
