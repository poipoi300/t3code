#![cfg(feature = "claude-adapter")]

pub use crate::ports::claude::adapters::claude_adapter_v2::*;
use crate::{
    api::{Clock, IdGenerator},
    provider::*,
    runtime::sessions::{ContinuationCallbacks, ContinuationDelivery, ContinuationRequest},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use t3_claude_cli::{
    Client, Event, McpServer, PendingRequest, Session, SessionOptions,
    control::{
        DialogResponse, ElicitationResponse, OutboundRequest, PermissionDecision, Priority,
        ResumeChoice, UserMessage,
    },
    launch::{LaunchOptions, SystemPrompt, Tools, describe_spawn},
    message::Content,
};
use t3_contracts::{
    base_schemas::*,
    orchestration_v2::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_policy::ProviderApprovalDecision,
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex as AsyncMutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const EVENT_CAPACITY: usize = 64;
pub const CLAUDE_ATTACHMENT_MAX_BYTES: u64 = 28 * 1024 * 1024;
const CLAUDE_ATTACHMENT_MAX_PAGES: u64 = 100;

pub trait ClaudeAttachments: Send + Sync {
    fn path(&self, attachment: &t3_contracts::chat_attachment::ChatAttachment) -> Option<PathBuf>;
    fn extracted_text_path(&self, _path: &std::path::Path) -> Option<PathBuf> {
        None
    }
}

#[derive(Default)]
pub struct DirectoryClaudeAttachments {
    pub directory: Option<PathBuf>,
}
impl DirectoryClaudeAttachments {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory: Some(directory),
        }
    }
}
impl ClaudeAttachments for DirectoryClaudeAttachments {
    fn path(&self, attachment: &t3_contracts::chat_attachment::ChatAttachment) -> Option<PathBuf> {
        use t3_contracts::chat_attachment::ChatAttachment;
        let (id, name, mime, image) = match attachment {
            ChatAttachment::Image(a) => (&a.id, &a.name, &a.mime_type, true),
            ChatAttachment::File(a) => (&a.id, &a.name, &a.mime_type, false),
            ChatAttachment::Unknown(_) => return None,
        };
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return None;
        }
        let extension = std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let extension = if image {
            match mime.to_ascii_lowercase().as_str() {
                "image/jpeg" | "image/jpg" => "jpg",
                "image/svg+xml" => "svg",
                "image/png" => "png",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "image/avif" => "avif",
                "image/bmp" => "bmp",
                "image/heic" => "heic",
                "image/heif" => "heif",
                "image/tiff" => "tiff",
                _ if matches!(
                    extension.as_str(),
                    "avif"
                        | "bmp"
                        | "gif"
                        | "heic"
                        | "heif"
                        | "ico"
                        | "jpeg"
                        | "jpg"
                        | "png"
                        | "svg"
                        | "tiff"
                        | "webp"
                ) =>
                {
                    &extension
                }
                _ => "png",
            }
        } else if !extension.is_empty()
            && extension.len() <= 10
            && extension != "part"
            && extension.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            &extension
        } else {
            "bin"
        };
        Some(self.directory.as_ref()?.join(format!("{id}.{extension}")))
    }
    fn extracted_text_path(&self, path: &std::path::Path) -> Option<PathBuf> {
        if !matches!(
            path.extension()?.to_str()?,
            "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "rtf" | "epub" | "ipynb"
        ) {
            return None;
        }
        let mut name = path.as_os_str().to_owned();
        name.push(".txt");
        let path = PathBuf::from(name);
        path.is_file().then_some(path)
    }
}

#[derive(Clone, Copy, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAttachmentUsage {
    pub pages_used: u64,
    pub bytes_used: u64,
}
impl ClaudeAttachmentUsage {
    fn add(self, other: Self) -> Self {
        Self {
            pages_used: self.pages_used.saturating_add(other.pages_used),
            bytes_used: self.bytes_used.saturating_add(other.bytes_used),
        }
    }
    fn subtract(self, other: Self) -> Self {
        Self {
            pages_used: self.pages_used.saturating_sub(other.pages_used),
            bytes_used: self.bytes_used.saturating_sub(other.bytes_used),
        }
    }
}
#[derive(Default)]
struct AttachmentBudget {
    committed: ClaudeAttachmentUsage,
    pending: ClaudeAttachmentUsage,
    compacting: Option<ClaudeAttachmentUsage>,
}
struct AttachmentReservation {
    inner: Arc<RuntimeInner>,
    usage: ClaudeAttachmentUsage,
}
impl AttachmentReservation {
    fn commit(mut self) {
        let mut budget = lock(&self.inner.attachments);
        budget.pending = budget.pending.subtract(self.usage);
        budget.committed = budget.committed.add(self.usage);
        self.usage = ClaudeAttachmentUsage::default();
    }
}
impl Drop for AttachmentReservation {
    fn drop(&mut self) {
        let mut budget = lock(&self.inner.attachments);
        budget.pending = budget.pending.subtract(self.usage);
    }
}
pub struct PreparedClaudeMessage {
    pub message: UserMessage,
    pub usage: ClaudeAttachmentUsage,
}

pub fn prepare_claude_message(
    message: &TurnMessage,
    priority: Option<Priority>,
    source: &dyn ClaudeAttachments,
    mut usage: ClaudeAttachmentUsage,
) -> ProviderResult<PreparedClaudeMessage> {
    use std::io::Read;
    use t3_contracts::chat_attachment::ChatAttachment;
    let mut result = make_user_message(message, priority);
    if message.attachments.is_empty() {
        usage.bytes_used = usage.bytes_used.saturating_add(
            serde_json::to_vec(&result.message.content)
                .map_err(|_| operation(ProviderOperation::Start, "Could not encode Claude prompt"))?
                .len() as u64,
        );
        return Ok(PreparedClaudeMessage {
            message: result,
            usage,
        });
    }
    let mut text = message.text.clone();
    let paths: Vec<_> = message.attachments.iter().map(|a| source.path(a)).collect();
    for (attachment, path) in message.attachments.iter().zip(&paths) {
        let (kind, name, id) = match attachment {
            ChatAttachment::Image(a) => ("image", &a.name, &a.id),
            ChatAttachment::File(a) => ("file", &a.name, &a.id),
            ChatAttachment::Unknown(a) => (a.r#type.0.as_str(), &a.name, &a.id),
        };
        let context = match path {
            Some(path) => {
                let extracted = source
                    .extracted_text_path(path)
                    .map(|p| format!(" (text extracted to: {})", p.display()))
                    .unwrap_or_default();
                format!(
                    "[Attached {kind} \"{name}\" is saved at: {}{extracted}]",
                    path.display()
                )
            }
            None => format!(
                "[Attached {kind} \"{name}\" is unavailable: could not resolve a saved path for attachment '{id}'.]"
            ),
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&context);
    }
    let text_block: t3_claude_cli::message::ContentBlock =
        serde_json::from_value(json!({"type":"text","text":text})).map_err(|_| {
            operation(
                ProviderOperation::Start,
                "Could not encode Claude attachment text",
            )
        })?;
    usage.bytes_used = usage.bytes_used.saturating_add(
        serde_json::to_vec(&vec![&text_block])
            .map_err(|_| {
                operation(
                    ProviderOperation::Start,
                    "Could not encode Claude attachment text",
                )
            })?
            .len() as u64,
    );
    let mut images = BTreeMap::new();
    for (index, attachment) in message.attachments.iter().enumerate() {
        let ChatAttachment::Image(image) = attachment else {
            continue;
        };
        if !matches!(
            image.mime_type.as_str(),
            "image/gif" | "image/jpeg" | "image/png" | "image/webp"
        ) {
            return Err(ProviderError::Protocol {
                reason: None,
                detail: format!(
                    "Unsupported Claude image attachment type '{}'",
                    image.mime_type
                ),
                payload: None,
            });
        }
        let path = paths[index].as_ref().ok_or_else(|| {
            operation(
                ProviderOperation::Start,
                format!("Invalid attachment id '{}'", image.id),
            )
        })?;
        let bytes = std::fs::read(path).map_err(|_| ProviderError::Protocol {
            reason: Some(ProtocolReason::AttachmentUnreadable),
            detail: format!("Failed to read attachment '{}'", image.id),
            payload: None,
        })?;
        let retained = 4 * (bytes.len() as u64).div_ceil(3) + 128;
        if retained > CLAUDE_ATTACHMENT_MAX_BYTES.saturating_sub(usage.bytes_used) {
            continue;
        }
        usage.bytes_used += retained;
        images.insert(
            index,
            t3_claude_cli::control::image_block(&image.mime_type, &bytes).map_err(protocol)?,
        );
    }
    let mut blocks = Vec::new();
    for (index, attachment) in message.attachments.iter().enumerate() {
        if let Some(image) = images.remove(&index) {
            blocks.push(image);
        }
        let ChatAttachment::File(file) = attachment else {
            continue;
        };
        if !file
            .mime_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("application/pdf")
            || file.source.as_ref().and_then(|s| s.as_ref()).is_some()
            || usage.pages_used >= CLAUDE_ATTACHMENT_MAX_PAGES
        {
            continue;
        }
        let Some(path) = &paths[index] else { continue };
        let remaining = CLAUDE_ATTACHMENT_MAX_BYTES
            .saturating_sub(usage.bytes_used)
            .saturating_sub(1024)
            .saturating_mul(3)
            / 4;
        let limit = remaining.min(20 * 1024 * 1024);
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > limit || limit == 0 {
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(limit + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > limit {
            continue;
        }
        let Some(pages) = estimate_claude_pdf_pages(&bytes) else {
            continue;
        };
        let retained = 4 * (bytes.len() as u64).div_ceil(3) + 128;
        if pages > CLAUDE_ATTACHMENT_MAX_PAGES.saturating_sub(usage.pages_used)
            || retained > CLAUDE_ATTACHMENT_MAX_BYTES.saturating_sub(usage.bytes_used)
        {
            continue;
        }
        usage.pages_used += pages;
        usage.bytes_used += retained;
        blocks.push(t3_claude_cli::control::pdf_block(&bytes).map_err(protocol)?);
    }
    blocks.push(text_block);
    result.message.content = Content::Blocks(blocks);
    Ok(PreparedClaudeMessage {
        message: result,
        usage,
    })
}

pub fn estimate_claude_pdf_pages(bytes: &[u8]) -> Option<u64> {
    if bytes.len() > 20 * 1024 * 1024
        || !bytes.starts_with(b"%PDF-")
        || !bytes[bytes.len().saturating_sub(1024)..]
            .windows(5)
            .any(|w| w == b"%%EOF")
    {
        return None;
    }
    let delimiter = |b: u8| b <= 32 || b"()<>[]{}/%".contains(&b);
    let (mut pages, mut count, mut catalog, mut tree, mut previous, mut i) =
        (0, 0, false, false, &b""[..], 0);
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            while i < bytes.len() && !b"\r\n".contains(&bytes[i]) {
                i += 1;
            }
            continue;
        }
        if b <= 32 {
            i += 1;
            continue;
        }
        if previous == b"/Type" && b != b'/' {
            return None;
        }
        if b == b'(' {
            let mut depth = 1;
            i += 1;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'\\' => i += 1,
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth != 0 {
                return None;
            }
            previous = b"";
            continue;
        }
        if b == b'<' && bytes.get(i + 1) != Some(&b'<') {
            i += 1;
            while bytes.get(i).is_some_and(|b| *b != b'>') {
                i += 1;
            }
            if i == bytes.len() {
                return None;
            }
            i += 1;
            previous = b"";
            continue;
        }
        if b == b'<' && bytes.get(i + 1) == Some(&b'<') {
            i += 2;
            previous = b"";
            continue;
        }
        if delimiter(b) && b != b'/' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && !delimiter(bytes[i]) {
            i += 1;
        }
        let token = &bytes[start..i];
        if token.len() > 64
            || token.contains(&b'#')
            || matches!(token, b"/ObjStm" | b"/XRef" | b"/Encrypt" | b"/Prev")
        {
            return None;
        }
        if token == b"stream" {
            i += bytes[i..].windows(9).position(|w| w == b"endstream")? + 9;
            previous = b"";
            continue;
        }
        if previous == b"/Type" {
            match token {
                b"/Page" => pages += 1,
                b"/Catalog" => catalog = true,
                b"/Pages" => tree = true,
                _ => {}
            }
        }
        if previous == b"/Count" {
            if token.len() > 9
                || !token.first().is_some_and(|b| (b'1'..=b'9').contains(b))
                || !token.iter().all(u8::is_ascii_digit)
            {
                return None;
            }
            let mut next = i;
            while next < bytes.len() {
                if bytes[next] <= 32 {
                    next += 1;
                } else if bytes[next] == b'%' {
                    while next < bytes.len() && !b"\r\n".contains(&bytes[next]) {
                        next += 1;
                    }
                } else {
                    break;
                }
            }
            if next < bytes.len() && !delimiter(bytes[next]) {
                return None;
            }
            count = count.max(std::str::from_utf8(token).ok()?.parse::<u64>().ok()?);
        }
        previous = token;
    }
    (catalog && tree && pages > 0 && count == pages).then_some(pages)
}

pub fn fork_claude_session_file(
    root: &std::path::Path,
    source: &str,
    target: &str,
    cursor: Option<&str>,
    now: &str,
) -> ProviderResult<ClaudeForkTranscript> {
    use std::io::Write;
    if uuid::Uuid::parse_str(source).is_err()
        || uuid::Uuid::parse_str(target).is_err()
        || source == target
    {
        return Err(operation(
            ProviderOperation::Fork,
            "Native fork requires distinct UUID session ids",
        ));
    }
    let root = root.canonicalize().map_err(|_| {
        operation(
            ProviderOperation::Fork,
            "Claude transcript directory is unavailable",
        )
    })?;
    let filename = format!("{source}.jsonl");
    let mut candidates = Vec::new();
    if root.join(&filename).is_file() {
        candidates.push(root.join(&filename));
    }
    for entry in std::fs::read_dir(&root)
        .map_err(|_| operation(ProviderOperation::Fork, "Could not list Claude projects"))?
    {
        let entry = entry
            .map_err(|_| operation(ProviderOperation::Fork, "Could not list Claude projects"))?;
        if entry
            .file_type()
            .map_err(|_| operation(ProviderOperation::Fork, "Could not inspect Claude project"))?
            .is_dir()
            && entry.path().join(&filename).is_file()
        {
            candidates.push(entry.path().join(&filename));
        }
    }
    candidates.sort();
    let [path] = candidates.as_slice() else {
        return Err(operation(
            ProviderOperation::Fork,
            "Claude source session was absent or ambiguous",
        ));
    };
    let path = path.canonicalize().map_err(|_| {
        operation(
            ProviderOperation::Fork,
            "Could not locate Claude transcript",
        )
    })?;
    if !path.starts_with(&root) {
        return Err(operation(
            ProviderOperation::Fork,
            "Claude transcript leaves the injected root",
        ));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|_| operation(ProviderOperation::Fork, "Could not read Claude transcript"))?;
    let rows = content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let fork = fork_claude_records(&rows, source, target, cursor, now, || {
        uuid::Uuid::new_v4().to_string()
    })?;
    let directory = path
        .parent()
        .ok_or_else(|| operation(ProviderOperation::Fork, "Invalid Claude project directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|_| operation(ProviderOperation::Fork, "Could not create fork transcript"))?;
    for row in &fork.entries {
        serde_json::to_writer(&mut file, row)
            .map_err(|_| operation(ProviderOperation::Fork, "Could not write fork transcript"))?;
        file.write_all(b"\n")
            .map_err(|_| operation(ProviderOperation::Fork, "Could not write fork transcript"))?;
    }
    file.flush()
        .map_err(|_| operation(ProviderOperation::Fork, "Could not flush fork transcript"))?;
    file.persist_noclobber(directory.join(format!("{target}.jsonl")))
        .map_err(|_| operation(ProviderOperation::Fork, "Could not publish fork transcript"))?;
    Ok(fork)
}
type Subscribers = Vec<(
    mpsc::Sender<ProviderResult<ProviderEvent>>,
    CancellationToken,
)>;
type ConnectFuture<'a> =
    Pin<Box<dyn Future<Output = ProviderResult<ClaudeConnection>> + Send + 'a>>;

pub struct ClaudeConnection {
    pub session: Session,
    pub process: Option<t3_process::SpawnedProcess>,
}
pub trait ClaudeConnector: Send + Sync {
    fn connect<'a>(
        &'a self,
        options: &'a LaunchOptions,
        cancellation: CancellationToken,
    ) -> ConnectFuture<'a>;
}
pub struct ProcessConnector;
impl ClaudeConnector for ProcessConnector {
    fn connect<'a>(
        &'a self,
        options: &'a LaunchOptions,
        cancellation: CancellationToken,
    ) -> ConnectFuture<'a> {
        Box::pin(async move {
            let description = describe_spawn(options).map_err(protocol)?;
            let mut process_options =
                t3_process::ProcessOptions::new(description.command).args(description.args);
            process_options.cwd = description.cwd;
            process_options.inherit_env = false;
            process_options.env = description
                .env
                .into_iter()
                .map(|(key, value)| (key.into(), Some(value.into())))
                .collect();
            process_options.cancellation = cancellation;
            process_options.timeout = None;
            #[cfg(unix)]
            {
                // Enter a possibly stalled mount in the child; t3-process owns its process group.
                let cwd = process_options
                    .cwd
                    .take()
                    .unwrap_or_else(|| PathBuf::from("."));
                let cwd = if cwd.is_absolute() {
                    cwd
                } else {
                    PathBuf::from(".").join(cwd)
                };
                let program = std::mem::replace(&mut process_options.program, "/bin/sh".into());
                let arguments = std::mem::take(&mut process_options.args);
                process_options.args = vec![
                    "-c".into(),
                    "cd -P -- \"$1\" || exit 125\nshift\nexec \"$@\"".into(),
                    "t3-claude".into(),
                    cwd.into_os_string(),
                    program,
                ];
                process_options.args.extend(arguments);
            }
            let mut process =
                tokio::task::spawn_blocking(move || t3_process::spawn(&process_options))
                    .await
                    .map_err(|_| operation(ProviderOperation::Open, "Claude process spawn failed"))?
                    .map_err(|_| {
                        operation(ProviderOperation::Open, "Could not spawn Claude Code")
                    })?;
            let stdout = process
                .stdout
                .take()
                .ok_or_else(|| operation(ProviderOperation::Open, "Missing Claude stdout"))?;
            let stdin = process
                .stdin
                .take()
                .ok_or_else(|| operation(ProviderOperation::Open, "Missing Claude stdin"))?;
            Ok(ClaudeConnection {
                session: Session::connect(stdout, stdin, SessionOptions::default()),
                process: Some(process),
            })
        })
    }
}

