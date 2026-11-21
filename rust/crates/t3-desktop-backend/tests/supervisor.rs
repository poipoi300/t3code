//! The supervisor against a fake server: a shell script that reads the envelope from stdin and
//! then behaves as its first argument says. Readiness is answered by a listener the test owns.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use t3_desktop_backend::{ConfigSource, StartConfig, Supervisor, SupervisorEvent, Timings};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc::UnboundedReceiver,
};

const SCRIPT: &str = r#"
IFS= read -r line
[ -n "$FAKE_ENVELOPE_FILE" ] && printf '%s\n' "$line" > "$FAKE_ENVELOPE_FILE"
case "$1" in
serve) echo "fake backend up"; exec sleep 300 ;;
crash) echo "boom" >&2; exit 3 ;;
policy) echo "blocked by policy" >&2; exit 77 ;;
owned) exit 78 ;;
hang) exec sleep 300 ;;
migrate) echo "Running database migrations"; exec sleep 300 ;;
exit-on-go) until [ -e "$FAKE_GO" ]; do sleep 0.02; done; exit 3 ;;
esac
"#;

fn instant(_: u32) -> Duration {
    Duration::ZERO
}

fn an_hour(_: u32) -> Duration {
    Duration::from_secs(3600)
}

fn quick() -> Timings {
    Timings {
        readiness_timeout: Duration::from_millis(40),
        readiness_interval: Duration::from_millis(5),
        probe_timeout: Duration::from_millis(100),
        terminate_grace: Duration::from_secs(2),
        output_drain: Duration::from_secs(2),
        restart_delay: instant,
    }
}

struct Fake {
    dir: tempfile::TempDir,
    script: PathBuf,
}

impl Fake {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-backend.sh");
        std::fs::write(&script, SCRIPT).unwrap();
        Self { dir, script }
    }

    fn envelope_file(&self) -> PathBuf {
        self.dir.path().join("envelope")
    }

    fn config(&self, mode: &str, port: u16) -> StartConfig {
        let mut config = StartConfig::with_line(
            "/bin/sh",
            vec![self.script.clone().into(), OsString::from(mode)],
            r#"{"secret":"private-fixture-token"}"#.into(),
            "127.0.0.1",
            port,
        );
        config.env.insert(
            "FAKE_ENVELOPE_FILE".into(),
            Some(self.envelope_file().into()),
        );
        config
            .env
            .insert("FAKE_GO".into(), Some(self.dir.path().join("go").into()));
        config
    }
}

/// A port with nothing listening.
async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

/// A listener that answers every request with 200, or 503 until `ready` is set; one message per
/// request on the returned channel.
async fn readiness_server(ready: Arc<AtomicBool>) -> (u16, UnboundedReceiver<()>) {
    readiness_server_after(ready, None).await
}

/// Like [`readiness_server`], but answers 200 only once `file` exists (the fake server wrote the
/// envelope there before it could be ready).
async fn readiness_server_after(
    ready: Arc<AtomicBool>,
    file: Option<PathBuf>,
) -> (u16, UnboundedReceiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (requests, received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let ready = ready.clone();
            let requests = requests.clone();
            let file = file.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).await;
                assert!(
                    String::from_utf8_lossy(&buffer)
                        .starts_with("GET /.well-known/t3/environment HTTP/1.1")
                );
                let status =
                    if ready.load(Ordering::SeqCst) && file.as_deref().is_none_or(Path::exists) {
                        "200 OK"
                    } else {
                        "503 Service Unavailable"
                    };
                let _ = stream
                    .write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes())
                    .await;
                let _ = requests.send(());
            });
        }
    });
    (port, received)
}

async fn next(events: &mut UnboundedReceiver<SupervisorEvent>) -> SupervisorEvent {
    tokio::time::timeout(Duration::from_secs(20), events.recv())
        .await
        .expect("no supervisor event within 20 s")
        .expect("the supervisor ended without the expected event")
}

/// Events until `wanted` matches, skipping everything else.
async fn until<T>(
    events: &mut UnboundedReceiver<SupervisorEvent>,
    mut wanted: impl FnMut(SupervisorEvent) -> Option<T>,
) -> T {
    loop {
        if let Some(found) = wanted(next(events).await) {
            return found;
        }
    }
}

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[tokio::test]
async fn the_envelope_arrives_on_stdin_then_the_server_is_ready_and_quit_stops_its_group() {
    let fake = Fake::new();
    let (port, _requests) =
        readiness_server_after(Arc::new(AtomicBool::new(true)), Some(fake.envelope_file())).await;
    let config = fake.config("serve", port);
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());

    assert_eq!(next(&mut events).await, SupervisorEvent::Starting);
    let SupervisorEvent::Spawned { pid } = next(&mut events).await else {
        panic!("expected Spawned");
    };
    assert_eq!(next(&mut events).await, SupervisorEvent::Ready { pid });
    assert_eq!(
        std::fs::read_to_string(fake.envelope_file()).unwrap(),
        "{\"secret\":\"private-fixture-token\"}\n"
    );
    assert!(alive(pid));

    supervisor.stop().await;
    assert_eq!(next(&mut events).await, SupervisorEvent::Stopped);
    assert!(!alive(pid), "the server outlived the supervisor");
}

