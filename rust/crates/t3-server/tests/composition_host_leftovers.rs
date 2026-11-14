//! Host services that span domains, served by a started server over a real socket:
//! `shell.openInEditor`, `server.checkSecretSource`, `secrets.answerRequest` and `peerLinks.*`.
//! Fakes sit at the process and network edge only: the editors installed and the launches
//! (recorded), a fake `op` on the server's PATH, a loopback stand-in for the other environment,
//! and a scripted provider whose turn stays running so its cards can be answered.
#![cfg(all(
    feature = "local-services",
    feature = "host",
    feature = "workspace",
    feature = "providers",
    feature = "cloud",
    feature = "project"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod c_support;

use async_trait::async_trait;
use c_support::{Socket, pair, ticket, unary};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use t3_contracts::{
    editor::{EditorId, EditorLaunchStyle},
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
};
use t3_host::editor::{DetachedEditorLaunch, DiscoveredEditor, EditorCommand};
use t3_orchestration::{api::*, provider::*};
use t3_rpc::{Client, Exit};
use t3_server::{
    adapters::{
        config::{ConfigInputs, ServerFlags},
        host_leftovers::{EditorEdge, HostLeftoversAdapter, HostLeftoversOptions},
    },
    runtime::{RunningServer, StartOptions},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const THREAD: &str = "thread:host-leftovers";

// ---------------------------------------------------------------------------------------------
// Harness

struct Server {
    runtime: RunningServer,
    base: String,
    http: reqwest::Client,
    admin: String,
}

fn inputs(home: &Path, env: BTreeMap<String, String>) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(home.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env,
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

async fn start(home: &Path, env: BTreeMap<String, String>, options: StartOptions) -> Server {
    let runtime = t3_server::runtime::start_with(inputs(home, env), options)
        .await
        .unwrap();
    let base = format!("http://{}", runtime.address);
    let http = reqwest::Client::new();
    let admin = pair(&http, &base, &runtime.pairing_credential).await;
    Server {
        runtime,
        base,
        http,
        admin,
    }
}

impl Server {
    /// A browser session limited to `scopes`.
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
    fn served(&self, methods: &[&str]) {
        for method in methods {
            assert!(
                !self.runtime.unserved_methods.contains(method),
                "{method} is still unserved"
            );
        }
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

/// The first failure (`Fail` error or `Die` defect).
fn failure(exit: Exit) -> Value {
    let encoded = serde_json::to_value(&exit).unwrap();
    let cause = &encoded["cause"][0];
    let value = if cause["error"].is_null() {
        &cause["defect"]
    } else {
        &cause["error"]
    };
    assert!(!value.is_null(), "expected a failure, got {encoded}");
    value.clone()
}

/// The RPC guard's refusal, naming the method's exact permission.
fn missing_scope(exit: Exit, scope: &str) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert_eq!(error["requiredPermission"], scope, "{error}");
}

fn settings(home: &Path, settings: Value) {
    std::fs::create_dir_all(home.join("userdata")).unwrap();
    std::fs::write(home.join("userdata/settings.json"), settings.to_string()).unwrap();
}

// ---------------------------------------------------------------------------------------------
// shell.openInEditor

struct Recorder(mpsc::UnboundedSender<EditorCommand>);
#[async_trait]
impl DetachedEditorLaunch for Recorder {
    async fn launch(&self, command: EditorCommand) -> t3_host::Result<()> {
        self.0.send(command).map_err(|_| t3_host::Error::Service)
    }
}

#[tokio::test]
async fn editor_links_open_only_local_targets_inside_server_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let scratch = home.join("scratch").join("thread-1");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("notes.md"), "notes\n").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("notes.md"), "notes\n").unwrap();
    let editor = home.join("bin").join("fake-code");
    let file_manager = home.join("bin").join("explorer.exe");
    let (sender, mut launches) = mpsc::unbounded_channel();
    let server = start(
        home,
        Default::default(),
        StartOptions {
            host_leftovers: HostLeftoversOptions {
                editors: Some(EditorEdge {
                    editors: vec![
                        DiscoveredEditor {
                            id: EditorId::Vscode,
                            command: editor.clone(),
                            base_args: vec![],
                            style: EditorLaunchStyle::Goto,
                        },
                        DiscoveredEditor {
                            id: EditorId::FileManager,
                            command: file_manager.clone(),
                            base_args: vec![],
                            style: EditorLaunchStyle::DirectPath,
                        },
                    ],
                    launch: Arc::new(Recorder(sender)),
                }),
                started: None,
            },
            ..Default::default()
        },
    )
    .await;
    server.served(&["shell.openInEditor"]);
    let (mut socket, mut client) = server.socket(&server.admin).await;
    let target = format!("{}:3:2", scratch.join("notes.md").display());

    // A file in a server-owned workspace opens at its position, through the launcher.
    ok(unary(
        &mut socket,
        &mut client,
        "shell.openInEditor",
        json!({"cwd": target, "editor": "vscode"}),
    )
    .await);
    let launched = launches.try_recv().expect("the editor was launched");
    assert_eq!(launched.command, editor);
    assert_eq!(launched.args[0], "--goto");
    assert!(
        launched.args[1].ends_with("notes.md:3:2"),
        "{:?}",
        launched.args
    );

    // UNC, device and verbatim paths never reach a process; neither does a path outside every
    // workspace the server owns. The launcher has answered before the RPC does.
    let outside = elsewhere.path().join("notes.md").display().to_string();
    for cwd in [
        "//attacker.invalid/share/notes.md",
        r"\\attacker.invalid\share\notes.md",
        r"/\attacker.invalid\share\notes.md",
        r"\\.\pipe\t3-editor",
        r"\\?\C:\Windows\win.ini",
        "//?/C:/Windows/win.ini",
        outside.as_str(),
    ] {
        let exit = unary(
            &mut socket,
            &mut client,
            "shell.openInEditor",
            json!({"cwd": cwd, "editor": "vscode"}),
        )
        .await;
        let error = failure(exit);
        assert_eq!(
            error["_tag"], "ExternalLauncherUnsupportedTargetError",
            "{cwd}: {error}"
        );
        assert_eq!(error["editor"], "vscode");
        assert!(launches.try_recv().is_err(), "{cwd} started a process");
    }
    let exit = unary(
        &mut socket,
        &mut client,
        "shell.openInEditor",
        json!({"cwd": target, "editor": "vscode", "projectRoot": "//attacker.invalid/share"}),
    )
    .await;
    assert_eq!(
        failure(exit)["_tag"],
        "ExternalLauncherUnsupportedTargetError"
    );
    assert!(launches.try_recv().is_err());

    // An editor this machine does not have.
    let exit = unary(
        &mut socket,
        &mut client,
        "shell.openInEditor",
        json!({"cwd": target, "editor": "zed"}),
    )
    .await;
    let error = failure(exit);
    assert_eq!(
        error["_tag"], "ExternalLauncherUnsupportedEditorError",
        "{error}"
    );
    assert!(launches.try_recv().is_err());

    let reader = server.cookie(&["orchestration:read"]).await;
    let (mut socket, mut client) = server.socket(&reader).await;
    let exit = unary(
        &mut socket,
        &mut client,
        "shell.openInEditor",
        json!({"cwd": target, "editor": "vscode"}),
    )
    .await;
    missing_scope(exit, "orchestration:operate");
    assert!(launches.try_recv().is_err());

    // Reveal: a file name PowerShell would read as code reaches the file manager as data.
    let (mut socket, mut client) = server.socket(&server.admin).await;
    let mut names = vec![
        "single'quote.txt",
        "left\u{2018}quote.txt",
        "x\u{2019}; Start-Process calc; \u{2019}y.txt",
        "low\u{201A}quote.txt",
        "reversed\u{201B}quote.txt",
        "semi;colon & amp.txt",
        "back`tick $(whoami).txt",
        "-leading-dash.txt",
        "comma,name.txt",
    ];
    if cfg!(unix) {
        names.push("pipe|bar.txt");
    }
    for name in names {
        let file = scratch.join(name);
        std::fs::write(&file, "x").unwrap();
        ok(unary(
            &mut socket,
            &mut client,
            "shell.openInEditor",
            json!({"cwd": file.display().to_string(), "editor": "file-manager", "reveal": true}),
        )
        .await);
        if cfg!(target_os = "linux") {
            // Linux asks the desktop over D-Bus first; the fallback opens the folder.
            while launches.try_recv().is_ok() {}
            continue;
        }
        let launched = launches.try_recv().expect("revealed");
        assert_eq!(launched.command, file_manager, "{name}");
        let last = launched.args.last().unwrap();
        if cfg!(windows) {
            assert_eq!(launched.args.len(), 1, "{name}");
            assert!(launched.verbatim);
            assert!(
                last.starts_with("/select,\"") && last.ends_with(&format!("{name}\"")),
                "{last}"
            );
        } else {
            assert_eq!(launched.args[0], "-R");
            assert!(last.ends_with(name), "{last}");
        }
        assert!(launches.try_recv().is_err(), "{name} started a second process");
    }

    // A link inside a workspace to a network share is refused before it is followed.
    let link = scratch.join("share");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(r"\\nonexistent.invalid\share", &link)
        .expect("directory symlinks need Developer Mode or the symlink privilege");
    #[cfg(unix)]
    std::os::unix::fs::symlink("//nonexistent.invalid/share", &link).unwrap();
    for editor in ["vscode", "file-manager"] {
        let exit = unary(
            &mut socket,
            &mut client,
            "shell.openInEditor",
            json!({"cwd": link.join("notes.md").display().to_string(), "editor": editor, "reveal": true}),
        )
        .await;
        assert_eq!(failure(exit)["_tag"], "ExternalLauncherUnsupportedTargetError");
        assert!(launches.try_recv().is_err());
    }
    server.runtime.shutdown().await.unwrap();
}

