//! Port of apps/web/src/components/files/FileBrowserPanel.tsx FileBrowserPanel and
//! FileExplorerPane.tsx FileExplorerPane. The tree model (lazy folders, search, expansion,
//! reveal) is the client's `workspace::project_files`; this file draws it.
//!
//! Dropped classes with no NSH meaning: `[transition:background-color_5000000s…]` (autofill),
//! the search input's `[&::-webkit-search-*]` resets. Pierre's tree is NSH's `Tree`
//! (virtualised: only rows in view are made).

use std::{collections::BTreeMap, rc::Rc};

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::{
    services::filesystem::ReadAccess,
    types::EnvironmentId,
    workspace::project_files::tree::{FileTreeNode, FileTreeView, Problem, Reveal},
};
use t3_contracts::project::ProjectEntryKind;

use crate::i18n::{Text, Value, t, tr};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

use crate::runtime::tasks::watch;

/// The client tree the panel draws (`ProjectFiles::tree`).
pub trait FileTreeHandle {
    fn view(&self) -> FileTreeView;
    fn changes(&self) -> LocalBoxStream<'static, ()>;
    fn set_expanded(&self, ids: &[SharedString]);
    fn toggle_all(&self);
    fn refresh(&self);
    fn set_query(&self, query: String);
    /// A selection in the tree; true when it is a file to open.
    fn select(&self, path: &str) -> bool;
    fn load_ancestors(&self, path: &str);
    fn reveal(&self, path: Option<&str>, reveal_id: u64) -> Option<Reveal>;
    fn observe_mutation(&self, mutation: Option<&str>);
}

