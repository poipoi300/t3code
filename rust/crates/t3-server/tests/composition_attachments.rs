//! Message attachments end to end over a real socket: upload, send, the provider receiving the
//! claimed file, the thread showing it, a signed preview serving its bytes, and every refusal
//! (another session's upload, a second claim, another thread's attachment, a traversal string,
//! oversize and over-count), then the pending sweep on TypeScript's schedule with a driven clock.
//! The provider is a scripted stand-in that reads what it is handed from the attachments
//! directory: the Replay (dev) ACP adapter still refuses attachments (`acp.rs`, ACP owner).
#![cfg(all(
    feature = "local-services",
    feature = "orchestration",
    feature = "workspace"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod c_support;

use async_trait::async_trait;
use c_support::{Socket, pair, ticket, unary};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use t3_contracts::{
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};
use t3_orchestration::{api::*, provider::*};
use t3_rpc::{Client, Exit};
use t3_server::{
    adapters::{
        attachments::AttachmentOptions,
        config::{ConfigInputs, ServerFlags},
    },
    runtime::{RunningServer, StartOptions},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-11-14T12:00:00.000Z";
const THREAD: &str = "thread:attachments";
const OTHER_THREAD: &str = "thread:attachments-other";
/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

// ---------------------------------------------------------------------------------------------
// The scripted provider: each turn records the attachments it was handed and the bytes it read.

#[derive(Debug, Clone)]
struct Received {
    thread: String,
    attachment: Value,
    bytes: Vec<u8>,
}

fn sample(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples")
        .join(format!("{name}.jsonl"));
    let text = std::fs::read_to_string(path).expect("sample fixture");
    serde_json::from_str(text.lines().next().expect("sample line")).expect("sample json")
}

fn expanded<T: Clone>(id: &IdOrIndex<T>) -> ProviderResult<T> {
    match id {
        IdOrIndex::Id(id) => Ok(id.clone()),
        _ => Err(ProviderError::Unsupported("compact id in scripted adapter")),
    }
}

fn provider_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::Operation {
        operation: ProviderOperation::Open,
        detail: error.to_string(),
    }
}

struct Recording {
    instance: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
    attachments_dir: PathBuf,
    received: mpsc::UnboundedSender<Received>,
}

#[async_trait]
impl ProviderAdapter for Recording {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance
    }
    fn driver(&self) -> ProviderDriverKind {
        ProviderDriverKind("replay".into())
    }
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        Ok(self.capabilities.clone())
    }
    async fn plan_selection_transition(
        &self,
        _input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        Ok(SelectionTransitionPlan::ApplyOnNextTurn)
    }
    async fn open_session(
        &self,
        input: OpenSession,
        _cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        let mut session = sample("OrchestrationV2ProviderSession");
        session["id"] = json!(input.provider_session_id);
        session["driver"] = "replay".into();
        session["providerInstanceId"] = "replay".into();
        session["status"] = "ready".into();
        session["model"] = json!(input.model_selection.model);
        session["capabilities"] =
            serde_json::to_value(&self.capabilities).map_err(provider_error)?;
        Ok(Arc::new(RecordingSession {
            session: serde_json::from_value(session).map_err(provider_error)?,
            subscribers: Mutex::new(Vec::new()),
            attachments_dir: self.attachments_dir.clone(),
            received: self.received.clone(),
        }))
    }
}

struct RecordingSession {
    session: OrchestrationV2ProviderSession,
    subscribers: Mutex<Vec<mpsc::Sender<ProviderResult<ProviderEvent>>>>,
    attachments_dir: PathBuf,
    received: mpsc::UnboundedSender<Received>,
}

fn event(kind: ProviderEventKind) -> ProviderEvent {
    ProviderEvent {
        driver: ProviderDriverKind("replay".into()),
        kind,
    }
}