#[tokio::test]
async fn a_crashing_server_restarts_with_growing_attempts_and_reports_on_the_third() {
    let fake = Fake::new();
    let config = fake.config("crash", closed_port().await);
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());

    let mut restarts = Vec::new();
    let failure = until(&mut events, |event| match event {
        SupervisorEvent::Restarting {
            attempt, reason, ..
        } => {
            restarts.push((attempt, reason));
            None
        }
        SupervisorEvent::Failing(failure) => Some(failure),
        _ => None,
    })
    .await;
    supervisor.stop().await;

    assert_eq!(
        restarts,
        vec![
            (1, "code=3".to_owned()),
            (2, "code=3".to_owned()),
            (3, "code=3".to_owned())
        ]
    );
    assert_eq!(failure.reason, "The server could not start (code=3).");
    assert!(!failure.managed_policy);
    assert!(failure.output.contains("boom"), "{failure:?}");
}

#[tokio::test]
async fn the_backoff_delay_follows_the_policy_curve() {
    let fake = Fake::new();
    let config = fake.config("crash", closed_port().await);
    let timings = Timings {
        restart_delay: |previous| Duration::from_millis(1 + u64::from(previous)),
        ..quick()
    };
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), timings);
    let mut delays = Vec::new();
    until(&mut events, |event| match event {
        SupervisorEvent::Restarting { delay, .. } => {
            delays.push(delay);
            (delays.len() == 3).then_some(())
        }
        _ => None,
    })
    .await;
    supervisor.stop().await;
    assert_eq!(
        delays,
        [1, 2, 3].map(Duration::from_millis),
        "the supervisor passes the number of restarts so far to the delay"
    );
}

#[tokio::test]
async fn a_managed_policy_refusal_is_reported_at_the_first_exit() {
    let fake = Fake::new();
    let config = fake.config("policy", closed_port().await);
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());
    let first_restart = until(&mut events, |event| match event {
        SupervisorEvent::Restarting { attempt, .. } => Some(attempt),
        _ => None,
    })
    .await;
    let SupervisorEvent::Failing(failure) = next(&mut events).await else {
        panic!("a managed-policy exit is reported immediately");
    };
    supervisor.stop().await;
    assert_eq!(first_restart, 1);
    assert!(failure.managed_policy);
    assert!(failure.output.contains("blocked by policy"));
}

#[tokio::test]
async fn an_owned_state_directory_stops_the_supervisor_without_restarting() {
    let fake = Fake::new();
    let config = fake.config("owned", closed_port().await);
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());
    assert_eq!(next(&mut events).await, SupervisorEvent::Starting);
    assert!(matches!(
        next(&mut events).await,
        SupervisorEvent::Spawned { .. }
    ));
    assert_eq!(next(&mut events).await, SupervisorEvent::StateDirOwned);
    assert_eq!(next(&mut events).await, SupervisorEvent::Stopped);
    assert!(events.recv().await.is_none());
    supervisor.stop().await;
}

#[tokio::test]
async fn readiness_resets_the_restart_count() {
    let fake = Fake::new();
    let (port, _requests) = readiness_server(Arc::new(AtomicBool::new(true))).await;
    let config = fake.config("exit-on-go", port);
    let go = fake.dir.path().join("go");
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());

    until(&mut events, |event| {
        matches!(event, SupervisorEvent::Ready { .. }).then_some(())
    })
    .await;
    std::fs::write(&go, "").unwrap();
    // `go` stays, so every run exits as soon as it is ready; each ready run restarts from 1.
    let mut attempts = Vec::new();
    until(&mut events, |event| match event {
        SupervisorEvent::Restarting { attempt, .. } => {
            attempts.push(attempt);
            (attempts.len() == 2).then_some(())
        }
        SupervisorEvent::Failing(failure) => panic!("reported too early: {failure:?}"),
        _ => None,
    })
    .await;
    supervisor.stop().await;
    assert_eq!(attempts, [1, 1]);
}

#[tokio::test]
async fn a_server_that_never_answers_is_reported_but_kept_running() {
    let fake = Fake::new();
    let config = fake.config("hang", closed_port().await);
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());
    let SupervisorEvent::Spawned { pid } = until(&mut events, |event| {
        matches!(event, SupervisorEvent::Spawned { .. }).then_some(event)
    })
    .await
    else {
        panic!("expected Spawned");
    };
    let SupervisorEvent::Failing(failure) = next(&mut events).await else {
        panic!("a silent server is reported");
    };
    assert_eq!(
        failure.reason,
        "The server started but did not become ready in time."
    );
    assert!(alive(pid), "a slow server is not killed");
    supervisor.stop().await;
    assert!(!alive(pid));
}

