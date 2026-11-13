use std::collections::BTreeSet;
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    keybindings::ScriptRunCommandPattern,
    project::{ProjectScript, ProjectScriptIcon},
};

pub const MAX_SCRIPT_ID_LENGTH: usize = 24;
#[derive(Debug, Clone)]
pub struct ProjectScriptInput {
    pub name: TrimmedNonEmptyString,
    pub command: TrimmedNonEmptyString,
    pub icon: ProjectScriptIcon,
    pub run_on_worktree_create: bool,
    pub wait_for_setup: bool,
    pub run_on_settle: bool,
    pub stop_on_settle: bool,
    pub preview_url: Option<TrimmedNonEmptyString>,
    pub auto_open_preview: bool,
}
pub fn build_project_script(id: TrimmedNonEmptyString, input: ProjectScriptInput) -> ProjectScript {
    ProjectScript {
        id,
        name: input.name,
        command: input.command,
        icon: input.icon,
        run_on_worktree_create: input.run_on_worktree_create,
        r#async: (input.run_on_worktree_create && input.wait_for_setup).then_some(Some(false)),
        run_on_settle: input.run_on_settle.then_some(Some(true)),
        stop_on_settle: input.stop_on_settle.then_some(Some(true)),
        auto_open_preview: input
            .preview_url
            .as_ref()
            .map(|_| Some(input.auto_open_preview)),
        preview_url: input.preview_url.map(Some),
    }
}
pub fn release_claimed_roles(script: &ProjectScript, saved: &ProjectScriptInput) -> ProjectScript {
    let mut next = script.clone();
    if saved.run_on_worktree_create && script.run_on_worktree_create {
        next.run_on_worktree_create = false;
    }
    if saved.run_on_settle && script.run_on_settle == Some(Some(true)) {
        next.run_on_settle = Some(Some(false));
    }
    next
}
pub fn command_for_project_script(id: &str) -> Option<ScriptRunCommandPattern> {
    serde_json::from_value(serde_json::Value::String(format!("script.{id}.run"))).ok()
}
pub fn project_script_id_from_command(command: &str) -> Option<&str> {
    let command = command.trim();
    command_for_project_script(command.strip_prefix("script.")?.strip_suffix(".run")?)?;
    command.strip_prefix("script.")?.strip_suffix(".run")
}
pub fn next_project_script_id<'a>(
    name: &str,
    existing: impl IntoIterator<Item = &'a str>,
    now_epoch_ms: u64,
) -> String {
    let mut base = String::new();
    for ch in name.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            base.push(ch);
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    base.truncate(MAX_SCRIPT_ID_LENGTH);
    base = base.trim_end_matches('-').into();
    if base.is_empty() {
        base = "script".into();
    }
    let taken: BTreeSet<_> = existing.into_iter().collect();
    if !taken.contains(base.as_str()) {
        return base;
    }
    for suffix in 2..10_000 {
        let suffix = format!("-{suffix}");
        let candidate = format!(
            "{}{suffix}",
            &base[..base.len().min(MAX_SCRIPT_ID_LENGTH - suffix.len())]
        );
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    let mut fallback = format!("{base}-{now_epoch_ms}");
    fallback.truncate(MAX_SCRIPT_ID_LENGTH);
    fallback
}
pub fn primary_project_script(scripts: &[ProjectScript]) -> Option<&ProjectScript> {
    scripts
        .iter()
        .find(|s| !s.run_on_worktree_create && s.run_on_settle != Some(Some(true)))
        .or_else(|| scripts.iter().find(|s| s.run_on_settle != Some(Some(true))))
}
pub fn setup_project_script(scripts: &[ProjectScript]) -> Option<&ProjectScript> {
    scripts.iter().find(|s| s.run_on_worktree_create)
}
pub fn settle_project_script(scripts: &[ProjectScript]) -> Option<&ProjectScript> {
    scripts.iter().find(|s| s.run_on_settle == Some(Some(true)))
}
