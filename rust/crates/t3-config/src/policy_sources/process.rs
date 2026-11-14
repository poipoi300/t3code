//! A child process run under one deadline that covers both its output and its exit, as
//! TypeScript's `Effect.timeout` around `run` does (`managedPolicyFile.ts:335-350`).

use super::Converted;
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Runs `command` with stdin null, stderr null and stdout piped unless the caller set it, and
/// returns what it printed and how it exited. Past `timeout` the child is killed and the result
/// is `TimedOut`.
pub(crate) fn run_with_deadline(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<Converted> {
    let deadline = Instant::now() + timeout;
    let mut child = command.stdin(Stdio::null()).stderr(Stdio::null()).spawn()?;
    let (sender, receiver) = mpsc::channel();
    match child.stdout.take() {
        Some(mut stdout) => {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = sender.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
            });
        }
        None => {
            let _ = sender.send(Ok(Vec::new()));
        }
    }
    let bytes = match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Err(_) => {
            // Our own child, not yet reaped: its PID cannot have been reused.
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the command did not finish",
            ));
        }
    };
    // The output has ended; the exit is held to the same deadline (review L3).
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the command did not exit",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Converted {
        stdout: String::from_utf8_lossy(&bytes).into_owned(),
        exit_code: status.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command that runs for about 30 seconds and prints nothing until it ends.
    fn long_running() -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("ping");
            command.args(["-n", "31", "127.0.0.1"]);
            command
        }
        #[cfg(unix)]
        {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        }
    }

    #[test]
    fn returns_the_output_and_exit_code_of_a_command_that_finishes() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/c", "echo converted"]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "echo converted"]);
            command
        };
        command.stdout(Stdio::piped());
        let converted = run_with_deadline(&mut command, Duration::from_secs(30)).unwrap();
        assert_eq!(converted.stdout.trim_end(), "converted");
        assert_eq!(converted.exit_code, Some(0));
    }

    /// Output that never ends, and (review L3) a child whose output has ended but which does not
    /// exit: both are killed at the deadline instead of holding start.
    #[test]
    fn the_deadline_covers_the_output_and_the_exit() {
        for stdout in [Stdio::piped(), Stdio::null()] {
            let mut command = long_running();
            command.stdout(stdout);
            let error = run_with_deadline(&mut command, Duration::from_millis(500)).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        }
    }
}
