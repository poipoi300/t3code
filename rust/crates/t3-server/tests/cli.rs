#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
};

fn command(root: &Path) -> Command {
    program(root, env!("CARGO_BIN_EXE_t3"))
}

fn program(root: &Path, executable: &str) -> Command {
    let mut command = Command::new(executable);
    command.env_clear().current_dir(root).kill_on_drop(true);
    for key in [
        "PATH",
        "SystemRoot",
        "COMSPEC",
        "TEMP",
        "TMP",
        "SYSTEMDRIVE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("USERPROFILE", root.join("account"))
        .env("HOME", root.join("account"));
    command.env("TOKIO_WORKER_THREADS", "2");
    command
}

async fn output(root: &Path, args: &[&str]) -> std::process::Output {
    tokio::time::timeout(Duration::from_secs(60), command(root).args(args).output())
        .await
        .expect("CLI command did not finish")
        .expect("launch t3")
}
fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}
fn stderr(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

async fn launch(mut command: Command) -> (Child, String, String) {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut connection = None;
    let mut token = None;
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if let Some(value) = line.strip_prefix("Connection string: ") {
                connection = Some(value.to_owned());
            }
            if let Some(value) = line.strip_prefix("Token: ") {
                token = Some(value.to_owned());
            }
            if line.starts_with("App link: ") {
                return;
            }
        }
        panic!("server exited before announcing access details");
    })
    .await
    .expect("server did not announce readiness");
    (child, connection.unwrap(), token.unwrap())
}

#[tokio::test]
async fn help_version_and_syntax_errors_do_not_create_state() {
    let root = TempDir::new().unwrap();
    let help = output(root.path(), &["--help"]).await;
    assert!(help.status.success(), "{}", stderr(&help));
    for name in [
        "serve",
        "pair",
        "auth",
        "environment",
        "browser",
        "project",
        "thread",
        "status",
        "service",
        "policy",
        "theme",
        "trace",
        "triage",
    ] {
        assert!(stdout(&help).contains(name), "missing {name}");
    }
    let version = output(root.path(), &["--version"]).await;
    assert!(version.status.success());
    assert!(stdout(&version).contains(env!("CARGO_PKG_VERSION")));
    assert!(output(root.path(), &["serve", "-v"]).await.status.success());
    for args in [
        vec!["serve", "--port", "65536"],
        vec!["serve", "--port", "-1"],
        vec!["serve", "--mode", "other"],
        vec!["serve", "--host"],
        vec!["pair", "--scope", "everything"],
        vec!["pair", "--ttl", "banana"],
        vec!["thread", "send"],
        vec!["not-a-command"],
        vec!["serve", "--unknown"],
    ] {
        let result = output(root.path(), &args).await;
        assert_eq!(
            result.status.code(),
            Some(1),
            "{args:?}: {}",
            stderr(&result)
        );
        assert!(result.stdout.is_empty(), "{args:?}");
        assert!(!result.stderr.is_empty());
    }
    assert!(!root.path().join("account").exists());
    let invalid_env = command(root.path())
        .env("T3CODE_PORT", "invalid")
        .args(["serve", "--port", "0"])
        .output()
        .await
        .unwrap();
    assert_eq!(invalid_env.status.code(), Some(1));
    assert!(invalid_env.stdout.is_empty());
    assert!(!root.path().join("account").exists());
}

