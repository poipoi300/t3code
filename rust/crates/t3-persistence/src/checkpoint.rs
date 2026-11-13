use std::{
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::Duration,
};

use rusqlite::Connection;

use crate::Result;

pub(crate) struct CheckpointWorker {
    sender: Option<mpsc::SyncSender<()>>,
    thread: Option<JoinHandle<()>>,
    warning: Arc<Mutex<Option<String>>>,
}

impl CheckpointWorker {
    pub(crate) fn start(path: &Path) -> std::io::Result<Self> {
        Self::spawn(path, |_, _| {})
    }

    #[cfg(test)]
    pub(crate) fn start_observed(
        path: &Path,
    ) -> std::io::Result<(Self, mpsc::Receiver<(bool, bool)>)> {
        let (sender, receiver) = mpsc::channel();
        Ok((
            Self::spawn(path, move |force, retry_idle| {
                let _ = sender.send((force, retry_idle));
            })?,
            receiver,
        ))
    }

    fn spawn(path: &Path, observed: impl Fn(bool, bool) + Send + 'static) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let warning = Arc::new(Mutex::new(None));
        let error = warning.clone();
        let path = path.to_owned();
        let thread = std::thread::Builder::new().name("sqlite-checkpoint".into()).spawn(move || {
            let run = || -> Result<()> {
                if receiver.recv().is_err() { return Ok(()); }
                let connection = Connection::open(path)?;
                connection.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA checkpoint_fullfsync = ON;")?;
                let mut force = false;
                loop {
                    let retry_idle = check(&connection, force)?;
                    observed(force, retry_idle);
                    force = if retry_idle {
                        match receiver.recv_timeout(Duration::from_millis(100)) {
                            Ok(()) => false,
                            Err(mpsc::RecvTimeoutError::Timeout) => true,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        if receiver.recv().is_err() { break; }
                        false
                    };
                }
                Ok(())
            };
            if let Err(cause) = run() {
                *error.lock().unwrap_or_else(|error| error.into_inner()) = Some(format!("SQLite checkpoint worker unavailable; the writer checkpoints its own WAL: {cause}"));
            }
        })?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            warning,
        })
    }

    pub(crate) fn notify(&self) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(());
        }
    }

    pub(crate) fn warning(&self) -> Option<String> {
        self.warning
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl Drop for CheckpointWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Retries at idle only while a checkpoint progresses; pinned readers wait for the next write.
pub(crate) fn check(connection: &Connection, force: bool) -> Result<bool> {
    let frames = || -> rusqlite::Result<(i64, i64)> {
        connection.query_row("PRAGMA wal_checkpoint(NOOP)", [], |row| {
            Ok((row.get(1)?, row.get(2)?))
        })
    };
    let before = frames()?;
    let mut state = before;
    if state.0 - state.1 >= 1000 || (force && state.0 > state.1) {
        connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
        state = frames()?;
    }
    Ok(state.0 > state.1 && (!force || state.1 > before.1))
}
