#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2::{self as mapping, Mutation, Projection};
use crate::{
    api::{Clock, IdGenerator},
    provider::*,
};
use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use t3_codex_app_server::{self as native};
use t3_contracts::{
    base_schemas::*,
    model_selection::ModelSelection,
    orchestration_v2::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const CODEX_STOP_CONFIRM_TIMEOUT_MS: u64 = 10_000;

#[derive(Clone, Debug)]
pub struct ResolvedCodexAttachment {
    pub path: PathBuf,
    pub extracted_text_path: Option<PathBuf>,
    pub image_data_url: Option<String>,
}

// Local host seam until attachment/document services are wired into runtime composition.
#[async_trait]
pub trait CodexAttachmentResolver: Send + Sync {
    /// Stateful host resolvers supply a fresh owner for each native session.
    fn session_resolver(&self) -> ProviderResult<Option<Arc<dyn CodexAttachmentResolver>>> {
        Ok(None)
    }

    async fn resolve(
        &self,
        attachment: &t3_contracts::chat_attachment::ChatAttachment,
    ) -> ProviderResult<Option<ResolvedCodexAttachment>>;
}

#[derive(Clone, Debug)]
pub struct CodexContinuationRequest {
    pub thread_id: ThreadId,
    pub provider_thread_id: ProviderThreadId,
    pub driver: ProviderDriverKind,
    pub detail: String,
    pub notification: OrchestrationV2Notification,
}

// Local host seam for app-owned wakes and commands whose native owner has exited.
#[async_trait]
pub trait CodexBackgroundHost: Send + Sync {
    async fn offer(&self, request: CodexContinuationRequest) -> ProviderResult<()>;
    async fn has_pending_wake(&self, _thread: &ThreadId) -> ProviderResult<bool> {
        Ok(false)
    }
    async fn terminate_untracked_command(
        &self,
        _native_thread: &str,
        _process_id: &str,
    ) -> ProviderResult<bool> {
        Ok(false)
    }
}

#[derive(Default)]
pub struct CodexHostTurnContext {
    pub additional_context: BTreeMap<String, native::AdditionalContextEntry>,
    pub cyber_access: Option<Option<String>>,
    pub omit_service_tier: bool,
}

// Local host seam for shared provider instructions, MCP availability and model metadata.
#[async_trait]
pub trait CodexTurnContextProvider: Send + Sync {
    async fn context(&self, input: &TurnInput) -> ProviderResult<CodexHostTurnContext>;
}

// Local host seam for the authoritative shared Codex allowance conversion helpers.
pub trait CodexUsagePolicy: Send + Sync {
    fn merge(&self, previous: Option<&Value>, update: &Value) -> Option<Value>;
    fn reset_at(&self, snapshot: &Value) -> Option<String>;
    fn updated(&self, previous: Option<&Value>, update: &Value, checked_at: &IsoDateTime);
}

pub struct CodexMcpAppCaptureInput {
    pub thread_id: ThreadId,
    pub server: String,
    pub tool: String,
    pub resource_uri: String,
    pub contents: Vec<Value>,
}

pub struct CodexCapturedMcpApp {
    reference: Value,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}
impl CodexCapturedMcpApp {
    pub fn new(reference: Value, cleanup: impl FnOnce() + Send + 'static) -> Self {
        Self {
            reference,
            cleanup: Some(Box::new(cleanup)),
        }
    }
    pub(crate) fn commit(mut self) -> Value {
        self.cleanup = None;
        std::mem::take(&mut self.reference)
    }
}
impl Drop for CodexCapturedMcpApp {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

// Local host seam for CSP-sanitized MCP app snapshots in the authoritative attachment store.
#[async_trait]
pub trait CodexMcpAppHost: Send + Sync {
    async fn capture(
        &self,
        input: CodexMcpAppCaptureInput,
    ) -> ProviderResult<Option<CodexCapturedMcpApp>>;
}

pub type GeneratedImageMarkdown = Arc<dyn Fn(&str, &[String]) -> String + Send + Sync>;

pub type CodexAgentTracker = Arc<dyn Fn() -> Box<dyn FnOnce() + Send> + Send + Sync>;
struct AgentLease(Option<Box<dyn FnOnce() + Send>>);
impl Drop for AgentLease {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            release();
        }
    }
}

pub type VersionGate = Arc<dyn Fn(&str) -> ProviderResult<()> + Send + Sync>;

#[derive(Clone)]
enum ProcessExit {
    Exited(Option<i32>),
    Failed(String),
    Stopped,
}

pub(crate) struct ProcessDiagnostics {
    stderr: Mutex<Vec<u8>>,
    exit: watch::Sender<Option<ProcessExit>>,
    stderr_done: watch::Sender<bool>,
}

impl ProcessDiagnostics {
    fn new() -> Self {
        Self {
            stderr: Mutex::new(vec![]),
            exit: watch::channel(None).0,
            stderr_done: watch::channel(false).0,
        }
    }

