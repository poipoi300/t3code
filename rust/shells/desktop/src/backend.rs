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

/// `T3CODE_HOME`, else `~/.t3` (`T3_HOME_DIR` in the TypeScript defaults).
fn t3_home() -> Option<PathBuf> {
    std::env::var_os("T3CODE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let user = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
            Some(PathBuf::from(user).join(".t3"))
        })
}

fn log_dir(home: Option<&PathBuf>) -> String {
    home.map_or_else(String::new, |home| {
        t3_desktop_backend::logs_dir(home).display().to_string()
    })
}

/// The window's status for a supervisor view. Without an updater the offers are the managed
/// policy sentence or the releases page.
fn status(view: BackendView, home: Option<&PathBuf>) -> LocalBackendStatus {
    use t3_client::view_logic::shell::startup_recovery::{RecoveryOffer, StartupFailure};
    let failed = |failure, offer| {
        LocalBackendStatus::Failed(Box::new(FailedStartup {
            failure,
            offer,
            app_version: env!("CARGO_PKG_VERSION").into(),
            log_dir: log_dir(home),
        }))
    };
    match view {
        BackendView::Starting => LocalBackendStatus::Starting,
        BackendView::Restarting { attempt } => LocalBackendStatus::Restarting { attempt },
        BackendView::Ready => LocalBackendStatus::Ready,
        BackendView::Failed(failure) => {
            let offer = if failure.managed_policy {
                RecoveryOffer::ManagedPolicy
            } else {
                RecoveryOffer::Releases
            };
            failed(failure, offer)
        }
        // `showErrorBox("This T3 home is unavailable", homeOwnedMessage)`; the user closes the
        // other server and reopens the app, so nothing here retries.
        BackendView::HomeOwned(message) => failed(
            StartupFailure::new("This T3 home is unavailable.", false, &message),
            RecoveryOffer::HomeOwned,
        ),
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
    // A configured port is used as given; otherwise the first free one from the default
    // (`resolveDesktopBackendPort`).
    let configured: Option<u16> = std::env::var("T3CODE_PORT")
        .ok()
        .and_then(|port| port.parse().ok());
    let home = t3_home();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .ok()?;
    let supervisor: Arc<Mutex<Option<Supervisor>>> = Arc::default();
    // Copy, Open releases, Reinstall and Quit are done by the window itself (it opens links through
    // the app's link policy, writes the clipboard and quits through the app's own shutdown, after
    // which `ShellBackend` stops the server). Only the updater would be the shell's.
    let publisher: LocalBackendPublisher = install_local_backend(|action| {
        if action == RecoveryAction::Update {
            eprintln!("t3-desktop: updates are not available yet");
        }
    });
    let source = LocalServer {
        program,
        port: configured.unwrap_or(DEFAULT_PORT),
        scan_ports: configured.is_none(),
        host: "127.0.0.1".into(),
        t3_home: home.as_ref().map(|home| home.display().to_string()),
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
                publisher.publish(status(view, home.as_ref()));
            }
        }
    });
    Some(ShellBackend {
        runtime,
        supervisor,
    })
}
