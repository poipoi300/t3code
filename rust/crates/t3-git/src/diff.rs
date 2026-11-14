use crate::{
    ExecuteOptions, Git, GitError, OutputMode,
    checkpoint::{checkpoint_ref, validate_revision},
};
use std::collections::HashSet;

pub const DIFF_MAX_BYTES: usize = 10_000_000;
pub const TRUNCATED_MARKER: &str = "\n\n[truncated]";
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffFormat {
    #[default]
    Patch,
    Numstat,
}
#[derive(Debug, Clone)]
pub struct DiffOptions<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub fallback_from_to_head: bool,
    pub ignore_whitespace: bool,
    pub format: DiffFormat,
    pub max_output_bytes: usize,
    pub file_paths: Option<&'a [String]>,
}
impl<'a> DiffOptions<'a> {
    pub fn new(from: &'a str, to: &'a str) -> Self {
        Self {
            from,
            to,
            fallback_from_to_head: false,
            ignore_whitespace: true,
            format: DiffFormat::Patch,
            max_output_bytes: DIFF_MAX_BYTES,
            file_paths: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub additions: u64,
    pub deletions: u64,
}

pub fn parse_numstat(output: &str) -> Vec<DiffFile> {
    let mut records = output.split('\0');
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, '\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let count = |value: &str| {
            if value == "-" {
                Some(0)
            } else if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                value.parse().ok()
            } else {
                None
            }
        };
        let (Some(additions), Some(deletions)) = (count(additions), count(deletions)) else {
            continue;
        };
        let (path, previous_path) = if path.is_empty() {
            let previous = records.next();
            (records.next().unwrap_or(""), previous.map(str::to_owned))
        } else {
            (path, None)
        };
        if path.is_empty() {
            continue;
        }
        files.push(DiffFile {
            path: path.into(),
            previous_path,
            additions,
            deletions,
        });
    }
    let collator = icu_collator::Collator::try_new(Default::default(), Default::default());
    files.sort_by(|left, right| match &collator {
        Ok(collator) => collator.compare(&left.path, &right.path),
        Err(_) => left.path.cmp(&right.path),
    });
    files
}

/// Aggregate query budget includes the marker and never splits a UTF-8 character.
pub fn truncate_patch(patch: &str, budget: usize) -> String {
    if patch.len() <= budget {
        return patch.into();
    }
    let marker = if budget < TRUNCATED_MARKER.len() {
        &TRUNCATED_MARKER[..budget]
    } else {
        TRUNCATED_MARKER
    };
    let mut end = budget.saturating_sub(marker.len());
    while !patch.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{marker}", &patch[..end])
}

