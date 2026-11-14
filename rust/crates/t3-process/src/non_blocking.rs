use crate::{ProcessError, ProcessOptions, ResolvedCommand, SpawnedProcess, spawn};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::sync::{Semaphore, watch};

const ENV: &str = "/usr/bin/env";

pub fn env_trampoline(
    command: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
) -> Option<ResolvedCommand> {
    if command.is_empty() || command == "-" || command.as_encoded_bytes().contains(&b'=') {
        return None;
    }
    let mut wrapped = Vec::new();
    if let Some(cwd) = cwd {
        wrapped.extend([OsString::from("-C"), cwd.as_os_str().into()]);
    }
    wrapped.extend(["--".into(), command.into()]);
    wrapped.extend_from_slice(args);
    Some(ResolvedCommand {
        program: ENV.into(),
        args: wrapped,
        shell_line: None,
    })
}

/// The cached probe outlives any caller cancelled while awaiting it.
pub async fn supports_env_chdir() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    static PROBE: OnceLock<watch::Receiver<Option<bool>>> = OnceLock::new();
    let mut result = PROBE
        .get_or_init(|| {
            let (send, receive) = watch::channel(None);
            tokio::spawn(async move {
                let supported = tokio::process::Command::new(ENV)
                    .args(["-C", "/", "--", "/bin/sh", "-c", ":"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .status()
                    .await
                    .is_ok_and(|status| status.success());
                let _ = send.send(Some(supported));
            });
            receive
        })
        .clone();
    result
        .wait_for(Option::is_some)
        .await
        .ok()
        .and_then(|value| *value)
        .unwrap_or(false)
}

struct CheckScheduler {
    slots: Arc<Semaphore>,
    directories: Mutex<BTreeMap<PathBuf, Weak<Semaphore>>>,
}

impl Default for CheckScheduler {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
            directories: Mutex::default(),
        }
    }
}

impl CheckScheduler {
    async fn check_with(
        &self,
        directory: PathBuf,
        check: impl FnOnce() -> io::Result<()> + Send + 'static,
    ) -> io::Result<()> {
        let directory = {
            let mut directories = self
                .directories
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            directories.retain(|_, value| value.strong_count() > 0);
            if let Some(existing) = directories.get(&directory).and_then(Weak::upgrade) {
                existing
            } else {
                let gate = Arc::new(Semaphore::new(1));
                directories.insert(directory, Arc::downgrade(&gate));
                gate
            }
        };
        let directory_permit = directory.acquire_owned().await.map_err(io::Error::other)?;
        let global_permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(io::Error::other)?;
        // A cancelled caller cannot free a slot while its mount check still holds a worker.
        tokio::task::spawn_blocking(move || {
            let _directory = directory_permit;
            let _global = global_permit;
            check()
        })
        .await
        .map_err(io::Error::other)?
    }
}

#[derive(Clone, Copy)]
enum Access {
    Present,
    Executable,
}
type AccessFn = fn(&Path, Access) -> io::Result<()>;

fn native_access(path: &Path, mode: Access) -> io::Result<()> {
    #[cfg(unix)]
    {
        use nix::unistd::{AccessFlags, access};
        access(
            path,
            match mode {
                Access::Present => AccessFlags::F_OK,
                Access::Executable => AccessFlags::X_OK,
            },
        )
        .map_err(|error| io::Error::from_raw_os_error(error as i32))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Err(io::Error::other(
            "POSIX preflight is unsupported on this host",
        ))
    }
}

async fn check(
    scheduler: &CheckScheduler,
    directory: PathBuf,
    path: PathBuf,
    mode: Access,
    access: AccessFn,
) -> io::Result<()> {
    scheduler
        .check_with(directory, move || access(&path, mode))
        .await
}

fn resolved_cwd(path: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

async fn prepare(
    options: &ProcessOptions,
    enabled: bool,
    scheduler: &CheckScheduler,
    access: AccessFn,
) -> Result<ProcessOptions, ProcessError> {
    if !enabled
        || options.program.as_encoded_bytes().contains(&0)
        || options
            .args
            .iter()
            .any(|arg| arg.as_encoded_bytes().contains(&0))
    {
        return Ok(options.clone());
    }
    let given_cwd = options.spawn_cwd.as_deref().or(options.cwd.as_deref());
    let cwd = given_cwd
        .map(resolved_cwd)
        .transpose()
        .map_err(|source| ProcessError::Spawn {
            program: options.program.clone(),
            source,
        })?;
    let Some(trampoline) = env_trampoline(&options.program, &options.args, cwd.as_deref()) else {
        return Ok(options.clone());
    };
    if let (Some(given), Some(resolved)) = (given_cwd, &cwd) {
        check(
            scheduler,
            resolved.clone(),
            given.to_owned(),
            Access::Present,
            access,
        )
        .await
        .map_err(|source| ProcessError::Spawn {
            program: options.program.clone(),
            source,
        })?;
    }
    let from_cwd = |path: &Path| {
        if path.is_absolute() || cwd.is_none() {
            path.to_owned()
        } else {
            cwd.as_ref()
                .map_or_else(|| path.to_owned(), |cwd| cwd.join(path))
        }
    };
    let candidates = if options.program.as_encoded_bytes().contains(&b'/') {
        vec![from_cwd(Path::new(&options.program))]
    } else {
        let path = options
            .env
            .get(OsStr::new("PATH"))
            .cloned()
            .unwrap_or_else(|| {
                options
                    .inherit_env
                    .then(|| std::env::var_os("PATH"))
                    .flatten()
            })
            .unwrap_or_else(|| "/usr/bin:/bin".into());
        std::env::split_paths(&path)
            .map(|entry| {
                from_cwd(if entry.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    &entry
                })
                .join(&options.program)
            })
            .collect()
    };
    let mut denied = false;
    let mut found = false;
    for candidate in candidates {
        let directory = candidate
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_owned();
        match check(scheduler, directory, candidate, Access::Executable, access).await {
            Ok(()) => {
                found = true;
                break;
            }
            Err(error) if error.raw_os_error() == Some(13) => denied = true,
            Err(error) if error.raw_os_error() == Some(2) => {}
            // Leave uncommon errno values for env to report, matching the upstream adapter.
            Err(_) => {
                found = true;
                break;
            }
        }
    }
    if !found {
        return Err(if denied {
            ProcessError::Spawn {
                program: options.program.clone(),
                source: io::Error::from_raw_os_error(13),
            }
        } else {
            ProcessError::NotFound {
                program: options.program.clone(),
            }
        });
    }
    let mut prepared = options.clone();
    prepared.program = trampoline.program;
    prepared.args = trampoline.args;
    prepared.cwd = None;
    prepared.spawn_cwd = None;
    Ok(prepared)
}

