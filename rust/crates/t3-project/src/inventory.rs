//! The native session inventory behind `agentSessions.scan` and `agentSessions.import`: the
//! discovery, bounded reads and Git identities of `AgentSessionScanner.ts` (`make`). The merge,
//! exclusions and import live in `sessions.rs`; record interpretation in `transcript.rs`.
//!
//! Session stores live in the user's home and are written by other programs, so every file is
//! hostile input: sizes and record counts are bounded before data is retained, JSON is read
//! through the bounded projection reader (`session_json.rs`), entries below a store root are read
//! only when they are regular files or directories (never links), and a path taken from file
//! content (a session's cwd, Codex project roots, a `.git` pointer) must be a local-disk absolute
//! path before any filesystem call. File contents never reach a log or an error.
use crate::{
    Error, Result,
    session_json::{read_projected_json, select_transcript_path},
    sessions::{
        RecentSession, RecentThreads, SessionCandidate, SessionCandidates, SessionInventory,
        SessionWorktree, same_transcript_identity,
    },
    transcript::parse_session_records,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf, Prefix},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use t3_contracts::{
    agent_sessions::{
        AgentSessionImportSource, AgentSessionInode, AgentSessionProjectGit, AgentSessionSource,
    },
    provider_instance::ProviderInstanceId,
};

const TRANSCRIPT_PREFIX_BYTES: usize = 32 * 1024;
const METADATA_READ_BYTES: u64 = 8 * 1024;
const MAX_TRANSCRIPT_SCAN_BYTES: u64 = 1024 * 1024;
const MAX_CODEX_SESSION_INDEX_BYTES: u64 = 1024 * 1024;
const MAX_CODEX_PROJECT_ROOT_ROWS: i64 = 100_000;
pub const MAX_TRANSCRIPTS_PER_SOURCE: usize = 5000;
const MAX_DISCOVERY_OPERATIONS_PER_SOURCE: usize = MAX_TRANSCRIPTS_PER_SOURCE * 4;
const MAX_METADATA_BYTES_PER_SOURCE: u64 = 64 * 1024 * 1024;
const MAX_METADATA_OPERATIONS_PER_SOURCE: usize = MAX_TRANSCRIPTS_PER_SOURCE * 4;
const MAX_METADATA_RECORDS_PER_SOURCE: usize = 100_000;
const MAX_METADATA_RECORDS_PER_TRANSCRIPT: usize = 1_000;
const RECENT_THREAD_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const MAX_IMPORTED_TRANSCRIPT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_IMPORTED_MESSAGES: usize = 200;
const MAX_IMPORT_HISTORY_BYTES: usize = 32 * 1024 * 1024;
const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_IMPORT_TRANSCRIPTS: usize = 100;
const MAX_IMPORT_RECORDS: usize = 100_000;
const GIT_FILE_READ_DEADLINE: Duration = Duration::from_secs(2);
const MAX_GIT_FILE_BYTES: u64 = 1024 * 1024;
const GIT_FILE_READ_CONCURRENCY: usize = 16;
const BLOCKED_GIT_FILE_RETRY_MS: i64 = 5 * 60 * 1000;
const MAX_BLOCKED_GIT_FILE_PATHS: usize = 1024;
/// zstd frames may ask for a window of up to 2 GiB; Codex writes far smaller ones.
const ZSTD_WINDOW_LOG_MAX: u32 = 27;
const COMPRESSED_SUFFIX: &str = ".jsonl.zst";
const CODEX_STATE_DATABASE: &str = "state_5.sqlite";
const MAX_CODEX_STATE_BYTES: u64 = 256 * 1024 * 1024;
const CODEX_STATE_QUERY_DEADLINE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------------------------
// Paths

/// A path on this machine's own disks: absolute, never two leading separators in any mix
/// (`//host`, `\\?\`, `/\host`), and on Windows no prefix but a drive letter. Checked before any
/// filesystem call on a path taken from file content, so a UNC path never makes Windows
/// authenticate to a remote host.
pub fn local_absolute(path: &str) -> bool {
    let mut chars = path.chars();
    let separator = |c: Option<char>| matches!(c, Some('/' | '\\'));
    if path.contains('\0') || (separator(chars.next()) && separator(chars.next())) {
        return false;
    }
    let path = Path::new(path);
    if !path.is_absolute() {
        return false;
    }
    match path.components().next() {
        Some(Component::Prefix(prefix)) => matches!(prefix.kind(), Prefix::Disk(_)),
        _ => true,
    }
}

/// `path.resolve`: absolute and lexically normalised, without touching the filesystem.
fn resolve(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn expand_home(value: &str, home: &Path) -> String {
    t3_workspace::paths::expand_home(value, home)
        .to_string_lossy()
        .into_owned()
}

fn normalize_for_match(value: &str, fold: bool) -> String {
    let normalized = format!("{}/", value.replace('\\', "/"));
    if fold {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

/// `normalizeProjectPathForComparison`.
fn comparison_key(value: &str) -> String {
    let value = value.replace('\\', "/");
    let value = if value.len() > 1 {
        value.trim_end_matches('/').to_owned()
    } else {
        value
    };
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

/// What the scan never offers as a project (`isExcludedProjectPath`).
#[derive(Clone)]
pub struct Exclusions {
    roots: BTreeSet<String>,
    ancestors: Vec<PathBuf>,
    base_dir: PathBuf,
    worktrees_dir: PathBuf,
    fold: bool,
}
impl Exclusions {
    pub fn new(paths: &InventoryPaths) -> Self {
        Self {
            roots: [
                paths.home.clone(),
                paths.temporary_root.clone(),
                "/tmp".into(),
                "/private/tmp".into(),
            ]
            .iter()
            .map(|p| comparison_key(&resolve(p).to_string_lossy()))
            .collect(),
            ancestors: vec![
                paths.home.join("Downloads"),
                paths.home.join("Documents").join("Codex"),
            ],
            base_dir: resolve(&paths.base_dir),
            worktrees_dir: resolve(&paths.worktrees_dir),
            fold: cfg!(windows),
        }
    }
    pub fn excluded(&self, candidate: &Path) -> bool {
        let text = candidate.to_string_lossy();
        let normalized = normalize_for_match(&text, self.fold);
        let starts = |prefix: &Path| {
            normalized.starts_with(&normalize_for_match(&prefix.to_string_lossy(), self.fold))
        };
        self.roots.contains(&comparison_key(&text))
            || self.ancestors.iter().any(|a| starts(a))
            || starts(&self.base_dir)
            || starts(&self.worktrees_dir)
            || normalized.contains("/.t3/worktrees/")
    }
}

// ---------------------------------------------------------------------------------------------
// File identity and bounded reads

/// `transcriptIdentity`: what Node's `stat` reports, from the open handle.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub file_path: String,
    pub size: u64,
    pub mtime_ms: Option<i64>,
    pub device: u64,
    pub inode: Option<String>,
    pub birthtime_ms: Option<i64>,
}
fn millis(time: std::io::Result<SystemTime>) -> Option<i64> {
    let time = time.ok()?;
    Some(match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).ok()?,
        Err(before) => -i64::try_from(before.duration().as_millis()).ok()?,
    })
}
fn file_identity(path: &str, file: &std::fs::File) -> std::io::Result<Identity> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    #[cfg(windows)]
    let (device, inode) = {
        let information = winapi_util::file::information(file)?;
        (
            information.volume_serial_number(),
            Some(information.file_index().to_string()),
        )
    };
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), Some(metadata.ino().to_string()))
    };
    #[cfg(not(any(windows, unix)))]
    let (device, inode) = (0, None);
    Ok(Identity {
        file_path: path.into(),
        size: metadata.len(),
        mtime_ms: millis(metadata.modified()),
        device,
        inode,
        birthtime_ms: millis(metadata.created()),
    })
}
fn number(value: i64) -> Option<t3_contracts::speech::SpeechNumber> {
    serde_json::from_value(json!(value)).ok()
}
impl Identity {
    fn source(
        &self,
        provider: AgentSessionSource,
        instance: ProviderInstanceId,
        session: &str,
    ) -> Option<AgentSessionImportSource> {
        Some(AgentSessionImportSource {
            provider,
            provider_instance_id: instance,
            provider_session_id: session.to_owned().into(),
            file_path: self.file_path.clone().into(),
            size: self.size,
            mtime_ms: self.mtime_ms.and_then(number),
            device: number(i64::try_from(self.device).ok()?)?,
            inode: self.inode.clone().map(AgentSessionInode::String),
            birthtime_ms: self.birthtime_ms.and_then(number),
        })
    }
    fn matches(&self, source: &AgentSessionImportSource) -> bool {
        self.source(
            source.provider.clone(),
            source.provider_instance_id.clone(),
            "x",
        )
        .is_some_and(|mine| same_transcript_identity(&mine, source))
    }
}

/// Opens `path` for reading without following a final link: on Windows a reparse point is
/// opened as itself (so the handle is not a regular file), on Unix the open fails.
pub(crate) fn open_handle(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT.
        options.custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}
