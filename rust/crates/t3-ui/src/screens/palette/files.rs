//! Port of apps/web/src/components/files/ProjectFilePicker.tsx ProjectFilePicker.

use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::view_logic::shell::palette_files::{
    self as client, ContentSearchView, FileSearchView, PaletteSearchClient, SearchOptions,
    SearchTarget, TextPart,
};

use super::{ContentOptions, PanelSize, command_palette_content};
use crate::i18n::{Text, Value, t, tr};
use crate::theme::{cn, tw};
use crate::ui::command::{self, CommandOptions, CommandState, InputOptions};
use crate::ui::toggle;

use crate::runtime::tasks::watch;

pub type OpenFile = Rc<dyn Fn(&SearchTarget, &str, Option<u64>)>;

/// Root-provided query service and the shared file-panel controller; no per-row render callbacks.
#[derive(Clone)]
pub struct PaletteSearchRuntime {
    pub client: Rc<dyn PaletteSearchClient>,
    pub open_file: OpenFile,
}

pub fn project_file_picker() -> impl IntoElement {
    let host = use_context::<PaletteSearchRuntime>();
    search_target(host, false)
}

/// Port of search/ProjectContentSearchDialog.tsx ProjectContentSearchDialog.
pub fn project_content_search_dialog() -> impl IntoElement {
    let host = use_context::<PaletteSearchRuntime>();
    search_target(host, true)
}

fn search_target(host: Option<PaletteSearchRuntime>, content: bool) -> impl IntoElement {
    let target = signal(None::<Option<SearchTarget>>);
    if let Some(host) = &host {
        let mut changes = host.client.watch_target();
        let task = watch(async move {
            while let Some(next) = changes.next().await {
                target.set(Some(next));
            }
        });
        cleanup(move || task.cancel());
    }
    dynamic(move || {
        let target = target.get()?;
        let host = host.clone()?;
        Some(untracked(|| match (target, content) {
            (None, false) => empty_project_file_picker().into_element(),
            (None, true) => empty_content_search_dialog().into_element(),
            (Some(target), false) => open_project_file_picker(host, target).into_element(),
            (Some(target), true) => open_content_search_dialog(host, target).into_element(),
        }))
    })
}

fn input(label: &'static str, placeholder: &'static str) -> InputOptions {
    let palette = super::palette_controller();
    let mut input = InputOptions::new(label, placeholder);
    input.on_cancel = Some(Rc::new(move || palette.escape()));
    input
}

fn empty_project_file_picker() -> impl IntoElement {
    let state = CommandState::new(
        Vec::new,
        CommandOptions {
            filter: false,
            ..Default::default()
        },
        |_| {},
    );
    state.disabled.set(true);
    command_palette_content(
        state,
        ContentOptions {
            input: input("File picker", "Search files…"),
            escape_label: "Back".into(),
            footer_action_label: Some("Open file".into()),
            ..Default::default()
        },
        view()
            .app_class(tw("py-10 text-center text-sm text-muted-foreground"))
            .child(text(move || t("Open a project to search its files."))),
    )
}

fn empty_content_search_dialog() -> impl IntoElement {
    let state = CommandState::new(
        Vec::new,
        CommandOptions {
            filter: false,
            ..Default::default()
        },
        |_| {},
    );
    state.disabled.set(true);
    command_palette_content(
        state,
        ContentOptions {
            input: input("Search project contents", "Search project contents…"),
            escape_label: "Back".into(),
            footer_action_label: Some("Open file".into()),
            panel_size: PanelSize::Fill,
            ..Default::default()
        },
        text(move || t("Open a project to search its files."))
            .app_class(tw("m-auto px-6 text-center text-muted-foreground text-sm")),
    )
}

