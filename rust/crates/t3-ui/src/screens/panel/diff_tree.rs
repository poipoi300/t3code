//! Port of apps/web/src/components/diffs/DiffFileTree.tsx DiffFileTree, inside DiffPanel.tsx's
//! FileExplorerPane.
//!
//! The rows are NSH's `Tree` (virtualised, keyboard and screen-reader navigable), standing where
//! the web mounts @pierre/trees; its items are the client's `workspace::diff_panel::tree`.

use nsh_base::{DiffView, DiffViewApi, TreeEntry, TreeItem};
use nsh_core::prelude::*;
use t3_client::workspace::diff_panel::{
    DiffPanelSnapshot,
    tree::{DiffTreeNode, GitStatus, ancestor_ids, directory_ids},
};

use super::diff::DiffPanelRuntime;
use crate::i18n::{Value, t, tr};
use crate::runtime::Shared;
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

fn tree_items(nodes: &[DiffTreeNode]) -> Vec<TreeItem> {
    nodes
        .iter()
        .map(|node| {
            TreeItem::new(node.id.clone(), node.label.clone()).children(tree_items(&node.children))
        })
        .collect()
}

fn find<'a>(nodes: &'a [DiffTreeNode], id: &str) -> Option<&'a DiffTreeNode> {
    nodes.iter().find_map(|node| {
        if node.id == id {
            Some(node)
        } else if node.is_directory() && id.starts_with(&node.id) {
            find(&node.children, id)
        } else {
            None
        }
    })
}

/// `revealDiffFile`: unfold the file and scroll its header to the top.
pub fn reveal_diff_file(
    runtime: &DiffPanelRuntime,
    snapshot: &DiffPanelSnapshot,
    diff: ComponentRef<DiffView>,
    path: &str,
) -> bool {
    let Some((key, _)) = snapshot.shown_files.iter().find(|(_, p)| p == path) else {
        return false;
    };
    runtime.0.unfold_file(snapshot, key);
    if let Some(api) = diff.api::<DiffViewApi>() {
        api.scroll_to_file(path.to_owned());
    }
    true
}

/// FileExplorerPane with DiffFileTree. The pane takes the web's default 40% of the row; its
/// resize handle (`storageKey="t3code:diff-file-tree-width"`) is FileExplorerPane's, not built.
pub fn diff_file_tree_pane(
    runtime: DiffPanelRuntime,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    diff: ComponentRef<DiffView>,
) -> impl IntoElement {
    view()
        .app_class(tw(
            "flex w-2/5 min-w-0 shrink-0 flex-col border-l border-border",
        ))
        .child(diff_file_tree(runtime, snapshot, diff))
}