pub struct ClaudeAdapterOptions {
    pub instance_id: ProviderInstanceId,
    pub launch: LaunchOptions,
    pub connector: Arc<dyn ClaudeConnector>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub new_native_session_id: Arc<dyn Fn() -> String + Send + Sync>,
    pub mcp_servers: BTreeMap<String, Arc<dyn McpServer>>,
    pub transcript_root: Option<PathBuf>,
    pub attachments: Arc<dyn ClaudeAttachments>,
    pub host: ClaudeHostOptions,
}

pub fn claude_registration(
    options: ClaudeAdapterOptions,
    continuation_key: String,
    enabled: bool,
    credential_access: Vec<
        Arc<crate::ports::sessions_recovery::provider_adapter_registry::CredentialAccess>,
    >,
) -> crate::ports::sessions_recovery::provider_adapter_registry::AdapterRegistration {
    crate::ports::sessions_recovery::provider_adapter_registry::AdapterRegistration {
        adapter: Arc::new(ClaudeAdapterV2::new(options)),
        continuation_key,
        enabled,
        credential_access,
    }
}

pub type ClaudeModelCompiler = dyn Fn(&t3_contracts::model_selection::ModelSelection) -> ProviderResult<ClaudeCompiledModel>
    + Send
    + Sync;
#[derive(Clone)]
pub struct ClaudeCompiledModel {
    pub model: String,
    pub effort: Option<String>,
    pub settings: Value,
}

#[async_trait]
pub trait ClaudeMcpServerFactory: Send + Sync {
    async fn create(
        &self,
        input: &OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<BTreeMap<String, Arc<dyn McpServer>>>;
}

#[derive(Default)]
pub struct ClaudeHostOptions {
    pub home_directory: Option<PathBuf>,
    pub settings_directory: Option<PathBuf>,
    pub managed_settings: Vec<Value>,
    pub runtime_instructions: String,
    pub attachment_directory: Option<PathBuf>,
    pub auto_compact_window: Option<String>,
    pub compile_model: Option<Arc<ClaudeModelCompiler>>,
    pub mcp_server_factory: Option<Arc<dyn ClaudeMcpServerFactory>>,
    pub continuation_requests: Option<mpsc::Sender<ContinuationRequest>>,
}
pub struct PreparedClaudeLaunch {
    pub options: LaunchOptions,
    pub policy: ClaudeRuntimeQueryPolicy,
    pub model: ClaudeCompiledModel,
    pub auto_compact_window: Option<u64>,
}

fn claude_local_settings_directory(cwd: &std::path::Path, _host: &ClaudeHostOptions) -> PathBuf {
    #[cfg(unix)]
    for directory in cwd.ancestors() {
        let git = directory.join(".git");
        if !git.exists() {
            continue;
        }
        let root = if git.is_file() {
            std::fs::read_to_string(&git)
                .ok()
                .and_then(|text| {
                    text.trim()
                        .strip_prefix("gitdir:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .map(|git_dir| directory.join(git_dir))
                .and_then(|git_dir| {
                    let common = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
                    let common = std::fs::canonicalize(git_dir.join(common.trim())).ok()?;
                    (common.file_name()? == ".git")
                        .then(|| common.parent().map(std::path::Path::to_owned))
                        .flatten()
                })
                .unwrap_or_else(|| directory.to_owned())
        } else {
            directory.to_owned()
        };
        return if _host.home_directory.as_deref() == Some(root.as_path()) {
            cwd.to_owned()
        } else {
            root
        };
    }
    cwd.to_owned()
}

fn claude_settings_files(cwd: Option<&std::path::Path>, host: &ClaudeHostOptions) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(cwd) = cwd {
        let local = claude_local_settings_directory(cwd, host);
        files.push(local.join(".claude/settings.local.json"));
        if local != cwd {
            files.push(cwd.join(".claude/settings.local.json"));
        }
        files.push(cwd.join(".claude/settings.json"));
    }
    if let Some(home) = &host.settings_directory {
        files.push(home.join("settings.json"));
    }
    files
}

fn claude_tool_search_override(value: &str) -> Option<&'static str> {
    if value == "auto" {
        return Some("true");
    }
    let percent = value.strip_prefix("auto:")?;
    let percent = percent.trim_matches(|c| {
        matches!(c,
        '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
    });
    let unsigned = percent.strip_prefix(['+', '-']).unwrap_or(percent);
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let scientific = unsigned
        .split_once(['e', 'E'])
        .is_some_and(|(base, exponent)| {
            let base_valid = base.split_once('.').map_or_else(
                || digits(base),
                |(whole, fraction)| {
                    (digits(whole) && (fraction.is_empty() || digits(fraction)))
                        || (whole.is_empty() && digits(fraction))
                },
            );
            base_valid && digits(exponent.strip_prefix(['+', '-']).unwrap_or(exponent))
        });
    let grouped = unsigned
        .find(['_', ',', '\u{00a0}', '\u{202f}', ' '])
        .and_then(|index| {
            let separator = unsigned[index..].chars().next()?;
            let groups = unsigned.split(separator).collect::<Vec<_>>();
            (groups[0].len() <= 3
                && digits(groups[0])
                && groups[1..]
                    .iter()
                    .all(|group| group.len() == 3 && digits(group)))
            .then(|| percent.replace(separator, ""))
        });
    let number = if percent.encode_utf16().count() <= 32 && scientific {
        percent
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && n.fract() == 0.0)
    } else if percent.encode_utf16().count() <= 32 && grouped.is_some() {
        grouped.and_then(|value| value.parse::<f64>().ok())
    } else {
        let length = unsigned.bytes().take_while(u8::is_ascii_digit).count();
        let sign = percent.len() - unsigned.len();
        (length > 0)
            .then(|| percent[..sign + length].parse::<f64>().ok())
            .flatten()
    };
    number
        .is_none_or(|n| n > 0.0 && n < 100.0)
        .then_some("true")
}

fn resolve_claude_settings(
    options: &LaunchOptions,
    host: &ClaudeHostOptions,
    launch: &Value,
    model: &str,
) -> (Option<u64>, Option<&'static str>) {
    let settings = claude_settings_files(options.cwd.as_deref(), host)
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|text| {
            (
                parse_claude_settings(&text).unwrap_or(Value::Null),
                serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
            )
        })
        .collect::<Vec<_>>();
    let environment = |name: &str| {
        launch["env"][name]
            .as_str()
            .or_else(|| {
                settings
                    .iter()
                    .find_map(|(lenient, _)| lenient["env"][name].as_str())
            })
            .or_else(|| options.env.get(name).map(String::as_str))
            .map(str::to_owned)
    };
    let tool_search = environment("ENABLE_TOOL_SEARCH")
        .as_deref()
        .and_then(claude_tool_search_override);
    for name in ["DISABLE_COMPACT", "DISABLE_AUTO_COMPACT"] {
        if environment(name).is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }) {
            return (None, tool_search);
        }
    }
    let enabled = launch["autoCompactEnabled"].as_bool().or_else(|| {
        settings
            .iter()
            .find_map(|(_, strict)| strict["autoCompactEnabled"].as_bool())
    });
    if enabled == Some(false) {
        return (None, tool_search);
    }
    if let Some(window) = host
        .auto_compact_window
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        return (
            window.parse::<u64>().ok().filter(|value| *value > 0),
            tool_search,
        );
    }
    let valid = |value: &Value| {
        value
            .as_f64()
            .filter(|value| value.fract() == 0.0 && (100_000.0..=1_000_000.0).contains(value))
            .map(|value| value as u64)
    };
    if let Some(window) = environment("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
        .and_then(|value| value.trim().parse::<f64>().ok())
        .and_then(|value| valid(&json!(value)))
    {
        return (Some(window), tool_search);
    }
    let model = model
        .strip_suffix("[1m]")
        .or_else(|| model.strip_suffix("[1M]"))
        .unwrap_or(model);
    let window_from = |settings: &Value| {
        let scoped = &settings["modelSettings"][model]["autoCompactWindow"];
        if scoped == "auto" {
            return Some(None);
        }
        valid(scoped)
            .or_else(|| valid(&settings["autoCompactWindow"]))
            .map(Some)
    };
    (
        window_from(launch)
            .or_else(|| settings.iter().find_map(|(_, strict)| window_from(strict)))
            .flatten(),
        tool_search,
    )
}

