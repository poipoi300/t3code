#![cfg(all(feature = "local-services", feature = "git"))]
#![allow(clippy::unwrap_used)]
#[path = "c_support/mod.rs"]
mod support;

use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_contracts::{auth::EnvironmentAuthorizationError, base_schemas::AuthSessionId};
use t3_rpc::{Client, Exit};
use t3_server::{
    adapters::{
        config::{LocalServices, resolve},
        git::{ExecuteOptions, Git, GitAdapter, WorkspaceAuthority, WorktreeOptions},
    },
    services::{Domains, ServiceRegistry},
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Authority(Mutex<BTreeMap<String, Vec<PathBuf>>>);
impl WorkspaceAuthority for Authority {
    fn roots<'a>(
        &'a self,
        session: &'a AuthSessionId,
    ) -> BoxFuture<'a, Result<Vec<PathBuf>, EnvironmentAuthorizationError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&session.0)
                .cloned()
                .unwrap_or_default())
        })
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    git: Git,
    services: Arc<ServiceRegistry>,
    authority: Arc<Authority>,
    session: AuthSessionId,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    base: String,
    cookie: String,
    http: reqwest::Client,
}

impl Fixture {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let home = temp.path().join("home");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&home).unwrap();
        let environment = [
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
            ("XDG_CONFIG_HOME", home.to_string_lossy().into_owned()),
            (
                "GIT_CONFIG_GLOBAL",
                home.join("no-global").to_string_lossy().into_owned(),
            ),
            (
                "GIT_CONFIG_SYSTEM",
                home.join("no-system").to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("GIT_ATTR_NOSYSTEM", "1".into()),
            ("GIT_CONFIG_COUNT", "0".into()),
            ("GIT_AUTHOR_NAME", "Test".into()),
            ("GIT_AUTHOR_EMAIL", "test@example.invalid".into()),
            ("GIT_COMMITTER_NAME", "Test".into()),
            ("GIT_COMMITTER_EMAIL", "test@example.invalid".into()),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), Some(value.into())))
        .chain(
            ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG", "GIT_TEMPLATE_DIR"]
                .into_iter()
                .map(|key| (key.into(), None)),
        )
        .collect();
        let shutdown = CancellationToken::new();
        let git = Git::new(&repo)
            .with_environment(environment)
            .with_cancellation(shutdown.clone());
        git.execute(
            &["init", "--initial-branch=main"],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
        git.execute(
            &["config", "core.autocrlf", "false"],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
        std::fs::write(repo.join("file.txt"), "before\n").unwrap();
        git.commit_message("initial", None).await.unwrap().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let local = LocalServices::open(resolve(&support::inputs(&home)).unwrap(), address.port())
            .await
            .unwrap();
        let pairing = local.auth.startup_pairing_credential().await.unwrap();
        let authority = Arc::new(Authority::default());
        let mut domains = Domains::default();
        domains
            .insert_runtime(Arc::new(GitAdapter::new(
                git.clone(),
                authority.clone(),
                local.config.paths.state_dir.clone(),
                local.config.paths.secrets(),
            )))
            .unwrap();
        let services = Arc::new(ServiceRegistry {
            handlers: t3_server::handlers::registry_with(&domains).unwrap(),
            domains,
            local,
            shutdown: shutdown.clone(),
            connections: Default::default(),
            lifecycle: Default::default(),
        });
        let router = t3_server::http::router(services.clone());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        });
        let base = format!("http://{address}");
        let http = reqwest::Client::new();
        let cookie = support::pair(&http, &base, &pairing).await;
        let headers = [("cookie".parse().unwrap(), cookie.parse().unwrap())]
            .into_iter()
            .collect();
        let session = services
            .local
            .auth
            .authenticate(&t3_server::adapters::auth::request(
                "GET",
                &"/api/auth/session".parse().unwrap(),
                &headers,
                None,
            ))
            .await
            .unwrap()
            .session_id;
        authority
            .0
            .lock()
            .unwrap()
            .insert(session.0.clone(), vec![repo]);
        Self {
            temp,
            git,
            services,
            authority,
            session,
            task,
            base,
            cookie,
            http,
        }
    }

    async fn connect(&self, cookie: &str) -> (support::Socket, Client) {
        let ticket = support::ticket(&self.http, &self.base, cookie, "rpc").await;
        let url = format!(
            "{}/ws?orchestrationProtocol=2&wsTicket={ticket}",
            self.base.replacen("http", "ws", 1)
        );
        let (socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        (socket, Client::new(0, Default::default()))
    }

    async fn cookie_with_scopes(&self, scopes: &[&str]) -> String {
        let response = self
            .http
            .post(format!("{}/api/auth/pairing-token", self.base))
            .header("cookie", &self.cookie)
            .json(&json!({"scopes":scopes}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let value: Value = response.json().await.unwrap();
        support::pair(
            &self.http,
            &self.base,
            value["credential"].as_str().unwrap(),
        )
        .await
    }

    async fn session_for(&self, cookie: &str) -> AuthSessionId {
        let headers = [("cookie".parse().unwrap(), cookie.parse().unwrap())]
            .into_iter()
            .collect();
        self.services
            .local
            .auth
            .authenticate(&t3_server::adapters::auth::request(
                "GET",
                &"/api/auth/session".parse().unwrap(),
                &headers,
                None,
            ))
            .await
            .unwrap()
            .session_id
    }

    async fn shutdown(self) {
        self.services.shutdown.cancel();
        self.task.await.unwrap().unwrap();
        self.services.connections.close();
        self.services.connections.wait().await;
        self.services.domain::<GitAdapter>().unwrap().drain().await;
        let services = match Arc::try_unwrap(self.services) {
            Ok(s) => s,
            Err(_) => panic!("server still referenced"),
        };
        drop(services.domains);
        services.local.shutdown().await.unwrap();
    }
}

fn calls(cwd: &std::path::Path) -> Vec<(&'static str, Value)> {
    vec![
        ("vcs.pull", json!({"cwd":cwd})),
        ("vcs.listRefs", json!({"cwd":cwd})),
        ("vcs.createRef", json!({"cwd":cwd,"refName":"blocked"})),
        ("vcs.switchRef", json!({"cwd":cwd,"refName":"main"})),
        ("vcs.init", json!({"cwd":cwd})),
        (
            "vcs.removeWorktree",
            json!({"cwd":cwd,"path":cwd,"force":true}),
        ),
        ("review.getBranchCommits", json!({"cwd":cwd})),
        ("review.getDiffPreview", json!({"cwd":cwd})),
        (
            "review.getDiffFileContents",
            json!({"cwd":cwd,"sourceKind":"working-tree","changeType":"change","baseRef":null,"headRef":null,"oldPath":"file.txt","newPath":"file.txt"}),
        ),
    ]
}

fn failed(exit: Exit, tag: &str) -> Value {
    match exit {
        Exit::Failure { cause } => {
            let value = serde_json::to_value(cause).unwrap();
            assert!(value.to_string().contains(tag), "{value}");
            value
        }
        other => panic!("expected {tag}, got {other:?}"),
    }
}

#[tokio::test]
async fn startup_without_a_workspace_authority_reports_explicitly_unsupported_methods() {
    let fx = support::Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    for (method, payload) in calls(fx.temp.path()) {
        failed(
            support::unary(&mut socket, &mut client, method, payload).await,
            "NotImplemented",
        );
    }
    drop(socket);
    fx.shutdown().await;
}

#[tokio::test]
async fn every_git_rpc_rejects_missing_scopes_outside_projects_and_revoked_workspace_grants() {
    let fx = Fixture::start().await;
    let unauthenticated = fx
        .http
        .get(format!("{}/ws?orchestrationProtocol=2", fx.base))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);
    let limited = fx.cookie_with_scopes(&["settings:write"]).await;
    let limited_session = fx.session_for(&limited).await;
    fx.authority
        .0
        .lock()
        .unwrap()
        .insert(limited_session.0, vec![fx.git.cwd().into()]);
    let (mut socket, mut client) = fx.connect(&limited).await;
    for (method, payload) in calls(fx.git.cwd()) {
        let value = failed(
            support::unary(&mut socket, &mut client, method, payload).await,
            "EnvironmentAuthorizationError",
        );
        assert!(
            value.to_string().contains("requiredScope"),
            "{method}: {value}"
        );
        assert!(
            value.to_string().contains("missing required scope"),
            "{method}: {value}"
        );
    }
    drop(socket);
    let (mut socket, mut client) = fx.connect(&fx.cookie).await;
    let outside = fx.temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("file.txt"), "untouched").unwrap();
    for (method, payload) in calls(&outside) {
        failed(
            support::unary(&mut socket, &mut client, method, payload).await,
            "EnvironmentAuthorizationError",
        );
    }
    assert!(!outside.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("file.txt")).unwrap(),
        "untouched"
    );
    for cwd in ["relative/path", "C:\\invalid\0path"] {
        for (method, payload) in calls(std::path::Path::new(cwd)) {
            failed(
                support::unary(&mut socket, &mut client, method, payload).await,
                "EnvironmentAuthorizationError",
            );
        }
    }
    let output = fx
        .git
        .execute(
            &["show-ref", "--verify", "refs/heads/blocked"],
            ExecuteOptions {
                allow_nonzero: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_ne!(output.code, Some(0));
    fx.authority.0.lock().unwrap().clear();
    for (method, payload) in calls(fx.git.cwd()) {
        failed(
            support::unary(&mut socket, &mut client, method, payload).await,
            "EnvironmentAuthorizationError",
        );
    }
    drop(socket);
    fx.shutdown().await;
}

#[tokio::test]
async fn real_git_refs_review_and_provisioning_round_trip_over_effect_rpc() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie).await;
    let cwd = fx.git.cwd();
    let created = support::unary_ok(
        &mut socket,
        &mut client,
        "vcs.createRef",
        json!({"cwd":cwd,"refName":"topic","switchRef":true}),
    )
    .await;
    assert_eq!(created["refName"], "topic");
    assert_eq!(
        fx.git.current_branch().await.unwrap().as_deref(),
        Some("topic")
    );
    std::fs::write(cwd.join("file.txt"), "committed\n").unwrap();
    fx.git
        .commit_message("feature", None)
        .await
        .unwrap()
        .unwrap();
    let commits = support::unary_ok(
        &mut socket,
        &mut client,
        "review.getBranchCommits",
        json!({"cwd":cwd,"baseRef":"main"}),
    )
    .await;
    assert_eq!(commits["commits"].as_array().unwrap().len(), 1);
    assert_eq!(commits["commits"][0]["subject"], "feature");
    let refs = support::unary_ok(
        &mut socket,
        &mut client,
        "vcs.listRefs",
        json!({"cwd":cwd,"query":"topic","refKind":"local"}),
    )
    .await;
    assert_eq!(refs["isRepo"], true);
    assert_eq!(refs["refs"].as_array().unwrap().len(), 1);
    std::fs::write(cwd.join("file.txt"), "working\n").unwrap();
    let preview = support::unary_ok(
        &mut socket,
        &mut client,
        "review.getDiffPreview",
        json!({"cwd":cwd,"sourceKind":"working-tree"}),
    )
    .await;
    assert!(preview.to_string().contains("file.txt"), "{preview}");
    let (_, contents) = calls(cwd).pop().unwrap();
    let contents = support::unary_ok(
        &mut socket,
        &mut client,
        "review.getDiffFileContents",
        contents,
    )
    .await;
    assert_eq!(contents["oldContents"], "committed\n");
    assert_eq!(contents["newContents"], "working\n");
    std::fs::write(cwd.join("file.txt"), "committed\n").unwrap();
    let switched = support::unary_ok(
        &mut socket,
        &mut client,
        "vcs.switchRef",
        json!({"cwd":cwd,"refName":"main"}),
    )
    .await;
    assert_eq!(switched["refName"], "main");
    assert_eq!(
        fx.git.current_branch().await.unwrap().as_deref(),
        Some("main")
    );
    let new = fx.temp.path().join("new");
    std::fs::create_dir(&new).unwrap();
    fx.authority
        .0
        .lock()
        .unwrap()
        .get_mut(&fx.session.0)
        .unwrap()
        .push(new.clone());
    support::unary_ok(&mut socket, &mut client, "vcs.init", json!({"cwd":new})).await;
    assert!(fx.git.at(&new).is_repository().await.unwrap());
    failed(
        support::unary(
            &mut socket,
            &mut client,
            "vcs.init",
            json!({"cwd":new,"kind":"jj"}),
        )
        .await,
        "VcsUnsupportedOperationError",
    );
    drop(socket);
    fx.shutdown().await;
}