/// A regular file below a store root (never a link): opened first, then its type and identity
/// are read from the handle, so a link swapped in before the open is never read.
fn open_regular(path: &Path) -> Option<(std::fs::File, Identity)> {
    let file = open_handle(path).ok()?;
    let identity = file_identity(&path.to_string_lossy(), &file).ok()?;
    Some((file, identity))
}
fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}
/// Directory entry names; links are left for the caller's checks to refuse.
fn list_directory(path: &Path) -> Vec<String> {
    if !is_real_directory(path) {
        return vec![];
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}
fn compressed(path: &str) -> bool {
    path.ends_with(COMPRESSED_SUFFIX)
}
/// The decompressed (or plain) bytes of a transcript.
fn transcript_reader(file: std::fs::File, path: &str) -> std::io::Result<Box<dyn Read>> {
    if !compressed(path) {
        return Ok(Box::new(file));
    }
    let mut decoder = zstd::stream::read::Decoder::new(file)?;
    decoder.window_log_max(ZSTD_WINDOW_LOG_MAX)?;
    Ok(Box::new(decoder))
}

/// Reads one record (up to the next newline) from a buffered stream, without buffering the line.
struct Record<'a, R> {
    inner: &'a mut R,
    done: bool,
    /// Whether any byte of this record was consumed.
    started: bool,
    /// Bytes at the front of the inner buffer known to hold no newline, so a long line is
    /// scanned once rather than once per byte the parser pulls.
    segment: usize,
}
impl<'a, R: BufRead> Record<'a, R> {
    fn new(inner: &'a mut R) -> Self {
        Self {
            inner,
            done: false,
            started: false,
            segment: 0,
        }
    }
}
impl<R: BufRead> BufRead for Record<'_, R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.done {
            return Ok(&[]);
        }
        if self.segment == 0 {
            let available = self.inner.fill_buf()?;
            if available.is_empty() {
                self.done = true;
                return Ok(&[]);
            }
            self.started = true;
            match available.iter().position(|b| *b == b'\n') {
                Some(0) => {
                    self.inner.consume(1);
                    self.done = true;
                    return Ok(&[]);
                }
                Some(end) => self.segment = end,
                None => self.segment = available.len(),
            }
        }
        let segment = self.segment;
        Ok(&self.inner.fill_buf()?[..segment])
    }
    fn consume(&mut self, amount: usize) {
        let amount = amount.min(self.segment);
        self.inner.consume(amount);
        self.segment -= amount;
    }
}
impl<R: BufRead> Read for Record<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(out.len());
        out[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

/// Splits bytes arriving in chunks into lines; `examined` counts the bytes searched for a newline.
#[derive(Default)]
pub(crate) struct LineSplitter {
    pending: Vec<u8>,
    /// Bytes at the front of `pending` already known to hold no newline.
    searched: usize,
    pub(crate) examined: usize,
}
impl LineSplitter {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }
    pub(crate) fn next_line(&mut self) -> Option<Vec<u8>> {
        let start = self.searched;
        let found = self.pending[start..].iter().position(|b| *b == b'\n');
        self.examined += found.map_or(self.pending.len() - start, |index| index + 1);
        let Some(index) = found else {
            self.searched = self.pending.len();
            return None;
        };
        // What follows the line has not been searched yet.
        self.searched = 0;
        Some(self.pending.drain(..=start + index).collect())
    }
    pub(crate) fn rest(&self) -> &[u8] {
        &self.pending
    }
}

// ---------------------------------------------------------------------------------------------
// Record helpers (`AgentSessionScanner.ts`)

/// `extractCwd`: `Some(None)` stops metadata scanning for a Codex subagent rollout.
fn extract_cwd(line: &str, source: &AgentSessionSource) -> Option<Option<String>> {
    let record: Value = serde_json::from_str(line).ok()?;
    let record = record.as_object()?;
    if let Some(cwd) = record.get("cwd").and_then(Value::as_str)
        && !cwd.trim().is_empty()
    {
        return Some(Some(cwd.into()));
    }
    let payload = record.get("payload")?.as_object()?;
    if *source == AgentSessionSource::Codex
        && record.get("type").and_then(Value::as_str) == Some("session_meta")
        && payload
            .get("source")
            .and_then(Value::as_object)
            .is_some_and(|s| s.contains_key("subagent") || s.contains_key("subAgent"))
    {
        return Some(None);
    }
    payload
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.trim().is_empty())
        .map(|cwd| Some(cwd.into()))
}
fn decoded_cwd(record: &Value) -> Option<String> {
    [&record["cwd"], &record["payload"]["cwd"]]
        .into_iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|cwd| !cwd.is_empty())
        .map(str::to_owned)
}
fn hidden_claude(record: &Value) -> bool {
    record["isSidechain"] == true || record["isMeta"] == true || record["isCompactSummary"] == true
}
fn text_block(block: &Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("text" | "input_text" | "output_text")
    )
}
/// `shouldRetainDecodedRecord`.
fn retain(source: &AgentSessionSource, record: &Value) -> bool {
    match source {
        AgentSessionSource::Pi => true,
        AgentSessionSource::ClaudeAgent => {
            if hidden_claude(record)
                || !matches!(record["type"].as_str(), Some("user" | "assistant"))
            {
                return false;
            }
            match &record["message"]["content"] {
                Value::String(text) => !text.trim().is_empty(),
                Value::Array(blocks) => blocks.iter().any(|b| {
                    text_block(b) && b["text"].as_str().is_some_and(|t| !t.trim().is_empty())
                }),
                _ => false,
            }
        }
        AgentSessionSource::Codex => {
            decoded_cwd(record).is_some()
                || matches!(
                    record["type"].as_str(),
                    Some("session_meta" | "turn_context")
                )
                || record["type"] == "event_msg" && record["payload"]["type"] == "user_message"
                || record["type"] == "response_item"
                    && record["payload"]["type"] == "message"
                    && matches!(
                        record["payload"]["role"].as_str(),
                        Some("user" | "assistant")
                    )
        }
    }
}
fn pi_conversation(record: &Value) -> bool {
    record["type"] == "message"
        && matches!(
            record["message"]["role"].as_str(),
            Some("user" | "assistant")
        )
}

/// `ClaudeFields`: Claude repeats session fields on records that need not be retained.
#[derive(Default, Clone)]
struct ClaudeFields {
    session_id: Option<String>,
    title: Option<String>,
    custom_title: Option<Option<String>>,
    model: Option<String>,
}
impl ClaudeFields {
    fn fold(&mut self, record: &Value) {
        if hidden_claude(record) {
            return;
        }
        let trimmed = |v: &Value| {
            v.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        if let Some(id) = trimmed(&record["sessionId"]) {
            self.session_id = Some(id);
        }
        if let Some(title) = trimmed(&record["aiTitle"]) {
            self.title = Some(title);
        }
        if let Some(custom) = record.get("customTitle").and_then(Value::as_str) {
            self.custom_title = Some(Some(custom.trim().to_owned()).filter(|s| !s.is_empty()));
        }
        if let Some(model) = trimmed(&record["message"]["model"]).filter(|m| m != "<synthetic>") {
            self.model = Some(model);
        }
    }
    /// One trailing record carrying the folded fields: `parse_session_records` lets the last
    /// value win, and this record is not a message.
    fn record(&self) -> Value {
        let mut record = json!({"type": "t3-session-fields"});
        if let Some(id) = &self.session_id {
            record["sessionId"] = json!(id);
        }
        if let Some(title) = &self.title {
            record["aiTitle"] = json!(title);
        }
        if let Some(custom) = &self.custom_title {
            record["customTitle"] = json!(custom.clone().unwrap_or_default());
        }
        if let Some(model) = &self.model {
            record["message"] = json!({"model": model});
        }
        record
    }
}

/// `createClaudeHistoryWindow`: the first user message and the latest 200 records.
#[derive(Default)]
struct ClaudeWindow {
    recent: std::collections::VecDeque<(usize, Value, usize)>,
    first_user: Option<(usize, Value)>,
    dropped: usize,
    next: usize,
}
impl ClaudeWindow {
    /// Returns the bytes released by an evicted record.
    fn push(&mut self, record: Value, bytes: usize) -> usize {
        let id = self.next;
        self.next += 1;
        if self.first_user.is_none() && record["type"] == "user" {
            self.first_user = Some((id, record.clone()));
        }
        self.recent.push_back((id, record, bytes));
        if self.recent.len() <= MAX_IMPORTED_MESSAGES {
            return 0;
        }
        match self.recent.pop_front() {
            Some((evicted, _, _))
                if self.first_user.as_ref().is_some_and(|(f, _)| *f == evicted) =>
            {
                0
            }
            Some((_, _, bytes)) => {
                self.dropped += 1;
                bytes
            }
            None => 0,
        }
    }
    fn records(self) -> Vec<Value> {
        let held: Vec<_> = self.recent.iter().map(|(id, _, _)| *id).collect();
        let mut records: Vec<_> = self.recent.into_iter().map(|(_, r, _)| r).collect();
        if let Some((id, first)) = self.first_user
            && !held.contains(&id)
        {
            records.insert(0, first);
        }
        records
    }
}

/// `codexHistoryBase`: a rewound rollout inherits a byte prefix of the one it replaced.
fn codex_history_base(records: &[Value]) -> Option<(String, u64)> {
    let base = &records.iter().find(|r| r["type"] == "session_meta")?["payload"]["history_base"];
    let thread = base["thread_id"].as_str()?;
    let offset = base["end_byte_offset"].as_f64()?;
    (offset >= 0.0 && offset.fract() == 0.0 && offset <= 9_007_199_254_740_991.0)
        .then(|| (thread.to_owned(), offset as u64))
}
/// Original rollouts end in their thread ID, rewound ones in `<thread ID>_<rollout ID>`.
fn codex_rollout_ids(path: &str) -> Option<(String, Option<String>)> {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)-([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})(?:_([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}))?\.jsonl(?:\.zst)?$",
        )
        .expect("rollout pattern")
    });
    let captures = PATTERN.captures(path)?;
    Some((
        captures.get(1)?.as_str().to_owned(),
        captures.get(2).map(|m| m.as_str().to_owned()),
    ))
}
/// `parseCodexThreadNames`: the newest nonempty name per thread wins.
pub fn parse_codex_thread_names(contents: &str) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for line in contents.split('\n') {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let (Some(id), Some(name)) = (entry["id"].as_str(), entry["thread_name"].as_str())
            && !name.trim().is_empty()
        {
            names.insert(id.to_owned(), name.trim().to_owned());
        }
    }
    names
}

// ---------------------------------------------------------------------------------------------
// Git configuration (`packages/shared/src/git.ts`, `SourceControlProvider.ts`)

pub mod git_config {
    use regex::Regex;
    use std::sync::LazyLock;