    pub(crate) async fn enrich(&self, error: ProviderError) -> ProviderError {
        let mut exit = self.exit.subscribe();
        let _ = tokio::time::timeout(Duration::from_millis(250), async {
            while exit.borrow_and_update().is_none() {
                if exit.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        let observed = exit.borrow().clone();
        let Some(observed) = observed else {
            return error;
        };
        let mut stderr_done = self.stderr_done.subscribe();
        let _ = tokio::time::timeout(Duration::from_millis(250), async {
            while !*stderr_done.borrow_and_update() {
                if stderr_done.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        let tail = String::from_utf8_lossy(&self.stderr.lock().await).into_owned();
        let tail = clean_stderr(&tail);
        let line = tail
            .lines()
            .rev()
            .find(|line| {
                let line = line.to_lowercase();
                line.contains("fatal") || line.contains("error")
            })
            .or_else(|| tail.lines().rev().find(|line| !line.trim().is_empty()))
            .unwrap_or_default();
        let excerpt = line.chars().take(512).collect::<String>();
        let status = match observed {
            ProcessExit::Exited(code) => format!("Codex app-server exited ({code:?})"),
            ProcessExit::Failed(detail) => format!("Codex app-server exit failed: {detail}"),
            ProcessExit::Stopped => "Codex app-server stopped".into(),
        };
        operation(
            ProviderOperation::Stream,
            if excerpt.is_empty() {
                format!("{status}: {error}")
            } else {
                format!("{status}: {error}; stderr: {excerpt}")
            },
        )
    }
}

fn clean_stderr(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut clean = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for ch in chars.by_ref() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '^' | '_') => {
                    while let Some(ch) = chars.next() {
                        if ch == '\u{7}' {
                            break;
                        }
                        if ch == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if !ch.is_control() || matches!(ch, '\n' | '\r' | '\t') {
            clean.push(ch);
        }
    }
    clean
}

struct ProcessScope {
    cancellation: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    diagnostics: Arc<ProcessDiagnostics>,
}

impl Drop for ProcessScope {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl ProcessScope {
    fn start(
        mut process: t3_process::SpawnedProcess,
        stderr: Option<tokio::process::ChildStderr>,
        parent: &CancellationToken,
        agent: AgentLease,
    ) -> Arc<Self> {
        let cancellation = parent.child_token();
        let stop = cancellation.clone();
        let diagnostics = Arc::new(ProcessDiagnostics::new());
        let observed = diagnostics.clone();
        let task = tokio::spawn(async move {
            let _agent = agent;
            let tail = observed.clone();
            let mut drain = tokio::spawn(async move {
                if let Some(mut stderr) = stderr {
                    let mut buffer = [0; 4096];
                    while let Ok(count) = stderr.read(&mut buffer).await {
                        if count == 0 {
                            break;
                        }
                        let mut bytes = tail.stderr.lock().await;
                        bytes.extend_from_slice(&buffer[..count]);
                        if bytes.len() > 32 * 1024 {
                            let excess = bytes.len() - 32 * 1024;
                            bytes.drain(..excess);
                        }
                    }
                }
                tail.stderr_done.send_replace(true);
            });
            let status = tokio::select! {
                _=stop.cancelled()=>{let _=process.terminate(Duration::from_secs(2)).await;ProcessExit::Stopped},
                result=process.wait()=>match result {Ok(status)=>ProcessExit::Exited(status.code()),Err(error)=>ProcessExit::Failed(error.to_string())},
            };
            observed.exit.send_replace(Some(status));
            if tokio::time::timeout(Duration::from_millis(250), &mut drain)
                .await
                .is_err()
            {
                drain.abort();
                let _ = drain.await;
            }
            observed.stderr_done.send_replace(true);
        });
        Arc::new(Self {
            cancellation,
            task: Mutex::new(Some(task)),
            diagnostics,
        })
    }

    async fn close(&self) {
        self.cancellation.cancel();
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

#[derive(Clone)]
pub struct McpServerInjection {
    pub endpoint: String,
    pub authorization_header: String,
}

#[derive(Clone)]
pub struct CodexAdapterOptions {
    pub instance_id: ProviderInstanceId,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub process: t3_process::ProcessOptions,
    pub client_version: String,
    pub version_gate: Option<VersionGate>,
    pub track_agent: Option<CodexAgentTracker>,
    pub mcp: BTreeMap<String, McpServerInjection>,
    pub event_capacity: usize,
    pub attachments: Option<Arc<dyn CodexAttachmentResolver>>,
    pub background: Option<Arc<dyn CodexBackgroundHost>>,
    pub turn_context: Option<Arc<dyn CodexTurnContextProvider>>,
    pub usage_policy: Option<Arc<dyn CodexUsagePolicy>>,
    pub generated_image_markdown: Option<GeneratedImageMarkdown>,
    pub mcp_apps: Option<Arc<dyn CodexMcpAppHost>>,
    /// The thread's T3 MCP server, read at thread start, resume and fork; `mcp` is the static
    /// fallback.
    pub t3_mcp: Option<Arc<dyn crate::provider::T3McpSessions>>,
}

impl CodexAdapterOptions {
    pub fn new(
        instance_id: ProviderInstanceId,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        let mut process = t3_process::ProcessOptions::new("codex").args([
            "app-server",
            "-c",
            "thread_unload_delay_secs=0",
        ]);
        process.timeout = None;
        process.env.insert(
            "CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into(),
            Some("T3 Code".into()),
        );
        Self {
            instance_id,
            clock,
            ids,
            process,
            client_version: env!("CARGO_PKG_VERSION").into(),
            version_gate: None,
            track_agent: None,
            mcp: BTreeMap::new(),
            event_capacity: 128,
            attachments: None,
            background: None,
            turn_context: None,
            usage_policy: None,
            generated_image_markdown: None,
            mcp_apps: None,
            t3_mcp: None,
        }
    }
}

pub fn codex_registration(
    options: CodexAdapterOptions,
    continuation_key: String,
    enabled: bool,
    credential_access: Vec<
        Arc<crate::ports::sessions_recovery::provider_adapter_registry::CredentialAccess>,
    >,
) -> crate::ports::sessions_recovery::provider_adapter_registry::AdapterRegistration {
    crate::ports::sessions_recovery::provider_adapter_registry::AdapterRegistration {
        adapter: Arc::new(CodexAdapterV2::new(options)),
        continuation_key,
        enabled,
        credential_access,
    }
}

pub struct CodexAdapterV2 {
    options: CodexAdapterOptions,
}

impl CodexAdapterV2 {
    pub fn new(options: CodexAdapterOptions) -> Self {
        Self { options }
    }

    /// Connects an already-owned transport; useful for hosts and recorded protocol replay.
    pub async fn open_connected(
        &self,
        input: OpenSession,
        client: native::Client,
        events: native::Events,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<CodexSessionRuntime>> {
        CodexSessionRuntime::connect(
            self.options.clone(),
            input,
            client,
            events,
            cancellation,
            None,
        )
        .await
    }
}

#[async_trait]
impl ProviderAdapter for CodexAdapterV2 {
    // TS's status declares no `supportedRuntimeModes`: every mode.
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
    }
    async fn open_session_with_policy(
        &self,
        mut input: OpenSession,
        options: RuntimePolicyOptions,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        if mapping::string_option(&input.model_selection, "reasoningEffort").is_none()
            && let Some(effort) = options.reasoning_effort
        {
            input
                .model_selection
                .options
                .get_or_insert_with(|| t3_contracts::model::ProviderOptionSelections(vec![]))
                .0
                .push(t3_contracts::model::ProviderOptionSelection {
                    id: "reasoningEffort".into(),
                    value: t3_contracts::model::ProviderOptionSelectionValue::String(effort),
                });
        }
        self.open_session(input, cancellation).await
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.options.instance_id
    }
    fn driver(&self) -> ProviderDriverKind {
        ProviderDriverKind("codex".into())
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        mapping::capabilities()
    }
    async fn plan_selection_transition(
        &self,
        _: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        Ok(SelectionTransitionPlan::ApplyOnNextTurn)
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        if cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let mut options = self.options.process.clone();
        // A session serves several workspaces; only per-thread RPCs carry cwd.
        options.cwd = None;
        options.spawn_cwd = None;
        options.cancellation = cancellation.child_token();
        let mut process =
            t3_process::spawn(&options).map_err(|e| operation(ProviderOperation::Open, e))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| protocol("Codex stdout is not piped"))?;
        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| protocol("Codex stdin is not piped"))?;
        let stderr = process.stderr.take();
        let (client, events) =
            native::Client::connect(stdout, stdin, native::ClientOptions::default());
        let agent = AgentLease(self.options.track_agent.as_ref().map(|track| track()));
        let process = ProcessScope::start(process, stderr, &cancellation, agent);
        let runtime = CodexSessionRuntime::connect(
            self.options.clone(),
            input,
            client,
            events,
            cancellation,
            Some(process),
        )
        .await?;
        Ok(runtime)
    }
}

pub struct CodexSessionRuntime {
    session: OrchestrationV2ProviderSession,
    options: CodexAdapterOptions,
    input: OpenSession,
    client: native::Client,
    control: mpsc::Sender<Mutation>,
    receiver: Mutex<Option<mpsc::Receiver<ProviderResult<ProviderEvent>>>>,
    cancellation: CancellationToken,
    worker: Mutex<Option<JoinHandle<()>>>,
    process: Option<Arc<ProcessScope>>,
    // Native turn creation and thread loading have ambiguous delivery on cancellation.
    operation_lane: Mutex<()>,
}

impl Drop for CodexSessionRuntime {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl CodexSessionRuntime {
    async fn connect(
        mut options: CodexAdapterOptions,
        input: OpenSession,
        client: native::Client,
        events: native::Events,
        parent: CancellationToken,
        process: Option<Arc<ProcessScope>>,
    ) -> ProviderResult<Arc<Self>> {
        if options.event_capacity == 0 {
            return Err(protocol("Event capacity must be positive"));
        }
        if input.model_selection.instance_id != options.instance_id {
            return Err(protocol(
                "Codex selection belongs to another provider instance",
            ));
        }
        if let Some(resolver) = &options.attachments
            && let Some(session) = resolver.session_resolver()?
        {
            options.attachments = Some(session);
        }
        let session = mapping::decode(json!({
            "id":input.provider_session_id,"driver":"codex","providerInstanceId":options.instance_id,
            "status":"ready","cwd": input.runtime_policy.cwd.as_ref().map(|p|p.to_string_lossy().into_owned()).unwrap_or_else(||".".into()),
            "model":input.model_selection.model,"capabilities":mapping::capabilities()?,
            "createdAt":options.clock.now(),"updatedAt":options.clock.now(),"lastError":null
        }))?;
        let cancellation = process.as_ref().map_or_else(
            || parent.child_token(),
            |process| process.cancellation.clone(),
        );
        let (sender, receiver) = mpsc::channel(options.event_capacity);
        let (control, commands) = mpsc::channel(32);
        let mut projection = Projection::new(
            options.clock.clone(),
            options.ids.clone(),
            input.clone(),
            options.instance_id.clone(),
        );
        projection.enable_background(options.background.is_some());
        projection.set_usage_policy(options.usage_policy.clone());
        projection.set_image_markdown(options.generated_image_markdown.clone());
        projection.enable_mcp_apps(options.mcp_apps.is_some());
        let stop = cancellation.clone();
        let diagnostics = process.as_ref().map(|process| process.diagnostics.clone());
        let worker = tokio::spawn(mapping::run_runtime(
            projection,
            events,
            commands,
            sender,
            stop,
            mapping::RuntimeIo {
                client: Some(client.clone()),
                diagnostics: diagnostics.clone(),
                background: options.background.clone(),
                mcp_apps: options.mcp_apps.clone(),
            },
        ));
        let runtime = Arc::new(Self {
            session,
            options,
            input,
            client,
            control,
            receiver: Mutex::new(Some(receiver)),
            cancellation,
            worker: Mutex::new(Some(worker)),
            process,
            operation_lane: Mutex::new(()),
        });
        let handshake = async {
            let initialized = runtime
                .client
                .initialize(&native::InitializeParams::t3(
                    &runtime.options.client_version,
                ))
                .await
                .map_err(native_error)?;
            if let (Some(gate), Some(response)) = (&runtime.options.version_gate, initialized)
                && let Some(version) = response
                    .user_agent
                    .split('/')
                    .nth(1)
                    .and_then(|v| v.split_whitespace().next())
            {
                gate(version)?;
            }
            runtime.client.initialized().await.map_err(native_error)
        };
        let result = tokio::select! { _ = runtime.cancellation.cancelled() => Err(ProviderError::Cancelled), result=handshake => result };
        if let Err(error) = result {
            let error = if let Some(diagnostics) = diagnostics {
                diagnostics.enrich(error).await
            } else {
                error
            };
            let _ = runtime.close().await;
            return Err(error);
        }
        Ok(runtime)
    }

    pub async fn list_models(&self) -> ProviderResult<Vec<native::Model>> {
        let mut result = vec![];
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(cursor.clone()) {
                return Err(protocol("Model pagination repeated a cursor"));
            }
            let page = self
                .client
                .model_list(&native::ModelListParams {
                    cursor: cursor
                        .clone()
                        .map_or(native::Field::Null, native::Field::Value),
                    include_hidden: native::Field::Value(true),
                    ..Default::default()
                })
                .await
                .map_err(native_error)?;
            result.extend(page.data);
            cursor = field_option(page.next_cursor);
            if cursor.is_none() {
                return Ok(result);
            }
        }
    }

    pub(crate) async fn mutate<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Projection) -> ProviderResult<(T, Vec<ProviderEvent>)> + Send + 'static,
    ) -> ProviderResult<T> {
        let (send, receive) = oneshot::channel();
        let mutation: Mutation = Box::new(move |projection| match f(projection) {
            Ok((value, events)) => (
                events,
                Box::new(move || {
                    let _ = send.send(Ok(value));
                }),
            ),
            Err(error) => (
                vec![],
                Box::new(move || {
                    let _ = send.send(Err(error));
                }),
            ),
        });
        tokio::select! { _=self.cancellation.cancelled()=>return Err(ProviderError::SessionStopped),result=self.control.send(mutation)=>result.map_err(|_|ProviderError::SessionStopped)? }
        tokio::select! { biased; result=receive=>result.map_err(|_|ProviderError::SessionStopped)?, _=self.cancellation.cancelled()=>Err(ProviderError::SessionStopped) }
    }

    async fn rpc<P: Serialize + Sync, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> ProviderResult<R> {
        tokio::select! { _=self.cancellation.cancelled()=>Err(ProviderError::SessionStopped), result=self.client.request(method,params)=>result.map_err(native_error) }
    }

    fn thread_params(
        &self,
        thread: Option<&ThreadId>,
        model: Option<&ModelSelection>,
        policy: Option<&RuntimePolicy>,
    ) -> Value {
        let mut params = json!({"config": {
            "tools.update_plan.enabled":true,"mcp_optional_startup_grace_ms":10000,"features.concurrent_reasoning_summaries":true
        }});
        if let Some(thread) = thread {
            params["config"]["shell_environment_policy.set.T3CODE_THREAD_ID"] = json!(thread);
            let live = self
                .options
                .t3_mcp
                .as_ref()
                .and_then(|sessions| sessions.read(thread))
                .map(|session| McpServerInjection {
                    endpoint: session.endpoint,
                    authorization_header: session.authorization_header,
                });
            if let Some(mcp) = live.as_ref().or_else(|| self.options.mcp.get(&thread.0)) {
                let mut server = json!({"url":mcp.endpoint,"http_headers":{"Authorization":mcp.authorization_header},"tool_timeout_sec":3900});
                server["disabled_tools"] = json!(["t3_thread_wait"]);
                params["config"]["mcp_servers.t3-code"] = server.clone();
                server.as_object_mut().map(|o| o.remove("disabled_tools"));
                server["enabled_tools"] = json!(["t3_thread_wait"]);
                server["omit_tools_from"] = json!(["code_mode", "deferred"]);
                params["config"]["mcp_servers.t3-code-direct"] = server;
            }
        }
        if let Some(model) = model {
            params["model"] = json!(model.model);
        }
        if let Some(cwd) = policy.and_then(|p| p.cwd.as_ref()) {
            params["cwd"] = json!(cwd.to_string_lossy());
        }
        params
    }

    async fn start_native(
        &self,
        input: TurnInput,
        options: RuntimePolicyOptions,
        native_input: Vec<Value>,
    ) -> ProviderResult<()> {
        if input.model_selection.instance_id != self.options.instance_id {
            return Err(protocol(
                "Codex selection belongs to another provider instance",
            ));
        }
        let thread = native_thread(&input.provider_thread)?.to_owned();
        let mut params =
            mapping::build_turn_start_params_with_input(&input, &options, native_input)?;
        if let Some(provider) = &self.options.turn_context {
            let context = provider.context(&input).await?;
            if context.omit_service_tier {
                params
                    .as_object_mut()
                    .map(|params| params.shift_remove("serviceTier"));
            }
            if let Some(default) = context.cyber_access
                && let Some(program) =
                    mapping::string_option(&input.model_selection, "cyberAccessProgram")
                        .filter(|program| {
                            matches!(*program, "standard" | "daybreakBlue" | "daybreakRed")
                        })
                        .or(default.as_deref().filter(|program| {
                            matches!(*program, "standard" | "daybreakBlue" | "daybreakRed")
                        }))
            {
                params["cyberAccessProgram"] = json!(program);
            }
            if !context.additional_context.is_empty() {
                let mut merged = serde_json::to_value(context.additional_context)
                    .map_err(|e| protocol(e.to_string()))?;
                if let Some(app) = params["additionalContext"].as_object() {
                    for (key, value) in app {
                        merged[key] = value.clone();
                    }
                }
                params["additionalContext"] = merged;
            }
        }
        let config:ProviderResult<Value>=self.rpc("config/read",&json!({"cwd":input.runtime_policy.cwd.as_ref().map(|p|p.to_string_lossy()),"includeLayers":false})).await;
        if let Ok(config) = config
            && let Some(summary) = config["config"]
                .get("model_reasoning_summary")
                .filter(|s| s.is_string())
        {
            params["summary"] = summary.clone();
        }
        let pending = input.clone();
        let remembered = params.clone();
        let receipt = self
            .mutate(move |state| state.prepare_start_tracked(pending, remembered))
            .await?;
        let response: ProviderResult<native::TurnResponse> = self.rpc("turn/start", &params).await;
        match response {
            Ok(response) => {
                if let Some(started) = receipt.borrow().as_ref() {
                    return if *started == response.turn.id {
                        Ok(())
                    } else {
                        Err(protocol(
                            "Codex returned a turn belonging to an earlier run",
                        ))
                    };
                }
                let turn_id = response.turn.id.clone();
                let result = self
                    .mutate(move |state| state.bind_start(&thread, &response.turn))
                    .await;
                if matches!(result, Err(ProviderError::SessionStopped))
                    && receipt.borrow().as_ref() == Some(&turn_id)
                {
                    return Ok(());
                }
                result
            }
            Err(error) => {
                self.mutate(move |state| {
                    state.cancel_start(&thread);
                    Ok(((), vec![]))
                })
                .await?;
                Err(error)
            }
        }
    }
    async fn update_goal(&self, thread: String, goal: Value) -> ProviderResult<()> {
        self.mutate(move |state| Ok(((), state.set_goal(&thread, goal)?)))
            .await
    }
    async fn activate_started_goal(&self, thread: &str) -> ProviderResult<()> {
        let target = thread.to_owned();
        let stopped = self
            .mutate(move |state| Ok((state.goal_activation_stopped(&target), vec![])))
            .await?;
        if !stopped {
            let response: Value = self
                .rpc(
                    "thread/goal/set",
                    &json!({"threadId":thread,"status":"active"}),
                )
                .await?;
            self.update_goal(thread.to_owned(), response["goal"].clone())
                .await?;
            let target = thread.to_owned();
            let stopped = self
                .mutate(move |state| Ok((state.goal_activation_stopped(&target), vec![])))
                .await?;
            if stopped {
                let response: Value = self
                    .rpc(
                        "thread/goal/set",
                        &json!({"threadId":thread,"status":"paused"}),
                    )
                    .await?;
                self.update_goal(thread.to_owned(), response["goal"].clone())
                    .await?;
            }
        }
        Ok(())
    }
    async fn run_goal_command(
        &self,
        input: TurnInput,
        options: RuntimePolicyOptions,
        command: mapping::GoalCommand,
    ) -> ProviderResult<()> {
        use mapping::GoalCommand;
        if let GoalCommand::Set(objective) = &command
            && objective.chars().count() > 4000
        {
            return Err(protocol(format!(
                "The goal objective is {} characters; Codex supports at most 4000. Shorten it and send /goal again. The current goal was not changed.",
                objective.chars().count()
            )));
        }
        let thread = native_thread(&input.provider_thread)?.to_owned();
        let remembered = input.provider_thread.clone();
        self.mutate(move |state| {
            state.remember_thread(remembered)?;
            Ok(((), vec![]))
        })
        .await?;
        let current: Value = self
            .rpc("thread/goal/get", &json!({"threadId":thread}))
            .await?;
        let current = mapping::provider_goal(current["goal"].clone());
        self.update_goal(thread.clone(), current.clone()).await?;
        let reply = match &command {
            GoalCommand::Show => Some(if current.is_null() {
                "No goal is set.".into()
            } else {
                format!(
                    "Goal {}: {}",
                    current["status"]
                        .as_str()
                        .unwrap_or_default()
                        .replace('_', " "),
                    current["objective"].as_str().unwrap_or_default()
                )
            }),
            GoalCommand::Clear => {
                let response: Value = self
                    .rpc("thread/goal/clear", &json!({"threadId":thread}))
                    .await?;
                self.update_goal(thread.clone(), Value::Null).await?;
                if response["cleared"] == true {
                    None
                } else {
                    Some("No goal is set.".into())
                }
            }
            GoalCommand::Pause if !current.is_null() => {
                let response: Value = self
                    .rpc(
                        "thread/goal/set",
                        &json!({"threadId":thread,"status":"paused"}),
                    )
                    .await?;
                self.update_goal(thread.clone(), response["goal"].clone())
                    .await?;
                None
            }
            GoalCommand::Resume | GoalCommand::Pause if current.is_null() => {
                Some("No goal is set.".into())
            }
            _ => {
                let activation_thread = thread.clone();
                let hold = matches!(command, GoalCommand::Set(_));
                self.mutate(move |state| {
                    if hold {
                        state.hold_goal_updates(activation_thread.clone());
                    }
                    state.begin_goal_activation(activation_thread);
                    Ok(((), vec![]))
                })
                .await?;
                let result=async {
                    let native_input=if let GoalCommand::Set(objective)=&command {
                        if !current.is_null(){let _:Value=self.rpc("thread/goal/clear",&json!({"threadId":thread})).await?;}
                        let response:Value=self.rpc("thread/goal/set",&json!({"threadId":thread,"objective":objective,"status":"paused"})).await?;self.update_goal(thread.clone(),response["goal"].clone()).await?;
                        let mut message=input.message.clone();message.text=format!("This thread's goal is already set. Do not call create_goal. Start working on it now: {objective}");self.message_input(&message).await?
                    }else{vec![]};
                    self.start_native(input.clone(),options,native_input).await?;
                    self.activate_started_goal(&thread).await?;
                    Ok(())
                }.await;
                let target = thread.clone();
                self.mutate(move |state| state.finish_goal_activation(&target))
                    .await?;
                self.mutate(move |state| state.release_goal_updates(&thread))
                    .await?;
                return result;
            }
        };
        self.mutate(move |state| state.complete_goal_command(input, reply))
            .await
    }
    async fn terminate_background_process(
        &self,
        thread: &str,
        process: &str,
    ) -> ProviderResult<()> {
        let confirmed = tokio::select! {
            _ = self.cancellation.cancelled() => return Err(ProviderError::SessionStopped),
            result = tokio::time::timeout(Duration::from_secs(10), terminate_native_background_process(&self.client, thread, process, self.options.background.as_ref())) => result.map_err(|_|protocol("Codex background terminal termination timed out"))??,
        };
        if !confirmed {
            return Err(protocol(
                "Codex background terminal remained active after termination",
            ));
        }
        let thread = thread.to_owned();
        let process = process.to_owned();
        match self
            .mutate(move |state| state.confirm_command_stopped(&thread, &process))
            .await
        {
            Ok(()) | Err(ProviderError::SessionStopped) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn message_input(&self, message: &TurnMessage) -> ProviderResult<Vec<Value>> {
        let mut resolved = Vec::with_capacity(message.attachments.len());
        for attachment in &message.attachments {
            let result = match &self.options.attachments {
                Some(resolver) => {
                    tokio::select! {_=self.cancellation.cancelled()=>return Err(ProviderError::Cancelled),result=resolver.resolve(attachment)=>result?}
                }
                None => None,
            };
            resolved.push(result);
        }
        mapping::message_input_resolved(message, &resolved)
    }

    async fn resume_native(
        &self,
        params: Value,
        thread: &OrchestrationV2ProviderThread,
    ) -> ProviderResult<Value> {
        match self.rpc("thread/resume", &params).await {
            Ok(value) => Ok(value),
            Err(error) if archived(&error) => {
                let _: Value = self
                    .rpc(
                        "thread/unarchive",
                        &json!({"threadId":native_thread(thread)?}),
                    )
                    .await?;
                self.rpc("thread/resume", &params).await
            }
            Err(error) if error.to_string().contains(" is closing") => {
                let target = native_thread(thread)?.to_owned();
                let closing = self
                    .mutate(move |state| state.wait_thread_closed(&target))
                    .await?;
                if let Some(mut closing) = closing {
                    let _ = tokio::time::timeout(Duration::from_secs(10), async {
                        while !*closing.borrow_and_update() {
                            if closing.changed().await.is_err() {
                                break;
                            }
                        }
                    })
                    .await;
                }
                self.rpc("thread/resume", &params).await
            }
            Err(error) if active_writer(&error) => Err(ProviderError::ThreadInUse(
                mapping::expanded_id(&thread.id)?,
            )),
            Err(error) => Err(error),
        }
    }

    /// Reverts the newest `count` native turns, stopping short of `stop_at`: a retry after a
    /// revert T3 did not record finds the target among the newest turns and removes nothing more
    /// (re-review 4 F7).
    async fn revert(
        &self,
        thread: &str,
        count: usize,
        stop_at: Option<&str>,
    ) -> ProviderResult<Value> {
        let mut remaining = count;
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        let mut before = None;
        while remaining > 0 {
            if !seen.insert(cursor.clone()) {
                return Err(protocol("Thread history pagination repeated a cursor"));
            }
            let page:native::ThreadTurnsListResponse=self.rpc("thread/turns/list", &json!({"threadId":thread,"cursor":cursor,"limit":remaining.min(100),"sortDirection":"desc","itemsView":"summary"})).await?;
            for turn in page.data {
                if stop_at == Some(turn.id.as_str()) {
                    remaining = 0;
                    break;
                }
                before = Some(turn.id);
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            cursor = field_option(page.next_cursor);
            if cursor.is_none() {
                break;
            }
        }
        if let Some(before) = before {
            self.rpc(
                "thread/revert",
                &json!({"threadId":thread,"beforeTurnId":before}),
            )
            .await
        } else {
            self.rpc(
                "thread/read",
                &json!({"threadId":thread,"includeTurns":false}),
            )
            .await
        }
    }

    async fn snapshot_result(
        &self,
        mut thread: OrchestrationV2ProviderThread,
        value: Option<Value>,
        head: Option<OrchestrationV2ProviderRef>,
    ) -> ProviderResult<ThreadSnapshot> {
        thread.native_conversation_head_ref = head;
        thread.status = ProviderThreadStatus::Idle;
        if let Some(payload) = value.as_ref() {
            if let Some(id) = payload["thread"]["id"].as_str() {
                thread.native_thread_ref = Some(mapping::decode(mapping::native_ref(id))?);
            }
            thread.updated_at = mapping::decode(mapping::timestamp(
                payload["thread"].get("updatedAt"),
                self.options.clock.as_ref(),
            ))?;
        }
        Ok(ThreadSnapshot {
            provider_thread: thread,
            provider_turns: vec![],
            messages: vec![],
            reasoning_message_ids: None,
            runtime_requests: vec![],
            provider_payload: value.map(|v| v["thread"].clone()),
        })
    }
}

#[async_trait]
impl SessionRuntime for CodexSessionRuntime {
    async fn ensure_thread_with_policy(
        &self,
        input: EnsureThread,
        _: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.ensure_thread(input).await
    }
    async fn resume_thread_with_policy(
        &self,
        input: ResumeThread,
        _: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.resume_thread(input).await
    }
    async fn fork_with_policy(
        &self,
        input: ForkInput,
        _: RuntimePolicyOptions,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.fork(input).await
    }
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
        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or(ProviderError::SubscriptionClosed)?;
        Ok(EventSubscription {
            events: receiver,
            cancellation: self.cancellation.clone(),
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.cancellation.cancel();
        let _ = self.client.shutdown().await;
        if let Some(task) = self.worker.lock().await.take() {
            let _ = task.await;
        }
        if let Some(process) = &self.process {
            process.close().await;
        }
        Ok(())
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let _lane = self.operation_lane.lock().await;
        let params = self.thread_params(
            Some(&input.thread_id),
            Some(&input.model_selection),
            Some(&input.runtime_policy),
        );
        let response: Value = self.rpc("thread/start", &params).await?;
        let thread = mapping::provider_thread(
            &response["thread"],
            &self.session,
            Some(&input.thread_id),
            None,
            None,
            self.options.clock.as_ref(),
        )?;
        self.mutate(move |state| {
            state.remember_thread(thread.clone())?;
            Ok((thread, vec![]))
        })
        .await
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let _lane = self.operation_lane.lock().await;
        let mut params = self.thread_params(
            input.thread_id.as_ref().or_else(|| {
                input
                    .provider_thread
                    .app_thread_id
                    .as_ref()
                    .and_then(mapping::string_id_ref)
            }),
            input.model_selection.as_ref(),
            input.runtime_policy.as_ref(),
        );
        params["threadId"] = json!(native_thread(&input.provider_thread)?);
        params["excludeTurns"] = json!(true);
        let response = self.resume_native(params, &input.provider_thread).await?;
        let mut thread = input.provider_thread;
        thread.provider_session_id = Some(mapping::decode(json!(self.input.provider_session_id))?);
        thread.provider_instance_id = mapping::decode(json!(self.options.instance_id))?;
        thread.status = ProviderThreadStatus::Idle;
        thread.native_thread_ref = Some(mapping::decode(mapping::native_ref(
            response["thread"]["id"]
                .as_str()
                .ok_or_else(|| protocol("Resume response has no thread id"))?,
        ))?);
        thread.updated_at = mapping::decode(mapping::timestamp(
            response["thread"].get("updatedAt"),
            self.options.clock.as_ref(),
        ))?;
        self.mutate(move |state| {
            state.remember_thread(thread.clone())?;
            Ok((thread, vec![]))
        })
        .await
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        self.start_turn_with_policy(input, RuntimePolicyOptions::default())
            .await
    }
    async fn start_turn_with_policy(
        &self,
        input: TurnInput,
        options: RuntimePolicyOptions,
    ) -> ProviderResult<()> {
        let _lane = self.operation_lane.lock().await;
        if let Some(command) = mapping::goal_command(&input.message) {
            return self.run_goal_command(input, options, command).await;
        }
        let native_input = self.message_input(&input.message).await?;
        let thread = native_thread(&input.provider_thread)?.to_owned();
        let target = thread.clone();
        let activate = self
            .mutate(move |state| state.take_goal_wake_activation(&target))
            .await?;
        let result = self.start_native(input, options, native_input).await;
        if !activate {
            return result;
        }
        let result = match result {
            Ok(()) => self.activate_started_goal(&thread).await,
            Err(error) => Err(error),
        };
        self.mutate(move |state| state.finish_goal_activation(&thread))
            .await?;
        result
    }
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
        let requested = mapping::native_turn_id(&input.provider_turn_id)?;
        let waiter = self
            .mutate(move |state| state.wait_target(&requested))
            .await?;
        let native = tokio::select! {_=self.cancellation.cancelled()=>return Err(ProviderError::Cancelled),result=waiter=>result.map_err(|_|ProviderError::SubscriptionClosed)??};
        let expected = native.clone();
        let message_id = input.message.message_id.clone();
        let thread_id = input.thread_id.clone();
        let run_id = input.run_id.clone();
        let params = json!({"threadId":native_thread(&input.provider_thread)?,"expectedTurnId":native,"input":self.message_input(&input.message).await?});
        self.mutate(move |state| state.prepare_steer(&native, thread_id, run_id, message_id))
            .await?;
        let response: ProviderResult<native::TurnSteerResponse> =
            self.rpc("turn/steer", &params).await;
        match response {
            Ok(response) if response.turn_id == expected => Ok(()),
            Ok(_) => Err(ProviderError::SteerDeclined(input.provider_turn_id)),
            Err(error) => Err(error),
        }
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        let thread = native_thread(&input.provider_thread)?.to_owned();
        let target = input.provider_turn_id.clone();
        let goal_thread = thread.clone();
        let pause = self
            .mutate(move |state| state.prepare_goal_stop(&goal_thread))
            .await?;
        if pause
            && let Ok(Ok(response)) = tokio::time::timeout(
                Duration::from_secs(10),
                self.rpc::<_, Value>(
                    "thread/goal/set",
                    &json!({"threadId":thread,"status":"paused"}),
                ),
            )
            .await
        {
            self.update_goal(thread.clone(), response["goal"].clone())
                .await?;
        }
        let native_thread = thread.clone();
        let (interrupts, tracked, mut confirmation) = self
            .mutate(move |state| {
                let ((interrupts, confirmation), events) =
                    state.prepare_confirmed_interrupt(&target, &native_thread)?;
                let tracked = state
                    .lineage_threads(&native_thread)
                    .into_iter()
                    .flat_map(|thread| {
                        state
                            .tracked_commands(&thread)
                            .into_iter()
                            .map(move |process| (thread.clone(), process))
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                Ok(((interrupts, tracked, confirmation), events))
            })
            .await?;
        let result = async {
            for (thread_id, turn_id) in interrupts {
                let target = turn_id.clone();
                let ready = self
                    .mutate(move |state| state.wait_native_start(&target))
                    .await?;
                match tokio::time::timeout(Duration::from_secs(10), ready).await {
                    Ok(Ok(())) => {}
                    _ => {
                        return Err(protocol(
                            "Codex did not start the queued turn within 10 seconds; Stop could not be delivered.",
                        ));
                    }
                }
                let target = turn_id.clone();
                if !self
                    .mutate(move |state| Ok((state.needs_interrupt(&target), vec![])))
                    .await?
                {
                    continue;
                }
                let _: Value = self
                    .rpc(
                        "turn/interrupt",
                        &json!({"threadId":thread_id,"turnId":turn_id}),
                    )
                    .await?;
            }
            let cleanup = async {
                let mut failure = None;
                for (thread, process) in tracked {
                    let result = self.terminate_background_process(&thread, &process).await;
                    if failure.is_none() {
                        failure = result.err();
                    }
                }
                failure
            };
            let (cleanup_error, confirmed) = tokio::join!(
                cleanup,
                tokio::time::timeout(
                    Duration::from_millis(CODEX_STOP_CONFIRM_TIMEOUT_MS),
                    confirmation.wait()
                )
            );
            match confirmed {
                Ok(result) => result?,
                Err(_) => {
                    let roots = confirmation.roots.clone();
                    let confirmed = self
                        .mutate(move |state| state.settle_interrupt_roots(&roots, false))
                        .await?;
                    if !confirmed {
                        return Err(ProviderError::InterruptTimeout {
                            turn_id: confirmation.turn_id.clone(),
                            timeout_ms: CODEX_STOP_CONFIRM_TIMEOUT_MS,
                        });
                    }
                }
            }
            if let Some(error) = cleanup_error {
                return Err(error);
            }
            Ok(())
        }
        .await;
        if result.is_err()
            && !confirmation.joined
            && !matches!(result, Err(ProviderError::InterruptTimeout { .. }))
        {
            let roots = confirmation.roots;
            match self
                .mutate(move |state| state.settle_interrupt_roots(&roots, true))
                .await
            {
                Ok(_) | Err(ProviderError::SessionStopped | ProviderError::Cancelled) => {}
                Err(error) => return Err(error),
            }
        }
        result
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        let request_id = input.request_id.clone();
        let response = input.clone();
        let pending = self
            .mutate(move |state| state.take_response(&response))
            .await?;
        let result = pending.send(input.clone()).await;
        let succeeded = result.is_ok();
        self.mutate(move |state| state.finish_response(request_id, input, succeeded))
            .await?;
        result
    }
    async fn snapshot(
        &self,
        thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        let response: Value = self
            .rpc(
                "thread/read",
                &json!({"threadId":native_thread(&thread)?,"includeTurns":true}),
            )
            .await?;
        let head = thread.native_conversation_head_ref.clone();
        let status = thread.status.clone();
        let mut snapshot = self.snapshot_result(thread, Some(response), head).await?;
        snapshot.provider_thread.status = status;
        Ok(snapshot)
    }
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        let _lane = self.operation_lane.lock().await;
        let count = mapping::rollback_count(&input)?;
        let head = match &input.target {
            RollbackTarget::ThreadStart { .. } => None,
            RollbackTarget::ProviderTurn { provider_turn, .. } => {
                provider_turn.native_turn_ref.clone()
            }
        };
        if count == 0 {
            return self
                .snapshot_result(input.provider_thread, None, head)
                .await;
        }
        let thread = native_thread(&input.provider_thread)?;
        let metadata: Value = self
            .rpc(
                "thread/read",
                &json!({"threadId":thread,"includeTurns":false}),
            )
            .await?;
        if metadata["thread"]["historyMode"] != "paginated" {
            return Err(protocol("Cannot roll back a legacy Codex thread"));
        }
        if metadata["thread"]["status"]["type"] == "notLoaded" {
            let mut params = self.thread_params(
                input
                    .provider_thread
                    .app_thread_id
                    .as_ref()
                    .and_then(mapping::string_id_ref),
                Some(&self.input.model_selection),
                Some(&self.input.runtime_policy),
            );
            params["threadId"] = json!(thread);
            params["excludeTurns"] = json!(true);
            self.resume_native(params, &input.provider_thread).await?;
        }
        let target = head
            .as_ref()
            .and_then(|head| head.native_id.as_ref())
            .map(|id| id.0.clone());
        let response = self.revert(thread, count, target.as_deref()).await?;
        self.snapshot_result(input.provider_thread, Some(response), head)
            .await
    }
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        let _lane = self.operation_lane.lock().await;
        let (boundary, count) = mapping::fork_boundary(&input)?;
        let mut params = self.thread_params(
            Some(&input.target_thread_id),
            input.model_selection.as_ref(),
            input.runtime_policy.as_ref(),
        );
        params["threadId"] = json!(native_thread(&input.source_provider_thread)?);
        params["excludeTurns"] = json!(true);
        if let Some(boundary) = boundary {
            params["lastTurnId"] = json!(boundary);
        }
        let response: ProviderResult<Value> = self.rpc("thread/fork", &params).await;
        let mut response = match response {
            Err(error) if archived(&error) => {
                let _: Value = self
                    .rpc(
                        "thread/unarchive",
                        &json!({"threadId":native_thread(&input.source_provider_thread)?}),
                    )
                    .await?;
                self.rpc("thread/fork", &params).await?
            }
            result => result?,
        };
        if count > 0 {
            let thread = response["thread"]["id"]
                .as_str()
                .ok_or_else(|| protocol("Fork response has no thread id"))?;
            let metadata: Value = self
                .rpc(
                    "thread/read",
                    &json!({"threadId":thread,"includeTurns":false}),
                )
                .await?;
            if metadata["thread"]["historyMode"] != "paginated" {
                return Err(protocol("Cannot trim a legacy Codex fork"));
            }
            response = self.revert(thread, count, None).await?;
        }
        let mut forked = json!({"providerThreadId":input.source_provider_thread.id});
        if let Some(turn) = input.provider_turn_id {
            forked["providerTurnId"] = json!(turn);
        }
        let thread = mapping::provider_thread(
            &response["thread"],
            &self.session,
            Some(&input.target_thread_id),
            input.owner_node_id.as_ref(),
            Some(forked),
            self.options.clock.as_ref(),
        )?;
        self.mutate(move |state| {
            state.remember_thread(thread.clone())?;
            Ok((thread, vec![]))
        })
        .await
    }
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        Some(self)
    }
}

pub(crate) fn protocol(detail: impl Into<String>) -> ProviderError {
    ProviderError::Protocol {
        reason: None,
        detail: detail.into(),
        payload: None,
    }
}
pub(crate) fn operation(
    operation: ProviderOperation,
    error: impl std::fmt::Display,
) -> ProviderError {
    ProviderError::Operation {
        operation,
        detail: error.to_string(),
    }
}
pub(crate) fn native_error(error: native::Error) -> ProviderError {
    operation(ProviderOperation::Stream, error)
}
pub(crate) fn native_thread(thread: &OrchestrationV2ProviderThread) -> ProviderResult<&str> {
    thread
        .native_thread_ref
        .as_ref()
        .and_then(|r| r.native_id.as_ref())
        .map(|s| s.0.as_str())
        .ok_or_else(|| protocol("Provider thread has no native Codex id"))
}
fn field_option<T>(field: native::Field<T>) -> Option<T> {
    match field {
        native::Field::Value(value) => Some(value),
        _ => None,
    }
}
fn archived(error: &ProviderError) -> bool {
    error.to_string().contains(" is archived") || error.to_string().contains("codex unarchive")
}
fn active_writer(error: &ProviderError) -> bool {
    error.to_string().contains("already has an active writer")
}
pub(crate) async fn terminate_native_background_process(
    client: &native::Client,
    thread: &str,
    process: &str,
    background: Option<&Arc<dyn CodexBackgroundHost>>,
) -> ProviderResult<bool> {
    let terminated: native::Result<native::BackgroundTerminalsTerminateResponse> = client
        .request(
            "thread/backgroundTerminals/terminate",
            &json!({"threadId":thread,"processId":process}),
        )
        .await;
    let mut confirmed = match terminated {
        Ok(response) => response.terminated,
        Err(native::Error::Closed) => true,
        Err(native::Error::Rpc { error, .. })
            if error.code == -32600.0 && error.message.starts_with("thread not found:") =>
        {
            true
        }
        Err(native::Error::Rpc { error, .. }) if error.code == -32601.0 => false,
        Err(error) => return Err(native_error(error)),
    };
    if !confirmed {
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(cursor.clone()) {
                return Err(protocol("Background terminal pagination repeated a cursor"));
            }
            let mut params = json!({"threadId":thread});
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let page: ProviderResult<native::BackgroundTerminalsListResponse> = client
                .request("thread/backgroundTerminals/list", &params)
                .await
                .map_err(native_error);
            let page = match page {
                Ok(page) => page,
                Err(error) if unknown_method(&error) => break,
                Err(error) => return Err(error),
            };
            if page
                .data
                .iter()
                .any(|terminal| terminal.process_id == process)
            {
                break;
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                confirmed = true;
                break;
            }
        }
        if !confirmed && let Some(host) = background {
            confirmed = host.terminate_untracked_command(thread, process).await?;
        }
    }
    Ok(confirmed)
}

fn unknown_method(error: &ProviderError) -> bool {
    error.to_string().contains("-32601")
}

#[async_trait]
impl SessionExtensions for CodexSessionRuntime {
    fn capabilities(&self) -> ExtensionCapabilities {
        ExtensionCapabilities {
            inject_history: true,
            compact: true,
            stop_task: true,
            unload: true,
            feedback: true,
            mcp_apps: true,
            ..Default::default()
        }
    }
    async fn pending_background_work(
        &self,
        thread: Option<OrchestrationV2ProviderThread>,
        include_subagents: bool,
    ) -> ProviderResult<bool> {
        self.mutate(move |state| {
            Ok((
                state.pending_background(thread.as_ref(), include_subagents),
                vec![],
            ))
        })
        .await
    }
    async fn model_context_window(
        &self,
        _: ModelSelection,
        _: Option<PathBuf>,
    ) -> ProviderResult<Option<u64>> {
        Ok(None)
    }
    fn can_reuse_context_usage(&self, a: &ModelSelection, b: &ModelSelection) -> bool {
        mapping::can_reuse_usage(a, b)
    }
    fn normalize_thread_load_selection(&self, selection: ModelSelection) -> ModelSelection {
        mapping::normalize_load_selection(selection)
    }
    async fn inject_history(
        &self,
        thread: OrchestrationV2ProviderThread,
        messages: Vec<OrchestrationV2HistoricalMessage>,
        context: String,
    ) -> ProviderResult<bool> {
        let mut items = vec![];
        if !context.is_empty() {
            items.push(json!({"type":"message","role":"developer","content":[{"type":"input_text","text":context}]}));
        }
        for message in messages {
            let value = serde_json::to_value(message).map_err(|e| protocol(e.to_string()))?;
            items.push(json!({"type":"message","role":value["role"],"content":[{"type":"input_text","text":value["text"]}]}));
        }
        let result: ProviderResult<Value> = self
            .rpc(
                "thread/inject_items",
                &json!({"threadId":native_thread(&thread)?,"items":items}),
            )
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(error) if unknown_method(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
    async fn compact(&self, input: TurnInput) -> ProviderResult<()> {
        let thread = native_thread(&input.provider_thread)?.to_owned();
        self.mutate(move |state| state.prepare_start(input)).await?;
        let result: ProviderResult<Value> = self
            .rpc("thread/compact/start", &json!({"threadId":thread}))
            .await;
        if let Err(error) = result {
            self.mutate(move |state| {
                state.cancel_start(&thread);
                Ok(((), vec![]))
            })
            .await?;
            return Err(error);
        }
        Ok(())
    }
    async fn apply_selection(
        &self,
        _: OrchestrationV2ProviderThread,
        _: ModelSelection,
    ) -> ProviderResult<()> {
        Err(ProviderError::Unsupported(
            "Codex applies model selection on turn/start",
        ))
    }
    async fn stop_task(
        &self,
        thread: OrchestrationV2ProviderThread,
        task_id: String,
    ) -> ProviderResult<()> {
        self.terminate_background_process(native_thread(&thread)?, &task_id)
            .await
    }
    async fn await_tasks(
        &self,
        _: OrchestrationV2ProviderThread,
        _: Vec<String>,
        _: bool,
    ) -> ProviderResult<Vec<String>> {
        Err(ProviderError::Unsupported("Codex await_tasks"))
    }
    async fn stop_subagent(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
        _: Option<IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>> {
        Err(ProviderError::Unsupported("Codex stop_subagent"))
    }
    async fn unload(
        &self,
        thread: OrchestrationV2ProviderThread,
        archive: bool,
    ) -> ProviderResult<()> {
        let native = native_thread(&thread)?.to_owned();
        let closing = native.clone();
        self.mutate(move |state| state.begin_unload(closing))
            .await?;
        let _: Value = self
            .rpc(
                if archive {
                    "thread/archive"
                } else {
                    "thread/unsubscribe"
                },
                &json!({"threadId":native_thread(&thread)?}),
            )
            .await?;
        self.mutate(move |state| state.confirm_unload(&native))
            .await?;
        Ok(())
    }
    async fn upload_feedback(
        &self,
        thread: OrchestrationV2ProviderThread,
        reason: Option<String>,
    ) -> ProviderResult<String> {
        let mut params =
            json!({"threadId":native_thread(&thread)?,"classification":"bug","includeLogs":true});
        if let Some(reason) = reason {
            params["reason"] = json!(reason);
        }
        let response: native::FeedbackUploadResponse = self.rpc("feedback/upload", &params).await?;
        Ok(response.thread_id)
    }
    async fn list_tools(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
    ) -> ProviderResult<Vec<McpTool>> {
        let response:Value=self.rpc("mcpServerStatus/list",&json!({"threadId":native_thread(&thread)?,"serverName":server,"detail":"toolsAndAuthOnly"})).await?;
        let mut tools = vec![];
        if let Some(entry) = response["data"]
            .as_array()
            .and_then(|a| a.iter().find(|s| s["name"] == server))
        {
            for value in entry["tools"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(_, v)| v)
            {
                tools.push(McpTool {
                    name: value["name"].as_str().unwrap_or_default().into(),
                    input_schema: value["inputSchema"].clone(),
                    meta: value.get("_meta").cloned(),
                    annotations: value.get("annotations").cloned(),
                });
            }
        }
        Ok(tools)
    }
    async fn call_tool(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
        tool: String,
        arguments: Value,
    ) -> ProviderResult<McpResult> {
        let response:Value=self.rpc("mcpServer/tool/call",&json!({"threadId":native_thread(&thread)?,"server":server,"tool":tool,"arguments":arguments})).await?;
        Ok(McpResult {
            content: response["content"]
                .as_array()
                .cloned()
                .ok_or_else(|| protocol("MCP result has no content array"))?,
            structured_content: response.get("structuredContent").cloned(),
            is_error: response["isError"].as_bool(),
            meta: response.get("_meta").cloned(),
        })
    }
    async fn read_resource(
        &self,
        thread: OrchestrationV2ProviderThread,
        server: String,
        uri: String,
    ) -> ProviderResult<Vec<Value>> {
        let response: Value = self
            .rpc(
                "mcpServer/resource/read",
                &json!({"threadId":native_thread(&thread)?,"server":server,"uri":uri}),
            )
            .await?;
        response["contents"]
            .as_array()
            .cloned()
            .ok_or_else(|| protocol("MCP resource has no contents array"))
    }
}