#[tokio::test]
async fn worktree_removal_authorizes_its_target_and_requires_membership_in_the_checkout() {
    let fx = Fixture::start().await;
    let linked = fx.temp.path().join("linked");
    fx.git
        .add_worktree(WorktreeOptions {
            new_branch: Some("linked".into()),
            ..WorktreeOptions::new(&linked, "main")
        })
        .await
        .unwrap();
    let (mut socket, mut client) = fx.connect(&fx.cookie).await;
    let payload = json!({"cwd":fx.git.cwd(),"path":linked,"force":true});
    failed(
        support::unary(
            &mut socket,
            &mut client,
            "vcs.removeWorktree",
            payload.clone(),
        )
        .await,
        "EnvironmentAuthorizationError",
    );
    assert!(linked.join("file.txt").exists());
    fx.authority
        .0
        .lock()
        .unwrap()
        .get_mut(&fx.session.0)
        .unwrap()
        .push(linked.clone());
    let unrelated = fx.temp.path().join("unrelated");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("keep"), "keep").unwrap();
    fx.authority
        .0
        .lock()
        .unwrap()
        .get_mut(&fx.session.0)
        .unwrap()
        .push(unrelated.clone());
    failed(
        support::unary(
            &mut socket,
            &mut client,
            "vcs.removeWorktree",
            json!({"cwd":fx.git.cwd(),"path":unrelated,"force":true}),
        )
        .await,
        "EnvironmentAuthorizationError",
    );
    assert_eq!(
        std::fs::read_to_string(unrelated.join("keep")).unwrap(),
        "keep"
    );
    let main = json!({"cwd":fx.git.cwd(),"path":fx.git.cwd(),"force":true});
    failed(
        support::unary(&mut socket, &mut client, "vcs.removeWorktree", main).await,
        "EnvironmentAuthorizationError",
    );
    support::unary_ok(&mut socket, &mut client, "vcs.removeWorktree", payload).await;
    assert!(!linked.exists());
    assert!(fx.git.cwd().join("file.txt").exists());
    drop(socket);
    fx.shutdown().await;
}