fn turn_events(input: &TurnInput) -> ProviderResult<Vec<ProviderEvent>> {
    let provider_thread_id = expanded(&input.provider_thread.id)?;
    let provider_turn_id = ProviderTurnId(format!("provider-turn:{}", input.run_id.0));
    let turn = |status: &str, completed: Option<&str>| -> ProviderResult<ProviderEvent> {
        let turn = json!({"id":provider_turn_id,"providerThreadId":provider_thread_id,"nodeId":input.root_node_id,"runAttemptId":input.attempt_id,"nativeTurnRef":null,"ordinal":input.provider_turn_ordinal,"status":status,"startedAt":NOW,"completedAt":completed});
        Ok(event(ProviderEventKind::Turn {
            provider_turn: Box::new(serde_json::from_value(turn).map_err(provider_error)?),
            thread_id: None,
        }))
    };
    Ok(vec![
        turn("running", None)?,
        turn("completed", Some(NOW))?,
        event(ProviderEventKind::Terminal {
            provider_thread_id,
            provider_turn_id: provider_turn_id.clone(),
            run_ordinal: input.run_ordinal,
            disposition: ThreadDisposition::Reusable,
            outcome: TerminalOutcome::Completed { reason: None },
        }),
    ])
}

#[async_trait]
impl SessionRuntime for RecordingSession {
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
        let (sender, events) = mpsc::channel(64);
        self.subscribers.lock().unwrap().push(sender);
        Ok(EventSubscription {
            events,
            cancellation: CancellationToken::new(),
        })
    }
    async fn close(&self) -> ProviderResult<()> {
        self.subscribers.lock().unwrap().clear();
        Ok(())
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        let mut thread = match input.existing_provider_thread {
            Some(thread) => serde_json::to_value(thread).map_err(provider_error)?,
            None => {
                let mut thread = sample("OrchestrationV2ProviderThread");
                thread["id"] = format!("provider-thread:{}", input.thread_id.0).into();
                thread["ownerNodeId"] = Value::Null;
                thread["lastRunOrdinal"] = Value::Null;
                thread["createdAt"] = NOW.into();
                thread["updatedAt"] = NOW.into();
                thread
            }
        };
        thread["driver"] = "replay".into();
        thread["providerInstanceId"] = "replay".into();
        thread["providerSessionId"] = json!(self.session.id);
        thread["appThreadId"] = json!(input.thread_id);
        thread["nativeThreadRef"] = json!({"driver":"replay","nativeId":format!("native:{}", input.thread_id.0),"strength":"strong"});
        serde_json::from_value(thread).map_err(provider_error)
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        Ok(input.provider_thread)
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        // What a provider adapter does with a claimed attachment: read it from the server's
        // attachments directory by its id.
        for attachment in &input.message.attachments {
            let path = t3_orchestration::ports::runs_ingestion::attachment_claims::attachment_path(
                &self.attachments_dir,
                attachment,
            )
            .ok_or_else(|| provider_error("unresolvable attachment"))?;
            let bytes = std::fs::read(&path).map_err(provider_error)?;
            let _ = self.received.send(Received {
                thread: input.thread_id.0.clone(),
                attachment: serde_json::to_value(attachment).map_err(provider_error)?,
                bytes,
            });
        }
        let events = turn_events(&input)?;
        let subscribers = self.subscribers.lock().unwrap().clone();
        tokio::spawn(async move {
            for event in events {
                for subscriber in &subscribers {
                    let _ = subscriber.send(Ok(event.clone())).await;
                }
            }
        });
        Ok(())
    }
    async fn steer_turn(&self, _input: SteerInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("steer_turn"))
    }
    async fn interrupt_turn(&self, _input: InterruptInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("interrupt_turn"))
    }
    async fn respond(&self, _input: RuntimeResponse) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("respond"))
    }
    async fn snapshot(
        &self,
        _thread: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("snapshot"))
    }
    async fn rollback(&self, _input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        Err(ProviderError::Unsupported("rollback"))
    }
    async fn fork(&self, _input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        Err(ProviderError::Unsupported("fork"))
    }
}

// ---------------------------------------------------------------------------------------------
// Harness

struct Server {
    runtime: RunningServer,
    base: String,
    http: reqwest::Client,
    admin: String,
    attachments: PathBuf,
    clock: Arc<AtomicU64>,
}

