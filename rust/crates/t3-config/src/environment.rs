use crate::{Result, files, home::HomeLayout, invalid};
use std::path::{Path, PathBuf};
use t3_contracts::base_schemas::EnvironmentId;

pub async fn environment_id(layout: &HomeLayout) -> Result<EnvironmentId> {
    let path = layout.environment_id();
    fn existing(path: &Path) -> Result<Option<String>> {
        Ok(files::read(path)?
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()))
    }
    if let Some(id) = existing(&path)? {
        return Ok(EnvironmentId(id));
    }
    let generated = format!("{}\n", uuid::Uuid::new_v4());
    files::publish_if_absent(&path, generated.as_bytes())?;
    if let Some(id) = existing(&path)? {
        return Ok(EnvironmentId(id));
    }
    let recovery = layout.environment_id_recovery();
    files::publish_if_absent(&recovery, generated.as_bytes())?;
    let recovered = files::read(&recovery)?
        .ok_or_else(|| invalid("environment ID", "missing recovery file"))?;
    files::atomic_write(&path, recovered.as_bytes()).await?;
    existing(&path)?
        .map(EnvironmentId)
        .ok_or_else(|| invalid("environment ID", "missing or empty after initialization"))
}

pub fn resolve_worktree_home(cwd: &Path) -> Option<PathBuf> {
    let cwd = std::path::absolute(if cwd.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cwd
    })
    .ok()?;
    let mut resolved = PathBuf::new();
    for part in cwd.components() {
        match part {
            std::path::Component::CurDir => (),
            std::path::Component::ParentDir => {
                if resolved.file_name().is_some() {
                    resolved.pop();
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    for directory in resolved.ancestors() {
        let git = directory.join(".git");
        let Ok(metadata) = std::fs::metadata(&git) else {
            continue;
        };
        if !metadata.is_file() {
            return None;
        }
        let contents = std::fs::read_to_string(&git).ok()?;
        let gitdir = contents
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix("gitdir:").map(str::trim))?;
        let normalized = gitdir.replace('\\', "/");
        let segments: Vec<_> = normalized
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();
        return (segments.len() >= 3 && segments[segments.len() - 2] == "worktrees")
            .then(|| directory.join(".t3"));
    }
    None
}

pub fn hydrate_posix_home(env: &mut crate::config::Environment, account_home: &str) {
    if env.get("HOME").is_none_or(|s| s.trim().is_empty()) && !account_home.is_empty() {
        env.insert("HOME".into(), account_home.into());
    }
}
pub fn parse_machine_info_value(raw: &str, key: &str) -> Option<String> {
    raw.lines().map(str::trim).find_map(|s| {
        let value = s.strip_prefix(&format!("{key}="))?.trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        (!value.trim().is_empty()).then(|| value.trim().to_owned())
    })
}
pub fn environment_label(
    friendly: Option<&str>,
    hostname: Option<&str>,
    cwd_basename: &str,
) -> String {
    [friendly, hostname, Some(cwd_basename)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("T3 environment")
        .into()
}
