use crate::thread_views::shelves::SnoozePreset;
use t3_contracts::ipc::ContextMenuItemSchema as Item;

#[derive(Clone, Debug, Default)]
pub struct MenuState {
    pub can_operate: bool,
    pub can_find: bool,
    pub branch: Option<String>,
    pub project_filter: Option<(String, bool)>,
    pub pinned: bool,
    pub settled: bool,
    pub auto_settle: bool,
    pub snoozed: bool,
    pub can_snooze: bool,
    pub regenerating: bool,
    pub running: bool,
    pub settlement: bool,
    pub auto_settle_opt_out: bool,
    pub snooze: bool,
    pub pinning: bool,
    pub title_regeneration: bool,
    pub presets: Vec<SnoozePreset>,
    pub can_share: bool,
    pub can_promote: bool,
}
fn item(id: &str, label: &str, icon: &str) -> Item {
    Item {
        id: id.into(),
        label: label.into(),
        icon: (!icon.is_empty()).then(|| icon.into()),
        detail: None,
        destructive: None,
        disabled: None,
        header: None,
        separator_before: None,
        checked: None,
        children: None,
    }
}
pub fn requires_operate(id: &str) -> bool {
    !matches!(
        id,
        "find"
            | "new-thread-on-branch"
            | "project-settings"
            | "mark-unread"
            | "share-guest"
            | "copy"
            | "copy-path"
            | "copy-branch"
            | "copy-thread"
            | "copy-thread-id"
    )
}
pub fn build(state: &MenuState) -> Vec<Item> {
    let mut items = vec![];
    if state.can_find {
        items.push(item("find", "Find in thread", "text-search"));
    }
    if let Some(branch) = &state.branch {
        items.push(item(
            "new-thread-on-branch",
            &format!("New thread on {branch}"),
            "message-square-plus",
        ));
    }
    if state.pinning {
        items.push(if state.pinned {
            item("unpin", "Unpin thread", "pin-off")
        } else {
            item("pin", "Pin thread", "pin")
        });
    }
    if state.settlement {
        items.push(if state.settled {
            item("unsettle", "Un-settle thread", "circle-check")
        } else {
            item("settle", "Settle thread", "circle-check")
        });
    }
    if state.snooze {
        items.push(if state.snoozed {
            item("unsnooze", "Wake thread", "clock")
        } else {
            let mut row = item("snooze", "Snooze", "clock");
            row.disabled = Some(!state.can_snooze);
            let mut children: Vec<_> = state
                .presets
                .iter()
                .map(|p| {
                    let mut row = item(&format!("snooze:{}", p.id), &p.label, "");
                    row.detail = Some(p.when_label.clone());
                    row
                })
                .collect();
            let mut custom = item("snooze:custom", "Custom…", "");
            custom.separator_before = Some(!children.is_empty());
            children.push(custom);
            row.children = Some(children);
            row
        });
    }
    let mut rename = item("rename", "Rename thread", "pencil");
    rename.separator_before = Some(true);
    items.push(rename);
    if state.title_regeneration {
        let mut row = item(
            "regenerate-title",
            if state.regenerating {
                "Regenerating…"
            } else {
                "Regenerate title"
            },
            "refresh-cw",
        );
        row.disabled = Some(state.regenerating);
        items.push(row);
    }
    items.push(item("mark-unread", "Mark unread", "mail-open"));
    if state.can_share {
        items.push(item("share-guest", "Share with guest", "user-plus"));
    }
    if let Some((label, active)) = &state.project_filter {
        items.push(item(
            "filter-by-project",
            &if *active {
                "Show all projects".into()
            } else {
                format!("Filter by {label}")
            },
            "folder-tree",
        ));
    }
    if state.can_promote {
        let mut row = item("turn-into-project", "Turn into project", "folder-plus");
        row.disabled = Some(state.running);
        items.push(row);
    }
    if state.auto_settle_opt_out {
        let mut row = item("auto-settle", "Auto-settle behavior", "timer");
        let mut enabled = item("auto-settle:enabled", "Follow settings", "");
        enabled.checked = Some(state.auto_settle);
        let mut disabled = item("auto-settle:disabled", "Never", "");
        disabled.checked = Some(!state.auto_settle);
        row.children = Some(vec![enabled, disabled]);
        items.push(row);
    }
    let mut copy = item("copy", "Copy", "copy");
    copy.separator_before = Some(true);
    let mut children = vec![item("copy-path", "Path", "folder")];
    if state.branch.is_some() {
        children.push(item("copy-branch", "Branch", "git-branch"));
    }
    children.extend([
        item("copy-thread", "Thread", "message-square"),
        item("copy-thread-id", "Thread ID", "hash"),
    ]);
    copy.children = Some(children);
    items.push(copy);
    items.push(item("project-settings", "Project settings", "settings"));
    let mut archive = item("archive", "Archive thread", "archive");
    archive.disabled = Some(state.running);
    archive.separator_before = Some(true);
    items.push(archive);
    let mut delete = item("delete", "Delete", "trash");
    delete.destructive = Some(true);
    items.push(delete);
    if !state.can_operate {
        for item in &mut items {
            if requires_operate(&item.id) {
                item.disabled = Some(true);
                if let Some(children) = &mut item.children {
                    for child in children {
                        child.disabled = Some(true);
                    }
                }
            }
        }
    }
    items
}

