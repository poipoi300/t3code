use t3_contracts::git::{GitStackedAction, VcsStatusChangeRequestState, VcsStatusResult};

pub struct ChangeRequestTerminology<'a> {
    pub singular: &'a str,
    pub short_label: &'a str,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickActionKind {
    Run(GitStackedAction),
    Pull,
    Publish,
    Hint(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickAction {
    pub label: String,
    pub disabled: bool,
    pub kind: QuickActionKind,
}
fn hint(label: &str, hint: &str) -> QuickAction {
    QuickAction {
        label: label.into(),
        disabled: true,
        kind: QuickActionKind::Hint(hint.into()),
    }
}
fn run(label: impl Into<String>, action: GitStackedAction) -> QuickAction {
    QuickAction {
        label: label.into(),
        disabled: false,
        kind: QuickActionKind::Run(action),
    }
}
fn open_pr(status: &VcsStatusResult) -> bool {
    status
        .pr
        .as_ref()
        .is_some_and(|pr| pr.state == VcsStatusChangeRequestState::Open)
}
pub fn quick_action(
    status: Option<&VcsStatusResult>,
    busy: bool,
    default_ref: bool,
    primary_remote: bool,
    terminology: &ChangeRequestTerminology<'_>,
) -> QuickAction {
    if busy {
        return hint("Commit", "Git action in progress.");
    }
    let Some(s) = status else {
        return hint("Commit", "Git status is unavailable.");
    };
    if s.ref_name.is_none() {
        return hint(
            "Commit",
            &format!(
                "Create and checkout a ref before pushing or opening a {}.",
                terminology.singular
            ),
        );
    }
    let has_pr = open_pr(s);
    let ahead = s.ahead_count > 0;
    let behind = s.behind_count > 0;
    let default_delta = s.ahead_of_default_count.flatten().unwrap_or(s.ahead_count) > 0;
    if s.has_working_tree_changes {
        if behind || (!s.has_upstream && !primary_remote) {
            return run("Commit", GitStackedAction::Commit);
        }
        if has_pr || default_ref {
            return run("Commit & push", GitStackedAction::CommitPush);
        }
        return run(
            format!("Commit, push & {}", terminology.short_label),
            GitStackedAction::CommitPushPr,
        );
    }
    if !s.has_upstream {
        if !primary_remote {
            return QuickAction {
                label: "Publish repository".into(),
                disabled: false,
                kind: QuickActionKind::Publish,
            };
        }
        if !ahead {
            return if has_pr {
                hint("Commit", "Branch is up to date. No action needed.")
            } else {
                hint("Push", "No local commits to push.")
            };
        }
        if has_pr || default_ref {
            return run(
                "Push",
                if default_ref {
                    GitStackedAction::CommitPush
                } else {
                    GitStackedAction::Push
                },
            );
        }
        return run(
            format!("Push & create {}", terminology.short_label),
            GitStackedAction::CreatePr,
        );
    }
    if ahead && behind {
        return hint(
            "Sync ref",
            "Branch has diverged from upstream. Rebase/merge first.",
        );
    }
    if behind {
        return QuickAction {
            label: "Pull".into(),
            disabled: false,
            kind: QuickActionKind::Pull,
        };
    }
    if ahead {
        if has_pr || default_ref {
            return run(
                "Push",
                if default_ref {
                    GitStackedAction::CommitPush
                } else {
                    GitStackedAction::Push
                },
            );
        }
        return run(
            format!("Push & create {}", terminology.short_label),
            GitStackedAction::CreatePr,
        );
    }
    if has_pr {
        return hint("Commit", "Branch is up to date. No action needed.");
    }
    if default_delta && !default_ref {
        return run(
            format!("Create {}", terminology.short_label),
            GitStackedAction::CreatePr,
        );
    }
    hint("Commit", "Branch is up to date. No action needed.")
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAction {
    Commit,
    Push,
    CreatePr,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub action: DialogAction,
    pub label: String,
    pub disabled: bool,
}
pub fn menu_items(
    status: Option<&VcsStatusResult>,
    busy: bool,
    primary_remote: bool,
    terminology: &ChangeRequestTerminology<'_>,
) -> Vec<MenuItem> {
    let Some(s) = status else {
        return Vec::new();
    };
    let has_branch = s.ref_name.is_some();
    let push_remote = s.has_upstream || primary_remote;
    let mut items = vec![MenuItem {
        action: DialogAction::Commit,
        label: "Commit".into(),
        disabled: busy || !s.has_working_tree_changes,
    }];
    if !primary_remote {
        return items;
    }
    items.push(MenuItem {
        action: DialogAction::Push,
        label: "Push".into(),
        disabled: busy || !has_branch || s.behind_count > 0 || s.ahead_count == 0 || !push_remote,
    });
    if !open_pr(s) {
        items.push(MenuItem {
            action: DialogAction::CreatePr,
            label: format!("Create {}", terminology.short_label),
            disabled: busy
                || !has_branch
                || s.ahead_of_default_count.flatten().unwrap_or(s.ahead_count) == 0
                || s.behind_count > 0
                || !push_remote,
        });
    }
    items
}
pub fn usable_menu(items: &[MenuItem], can_write: bool, can_publish: bool, notice: bool) -> bool {
    can_publish || notice || (can_write && items.iter().any(|i| !i.disabled))
}
pub fn requires_default_branch_confirmation(action: &GitStackedAction, default: bool) -> bool {
    default
        && matches!(
            action,
            GitStackedAction::Push
                | GitStackedAction::CreatePr
                | GitStackedAction::CommitPush
                | GitStackedAction::CommitPushPr
        )
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultBranchDialogCopy {
    pub title: String,
    pub description: String,
    pub continue_label: String,
}
pub fn default_branch_dialog_copy(
    action: &GitStackedAction,
    branch: &str,
    includes_commit: bool,
    terminology: &ChangeRequestTerminology<'_>,
) -> DefaultBranchDialogCopy {
    let suffix = format!(
        " on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there."
    );
    if matches!(
        action,
        GitStackedAction::Push | GitStackedAction::CommitPush
    ) {
        if includes_commit {
            return DefaultBranchDialogCopy {
                title: "Commit & push to default ref?".into(),
                description: format!("This action will commit and push changes{suffix}"),
                continue_label: format!("Commit & push to {branch}"),
            };
        }
        return DefaultBranchDialogCopy {
            title: "Push to default ref?".into(),
            description: format!("This action will push local commits{suffix}"),
            continue_label: format!("Push to {branch}"),
        };
    }
    let short = terminology.short_label;
    let singular = terminology.singular;
    if includes_commit {
        DefaultBranchDialogCopy {
            title: format!("Commit, push & create {short} from default ref?"),
            description: format!("This action will commit, push, and create a {singular}{suffix}"),
            continue_label: format!("Commit, push & create {short}"),
        }
    } else {
        DefaultBranchDialogCopy {
            title: format!("Push & create {short} from default ref?"),
            description: format!(
                "This action will push local commits and create a {singular}{suffix}"
            ),
            continue_label: format!("Push & create {short}"),
        }
    }
}
pub fn feature_branch_unavailable_reason(branch: &str, dirty: bool) -> Option<String> {
    (!dirty).then(||format!("There are no changes to commit, so a feature branch can't be named. Create a branch yourself or continue on {branch}."))
}
pub const SUCCESS_VISIBLE_MS: u64 = 10_000;
pub fn result_dismiss_after_visible_ms(success: bool) -> Option<u64> {
    success.then_some(SUCCESS_VISIBLE_MS)
}
pub struct ProgressInput<'a> {
    pub running: bool,
    pub operation: Option<&'a str>,
    pub label: Option<&'a str>,
    pub output: Option<&'a str>,
    pub phase_started: Option<u64>,
    pub hook_started: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressPresentation {
    pub status: String,
    pub output: Option<String>,
    pub started_at: Option<u64>,
}
pub fn progress_presentation(input: ProgressInput<'_>) -> Option<ProgressPresentation> {
    if !input.running || !matches!(input.operation, Some("pull" | "run_change_request")) {
        return None;
    }
    let pull = input.operation == Some("pull");
    let status = input
        .label
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "Running source control action")
        .unwrap_or(if pull {
            "Pulling latest changes..."
        } else {
            "Starting source control action..."
        });
    Some(ProgressPresentation {
        status: status.into(),
        output: if pull {
            None
        } else {
            input
                .output
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        },
        started_at: if pull {
            input.phase_started
        } else {
            input.hook_started.or(input.phase_started)
        },
    })
}
pub fn format_elapsed(start: Option<u64>, now: u64) -> Option<String> {
    let seconds = now.saturating_sub(start?) / 1000;
    Some(if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    })
}
pub fn is_temporary_worktree_branch(branch: &str) -> bool {
    let value = branch.trim().to_lowercase();
    let short = value
        .strip_prefix("t3/")
        .or_else(|| value.strip_prefix("t3-"))
        .or_else(|| value.strip_prefix("t3code/"))
        .or_else(|| value.strip_prefix("t3code-"));
    if short.is_some_and(|token| token.len() == 8 && token.bytes().all(|b| b.is_ascii_hexdigit())) {
        return true;
    }
    let Some(uuid) = value.strip_prefix("t3code/") else {
        return false;
    };
    let bytes = uuid.as_bytes();
    bytes.len() == 36
        && bytes[14] == b'4'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
        && bytes.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
pub fn live_thread_branch_update(
    thread: Option<&str>,
    status: Option<&VcsStatusResult>,
) -> Option<Option<String>> {
    let status = status?;
    let branch = status.ref_name.as_ref().map(|v| v.0.as_str());
    if (branch.is_none() && thread.is_some()) || thread == branch {
        return None;
    }
    if let (Some(thread), Some(branch)) = (thread, branch)
        && !is_temporary_worktree_branch(thread)
        && is_temporary_worktree_branch(branch)
    {
        return None;
    }
    Some(branch.map(str::to_string))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesTotals {
    pub insertions: u64,
    pub deletions: u64,
    pub base_ref: Option<String>,
}
pub fn changes_totals(
    status: Option<&VcsStatusResult>,
    preview: Option<ChangesTotals>,
) -> Option<ChangesTotals> {
    if preview.is_some() {
        return preview;
    }
    let status = status?;
    let branch = status.branch_changes.as_ref().and_then(Option::as_ref);
    let tree = &status.working_tree;
    let selected = branch.filter(|b| b.insertions > 0 || b.deletions > 0).or(
        if tree.insertions > 0 || tree.deletions > 0 {
            None
        } else {
            branch
        },
    );
    Some(match selected {
        Some(b) => ChangesTotals {
            insertions: b.insertions,
            deletions: b.deletions,
            base_ref: b.base_ref.as_ref().map(|r| r.0.clone()),
        },
        None => ChangesTotals {
            insertions: tree.insertions,
            deletions: tree.deletions,
            base_ref: None,
        },
    })
}
pub fn thread_branch_update(
    result: &t3_contracts::git::GitRunStackedActionResult,
) -> Option<String> {
    use t3_contracts::git::GitBranchStepStatus;
    (result.branch.status == GitBranchStepStatus::Created)
        .then(|| {
            result
                .branch
                .name
                .as_ref()
                .and_then(Option::as_ref)
                .map(|n| n.0.clone())
        })
        .flatten()
        .filter(|n| !n.is_empty())
}
