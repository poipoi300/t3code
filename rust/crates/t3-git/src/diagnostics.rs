use crate::{ExecuteOptions, Git, GitError, OutputMode};
use regex::Regex;
use std::{path::Path, sync::OnceLock};
use t3_contracts::{git::GitCommandFailureReason as Reason, vcs::VcsProcessExitFailureKind};
use t3_process::{ProcessError, ProcessOutput};

pub const INDEX_LOCKED_DETAIL: &str =
    "Git's index is locked. Wait for other Git operations to finish, then try again.";

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct VcsFailure {
    pub error: t3_contracts::vcs::VcsError,
    message: String,
}
impl VcsFailure {
    pub(crate) fn exit(
        operation: &str,
        cwd: &Path,
        argument_count: usize,
        output: &ProcessOutput,
        custom: Option<VcsProcessExitFailureKind>,
        not_found: Option<&str>,
    ) -> Self {
        let kind = classify_process_failure(&output.stderr_text(), custom);
        let detail = if index_locked(&output.stderr_text()) {
            INDEX_LOCKED_DETAIL
        } else {
            match kind {
                VcsProcessExitFailureKind::Authentication => "Authentication failed.",
                VcsProcessExitFailureKind::RateLimited => "API rate limit exceeded.",
                VcsProcessExitFailureKind::NotFound => {
                    not_found.unwrap_or("VCS resource not found.")
                }
                VcsProcessExitFailureKind::CommandFailed => {
                    "Process exited with a non-zero status."
                }
            }
        };
        let retryable =
            kind == VcsProcessExitFailureKind::CommandFailed && transient(&output.stderr_text());
        let value = serde_json::json!({"_tag":"VcsProcessExitError", "operation":operation,"command":"git","cwd":cwd.to_string_lossy(),"argumentCount":argument_count,"exitCode":output.code.unwrap_or(0),"detail":detail,"failureKind":kind,"stderrLength":output.stderr_text().encode_utf16().count(),"stderrTruncated":output.stderr_truncated});
        let mut value = value;
        if retryable {
            value["retryable"] = true.into();
        }
        Self::from_value(
            value,
            format!(
                "VCS process failed in {operation}: git ({}) exited with {} - {detail}",
                cwd.display(),
                output.code.unwrap_or(0)
            ),
        )
    }
    fn from_value(value: serde_json::Value, message: String) -> Self {
        let operation = value["operation"].as_str().unwrap_or("Git.execute");
        let command = value["command"].as_str().unwrap_or("git");
        let cwd = value["cwd"].as_str().unwrap_or("");
        let message = match value["_tag"].as_str() {
            Some("VcsProcessSpawnError") => {
                format!("VCS process failed to spawn in {operation}: {command} ({cwd})")
            }
            Some("VcsProcessExitError") => format!(
                "VCS process failed in {operation}: {command} ({cwd}) exited with {} - {}",
                value["exitCode"],
                value["detail"].as_str().unwrap_or("")
            ),
            Some("VcsProcessTimeoutError") => format!(
                "VCS process timed out in {operation}: {command} ({cwd}) after {}ms",
                value["timeoutMs"]
            ),
            Some("VcsProcessStdinWriteError") => format!(
                "VCS process failed to write {} bytes to stdin in {operation}: {command} ({cwd})",
                value["stdinBytes"]
            ),
            Some("VcsProcessOutputReadError") => format!(
                "VCS process failed to read {} in {operation}: {command} ({cwd})",
                value["stream"].as_str().unwrap_or("")
            ),
            Some("VcsProcessOutputLimitError") => format!(
                "VCS process {} produced {} bytes in {operation}: {command} ({cwd}), exceeding the {} byte limit",
                value["stream"].as_str().unwrap_or(""),
                value["observedBytes"],
                value["maxBytes"]
            ),
            Some("VcsProcessMissingExitCodeError") => format!(
                "VCS process completed without an exit code in {operation}: {command} ({cwd})"
            ),
            Some("VcsUnsupportedOperationError") => format!(
                "VCS operation is unsupported for {} in {operation}: {}",
                value["kind"].as_str().unwrap_or("unknown"),
                value["detail"].as_str().unwrap_or("")
            ),
            _ => message,
        };
        // This fallback also contains only fixed diagnostic text; it cannot leak captured output.
        let error = serde_json::from_value(value).unwrap_or_else(|_| t3_contracts::vcs::VcsError::MissingExitCode(t3_contracts::vcs::VcsProcessMissingExitCodeError {
            _tag: t3_contracts::vcs::VcsProcessMissingExitCodeErrorTag::VcsProcessMissingExitCodeError, operation: "Git.execute".into(), command: "git".into(), cwd: String::new(), argument_count: None,
        }));
        Self { error, message }
    }
    pub(crate) fn unsupported(
        operation: &str,
        kind: t3_contracts::vcs::VcsDriverKind,
        detail: &str,
    ) -> Self {
        Self::from_value(
            serde_json::json!({"_tag":"VcsUnsupportedOperationError","operation":operation,"kind":kind,"detail":detail}),
            detail.into(),
        )
    }
    pub(crate) fn process(
        operation: &str,
        cwd: &Path,
        argument_count: usize,
        input_bytes: usize,
        error: ProcessError,
    ) -> Self {
        let mut value = serde_json::json!({"operation":operation,"command":"git","cwd":cwd.to_string_lossy(),"argumentCount":argument_count});
        let (tag, detail) = match error {
            ProcessError::Timeout { timeout } => {
                value["timeoutMs"] = (timeout.as_millis() as u64).into();
                ("VcsProcessTimeoutError", "VCS process timed out.")
            }
            ProcessError::OutputLimit {
                stream,
                max_bytes,
                observed_bytes,
            } => {
                value["stream"] = stream.into();
                value["maxBytes"] = max_bytes.into();
                value["observedBytes"] = observed_bytes.into();
                (
                    "VcsProcessOutputLimitError",
                    "VCS process output limit exceeded.",
                )
            }
            ProcessError::Io {
                stream: "stdin", ..
            } => {
                value["stdinBytes"] = input_bytes.into();
                value["cause"] = serde_json::Value::Null;
                (
                    "VcsProcessStdinWriteError",
                    "VCS process failed to write stdin.",
                )
            }
            ProcessError::Io { stream, .. } => {
                value["stream"] = stream.into();
                value["cause"] = serde_json::Value::Null;
                (
                    "VcsProcessOutputReadError",
                    "VCS process failed to read output.",
                )
            }
            _ => {
                value["cause"] = serde_json::Value::Null;
                ("VcsProcessSpawnError", "VCS process failed to spawn.")
            }
        };
        value["_tag"] = tag.into();
        Self::from_value(
            value,
            format!("{detail} {operation}: git ({})", cwd.display()),
        )
    }
    pub(crate) fn missing_exit(operation: &str, cwd: &Path, argument_count: usize) -> Self {
        Self::from_value(
            serde_json::json!({"_tag":"VcsProcessMissingExitCodeError","operation":operation,"command":"git","cwd":cwd.to_string_lossy(),"argumentCount":argument_count}),
            format!(
                "VCS process completed without an exit code in {operation}: git ({})",
                cwd.display()
            ),
        )
    }
}

