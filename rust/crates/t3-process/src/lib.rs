//! Native asynchronous child processes with owned process-tree lifetimes.
//! This crate does not compile for wasm32-unknown-unknown.

mod detached;
mod non_blocking;
pub mod provider_environment;
mod resolve;
pub use detached::{DetachedConsole, spawn_detached};
pub use non_blocking::{env_trampoline, spawn_non_blocking, supports_env_chdir};

use process_wrap::tokio::{ChildWrapper, CommandWrap};
pub use resolve::{ResolvedCommand, resolve_command, resolve_command_path};
use std::{
    collections::BTreeMap, ffi::OsString, io, path::PathBuf, process::Stdio, sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{ChildStderr, ChildStdin, ChildStdout},
};
pub use tokio_util::sync::CancellationToken;

pub type Environment = BTreeMap<OsString, Option<OsString>>;
pub type OutputCallback = Arc<dyn Fn(&[u8]) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Error,
    Truncate,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("executable not found: {program:?}")]
    NotFound { program: OsString },
    #[error("failed to spawn {program:?}: {source}")]
    Spawn {
        program: OsString,
        #[source]
        source: io::Error,
    },
    #[error("process I/O failed on {stream}: {source}")]
    Io {
        stream: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("process exited unsuccessfully ({code:?})")]
    NonZeroExit {
        code: Option<i32>,
        output: Box<ProcessOutput>,
    },
    #[error("process timed out after {timeout:?}")]
    Timeout { timeout: Duration },
    #[error("process was cancelled")]
    Cancelled,
    #[error("{stream} produced {observed_bytes} bytes, exceeding the {max_bytes} byte limit")]
    OutputLimit {
        stream: &'static str,
        max_bytes: usize,
        observed_bytes: usize,
    },
}

#[derive(Clone)]
pub struct ProcessOptions {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Execution cwd can differ from the diagnostic cwd (for deleted Git workspaces).
    pub spawn_cwd: Option<PathBuf>,
    pub env: Environment,
    pub inherit_env: bool,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
    /// On Unix, signal the owned group before escalating. Windows job termination is immediate.
    pub termination_grace: Duration,
    /// Per stream, before an optional marker; both pipes are always drained concurrently.
    pub max_output_bytes: usize,
    pub output_mode: OutputMode,
    pub truncated_marker: String,
    pub check_exit: bool,
    pub timed_out_result: bool,
    pub cancellation: CancellationToken,
    pub on_stdout_chunk: Option<OutputCallback>,
    pub on_stderr_chunk: Option<OutputCallback>,
}

impl ProcessOptions {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: vec![],
            cwd: None,
            spawn_cwd: None,
            env: Environment::new(),
            inherit_env: true,
            stdin: None,
            timeout: Some(Duration::from_secs(60)),
            termination_grace: Duration::ZERO,
            max_output_bytes: 8 * 1024 * 1024,
            output_mode: OutputMode::Error,
            truncated_marker: String::new(),
            check_exit: true,
            timed_out_result: false,
            cancellation: CancellationToken::new(),
            on_stdout_chunk: None,
            on_stderr_chunk: None,
        }
    }
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_invalid_utf8: bool,
    pub stderr_invalid_utf8: bool,
}
impl ProcessOutput {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Dropping an unfinished child terminates its group/job; successful wait preserves helpers.
pub struct SpawnedProcess {
    child: Box<dyn ChildWrapper>,
    #[cfg(windows)]
    job: Option<win32job::Job>,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    reaped: bool,
}
impl SpawnedProcess {
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    pub async fn wait(&mut self) -> Result<std::process::ExitStatus, ProcessError> {
        let status = self.wait_for_exit().await?;
        self.finish()?;
        Ok(status)
    }
    fn finish(&mut self) -> Result<(), ProcessError> {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            // Successful completion releases detached helpers instead of terminating them.
            job.set_extended_limit_info(&win32job::ExtendedLimitInfo::new())
                .map_err(|source| ProcessError::Io {
                    stream: "job",
                    source: source.into(),
                })?;
            self.job.take();
        }
        self.reaped = true;
        Ok(())
    }
    fn start_kill(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        // Closing our only kill-on-close handle terminates every assigned descendant.
        self.job.take();
        self.child.start_kill()
    }
    async fn wait_for_exit(&mut self) -> Result<std::process::ExitStatus, ProcessError> {
        // A daemon may outlive its leader; process-wrap's wait also waits for job/group members.
        let native = self.child.inner_mut() as &mut dyn std::any::Any;
        native
            .downcast_mut::<tokio::process::Child>()
            .expect("spawn owns a native Tokio child")
            .wait()
            .await
            .map_err(|source| ProcessError::Io {
                stream: "exitCode",
                source,
            })
    }
    pub async fn kill(&mut self) -> Result<(), ProcessError> {
        if !self.reaped {
            self.start_kill().map_err(|source| ProcessError::Io {
                stream: "kill",
                source,
            })?;
            self.wait_for_exit().await?;
            self.reaped = true;
        }
        Ok(())
    }
    pub async fn terminate(&mut self, grace: Duration) -> Result<(), ProcessError> {
        #[cfg(unix)]
        if !self.reaped && !grace.is_zero() {
            // process-wrap dispatches this to the group we created, never to a discovered PID.
            let _ = self.child.signal(15);
            if let Ok(result) = tokio::time::timeout(grace, self.wait()).await {
                result?;
                // Reap/kill remaining group members even when the leader exited first.
                let _ = self.child.start_kill();
                return Ok(());
            }
        }
        #[cfg(not(unix))]
        let _ = grace;
        self.kill().await
    }
}
impl Drop for SpawnedProcess {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.start_kill();
        }
    }
}

