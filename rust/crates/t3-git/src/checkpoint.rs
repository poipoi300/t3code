use crate::{ExecuteOptions, Git, GitError, OutputMode, env};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_process::{Environment, ProcessOutput};

pub const CHECKPOINT_REF_PREFIX: &str = "refs/t3/orchestration-v2/checkpoints/";

/// Base64url encodes the first 32 ASCII hex digits, not the digest bytes.
pub fn checkpoint_ref(scope_id: &str, ordinal: u64) -> String {
    let digest = Sha256::digest(scope_id.as_bytes());
    let hex: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{CHECKPOINT_REF_PREFIX}{}/ordinal/{ordinal}",
        URL_SAFE_NO_PAD.encode(hex)
    )
}

const INDEX_CONFIG: &[&str] = &[
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "sparse.expectFilesOutsideOfPatterns=false",
    "-c",
    "core.splitIndex=false",
    "-c",
    "splitIndex.sharedIndexExpire=never",
];
fn durable_write() -> Vec<&'static str> {
    let mut args = vec![
        "-c",
        "core.fsync=objects,reference",
        "-c",
        "core.fsyncMethod=fsync",
    ];
    if cfg!(any(windows, target_os = "macos")) {
        args.extend(["-c", "core.fsyncMethod=batch"]);
    }
    args
}
fn long_paths() -> &'static [&'static str] {
    if cfg!(windows) {
        &["-c", "core.longpaths=true"]
    } else {
        &[]
    }
}

struct ScratchIndex(PathBuf);
impl ScratchIndex {
    fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.as_os_str().to_owned();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}
impl Drop for ScratchIndex {
    fn drop(&mut self) {
        self.cleanup();
    }
}
struct HeadMove {
    to: String,
    at: String,
    reason: String,
}

struct CaptureRepository {
    common: PathBuf,
    index: Option<PathBuf>,
    prefix: String,
    head: bool,
    head_oid: Option<String>,
}

#[derive(Default)]
struct IndexScan {
    pending: Vec<u8>,
    flagged: Vec<String>,
    skipped: Vec<String>,
    flagged_bytes: usize,
    fallback: bool,
    sparse: bool,
    restore: bool,
    keep: bool,
    start: bool,
}
impl IndexScan {
    fn feed(&mut self, mut chunk: &[u8]) {
        while !chunk.is_empty() && !self.fallback {
            if self.start {
                let tag = chunk[0];
                self.keep = if self.restore {
                    matches!(tag, b'S' | b's')
                } else {
                    tag.is_ascii_lowercase() || tag == b'S'
                };
                if self.sparse && tag == b's' && !self.restore {
                    self.fallback = true;
                    return;
                }
                self.start = false;
            }
            let end = chunk.iter().position(|byte| *byte == 0);
            let count = end.unwrap_or(chunk.len());
            if self.keep {
                self.pending.extend_from_slice(&chunk[..count]);
                if self.pending.len() > 1024 * 1024 {
                    self.fallback = true;
                    return;
                }
            }
            if let Some(end) = end {
                if self.keep {
                    if self.pending.len() < 3 {
                        self.fallback = true;
                        return;
                    }
                    let skipped = self.sparse
                        && (self.pending[0] == b'S' || self.restore && self.pending[0] == b's');
                    if !skipped || self.pending.last() != Some(&b'/') {
                        match std::str::from_utf8(&self.pending[2..]) {
                            Ok(entry) => {
                                if skipped {
                                    self.skipped.push(entry.into());
                                } else {
                                    self.flagged_bytes += self.pending.len();
                                    self.flagged.push(entry.into());
                                }
                            }
                            Err(_) => {
                                self.fallback = true;
                                return;
                            }
                        }
                    }
                    if self.flagged_bytes > 1024 * 1024 {
                        self.fallback = true;
                        return;
                    }
                }
                self.pending.clear();
                self.start = true;
                chunk = &chunk[end + 1..];
            } else {
                break;
            }
        }
    }
}

fn checkpoint_path_contains(parent: &str, path: &str) -> bool {
    let (parent, path) = if cfg!(windows) {
        (parent.to_lowercase(), path.to_lowercase())
    } else {
        (parent.to_owned(), path.to_owned())
    };
    path == parent || path.starts_with(&format!("{parent}/"))
}

fn stage_pathspecs(paths: &[String]) -> Vec<u8> {
    paths
        .iter()
        .flat_map(|path| format!(":(literal){path}\0").into_bytes())
        .collect()
}