    fn regex(pattern: &str) -> Regex {
        Regex::new(pattern).expect("static pattern")
    }
    /// `gitConfigLines`: joins continued lines and drops lines inside an open quote.
    fn lines(text: &str) -> Vec<String> {
        static TOKEN: LazyLock<Regex> = LazyLock::new(|| regex(r##"\\[\s\S]?|["#;]"##));
        let mut out = vec![];
        let mut pending = String::new();
        let mut quoted = false;
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut continued = false;
            if line.ends_with('\\') || !pending.is_empty() {
                for token in TOKEN.find_iter(line) {
                    match token.as_str() {
                        "\"" => quoted = !quoted,
                        "#" | ";" if !quoted => break,
                        t if t.starts_with('\\') => continued = true,
                        _ => {}
                    }
                }
            }
            if continued {
                pending.push_str(&line[..line.len() - 1]);
                continue;
            }
            if !quoted {
                out.push(format!("{pending}{line}"));
            }
            pending.clear();
            quoted = false;
        }
        if !pending.is_empty() && !quoted {
            out.push(pending);
        }
        out
    }
    /// `parseGitConfigValue`.
    fn value(raw: &str) -> String {
        let chars: Vec<char> = raw.chars().collect();
        let mut out = String::new();
        let mut quoted = false;
        let mut index = 0;
        while index < chars.len() {
            let c = chars[index];
            if c == '\\' && index + 1 < chars.len() {
                out.push(chars[index + 1]);
                index += 2;
                continue;
            }
            if c == '"' {
                quoted = !quoted;
            } else if !quoted && (c == '#' || c == ';') {
                break;
            } else {
                out.push(c);
            }
            index += 1;
        }
        out.trim().to_owned()
    }
    /// `isTrueGitConfigValue`: Git's boolean parser accepts integers in its signed 32-bit range.
    fn truthy(value: &str) -> bool {
        static NUMERIC: LazyLock<Regex> =
            LazyLock::new(|| regex(r"^([+-]?)(0x[0-9a-f]+|0[0-7]*|[1-9][0-9]*)([kmg]?)$"));
        if matches!(value, "true" | "yes" | "on") {
            return true;
        }
        let Some(numeric) = NUMERIC.captures(value) else {
            return false;
        };
        let digits = &numeric[2];
        let magnitude = if let Some(hex) = digits.strip_prefix("0x") {
            i128::from_str_radix(hex, 16).ok()
        } else if digits.starts_with('0') {
            i128::from_str_radix(digits, 8).ok().or(Some(0))
        } else {
            digits.parse::<i128>().ok()
        };
        let scale: i128 = match &numeric[3] {
            "k" => 1024,
            "m" => 1024 * 1024,
            "g" => 1024 * 1024 * 1024,
            _ => 1,
        };
        let sign: i128 = if &numeric[1] == "-" { -1 } else { 1 };
        magnitude.is_some_and(|m| {
            let n = m.saturating_mul(scale).saturating_mul(sign);
            n != 0 && (-2_147_483_648..=2_147_483_647).contains(&n)
        })
    }
    /// `isBareGitConfig`.
    pub fn is_bare(text: &str) -> bool {
        static HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r"^\[\s*([^\]]*?)\s*\](.*)$"));
        static BARE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^bare\s*(?:=(.*)|[#;].*)?$"));
        let mut in_core = false;
        let mut bare = false;
        for raw in lines(text) {
            let mut line = raw.trim().to_owned();
            if let Some(header) = HEADER.captures(&line) {
                in_core = header[1].eq_ignore_ascii_case("core");
                line = header[2].trim().to_owned();
            }
            if !in_core {
                continue;
            }
            let Some(found) = BARE.captures(&line) else {
                continue;
            };
            let setting = found
                .get(1)
                .map_or_else(|| "true".to_owned(), |v| value(v.as_str()).to_lowercase());
            bare = truthy(&setting);
        }
        bare
    }
    /// `parseOriginUrlFromGitConfig`: origin's first URL, else the first other remote's.
    pub fn origin_url(text: &str) -> Option<String> {
        static CONTINUATION: LazyLock<Regex> = LazyLock::new(|| regex(r"\\\r?\n[ \t]*"));
        static HEADER: LazyLock<Regex> =
            LazyLock::new(|| regex(r#"(?i)^\[\s*remote(?:\s+"([^"]+)"|\.([^\]\s]+))\s*\](.*)$"#));
        static URL: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^url\s*=\s*(.*)$"));
        let joined = CONTINUATION.replace_all(text, "");
        let mut section: Option<String> = None;
        let (mut origin, mut first) = (None, None);
        for raw in joined.split('\n') {
            let mut line = raw.strip_suffix('\r').unwrap_or(raw).trim().to_owned();
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if let Some(header) = HEADER.captures(&line) {
                section = header
                    .get(1)
                    .map(|m| m.as_str().to_owned())
                    .or_else(|| header.get(2).map(|m| m.as_str().to_lowercase()));
                line = header[3].trim().to_owned();
            } else if line.starts_with('[') {
                section = None;
                continue;
            }
            let Some(name) = &section else { continue };
            let Some(found) = URL.captures(&line) else {
                continue;
            };
            let url = value(&found[1]);
            if url.is_empty() {
                continue;
            }
            if name == "origin" {
                origin.get_or_insert(url);
            } else {
                first.get_or_insert(url);
            }
        }
        origin.or(first)
    }
    /// `repositoryPathFromRemoteUrl`: `owner/name`, or a nested group path.
    pub fn repository_path(url: Option<&str>) -> Option<String> {
        static SCHEME: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^(?:ssh|https?|git)://"));
        // `(?!:)` after the scp-style host is checked below; the regex crate has no lookahead.
        static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
            regex(
                r"(?i)^(?:[^@/\s]+@[^:/\s]+:|([^@:/\\\s]{2,}):|(?:ssh|https?|git)://[^/]+/)((?:[^/\s]+/)+[^/\s]+?)(?:\.git)?/?$",
            )
        });
        let remote = url.map(str::trim).unwrap_or("");
        let trimmed = if SCHEME.is_match(remote) {
            remote.split(['?', '#']).next().unwrap_or("")
        } else {
            remote
        };
        if trimmed.is_empty() {
            return None;
        }
        let captures = PATTERN.captures(trimmed)?;
        if captures.get(1).is_some() && captures[2].starts_with(':') {
            return None;
        }
        let path = captures[2].trim();
        (!path.is_empty()).then(|| path.to_owned())
    }
}

// ---------------------------------------------------------------------------------------------
// Inventory

pub struct InventoryPaths {
    /// The account home (`os.homedir()`), never a client's.
    pub home: PathBuf,
    pub temporary_root: PathBuf,
    pub base_dir: PathBuf,
    pub worktrees_dir: PathBuf,
}

/// What the inventory needs from the host: live settings and the import records.
#[async_trait]
pub trait InventoryHost: Send + Sync {
    /// The server settings as JSON (`providerInstances` is read).
    async fn settings(&self) -> Result<Value>;
    /// `(cwd, source)` of every imported transcript recorded in the runtime repository.
    async fn imported_transcripts(&self) -> Result<Vec<(String, AgentSessionImportSource)>>;
    fn now_ms(&self) -> i64;
}

#[derive(Clone)]
struct Transcript {
    file_path: String,
    mtime_ms: i64,
    instance: ProviderInstanceId,
    home_path: PathBuf,
    identity: Identity,
    project_roots: Option<Vec<String>>,
}
#[derive(Clone)]
struct RawCandidate {
    cwd: String,
    source: AgentSessionSource,
    instance: ProviderInstanceId,
    home_path: PathBuf,
    last_active_ms: i64,
    transcripts: Vec<Transcript>,
}
#[derive(Clone)]
struct Collected {
    candidates: Vec<RawCandidate>,
    truncated: bool,
    metadata_truncated: bool,
}
struct Budget {
    bytes: u64,
    operations: usize,
    records: usize,
    truncated: bool,
}
#[derive(Clone)]
struct DirectoryProject {
    key: String,
    path: String,
    git: Option<AgentSessionProjectGit>,
    worktree_git_dir: Option<PathBuf>,
}
enum GitIdentity {
    Repository(Option<AgentSessionProjectGit>),
    Worktree {
        git_dir: PathBuf,
        main_checkout: Option<PathBuf>,
        git: Option<AgentSessionProjectGit>,
    },
    NotGit,
}

pub struct NativeSessionInventory {
    paths: InventoryPaths,
    environment: BTreeMap<String, String>,
    exclusions: Exclusions,
    host: Arc<dyn InventoryHost>,
    cache: Mutex<Option<Collected>>,
    /// One collection at a time; a caller that waited behind another reuses its result.
    collecting: tokio::sync::Mutex<()>,
    collections: std::sync::atomic::AtomicU64,
    /// Concurrent client imports must not hold several selected-history budgets.
    import_lock: Arc<tokio::sync::Mutex<()>>,
    blocked_git_files: Arc<Mutex<HashMap<PathBuf, i64>>>,
}

fn source_name(source: &AgentSessionSource) -> &'static str {
    match source {
        AgentSessionSource::ClaudeAgent => "claudeAgent",
        AgentSessionSource::Codex => "codex",
        AgentSessionSource::Pi => "pi",
    }
}
/// An environment variable; Windows names are case-insensitive.
fn env_get(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    if cfg!(windows) {
        env.iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.clone())
    } else {
        env.get(key).cloned()
    }
}
fn scan_error(operation: &'static str) -> Error {
    Error::Operation {
        operation,
        detail: "unavailable".into(),
    }
}
fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl NativeSessionInventory {
    pub fn new(
        paths: InventoryPaths,
        environment: BTreeMap<String, String>,
        host: Arc<dyn InventoryHost>,
    ) -> Self {
        Self {
            exclusions: Exclusions::new(&paths),
            paths,
            environment,
            host,
            cache: Mutex::new(None),
            collecting: tokio::sync::Mutex::new(()),
            collections: std::sync::atomic::AtomicU64::new(0),
            import_lock: Arc::new(tokio::sync::Mutex::new(())),
            blocked_git_files: Arc::default(),
        }
    }

    fn excluded(&self, path: &Path) -> bool {
        self.exclusions.excluded(path)
    }

    /// The configured store roots per source, in TypeScript's order and with its fallbacks.
    fn homes(
        &self,
        settings: &Value,
        source: &AgentSessionSource,
    ) -> Vec<(PathBuf, ProviderInstanceId)> {
        let name = source_name(source);
        let empty = serde_json::Map::new();
        let configured = settings["providerInstances"].as_object().unwrap_or(&empty);
        let mut instances: Vec<(String, Value)> = configured
            .iter()
            .filter(|(_, instance)| {
                instance["driver"] == name
                    && t3_shared::project_settings::provider_instance_enabled(instance)
            })
            .map(|(id, instance)| (id.clone(), instance.clone()))
            .collect();
        if !configured.contains_key(name) {
            let default = json!({"driver": name});
            if t3_shared::project_settings::provider_instance_enabled(&default) {
                instances.push((name.to_owned(), default));
            }
        }
        instances.sort_by_key(|(id, _)| usize::from(id != name));
        let home = &self.paths.home;
        let mut homes = vec![];
        let mut seen = BTreeSet::new();
        for (id, instance) in instances {
            // The instance's last setting of `key` when it holds text (an unresolved secret
            // reference does not), else the host's.
            let configured_env = |key: &str| -> Option<String> {
                instance["environment"]
                    .as_array()
                    .and_then(|vars| vars.iter().rev().find(|v| v["name"] == key))
                    .and_then(|v| v["value"].as_str().map(str::to_owned))
                    .or_else(|| env_get(&self.environment, key))
            };
            let config = &instance["config"];
            if !(config.is_null() || config.is_object()) {
                continue;
            }
            let text = |key: &str| -> Option<String> {
                match &config[key] {
                    Value::Null => Some(String::new()),
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                }
            };
            let path = match source {
                AgentSessionSource::Pi => {
                    let env = self.pi_environment(&instance);
                    let pi_home = env_get(&env, if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                        .filter(|h| !h.is_empty())
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.clone());
                    let setting = |key: &str| {
                        env_get(&env, key)
                            .map(|v| v.trim().to_owned())
                            .filter(|v| !v.is_empty())
                    };
                    if let Some(dir) = setting("PI_CODING_AGENT_SESSION_DIR") {
                        resolve(Path::new(&expand_home(&dir, &pi_home)))
                    } else {
                        let agent = setting("PI_CODING_AGENT_DIR")
                            .map(|d| PathBuf::from(expand_home(&d, &pi_home)))
                            .unwrap_or_else(|| pi_home.join(".pi").join("agent"));
                        resolve(&agent.join("sessions"))
                    }
                }
                AgentSessionSource::ClaudeAgent => {
                    let Some(configured) = text("homePath") else {
                        continue;
                    };
                    if !configured.trim().is_empty() {
                        resolve(Path::new(&expand_home(configured.trim(), home)))
                    } else if let Some(env) =
                        configured_env("CLAUDE_CONFIG_DIR").filter(|v| !v.trim().is_empty())
                    {
                        resolve(Path::new(&expand_home(env.trim(), home)))
                    } else {
                        home.join(".claude")
                    }
                }
                AgentSessionSource::Codex => {
                    let (Some(mut home_path), Some(shadow)) =
                        (text("homePath"), text("shadowHomePath"))
                    else {
                        continue;
                    };
                    if home_path.trim().is_empty()
                        && shadow.trim().is_empty()
                        && let Some(codex_home) =
                            configured_env("CODEX_HOME").filter(|v| !v.trim().is_empty())
                    {
                        home_path = codex_home;
                    }
                    if home_path.trim().is_empty() {
                        resolve(&home.join(".codex"))
                    } else {
                        resolve(Path::new(&expand_home(&home_path, home)))
                    }
                }
            };
            if !local_absolute(&path.to_string_lossy()) {
                continue;
            }
            let key = std::fs::canonicalize(&path)
                .map(|p| comparison_key(&p.to_string_lossy()))
                .unwrap_or_else(|_| comparison_key(&path.to_string_lossy()));
            if seen.insert(key) {
                homes.push((path, ProviderInstanceId(id)));
            }
        }
        homes
    }

    /// `mergeProviderInstanceEnvironment` over the host environment, for Pi's paths.
    fn pi_environment(&self, instance: &Value) -> BTreeMap<String, String> {
        let mut env = self.environment.clone();
        for variable in instance["environment"].as_array().into_iter().flatten() {
            // An unresolved secret reference carries no value.
            let (Some(name), Some(value)) = (variable["name"].as_str(), variable["value"].as_str())
            else {
                continue;
            };
            if cfg!(windows) {
                env.retain(|key, _| !key.eq_ignore_ascii_case(name));
            }
            env.insert(name.to_owned(), value.to_owned());
        }
        env
    }

    fn discover(
        source: &AgentSessionSource,
        root: &Path,
        instance: &ProviderInstanceId,
        budget: usize,
    ) -> (Vec<Transcript>, bool) {
        let mut operations = budget;
        let mut truncated = false;
        let mut transcripts = vec![];
        let read_dir = |dir: &Path, operations: &mut usize, truncated: &mut bool| {
            if *operations == 0 {
                *truncated = true;
                return vec![];
            }
            *operations -= 1;
            list_directory(dir)
        };
        let mut push = |path: PathBuf, operations: &mut usize, truncated: &mut bool| -> bool {
            if *operations == 0 {
                *truncated = true;
                return false;
            }
            *operations -= 1;
            if let Some((_, identity)) = open_regular(&path)
                && let Some(mtime) = identity.mtime_ms
            {
                transcripts.push(Transcript {
                    file_path: path.to_string_lossy().into_owned(),
                    mtime_ms: mtime,
                    instance: instance.clone(),
                    home_path: root.to_owned(),
                    identity,
                    project_roots: None,
                });
            }
            true
        };
        match source {
            AgentSessionSource::ClaudeAgent => {
                let projects = root.join("projects");
                for project in read_dir(&projects, &mut operations, &mut truncated) {
                    if operations == 0 {
                        truncated = true;
                        break;
                    }
                    let directory = projects.join(&project);
                    for entry in read_dir(&directory, &mut operations, &mut truncated)
                        .into_iter()
                        .filter(|e| e.ends_with(".jsonl"))
                    {
                        if !push(directory.join(entry), &mut operations, &mut truncated) {
                            break;
                        }
                    }
                }
            }
            AgentSessionSource::Codex => {
                let sessions = root.join("sessions");
                let sorted = |mut v: Vec<String>| {
                    v.sort();
                    v.reverse();
                    v
                };
                'years: for year in sorted(read_dir(&sessions, &mut operations, &mut truncated)) {
                    if operations == 0 {
                        truncated = true;
                        break;
                    }
                    for month in sorted(read_dir(
                        &sessions.join(&year),
                        &mut operations,
                        &mut truncated,
                    )) {
                        if operations == 0 {
                            truncated = true;
                            break 'years;
                        }
                        for day in sorted(read_dir(
                            &sessions.join(&year).join(&month),
                            &mut operations,
                            &mut truncated,
                        )) {
                            if operations == 0 {
                                truncated = true;
                                break 'years;
                            }
                            let directory = sessions.join(&year).join(&month).join(&day);
                            for entry in
                                sorted(read_dir(&directory, &mut operations, &mut truncated))
                            {
                                if !entry.starts_with("rollout-")
                                    || !(entry.ends_with(".jsonl") || compressed(&entry))
                                {
                                    continue;
                                }
                                if !push(directory.join(entry), &mut operations, &mut truncated) {
                                    break;
                                }
                            }
                        }
                    }
                }
                let roots = Self::codex_project_roots(root);
                for transcript in &mut transcripts {
                    if let Some((thread, _)) = codex_rollout_ids(&transcript.file_path) {
                        transcript.project_roots = roots.get(&thread).cloned();
                    }
                }
            }
            AgentSessionSource::Pi => {
                for entry in read_dir(root, &mut operations, &mut truncated) {
                    if operations == 0 {
                        truncated = true;
                        break;
                    }
                    let path = root.join(&entry);
                    let files = if entry.ends_with(".jsonl") {
                        vec![path]
                    } else {
                        read_dir(&path, &mut operations, &mut truncated)
                            .into_iter()
                            .filter(|n| n.ends_with(".jsonl"))
                            .map(|n| path.join(n))
                            .collect()
                    };
                    for file in files {
                        if !push(file, &mut operations, &mut truncated) {
                            break;
                        }
                    }
                }
            }
        }
        (transcripts, truncated)
    }

    /// `readCodexProjectRoots`: Codex records a chat's project only in its state database.
    fn codex_project_roots(root: &Path) -> HashMap<String, Vec<String>> {
        use t3_persistence::rusqlite::{Connection, OpenFlags};
        let path = root.join(CODEX_STATE_DATABASE);
        let mut roots: HashMap<String, Vec<String>> = HashMap::new();
        // A regular file of bounded size, checked on a no-follow handle before SQLite opens it.
        if open_regular(&path).is_none_or(|(_, identity)| identity.size > MAX_CODEX_STATE_BYTES) {
            return roots;
        }
        let mut read = || -> t3_persistence::rusqlite::Result<()> {
            let connection = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )?;
            connection.busy_timeout(Duration::from_millis(100))?;
            connection.pragma_update(None, "query_only", true)?;
            connection.pragma_update(None, "trusted_schema", false)?;
            let pages: i64 = connection.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let page_size: i64 = connection.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            if pages.saturating_mul(page_size) > MAX_CODEX_STATE_BYTES as i64 {
                return Ok(());
            }
            // Views (or anything else a hostile file could name) never run: both must be tables.
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN ('threads', 'project_roots')",
                [],
                |r| r.get(0),
            )?;
            if tables != 2 {
                return Ok(());
            }
            // The query is interrupted at the deadline; the watcher ends when the query does.
            let interrupt = connection.get_interrupt_handle();
            let (finished, watched) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || {
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    watched.recv_timeout(CODEX_STATE_QUERY_DEADLINE)
                {
                    interrupt.interrupt();
                }
            });
            let _finished = finished;
            let mut statement = connection.prepare(
                "SELECT threads.id AS id, project_roots.path AS path
                 FROM threads JOIN project_roots ON project_roots.project_id = threads.project_id
                 ORDER BY threads.rowid DESC, project_roots.position LIMIT ?1",
            )?;
            use t3_persistence::rusqlite::types::Value as Sql;
            let rows = statement.query_map([MAX_CODEX_PROJECT_ROOT_ROWS], |row| {
                Ok((row.get::<_, Sql>(0)?, row.get::<_, Sql>(1)?))
            });
            for row in rows?.flatten() {
                let (Sql::Text(id), Sql::Text(path)) = row else {
                    continue;
                };
                if path.trim().is_empty() {
                    continue;
                }
                roots.entry(id).or_default().push(path);
            }
            Ok(())
        };
        if read().is_err() {
            roots.clear();
        }
        roots
    }

    /// `readCwd`: the first recorded cwd within the metadata budgets.
    fn read_cwd(
        source: &AgentSessionSource,
        transcript: &Transcript,
        budget: &mut Budget,
    ) -> Option<String> {
        if transcript.identity.size == 0 {
            return None;
        }
        if budget.bytes == 0 || budget.operations < 2 || budget.records == 0 {
            budget.truncated = true;
            return None;
        }
        budget.operations -= 1;
        let (file, _) = open_regular(Path::new(&transcript.file_path))?;
        let mut reader = transcript_reader(file, &transcript.file_path).ok()?;
        let size = if compressed(&transcript.file_path) {
            u64::MAX
        } else {
            transcript.identity.size
        };
        let max = MAX_TRANSCRIPT_SCAN_BYTES.min(size);
        let mut read = 0u64;
        let mut records = 0usize;
        let mut pending = LineSplitter::default();
        let mut reserve = |budget: &mut Budget| {
            if records == MAX_METADATA_RECORDS_PER_TRANSCRIPT || budget.records == 0 {
                budget.truncated = true;
                return false;
            }
            records += 1;
            budget.records -= 1;
            true
        };
        let mut chunk = vec![0u8; METADATA_READ_BYTES as usize];
        while read < max {
            if budget.bytes == 0 || budget.operations == 0 {
                budget.truncated = true;
                return None;
            }
            let size = METADATA_READ_BYTES.min(max - read).min(budget.bytes) as usize;
            budget.operations -= 1;
            budget.bytes -= size as u64;
            let count = reader.read(&mut chunk[..size]).ok()?;
            if count == 0 {
                let last = String::from_utf8_lossy(pending.rest()).into_owned();
                return (!last.is_empty() && reserve(budget))
                    .then(|| extract_cwd(last.trim(), source).flatten())
                    .flatten();
            }
            read += count as u64;
            pending.push(&chunk[..count]);
            while let Some(line) = pending.next_line() {
                if !reserve(budget) {
                    return None;
                }
                match extract_cwd(String::from_utf8_lossy(&line).trim(), source) {
                    Some(Some(cwd)) => return Some(cwd),
                    Some(None) => return None,
                    None => {}
                }
            }
        }
        if read < size {
            budget.truncated = true;
            return None;
        }
        let last = String::from_utf8_lossy(pending.rest()).into_owned();
        (!last.is_empty() && reserve(budget))
            .then(|| extract_cwd(last.trim(), source).flatten())
            .flatten()
    }

    fn collect_sync(
        &self,
        settings: &Value,
        imported: &[(String, AgentSessionImportSource)],
    ) -> Collected {
        let mut imported_by_file: HashMap<
            (String, String),
            Vec<(String, AgentSessionImportSource)>,
        > = HashMap::new();
        for (cwd, source) in imported {
            imported_by_file
                .entry((
                    source.provider_instance_id.0.clone(),
                    source.file_path.0.clone(),
                ))
                .or_default()
                .push((cwd.clone(), source.clone()));
        }
        let mut raw = vec![];
        let mut truncated = false;
        let mut metadata_truncated = false;
        for source in [
            AgentSessionSource::ClaudeAgent,
            AgentSessionSource::Codex,
            AgentSessionSource::Pi,
        ] {
            let homes = self.homes(settings, &source);
            let count = homes.len().max(1);
            let (base, extra) = (
                MAX_DISCOVERY_OPERATIONS_PER_SOURCE / count,
                MAX_DISCOVERY_OPERATIONS_PER_SOURCE % count,
            );
            let mut transcripts = vec![];
            for (index, (home, instance)) in homes.iter().enumerate() {
                let budget = base + usize::from(index < extra);
                if budget == 0 {
                    truncated = true;
                    continue;
                }
                let (found, cut) = Self::discover(&source, home, instance, budget);
                truncated |= cut;
                transcripts.extend(found);
            }
            transcripts.sort_by(|a, b| {
                b.mtime_ms
                    .cmp(&a.mtime_ms)
                    .then_with(|| a.file_path.cmp(&b.file_path))
            });
            if transcripts.len() > MAX_TRANSCRIPTS_PER_SOURCE {
                truncated = true;
            }
            // Round-robin across instances so one large home cannot take the whole budget.
            let mut queues: Vec<(ProviderInstanceId, std::collections::VecDeque<Transcript>)> =
                vec![];
            for transcript in transcripts {
                match queues.iter_mut().find(|(id, _)| *id == transcript.instance) {
                    Some((_, queue)) => queue.push_back(transcript),
                    None => queues.push((transcript.instance.clone(), [transcript].into())),
                }
            }
            let mut selected = vec![];
            while selected.len() < MAX_TRANSCRIPTS_PER_SOURCE
                && queues.iter().any(|(_, q)| !q.is_empty())
            {
                for (_, queue) in &mut queues {
                    if selected.len() == MAX_TRANSCRIPTS_PER_SOURCE {
                        break;
                    }
                    if let Some(next) = queue.pop_front() {
                        selected.push(next);
                    }
                }
            }
            let mut budget = Budget {
                bytes: MAX_METADATA_BYTES_PER_SOURCE,
                operations: MAX_METADATA_OPERATIONS_PER_SOURCE,
                records: MAX_METADATA_RECORDS_PER_SOURCE,
                truncated: false,
            };
            let mut groups: Vec<RawCandidate> = vec![];
            for transcript in selected {
                // Unchanged imported transcripts keep their import cwd and spend no budget; Codex
                // rereads its first record, whose subagent marker must still exclude it.
                let recorded = (source != AgentSessionSource::Codex)
                    .then(|| {
                        imported_by_file
                            .get(&(transcript.instance.0.clone(), transcript.file_path.clone()))
                    })
                    .flatten()
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|(_, s)| s.provider == source && transcript.identity.matches(s))
                            .map(|(cwd, _)| cwd.clone())
                    });
                let Some(cwd) =
                    recorded.or_else(|| Self::read_cwd(&source, &transcript, &mut budget))
                else {
                    continue;
                };
                let cwd = self.project_cwd(&cwd, transcript.project_roots.as_deref());
                match groups
                    .iter_mut()
                    .find(|g| g.instance == transcript.instance && g.cwd == cwd)
                {
                    Some(group) => {
                        group.last_active_ms = group.last_active_ms.max(transcript.mtime_ms);
                        group.transcripts.push(transcript);
                    }
                    None => groups.push(RawCandidate {
                        cwd,
                        source: source.clone(),
                        instance: transcript.instance.clone(),
                        home_path: transcript.home_path.clone(),
                        last_active_ms: transcript.mtime_ms,
                        transcripts: vec![transcript],
                    }),
                }
            }
            metadata_truncated |= budget.truncated;
            raw.extend(groups);
        }
        Collected {
            candidates: raw,
            truncated: truncated || metadata_truncated,
            metadata_truncated,
        }
    }

    /// `projectCwd`: only chats left in an excluded folder (Codex's scratch) take their project's root.
    fn project_cwd(&self, cwd: &str, roots: Option<&[String]>) -> String {
        match roots.and_then(|r| r.first()) {
            Some(first)
                if self.excluded(&resolve(Path::new(&expand_home(
                    cwd.trim(),
                    &self.paths.home,
                )))) =>
            {
                first.clone()
            }
            _ => cwd.to_owned(),
        }
    }

    /// TypeScript runs a full collection per call; any `orchestration:read` session can call
    /// scan in a loop, so concurrent calls share one in-flight collection instead.
    async fn collect(self: &Arc<Self>) -> Result<Collected> {
        use std::sync::atomic::Ordering;
        let seen = self.collections.load(Ordering::SeqCst);
        let _single = self.collecting.lock().await;
        if self.collections.load(Ordering::SeqCst) != seen
            && let Some(collected) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).clone()
        {
            return Ok(collected);
        }
        let settings = self
            .host
            .settings()
            .await
            .map_err(|_| scan_error("read-settings"))?;
        let imported = self
            .host
            .imported_transcripts()
            .await
            .map_err(|_| scan_error("read-projects"))?;
        let this = self.clone();
        let collected =
            tokio::task::spawn_blocking(move || this.collect_sync(&settings, &imported))
                .await
                .map_err(|_| Error::Unavailable("session inventory"))?;
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(collected.clone());
        self.collections.fetch_add(1, Ordering::SeqCst);
        Ok(collected)
    }

    /// A candidate directory from session content: local, absolute and not excluded.
    fn candidate_directory(&self, cwd: &str) -> Option<PathBuf> {
        let expanded = expand_home(cwd.trim(), &self.paths.home);
        if !local_absolute(&expanded) {
            return None;
        }
        Some(resolve(Path::new(&expanded)))
    }

    /// `projectDirectoryStats`: an existing, non-excluded directory whose real path is not excluded.
    fn project_directory(&self, directory: &Path) -> bool {
        real_project_directory(&self.exclusions, directory).is_some()
    }

    /// `directoryIdentity`: the real path, compared as TypeScript compares project paths.
    fn directory_identity(directory: &Path) -> String {
        let real =
            t3_workspace::paths::canonicalize(directory).unwrap_or_else(|_| directory.to_owned());
        format!("path:{}", comparison_key(&real.to_string_lossy()))
    }

    /// `readGitFiles`: regular files only, at most 1 MiB, each read bounded by a deadline; a path
    /// whose read outlived it is skipped for five minutes.
    async fn read_git_files(&self, paths: Vec<PathBuf>) -> HashMap<PathBuf, String> {
        use futures_util::StreamExt;
        let now = self.host.now_ms();
        let unique: BTreeSet<PathBuf> = paths.into_iter().collect();
        let blocked = self.blocked_git_files.clone();
        futures_util::stream::iter(unique)
            .map(|path| {
                let blocked = blocked.clone();
                async move {
                    if !local_absolute(&path.to_string_lossy()) {
                        return (path, String::new());
                    }
                    {
                        let mut blocked = blocked.lock().unwrap_or_else(|e| e.into_inner());
                        if blocked.get(&path).is_some_and(|until| *until > now) {
                            return (path, String::new());
                        }
                        blocked.remove(&path);
                    }
                    let target = path.clone();
                    let read = tokio::task::spawn_blocking(move || {
                        let (file, _) = open_regular(&target)?;
                        let mut text = String::new();
                        file.take(MAX_GIT_FILE_BYTES)
                            .read_to_string(&mut text)
                            .ok()?;
                        Some(text)
                    });
                    match tokio::time::timeout(GIT_FILE_READ_DEADLINE, read).await {
                        Ok(Ok(text)) => (path, text.unwrap_or_default()),
                        Ok(Err(_)) => (path, String::new()),
                        Err(_) => {
                            let mut blocked = blocked.lock().unwrap_or_else(|e| e.into_inner());
                            if blocked.len() >= MAX_BLOCKED_GIT_FILE_PATHS
                                && let Some(first) = blocked.keys().next().cloned()
                            {
                                blocked.remove(&first);
                            }
                            blocked.insert(path.clone(), now + BLOCKED_GIT_FILE_RETRY_MS);
                            (path, String::new())
                        }
                    }
                }
            })
            .buffer_unordered(GIT_FILE_READ_CONCURRENCY)
            .collect()
            .await
    }

    /// A path a `.git` pointer or `commondir` names, resolved against its directory and then by
    /// the hardened resolver, so a link inside it to a share is refused before it is followed.
    fn pointed(base: &Path, target: &str) -> Option<PathBuf> {
        let lexical = Self::pointed_lexical(base, target)?;
        t3_workspace::paths::canonicalize(&lexical).ok()
    }
    fn pointed_lexical(base: &Path, target: &str) -> Option<PathBuf> {
        let target = target.trim();
        if target.is_empty() {
            return None;
        }
        let joined = if Path::new(target).is_absolute() || target.starts_with(['/', '\\']) {
            PathBuf::from(target)
        } else {
            base.join(target)
        };
        local_absolute(&joined.to_string_lossy()).then(|| resolve(&joined))
    }

    /// `readGitIdentities`, in three phases of concurrent reads.
    async fn git_identities(&self, directories: &[PathBuf]) -> HashMap<PathBuf, GitIdentity> {
        let mut identities = HashMap::new();
        let mut repositories: Vec<(PathBuf, PathBuf, Option<PathBuf>)> = vec![];
        let mut pointers = vec![];
        for directory in directories {
            let git = directory.join(".git");
            match std::fs::symlink_metadata(&git) {
                Err(_) => {
                    identities.insert(directory.clone(), GitIdentity::NotGit);
                }
                Ok(m) if m.file_type().is_dir() => {
                    repositories.push((directory.clone(), git, None))
                }
                Ok(_) => pointers.push((directory.clone(), git)),
            }
        }
        let texts = self
            .read_git_files(pointers.iter().map(|(_, g)| g.clone()).collect())
            .await;
        let mut linked = vec![];
        for (directory, git_path) in pointers {
            let target = texts
                .get(&git_path)
                .and_then(|t| {
                    t.lines()
                        .find_map(|l| l.trim_start().strip_prefix("gitdir:"))
                })
                .and_then(|t| Self::pointed(&directory, t));
            let Some(git_dir) = target else {
                identities.insert(directory, GitIdentity::NotGit);
                continue;
            };
            let parent_is_worktrees = git_dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "worktrees");
            if parent_is_worktrees {
                linked.push((directory, git_dir));
            } else {
                repositories.push((directory, git_dir, None));
            }
        }
        let texts = self
            .read_git_files(linked.iter().map(|(_, g)| g.join("commondir")).collect())
            .await;
        for (directory, git_dir) in linked {
            let common = texts
                .get(&git_dir.join("commondir"))
                .map(|t| t.trim().to_owned())
                .unwrap_or_default();
            match Self::pointed(&git_dir, &common) {
                Some(common) => repositories.push((directory, common, Some(git_dir))),
                None => {
                    identities.insert(
                        directory,
                        GitIdentity::Worktree {
                            git_dir,
                            main_checkout: None,
                            git: None,
                        },
                    );
                }
            }
        }
        let texts = self
            .read_git_files(
                repositories
                    .iter()
                    .map(|(_, g, _)| g.join("config"))
                    .collect(),
            )
            .await;
        for (directory, git_dir, worktree) in repositories {
            let config = texts
                .get(&git_dir.join("config"))
                .cloned()
                .unwrap_or_default();
            let origin = git_config::origin_url(&config);
            let git = Some(AgentSessionProjectGit {
                remote_key: origin.as_deref().map(t3_git::remote_repository_key),
                repository: git_config::repository_path(origin.as_deref()),
            });
            match worktree {
                // Bare repositories have no main checkout; their worktrees stand alone.
                Some(worktree) if !git_config::is_bare(&config) => {
                    identities.insert(
                        directory,
                        GitIdentity::Worktree {
                            git_dir: worktree,
                            main_checkout: (!config.is_empty()
                                && git_dir.file_name().is_some_and(|n| n == ".git"))
                            .then(|| git_dir.parent().map(Path::to_owned))
                            .flatten(),
                            git,
                        },
                    );
                }
                _ => {
                    identities.insert(directory, GitIdentity::Repository(git));
                }
            }
        }
        identities
    }

    /// `resolveDirectoryProjects`: a linked worktree counts toward its main checkout.
    async fn directory_projects(
        &self,
        directories: &[PathBuf],
    ) -> HashMap<PathBuf, Option<DirectoryProject>> {
        let unique: Vec<PathBuf> = directories
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let this_paths = unique.clone();
        let exclusions = self.exclusions.clone();
        let existing: Vec<(PathBuf, Option<PathBuf>)> = tokio::task::spawn_blocking(move || {
            this_paths
                .into_iter()
                .map(|d| {
                    let real = real_project_directory(&exclusions, &d);
                    (d, real)
                })
                .collect()
        })
        .await
        .unwrap_or_default();
        // Git identities are read below the resolved directory, never through a link.
        let present: Vec<PathBuf> = existing.iter().filter_map(|(_, real)| real.clone()).collect();
        let identities = self.git_identities(&present).await;
        let mut projects = HashMap::new();
        for (directory, real) in existing {
            let ok = real.is_some();
            let identity = real.as_ref().and_then(|real| identities.get(real));
            let project = match (ok, identity) {
                (
                    true,
                    Some(GitIdentity::Worktree {
                        git_dir,
                        main_checkout,
                        git,
                    }),
                ) => main_checkout
                    .as_ref()
                    .filter(|main| self.project_directory(main))
                    .map(|main| DirectoryProject {
                        key: Self::directory_identity(main),
                        path: main.to_string_lossy().into_owned(),
                        git: git.clone(),
                        worktree_git_dir: Some(git_dir.clone()),
                    }),
                (true, Some(identity)) => Some(DirectoryProject {
                    key: Self::directory_identity(&directory),
                    path: directory.to_string_lossy().into_owned(),
                    git: match identity {
                        GitIdentity::Repository(git) => git.clone(),
                        _ => None,
                    },
                    worktree_git_dir: None,
                }),
                _ => None,
            };
            projects.insert(directory, project);
        }
        projects
    }

    fn read_codex_thread_names(home: &Path) -> HashMap<String, String> {
        use std::io::{Seek, SeekFrom};
        let Some((mut file, identity)) = open_regular(&home.join("session_index.jsonl")) else {
            return HashMap::new();
        };
        let start = identity.size.saturating_sub(MAX_CODEX_SESSION_INDEX_BYTES);
        if file.seek(SeekFrom::Start(start)).is_err() {
            return HashMap::new();
        }
        let mut bytes = vec![];
        if file
            .take(MAX_CODEX_SESSION_INDEX_BYTES)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return HashMap::new();
        }
        let text = String::from_utf8_lossy(&bytes);
        // A tail read can start mid-record.
        let text = if start > 0 {
            text.split_once('\n').map_or("", |(_, rest)| rest)
        } else {
            &text
        };
        parse_codex_thread_names(text)
    }
}