fn matches(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).is_ok_and(|regex| regex.is_match(text))
}

pub fn is_non_repository(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("not a git repository")
        || lower.contains("gitfile does not point to a valid repository")
}

pub fn classify_git_failure(stderr: &str) -> Option<Reason> {
    static PATTERNS: OnceLock<Vec<(Regex, Reason)>> = OnceLock::new();
    let diagnostics = stderr.lines().filter(|line| {
        matches(r"^(?:fatal|error):|^!\s|^\s+!\s", line)
            || (!line.starts_with("remote:") && matches(r"(?i)Permission denied \((?:publickey|password|keyboard-interactive)|Permission denied, please try again|Host key verification failed", line))
    }).collect::<Vec<_>>().join("\n");
    if diagnostics.is_empty() {
        return None;
    }
    if is_non_repository(&diagnostics) {
        return Some(Reason::NotARepository);
    }
    let patterns = PATTERNS.get_or_init(|| [
        (r"(?i)would clobber existing tag", Reason::TagWouldBeClobbered),
        (r"(?i)is already (?:used by worktree at|checked out at)", Reason::BranchCheckedOutInWorktree),
        (r"(?i)a branch named .+ already exists", Reason::BranchAlreadyExists),
        (r"(?i)'refs/[^'\n]+' exists; cannot create 'refs/", Reason::RefNameConflict),
        (r"(?i)could not lock config file .+: File exists", Reason::ConfigLocked),
        (r"(?i)(?:authentication failed|could not read Username|could not read Password|permission denied \([a-z-]+(?:,[a-z-]+)*\)|permission denied, please try again)", Reason::AuthenticationFailed),
        (r"(?i)host key verification failed", Reason::HostKeyUnverified),
        (r"(?i)(?:could not read from remote repository|does not appear to be a git repository|repository .+ not found)", Reason::RemoteUnreachable),
        (r"(?i)fatal: '[^']+' already exists|destination path .+ already exists", Reason::PathAlreadyExists),
        (r"(?im)^fatal: (?:invalid reference: |not a valid object name|needed a single revision|bad revision |'.+' is not a commit and a branch '.+' cannot be created from it)", Reason::RefNotFound),
        (r"(?im)^[ \t]*![ \t]+\[remote rejected\]", Reason::PushRejectedByRemote),
        (r"(?im)^[ \t]*![ \t]+\[rejected\][^\n]*\((?:non-fast-forward|fetch first)\)[\s\S]*^error: failed to push some refs", Reason::PushRejectedNonFastForward),
    ].into_iter().filter_map(|(pattern, reason)| Regex::new(pattern).ok().map(|regex| (regex, reason))).collect());
    patterns.iter().find_map(|(regex, reason)| {
        if !regex.is_match(&diagnostics)
            || (*reason == Reason::RefNotFound
                && matches(
                    r"(?im)^error: .+(?:, not a commit|expected commit type)",
                    &diagnostics,
                ))
        {
            None
        } else {
            Some(reason.clone())
        }
    })
}