#[tokio::test]
async fn git_discovery_cannot_expand_a_project_grant_to_its_parent_repository() {
    let fx = Fixture::start().await;
    let nested = fx.git.cwd().join("nested");
    std::fs::create_dir(&nested).unwrap();
    fx.authority
        .0
        .lock()
        .unwrap()
        .insert(fx.session.0.clone(), vec![nested.clone()]);
    let (mut socket, mut client) = fx.connect(&fx.cookie).await;
    for (method, payload) in calls(&nested).into_iter().filter(|(m, _)| *m != "vcs.init") {
        failed(
            support::unary(&mut socket, &mut client, method, payload).await,
            "EnvironmentAuthorizationError",
        );
    }
    assert_eq!(
        fx.git.current_branch().await.unwrap().as_deref(),
        Some("main")
    );
    drop(socket);
    fx.shutdown().await;
}

#[tokio::test]
async fn linked_directories_cannot_escape_project_authorization() {
    let fx = Fixture::start().await;
    let outside = fx.temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("file.txt"), "outside\n").unwrap();
    let link = fx.git.cwd().join("escape");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside, &link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let (mut socket, mut client) = fx.connect(&fx.cookie).await;
    for (method, payload) in calls(&link) {
        failed(
            support::unary(&mut socket, &mut client, method, payload).await,
            "EnvironmentAuthorizationError",
        );
    }
    assert!(!outside.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("file.txt")).unwrap(),
        "outside\n"
    );
    drop(socket);
    fx.shutdown().await;
}