#[tokio::test]
async fn every_unassembled_command_is_recognized_and_fails_explicitly() {
    let root = TempDir::new().unwrap();
    let cases: &[&[&str]] = &[
        &["app", "."],
        &["acp-mcp-bridge"],
        &["acp-mcp-call", "tool", "{}"],
        &[
            "auth",
            "pairing",
            "create",
            "--scope",
            "orchestration:read",
            "--ttl",
            "1h",
            "--label",
            "phone",
            "--base-url",
            "https://example.com",
            "--json",
        ],
        &["auth", "pairing", "list", "--json"],
        &["auth", "pairing", "revoke", "id"],
        &[
            "auth",
            "session",
            "issue",
            "--subject",
            "bot",
            "--label",
            "bot",
            "--ttl",
            "30d",
            "--scope",
            "access:read",
            "--token-only",
            "--json",
        ],
        &["auth", "session", "list", "--json"],
        &["auth", "session", "revoke", "id"],
        &[
            "environment",
            "link",
            "http://127.0.0.1:1",
            "--url",
            "https://example.com",
            "--pairing-code",
            "code",
            "--access",
            "full-access",
        ],
        &["environment", "list", "--json"],
        &["environment", "unlink", "id"],
        &["browser", "setup"],
        &["project", "add", ".", "--title", "Title"],
        &["project", "remove", "id", "--force"],
        &["project", "rename", "id", "Title"],
        &["thread", "list", "--project", ".", "--json"],
        &[
            "thread",
            "start",
            ".",
            "hello",
            "--title",
            "Title",
            "--provider",
            "replay",
            "--model",
            "replay",
            "--worktree",
            "main",
            "--branch",
            "test",
            "--wait",
            "--json",
        ],
        &["thread", "send", "id", "hello", "--wait", "--json"],
        &["thread", "show", "id", "--messages", "2", "--json"],
        &["up", "--connect", "--no-lan", "--yes", "--headless"],
        &["connect"],
        &["connect", "login", "--headless"],
        &["connect", "link", "--publish-only"],
        &["connect", "status", "--json"],
        &["connect", "publish", "--disable"],
        &["connect", "unlink"],
        &["connect", "logout"],
        &["service", "install", "--allow-downgrade"],
        &["service", "update", "--allow-downgrade"],
        &["service", "restart"],
        &["service", "uninstall"],
        &["service", "status"],
        &["service", "protect", "--undo", "--yes"],
        &[
            "update",
            "0.1.0",
            "--channel",
            "preview",
            "--allow-downgrade",
            "-y",
        ],
        &["install"],
        &["uninstall", "-y"],
        &["doctor", "--fix", "--dry-run"],
        &["policy", "show"],
        &["policy", "check", "policy.json"],
        &["policy", "templates", ".", "--policy", "policy.json"],
        &["theme", "set", "dark", "--id", "dark"],
        &["theme", "clear"],
        &["theme", "show"],
        &["trace", "summary", "--since", "15 minutes", "--limit", "5"],
        &["triage", "--agent", "codex", "--model", "luna"],
        &["__service-launcher"],
        &[
            "__service-preflight",
            "--database-path",
            "db",
            "--launcher-protocol",
            "1",
        ],
        &[
            "__service-launcher",
            "--base-dir",
            ".",
            "--log-file",
            "server.log",
            "--service-unit",
            "unit",
        ],
        &["__document-text", "file", "docx"],
        &["__speech-worker", "discover", "module"],
        &["__claude-history", "method", "session", "{}"],
        &["__ssh-helper", "pick-port", "ports", "3773", "20"],
        &["__ssh-helper", "wait-ready", "3773", "1000", "100"],
        &["__ssh-helper", "runtime-port", "runtime"],
        &["__ssh-helper", "update-remaining", "marker"],
        &["--completions", "powershell"],
        &["--completions", "pwsh"],
        &["--wizard"],
        &["thread", "list", "--no-json"],
        &[
            "environment",
            "link",
            "http://127.0.0.1:1",
            "--pairing-code",
            "code",
            "--access",
            "auto-accept-edits",
        ],
        &["serve", "--tailscale-serve"],
        &["serve", "--lan"],
        &["pair", "--tailscale"],
    ];
    for args in cases {
        let result = output(root.path(), args).await;
        assert_eq!(
            result.status.code(),
            Some(69),
            "{args:?}: {}",
            stderr(&result)
        );
        assert!(result.stdout.is_empty(), "{args:?}");
        assert!(
            stderr(&result).contains("not yet available in the Rust server"),
            "{args:?}"
        );
    }
    assert!(!root.path().join("account").exists());
}