pub fn classify_process_failure(
    stderr: &str,
    custom: Option<VcsProcessExitFailureKind>,
) -> VcsProcessExitFailureKind {
    let lower = stderr.to_lowercase();
    if custom == Some(VcsProcessExitFailureKind::Authentication)
        || [
            "authentication failed",
            "not logged in",
            "no oauth token",
            "unauthorized",
        ]
        .iter()
        .any(|value| lower.contains(value))
    {
        VcsProcessExitFailureKind::Authentication
    } else if custom == Some(VcsProcessExitFailureKind::RateLimited)
        || [
            "api rate limit",
            "rate limit exceeded",
            "secondary rate limit",
            "too many requests",
            "http 429",
        ]
        .iter()
        .any(|value| lower.contains(value))
    {
        VcsProcessExitFailureKind::RateLimited
    } else {
        custom.unwrap_or(VcsProcessExitFailureKind::CommandFailed)
    }
}

pub(crate) fn transient(stderr: &str) -> bool {
    matches(
        r#"(?i)unable to create [^\n]*\.lock['"]?: file exists|(?:unable to stat|lstat\(|error: open\()[^\n]+: no such file or directory"#,
        stderr,
    )
}
pub(crate) fn index_locked(stderr: &str) -> bool {
    matches(
        r#"(?im)^fatal: unable to create ['"][^\r\n]*[/\\]index\.lock['"]: file exists\.?$"#,
        stderr,
    )
}
pub(crate) fn ref_locked(stderr: &str) -> bool {
    stderr.lines().any(|line| matches(r#"(?i)^(?:(?:error|fatal): cannot lock ref\b|fatal: Unable to create ['"].+\.lock['"]:|error: fetching ref \S+ failed: (?:incorrect old value provided|reference already exists)$)"#, line.trim()))
}
pub(crate) fn fetch_detail(stderr: &str) -> Option<&'static str> {
    if stderr.lines().any(|line| matches(r"(?i)^(?:fatal: (?:Authentication failed|could not read (?:Username|Password))\b|\S+: Permission denied \(publickey)", line.trim())) {
        Some("Git could not authenticate with the remote. Check Git credentials or SSH access on the server, then retry.")
    } else if stderr.lines().any(|line| matches(r"(?i)^(?:(?:fatal: |ssh: )?Could not resolve host(?:name)?\b|fatal: unable to access .+: (?:Could not resolve host|Failed to connect)\b|ssh: connect to host \S+ port \d+: (?:Connection timed out|Connection refused|Network is unreachable)\b)", line.trim())) {
        Some("Git could not reach the remote. Check the server's network connection and remote host, then retry.")
    } else if stderr.lines().any(|line| matches(r"(?i)^(?:remote: Repository not found\.?$|fatal: repository .+ not found$|fatal: .+ does not appear to be a git repository$)", line.trim())) {
        Some("Git could not access the remote repository. Check the remote URL and repository permissions on the server.")
    } else if ref_locked(stderr) { Some("Git could not update a local reference. Another Git operation or a stale lock may be blocking the fetch; check the repository on the server, then retry.") } else { None }
}

#[derive(Debug, Clone)]
pub struct CommandError {
    pub operation: String,
    pub command: String,
    pub cwd: String,
    pub argument_count: usize,
    pub exit_code: Option<i32>,
    pub reason: Option<Reason>,
    pub detail: String,
    pub stdout_length: usize,
    pub stderr_length: usize,
}
impl std::error::Error for CommandError {}
impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Git command failed in {} ({}): {}",
            self.operation, self.cwd, self.detail
        )?;
        if let Some(reason) = &self.reason {
            let label = match reason {
                Reason::AuthenticationFailed => "authentication_failed",
                Reason::BranchAlreadyExists => "branch_already_exists",
                Reason::BranchCheckedOutInWorktree => "branch_checked_out_in_worktree",
                Reason::ConfigLocked => "config_locked",
                Reason::HostKeyUnverified => "host_key_unverified",
                Reason::NotARepository => "not_a_repository",
                Reason::PathAlreadyExists => "path_already_exists",
                Reason::PushRejectedByRemote => "push_rejected_by_remote",
                Reason::PushRejectedNonFastForward => "push_rejected_non_fast_forward",
                Reason::RefNameConflict => "ref_name_conflict",
                Reason::RefNotFound => "ref_not_found",
                Reason::RemoteUnreachable => "remote_unreachable",
                Reason::TagWouldBeClobbered => "tag_would_be_clobbered",
            };
            write!(formatter, " ({label})")?;
        }
        Ok(())
    }
}
impl CommandError {
    pub(crate) fn new(
        operation: &str,
        cwd: &Path,
        argument_count: usize,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            operation: operation.into(),
            command: "git".into(),
            cwd: cwd.to_string_lossy().into_owned(),
            argument_count,
            exit_code: None,
            reason: None,
            detail: sanitize_diagnostic(&detail.into()),
            stdout_length: 0,
            stderr_length: 0,
        }
    }
    pub(crate) fn exit(
        operation: &str,
        cwd: &Path,
        args: &[impl AsRef<std::ffi::OsStr>],
        output: &ProcessOutput,
        fallback: Option<&str>,
    ) -> Self {
        let reason = classify_git_failure(&output.stderr_text());
        let detail = fallback.unwrap_or(match reason {
            Some(Reason::PushRejectedByRemote) => "The remote rejected the push. Push this branch from a terminal to see the server's message.",
            Some(Reason::PushRejectedNonFastForward) => "The remote has commits this branch doesn't have. Pull or rebase, then push again.",
            Some(Reason::ConfigLocked) => "Another Git process kept the repository's config locked. Wait for it to finish, then retry.",
            _ => "Git command exited with a non-zero status.",
        });
        Self {
            exit_code: output.code,
            reason,
            stdout_length: output.stdout_text().encode_utf16().count(),
            stderr_length: output.stderr_text().encode_utf16().count(),
            ..Self::new(operation, cwd, args.len(), detail)
        }
    }
    pub fn to_contract(&self) -> Result<t3_contracts::git::GitCommandError, serde_json::Error> {
        serde_json::from_value(
            serde_json::json!({ "_tag": "GitCommandError", "operation": self.operation, "command": self.command, "cwd": self.cwd, "argumentCount": self.argument_count, "exitCode": self.exit_code, "reason": self.reason, "detail": self.detail, "stdoutLength": self.stdout_length, "stderrLength": self.stderr_length }),
        )
    }
}