#[tokio::test]
async fn pull_uses_local_remote_and_git_failures_never_expose_remote_credentials() {
    let fx = Fixture::start().await;
    let bare = fx.temp.path().join("remote.git");
    fx.git
        .execute(
            &["init", "--bare", bare.to_str().unwrap()],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
    fx.git
        .execute(
            &["remote", "add", "origin", bare.to_str().unwrap()],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
    fx.git
        .execute(&["push", "-u", "origin", "main"], ExecuteOptions::default())
        .await
        .unwrap();
    let peer = fx.temp.path().join("peer");
    fx.git
        .execute(
            &[
                "clone",
                "--branch",
                "main",
                bare.to_str().unwrap(),
                peer.to_str().unwrap(),
            ],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
    let peer_git = fx.git.at(&peer);
    std::fs::write(peer.join("remote.txt"), "from remote\n").unwrap();
    peer_git
        .commit_message("remote change", None)
        .await
        .unwrap()
        .unwrap();
    peer_git
        .execute(&["push"], ExecuteOptions::default())
        .await
        .unwrap();
    let (mut socket, mut client) = fx.connect(&fx.cookie).await;
    let result = support::unary_ok(
        &mut socket,
        &mut client,
        "vcs.pull",
        json!({"cwd":fx.git.cwd()}),
    )
    .await;
    assert_eq!(result["status"], "pulled");
    assert_eq!(
        std::fs::read_to_string(fx.git.cwd().join("remote.txt")).unwrap(),
        "from remote\n"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().fallback(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    "https://user:private-token@example.invalid/private.git",
                )
            }),
        )
        .with_graceful_shutdown(stop.cancelled_owned())
        .await
        .unwrap();
    });
    let remote = format!("http://user:private-token@{address}/private.git");
    fx.git
        .execute(
            &["remote", "set-url", "origin", &remote],
            ExecuteOptions::default(),
        )
        .await
        .unwrap();
    let failure = failed(
        support::unary(
            &mut socket,
            &mut client,
            "vcs.pull",
            json!({"cwd":fx.git.cwd()}),
        )
        .await,
        "GitCommandError",
    );
    let encoded = failure.to_string();
    assert!(!encoded.contains("private-token"), "{failure}");
    assert!(!encoded.contains(&remote), "{failure}");
    cancel.cancel();
    task.await.unwrap();
    drop(socket);
    fx.shutdown().await;
}
