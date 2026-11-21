//! The desktop shell's supervisor for the local `t3 serve` backend.
//! This crate does not compile for wasm32-unknown-unknown (it spawns processes).
//!
//! Port of apps/desktop/src/backend/DesktopBackendManager.ts: spawn the server with the bootstrap
//! envelope on its stdin, probe readiness, restart with backoff, report crashes and exits as
//! [`SupervisorEvent`]s the shell turns into the O08 startup states, and stop the whole process
//! group on quit. The restart and reporting policy is `t3_client::view_logic::shell::startup_recovery`,
//! shared with the UI. All platform differences (process groups, job objects, signals) live in
//! `t3-process`, so this crate has no `cfg` of its own.
//!
//! The envelope travels on stdin (`--bootstrap-fd 0`, the TypeScript desktop's "stdin" delivery)
//! rather than descriptor 3: `std::process::Command` cannot place a pipe on descriptor 3 without
//! `unsafe` (`pre_exec` on Unix, the CRT startup block on Windows), and the workspace denies
//! `unsafe` outside the one approved Windows adapter.

mod local;
mod readiness;
mod tail;
mod view;

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use t3_client::view_logic::shell::startup_recovery::{StartupEvent, StartupFailure, plan_restart};
use t3_contracts::desktop_bootstrap::{DesktopBackendBootstrap, SERVER_EXIT_CODE_STATE_DIR_OWNED};
use t3_process::{CancellationToken, Environment, ProcessOptions};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    task::JoinHandle,
};

pub use local::LocalServer;
pub use readiness::READINESS_PATH;
pub use tail::is_running_database_migrations;
use tail::{OutputTail, Stream};
pub use view::{BackendView, ViewTracker};

/// Everything one launch of the server needs. Resolved again for every run, so each run gets a
/// fresh bootstrap token (`configResolve`).
#[derive(Clone)]
pub struct StartConfig {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Environment,
    /// The envelope, one JSON line without its newline. Holds the bootstrap token; never logged.
    pub(crate) bootstrap_line: String,
    /// Where the readiness probe connects.
    pub ready_host: String,
    pub ready_port: u16,
}

impl StartConfig {
    /// `t3 serve --bootstrap-fd 0` with `bootstrap` on stdin. The probe connects to the envelope's
    /// port on `ready_host` (the server may bind a wider host than the one the desktop dials).
    pub fn serve(
        program: impl Into<OsString>,
        bootstrap: &DesktopBackendBootstrap,
        ready_host: impl Into<String>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            program: program.into(),
            args: vec!["serve".into(), "--bootstrap-fd".into(), "0".into()],
            cwd: None,
            env: Environment::new(),
            bootstrap_line: serde_json::to_string(bootstrap)?,
            ready_host: ready_host.into(),
            ready_port: bootstrap.port,
        })
    }

    /// Any program that reads one envelope line from stdin (a fake server in tests).
    pub fn with_line(
        program: impl Into<OsString>,
        args: Vec<OsString>,
        bootstrap_line: String,
        ready_host: impl Into<String>,
        ready_port: u16,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
            env: Environment::new(),
            bootstrap_line,
            ready_host: ready_host.into(),
            ready_port,
        }
    }
}

impl std::fmt::Debug for StartConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartConfig")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("ready_host", &self.ready_host)
            .field("ready_port", &self.ready_port)
            .finish_non_exhaustive()
    }
}

/// Produces the configuration for the next run; `Err` is the reason it could not
/// (`failed to generate desktop backend configuration`) and counts as a failed run.
pub trait ConfigSource: Send + Sync + 'static {
    fn resolve(&self) -> Result<StartConfig, String>;
}

