use super::projects::normalize_project_path_for_comparison;
use crate::time::epoch_ms as timestamp;
use t3_contracts::git::VcsRef;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EnvMode {
    #[default]
    Local,
    Worktree,
}
pub fn environment_option_label(
    primary: bool,
    environment: &str,
    runtime: Option<&str>,
    saved: Option<&str>,
) -> String {
    let values: Vec<_> = [runtime, saved]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();
    if primary {
        values
            .into_iter()
            .find(|v| !matches!(v.to_lowercase().as_str(), "local" | "local environment"))
            .unwrap_or("This device")
            .into()
    } else {
        values.first().copied().unwrap_or(environment).into()
    }
}
pub fn show_environment_indicator(active_primary: Option<bool>, can_pick: bool) -> bool {
    can_pick || active_primary == Some(false)
}
pub fn show_context_strip(
    draft: bool,
    persist: bool,
    has_project: bool,
    git_repo: bool,
    environment_indicator: bool,
) -> bool {
    has_project && (draft || (persist && (git_repo || environment_indicator)))
}
pub fn context_labels_compact(compact: bool, needed_width: f64, available_width: f64) -> bool {
    needed_width > available_width - if compact { 16.0 } else { 0.0 }
}
pub fn env_mode_label(mode: EnvMode) -> &'static str {
    if mode == EnvMode::Worktree {
        "New worktree"
    } else {
        "Current checkout"
    }
}
pub fn worktree_submodules_label(
    mode: &t3_contracts::environment::WorktreeSubmodules,
) -> &'static str {
    use t3_contracts::environment::WorktreeSubmodules;
    match mode {
        WorktreeSubmodules::Recursive => "Recursive",
        WorktreeSubmodules::TopLevel => "Top level only",
        WorktreeSubmodules::None => "Skip",
    }
}
pub fn current_workspace_label(worktree: Option<&str>) -> &'static str {
    if worktree.is_some_and(|v| !v.is_empty()) {
        "Current worktree"
    } else {
        env_mode_label(EnvMode::Local)
    }
}
pub fn locked_workspace_label(worktree: Option<&str>, mode: EnvMode) -> String {
    if let Some(path) = worktree.filter(|v| !v.is_empty()) {
        return path
            .split(['/', '\\'])
            .rfind(|v| !v.is_empty())
            .unwrap_or("Worktree")
            .into();
    }
    if mode == EnvMode::Worktree {
        env_mode_label(mode)
    } else {
        "Local checkout"
    }
    .into()
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousWorktree {
    pub branch: Option<String>,
    pub path: String,
}
pub struct WorktreeThread<'a> {
    pub branch: Option<&'a str>,
    pub path: Option<&'a str>,
    pub updated_at: &'a str,
    pub archived: bool,
}
pub fn previous_worktree_seed(
    threads: &[WorktreeThread<'_>],
    current: Option<&str>,
) -> Option<PreviousWorktree> {
    let mut latest: Option<(i64, PreviousWorktree)> = None;
    for thread in threads {
        let Some(path) = thread.path.filter(|v| !v.is_empty()) else {
            continue;
        };
        if thread.archived || Some(path) == current {
            continue;
        }
        let Some(time) = timestamp(thread.updated_at) else {
            continue;
        };
        if latest.as_ref().is_none_or(|(previous, _)| time > *previous) {
            latest = Some((
                time,
                PreviousWorktree {
                    branch: thread.branch.map(str::to_string),
                    path: path.into(),
                },
            ));
        }
    }
    latest.map(|(_, seed)| seed)
}
pub fn previous_worktree_label(seed: &PreviousWorktree) -> String {
    match seed.branch.as_deref().filter(|b| !b.is_empty()) {
        Some(b) => format!("Previous worktree ({b})"),
        None => "Previous worktree".into(),
    }
}
pub fn effective_env_mode(
    worktree: Option<&str>,
    has_server_thread: bool,
    draft_mode: Option<EnvMode>,
    preparing: bool,
) -> EnvMode {
    let has_path = worktree.is_some_and(|p| !p.is_empty());
    if !has_server_thread {
        if has_path {
            EnvMode::Local
        } else {
            draft_mode.unwrap_or_default()
        }
    } else if has_path || preparing {
        EnvMode::Worktree
    } else {
        EnvMode::Local
    }
}
pub fn draft_env_mode_after_branch_change(
    next: Option<&str>,
    current: Option<&str>,
    effective: EnvMode,
) -> EnvMode {
    if next.is_some_and(|p| !p.is_empty())
        || (effective == EnvMode::Worktree && !current.is_some_and(|p| !p.is_empty()))
    {
        EnvMode::Worktree
    } else {
        EnvMode::Local
    }
}
pub fn branch_toolbar_value<'a>(
    mode: EnvMode,
    worktree: Option<&str>,
    thread_branch: Option<&'a str>,
    git_branch: Option<&'a str>,
) -> Option<&'a str> {
    if mode == EnvMode::Worktree && !worktree.is_some_and(|p| !p.is_empty()) {
        thread_branch.or(git_branch)
    } else {
        git_branch.or(thread_branch)
    }
}
pub fn branch_trigger_label(
    worktree: Option<&str>,
    mode: EnvMode,
    branch: Option<&str>,
    remote: Option<bool>,
    start_from_origin: bool,
) -> String {
    let Some(branch) = branch.filter(|b| !b.is_empty()) else {
        return "Select ref".into();
    };
    if mode == EnvMode::Worktree && !worktree.is_some_and(|p| !p.is_empty()) {
        format!(
            "From {branch}{}",
            if start_from_origin && remote == Some(false) {
                " (remote)"
            } else {
                ""
            }
        )
    } else {
        branch.into()
    }
}
pub fn toolbar_pr_branch<'a>(
    thread_branch: Option<&'a str>,
    active_branch: Option<&str>,
) -> Option<&'a str> {
    thread_branch.filter(|b| Some(*b) == active_branch)
}
pub fn local_checkout_branch_mismatch<'a>(
    mode: EnvMode,
    worktree: Option<&str>,
    thread: Option<&'a str>,
    git: Option<&'a str>,
) -> Option<(&'a str, &'a str)> {
    if mode != EnvMode::Local || worktree.is_some() {
        return None;
    }
    match (thread, git) {
        (Some(t), Some(g)) if !t.is_empty() && !g.is_empty() && t != g => Some((t, g)),
        _ => None,
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSelectionTarget {
    pub checkout_cwd: String,
    pub next_worktree_path: Option<String>,
    pub reuse_existing_worktree: bool,
    pub stays_in_active_checkout: bool,
}
pub fn default_worktree_base_branch(refs: &[VcsRef]) -> Option<&str> {
    refs.iter()
        .find(|reference| reference.is_default)
        .or_else(|| refs.iter().find(|reference| reference.current))
        .map(|reference| reference.name.0.as_str())
}
fn inside(path: &str, root: &str) -> bool {
    let windows = super::projects::is_windows_drive_path(root) || root.starts_with("\\\\");
    let path = normalize_project_path_for_comparison(path);
    let root = normalize_project_path_for_comparison(root);
    let (path, root) = if windows {
        (path.replace('\\', "/"), root.replace('\\', "/"))
    } else {
        (path, root)
    };
    if path == root {
        return true;
    }
    let Some(relative) = path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))) else {
        return false;
    };
    let mut depth = 0;
    for part in relative.split('/') {
        match part {
            ".." => depth -= 1,
            "" | "." => {}
            _ => depth += 1,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}
pub fn branch_selection_target(
    project_cwd: &str,
    project_checkout_root: Option<&str>,
    active_worktree: Option<&str>,
    reference: &VcsRef,
) -> BranchSelectionTarget {
    if let Some(path) = &reference.worktree_path {
        let project_checkout = match project_checkout_root {
            None => inside(project_cwd, &path.0),
            Some(root) => {
                normalize_project_path_for_comparison(root)
                    == normalize_project_path_for_comparison(&path.0)
            }
        };
        return BranchSelectionTarget {
            checkout_cwd: path.0.clone(),
            next_worktree_path: (!project_checkout).then(|| path.0.clone()),
            reuse_existing_worktree: true,
            stays_in_active_checkout: if project_checkout {
                active_worktree.is_none()
            } else {
                active_worktree == Some(path.0.as_str())
            },
        };
    }
    let next = if active_worktree.is_some() && reference.is_default {
        None
    } else {
        active_worktree
    };
    BranchSelectionTarget {
        checkout_cwd: next.unwrap_or(project_cwd).into(),
        next_worktree_path: next.map(str::to_string),
        reuse_existing_worktree: false,
        stays_in_active_checkout: next == active_worktree,
    }
}
pub fn worktree_change_blocked(
    interruptible_running: bool,
    current: Option<&str>,
    next: Option<&str>,
) -> bool {
    current != next && interruptible_running
}
pub fn sanitize_new_ref_name(value: &str) -> String {
    let mut result = String::new();
    let mut whitespace = false;
    for ch in value.trim().chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b') {
            if !whitespace {
                result.push('-');
            }
            whitespace = true;
        } else {
            result.push(ch);
            whitespace = false;
        }
    }
    result
}
pub fn include_branch_picker_item(
    value: &str,
    query: &str,
    create: Option<&str>,
    checkout_pr: Option<&str>,
) -> bool {
    if query.is_empty() || create == Some(value) || checkout_pr == Some(value) {
        return true;
    }
    let lower = value.to_lowercase();
    let sanitized = sanitize_new_ref_name(query);
    lower.contains(query)
        || (!sanitized.is_empty() && sanitized != query && lower.contains(&sanitized))
}
pub fn local_branch_from_remote(value: &str) -> &str {
    value
        .find('/')
        .filter(|i| *i > 0 && *i < value.len() - 1)
        .map_or(value, |i| &value[i + 1..])
}
pub fn dedupe_remote_refs_with_local_matches(refs: &[VcsRef]) -> Vec<&VcsRef> {
    let local: std::collections::BTreeSet<_> = refs
        .iter()
        .filter(|r| r.is_remote != Some(Some(true)))
        .map(|r| r.name.0.as_str())
        .collect();
    refs.iter()
        .filter(|r| {
            r.is_remote != Some(Some(true))
                || r.remote_name
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|v| v.0.as_str())
                    != Some("origin")
                || {
                    let name = local_branch_from_remote(&r.name.0);
                    name.is_empty() || !local.contains(name)
                }
        })
        .collect()
}
