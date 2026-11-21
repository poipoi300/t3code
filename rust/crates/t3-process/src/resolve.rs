use crate::{Environment, ProcessError};
#[cfg(windows)]
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

#[cfg(windows)]
#[derive(PartialEq, Eq)]
struct CacheKey {
    program: OsString,
    path: Option<OsString>,
    extensions: Vec<String>,
    msystem: bool,
}

#[cfg(windows)]
#[derive(Default)]
struct ResolutionCache(VecDeque<(CacheKey, PathBuf, Instant)>);

#[cfg(windows)]
impl ResolutionCache {
    fn get(&self, key: &CacheKey, now: Instant) -> Option<PathBuf> {
        self.0
            .iter()
            .find(|(cached, _, expires)| cached == key && *expires > now)
            .map(|(_, path, _)| path.clone())
            .filter(|path| path.is_file())
    }
    fn insert(&mut self, key: CacheKey, path: PathBuf, now: Instant) {
        self.0
            .retain(|(cached, _, expires)| cached != &key && *expires > now);
        if self.0.len() == 512 {
            self.0.pop_front();
        }
        self.0.push_back((key, path, now + Duration::from_secs(30)));
    }
}

#[cfg(windows)]
fn resolution_cache() -> std::sync::MutexGuard<'static, ResolutionCache> {
    static CACHE: OnceLock<Mutex<ResolutionCache>> = OnceLock::new();
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Reserved for compatibility; resolution returns `None` and std handles batch quoting.
    pub shell_line: Option<OsString>,
}

/// Find an executable for callers that inspect the file before spawning it.
/// Preserve the invoked path so version-manager shims retain their dispatch names.
pub fn resolve_command_path(program: &OsStr, env: &Environment) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use nix::unistd::{AccessFlags, access};

        let is_executable = |path: &Path| path.is_file() && access(path, AccessFlags::X_OK).is_ok();
        let path = if program.as_encoded_bytes().contains(&b'/') {
            let path = PathBuf::from(program);
            is_executable(&path).then_some(path)?
        } else {
            let search = ["PATH", "Path", "path"]
                .into_iter()
                .find_map(|key| env.get(OsStr::new(key)).and_then(Option::as_deref))?;
            std::env::split_paths(search)
                .map(|entry| match entry.to_str() {
                    Some(text) => PathBuf::from(text.trim().trim_matches('"')),
                    None => entry,
                })
                .filter(|entry| !entry.as_os_str().is_empty())
                .map(|entry| entry.join(program))
                .find(|path| is_executable(path))?
        };
        std::path::absolute(path).ok()
    }
    #[cfg(not(unix))]
    {
        resolve_command(program, &[], env, false)
            .ok()
            .map(|resolved| PathBuf::from(resolved.program))
    }
}