fn parse_claude_settings(text: &str) -> ProviderResult<Value> {
    let mut bytes = text.trim_start_matches('\u{feff}').as_bytes().to_vec();
    let (mut index, mut string, mut escape) = (0, false, false);
    while index < bytes.len() {
        let byte = bytes[index];
        if string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                string = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && !b"\r\n".contains(&bytes[index]) {
                bytes[index] = b' ';
                index += 1;
            }
            continue;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    bytes[index] = b' ';
                    bytes[index + 1] = b' ';
                    index += 2;
                    closed = true;
                    break;
                }
                bytes[index] = b' ';
                index += 1;
            }
            if !closed {
                return Err(operation(
                    ProviderOperation::Open,
                    "Invalid Claude settings JSON",
                ));
            }
            continue;
        }
        index += 1;
    }
    string = false;
    escape = false;
    for index in 0..bytes.len() {
        let byte = bytes[index];
        if string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                string = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b','
            && bytes[index + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| b"}]".contains(b))
        {
            bytes[index] = b' ';
        }
    }
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| operation(ProviderOperation::Open, "Invalid Claude settings JSON"))
}
fn claude_flag_path(
    text: &str,
    cwd: Option<&std::path::Path>,
    home: Option<&std::path::Path>,
) -> ProviderResult<PathBuf> {
    if text.is_empty() {
        return Err(operation(
            ProviderOperation::Open,
            "Empty Claude launch file path",
        ));
    }
    let path = if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
        home.ok_or_else(|| {
            operation(
                ProviderOperation::Open,
                "Claude launch file needs a host home directory",
            )
        })?
        .join(text.get(2..).unwrap_or(""))
    } else {
        PathBuf::from(text)
    };
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.unwrap_or(std::path::Path::new(".")).join(path)
    })
}
fn read_claude_settings(path: &std::path::Path, optional: bool) -> ProviderResult<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_claude_settings(&text),
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(_) => Err(operation(
            ProviderOperation::Open,
            "Could not read Claude settings file",
        )),
    }
}
fn settings_availability(settings: &Value) -> BypassAvailability {
    BypassAvailability {
        bypass_disabled: settings["permissions"]["disableBypassPermissionsMode"] == "disable",
        auto_disabled: settings["disableAutoMode"] == "disable"
            || settings["permissions"]["disableAutoMode"] == "disable",
    }
}
fn direct_claude_model(
    selection: &t3_contracts::model_selection::ModelSelection,
) -> ClaudeCompiledModel {
    use t3_contracts::model::ProviderOptionSelectionValue;
    let mut compiled = ClaudeCompiledModel {
        model: selection.model.clone(),
        effort: None,
        settings: json!({}),
    };
    for option in selection.options.iter().flat_map(|v| &v.0) {
        match (option.id.as_str(), &option.value) {
            ("effort", ProviderOptionSelectionValue::String(value))
                if matches!(value.trim(), "low" | "medium" | "high" | "xhigh" | "max") =>
            {
                compiled.effort = Some(value.trim().into())
            }
            ("thinking", ProviderOptionSelectionValue::Boolean(value)) => {
                compiled.settings["alwaysThinkingEnabled"] = json!(value)
            }
            ("fastMode" | "ultracode", ProviderOptionSelectionValue::Boolean(value)) => {
                compiled.settings[&option.id] = json!(value)
            }
            _ => {}
        }
    }
    compiled
}

pub fn prepare_claude_launch(
    base: &LaunchOptions,
    input: &TurnInput,
    policy_options: &RuntimePolicyOptions,
    host: &ClaudeHostOptions,
) -> ProviderResult<PreparedClaudeLaunch> {
    let settings = prepare_claude_launch_settings(base, input, policy_options, host)?;
    let append = read_claude_append_files(base, input.runtime_policy.cwd.as_deref(), host)?;
    Ok(finish_claude_launch(settings, append))
}

struct PreparedClaudeSettings {
    launch: PreparedClaudeLaunch,
    append_before_files: Vec<String>,
    append_after_files: Vec<String>,
}

fn finish_claude_launch(
    mut settings: PreparedClaudeSettings,
    files: Vec<String>,
) -> PreparedClaudeLaunch {
    if !matches!(
        settings.launch.options.system_prompt,
        SystemPrompt::Custom(_)
    ) {
        settings.append_before_files.extend(files);
        settings
            .append_before_files
            .extend(settings.append_after_files);
        settings.launch.options.system_prompt = SystemPrompt::ClaudeCode {
            append: (!settings.append_before_files.is_empty())
                .then(|| settings.append_before_files.join("\n\n")),
        };
    }
    settings.launch
}

fn read_claude_append_files(
    base: &LaunchOptions,
    cwd: Option<&std::path::Path>,
    host: &ClaudeHostOptions,
) -> ProviderResult<Vec<String>> {
    let mut texts = Vec::new();
    for (_, file) in base
        .extra_args
        .iter()
        .filter(|(key, _)| key == "append-system-prompt-file")
    {
        let file = file.as_deref().ok_or_else(|| {
            operation(ProviderOperation::Open, "Missing Claude append prompt file")
        })?;
        let path = claude_flag_path(file, cwd, host.home_directory.as_deref())?;
        let mut text = std::fs::read_to_string(path).map_err(|_| {
            operation(
                ProviderOperation::Open,
                "Could not read Claude append prompt file",
            )
        })?;
        if text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        if !text.is_empty() {
            texts.push(text);
        }
    }
    Ok(texts)
}

fn prepare_claude_launch_settings(
    base: &LaunchOptions,
    input: &TurnInput,
    policy_options: &RuntimePolicyOptions,
    host: &ClaudeHostOptions,
) -> ProviderResult<PreparedClaudeSettings> {
    use t3_claude_cli::launch::{Settings, Thinking};
    let cwd = input.runtime_policy.cwd.as_deref();
    let mut options = base.clone();
    options.cwd = input.runtime_policy.cwd.clone();
    let mut inline = Vec::new();
    let mut settings = options.settings.take();
    options.extra_args.retain(|(name, value)| {
        match name.as_str() {
            "append-system-prompt" => inline.push(value.clone()),
            "append-system-prompt-file" => {}
            "settings" => {
                settings = value
                    .clone()
                    .map(Settings::Path)
                    .or(Some(Settings::Path(String::new())))
            }
            "add-dir" => {
                if let Some(value) = value {
                    options.additional_directories.push(value.clone());
                }
            }
            "permission-mode" | "dangerously-skip-permissions" => {}
            _ => return true,
        }
        false
    });
    let mut launch_settings = match settings {
        Some(Settings::Json(settings)) => settings,
        Some(Settings::Path(text)) if text.trim_start().starts_with('{') => {
            parse_claude_settings(&text)?
        }
        Some(Settings::Path(text)) => read_claude_settings(
            &claude_flag_path(&text, cwd, host.home_directory.as_deref())?,
            false,
        )?,
        None => json!({}),
    };
    let mut availability = BypassAvailability::default();
    let mut sources = host.managed_settings.clone();
    if let Some(managed) = &base.managed_settings {
        sources.push(managed.clone());
    }
    if let Some(directory) = &host.settings_directory {
        for name in ["settings.json", "remote-settings.json"] {
            match read_claude_settings(&directory.join(name), true) {
                Ok(value) => sources.push(value),
                Err(_) => availability.bypass_disabled = true,
            }
        }
    }
    if let Some(cwd) = cwd {
        for name in ["settings.json", "settings.local.json"] {
            match read_claude_settings(&cwd.join(".claude").join(name), true) {
                Ok(value) => sources.push(value),
                Err(_) => availability.bypass_disabled = true,
            }
        }
    }
    sources.push(launch_settings.clone());
    for source in sources {
        let current = settings_availability(&source);
        availability.bypass_disabled |= current.bypass_disabled;
        availability.auto_disabled |= current.auto_disabled;
    }
    // SDK rejects false here; only an explicit disable survives its schema.
    for path in [
        "/disableAutoMode",
        "/permissions/disableAutoMode",
        "/permissions/disableBypassPermissionsMode",
    ] {
        if launch_settings.pointer(path) == Some(&Value::Bool(false)) {
            let (parent, key) = path.rsplit_once('/').unwrap_or(("", path));
            if let Some(object) = launch_settings
                .pointer_mut(parent)
                .and_then(Value::as_object_mut)
            {
                object.remove(key);
            }
        }
    }
    let mut model = if let Some(compile) = &host.compile_model {
        compile(&input.model_selection)?
    } else {
        direct_claude_model(&input.model_selection)
    };
    if policy_options.reasoning_effort.is_some() {
        model.effort = policy_options.reasoning_effort.clone();
    }
    let object = launch_settings
        .as_object_mut()
        .ok_or_else(|| operation(ProviderOperation::Open, "Invalid Claude settings object"))?;
    if let Some(settings) = model.settings.as_object() {
        object.extend(settings.clone());
    }
    let summaries = model.settings["alwaysThinkingEnabled"] != false
        && !options
            .extra_args
            .iter()
            .any(|(key, value)| key == "thinking-display" && value.as_deref() == Some("omitted"));
    if summaries {
        if !options
            .extra_args
            .iter()
            .any(|(key, _)| key == "thinking-display")
        {
            options
                .extra_args
                .push(("thinking-display".into(), Some("summarized".into())));
        }
        options.thinking = Some(Thinking::Adaptive {
            display: Some("summarized".into()),
        });
        launch_settings["showThinkingSummaries"] = json!(true);
    } else if model.settings["alwaysThinkingEnabled"] == false {
        options.thinking = Some(Thinking::Disabled);
    }
    if let Some(window) = host.auto_compact_window.as_ref().filter(|s| !s.is_empty()) {
        let value = window
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                operation(
                    ProviderOperation::Open,
                    "Invalid Claude auto compact window",
                )
            })?;
        launch_settings["autoCompactWindow"] = json!(value);
        if !launch_settings["env"].is_object() {
            launch_settings["env"] = json!({});
        }
        launch_settings["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"] = json!(window);
    }
    if matches!(model.model.as_str(), "opus" | "sonnet" | "haiku" | "fable") {
        if !launch_settings["env"].is_object() {
            launch_settings["env"] = json!({});
        }
        launch_settings["env"][format!("ANTHROPIC_DEFAULT_{}_MODEL", model.model.to_uppercase())] =
            json!(model.model);
    }
    let mut append = Vec::new();
    if !host.runtime_instructions.is_empty() {
        append.push(host.runtime_instructions.clone());
    }
    if let SystemPrompt::ClaudeCode { append: Some(text) } = &base.system_prompt {
        append.push(text.clone());
    }
    if let Some(cwd) = cwd {
        options
            .additional_directories
            .push(cwd.to_string_lossy().into_owned());
    }
    if let Some(directory) = &host.attachment_directory {
        options
            .additional_directories
            .push(directory.to_string_lossy().into_owned());
    }
    options.additional_directories.dedup();
    options.model = Some(model.model.clone());
    options.effort = model.effort.clone().or(options.effort);
    options.settings = Some(Settings::Json(launch_settings));
    let policy = runtime_query_policy(&input.runtime_policy, availability);
    options.permission_mode = Some(policy.permission_mode);
    options.allow_dangerously_skip_permissions = policy.allow_dangerously_skip_permissions;
    let (auto_compact_window, tool_search) = resolve_claude_settings(
        &options,
        host,
        match &options.settings {
            Some(Settings::Json(value)) => value,
            _ => &Value::Null,
        },
        &model.model,
    );
    if let (Some(value), Some(Settings::Json(settings))) = (tool_search, &mut options.settings) {
        if !settings["env"].is_object() {
            settings["env"] = json!({});
        }
        settings["env"]["ENABLE_TOOL_SEARCH"] = json!(value);
    }
    Ok(PreparedClaudeSettings {
        launch: PreparedClaudeLaunch {
            options,
            policy,
            model,
            auto_compact_window,
        },
        append_before_files: append,
        append_after_files: inline
            .into_iter()
            .flatten()
            .filter(|text| !text.is_empty())
            .collect(),
    })
}
pub struct ClaudeAdapterV2 {
    options: Arc<ClaudeAdapterOptions>,
}
impl ClaudeAdapterV2 {
    pub fn new(options: ClaudeAdapterOptions) -> Self {
        Self {
            options: Arc::new(options),
        }
    }
}