fn batch_item<'a>(menu: &'a super::actions::ThreadActionMenu, id: &str) -> Option<&'a Item> {
    menu.items.iter().find(|item| item.id == id)
}

pub(super) fn batch_targets(
    menus: &[(crate::types::ThreadKey, super::actions::ThreadActionMenu)],
    id: &str,
) -> Result<Vec<crate::types::ThreadKey>, crate::ClientError> {
    let eligible = menus
        .iter()
        .filter_map(|(key, menu)| {
            let item = batch_item(menu, id)?;
            let disabled = item.disabled == Some(true) || menu.unavailable_reason.is_some();
            if disabled && id == "regenerate-title" && item.label == "Regenerating…" {
                return None;
            }
            Some((key, disabled))
        })
        .collect::<Vec<_>>();
    if (id == "snooze" && eligible.len() != menus.len())
        || eligible.iter().any(|(_, disabled)| *disabled)
    {
        return Err(crate::ClientError::PermissionDenied {
            method: "orchestration.dispatchCommand".into(),
        });
    }
    Ok(eligible.into_iter().map(|(key, _)| key.clone()).collect())
}

pub(super) fn build_batch(
    menus: &[(crate::types::ThreadKey, super::actions::ThreadActionMenu)],
) -> super::actions::ThreadActionMenu {
    let count = menus.len();
    let mut items = Vec::new();
    for (id, label) in [
        ("unpin", "Unpin"),
        ("settle", "Settle"),
        ("snooze", "Snooze"),
        ("regenerate-title", "Regenerate title"),
        ("mark-unread", "Mark unread"),
        ("delete", "Delete"),
    ] {
        let matching: Vec<_> = menus
            .iter()
            .filter_map(|(_, menu)| batch_item(menu, id))
            .collect();
        if matches!(id, "unpin" | "regenerate-title") && matching.is_empty() {
            continue;
        }
        if id == "snooze"
            && (matching.len() != count || matching.iter().any(|item| item.disabled == Some(true)))
        {
            continue;
        }
        let matching_count = if matches!(id, "unpin" | "regenerate-title") {
            matching.len()
        } else {
            count
        };
        let mut row = item(id, &format!("{label} ({matching_count})"), "");
        row.disabled = Some(batch_targets(menus, id).map_or(true, |keys| keys.is_empty()));
        row.destructive = Some(id == "delete");
        if id == "snooze" {
            row.children = matching.first().and_then(|item| item.children.clone());
        }
        items.push(row);
    }
    super::actions::ThreadActionMenu {
        items,
        unavailable_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_both_lifecycle_directions_without_permission() {
        for reversed in [false, true] {
            let items = build(&MenuState {
                pinned: reversed,
                settled: reversed,
                snoozed: reversed,
                auto_settle: true,
                can_snooze: true,
                settlement: true,
                auto_settle_opt_out: true,
                snooze: true,
                pinning: true,
                title_regeneration: true,
                ..Default::default()
            });
            let disabled: Vec<_> = items
                .iter()
                .filter(|i| i.disabled == Some(true))
                .map(|i| i.id.as_str())
                .collect();
            assert_eq!(
                disabled,
                if reversed {
                    vec![
                        "unpin",
                        "unsettle",
                        "unsnooze",
                        "rename",
                        "regenerate-title",
                        "auto-settle",
                        "archive",
                        "delete",
                    ]
                } else {
                    vec![
                        "pin",
                        "settle",
                        "snooze",
                        "rename",
                        "regenerate-title",
                        "auto-settle",
                        "archive",
                        "delete",
                    ]
                }
            );
            assert!(
                items
                    .iter()
                    .find(|i| i.id == "snooze")
                    .and_then(|i| i.children.as_ref())
                    .is_none_or(|children| children.iter().all(|c| c.disabled == Some(true)))
            );
        }
    }
    #[test]
    fn source_running_archive_and_promote_are_disabled() {
        let items = build(&MenuState {
            can_operate: true,
            running: true,
            can_promote: true,
            ..Default::default()
        });
        for id in ["archive", "turn-into-project"] {
            assert!(items.iter().any(|i| i.id == id && i.disabled == Some(true)));
        }
    }
}
