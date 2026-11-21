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
        } else if cfg!(target_os = "macos") {
            assert_eq!(launched.args[0], "-R");
            assert!(last.ends_with(name), "{last}");
        } else {
            // Fixed test discovery does not hit session D-Bus (`reveal_linux` defaults to false),
            // so Linux reveal falls back to opening the containing scratch folder.
            assert_eq!(launched.args.len(), 1, "{name}");
            assert_eq!(
                Path::new(last),
                std::fs::canonicalize(&scratch).unwrap(),
                "{last}"
            );
        }
        assert!(
            launches.try_recv().is_err(),
            "{name} started a second process"
        );
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
        assert_eq!(
            failure(exit)["_tag"],
            "ExternalLauncherUnsupportedTargetError"
        );
        assert!(launches.try_recv().is_err());
    }
    // The UNC case above answers the same with either resolver: the contract carries no reason,
    // and following the link would only show on the network. This case tells them apart without
    // one: a link inside the workspace to `target::$INDEX_ALLOCATION`, which Windows opens as the
    // directory `target`. Following it first starts the editor on `target\notes.md`; the
    // hardened resolver refuses the `:` in the link's target before following it.
    #[cfg(windows)]
    {
        let target = scratch.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("notes.md"), "x").unwrap();
        let stream_link = scratch.join("stream_link");
        std::os::windows::fs::symlink_dir(
            format!("{}::$INDEX_ALLOCATION", target.display()),
            &stream_link,
        )
        .unwrap();
        for editor in ["vscode", "file-manager"] {
            let exit = unary(
                &mut socket,
                &mut client,
                "shell.openInEditor",
                json!({"cwd": stream_link.join("notes.md").display().to_string(), "editor": editor, "reveal": true}),
            )
            .await;
            assert_eq!(
                failure(exit)["_tag"],
                "ExternalLauncherUnsupportedTargetError"
            );
            assert!(
                launches.try_recv().is_err(),
                "{editor} was launched through the link"
            );
        }
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
        if let Some(cwd) = input.runtime_policy.cwd.as_ref() {
            session["cwd"] = json!(cwd);
        }
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

// ---------------------------------------------------------------------------------------------
// agentSessions.{scan, import}

fn recent_file(path: &Path, contents: String) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
}
fn same_dir(a: &str, b: &Path) -> bool {
    std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
}