#[async_trait]
impl ProviderAdapter for ClaudeAdapterV2 {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.options.instance_id
    }
    fn driver(&self) -> ProviderDriverKind {
        ProviderDriverKind("claudeAgent".into())
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        claude_capabilities()
    }
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        Ok(if input.current.instance_id != input.target.instance_id {
            SelectionTransitionPlan::CreateWithHandoff
        } else {
            SelectionTransitionPlan::ApplyOnNextTurn
        })
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.open_session_with_policy(input, RuntimePolicyOptions::default(), cancellation)
            .await
    }
    async fn open_session_with_policy(
        &self,
        input: OpenSession,
        policy_options: RuntimePolicyOptions,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        if cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let session_cancellation = cancellation.child_token();
        let cancellation_guard = session_cancellation.clone().drop_guard();
        let mut mcp_servers = self.options.mcp_servers.clone();
        if let Some(factory) = &self.options.host.mcp_server_factory {
            let servers = tokio::select! {
                biased;
                _ = session_cancellation.cancelled() => return Err(ProviderError::Cancelled),
                result = factory.create(&input, session_cancellation.child_token()) => result?,
            };
            for (name, server) in servers {
                if mcp_servers.insert(name, server).is_some() {
                    return Err(operation(
                        ProviderOperation::Open,
                        "Duplicate Claude MCP server name",
                    ));
                }
            }
        }
        let now = self.options.clock.now();
        let session = decode(
            json!({"id":input.provider_session_id,"driver":"claudeAgent","providerInstanceId":self.options.instance_id,
            "status":"ready","cwd":input.runtime_policy.cwd.as_deref().unwrap_or(std::path::Path::new(".")).to_string_lossy(),
            "model":input.model_selection.model,"capabilities":claude_capabilities()?,"createdAt":now,"updatedAt":now,"lastError":null}),
        )?;
        let runtime = Arc::new(ClaudeSessionRuntime {
            session,
            inner: Arc::new(RuntimeInner {
                projection: Mutex::new(ClaudeProjection::new(
                    self.options.instance_id.clone(),
                    input.provider_session_id.clone(),
                    self.options.clock.clone(),
                    self.options.ids.clone(),
                )),
                subscribers: Mutex::new(Vec::new()),
                pending: Mutex::new(BTreeMap::new()),
                permission_policy: Mutex::new(None),
                attachments: Mutex::new(AttachmentBudget::default()),
                wake: Mutex::new(NativeWake::default()),
                continuation_requests: self.options.host.continuation_requests.clone(),
                clock: self.options.clock.clone(),
                cancellation: session_cancellation,
            }),
            options: self.options.clone(),
            mcp_servers,
            open: input,
            query: AsyncMutex::new(None),
            operations: AsyncMutex::new(()),
            policy_options: Mutex::new(policy_options),
        });
        let _ = cancellation_guard.disarm();
        Ok(runtime)
    }
}