/// The Files panel's client services (`runtime/services/files.rs`).
pub trait FilesService {
    fn tree(
        &self,
        environment: &EnvironmentId,
        cwd: &str,
        host_folder: bool,
    ) -> Rc<dyn FileTreeHandle>;
    fn read_access(&self, environment: &EnvironmentId) -> ReadAccess;
    fn access_changes(&self, environment: &EnvironmentId) -> LocalBoxStream<'static, ()>;
    /// A web `localStorage` preference, kept in the client's local preferences.
    fn preference(&self, key: &'static str) -> LocalBoxFuture<'static, Option<serde_json::Value>>;
    fn set_preference(&self, key: &'static str, value: serde_json::Value);
}

#[derive(Clone)]
pub struct FilesRuntime(pub Rc<dyn FilesService>);

/// A provided runtime (tests), else the host client's. `None` for the demo backend.
pub fn files_runtime() -> Option<FilesRuntime> {
    use_context::<FilesRuntime>().or_else(|| {
        crate::runtime::rt()
            .backend()
            .client()
            .map(|client| crate::runtime::services::files::files(&client))
    })
}

pub fn problem_text(problem: &Problem) -> Text {
    if problem.owned {
        Text::client_key(problem.text.clone())
    } else {
        Text::content(problem.text.clone())
    }
}

const MIN_WIDTH: f32 = 160.0;
const MAX_WIDTH: f32 = 560.0;
const FILE_CONTENT_MIN_WIDTH: f32 = 256.0;
const EXPLORER_WIDTH_KEY: &str = "t3code.fileExplorerWidth";

/// FileExplorerPane's clamp: never wider than the row less the 256px content reserve.
pub fn explorer_width(requested: f32, row: f32) -> f32 {
    let max = (row - FILE_CONTENT_MIN_WIDTH).clamp(MIN_WIDTH, MAX_WIDTH);
    requested.clamp(MIN_WIDTH, max)
}

/// Port of FileExplorerPane: a persisted, resizable tree column; fills the row when not resizable.
pub fn file_explorer_pane(
    runtime: FilesRuntime,
    resizable: bool,
    row_width: Memo<f32>,
    children: impl IntoElement,
) -> impl IntoElement {
    let width = signal(256.0_f32);
    let start = signal(0.0_f32);
    let service = runtime.0.clone();
    watch(async move {
        if let Some(saved) = service
            .preference(EXPLORER_WIDTH_KEY)
            .await
            .and_then(|v| v.as_f64())
        {
            width.try_set(saved as f32);
        }
    });
    let save = runtime.0.clone();
    let clamped = computed(move || explorer_width(width.get(), row_width.get()));
    let pane = view().label(t("File explorer")).app_class(tw(&cn([
        Some("relative flex min-h-0 min-w-0 shrink-0 bg-background"),
        Some(if resizable {
            "border-l border-border/60"
        } else {
            "flex-1"
        }),
    ])));
    let pane = if resizable {
        pane.w(move || px(clamped.get()))
    } else {
        pane
    };
    pane.child(resizable.then(move || {
            nsh_base::resize_handle()
                .test_id("file-explorer-resize")
                .edge(Some(nsh_base::HandleEdge::Leading))
                .accessibility_label(Some(SharedString::from(t("Resize file explorer"))))
                .on_drag_start(move || start.set(clamped.get_untracked()))
                .on_drag_move(move |offset: &Points| width.set(start.get_untracked() - offset.0))
                .on_drag_end(move |offset: &Points| {
                    let next = explorer_width(start.get_untracked() - offset.0, row_width.get_untracked());
                    width.set(next);
                    save.set_preference(EXPLORER_WIDTH_KEY, serde_json::json!(next));
                })
                .child(view().app_class(tw("pointer-events-none h-full w-px bg-transparent group-hover:bg-border group-focus-visible:bg-primary/60 group-active:bg-primary/60")))
        }))
        .child(children)
}

fn with_tooltip(
    caption: impl Fn() -> String + 'static,
    control: impl IntoElement,
) -> impl IntoElement {
    view()
        .app_class(tw("inline-flex shrink-0"))
        .tooltip(tooltip(
            tooltip_create_handle(),
            TooltipOptions::default(),
            text(move || SharedString::from(caption())),
        ))
        .child(control)
}

/// What the panel needs from its parent (FileBrowserPanelProps).
#[derive(Clone)]
pub struct FileBrowserProps {
    pub environment: EnvironmentId,
    pub cwd: String,
    pub project_name: String,
    pub selected_path: Memo<Option<String>>,
    pub reveal_id: Memo<u64>,
    pub on_open_file: Rc<dyn Fn(String)>,
    /// Host-folder entries are absolute: no expand-all, search only loaded folders.
    pub host_folder: bool,
    pub mutation: Memo<Option<String>>,
    /// Hidden tabs stay mounted; they stop following the tree while hidden.
    pub visible: Memo<bool>,
}

/// The tree's rows as NSH items. A folder not listed yet (or collapsed) holds one placeholder
/// row, so it can expand before its children are known.
fn tree_items(
    nodes: &[FileTreeNode],
    states: &mut BTreeMap<Rc<str>, (bool, bool)>,
) -> Vec<nsh_base::TreeItem> {
    nodes
        .iter()
        .map(|node| {
            states.insert(node.id.clone(), (node.ignored, node.loading));
            let item = nsh_base::TreeItem::new(node.id.to_string(), node.label.to_string());
            match (&node.kind, &node.children) {
                (ProjectEntryKind::File, _) => item,
                (_, Some(children)) => item.children(tree_items(children, states)),
                (_, None) => item.child(
                    nsh_base::TreeItem::new(SharedString::from(format!("{}\u{0}", node.id)), "")
                        .disabled(true),
                ),
            }
        })
        .collect()
}

/// Port of FileBrowserPanel.
pub fn file_browser_panel(runtime: FilesRuntime, props: FileBrowserProps) -> impl IntoElement {
    let handle = runtime
        .0
        .tree(&props.environment, &props.cwd, props.host_folder);
    let view_state = signal_with(handle.view(), nsh_core::element::SignalEquals::Never);
    let selected = signal(props.selected_path.get_untracked());
    let tree_ref = component_ref::<nsh_base::Tree>();
    let handle = stored(handle);
    let panel_path = {
        let cwd = props.cwd.clone();
        let host = props.host_folder;
        move |relative: &str| {
            if host {
                t3_client::workspace::project_files::preview::join_host_path(&cwd, relative)
            } else {
                relative.to_owned()
            }
        }
    };
    let selected_path = props.selected_path;
    let reveal_id = props.reveal_id;
    let try_reveal = move || {
        let path = selected_path.get_untracked();
        if let Some(reveal) = handle.with(|h| h.reveal(path.as_deref(), reveal_id.get_untracked()))
        {
            selected.set(Some(reveal.id.to_string()));
            if let Some(api) = tree_ref.api::<nsh_base::TreeApi>() {
                api.reveal_item(reveal.id.to_string());
                api.focus();
            }
        }
    };
    // The client's tree changes outside NSH; this task ends with the panel.
    {
        let mut changes = handle.with(|h| h.changes());
        watch(async move {
            while changes.next().await.is_some() {
                let Some(view) = handle.try_with(|h| h.view()) else {
                    return;
                };
                if !view_state.try_set(view) {
                    return;
                }
                try_reveal();
            }
        });
    }
    // oxlint-style reason: loads the selected file's ancestors, as loadFileAncestors on a new path.
    effect(move || {
        let path = selected_path.get();
        let _ = reveal_id.get();
        if let Some(path) = path.as_deref() {
            untracked(|| handle.with(|h| h.load_ancestors(path)));
        }
        untracked(try_reveal);
    });
    effect(move || {
        let mutation = props.mutation.get();
        untracked(|| handle.with(|h| h.observe_mutation(mutation.as_deref())));
    });

    let query = signal(String::new());
    let search_focused = signal(false);
    let error =
        computed(move || view_state.with(|v| v.error.clone().or_else(|| v.search.error.clone())));
    let open = props.on_open_file.clone();
    let project_name = props.project_name.clone();
    let host_folder = props.host_folder;

    let toggle_state = computed(move || {
        view_state.with(|v| {
            (
                v.has_directories && !host_folder,
                v.expand_all || v.all_expanded,
                v.all_expanded,
            )
        })
    });
    let subheader = view()
        .app_class(tw("flex h-10 min-h-10 shrink-0 items-center gap-1 border-b border-border/60 bg-background px-2"))
        .child(refresh_files_button(
            computed(move || view_state.with(|v| v.pending || v.search.pending)),
            move || handle.with(|h| h.refresh()),
        ))
        .child(file_search_field(
            query,
            search_focused,
            tr("Search {project} files", &[("project", Value::Text(project_name.clone()))]),
            move |value| handle.with(|h| h.set_query(value)),
        ))
        .child(dynamic(move || {
            // Rebuilt only when the button's own state changes, never per tree change, so a press
            // during folder loading reaches a mounted control.
            let (shown, expanded, all) = toggle_state.get();
            shown.then(|| {
                untracked(|| {
                    let label = move || if expanded { t("Collapse all folders") } else { t("Expand all folders") };
                    with_tooltip(
                        label,
                        button(
                            ButtonContent::Custom {
                                label: label(),
                                child: icon_with_class(
                                    if all { Icon::ChevronsDownUp } else { Icon::ChevronsUpDown },
                                    "size-3.5",
                                )
                                .into_element(),
                            },
                            ButtonVariant::Ghost,
                            ButtonSize::IconXs,
                            || false,
                            move || handle.with(|h| h.toggle_all()),
                        ),
                    )
                    .into_element()
                })
            })
        }));

    let retry = move || handle.with(|h| h.refresh());
    let error_row = dynamic(move || {
        error.get().map(|problem| {
            untracked(|| {
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(format!(
                        "{} {}",
                        problem_text(&problem).resolve(),
                        t("Click to retry.")
                    ))))
                    .on_press(retry)
                    .app_class(tw("p-4 text-left text-xs leading-relaxed text-destructive"))
                    .child(text(format!(
                        "{} {}",
                        problem_text(&problem).resolve(),
                        t("Click to retry.")
                    )))
                    .into_element()
            })
        })
    });
    let truncated_row = when(
        move || view_state.with(|v| v.search.active && v.search.truncated && !v.search.pending),
        || {
            view()
                .app_class(tw("px-3 py-1 text-xs text-muted-foreground"))
                .child(text(t("More matches available. Refine your search.")))
        },
    );
    // `sr-only` when rows already show: no element, the status is announced instead.
    let loading_row = when(
        move || {
            view_state.with(|v| {
                (!v.ready || v.search.pending) && !(v.has_entries && !v.search.no_matches)
            })
        },
        || {
            view()
                .app_class(tw("px-3 py-1 text-xs text-muted-foreground"))
                .child(text(t("Loading files…")))
        },
    );
    let no_matches = when(
        move || {
            view_state.with(|v| {
                v.search.no_matches
                    && v.ready
                    && !v.pending
                    && !v.search.pending
                    && v.error.is_none()
                    && v.search.error.is_none()
            })
        },
        || {
            view()
                .app_class(tw("px-3 py-4 text-xs text-muted-foreground"))
                .child(text(t("No matching files.")))
        },
    );