/// The server's own home holds Claude and Codex sessions (no settings: both default instances);
/// scan finds them with TypeScript's fields, import creates the threads once.
#[tokio::test]
async fn agent_sessions_are_scanned_and_imported_once_from_the_server_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    // Outside the T3 base directory (this harness uses the home as base), which scan never offers.
    let elsewhere = tempfile::tempdir().unwrap();
    let workspace = elsewhere.path().join("agent-project");
    std::fs::create_dir_all(&workspace).unwrap();
    let claude_id = "33333333-3333-4333-8333-333333333333";
    let codex_id = "019a0000-0000-7000-8000-0000000000aa";
    recent_file(
        &home.join(".claude").join("projects").join("-agent-project").join(format!("{claude_id}.jsonl")),
        [
            json!({"type": "user", "cwd": workspace, "sessionId": claude_id, "timestamp": "2026-11-13T12:00:00.000Z", "message": {"role": "user", "content": "Fix the project"}}),
            json!({"type": "assistant", "sessionId": claude_id, "timestamp": "2026-11-13T12:01:00.000Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "Done"}]}}),
        ]
        .iter()
        .map(|r| format!("{r}\n"))
        .collect(),
    );
    recent_file(
        &home.join(".codex").join("sessions").join("2026").join("11").join("13").join(format!("rollout-2026-11-13T10-00-00-{codex_id}.jsonl")),
        [
            json!({"type": "session_meta", "payload": {"id": codex_id, "cwd": workspace}}),
            json!({"type": "event_msg", "timestamp": "2026-11-13T10:00:00.000Z", "payload": {"type": "user_message", "message": "Review this code"}}),
            json!({"type": "response_item", "timestamp": "2026-11-13T10:01:00.000Z", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Looks good"}]}}),
        ]
        .iter()
        .map(|r| format!("{r}\n"))
        .collect(),
    );
    let server = start(home, Default::default(), StartOptions::default()).await;
    server.served(&["agentSessions.scan", "agentSessions.import"]);
    let (mut socket, mut client) = server.socket(&server.admin).await;

    let scanned = ok(unary(&mut socket, &mut client, "agentSessions.scan", json!({})).await);
    let candidates = scanned["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1, "{scanned}");
    let candidate = &candidates[0];
    assert!(
        same_dir(candidate["path"].as_str().unwrap(), &workspace),
        "{candidate}"
    );
    assert_eq!(candidate["title"], "agent-project");
    assert_eq!(candidate["threadCount"], 2);
    assert_eq!(candidate["alreadyImported"], false);
    assert_eq!(candidate["git"], Value::Null);
    let mut sources: Vec<_> = candidate["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect();
    sources.sort();
    assert_eq!(sources, ["claudeAgent", "codex"]);
    assert!(scanned["scannedAt"].is_string());

    // The project a ProjectService commit would have written.
    let layout = t3_config::home::HomeLayout {
        base_dir: home.into(),
        state_dir: home.join("userdata"),
    };
    t3_persistence::rusqlite::Connection::open(layout.database())
        .unwrap()
        .execute(
            "INSERT INTO projection_projects
             (project_id, title, workspace_root, scripts_json, created_at, updated_at)
             VALUES ('project:agents', 'Agents', ?1, '[]', '2026-11-14T12:00:00.000Z', '2026-11-14T12:00:00.000Z')",
            [workspace.to_string_lossy()],
        )
        .unwrap();
    let scanned = ok(unary(&mut socket, &mut client, "agentSessions.scan", json!({})).await);
    assert_eq!(
        scanned["candidates"][0]["alreadyImported"], true,
        "{scanned}"
    );
    assert_eq!(scanned["candidates"][0]["projectId"], "project:agents");

    let imported = ok(unary(
        &mut socket,
        &mut client,
        "agentSessions.import",
        json!({"projectId": "project:agents"}),
    )
    .await);
    assert_eq!(imported, json!({"importedCount": 2, "skippedCount": 0}));
    let snapshot = |id: String| {
        let (http, base, cookie) = (
            server.http.clone(),
            server.base.clone(),
            server.admin.clone(),
        );
        async move {
            http.get(format!("{base}/api/orchestration/threads/{id}"))
                .header("cookie", cookie)
                .header("x-t3-orchestration-protocol", "2")
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let claude = snapshot(format!("import:claudeAgent:{claude_id}")).await;
    let text = claude.to_string();
    assert!(
        text.contains("Fix the project") && text.contains("Done"),
        "{text}"
    );
    let codex = snapshot(format!("import:codex:{codex_id}")).await;
    assert!(codex.to_string().contains("Review this code"), "{codex}");

    // A second import finds both already imported and adds nothing.
    let again = ok(unary(
        &mut socket,
        &mut client,
        "agentSessions.import",
        json!({"projectId": "project:agents"}),
    )
    .await);
    assert_eq!(again, json!({"importedCount": 2, "skippedCount": 0}));
    let messages = |snapshot: &Value| snapshot.to_string().matches("Fix the project").count();
    assert_eq!(
        messages(&snapshot(format!("import:claudeAgent:{claude_id}")).await),
        messages(&claude)
    );

    // TypeScript's typed failures.
    let error = failure(
        unary(
            &mut socket,
            &mut client,
            "agentSessions.import",
            json!({"projectId": "project:absent"}),
        )
        .await,
    );
    assert_eq!(
        error["_tag"], "AgentSessionImportProjectNotFoundError",
        "{error}"
    );
    assert_eq!(error["projectId"], "project:absent");
    let error = failure(
        unary(
            &mut socket,
            &mut client,
            "agentSessions.import",
            json!({"projectId": "project:agents", "expectedWorkspaceRoot": home.join("elsewhere").display().to_string()}),
        )
        .await,
    );
    assert_eq!(
        error["_tag"], "AgentSessionImportProjectChangedError",
        "{error}"
    );

    // Missing scopes: scan needs orchestration:read, import orchestration:operate.
    let none = server.cookie(&["access:read"]).await;
    let (mut other, mut other_client) = server.socket(&none).await;
    missing_scope(
        unary(
            &mut other,
            &mut other_client,
            "agentSessions.scan",
            json!({}),
        )
        .await,
        "orchestration:read",
    );
    let reader = server.cookie(&["orchestration:read"]).await;
    let (mut other, mut other_client) = server.socket(&reader).await;
    missing_scope(
        unary(
            &mut other,
            &mut other_client,
            "agentSessions.import",
            json!({"projectId": "project:agents"}),
        )
        .await,
        "orchestration:operate",
    );
    drop((socket, other));
    server.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn database_storage_capability_matches_served_methods() {
    let home = tempfile::tempdir().unwrap();
    let s = start(home.path(), Default::default(), Default::default()).await;
    let descriptor: Value = s
        .http
        .get(format!("{}/.well-known/t3/environment", s.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(descriptor["capabilities"]["databaseStorage"], true);
    for key in ["storageCleanup", "storageCleanupRun"] {
        assert!(
            descriptor["capabilities"].get(key).is_none(),
            "default cleanup must stay disabled: {key}"
        );
    }
    s.served(&["server.getDatabaseStorage", "server.reclaimDatabaseSpace"]);
    s.runtime.shutdown().await.unwrap();
}

/// Closing a session or releasing a writer can start an automatic sweep; the stream carries
/// its report before the manual one.
#[cfg(feature = "storage-cleanup")]
async fn manual_chunk(reader: &mut c_support::Socket, client: &mut t3_rpc::Client) -> Value {
    loop {
        let chunk = c_support::chunk(reader, client).await;
        if chunk["trigger"] == "manual" {
            return chunk;
        }
    }
}

/// Cleanup is opt-in: with the native fence enabled the same descriptor advertises it and the
/// manual run completes with the Git worker fence and a real forge registry.
#[cfg(all(feature = "storage-cleanup", feature = "forge"))]
#[tokio::test]
async fn native_cleanup_is_advertised_and_runs_only_when_enabled() {
    let home = tempfile::tempdir().unwrap();
    let s = start(
        home.path(),
        Default::default(),
        StartOptions {
            native_cleanup: true,
            ..Default::default()
        },
    )
    .await;
    let descriptor: Value = s
        .http
        .get(format!("{}/.well-known/t3/environment", s.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for key in ["storageCleanup", "storageCleanupRun"] {
        assert_eq!(descriptor["capabilities"][key], true, "{key}");
    }
    let (mut writer, mut client) = s.socket(&s.cookie(&["settings:write"]).await).await;
    let report = ok(unary(
        &mut writer,
        &mut client,
        "server.runStorageCleanup",
        json!({}),
    )
    .await);
    assert_eq!(report["trigger"], "manual", "{report}");
    drop(writer);
    s.runtime.shutdown().await.unwrap();
}

#[cfg(feature = "storage-cleanup")]
#[derive(Default)]
struct CleanupPermits(Arc<Mutex<Vec<(&'static str, bool)>>>);

#[cfg(feature = "storage-cleanup")]
struct PermitProbe(Arc<Mutex<Vec<(&'static str, bool)>>>, PathBuf);

#[cfg(feature = "storage-cleanup")]
impl Drop for PermitProbe {
    fn drop(&mut self) {
        self.0.lock().unwrap().push(("dropped", self.1.exists()));
    }
}

#[cfg(feature = "storage-cleanup")]
#[async_trait]
impl t3_server::adapters::storage_cleanup::CleanupWorktrees for CleanupPermits {
    async fn permit(&self, path: &Path) -> t3_host::Result<Box<dyn Send + Sync>> {
        // Records whether the checkout still exists when the permit is taken and dropped, so a
        // test can prove removal happened while the permit was held.
        self.0.lock().unwrap().push(("acquired", path.exists()));
        Ok(Box::new(PermitProbe(self.0.clone(), path.to_owned())))
    }
    async fn branch_pull_request(
        &self,
        _: &t3_git::Git,
        _: &str,
    ) -> t3_host::Result<Option<t3_host::storage_cleanup::MergedPullRequest>> {
        Ok(None)
    }
    async fn invalidate_git_status(&self, _: &Path) {}
}

#[cfg(feature = "storage-cleanup")]
#[tokio::test]
async fn storage_cleanup_obeys_scopes_and_preserves_projection_ownership() {
    let home = tempfile::tempdir().unwrap();
    settings(
        home.path(),
        json!({"providerInstances":{"replay":{"driver":"replay","enabled":true}}, "storageCleanup":{"browserArtifactsAfterDays":7,"logsAfterDays":7,"worktreeOnDelete":true,"worktreeAfterDays":7}}),
    );
    let gate = Arc::new(t3_server::writers::FolderGate::default());
    let (turns, _events) = mpsc::unbounded_channel();
    let (mut options, slot) = held_options(turns);
    let permits = Arc::new(CleanupPermits::default());
    options.writer_gate = Some(gate.clone());
    options.cleanup_worktrees = Some(permits.clone());
    // The terminal needs a shell: a minimal environment, no user profile.
    let shell_env = if cfg!(windows) {
        BTreeMap::new()
    } else {
        BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("SHELL".to_owned(), "/bin/sh".to_owned()),
        ])
    };
    let s = start(home.path(), shell_env, options).await;
    let descriptor: Value = s
        .http
        .get(format!("{}/.well-known/t3/environment", s.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!(
        "Cleanup host: {} unserved out of {} registered method names",
        s.runtime.unserved_methods.len(),
        t3_contracts::rpc::WS_METHODS.len()
    );
    for flag in ["storageCleanup", "storageCleanupRun"] {
        assert_eq!(
            descriptor["capabilities"][flag], true,
            "{flag}: {descriptor}"
        );
    }
    let (mut reader, mut read_client) = s.socket(&s.cookie(&["orchestration:read"]).await).await;
    let (mut writer, mut write_client) = s.socket(&s.cookie(&["settings:write"]).await).await;
    let report_method = "server.getStorageCleanupReport";
    let run_method = "server.runStorageCleanup";
    write_client
        .start(report_method, json!({}), t3_rpc::CallKind::Stream)
        .unwrap();
    let t3_rpc::CallEvent::Exit { exit, .. } =
        c_support::event(&mut writer, &mut write_client).await
    else {
        panic!("read permission was not enforced")
    };
    missing_scope(exit, "orchestration:read");
    read_client
        .start(report_method, json!({}), t3_rpc::CallKind::Stream)
        .unwrap();
    while c_support::chunk(&mut reader, &mut read_client)
        .await
        .is_null()
    {}
    let state = home.path().join("userdata");
    let old = state.join("browser-artifacts/expired.png");
    let rotated = state.join("logs/nested/tool.log.1");
    let current = state.join("logs/nested/tool.log");
    let nested = state.join("browser-artifacts/nested/keep.png");
    for path in [&old, &rotated, &current, &nested] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH).unwrap();
    }
    missing_scope(
        unary(&mut reader, &mut read_client, run_method, json!({})).await,
        "settings:write",
    );
    assert!(old.exists() && rotated.exists());
    let repo = home.path().join("repo");
    let worktree = home.path().join("worktrees/feature");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "root",
    ]);
    git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        worktree.to_str().unwrap(),
    ]);
    let db = t3_persistence::rusqlite::Connection::open(state.join("statev2.sqlite")).unwrap();
    let date = "2020-01-01T00:00:00.000Z";
    let mut thread: Value = serde_json::from_str(
        include_str!("../../../fixtures/samples/OrchestrationV2AppThread.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    for (key,v) in json!({"id":"owner","projectId":"p","title":"test","worktreePath":worktree,"branch":"feature","createdAt":date,"updatedAt":date,"settledAt":null,"settledOverride":null,"deletedAt":date,"lineage":{"rootThreadId":"owner","parentThreadId":null,"relationshipToParent":null}}).as_object().unwrap() {thread[key]=v.clone();}
    db.execute("INSERT INTO projection_projects(project_id,title,workspace_root,scripts_json,created_at,updated_at) VALUES ('p','project',?1,'[]',?2,?2)",[repo.to_str().unwrap(),date]).unwrap();
    db.execute("INSERT INTO orchestration_v2_projection_threads(thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,created_at,updated_at,deleted_at,payload_json) VALUES ('owner','p','test','codex','full-access','default',?1,?1,?1,?2)",[date,&thread.to_string()]).unwrap();
    db.execute("INSERT INTO orchestration_v2_effect_outbox(effect_id,command_id,thread_id,effect_type,payload_json,status,available_at,created_at,updated_at) VALUES ('job','cmd','owner','test','{}','failed',?1,?1,?1)",[date]).unwrap();
    let manual = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(manual["trigger"], "manual");
    assert_eq!(
        c_support::chunk(&mut reader, &mut read_client).await,
        manual
    );
    assert!(!old.exists() && !rotated.exists());
    assert!(current.exists() && nested.exists());
    assert!(
        worktree.exists(),
        "failed effects keep the checkout: {manual}"
    );
    db.execute(
        "UPDATE orchestration_v2_effect_outbox SET status='succeeded' WHERE effect_id='job'",
        [],
    )
    .unwrap();
    // An archived thread still owns its checkout after its project is soft-deleted.
    let mut archived = thread.clone();
    archived["deletedAt"] = Value::Null;
    archived["archivedAt"] = date.into();
    db.execute(
        "UPDATE projection_projects SET deleted_at=?1 WHERE project_id='p'",
        [date],
    )
    .unwrap();
    db.execute("UPDATE orchestration_v2_projection_threads SET archived_at=?1,deleted_at=NULL,payload_json=?2 WHERE thread_id='owner'",[date,&archived.to_string()]).unwrap();
    let kept = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(c_support::chunk(&mut reader, &mut read_client).await, kept);
    let row = kept["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["threadId"] == "owner")
        .unwrap();
    assert_eq!(row["reason"], "Project is unavailable", "{kept}");
    assert!(row["projectPath"].is_null() && worktree.exists());
    db.execute("UPDATE orchestration_v2_projection_threads SET archived_at=NULL,deleted_at=?1,payload_json=?2 WHERE thread_id='owner'",[date,&thread.to_string()]).unwrap();
    db.execute("INSERT INTO projection_projects(project_id,title,workspace_root,scripts_json,created_at,updated_at) VALUES ('other','other',?1,'[]',?2,?2)",[repo.to_str().unwrap(),date]).unwrap();
    let (mut admin, mut admin_client) = s.socket(&s.admin).await;
    ok(unary(&mut admin,&mut admin_client,"orchestration.dispatchCommand", json!({"type":"thread.create","commandId":"create:other","threadId":"other-thread","projectId":"other","title":"Other","modelSelection":{"instanceId":"replay","model":"replay-model"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"})).await);
    let nested = worktree.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let terminal = ok(unary(
        &mut admin,
        &mut admin_client,
        "terminal.open",
        json!({"threadId":"other-thread","terminalId":"cleanup-probe","cwd":nested}),
    )
    .await);
    assert_eq!(terminal["status"], "running", "{terminal}");
    let kept = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(c_support::chunk(&mut reader, &mut read_client).await, kept);
    assert!(
        kept["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["threadId"] == "owner"
                && e["reason"]
                    .as_str()
                    .is_some_and(|s| s.to_ascii_lowercase().contains("open terminal"))),
        "{kept}"
    );
    assert!(worktree.join(".git").is_file());
    ok(unary(
        &mut admin,
        &mut admin_client,
        "terminal.close",
        json!({"threadId":"other-thread","terminalId":"cleanup-probe","deleteHistory":true}),
    )
    .await);
    let runtime = &slot
        .get()
        .unwrap()
        .secrets
        .as_ref()
        .unwrap()
        .cards
        .as_ref()
        .unwrap()
        .runtime;
    let session_id = t3_contracts::base_schemas::ProviderSessionId("cleanup-session".into());
    let native = runtime
        .sessions
        .attach_with_policy(
            ProviderInstanceId("replay".into()),
            OpenSession {
                thread_id: "other-thread".to_owned().into(),
                provider_session_id: session_id.clone(),
                model_selection: serde_json::from_value(
                    json!({"instanceId":"replay","model":"replay-model"}),
                )
                .unwrap(),
                runtime_policy: RuntimePolicy {
                    runtime_mode: serde_json::from_value(json!("full-access")).unwrap(),
                    interaction_mode: serde_json::from_value(json!("default")).unwrap(),
                    cwd: Some(nested.clone()),
                    approval_policy: None,
                    sandbox_policy: None,
                    max_runtime_mode: None,
                },
                prompt_suggestions: None,
                resume_from_session: None,
                initial_native_thread_id: None,
                initial_provider_item_identity_version: None,
            },
            Default::default(),
        )
        .await
        .unwrap();
    assert!(runtime.sessions.is_live(&session_id));
    let kept = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(c_support::chunk(&mut reader, &mut read_client).await, kept);
    assert!(
        kept["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["threadId"] == "owner"
                && e["reason"].as_str().is_some_and(|s| s
                    .to_ascii_lowercase()
                    .contains("provider session is still open"))),
        "{kept}"
    );
    assert!(worktree.join(".git").is_file());
    native.close().await.unwrap();
    assert!(!runtime.sessions.is_live(&session_id));
    let host_writer = gate.enter("Git action", None, Some(&worktree)).await;
    let kept = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(manual_chunk(&mut reader, &mut read_client).await, kept);
    assert!(
        worktree.join(".git").is_file(),
        "deleted under an admitted host writer: {kept}"
    );
    drop(host_writer);
    let launch = t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::WorkspaceReservations::global()
        .reserve(&worktree, t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::ReservationKind::TurnStart, &tokio_util::sync::CancellationToken::new()).await.unwrap();
    let kept = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(manual_chunk(&mut reader, &mut read_client).await, kept);
    assert!(
        worktree.join(".git").is_file(),
        "deleted under a launch reservation: {kept}"
    );
    drop(launch);
    db.execute("INSERT INTO orchestration_v2_projection_turn_items(turn_item_id,thread_id,ordinal,type,status,updated_at,payload_json) VALUES('running-command','owner',1,'command_execution','running',?1,'{}')", [date]).unwrap();
    let kept = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(manual_chunk(&mut reader, &mut read_client).await, kept);
    assert!(
        worktree.join(".git").is_file(),
        "deleted under a running command: {kept}"
    );
    db.execute("UPDATE orchestration_v2_projection_turn_items SET status='completed' WHERE turn_item_id='running-command'", []).unwrap();
    let after = ok(unary(&mut writer, &mut write_client, run_method, json!({})).await);
    assert_eq!(manual_chunk(&mut reader, &mut read_client).await, after);
    assert!(!worktree.exists(), "{after}");
    assert!(
        permits
            .0
            .lock()
            .unwrap()
            .ends_with(&[("acquired", true), ("dropped", false)]),
        "removal must finish under the checkout permit"
    );
    let payload: String = db
        .query_row(
            "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id='owner'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&payload).unwrap(), thread);
    drop(db);
    drop((reader, writer));
    s.runtime.shutdown().await.unwrap();
}