enum Callback {
    Permission {
        request: Box<t3_claude_cli::control::ToolPermission>,
        responder: t3_claude_cli::Responder<PermissionDecision>,
        managed: bool,
    },
    Elicitation(t3_claude_cli::Responder<ElicitationResponse>),
    Dialog(t3_claude_cli::Responder<DialogResponse>),
}
impl Callback {
    fn native_id(&self) -> &str {
        match self {
            Self::Permission { responder, .. } => responder.request_id(),
            Self::Elicitation(r) => r.request_id(),
            Self::Dialog(r) => r.request_id(),
        }
    }
    fn cancellation(&self) -> CancellationToken {
        match self {
            Self::Permission { responder, .. } => responder.cancellation(),
            Self::Elicitation(r) => r.cancellation(),
            Self::Dialog(r) => r.cancellation(),
        }
    }
}
struct RuntimeInner {
    projection: Mutex<ClaudeProjection>,
    subscribers: Mutex<Subscribers>,
    pending: Mutex<BTreeMap<String, Callback>>,
    permission_policy: Mutex<
        Option<(
            ClaudeRuntimeQueryPolicy,
            t3_claude_cli::control::PermissionMode,
        )>,
    >,
    attachments: Mutex<AttachmentBudget>,
    wake: Mutex<NativeWake>,
    continuation_requests: Option<mpsc::Sender<ContinuationRequest>>,
    clock: Arc<dyn Clock>,
    cancellation: CancellationToken,
}
#[derive(Default)]
struct NativeWake {
    generation: u64,
    offered: bool,
    retired: bool,
    opened: bool,
    ended: bool,
    frames: Vec<Value>,
    notification: Option<OrchestrationV2Notification>,
    detail: Option<String>,
    ambiguous_user_turn: bool,
    candidate_frames: Vec<Value>,
    monitor_event: bool,
    own_end: Option<usize>,
    absorbed_ends: BTreeSet<usize>,
    next_notification: Option<OrchestrationV2Notification>,
    next_detail: Option<String>,
    scheduled_calls: BTreeSet<String>,
    scheduled_at: Option<f64>,
    early_scheduled_wake: bool,
}
impl NativeWake {
    fn clear_schedule(&mut self) {
        self.scheduled_calls.clear();
        self.scheduled_at = None;
        self.early_scheduled_wake = false;
    }
    fn root_start(frame: &Value) -> bool {
        frame["type"] == "system" && frame["subtype"] == "init"
    }
    fn root_frame(frame: &Value) -> bool {
        Self::root_start(frame)
            || frame["parent_tool_use_id"].is_null()
                && matches!(
                    frame["type"].as_str(),
                    Some("assistant" | "user" | "stream_event")
                )
    }
    fn observe_schedule(&mut self, frame: &Value, now: i64, idle: bool) -> bool {
        if Self::root_start(frame) {
            if let Some(scheduled) = self.scheduled_at {
                let possible = now as f64 >= scheduled - 120_000.0;
                if now as f64 >= scheduled {
                    self.scheduled_at = None;
                } else if idle && possible {
                    self.early_scheduled_wake = true;
                }
                return possible;
            }
        } else if frame["type"] == "result" {
            if std::mem::take(&mut self.early_scheduled_wake)
                && frame["origin"]["kind"] != "task-notification"
            {
                self.scheduled_at = None;
            }
        } else if frame["type"] == "assistant" {
            for block in frame["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use"
                    && block["name"] == "ScheduleWakeup"
                    && let Some(id) = block["id"].as_str()
                {
                    self.scheduled_calls.insert(id.into());
                }
            }
        } else if frame["type"] == "user" {
            let returned = frame["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| {
                    (block["type"] == "tool_result")
                        .then(|| block["tool_use_id"].as_str())
                        .flatten()
                })
                .filter(|id| self.scheduled_calls.remove(*id))
                .count();
            if returned > 0 {
                self.early_scheduled_wake = false;
                self.scheduled_at = if returned == 1 {
                    frame["tool_use_result"]["scheduledFor"]
                        .as_f64()
                        .filter(|value| value.is_finite() && *value > 0.0)
                } else {
                    None
                };
            }
        }
        false
    }
    fn is_monitor_event(&self) -> bool {
        self.frames
            .iter()
            .find(|frame| Self::root_start(frame))
            .is_some_and(|frame| frame["_t3_monitor_event_start"] == true)
            && self
                .frames
                .iter()
                .find(|frame| frame["type"] == "result")
                .is_none_or(|frame| frame["origin"]["kind"] == "task-notification")
    }
    fn absorb_open_tail(&mut self) {
        if self.offered
            && !self.frames.is_empty()
            && self
                .frames
                .last()
                .is_some_and(|frame| frame["type"] != "result")
        {
            let end = self.frames.len();
            if self.absorbed_ends.insert(end) && self.monitor_event && self.own_end.is_none() {
                self.own_end = Some(end);
            }
        }
    }
}
struct WakeCallbacks {
    inner: std::sync::Weak<RuntimeInner>,
    generation: u64,
}
#[async_trait]
impl ContinuationCallbacks for WakeCallbacks {
    async fn clear_if_current(&self) -> crate::error::Result<()> {
        if let Some(inner) = self.inner.upgrade() {
            let mut wake = lock(&inner.wake);
            if wake.generation == self.generation && wake.offered {
                wake.frames.clear();
                wake.absorbed_ends.clear();
                wake.monitor_event = false;
                wake.own_end = None;
                wake.next_notification = None;
                wake.next_detail = None;
                wake.notification = None;
                wake.detail = None;
                wake.retired = true;
                if wake.ended {
                    wake.offered = false;
                    wake.retired = false;
                }
            }
        }
        Ok(())
    }
    fn watch_unstarted_run(&self) -> bool {
        true
    }
    async fn on_run_ended_without_starting(&self) -> crate::error::Result<()> {
        self.clear_if_current().await
    }
    async fn dispatch_if_current(
        &self,
        dispatcher: &dyn crate::api::CommandDispatcher,
        command: OrchestrationV2ServerCommand,
    ) -> crate::error::Result<Option<crate::stores::receipts::CommandReceipt>> {
        let Some(inner) = self.inner.upgrade() else {
            return Ok(None);
        };
        {
            let wake = lock(&inner.wake);
            if wake.generation != self.generation || !wake.offered || wake.retired {
                return Ok(None);
            }
        }
        dispatcher.dispatch(command).await.map(Some)
    }
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
impl RuntimeInner {
    async fn buffer_native_wake(self: &Arc<Self>, frame: &Value) -> ProviderResult<bool> {
        let (idle, monitor) = {
            let projection = lock(&self.projection);
            (
                projection.active_turn_id().is_none(),
                projection.has_running_monitor(),
            )
        };
        let now = if NativeWake::root_start(frame) {
            crate::ports::decisions::orchestrator::epoch_millis(&self.clock.now().0).ok_or_else(
                || operation(ProviderOperation::Stream, "Invalid Claude wake timestamp"),
            )?
        } else {
            0
        };
        let possible_scheduled = lock(&self.wake).observe_schedule(frame, now, idle);
        let mut frame = frame.clone();
        if NativeWake::root_start(&frame) {
            frame["_t3_monitor_event_start"] = json!(idle && monitor && !possible_scheduled);
        }
        let frame = &frame;
        if frame["type"] == "result"
            && frame["origin"]["kind"] == "task-notification"
            && frame["num_turns"] == 0
            && frame["result"] == ""
            && frame["subtype"] == "success"
            && frame["is_error"] != true
        {
            return Ok(true);
        }
        let root_frame = NativeWake::root_frame(frame)
            || frame["parent_tool_use_id"].is_null() && frame["type"] == "result";
        if !root_frame {
            return Ok(false);
        }
        if !idle {
            let active_message = lock(&self.projection)
                .active_input()
                .map(|input| input.message.message_id.0.clone());
            let mut flush = Vec::new();
            let mut wake_group = false;
            let mut held = false;
            {
                let mut wake = lock(&self.wake);
                if wake.ambiguous_user_turn {
                    let accepted = frame["type"] == "user"
                        && frame["uuid"].as_str() == active_message.as_deref();
                    if accepted && wake.candidate_frames.is_empty() {
                        wake.ambiguous_user_turn = false;
                    } else if frame["type"] == "result" {
                        let echoed = frame["user_message_uuid"].as_str();
                        let foreign = echoed.is_some() && echoed != active_message.as_deref();
                        if frame["origin"]["kind"] == "task-notification" || foreign {
                            let candidates = std::mem::take(&mut wake.candidate_frames);
                            wake.frames.extend(candidates);
                            wake.frames.push(frame.clone());
                            wake.ended = true;
                            wake_group = true;
                            held = true;
                        } else {
                            flush = std::mem::take(&mut wake.candidate_frames);
                            wake.ambiguous_user_turn = false;
                            if wake
                                .own_end
                                .is_some_and(|end| wake.absorbed_ends.contains(&end))
                            {
                                wake.ended = true;
                            }
                        }
                    } else if matches!(frame["type"].as_str(), Some("assistant" | "stream_event")) {
                        if wake.candidate_frames.len() >= 4096 {
                            return Err(operation(
                                ProviderOperation::Stream,
                                "Claude ambiguous prompt buffer exceeded its frame limit",
                            ));
                        }
                        wake.candidate_frames.push(frame.clone());
                        held = true;
                    }
                }
            }
            if !flush.is_empty() {
                let mut events = Vec::new();
                {
                    let mut projection = lock(&self.projection);
                    for frame in flush {
                        events.extend(projection.handle_frame(&frame)?);
                    }
                }
                self.emit(events).await?;
            }
            if wake_group {
                self.offer_wake(None).await?;
            }
            if held {
                return Ok(true);
            }
        }
        let should_buffer =
            lock(&self.projection).idle_native_wake_frame(frame) || idle && possible_scheduled;
        let (buffered, retired_ended) = {
            let mut wake = lock(&self.wake);
            if wake.retired && wake.offered {
                if frame["type"] == "result" {
                    wake.offered = false;
                    wake.retired = false;
                    wake.opened = false;
                    wake.ended = false;
                    (true, true)
                } else {
                    (true, false)
                }
            } else if idle && (should_buffer || wake.offered) {
                if wake.frames.len() >= 4096 {
                    return Err(operation(
                        ProviderOperation::Stream,
                        "Claude native wake buffer exceeded its frame limit",
                    ));
                }
                wake.frames.push(frame.clone());
                wake.opened |= NativeWake::root_frame(frame);
                wake.ended |= frame["type"] == "result";
                if wake.monitor_event && wake.own_end.is_none() && frame["type"] == "result" {
                    wake.own_end = Some(wake.frames.len());
                }
                (true, false)
            } else {
                (false, false)
            }
        };
        if retired_ended {
            let events = lock(&self.projection).consume_native_wake()?;
            self.emit(events).await?;
        } else if buffered {
            self.offer_wake(None).await?;
        }
        Ok(buffered)
    }
    async fn offer_wake(
        self: &Arc<Self>,
        notification: Option<OrchestrationV2Notification>,
    ) -> ProviderResult<()> {
        let Some(sender) = &self.continuation_requests else {
            return Ok(());
        };
        let (generation, notification, detail) = {
            let mut wake = lock(&self.wake);
            if wake.offered {
                if notification.is_some() && wake.monitor_event {
                    wake.next_notification = notification;
                    wake.next_detail = wake.detail.take();
                }
                return Ok(());
            }
            if let Some(notification) = notification {
                wake.notification = Some(notification);
            }
            wake.monitor_event = wake.notification.is_none() && wake.is_monitor_event();
            if wake.monitor_event {
                wake.own_end = wake
                    .frames
                    .iter()
                    .position(|frame| frame["type"] == "result")
                    .map(|index| index + 1)
                    .or_else(|| wake.absorbed_ends.first().copied());
                wake.notification = Some(decode(
                    json!({"source":{"kind":"monitor"},"outcome":"updated","summary":"Monitor reported new output"}),
                )?);
            }
            let child_notification = wake
                .notification
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| operation(ProviderOperation::Stream, "Invalid wake notification"))?
                .is_some_and(|notification| {
                    notification["source"]["kind"] == "subagent"
                        || notification["source"]["kind"] == "background_task"
                            && notification["source"]["work"] == "subagent"
                });
            if !wake.opened && !wake.ended && !child_notification {
                return Ok(());
            }
            wake.generation += 1;
            wake.offered = true;
            wake.retired = false;
            (
                wake.generation,
                wake.notification.clone(),
                wake.detail.clone(),
            )
        };
        let request = {
            let projection = lock(&self.projection);
            let thread = projection
                .thread
                .as_ref()
                .ok_or(ProviderError::SessionStopped)?;
            ContinuationRequest {
                thread_id: serde_json::from_value(
                    serde_json::to_value(&thread.app_thread_id).map_err(|_| {
                        operation(ProviderOperation::Stream, "Invalid continuation thread")
                    })?,
                )
                .map_err(|_| {
                    operation(
                        ProviderOperation::Stream,
                        "Continuation requires expanded thread ids",
                    )
                })?,
                provider_thread_id: serde_json::from_value(
                    serde_json::to_value(&thread.id).map_err(|_| {
                        operation(ProviderOperation::Stream, "Invalid continuation thread")
                    })?,
                )
                .map_err(|_| {
                    operation(
                        ProviderOperation::Stream,
                        "Continuation requires expanded thread ids",
                    )
                })?,
                driver: ProviderDriverKind("claudeAgent".into()),
                detail,
                notification,
                delegated_completion: None,
                delivery: ContinuationDelivery::AdapterBuffered,
                message_id: None,
                callbacks: Some(Arc::new(WakeCallbacks {
                    inner: Arc::downgrade(self),
                    generation,
                })),
            }
        };
        tokio::select! {_=self.cancellation.cancelled()=>Err(ProviderError::SessionStopped),result=sender.send(request)=>result.map_err(|_|operation(ProviderOperation::Stream,"Claude continuation queue closed"))}
    }
    async fn persist_attachment_usage(&self) -> ProviderResult<()> {
        let usage = lock(&self.attachments).committed;
        let event = {
            let mut projection = lock(&self.projection);
            let Some(thread) = projection.thread.as_mut() else {
                return Ok(());
            };
            let mut metadata =
                serde_json::to_value(thread.native_metadata.as_ref().and_then(|v| v.as_ref()))
                    .map_err(|_| {
                        operation(ProviderOperation::Stream, "Invalid Claude native metadata")
                    })?;
            if !metadata.is_object() {
                metadata = json!({});
            }
            metadata["claudeAttachmentBudget"] = serde_json::to_value(usage).map_err(|_| {
                operation(
                    ProviderOperation::Stream,
                    "Invalid Claude attachment budget",
                )
            })?;
            thread.native_metadata =
                Some(Some(serde_json::from_value(metadata).map_err(|_| {
                    operation(
                        ProviderOperation::Stream,
                        "Invalid Claude attachment budget",
                    )
                })?));
            ProviderEvent {
                driver: ProviderDriverKind("claudeAgent".into()),
                kind: ProviderEventKind::Thread {
                    provider_thread: Box::new(thread.clone()),
                    pending_background_work: None,
                    session_scoped: None,
                    retained_native_turn_ids: None,
                },
            }
        };
        self.emit(vec![event]).await
    }
    async fn emit(&self, events: Vec<ProviderEvent>) -> ProviderResult<()> {
        for event in events {
            let subscribers = {
                let mut subscribers = lock(&self.subscribers);
                subscribers.retain(|(sender, token)| !sender.is_closed() && !token.is_cancelled());
                subscribers.clone()
            };
            for (sender, token) in subscribers {
                tokio::select! { biased;
                    _ = self.cancellation.cancelled() => return Err(ProviderError::SessionStopped),
                    _ = token.cancelled() => {},
                    _ = sender.send(Ok(event.clone())) => {},
                }
            }
        }
        Ok(())
    }
    async fn error(&self, error: ProviderError) {
        let subscribers = lock(&self.subscribers).clone();
        let detail = error.to_string();
        for (sender, token) in subscribers {
            tokio::select! { _ = self.cancellation.cancelled() => {}, _ = token.cancelled() => {},
            _ = sender.send(Err(operation(ProviderOperation::Stream,&detail))) => {} }
        }
    }
    async fn callback(&self, request: PendingRequest) -> ProviderResult<()> {
        let (callback, native, name, input, kind, prompt) = match request {
            PendingRequest::ToolPermission { request, responder } => {
                let request = request.get().clone();
                if !lock(&self.projection)
                    .has_permission_owner(request.agent_id.as_deref(), &request.tool_use_id)
                {
                    return responder
                        .respond(PermissionDecision::Deny {
                            message: "No active Claude turn for this request".into(),
                            interrupt: None,
                            decision_classification: None,
                        })
                        .await
                        .map_err(protocol);
                }
                let automatic = {
                    let projection = lock(&self.projection);
                    let policy = lock(&self.permission_policy);
                    projection
                        .permission_runtime_policy(
                            request.agent_id.as_deref(),
                            &request.tool_use_id,
                        )
                        .zip(policy.as_ref())
                        .and_then(|(runtime, (query, active_mode))| {
                            automatic_permission_result(&request, runtime, query, *active_mode)
                        })
                };
                if let Some(decision) = automatic {
                    return responder.respond(decision).await.map_err(protocol);
                }
                let kind = if request.tool_name == "AskUserQuestion" {
                    "user_input"
                } else {
                    tool_classification(&request.tool_name).1
                };
                let prompt = request
                    .input
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned)
                    .or(request.title.clone())
                    .or(request.description.clone())
                    .or(request.decision_reason.clone())
                    .unwrap_or_else(|| request.tool_name.clone());
                let input = Value::Object(request.input.clone());
                let name = request.tool_name.clone();
                let native = request.tool_use_id.clone();
                let managed = lock(&self.projection)
                    .permission_runtime_policy(request.agent_id.as_deref(), &request.tool_use_id)
                    .is_some_and(|p| p.max_runtime_mode.is_some());
                (
                    Callback::Permission {
                        request: Box::new(request),
                        responder,
                        managed,
                    },
                    native,
                    name,
                    input,
                    kind,
                    prompt,
                )
            }
            PendingRequest::Elicitation { request, responder } => {
                let request = request.get();
                if request.mode.as_deref() == Some("url")
                    || !approval_only_schema(request.requested_schema.as_ref())
                {
                    return responder
                        .respond(ElicitationResponse::Decline)
                        .await
                        .map_err(protocol);
                }
                (
                    Callback::Elicitation(responder),
                    request
                        .elicitation_id
                        .clone()
                        .unwrap_or_else(|| request.mcp_server_name.clone()),
                    request.mcp_server_name.clone(),
                    Value::Null,
                    "mcp-elicitation",
                    request.message.clone(),
                )
            }
            PendingRequest::UserDialog { request, responder } => {
                let request = request.get();
                let native = responder.request_id().to_owned();
                let input = json!({"questions":[{"question":"How should Claude resume?","header":"Resume","options":[
                    {"label":"Compact","description":"Compact the session"},{"label":"Continue","description":"Continue the session"},{"label":"Never","description":"Do not resume"}]}]});
                (
                    Callback::Dialog(responder),
                    native,
                    request.dialog_kind.clone(),
                    input,
                    "user_input",
                    "Resume Claude session".into(),
                )
            }
            PendingRequest::Hook { request, responder } => {
                let event = request.get().input["hook_event_name"]
                    .as_str()
                    .unwrap_or_default();
                let supported = matches!(
                    event,
                    "SubagentStart"
                        | "SubagentStop"
                        | "PostToolUse"
                        | "PreToolUse"
                        | "Stop"
                        | "SessionStart"
                        | "Notification"
                );
                if !supported {
                    return responder
                        .reject(t3_claude_cli::Error::Unsupported("hook".into()))
                        .await
                        .map_err(protocol);
                }
                return responder
                    .respond(t3_claude_cli::control::HookOutput::default())
                    .await
                    .map_err(protocol);
            }
            PendingRequest::Mcp { responder, .. } => {
                return responder
                    .reject(t3_claude_cli::Error::Unsupported(
                        "unregistered MCP server".into(),
                    ))
                    .await
                    .map_err(protocol);
            }
        };
        let (id, mut events) = {
            let mut projection = lock(&self.projection);
            if name == "ExitPlanMode" {
                let now = projection
                    .active_input()
                    .map(|_| ())
                    .ok_or(ProviderError::SessionStopped);
                now?;
            }
            let agent = match &callback {
                Callback::Permission { request, .. } => request.agent_id.as_deref(),
                _ => None,
            };
            projection.pending_request_for_agent(agent, &native, &name, &input, kind, &prompt)?
        };
        if callback.cancellation().is_cancelled() {
            return Ok(());
        }
        lock(&self.pending).insert(id.0.clone(), callback);
        self.emit(std::mem::take(&mut events)).await
    }
    async fn cancel_callback(&self, native_id: &str) -> ProviderResult<()> {
        let id = lock(&self.pending)
            .iter()
            .find_map(|(id, callback)| (callback.native_id() == native_id).then_some(id.clone()));
        if let Some(id) = id {
            lock(&self.pending).remove(&id);
            let events = lock(&self.projection).resolve_request(&RuntimeRequestId(id), None)?;
            self.emit(events).await?;
        }
        Ok(())
    }
}
fn automatic_permission_result(
    request: &t3_claude_cli::control::ToolPermission,
    runtime: &RuntimePolicy,
    query: &ClaudeRuntimeQueryPolicy,
    active_mode: t3_claude_cli::control::PermissionMode,
) -> Option<PermissionDecision> {
    use t3_claude_cli::control::PermissionMode;
    use t3_contracts::provider_policy::ProviderInteractionMode;
    if matches!(
        request.tool_name.as_str(),
        "AskUserQuestion" | "ExitPlanMode"
    ) {
        return None;
    }
    let deny = |message| PermissionDecision::Deny {
        message,
        interrupt: None,
        decision_classification: None,
    };
    if runtime.interaction_mode == ProviderInteractionMode::Plan
        && runtime
            .approval_policy
            .as_ref()
            .is_some_and(|policy| policy == "never")
    {
        return Some(deny(
            "This tool requires approval in Plan mode, but approvals are disabled for this thread."
                .into(),
        ));
    }
    if !query.install_permission_callback
        && query.permission_mode != PermissionMode::BypassPermissions
    {
        return Some(PermissionDecision::Allow {
            updated_input: request.input.clone(),
            updated_permissions: None,
            decision_classification: None,
        });
    }
    let reason = strip_claude_vt_controls(request.decision_reason.as_deref().unwrap_or_default());
    let reason = reason.trim();
    let lower = reason.to_ascii_lowercase();
    let removal_prefix = ["dangerous rmdir", "dangerous rm"]
        .into_iter()
        .any(|prefix| {
            lower.strip_prefix(prefix).is_some_and(|tail| {
                tail.chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
            })
        });
    if request.tool_name == "Bash"
        && active_mode == PermissionMode::BypassPermissions
        && request.matched_ask_rule.is_none()
        && (removal_prefix
            || matches!(
                reason,
                "This shell -c script runs rm and could not be checked"
                    | "A permission rule denies a command in this shell -c script"
            ))
    {
        return Some(deny(format!(
            "Claude Code's removal safety check denied this command; the user did not reject it. The command did not run. Do not bypass the check by splitting the removal, putting it in a script, or using another tool or shell. If the warning below supplies a safe rewrite, use it and let the same check evaluate it. Otherwise continue the rest of the task without the removal and tell the user what you needed to delete and why. Warning: {reason}"
        )));
    }
    None
}

