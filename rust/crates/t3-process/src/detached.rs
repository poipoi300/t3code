//! Launches a program the user keeps (an editor or file manager): not owned by this process tree.
use crate::{ProcessError, ProcessOptions, resolve_command};
use process_wrap::tokio::CommandWrap;
use std::{io, process::Stdio};

/// How a detached Windows child gets its console; ignored elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachedConsole {
    /// No console at all (Node's `detached: true`).
    None,
    /// A hidden console of its own. Windows PowerShell started without one exits 0 without
    /// running its command (`WINDOWS_POWERSHELL_DETACHED` in `externalLauncher.ts`).
    Hidden,
}

/// Starts `options.program` with its arguments, environment and cwd, null stdio, outside this
/// crate's job and process-group ownership, so it keeps running when the caller or the server
/// exits: Node's `spawn(.., { detached: true, stdio: "ignore" })` then `unref()`. Returns the
/// child's pid once the OS created it; spawn failures are returned. On Unix the child leads a
/// new session and Tokio reaps it when it exits. On Windows it leaves a job the caller runs in
/// when that job allows breakaway, and otherwise stays in it.
///
/// With `verbatim`, Windows appends each argument to the command line unchanged, for a program
/// that parses its own command line (Explorer's `/select,"<path>"`); the caller guarantees the
/// text is in that program's syntax. Elsewhere arguments are always discrete.
pub async fn spawn_detached(
    options: &ProcessOptions,
    console: DetachedConsole,
    verbatim: bool,
) -> Result<u32, ProcessError> {
    if options.cancellation.is_cancelled() {
        return Err(ProcessError::Cancelled);
    }
    let resolved = resolve_command(
        &options.program,
        &options.args,
        &options.env,
        options.inherit_env,
    )?;
    let cwd = options.spawn_cwd.clone().or_else(|| options.cwd.clone());
    let env = options.env.clone();
    let inherit_env = options.inherit_env;
    let program = options.program.clone();
    let spawned = tokio::task::spawn_blocking(move || {
        let build = || {
            CommandWrap::with_new(&resolved.program, |command| {
                #[cfg(windows)]
                if verbatim {
                    for arg in &resolved.args {
                        command.raw_arg(arg);
                    }
                } else {
                    command.args(&resolved.args);
                }
                #[cfg(not(windows))]
                {
                    let _ = verbatim;
                    command.args(&resolved.args);
                }
                if let Some(cwd) = &cwd {
                    command.current_dir(cwd);
                }
                if !inherit_env {
                    command.env_clear();
                }
                for (key, value) in &env {
                    match value {
                        Some(value) => command.env(key, value),
                        None => command.env_remove(key),
                    };
                }
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(false);
            })
        };
        #[cfg(windows)]
        let child = {
            use windows::Win32::System::Threading::{
                CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
                DETACHED_PROCESS,
            };
            let flags = CREATE_NEW_PROCESS_GROUP
                | match console {
                    DetachedConsole::None => DETACHED_PROCESS,
                    DetachedConsole::Hidden => CREATE_NO_WINDOW,
                };
            let mut breakaway = build();
            breakaway.wrap(process_wrap::tokio::CreationFlags(
                flags | CREATE_BREAKAWAY_FROM_JOB,
            ));
            match breakaway.spawn() {
                // ERROR_ACCESS_DENIED: the caller's job forbids breakaway.
                Err(error) if error.raw_os_error() == Some(5) => {
                    let mut command = build();
                    command.wrap(process_wrap::tokio::CreationFlags(flags));
                    command.spawn()?
                }
                spawned => spawned?,
            }
        };
        #[cfg(unix)]
        let child = {
            let _ = console;
            let mut command = build();
            command.wrap(process_wrap::tokio::ProcessSession);
            command.spawn()?
        };
        // Dropping the handle neither kills nor waits: no kill-on-drop, job or owned group.
        child
            .id()
            .ok_or_else(|| io::Error::other("the detached child has no pid"))
    })
    .await
    .map_err(|source| ProcessError::Spawn {
        program: program.clone(),
        source: io::Error::other(source),
    })?;
    spawned.map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            ProcessError::NotFound { program }
        } else {
            ProcessError::Spawn { program, source }
        }
    })
}