pub fn resolve_command(
    program: &OsStr,
    args: &[OsString],
    patch: &Environment,
    inherit: bool,
) -> Result<ResolvedCommand, ProcessError> {
    #[cfg(windows)]
    {
        let get = |key: &str| {
            patch
                .iter()
                .rev()
                .find(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case(key))
                .map_or_else(
                    || inherit.then(|| std::env::var_os(key)).flatten(),
                    |(_, value)| value.clone(),
                )
        };
        let extensions = path_extensions(get("PATHEXT").as_deref());
        let path = get("PATH");
        let msystem = get("MSYSTEM").is_some_and(|value| !value.is_empty());
        let explicit = program.to_string_lossy().contains(['/', '\\']);
        let candidates = command_candidates(program, &extensions);
        let directories = if explicit {
            vec![std::path::PathBuf::new()]
        } else {
            path.as_ref()
                .map(|path| {
                    path.to_string_lossy()
                        .split(';')
                        .filter_map(|part| {
                            let part = part.trim().trim_matches('"');
                            if part.is_empty() {
                                None
                            } else {
                                Some(std::path::PathBuf::from(part))
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        // Match shell.ts's positive 30-second cache; relative PATH entries must follow cwd.
        let cache_key =
            (!explicit && directories.iter().all(|path| path.is_absolute())).then(|| CacheKey {
                program: program.to_owned(),
                path,
                extensions,
                msystem,
            });
        if let Some(found) = cache_key
            .as_ref()
            .and_then(|key| resolution_cache().get(key, Instant::now()))
        {
            return Ok(ResolvedCommand {
                program: found.into_os_string(),
                args: args.to_vec(),
                shell_line: None,
            });
        }
        let found = directories
            .iter()
            .flat_map(|directory| {
                candidates
                    .iter()
                    .map(move |candidate| directory.join(candidate))
            })
            .find(|path| path.is_file())
            .or_else(|| explicit.then(|| std::path::PathBuf::from(program)));
        let mut found = found.ok_or_else(|| ProcessError::NotFound {
            program: program.to_owned(),
        })?;
        // The cmd/git.exe launcher adds a process and a console; use its installed native peer.
        if !msystem
            && found
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("git.exe"))
            && let Some(root) = found
                .parent()
                .filter(|parent| {
                    parent.file_name().is_some_and(|name| {
                        name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("bin")
                    })
                })
                .and_then(Path::parent)
        {
            for build in ["ucrt64", "clangarm64", "mingw64", "mingw32"] {
                let native = root.join(build).join("bin/git.exe");
                if native.is_file() {
                    found = native;
                    break;
                }
            }
        }
        if let Some(key) = cache_key {
            resolution_cache().insert(key, found.clone(), Instant::now());
        }
        Ok(ResolvedCommand {
            program: found.into_os_string(),
            args: args.to_vec(),
            shell_line: None,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (patch, inherit);
        Ok(ResolvedCommand {
            program: program.to_owned(),
            args: args.to_vec(),
            shell_line: None,
        })
    }
}

#[cfg(any(windows, test))]
fn path_extensions(value: Option<&OsStr>) -> Vec<String> {
    let mut result = Vec::new();
    for extension in value
        .unwrap_or_else(|| OsStr::new(".COM;.EXE;.BAT;.CMD"))
        .to_string_lossy()
        .split(';')
    {
        let extension = extension.trim();
        if extension.is_empty() {
            continue;
        }
        let extension = if extension.starts_with('.') {
            extension.to_uppercase()
        } else {
            format!(".{}", extension.to_uppercase())
        };
        if !result.contains(&extension) {
            result.push(extension);
        }
    }
    if result.is_empty() {
        return vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()];
    }
    result
}

#[cfg(any(windows, test))]
fn command_candidates(program: &OsStr, extensions: &[String]) -> Vec<OsString> {
    let path = Path::new(program);
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_uppercase()));
    if let Some(extension) = extension.filter(|ext| extensions.contains(ext)) {
        return vec![
            program.to_owned(),
            path.with_extension(extension.trim_start_matches('.'))
                .into_os_string(),
            path.with_extension(extension.trim_start_matches('.').to_lowercase())
                .into_os_string(),
        ];
    }
    extensions
        .iter()
        .flat_map(|extension| [extension.clone(), extension.to_lowercase()])
        .map(|extension| {
            let mut candidate = program.to_owned();
            candidate.push(extension);
            candidate
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_pathext_matches_typescript() {
        assert_eq!(
            path_extensions(Some(OsStr::new(" exe ; .cmd;EXE;"))),
            vec![".EXE", ".CMD"]
        );
        assert_eq!(
            command_candidates(OsStr::new("tool"), &[".EXE".into()]),
            vec![OsString::from("tool.EXE"), "tool.exe".into()]
        );
    }

    #[cfg(windows)]
    #[test]
    fn positive_cache_expires_evicts_and_rechecks_files() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("tool.exe");
        std::fs::write(&path, "")?;
        let key = |index| CacheKey {
            program: format!("tool{index}").into(),
            path: None,
            extensions: vec![],
            msystem: false,
        };
        let now = Instant::now();
        let mut cache = ResolutionCache::default();
        for index in 0..513 {
            cache.insert(key(index), path.clone(), now);
        }
        assert!(cache.get(&key(0), now).is_none());
        assert_eq!(cache.get(&key(512), now), Some(path.clone()));
        assert!(
            cache
                .get(&key(512), now + Duration::from_secs(30))
                .is_none()
        );
        std::fs::remove_file(path)?;
        assert!(cache.get(&key(512), now).is_none());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn resolution_cache_separates_environments_and_does_not_cache_misses()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let mut env = Environment::new();
        env.insert("PATH".into(), Some(temp.path().as_os_str().to_owned()));
        env.insert("PATHEXT".into(), Some("EXE".into()));
        let program = OsStr::new("cache-fixture");
        assert!(matches!(
            resolve_command(program, &[], &env, false),
            Err(ProcessError::NotFound { .. })
        ));
        let exe = temp.path().join("cache-fixture.EXE");
        std::fs::write(&exe, "")?;
        assert_eq!(resolve_command(program, &[], &env, false)?.program, exe);
        let args = vec!["new argument".into()];
        assert_eq!(resolve_command(program, &args, &env, false)?.args, args);
        let batch = temp.path().join("cache-fixture.CMD");
        std::fs::write(&batch, "")?;
        env.insert("PATHEXT".into(), Some("CMD".into()));
        assert_eq!(resolve_command(program, &[], &env, false)?.program, batch);
        let other = temp.path().join("other");
        std::fs::create_dir(&other)?;
        let other_batch = other.join("cache-fixture.CMD");
        std::fs::write(&other_batch, "")?;
        env.insert("PATH".into(), Some(other.as_os_str().to_owned()));
        assert_eq!(
            resolve_command(program, &[], &env, false)?.program,
            other_batch
        );
        Ok(())
    }
}