    let states = signal(Rc::new(BTreeMap::<Rc<str>, (bool, bool)>::new()));
    let items = computed(move || {
        view_state.with(|v| {
            let mut map = BTreeMap::new();
            let items = tree_items(&v.nodes, &mut map);
            untracked(|| states.set(Rc::new(map)));
            items
        })
    });
    let expanded = computed(move || {
        view_state.with(|v| {
            v.expanded
                .iter()
                .map(|id| SharedString::from(id.to_string()))
                .collect::<Vec<_>>()
        })
    });
    let tree_label = tr(
        "{project} files",
        &[("project", Value::Text(props.project_name.clone()))],
    );
    let tree = nsh_base::tree()
        .component_ref(&tree_ref)
        .items(move || items.get())
        .expanded(move || Some(expanded.get()))
        .selected(move || nsh_base::SelectValue::Controlled(selected.get().map(SharedString::from)))
        .accessibility_label(Some(SharedString::from(tree_label)))
        .app_class(tw("min-h-0 flex-1 overflow-hidden"))
        .list_class(Some(tw("px-1.5 py-1")))
        .item_class(Some(tw(
            "group flex h-6 min-w-0 items-center gap-1 rounded-md pe-2 text-xs text-foreground hover:bg-foreground/7 selected:bg-foreground/12 focus-visible:ring-2 focus-visible:ring-ring disabled:hover:bg-transparent",
        )))
        .render_item(move |entry: &nsh_base::TreeEntry| file_tree_row(entry.clone(), states))
        .on_expanded_change(move |ids: &Vec<SharedString>| {
            let ids: Vec<SharedString> = ids.iter().filter(|id| !id.ends_with('\u{0}')).cloned().collect();
            handle.with(|h| h.set_expanded(&ids));
        })
        .on_selected_change({
            let open = open.clone();
            let panel_path = panel_path.clone();
            move |id: &SharedString| {
                if id.ends_with('\u{0}') {
                    return;
                }
                selected.set(Some(id.to_string()));
                if handle.with(|h| h.select(id)) {
                    open(panel_path(id));
                }
            }
        })
        .on_action(move |id: &SharedString| {
            if !id.ends_with('\u{0}') && handle.with(|h| h.select(id)) {
                open(panel_path(id));
            }
        });

    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col bg-background"))
        .test_id("file-browser-panel")
        .child(subheader)
        .child(error_row)
        .child(truncated_row)
        .child(loading_row)
        .child(no_matches)
        // Pierre's tree unmounts while a search has no matches; NSH keeps it mounted and hidden so
        // its scroll and focus survive the search.
        .child(
            view()
                .app_class(move || {
                    Some(tw(if view_state.with(|v| v.search.no_matches) {
                        "hidden"
                    } else {
                        "flex min-h-0 flex-1"
                    }))
                })
                .child(tree),
        )
}

