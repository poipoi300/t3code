use crate::{
    environment::{CodexHomeLayout, absolute_path, codex_home_layout},
    sensitive::{ServiceError, ServiceResult},
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const SHARED: &[&str] = &[
    "sessions",
    "archived_sessions",
    "sqlite",
    "shell_snapshots",
    "worktrees",
    "skills",
    "plugins",
    "cache",
    "logs",
    "mcp-oauth-locks",
    "thread-writer-locks",
];
const PRIVATE: &[&str] = &["auth.json", "auth.lock", "models_cache.json"];
const LOCAL: &[&str] = &["log", "memories", "tmp", ".DS_Store"];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeWarning {
    LiveWriterLocks,
}
pub fn managed_codex_layout(
    state: &Path,
    instance: &str,
    shared: &str,
    shadow: &str,
    home: &Path,
    cwd: &Path,
) -> ServiceResult<CodexHomeLayout> {
    if instance.is_empty() || matches!(instance, "." | "..") || instance.contains(['/', '\\']) {
        return Err(ServiceError::Response);
    }
    let shadow = if !shadow.trim().is_empty() || instance == "codex" {
        PathBuf::from(shadow)
    } else {
        state
            .join("providers")
            .join("codex")
            .join(instance)
            .join("shadow")
    };
    Ok(codex_home_layout(
        shared,
        &shadow.to_string_lossy(),
        home,
        cwd,
    ))
}
fn link_state(path: &Path) -> ServiceResult<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ServiceError::Storage),
    }
}
fn unlink(path: &Path) -> ServiceResult<()> {
    fs::remove_file(path)
        .or_else(|_| fs::remove_dir(path))
        .map_err(|_| ServiceError::Storage)
}
fn create_link(target: &Path, link: &Path) -> ServiceResult<()> {
    #[cfg(windows)]
    let result = if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    #[cfg(not(any(windows, unix)))]
    let result = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks unavailable",
    ));
    result.map_err(|_| ServiceError::Storage)
}
fn comparable(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
            return PathBuf::from(format!("\\\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix("\\\\?\\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}
pub fn materialize_codex_shadow(layout: &CodexHomeLayout) -> ServiceResult<Vec<HomeWarning>> {
    if !layout.auth_overlay {
        return Ok(vec![]);
    }
    let shadow = layout
        .effective_home
        .as_ref()
        .ok_or(ServiceError::Storage)?;
    if comparable(shadow) == comparable(&layout.shared_home) {
        return Err(ServiceError::Identity);
    }
    fs::create_dir_all(&layout.shared_home).map_err(|_| ServiceError::Storage)?;
    fs::create_dir_all(shadow).map_err(|_| ServiceError::Storage)?;
    let shared_canonical =
        fs::canonicalize(&layout.shared_home).map_err(|_| ServiceError::Storage)?;
    let shadow_canonical = fs::canonicalize(shadow).map_err(|_| ServiceError::Storage)?;
    if shared_canonical == shadow_canonical {
        return Err(ServiceError::Identity);
    }
    for directory in SHARED {
        fs::create_dir_all(layout.shared_home.join(directory))
            .map_err(|_| ServiceError::Storage)?;
    }
    let mut entries: BTreeSet<_> = SHARED.iter().map(|s| s.to_string()).collect();
    for entry in fs::read_dir(&layout.shared_home).map_err(|_| ServiceError::Storage)? {
        let entry = entry
            .map_err(|_| ServiceError::Storage)?
            .file_name()
            .to_str()
            .ok_or(ServiceError::Storage)?
            .to_owned();
        if !PRIVATE.contains(&entry.as_str()) && !LOCAL.contains(&entry.as_str()) {
            entries.insert(entry);
        }
    }
    for private in ["auth.lock", "models_cache.json"] {
        let path = shadow.join(private);
        if link_state(&path)?.is_some_and(|m| m.file_type().is_symlink()) {
            unlink(&path)?;
        }
    }
    let mut warnings = vec![];
    for entry in entries {
        let target = layout.shared_home.join(&entry);
        let link = shadow.join(&entry);
        match link_state(&link)? {
            None => create_link(&target, &link)?,
            Some(metadata) if metadata.file_type().is_symlink() => {
                let existing = fs::read_link(&link).map_err(|_| ServiceError::Storage)?;
                let resolved = absolute_path(&existing, shadow);
                if comparable(&resolved) != comparable(&target) {
                    unlink(&link)?;
                    create_link(&target, &link)?;
                }
            }
            Some(metadata) => {
                if ![
                    "mcp-oauth-locks",
                    "thread-writer-locks",
                    ".sqlite-maintenance.lock",
                ]
                .contains(&entry.as_str())
                {
                    return Err(ServiceError::Storage);
                }
                if entry == "thread-writer-locks" {
                    let mut live = false;
                    for entry in fs::read_dir(&link).map_err(|_| ServiceError::Storage)? {
                        let name = entry.map_err(|_| ServiceError::Storage)?.file_name();
                        let name = name.to_string_lossy();
                        if name.ends_with(".lock") && name != ".coordination.lock" {
                            live = true;
                            break;
                        }
                    }
                    if live {
                        warnings.push(HomeWarning::LiveWriterLocks);
                        continue;
                    }
                }
                let resolved = fs::canonicalize(&link).map_err(|_| ServiceError::Storage)?;
                if resolved == shadow_canonical || !resolved.starts_with(&shadow_canonical) {
                    return Err(ServiceError::Identity);
                }
                if metadata.is_dir() {
                    fs::remove_dir_all(&link)
                } else {
                    fs::remove_file(&link)
                }
                .map_err(|_| ServiceError::Storage)?;
                create_link(&target, &link)?;
            }
        }
    }
    if link_state(&shadow.join("auth.json"))?.is_some_and(|m| m.file_type().is_symlink()) {
        return Err(ServiceError::Identity);
    }
    Ok(warnings)
}