#[derive(Debug, Clone)]
pub struct CheckpointRun {
    pub id: String,
    pub completed: bool,
}
#[derive(Debug, Clone)]
pub struct CheckpointScope {
    pub id: String,
    pub cwd: std::path::PathBuf,
    pub root_run: bool,
}
#[derive(Debug, Clone)]
pub struct CheckpointRecord {
    pub reference: String,
    pub scope_id: String,
    pub run_id: Option<String>,
    pub ready: bool,
    pub ordinal_within_scope: u64,
    pub app_run_ordinal: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct CheckpointContext {
    pub thread_id: String,
    pub runs: Vec<CheckpointRun>,
    pub scopes: Vec<CheckpointScope>,
    pub checkpoints: Vec<CheckpointRecord>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDiff {
    pub thread_id: String,
    pub from_turn_count: u64,
    pub to_turn_count: u64,
    pub diff: String,
}

impl Git {
    pub async fn diff_checkpoints(&self, options: DiffOptions<'_>) -> Result<String, GitError> {
        self.checkpoint_reader()
            .await?
            .diff_checkpoints_bound(options)
            .await
    }

    async fn diff_checkpoints_bound(&self, options: DiffOptions<'_>) -> Result<String, GitError> {
        if options.file_paths.is_some_and(<[String]>::is_empty) {
            return Ok(String::new());
        }
        validate_revision(options.from)?;
        validate_revision(options.to)?;
        let turn_start = self.resolve_turn_start(options.from, options.to).await;
        let from = if options.fallback_from_to_head {
            match self.resolve_checkpoint(options.from).await? {
                Some(_) => turn_start,
                None => self.head_commit().await?.ok_or_else(|| {
                    GitError::Unavailable(
                        "Checkpoint ref is unavailable for diff operation.".into(),
                    )
                })?,
            }
        } else {
            turn_start
        };
        let from = format!("{from}^{{commit}}");
        let to = format!("{}^{{commit}}", options.to);
        let mut args = vec![
            "-c",
            "diff.relative=false",
            "-c",
            "diff.suppressBlankEmpty=false",
            "diff",
        ];
        if options.format == DiffFormat::Numstat {
            args.extend(["--numstat", "-z"]);
        } else {
            args.push("--patch");
        }
        args.extend([
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--submodule=short",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ]);
        if options.ignore_whitespace {
            args.push("--ignore-all-space");
        }
        args.extend([from.as_str(), to.as_str()]);
        let paths: Vec<_> = options
            .file_paths
            .unwrap_or_default()
            .iter()
            .map(|path| format!(":(top,literal){path}"))
            .collect();
        if options.file_paths.is_some() {
            args.push("--");
            args.extend(paths.iter().map(String::as_str));
        }
        Ok(self
            .execute(
                &args,
                ExecuteOptions {
                    max_output_bytes: options.max_output_bytes,
                    output_mode: if options.format == DiffFormat::Numstat {
                        OutputMode::Error
                    } else {
                        OutputMode::Truncate
                    },
                    ..Default::default()
                },
            )
            .await?
            .stdout_text())
    }

    async fn resolve_turn_start(&self, from: &str, to: &str) -> String {
        let result = self
            .execute(
                &[
                    "-c",
                    "log.showSignature=false",
                    "log",
                    "--no-walk=unsorted",
                    "--format=%H %P",
                    &format!("{from}^{{commit}}"),
                    &format!("{to}^2"),
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await;
        if let Ok(result) = result
            && result.code == Some(0)
        {
            let text = result.stdout_text();
            let mut lines = text.lines();
            let from_commit = lines.next().unwrap_or("").split_whitespace().next();
            let start: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
            if let [commit, _, started_from, ..] = start.as_slice()
                && Some(*started_from) == from_commit
            {
                return (*commit).into();
            }
        }
        from.into()
    }

    async fn checkpoint_lines(
        &self,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<Vec<String>, GitError> {
        let mut command = vec!["-c", "log.showSignature=false"];
        command.extend_from_slice(args);
        let output = self
            .execute(
                &command,
                ExecuteOptions {
                    stdin,
                    max_output_bytes: DIFF_MAX_BYTES,
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?;
        Ok(output
            .stdout_text()
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }
    async fn checkpoint_paths(
        &self,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<Vec<String>, GitError> {
        let mut command = vec!["-c", "diff.relative=false", "-c", "log.showSignature=false"];
        command.extend_from_slice(args);
        command.extend([
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
        ]);
        let output = self
            .checkpoint_reader()
            .await?
            .execute(
                &command,
                ExecuteOptions {
                    stdin,
                    max_output_bytes: DIFF_MAX_BYTES,
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?;
        Ok(output
            .stdout_text()
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect())
    }
    async fn imported_checkpoint_commits(
        &self,
        captured_at: &str,
        start_head: &str,
    ) -> Result<HashSet<String>, GitError> {
        let since = format!("--since=@{captured_at}");
        let entries = self
            .checkpoint_lines(
                &[
                    "log",
                    "--walk-reflogs",
                    "--all",
                    &since,
                    "--format=%H %gD %gs",
                ],
                None,
            )
            .await?;
        let ref_move=regex::Regex::new(r"(?i)^(?:$|(?:checkout|reset|branch|clone|fetch)\b|[^:]*\((?:start|finish|abort|reset)\): |(?:merge|pull|rebase|cherry-pick)\b[^()]*: fast-forward$)").expect("ref move reflog pattern");
        let mut fetched_tips = HashSet::new();
        let mut created_here = HashSet::new();
        for entry in &entries {
            let mut fields = entry.splitn(3, ' ');
            let commit = fields.next().unwrap_or("");
            let selector = fields.next().unwrap_or("");
            let subject = fields.next().unwrap_or("");
            if selector.starts_with("refs/remotes/") {
                if subject != "update by push" {
                    fetched_tips.insert(commit);
                }
            } else if !ref_move.is_match(subject) {
                created_here.insert(commit);
            }
        }
        if fetched_tips.is_empty() {
            return Ok(HashSet::new());
        }
        let mut input = fetched_tips.into_iter().collect::<Vec<_>>().join("\n");
        input.push_str(&format!("\n^{start_head}\n"));
        Ok(self
            .checkpoint_lines(&["rev-list", &since, "--stdin"], Some(input.into_bytes()))
            .await?
            .into_iter()
            .filter(|commit| !created_here.contains(commit.as_str()))
            .collect())
    }

    pub async fn list_authored_paths(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Option<HashSet<String>>, GitError> {
        validate_revision(from)?;
        validate_revision(to)?;
        let turn_start = self.resolve_turn_start(from, to).await;
        let from = format!("{turn_start}^{{commit}}");
        let to = format!("{to}^{{commit}}");
        let heads = self
            .text(&[
                "-c",
                "log.showSignature=false",
                "log",
                "--no-walk=unsorted",
                "--format=%ct %P",
                &from,
                &to,
            ])
            .await?;
        let mut lines = heads.trim_end_matches('\n').split('\n');
        let mut first = lines.next().unwrap_or("").split(' ');
        let captured_at = first.next().unwrap_or("");
        let start = first.next().unwrap_or("");
        let end = lines.next().unwrap_or("").split(' ').nth(1).unwrap_or("");
        if start.is_empty() || end.is_empty() || start == end {
            return Ok(None);
        }
        let before = format!("{end}..{start}");
        let after = format!("{start}..{end}");
        let since = format!("--since=@{captured_at}");
        let (recent, a, b, c) = tokio::try_join!(
            async {
                self.checkpoint_lines(&["rev-list", &since, &after], None)
                    .await
            },
            async { self.checkpoint_paths(&["diff", start, &from], None).await },
            async { self.checkpoint_paths(&["diff", end, &to], None).await },
            async {
                self.checkpoint_paths(
                    &["log", "--format=", "--diff-merges=remerge", &before],
                    None,
                )
                .await
            }
        )?;
        let imported = if recent.is_empty() {
            HashSet::new()
        } else {
            self.imported_checkpoint_commits(captured_at, start)
                .await
                .unwrap_or_default()
        };
        let own: Vec<_> = recent
            .iter()
            .filter(|commit| !imported.contains(*commit))
            .map(String::as_str)
            .collect();
        let own_paths = if own.is_empty() {
            Vec::new()
        } else {
            self.checkpoint_paths(
                &[
                    "log",
                    "--no-walk=unsorted",
                    "--stdin",
                    "--format=",
                    "--diff-merges=remerge",
                ],
                Some(format!("{}\n", own.join("\n")).into_bytes()),
            )
            .await?
        };
        Ok(Some(
            a.into_iter().chain(b).chain(c).chain(own_paths).collect(),
        ))
    }

    /// Query-level authored-path filtering and a single byte budget across path batches.
    pub async fn filtered_checkpoint_diff(
        &self,
        from: &str,
        to: &str,
        ignore_whitespace: bool,
        budget: usize,
    ) -> Result<String, GitError> {
        let authored = self.list_authored_paths(from, to).await.ok().flatten();
        let mut comparison = DiffOptions::new(from, to);
        comparison.ignore_whitespace = ignore_whitespace;
        let files = if authored.is_some() {
            let mut numstat = comparison.clone();
            numstat.format = DiffFormat::Numstat;
            self.diff_checkpoints(numstat)
                .await
                .map(|output| parse_numstat(&output))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let retained: Vec<_> = files
            .iter()
            .filter(|file| {
                authored.as_ref().is_none_or(|paths| {
                    paths.contains(&file.path)
                        || file
                            .previous_path
                            .as_ref()
                            .is_some_and(|path| paths.contains(path))
                })
            })
            .collect();
        let batches = if retained.len() == files.len() {
            vec![None]
        } else {
            let mut batches = Vec::new();
            let mut batch = Vec::new();
            let mut length = 0;
            for file in retained {
                let mut paths = Vec::new();
                if let Some(path) = &file.previous_path {
                    paths.push(path.clone());
                }
                paths.push(file.path.clone());
                let size: usize = paths
                    .iter()
                    .map(|path| path.encode_utf16().count() + 16)
                    .sum();
                if !batch.is_empty() && length + size > 24_000 {
                    batches.push(Some(batch));
                    batch = Vec::new();
                    length = 0;
                }
                batch.extend(paths);
                length += size;
            }
            if !batch.is_empty() {
                batches.push(Some(batch));
            }
            batches
        };
        let mut diff = String::new();
        for batch in &batches {
            let remaining = budget.saturating_sub(diff.len());
            let mut options = comparison.clone();
            options.file_paths = batch.as_deref();
            options.max_output_bytes = remaining.saturating_add(1);
            let patch = self.diff_checkpoints(options).await?;
            let overflow = patch.len() > remaining;
            diff.push_str(&patch);
            if overflow {
                return Ok(truncate_patch(&diff, budget));
            }
        }
        Ok(diff)
    }

    pub async fn get_turn_diff(
        &self,
        context: &CheckpointContext,
        from: u64,
        to: u64,
        ignore_whitespace: Option<bool>,
    ) -> Result<TurnDiff, GitError> {
        let mut result = TurnDiff {
            thread_id: context.thread_id.clone(),
            from_turn_count: from,
            to_turn_count: to,
            diff: String::new(),
        };
        if from == to {
            return Ok(result);
        }
        let ready: Vec<_> = context
            .checkpoints
            .iter()
            .filter(|checkpoint| {
                checkpoint.ready
                    && checkpoint.app_run_ordinal.is_some()
                    && checkpoint.run_id.as_ref().is_some_and(|id| {
                        context
                            .runs
                            .iter()
                            .any(|run| &run.id == id && run.completed)
                    })
            })
            .collect();
        let max = ready
            .iter()
            .filter_map(|checkpoint| checkpoint.app_run_ordinal)
            .max()
            .unwrap_or(0);
        if to > max {
            return Err(GitError::Unavailable(format!(
                "requested turn {to}, available {max}"
            )));
        }
        let destination = ready
            .iter()
            .find(|checkpoint| checkpoint.app_run_ordinal == Some(to))
            .ok_or_else(|| GitError::Unavailable("destination checkpoint".into()))?;
        let scope = context
            .scopes
            .iter()
            .find(|scope| scope.id == destination.scope_id)
            .ok_or_else(|| GitError::Unavailable("checkpoint workspace path".into()))?;
        let previous = if from == to.saturating_sub(1) {
            context.checkpoints.iter().find(|checkpoint| {
                checkpoint.ready
                    && checkpoint.scope_id == destination.scope_id
                    && checkpoint.ordinal_within_scope.checked_add(1)
                        == Some(destination.ordinal_within_scope)
            })
        } else {
            None
        };
        let source = if from == 0 {
            checkpoint_ref(
                &context
                    .scopes
                    .iter()
                    .find(|scope| scope.root_run)
                    .ok_or_else(|| GitError::Unavailable("baseline scope".into()))?
                    .id,
                0,
            )
        } else {
            previous
                .or_else(|| {
                    ready
                        .iter()
                        .copied()
                        .find(|checkpoint| checkpoint.app_run_ordinal == Some(from))
                })
                .ok_or_else(|| GitError::Unavailable("source checkpoint".into()))?
                .reference
                .clone()
        };
        result.diff = self
            .at(&scope.cwd)
            .filtered_checkpoint_diff(
                &source,
                &destination.reference,
                ignore_whitespace.unwrap_or(true),
                DIFF_MAX_BYTES,
            )
            .await?;
        Ok(result)
    }
    pub async fn get_full_thread_diff(
        &self,
        context: &CheckpointContext,
        to: u64,
        ignore_whitespace: Option<bool>,
    ) -> Result<TurnDiff, GitError> {
        self.get_turn_diff(context, 0, to, ignore_whitespace).await
    }
}
