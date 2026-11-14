//! Package A: the environment's forge registry over real sockets: the source-control methods,
//! project clones and `git.resolvePullRequest`. Forges are faked at the edges only: a fake `glab`
//! on the server's PATH, a loopback stand-in for github.com, and local Git remotes in temporary
//! folders. Nothing reaches a real forge, CLI configuration or credential.
#![cfg(all(
    feature = "local-services",
    feature = "workspace",
    feature = "git",
    feature = "forge",
    feature = "project"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::adapters::config::{ConfigInputs, ServerFlags};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const FORGE_METHODS: &[&str] = &[
    "sourceControl.lookupRepository",
    "sourceControl.cloneRepository",
    "sourceControl.publishRepository",
    "sourceControl.signInStart",
    "sourceControl.signInWait",
    "sourceControl.signInCancel",
    "sourceControl.appAccess",
    "sourceControl.starred",
    "sourceControl.star",
    "server.discoverSourceControl",
    "projectClone.start",
    "projectClone.cancel",
    "projectClone.retry",
    "subscribeProjectClones",
    "git.resolvePullRequest",
];

struct Server {
    temp: tempfile::TempDir,
    runtime: t3_server::runtime::RunningServer,
    origin: String,
    cookie: String,
    socket: Socket,
    client: Client,
    /// Stream chunks that arrived while a unary call was waiting for its exit.
    chunks: std::collections::VecDeque<(t3_rpc::RequestId, Vec<Value>)>,
}

/// Git for fixtures, isolated from the developer's global and system configuration.
fn git(cwd: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "user.name=t3", "-c", "user.email=t3@example.invalid"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join(".no-global-config"))
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repository(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    std::fs::write(path.join("hello.txt"), "hello\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "init"]);
}

fn bare(path: &Path, from: Option<&Path>) {
    let parent = path.parent().unwrap();
    match from {
        Some(source) => git(
            parent,
            &[
                "clone",
                "-q",
                "--bare",
                &source.to_string_lossy(),
                &path.to_string_lossy(),
            ],
        ),
        None => git(
            parent,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &path.to_string_lossy(),
            ],
        ),
    }
}

/// The directories of the real `git`, so the server's PATH has Git and nothing else of ours.
/// Git for Windows' `mingw64in` also needs its `usrin` (its helpers crash without it when
/// the test runs from Git Bash, whose `MSYSTEM` the server inherits).
fn git_directories() -> Vec<PathBuf> {
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    let directory = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .find(|directory| directory.join(name).is_file())
        .expect("git on PATH");
    let mut directories = vec![directory.clone()];
    if let Some(root) = directory
        .parent()
        .filter(|parent| {
            parent
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("mingw64"))
        })
        .and_then(Path::parent)
    {
        directories.push(root.join("usr").join("bin"));
    }
    directories
}

