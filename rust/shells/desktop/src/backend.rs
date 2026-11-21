//! Starts and supervises the local `t3` server for the desktop window.
//!
//! All behaviour lives in `t3-desktop-backend` (spawn, readiness, restarts, quit); this file only
//! finds the binary, runs the supervisor on its own thread and hands its states to the window.
//! Supervision starts when a server binary is found (`T3CODE_SERVER_PATH`, or `t3` beside this
//! executable); otherwise the window keeps its previous behaviour.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use t3_desktop_backend::{
    BackendView, LocalServer, Supervisor, SupervisorEvent, Timings, ViewTracker,
};
use t3_ui::screens::local_backend::{
    FailedStartup, LocalBackendPublisher, LocalBackendStatus, RecoveryAction, install_local_backend,
};

const DEFAULT_PORT: u16 = 3773;

/// `T3CODE_SERVER_PATH`, else the `t3` binary the package puts beside the desktop executable.
fn server_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("T3CODE_SERVER_PATH") {
        return Some(path.into());
    }
    let name = if cfg!(windows) { "t3.exe" } else { "t3" };
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    beside.is_file().then_some(beside)
}

fn log_dir() -> String {
    let home = std::env::var_os("T3CODE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let user = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
            Some(PathBuf::from(user).join(".t3"))
        });
    home.map(|home| home.join("userdata").join("logs"))
        .map_or_else(String::new, |dir| dir.display().to_string())
}

/// The window's status for a supervisor view. Without an updater the offers are the managed
/// policy sentence or the releases page.
fn status(view: BackendView) -> LocalBackendStatus {
    match view {
        BackendView::Starting => LocalBackendStatus::Starting,
        BackendView::Restarting { attempt } => LocalBackendStatus::Restarting { attempt },
        BackendView::Ready => LocalBackendStatus::Ready,
        BackendView::Failed(failure) => {
            let offer = if failure.managed_policy {
                t3_client::view_logic::shell::startup_recovery::RecoveryOffer::ManagedPolicy
            } else {
                t3_client::view_logic::shell::startup_recovery::RecoveryOffer::Releases
            };
            LocalBackendStatus::Failed(Box::new(FailedStartup {
                failure,
                offer,
                app_version: env!("CARGO_PKG_VERSION").into(),
                log_dir: log_dir(),
            }))
        }
    }
}

/// Owns the supervisor; stops the server when dropped.
pub struct ShellBackend {
    runtime: tokio::runtime::Runtime,
    supervisor: Arc<Mutex<Option<Supervisor>>>,
}

impl ShellBackend {
    /// Stops the server and its process group, then returns.
    pub fn stop(&self) {
        let supervisor = self.supervisor.lock().ok().and_then(|mut slot| slot.take());
        if let Some(supervisor) = supervisor {
            self.runtime.block_on(supervisor.stop());
        }
    }
}

impl Drop for ShellBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Call on the thread that runs the UI, before the app starts.
pub fn start() -> Option<ShellBackend> {
    let program = server_binary()?;
    let port = std::env::var("T3CODE_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .ok()?;
    let supervisor: Arc<Mutex<Option<Supervisor>>> = Arc::default();
    let quitting = supervisor.clone();
    let handle = runtime.handle().clone();
    let publisher: LocalBackendPublisher = install_local_backend(move |action| match action {
        RecoveryAction::Quit => {
            // Off the UI thread: stopping can take the server's whole grace period.
            let supervisor = quitting.lock().ok().and_then(|mut slot| slot.take());
            let handle = handle.clone();
            std::thread::spawn(move || {
                if let Some(supervisor) = supervisor {
                    handle.block_on(supervisor.stop());
                }
                std::process::exit(0);
            });
        }
        // The updater, release page and clipboard are not wired into this shell yet.
        other => eprintln!("t3-desktop: {other:?} is not available yet"),
    });
    let source = LocalServer {
        program,
        port,
        host: "127.0.0.1".into(),
        t3_home: std::env::var("T3CODE_HOME").ok(),
    };
    let (started, mut events) =
        runtime.block_on(async { Supervisor::start(source, Timings::default()) });
    if let Ok(mut slot) = supervisor.lock() {
        *slot = Some(started);
    }
    runtime.spawn(async move {
        let mut tracker = ViewTracker::default();
        while let Some(event) = events.recv().await {
            if let SupervisorEvent::Stopped = event {
                break;
            }
            // The window is the main window, so it is always visible once the app runs.
            if let Some(view) = tracker.apply(&event, true) {
                publisher.publish(status(view));
            }
        }
    });
    Some(ShellBackend {
        runtime,
        supervisor,
    })
}
