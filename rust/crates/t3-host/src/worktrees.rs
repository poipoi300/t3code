use crate::{Error, Result};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub fn managed_worktrees_directories(
    settings: &t3_contracts::settings::ServerSettings,
    default: &Path,
    home: &Path,
) -> Vec<PathBuf> {
    let mut result = vec![default.to_owned()];
    for setting in std::iter::once(&settings.worktrees_directory)
        .chain(settings.previous_worktrees_directories.iter())
    {
        if let Some(path) = t3_config::home::resolve_worktrees_directory(&setting.0, default, home)
            && !result.contains(&path)
        {
            result.push(path);
        }
    }
    result
}
fn real_location(path: &Path) -> Result<PathBuf> {
    match dunce::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            let parent = path.parent().ok_or(Error::Invalid)?;
            let name = path.file_name().ok_or(Error::Invalid)?;
            Ok(real_location(parent)?.join(name))
        }
        Err(_) => Err(Error::Io),
    }
}
pub fn is_managed_worktree_path(path: &Path, roots: &[PathBuf]) -> Result<bool> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Ok(false);
    }
    let candidate = real_location(path)?;
    for root in roots {
        let root = real_location(root)?;
        if root.parent().is_none() {
            continue;
        }
        if let Ok(relative) = candidate.strip_prefix(&root)
            && relative.components().count() == 2
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn validate_cleanup_worktree(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    let text = path.to_string_lossy();
    if text.starts_with("//") || text.starts_with("\\\\") {
        return Err(Error::Refused("network worktrees cannot be inspected"));
    }
    let real = dunce::canonicalize(path).map_err(|_| Error::Io)?;
    let parent =
        dunce::canonicalize(path.parent().ok_or(Error::Invalid)?).map_err(|_| Error::Io)?;
    if real != parent.join(path.file_name().ok_or(Error::Invalid)?) {
        return Err(Error::Refused("worktree is a symbolic link"));
    }
    if !roots
        .iter()
        .filter_map(|r| dunce::canonicalize(r).ok())
        .any(|r| r.parent().is_some() && real != r && real.starts_with(r))
    {
        return Err(Error::Refused("outside the managed worktree folder"));
    }
    if !fs::metadata(path.join(".git"))
        .map_err(|_| Error::Io)?
        .is_file()
    {
        return Err(Error::Refused("not a linked worktree"));
    }
    Ok(real)
}