/// Async preflight and spawn; Linux/GNU env enters cwd and searches PATH after exec.
pub async fn spawn_non_blocking(options: &ProcessOptions) -> Result<SpawnedProcess, ProcessError> {
    static CHECKS: OnceLock<CheckScheduler> = OnceLock::new();
    let spawning = async {
        let prepared = prepare(
            options,
            supports_env_chdir().await,
            CHECKS.get_or_init(CheckScheduler::default),
            native_access,
        )
        .await?;
        tokio::task::spawn_blocking(move || spawn(&prepared))
            .await
            .map_err(|source| ProcessError::Spawn {
                program: options.program.clone(),
                source: io::Error::other(source),
            })?
    };
    tokio::select! {
        biased;
        _ = options.cancellation.cancelled() => Err(ProcessError::Cancelled),
        result = spawning => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn preflight_checks_use_two_global_slots_and_one_per_directory() {
        let checks = Arc::new(CheckScheduler::default());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (arrivals, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = tokio::task::JoinSet::new();
        for directory in ["a", "a", "b", "c", "a"] {
            let checks = checks.clone();
            let active = active.clone();
            let peak = peak.clone();
            let arrivals = arrivals.clone();
            tasks.spawn(async move {
                checks
                    .check_with(directory.into(), move || {
                        let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(running, Ordering::SeqCst);
                        let (release, wait) = tokio::sync::oneshot::channel();
                        arrivals
                            .send((directory, release))
                            .map_err(io::Error::other)?;
                        let result = wait.blocking_recv().map_err(io::Error::other);
                        active.fetch_sub(1, Ordering::SeqCst);
                        result
                    })
                    .await
            });
        }
        let first = received.recv().await.expect("first check");
        let second = received.recv().await.expect("second check");
        assert_ne!(first.0, second.0);
        assert_eq!(active.load(Ordering::SeqCst), 2);
        first.1.send(()).expect("release first");
        second.1.send(()).expect("release second");
        for _ in 0..3 {
            received
                .recv()
                .await
                .expect("next check")
                .1
                .send(())
                .expect("release");
        }
        while let Some(result) = tasks.join_next().await {
            result.expect("task").expect("check");
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn prepared_spawn_uses_only_absolute_env_and_no_native_cwd() {
        let mut options = ProcessOptions::new("tool").args(["a b", "$HOME", "", "A=b"]);
        options.cwd = Some(std::env::current_dir().expect("cwd"));
        let prepared = prepare(&options, true, &CheckScheduler::default(), |_, _| Ok(()))
            .await
            .expect("prepare");
        assert_eq!(prepared.program, "/usr/bin/env");
        assert!(prepared.cwd.is_none());
        assert!(prepared.spawn_cwd.is_none());
        assert_eq!(&prepared.args[prepared.args.len() - 4..], options.args);
        assert_eq!(prepared.env, options.env);
        let unchanged = prepare(&options, false, &CheckScheduler::default(), |_, _| {
            panic!("no preflight")
        })
        .await
        .expect("pass through");
        assert_eq!(unchanged.program, options.program);
        assert_eq!(unchanged.cwd, options.cwd);
    }

    #[tokio::test]
    async fn executable_preflight_preserves_path_search_and_errno_policy() {
        fn access(path: &Path, _: Access) -> io::Result<()> {
            if path.to_string_lossy().contains("denied") {
                Err(io::Error::from_raw_os_error(13))
            } else if path.to_string_lossy().contains("missing") {
                Err(io::Error::from_raw_os_error(2))
            } else if path.to_string_lossy().contains("other") {
                Err(io::Error::from_raw_os_error(20))
            } else {
                Ok(())
            }
        }
        let checks = CheckScheduler::default();
        for (paths, errno) in [
            (vec!["/denied", "/fresh"], None),
            (vec!["/missing", "/fresh"], None),
            (vec!["/denied", "/other"], None),
            (vec!["/denied", "/missing"], Some(13)),
            (vec!["/missing"], Some(2)),
        ] {
            let mut options = ProcessOptions::new("tool");
            options.inherit_env = false;
            options.env.insert(
                "PATH".into(),
                Some(std::env::join_paths(paths).expect("PATH")),
            );
            let result = prepare(&options, true, &checks, access).await;
            match errno {
                None => assert_eq!(result.expect("found").program, ENV),
                Some(13) => assert!(
                    matches!(result, Err(ProcessError::Spawn { source, .. }) if source.raw_os_error() == Some(13))
                ),
                Some(2) => assert!(matches!(result, Err(ProcessError::NotFound { .. }))),
                _ => unreachable!(),
            }
        }
    }
}
