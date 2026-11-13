use super::super::projects::{
    is_windows_drive_path, normalize_project_path_for_comparison,
    normalize_project_path_for_dispatch,
};
use crate::types::ThreadKey;
use crate::view_logic::session::links::{format_file_path_position, split_file_path_position};

pub fn is_windows_platform(platform: &str) -> bool {
    platform.to_ascii_lowercase().starts_with("win")
}
pub fn is_windows_absolute_path(path: &str) -> bool {
    path.starts_with("\\\\") || is_windows_drive_path(path)
}
fn separator(path: &str) -> char {
    if is_windows_absolute_path(path) {
        '\\'
    } else if path.starts_with('/') {
        '/'
    } else if path.contains('\\') {
        '\\'
    } else {
        '/'
    }
}
pub fn has_trailing_path_separator(path: &str) -> bool {
    path.ends_with('/') || (!path.starts_with('/') && path.ends_with('\\'))
}
pub fn is_explicit_relative_project_path(path: &str) -> bool {
    matches!(path, "." | "..")
        || ["./", "../", ".\\", "..\\"]
            .iter()
            .any(|p| path.starts_with(p))
}
fn absolute_parts(path: &str) -> Option<(String, char, Vec<&str>)> {
    if is_windows_drive_path(path) {
        return Some((
            format!("{}\\", &path[..2]),
            '\\',
            path.get(3..)
                .unwrap_or("")
                .split(['/', '\\'])
                .filter(|s| !s.is_empty())
                .collect(),
        ));
    }
    if path.starts_with("\\\\") {
        let mut parts = path.split(['/', '\\']).filter(|s| !s.is_empty());
        return Some((
            format!("\\\\{}\\{}\\", parts.next()?, parts.next()?),
            '\\',
            parts.collect(),
        ));
    }
    path.strip_prefix('/').map(|rest| {
        (
            "/".into(),
            '/',
            rest.split('/').filter(|s| !s.is_empty()).collect(),
        )
    })
}
pub fn is_filesystem_browse_query(path: &str, platform: &str) -> bool {
    ["./", "../", ".\\", "..\\", "/", "~/"]
        .iter()
        .any(|p| path.starts_with(p))
        || (is_windows_platform(platform) && is_windows_absolute_path(path))
}
pub fn is_unsupported_windows_project_path(path: &str, platform: &str) -> bool {
    is_windows_absolute_path(path) && !is_windows_platform(platform)
}
pub fn resolve_project_path_for_dispatch(path: &str, cwd: Option<&str>) -> String {
    let path = path.trim();
    if !is_explicit_relative_project_path(path) {
        return normalize_project_path_for_dispatch(path);
    }
    let base = cwd.map(normalize_project_path_for_dispatch);
    let Some((root, sep, mut parts)) = base.as_deref().and_then(absolute_parts) else {
        return normalize_project_path_for_dispatch(path);
    };
    for segment in path.split(['/', '\\']) {
        match segment {
            "" | "." => (),
            ".." => {
                parts.pop();
            }
            _ => parts.push(segment),
        }
    }
    normalize_project_path_for_dispatch(&format!("{root}{}", parts.join(&sep.to_string())))
}
pub fn infer_project_title_from_path(path: &str) -> String {
    let normalized = normalize_project_path_for_dispatch(path);
    let last = absolute_parts(&normalized)
        .and_then(|(_, _, parts)| parts.last().copied())
        .or_else(|| {
            if absolute_parts(&normalized).is_some() {
                None
            } else {
                normalized.split(['/', '\\']).rfind(|s| !s.is_empty())
            }
        });
    last.unwrap_or(&normalized).into()
}
fn last_separator(path: &str) -> Option<usize> {
    path.rfind(|c| c == '/' || (!path.starts_with('/') && c == '\\'))
}
pub fn get_browse_leaf_path_segment(path: &str) -> &str {
    &path[last_separator(path).map_or(0, |i| i + 1)..]
}
pub fn get_browse_directory_path(path: &str) -> &str {
    if has_trailing_path_separator(path) {
        path
    } else {
        last_separator(path).map_or(path, |i| &path[..i + 1])
    }
}
pub fn ensure_browse_directory_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || has_trailing_path_separator(path) {
        path.into()
    } else {
        format!("{path}{}", separator(path))
    }
}
pub fn append_browse_path_segment(path: &str, segment: &str) -> String {
    format!(
        "{}{segment}{}",
        get_browse_directory_path(path),
        separator(path)
    )
}
pub fn get_browse_parent_path(path: &str) -> Option<String> {
    let trimmed = normalize_project_path_for_dispatch(path);
    if let Some((root, sep, parts)) = absolute_parts(&trimmed) {
        return match parts.len() {
            0 => None,
            1 => Some(root),
            n => Some(format!(
                "{root}{}{sep}",
                parts[..n - 1].join(&sep.to_string())
            )),
        };
    }
    last_separator(&trimmed).map(|i| trimmed[..i + 1].into())
}
pub fn can_navigate_up(path: &str) -> bool {
    has_trailing_path_separator(path) && get_browse_parent_path(path).is_some()
}
pub fn find_project_by_path<'a, T>(
    projects: &'a [T],
    candidate: &str,
    path: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    let key = normalize_project_path_for_comparison(candidate);
    if key.is_empty() {
        return None;
    }
    projects
        .iter()
        .find(|p| normalize_project_path_for_comparison(path(p)) == key)
}