fn inputs(home: &Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(home.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: home.into(),
        process_cwd: home.into(),
        bootstrap: None,
        headless: true,
        force_auto_bootstrap: Some(false),
        service_managed: false,
        cli_log_level: None,
        automatic_port: None,
        persisted_observability: Default::default(),
        static_dir: None,
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn start(home: &Path, received: mpsc::UnboundedSender<Received>) -> Server {
    start_on_port(home, received, 0).await
}

async fn start_on_port(
    home: &Path,
    received: mpsc::UnboundedSender<Received>,
    port: u16,
) -> Server {
    std::fs::create_dir_all(home.join("userdata")).unwrap();
    std::fs::write(
        home.join("userdata/settings.json"),
        r#"{"providerInstances":{"replay":{"driver":"replay","enabled":true}}}"#,
    )
    .unwrap();
    let attachments = home.join("userdata").join("attachments");
    // Starts at the wall clock so uploaded files' modification times line up with it.
    let clock = Arc::new(AtomicU64::new(wall_ms()));
    let read = clock.clone();
    let mut config = inputs(home);
    config.flags.port = Some(port);
    let runtime = t3_server::runtime::start_with(
        config,
        StartOptions {
            providers: vec![Arc::new(Recording {
                instance: ProviderInstanceId("replay".into()),
                capabilities: serde_json::from_value(
                    sample("OrchestrationV2ProviderSession")["capabilities"].clone(),
                )
                .unwrap(),
                attachments_dir: attachments.clone(),
                received,
            })],
            attachments: AttachmentOptions {
                clock: Some(Arc::new(move || read.load(Ordering::SeqCst))),
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let base = format!("http://{}", runtime.address);
    let http = reqwest::Client::new();
    let admin = pair(&http, &base, &runtime.pairing_credential).await;
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let layout = t3_config::home::HomeLayout {
        base_dir: home.into(),
        state_dir: home.join("userdata"),
    };
    t3_persistence::rusqlite::Connection::open(layout.database())
        .unwrap()
        .execute(
            "INSERT OR IGNORE INTO projection_projects
             (project_id, title, workspace_root, scripts_json, created_at, updated_at)
             VALUES ('project:attachments', 'Attachments', ?1, '[]', ?2, ?2)",
            [workspace.to_string_lossy().as_ref(), NOW],
        )
        .unwrap();
    Server {
        runtime,
        base,
        http,
        admin,
        attachments,
        clock,
    }
}

impl Server {
    async fn cookie(&self, scopes: &[&str]) -> String {
        let grant = self
            .http
            .post(format!("{}/api/auth/pairing-token", self.base))
            .header("cookie", &self.admin)
            .json(&json!({"scopes": scopes}))
            .send()
            .await
            .unwrap();
        assert_eq!(grant.status(), 200);
        let grant = grant.json::<Value>().await.unwrap();
        pair(
            &self.http,
            &self.base,
            grant["credential"].as_str().unwrap(),
        )
        .await
    }
    async fn socket(&self, cookie: &str) -> (Socket, Client) {
        let ticket = ticket(&self.http, &self.base, cookie, "rpc").await;
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{}/ws?orchestrationProtocol=2&wsTicket={ticket}",
            self.runtime.address
        ))
        .await
        .unwrap();
        (socket, Client::new(0, Default::default()))
    }
    /// The pending upload's id after its bytes are stored.
    async fn upload(&self, socket: &mut Socket, client: &mut Client, name: &str) -> String {
        let grant = ok(unary(
            socket,
            client,
            "attachments.createUploadUrl",
            json!({"type":"image","name":name,"mimeType":"image/png","sizeBytes":PNG.len()}),
        )
        .await);
        let stored = self
            .http
            .post(format!(
                "{}{}",
                self.base,
                grant["relativeUrl"].as_str().unwrap()
            ))
            .body(PNG)
            .send()
            .await
            .unwrap();
        assert_eq!(stored.status(), 204, "{}", stored.text().await.unwrap());
        grant["attachmentId"].as_str().unwrap().to_owned()
    }
    fn stored(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.attachments)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|kind| !kind.is_dir()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn ok(exit: Exit) -> Value {
    match exit {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!(
            "expected success, got {}",
            serde_json::to_value(other).unwrap()
        ),
    }
}

/// The typed failure of a call.
fn failure(exit: Exit) -> Value {
    let encoded = serde_json::to_value(&exit).unwrap();
    let error = encoded["cause"][0]["error"].clone();
    assert!(!error.is_null(), "expected a typed failure, got {encoded}");
    error
}

fn link_file(target: &Path, link: &Path) {
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, link)
        .expect("file symlinks need Developer Mode or the symlink privilege");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

fn image(id: &str) -> Value {
    json!({"type":"image","id":id,"name":"pixel.png","mimeType":"image/png","sizeBytes":PNG.len()})
}

async fn create_thread(socket: &mut Socket, client: &mut Client, thread: &str) {
    ok(unary(
        socket,
        client,
        "orchestration.dispatchCommand",
        json!({"type":"thread.create","commandId":format!("command:create:{thread}"),"threadId":thread,"projectId":"project:attachments","title":"Attachments","modelSelection":{"instanceId":"replay","model":"replay-model"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
    )
    .await);
}

async fn send(
    socket: &mut Socket,
    client: &mut Client,
    thread: &str,
    key: &str,
    attachments: Vec<Value>,
) -> Exit {
    unary(
        socket,
        client,
        "orchestration.dispatchCommand",
        json!({"type":"message.dispatch","commandId":format!("command:{key}"),"threadId":thread,"messageId":format!("message:{key}"),"text":"Look at this","attachments":attachments,"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
    )
    .await
}

/// A send the claim refused: the dispatch error, carrying the claim's reason.
fn refused(exit: Exit, reason: &str) {
    let error = failure(exit);
    assert_eq!(
        error["_tag"], "OrchestrationV2DispatchCommandError",
        "{error}"
    );
    assert!(
        error["detail"]
            .as_str()
            .unwrap_or_default()
            .contains(reason),
        "{reason} not in {error}"
    );
}

// ---------------------------------------------------------------------------------------------
// Tests

#[tokio::test]
async fn an_uploaded_image_reaches_the_provider_the_thread_and_a_signed_preview() {
    let temp = tempfile::tempdir().unwrap();
    let (received, mut handed) = mpsc::unbounded_channel();
    let server = start(temp.path(), received).await;
    let (mut socket, mut client) = server.socket(&server.admin).await;
    create_thread(&mut socket, &mut client, THREAD).await;

    let pending = server.upload(&mut socket, &mut client, "pixel.png").await;
    assert!(pending.starts_with("pending-"), "{pending}");
    ok(send(
        &mut socket,
        &mut client,
        THREAD,
        "first",
        vec![image(&pending)],
    )
    .await);

    // The provider is handed the claimed copy: a thread-owned id whose bytes are the upload's.
    let got = handed.recv().await.unwrap();
    assert_eq!(got.thread, THREAD);
    assert_eq!(got.bytes, PNG);
    let claimed = got.attachment["id"].as_str().unwrap().to_owned();
    assert!(claimed.starts_with("thread-attachments-"), "{claimed}");
    assert_eq!(got.attachment["name"], "pixel.png");
    assert_eq!(got.attachment["mimeType"], "image/png");

    // The thread shows it.
    let snapshot: Value = server
        .http
        .get(format!(
            "{}/api/orchestration/threads/{THREAD}",
            server.base
        ))
        .header("cookie", &server.admin)
        .header("x-t3-orchestration-protocol", "2")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(snapshot.to_string().contains(&claimed), "{snapshot}");

    // A signed preview serves the bytes, with the type from the stored extension.
    let grant = ok(unary(
        &mut socket,
        &mut client,
        "assets.createUrl",
        json!({"resource":{"_tag":"attachment","attachmentId":claimed,"mimeType":"text/html","disposition":"inline"}}),
    )
    .await);
    let url = format!("{}{}", server.base, grant["relativeUrl"].as_str().unwrap());
    let preview = server.http.get(&url).send().await.unwrap();
    assert_eq!(preview.status(), 200);
    assert_eq!(preview.headers()["content-type"], "image/png");
    assert_eq!(preview.headers()["x-content-type-options"], "nosniff");
    assert_eq!(preview.bytes().await.unwrap().as_ref(), PNG);
    // A link planted under a stored attachment's name is never served or signed.
    let outside = temp.path().join("outside-secret.png");
    std::fs::write(&outside, PNG).unwrap();
    let planted = "thread-attachments-00000000-0000-4000-8000-000000000001";
    link_file(&outside, &server.attachments.join(format!("{planted}.png")));
    let exit = unary(
        &mut socket,
        &mut client,
        "assets.createUrl",
        json!({"resource":{"_tag":"attachment","attachmentId":planted}}),
    )
    .await;
    assert!(matches!(exit, Exit::Failure { .. }), "{exit:?}");
    // The signature binds the grant: an altered token serves nothing.
    let (prefix, rest) = url.split_once("/api/assets/").unwrap();
    let (token, name) = rest.split_once('/').unwrap();
    let mut tampered = token.to_owned();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    let altered = format!("{prefix}/api/assets/{tampered}/{name}");
    assert_eq!(
        server.http.get(&altered).send().await.unwrap().status(),
        404
    );
    // The grant expires (one hour, rounded to the half hour for HTML): two hours later, nothing.
    server.clock.fetch_add(2 * 60 * 60 * 1000, Ordering::SeqCst);
    assert_eq!(server.http.get(&url).send().await.unwrap().status(), 404);
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn claims_are_once_only_and_only_by_the_uploading_session_for_its_own_threads() {
    let temp = tempfile::tempdir().unwrap();
    let (received, mut handed) = mpsc::unbounded_channel();
    let server = start(temp.path(), received).await;
    let (mut socket, mut client) = server.socket(&server.admin).await;
    create_thread(&mut socket, &mut client, THREAD).await;
    create_thread(&mut socket, &mut client, OTHER_THREAD).await;

    // A second claim of the same upload is refused.
    let pending = server.upload(&mut socket, &mut client, "pixel.png").await;
    ok(send(
        &mut socket,
        &mut client,
        THREAD,
        "once",
        vec![image(&pending)],
    )
    .await);
    let claimed = handed.recv().await.unwrap().attachment["id"]
        .as_str()
        .unwrap()
        .to_owned();
    refused(
        send(
            &mut socket,
            &mut client,
            THREAD,
            "twice",
            vec![image(&pending)],
        )
        .await,
        "attachment not found",
    );

    // Another session's upload is refused, and stays claimable by the session that made it.
    let other = server
        .cookie(&["orchestration:read", "orchestration:operate"])
        .await;
    let (mut other_socket, mut other_client) = server.socket(&other).await;
    let theirs = server
        .upload(&mut other_socket, &mut other_client, "theirs.png")
        .await;
    refused(
        send(
            &mut socket,
            &mut client,
            THREAD,
            "foreign",
            vec![image(&theirs)],
        )
        .await,
        "attachment not found",
    );
    ok(send(
        &mut other_socket,
        &mut other_client,
        OTHER_THREAD,
        "theirs",
        vec![image(&theirs)],
    )
    .await);
    assert_eq!(handed.recv().await.unwrap().thread, OTHER_THREAD);

    // Replaying a still-valid upload URL cannot reopen a consumed attachment after discard.
    ok(unary(
        &mut socket,
        &mut client,
        "attachments.delete",
        json!({"attachmentId": pending}),
    )
    .await);
    assert!(
        server.stored().contains(&format!("{pending}.png.owner")),
        "discard removed a consumed upload's owner record"
    );

    // Initial messages with existing files must identify the target thread for a membership check.
    let launch = unary(&mut socket, &mut client, "orchestration.launchThread", json!({
        "commandId":"launch-without-an-attachment-target", "projectId":"project:attachments",
        "title":"Attachment launch", "modelSelection":{"instanceId":"replay","model":"replay-model"},
        "runtimeMode":"full-access", "interactionMode":"default", "workspaceStrategy":{"type":"root"},
        "initialMessage":{"text":"Use image", "attachments":[image(&claimed)]}
    })).await;
    let launch_error = failure(launch);
    assert!(
        launch_error.to_string().contains("thread id at launch"),
        "{launch_error}"
    );

    // Thread A's claimed attachment cannot be sent through thread B; on thread A it can.
    refused(
        send(
            &mut socket,
            &mut client,
            OTHER_THREAD,
            "cross",
            vec![image(&claimed)],
        )
        .await,
        "does not belong to this thread",
    );
    ok(send(
        &mut socket,
        &mut client,
        THREAD,
        "again",
        vec![{
            let mut altered = image(&claimed);
            altered["name"] = "client-renamed.png".into();
            altered
        }],
    )
    .await);
    let again = handed.recv().await.unwrap();
    assert_eq!(again.thread, THREAD);
    assert_eq!(again.attachment["name"], "pixel.png");
    assert_eq!(again.attachment["id"], claimed.as_str());

    // An upload whose stored file is swapped for a link is not followed: the claim refuses it
    // and nothing of the target reaches the provider.
    let outside = temp.path().join("outside-secret.png");
    std::fs::write(&outside, PNG).unwrap();
    let linked = server.upload(&mut socket, &mut client, "linked.png").await;
    let stored = server.attachments.join(format!("{linked}.png"));
    std::fs::remove_file(&stored).unwrap();
    link_file(&outside, &stored);
    refused(
        send(
            &mut socket,
            &mut client,
            THREAD,
            "linked",
            vec![image(&linked)],
        )
        .await,
        "attachment not found",
    );
    assert!(
        handed.try_recv().is_err(),
        "a linked upload reached the provider"
    );

    // References are ids, never paths: a traversal string reaches nothing.
    for traversal in [
        "../../userdata/settings.json",
        "..\\..\\secrets\\asset-access-signing-key",
        "pending-00000000-0000-4000-8000-000000000000-png/../../settings",
    ] {
        let exit = send(
            &mut socket,
            &mut client,
            THREAD,
            &format!("traversal-{}", traversal.len()),
            vec![image(traversal)],
        )
        .await;
        assert!(
            matches!(exit, Exit::Failure { .. }),
            "{traversal}: {exit:?}"
        );
    }
    assert!(
        handed.try_recv().is_err(),
        "a traversal reached the provider"
    );
    let names = server.stored();
    assert!(
        names.iter().all(|name| !name.contains("settings")),
        "{names:?}"
    );
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversize_and_over_count_are_refused_before_storage_and_unclaimed_uploads_are_swept() {
    let temp = tempfile::tempdir().unwrap();
    let (received, mut handed) = mpsc::unbounded_channel();
    let server = start(temp.path(), received).await;
    let (mut socket, mut client) = server.socket(&server.admin).await;
    create_thread(&mut socket, &mut client, THREAD).await;

    // Oversize: no upload URL, so no byte is ever accepted.
    let too_big = unary(
        &mut socket,
        &mut client,
        "attachments.createUploadUrl",
        json!({"type":"image","name":"big.png","mimeType":"image/png","sizeBytes":10 * 1024 * 1024 + 1}),
    )
    .await;
    assert!(matches!(too_big, Exit::Failure { .. }), "{too_big:?}");
    // A body longer than its signed size is refused, and nothing is published.
    let grant = ok(unary(
        &mut socket,
        &mut client,
        "attachments.createUploadUrl",
        json!({"type":"image","name":"short.png","mimeType":"image/png","sizeBytes":4}),
    )
    .await);
    let long = server
        .http
        .post(format!(
            "{}{}",
            server.base,
            grant["relativeUrl"].as_str().unwrap()
        ))
        .body(PNG)
        .send()
        .await
        .unwrap();
    assert_eq!(long.status(), 400);
    assert!(server.stored().is_empty(), "{:?}", server.stored());

    // Over-count per message: refused before anything is copied.
    let pending = server.upload(&mut socket, &mut client, "pixel.png").await;
    let crowd: Vec<Value> = std::iter::once(image(&pending))
        .chain((0..100).map(|n| image(&format!("pending-00000000-0000-4000-8000-{n:012}-png"))))
        .collect();
    refused(
        send(&mut socket, &mut client, THREAD, "crowd", crowd).await,
        "up to 100",
    );
    // Over-count per session: one message's worth of unsent uploads at a time.
    // `pixel.png` and `short.png` are outstanding already.
    for _ in 2..100 {
        ok(unary(
            &mut socket,
            &mut client,
            "attachments.createUploadUrl",
            json!({"type":"image","name":"more.png","mimeType":"image/png","sizeBytes":PNG.len()}),
        )
        .await);
    }
    let over = failure(
        unary(
            &mut socket,
            &mut client,
            "attachments.createUploadUrl",
            json!({"type":"image","name":"one-more.png","mimeType":"image/png","sizeBytes":PNG.len()}),
        )
        .await,
    );
    assert!(over.to_string().contains("too many uploads"), "{over}");

    // The pending upload is still claimable (the refused send claimed nothing).
    ok(send(
        &mut socket,
        &mut client,
        THREAD,
        "kept",
        vec![image(&pending)],
    )
    .await);
    let claimed = handed.recv().await.unwrap().attachment["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let unsent = server.upload_from_fresh_session().await;
    let before = server.stored();
    assert!(before.iter().any(|n| n.starts_with(&unsent)), "{before:?}");

    // TypeScript's schedule: an issue sweeps at most every 15 minutes, removing pending uploads
    // older than 24 hours. Just under 24 hours later nothing goes; past it the unsent and the
    // claimed upload's pending source go, and the thread's copy stays.
    server
        .clock
        .fetch_add(24 * 60 * 60 * 1000 - 60_000, Ordering::SeqCst);
    server.issue().await;
    assert!(server.stored().iter().any(|n| n.starts_with(&unsent)));
    server.clock.fetch_add(16 * 60 * 1000, Ordering::SeqCst);
    server.issue().await;
    let after = server.stored();
    assert!(!after.iter().any(|n| n.starts_with(&unsent)), "{after:?}");
    assert!(!after.iter().any(|n| n.starts_with(&pending)), "{after:?}");
    assert!(after.iter().any(|n| n.starts_with(&claimed)), "{after:?}");
    server.runtime.shutdown().await.unwrap();
}

impl Server {
    /// Issues one upload URL (which runs the sweep when it is due) from a fresh session, so the
    /// first session's outstanding allowance does not matter.
    async fn issue(&self) {
        let cookie = self
            .cookie(&["orchestration:read", "orchestration:operate"])
            .await;
        let (mut socket, mut client) = self.socket(&cookie).await;
        ok(unary(
            &mut socket,
            &mut client,
            "attachments.createUploadUrl",
            json!({"type":"image","name":"tick.png","mimeType":"image/png","sizeBytes":PNG.len()}),
        )
        .await);
    }
    /// An upload from a fresh session (the first one has used its allowance).
    async fn upload_from_fresh_session(&self) -> String {
        let cookie = self
            .cookie(&["orchestration:read", "orchestration:operate"])
            .await;
        let (mut socket, mut client) = self.socket(&cookie).await;
        self.upload(&mut socket, &mut client, "unsent.png").await
    }
}

/// The owner is recorded beside the upload, so a composer draft restored after a restart still
/// sends: by the session that uploaded it, once; never by another session.
#[tokio::test]
async fn an_upload_survives_a_restart_for_its_own_session_only() {
    let temp = tempfile::tempdir().unwrap();
    let (received, mut handed) = mpsc::unbounded_channel();
    let server = start(temp.path(), received.clone()).await;
    let mine_cookie = server.admin.clone();
    let theirs_cookie = server
        .cookie(&["orchestration:read", "orchestration:operate"])
        .await;
    let (mut socket, mut client) = server.socket(&mine_cookie).await;
    create_thread(&mut socket, &mut client, THREAD).await;
    let mine = server.upload(&mut socket, &mut client, "mine.png").await;
    let sent = server.upload(&mut socket, &mut client, "sent.png").await;
    ok(send(
        &mut socket,
        &mut client,
        THREAD,
        "before",
        vec![image(&sent)],
    )
    .await);
    handed.recv().await.unwrap();
    let (mut their_socket, mut their_client) = server.socket(&theirs_cookie).await;
    let theirs = server
        .upload(&mut their_socket, &mut their_client, "theirs.png")
        .await;
    let port = server.runtime.address.port();
    drop((socket, their_socket));
    server.runtime.shutdown().await.unwrap();

    // Local browser cookie names include the port. Restart the same endpoint with the
    // same state directory so the original cookie authenticates the original session.
    let server = start_on_port(temp.path(), received, port).await;
    let (mut socket, mut client) = server.socket(&mine_cookie).await;
    refused(
        send(
            &mut socket,
            &mut client,
            THREAD,
            "theirs",
            vec![image(&theirs)],
        )
        .await,
        "attachment not found",
    );
    refused(
        send(
            &mut socket,
            &mut client,
            THREAD,
            "resent",
            vec![image(&sent)],
        )
        .await,
        "attachment not found",
    );
    ok(send(
        &mut socket,
        &mut client,
        THREAD,
        "after",
        vec![image(&mine)],
    )
    .await);
    let got = handed.recv().await.unwrap();
    assert_eq!(got.bytes, PNG);
    assert!(
        got.attachment["id"]
            .as_str()
            .unwrap()
            .starts_with("thread-attachments-")
    );
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn overlapping_sends_from_one_session_claim_one_copy() {
    let temp = tempfile::tempdir().unwrap();
    let (received, mut handed) = mpsc::unbounded_channel();
    let server = start(temp.path(), received).await;
    let (mut a, mut ca) = server.socket(&server.admin).await;
    let (mut b, mut cb) = server.socket(&server.admin).await;
    create_thread(&mut a, &mut ca, THREAD).await;
    create_thread(&mut b, &mut cb, OTHER_THREAD).await;
    let id = server.upload(&mut a, &mut ca, "pixel.png").await;
    let (first, second) = tokio::join!(
        send(&mut a, &mut ca, THREAD, "left", vec![image(&id)]),
        send(&mut b, &mut cb, OTHER_THREAD, "right", vec![image(&id)])
    );
    let winner = match (first, second) {
        (Exit::Success { .. }, failure) => {
            refused(failure, "attachment not found");
            THREAD
        }
        (failure, Exit::Success { .. }) => {
            refused(failure, "attachment not found");
            OTHER_THREAD
        }
        exits => panic!("expected one successful send: {exits:?}"),
    };
    assert_eq!(handed.recv().await.unwrap().thread, winner);
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn previews_require_a_pending_owner_or_a_readable_thread_record() {
    let temp = tempfile::tempdir().unwrap();
    let (tx, mut delivered) = mpsc::unbounded_channel();
    let server = start(temp.path(), tx).await;
    let (mut owner, mut owner_client) = server.socket(&server.admin).await;
    let id = server.upload(&mut owner, &mut owner_client, "a.png").await;
    let resource = |id: &str| json!({"resource":{"_tag":"attachment","attachmentId":id}});
    ok(unary(
        &mut owner,
        &mut owner_client,
        "assets.createUrl",
        resource(&id),
    )
    .await);
    let reader = server.cookie(&["orchestration:read"]).await;
    let (mut reader, mut client) = server.socket(&reader).await;
    let missing = "pending-00000000-0000-4000-8000-000000000001";
    let stray = "thread-a-00000000-0000-4000-8000-000000000002";
    std::fs::write(server.attachments.join(format!("{stray}.png")), PNG).unwrap();
    for candidate in [missing, &id, stray] {
        let error = failure(
            unary(
                &mut reader,
                &mut client,
                "assets.createUrl",
                resource(candidate),
            )
            .await,
        );
        assert_eq!(error["_tag"], "AssetAttachmentNotFoundError", "{error}");
    }
    create_thread(&mut owner, &mut owner_client, THREAD).await;
    ok(send(
        &mut owner,
        &mut owner_client,
        THREAD,
        "shared-preview",
        vec![image(&id)],
    )
    .await);
    let record = delivered.recv().await.unwrap();
    let claimed = record.attachment["id"].as_str().unwrap();
    ok(unary(
        &mut reader,
        &mut client,
        "assets.createUrl",
        resource(claimed),
    )
    .await);
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn claim_metadata_must_match_the_upload() {
    let temp = tempfile::tempdir().unwrap();
    let (tx, _) = mpsc::unbounded_channel();
    let server = start(temp.path(), tx).await;
    let (mut socket, mut client) = server.socket(&server.admin).await;
    create_thread(&mut socket, &mut client, THREAD).await;
    let id = server.upload(&mut socket, &mut client, "a.png").await;
    for (key, field, replacement) in [
        ("mime", "mimeType", json!("image/jpeg")),
        ("kind", "type", json!("file")),
        ("size", "sizeBytes", json!(1)),
    ] {
        let mut attachment = image(&id);
        attachment[field] = replacement;
        assert!(
            matches!(
                send(&mut socket, &mut client, THREAD, key, vec![attachment]).await,
                Exit::Failure { .. }
            ),
            "accepted changed {field}"
        );
    }
    ok(send(
        &mut socket,
        &mut client,
        THREAD,
        "original",
        vec![image(&id)],
    )
    .await);
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn discarding_an_unspent_url_revokes_http_upload() {
    let temp = tempfile::tempdir().unwrap();
    let (tx, _) = mpsc::unbounded_channel();
    let server = start(temp.path(), tx).await;
    let (mut socket, mut client) = server.socket(&server.admin).await;
    let grant = ok(unary(
        &mut socket,
        &mut client,
        "attachments.createUploadUrl",
        json!({"type":"image","name":"pixel.png","mimeType":"image/png","sizeBytes":PNG.len()}),
    )
    .await);
    ok(unary(
        &mut socket,
        &mut client,
        "attachments.delete",
        json!({"attachmentId":grant["attachmentId"]}),
    )
    .await);
    let response = server
        .http
        .post(format!(
            "{}{}",
            server.base,
            grant["relativeUrl"].as_str().unwrap()
        ))
        .body(PNG)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert!(server.stored().is_empty());
    server.runtime.shutdown().await.unwrap();
}