impl<F> ConfigSource for F
where
    F: Fn() -> Result<StartConfig, String> + Send + Sync + 'static,
{
    fn resolve(&self) -> Result<StartConfig, String> {
        self()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// Give up waiting for one readiness window (`DEFAULT_BACKEND_READINESS_TIMEOUT`, 3 minutes:
    /// large databases take over a minute to migrate).
    pub readiness_timeout: Duration,
    /// Between probes (`DEFAULT_BACKEND_READINESS_INTERVAL`).
    pub readiness_interval: Duration,
    /// One probe (`DEFAULT_BACKEND_READINESS_REQUEST_TIMEOUT`).
    pub probe_timeout: Duration,
    /// SIGTERM before SIGKILL (`DEFAULT_BACKEND_TERMINATE_GRACE`).
    pub terminate_grace: Duration,
    /// Wait for the output pipes after the server exits (`DEFAULT_BACKEND_OUTPUT_DRAIN_TIMEOUT`).
    pub output_drain: Duration,
    /// The delay before restart number `attempt - 1 + 1`, given restarts so far. The policy's
    /// curve by default; tests pass a zero delay instead of sleeping.
    pub restart_delay: fn(u32) -> Duration,
}

fn policy_delay(previous_attempts: u32) -> Duration {
    Duration::from_millis(plan_restart(previous_attempts, None).delay_ms)
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            readiness_timeout: Duration::from_secs(180),
            readiness_interval: Duration::from_millis(100),
            probe_timeout: Duration::from_secs(1),
            terminate_grace: Duration::from_secs(5),
            output_drain: Duration::from_secs(5),
            restart_delay: policy_delay,
        }
    }
}

/// What the shell hears about the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorEvent {
    /// The first launch is about to happen.
    Starting,
    Spawned {
        pid: u32,
    },
    /// The server answered its readiness probe.
    Ready {
        pid: u32,
    },
    /// The server exited without being asked to; restart number `attempt` follows `delay` later.
    Restarting {
        attempt: u32,
        delay: Duration,
        reason: String,
    },
    /// Restarts did not help, or the server never answered: the window reports this
    /// (`onStartupFailure`).
    Failing(StartupFailure),
    /// Another T3 server owns the state directory (exit code 78): not restarted.
    StateDirOwned,
    /// The supervisor stopped, after [`Supervisor::stop`] or `StateDirOwned`.
    Stopped,
}

impl SupervisorEvent {
    /// The input of `StartupRecovery::on_event`.
    pub fn startup_event(&self) -> Option<StartupEvent> {
        match self {
            Self::Ready { .. } => Some(StartupEvent::Ready),
            Self::Failing(failure) => Some(StartupEvent::Failing(failure.clone())),
            _ => None,
        }
    }
}

/// Owns the server and its restarts until [`Supervisor::stop`].
pub struct Supervisor {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Supervisor {
    /// Starts supervising; must be called inside a tokio runtime.
    pub fn start(
        source: impl ConfigSource,
        timings: Timings,
    ) -> (Self, mpsc::UnboundedReceiver<SupervisorEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_supervisor(
            Arc::new(source),
            timings,
            events,
            cancel.clone(),
        ));
        (Self { cancel, task }, receiver)
    }

    /// Stops the server (SIGTERM, then SIGKILL after the grace period; the whole process group)
    /// and returns once it is gone.
    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = self.task.await;
    }
}

type SharedTail = Arc<Mutex<OutputTail>>;

fn tail_text(tail: &SharedTail) -> String {
    tail.lock()
        .map(|tail| tail.text().to_owned())
        .unwrap_or_default()
}

async fn drain(mut reader: impl AsyncRead + Unpin, stream: Stream, tail: SharedTail) {
    let mut chunk = [0u8; 8192];
    while let Ok(count) = reader.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        if let Ok(mut tail) = tail.lock() {
            tail.push(stream, &chunk[..count]);
        }
    }
}

/// How one run of the server ended.
enum RunEnd {
    /// Quit: the server was terminated on request.
    Stopped,
    Exited {
        reason: String,
        code: Option<i32>,
        was_ready: bool,
    },
}

fn exit_reason(exit: &t3_process::LeaderExit) -> String {
    match (exit.status.code(), exit.signal_name) {
        (Some(code), _) => format!("code={code}"),
        (None, Some(signal)) => format!("killed by {signal}"),
        (None, None) => "exited without a status".to_owned(),
    }
}

/// `waitForHttpReady` plus the migration rule of `onReadinessFailure`: probe until the server
/// answers; when a window of `readiness_timeout` passes without it, report the failure unless the
/// server is still migrating its database, then probe again (`repeat while not ready`).
async fn wait_until_ready(
    config: &StartConfig,
    timings: Timings,
    tail: &SharedTail,
    events: &mpsc::UnboundedSender<SupervisorEvent>,
) {
    loop {
        let window = tokio::time::timeout(timings.readiness_timeout, async {
            while !readiness::probe(&config.ready_host, config.ready_port, timings.probe_timeout)
                .await
            {
                tokio::time::sleep(timings.readiness_interval).await;
            }
        })
        .await;
        if window.is_ok() {
            return;
        }
        let output = tail_text(tail);
        if !is_running_database_migrations(&output) {
            let _ = events.send(SupervisorEvent::Failing(StartupFailure::not_ready(&output)));
        }
    }
}