fn strip_claude_vt_controls(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut result = String::new();
    while let Some(c) = chars.next() {
        let sequence = if c == '\u{1b}' { chars.next() } else { Some(c) };
        match sequence {
            Some(marker) if (c == '\u{1b}' && marker == '[') || marker == '\u{9b}' => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(marker)
                if (c == '\u{1b}' && matches!(marker, ']' | 'P' | 'X' | '^' | '_'))
                    || marker == '\u{9d}' =>
            {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || c == '\u{9c}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(' '..='/') if c == '\u{1b}' => {
                for c in chars.by_ref() {
                    if ('0'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some('@'..='_') if c == '\u{1b}' => {}
            Some(next) if c == '\u{1b}' => {
                result.push(c);
                result.push(next);
            }
            Some(c) => result.push(c),
            None => result.push(c),
        }
    }
    result
}

fn approval_only_schema(schema: Option<&Value>) -> bool {
    schema
        .and_then(|v| v["properties"].as_object())
        .is_some_and(|p| p.is_empty())
}

struct LiveQuery {
    client: Client,
    task: JoinHandle<()>,
    cancellation: CancellationToken,
    policy: ClaudeRuntimeQueryPolicy,
    native: String,
    model: String,
    effort: Option<String>,
    settings: Value,
    selection_valid: bool,
}
impl LiveQuery {
    fn can_apply_selection(&self, next: &ClaudeCompiledModel) -> bool {
        self.settings["alwaysThinkingEnabled"] == next.settings["alwaysThinkingEnabled"]
            && (next.effort.is_some() || self.effort.is_none())
    }

    fn selection_matches(&self, next: &ClaudeCompiledModel) -> bool {
        self.selection_valid
            && self.model == next.model
            && self.effort == next.effort
            && self.settings == next.settings
    }

    async fn apply_selection(&mut self, next: ClaudeCompiledModel) -> ProviderResult<()> {
        let set_model = !self.selection_valid || self.model != next.model;
        // Cancellation or a failed control leaves the native selection uncertain until reapplied.
        self.selection_valid = false;
        if set_model {
            self.client
                .request(OutboundRequest::SetModel {
                    model: next.model.clone(),
                })
                .await
                .map_err(protocol)?;
        }
        self.client
            .request(OutboundRequest::ApplyFlagSettings {
                settings: t3_claude_cli::control::FlagSettings {
                    effort_level: next.effort.clone(),
                    fast_mode: next.settings["fastMode"].as_bool(),
                    ultracode: next.settings["ultracode"].as_bool(),
                },
            })
            .await
            .map_err(protocol)?;
        self.model = next.model;
        self.effort = next.effort;
        self.settings = next.settings;
        self.selection_valid = true;
        Ok(())
    }
}
pub struct ClaudeSessionRuntime {
    session: OrchestrationV2ProviderSession,
    inner: Arc<RuntimeInner>,
    options: Arc<ClaudeAdapterOptions>,
    mcp_servers: BTreeMap<String, Arc<dyn McpServer>>,
    open: OpenSession,
    // Keep native query identity stable until controls acknowledge before replacing it.
    query: AsyncMutex<Option<LiveQuery>>,
    // Serialize turn admission and query replacement through native control acknowledgements.
    operations: AsyncMutex<()>,
    policy_options: Mutex<RuntimePolicyOptions>,
}
impl Drop for ClaudeSessionRuntime {
    fn drop(&mut self) {
        self.inner.cancellation.cancel();
    }
}
impl ClaudeSessionRuntime {
    async fn prepare_message(
        &self,
        message: &TurnMessage,
        priority: Option<Priority>,
    ) -> ProviderResult<(UserMessage, AttachmentReservation)> {
        let previous = {
            let budget = lock(&self.inner.attachments);
            budget.committed.add(budget.pending)
        };
        let message = message.clone();
        let source = self.options.attachments.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_claude_message(&message, priority, source.as_ref(), previous)
        })
        .await
        .map_err(|_| {
            operation(
                ProviderOperation::Start,
                "Claude attachment preparation failed",
            )
        })??;
        let usage = prepared.usage.subtract(previous);
        {
            let mut budget = lock(&self.inner.attachments);
            budget.pending = budget.pending.add(usage);
        }
        Ok((
            prepared.message,
            AttachmentReservation {
                inner: self.inner.clone(),
                usage,
            },
        ))
    }
    async fn prepare_launch(
        &self,
        input: &TurnInput,
        policy_options: &RuntimePolicyOptions,
    ) -> ProviderResult<PreparedClaudeLaunch> {
        let base = self.options.clone();
        let owned_input = input.clone();
        let owned_policy = policy_options.clone();
        let settings = tokio::task::spawn_blocking(move || {
            prepare_claude_launch_settings(&base.launch, &owned_input, &owned_policy, &base.host)
        });
        let base = self.options.clone();
        let cwd = input.runtime_policy.cwd.clone();
        let append = tokio::task::spawn_blocking(move || {
            read_claude_append_files(&base.launch, cwd.as_deref(), &base.host)
        });
        let (settings, append) = tokio::join!(settings, append);
        let settings = settings
            .map_err(|_| operation(ProviderOperation::Open, "Claude launch settings failed"))??;
        let append = append.map_err(|_| {
            operation(
                ProviderOperation::Open,
                "Claude append prompt preparation failed",
            )
        })??;
        Ok(finish_claude_launch(settings, append))
    }
    async fn launch(
        &self,
        input: &TurnInput,
        mut prepared: PreparedClaudeLaunch,
    ) -> ProviderResult<Client> {
        lock(&self.inner.projection).auto_compact_window = prepared.auto_compact_window;
        if self.mcp_servers.contains_key("t3-code")
            || prepared.options.mcp_servers.contains_key("t3-code")
        {
            let allowed: &[&str] = if input
                .runtime_policy
                .sandbox_policy
                .as_ref()
                .is_some_and(|policy| policy["type"] == "readOnly")
            {
                CLAUDE_READ_ONLY_T3_MCP_ALLOWED_TOOLS
            } else {
                &["mcp__t3-code__*"]
            };
            for tool in allowed {
                if !prepared
                    .policy
                    .allowed_tools
                    .iter()
                    .any(|existing| existing == tool)
                {
                    prepared.policy.allowed_tools.push((*tool).into());
                }
            }
            if let SystemPrompt::ClaudeCode { append } = &mut prepared.options.system_prompt {
                append
                    .get_or_insert_default()
                    .push_str(CLAUDE_SEND_TO_USER_INSTRUCTIONS);
            }
        }
        let policy = prepared.policy;
        let model = prepared.model;
        let native = input
            .provider_thread
            .native_thread_ref
            .as_ref()
            .and_then(|v| v.native_id.as_ref())
            .map(|v| v.0.clone())
            .ok_or_else(|| operation(ProviderOperation::Start, "Missing Claude native session"))?;
        let mut current = self.query.lock().await;
        if let Some(query) = current.as_mut() {
            if query.native == native
                && query.policy == policy
                && query.can_apply_selection(&model)
                && !query.task.is_finished()
                && !query.cancellation.is_cancelled()
            {
                if !query.selection_matches(&model) {
                    query.apply_selection(model).await?;
                }
                return Ok(query.client.clone());
            }
            if lock(&self.inner.projection).pending_background_work(true) {
                return Err(operation(
                    ProviderOperation::Start,
                    "Background Claude work blocks query replacement",
                ));
            }
        }
        if let Some(query) = current.take() {
            query.cancellation.cancel();
            query
                .task
                .await
                .map_err(|_| operation(ProviderOperation::Close, "Claude query task failed"))?;
        }
        let mut options = prepared.options;
        if matches!(options.system_prompt, SystemPrompt::Unspecified) {
            options.system_prompt = SystemPrompt::ClaudeCode { append: None };
        }
        options.cwd = input.runtime_policy.cwd.clone();
        options.permission_mode = Some(policy.permission_mode);
        options.allow_dangerously_skip_permissions = policy.allow_dangerously_skip_permissions;
        options.can_use_tool = true;
        options.include_partial_messages = true;
        options.tools = policy.tools.clone().map(Tools::List);
        options.allowed_tools = policy.allowed_tools.clone();
        options.initialize.prompt_suggestions =
            Some(self.open.prompt_suggestions.as_ref().is_some_and(|f| f()));
        if input.native_thread_has_turns == Some(true)
            || input.provider_thread.native_conversation_head_ref.is_some()
            || self.open.initial_native_thread_id.is_some()
        {
            options.resume = Some(native.clone());
            options.session_id = None;
        } else {
            options.session_id = Some(native.clone());
            options.resume = None;
        }
        options.resume_session_at = input
            .provider_thread
            .native_conversation_head_ref
            .as_ref()
            .and_then(|r| r.native_id.as_ref())
            .map(|id| id.0.clone());
        let dynamic_mcp = std::mem::take(&mut options.mcp_servers);
        for name in self.mcp_servers.keys() {
            options
                .mcp_servers
                .insert(name.clone(), json!({"type":"sdk","name":name}));
        }
        options.initialize.sdk_mcp_servers = Some(self.mcp_servers.keys().cloned().collect());
        let description = describe_spawn(&options).map_err(protocol)?;
        let cancellation = self.inner.cancellation.child_token();
        lock(&self.inner.projection).reset_process_usage();
        {
            let mut wake = lock(&self.inner.wake);
            wake.clear_schedule();
        }
        *lock(&self.inner.permission_policy) = Some((policy.clone(), policy.permission_mode));
        let connection = self
            .options
            .connector
            .connect(&options, cancellation.clone())
            .await?;
        let client = connection.session.client();
        for (name, server) in &self.mcp_servers {
            client.register_mcp_server(name.clone(), server.clone());
        }
        let inner = self.inner.clone();
        let token = cancellation.clone();
        let task = tokio::spawn(async move {
            run_query(inner, connection, token).await;
        });
        *current = Some(LiveQuery {
            client: client.clone(),
            task,
            cancellation: cancellation.clone(),
            policy,
            native,
            model: model.model,
            effort: model.effort,
            settings: model.settings,
            selection_valid: true,
        });
        drop(current);
        let result = tokio::select! { _ = cancellation.cancelled() => Err(ProviderError::Cancelled), result=client.initialize(description.initialize)=>result.map(|_|()).map_err(protocol) };
        if let Err(error) = result {
            self.stop_query().await?;
            return Err(error);
        }
        if !dynamic_mcp.is_empty() {
            let registration = client
                .request(OutboundRequest::McpSetServers {
                    servers: dynamic_mcp,
                })
                .await;
            if let Err(error) = registration {
                self.stop_query().await?;
                return Err(protocol(error));
            }
        }
        Ok(client)
    }
    async fn stop_query(&self) -> ProviderResult<()> {
        let query = self.query.lock().await.take();
        if let Some(query) = query {
            query.cancellation.cancel();
            query
                .task
                .await
                .map_err(|_| operation(ProviderOperation::Close, "Claude query task failed"))?;
        }
        Ok(())
    }
    async fn client(&self) -> ProviderResult<Client> {
        self.query
            .lock()
            .await
            .as_ref()
            .filter(|q| !q.task.is_finished() && !q.cancellation.is_cancelled())
            .map(|q| q.client.clone())
            .ok_or(ProviderError::SessionStopped)
    }
}
async fn run_query(
    inner: Arc<RuntimeInner>,
    mut connection: ClaudeConnection,
    cancellation: CancellationToken,
) {
    let mut stderr_task = connection
        .process
        .as_mut()
        .and_then(|p| p.stderr.take())
        .map(|mut stderr| {
            tokio::spawn(async move {
                let mut buffer = [0u8; 8192];
                while matches!(stderr.read(&mut buffer).await,Ok(n) if n>0) {}
            })
        });
    loop {
        let result = tokio::select! { biased; _=cancellation.cancelled()=>break,event=connection.session.next_event()=>event };
        let result = match result {
            Some(Ok(Event::Message(message))) => {
                let frame = lock(&inner.projection).observe_result(message.raw());
                if frame["type"] == "system"
                    && matches!(frame["subtype"].as_str(), Some("init" | "status"))
                    && let Ok(mode) = serde_json::from_value(frame["permissionMode"].clone())
                    && let Some((_, active_mode)) = lock(&inner.permission_policy).as_mut()
                {
                    *active_mode = mode;
                }
                match inner.buffer_native_wake(&frame).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        inner.error(error).await;
                        break;
                    }
                }
                let budget_changed = if frame["type"] == "system" {
                    let mut budget = lock(&inner.attachments);
                    if frame["subtype"] == "status" && frame["status"] == "compacting" {
                        if budget.compacting.is_none() {
                            budget.compacting = Some(budget.committed);
                        }
                        false
                    } else if frame["subtype"] == "compact_boundary" {
                        if frame["compact_metadata"].get("preserved_segment").is_none()
                            && frame["compact_metadata"]
                                .get("preserved_messages")
                                .is_none()
                        {
                            let compacted = budget.compacting.unwrap_or(budget.committed);
                            budget.committed = budget.committed.subtract(compacted);
                        }
                        budget.compacting = None;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                let events = lock(&inner.projection).handle_frame(&frame);
                let notification = lock(&inner.projection).native_wake_notification(&frame);
                match events {
                    Ok(events) => {
                        let result = inner.emit(events).await;
                        if let Err(error) = result {
                            Err(error)
                        } else if let Err(error) = notification {
                            Err(error)
                        } else {
                            let result = if budget_changed {
                                inner.persist_attachment_usage().await
                            } else {
                                Ok(())
                            };
                            if result.is_ok()
                                && let Some(notification) = notification.ok().flatten()
                            {
                                if let Some(summary) = frame["summary"]
                                    .as_str()
                                    .filter(|summary| !summary.is_empty())
                                {
                                    lock(&inner.wake).detail = Some(summary.into());
                                }
                                inner.offer_wake(Some(notification)).await
                            } else {
                                result
                            }
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            Some(Ok(Event::Request(request))) => inner.callback(*request).await,
            Some(Ok(Event::Cancelled { request_id })) => inner.cancel_callback(&request_id).await,
            Some(Ok(
                Event::Diagnostic { .. } | Event::NonJson { .. } | Event::UnmatchedResponse { .. },
            )) => Ok(()),
            Some(Err(error)) => Err(protocol(error)),
            None => break,
        };
        if let Err(error) = result {
            inner.error(error).await;
        }
    }
    cancellation.cancel();
    lock(&inner.wake).clear_schedule();
    let ids = lock(&inner.pending).keys().cloned().collect::<Vec<_>>();
    lock(&inner.pending).clear();
    for id in ids {
        let events = lock(&inner.projection).resolve_request(&RuntimeRequestId(id), None);
        if let Ok(events) = events {
            let _ = inner.emit(events).await;
        }
    }
    let terminal = lock(&inner.projection).query_ended();
    if let Ok(events) = terminal {
        let _ = inner.emit(events).await;
    }
    let _ = connection
        .session
        .shutdown_with_cancellation(cancellation.clone())
        .await;
    if let Some(process) = connection.process.as_mut() {
        let _ = process.terminate(Duration::from_secs(1)).await;
    }
    if let Some(task) = stderr_task.take() {
        let _ = task.await;
    }
}

#[async_trait]
impl SessionRuntime for ClaudeSessionRuntime {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        true
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        if self.inner.cancellation.is_cancelled() {
            return Err(ProviderError::SessionStopped);
        }
        let (sender, events) = mpsc::channel(EVENT_CAPACITY);
        let cancellation = self.inner.cancellation.child_token();
        lock(&self.inner.subscribers).push((sender, cancellation.clone()));
        Ok(EventSubscription {
            events,
            cancellation,
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.inner.cancellation.cancel();
        self.stop_query().await
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let options = lock(&self.policy_options).clone();
        self.ensure_thread_with_policy(input, options).await
    }
    async fn ensure_thread_with_policy(
        &self,
        input: EnsureThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let _lane = self.operations.lock().await;
        if self.inner.cancellation.is_cancelled() {
            return Err(ProviderError::SessionStopped);
        }
        let native = if input.force_new_native_thread {
            (self.options.new_native_session_id)()
        } else {
            self.open
                .initial_native_thread_id
                .clone()
                .unwrap_or_else(|| (self.options.new_native_session_id)())
        };
        let thread = lock(&self.inner.projection).ensure(&input, &native)?;
        *lock(&self.policy_options) = options;
        Ok(thread)
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let options = lock(&self.policy_options).clone();
        self.resume_thread_with_policy(input, options).await
    }
    async fn resume_thread_with_policy(
        &self,
        input: ResumeThread,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let _lane = self.operations.lock().await;
        if self.inner.cancellation.is_cancelled() {
            return Err(ProviderError::SessionStopped);
        }
        if input.provider_thread.native_thread_ref.is_none() {
            return Err(operation(
                ProviderOperation::Resume,
                "Missing Claude native session",
            ));
        }
        lock(&self.inner.projection).thread = Some(input.provider_thread.clone());
        let usage = input
            .provider_thread
            .native_metadata
            .as_ref()
            .and_then(|m| m.as_ref())
            .and_then(|m| m.claude_attachment_budget.as_ref())
            .and_then(|u| u.as_ref());
        lock(&self.inner.attachments).committed = usage
            .and_then(|u| serde_json::to_value(u).ok())
            .and_then(|u| serde_json::from_value(u).ok())
            .unwrap_or_default();
        *lock(&self.policy_options) = options;
        Ok(input.provider_thread)
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        let options = lock(&self.policy_options).clone();
        self.start_turn_with_policy(input, options).await
    }
    async fn start_turn_with_policy(
        &self,
        input: TurnInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<()> {
        let _lane = self.operations.lock().await;
        if self.inner.cancellation.is_cancelled() {
            return Err(ProviderError::SessionStopped);
        }
        if lock(&self.inner.projection).active_turn_id().is_some() {
            return Err(operation(
                ProviderOperation::Start,
                "Previous Claude turn is active",
            ));
        }
        let wake_frames = {
            let mut wake = lock(&self.inner.wake);
            if wake.offered
                && !wake.retired
                && input.message.created_by == OrchestrationV2Actor::Agent
                && input.message.creation_source == Some(OrchestrationV2CreationSource::Provider)
            {
                let monitor = wake.monitor_event;
                let end = if monitor {
                    wake.own_end.filter(|end| {
                        wake.frames
                            .get(*end..)
                            .is_some_and(|frames| frames.iter().any(NativeWake::root_frame))
                    })
                } else {
                    None
                }
                .unwrap_or(wake.frames.len());
                let absorbed = wake.absorbed_ends.contains(&end);
                let frames = wake.frames.drain(..end).collect::<Vec<_>>();
                wake.absorbed_ends = std::mem::take(&mut wake.absorbed_ends)
                    .into_iter()
                    .filter_map(|offset| offset.checked_sub(end).filter(|offset| *offset > 0))
                    .collect();
                wake.offered = false;
                wake.retired = false;
                wake.opened = wake.frames.iter().any(NativeWake::root_frame);
                wake.ended = wake.frames.iter().any(|frame| frame["type"] == "result");
                wake.notification = wake.next_notification.take();
                wake.detail = wake.next_detail.take();
                wake.monitor_event = false;
                wake.own_end = None;
                Some((frames, absorbed, monitor))
            } else {
                None
            }
        };
        if let Some((frames, absorbed, monitor)) = wake_frames {
            let mut events = lock(&self.inner.projection).begin(input)?;
            {
                let mut projection = lock(&self.inner.projection);
                if !monitor {
                    events.extend(projection.consume_native_wake()?);
                }
                for frame in frames {
                    events.extend(projection.handle_frame(&frame)?);
                }
                if absorbed {
                    events.extend(projection.complete_absorbed_wake()?);
                }
            }
            self.inner.emit(events).await?;
            return self.inner.offer_wake(None).await;
        }
        let ambiguous = {
            let projection = lock(&self.inner.projection);
            projection.pending_background_work(true) || projection.has_unconsumed_native_wake()
        };
        lock(&self.inner.wake).ambiguous_user_turn = ambiguous;
        let (message, reservation) = self.prepare_message(&input.message, None).await?;
        let prepared = self.prepare_launch(&input, &options).await?;
        let client = self.launch(&input, prepared).await?;
        *lock(&self.policy_options) = options;
        let events = lock(&self.inner.projection).begin(input)?;
        self.inner.emit(events).await?;
        client.send_user(message).await.map_err(protocol)?;
        lock(&self.inner.wake).absorb_open_tail();
        reservation.commit();
        self.inner.persist_attachment_usage().await
    }
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
        let _lane = self.operations.lock().await;
        let (message, reservation) = self
            .prepare_message(&input.message, Some(Priority::Now))
            .await?;
        let client = self.client().await?;
        if let Some(selection) = &input.model_selection {
            client
                .request(OutboundRequest::SetModel {
                    model: selection.model.clone(),
                })
                .await
                .map_err(protocol)?;
        }
        let events = lock(&self.inner.projection)
            .remember_steer(input.message.message_id.0.clone(), input)?;
        self.inner.emit(events).await?;
        client.send_user(message).await.map_err(protocol)?;
        reservation.commit();
        self.inner.persist_attachment_usage().await
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        if lock(&self.inner.projection).active_turn_id().as_ref() != Some(&input.provider_turn_id) {
            return Err(ProviderError::SessionStopped);
        }
        lock(&self.inner.projection).interrupt();
        self.client()
            .await?
            .request(OutboundRequest::Interrupt)
            .await
            .map_err(protocol)?;
        if input.request_runtime_restart {
            self.stop_query().await?;
        }
        Ok(())
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        {
            let pending = lock(&self.inner.pending);
            let callback = pending.get(&input.request_id.0).ok_or_else(|| {
                operation(ProviderOperation::Respond, "Request is no longer pending")
            })?;
            if callback.cancellation().is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            match callback {
                Callback::Permission { request, .. } if request.tool_name == "AskUserQuestion" => {
                    if input.answers.is_none()
                        && !matches!(
                            input.decision,
                            Some(
                                ProviderApprovalDecision::Cancel
                                    | ProviderApprovalDecision::Decline
                            )
                        )
                    {
                        return Err(operation(
                            ProviderOperation::Respond,
                            "Explicit answers are required",
                        ));
                    }
                }
                Callback::Dialog(_) => {
                    if input.answers.is_none()
                        && !matches!(
                            input.decision,
                            Some(
                                ProviderApprovalDecision::Cancel
                                    | ProviderApprovalDecision::Decline
                            )
                        )
                    {
                        return Err(operation(
                            ProviderOperation::Respond,
                            "Explicit resume choice is required",
                        ));
                    }
                }
                _ => {
                    if input.decision.is_none() {
                        return Err(operation(
                            ProviderOperation::Respond,
                            "Explicit permission decision is required",
                        ));
                    }
                }
            }
        }
        let callback = lock(&self.inner.pending)
            .remove(&input.request_id.0)
            .ok_or(ProviderError::Cancelled)?;
        let result = match callback {
            Callback::Permission {
                request,
                responder,
                managed,
            } => {
                let decision = if request.tool_name == "AskUserQuestion"
                    && !matches!(
                        input.decision,
                        Some(ProviderApprovalDecision::Cancel | ProviderApprovalDecision::Decline)
                    ) {
                    let mut updated = request.input.clone();
                    updated.insert(
                        "answers".into(),
                        claude_sdk_user_input_answers(
                            input.answers.as_ref().ok_or(ProviderError::Cancelled)?,
                        ),
                    );
                    PermissionDecision::Allow {
                        updated_input: updated,
                        updated_permissions: None,
                        decision_classification: Some("user_temporary".into()),
                    }
                } else {
                    permission_result_from_decision(
                        &request,
                        input.decision.ok_or(ProviderError::Cancelled)?,
                        input.feedback.as_deref(),
                        managed,
                    )
                };
                responder.respond(decision).await
            }
            Callback::Elicitation(responder) => {
                responder
                    .respond(match input.decision {
                        Some(
                            ProviderApprovalDecision::Accept
                            | ProviderApprovalDecision::AcceptForSession,
                        ) => ElicitationResponse::Accept {
                            content: Some(BTreeMap::new()),
                        },
                        Some(ProviderApprovalDecision::Cancel) => ElicitationResponse::Cancel,
                        _ => ElicitationResponse::Decline,
                    })
                    .await
            }
            Callback::Dialog(responder) => {
                let choice = input
                    .answers
                    .as_ref()
                    .and_then(|answers| answers.values().next())
                    .and_then(Value::as_str);
                let response = match choice {
                    Some("Compact" | "compact") => DialogResponse::Completed {
                        result: ResumeChoice::Compact,
                    },
                    Some("Continue" | "continue") => DialogResponse::Completed {
                        result: ResumeChoice::Continue,
                    },
                    Some("Never" | "never") => DialogResponse::Completed {
                        result: ResumeChoice::Never,
                    },
                    _ => DialogResponse::Cancelled,
                };
                responder.respond(response).await
            }
        };
        let events = lock(&self.inner.projection).resolve_request(
            &input.request_id,
            if result.is_ok() { Some(&input) } else { None },
        )?;
        self.inner.emit(events).await?;
        result.map_err(protocol)
    }
    async fn snapshot(
        &self,
        thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        Ok(lock(&self.inner.projection).snapshot(thread))
    }
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        let _lane = self.operations.lock().await;
        if lock(&self.inner.projection).active_turn_id().is_some() {
            return Err(operation(
                ProviderOperation::Rollback,
                "Cannot roll back an active Claude turn",
            ));
        }
        if self.inner.cancellation.is_cancelled() {
            return Err(ProviderError::SessionStopped);
        }
        let cursor = rollback_resume_cursor(&input)?;
        self.stop_query().await?;
        let now = self.options.clock.now();
        let reset = matches!(input.target, RollbackTarget::ThreadStart { .. }) && cursor.is_none();
        let mut thread = if reset {
            let native = (self.options.new_native_session_id)();
            let ensure = EnsureThread {
                thread_id: self.open.thread_id.clone(),
                title: None,
                model_selection: self.open.model_selection.clone(),
                runtime_policy: self.open.runtime_policy.clone(),
                provider_session_id: Some(self.open.provider_session_id.clone()),
                existing_provider_thread: None,
                force_new_native_thread: true,
            };
            let mut thread = lock(&self.inner.projection).ensure(&ensure, &native)?;
            thread.owner_node_id = input.provider_thread.owner_node_id.clone();
            thread.forked_from = input.provider_thread.forked_from.clone();
            thread.handoff_ids = input.provider_thread.handoff_ids.clone();
            thread
        } else {
            let mut thread = input.provider_thread;
            thread.provider_session_id =
                Some(t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                    self.open.provider_session_id.clone(),
                ));
            thread.native_conversation_head_ref = cursor
                .map(|cursor| decode(native_ref(&cursor, "weak")))
                .transpose()?;
            thread.status = ProviderThreadStatus::Idle;
            if let RollbackTarget::ProviderTurn {
                app_run_ordinal, ..
            } = input.target
            {
                thread.last_run_ordinal = Some(app_run_ordinal);
            }
            thread
        };
        thread.updated_at = now;
        let mut projection = lock(&self.inner.projection);
        projection.reset_after_rollback(thread.clone());
        Ok(ThreadSnapshot {
            provider_thread: thread,
            provider_turns: vec![],
            messages: vec![],
            reasoning_message_ids: None,
            runtime_requests: vec![],
            provider_payload: None,
        })
    }
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        let _lane = self.operations.lock().await;
        if lock(&self.inner.projection).active_turn_id().is_some() {
            return Err(operation(
                ProviderOperation::Fork,
                "Cannot fork an active Claude turn",
            ));
        }
        if self.inner.cancellation.is_cancelled() {
            return Err(ProviderError::SessionStopped);
        }
        let cursor = claude_fork_cursor(&input)?;
        let source = input
            .source_provider_thread
            .native_thread_ref
            .as_ref()
            .and_then(|r| r.native_id.as_ref())
            .map(|id| id.0.clone())
            .ok_or_else(|| operation(ProviderOperation::Fork, "Missing Claude native session"))?;
        let root = self.options.transcript_root.clone().ok_or_else(|| {
            operation(
                ProviderOperation::Fork,
                "Claude transcript root was not injected",
            )
        })?;
        let native = (self.options.new_native_session_id)();
        if uuid::Uuid::parse_str(&source).is_err()
            || uuid::Uuid::parse_str(&native).is_err()
            || source == native
        {
            return Err(operation(
                ProviderOperation::Fork,
                "Native fork requires distinct UUID session ids",
            ));
        }
        self.stop_query().await?;
        let now = self.options.clock.now();
        let source_id = source.clone();
        let target_id = native.clone();
        let timestamp = now.0.clone();
        let fork = tokio::task::spawn_blocking(move || {
            fork_claude_session_file(&root, &source_id, &target_id, cursor.as_deref(), &timestamp)
        })
        .await
        .map_err(|_| {
            operation(
                ProviderOperation::Fork,
                "Claude transcript fork task failed",
            )
        })??;
        let source_metadata = serde_json::to_value(&input.source_provider_thread.native_metadata)
            .map_err(|_| {
            operation(ProviderOperation::Fork, "Invalid source native metadata")
        })?;
        let mut metadata = json!({});
        if let Some(head) = fork.head_message_id {
            metadata["forkHeadMessageId"] = json!(head);
        }
        if let Some(budget) = source_metadata.get("claudeAttachmentBudget") {
            metadata["claudeAttachmentBudget"] = budget.clone();
        }
        let mut forked_from = json!({"providerThreadId":input.source_provider_thread.id});
        if let Some(turn) = input.provider_turn_id {
            forked_from["providerTurnId"] = json!(turn);
        }
        decode(
            json!({"id":provider_id("provider-thread","native-thread",&native),"driver":"claudeAgent","providerInstanceId":self.options.instance_id,"providerSessionId":self.open.provider_session_id,"appThreadId":input.target_thread_id,"ownerNodeId":input.owner_node_id,"nativeThreadRef":native_ref(&native,"strong"),"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[],"forkedFrom":forked_from,"nativeMetadata":metadata,"createdAt":now,"updatedAt":now}),
        )
    }
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        Some(self)
    }
}

#[async_trait]
impl SessionExtensions for ClaudeSessionRuntime {
    fn capabilities(&self) -> ExtensionCapabilities {
        ExtensionCapabilities {
            compact: true,
            apply_selection: true,
            stop_task: true,
            unload: true,
            ..ExtensionCapabilities::default()
        }
    }
    async fn pending_background_work(
        &self,
        thread: Option<OrchestrationV2ProviderThread>,
        include_subagents: bool,
    ) -> ProviderResult<bool> {
        let projection = lock(&self.inner.projection);
        if thread.as_ref().is_some_and(|thread| {
            projection
                .thread
                .as_ref()
                .is_none_or(|current| current.id != thread.id)
        }) {
            return Ok(false);
        }
        let pending = projection.pending_background_work(include_subagents);
        drop(projection);
        let wake = lock(&self.inner.wake);
        Ok(pending
            || (thread.is_none()
                && (wake.scheduled_at.is_some()
                    || wake.frames.iter().any(|frame| {
                        matches!(
                            frame["type"].as_str(),
                            Some("user" | "assistant" | "result")
                        )
                    }))))
    }
    async fn model_context_window(
        &self,
        _selection: t3_contracts::model_selection::ModelSelection,
        _cwd: Option<PathBuf>,
    ) -> ProviderResult<Option<u64>> {
        Ok(lock(&self.inner.projection).max_tokens)
    }
    fn can_reuse_context_usage(
        &self,
        previous: &t3_contracts::model_selection::ModelSelection,
        next: &t3_contracts::model_selection::ModelSelection,
    ) -> bool {
        previous.instance_id == next.instance_id && previous.model == next.model
    }
    fn normalize_thread_load_selection(
        &self,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> t3_contracts::model_selection::ModelSelection {
        selection
    }
    async fn inject_history(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _messages: Vec<OrchestrationV2HistoricalMessage>,
        _context: String,
    ) -> ProviderResult<bool> {
        Err(ProviderError::Unsupported("Claude history injection"))
    }
    async fn compact(&self, mut input: TurnInput) -> ProviderResult<()> {
        input.message.text = "/compact".into();
        self.start_turn(input).await
    }
    async fn apply_selection(
        &self,
        thread: OrchestrationV2ProviderThread,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> ProviderResult<()> {
        let _lane = self.operations.lock().await;
        let mut query = self.query.lock().await;
        let query = query
            .as_mut()
            .filter(|query| !query.task.is_finished() && !query.cancellation.is_cancelled())
            .ok_or(ProviderError::SessionStopped)?;
        if thread
            .native_thread_ref
            .as_ref()
            .and_then(|reference| reference.native_id.as_ref())
            .map(|id| id.0.as_str())
            != Some(query.native.as_str())
        {
            return Err(ProviderError::SessionStopped);
        }
        if selection.instance_id != self.options.instance_id {
            return Err(ProviderError::Unsupported("Claude live provider change"));
        }
        let compiled = match &self.options.host.compile_model {
            Some(compile) => compile(&selection)?,
            None => direct_claude_model(&selection),
        };
        if query.model != compiled.model
            || !query.can_apply_selection(&compiled)
            || query.selection_matches(&compiled)
        {
            return Ok(());
        }
        query.apply_selection(compiled).await
    }
    async fn stop_task(
        &self,
        thread: OrchestrationV2ProviderThread,
        task_id: String,
    ) -> ProviderResult<()> {
        let query = self.query.lock().await;
        let query = query
            .as_ref()
            .filter(|query| !query.task.is_finished() && !query.cancellation.is_cancelled())
            .ok_or(ProviderError::SessionStopped)?;
        if thread
            .native_thread_ref
            .as_ref()
            .and_then(|reference| reference.native_id.as_ref())
            .map(|id| id.0.as_str())
            != Some(query.native.as_str())
        {
            return Err(ProviderError::SessionStopped);
        }
        query
            .client
            .request(OutboundRequest::StopTask { task_id })
            .await
            .map_err(protocol)?;
        Ok(())
    }
    async fn await_tasks(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _task_ids: Vec<String>,
        _awaited: bool,
    ) -> ProviderResult<Vec<String>> {
        Err(ProviderError::Unsupported(
            "Claude awaited background tasks",
        ))
    }
    async fn stop_subagent(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _native_task_id: String,
        _requested_at: Option<IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>> {
        Err(ProviderError::Unsupported(
            "Claude confirmed subagent outcome",
        ))
    }
    async fn unload(
        &self,
        thread: OrchestrationV2ProviderThread,
        _archive: bool,
    ) -> ProviderResult<()> {
        let _lane = self.operations.lock().await;
        if self.query.lock().await.as_ref().is_some_and(|query| {
            thread
                .native_thread_ref
                .as_ref()
                .and_then(|reference| reference.native_id.as_ref())
                .map(|id| id.0.as_str())
                != Some(query.native.as_str())
        }) {
            return Err(ProviderError::SessionStopped);
        }
        self.stop_query().await
    }
    async fn upload_feedback(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _reason: Option<String>,
    ) -> ProviderResult<String> {
        Err(ProviderError::Unsupported("Claude feedback"))
    }
    async fn list_tools(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
    ) -> ProviderResult<Vec<McpTool>> {
        Err(ProviderError::Unsupported("Claude MCP Apps tool listing"))
    }
    async fn call_tool(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
        _tool: String,
        _arguments: Value,
    ) -> ProviderResult<McpResult> {
        Err(ProviderError::Unsupported("Claude MCP Apps tool execution"))
    }
    async fn read_resource(
        &self,
        _thread: OrchestrationV2ProviderThread,
        _server: String,
        _uri: String,
    ) -> ProviderResult<Vec<Value>> {
        Err(ProviderError::Unsupported("Claude MCP Apps resource read"))
    }
}
pub fn make_user_message(message: &TurnMessage, priority: Option<Priority>) -> UserMessage {
    let mut user = UserMessage::new(Content::Text(message.text.clone()));
    user.uuid = Some(message.message_id.0.clone());
    user.priority = priority;
    user
}

pub fn claude_capabilities() -> ProviderResult<OrchestrationV2ProviderCapabilities> {
    decode(serde_json::json!({
        "sessions": {
            "supportsMultipleProviderThreadsPerSession": false,
            "supportsModelSwitchInSession": true,
            "supportsProviderSwitchingViaHandoff": true,
            "supportsRuntimeModeSwitchInSession": false,
            "pendingRequestsSurviveRestart": false
        },
        "threads": {
            "canCreateEmptyThread": true,
            "canReadThreadSnapshot": false,
            "canRollbackThread": true,
            "canForkThread": true,
            "canForkFromTurn": true,
            "canForkFromSubagentThread": false,
            "exposesNativeThreadId": true
        },
        "turns": {
            "exposesNativeTurnId": false,
            "emitsTurnStarted": true,
            "emitsTurnCompleted": true,
            "supportsInterrupt": true,
            "supportsActiveSteering": true,
            "activeSteeringInterruptsTools": true,
            "supportsSteeringByInterruptRestart": false,
            "supportsQueuedMessages": true,
            "terminalStatusQuality": "strong"
        },
        "streaming": {
            "streamsAssistantText": true,
            "streamsReasoning": true,
            "streamsToolOutput": false,
            "streamsPlanText": false,
            "emitsMessageCompleted": true
        },
        "tools": {
            "exposesToolItemIds": true,
            "emitsToolStarted": true,
            "emitsToolCompleted": true,
            "emitsToolOutput": true,
            "supportsMcpTools": true,
            "supportsDynamicToolCallbacks": true
        },
        "approvals": {
            "supportsCommandApproval": true,
            "supportsFileReadApproval": true,
            "supportsFileChangeApproval": true,
            "supportsApplyPatchApproval": false,
            "approvalsHaveNativeRequestIds": true,
            "approvalCallbacksAreLiveOnly": true,
            "approvalsCanOriginateFromSubagents": false
        },
        "planning": {
            "emitsPlanUpdated": true,
            "emitsTodoList": true,
            "emitsProposedPlan": true,
            "supportsStructuredQuestions": true,
            "planDeltasHaveItemIds": false
        },
        "subagents": {
            "supportsSubagents": true,
            "exposesSubagentThreadIds": false,
            "emitsSubagentLifecycle": true,
            "canWaitForSubagents": false,
            "canCloseSubagents": false,
            "canForkSubagentThread": false
        },
        "context": {
            "acceptsSystemContext": true,
            "acceptsDeveloperContext": true,
            "acceptsSyntheticUserContext": true,
            "canGenerateSummaries": true,
            "canConsumeHandoffSummaries": true,
            "supportsDeltaHandoff": true,
            "supportsFullThreadHandoff": true,
            "maxRecommendedHandoffChars": null
        },
        "checkpointing": {
            "appCanCheckpointFilesystem": true,
            "supportsNestedCheckpointScopes": true,
            "providerCanRollbackConversation": true,
            "providerRollbackReturnsSnapshot": true,
            "providerCanReadConversationSnapshot": false
        },
        "identity": {
            "nativeThreadIds": "strong",
            "nativeTurnIds": "weak",
            "nativeItemIds": "strong",
            "nativeRequestIds": "strong"
        },
        "runtimePolicy": {
            "enforcement": "native"
        }
    }))
}