/// One tree row as Pierre draws it in compact density: indent, chevron, icon, name.
/// Each row's (ignored, loading) state by node id, rebuilt with the items.
type RowStates = Signal<Rc<BTreeMap<Rc<str>, (bool, bool)>>>;

fn file_tree_row(entry: nsh_base::TreeEntry, states: RowStates) -> AnyElement {
    if entry.id.ends_with('\u{0}') {
        return view()
            .pl(px(8.0 + entry.depth as f32 * 12.0))
            .app_class(tw(
                "flex min-w-0 items-center text-xs text-muted-foreground",
            ))
            .child(text(t("Loading…")))
            .into_element();
    }
    let id: Rc<str> = Rc::from(entry.id.as_ref());
    let state = {
        let id = id.clone();
        computed(move || states.with(|s| s.get(&id).copied().unwrap_or_default()))
    };
    let folder = entry.folder;
    view()
        .pl(px(4.0 + entry.depth as f32 * 12.0))
        .app_class(move || {
            Some(tw(&cn([
                Some("flex min-w-0 flex-1 items-center gap-1"),
                state.get().0.then_some("opacity-60"),
            ])))
        })
        .child(if folder {
            view()
                .app_class(tw(
                    "flex size-4 shrink-0 items-center justify-center text-muted-foreground",
                ))
                .child(icon_with_class(
                    Icon::ChevronRight,
                    "size-3 transition-transform group-aria-expanded:rotate-90",
                ))
                .into_element()
        } else {
            view().app_class(tw("size-4 shrink-0")).into_element()
        })
        .child(dynamic(move || {
            Some(icon_with_class(
                match (folder, state.get().1) {
                    (true, true) => Icon::LoaderCircle,
                    (true, false) => Icon::Folder,
                    (false, _) => Icon::File,
                },
                "size-3.5 shrink-0 text-muted-foreground",
            ))
        }))
        .child(text(entry.label.clone()).app_class(tw("min-w-0 truncate")))
        .into_element()
}