async fn run_once(
    config: StartConfig,
    timings: Timings,
    events: &mpsc::UnboundedSender<SupervisorEvent>,
    cancel: &CancellationToken,
    tail: &SharedTail,
) -> RunEnd {
    let fail = |reason: String| RunEnd::Exited {
        reason,
        code: None,
        was_ready: false,
    };
    let mut options = ProcessOptions::new(config.program.clone()).args(config.args.clone());
    options.cwd = config.cwd.clone();
    options.env = config.env.clone();
    options.timeout = None;
    options.termination_grace = timings.terminate_grace;
    let mut child = match t3_process::spawn(&options) {
        Ok(child) => child,
        Err(error) => return fail(format!("failed to spawn the server: {error}")),
    };
    if let Ok(mut tail) = tail.lock() {
        tail.clear();
    }
    let mut drains = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        drains.push(tokio::spawn(drain(stdout, Stream::Stdout, tail.clone())));
    }
    if let Some(stderr) = child.stderr.take() {
        drains.push(tokio::spawn(drain(stderr, Stream::Stderr, tail.clone())));
    }
    // The server reads one line; a server that already exited makes this fail, which the exit
    // status reports.
    if let Some(mut stdin) = child.stdin.take() {
        let line = format!("{}\n", config.bootstrap_line);
        let _ = stdin.write_all(line.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let pid = child.id().unwrap_or_default();
    let _ = events.send(SupervisorEvent::Spawned { pid });

    let mut ready = std::pin::pin!(wait_until_ready(&config, timings, tail, events));
    let mut was_ready = false;
    let end = loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let _ = child.terminate(timings.terminate_grace).await;
                break RunEnd::Stopped;
            }
            exit = child.wait_and_kill_group() => {
                break match exit {
                    Ok(exit) => RunEnd::Exited {
                        reason: exit_reason(&exit),
                        code: exit.status.code(),
                        was_ready,
                    },
                    Err(error) => RunEnd::Exited {
                        reason: format!("failed to read the server's exit status: {error}"),
                        code: None,
                        was_ready,
                    },
                };
            }
            () = &mut ready, if !was_ready => {
                was_ready = true;
                let _ = events.send(SupervisorEvent::Ready { pid });
            }
        }
    };
    // Pipes held open by a helper that left the group would keep these tasks (and their writes to
    // the shared tail) alive into the next run: abort what is still running after the grace.
    let aborts: Vec<_> = drains.iter().map(JoinHandle::abort_handle).collect();
    let drained = tokio::time::timeout(timings.output_drain, async {
        for task in drains {
            let _ = task.await;
        }
    })
    .await;
    if drained.is_err() {
        for abort in aborts {
            abort.abort();
        }
    }
    end
}

async fn run_supervisor(
    source: Arc<dyn ConfigSource>,
    timings: Timings,
    events: mpsc::UnboundedSender<SupervisorEvent>,
    cancel: CancellationToken,
) {
    let tail: SharedTail = Arc::default();
    let mut attempts = 0u32;
    let _ = events.send(SupervisorEvent::Starting);
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let end = match source.resolve() {
            Ok(config) => run_once(config, timings, &events, &cancel, &tail).await,
            Err(reason) => RunEnd::Exited {
                reason,
                code: None,
                was_ready: false,
            },
        };
        let RunEnd::Exited {
            reason,
            code,
            was_ready,
        } = end
        else {
            break;
        };
        if was_ready {
            attempts = 0;
        }
        if code == Some(SERVER_EXIT_CODE_STATE_DIR_OWNED) {
            let _ = events.send(SupervisorEvent::StateDirOwned);
            break;
        }
        let plan = plan_restart(attempts, code);
        attempts = plan.attempt;
        let delay = (timings.restart_delay)(plan.attempt - 1);
        let _ = events.send(SupervisorEvent::Restarting {
            attempt: plan.attempt,
            delay,
            reason: reason.clone(),
        });
        if plan.report_failure {
            let managed = code
                == Some(
                    t3_client::view_logic::shell::startup_recovery::SERVER_EXIT_CODE_MANAGED_POLICY,
                );
            let output = tail_text(&tail);
            let _ = events.send(SupervisorEvent::Failing(StartupFailure::exited(
                &reason, managed, &output,
            )));
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(delay) => {}
        }
    }
    let _ = events.send(SupervisorEvent::Stopped);
}