/// The production launcher starts a real detached process with discrete arguments: here Git,
/// whose `ls-remote` reaches a loopback listener (the event that marks the launch).
#[tokio::test]
async fn the_production_launcher_starts_a_detached_process() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/editor-launch", listener.local_addr().unwrap());
    t3_server::adapters::host_leftovers::editors::DetachedLaunch
        .launch(EditorCommand {
            command: PathBuf::from("git"),
            args: vec!["ls-remote".into(), url, "refs/heads/a b".into()],
            verbatim: false,
        })
        .await
        .unwrap();
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut head = vec![0; 512];
    let read = tokio::io::AsyncReadExt::read(&mut stream, &mut head)
        .await
        .unwrap();
    let head = String::from_utf8_lossy(&head[..read]);
    assert!(head.starts_with("GET /editor-launch/info/refs"), "{head}");
}

// ---------------------------------------------------------------------------------------------
// server.checkSecretSource

/// A fake 1Password CLI on the server's PATH: one readable reference.
fn fake_op(bin: &Path) {
    std::fs::create_dir_all(bin).unwrap();
    #[cfg(windows)]
    std::fs::write(
        bin.join("op.cmd"),
        "@echo off\r\nset hit=\r\nfor %%a in (%*) do if \"%%~a\"==\"op://vault/item/field\" set hit=1\r\nif not defined hit exit /b 1\r\n<nul set /p=\"s3cret-value\"\r\nexit /b 0\r\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin.join("op");
        std::fs::write(
            &path,
            "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = \"op://vault/item/field\" ] && { printf %s s3cret-value; exit 0; }; done\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn path_env(bin: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([("PATH".to_owned(), bin.to_string_lossy().into_owned())]);
    if cfg!(windows) {
        env.insert("PATHEXT".into(), ".COM;.EXE;.BAT;.CMD".into());
        if let Ok(root) = std::env::var("SYSTEMROOT") {
            env.insert("SYSTEMROOT".into(), root);
        }
    }
    env
}

#[tokio::test]
async fn a_secret_source_check_reads_fresh_and_never_returns_the_value() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    fake_op(&home.join("bin"));
    settings(
        home,
        json!({"secretProviders": {"vault": {"kind": "one_password", "label": "Team vault", "config": {"account": "acct"}}}}),
    );
    let server = start(home, path_env(&home.join("bin")), StartOptions::default()).await;
    server.served(&["server.checkSecretSource"]);
    let (mut socket, mut client) = server.socket(&server.admin).await;

    let exit = unary(
        &mut socket,
        &mut client,
        "server.checkSecretSource",
        json!({"source": {"provider": "vault", "reference": "op://vault/item/field"}}),
    )
    .await;
    let encoded = serde_json::to_string(&exit).unwrap();
    assert!(!encoded.contains("s3cret-value"), "{encoded}");
    assert_eq!(ok(exit), json!({"_tag": "Resolved"}));

    // TypeScript's failure is a `Failed` result naming the provider, never a transport error.
    let exit = unary(
        &mut socket,
        &mut client,
        "server.checkSecretSource",
        json!({"source": {"provider": "vault", "reference": "op://vault/item/missing"}}),
    )
    .await;
    assert_eq!(
        ok(exit),
        json!({"_tag": "Failed", "detail": "Could not read this secret from \"Team vault\". Check the provider's settings and that its store is unlocked."})
    );

    let reader = server
        .cookie(&["orchestration:read", "providers:manage"])
        .await;
    let (mut socket, mut client) = server.socket(&reader).await;
    let exit = unary(
        &mut socket,
        &mut client,
        "server.checkSecretSource",
        json!({"source": {"provider": "vault", "reference": "op://vault/item/field"}}),
    )
    .await;
    missing_scope(exit, "settings:write");
    server.runtime.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// Cards: secrets.answerRequest and peerLinks.answerRequest

fn sample(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples")
        .join(format!("{name}.jsonl"));
    let text = std::fs::read_to_string(path).expect("sample fixture");
    serde_json::from_str(text.lines().next().expect("sample line")).expect("sample json")
}

fn provider_error(error: serde_json::Error) -> ProviderError {
    ProviderError::Operation {
        operation: ProviderOperation::Open,
        detail: error.to_string(),
    }
}

/// A provider whose turns start and keep running, so their run stays active.
struct Held {
    instance: ProviderInstanceId,
    capabilities: OrchestrationV2ProviderCapabilities,
    turns: mpsc::UnboundedSender<TurnInput>,
}
impl Held {
    fn new(turns: mpsc::UnboundedSender<TurnInput>) -> Arc<Self> {
        Arc::new(Self {
            instance: ProviderInstanceId("replay".into()),
            capabilities: serde_json::from_value(
                sample("OrchestrationV2ProviderSession")["capabilities"].clone(),
            )
            .unwrap(),
            turns,
        })
    }
}
#[async_trait]
impl ProviderAdapter for Held {
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
        Ok(Arc::new(HeldSession {
            session: serde_json::from_value(session).map_err(provider_error)?,
            subscribers: Mutex::new(Vec::new()),
            turns: self.turns.clone(),
        }))
    }
}

struct HeldSession {
    session: OrchestrationV2ProviderSession,
    subscribers: Mutex<Vec<mpsc::Sender<ProviderResult<ProviderEvent>>>>,
    turns: mpsc::UnboundedSender<TurnInput>,
}
#[async_trait]
impl SessionRuntime for HeldSession {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        &self.session
    }
    fn can_resume_after_restart(&self) -> bool {
        false
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
                thread["createdAt"] = "2026-11-14T12:00:00.000Z".into();
                thread["updatedAt"] = "2026-11-14T12:00:00.000Z".into();
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
    /// Reports the turn running and keeps it so; the test learns of it from `turns`.
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        let IdOrIndex::Id(provider_thread_id) = &input.provider_thread.id else {
            return Err(ProviderError::Unsupported("compact id"));
        };
        let turn = json!({"id":format!("provider-turn:{}", input.run_id.0),"providerThreadId":provider_thread_id,"nodeId":input.root_node_id,"runAttemptId":input.attempt_id,"nativeTurnRef":null,"ordinal":input.provider_turn_ordinal,"status":"running","startedAt":"2026-11-14T12:00:00.000Z","completedAt":null});
        let event = ProviderEvent {
            driver: ProviderDriverKind("replay".into()),
            kind: ProviderEventKind::Turn {
                provider_turn: Box::new(serde_json::from_value(turn).map_err(provider_error)?),
                thread_id: None,
            },
        };
        let subscribers = self.subscribers.lock().unwrap().clone();
        for subscriber in &subscribers {
            let _ = subscriber.send(Ok(event.clone())).await;
        }
        let _ = self.turns.send(input);
        Ok(())
    }
    async fn steer_turn(&self, _input: SteerInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("steer_turn"))
    }
    async fn interrupt_turn(&self, _input: InterruptInput) -> ProviderResult<()> {
        Ok(())
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

fn held_options(
    turns: mpsc::UnboundedSender<TurnInput>,
) -> (StartOptions, Arc<OnceLock<Arc<HostLeftoversAdapter>>>) {
    let slot = Arc::new(OnceLock::new());
    (
        StartOptions {
            providers: vec![Held::new(turns)],
            host_leftovers: HostLeftoversOptions {
                editors: None,
                started: Some(slot.clone()),
            },
            ..Default::default()
        },
        slot,
    )
}

/// The thread's turn items of one type, as JSON.
async fn cards(adapter: &HostLeftoversAdapter, kind: &str) -> Vec<Value> {
    let runtime = adapter
        .secrets
        .as_ref()
        .and_then(|secrets| secrets.cards.clone())
        .expect("secret cards")
        .runtime
        .clone();
    let projection = runtime
        .threads
        .records(
            THREAD.to_owned().into(),
            vec![RecordKind::TurnItems],
            RecordFilter {
                turn_item_types: Some(vec![kind.into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    serde_json::to_value(projection).unwrap()["turnItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn cards_are_answered_once_and_a_pending_link_request_grants_nothing_across_restart() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    settings(
        home,
        json!({"providerInstances": {"replay": {"driver": "replay", "enabled": true}}, "linkedEnvironmentsEnabled": true}),
    );
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let (turns, mut started) = mpsc::unbounded_channel();
    let (options, slot) = held_options(turns.clone());
    let server = start(home, Default::default(), options).await;
    server.served(&["secrets.answerRequest", "peerLinks.answerRequest"]);
    let adapter = slot.get().expect("started adapter").clone();

    // The project row a ProjectService commit would have written.
    let layout = t3_config::home::HomeLayout {
        base_dir: home.into(),
        state_dir: home.join("userdata"),
    };
    t3_persistence::rusqlite::Connection::open(layout.database())
        .unwrap()
        .execute(
            "INSERT INTO projection_projects
             (project_id, title, workspace_root, scripts_json, created_at, updated_at)
             VALUES ('project:cards', 'Cards', ?1, '[]', '2026-11-14T12:00:00.000Z', '2026-11-14T12:00:00.000Z')",
            [workspace.to_string_lossy()],
        )
        .unwrap();
    let (mut socket, mut client) = server.socket(&server.admin).await;
    ok(unary(
        &mut socket,
        &mut client,
        "orchestration.dispatchCommand",
        json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":"project:cards","title":"Cards","modelSelection":{"instanceId":"replay","model":"replay-model"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
    )
    .await);
    ok(unary(
        &mut socket,
        &mut client,
        "orchestration.dispatchCommand",
        json!({"type":"message.dispatch","commandId":"command:message","threadId":THREAD,"messageId":"message:cards","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
    )
    .await);
    let turn = started.recv().await.expect("the turn started");

    // --- secrets.answerRequest: the agent's request_secret card, as its MCP tool records it.
    let secrets = adapter.secrets.clone().expect("secrets");
    let runtime = secrets.cards.clone().expect("cards").runtime.clone();
    let pending: t3_contracts::orchestration_v2::InternalCommandSecretRequestRecord =
        serde_json::from_value(json!({"commandId":"secret-request:turn-item:secret-1:pending","threadId":THREAD,"runId":turn.run_id,"nodeId":turn.root_node_id,"turnItemId":"turn-item:secret-1","label":"Deploy key","reason":"To deploy","secretStatus":"pending"})).unwrap();
    let receipt = runtime
        .threads
        .dispatch(
            t3_contracts::orchestration_v2::OrchestrationV2ServerCommand::Internal(Box::new(
                t3_contracts::orchestration_v2::OrchestrationV2InternalCommand::SecretRequestRecord(
                    Box::new(pending),
                ),
            )),
        )
        .await
        .unwrap();
    assert!(
        receipt.status == t3_orchestration::stores::receipts::ReceiptStatus::Accepted,
        "{:?}",
        receipt.error
    );
    let exit = unary(
        &mut socket,
        &mut client,
        "secrets.answerRequest",
        json!({"threadId": THREAD, "turnItemId": "turn-item:secret-1", "answer": {"type": "save", "secret": "hunter2-value"}}),
    )
    .await;
    assert_eq!(ok(exit), Value::Null);
    let saved = cards(&adapter, "secret_request").await;
    assert_eq!(saved[0]["secretStatus"], "saved", "{saved:?}");
    assert!(!Value::Array(saved).to_string().contains("hunter2-value"));
    let requests = secrets.requests.clone().expect("requests");
    assert!(
        requests
            .saved_ref(
                &THREAD.to_owned().into(),
                &"turn-item:secret-1".to_owned().into()
            )
            .unwrap()
            .is_some(),
        "the answer is in the secret store behind a one-use ref"
    );
    for (item, reason) in [
        ("turn-item:secret-1", "already_answered"),
        ("turn-item:absent", "not_found"),
    ] {
        let exit = unary(
            &mut socket,
            &mut client,
            "secrets.answerRequest",
            json!({"threadId": THREAD, "turnItemId": item, "answer": {"type": "decline"}}),
        )
        .await;
        let error = failure(exit);
        assert_eq!(error["_tag"], "SecretRequestError", "{error}");
        assert_eq!(error["reason"], reason, "{error}");
    }

    // --- peerLinks.answerRequest: the agent's t3_environment_link card.
    let peers = adapter.peers.clone().expect("peer links");
    let pending = peers
        .requests
        .request(
            &t3_cloud::link_requests::LinkCaller {
                thread_id: THREAD.to_owned().into(),
                provider_instance_id: ProviderInstanceId("replay".into()),
                namespace: "test".into(),
                linked_origin: false,
            },
            serde_json::from_value(json!({"url": "https://build.example.test", "reason": "Run the build there", "requestedAccess": "read-only", "clientRequestId": "link-1"})).unwrap(),
        )
        .await
        .unwrap();
    let card = serde_json::to_value(&pending).unwrap()["turnItemId"]
        .as_str()
        .unwrap()
        .to_owned();
    // A malformed answer is refused before it reaches the card.
    let exit = unary(
        &mut socket,
        &mut client,
        "peerLinks.answerRequest",
        json!({"threadId": THREAD, "turnItemId": card, "answer": {"type": "grant-everything"}}),
    )
    .await;
    assert!(
        failure(exit)
            .to_string()
            .contains("peerLinks.answerRequest")
    );
    let reader = server.cookie(&["orchestration:read", "access:read"]).await;
    let (mut read_socket, mut read_client) = server.socket(&reader).await;
    let exit = unary(
        &mut read_socket,
        &mut read_client,
        "peerLinks.answerRequest",
        json!({"threadId": THREAD, "turnItemId": card, "answer": {"type": "decline"}}),
    )
    .await;
    missing_scope(exit, "access:write");
    let exit = unary(
        &mut read_socket,
        &mut read_client,
        "secrets.answerRequest",
        json!({"threadId": THREAD, "turnItemId": "turn-item:secret-1", "answer": {"type": "decline"}}),
    )
    .await;
    missing_scope(exit, "orchestration:operate");
    drop((read_socket, socket));
    drop(adapter);
    drop(peers);
    drop(secrets);
    drop(requests);
    drop((runtime, slot));
    server.runtime.shutdown().await.unwrap();

    // The request was left unanswered across a restart: nothing is linked, the card is not.
    let (options, slot) = held_options(turns);
    let server = start(home, Default::default(), options).await;
    let adapter = slot.get().expect("started adapter").clone();
    let (mut socket, mut client) = server.socket(&server.admin).await;
    let listed = ok(unary(&mut socket, &mut client, "peerLinks.list", json!({})).await);
    assert_eq!(listed, json!({"links": []}));
    let links = cards(&adapter, "link_request").await;
    assert_eq!(links.len(), 1, "{links:?}");
    assert_ne!(links[0]["linkStatus"], "linked", "{links:?}");
    if links[0]["linkStatus"] == "pending" {
        ok(unary(
            &mut socket,
            &mut client,
            "peerLinks.answerRequest",
            json!({"threadId": THREAD, "turnItemId": card, "answer": {"type": "decline"}}),
        )
        .await);
        assert_eq!(
            cards(&adapter, "link_request").await[0]["linkStatus"],
            "declined"
        );
    }
    for (item, reason) in [
        (card.as_str(), "already_answered"),
        ("turn-item:absent", "not_found"),
    ] {
        let exit = unary(
            &mut socket,
            &mut client,
            "peerLinks.answerRequest",
            json!({"threadId": THREAD, "turnItemId": item, "answer": {"type": "decline"}}),
        )
        .await;
        let error = failure(exit);
        assert_eq!(error["_tag"], "PeerLinkRequestError", "{error}");
        assert_eq!(error["reason"], reason, "{error}");
    }
    let listed = ok(unary(&mut socket, &mut client, "peerLinks.list", json!({})).await);
    assert_eq!(listed, json!({"links": []}));
    drop((socket, adapter, slot));
    server.runtime.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// peerLinks.{list, link, unlink}

#[derive(Default)]
struct Remote {
    /// The access each accepted pairing decision asked for.
    decisions: Mutex<Vec<Value>>,
}

/// A loopback stand-in for another environment: its descriptor and MCP OAuth endpoints. The
/// pairing code `PAIR-OK` is accepted; any other is refused.
async fn remote() -> (String, Arc<Remote>, tokio::task::JoinHandle<()>) {
    use axum::{
        Json, Router,
        extract::State,
        http::StatusCode,
        routing::{get, post},
    };
    let state = Arc::new(Remote::default());
    let app = Router::new()
        .route(
            "/.well-known/t3/environment",
            get(|| async {
                Json(json!({"environmentId":"env-remote","label":"Build box","platform":{"os":"linux","arch":"x64"},"serverVersion":"1.0.0","capabilities":{"repositoryIdentity":true,"mcpModeLimitHeader":true,"linkFence":true}}))
            }),
        )
        .route(
            "/oauth/mcp/register",
            post(|| async {
                Json(json!({"client_id":"client-1","client_name":"T3 Code","redirect_uris":["http://127.0.0.1/t3code-peer-link"],"grant_types":["authorization_code"],"response_types":["code"],"token_endpoint_auth_method":"none"}))
            }),
        )
        .route(
            "/oauth/mcp/decision",
            post(
                |State(remote): State<Arc<Remote>>, Json(body): Json<Value>| async move {
                    if body["decision"]["code"] != "PAIR-OK" {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({"error":"access_denied","error_description":"wrong pairing code"})),
                        );
                    }
                    remote.decisions.lock().unwrap().push(body["decision"]["access"].clone());
                    let state = body["authorization"]["state"].as_str().unwrap_or_default();
                    (
                        StatusCode::OK,
                        Json(json!({"redirectTo": format!("http://127.0.0.1/t3code-peer-link?code=code-1&state={state}")})),
                    )
                },
            ),
        )
        .route(
            "/oauth/mcp/token",
            post(|| async {
                Json(json!({"access_token":"remote-token-value","token_type":"Bearer","expires_in":3600,"scope":"mcp"}))
            }),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (origin, state, task)
}

#[tokio::test]
async fn peer_links_are_listed_linked_with_the_requested_access_and_unlinked() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let (origin, remote, task) = remote().await;
    let link = |code: &str| json!({"url": origin, "pairingCode": code, "access": "read-only"});

    // Off in Settings → Labs: linking is refused, listing still works.
    settings(home, json!({"linkedEnvironmentsEnabled": false}));
    let server = start(home, Default::default(), StartOptions::default()).await;
    server.served(&["peerLinks.list", "peerLinks.link", "peerLinks.unlink"]);
    let (mut socket, mut client) = server.socket(&server.admin).await;
    let error = failure(unary(&mut socket, &mut client, "peerLinks.link", link("PAIR-OK")).await);
    assert_eq!(error["_tag"], "PeerLinkError", "{error}");
    assert_eq!(error["reason"], "disabled", "{error}");
    assert_eq!(
        ok(unary(&mut socket, &mut client, "peerLinks.list", json!({})).await),
        json!({"links": []})
    );
    assert!(remote.decisions.lock().unwrap().is_empty());
    drop(socket);
    server.runtime.shutdown().await.unwrap();

    settings(home, json!({"linkedEnvironmentsEnabled": true}));
    let server = start(home, Default::default(), StartOptions::default()).await;
    let (mut socket, mut client) = server.socket(&server.admin).await;

    // A wrong pairing code links nothing; plain http to another host is refused unsent.
    let error = failure(unary(&mut socket, &mut client, "peerLinks.link", link("WRONG")).await);
    assert_eq!(error["reason"], "pairing_rejected", "{error}");
    let error = failure(
        unary(
            &mut socket,
            &mut client,
            "peerLinks.link",
            json!({"url": "http://build.example.test", "pairingCode": "PAIR-OK", "access": "read-only"}),
        )
        .await,
    );
    assert_eq!(error["reason"], "unreachable", "{error}");
    assert!(remote.decisions.lock().unwrap().is_empty());

    // The link holds exactly the access asked for, and the token never reaches the client.
    let exit = unary(&mut socket, &mut client, "peerLinks.link", link("PAIR-OK")).await;
    let encoded = serde_json::to_string(&exit).unwrap();
    assert!(!encoded.contains("remote-token-value"), "{encoded}");
    let linked = ok(exit);
    assert_eq!(linked["environmentId"], "env-remote", "{linked}");
    assert_eq!(linked["access"], "read-only", "{linked}");
    assert_eq!(*remote.decisions.lock().unwrap(), vec![json!("read-only")]);
    let exit = unary(&mut socket, &mut client, "peerLinks.list", json!({})).await;
    assert!(
        !serde_json::to_string(&exit)
            .unwrap()
            .contains("remote-token-value")
    );
    let listed = ok(exit);
    assert_eq!(
        listed["links"][0]["environmentId"], "env-remote",
        "{listed}"
    );
    assert_eq!(listed["links"][0]["status"], "reachable", "{listed}");
    assert_eq!(listed["links"][0]["access"], "read-only", "{listed}");

    // Missing scopes: listing needs access:read, linking and unlinking access:write.
    let nothing = server.cookie(&["orchestration:read"]).await;
    let (mut other, mut other_client) = server.socket(&nothing).await;
    missing_scope(
        unary(&mut other, &mut other_client, "peerLinks.list", json!({})).await,
        "access:read",
    );
    let reader = server.cookie(&["orchestration:read", "access:read"]).await;
    let (mut other, mut other_client) = server.socket(&reader).await;
    missing_scope(
        unary(
            &mut other,
            &mut other_client,
            "peerLinks.link",
            link("PAIR-OK"),
        )
        .await,
        "access:write",
    );
    missing_scope(
        unary(
            &mut other,
            &mut other_client,
            "peerLinks.unlink",
            json!({"environmentId": "env-remote"}),
        )
        .await,
        "access:write",
    );

    let removed = ok(unary(
        &mut socket,
        &mut client,
        "peerLinks.unlink",
        json!({"environmentId": "env-remote"}),
    )
    .await);
    assert_eq!(removed, json!({"removed": true}));
    assert_eq!(
        ok(unary(&mut socket, &mut client, "peerLinks.list", json!({})).await),
        json!({"links": []})
    );
    let removed = ok(unary(
        &mut socket,
        &mut client,
        "peerLinks.unlink",
        json!({"environmentId": "env-remote"}),
    )
    .await);
    assert_eq!(removed, json!({"removed": false}));
    drop((socket, other));
    server.runtime.shutdown().await.unwrap();
    task.abort();
}