impl Git {
    pub(crate) fn vcs_exit(
        &self,
        operation: &str,
        argument_count: usize,
        output: &ProcessOutput,
    ) -> GitError {
        if output.code.is_none() {
            VcsFailure::missing_exit(operation, self.cwd(), argument_count).into()
        } else {
            VcsFailure::exit(operation, self.cwd(), argument_count, output, None, None).into()
        }
    }
    pub async fn execute_vcs(
        &self,
        operation: &str,
        args: &[impl AsRef<std::ffi::OsStr>],
        mut options: ExecuteOptions,
        classifier: Option<fn(&str) -> Option<VcsProcessExitFailureKind>>,
        not_found_detail: Option<&str>,
    ) -> Result<ProcessOutput, GitError> {
        let allow = options.allow_nonzero;
        options.allow_nonzero = true;
        let bytes = options.stdin.as_ref().map_or(0, Vec::len);
        let output = self
            .execute(args, options)
            .await
            .map_err(|error| match error {
                GitError::Process(ProcessError::Cancelled) => error,
                GitError::Process(error) => {
                    VcsFailure::process(operation, self.cwd(), args.len(), bytes, error).into()
                }
                error => error,
            })?;
        if output.code.is_none() {
            return Err(VcsFailure::missing_exit(operation, self.cwd(), args.len()).into());
        }
        if !allow && output.code != Some(0) {
            return Err(VcsFailure::exit(
                operation,
                self.cwd(),
                args.len(),
                &output,
                classifier.and_then(|classify| classify(&output.stderr_text())),
                not_found_detail,
            )
            .into());
        }
        Ok(output)
    }
    pub(crate) fn command_error(&self, operation: &str, detail: impl Into<String>) -> GitError {
        CommandError::new(operation, self.cwd(), 0, detail).into()
    }
    pub async fn execute_command(
        &self,
        operation: &str,
        args: &[impl AsRef<std::ffi::OsStr>],
        mut options: ExecuteOptions,
    ) -> Result<ProcessOutput, GitError> {
        let allow_nonzero = options.allow_nonzero;
        let fallback = options.fallback_error_detail.or(match operation {
            "GitVcsDriver.createBranch"
            | "GitVcsDriver.createRef"
            | "GitVcsDriver.switchRef.createTrackingBranch"
            | "GitVcsDriver.fetchRemoteBranch.materialize" => Some("git branch create failed"),
            "GitVcsDriver.switchRef.checkout" => Some("git checkout failed"),
            "GitVcsDriver.deleteLocalBranch" => Some("git branch delete failed"),
            "GitVcsDriver.renameBranch" => Some("git branch rename failed"),
            "GitVcsDriver.initRepo" => Some("git init failed"),
            "GitVcsDriver.fetchRefIntoBranch" => Some("git fetch ref into branch failed"),
            "GitVcsDriver.fetchRefCommit" => Some("git fetch ref failed"),
            "GitVcsDriver.refreshCheckedOutBranch.keepPrevious" => {
                Some("git failed to record the previous checkout commit")
            }
            "GitVcsDriver.refreshCheckedOutBranch.move" => {
                Some("git failed to move the checkout onto the pull request head")
            }
            _ => None,
        });
        options.unbounded_concurrency |= options
            .timeout
            .is_none_or(|timeout| timeout > std::time::Duration::from_secs(30));
        options.allow_nonzero = true;
        if !options.append_truncation_marker {
            options.output_mode = OutputMode::Error;
        }
        options
            .env
            .entry("GIT_OPTIONAL_LOCKS".into())
            .or_insert_with(|| Some("0".into()));
        #[cfg(windows)]
        {
            // The environment config is appended by execute after safe.bareRepository.
            options.long_paths = true;
        }
        let output = self
            .execute(args, options)
            .await
            .map_err(|error| match error {
                GitError::RepositoryChanged { .. } => error,
                GitError::Process(ProcessError::Cancelled) => error,
                GitError::Process(ProcessError::Timeout { .. }) => {
                    self.command_error(operation, "Git command timed out.")
                }
                GitError::Process(ProcessError::OutputLimit { max_bytes, .. }) => self
                    .command_error(
                        operation,
                        format!("Git output exceeded {max_bytes} bytes and was truncated."),
                    ),
                GitError::Process(ProcessError::Io {
                    stream: "stdin", ..
                }) => self.command_error(operation, "Failed to write Git process input."),
                GitError::Process(ProcessError::Io { .. }) => {
                    self.command_error(operation, "Failed to read Git process output.")
                }
                _ => self.command_error(operation, "Failed to spawn Git process."),
            })?;
        if !allow_nonzero && output.code != Some(0) {
            return Err(CommandError::exit(operation, self.cwd(), args, &output, fallback).into());
        }
        Ok(output)
    }
    pub(crate) async fn core_text(
        &self,
        operation: &str,
        args: &[&str],
        allow_nonzero: bool,
    ) -> Result<String, GitError> {
        Ok(self
            .execute_command(
                operation,
                args,
                ExecuteOptions {
                    allow_nonzero,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text())
    }
}

pub(crate) fn sanitize_diagnostic(text: &str) -> String {
    static URLS: OnceLock<Option<Regex>> = OnceLock::new();
    let urls = URLS.get_or_init(|| Regex::new(r#"(?i)[a-z][a-z0-9+.-]*://[^\s<>"']+"#).ok());
    urls.as_ref().map_or_else(
        || "[redacted diagnostic]".into(),
        |urls| urls.replace_all(text, "[redacted URL]").into_owned(),
    )
}