#[tokio::test]
async fn a_database_migration_in_progress_is_waited_for_not_reported() {
    let fake = Fake::new();
    let ready = Arc::new(AtomicBool::new(false));
    let (port, mut requests) = readiness_server(ready.clone()).await;
    let config = fake.config("migrate", port);
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());
    let SupervisorEvent::Spawned { pid } = until(&mut events, |event| {
        matches!(event, SupervisorEvent::Spawned { .. }).then_some(event)
    })
    .await
    else {
        panic!("expected Spawned");
    };
    // Many 40 ms windows pass while the server logs a migration and answers 503.
    for _ in 0..40 {
        requests.recv().await.unwrap();
    }
    ready.store(true, Ordering::SeqCst);
    assert_eq!(next(&mut events).await, SupervisorEvent::Ready { pid });
    supervisor.stop().await;
}

#[tokio::test]
async fn a_missing_server_binary_is_a_failed_run_that_restarts() {
    let config = StartConfig::with_line(
        "/nonexistent/t3-server",
        vec![],
        "{}".into(),
        "127.0.0.1",
        closed_port().await,
    );
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), quick());
    let (attempt, reason) = until(&mut events, |event| match event {
        SupervisorEvent::Restarting {
            attempt, reason, ..
        } => Some((attempt, reason)),
        _ => None,
    })
    .await;
    supervisor.stop().await;
    assert_eq!(attempt, 1);
    assert!(reason.contains("t3-server"), "{reason}");
}

#[tokio::test]
async fn each_run_resolves_its_configuration_again_and_a_failed_resolve_restarts() {
    let resolved = Arc::new(AtomicUsize::new(0));
    let counter = resolved.clone();
    let source = move || -> Result<StartConfig, String> {
        counter.fetch_add(1, Ordering::SeqCst);
        Err("no free port".into())
    };
    let (supervisor, mut events) = Supervisor::start(source, quick());
    let mut reasons = Vec::new();
    until(&mut events, |event| match event {
        SupervisorEvent::Restarting { reason, .. } => {
            reasons.push(reason);
            (reasons.len() == 2).then_some(())
        }
        _ => None,
    })
    .await;
    supervisor.stop().await;
    assert_eq!(reasons, ["no free port", "no free port"]);
    assert!(resolved.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn quit_during_the_restart_delay_does_not_wait_for_it() {
    let fake = Fake::new();
    let config = fake.config("crash", closed_port().await);
    let timings = Timings {
        restart_delay: an_hour,
        ..quick()
    };
    let (supervisor, mut events) = Supervisor::start(move || Ok(config.clone()), timings);
    until(&mut events, |event| {
        matches!(event, SupervisorEvent::Restarting { .. }).then_some(())
    })
    .await;
    supervisor.stop().await;
    assert_eq!(next(&mut events).await, SupervisorEvent::Stopped);
}

#[test]
fn the_configuration_never_prints_the_envelope() {
    let config = StartConfig::with_line(
        "t3",
        vec![],
        r#"{"desktopBootstrapToken":"private-fixture-token"}"#.into(),
        "127.0.0.1",
        1,
    );
    assert!(!format!("{config:?}").contains("private-fixture-token"));
}

#[allow(dead_code)]
fn _assert_source_is_object_safe(_: &dyn ConfigSource) {}

/// The real `t3 serve --bootstrap-fd 0`, built separately (`T3_REAL_SERVER_BIN`): it reads the
/// envelope the supervisor writes, listens on the envelope's port and answers the readiness probe.
/// Ignored by default because it needs that binary; run it with `--ignored`.
#[tokio::test]
#[ignore = "needs T3_REAL_SERVER_BIN, a built `t3`"]
async fn the_real_server_reads_the_envelope_and_becomes_ready() {
    let binary = std::env::var("T3_REAL_SERVER_BIN").expect("T3_REAL_SERVER_BIN");
    let home = tempfile::tempdir().unwrap();
    let port = closed_port().await;
    let server = t3_desktop_backend::LocalServer {
        program: binary.into(),
        port,
        host: "127.0.0.1".into(),
        t3_home: Some(home.path().display().to_string()),
    };
    let source = move || {
        let mut config = server.resolve()?;
        config.env.insert("HOME".into(), Some(home.path().into()));
        Ok(config)
    };
    let timings = Timings {
        readiness_timeout: Duration::from_secs(60),
        ..Timings::default()
    };
    let (supervisor, mut events) = Supervisor::start(source, timings);
    let pid = until(&mut events, |event| match event {
        SupervisorEvent::Ready { pid } => Some(pid),
        SupervisorEvent::Failing(failure) => panic!("the real server failed: {failure:?}"),
        _ => None,
    })
    .await;
    assert!(alive(pid));
    supervisor.stop().await;
    assert!(!alive(pid));
}