/// `t3 serve` with a dev URL (flag or `VITE_DEV_SERVER_URL`) starts dev mode: Replay (dev), the loopback redirect to the dev URL. `t3 pair` prints the dev URL's pairing
/// link as TypeScript does, plus one on the server's own origin for the desktop and mobile apps.
#[tokio::test]
async fn serve_with_a_dev_url_starts_dev_mode_and_pairs_on_both_origins() {
    for from_env in [false, true] {
        let root = TempDir::new().unwrap();
        let home = root.path().join("server");
        let home_text = home.to_str().unwrap();
        let dev = "http://127.0.0.1:5733";
        let mut cmd = command(root.path());
        cmd.args([
            "serve",
            "--home-dir",
            home_text,
            "--host",
            "127.0.0.1",
            "--port",
            "0",
        ]);
        if from_env {
            cmd.env("VITE_DEV_SERVER_URL", dev);
        } else {
            cmd.args(["--dev-url", dev]);
        }
        let (mut child, origin, startup) = launch(cmd).await;
        assert!(
            !origin.contains("5733"),
            "serve announces the server origin: {origin}"
        );
        // An explicit home keeps `userdata`; only an implicit one moves dev state to `dev`.
        let runtime: Value = serde_json::from_slice(
            &std::fs::read(home.join("userdata/server-runtime.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(runtime["devUrl"], format!("{dev}/"));
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let shell = http
            .get(format!("{origin}/threads/x?y=1"))
            .send()
            .await
            .unwrap();
        assert_eq!(shell.status(), 302);
        assert_eq!(shell.headers()["location"], format!("{dev}/threads/x?y=1"));
        assert_eq!(
            http.post(format!("{origin}/api/auth/browser-session"))
                .json(&json!({"credential": startup}))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let settings = std::fs::read_to_string(home.join("userdata/settings.json")).unwrap();
        assert!(settings.contains("devReplay"), "{settings}");

        let pair = output(root.path(), &["pair", "--home-dir", home_text]).await;
        assert!(pair.status.success(), "{}", stderr(&pair));
        let text = stdout(&pair);
        let token = text
            .lines()
            .find_map(|line| line.strip_prefix("Token: "))
            .unwrap();
        assert!(
            text.contains(&format!("Pairing URL: {dev}/pair#token={token}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("Server pairing URL: {origin}/pair#token={token}")),
            "{text}"
        );
        assert_eq!(
            http.post(format!("{origin}/api/auth/browser-session"))
                .json(&json!({"credential": token}))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
}

#[tokio::test]
async fn ephemeral_server_and_pair_tokens_work_through_real_http() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("server");
    let home_text = home.to_str().unwrap();
    let mut cmd = command(root.path());
    cmd.args([
        "serve",
        "--home-dir",
        home_text,
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--no-browser",
        "--log-ws-events=false",
    ]);
    cmd.env("T3CODE_PORT", "1")
        .env("T3CODE_HOST", "not-a-host")
        .env("T3CODE_NO_BROWSER", "false");
    let (mut child, origin, startup) = launch(cmd).await;
    assert_ne!(origin.rsplit(':').next().unwrap(), "0");
    let runtime: Value =
        serde_json::from_slice(&std::fs::read(home.join("userdata/server-runtime.json")).unwrap())
            .unwrap();
    assert_eq!(runtime["origin"], origin);
    assert_eq!(runtime["pid"], child.id().unwrap());
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut descriptor: Value = http
        .get(format!("{origin}/.well-known/t3/environment"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        http.post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({"credential": startup}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let pair = command(root.path())
        .env("T3CODE_PORT", "invalid")
        .env("VITE_DEV_SERVER_URL", "invalid")
        .env("T3CODE_BOOTSTRAP_FD", "invalid")
        .args([
            "pair",
            "--home-dir",
            home_text,
            "--ttl",
            "15 minutes",
            "--label",
            "CLI test",
            "--scope",
            "orchestration:read",
            "--scope",
            "orchestration:read",
        ])
        .output()
        .await
        .unwrap();
    assert!(pair.status.success(), "{}", stderr(&pair));
    let text = stdout(&pair);
    assert!(text.starts_with("Pairing with "));
    assert!(text.contains("█"));
    let token = text
        .lines()
        .find_map(|line| line.strip_prefix("Token: "))
        .unwrap();
    assert!(text.contains(&format!("Pairing URL: {origin}/pair#token={token}")));
    let path = home.join("userdata/statev2.sqlite");
    let db =
        tokio::task::spawn_blocking(move || t3_server::adapters::persistence::Database::open(path))
            .await
            .unwrap()
            .unwrap();
    let credential = token.to_owned();
    let (created, expires, label, scopes): (String, String, String, String) = db.reader(move |connection| {
        Ok(connection.query_row("SELECT created_at,expires_at,label,scopes FROM auth_pairing_links WHERE credential=?1", [credential], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?)
    }).await.unwrap();
    db.close().await.unwrap();
    let millis = |raw| t3_auth::clock::epoch(&t3_contracts::wire::WireDateTime(raw)).unwrap();
    assert_eq!(millis(expires.clone()) - millis(created), 900_000);
    assert!(text.contains(&format!("Expires: {expires}")));
    assert_eq!(label, "CLI test");
    assert_eq!(
        serde_json::from_str::<Value>(&scopes).unwrap(),
        json!(["orchestration:read"])
    );
    let paired = http
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({"credential": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 200);
    let body: Value = paired.json().await.unwrap();
    assert_eq!(body["permissions"], json!(["orchestration:read"]));
    assert_eq!(
        http.post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({"credential": token}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let absent = output(root.path(), &["pair", "--home-dir", home_text]).await;
    assert_eq!(absent.status.code(), Some(1));
    assert!(stderr(&absent).contains("No running T3 Code server found."));
    descriptor["orchestrationProtocolVersion"] = json!(1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let impostor = listener.local_addr().unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let app = axum::Router::new().route(
        "/.well-known/t3/environment",
        axum::routing::get(move || {
            let descriptor = descriptor.clone();
            async move { axum::Json(descriptor) }
        }),
    );
    let task = tokio::spawn(
        axum::serve(listener, app)
            .with_graceful_shutdown(cancellation.clone().cancelled_owned())
            .into_future(),
    );
    let mismatched = root.path().join("mismatched");
    std::fs::create_dir_all(mismatched.join("userdata")).unwrap();
    let mut state = runtime;
    state["pid"] = json!(std::process::id());
    state["origin"] = json!(format!("http://{impostor}"));
    state["port"] = json!(impostor.port());
    std::fs::write(
        mismatched.join("userdata/server-runtime.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let incompatible = output(
        root.path(),
        &["pair", "--home-dir", mismatched.to_str().unwrap()],
    )
    .await;
    assert_eq!(incompatible.status.code(), Some(1));
    assert!(stderr(&incompatible).contains("speaks orchestration protocol 1"));
    assert!(!mismatched.join("userdata/statev2.sqlite").exists());
    assert!(incompatible.stdout.is_empty());
    cancellation.cancel();
    task.await.unwrap().unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let result = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", root.join("no-global-config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", stderr(&result));
}

#[tokio::test]
async fn explicit_home_then_worktree_then_ambient_home_precedence() {
    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=CLI Test",
            "-c",
            "user.email=cli@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    let worktree = root.path().join("worktree");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "cli-test",
            worktree.to_str().unwrap(),
        ],
    );
    let nested = worktree.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let ambient = root.path().join("ambient");
    let explicit = root.path().join("explicit");
    for (cwd, flag, expected) in [
        (repo.as_path(), None, ambient.clone()),
        (nested.as_path(), None, worktree.join(".t3")),
        (nested.as_path(), Some("--home-dir"), explicit.clone()),
        (nested.as_path(), Some("--base-dir"), explicit.clone()),
    ] {
        let mut cmd = command(root.path());
        cmd.current_dir(cwd).env("T3CODE_HOME", &ambient).args([
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            "0",
        ]);
        if let Some(flag) = flag {
            cmd.arg(flag).arg(&explicit);
        }
        let (mut child, _, _) = launch(cmd).await;
        let runtime: Value = serde_json::from_slice(
            &std::fs::read(expected.join("userdata/server-runtime.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(runtime["pid"], child.id().unwrap(), "{expected:?}");
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
    assert!(!root.path().join("account/.t3").exists());
    let mut default = command(root.path());
    default.args([
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--no-auto-bootstrap-project-from-cwd",
        "--no-no-browser",
    ]);
    let (mut child, _, _) = launch(default).await;
    assert!(
        root.path()
            .join("account/.t3/userdata/statev2.sqlite")
            .is_file()
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn status_reads_settings_without_opening_a_database() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("status");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("service.env"),
        "export T3CODE_LAN = \"true\" # local access\nT3CODE_HOST=127.0.0.1 # bind\n",
    )
    .unwrap();
    let result = output(
        root.path(),
        &["status", "--home-dir", home.to_str().unwrap(), "--json"],
    )
    .await;
    assert!(result.status.success(), "{}", stderr(&result));
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["connect"], Value::Null);
    assert_eq!(value["service"]["supported"], false);
    assert_eq!(
        value["lan"],
        json!({"enabled":true,"inheritedDefault":false,"hostOverride":"127.0.0.1"})
    );
    assert!(!home.join("userdata").exists());
    std::fs::write(
        home.join("service.env"),
        "T3CODE_LAN=true\nT3CODE_LAN=off\nT3CODE_HOST=\"first\\nsecond\" # note\n",
    )
    .unwrap();
    let multiline = output(
        root.path(),
        &["status", "--home-dir", home.to_str().unwrap(), "--json"],
    )
    .await;
    assert!(multiline.status.success(), "{}", stderr(&multiline));
    let value: Value = serde_json::from_slice(&multiline.stdout).unwrap();
    assert_eq!(value["lan"]["enabled"], false);
    assert_eq!(value["lan"]["hostOverride"], "first\nsecond");
    std::fs::write(home.join("service.env"), "T3CODE_LAN=invalid\n").unwrap();
    let invalid = output(
        root.path(),
        &["status", "--home-dir", home.to_str().unwrap(), "--json"],
    )
    .await;
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
}

#[cfg(any(unix, all(windows, target_env = "msvc")))]
#[tokio::test]
async fn desktop_bootstrap_closed_and_malformed_descriptors() {
    let root = TempDir::new().unwrap();
    let ordinary = output(root.path(), &["status"]).await;
    let closed = output(root.path(), &["--bootstrap-fd", "999999", "status"]).await;
    assert_eq!(closed.status.code(), ordinary.status.code());
    assert_eq!(stdout(&closed), stdout(&ordinary));
    let input = root.path().join("malformed");
    std::fs::write(&input, b"{\"private-fixture\":\"malformed\"}\n").unwrap();
    let result = command(root.path())
        .args(["--bootstrap-fd", "0", "status"])
        .stdin(std::fs::File::open(input).unwrap())
        .output()
        .await
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(
        stderr(&result).contains("Failed to decode bootstrap envelope from file descriptor 0.")
    );
    assert!(!stderr(&result).contains("private-fixture"));
}

#[cfg(unix)]
#[tokio::test]
async fn desktop_bootstrap_socket_starts_server_and_authenticates_without_eof() {
    use std::{
        io::Write,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };
    let root = TempDir::new().unwrap();
    let home = root.path().join("desktop");
    let (mut parent, inherited) = UnixStream::pair().unwrap();
    let mut cmd = program(root.path(), "/bin/sh");
    cmd.args([
        "-c",
        r#"exec 3<&0; exec 0</dev/null; exec "$@""#,
        "launcher",
        env!("CARGO_BIN_EXE_t3"),
        "--bootstrap-fd",
        "3",
        "serve",
        "--port",
        "0",
    ])
    .stdin(Stdio::from(OwnedFd::from(inherited)));
    let secret = "desktop-private-fixture";
    writeln!(
        parent,
        "{}",
        json!({"mode":"desktop","noBrowser":true,"port":3773,"host":"127.0.0.1",
        "desktopBootstrapToken":"superseded-fixture","desktopBootstrapSecret":secret,"t3Home":home,
        "tailscaleServeEnabled":false,"tailscaleServePort":443})
    )
    .unwrap();
    let (mut child, origin, announced) = launch(cmd).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert!(home.join("userdata/server-runtime.json").is_file());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let token = t3_auth::pairing::current_desktop_token(secret, now).unwrap();
    let exchange = |value: &str| {
        client
            .post(format!("{origin}/api/auth/browser-session"))
            .json(&json!({"credential":value}))
            .send()
    };
    assert!(exchange(&token).await.unwrap().status().is_success());
    assert!(
        !exchange("superseded-fixture")
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_ne!(announced, token);
    assert_ne!(announced, secret);
    assert_ne!(announced, "superseded-fixture");
    child.kill().await.unwrap();
}

/// The desktop reads these lines from the server's output to wait past its readiness budget
/// while a large database migrates (`packages/shared/src/databaseMigrationLog.ts`, written by
/// `Migrations.ts`). A new database runs the baseline, so the started line is followed by the
/// finished line.
#[tokio::test]
async fn serve_logs_the_migration_markers_the_desktop_waits_for() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("server");
    let mut cmd = command(root.path());
    cmd.args([
        "serve",
        "--home-dir",
        home.to_str().unwrap(),
        "--host",
        "127.0.0.1",
        "--port",
        "0",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut readers = Vec::new();
    let streams: [Box<dyn tokio::io::AsyncRead + Unpin + Send>; 2] = [
        Box::new(child.stdout.take().unwrap()),
        Box::new(child.stderr.take().unwrap()),
    ];
    for stream in streams {
        let log = log.clone();
        readers.push(tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut log = log.lock().unwrap();
                log.push_str(&line);
                log.push('\n');
            }
        }));
    }
    // Ready once the pairing output is complete.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if log.lock().unwrap().contains("App link: ") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("server did not announce readiness");
    child.kill().await.unwrap();
    for reader in readers {
        let _ = reader.await;
    }
    let log = log.lock().unwrap().clone();
    let started = log.find("Running database migrations");
    let finished = log.find("Migrations ran successfully");
    assert!(
        started.is_some() && finished.is_some() && started < finished,
        "{log}"
    );
}

/// A supervising desktop keeps the server's stdin open after the envelope. When that parent dies,
/// the pipe closes and the server must stop instead of staying behind with its port and home. A
/// parent that closes the stream after the line (Electron) must not stop it.
#[cfg(unix)]
#[tokio::test]
async fn the_server_stops_when_its_supervisor_closes_stdin_only_if_asked() {
    use tokio::io::AsyncWriteExt;
    for exit_on_close in [true, false] {
        let root = TempDir::new().unwrap();
        let home = root.path().join("desktop");
        let mut cmd = command(root.path());
        cmd.args(["--bootstrap-fd", "0", "serve", "--port", "0"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if exit_on_close {
            cmd.env("T3CODE_DESKTOP_EXIT_ON_STDIN_CLOSE", "1");
        }
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let envelope = json!({"mode":"desktop","noBrowser":true,"port":3773,"host":"127.0.0.1",
            "desktopBootstrapToken":"fixture","t3Home":home,
            "tailscaleServeEnabled":false,"tailscaleServePort":443});
        stdin
            .write_all(format!("{envelope}\n").as_bytes())
            .await
            .unwrap();
        stdin.flush().await.unwrap();
        // Ready once the server published its runtime file; the pipe is still open.
        let runtime = home.join("userdata/server-runtime.json");
        tokio::time::timeout(Duration::from_secs(60), async {
            while !runtime.is_file() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("server did not publish its runtime file");
        drop(stdin);
        let exited = tokio::time::timeout(
            Duration::from_secs(if exit_on_close { 30 } else { 2 }),
            child.wait(),
        )
        .await;
        if exit_on_close {
            let status = exited.expect("the server outlived its supervisor").unwrap();
            assert!(status.success(), "{status:?}");
        } else {
            assert!(exited.is_err(), "closing stdin alone stopped the server");
            child.kill().await.unwrap();
        }
    }
}
