use crate::{
    CommandError, ExecuteOptions, Git, GitError, LocalStatusOptions,
    core::{METADATA_LIMIT, NONINTERACTIVE, branch_from_ref, parse_remote_ref, path_key},
    diagnostics::{fetch_detail, ref_locked},
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString as Text,
    git::{
        GitCommandFailureReason, GitPushStepStatus, GitRunStackedActionResultPush, VcsPullResult,
        VcsPullResultStatus,
    },
};
use t3_process::{Environment, ProcessOutput};
use tokio::sync::{Mutex, OwnedMutexGuard};

const TEMP_INDEX_CONFIG: &[&str] = &[
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.splitIndex=false",
    "-c",
    "splitIndex.sharedIndexExpire=never",
];

#[derive(Clone)]
pub struct CommitOptions {
    /// None leaves the index alone; Some(empty) stages all changes.
    pub stage: Option<Vec<String>>,
    pub timeout: Option<Duration>,
    pub progress: Option<crate::CommitReporter>,
}
impl Default for CommitOptions {
    fn default() -> Self {
        Self {
            stage: None,
            timeout: Some(Duration::from_secs(30)),
            progress: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedCommit {
    pub staged_summary: String,
    pub staged_patch: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushTarget {
    pub remote_name: String,
    pub branch_name: String,
}
struct PushDestination {
    target: PushTarget,
    publishes: bool,
    upstream: Option<(String, String, String)>,
}

pub fn parse_commit_message(raw: &str) -> Option<(String, String)> {
    let normalized = raw.replace("\r\n", "\n");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }
    let (subject, body) = normalized.split_once('\n').unwrap_or((normalized, ""));
    let subject = subject.trim();
    (!subject.is_empty()).then(|| (subject.into(), body.trim().into()))
}

struct CancelOnDrop(crate::CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
pub(crate) struct OperationGuard(pub(crate) Arc<crate::core::CoreState>);
#[derive(Default)]
pub(crate) struct OperationScope {
    active: std::sync::atomic::AtomicUsize,
    finished: tokio::sync::Notify,
}
pub(crate) struct ScopedOperation(Arc<OperationScope>);
impl OperationScope {
    pub(crate) fn start(self: &Arc<Self>) -> ScopedOperation {
        self.active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        ScopedOperation(self.clone())
    }
    async fn drain(&self) {
        loop {
            let finished = self.finished.notified();
            tokio::pin!(finished);
            finished.as_mut().enable();
            if self.active.load(std::sync::atomic::Ordering::Acquire) == 0 {
                return;
            }
            finished.await;
        }
    }
}
impl Drop for ScopedOperation {
    fn drop(&mut self) {
        self.0
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        self.0.finished.notify_waiters();
    }
}
impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.0
            .operations
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        self.0.operations_finished.notify_waiters();
    }
}
impl Git {
    pub async fn drain_operations(&self) {
        if let Some(scope) = &self.operation_scope {
            scope.drain().await;
            return;
        }
        loop {
            let notified = self.core.operations_finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .core
                .operations
                .load(std::sync::atomic::Ordering::Acquire)
                == 0
            {
                return;
            }
            notified.await;
        }
    }
    pub(crate) async fn owned_operation<T, F, Fut>(&self, operation: F) -> Result<T, GitError>
    where
        T: Send + 'static,
        F: FnOnce(Git) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, GitError>> + Send + 'static,
    {
        let token = self.cancellation.child_token();
        let guard = CancelOnDrop(token.clone());
        let git = self.clone().with_cancellation(token);
        self.core
            .operations
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let operation_guard = OperationGuard(self.core.clone());
        let scope = self.operation_scope.as_ref().map(|scope| scope.start());
        let result = tokio::spawn(async move {
            let _guard = operation_guard;
            let _scope = scope;
            operation(git).await
        })
        .await
        .map_err(|_| GitError::InvalidOutput("owned Git operation did not complete".into()))?;
        drop(guard);
        result
    }
    pub(crate) async fn shared_config_lock(&self) -> OwnedMutexGuard<()> {
        let key = self
            .repository_paths(false)
            .await
            .ok()
            .flatten()
            .map(|paths| paths.repository.common_dir)
            .unwrap_or_else(|| self.cwd().to_owned());
        let lock = self
            .core
            .config_locks
            .lock()
            .await
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        lock.lock_owned().await
    }
    pub(crate) async fn checkout_lock(&self) -> Result<OwnedMutexGuard<()>, GitError> {
        let paths = self.repository_paths_uncached().await?.ok_or_else(|| {
            self.command_error(
                "GitWorkflowService.ensureGitCommand",
                "Failed to resolve the VCS driver for this Git command.",
            )
        })?;
        let root = std::fs::canonicalize(&paths.repository.root).unwrap_or(paths.repository.root);
        let lock = self
            .core
            .checkout_locks
            .lock()
            .await
            .entry(root)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        Ok(lock.lock_owned().await)
    }
    pub async fn write_config_value(&self, key: &str, value: &str) -> Result<(), GitError> {
        crate::checkpoint::validate_config_key(key)?;
        let result = {
            let _lock = self.shared_config_lock().await;
            self.retry_config_command(
                "GitVcsDriver.writeConfigValue",
                &["config", "--", key, value],
            )
            .await
        };
        self.invalidate_caches().await;
        result.map(|_| ())
    }
    pub(crate) async fn retry_config_command(
        &self,
        operation: &str,
        args: &[&str],
    ) -> Result<ProcessOutput, GitError> {
        for attempt in 0..=10 {
            match self
                .execute_command(operation, args, ExecuteOptions::default())
                .await
            {
                Err(GitError::Command(error))
                    if error.reason == Some(GitCommandFailureReason::ConfigLocked)
                        && attempt < 10 =>
                {
                    tokio::select! { _ = self.cancellation.cancelled() => return Err(t3_process::ProcessError::Cancelled.into()), _ = tokio::time::sleep(Duration::from_millis(50)) => {} }
                }
                result => return result,
            }
        }
        Err(self.command_error(operation, "Another Git process kept the repository's config locked. Wait for it to finish, then retry."))
    }
    pub(crate) async fn stage_commit_changes(
        &self,
        file_paths: &[String],
        environment: Option<&Environment>,
    ) -> Result<(), GitError> {
        for path in file_paths {
            if path.is_empty()
                || path.contains('\0')
                || cfg!(windows) && path.contains('\\')
                || std::path::Path::new(path).is_absolute()
                || path.split('/').any(|part| part == "..")
            {
                return Err(GitError::InvalidArgument(
                    "Selected paths must be NUL-free repository-relative paths.".into(),
                ));
            }
        }
        let environment = environment.cloned().unwrap_or_default();
        let temporary = !environment.is_empty();
        let config = if temporary { TEMP_INDEX_CONFIG } else { &[] };
        if !file_paths.is_empty() {
            if temporary {
                let head = self
                    .execute_command(
                        "GitVcsDriver.prepareCommitContext.head",
                        &["rev-parse", "--verify", "--quiet", "HEAD"],
                        ExecuteOptions {
                            allow_nonzero: true,
                            ..Default::default()
                        },
                    )
                    .await?;
                let mut args = config.to_vec();
                args.extend([
                    "read-tree",
                    if head.code == Some(0) {
                        "HEAD"
                    } else {
                        "--empty"
                    },
                ]);
                self.execute_command(
                    "GitVcsDriver.prepareCommitContext.readTree",
                    &args,
                    ExecuteOptions {
                        env: environment.clone(),
                        ..Default::default()
                    },
                )
                .await?;
            } else {
                let heads = self
                    .core_text(
                        "GitVcsDriver.commit.mergeHead",
                        &[
                            "rev-parse",
                            "--git-path",
                            "MERGE_HEAD",
                            "--git-path",
                            "CHERRY_PICK_HEAD",
                            "--git-path",
                            "REVERT_HEAD",
                        ],
                        false,
                    )
                    .await?;
                for (path, operation) in
                    heads
                        .trim()
                        .split('\n')
                        .zip(["merge", "cherry-pick", "revert"])
                {
                    if self.cwd().join(path).try_exists()? {
                        return Err(self.command_error("GitVcsDriver.commit", format!("Cannot commit selected files during a {operation}. Commit all files to finish the {operation}.")));
                    }
                }
                self.core_text("GitVcsDriver.commit.reset", &["reset", "--quiet"], false)
                    .await?;
            }
            let mut args = config.to_vec();
            args.extend(["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"]);
            let input: String = file_paths
                .iter()
                .map(|path| format!(":(top,literal){path}\0"))
                .collect();
            self.execute_command(
                "GitVcsDriver.commit.addSelected",
                &args,
                ExecuteOptions {
                    env: environment,
                    stdin: Some(input.into_bytes()),
                    ..Default::default()
                },
            )
            .await?;
        } else {
            let unmerged = self
                .execute_command(
                    "GitVcsDriver.commit.unmergedPaths",
                    &[
                        "-c",
                        "diff.relative=false",
                        "diff",
                        "--name-only",
                        "--diff-filter=U",
                        "-z",
                    ],
                    ExecuteOptions {
                        env: environment.clone(),
                        max_output_bytes: METADATA_LIMIT,
                        ..Default::default()
                    },
                )
                .await?;
            let paths: Vec<_> = unmerged
                .stdout_text()
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect();
            if !paths.is_empty() {
                let mut args: Vec<String> = [
                    "-c",
                    "diff.relative=false",
                    "diff",
                    "--check",
                    "--no-color",
                    "--",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect();
                args.extend(paths.iter().map(|path| format!(":(top,literal){path}")));
                let check = self
                    .execute_command(
                        "GitVcsDriver.commit.conflictMarkers",
                        &args,
                        ExecuteOptions {
                            env: environment.clone(),
                            allow_nonzero: true,
                            max_output_bytes: METADATA_LIMIT,
                            append_truncation_marker: true,
                            ..Default::default()
                        },
                    )
                    .await?;
                let reports = check.stdout_text();
                let root = self
                    .core_text(
                        "GitVcsDriver.commit.topLevel",
                        &["rev-parse", "--show-toplevel"],
                        false,
                    )
                    .await?;
                let mut conflicted = Vec::new();
                for path in paths {
                    let markers: std::collections::BTreeSet<usize> = reports
                        .lines()
                        .filter_map(|line| {
                            line.strip_prefix(&format!("{path}:"))?
                                .strip_suffix(": leftover conflict marker")?
                                .parse::<usize>()
                                .ok()
                                .and_then(|line| line.checked_sub(1))
                        })
                        .collect();
                    if markers.is_empty() {
                        continue;
                    }
                    let contents =
                        tokio::fs::read_to_string(PathBuf::from(root.trim()).join(&path))
                            .await
                            .map_err(|_| {
                                self.command_error(
                                    "GitVcsDriver.commit.conflictMarkers",
                                    format!("Could not check {path} for conflict markers."),
                                )
                            })?;
                    let reported = |marker: char| {
                        contents.lines().enumerate().any(|(index, line)| {
                            markers.contains(&index) && line.starts_with(marker)
                        })
                    };
                    if reported('<') && reported('>') {
                        conflicted.push(path);
                    }
                }
                if !conflicted.is_empty() {
                    return Err(self.command_error("GitVcsDriver.commit", format!("Cannot commit with unresolved merge conflicts in {}. Resolve them, then commit.", conflicted.join(", "))));
                }
            }
            let mut args = config.to_vec();
            args.extend(["add", "-A"]);
            self.execute_command(
                "GitVcsDriver.commit.addAll",
                &args,
                ExecuteOptions {
                    env: environment,
                    ..Default::default()
                },
            )
            .await?;
        }
        Ok(())
    }
    pub async fn prepare_commit_context(
        &self,
        file_paths: Option<&[String]>,
        include_patch: bool,
    ) -> Result<Option<PreparedCommit>, GitError> {
        let file_paths = file_paths.unwrap_or_default().to_vec();
        self.owned_operation(move |git| async move {
            git.prepare_commit_context_owned(&file_paths, include_patch)
                .await
        })
        .await
    }
    async fn prepare_commit_context_owned(
        &self,
        file_paths: &[String],
        include_patch: bool,
    ) -> Result<Option<PreparedCommit>, GitError> {
        let git_paths = self
            .core_text(
                "GitVcsDriver.prepareCommitContext.gitPaths",
                &["rev-parse", "--git-path", "index", "--git-path", "objects"],
                false,
            )
            .await?;
        let mut paths = git_paths.lines();
        let index = self.absolute_git_path(paths.next().unwrap_or("").trim())?;
        let objects = self.absolute_git_path(paths.next().unwrap_or("").trim())?;
        let directory = tempfile::Builder::new()
            .prefix("t3code-commit-index-")
            .tempdir()?;
        let temp_index = directory.path().join("index");
        let temp_objects = directory.path().join("objects");
        tokio::fs::create_dir(&temp_objects).await?;
        let alternates = self
            .core_text(
                "GitVcsDriver.prepareCommitContext.alternates",
                &["count-objects", "-v"],
                false,
            )
            .await?;
        let directories = std::iter::once(path_key(&objects)).chain(
            alternates
                .lines()
                .filter_map(|line| line.strip_prefix("alternate: ").map(str::to_owned)),
        );
        let alternates = directories
            .map(|path| {
                if path.starts_with('"') {
                    path
                } else {
                    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
                }
            })
            .collect::<Vec<_>>()
            .join(if cfg!(windows) { ";" } else { ":" });
        let mut env = crate::env(&[("GIT_ALTERNATE_OBJECT_DIRECTORIES", &alternates)]);
        env.insert(
            "GIT_INDEX_FILE".into(),
            Some(temp_index.as_os_str().to_owned()),
        );
        env.insert(
            "GIT_OBJECT_DIRECTORY".into(),
            Some(temp_objects.as_os_str().to_owned()),
        );
        if index.try_exists()? {
            tokio::fs::copy(&index, &temp_index).await?;
            let mtime =
                filetime::FileTime::from_last_modification_time(&std::fs::metadata(&index)?);
            let time = filetime::FileTime::from_unix_time(mtime.unix_seconds().max(0), 0);
            filetime::set_file_times(&temp_index, time, time)?;
            let mut args = TEMP_INDEX_CONFIG.to_vec();
            args.extend(["update-index", "--no-split-index"]);
            self.execute_command(
                "GitVcsDriver.prepareCommitContext.expandSplitIndex",
                &args,
                ExecuteOptions {
                    env: env.clone(),
                    ..Default::default()
                },
            )
            .await?;
        } else {
            let mut args = TEMP_INDEX_CONFIG.to_vec();
            args.extend(["read-tree", "--empty"]);
            self.execute_command(
                "GitVcsDriver.prepareCommitContext.emptyIndex",
                &args,
                ExecuteOptions {
                    env: env.clone(),
                    ..Default::default()
                },
            )
            .await?;
        }
        self.stage_commit_changes(file_paths, Some(&env)).await?;
        let summary = self
            .execute_command(
                "GitVcsDriver.prepareCommitContext.stagedSummary",
                &[
                    "-c",
                    "diff.relative=false",
                    "diff",
                    "--cached",
                    "--name-status",
                ],
                ExecuteOptions {
                    env: env.clone(),
                    max_output_bytes: METADATA_LIMIT,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text();
        if summary.trim().is_empty() {
            let merge = self
                .core_text(
                    "GitVcsDriver.prepareCommitContext.mergeHead",
                    &["rev-parse", "--git-path", "MERGE_HEAD"],
                    false,
                )
                .await?;
            if !self.absolute_git_path(merge.trim())?.try_exists()? {
                return Ok(None);
            }
        }
        let patch = if include_patch {
            self.execute_command(
                "GitVcsDriver.prepareCommitContext.stagedPatch",
                &[
                    "-c",
                    "diff.relative=false",
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--cached",
                    "--patch",
                    "--minimal",
                ],
                ExecuteOptions {
                    env,
                    max_output_bytes: 49_000,
                    append_truncation_marker: true,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text()
        } else {
            String::new()
        };
        Ok(Some(PreparedCommit {
            staged_summary: summary.trim().into(),
            staged_patch: patch,
        }))
    }
    pub async fn commit(
        &self,
        subject: &str,
        body: &str,
        options: CommitOptions,
    ) -> Result<String, GitError> {
        let subject = subject.to_owned();
        let body = body.to_owned();
        self.owned_operation(
            move |git| async move { git.commit_owned(&subject, &body, options).await },
        )
        .await
    }
    async fn commit_owned(
        &self,
        subject: &str,
        body: &str,
        options: CommitOptions,
    ) -> Result<String, GitError> {
        let result = async {
            if let Some(paths) = &options.stage {
                self.stage_commit_changes(paths, None).await?;
            }
            let mut args = vec!["commit", "-m", subject];
            let body = body.trim();
            if !body.is_empty() {
                args.extend(["-m", body]);
            }
            let monitor = options
                .progress
                .as_ref()
                .map(|reporter| crate::progress::CommitMonitor::new(reporter.clone()))
                .transpose()?;
            let mut env = Environment::new();
            if let Some(monitor) = &monitor {
                env.insert(
                    "GIT_TRACE2_EVENT".into(),
                    Some(monitor.path.as_os_str().to_owned()),
                );
            }
            let stdout = options
                .progress
                .as_ref()
                .map(|reporter| crate::progress::output_lines("stdout", reporter.clone()));
            let stderr = options
                .progress
                .as_ref()
                .map(|reporter| crate::progress::output_lines("stderr", reporter.clone()));
            let result = self
                .execute_command(
                    "GitVcsDriver.commit.commit",
                    &args,
                    ExecuteOptions {
                        timeout: options.timeout,
                        max_output_bytes: METADATA_LIMIT,
                        env,
                        on_stdout_chunk: stdout.as_ref().map(|output| output.callback()),
                        on_stderr_chunk: stderr.as_ref().map(|output| output.callback()),
                        ..Default::default()
                    },
                )
                .await;
            drop(stdout);
            drop(stderr);
            drop(monitor);
            result?;
            Ok(self
                .core_text(
                    "GitVcsDriver.commit.revParseHead",
                    &["rev-parse", "HEAD"],
                    false,
                )
                .await?
                .trim()
                .into())
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    pub async fn commit_message(
        &self,
        message: &str,
        file_paths: Option<&[String]>,
    ) -> Result<Option<String>, GitError> {
        let _lease = self.checkout_lock().await?;
        let (subject, body) = parse_commit_message(message).ok_or_else(|| {
            self.command_error("GitManager.runCommitStep", "A commit message is required.")
        })?;
        if self
            .prepare_commit_context(file_paths, false)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        self.commit(
            &subject,
            &body,
            CommitOptions {
                stage: Some(file_paths.unwrap_or_default().to_vec()),
                timeout: Some(Duration::from_secs(600)),
                ..Default::default()
            },
        )
        .await
        .map(Some)
    }
    pub async fn create_branch(&self, branch: &str, start_point: &str) -> Result<(), GitError> {
        let result = self.create_branch_at(branch, start_point).await;
        self.invalidate_caches().await;
        result
    }
    async fn create_branch_at(&self, branch: &str, start_point: &str) -> Result<(), GitError> {
        self.refuse_option_ref("GitVcsDriver.createBranch", start_point)?;
        let start = if start_point == "-" {
            "@{-1}"
        } else {
            start_point
        };
        self.execute_command(
            "GitVcsDriver.createBranch",
            &["branch", "--no-track", "--", branch, start],
            ExecuteOptions::default(),
        )
        .await?;
        let start_ref = self
            .core_text(
                "GitVcsDriver.createBranch.resolveStartPoint",
                &["rev-parse", "--symbolic-full-name", start],
                true,
            )
            .await?;
        if start_ref.trim().starts_with("refs/remotes/") {
            let _lock = self.shared_config_lock().await;
            let _ = self
                .retry_config_command(
                    "GitVcsDriver.createBranch.track",
                    &[
                        "branch",
                        &format!("--set-upstream-to={}", start_ref.trim()),
                        "--",
                        branch,
                    ],
                )
                .await;
        }
        Ok(())
    }
    pub async fn create_ref(&self, name: &str, switch: bool) -> Result<String, GitError> {
        let result = async {
            {
                let _lock = self.shared_config_lock().await;
                self.execute_command(
                    "GitVcsDriver.createRef",
                    &["branch", "--", name],
                    ExecuteOptions {
                        timeout: Some(Duration::from_secs(10)),
                        ..Default::default()
                    },
                )
                .await?;
            }
            if switch {
                self.switch_ref(name).await?;
            }
            Ok(name.into())
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    pub async fn delete_local_branch(&self, name: &str, force: bool) -> Result<(), GitError> {
        let _lock = self.shared_config_lock().await;
        let result = self
            .execute_command(
                "GitVcsDriver.deleteLocalBranch",
                &["branch", if force { "-D" } else { "-d" }, "--", name],
                ExecuteOptions {
                    timeout: Some(Duration::from_secs(10)),
                    ..Default::default()
                },
            )
            .await;
        self.invalidate_caches().await;
        result.map(|_| ())
    }
    pub async fn list_local_branch_names(&self) -> Result<Vec<String>, GitError> {
        Ok(self
            .core_text(
                "GitVcsDriver.listLocalBranchNames",
                &[
                    "branch",
                    "--list",
                    "--no-column",
                    "--format=%(refname:lstrip=2)",
                ],
                false,
            )
            .await?
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect())
    }
    pub async fn rename_branch(
        &self,
        old: &str,
        new: &str,
        exact: bool,
    ) -> Result<String, GitError> {
        if old == new {
            self.invalidate_caches().await;
            return Ok(new.into());
        }
        let result = async {
            let target = if exact {
                new.into()
            } else {
                self.available_branch_name(new).await?
            };
            let _lock = self.shared_config_lock().await;
            self.execute_command(
                "GitVcsDriver.renameBranch",
                &["branch", "-m", "--", old, &target],
                ExecuteOptions {
                    timeout: Some(Duration::from_secs(10)),
                    ..Default::default()
                },
            )
            .await?;
            Ok(target)
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    async fn available_branch_name(&self, requested: &str) -> Result<String, GitError> {
        let output = self
            .execute_command(
                "GitVcsDriver.resolveAvailableBranchName",
                &["for-each-ref", "--format=%(refname:strip=2)", "refs/heads/"],
                ExecuteOptions {
                    max_output_bytes: METADATA_LIMIT,
                    ..Default::default()
                },
            )
            .await?;
        let branches: std::collections::BTreeSet<_> = output
            .stdout_text()
            .lines()
            .map(str::to_lowercase)
            .collect();
        let segments: Vec<_> = requested.split('/').collect();
        let blocked_parent = (1..segments.len())
            .any(|index| branches.contains(&segments[..index].join("/").to_lowercase()));
        let desired = if blocked_parent {
            segments.join("-")
        } else {
            requested.into()
        };
        for suffix in 0..=100 {
            let candidate = if suffix == 0 {
                desired.clone()
            } else {
                format!("{desired}-{suffix}")
            };
            let lower = candidate.to_lowercase();
            if !branches.contains(&lower)
                && !branches
                    .iter()
                    .any(|branch| branch.starts_with(&format!("{lower}/")))
            {
                return Ok(candidate);
            }
        }
        Err(self.command_error(
            "GitVcsDriver.renameBranch",
            format!("Could not find an available branch name for '{desired}'."),
        ))
    }
    fn refuse_option_ref(&self, operation: &str, reference: &str) -> Result<(), GitError> {
        if reference.starts_with('-') && reference != "-" {
            Err(self.command_error(operation, "Ref names cannot start with '-'."))
        } else {
            Ok(())
        }
    }
    pub async fn switch_ref(&self, name: &str) -> Result<Option<String>, GitError> {
        let _lease = self.checkout_lock().await?;
        let result = self.switch_ref_unlocked(name).await;
        self.invalidate_caches().await;
        result
    }
    pub(crate) async fn switch_ref_unlocked(&self, name: &str) -> Result<Option<String>, GitError> {
        let local = self.ref_exists(&format!("refs/heads/{name}")).await?;
        let remote = self.ref_exists(&format!("refs/remotes/{name}")).await?;
        let tracking = if remote && !local {
            let output = self
                .core_text(
                    "GitVcsDriver.switchRef.localTrackingBranch",
                    &[
                        "for-each-ref",
                        "--format=%(refname:lstrip=2)\t%(upstream)",
                        "refs/heads",
                    ],
                    true,
                )
                .await?;
            output.lines().find_map(|line| {
                let (branch, upstream) = line.trim().split_once('\t')?;
                (upstream.trim() == format!("refs/remotes/{name}"))
                    .then(|| branch.trim().to_owned())
            })
        } else {
            None
        };
        let candidate = name
            .split_once('/')
            .map(|(_, branch)| branch.trim())
            .filter(|name| !name.is_empty());
        let candidate_exists = if remote
            && !local
            && tracking.is_none()
            && let Some(candidate) = candidate
        {
            self.ref_exists(&format!("refs/heads/{candidate}")).await?
        } else {
            false
        };
        let new_tracking = if !local && remote && tracking.is_none() && !candidate_exists {
            candidate
        } else {
            None
        };
        let target = if local {
            name
        } else {
            new_tracking.or(tracking.as_deref()).unwrap_or(name)
        };
        self.refuse_option_ref("GitVcsDriver.switchRef", target)?;
        if let Some(branch) = new_tracking {
            let start = format!("refs/remotes/{name}");
            self.execute_command(
                "GitVcsDriver.switchRef.createTrackingBranch",
                &["branch", "--no-track", "--", branch, &start],
                ExecuteOptions::default(),
            )
            .await?;
            let tracked = {
                let _lock = self.shared_config_lock().await;
                self.retry_config_command(
                    "GitVcsDriver.switchRef.trackRemote",
                    &[
                        "branch",
                        &format!("--set-upstream-to={start}"),
                        "--",
                        branch,
                    ],
                )
                .await
            };
            if let Err(error) = tracked {
                let _ = self
                    .execute_command(
                        "GitVcsDriver.switchRef.removeUntrackedBranch",
                        &["update-ref", "-d", &format!("refs/heads/{branch}"), &start],
                        ExecuteOptions {
                            allow_nonzero: true,
                            ..Default::default()
                        },
                    )
                    .await;
                return Err(error);
            }
        }
        let mut args = vec!["checkout"];
        let track = remote && !local && tracking.is_none() && new_tracking.is_none();
        if track {
            args.push("--track");
        }
        args.extend([target, "--"]);
        let _config_lock = if track || !local && !remote {
            Some(self.shared_config_lock().await)
        } else {
            None
        };
        self.execute_command(
            "GitVcsDriver.switchRef.checkout",
            &args,
            ExecuteOptions {
                timeout: None,
                ..Default::default()
            },
        )
        .await?;
        let branch = self
            .core_text(
                "GitVcsDriver.switchRef.currentBranch",
                &["branch", "--show-current"],
                false,
            )
            .await?;
        Ok((!branch.trim().is_empty()).then(|| branch.trim().into()))
    }
    pub async fn set_branch_upstream(
        &self,
        branch: &str,
        remote: &str,
        remote_branch: &str,
    ) -> Result<(), GitError> {
        let _lock = self.shared_config_lock().await;
        let result = self
            .retry_config_command(
                "GitVcsDriver.setBranchUpstream",
                &[
                    "branch",
                    "--set-upstream-to",
                    &format!("{remote}/{remote_branch}"),
                    "--",
                    branch,
                ],
            )
            .await;
        self.invalidate_caches().await;
        result.map(|_| ())
    }
    pub async fn resolve_commit(&self, revision: &str) -> Result<String, GitError> {
        Ok(self
            .core_text(
                "GitVcsDriver.resolveCommit",
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{revision}^{{commit}}"),
                ],
                false,
            )
            .await?
            .trim()
            .into())
    }
    pub async fn fetch_remote(
        &self,
        remote: &str,
        reference: Option<&str>,
    ) -> Result<(), GitError> {
        let result = async {
            if !self.fetch_remote_scoped(remote, reference).await? {
                self.fetch_remote_scoped(remote, None).await?;
            }
            Ok(())
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    pub(crate) async fn fetch_remote_scoped(
        &self,
        remote: &str,
        reference: Option<&str>,
    ) -> Result<bool, GitError> {
        crate::checkpoint::validate_ref_name(&format!("refs/remotes/{remote}/probe"))?;
        let branch = reference.map(|name| name.strip_prefix(&format!("{remote}/")).unwrap_or(name));
        if let Some(branch) = branch {
            crate::checkpoint::validate_ref_name(&format!("refs/heads/{branch}"))?;
        }
        let spec =
            branch.map(|branch| format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"));
        let mut args = vec!["fetch", "--quiet"];
        if branch.is_some() {
            args.push("--no-tags");
        }
        args.extend(["--", remote]);
        if let Some(spec) = spec.as_deref() {
            args.push(spec);
        }
        let options = ExecuteOptions {
            env: crate::env(NONINTERACTIVE),
            allow_nonzero: true,
            ..Default::default()
        };
        let mut output = self
            .execute_command("GitVcsDriver.fetchRemote", &args, options.clone())
            .await?;
        if output.code != Some(0) && ref_locked(&output.stderr_text()) {
            output = self
                .execute_command("GitVcsDriver.fetchRemote", &args, options)
                .await?;
        }
        if output.code == Some(0) {
            return Ok(true);
        }
        if let Some(branch) = branch
            && output
                .stderr_text()
                .lines()
                .any(|line| line == format!("fatal: couldn't find remote ref refs/heads/{branch}"))
        {
            return Ok(false);
        }
        Err(CommandError::exit(
            "GitVcsDriver.fetchRemote",
            self.cwd(),
            &args,
            &output,
            Some(fetch_detail(&output.stderr_text()).unwrap_or("git fetch failed")),
        )
        .into())
    }
    pub async fn fetch_ref_commit(&self, reference: &str) -> Result<String, GitError> {
        crate::checkpoint::validate_fetch_source(reference)?;
        let result = async {
            let remote = self.primary_remote_name().await?.ok_or_else(|| {
                self.command_error(
                    "GitVcsDriver.resolvePrimaryRemoteName",
                    "No git remote is configured for this repository.",
                )
            })?;
            self.execute_command(
                "GitVcsDriver.fetchRefCommit",
                &["fetch", "--quiet", "--no-tags", "--", &remote, reference],
                ExecuteOptions::default(),
            )
            .await?;
            self.resolve_commit("FETCH_HEAD").await
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    pub async fn fetch_ref_into_branch(
        &self,
        reference: &str,
        branch: &str,
    ) -> Result<(), GitError> {
        crate::checkpoint::validate_fetch_source(reference)?;
        crate::checkpoint::validate_ref_name(&format!("refs/heads/{branch}"))?;
        let result = async {
            let remote = self.primary_remote_name().await?.ok_or_else(|| {
                self.command_error(
                    "GitVcsDriver.resolvePrimaryRemoteName",
                    "No git remote is configured for this repository.",
                )
            })?;
            self.execute_command(
                "GitVcsDriver.fetchRefIntoBranch",
                &[
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--",
                    &remote,
                    &format!("+{reference}:refs/heads/{branch}"),
                ],
                ExecuteOptions::default(),
            )
            .await?;
            Ok(())
        }
        .await;
        self.invalidate_caches().await;
        result
    }
    pub async fn fetch_remote_tracking_branch(
        &self,
        remote: &str,
        branch: &str,
    ) -> Result<(), GitError> {
        crate::checkpoint::validate_ref_name(&format!("refs/remotes/{remote}/{branch}"))?;
        crate::checkpoint::validate_ref_name(&format!("refs/heads/{branch}"))?;
        let result = self
            .execute_command(
                "GitVcsDriver.fetchRemoteTrackingBranch",
                &[
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--",
                    remote,
                    &format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
                ],
                ExecuteOptions::default(),
            )
            .await;
        self.invalidate_caches().await;
        result.map(|_| ())
    }
    pub async fn pull_current_branch(&self) -> Result<VcsPullResult, GitError> {
        let result = self.pull_current_branch_inner().await;
        self.invalidate_caches().await;
        result
    }
    async fn pull_current_branch_inner(&self) -> Result<VcsPullResult, GitError> {
        let details = self
            .status_details_local(LocalStatusOptions::default())
            .await?;
        let branch = details.branch.ok_or_else(|| {
            self.command_error(
                "GitVcsDriver.pullCurrentBranch",
                "Cannot pull from detached HEAD.",
            )
        })?;
        if !details.has_upstream {
            return Err(self.command_error(
                "GitVcsDriver.pullCurrentBranch",
                "Current branch has no upstream configured. Push with upstream first.",
            ));
        }
        if let Some(remote) = self
            .read_config_value(&format!("branch.{branch}.remote"))
            .await?
        {
            self.await_status_fetch(&remote).await;
        }
        let before = self
            .core_text(
                "GitVcsDriver.pullCurrentBranch.beforeSha",
                &["rev-parse", "HEAD"],
                true,
            )
            .await?;
        let options = ExecuteOptions {
            timeout: None,
            append_truncation_marker: true,
            allow_nonzero: true,
            ..Default::default()
        };
        let mut args = vec!["pull".to_owned(), "--ff-only".to_owned()];
        let mut output = self
            .execute_command(
                "GitVcsDriver.pullCurrentBranch.pull",
                &args,
                options.clone(),
            )
            .await?;
        if output.code != Some(0) && ref_locked(&output.stderr_text()) {
            let remote = self
                .read_config_value(&format!("branch.{branch}.remote"))
                .await?;
            let merges = self
                .core_text(
                    "GitVcsDriver.pullCurrentBranch.mergeRefs",
                    &["config", "--get-all", &format!("branch.{branch}.merge")],
                    true,
                )
                .await?;
            let merges: Vec<_> = merges
                .trim()
                .lines()
                .filter(|line| !line.is_empty())
                .collect();
            let current = branch_from_ref(
                &self
                    .core_text(
                        "GitVcsDriver.pullCurrentBranch.currentBranch",
                        &["symbolic-ref", "--quiet", "HEAD"],
                        true,
                    )
                    .await?,
            );
            let head = self
                .core_text(
                    "GitVcsDriver.pullCurrentBranch.currentSha",
                    &["rev-parse", "HEAD"],
                    false,
                )
                .await?;
            if let Some(remote) = remote
                && merges.len() == 1
                && current.as_deref() == Some(&branch)
                && head.trim() == before.trim()
            {
                args = vec![
                    "pull".into(),
                    "--ff-only".into(),
                    "--".into(),
                    remote,
                    merges[0].into(),
                ];
                output = self
                    .execute_command("GitVcsDriver.pullCurrentBranch.pull", &args, options)
                    .await?;
            }
        }
        if output.code != Some(0) {
            return Err(CommandError::exit(
                "GitVcsDriver.pullCurrentBranch.pull",
                self.cwd(),
                &args,
                &output,
                Some(fetch_detail(&output.stderr_text()).unwrap_or("git pull failed")),
            )
            .into());
        }
        let after = self
            .core_text(
                "GitVcsDriver.pullCurrentBranch.afterSha",
                &["rev-parse", "HEAD"],
                true,
            )
            .await?;
        let refreshed = self
            .status_details_local(LocalStatusOptions::default())
            .await?;
        Ok(VcsPullResult {
            status: if !before.trim().is_empty() && before.trim() == after.trim() {
                VcsPullResultStatus::SkippedUpToDate
            } else {
                VcsPullResultStatus::Pulled
            },
            ref_name: Text(branch),
            upstream_ref: refreshed.upstream_ref.map(Text),
        })
    }
    async fn publish_branch_name(&self, branch: &str) -> String {
        parse_remote_ref(branch, &self.list_remote_names().await.unwrap_or_default())
            .map(|(_, branch)| branch)
            .unwrap_or_else(|| branch.into())
    }
    async fn push_remote(&self, branch: &str) -> Result<Option<String>, GitError> {
        if let Some(remote) = self
            .read_config_value(&format!("branch.{branch}.pushRemote"))
            .await?
        {
            return Ok(Some(remote));
        }
        if let Some(remote) = self.read_config_value("remote.pushDefault").await? {
            return Ok(Some(remote));
        }
        self.primary_remote_name().await
    }
    async fn push_destination(
        &self,
        branch: &str,
        has_upstream: bool,
    ) -> Result<Option<PushDestination>, GitError> {
        if !has_upstream {
            return Ok(self
                .push_remote(branch)
                .await?
                .map(|remote_name| PushDestination {
                    target: PushTarget {
                        remote_name,
                        branch_name: branch.into(),
                    },
                    publishes: true,
                    upstream: None,
                })
                .map(|mut destination| {
                    destination.target.branch_name = branch.into();
                    destination
                }));
        }
        if self
            .read_config_value(&format!("branch.{branch}.remote"))
            .await?
            .as_deref()
            == Some(".")
        {
            let Some(upstream) = self.current_upstream().await.ok().flatten() else {
                return Ok(None);
            };
            return Ok(self
                .push_remote(branch)
                .await
                .ok()
                .flatten()
                .map(|remote_name| PushDestination {
                    target: PushTarget {
                        remote_name,
                        branch_name: branch.into(),
                    },
                    publishes: true,
                    upstream: Some((".".into(), upstream.clone(), upstream)),
                }));
        }
        let Some(upstream) = self.current_upstream().await.ok().flatten() else {
            return Ok(None);
        };
        let parsed = parse_remote_ref(
            &upstream,
            &self.list_remote_names().await.unwrap_or_default(),
        )
        .or_else(|| {
            upstream
                .split_once('/')
                .map(|(remote, branch)| (remote.into(), branch.into()))
        });
        let Some((remote, upstream_branch)) = parsed else {
            return Ok(None);
        };
        let is_alias = branch == upstream_branch
            || branch.ends_with(&format!("/{upstream_branch}"))
                && upstream.ends_with(&format!("/{branch}"));
        if is_alias {
            return Ok(Some(PushDestination {
                target: PushTarget {
                    remote_name: remote.clone(),
                    branch_name: upstream_branch.clone(),
                },
                publishes: false,
                upstream: Some((remote, upstream_branch, upstream)),
            }));
        }
        let publish_remote = self
            .push_remote(branch)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| remote.clone());
        Ok(Some(PushDestination {
            target: PushTarget {
                remote_name: publish_remote,
                branch_name: self.publish_branch_name(branch).await,
            },
            publishes: true,
            upstream: Some((remote, upstream_branch, upstream)),
        }))
    }
    pub async fn resolve_push_target(
        &self,
        branch: &str,
        has_upstream: bool,
    ) -> Result<Option<PushTarget>, GitError> {
        let mut target = self
            .push_destination(branch, has_upstream)
            .await?
            .map(|destination| destination.target);
        if !has_upstream && let Some(target) = &mut target {
            target.branch_name = self.publish_branch_name(branch).await;
        }
        Ok(target)
    }
    async fn record_pushed_upstream(&self, branch: &str, remote: &str, publish: &str) -> bool {
        async {
            self.write_config_value(
                &format!("branch.{branch}.merge"),
                &format!("refs/heads/{publish}"),
            )
            .await?;
            self.write_config_value(&format!("branch.{branch}.remote"), remote)
                .await?;
            if self
                .read_config_value("branch.autoSetupRebase")
                .await?
                .is_some_and(|value| value == "always" || value == "remote")
            {
                self.write_config_value(&format!("branch.{branch}.rebase"), "true")
                    .await?;
            }
            Ok::<_, GitError>(())
        }
        .await
        .is_ok()
    }
    pub async fn push_current_branch(
        &self,
        fallback_branch: Option<&str>,
        requested_remote: Option<&str>,
    ) -> Result<GitRunStackedActionResultPush, GitError> {
        let result = self
            .push_current_branch_inner(fallback_branch, requested_remote)
            .await;
        self.invalidate_caches().await;
        result
    }
    async fn push_current_branch_inner(
        &self,
        fallback: Option<&str>,
        requested: Option<&str>,
    ) -> Result<GitRunStackedActionResultPush, GitError> {
        let details = self.status_details().await?;
        let branch = details.branch.as_deref().or(fallback).ok_or_else(|| {
            self.command_error(
                "GitVcsDriver.pushCurrentBranch",
                "Cannot push from detached HEAD.",
            )
        })?;
        let result =
            |status, upstream: Option<String>, set: Option<bool>| GitRunStackedActionResultPush {
                status,
                branch: Some(Some(Text(branch.into()))),
                upstream_branch: upstream.map(|value| Some(Text(value))),
                set_upstream: set.map(Some),
            };
        if let Some(remote) = requested.map(str::trim).filter(|remote| !remote.is_empty()) {
            crate::checkpoint::validate_ref_name(&format!("refs/remotes/{remote}/probe"))?;
            let publish = self.publish_branch_name(branch).await;
            crate::checkpoint::validate_ref_name(&format!("refs/heads/{publish}"))?;
            self.execute_command(
                "GitVcsDriver.pushCurrentBranch.pushWithRequestedRemote",
                &["push", "--", remote, &format!("HEAD:refs/heads/{publish}")],
                ExecuteOptions {
                    timeout: None,
                    ..Default::default()
                },
            )
            .await?;
            return Ok(result(
                GitPushStepStatus::Pushed,
                Some(format!("{remote}/{publish}")),
                Some(self.record_pushed_upstream(branch, remote, &publish).await),
            ));
        }
        let no_delta = details.ahead_count == 0 && details.behind_count == 0;
        if no_delta && details.has_upstream {
            return Ok(result(
                GitPushStepStatus::SkippedUpToDate,
                details.upstream_ref.clone(),
                None,
            ));
        }
        let mut destination = self.push_destination(branch, details.has_upstream).await?;
        if !details.has_upstream
            && let Some(destination) = &mut destination
        {
            destination.target.branch_name = self.publish_branch_name(branch).await;
        }
        if no_delta
            && self
                .comparison_base(branch, false, None)
                .await
                .ok()
                .flatten()
                .is_some()
        {
            if let Some(destination) = &destination {
                let contains = self
                    .execute_command(
                        "GitVcsDriver.pushCurrentBranch.publishContainsHead",
                        &[
                            "merge-base",
                            "--is-ancestor",
                            "HEAD",
                            &format!(
                                "refs/remotes/{}/{}",
                                destination.target.remote_name, destination.target.branch_name
                            ),
                        ],
                        ExecuteOptions {
                            allow_nonzero: true,
                            ..Default::default()
                        },
                    )
                    .await;
                if contains.is_ok_and(|output| output.code == Some(0)) {
                    return Ok(result(GitPushStepStatus::SkippedUpToDate, None, None));
                }
            } else {
                return Ok(result(GitPushStepStatus::SkippedUpToDate, None, None));
            }
        }
        if destination.is_none()
            && (!details.has_upstream
                || self
                    .read_config_value(&format!("branch.{branch}.remote"))
                    .await?
                    .as_deref()
                    == Some("."))
        {
            return Err(self.command_error(
                "GitVcsDriver.pushCurrentBranch",
                "Cannot push because no git remote is configured for this repository.",
            ));
        }
        if let Some(destination) = destination {
            let PushTarget {
                remote_name: remote,
                branch_name: publish,
            } = destination.target;
            crate::checkpoint::validate_ref_name(&format!("refs/remotes/{remote}/probe"))?;
            crate::checkpoint::validate_ref_name(&format!("refs/heads/{publish}"))?;
            if destination.publishes {
                let had_upstream = destination.upstream.is_some();
                if let Some((upstream_remote, upstream_branch, upstream_ref)) = destination.upstream
                    && self.read_branch_base(branch).await?.is_none()
                {
                    self.write_config_value(
                        &format!("branch.{branch}.t3-merge-base"),
                        &upstream_branch,
                    )
                    .await?;
                    if self
                        .read_config_value(&format!("branch.{branch}.t3-merge-base-ref"))
                        .await?
                        .is_none()
                    {
                        self.write_config_value(
                            &format!("branch.{branch}.t3-merge-base-ref"),
                            &if upstream_remote == "." {
                                format!("refs/heads/{upstream_branch}")
                            } else {
                                format!("refs/remotes/{upstream_ref}")
                            },
                        )
                        .await?;
                    }
                }
                self.execute_command(
                    if had_upstream {
                        "GitVcsDriver.pushCurrentBranch.pushOwnBranch"
                    } else {
                        "GitVcsDriver.pushCurrentBranch.pushWithUpstream"
                    },
                    &["push", "--", &remote, &format!("HEAD:refs/heads/{publish}")],
                    ExecuteOptions {
                        timeout: None,
                        ..Default::default()
                    },
                )
                .await?;
                return Ok(result(
                    GitPushStepStatus::Pushed,
                    Some(format!("{remote}/{publish}")),
                    Some(self.record_pushed_upstream(branch, &remote, &publish).await),
                ));
            }
            self.execute_command(
                "GitVcsDriver.pushCurrentBranch.pushUpstream",
                &["push", "--", &remote, &format!("HEAD:refs/heads/{publish}")],
                ExecuteOptions {
                    timeout: None,
                    ..Default::default()
                },
            )
            .await?;
            return Ok(result(
                GitPushStepStatus::Pushed,
                destination.upstream.map(|(_, _, upstream)| upstream),
                Some(false),
            ));
        }
        self.execute_command(
            "GitVcsDriver.pushCurrentBranch.push",
            &["push"],
            ExecuteOptions {
                timeout: None,
                ..Default::default()
            },
        )
        .await?;
        Ok(result(
            GitPushStepStatus::Pushed,
            details.upstream_ref.clone(),
            Some(false),
        ))
    }
    pub async fn refresh_checked_out_branch(
        &self,
        target: &str,
        reset_when_head: Option<&str>,
    ) -> Result<CheckoutRefresh, GitError> {
        let resolved = self.resolve_commit(target).await?;
        let target = resolved.as_str();
        let result = async {
            let head = self.resolve_commit("HEAD").await?;
            if head == target {
                return Ok(CheckoutRefresh {
                    head_commit: head,
                    moved: false,
                    on_target: true,
                });
            }
            if !self
                .core_text(
                    "GitVcsDriver.refreshCheckedOutBranch.status",
                    &["status", "--porcelain"],
                    false,
                )
                .await?
                .trim()
                .is_empty()
            {
                return Ok(CheckoutRefresh {
                    head_commit: head,
                    moved: false,
                    on_target: false,
                });
            }
            let ancestor = self
                .execute_command(
                    "GitVcsDriver.refreshCheckedOutBranch.isAncestor",
                    &["merge-base", "--is-ancestor", &head, target],
                    ExecuteOptions {
                        allow_nonzero: true,
                        ..Default::default()
                    },
                )
                .await?
                .code
                == Some(0);
            if !ancestor && Some(head.as_str()) != reset_when_head {
                return Ok(CheckoutRefresh {
                    head_commit: head,
                    moved: false,
                    on_target: false,
                });
            }
            if !ancestor {
                self.execute_command(
                    "GitVcsDriver.refreshCheckedOutBranch.keepPrevious",
                    &["update-ref", "refs/t3code/pre-refresh", &head],
                    ExecuteOptions::default(),
                )
                .await?;
            }
            self.execute_command(
                "GitVcsDriver.refreshCheckedOutBranch.move",
                &[
                    if ancestor { "merge" } else { "reset" },
                    if ancestor { "--ff-only" } else { "--merge" },
                    target,
                ],
                ExecuteOptions {
                    timeout: None,
                    ..Default::default()
                },
            )
            .await?;
            Ok(CheckoutRefresh {
                head_commit: target.into(),
                moved: true,
                on_target: true,
            })
        }
        .await;
        self.invalidate_caches().await;
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRefresh {
    pub head_commit: String,
    pub moved: bool,
    pub on_target: bool,
}