fn open_project_file_picker(host: PaletteSearchRuntime, target: SearchTarget) -> impl IntoElement {
    let current = signal(FileSearchView {
        pending: true,
        ..Default::default()
    });
    let matches = computed(move || {
        current.with(|view| {
            if view.pending {
                vec![]
            } else {
                client::file_picker_matches(
                    &view.entries,
                    &view.matched_query,
                    client::FILE_PICKER_LIMIT,
                )
            }
        })
    });
    let palette = super::palette_controller();
    let source = host.client.clone();
    let destination = target.clone();
    let state = CommandState::new(
        move || {
            matches.with(|matches| {
                matches
                    .iter()
                    .map(|item| {
                        command::Item::new(format!("file:{}", item.path), item.name.clone())
                    })
                    .collect()
            })
        },
        CommandOptions {
            filter: false,
            ..Default::default()
        },
        move |value| {
            if current.with_untracked(|view| view.pending || view.error.is_some())
                || !source.can_open(&destination)
            {
                return;
            }
            let Some(item) = matches.with_untracked(|items| {
                items
                    .iter()
                    .find(|item| format!("file:{}", item.path) == value)
                    .cloned()
            }) else {
                return;
            };
            palette.close();
            (host.open_file)(&destination, &item.path, None);
        },
    );
    let target = stored(target);
    let service = host.client;
    effect(move || {
        let query = state.query.get();
        let mut changes = service.files(target.get(), query);
        current.set(FileSearchView {
            pending: true,
            ..Default::default()
        });
        // Effect-owned task cancellation drops the old resource subscription before another query.
        let task = watch(async move {
            while let Some(next) = changes.next().await {
                current.set(next);
            }
        });
        cleanup(move || task.cancel());
    });
    let empty = computed(move || matches.with(|items| items.is_empty()));
    let active = computed(move || current.with(|view| client::has_query(&view.matched_query)));
    command_palette_content(
        state,
        ContentOptions {
            input: input("File picker", "Search files…"),
            escape_label: "Back".into(),
            footer_action_label: Some("Open file".into()),
            panel_size: PanelSize::TallList,
            ..Default::default()
        },
        dynamic(move || {
            if empty.get() {
                return view()
                    .app_class(tw("py-10 text-center text-sm text-muted-foreground"))
                    .child(text(move || {
                        current.with(|view| file_empty_state(&state.query.get(), view).resolve())
                    }))
                    .into_element();
            }
            untracked(|| {
                command::command_list(
                    state,
                    "File picker",
                    view()
                        // Project names are literal content, unlike CommandGroupLabel's key-only slot.
                        .child(
                            text(move || target.with(|target| target.project_name.clone()))
                                .app_class(tw(
                                    "px-2 py-1.5 font-medium text-muted-foreground text-xs",
                                )),
                        )
                        .child(command::command_collection(state, move |item| {
                            let matched = computed(move || {
                                item.with(|item| {
                                    item.as_ref().and_then(|item| {
                                        matches.with(|matches| {
                                            matches
                                                .iter()
                                                .find(|matched| {
                                                    format!("file:{}", matched.path)
                                                        == item.value.as_ref()
                                                })
                                                .cloned()
                                        })
                                    })
                                })
                            });
                            file_result_row(state, item, matched, active)
                        })),
                )
                .into_element()
            })
        }),
    )
}