fn refresh_files_button(pending: Memo<bool>, on_refresh: impl Fn() + 'static) -> impl IntoElement {
    // RefreshIcon's spin is a repainting animation; the tooltip says it instead.
    with_tooltip(
        move || {
            if pending.get() {
                t("Refreshing…")
            } else {
                t("Refresh files")
            }
        },
        button(
            ButtonContent::Icon {
                icon: Icon::RefreshCw,
                label: t("Refresh workspace files"),
            },
            ButtonVariant::Ghost,
            ButtonSize::IconXs,
            || false,
            on_refresh,
        ),
    )
}

/// Port of FileSearchField: `InputGroup variant="ghost"` around `InputGroupInput size="sm"`.
/// Escape clears the search and leaves the field.
fn file_search_field(
    value: Signal<String>,
    focused: Signal<bool>,
    label: String,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    let on_change = Rc::new(on_change);
    let changed = on_change.clone();
    let input = element_ref();
    input.on_focus_in(move || focused.set(true));
    input.on_focus_out(move || focused.set(false));
    view()
        .app_class(move || {
            Some(tw(&cn([
                Some("relative inline-flex w-full min-w-0 items-center rounded-lg border text-base text-foreground sm:text-sm border-transparent bg-transparent shadow-none hover:bg-muted/40 h-7 flex-1"),
                focused.get().then_some("bg-background"),
            ])))
        })
        .child(
            nsh_base::input()
                .element_ref(&input)
                .value(move || value.get())
                .on_change(move |next: &str| {
                    value.set(next.to_owned());
                    changed(next.to_owned());
                })
                .on_key_down(move |event: &nsh_core::KeyEvent| {
                    if event.key.as_ref() == "Escape" {
                        value.set(String::new());
                        on_change(String::new());
                        input.blur();
                    }
                })
                .placeholder(t("Search files"))
                .label(SharedString::from(label))
                .app_class(tw("h-7.5 w-full min-w-0 rounded-lg px-[9px] leading-7.5 outline-none placeholder:text-placeholder sm:h-6.5 sm:leading-6.5")),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_explorer_keeps_room_for_the_file() {
        assert_eq!(explorer_width(400.0, 500.0), 244.0_f32.max(MIN_WIDTH));
        assert_eq!(explorer_width(100.0, 2000.0), MIN_WIDTH);
        assert_eq!(explorer_width(900.0, 2000.0), MAX_WIDTH);
        assert_eq!(
            explorer_width(300.0, 300.0),
            MIN_WIDTH,
            "a narrow row still shows the minimum"
        );
    }
}