/// A Pi entry's tree links: `type`, `id` and `parentId` only.
fn pi_links(value: &Value) -> Value {
    let mut kept = serde_json::Map::new();
    for key in ["type", "id", "parentId"] {
        if let Some(v) = value.get(key) {
            kept.insert(key.into(), v.clone());
        }
    }
    Value::Object(kept)
}
/// What a retained value costs the history budget: its encoded size.
fn retained_bytes(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

/// `readTranscript`'s result.
struct Snapshot {
    records: Vec<Value>,
    dropped: usize,
    cwd: Option<String>,
    history_bytes: usize,
}

/// `projectDirectoryStats`: the real path of a directory a session names, from the hardened
/// resolver first (it refuses links to shares, device names and the like before following
/// them), then checked to be a directory that is neither excluded nor reached from one.
fn real_project_directory(exclusions: &Exclusions, directory: &Path) -> Option<PathBuf> {
    if exclusions.excluded(directory) {
        return None;
    }
    let real = t3_workspace::paths::canonicalize(directory).ok()?;
    (std::fs::metadata(&real).is_ok_and(|m| m.is_dir())
        && local_absolute(&real.to_string_lossy())
        && !exclusions.excluded(&real))
    .then_some(real)
}

/// `readTranscript`: the selected records within the byte, record and history budgets, from a
/// file whose identity is the expected one before and after the read.
fn read_transcript(
    path: &str,
    expected: &Identity,
    source: &AgentSessionSource,
    prefix: Option<(u64, usize)>,
) -> Option<Snapshot> {
    let is_compressed = compressed(path);
    let read_bytes = prefix.map_or(
        if is_compressed {
            MAX_IMPORTED_TRANSCRIPT_BYTES
        } else {
            expected.size
        },
        |(end, _)| end,
    );
    if (!is_compressed && read_bytes > expected.size) || read_bytes > MAX_IMPORTED_TRANSCRIPT_BYTES
    {
        return None;
    }
    let (file, identity) = open_regular(Path::new(path))?;
    if identity != *expected {
        return None;
    }
    let check = file.try_clone().ok()?;
    let mut reader = BufReader::with_capacity(
        TRANSCRIPT_PREFIX_BYTES,
        transcript_reader(file, path).ok()?.take(read_bytes),
    );
    let mut records = vec![];
    let mut window = ClaudeWindow::default();
    let mut fields = ClaudeFields::default();
    let mut cwd = None;
    let mut history = prefix.map_or(0, |(_, h)| h);
    let mut count = 0usize;
    loop {
        let mut record = Record::new(&mut reader);
        let mut budget = MAX_IMPORT_HISTORY_BYTES.checked_sub(history)?;
        let available = budget;
        let value = match read_projected_json(&mut record, &mut budget, select_transcript_path) {
            Ok(value) => value,
            Err(crate::session_json::TranscriptJsonError::Malformed) => None,
            Err(_) => return None,
        };
        // A malformed record may stop early: drain the rest of its line.
        while !record.done {
            let mut sink = [0u8; 4096];
            if record.read(&mut sink).ok()? == 0 {
                break;
            }
        }
        if !record.started {
            break;
        }
        let bytes = available - budget;
        count += 1;
        if count > MAX_IMPORT_RECORDS && *source != AgentSessionSource::ClaudeAgent {
            return None;
        }
        let Some(value) = value else { continue };
        let valid = crate::transcript::valid_record(&value);
        if !valid {
            if *source == AgentSessionSource::Pi {
                // Pi accepts messages this schema rejects; keep their tree links.
                let links = ["type", "id", "parentId"];
                let valid_links = value.as_object().is_some_and(|o| {
                    o.get("type").is_none_or(Value::is_string)
                        && o.get("id").is_none_or(Value::is_string)
                        && o.get("parentId")
                            .is_none_or(|v| v.is_string() || v.is_null())
                });
                if valid_links {
                    let mut kept = serde_json::Map::new();
                    for key in links {
                        if let Some(v) = value.get(key) {
                            kept.insert(key.into(), v.clone());
                        }
                    }
                    let kept = Value::Object(kept);
                    history += retained_bytes(&kept);
                    if history > MAX_IMPORT_HISTORY_BYTES {
                        return None;
                    }
                    records.push(kept);
                }
            }
            continue;
        }
        if cwd.is_none() {
            cwd = decoded_cwd(&value);
        }
        if *source == AgentSessionSource::ClaudeAgent {
            fields.fold(&value);
        }
        if retain(source, &value) {
            match source {
                AgentSessionSource::ClaudeAgent => {
                    history = history + bytes - window.push(value, bytes);
                }
                AgentSessionSource::Pi
                    if value["type"] == "message" && !pi_conversation(&value) =>
                {
                    // Tool output can dwarf the conversation; keep only the tree links.
                    let links = pi_links(&value);
                    history += retained_bytes(&links);
                    if history > MAX_IMPORT_HISTORY_BYTES {
                        return None;
                    }
                    records.push(links);
                }
                _ => {
                    records.push(value);
                    history += bytes;
                }
            }
        }
    }
    // A file that ends before its stat size, or before the prefix a rewound rollout names, changed
    // under the read; only a compressed rollout's full length is unknown until its end.
    if (prefix.is_some() || !is_compressed) && reader.into_inner().limit() > 0 {
        return None;
    }
    if file_identity(path, &check).ok()? != *expected {
        return None;
    }
    let dropped = window.dropped;
    let mut records = if *source == AgentSessionSource::ClaudeAgent {
        window.records()
    } else {
        records
    };
    if *source == AgentSessionSource::ClaudeAgent {
        records.push(fields.record());
    }
    Some(Snapshot {
        records,
        dropped,
        cwd,
        history_bytes: history,
    })
}

#[async_trait]
impl SessionInventory for Arc<NativeSessionInventory> {
    async fn candidates(&self) -> Result<SessionCandidates> {
        let collected = self.collect().await?;
        let directories: Vec<PathBuf> = collected
            .candidates
            .iter()
            .filter_map(|c| self.candidate_directory(&c.cwd))
            .collect();
        let projects = self.directory_projects(&directories).await;
        let mut candidates = vec![];
        for candidate in collected.candidates {
            let Some(directory) = self.candidate_directory(&candidate.cwd) else {
                continue;
            };
            let Some(Some(project)) = projects.get(&directory) else {
                continue;
            };
            candidates.push(SessionCandidate {
                workspace_root: project.path.clone(),
                source: candidate.source,
                thread_count: candidate.transcripts.len() as u64,
                last_active_at: Some(t3_contracts::wire::WireDateTime(iso(
                    candidate.last_active_ms
                ))),
                git: project.git.clone(),
            });
        }
        Ok(SessionCandidates {
            candidates,
            truncated: collected.truncated,
        })
    }

    async fn recent_threads(
        &self,
        root: &str,
        completed: &[AgentSessionImportSource],
    ) -> Result<RecentThreads> {
        let inventory = self.clone();
        let home = inventory.paths.home.clone();
        let expanded = expand_home(root, &home);
        if !local_absolute(&expanded) {
            return Ok(Box::pin(futures_util::stream::empty()));
        }
        let root = resolve(Path::new(&expanded));
        let real_root = t3_workspace::paths::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if inventory.excluded(&root) || inventory.excluded(&real_root) {
            return Ok(Box::pin(futures_util::stream::empty()));
        }
        let root_identity = NativeSessionInventory::directory_identity(&root);
        let now = inventory.host.now_ms();
        let cutoff = now - RECENT_THREAD_WINDOW_MS;
        // Imports recorded since a budget-truncated collection can free budget it cut off.
        let cached = inventory
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let collected = match cached {
            Some(collected) if !collected.metadata_truncated => collected,
            _ => inventory.collect().await?,
        };
        let candidates = collected.candidates;
        let directories: Vec<PathBuf> = candidates
            .iter()
            .filter_map(|c| inventory.candidate_directory(&c.cwd))
            .collect();
        let projects = inventory.directory_projects(&directories).await;
        let mut eligible = vec![];
        for candidate in &candidates {
            let Some(directory) = inventory.candidate_directory(&candidate.cwd) else {
                continue;
            };
            let belongs = NativeSessionInventory::directory_identity(&directory) == root_identity
                || projects
                    .get(&directory)
                    .and_then(Option::as_ref)
                    .is_some_and(|p| p.key == root_identity);
            if !belongs {
                continue;
            }
            for transcript in &candidate.transcripts {
                if transcript.mtime_ms < cutoff || transcript.mtime_ms > now {
                    continue;
                }
                eligible.push((candidate.clone(), transcript.clone()));
            }
        }
        eligible.sort_by(|(_, a), (_, b)| {
            b.mtime_ms
                .cmp(&a.mtime_ms)
                .then_with(|| a.file_path.cmp(&b.file_path))
        });
        let mut completed_by_file: HashMap<(String, String), Vec<AgentSessionImportSource>> =
            HashMap::new();
        for source in completed {
            completed_by_file
                .entry((
                    source.provider_instance_id.0.clone(),
                    source.file_path.0.clone(),
                ))
                .or_default()
                .push(source.clone());
        }
        let rollouts: HashMap<(String, String), String> = candidates
            .iter()
            .filter(|c| c.source == AgentSessionSource::Codex)
            .flat_map(|c| {
                c.transcripts.iter().filter_map(move |t| {
                    let (thread, rollout) = codex_rollout_ids(&t.file_path)?;
                    Some((
                        (c.instance.0.clone(), rollout.unwrap_or(thread)),
                        t.file_path.clone(),
                    ))
                })
            })
            .collect();
        let state = RecentState {
            inventory,
            root_identity,
            projects,
            eligible: eligible.into(),
            completed_by_file,
            imported_sessions: BTreeSet::new(),
            names: HashMap::new(),
            worktrees: HashMap::new(),
            rollouts,
            bytes_remaining: MAX_IMPORT_BYTES,
            transcripts_remaining: MAX_IMPORT_TRANSCRIPTS,
        };
        Ok(Box::pin(futures_util::stream::unfold(
            state,
            |mut state| async move {
                loop {
                    let (candidate, transcript) = state.eligible.pop_front()?;
                    let lock = state.inventory.import_lock.clone();
                    let _guard = lock.lock().await;
                    match state.next(candidate, transcript).await {
                        Ok(Some(outcome)) => return Some((Ok(outcome), state)),
                        Ok(None) => continue,
                        Err(error) => return Some((Err(error), state)),
                    }
                }
            },
        )))
    }
}

struct RecentState {
    inventory: Arc<NativeSessionInventory>,
    root_identity: String,
    projects: HashMap<PathBuf, Option<DirectoryProject>>,
    eligible: std::collections::VecDeque<(RawCandidate, Transcript)>,
    completed_by_file: HashMap<(String, String), Vec<AgentSessionImportSource>>,
    imported_sessions: BTreeSet<(String, String)>,
    names: HashMap<PathBuf, HashMap<String, String>>,
    worktrees: HashMap<PathBuf, Option<SessionWorktree>>,
    rollouts: HashMap<(String, String), String>,
    bytes_remaining: u64,
    transcripts_remaining: usize,
}

impl RecentState {
    async fn belongs(&mut self, directory: &Path) -> bool {
        if NativeSessionInventory::directory_identity(directory) == self.root_identity {
            return true;
        }
        if !self.projects.contains_key(directory) {
            let resolved = self
                .inventory
                .directory_projects(&[directory.to_owned()])
                .await;
            self.projects.insert(
                directory.to_owned(),
                resolved.get(directory).cloned().flatten(),
            );
        }
        self.projects
            .get(directory)
            .and_then(Option::as_ref)
            .is_some_and(|p| p.key == self.root_identity)
    }

    async fn worktree(&mut self, directory: &Path) -> Option<SessionWorktree> {
        if let Some(known) = self.worktrees.get(directory) {
            return known.clone();
        }
        let git_dir = (NativeSessionInventory::directory_identity(directory) != self.root_identity)
            .then(|| {
                self.projects
                    .get(directory)
                    .cloned()
                    .flatten()
                    .and_then(|p| p.worktree_git_dir)
            })
            .flatten();
        let mut worktree = None;
        if let Some(git_dir) = git_dir {
            let head_path = git_dir.join("HEAD");
            let heads = self.inventory.read_git_files(vec![head_path.clone()]).await;
            let head = heads
                .get(&head_path)
                .map(|h| h.trim().to_owned())
                .unwrap_or_default();
            worktree = Some(SessionWorktree {
                path: directory.to_string_lossy().into_owned(),
                branch: head
                    .strip_prefix("ref: refs/heads/")
                    .filter(|b| !b.is_empty())
                    .map(str::to_owned),
            });
        }
        self.worktrees
            .insert(directory.to_owned(), worktree.clone());
        worktree
    }

    /// Reads the prefixes a rewound rollout inherits, oldest first; without all of them the
    /// rollout is imported on its own.
    async fn codex_history_base(
        &mut self,
        instance: &ProviderInstanceId,
        records: &[Value],
        mut history: usize,
    ) -> Option<Vec<Value>> {
        let mut prefixes: Vec<Vec<Value>> = vec![];
        let mut visited = BTreeSet::new();
        let mut base = codex_history_base(records);
        while let Some((thread, end)) = base {
            let path = (!visited.contains(&thread))
                .then(|| {
                    self.rollouts
                        .get(&(instance.0.clone(), thread.clone()))
                        .cloned()
                })
                .flatten()?;
            if end > self.bytes_remaining {
                return None;
            }
            visited.insert(thread);
            let (_, identity) = open_regular(Path::new(&path))?;
            self.bytes_remaining -= end;
            let source = AgentSessionSource::Codex;
            let read_path = path.clone();
            let inherited = tokio::task::spawn_blocking(move || {
                read_transcript(&read_path, &identity, &source, Some((end, history)))
            })
            .await
            .ok()??;
            history = inherited.history_bytes;
            base = codex_history_base(&inherited.records);
            prefixes.insert(
                0,
                inherited
                    .records
                    .into_iter()
                    .filter(|r| r["type"] != "session_meta")
                    .collect(),
            );
        }
        Some(prefixes.into_iter().flatten().collect())
    }

    async fn next(
        &mut self,
        candidate: RawCandidate,
        transcript: Transcript,
    ) -> Result<Option<RecentSession>> {
        let key = (candidate.instance.0.clone(), transcript.file_path.clone());
        let completed = self.completed_by_file.get(&key).cloned();
        if completed.is_none() && (self.transcripts_remaining == 0 || self.bytes_remaining == 0) {
            return Ok(Some(RecentSession::Skipped));
        }
        let path = transcript.file_path.clone();
        let Some((_, identity)) =
            tokio::task::spawn_blocking(move || open_regular(Path::new(&path)))
                .await
                .ok()
                .flatten()
        else {
            return Ok(Some(RecentSession::Skipped));
        };
        if let Some(done) = completed.as_ref().and_then(|sources| {
            sources
                .iter()
                .find(|s| s.provider == candidate.source && identity.matches(s))
        }) {
            let session = (
                done.provider_instance_id.0.clone(),
                done.provider_session_id.0.clone(),
            );
            if !self.imported_sessions.insert(session) {
                return Ok(None);
            }
            return Ok(Some(RecentSession::AlreadyImported(done.clone())));
        }
        if self.transcripts_remaining == 0
            || identity.size > MAX_IMPORTED_TRANSCRIPT_BYTES
            || identity.size > self.bytes_remaining
        {
            return Ok(Some(RecentSession::Skipped));
        }
        self.transcripts_remaining -= 1;
        self.bytes_remaining -= identity.size;
        let (path, expected, source) = (
            transcript.file_path.clone(),
            identity.clone(),
            candidate.source.clone(),
        );
        let Some(snapshot) =
            tokio::task::spawn_blocking(move || read_transcript(&path, &expected, &source, None))
                .await
                .ok()
                .flatten()
        else {
            return Ok(Some(RecentSession::Skipped));
        };
        let Some(cwd) = &snapshot.cwd else {
            return Ok(Some(RecentSession::Skipped));
        };
        let cwd = self
            .inventory
            .project_cwd(cwd, transcript.project_roots.as_deref());
        let Some(directory) = self.inventory.candidate_directory(&cwd) else {
            return Ok(Some(RecentSession::Skipped));
        };
        if !self.belongs(&directory).await {
            return Ok(Some(RecentSession::Skipped));
        }
        let worktree = self.worktree(&directory).await;
        let inherited = if candidate.source == AgentSessionSource::Codex {
            self.codex_history_base(
                &candidate.instance,
                &snapshot.records,
                snapshot.history_bytes,
            )
            .await
        } else {
            None
        };
        let records = match inherited {
            Some(mut inherited) => {
                inherited.extend(snapshot.records);
                inherited
            }
            None => snapshot.records,
        };
        let suffix = if compressed(&transcript.file_path) {
            COMPRESSED_SUFFIX
        } else {
            ".jsonl"
        };
        let fallback = Path::new(&transcript.file_path)
            .file_name()
            .map(|n| n.to_string_lossy().trim_end_matches(suffix).to_owned())
            .unwrap_or_default();
        let Some(mut thread) = parse_session_records(
            candidate.source.clone(),
            candidate.instance.clone(),
            &fallback,
            &iso(transcript.mtime_ms),
            &records,
            snapshot.dropped,
        ) else {
            return Ok(Some(RecentSession::Skipped));
        };
        if candidate.source == AgentSessionSource::Codex {
            if !self.names.contains_key(&candidate.home_path) {
                let home = candidate.home_path.clone();
                let names = tokio::task::spawn_blocking(move || {
                    NativeSessionInventory::read_codex_thread_names(&home)
                })
                .await
                .unwrap_or_default();
                self.names.insert(candidate.home_path.clone(), names);
            }
            if let Some(title) = self
                .names
                .get(&candidate.home_path)
                .and_then(|n| n.get(&thread.provider_session_id))
            {
                thread.title = title.clone();
            }
        }
        let Some(source) = identity.source(
            candidate.source.clone(),
            thread.provider_instance_id.clone(),
            &thread.provider_session_id,
        ) else {
            return Ok(Some(RecentSession::Skipped));
        };
        let session = (
            thread.provider_instance_id.0.clone(),
            thread.provider_session_id.clone(),
        );
        if !self.imported_sessions.insert(session) {
            return Ok(Some(RecentSession::Duplicate(source)));
        }
        Ok(Some(RecentSession::Importable {
            thread: Box::new(thread),
            source,
            worktree,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A newline-free megabyte arriving in 8 KiB chunks is searched once, not once per chunk.
    #[test]
    fn the_cwd_reader_searches_only_new_bytes() {
        let mut lines = LineSplitter::default();
        let chunk = vec![b'x'; 8 * 1024];
        for _ in 0..128 {
            lines.push(&chunk);
            assert!(lines.next_line().is_none());
        }
        lines.push(b"\n");
        assert_eq!(lines.next_line().map(|l| l.len()), Some(128 * 8 * 1024 + 1));
        assert!(lines.examined <= 128 * 8 * 1024 + 1, "examined {}", lines.examined);
    }

    /// A transcript is opened without following a link: the handle is the link itself, never
    /// its target, so a file swapped for a link cannot be read through it.
    #[test]
    fn transcripts_are_opened_without_following_links() {
        let temp = tempfile::tempdir().expect("temp");
        let target = temp.path().join("target.jsonl");
        std::fs::write(&target, "{}\n").expect("target");
        let link = temp.path().join("link.jsonl");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &link).expect("symlink");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let opened = open_handle(&link);
        assert!(
            opened.is_err() || opened.is_ok_and(|f| !f.metadata().expect("metadata").is_file()),
            "the link's target was opened"
        );
    }

    /// A Pi import holds at most the selected-history budget, whatever its tool records carry.
    #[test]
    fn pi_tool_records_count_against_the_history_budget() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("pi.jsonl");
        let payload = "x".repeat(12 * 1024 * 1024);
        let mut text = String::new();
        text.push_str(&format!("{}\n", json!({"type": "session", "id": "pi", "cwd": "/w"})));
        text.push_str(&format!("{}\n", json!({"type": "message", "id": "u", "parentId": null, "message": {"role": "user", "content": "hi"}})));
        for index in 0..3 {
            text.push_str(&format!(
                "{}\n",
                json!({"type": "message", "id": format!("t{index}"), "parentId": "u", "message": {"role": "toolResult"}, "payload": {"content": [{"type": "text", "text": payload}]}})
            ));
        }
        // An invalid record whose links are huge.
        text.push_str(&format!("{}\n", json!({"type": "message", "id": payload, "parentId": "u", "message": {"role": 7}})));
        std::fs::write(&path, text).expect("pi");
        let (_, identity) = open_regular(&path).expect("regular");
        let held = |snapshot: &Snapshot| serde_json::to_string(&snapshot.records).expect("json").len();
        match read_transcript(&path.to_string_lossy(), &identity, &AgentSessionSource::Pi, None) {
            None => {}
            Some(snapshot) => {
                assert!(held(&snapshot) <= MAX_IMPORT_HISTORY_BYTES, "held {}", held(&snapshot));
                assert!(snapshot.history_bytes >= held(&snapshot) / 2);
            }
        }
    }

    #[test]
    fn content_paths_must_be_local_disk_paths() {
        for refused in [
            "//host/share",
            r"\\host\share",
            r"/\host\share",
            r"\\?\C:\x",
            r"\\.\pipe\x",
            "relative/path",
            "",
        ] {
            assert!(!local_absolute(refused), "{refused}");
        }
        #[cfg(windows)]
        assert!(local_absolute(r"C:\Users\me\project"));
        #[cfg(unix)]
        assert!(local_absolute("/home/me/project"));
    }

    #[test]
    fn origin_url_bare_and_repository_path_follow_git() {
        let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:up/stream.git\n[remote \"origin\"]\n\turl = https://github.com/acme/widgets.git\n";
        assert_eq!(
            git_config::origin_url(config).as_deref(),
            Some("https://github.com/acme/widgets.git")
        );
        assert!(!git_config::is_bare(config));
        assert!(git_config::is_bare("[core]\n\tbare\n"));
        assert!(git_config::is_bare("[core] bare = yes\n"));
        assert!(git_config::is_bare("[core]\n\tbare = 1\n"));
        assert!(!git_config::is_bare("[core]\n\tbare = 0\n"));
        assert_eq!(
            git_config::origin_url(
                "[remote \"fork\"]\n\turl = \"git@gitlab.com:g/s/p.git\" ; note\n"
            )
            .as_deref(),
            Some("git@gitlab.com:g/s/p.git")
        );
        assert_eq!(
            git_config::repository_path(Some("https://github.com/acme/widgets.git")).as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            git_config::repository_path(Some("git@gitlab.com:g/s/p.git")).as_deref(),
            Some("g/s/p")
        );
        assert_eq!(git_config::repository_path(Some("/local/path")), None);
        assert_eq!(git_config::repository_path(None), None);
    }

    #[test]
    fn codex_names_and_rollout_ids() {
        let names = parse_codex_thread_names(
            "{\"id\":\"a\",\"thread_name\":\"First\"}\nnot json\n{\"id\":\"a\",\"thread_name\":\"Renamed\"}\n{\"id\":\"b\",\"thread_name\":\"  \"}\n",
        );
        assert_eq!(names.get("a").map(String::as_str), Some("Renamed"));
        assert!(!names.contains_key("b"));
        let id = "019a0000-0000-7000-8000-000000000001";
        let rollout = "019a0000-0000-7000-8000-000000000002";
        assert_eq!(
            codex_rollout_ids(&format!("rollout-2026-01-01T00-00-00-{id}.jsonl")),
            Some((id.into(), None))
        );
        assert_eq!(
            codex_rollout_ids(&format!(
                "rollout-2026-01-01T00-00-00-{id}_{rollout}.jsonl.zst"
            )),
            Some((id.into(), Some(rollout.into())))
        );
    }

    #[test]
    fn subagent_rollouts_stop_the_cwd_scan() {
        assert_eq!(
            extract_cwd(
                r#"{"type":"session_meta","payload":{"cwd":"/p","source":{"subagent":{}}}}"#,
                &AgentSessionSource::Codex
            ),
            Some(None)
        );
        assert_eq!(
            extract_cwd(
                r#"{"type":"session_meta","payload":{"cwd":"/p"}}"#,
                &AgentSessionSource::Codex
            ),
            Some(Some("/p".into()))
        );
        assert_eq!(
            extract_cwd(r#"{"cwd":"  "}"#, &AgentSessionSource::ClaudeAgent),
            None
        );
        assert_eq!(extract_cwd("[]", &AgentSessionSource::ClaudeAgent), None);
    }

    #[test]
    fn the_claude_window_keeps_the_first_user_message_and_counts_the_rest() {
        let mut window = ClaudeWindow::default();
        for index in 0..205 {
            let record = json!({"type": if index == 0 { "user" } else { "assistant" }, "n": index});
            window.push(record, 10);
        }
        assert_eq!(window.dropped, 4);
        let records = window.records();
        assert_eq!(records.len(), 201);
        assert_eq!(records[0]["n"], 0);
        assert_eq!(records[1]["n"], 5);
    }
}