fn file_result_row(
    state: CommandState,
    item: Memo<Option<command::Item>>,
    matched: Memo<Option<client::FileMatch>>,
    active: Memo<bool>,
) -> impl IntoElement {
    let name = computed(move || {
        matched.with(|item| {
            item.as_ref()
                .map(|item| {
                    client::text_parts(
                        &item.name,
                        &item
                            .name_match_indices
                            .iter()
                            .map(|at| (*at, *at + 1))
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default()
        })
    });
    let path = computed(move || {
        matched.with(|item| {
            item.as_ref()
                .map(|item| {
                    client::text_parts(
                        &item.path,
                        &item
                            .path_match_indices
                            .iter()
                            .map(|at| (*at, *at + 1))
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default()
        })
    });
    command::command_item(
        state,
        item,
        move || {
            Some(item.with(|item| {
                item.as_ref()
                    .is_some_and(|item| state.highlighted.get().as_ref() == Some(&item.value))
            }))
        },
        view()
            .app_class(tw("flex min-w-0 flex-1 flex-col"))
            .child(
                view()
                    .app_class(tw(
                        "flex min-w-0 items-center gap-1.5 text-sm text-foreground",
                    ))
                    .child(highlighted_fuzzy_text(name, active)),
            )
            .child(
                view()
                    .app_class(tw("min-w-0 text-muted-foreground/70 text-xs"))
                    .child(highlighted_fuzzy_text(path, active)),
            ),
    )
}

fn file_empty_state(query: &str, view: &FileSearchView) -> Text {
    if let Some(error) = &view.error {
        return Text::content(error.clone());
    }
    match (view.pending, client::has_query(query)) {
        (true, true) => "Searching workspace files…".into(),
        (true, false) => "Indexing workspace files…".into(),
        (false, true) => "No matching files.".into(),
        (false, false) => "No files found.".into(),
    }
}

fn highlighted_fuzzy_text(parts: Memo<Vec<TextPart>>, active: Memo<bool>) -> impl IntoElement {
    // Native text nodes cannot paint nested dynamic regions; rebuild this bounded line outside it.
    dynamic(move || {
        let mut line = text(SharedString::default())
            .app_class(move || tw(&cn([active.get().then_some("text-muted-foreground")])));
        for part in parts.get() {
            line = line.child(text(part.text).app_class(tw(&cn([
                part.highlighted.then_some("font-semibold text-foreground"),
            ]))));
        }
        line
    })
}

/// Port of search/HighlightedSearchLine.tsx's plain-code fallback.
fn highlighted_search_line(parts: Memo<Vec<TextPart>>) -> impl IntoElement {
    dynamic(move || {
        let mut line = text(SharedString::default());
        for part in parts.get() {
            line = line.child(
                text(part.text).app_class(tw(&cn([part
                    .highlighted
                    .then_some("rounded-xs bg-primary/25 text-inherit")]))),
            );
        }
        line
    })
}

/// Port of search/ProjectContentSearchDialog.tsx OpenContentSearchDialog.
fn open_content_search_dialog(
    host: PaletteSearchRuntime,
    target: SearchTarget,
) -> impl IntoElement {
    let current = signal(ContentSearchView {
        checking_access: true,
        ..Default::default()
    });
    let query_pending = signal(true);
    let options = signal(SearchOptions::default());
    let palette = super::palette_controller();
    let source = host.client.clone();
    let destination = target.clone();
    let open_file = host.open_file;
    let state = CommandState::new(
        move || {
            current.with(|view| {
                view.matches
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let mut row = command::Item::new(index.to_string(), item.path.0.clone());
                        row.disabled = !view.can_read_files || view.pending || view.error.is_some();
                        row
                    })
                    .collect()
            })
        },
        CommandOptions {
            filter: false,
            ..Default::default()
        },
        move |value| {
            if query_pending.get_untracked() || !source.can_open(&destination) {
                return;
            }
            let matched = current.with_untracked(|view| {
                if !view.can_read_files || view.pending || view.error.is_some() {
                    return None;
                }
                view.matches.get(value.parse::<usize>().ok()?).cloned()
            });
            if let Some(item) = matched {
                palette.close();
                open_file(&destination, &item.path.0, Some(item.line_number));
            }
        },
    );
    let target = stored(target);
    let service = host.client;
    effect(move || {
        query_pending.set(true);
        let mut changes = service.contents(target.get(), state.query.get(), options.get());
        let task = watch(async move {
            while let Some(next) = changes.next().await {
                current.set(next);
                query_pending.set(false);
            }
        });
        cleanup(move || task.cancel());
    });
    effect(move || {
        state
            .disabled
            .set(current.with(|view| !view.can_read_files));
    });
    let accessory = view().app_class(tw("absolute inset-e-2.5 top-1/2 flex shrink-0 -translate-y-1/2 items-center gap-0.5 rounded-lg border bg-muted/30 p-0.5"))
        .child(search_option_button("Match case".into(), "Aa", options, current, 0))
        .child(search_option_button("Match whole word".into(), "ab", options, current, 1))
        .child(search_option_button("Use regular expression".into(), ".*", options, current, 2));
    command_palette_content(
        state,
        ContentOptions {
            // Interpolated input labels await UI-COMMAND's Text slots; no content is used as a key.
            input: input("Search project contents", "Search project contents…"),
            escape_label: "Back".into(),
            footer_action_label: Some("Open file".into()),
            panel_size: PanelSize::Fill,
            input_accessory: Some(accessory.into_element()),
            ..Default::default()
        },
        dynamic(move || {
            untracked(|| {
                let status = when(
                    move || {
                        current.with(|view| {
                            view.can_read_files
                                && (view.has_query
                                    || view.pending
                                    || view.error.is_some()
                                    || view.invalid_regex)
                        })
                    },
                    move || {
                        view().app_class(tw("flex h-9 shrink-0 items-center border-b px-3 text-xs text-muted-foreground"))
                .child(text(move || current.with(content_status)).app_class(move || tw(&cn([current.with(|view| view.error.is_some() || view.invalid_regex).then_some("text-destructive")]))))
                    },
                );
                let results = dynamic(move || {
                    if current.with(|view| view.matches.is_empty()) {
                        return view().app_class(tw("flex flex-1 items-center justify-center px-6 text-center text-sm text-muted-foreground"))
                    .child(text(move || current.with(content_empty_state).resolve())).into_element();
                    }
                    untracked(|| content_results(state, current).into_element())
                });
                vec![status.into_element(), results.into_element()]
            })
        }),
    )
}

fn search_option_button(
    label: Text,
    glyph: &'static str,
    options: Signal<SearchOptions>,
    current: Signal<ContentSearchView>,
    at: u8,
) -> impl IntoElement {
    toggle::toggle(
        toggle::Content::slot(
            label.resolve().to_string(),
            text(String::from(glyph)).app_class(tw("font-mono")),
        ),
        toggle::Variant::Segmented,
        toggle::Size::Segmented,
        move || {
            options.with(|options| match at {
                0 => options.case_sensitive,
                1 => options.whole_word,
                _ => options.use_regex,
            })
        },
        move || current.with(|view| !view.can_read_files),
        move |pressed| {
            options.update(|options| match at {
                0 => options.case_sensitive = pressed,
                1 => options.whole_word = pressed,
                _ => options.use_regex = pressed,
            })
        },
    )
}

fn content_status(view: &ContentSearchView) -> String {
    if view.pending {
        return t("Searching…");
    }
    if let Some(error) = &view.error {
        return error.clone();
    }
    if view.invalid_regex {
        return t("Invalid regular expression");
    }
    let files = view
        .matches
        .iter()
        .map(|item| &item.path.0)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    tr(
        "{count}{more} results in {files} files",
        &[
            (
                "count",
                Value::Text(
                    crate::i18n::formats().number(view.matches.len() as f64, Default::default()),
                ),
            ),
            (
                "more",
                Value::Text(if view.truncated { "+" } else { "" }.into()),
            ),
            (
                "files",
                Value::Text(crate::i18n::formats().number(files as f64, Default::default())),
            ),
        ],
    )
}

fn content_empty_state(view: &ContentSearchView) -> Text {
    if view.checking_access {
        "Checking file access…".into()
    } else if !view.can_read_files {
        Text::content(view.error.clone().unwrap_or_default())
    } else if view.has_query && !view.pending && view.error.is_none() {
        "No results found.".into()
    } else {
        "Type to search across your project.".into()
    }
}

#[derive(Clone, PartialEq)]
enum ContentRow {
    File { path: String, count: usize },
    Match(usize),
}

fn content_results(state: CommandState, current: Signal<ContentSearchView>) -> impl IntoElement {
    let rows = computed(move || {
        current.with(|view| {
            client::group_matches(&view.matches, view.matches.len())
                .into_iter()
                .flat_map(|group| {
                    std::iter::once(ContentRow::File {
                        path: group.path,
                        count: group.result_indices.len(),
                    })
                    .chain(group.result_indices.into_iter().map(ContentRow::Match))
                })
                .collect::<Vec<_>>()
        })
    });
    let list = element_ref();
    effect(move || {
        let index = state
            .highlighted
            .get()
            .and_then(|value| value.parse::<usize>().ok());
        if let Some(at) = rows.with(|rows| {
            rows.iter()
                .position(|row| matches!(row, ContentRow::Match(at) if Some(*at) == index))
        }) {
            list.scroll_to_index(at as u32, Percentage(0.5), px(0.0), false);
        }
    });
    // Parent mode preserves ScrollArea's content extent while recycling the source's grouped rows.
    let results = virtual_list(move || rows.with(Vec::len), move |at| {
        let row = computed(move || rows.with(|rows| rows.get(at.index()).cloned()));
        let is_header = computed(move || row.with(|row| matches!(row, Some(ContentRow::File { .. }))));
        let matched = computed(move || row.with(|row| match row {
            Some(ContentRow::Match(index)) => current.with(|view| view.matches.get(*index).cloned()), _ => None,
        }));
        let index = move || row.with(|row| match row { Some(ContentRow::Match(index)) => Some(*index), _ => None });
        dynamic(move || {
            if is_header.get() {
                return untracked(|| view().app_class(tw("flex h-8 items-center gap-2 bg-popover/95 px-3 text-xs backdrop-blur-sm"))
                    .child(text(move || row.with(|row| match row { Some(ContentRow::File { path, .. }) => path.rsplit('/').next().unwrap_or(path).to_owned(), _ => String::new() })).app_class(tw("font-medium text-foreground")))
                    .child(text(move || row.with(|row| match row { Some(ContentRow::File { path, .. }) => path.rsplit_once('/').map_or("", |(directory, _)| directory).to_owned(), _ => String::new() })).app_class(tw("min-w-0 truncate text-muted-foreground")))
                    .child(text(move || row.with(|row| match row { Some(ContentRow::File { count, .. }) => crate::i18n::formats().number(*count as f64, Default::default()), _ => String::new() })).app_class(tw("ml-auto rounded-full bg-muted px-1.5 py-0.5 tabular-nums text-3xs text-muted-foreground"))).into_element());
            }
            untracked(|| nsh_base::button()
                .accessibility_label(move || matched.with(|item| item.as_ref().map(|item| SharedString::from(format!("{}:{}", item.path.0, item.line_number)))))
                .disabled(move || current.with(|view| !view.can_read_files || view.pending || view.error.is_some()))
                .on_mouse_enter(move |_: &MouseEvent| { if let Some(index) = index() { state.highlight(Some(index.to_string().into())); } })
                .on_press(move || { if let Some(index) = index() { state.choose(&index.to_string()); } })
                .app_class(move || tw(&cn([Some("flex h-7 w-full min-w-0 items-center gap-3 px-3 text-left font-mono text-xs hover:bg-accent/60 disabled:pointer-events-none"), (state.highlighted.get().and_then(|value| value.parse::<usize>().ok()) == index()).then_some("bg-accent text-accent-foreground")])))
                .child(text(move || matched.with(|item| item.as_ref().map(|item| item.line_number.to_string()).unwrap_or_default())).app_class(tw("w-10 shrink-0 text-right tabular-nums text-muted-foreground/70")))
                .child(view().app_class(tw("min-w-0 flex-1 truncate"))
                    .child(highlighted_search_line(computed(move || matched.with(|item| item.as_ref().map(|item| client::text_parts(&item.line_content, &item.match_ranges.iter().map(|range| (range.start as usize,range.end as usize)).collect::<Vec<_>>())).unwrap_or_default()))))).into_element())
        })
    }).viewport(nsh_core::ListViewport::Parent).element_ref(&list)
        .estimated_item_size(px(28.0)).content_padding_y(px(8.0)).w_full();
    // Match the browser's flex-1 percentage basis, rather than NSH's zero-point basis.
    scroll()
        .app_class(tw("min-h-0 flex-1"))
        .basis(Percentage(0.0))
        .child(results)
}