pub fn get_orphaned_worktree_path_for_thread<'a>(
    threads: &'a [(ThreadKey, Option<String>)],
    target: &ThreadKey,
) -> Option<&'a str> {
    let path = threads
        .iter()
        .find(|(key, _)| key == target)?
        .1
        .as_deref()?
        .trim();
    if path.is_empty() {
        return None;
    }
    let normalized = normalize_project_path_for_comparison(path);
    (!threads.iter().any(|(key, other)| {
        key.environment == target.environment
            && key != target
            && other
                .as_deref()
                .is_some_and(|other| normalize_project_path_for_comparison(other) == normalized)
    }))
    .then_some(path)
}
pub fn format_worktree_path_for_display(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return path.into();
    }
    let normalized = trimmed.replace('\\', "/");
    normalized
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .into()
}
fn relative_segments(path: &str) -> Option<Vec<&str>> {
    if path.starts_with('/')
        || is_windows_absolute_path(path)
        || path.as_bytes().get(1) == Some(&b':')
    {
        return None;
    }
    let parts: Vec<_> = path
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    (!parts.is_empty() && !parts.contains(&"..")).then_some(parts)
}
pub fn resolve_diff_path_for_workspace(
    path: &str,
    workspace: Option<&str>,
    repository: Option<&str>,
) -> Option<String> {
    if path.starts_with('/') || is_windows_absolute_path(path) {
        let root = workspace?;
        let windows =
            is_windows_absolute_path(root) || (root.starts_with("//") && path.starts_with("\\\\"));
        let normalize = |p: &str| {
            if windows {
                strip_drive_slash(&p.replace('\\', "/")).to_string()
            } else {
                p.to_string()
            }
        };
        let root = normalize(root).trim_end_matches('/').to_string();
        let path = normalize(path);
        let (compare, base) = if windows {
            (path.to_lowercase(), root.to_lowercase())
        } else {
            (path.clone(), root.clone())
        };
        if !compare.starts_with(&format!("{base}/")) {
            return None;
        }
        return relative_segments(&path[root.len() + 1..]).map(|parts| parts.join("/"));
    }
    let parts = relative_segments(path)?;
    let (Some(workspace), Some(repository)) = (workspace, repository) else {
        return Some(parts.join("/"));
    };
    let workspace = normalize_project_path_for_comparison(workspace);
    let repo = normalize_project_path_for_comparison(repository);
    if workspace == repo {
        return Some(parts.join("/"));
    }
    let sep = if repo.contains('\\') { '\\' } else { '/' };
    let prefix = if repo.ends_with(sep) {
        repo.clone()
    } else {
        format!("{repo}{sep}")
    };
    let Some(suffix) = workspace.strip_prefix(&prefix) else {
        return Some(parts.join("/"));
    };
    let root_parts: Vec<_> = suffix
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    if !root_parts.iter().zip(&parts).all(|(a, b)| {
        if is_windows_absolute_path(repository) {
            a == &b.to_lowercase()
        } else {
            a == b
        }
    }) || parts.len() <= root_parts.len()
    {
        return None;
    }
    Some(parts[root_parts.len()..].join("/"))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffFileAction {
    ThreadFile { thread: ThreadKey, path: String },
    Editor(String),
}
pub fn open_diff_file_primary_action(
    thread: Option<&ThreadKey>,
    path: &str,
    cwd: Option<&str>,
    repository: Option<&str>,
) -> Option<DiffFileAction> {
    let path = resolve_diff_path_for_workspace(path, cwd, repository)?;
    Some(match thread {
        Some(thread) => DiffFileAction::ThreadFile {
            thread: thread.clone(),
            path,
        },
        None => DiffFileAction::Editor(cwd.map_or_else(
            || path.clone(),
            |cwd| crate::services::filesystem::workspace_file_path(cwd, &path),
        )),
    })
}
fn strip_drive_slash(path: &str) -> &str {
    if path.starts_with('/') && path.get(1..).is_some_and(is_windows_drive_path) {
        &path[1..]
    } else {
        path
    }
}
pub fn format_workspace_relative_path(path: &str, workspace: Option<&str>) -> String {
    let mut position = split_file_path_position(path, "");
    let normalized = position.path.replace('\\', "/");
    let normalized = strip_drive_slash(&normalized);
    let mut display = normalized.to_string();
    if let Some(workspace) = workspace.filter(|s| !s.is_empty()) {
        let root = workspace.trim_end_matches(['/', '\\']).replace('\\', "/");
        let root = strip_drive_slash(&root);
        let label = root.rsplit('/').next().unwrap_or("");
        let windows = is_windows_absolute_path(strip_drive_slash(workspace));
        let (compare, base, label_compare) = if windows {
            (
                normalized.to_lowercase(),
                root.to_lowercase(),
                label.to_lowercase(),
            )
        } else {
            (normalized.into(), root.into(), label.into())
        };
        if compare == base {
            display = label.into();
        } else if compare.starts_with(&format!("{base}/")) {
            display = format!("{label}/{}", &normalized[root.len() + 1..]);
        } else if !normalized.starts_with('/') && !compare.starts_with(&format!("{label_compare}/"))
        {
            let relative = if normalized.starts_with("./") {
                normalized.trim_start_matches('.').trim_start_matches('/')
            } else {
                normalized.trim_start_matches('/')
            };
            display = format!("{label}/{relative}");
        }
    }
    position.path = display;
    format_file_path_position(&position)
}
