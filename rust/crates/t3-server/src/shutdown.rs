use tokio_util::sync::CancellationToken;

/// Installs the process's termination handlers now and cancels `token` when one fires.
/// POSIX: SIGINT, SIGTERM. Windows: Ctrl+C, Ctrl+Break, console close, system shutdown.
/// Dropping the returned handle does not uninstall them; abort it to stop listening.
pub fn cancel_on_signal(token: CancellationToken) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let signals = Signals::install()?;
    Ok(tokio::spawn(async move {
        tokio::select! {
            () = signals.recv() => token.cancel(),
            () = token.cancelled() => {}
        }
    }))
}

/// Resolves on the first termination signal.
pub async fn signal() -> std::io::Result<()> {
    Signals::install()?.recv().await;
    Ok(())
}

#[cfg(unix)]
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}
#[cfg(unix)]
impl Signals {
    fn install() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }
    async fn recv(mut self) {
        tokio::select! { _ = self.interrupt.recv() => {}, _ = self.terminate.recv() => {} }
    }
}

#[cfg(windows)]
struct Signals {
    ctrl_c: tokio::signal::windows::CtrlC,
    ctrl_break: tokio::signal::windows::CtrlBreak,
    close: tokio::signal::windows::CtrlClose,
    shutdown: tokio::signal::windows::CtrlShutdown,
}
#[cfg(windows)]
impl Signals {
    fn install() -> std::io::Result<Self> {
        use tokio::signal::windows;
        Ok(Self {
            ctrl_c: windows::ctrl_c()?,
            ctrl_break: windows::ctrl_break()?,
            close: windows::ctrl_close()?,
            shutdown: windows::ctrl_shutdown()?,
        })
    }
    async fn recv(mut self) {
        tokio::select! {
            _ = self.ctrl_c.recv() => {},
            _ = self.ctrl_break.recv() => {},
            _ = self.close.recv() => {},
            _ = self.shutdown.recv() => {},
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct Signals;
#[cfg(not(any(unix, windows)))]
impl Signals {
    fn install() -> std::io::Result<Self> {
        Ok(Self)
    }
    async fn recv(self) {
        let _ = tokio::signal::ctrl_c().await;
    }
}
