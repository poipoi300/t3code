use crate::{
    ExecuteOptions, Git, GitError,
    core::{METADATA_LIMIT, unborn},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    time::Duration,
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString as Text, git::VcsStatusLocalResultBranchChanges, review::*,
    vcs::EncodedNumber, wire::WireDateTime,
};
use t3_process::Environment;
use tokio::time::Instant;

const REVIEW_ARGS: &[&str] = &[
    "-c",
    "diff.suppressBlankEmpty=false",
    "diff",
    "--find-renames",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--submodule=short",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];
const FILE_LIMIT: usize = 1024 * 1024;

struct ReviewIndex {
    env: Environment,
    _directory: Option<tempfile::TempDir>,
    unavailable: Option<String>,
}
impl ReviewIndex {
    fn empty() -> Self {
        Self {
            env: Environment::new(),
            _directory: None,
            unavailable: None,
        }
    }
    fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            unavailable: Some(detail.into()),
            ..Self::empty()
        }
    }
}
fn numstat(output: &str) -> Vec<ReviewDiffFileStat> {
    let mut fields = output.split('\0');
    let mut files = Vec::new();
    while let Some(record) = fields.next() {
        let mut parts = record.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let number = |value: &str| {
            if value == "-" {
                Some(0u64)
            } else {
                value.parse().ok()
            }
        };
        let (Some(add), Some(del)) = (number(add), number(del)) else {
            continue;
        };
        let (path, previous_path) = if path.is_empty() {
            let previous = fields.next();
            (fields.next().unwrap_or(""), previous.map(str::to_owned))
        } else {
            (path, None)
        };
        if !path.is_empty() {
            files.push(ReviewDiffFileStat {
                path: path.into(),
                previous_path,
                additions: EncodedNumber::Finite(add.into()),
                deletions: EncodedNumber::Finite(del.into()),
            });
        }
    }
    files
}
fn count(number: &EncodedNumber) -> u64 {
    match number {
        EncodedNumber::Finite(number) => number.as_u64().unwrap_or(0),
        _ => 0,
    }
}
fn valid_sha(value: &str) -> bool {
    [40, 64].contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn hash_diff(diff: &str, files: &[ReviewDiffFileStat]) -> Result<String, GitError> {
    let input = serde_json::to_vec(&(diff, files))
        .map_err(|_| GitError::InvalidOutput("review diff serialization failed".into()))?;
    Ok(Sha256::digest(input)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
struct ReadDiff {
    patch: String,
    files: Option<Vec<ReviewDiffFileStat>>,
    truncated: bool,
}
impl ReadDiff {
    fn empty() -> Self {
        Self {
            patch: String::new(),
            files: Some(vec![]),
            truncated: false,
        }
    }
}

impl Git {
    pub(crate) async fn review_merge_base(
        &self,
        branch: Option<&str>,
        explicit: Option<&str>,
        default: Option<&str>,
    ) -> Result<(Option<String>, String), GitError> {
        let base = if let Some(reference) = explicit {
            let comparison =
                if reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/") {
                    reference.into()
                } else if reference.contains('/')
                    && !reference.starts_with("refs/")
                    && self
                        .ref_exists(&format!("refs/remotes/{reference}"))
                        .await?
                {
                    format!("refs/remotes/{reference}")
                } else if !reference.starts_with("refs/")
                    && self.ref_exists(&format!("refs/heads/{reference}")).await?
                {
                    format!("refs/heads/{reference}")
                } else {
                    reference.into()
                };
            Some((reference.into(), comparison))
        } else if let Some(branch) = branch {
            self.comparison_base(branch, true, default)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let Some((display, reference)) = base else {
            return Ok((None, "HEAD".into()));
        };
        let output = self
            .execute_command(
                "GitVcsDriver.resolveReviewMergeBase",
                &["merge-base", "--end-of-options", &reference, "HEAD"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        if output.code != Some(0) || output.stdout_text().trim().is_empty() {
            if explicit.is_none()
                && self
                    .execute_command(
                        "GitVcsDriver.resolveReviewMergeBase.head",
                        &["rev-parse", "--verify", "--quiet", "HEAD"],
                        ExecuteOptions {
                            allow_nonzero: true,
                            ..Default::default()
                        },
                    )
                    .await?
                    .code
                    != Some(0)
            {
                return Ok((None, "HEAD".into()));
            }
            return Err(self.command_error(
                "GitVcsDriver.resolveReviewMergeBase",
                format!("Could not find a common commit between '{display}' and HEAD."),
            ));
        }
        Ok((Some(display), output.stdout_text().trim().into()))
    }
    pub(crate) async fn empty_tree_hash(&self) -> Result<String, GitError> {
        Ok(self
            .core_text(
                "GitVcsDriver.review.emptyTree",
                &[
                    "hash-object",
                    "-t",
                    "tree",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                ],
                false,
            )
            .await?
            .trim()
            .into())
    }
    async fn prepare_review_index(
        &self,
        path_args: &[String],
        only_path: Option<&str>,
        max_bytes: Option<u64>,
    ) -> Result<ReviewIndex, GitError> {
        let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
        args.extend(path_args.iter().map(String::as_str));
        let untracked = match self
            .execute_command(
                "GitVcsDriver.review.listUntracked",
                &args,
                ExecuteOptions {
                    max_output_bytes: METADATA_LIMIT,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(output) => output,
            Err(GitError::Command(error)) => {
                return Ok(ReviewIndex::unavailable(format!(
                    "Could not list untracked files: {}",
                    error.detail
                )));
            }
            Err(error) => return Err(error),
        };
        let paths: Vec<String> = untracked
            .stdout_text()
            .split('\0')
            .filter(|path| {
                !path.is_empty()
                    && only_path.is_none_or(|only| *path == only || *path == format!("{only}/"))
            })
            .map(str::to_owned)
            .collect();
        if paths.is_empty() {
            return Ok(ReviewIndex::empty());
        }
        if paths.len() > 5000 {
            return Ok(ReviewIndex::unavailable(
                "Too many untracked files to count.",
            ));
        }
        let removed = self
            .execute_command(
                "GitVcsDriver.readUnifiedWorkingTreeReviewDiff.stagedDeletions",
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--cached",
                    "--no-renames",
                    "--name-only",
                    "--diff-filter=D",
                    "-z",
                    "HEAD",
                    "--",
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    max_output_bytes: METADATA_LIMIT,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text();
        let removed: BTreeSet<_> = removed
            .split('\0')
            .filter(|path| !path.is_empty())
            .collect();
        let mut paths: Vec<_> = paths
            .into_iter()
            .filter(|path| !removed.contains(path.as_str()))
            .collect();
        if paths.is_empty() {
            return Ok(ReviewIndex::empty());
        }
        let omitted;
        {
            let mut sizes = Vec::new();
            for path in &paths {
                let size = tokio::fs::symlink_metadata(self.cwd().join(path))
                    .await
                    .ok()
                    .filter(|info| info.is_file())
                    .map_or(0, |info| info.len());
                sizes.push((path.clone(), size));
            }
            sizes.sort_by_key(|(_, size)| *size);
            let mut bytes = 0u64;
            let selected: Vec<_> = sizes
                .into_iter()
                .take_while(|(_, size)| {
                    bytes = bytes.saturating_add(*size);
                    bytes <= max_bytes.unwrap_or(256 * 1024 * 1024)
                })
                .map(|(path, _)| path)
                .collect();
            omitted = selected.len() < paths.len();
            if omitted && (selected.is_empty() || max_bytes.is_some()) {
                return Ok(ReviewIndex::unavailable(
                    "Untracked files are too large to count.",
                ));
            }
            paths = selected;
        }
        let index = self
            .core_text(
                "GitVcsDriver.readUnifiedWorkingTreeReviewDiff.indexPath",
                &["rev-parse", "--git-path", "index"],
                false,
            )
            .await?;
        let index = self.absolute_git_path(index.trim())?;
        let directory = tempfile::Builder::new()
            .prefix("t3code-review-index-")
            .tempdir()?;
        let temporary = directory.path().join("index");
        let mut environment = Environment::new();
        environment.insert(
            "GIT_INDEX_FILE".into(),
            Some(temporary.as_os_str().to_owned()),
        );
        if index.try_exists()? {
            tokio::fs::copy(&index, &temporary).await?;
            let time = filetime::FileTime::from_last_modification_time(&std::fs::metadata(&index)?);
            let time = filetime::FileTime::from_unix_time(time.unix_seconds().max(0), 0);
            filetime::set_file_times(&temporary, time, time)?;
        } else {
            self.execute_command(
                "GitVcsDriver.review.emptyIndex",
                &["read-tree", "--empty"],
                ExecuteOptions {
                    env: environment.clone(),
                    ..Default::default()
                },
            )
            .await?;
        }
        let config = [
            "-c",
            "core.splitIndex=false",
            "-c",
            "splitIndex.sharedIndexExpire=never",
        ];
        let mut args = config.to_vec();
        args.extend(["update-index", "--no-split-index"]);
        self.execute_command(
            "GitVcsDriver.readUnifiedWorkingTreeReviewDiff.expandSplitIndex",
            &args,
            ExecuteOptions {
                env: environment.clone(),
                ..Default::default()
            },
        )
        .await?;
        let mut args = config.to_vec();
        args.extend([
            "-c",
            "core.preloadIndex=false",
            "-c",
            "core.sparseCheckout=false",
            "--literal-pathspecs",
            "add",
            "--intent-to-add",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ]);
        self.execute_command(
            "GitVcsDriver.readUnifiedWorkingTreeReviewDiff.addUntracked",
            &args,
            ExecuteOptions {
                env: environment.clone(),
                stdin: Some(format!("{}\0", paths.join("\0")).into_bytes()),
                ..Default::default()
            },
        )
        .await?;
        Ok(ReviewIndex {
            env: environment,
            _directory: Some(directory),
            unavailable: omitted.then(|| "Untracked files are too large to count.".into()),
        })
    }
    pub(crate) async fn branch_change_totals(
        &self,
        branch: Option<&str>,
        default: Option<&str>,
        roots: &[String],
    ) -> Result<VcsStatusLocalResultBranchChanges, GitError> {
        let branch = branch.map(str::to_owned);
        let default = default.map(str::to_owned);
        let roots = roots.to_vec();
        self.owned_operation(move |git| async move {
            git.branch_change_totals_owned(branch.as_deref(), default.as_deref(), &roots)
                .await
        })
        .await
    }
    async fn branch_change_totals_owned(
        &self,
        branch: Option<&str>,
        default: Option<&str>,
        roots: &[String],
    ) -> Result<VcsStatusLocalResultBranchChanges, GitError> {
        let (mut base, mut merge) = self.review_merge_base(branch, None, default).await?;
        if let Some(branch) = branch
            && !self.ref_exists(&format!("refs/heads/{branch}")).await?
        {
            base = None;
            merge = "HEAD".into();
        }
        let root_key = roots.join("\0");
        let remembered = self
            .core
            .untracked_failures
            .lock()
            .await
            .get(self.cwd())
            .cloned();
        if let Some((key, _, retry_at, detail)) = &remembered
            && *key == root_key
            && Instant::now() < *retry_at
        {
            return Err(self.command_error("GitVcsDriver.readBranchChangeTotals", detail));
        }
        let paths = if roots.len() <= 128
            && roots.iter().map(|root| root.len() + 16).sum::<usize>() < 8000
        {
            roots
                .iter()
                .map(|root| format!(":(top,literal){root}"))
                .collect()
        } else {
            vec![]
        };
        let review = self
            .prepare_review_index(&paths, None, Some(256 * 1024 * 1024))
            .await?;
        if let Some(detail) = review.unavailable {
            let count = remembered
                .filter(|(key, _, _, _)| *key == root_key)
                .map_or(1, |(_, count, _, _)| count.saturating_add(1));
            let seconds = 60u64
                .saturating_mul(1u64.checked_shl(count - 1).unwrap_or(u64::MAX))
                .min(1800);
            let mut failures = self.core.untracked_failures.lock().await;
            if failures.len() >= 2048 && !failures.contains_key(self.cwd()) {
                failures.pop_first();
            }
            failures.insert(
                self.cwd().to_owned(),
                (
                    root_key,
                    count,
                    Instant::now() + Duration::from_secs(seconds),
                    detail.clone(),
                ),
            );
            return Err(self.command_error("GitVcsDriver.readBranchChangeTotals", detail));
        }
        self.core.untracked_failures.lock().await.remove(self.cwd());
        let options = ExecuteOptions {
            allow_nonzero: true,
            max_output_bytes: METADATA_LIMIT,
            env: review.env.clone(),
            ..Default::default()
        };
        let mut result = self
            .execute_command(
                "GitVcsDriver.readBranchChangeTotals",
                &[
                    "diff-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-M",
                    "--numstat",
                    "-z",
                    &merge,
                    "--",
                ],
                options.clone(),
            )
            .await?;
        if result.code != Some(0) && merge == "HEAD" && unborn(&result.stderr_text()) {
            merge = self.empty_tree_hash().await?;
            result = self
                .execute_command(
                    "GitVcsDriver.readBranchChangeTotals",
                    &[
                        "diff-index",
                        "--no-ext-diff",
                        "--no-textconv",
                        "-M",
                        "--numstat",
                        "-z",
                        &merge,
                        "--",
                    ],
                    options,
                )
                .await?;
        }
        if result.code != Some(0) {
            return Err(self.command_error(
                "GitVcsDriver.readBranchChangeTotals",
                "Could not read Changes totals.",
            ));
        }
        let files = numstat(&result.stdout_text());
        Ok(VcsStatusLocalResultBranchChanges {
            base_ref: base.map(Text),
            insertions: files.iter().map(|file| count(&file.additions)).sum(),
            deletions: files.iter().map(|file| count(&file.deletions)).sum(),
        })
    }
    async fn review_commit(&self, sha: &str) -> Result<(String, String), GitError> {
        let invalid = || {
            self.command_error(
                "GitVcsDriver.reviewCommit",
                "Selected commit is invalid or no longer available.",
            )
        };
        if !valid_sha(sha) {
            return Err(invalid());
        }
        let options = ExecuteOptions {
            allow_nonzero: true,
            env: crate::env(&[("GIT_NO_LAZY_FETCH", "1")]),
            ..Default::default()
        };
        if self
            .execute_command(
                "GitVcsDriver.reviewCommit.present",
                &["cat-file", "-e", &format!("{sha}^{{commit}}")],
                options.clone(),
            )
            .await?
            .code
            != Some(0)
        {
            return Err(invalid());
        }
        let object = self
            .core_text(
                "GitVcsDriver.reviewCommit.parents",
                &["cat-file", "commit", sha],
                false,
            )
            .await?;
        let parent = object
            .lines()
            .nth(1)
            .and_then(|line| line.strip_prefix("parent "));
        if let Some(parent) = parent {
            if self
                .execute_command(
                    "GitVcsDriver.reviewCommit.parent",
                    &["cat-file", "-e", &format!("{parent}^{{commit}}")],
                    options,
                )
                .await?
                .code
                != Some(0)
            {
                return Err(self.command_error(
                    "GitVcsDriver.reviewCommit",
                    "Selected commit's parent is not available locally.",
                ));
            }
            Ok((sha.to_lowercase(), parent.into()))
        } else {
            Ok((sha.to_lowercase(), self.empty_tree_hash().await?))
        }
    }
    pub async fn review_branch_commits(
        &self,
        input: &ReviewBranchCommitsInput,
    ) -> Result<ReviewBranchCommitsResult, GitError> {
        let git = self.at(&input.cwd.0);
        let Some(paths) = git.repository_paths_uncached().await? else {
            return Ok(ReviewBranchCommitsResult {
                commits: vec![],
                truncated: false,
            });
        };
        let git = git.at(&paths.repository.root);
        let head = git
            .execute_command(
                "GitVcsDriver.getReviewBranchCommits.head",
                &["rev-parse", "--verify", "--quiet", "HEAD"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        if head.code != Some(0) {
            if head.code == Some(1) && head.stderr_text().trim().is_empty() {
                return Ok(ReviewBranchCommitsResult {
                    commits: vec![],
                    truncated: false,
                });
            }
            return Err(git.command_error(
                "GitVcsDriver.reviewCommit",
                "Could not resolve the current branch commit.",
            ));
        }
        let (_, merge) = git
            .review_merge_base(
                paths.current_branch.as_deref(),
                input
                    .base_ref
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|value| value.0.as_str()),
                None,
            )
            .await?;
        let result = git
            .execute_command(
                "GitVcsDriver.getReviewBranchCommits",
                &[
                    "log",
                    "-z",
                    "--no-notes",
                    "--no-show-signature",
                    "--format=%H%x00%s%x00%cI",
                    "--max-count=101",
                    &format!("{merge}..HEAD"),
                ],
                ExecuteOptions {
                    max_output_bytes: 128 * 1024,
                    append_truncation_marker: true,
                    ..Default::default()
                },
            )
            .await?;
        let text = result.stdout_text();
        let text = text.strip_suffix(crate::TRUNCATED_MARKER).unwrap_or(&text);
        if !result.stdout_truncated && !text.is_empty() && !text.ends_with('\0') {
            return Err(git.command_error(
                "GitVcsDriver.reviewCommit",
                "Could not read branch commit metadata.",
            ));
        }
        let fields: Vec<_> = text.split('\0').collect();
        let mut commits = Vec::new();
        for fields in fields[..fields.len().saturating_sub(1)]
            .as_chunks::<3>()
            .0
            .iter()
            .take(101)
        {
            if !valid_sha(fields[0]) || fields[2].is_empty() {
                return Err(git.command_error(
                    "GitVcsDriver.reviewCommit",
                    "Could not read branch commit metadata.",
                ));
            }
            commits.push(ReviewBranchCommitsResultCommits {
                sha: fields[0].into(),
                subject: fields[1].chars().take(300).collect(),
                committed_at: fields[2].into(),
            });
        }
        if result.stdout_truncated && commits.is_empty() {
            return Err(git.command_error(
                "GitVcsDriver.reviewCommit",
                "Branch commit metadata exceeds the response limit.",
            ));
        }
        let truncated = result.stdout_truncated || commits.len() > 100;
        commits.truncate(100);
        Ok(ReviewBranchCommitsResult { commits, truncated })
    }
    pub async fn review_diff_preview(
        &self,
        input: &ReviewDiffPreviewInput,
    ) -> Result<ReviewDiffPreviewResult, GitError> {
        let input = input.clone();
        self.owned_operation(move |git| async move { git.review_diff_preview_owned(&input).await })
            .await
    }
    async fn read_review_diff(
        &self,
        reference: Option<&str>,
        path_args: &[String],
        environment: &Environment,
        input: &ReviewDiffPreviewInput,
        metadata_only: bool,
    ) -> Result<ReadDiff, GitError> {
        let Some(reference) = reference else {
            return Ok(ReadDiff::empty());
        };
        let mut reference = reference.to_owned();
        let mut args = REVIEW_ARGS.to_vec();
        args.push("--minimal");
        if input.ignore_whitespace == Some(true) {
            args.push("--ignore-all-space");
        }
        let stat_args = |reference: &str| {
            let mut output: Vec<String> = args.iter().map(|value| (*value).into()).collect();
            output.extend([
                "--numstat".into(),
                "-z".into(),
                reference.into(),
                "--".into(),
            ]);
            output.extend(path_args.to_vec());
            output
        };
        let options = ExecuteOptions {
            allow_nonzero: true,
            max_output_bytes: METADATA_LIMIT,
            env: environment.clone(),
            ..Default::default()
        };
        let mut stat = self
            .execute_command(
                "GitVcsDriver.getReviewDiffPreview.stat",
                &stat_args(&reference),
                options.clone(),
            )
            .await?;
        if stat.code != Some(0) && reference == "HEAD" && unborn(&stat.stderr_text()) {
            reference = self.empty_tree_hash().await?;
            stat = self
                .execute_command(
                    "GitVcsDriver.getReviewDiffPreview.unbornStat",
                    &stat_args(&reference),
                    options,
                )
                .await?;
        }
        if stat.code != Some(0) {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffPreview.stat",
                "Could not read complete diff statistics.",
            ));
        }
        let files = numstat(&stat.stdout_text());
        if metadata_only || files.is_empty() {
            return Ok(ReadDiff {
                patch: String::new(),
                files: Some(files),
                truncated: false,
            });
        }
        let mut patch_args: Vec<String> = args.iter().map(|value| (*value).into()).collect();
        patch_args.extend(["--patch".into(), reference, "--".into()]);
        patch_args.extend(path_args.to_vec());
        let patch = self
            .execute_command(
                "GitVcsDriver.getReviewDiffPreview.patch",
                &patch_args,
                ExecuteOptions {
                    env: environment.clone(),
                    max_output_bytes: if input.file.is_some() {
                        FILE_LIMIT
                    } else {
                        120_000
                    },
                    append_truncation_marker: true,
                    ..Default::default()
                },
            )
            .await?;
        // GitVcsDriverCore reports truncation separately; it does not put the marker in review patches.
        let patch_text = patch.stdout_text();
        let patch_text = if patch.stdout_truncated {
            patch_text
                .strip_suffix(crate::TRUNCATED_MARKER)
                .unwrap_or(&patch_text)
                .into()
        } else {
            patch_text
        };
        Ok(ReadDiff {
            patch: patch_text,
            files: Some(files),
            truncated: patch.stdout_truncated,
        })
    }
    async fn review_diff_preview_owned(
        &self,
        input: &ReviewDiffPreviewInput,
    ) -> Result<ReviewDiffPreviewResult, GitError> {
        let git = self.at(&input.cwd.0);
        let generated_at =
            WireDateTime(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        let Some(paths) = git.repository_paths_uncached().await? else {
            return Ok(ReviewDiffPreviewResult {
                cwd: input.cwd.clone(),
                generated_at,
                sources: vec![],
                incomplete: None,
            });
        };
        let git = git.at(&paths.repository.root);
        let mut path_args = Vec::new();
        if let Some(file) = &input.file {
            path_args.push(format!(":(top,literal){}", file.path));
            if let Some(old) = &file.previous_path {
                path_args.push(format!(":(top,literal){old}"));
            }
        }
        let commit_sha = input
            .commit_sha
            .as_ref()
            .and_then(Option::as_ref)
            .map(|sha| sha.0.as_str());
        let file_kind = input.file.as_ref().map(|file| &file.source_kind);
        let metadata_only = input.metadata_only == Some(true) && input.file.is_none();
        if commit_sha.is_some() || file_kind == Some(&ReviewDiffPreviewInputFileSourceKind::Commit)
        {
            if commit_sha.is_none()
                || input.file.is_some()
                    && file_kind != Some(&ReviewDiffPreviewInputFileSourceKind::Commit)
            {
                return Err(git.command_error(
                    "GitVcsDriver.reviewCommit",
                    "Selected commit request is invalid.",
                ));
            }
            let (sha, parent) = git.review_commit(commit_sha.unwrap_or("")).await?;
            let subject = git
                .execute_command(
                    "GitVcsDriver.readReviewCommitMessage",
                    &[
                        "show",
                        "-s",
                        "--no-notes",
                        "--no-show-signature",
                        "--format=%B",
                        &sha,
                    ],
                    ExecuteOptions {
                        max_output_bytes: 8192,
                        append_truncation_marker: true,
                        ..Default::default()
                    },
                )
                .await?
                .stdout_text();
            let subject: String = subject
                .split('\n')
                .next()
                .unwrap_or("")
                .chars()
                .take(300)
                .collect();
            let mut args: Vec<String> = REVIEW_ARGS.iter().map(|value| (*value).into()).collect();
            if input.ignore_whitespace == Some(true) {
                args.push("--ignore-all-space".into());
            }
            let mut stat_args = args.clone();
            stat_args.extend([
                "--numstat".into(),
                "-z".into(),
                parent.clone(),
                sha.clone(),
                "--".into(),
            ]);
            stat_args.extend(path_args.clone());
            let stat = git
                .execute_command(
                    "GitVcsDriver.getReviewDiffPreview.commitStat",
                    &stat_args,
                    ExecuteOptions {
                        max_output_bytes: METADATA_LIMIT,
                        ..Default::default()
                    },
                )
                .await?;
            let files = numstat(&stat.stdout_text());
            let mut patch_args = args;
            patch_args.extend(["--patch".into(), parent.clone(), sha.clone(), "--".into()]);
            patch_args.extend(path_args);
            let patch = if metadata_only {
                t3_process::ProcessOutput::default()
            } else {
                git.execute_command(
                    "GitVcsDriver.getReviewDiffPreview.commitPatch",
                    &patch_args,
                    ExecuteOptions {
                        max_output_bytes: if input.file.is_some() {
                            FILE_LIMIT
                        } else {
                            120_000
                        },
                        append_truncation_marker: true,
                        ..Default::default()
                    },
                )
                .await?
            };
            let text = patch.stdout_text();
            let text = if patch.stdout_truncated {
                text.strip_suffix(crate::TRUNCATED_MARKER)
                    .unwrap_or(&text)
                    .into()
            } else {
                text
            };
            return Ok(ReviewDiffPreviewResult {
                cwd: input.cwd.clone(),
                generated_at,
                sources: vec![ReviewDiffPreviewSource {
                    id: Text(sha.clone()),
                    kind: ReviewDiffPreviewSourceKind::Commit,
                    title: Text(format!("Commit {}", &sha[..7])),
                    commit_subject: (!subject.trim().is_empty())
                        .then(|| Text(subject.trim().into())),
                    base_ref: Some(Text(parent)),
                    head_ref: Some(Text(sha)),
                    diff_hash: Text(hash_diff(&text, &files)?),
                    diff: text,
                    files: Some(files),
                    truncated: patch.stdout_truncated,
                    patches_deferred: metadata_only.then_some(true),
                }],
                incomplete: None,
            });
        }
        let working_only = file_kind == Some(&ReviewDiffPreviewInputFileSourceKind::WorkingTree)
            || file_kind.is_none()
                && input.source_kind == Some(ReviewDiffPreviewInputSourceKind::WorkingTree);
        let branch_only = file_kind == Some(&ReviewDiffPreviewInputFileSourceKind::BranchRange)
            || file_kind.is_none()
                && input.source_kind == Some(ReviewDiffPreviewInputSourceKind::BranchRange);
        let explicit = input
            .base_ref
            .as_ref()
            .and_then(Option::as_ref)
            .map(|value| value.0.as_str());
        let (base_ref, merge) = if working_only {
            (explicit.map(str::to_owned), None)
        } else {
            let (base, merge) = git
                .review_merge_base(paths.current_branch.as_deref(), explicit, None)
                .await?;
            (base, Some(merge))
        };
        let index = if input.tracked_only == Some(true) {
            ReviewIndex::empty()
        } else {
            git.prepare_review_index(
                &path_args,
                input.file.as_ref().map(|file| file.path.as_str()),
                None,
            )
            .await?
        };
        let metadata = metadata_only && index.unavailable.is_none();
        let mut dirty = git
            .read_review_diff(
                if branch_only { None } else { Some("HEAD") },
                &path_args,
                &index.env,
                input,
                metadata,
            )
            .await?;
        let mut base = git
            .read_review_diff(merge.as_deref(), &path_args, &index.env, input, metadata)
            .await?;
        if index.unavailable.is_some() {
            if !branch_only {
                dirty.files = None;
                dirty.truncated = true;
            }
            if merge.is_some() {
                base.files = None;
                base.truncated = true;
            }
        }
        let make = |id: &str,
                    kind,
                    title: String,
                    base_ref: Option<String>,
                    head_ref: Option<String>,
                    diff: ReadDiff|
         -> Result<ReviewDiffPreviewSource, GitError> {
            let hash = hash_diff(&diff.patch, diff.files.as_deref().unwrap_or_default())?;
            let patches_deferred = (metadata_only && diff.files.is_some()).then_some(true);
            Ok(ReviewDiffPreviewSource {
                id: Text(id.into()),
                kind,
                title: Text(title),
                commit_subject: None,
                base_ref: base_ref.map(Text),
                head_ref: head_ref.map(Text),
                diff: diff.patch,
                diff_hash: Text(hash),
                files: diff.files,
                truncated: diff.truncated,
                patches_deferred,
            })
        };
        let sources = vec![
            make(
                "working-tree",
                ReviewDiffPreviewSourceKind::WorkingTree,
                "Uncommitted".into(),
                Some("HEAD".into()),
                None,
                dirty,
            )?,
            make(
                "branch-range",
                ReviewDiffPreviewSourceKind::BranchRange,
                base_ref
                    .as_ref()
                    .map_or_else(|| "Changes".into(), |base| format!("Changes vs {base}")),
                base_ref,
                Some(paths.current_branch.unwrap_or_else(|| "HEAD".into())),
                base,
            )?,
        ];
        let sources = sources
            .into_iter()
            .filter(|source| match input.source_kind {
                Some(ReviewDiffPreviewInputSourceKind::WorkingTree) => {
                    source.kind == ReviewDiffPreviewSourceKind::WorkingTree
                }
                Some(ReviewDiffPreviewInputSourceKind::BranchRange) => {
                    source.kind == ReviewDiffPreviewSourceKind::BranchRange
                }
                None => true,
            })
            .collect();
        Ok(ReviewDiffPreviewResult {
            cwd: input.cwd.clone(),
            generated_at,
            sources,
            incomplete: (input.tracked_only == Some(true)).then_some(true),
        })
    }
    async fn read_review_revision(&self, revision: &str, path: &str) -> Result<String, GitError> {
        let output = self
            .execute_command(
                "GitVcsDriver.getReviewDiffFileContents.revision",
                &["show", "--end-of-options", &format!("{revision}:{path}")],
                ExecuteOptions {
                    max_output_bytes: FILE_LIMIT,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text();
        if output.contains('\0') {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffFileContents",
                format!("Cannot expand binary file '{path}'."),
            ));
        }
        Ok(output)
    }
    async fn read_review_working_file(&self, root: &Path, path: &str) -> Result<String, GitError> {
        let requested = normalize_path(&root.join(path));
        if !requested.starts_with(root) {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffFileContents.workingTree.path.resolve",
                format!("Diff file '{path}' resolves outside the review workspace."),
            ));
        }
        let target = tokio::fs::canonicalize(&requested).await.map_err(|_| {
            self.command_error(
                "GitVcsDriver.getReviewDiffFileContents.workingTree.fs.realPath",
                format!("Could not resolve diff file '{path}'."),
            )
        })?;
        let real_root = tokio::fs::canonicalize(root).await?;
        if !target.starts_with(&real_root) {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffFileContents.workingTree.fs.realPath",
                format!("Diff file '{path}' resolves outside the review workspace."),
            ));
        }
        if self.is_internal_path(&target) {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffFileContents.workingTree.fs.realPath",
                format!("Diff file '{path}' is not available."),
            ));
        }
        let info = tokio::fs::metadata(&target).await?;
        if !info.is_file() {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffFileContents.workingTree.fs.stat",
                format!("Diff path '{path}' is not a file."),
            ));
        }
        if info.len() > FILE_LIMIT as u64 {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffFileContents.workingTree.fs.stat",
                format!("Diff file '{path}' exceeds the 1 MB expansion limit."),
            ));
        }
        let bytes = tokio::fs::read(target).await?;
        if bytes.contains(&0) {
            return Err(self.command_error(
                "GitVcsDriver.getReviewDiffFileContents.workingTree.fs.readFile",
                format!("Cannot expand binary file '{path}'."),
            ));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    pub async fn review_diff_file_contents(
        &self,
        input: &ReviewDiffFileContentsInput,
    ) -> Result<ReviewDiffFileContentsResult, GitError> {
        let git = self.at(&input.cwd.0);
        let root = git
            .core_text(
                "GitVcsDriver.getReviewDiffFileContents.repositoryRoot",
                &["rev-parse", "--show-toplevel"],
                false,
            )
            .await?;
        let root = Path::new(root.trim());
        let (old_revision, new_revision) =
            if input.source_kind == ReviewDiffPreviewSourceKind::Commit {
                let sha = input.head_ref.as_ref().ok_or_else(|| {
                    git.command_error(
                        "GitVcsDriver.getReviewDiffFileContents",
                        "Selected commit is no longer available.",
                    )
                })?;
                let (sha, parent) = git.review_commit(&sha.0).await?;
                if input.base_ref.as_ref().map(|base| base.0.as_str()) != Some(parent.as_str()) {
                    return Err(git.command_error(
                        "GitVcsDriver.getReviewDiffFileContents",
                        "Selected commit changed. Refresh the comparison.",
                    ));
                }
                (parent, Some(sha))
            } else if input.source_kind == ReviewDiffPreviewSourceKind::WorkingTree {
                (
                    input
                        .base_ref
                        .as_ref()
                        .map_or("HEAD", |base| base.0.as_str())
                        .into(),
                    None,
                )
            } else {
                (
                    git.review_merge_base(
                        None,
                        input.base_ref.as_ref().map(|base| base.0.as_str()),
                        None,
                    )
                    .await?
                    .1,
                    None,
                )
            };
        let old_contents = if input.change_type == ReviewDiffFileContentsInputChangeType::New {
            String::new()
        } else {
            git.read_review_revision(&old_revision, &input.old_path.0)
                .await?
        };
        let new_contents = if input.change_type == ReviewDiffFileContentsInputChangeType::Deleted {
            String::new()
        } else if let Some(revision) = new_revision {
            git.read_review_revision(&revision, &input.new_path.0)
                .await?
        } else {
            git.read_review_working_file(root, &input.new_path.0)
                .await?
        };
        Ok(ReviewDiffFileContentsResult {
            old_contents,
            new_contents,
        })
    }
    pub async fn read_range_context(&self, base: &str) -> Result<RangeContext, GitError> {
        let read = |operation: &'static str, args: Vec<String>, limit| async move {
            Ok::<_, GitError>(
                self.execute_command(
                    operation,
                    &args,
                    ExecuteOptions {
                        max_output_bytes: limit,
                        append_truncation_marker: true,
                        ..Default::default()
                    },
                )
                .await?
                .stdout_text(),
            )
        };
        let (commit_summary, diff_summary, diff_patch) = tokio::try_join!(
            read(
                "GitVcsDriver.readRangeContext.log",
                [
                    "log",
                    "--oneline",
                    "--no-color",
                    "--no-decorate",
                    "--no-show-signature",
                    "--end-of-options",
                    &format!("{base}..HEAD")
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                19_000
            ),
            read(
                "GitVcsDriver.readRangeContext.diffStat",
                [
                    "-c",
                    "diff.relative=false",
                    "diff",
                    "--stat",
                    "--no-color",
                    "--end-of-options",
                    &format!("{base}...HEAD")
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                19_000
            ),
            read(
                "GitVcsDriver.readRangeContext.diffPatch",
                [
                    "-c",
                    "diff.relative=false",
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--patch",
                    "--minimal",
                    "--end-of-options",
                    &format!("{base}...HEAD")
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                59_000
            )
        )?;
        Ok(RangeContext {
            commit_summary,
            diff_summary,
            diff_patch,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeContext {
    pub commit_summary: String,
    pub diff_summary: String,
    pub diff_patch: String,
}
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            component => result.push(component.as_os_str()),
        }
    }
    result
}