/// Port of DiffFileTree.tsx DiffFileTree.
pub fn diff_file_tree(
    runtime: DiffPanelRuntime,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    diff: ComponentRef<DiffView>,
) -> impl IntoElement {
    let nodes = computed(move || snapshot.with(|s| s.as_ref().map(|s| Shared(s.tree.clone()))));
    let items = computed(move || nodes.get().map_or_else(Vec::new, |n| tree_items(&n.0)));
    let directories = computed(move || nodes.get().map_or_else(Vec::new, |n| directory_ids(&n.0)));
    let file_count =
        computed(move || snapshot.with(|s| s.as_ref().map_or(0, |s| s.shown_files.len())));
    // initialExpansion: "open": every folder starts expanded; the user's folds are kept while the
    // folders still exist.
    let collapsed = signal(Vec::<String>::new());
    let expanded = computed(move || {
        let collapsed = collapsed.get();
        directories
            .get()
            .into_iter()
            .filter(|id| !collapsed.contains(id))
            .map(SharedString::from)
            .collect::<Vec<_>>()
    });
    let all_expanded = computed(move || collapsed.with(Vec::is_empty));
    let selected = signal(None::<SharedString>);
    let section_title = computed(move || {
        snapshot.with(|s| {
            s.as_ref()
                .map(|s| super::diff::label_text(&s.resolved.section_title))
                .unwrap_or_default()
        })
    });
    // The externally requested file (a chat diff link) is selected and its folders opened, once
    // per request.
    let handled = stored(None::<(String, u64)>);
    effect(move || {
        let Some(s) = snapshot.get() else {
            return;
        };
        let Some(path) = s.resolved.file_path.clone() else {
            handled.replace(None);
            return;
        };
        let request = (path.clone(), s.resolved.reveal_request_id);
        if handled.get() == Some(request.clone()) || find(&s.tree, &path).is_none() {
            return;
        }
        handled.replace(Some(request));
        let ancestors = ancestor_ids(&s.tree, &path);
        collapsed.update(|c| c.retain(|id| !ancestors.contains(id)));
        selected.set(Some(SharedString::from(path)));
    });
    let select_runtime = runtime.clone();
    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col bg-background"))
        .child(
            view()
                .app_class(tw(
                    "flex flex-row h-10 min-h-10 shrink-0 items-center gap-1 border-b border-border/60 bg-background px-2 text-xs text-muted-foreground",
                ))
                .child(text(t("Files")).app_class(tw("px-1 font-medium text-foreground")))
                .child(
                    text(move || SharedString::from(file_count.get().to_string()))
                        .app_class(tw("ml-auto tabular-nums")),
                )
                .child(dynamic(move || {
                    (!directories.get().is_empty()).then(|| {
                        let expanded_now = all_expanded.get();
                        let label = if expanded_now {
                            t("Collapse all folders")
                        } else {
                            t("Expand all folders")
                        };
                        view()
                            .app_class(tw("inline-flex shrink-0"))
                            .tooltip(tooltip(
                                tooltip_create_handle(),
                                TooltipOptions::default(),
                                text(label.clone()),
                            ))
                            .child(button(
                                ButtonContent::Custom {
                                    label,
                                    child: icon_with_class(
                                        if expanded_now {
                                            Icon::ChevronsDownUp
                                        } else {
                                            Icon::ChevronsUpDown
                                        },
                                        "size-3.5",
                                    )
                                    .into_element(),
                                },
                                ButtonVariant::Ghost,
                                ButtonSize::IconXs,
                                || false,
                                move || {
                                    if all_expanded.get_untracked() {
                                        collapsed.set(directories.get_untracked());
                                    } else {
                                        collapsed.set(Vec::new());
                                    }
                                },
                            ))
                    })
                })),
        )
        .child(
            nsh_base::tree()
                .items(move || items.get())
                .expanded(move || Some(expanded.get()))
                .selected(move || nsh_base::SelectValue::from(selected.get()))
                .accessibility_label(move || {
                    Some(SharedString::from(tr(
                        "{section} files",
                        &[("section", Value::Text(section_title.get()))],
                    )))
                })
                .item_class(tw(
                    "group flex flex-row h-6 items-center gap-1.5 rounded-sm px-1.5 text-xs text-foreground/90 hover:bg-accent selected:bg-accent focus-visible:ring-1 focus-visible:ring-ring",
                ))
                .on_expanded_change(move |ids: &Vec<SharedString>| {
                    let open: Vec<String> = ids.iter().map(ToString::to_string).collect();
                    collapsed.set(
                        directories
                            .get_untracked()
                            .into_iter()
                            .filter(|id| !open.contains(id))
                            .collect(),
                    );
                })
                .on_selected_change(move |id: &SharedString| {
                    let Some(s) = snapshot.get_untracked() else {
                        return;
                    };
                    if find(&s.tree, id).is_some_and(|node| !node.is_directory()) {
                        selected.set(Some(id.clone()));
                        reveal_diff_file(&select_runtime, &s.0, diff, id);
                    }
                })
                .render_item(move |entry: &TreeEntry| diff_tree_row(nodes, entry.clone()))
                .app_class(tw("min-h-0 flex-1 overflow-hidden")),
        )
}

fn status_class(status: GitStatus) -> &'static str {
    match status {
        GitStatus::Added => "text-diff-addition-foreground",
        GitStatus::Deleted => "text-diff-deletion-foreground",
        GitStatus::Renamed | GitStatus::Modified => "text-info-foreground",
    }
}

fn status_letter(status: GitStatus) -> &'static str {
    match status {
        GitStatus::Added => "A",
        GitStatus::Deleted => "D",
        GitStatus::Renamed => "R",
        GitStatus::Modified => "M",
    }
}

/// A row as @pierre/trees draws it in compact density: indent, chevron or file glyph, the
/// name, and the git status letter.
fn diff_tree_row(
    nodes: Memo<Option<Shared<Vec<DiffTreeNode>>>>,
    entry: TreeEntry,
) -> impl IntoElement {
    let status = nodes.with_untracked(|n| {
        n.as_ref()
            .and_then(|n| find(&n.0, &entry.id).and_then(|node| node.status))
    });
    view()
        .app_class(tw("flex flex-row min-w-0 flex-1 items-center gap-1.5"))
        .pl(px(entry.depth as f32 * 12.0))
        .child(if entry.folder {
            icon_with_class(
                Icon::ChevronRight,
                "size-3.5 shrink-0 text-muted-foreground transition-transform group-aria-expanded:rotate-90",
            )
            .into_element()
        } else {
            icon_with_class(Icon::File, "size-3.5 shrink-0 text-muted-foreground").into_element()
        })
        .child(text(entry.label.clone()).app_class(tw("min-w-0 flex-1 truncate")))
        .children(status.map(|status| {
            text(status_letter(status))
                .accessible(false)
                .app_class(tw(&cn([
                    Some("shrink-0 font-mono text-3xs"),
                    Some(status_class(status)),
                ])))
        }))
}