/// A fake `glab`: it logs `GITLAB_HOST` and its arguments, answers `mr view` with `mr.json` and
/// every API call with `project.json`.
fn fake_glab(bin: &Path, published: &Path) {
    std::fs::create_dir_all(bin).unwrap();
    // Also the `namespaces/acme` answer when publishing: it needs the namespace id.
    let project = json!({
        "id": 42,
        "path_with_namespace": "acme/web",
        "web_url": published.to_string_lossy(),
        "ssh_url_to_repo": "git@gitlab.com:acme/web.git",
    });
    std::fs::write(bin.join("project.json"), project.to_string()).unwrap();
    let request = json!({
        "iid": 7, "title": "Fixture change", "state": "opened",
        "web_url": "https://gitlab.com/acme/web/-/merge_requests/7",
        "source_branch": "feature", "target_branch": "main",
    });
    std::fs::write(bin.join("mr.json"), request.to_string()).unwrap();
    let mut listed = request.clone();
    listed["source_project_id"] = json!(42);
    listed["target_project_id"] = json!(42);
    listed["source_project"] = json!({"path_with_namespace": "acme/web"});
    listed["target_project"] = json!({"path_with_namespace": "acme/web"});
    std::fs::write(bin.join("mr-list.json"), json!([listed]).to_string()).unwrap();
    if cfg!(windows) {
        std::fs::write(
            bin.join("glab.cmd"),
            "@echo off\r\n\
             >>\"%~dp0glab-calls.txt\" echo %GITLAB_HOST% %1 %2\r\n\
             if \"%~1\"==\"--version\" (\r\n echo glab version 1.0.0\r\n exit /b 0\r\n)\r\n\
             if \"%~1\"==\"auth\" exit /b 1\r\n\
             if \"%~1 %~2\"==\"mr list\" goto list\r\n\
             if \"%~1\"==\"mr\" (\r\n type \"%~dp0mr.json\"\r\n exit /b 0\r\n)\r\n\
             type \"%~dp0project.json\"\r\n\
             exit /b 0\r\n\
             :list\r\n\
             set \"ARGS=%*\"\r\n\
             if not \"%ARGS:logged-out=%\"==\"%ARGS%\" (echo glab auth login 1>&2& exit /b 1)\r\n\
             type \"%~dp0mr-list.json\"\r\n",
        )
        .unwrap();
    } else {
        let script = bin.join("glab");
        std::fs::write(
            &script,
            "#!/bin/sh\nd=$(dirname \"$0\")\necho \"$GITLAB_HOST $1 $2\" >> \"$d/glab-calls.txt\"\n\
             case \"$1 $2 $3\" in *logged-out*) echo 'glab auth login' >&2; exit 1;; esac\n\
             case \"$1 $2\" in \"mr list\") cat \"$d/mr-list.json\"; exit 0;; esac\n\
             case \"$1\" in --version) echo 'glab version 1.0.0';; auth) exit 1;; \
             mr) cat \"$d/mr.json\";; *) cat \"$d/project.json\";; esac\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}

fn glab_calls(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("bin").join("glab-calls.txt"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().to_owned())
        .collect()
}

/// Requests the github.com stand-in received: method, path, authorization, body.
#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<Request>>>);
/// Method, path, authorization, body.
type Request = (String, String, String, String);
impl Recorded {
    fn all(&self) -> Vec<Request> {
        self.0.lock().unwrap().clone()
    }
}

async fn github_stand_in() -> (String, Recorded) {
    use axum::{
        http::{HeaderMap, Method, StatusCode, Uri},
        response::IntoResponse,
    };
    let recorded = Recorded::default();
    let log = recorded.clone();
    let app = axum::Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: String| {
            let log = log.clone();
            async move {
                let authorization = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                log.0.lock().unwrap().push((
                    method.to_string(),
                    uri.path().to_owned(),
                    authorization,
                    body,
                ));
                match (method.as_str(), uri.path()) {
                    ("POST", "/login/device/code") => (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        json!({
                            "device_code": "fixture-device", "user_code": "ABCD-1234",
                            "verification_uri": "https://github.com/login/device",
                            "expires_in": 900, "interval": 3600,
                        })
                        .to_string(),
                    )
                        .into_response(),
                    ("GET" | "PUT", "/user/starred/acme/web") => {
                        StatusCode::NO_CONTENT.into_response()
                    }
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), recorded)
}

impl Server {
    /// A server whose PATH holds the fake glab and Git only, with a GitHub token in settings for
    /// github.com and the stand-in in place of github.com.
    async fn start(github: &str) -> Self {
        Self::start_with(github, |_| (Vec::new(), Vec::new())).await
    }
    /// `extra` gives environment entries (later ones win) and PATH folders, from the home.
    async fn start_with(
        github: &str,
        extra: impl FnOnce(&Path) -> (Vec<(String, String)>, Vec<PathBuf>),
    ) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let (extra_env, extra_path) = extra(home);
        let published = home.join("published.git");
        bare(&published, None);
        fake_glab(&home.join("bin"), &published);
        let base = home.join(".t3");
        std::fs::create_dir_all(base.join("userdata")).unwrap();
        std::fs::write(
            base.join("userdata").join("settings.json"),
            json!({"sourceControlHosts": {"github": {"tokens": {"github.com": "fixture-token"}}}})
                .to_string(),
        )
        .unwrap();
        let path = std::env::join_paths(
            std::iter::once(home.join("bin"))
                .chain(git_directories())
                .chain(extra_path),
        )
        .unwrap();
        let env = ["SystemRoot", "ComSpec", "WINDIR"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
            .chain([
                ("PATH".to_owned(), path.to_string_lossy().into_owned()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    home.join(".no-global-config")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
                (
                    "T3CODE_GITHUB_APP_CLIENT_ID".into(),
                    "fixture-client".into(),
                ),
                ("T3CODE_DEV_GITHUB_ENDPOINT".into(), github.into()),
                ("GH_TOKEN".into(), "fixture-environment-token".into()),
            ])
            .chain(extra_env)
            .collect();
        let runtime = t3_server::runtime::start(ConfigInputs {
            flags: ServerFlags {
                base_dir: Some(base.to_string_lossy().into_owned()),
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
        })
        .await
        .unwrap();
        let origin = format!("http://{}", runtime.address);
        let paired = reqwest::Client::new()
            .post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({"credential": runtime.pairing_credential}))
            .send()
            .await
            .unwrap();
        let cookie = paired.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let socket = Self::connect(&origin, runtime.address, &cookie).await;
        Self {
            temp,
            runtime,
            origin,
            cookie,
            socket,
            client: Client::new(0, Default::default()),
            chunks: Default::default(),
        }
    }
    async fn connect(origin: &str, address: std::net::SocketAddr, cookie: &str) -> Socket {
        let ticket: Value = reqwest::Client::new()
            .post(format!("{origin}/api/auth/websocket-ticket/rpc"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        tokio_tungstenite::connect_async(format!(
            "ws://{address}/ws?orchestrationProtocol=2&wsTicket={}",
            ticket["ticket"].as_str().unwrap()
        ))
        .await
        .unwrap()
        .0
    }
    /// The account home; the T3 base is `~/.t3`, as in production.
    fn home(&self) -> &Path {
        self.temp.path()
    }
    fn base(&self) -> PathBuf {
        self.home().join(".t3")
    }
    /// Replaces this connection with one whose session holds only `scopes`.
    async fn reconnect_with(&mut self, scopes: &[&str]) {
        let http = reqwest::Client::new();
        let grant: Value = http
            .post(format!("{}/api/auth/pairing-token", self.origin))
            .header("cookie", &self.cookie)
            .json(&json!({ "scopes": scopes }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let paired = http
            .post(format!("{}/api/auth/browser-session", self.origin))
            .json(&json!({"credential": grant["credential"]}))
            .send()
            .await
            .unwrap();
        let cookie = paired.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        self.socket = Self::connect(&self.origin, self.runtime.address, &cookie).await;
        self.client = Client::new(0, Default::default());
        self.chunks.clear();
    }
    fn insert_project(&self, id: &str, root: &Path) {
        let layout = t3_config::home::HomeLayout {
            base_dir: self.base(),
            state_dir: self.base().join("userdata"),
        };
        t3_persistence::rusqlite::Connection::open(layout.database())
            .unwrap()
            .execute(
                "INSERT INTO projection_projects
                 (project_id, title, workspace_root, scripts_json, created_at, updated_at)
                 VALUES (?1, ?1, ?2, '[]', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                (id, root.to_string_lossy()),
            )
            .unwrap();
    }
    fn project_root(&self, id: &str) -> Option<String> {
        let layout = t3_config::home::HomeLayout {
            base_dir: self.base(),
            state_dir: self.base().join("userdata"),
        };
        t3_persistence::rusqlite::Connection::open(layout.database())
            .unwrap()
            .query_row(
                "SELECT workspace_root FROM projection_projects WHERE project_id = ?1",
                [id],
                |row| row.get(0),
            )
            .ok()
    }
    async fn flush(&mut self) {
        while let Some(message) = self.client.pop_outgoing() {
            let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                panic!("text frame expected");
            };
            self.socket.send(Message::Text(text.into())).await.unwrap();
        }
    }
    async fn event(&mut self) -> CallEvent {
        loop {
            self.flush().await;
            if let Some(event) = self.client.pop_event() {
                return event;
            }
            let frame = match self.socket.next().await.unwrap().unwrap() {
                Message::Text(text) => Frame::Text(text.to_string()),
                Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
                Message::Ping(payload) => {
                    self.socket.send(Message::Pong(payload)).await.unwrap();
                    continue;
                }
                other => panic!("connection ended: {other:?}"),
            };
            for message in t3_rpc::codec::decode(&frame).unwrap() {
                self.client.receive(message, 0);
            }
        }
    }
    /// A unary call's exit; chunks of an open stream are kept for [`Server::chunk`].
    async fn call(&mut self, tag: &str, payload: Value) -> Exit {
        self.call_as(tag, payload, CallKind::Unary).await
    }
    async fn call_as(&mut self, tag: &str, payload: Value, kind: CallKind) -> Exit {
        let id = self.client.start(tag, payload, kind).unwrap();
        let mut pending = Vec::new();
        let exit = loop {
            match self.event().await {
                CallEvent::Exit { id: done, exit } if done == id => break exit,
                CallEvent::Chunk { id, values } => pending.push((id, values)),
                _ => {}
            }
        };
        for (id, values) in pending {
            self.chunks.push_back((id, values));
        }
        exit
    }
    /// The next chunk of a stream, acknowledged as a real client does.
    async fn chunk(&mut self) -> Value {
        if let Some((id, values)) = self.chunks.pop_front() {
            self.client.acknowledge(id);
            return values.into_iter().next().unwrap();
        }
        match self.event().await {
            CallEvent::Chunk { id, values } => {
                self.client.acknowledge(id);
                values.into_iter().next().unwrap()
            }
            other => panic!("unexpected {other:?}"),
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

/// The first failure (`Fail` error or `Die` defect) as sent on the wire.
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

/// Refused by the RPC guard for lacking `scope`.
fn scope_refused(exit: Exit, scope: &str) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    let required = if error["requiredPermission"].is_string() {
        &error["requiredPermission"]
    } else {
        &error["requiredScope"]
    };
    assert_eq!(required, scope, "{error}");
}

/// Refused because the cwd is not one of the environment's workspaces.
fn outside_workspace(exit: Exit) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("outside this session's authorized project workspaces"),
        "{error}"
    );
}

/// Refused because the server does not clone into that folder: a repository failure, not a
/// missing scope.
fn destination_refused(exit: Exit) {
    let error = failure(exit);
    assert_eq!(error["_tag"], "SourceControlRepositoryError", "{error}");
    assert_eq!(error["operation"], "cloneRepository", "{error}");
    assert!(
        error["detail"]
            .as_str()
            .unwrap()
            .starts_with("This folder cannot be a clone destination."),
        "{error}"
    );
}

#[tokio::test]
async fn repositories_discovery_and_pull_requests_go_through_the_one_forge_registry() {
    let (github, _) = github_stand_in().await;
    let mut s = Server::start(&github).await;
    for method in FORGE_METHODS {
        assert!(
            !s.runtime.unserved_methods.contains(method),
            "{method} is unserved"
        );
    }

    // Lookup runs glab against the server's GitLab host only.
    let found = ok(s
        .call(
            "sourceControl.lookupRepository",
            json!({"provider": "gitlab", "repository": "acme/web"}),
        )
        .await);
    assert_eq!(found["nameWithOwner"], "acme/web");
    assert_eq!(found["sshUrl"], "git@gitlab.com:acme/web.git");
    let calls = glab_calls(s.home());
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("gitlab.com api"), "{calls:?}");
    // A repository URL naming another host never reaches glab (the review's client-chosen host).
    let foreign = failure(
        s.call(
            "sourceControl.lookupRepository",
            json!({"provider": "gitlab", "repository": "https://attacker.example/a/b"}),
        )
        .await,
    );
    assert_eq!(foreign["_tag"], "SourceControlRepositoryError");
    assert_eq!(glab_calls(s.home()).len(), 1);
    let unknown = failure(
        s.call(
            "sourceControl.lookupRepository",
            json!({"provider": "unknown", "repository": "acme/web"}),
        )
        .await,
    );
    assert_eq!(unknown["_tag"], "SourceControlRepositoryError");
    assert_eq!(
        unknown["detail"],
        "Choose a source control provider before continuing."
    );
    // A cwd the client names must be a workspace.
    let outside = s.home().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    outside_workspace(
        s.call(
            "sourceControl.lookupRepository",
            json!({"provider": "gitlab", "repository": "acme/web", "cwd": outside.to_string_lossy()}),
        )
        .await,
    );
    assert_eq!(glab_calls(s.home()).len(), 1);
    // Clone from a local remote into a new folder.
    let seed = s.home().join("seed");
    repository(&seed);
    let remote = s.home().join("remote.git");
    bare(&remote, Some(&seed));
    let destination = s.home().join("code").join("web");
    let cloned = ok(s
        .call(
            "sourceControl.cloneRepository",
            json!({"remoteUrl": remote.to_string_lossy(), "destinationPath": destination.to_string_lossy()}),
        )
        .await);
    assert_eq!(
        std::fs::read_to_string(destination.join("hello.txt")).unwrap(),
        "hello\n"
    );
    assert!(cloned["cwd"].as_str().unwrap().ends_with("web"));
    // A second clone into the same, now non-empty, folder fails with the service's reason.
    let occupied = failure(
        s.call(
            "sourceControl.cloneRepository",
            json!({"remoteUrl": remote.to_string_lossy(), "destinationPath": destination.to_string_lossy()}),
        )
        .await,
    );
    assert_eq!(
        occupied["detail"],
        "Destination path already exists and is not empty."
    );
    // Publish creates the repository through glab, adds it as origin and pushes.
    let project = s.home().join("project");
    repository(&project);
    let publish = json!({
        "cwd": project.to_string_lossy(), "provider": "gitlab", "repository": "acme/web",
        "visibility": "private", "protocol": "https",
    });
    outside_workspace(
        s.call("sourceControl.publishRepository", publish.clone())
            .await,
    );
    s.insert_project("project-1", &project);
    let published = ok(s.call("sourceControl.publishRepository", publish).await);
    assert_eq!(published["status"], "pushed");
    assert_eq!(published["remoteName"], "origin");
    let pushed = std::process::Command::new("git")
        .args([
            "--git-dir",
            &s.home().join("published.git").to_string_lossy(),
        ])
        .args(["rev-parse", "--verify", "refs/heads/main"])
        .output()
        .unwrap();
    assert!(pushed.status.success(), "main was not pushed");

    // git.resolvePullRequest reads the checkout's own forge (its origin is on gitlab.com).
    let review = s.home().join("review");
    repository(&review);
    git(
        &review,
        &["remote", "add", "origin", "git@gitlab.com:acme/web.git"],
    );
    let resolve = json!({"cwd": review.to_string_lossy(), "reference": "7"});
    outside_workspace(s.call("git.resolvePullRequest", resolve.clone()).await);
    s.insert_project("review-1", &review);
    let resolved = ok(s.call("git.resolvePullRequest", resolve).await);
    assert_eq!(resolved["pullRequest"]["number"], 7);
    assert_eq!(resolved["pullRequest"]["headBranch"], "feature");
    assert!(
        glab_calls(s.home())
            .iter()
            .any(|call| call == "gitlab.com mr view"),
        "{:?}",
        glab_calls(s.home())
    );

    // Discovery probes Git and every registered forge through the server's PATH.
    let discovered = ok(s.call("server.discoverSourceControl", json!({})).await);
    assert_eq!(discovered["versionControlSystems"][0]["kind"], "git");
    assert_eq!(
        discovered["versionControlSystems"][0]["status"],
        "available"
    );
    let providers = discovered["sourceControlProviders"].as_array().unwrap();
    let gitlab = providers
        .iter()
        .find(|provider| provider["kind"] == "gitlab")
        .unwrap();
    assert_eq!(gitlab["status"], "available");
    assert!(
        providers
            .iter()
            .any(|provider| provider["kind"] == "github")
    );

    // Each method keeps its contract scope.
    s.reconnect_with(&["orchestration:read"]).await;
    let clone_request = json!({"remoteUrl": remote.to_string_lossy(), "destinationPath": s.home().join("code").join("other").to_string_lossy()});
    scope_refused(
        s.call("sourceControl.cloneRepository", clone_request).await,
        "source-control:write",
    );
    scope_refused(
        s.call(
            "sourceControl.publishRepository",
            json!({"cwd": project.to_string_lossy(), "provider": "gitlab", "repository": "acme/web", "visibility": "private"}),
        )
        .await,
        "source-control:write",
    );
    assert!(!s.home().join("code").join("other").exists());
    s.reconnect_with(&["source-control:write"]).await;
    scope_refused(
        s.call(
            "sourceControl.lookupRepository",
            json!({"provider": "gitlab", "repository": "acme/web"}),
        )
        .await,
        "orchestration:read",
    );
    scope_refused(
        s.call("server.discoverSourceControl", json!({})).await,
        "orchestration:read",
    );
    scope_refused(
        s.call(
            "git.resolvePullRequest",
            json!({"cwd": review.to_string_lossy(), "reference": "7"}),
        )
        .await,
        "orchestration:read",
    );
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

/// Review L2 and re-review finding 3: whatever names a GitHub host the server is not configured
/// for (a URL, `host/owner/repo`, an SCP or `ssh://` remote, odd case or a trailing dot, or a
/// checkout's remote), the server's `GH_TOKEN` (which gh sends to any `*.ghe.com`) never leaves:
/// lookup, clone, publish, project clone and pull-request resolution. github.com is still asked.
#[tokio::test]
async fn github_hosts_the_server_is_not_configured_for_never_get_its_token() {
    let (github, recorded) = github_stand_in().await;
    let mut s = Server::start(&github).await;
    let leaked = |recorded: &Recorded| {
        recorded
            .all()
            .into_iter()
            .filter(|(_, _, authorization, _)| authorization.contains("fixture-environment-token"))
            .collect::<Vec<_>>()
    };
    let forms = [
        "https://fixture.ghe.com/acme/web",
        "fixture.ghe.com/acme/web",
        "FIXTURE.GHE.com/acme/web",
        "https://Fixture.GHE.com/acme/web",
        "https://fixture.ghe.com./acme/web",
        "git@fixture.ghe.com:acme/web.git",
        "ssh://git@fixture.ghe.com/acme/web.git",
    ];
    for (index, form) in forms.iter().enumerate() {
        let _ = s
            .call(
                "sourceControl.lookupRepository",
                json!({"provider": "github", "repository": form}),
            )
            .await;
        let destination = s.home().join("code").join(format!("ghe-{index}"));
        let _ = s
            .call(
                "sourceControl.cloneRepository",
                json!({"provider": "github", "repository": form, "destinationPath": destination.to_string_lossy()}),
            )
            .await;
        assert!(!destination.join(".git").exists(), "{form}");
        assert_eq!(leaked(&recorded), Vec::new(), "{form}");
    }
    let _ = s
        .call(
            "projectClone.start",
            json!({
                "projectId": "ghe-project", "title": "Web", "createdAt": "2026-11-14T00:00:00.000Z",
                "provider": "github", "repository": "fixture.ghe.com/acme/web",
                "destinationPath": s.home().join("clones").join("ghe").to_string_lossy(),
            }),
        )
        .await;
    assert_eq!(leaked(&recorded), Vec::new(), "projectClone.start");
    let project = s.home().join("project");
    repository(&project);
    s.insert_project("project-1", &project);
    let _ = s
        .call(
            "sourceControl.publishRepository",
            json!({"cwd": project.to_string_lossy(), "provider": "github", "repository": "fixture.ghe.com/acme/web", "visibility": "private"}),
        )
        .await;
    assert_eq!(leaked(&recorded), Vec::new(), "publishRepository");
    let review = s.home().join("review");
    repository(&review);
    git(
        &review,
        &[
            "remote",
            "add",
            "origin",
            "git@fixture.ghe.com:acme/web.git",
        ],
    );
    s.insert_project("review-1", &review);
    let _ = s
        .call(
            "git.resolvePullRequest",
            json!({"cwd": review.to_string_lossy(), "reference": "7"}),
        )
        .await;
    assert_eq!(leaked(&recorded), Vec::new(), "git.resolvePullRequest");
    // github.com keeps its own (settings) credential.
    let before = recorded.all().len();
    let _ = s
        .call(
            "sourceControl.lookupRepository",
            json!({"provider": "github", "repository": "https://github.com/acme/web"}),
        )
        .await;
    assert!(
        recorded.all()[before..]
            .iter()
            .any(|(_, _, authorization, _)| authorization.contains("Bearer fixture-token")),
        "{:?}",
        recorded.all()
    );
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

/// Re-review finding 1: a clone never creates a folder whose contents something later runs or
/// trusts, and ordinary folders stay destinations.
#[tokio::test]
async fn clones_never_land_where_code_or_configuration_is_trusted() {
    let (github, _) = github_stand_in().await;
    let mut s = Server::start_with(&github, |home| {
        (
            vec![
                // Git for Windows reads HOME when it differs from USERPROFILE.
                (
                    "HOME".into(),
                    home.join("git-home").to_string_lossy().into_owned(),
                ),
                (
                    "XDG_CONFIG_HOME".into(),
                    home.join("xdg").to_string_lossy().into_owned(),
                ),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    home.join("git-global")
                        .join("config")
                        .to_string_lossy()
                        .into_owned(),
                ),
                (
                    "PROGRAMDATA".into(),
                    home.join("program-data").to_string_lossy().into_owned(),
                ),
            ],
            // A PATH folder that does not exist yet.
            vec![home.join("future-tools").join("bin")],
        )
    })
    .await;
    let seed = s.home().join("seed");
    repository(&seed);
    let remote = s.home().join("remote.git");
    bare(&remote, Some(&seed));
    // An existing project, with no agent, hook or editor folders yet.
    let project = s.home().join("project");
    repository(&project);
    std::fs::remove_dir_all(project.join(".git").join("hooks")).unwrap();
    s.insert_project("project-1", &project);
    let mut refused = vec![
        project.join(".claude"),
        project.join(".git").join("hooks"),
        project.join(".husky").join("_"),
        project.join(".vscode"),
        project.join("src"),
        s.home().join("git-home").join(".config").join("git"),
        s.home().join("xdg").join("git"),
        s.home().join("git-global"),
        s.home().join("program-data").join("Git"),
        s.home().join("future-tools").join("bin"),
        s.home().join("future-tools"),
        s.home().join(".local").join("src").join("x"),
        s.base().join("worktrees").join("x"),
    ];
    if cfg!(windows) {
        let documents = s.home().join("Documents");
        refused.extend([
            documents.join("PowerShell"),
            documents
                .join("WindowsPowerShell")
                .join("Modules")
                .join("x"),
            // Windows drops a trailing dot and reads a stream suffix as the folder itself.
            documents.join("PowerShell."),
            documents.join("PowerShell::$INDEX_ALLOCATION"),
        ]);
    } else {
        refused.push(s.home().join("bin"));
    }
    for destination in &refused {
        destination_refused(
            s.call(
                "sourceControl.cloneRepository",
                json!({"remoteUrl": remote.to_string_lossy(), "destinationPath": destination.to_string_lossy()}),
            )
            .await,
        );
    }
    for destination in &refused[..4] {
        assert!(!destination.exists(), "{destination:?}");
    }
    assert!(!s.home().join("Documents").exists());
    assert!(!s.home().join("xdg").exists());
    assert!(!s.home().join("program-data").exists());
    assert!(!s.home().join("future-tools").exists());
    // Ordinary folders: under home, and folders of the T3 base that are not T3's own (a base may
    // be a work folder), with a managed folder written in another case.
    for destination in [
        s.home().join("code").join("web"),
        s.base().join("work").join("web"),
        s.base().join("PROJECTS").join("web"),
    ] {
        ok(s.call(
            "sourceControl.cloneRepository",
            json!({"remoteUrl": remote.to_string_lossy(), "destinationPath": destination.to_string_lossy()}),
        )
        .await);
        assert!(destination.join("hello.txt").is_file(), "{destination:?}");
    }
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

/// Review M1: hidden and application folders of the home, and T3's base outside its managed
/// project folders, never receive a clone: a repository there could become Git's global
/// configuration, an SSH configuration, or a runtime or agent the server trusts.
#[tokio::test]
async fn clones_never_land_in_hidden_application_or_t3_folders() {
    let (github, _) = github_stand_in().await;
    let mut s = Server::start(&github).await;
    let seed = s.home().join("seed");
    repository(&seed);
    let remote = s.home().join("remote.git");
    bare(&remote, Some(&seed));
    for refused_destination in [
        s.base().join("userdata").join("secrets").join("web"),
        s.home().join(".config").join("git"),
        s.home().join(".ssh").join("keys"),
        s.home().join("AppData").join("Roaming").join("fixture"),
        s.base().join("runtime").join("versions").join("9.9.9"),
        s.base().join("tools").join("acp-registry").join("fixture"),
    ] {
        let _ = failure(
            s.call(
                "sourceControl.cloneRepository",
                json!({"remoteUrl": remote.to_string_lossy(), "destinationPath": refused_destination.to_string_lossy()}),
            )
            .await,
        );
        assert!(!refused_destination.exists(), "{refused_destination:?}");
    }
    assert!(!s.home().join(".config").exists());
    assert!(!s.home().join(".ssh").exists());
    assert!(!s.base().join("tools").exists());
    // T3's managed project folders and ordinary home folders are destinations.
    for destination in [
        s.base().join("projects").join("web"),
        s.home().join("code").join("web"),
    ] {
        ok(s.call(
            "sourceControl.cloneRepository",
            json!({"remoteUrl": remote.to_string_lossy(), "destinationPath": destination.to_string_lossy()}),
        )
        .await);
        assert!(destination.join("hello.txt").is_file());
    }
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

/// Review L4: a refused destination is a repository failure for a session that holds
/// `source-control:write`, not a missing permission.
#[tokio::test]
async fn refused_destinations_answer_a_repository_error() {
    let (github, _) = github_stand_in().await;
    let mut s = Server::start(&github).await;
    for refused_destination in [
        PathBuf::from(r"\\fixture-server\share\web"),
        s.base().join("userdata").join("secrets").join("web"),
        s.home().join(".config").join("git"),
    ] {
        destination_refused(
            s.call(
                "sourceControl.cloneRepository",
                json!({"remoteUrl": s.home().join("remote.git").to_string_lossy(), "destinationPath": refused_destination.to_string_lossy()}),
            )
            .await,
        );
    }
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

/// Review L5: calls close together share one round of probes.
#[tokio::test]
async fn discovery_probes_once_for_calls_close_together() {
    let (github, _) = github_stand_in().await;
    let mut s = Server::start(&github).await;
    let first = ok(s.call("server.discoverSourceControl", json!({})).await);
    let second = ok(s.call("server.discoverSourceControl", json!({})).await);
    assert_eq!(first, second);
    let probes = glab_calls(s.home())
        .iter()
        .filter(|call| call.ends_with("--version"))
        .count();
    assert_eq!(probes, 1);
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

/// The next status value of the open stream that carries a remote part.
async fn remote_status(s: &mut Server) -> Value {
    loop {
        let event = s.chunk().await;
        eprintln!("status event: {event}");
        if !event["remote"].is_null() {
            return event["remote"].clone();
        }
    }
}

/// VCS status carries the branch's pull request from the checkout's own forge, as TypeScript's
/// status does; a checkout with no forge, or a forge CLI that is signed out, leaves status working
/// with no pull request and the stream open.
#[tokio::test]
async fn vcs_status_carries_the_pull_request_from_the_checkouts_forge() {
    let (github, _) = github_stand_in().await;
    // Status fetches the upstream remote: `false` as ssh fails it locally, so gitlab.com is
    // never contacted.
    let mut s = Server::start_with(&github, |_| {
        (vec![("GIT_SSH_COMMAND".into(), "false".into())], Vec::new())
    })
    .await;
    let published = |branch: &str, name: &str, s: &Server| {
        let repo = s.home().join(name);
        repository(&repo);
        git(&repo, &["switch", "-q", "-c", branch]);
        git(
            &repo,
            &["remote", "add", "origin", "git@gitlab.com:acme/web.git"],
        );
        git(
            &repo,
            &[
                "update-ref",
                &format!("refs/remotes/origin/{branch}"),
                "HEAD",
            ],
        );
        git(
            &repo,
            &["branch", &format!("--set-upstream-to=origin/{branch}")],
        );
        repo
    };
    let with_request = published("feature", "with-request", &s);
    s.insert_project("with-request", &with_request);
    let cwd = json!({"cwd": with_request.to_string_lossy()});
    s.client
        .start("subscribeVcsStatus", cwd.clone(), CallKind::Stream)
        .unwrap();
    let remote = remote_status(&mut s).await;
    assert_eq!(
        remote["pr"]["number"],
        7,
        "{remote} {:?}",
        glab_calls(s.home())
    );
    assert_eq!(remote["pr"]["state"], "open", "{remote}");
    assert_eq!(remote["pr"]["headRef"], "feature", "{remote}");
    let refreshed = ok(s.call("vcs.refreshStatus", cwd).await);
    assert_eq!(refreshed["pr"]["number"], 7, "{refreshed}");
    assert!(
        glab_calls(s.home())
            .iter()
            .any(|call| call == "gitlab.com mr list"),
        "{:?}",
        glab_calls(s.home())
    );

    for (name, setup) in [("no-forge", None), ("signed-out", Some("logged-out"))] {
        let repo = match setup {
            // A remote no forge recognises: a local bare repository the branch tracks.
            None => {
                let repo = s.home().join(name);
                repository(&repo);
                let plain = s.home().join(format!("{name}.git"));
                bare(&plain, Some(&repo));
                git(
                    &repo,
                    &["remote", "add", "origin", &plain.to_string_lossy()],
                );
                git(&repo, &["fetch", "-q", "origin"]);
                git(&repo, &["branch", "--set-upstream-to=origin/main"]);
                repo
            }
            Some(branch) => published(branch, name, &s),
        };
        s.insert_project(name, &repo);
        s.reconnect_with(&["orchestration:read", "orchestration:operate"])
            .await;
        let cwd = json!({"cwd": repo.to_string_lossy()});
        s.client
            .start("subscribeVcsStatus", cwd.clone(), CallKind::Stream)
            .unwrap();
        let remote = remote_status(&mut s).await;
        assert!(remote["pr"].is_null(), "{name}: {remote}");
        let refreshed = ok(s.call("vcs.refreshStatus", cwd).await);
        assert!(refreshed["pr"].is_null(), "{name}: {refreshed}");
        // The stream is still open: a local change reaches it.
        std::fs::write(repo.join("new.txt"), "new\n").unwrap();
        ok(s.call(
            "vcs.refreshStatus",
            json!({"cwd": repo.to_string_lossy(), "localOnly": true}),
        )
        .await);
        loop {
            let event = s.chunk().await;
            if event["local"]["hasWorkingTreeChanges"] == true {
                break;
            }
        }
    }
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn github_sign_in_and_stars_address_only_configured_hosts() {
    let (github, recorded) = github_stand_in().await;
    let mut s = Server::start(&github).await;
    let host = |host: &str| json!({"provider": "github", "host": host});

    // The device flow starts against github.com (the stand-in) with the server's app client id.
    let prompt = ok(s
        .call("sourceControl.signInStart", host("github.com"))
        .await);
    assert_eq!(prompt["userCode"], "ABCD-1234");
    assert_eq!(prompt["verificationUri"], "https://github.com/login/device");
    assert!(recorded.all().iter().any(|(method, path, _, body)| {
        method == "POST"
            && path == "/login/device/code"
            && body.contains("client_id=fixture-client")
    }));
    // Cancel ends the pending flow; waiting on it then reports the cancellation.
    assert_eq!(
        ok(s.call("sourceControl.signInCancel", host("github.com"))
            .await),
        Value::Null
    );
    let waited = failure(s.call("sourceControl.signInWait", host("github.com")).await);
    assert_eq!(waited["_tag"], "SourceControlSignInError");
    assert_eq!(waited["reason"], "cancelled");
    let elsewhere = failure(
        s.call("sourceControl.signInStart", host("github.example.com"))
            .await,
    );
    assert_eq!(elsewhere["reason"], "unsupported");
    let unsupported = failure(
        s.call(
            "sourceControl.signInStart",
            json!({"provider": "gitlab", "host": "gitlab.com"}),
        )
        .await,
    );
    assert_eq!(unsupported["reason"], "unsupported");

    // Stars use the settings token for github.com.
    let repository =
        |host: &str| json!({"provider": "github", "host": host, "repository": "acme/web"});
    assert_eq!(
        ok(s.call("sourceControl.starred", repository("github.com"))
            .await),
        true
    );
    assert_eq!(
        ok(s.call("sourceControl.star", repository("github.com")).await),
        true
    );
    let starred: Vec<_> = recorded
        .all()
        .into_iter()
        .filter(|(_, path, _, _)| path == "/user/starred/acme/web")
        .collect();
    assert_eq!(
        starred
            .iter()
            .map(|(method, ..)| method.as_str())
            .collect::<Vec<_>>(),
        ["GET", "PUT"]
    );
    assert!(
        starred
            .iter()
            .all(|(_, _, authorization, _)| authorization.contains("fixture-token"))
    );
    // A host the server is not configured for gets nothing: no request, no credential.
    let before = recorded.all().len();
    assert_eq!(
        ok(
            s.call("sourceControl.starred", repository("fixture.ghe.com"))
                .await
        ),
        false
    );
    assert_eq!(
        ok(s.call("sourceControl.star", repository("fixture.ghe.com"))
            .await),
        false
    );
    assert_eq!(
        ok(
            s.call("sourceControl.appAccess", repository("fixture.ghe.com"))
                .await
        )["status"],
        "signed-out"
    );
    assert_eq!(recorded.all().len(), before);
    // github.com has no app sign-in saved, so the app has no access yet.
    assert_eq!(
        ok(s.call("sourceControl.appAccess", repository("github.com"))
            .await)["status"],
        "signed-out"
    );

    s.reconnect_with(&["orchestration:read"]).await;
    for method in [
        "sourceControl.signInStart",
        "sourceControl.signInWait",
        "sourceControl.signInCancel",
    ] {
        scope_refused(s.call(method, host("github.com")).await, "settings:write");
    }
    scope_refused(
        s.call("sourceControl.star", repository("github.com")).await,
        "source-control:write",
    );
    s.reconnect_with(&["source-control:write"]).await;
    scope_refused(
        s.call("sourceControl.starred", repository("github.com"))
            .await,
        "orchestration:read",
    );
    scope_refused(
        s.call("sourceControl.appAccess", repository("github.com"))
            .await,
        "orchestration:read",
    );
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}

/// Waits on the clone stream for `project`'s snapshot in `phase`.
async fn until_phase(s: &mut Server, project: &str, phase: &str) -> Value {
    loop {
        let list = s.chunk().await;
        if let Some(entry) = list
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["projectId"] == project && entry["phase"] == phase)
        {
            return entry.clone();
        }
    }
}

#[tokio::test]
async fn project_clones_stream_start_retry_and_end_with_the_socket() {
    let (github, _) = github_stand_in().await;
    let mut s = Server::start(&github).await;
    s.client
        .start("subscribeProjectClones", json!({}), CallKind::Stream)
        .unwrap();
    assert_eq!(s.chunk().await, json!([]));

    let seed = s.home().join("seed");
    repository(&seed);
    let remote = s.home().join("remote.git");
    bare(&remote, Some(&seed));
    let destination = s.home().join("clones").join("web");
    let start = |id: &str, remote: &Path, destination: &Path| {
        json!({
            "projectId": id, "title": "Web", "createdAt": "2026-11-14T00:00:00.000Z",
            "remoteUrl": remote.to_string_lossy(), "destinationPath": destination.to_string_lossy(),
        })
    };
    let started = ok(s
        .call(
            "projectClone.start",
            start("clone-1", &remote, &destination),
        )
        .await);
    assert_eq!(started["projectId"], "clone-1");
    let done = until_phase(&mut s, "clone-1", "done").await;
    assert!(done["destinationPath"].as_str().unwrap().ends_with("web"));
    assert_eq!(
        std::fs::read_to_string(destination.join("hello.txt")).unwrap(),
        "hello\n"
    );
    assert!(s.project_root("clone-1").unwrap().ends_with("web"));
    // A finished clone has nothing to cancel or retry.
    assert_eq!(
        ok(
            s.call("projectClone.cancel", json!({"projectId": "clone-1"}))
                .await
        )["applied"],
        false
    );
    assert_eq!(
        ok(
            s.call("projectClone.retry", json!({"projectId": "clone-1"}))
                .await
        )["applied"],
        false
    );

    // A remote that does not exist yet fails; retried once it exists, the clone completes.
    let late = s.home().join("late.git");
    let late_destination = s.home().join("clones").join("late");
    ok(s.call(
        "projectClone.start",
        start("clone-2", &late, &late_destination),
    )
    .await);
    let failed = until_phase(&mut s, "clone-2", "failed").await;
    assert!(!failed["error"].is_null());
    bare(&late, Some(&seed));
    assert_eq!(
        ok(
            s.call("projectClone.retry", json!({"projectId": "clone-2"}))
                .await
        )["applied"],
        true
    );
    until_phase(&mut s, "clone-2", "done").await;
    assert!(late_destination.join("hello.txt").is_file());

    // Destinations in T3's own state or Git's configuration folder are refused before a project
    // or folder exists.
    for (id, refused_destination) in [
        (
            "clone-3",
            s.base().join("userdata").join("secrets").join("web"),
        ),
        ("clone-5", s.home().join(".config").join("git")),
    ] {
        destination_refused(
            s.call(
                "projectClone.start",
                start(id, &remote, &refused_destination),
            )
            .await,
        );
        assert!(s.project_root(id).is_none());
        assert!(!refused_destination.exists());
    }

    s.reconnect_with(&["orchestration:read"]).await;
    scope_refused(
        s.call(
            "projectClone.start",
            start("clone-4", &remote, &s.home().join("clones").join("other")),
        )
        .await,
        "source-control:write",
    );
    scope_refused(
        s.call("projectClone.cancel", json!({"projectId": "clone-1"}))
            .await,
        "source-control:write",
    );
    scope_refused(
        s.call("projectClone.retry", json!({"projectId": "clone-1"}))
            .await,
        "source-control:write",
    );
    s.reconnect_with(&["source-control:write"]).await;
    scope_refused(
        s.call_as("subscribeProjectClones", json!({}), CallKind::Stream)
            .await,
        "orchestration:read",
    );
    // The stream held no server handle past its socket: shutdown joins cleanly.
    s.socket.close(None).await.unwrap();
    s.runtime.shutdown().await.unwrap();
}