pub fn spawn(options: &ProcessOptions) -> Result<SpawnedProcess, ProcessError> {
    if options.cancellation.is_cancelled() {
        return Err(ProcessError::Cancelled);
    }
    let resolved = resolve_command(
        &options.program,
        &options.args,
        &options.env,
        options.inherit_env,
    )?;
    let mut command = CommandWrap::with_new(&resolved.program, |command| {
        command.args(&resolved.args);
        if let Some(cwd) = options.spawn_cwd.as_ref().or(options.cwd.as_ref()) {
            command.current_dir(cwd);
        }
        if !options.inherit_env {
            command.env_clear();
        }
        for (key, value) in &options.env {
            if let Some(value) = value {
                command.env(key, value);
            } else {
                command.env_remove(key);
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    });
    #[cfg(windows)]
    let job = {
        let mut limits = win32job::ExtendedLimitInfo::new();
        limits.limit_kill_on_job_close();
        let job = win32job::Job::create_with_limit_info(&limits).map_err(|source| {
            ProcessError::Spawn {
                program: options.program.clone(),
                source: source.into(),
            }
        })?;
        command.wrap(process_wrap::tokio::CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        job
    };
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    let mut child = command.spawn().map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound
            && options
                .spawn_cwd
                .as_ref()
                .or(options.cwd.as_ref())
                .is_none_or(|cwd| cwd.is_dir())
        {
            ProcessError::NotFound {
                program: options.program.clone(),
            }
        } else {
            ProcessError::Spawn {
                program: options.program.clone(),
                source,
            }
        }
    })?;
    let process = SpawnedProcess {
        stdin: child.stdin().take(),
        stdout: child.stdout().take(),
        stderr: child.stderr().take(),
        child,
        #[cfg(windows)]
        job: Some(job),
        reaped: false,
    };
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let handle = process
            .child
            .process_handle()
            .expect("native Tokio process handle");
        // Assign without yielding; descendants created before assignment can escape the job.
        process
            .job
            .as_ref()
            .expect("new job")
            .assign_process(handle.as_raw_handle() as isize)
            .map_err(|source| ProcessError::Spawn {
                program: options.program.clone(),
                source: source.into(),
            })?;
    }
    Ok(process)
}

struct Collected {
    bytes: Vec<u8>,
    truncated: bool,
    invalid_utf8: bool,
}
async fn collect(
    reader: Option<impl AsyncRead + Unpin>,
    stream: &'static str,
    options: &ProcessOptions,
) -> Result<Collected, ProcessError> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    if let Some(mut reader) = reader {
        let mut chunk = vec![0; 8192];
        loop {
            let count = reader
                .read(&mut chunk)
                .await
                .map_err(|source| ProcessError::Io { stream, source })?;
            if count == 0 {
                break;
            }
            if stream == "stdout"
                && let Some(callback) = &options.on_stdout_chunk
            {
                callback(&chunk[..count]);
            }
            if stream == "stderr"
                && let Some(callback) = &options.on_stderr_chunk
            {
                callback(&chunk[..count]);
            }
            let remaining = options.max_output_bytes.saturating_sub(bytes.len());
            if count > remaining {
                if options.output_mode == OutputMode::Error {
                    return Err(ProcessError::OutputLimit {
                        stream,
                        max_bytes: options.max_output_bytes,
                        observed_bytes: bytes.len().saturating_add(count),
                    });
                }
                truncated = true;
            }
            bytes.extend_from_slice(&chunk[..count.min(remaining)]);
        }
    }
    let invalid_utf8 = std::str::from_utf8(&bytes).is_err();
    if truncated {
        bytes.extend_from_slice(options.truncated_marker.as_bytes());
    }
    Ok(Collected {
        bytes,
        truncated,
        invalid_utf8,
    })
}

pub async fn run(mut options: ProcessOptions) -> Result<ProcessOutput, ProcessError> {
    let input = options.stdin.take();
    let mut child = spawn_non_blocking(&options).await?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let result = {
        let execution = async {
            let write = async {
                if let Some(mut stdin) = stdin
                    && let Some(bytes) = &input
                {
                    stdin
                        .write_all(bytes)
                        .await
                        .map_err(|source| ProcessError::Io {
                            stream: "stdin",
                            source,
                        })?;
                    stdin.shutdown().await.map_err(|source| ProcessError::Io {
                        stream: "stdin",
                        source,
                    })?;
                }
                Ok(())
            };
            let (stdout, stderr, (), status) = tokio::try_join!(
                collect(stdout, "stdout", &options),
                collect(stderr, "stderr", &options),
                write,
                child.wait_for_exit()
            )?;
            Ok(ProcessOutput {
                stdout: stdout.bytes,
                stderr: stderr.bytes,
                code: status.code(),
                timed_out: false,
                stdout_truncated: stdout.truncated,
                stderr_truncated: stderr.truncated,
                stdout_invalid_utf8: stdout.invalid_utf8,
                stderr_invalid_utf8: stderr.invalid_utf8,
            })
        };
        let timeout = async {
            match options.timeout {
                Some(timeout) => {
                    tokio::time::sleep(timeout).await;
                    ProcessError::Timeout { timeout }
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = options.cancellation.cancelled() => Err(ProcessError::Cancelled),
            result = execution => result,
            error = timeout => Err(error),
        }
    };
    if result.is_err() {
        child.terminate(options.termination_grace).await?;
    } else {
        child.finish()?;
    }
    let output = match result {
        Err(ProcessError::Timeout { .. }) if options.timed_out_result => ProcessOutput {
            timed_out: true,
            ..ProcessOutput::default()
        },
        result => result?,
    };
    if options.check_exit && !output.timed_out && output.code != Some(0) {
        return Err(ProcessError::NonZeroExit {
            code: output.code,
            output: Box::new(output),
        });
    }
    Ok(output)
}