impl Git {
    async fn capture_execute(
        &self,
        args: &[&str],
        commit_env: &Environment,
        stdin: Option<Vec<u8>>,
    ) -> Result<ProcessOutput, GitError> {
        self.execute(
            args,
            ExecuteOptions {
                capture: true,
                env: commit_env.clone(),
                stdin,
                ..Default::default()
            },
        )
        .await
    }
    async fn capture_repository(&self) -> Result<CaptureRepository, GitError> {
        let output = self
            .execute(
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-common-dir",
                    "--git-path",
                    "index",
                    "--show-prefix",
                    "--verify",
                    "--quiet",
                    "HEAD",
                ],
                ExecuteOptions {
                    capture: true,
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        let stdout = output.stdout_text();
        let lines: Vec<_> = stdout.split('\n').collect();
        let head = output.code == Some(0);
        let checked = head || output.code == Some(1);
        if checked && lines.len() == if head { 5 } else { 4 } && lines.last() == Some(&"") {
            return Ok(CaptureRepository {
                common: PathBuf::from(lines[0].trim()),
                index: Some(lines[1].trim().into()),
                prefix: lines[2].into(),
                head,
                head_oid: head.then(|| lines[3].to_owned()),
            });
        }
        let common = self.text(&["rev-parse", "--git-common-dir"]).await?;
        let prefix = self.text(&["rev-parse", "--show-prefix"]).await?;
        let head_exists = if checked {
            head
        } else {
            self.head_commit().await?.is_some()
        };
        Ok(CaptureRepository {
            common: self.absolute_git_path(common.trim_end_matches('\n'))?,
            index: None,
            prefix: prefix.trim_end_matches('\n').into(),
            head: head_exists,
            head_oid: None,
        })
    }

    async fn reuse_index(
        &self,
        repository: &CaptureRepository,
        scratch: &ScratchIndex,
        commit_env: &Environment,
        sparse: bool,
    ) -> Result<bool, GitError> {
        let index = match &repository.index {
            Some(index) => index.clone(),
            None => PathBuf::from(
                self.text(&["rev-parse", "--path-format=absolute", "--git-path", "index"])
                    .await?
                    .trim(),
            ),
        };
        let modified = std::fs::metadata(&index)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| GitError::InvalidOutput("index timestamp before epoch".into()))?;
        let seconds = modified.as_millis().saturating_sub(1) / 1000;
        let seconds = i64::try_from(seconds)
            .map_err(|_| GitError::InvalidOutput("index timestamp".into()))?;
        if seconds <= 0 {
            return Ok(false);
        }
        let timestamp = filetime::FileTime::from_unix_time(seconds, 0);
        tokio::fs::copy(&index, &scratch.0).await?;
        let mut args = INDEX_CONFIG.to_vec();
        args.extend(["read-tree", "--reset", "HEAD"]);
        self.capture_execute(&args, commit_env, None).await?;
        filetime::set_file_times(&scratch.0, timestamp, timestamp)?;
        let scan = Arc::new(Mutex::new(IndexScan {
            sparse,
            start: true,
            ..Default::default()
        }));
        let callback_scan = scan.clone();
        let mut args = INDEX_CONFIG.to_vec();
        args.extend(["ls-files", "--full-name", "--sparse", "--stage", "-v", "-z"]);
        self.execute(
            &args,
            ExecuteOptions {
                capture: true,
                env: commit_env.clone(),
                max_output_bytes: 4096,
                on_stdout_chunk: Some(Arc::new(move |chunk| {
                    if let Ok(mut scan) = callback_scan.lock() {
                        scan.feed(chunk);
                    }
                })),
                ..Default::default()
            },
        )
        .await?;
        let (mut flagged, skipped, fallback) = {
            let mut scan = scan
                .lock()
                .map_err(|_| GitError::InvalidOutput("index scan poisoned".into()))?;
            (
                std::mem::take(&mut scan.flagged),
                std::mem::take(&mut scan.skipped),
                scan.fallback || !scan.start,
            )
        };
        if fallback {
            return Ok(false);
        }
        if !skipped.is_empty() {
            let paths: Vec<_> = skipped
                .iter()
                .filter_map(|entry| entry.split_once('\t').map(|(_, path)| path))
                .collect();
            let stdin = format!("{}\0", paths.join("\0")).into_bytes();
            let mut args = INDEX_CONFIG.to_vec();
            args.extend(["sparse-checkout", "check-rules", "-z"]);
            let output = self
                .execute(
                    &args,
                    ExecuteOptions {
                        capture: true,
                        env: commit_env.clone(),
                        max_output_bytes: stdin.len(),
                        output_mode: OutputMode::Error,
                        stdin: Some(stdin),
                        ..Default::default()
                    },
                )
                .await?;
            let stdout = output.stdout_text();
            let selected: std::collections::HashSet<_> = stdout.split('\0').collect();
            for entry in &skipped {
                if entry
                    .split_once('\t')
                    .is_some_and(|(_, path)| selected.contains(path))
                {
                    flagged.push(entry.clone());
                }
            }
        }
        if !flagged.is_empty() {
            if flagged.iter().any(|entry| entry.starts_with("160000 ")) {
                return Ok(false);
            }
            let mut args = INDEX_CONFIG.to_vec();
            args.extend([
                "-c",
                "core.ignorestat=false",
                "update-index",
                "-z",
                "--index-info",
            ]);
            self.capture_execute(
                &args,
                commit_env,
                Some(format!("{}\0", flagged.join("\0")).into_bytes()),
            )
            .await?;
            filetime::set_file_times(&scratch.0, timestamp, timestamp)?;
        }
        Ok(true)
    }

