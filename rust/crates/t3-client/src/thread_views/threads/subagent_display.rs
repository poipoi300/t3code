use t3_contracts::{
    model_selection::ModelSelection, orchestration_project::OrchestrationProjectShell,
    orchestration_v2::*, server::ServerProvider,
};

pub fn terminal_status(status: &OrchestrationV2TurnItemStatus) -> bool {
    matches!(
        status,
        OrchestrationV2TurnItemStatus::Completed
            | OrchestrationV2TurnItemStatus::Failed
            | OrchestrationV2TurnItemStatus::Cancelled
            | OrchestrationV2TurnItemStatus::Interrupted
    )
}
pub struct GroupSummary {
    pub label: String,
    pub active: bool,
    pub failed: bool,
}
pub fn subagent_group_summary(statuses: &[OrchestrationV2TurnItemStatus]) -> GroupSummary {
    let active = statuses.iter().any(super::execution::work_active);
    GroupSummary {
        label: format!(
            "{} {} subagent{}",
            if active { "Kicked off" } else { "Ran" },
            statuses.len(),
            if statuses.len() == 1 { "" } else { "s" }
        ),
        active,
        failed: statuses.contains(&OrchestrationV2TurnItemStatus::Failed),
    }
}
pub fn summarize_subagent_statuses(statuses: &[OrchestrationV2TurnItemStatus]) -> String {
    let mut counts = [0; 5];
    for s in statuses {
        counts[match s {
            OrchestrationV2TurnItemStatus::Pending
            | OrchestrationV2TurnItemStatus::Running
            | OrchestrationV2TurnItemStatus::Waiting => 0,
            OrchestrationV2TurnItemStatus::Completed => 1,
            OrchestrationV2TurnItemStatus::Failed => 2,
            OrchestrationV2TurnItemStatus::Idle => 4,
            _ => 3,
        }] += 1;
    }
    counts
        .into_iter()
        .zip(["working", "done", "failed", "stopped", "idle"])
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect::<Vec<_>>()
        .join(" · ")
}
pub fn format_subagent_display_title(title: &str) -> String {
    let title = if title
        .get(..9)
        .is_some_and(|s| s.eq_ignore_ascii_case("Subagent:"))
    {
        title[9..].trim_start_matches(crate::view_logic::is_js_whitespace)
    } else {
        title
    };
    let Some(path) = title.strip_prefix("/root/") else {
        return title.into();
    };
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.is_empty() || path.split('/').any(str::is_empty) {
        return title.into();
    }
    let name = path.rsplit('/').next().unwrap_or("");
    let mut output = String::new();
    let mut boundary = true;
    for ch in name.chars() {
        if ch == '_' || ch.is_whitespace() {
            if !boundary {
                output.push(' ');
            }
            boundary = true;
        } else {
            if boundary {
                output.extend(ch.to_uppercase());
            } else {
                output.push(ch);
            }
            boundary = false;
        }
    }
    if output.trim().is_empty() {
        title.into()
    } else {
        output.trim_end().into()
    }
}

pub fn subagent_detail_preview(
    status: &OrchestrationV2TurnItemStatus,
    result: Option<&str>,
    progress: Option<&str>,
) -> Option<String> {
    let (preferred, fallback) = if terminal_status(status) {
        (result, progress)
    } else {
        (progress, result)
    };
    let detail = preferred
        .map(t3_contracts::wire::trim_js)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            fallback
                .map(t3_contracts::wire::trim_js)
                .filter(|s| !s.is_empty())
        })?;
    let mut compact = String::new();
    let mut units = 0;
    let mut gap = false;
    for ch in detail.chars() {
        if crate::view_logic::is_js_whitespace(ch) {
            gap = !compact.is_empty();
            continue;
        }
        if gap {
            compact.push(' ');
            units += 1;
            gap = false;
        }
        if units + ch.len_utf16() > 280 {
            let trimmed = compact.trim_end().len();
            compact.truncate(trimmed);
            compact.push('…');
            return Some(compact);
        }
        compact.push(ch);
        units += ch.len_utf16();
    }
    Some(compact)
}

pub struct MetadataInput<'a> {
    pub model: Option<&'a str>,
    pub selection: Option<&'a ModelSelection>,
    pub provider: Option<&'a ServerProvider>,
    pub parent_thread: Option<&'a OrchestrationV2ThreadShell>,
    pub child_thread: Option<&'a OrchestrationV2ThreadShell>,
    pub parent_project: Option<&'a OrchestrationProjectShell>,
    pub child_project: Option<&'a OrchestrationProjectShell>,
}
pub struct SubagentMetadata {
    pub model_label: String,
    pub model_slug: Option<String>,
    pub workspace: Vec<(&'static str, String)>,
}
pub fn resolve_subagent_metadata(
    input: MetadataInput<'_>,
    format_slug: impl Fn(&str) -> String,
) -> SubagentMetadata {
    let model = input
        .model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| input.selection.map(|s| s.model.trim()));
    let catalog = input.provider.and_then(|p| {
        crate::view_logic::chat_view::resolve_selectable_model(&p.driver.0, model, &p.models)
    });
    let slug = catalog.map(|m| m.slug.0.as_str()).or(model);
    let reported = catalog
        .map(|m| {
            m.short_name
                .as_ref()
                .and_then(Option::as_ref)
                .unwrap_or(&m.name)
                .0
                .clone()
        })
        .unwrap_or_else(|| {
            model
                .map(&format_slug)
                .unwrap_or_else(|| "Not reported".into())
        });
    let qualifier = catalog
        .and_then(|m| m.sub_provider.as_ref()?.as_ref())
        .map(|q| q.0.trim())
        .filter(|s| !s.is_empty());
    let label = qualifier
        .and_then(|q| {
            reported
                .get(..q.len())
                .filter(|prefix| prefix.eq_ignore_ascii_case(q))
                .and_then(|_| reported.get(q.len()..))
        })
        .and_then(|suffix| {
            let trimmed = suffix.trim_start();
            if suffix.starts_with(char::is_whitespace) || trimmed.starts_with(['.', ':', '/', '-'])
            {
                Some(
                    trimmed
                        .trim_start_matches(['.', ':', '/', '-'])
                        .trim_start(),
                )
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or(&reported)
        .to_owned();
    let parent_workspace = input
        .parent_thread
        .and_then(|t| t.worktree_path.as_ref())
        .map(|s| s.0.as_str())
        .or_else(|| input.parent_project.map(|p| p.workspace_root.0.as_str()));
    let child_workspace = input
        .child_thread
        .and_then(|t| t.worktree_path.as_ref())
        .map(|s| s.0.as_str())
        .or_else(|| input.child_project.map(|p| p.workspace_root.0.as_str()));
    let mut workspace = vec![];
    if let (Some(parent), Some(child)) = (input.parent_thread, input.child_project)
        && parent.project_id != child.id
    {
        workspace.push(("Project", child.title.0.clone()));
    }
    if let (Some(parent), Some(child)) = (parent_workspace, child_workspace)
        && parent != child
    {
        let branch = input.child_thread.and_then(|t| t.branch.as_ref());
        let label = if branch.is_some() {
            "Branch"
        } else if input
            .child_thread
            .is_some_and(|t| t.worktree_path.is_some())
        {
            "Worktree"
        } else {
            "Workspace"
        };
        workspace.push((
            label,
            branch.map(|b| b.0.clone()).unwrap_or_else(|| {
                child
                    .trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(child)
                    .into()
            }),
        ));
    }
    SubagentMetadata {
        model_label: label,
        model_slug: slug.map(str::to_owned),
        workspace,
    }
}