    async fn capture_stage_paths(
        &self,
        prefix: &str,
        exclusions: &[String],
        commit_env: &Environment,
        tracked_only: bool,
        files: &[String],
    ) -> Result<Vec<String>, GitError> {
        let mut paths = std::collections::BTreeSet::new();
        if !tracked_only {
            paths.extend(files.iter().cloned());
        }
        let mut args = INDEX_CONFIG.to_vec();
        args.extend(["ls-files", "--cached", "--full-name", "-v", "-z", "--", "."]);
        let cached = self
            .execute(
                &args,
                ExecuteOptions {
                    env: commit_env.clone(),
                    capture: true,
                    max_output_bytes: 16 * 1024 * 1024,
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?;
        let cached = cached.stdout_text();
        for entry in cached.split('\0').filter(|entry| !entry.is_empty()) {
            let path = entry
                .get(2..)
                .ok_or_else(|| GitError::InvalidOutput("checkpoint cached path".into()))?;
            if !entry.starts_with(['S', 's']) {
                paths.insert(path.to_owned());
            }
        }
        paths.retain(|path| {
            !exclusions
                .iter()
                .any(|excluded| checkpoint_path_contains(excluded, path))
        });
        let mut paths: Vec<_> = paths
            .into_iter()
            .map(|path| {
                path.strip_prefix(prefix)
                    .map(str::to_owned)
                    .ok_or_else(|| GitError::InvalidOutput("checkpoint path outside scope".into()))
            })
            .collect::<Result<_, _>>()?;
        if !paths.is_empty() {
            let ignored = self
                .execute(
                    &["check-ignore", "-z", "--stdin"],
                    ExecuteOptions {
                        env: commit_env.clone(),
                        capture: true,
                        stdin: Some(format!("{}\0", paths.join("\0")).into_bytes()),
                        allow_nonzero: true,
                        max_output_bytes: 16 * 1024 * 1024,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?;
            if !matches!(ignored.code, Some(0 | 1)) {
                return Err(self.vcs_exit("GitVcsDriver.filterIgnoredPaths", 3, &ignored));
            }
            let ignored = ignored.stdout_text();
            let ignored: std::collections::HashSet<_> = ignored.split('\0').collect();
            paths.retain(|path| !ignored.contains(path.as_str()));
        }
        Ok(paths)
    }

    pub async fn capture_checkpoint(&self, checkpoint_ref: &str) -> Result<(), GitError> {
        self.capture_checkpoint_with_previous(checkpoint_ref, None)
            .await
    }

    pub async fn capture_checkpoint_with_previous(
        &self,
        checkpoint_ref: &str,
        previous: Option<&str>,
    ) -> Result<(), GitError> {
        let checkpoint_ref = checkpoint_ref.to_owned();
        let previous = previous.map(str::to_owned);
        self.owned_operation(move |git| async move {
            git.capture_checkpoint_owned(&checkpoint_ref, previous.as_deref())
                .await
        })
        .await
    }

    async fn checkpoint_sparse_enabled(&self) -> Result<bool, GitError> {
        Ok(self
            .execute(
                &["config", "--bool", "core.sparseCheckout"],
                ExecuteOptions {
                    capture: true,
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text()
            .trim()
            == "true")
    }

    async fn checkpoint_add_supports_sparse(&self) -> Result<bool, GitError> {
        type Probe = Arc<tokio::sync::OnceCell<bool>>;
        static PROBES: std::sync::OnceLock<
            Mutex<std::collections::BTreeMap<std::ffi::OsString, Probe>>,
        > = std::sync::OnceLock::new();
        let probe = PROBES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| GitError::Unavailable("Git capability cache is unavailable.".into()))?
            .entry(self.program.clone())
            .or_default()
            .clone();
        let supported = probe
            .get_or_try_init(|| async {
                let help = self
                    .execute(
                        &["add", "-h"],
                        ExecuteOptions {
                            capture: true,
                            allow_nonzero: true,
                            ..Default::default()
                        },
                    )
                    .await?;
                let text = format!("{}{}", help.stdout_text(), help.stderr_text());
                Ok::<_, GitError>(text.contains("--sparse") || text.contains("--[no-]sparse"))
            })
            .await?;
        Ok(*supported)
    }

    async fn capture_checkpoint_owned(
        &self,
        checkpoint_ref: &str,
        previous: Option<&str>,
    ) -> Result<(), GitError> {
        validate_checkpoint_ref(checkpoint_ref)?;
        let (repository, head_moves, sparse) = tokio::join!(
            self.capture_repository(),
            self.read_head_moves(),
            self.checkpoint_sparse_enabled()
        );
        let repository = repository?;
        let sparse = sparse?;
        let head_moves = if repository.head_oid.is_some() {
            head_moves
        } else {
            Vec::new()
        };
        let head_mark = head_moves
            .first()
            .filter(|last| Some(&last.to) == repository.head_oid.as_ref())
            .map(|last| {
                format!(
                    "{} {}",
                    last.at,
                    head_moves
                        .iter()
                        .filter(|entry| entry.at == last.at)
                        .count()
                )
            });
        let scratch = ScratchIndex(
            repository
                .common
                .join(format!("t3-checkpoint-index-{}", uuid::Uuid::new_v4())),
        );
        let mut commit_env = env(&[
            ("GIT_AUTHOR_NAME", "T3 Code"),
            ("GIT_AUTHOR_EMAIL", "t3code@users.noreply.github.com"),
            ("GIT_COMMITTER_NAME", "T3 Code"),
            ("GIT_COMMITTER_EMAIL", "t3code@users.noreply.github.com"),
        ]);
        commit_env.insert(
            "GIT_INDEX_FILE".into(),
            Some(scratch.0.as_os_str().to_owned()),
        );
        let sparse = sparse && self.checkpoint_add_supports_sparse().await?;
        if repository.head {
            let reused = match self
                .reuse_index(&repository, &scratch, &commit_env, sparse)
                .await
            {
                Err(GitError::Process(t3_process::ProcessError::Cancelled)) => {
                    return Err(t3_process::ProcessError::Cancelled.into());
                }
                result => result.unwrap_or(false),
            };
            if !reused {
                if sparse
                    && self
                        .execute(
                            &["config", "--bool", "core.sparseCheckoutCone"],
                            ExecuteOptions {
                                allow_nonzero: true,
                                capture: true,
                                ..Default::default()
                            },
                        )
                        .await?
                        .stdout_text()
                        .trim()
                        != "true"
                {
                    return Err(GitError::Unsupported(
                        "Cannot rebuild a checkpoint index for non-cone sparse checkout.".into(),
                    ));
                }
                scratch.cleanup();
                let mut args = INDEX_CONFIG.to_vec();
                if sparse {
                    args.extend(["-c", "index.sparse=true", "read-tree", "--reset", "HEAD"]);
                } else {
                    args.extend(["read-tree", "HEAD"]);
                }
                self.capture_execute(&args, &commit_env, None).await?;
            }
        }
        let scan = self
            .checkpoint_scan(repository.head.then_some("HEAD"), &[], &commit_env)
            .await?;
        let exclusions = &scan.exclusions;
        let ignored = if repository.prefix.is_empty() {
            false
        } else {
            let basename = repository
                .prefix
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("");
            let probe = self
                .execute(
                    &[
                        "-c",
                        "core.fsmonitor=false",
                        "-c",
                        "core.untrackedCache=false",
                        "check-ignore",
                        "--no-index",
                        "-z",
                        "--stdin",
                    ],
                    ExecuteOptions {
                        stdin: Some(format!("../{basename}\0").into_bytes()),
                        allow_nonzero: true,
                        max_output_bytes: 16 * 1024 * 1024,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?;
            if !matches!(probe.code, Some(0 | 1)) {
                return Err(self.vcs_exit("GitVcsDriver.filterIgnoredPaths", 6, &probe));
            }
            !probe.stdout.is_empty()
        };
        if ignored {
            let additions = self
                .execute(
                    &[
                        "-c",
                        "core.quotePath=true",
                        "diff",
                        "--cached",
                        "--raw",
                        "--no-abbrev",
                        "--no-renames",
                        "--no-relative",
                        "--ignore-submodules=none",
                        "--no-color",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ita-visible-in-index",
                        "--diff-filter=A",
                        "--",
                        ".",
                    ],
                    ExecuteOptions {
                        capture: true,
                        max_output_bytes: 16 * 1024 * 1024,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?;
            if !additions.stdout.is_empty() {
                let mut stdin = String::new();
                for line in additions.stdout_text().lines() {
                    let (metadata, path) = line
                        .split_once('\t')
                        .ok_or_else(|| GitError::InvalidOutput("staged addition".into()))?;
                    let fields: Vec<_> = metadata.split(' ').collect();
                    if fields.len() != 5 || fields[0] != ":000000" || fields[4] != "A" {
                        return Err(GitError::InvalidOutput("staged addition metadata".into()));
                    }
                    stdin.push_str(&format!("{} {}\t{}\n", fields[1], fields[3], path));
                }
                let mut args = INDEX_CONFIG.to_vec();
                args.extend(["-c", "index.sparse=false", "update-index", "--index-info"]);
                self.capture_execute(&args, &commit_env, Some(stdin.into_bytes()))
                    .await?;
            }
            let mut args = INDEX_CONFIG.to_vec();
            args.extend(["ls-files", "--cached", "-z", "--", "."]);
            if self
                .execute(
                    &args,
                    ExecuteOptions {
                        env: commit_env.clone(),
                        max_output_bytes: 1,
                        capture: true,
                        ..Default::default()
                    },
                )
                .await?
                .stdout
                .is_empty()
            {
                return Err(GitError::Unsupported(
                    "The workspace directory is ignored by its Git repository.".into(),
                ));
            }
        }
        if !exclusions.is_empty() {
            let specs: Vec<_> = exclusions
                .iter()
                .map(|path| {
                    format!(
                        ":(top,literal{}){path}",
                        if cfg!(windows) { ",icase" } else { "" }
                    )
                })
                .collect();
            let mut args = INDEX_CONFIG.to_vec();
            args.extend(["ls-files", "--cached", "-z", "--"]);
            args.extend(specs.iter().map(String::as_str));
            let paths = self
                .execute(
                    &args,
                    ExecuteOptions {
                        env: commit_env.clone(),
                        max_output_bytes: 16 * 1024 * 1024,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?
                .stdout;
            if !paths.is_empty() {
                let mut args = INDEX_CONFIG.to_vec();
                args.extend(["update-index", "--force-remove", "-z", "--stdin"]);
                self.capture_execute(&args, &commit_env, Some(paths))
                    .await?;
            }
        }
        // A fixed file list cannot expose a scanned link when ignore rules change during staging.
        let stage_paths = if exclusions.is_empty() && scan.pruned.is_empty() {
            None
        } else {
            Some(
                self.capture_stage_paths(
                    &repository.prefix,
                    exclusions,
                    &commit_env,
                    ignored,
                    &scan.files,
                )
                .await?,
            )
        };
        let mut stage_args = INDEX_CONFIG.to_vec();
        stage_args.extend(durable_write());
        stage_args.extend(long_paths());
        stage_args.extend(["-c", "core.safecrlf=false", "add"]);
        if sparse {
            stage_args.push("--sparse");
        }
        stage_args.push(if ignored { "-u" } else { "-A" });
        if stage_paths.is_some() {
            stage_args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        } else {
            stage_args.extend(["--", "."]);
        }
        match self
            .capture_execute(
                &stage_args,
                &commit_env,
                stage_paths.as_deref().map(stage_pathspecs),
            )
            .await
        {
            Err(original @ GitError::Vcs(_)) => {
                let recovery = async {
                    let candidates: Vec<String> = if let Some(paths) = &stage_paths {
                        paths
                            .iter()
                            .filter(|path| path.ends_with('/'))
                            .cloned()
                            .collect()
                    } else {
                        let mut args = long_paths().to_vec();
                        args.extend([
                            "ls-files",
                            "--others",
                            "--exclude-standard",
                            "-z",
                            "--",
                            ".",
                        ]);
                        let untracked = self
                            .execute(
                                &args,
                                ExecuteOptions {
                                    env: commit_env.clone(),
                                    capture: true,
                                    max_output_bytes: 16 * 1024 * 1024,
                                    ..Default::default()
                                },
                            )
                            .await?;
                        if untracked.stdout_truncated {
                            return Ok(false);
                        }
                        untracked
                            .stdout_text()
                            .split('\0')
                            .filter(|path| path.ends_with('/'))
                            .map(str::to_owned)
                            .collect()
                    };
                    if candidates.len() > 64 {
                        return Ok(false);
                    }
                    let mut exclusions = Vec::new();
                    for path in candidates {
                        let nested = self.at(self.cwd.join(&path));
                        if nested.cwd.join(".git").try_exists()?
                            && nested.head_commit().await?.is_none()
                        {
                            exclusions.push(path);
                        }
                    }
                    if exclusions.is_empty() {
                        return Ok(false);
                    }
                    let args = stage_args.clone();
                    let stdin = if let Some(paths) = &stage_paths {
                        let paths: Vec<_> = paths
                            .iter()
                            .filter(|path| {
                                !exclusions.iter().any(|excluded| {
                                    checkpoint_path_contains(excluded.trim_end_matches('/'), path)
                                })
                            })
                            .cloned()
                            .collect();
                        Some(stage_pathspecs(&paths))
                    } else {
                        let specs: Vec<_> = exclusions
                            .iter()
                            .map(|path| format!(":(exclude,literal){path}"))
                            .collect();
                        let mut retry = args.clone();
                        retry.extend(specs.iter().map(String::as_str));
                        self.capture_execute(&retry, &commit_env, None).await?;
                        return Ok(true);
                    };
                    self.capture_execute(&args, &commit_env, stdin).await?;
                    Ok::<_, GitError>(true)
                };
                match recovery.await {
                    Ok(true) => {}
                    Err(GitError::Process(t3_process::ProcessError::Cancelled)) => {
                        return Err(t3_process::ProcessError::Cancelled.into());
                    }
                    _ => return Err(original),
                }
            }
            result => {
                result?;
            }
        }
        let mut args = INDEX_CONFIG.to_vec();
        args.extend(durable_write());
        args.push("write-tree");
        let tree = self
            .capture_execute(&args, &commit_env, None)
            .await?
            .stdout_text();
        let tree = tree.trim();
        if tree.is_empty() {
            return Err(GitError::InvalidOutput(
                "git write-tree returned an empty tree oid".into(),
            ));
        }
        let turn_start = if head_mark.is_some() {
            match previous {
                Some(previous) => self
                    .capture_turn_start(checkpoint_ref, previous, &head_moves, &commit_env)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        } else {
            None
        };
        let mut args = durable_write();
        args.extend(["commit-tree", tree]);
        if repository.head {
            args.extend(["-p", repository.head_oid.as_deref().unwrap_or("HEAD")]);
        }
        if let Some(start) = &turn_start {
            args.extend(["-p", start]);
        }
        let message = format!(
            "t3 checkpoint ref={checkpoint_ref}{}",
            head_mark
                .map(|mark| format!("\n\nT3-Head-Moved-At: {mark}"))
                .unwrap_or_default()
        );
        args.extend(["-m", &message]);
        let commit = self
            .capture_execute(&args, &commit_env, None)
            .await?
            .stdout_text();
        let commit = commit.trim();
        if commit.is_empty() {
            return Err(GitError::InvalidOutput(
                "git commit-tree returned an empty commit oid".into(),
            ));
        }
        let mut args = durable_write();
        args.extend(["update-ref", "--no-deref", checkpoint_ref, commit]);
        self.capture_execute(&args, &Environment::new(), None)
            .await?;
        Ok(())
    }

    async fn read_head_moves(&self) -> Vec<HeadMove> {
        let result = self
            .execute(
                &[
                    "-c",
                    "log.showSignature=false",
                    "log",
                    "--walk-reflogs",
                    "-n",
                    "512",
                    "--date=unix",
                    "--format=%H %gd %gs",
                    "HEAD",
                    "--",
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await;
        let Ok(result) = result else {
            return Vec::new();
        };
        if result.code != Some(0) {
            return Vec::new();
        }
        let pattern =
            regex::Regex::new(r"^([0-9a-f]+) HEAD@\{(\d+)\} (.*)$").expect("HEAD reflog pattern");
        result
            .stdout_text()
            .lines()
            .filter_map(|line| {
                pattern.captures(line).map(|parts| HeadMove {
                    to: parts[1].into(),
                    at: parts[2].into(),
                    reason: parts[3].into(),
                })
            })
            .collect()
    }

    async fn capture_turn_start(
        &self,
        reference: &str,
        previous: &str,
        moves: &[HeadMove],
        commit_env: &Environment,
    ) -> Result<Option<String>, GitError> {
        validate_revision(previous)?;
        let output = self
            .execute(
                &[
                    "-c",
                    "log.showSignature=false",
                    "log",
                    "--no-walk",
                    "--format=%H %T %ct %P%n%B",
                    &format!("{previous}^{{commit}}"),
                ],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        if output.code != Some(0) {
            return Ok(None);
        }
        let text = output.stdout_text();
        let fields: Vec<_> = text
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .collect();
        let [commit, tree, captured_at, previous_head, ..] = fields.as_slice() else {
            return Ok(None);
        };
        let pattern = regex::Regex::new(r"(?m)^T3-Head-Moved-At: (\d+) (\d+)$")
            .expect("checkpoint trailer pattern");
        let Some(mark) = pattern.captures(&text) else {
            return Ok(None);
        };
        let Some(oldest) = moves.iter().rposition(|entry| entry.at == mark[1]) else {
            return Ok(None);
        };
        if oldest == 511 {
            return Ok(None);
        }
        let Some(since) = mark[2]
            .parse::<usize>()
            .ok()
            .and_then(|count| oldest.checked_add(1)?.checked_sub(count))
        else {
            return Ok(None);
        };
        if moves
            .get(since)
            .is_none_or(|entry| entry.to != *previous_head)
        {
            return Ok(None);
        }
        let switches: Vec<_> = moves[..since]
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let from = &moves[index + 1].to;
                (entry.reason.starts_with("checkout: moving from ") && from != &entry.to)
                    .then_some((from, &entry.to))
            })
            .rev()
            .collect();
        let Some((_, landed_on)) = switches.last() else {
            return Ok(None);
        };
        let mut args = INDEX_CONFIG.to_vec();
        args.extend(["-c", "index.sparse=false", "read-tree", tree]);
        self.capture_execute(&args, commit_env, None).await?;
        for (from, to) in switches.iter() {
            let changes = self
                .execute(
                    &[
                        "-c",
                        "core.quotePath=true",
                        "diff-tree",
                        "-r",
                        "--no-renames",
                        from,
                        to,
                    ],
                    ExecuteOptions {
                        max_output_bytes: crate::DIFF_MAX_BYTES,
                        output_mode: OutputMode::Error,
                        ..Default::default()
                    },
                )
                .await?;
            let mut entries = String::new();
            for line in changes
                .stdout_text()
                .lines()
                .filter(|line| !line.is_empty())
            {
                let (metadata, path) = line
                    .split_once('\t')
                    .ok_or_else(|| GitError::InvalidOutput("branch switch tree entry".into()))?;
                let fields: Vec<_> = metadata.split_whitespace().collect();
                if fields.len() != 5 {
                    return Err(GitError::InvalidOutput(
                        "branch switch tree metadata".into(),
                    ));
                }
                if fields[0] == ":160000" || fields[1] == "160000" {
                    continue;
                }
                entries.push_str(&format!("{} {}\t{path}\n", fields[1], fields[3]));
            }
            if entries.is_empty() {
                continue;
            }
            let mut args = INDEX_CONFIG.to_vec();
            args.extend(["-c", "index.sparse=false", "update-index", "--index-info"]);
            self.capture_execute(&args, commit_env, Some(entries.into_bytes()))
                .await?;
        }
        let mut args = INDEX_CONFIG.to_vec();
        args.extend(durable_write());
        args.push("write-tree");
        let start_tree = self
            .capture_execute(&args, commit_env, None)
            .await?
            .stdout_text();
        let start_tree = start_tree.trim();
        if start_tree.is_empty() || start_tree == *tree {
            return Ok(None);
        }
        let mut args = durable_write();
        let message = format!("t3 checkpoint turn start ref={reference}");
        args.extend([
            "commit-tree",
            start_tree,
            "-p",
            landed_on,
            "-p",
            commit,
            "-m",
            &message,
        ]);
        let mut environment = commit_env.clone();
        environment.insert(
            "GIT_COMMITTER_DATE".into(),
            Some(format!("{captured_at} +0000").into()),
        );
        let start = self
            .capture_execute(&args, &environment, None)
            .await?
            .stdout_text();
        Ok((!start.trim().is_empty()).then(|| start.trim().to_owned()))
    }

    pub async fn resolve_checkpoint(
        &self,
        checkpoint_ref: &str,
    ) -> Result<Option<String>, GitError> {
        validate_revision(checkpoint_ref)?;
        let revision = format!("{checkpoint_ref}^{{commit}}");
        let output = self
            .execute(
                &["rev-parse", "--verify", "--quiet", &revision],
                ExecuteOptions {
                    env: env(&[("LC_ALL", "C")]),
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        if output.code == Some(1)
            && !output
                .stderr_text()
                .lines()
                .any(|line| line.starts_with("error:") || line.starts_with("fatal:"))
        {
            return Ok(None);
        }
        if output.code != Some(0) {
            return Err(self.vcs_exit(
                "GitVcsDriver.checkpoints.resolveCheckpointCommit",
                5,
                &output,
            ));
        }
        let text = output.stdout_text();
        Ok((!text.trim().is_empty()).then(|| text.trim().into()))
    }
    pub async fn has_checkpoint(&self, checkpoint_ref: &str) -> Result<bool, GitError> {
        Ok(self.resolve_checkpoint(checkpoint_ref).await?.is_some())
    }
    pub(crate) async fn head_commit(&self) -> Result<Option<String>, GitError> {
        let output = self
            .execute(
                &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok((output.code == Some(0) && !output.stdout.is_empty())
            .then(|| output.stdout_text().trim().to_owned()))
    }
    pub async fn delete_checkpoint_refs(&self, refs: &[impl AsRef<str>]) -> Result<(), GitError> {
        let mut commands = String::new();
        let mut seen = std::collections::BTreeSet::new();
        for reference in refs {
            let reference = reference.as_ref();
            validate_checkpoint_ref(reference)?;
            if seen.insert(reference) {
                commands.push_str(&format!("delete {reference}\n"));
            }
        }
        if !commands.is_empty() {
            self.execute(
                &["update-ref", "--no-deref", "--stdin"],
                ExecuteOptions {
                    capture: true,
                    stdin: Some(commands.into_bytes()),
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await?;
        }
        Ok(())
    }
    pub async fn list_checkpoint_refs(&self) -> Result<Vec<crate::GitRef>, GitError> {
        self.list_refs(&[CHECKPOINT_REF_PREFIX]).await
    }
    pub async fn prune_checkpoint_refs(
        &self,
        scope_id: &str,
        keep: &std::collections::HashSet<String>,
    ) -> Result<Vec<String>, GitError> {
        let reference = checkpoint_ref(scope_id, 0);
        let namespace = reference
            .strip_suffix("ordinal/0")
            .ok_or_else(|| GitError::InvalidOutput("checkpoint namespace".into()))?;
        let stale: Vec<_> = self
            .list_refs(&[namespace])
            .await?
            .into_iter()
            .filter(|reference| !keep.contains(&reference.name))
            .map(|reference| reference.name)
            .collect();
        self.delete_checkpoint_refs(&stale).await?;
        Ok(stale)
    }

    /// Mutates this workspace, preserving ignored files and resetting its index to HEAD.
    /// Call `restore_checkpoint_isolated` when enforcing thread/worktree ownership.
    pub async fn restore_checkpoint(
        &self,
        reference: &str,
        fallback_to_head: bool,
    ) -> Result<bool, GitError> {
        let commit = match self.resolve_checkpoint(reference).await? {
            Some(commit) => Some(commit),
            None if fallback_to_head => self.head_commit().await?,
            None => None,
        };
        let Some(commit) = commit else {
            return Ok(false);
        };
        let with_tree = format!("--with-tree={commit}");
        let tracked = self
            .text(&[
                "ls-files",
                "--cached",
                "--full-name",
                &with_tree,
                "-z",
                "--",
                ".",
            ])
            .await?;
        let tracked_paths: Vec<_> = tracked
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        let containment = self
            .checkpoint_scan(Some(&commit), &tracked_paths, &Environment::new())
            .await?;
        containment.recheck(&self.cancellation).await?;
        if !tracked.is_empty() {
            let sparse = self
                .execute(
                    &["config", "--bool", "core.sparseCheckout"],
                    ExecuteOptions {
                        allow_nonzero: true,
                        ..Default::default()
                    },
                )
                .await?
                .stdout_text()
                .trim()
                == "true";
            let mut restore = long_paths().to_vec();
            restore.extend([
                "-c",
                "core.hooksPath=/dev/null",
                "restore",
                "--source",
                &commit,
                "--worktree",
                "--staged",
            ]);
            let mut args = restore.clone();
            if !sparse {
                args.push("--ignore-skip-worktree-bits");
            }
            args.extend(["--", "."]);
            containment.recheck(&self.cancellation).await?;
            self.text(&args).await?;
            if sparse {
                let manual = self.manual_sparse_paths().await.unwrap_or_default();
                if !manual.is_empty() {
                    let mut args = restore;
                    args.extend([
                        "--ignore-skip-worktree-bits",
                        "--pathspec-from-file=-",
                        "--pathspec-file-nul",
                    ]);
                    let stdin = manual
                        .iter()
                        .flat_map(|path| format!(":(top,literal){path}\0").into_bytes())
                        .collect();
                    containment.recheck(&self.cancellation).await?;
                    self.execute(
                        &args,
                        ExecuteOptions {
                            stdin: Some(stdin),
                            ..Default::default()
                        },
                    )
                    .await?;
                }
            }
        }
        std::fs::create_dir_all(&self.cwd)?;
        let cleaned = self.clean_checkpoint_workspace(&containment).await;
        // Even a failed clean must leave staged state at HEAD.
        containment.recheck(&self.cancellation).await?;
        std::fs::create_dir_all(&self.cwd)?;
        let mut args = long_paths().to_vec();
        args.extend([
            "-c",
            "core.hooksPath=/dev/null",
            "reset",
            "--quiet",
            "--",
            ".",
        ]);
        self.text(&args).await?;
        cleaned?;
        Ok(true)
    }
    async fn manual_sparse_paths(&self) -> Result<Vec<String>, GitError> {
        let scan = Arc::new(Mutex::new(IndexScan {
            sparse: true,
            restore: true,
            start: true,
            ..Default::default()
        }));
        let callback = scan.clone();
        self.execute(
            &["ls-files", "--full-name", "--sparse", "-v", "-z", "--", "."],
            ExecuteOptions {
                max_output_bytes: 4096,
                on_stdout_chunk: Some(Arc::new(move |chunk| {
                    if let Ok(mut scan) = callback.lock() {
                        scan.feed(chunk);
                    }
                })),
                ..Default::default()
            },
        )
        .await?;
        let paths = {
            let mut scan = scan
                .lock()
                .map_err(|_| GitError::InvalidOutput("sparse path scan poisoned".into()))?;
            if scan.fallback || !scan.start {
                return Err(GitError::InvalidOutput("sparse path scan".into()));
            }
            std::mem::take(&mut scan.skipped)
        };
        if paths.is_empty() {
            return Ok(paths);
        }
        let stdin = format!("{}\0", paths.join("\0")).into_bytes();
        let rules = self
            .execute(
                &["sparse-checkout", "check-rules", "-z"],
                ExecuteOptions {
                    max_output_bytes: stdin.len(),
                    stdin: Some(stdin),
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?;
        Ok(rules
            .stdout_text()
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect())
    }

    async fn clean_checkpoint_workspace(
        &self,
        containment: &crate::safety::CheckpointScan,
    ) -> Result<(), GitError> {
        let excluded = containment.excluded_pathspecs();
        let mut args = long_paths().to_vec();
        args.extend(["clean", "-fd", "--", "."]);
        args.extend(excluded.iter().map(String::as_str));
        for pass in 0..4 {
            containment.recheck(&self.cancellation).await?;
            std::fs::create_dir_all(&self.cwd)?;
            let output = self
                .execute(
                    &args,
                    ExecuteOptions {
                        env: env(&[("LC_ALL", "C")]),
                        allow_nonzero: true,
                        ..Default::default()
                    },
                )
                .await?;
            let stderr = output.stderr_text();
            if output.code != Some(0) {
                let emptied = output.code == Some(1)
                    && stderr.trim().starts_with("warning: failed to remove ./: ")
                    && stderr.trim().lines().count() == 1
                    && std::fs::read_dir(&self.cwd)
                        .is_ok_and(|mut entries| entries.next().is_none());
                if !emptied {
                    return Err(self.vcs_exit(
                        "GitVcsDriver.checkpoints.restoreCheckpoint",
                        args.len(),
                        &output,
                    ));
                }
                return Ok(());
            }
            if !output
                .stdout_text()
                .lines()
                .any(|line| line.starts_with("Removing "))
            {
                return Ok(());
            }
            // Git for Windows can repeatedly remove a recreated empty "./".
            match std::fs::read_dir(&self.cwd) {
                Ok(mut entries) => {
                    if entries.next().is_none() {
                        return Ok(());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
            if pass == 3 {
                std::fs::create_dir_all(&self.cwd)?;
                let mut args = long_paths().to_vec();
                args.extend(["clean", "-dn", "--", "."]);
                args.extend(excluded.iter().map(String::as_str));
                if self
                    .execute(
                        &args,
                        ExecuteOptions {
                            env: env(&[("LC_ALL", "C")]),
                            ..Default::default()
                        },
                    )
                    .await?
                    .stdout_text()
                    .lines()
                    .any(|line| line.starts_with("Would remove "))
                {
                    return Err(GitError::Unavailable(
                        "Could not clean the checkpoint workspace: untracked files remain.".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_revision(reference: &str) -> Result<(), GitError> {
    if reference.is_empty() || reference.starts_with('-') || reference.contains('\0') {
        return Err(GitError::InvalidArgument(
            "revision is empty or starts with an option".into(),
        ));
    }
    Ok(())
}
fn validate_checkpoint_ref(reference: &str) -> Result<(), GitError> {
    validate_ref_name(reference)?;
    if !reference.starts_with("refs/t3/") {
        return Err(GitError::InvalidArgument(
            "checkpoint writes require a refs/t3/ reference".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_ref_name(reference: &str) -> Result<(), GitError> {
    if !reference.starts_with("refs/")
        || reference.contains("..")
        || reference.contains("@{")
        || reference
            .chars()
            .any(|c| c.is_control() || " ~^:?*[\\".contains(c))
        || reference.split('/').any(|part| {
            part.is_empty()
                || part.starts_with('.')
                || part.ends_with('.')
                || part.ends_with(".lock")
        })
    {
        return Err(GitError::InvalidArgument(
            "Invalid literal Git reference.".into(),
        ));
    }
    Ok(())
}
pub(crate) fn validate_fetch_source(reference: &str) -> Result<(), GitError> {
    validate_revision(reference)?;
    validate_ref_name(&format!("refs/source/{reference}"))
}

pub(crate) fn validate_config_key(key: &str) -> Result<(), GitError> {
    if key.starts_with('-') || key.contains('\0') || !key.contains('.') {
        return Err(GitError::InvalidArgument("Invalid Git config key.".into()));
    }
    Ok(())
}
