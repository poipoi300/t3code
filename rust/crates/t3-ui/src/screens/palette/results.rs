//! Port of apps/web/src/components/CommandPaletteResults.tsx CommandPaletteResults.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::view_logic::shell::palette::{self as client, Group, Item};

use crate::i18n::{Text, t};
use crate::keys::bindings::Keybindings;
use crate::runtime::Shared;
use crate::theme::{cn, tw};
use crate::ui::command::{self, CommandState};
use crate::ui::icon::{Icon, icon_with_class};

#[derive(Clone)]
enum Row {
    Label {
        key: String,
        label: String,
        first: bool,
    },
    Item {
        key: String,
        item: Rc<Item>,
    },
}

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Label {
                    key: a,
                    label: al,
                    first: af,
                },
                Self::Label {
                    key: b,
                    label: bl,
                    first: bf,
                },
            ) => a == b && al == bl && af == bf,
            (Self::Item { key: a, item: ai }, Self::Item { key: b, item: bi }) => {
                a == b && Rc::ptr_eq(ai, bi)
            }
            _ => false,
        }
    }
}

impl Row {
    fn key(&self) -> SharedString {
        match self {
            Self::Label { key, .. } | Self::Item { key, .. } => key.clone().into(),
        }
    }
}

/// The client's row projection, owned once rather than reconstructed by virtual cells.
#[derive(Clone)]
pub struct Results {
    pub groups: Shared<Vec<Group>>,
    rows: Vec<Row>,
}

impl PartialEq for Results {
    fn eq(&self, other: &Self) -> bool {
        self.groups == other.groups
    }
}

impl Results {
    pub fn new(groups: Shared<Vec<Group>>) -> Self {
        let rows = client::build_rows(&groups)
            .rows
            .into_iter()
            .map(|row| match row {
                client::Row::Label { key, label, first } => Row::Label {
                    key,
                    label: label.into(),
                    first,
                },
                client::Row::Item { key, item, .. } => Row::Item {
                    key,
                    item: Rc::new(item.clone()),
                },
            })
            .collect();
        Self { groups, rows }
    }

    /// Disabled rows are painted, but are absent from the web palette's keyboard index.
    pub fn command_items(&self) -> Vec<command::Item> {
        self.groups
            .iter()
            .flat_map(|group| &group.items)
            .filter(|item| !item.disabled)
            .map(|item| command::Item::new(item.value.clone(), title(item)))
            .collect()
    }
}

pub struct ResultsOptions {
    pub keys: Keybindings,
    pub empty_state_message: Option<Text>,
    pub actions_only: Memo<bool>,
}

fn command_palette_empty_state(options: &ResultsOptions) -> impl IntoElement + use<> {
    let custom = options.empty_state_message.clone();
    let actions = options.actions_only;
    view()
        .app_class(tw("py-10 text-center text-sm text-muted-foreground"))
        .child(text(move || {
            custom.as_ref().map_or_else(
                || {
                    if actions.get() {
                        t("No matching actions.").into()
                    } else {
                        t("No matching commands, projects, or threads.").into()
                    }
                },
                Text::resolve,
            )
        }))
}

pub fn command_palette_results(
    state: CommandState,
    results: Memo<Results>,
    options: ResultsOptions,
) -> impl IntoElement {
    let options = Rc::new(options);
    let empty = computed(move || results.with(|results| results.groups.is_empty()));
    dynamic(move || {
        if empty.get() {
            return command_palette_empty_state(&options).into_element();
        }
        let options = options.clone();
        untracked(|| {
            command::command_list(
                state,
                "Command palette",
                each(
                    move || results.with(|results| results.rows.clone()),
                    Row::key,
                    move |row, _| {
                        render_row(state, computed(move || Some(row.get())), options.keys)
                    },
                ),
            )
            .into_element()
        })
    })
}

pub fn command_palette_virtualized_results(
    state: CommandState,
    results: Memo<Results>,
    options: ResultsOptions,
) -> impl IntoElement {
    let list = element_ref();
    let viewport = element_ref();
    effect(move || {
        let value = state.highlighted.get();
        let index = results.with(|results| {
            results.rows.iter().position(|row| {
            matches!(row, Row::Item { item, .. } if Some(item.value.as_str()) == value.as_deref())
        })
        });
        if let Some(index) = index {
            if state
                .items
                .with(|items| items.first().map(|item| item.value.clone()))
                == value
            {
                viewport.scroll_to_offset(px(0.0), false);
            } else {
                // NSH has no scrollIndexIntoView viewportMargin; this pin exposes index alignment.
                list.scroll_to_index(index as u32, Percentage(0.5), px(0.0), false);
            }
        }
    });
    let options = Rc::new(options);
    let empty = computed(move || results.with(|results| results.rows.is_empty()));
    dynamic(move || {
        if empty.get() {
            return command_palette_empty_state(&options).into_element();
        }
        let options = options.clone();
        untracked(|| {
            // Parent mode gives the bounded scroll surface LegendList's intrinsic content extent.
            let rows = virtual_list(
                move || results.with(|results| results.rows.len()),
                move |at| {
                    let project =
                        move || results.with(|results| results.rows.get(at.index()).cloned());
                    // Publish the recycled cell's projection before its row bindings read it.
                    // Pulling the list from a binding can retire that very binding's row.
                    let row = signal(untracked(project));
                    effect(move || row.set(project()));
                    render_row(state, computed(move || row.get()), options.keys)
                },
            )
            .keys(move || {
                results.with(|results| results.rows.iter().map(Row::key).collect::<Vec<_>>())
            })
            .viewport(nsh_core::ListViewport::Parent)
            .element_ref(&list)
            .estimated_item_size(px(40.0))
            .content_padding(px(8.0))
            .w_full();
            scroll()
                .element_ref(&viewport)
                .role(AccessibilityRole::Listbox)
                .label(move || SharedString::from(t("Command palette")))
                .accessibility_state(move || nsh_core::AccessibilityState {
                    busy: state.loading.get(),
                    ..Default::default()
                })
                // Panel allocation supplies max-h-[inherit]; native scrolling replaces scroll-py/track hints.
                .app_class(tw(
                    "size-full min-w-0 flex min-h-0 flex-col overflow-x-hidden",
                ))
                .flex_shrink(1.0)
                .child(rows)
                .into_element()
        })
    })
}

fn render_row(state: CommandState, row: Memo<Option<Row>>, keys: Keybindings) -> impl IntoElement {
    let kind = computed(move || {
        row.with(|row| match row {
            Some(Row::Label { .. }) => 0,
            Some(Row::Item { item, .. }) if item.disabled => 1,
            Some(Row::Item { .. }) => 2,
            None => 3,
        })
    });
    let item = computed(move || {
        row.with(|row| match row {
            Some(Row::Item { item, .. }) => Some(Shared(item.clone())),
            _ => None,
        })
    });
    let option = computed(move || {
        item.with(|item| {
            item.as_ref().map(|item| {
                let mut option = command::Item::new(item.value.clone(), title(item));
                option.disabled = item.disabled;
                option
            })
        })
    });
    dynamic(move || {
        let kind = kind.get();
        untracked(|| match kind {
            0 => view()
                .app_class(move || {
                    tw(&cn([row
                        .with(|row| matches!(row, Some(Row::Label { first: false, .. })))
                        .then_some("pt-1.5")]))
                })
                .child(
                    text(move || {
                        row.with(|row| match row {
                            Some(Row::Label { label, .. }) => {
                                Text::client_key(label.clone()).resolve()
                            }
                            _ => SharedString::default(),
                        })
                    })
                    .app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs")),
                )
                .into_element(),
            1 => disabled_command_palette_result_row(item).into_element(),
            2 => command_palette_result_row(state, item, option, keys).into_element(),
            _ => view().into_element(),
        })
    })
}

pub fn command_palette_title_icon(children: impl IntoElement) -> impl IntoElement {
    view()
        .app_class(tw("flex h-5 shrink-0 items-center self-start pt-0.5"))
        .child(children)
}

fn title(item: &Item) -> Text {
    if item.styled_title
        || item.thread.is_some()
        || item.project.is_some()
        || item.value.starts_with("browse:")
        || item.value.starts_with("theme:")
    {
        Text::content(item.title.clone())
    } else {
        Text::client_key(item.title.clone())
    }
}

fn command_palette_title(item: Memo<Option<Shared<Item>>>) -> impl IntoElement {
    text(move || {
        item.with(|item| {
            item.as_ref()
                .map(|item| title(item).resolve())
                .unwrap_or_default()
        })
    })
    .app_class(tw("truncate"))
}

fn thread_row_context(item: Memo<Option<Shared<Item>>>) -> impl IntoElement {
    text(move || {
        item.with(|item| {
            let Some(item) = item else {
                return String::new();
            };
            let Some(subject) = item
                .thread_search
                .as_ref()
                .filter(|_| item.match_hint.is_some())
            else {
                return String::new();
            };
            [
                subject.project_title.as_deref(),
                subject.environment_label.as_deref(),
            ]
            .into_iter()
            .flatten()
            .filter(|part| {
                !part.is_empty()
                    && item
                        .match_hint
                        .as_ref()
                        .is_none_or(|hint| hint.text != *part)
            })
            .collect::<Vec<_>>()
            .join(" · ")
        })
    })
    .app_class(tw(
        "max-w-[45%] shrink-0 truncate text-muted-foreground/70 text-xs",
    ))
}

fn row_contents(item: Memo<Option<Shared<Item>>>) -> impl IntoElement {
    view()
        .app_class(move || {
            tw(&cn([
                Some("flex min-w-0 flex-1"),
                item.with(|item| item.as_ref().is_some_and(|item| item.description.is_some()))
                    .then_some("flex-col"),
            ]))
        })
        .child(
            view()
                .app_class(tw(
                    "flex min-w-0 items-center gap-1.5 text-sm text-foreground",
                ))
                .child(command_palette_title(item))
                .child(thread_row_context(item)),
        )
        .child(dynamic(move || {
            let item = item.get()?;
            if let Some(hint) = &item.match_hint {
                let label = Text::client_key(hint.label.clone());
                let value = hint.text.clone();
                return Some(
                    view()
                        .app_class(tw("truncate text-xs text-muted-foreground/85"))
                        .child(
                            text(move || format!("{}: ", label.resolve()))
                                .app_class(tw("text-muted-foreground/70")),
                        )
                        // Client's hint ranges need a shared text-parts projection; content remains intact.
                        .child(text(value))
                        .into_element(),
                );
            }
            let description = item.description.clone()?;
            let description = if item.thread.is_some() || item.project.is_some() {
                Text::content(description)
            } else {
                Text::client_key(description)
            };
            Some(
                text(move || description.resolve())
                    .app_class(tw("min-w-0 text-muted-foreground/70 text-xs"))
                    .into_element(),
            )
        }))
}

fn disabled_command_palette_result_row(item: Memo<Option<Shared<Item>>>) -> impl IntoElement {
    view().app_class(tw("flex min-h-8 select-none items-center gap-2 rounded-sm px-2 py-1.5 text-base opacity-64 sm:min-h-7 sm:text-sm"))
        .child(item_icon(item)).child(row_contents(item))
}

fn command_palette_result_row(
    state: CommandState,
    item: Memo<Option<Shared<Item>>>,
    option: Memo<Option<command::Item>>,
    keys: Keybindings,
) -> impl IntoElement {
    command::command_item(
        state,
        option,
        move || {
            Some(item.with(|item| {
                item.as_ref()
                    .is_some_and(|item| state.highlighted.get().as_deref() == Some(&item.value))
            }))
        },
        view()
            .app_class(tw("flex min-w-0 flex-1 items-center gap-2"))
            .child(item_icon(item))
            .child(row_contents(item))
            // CommandShortcut's String slot translates again; formatted shortcuts are content.
            .child(
                text(move || {
                    item.with(|item| {
                        item.as_ref()
                            .and_then(|item| item.shortcut.as_deref())
                            .and_then(|command| keys.shortcut_label(command))
                    })
                    .unwrap_or_default()
                })
                .app_class(tw(
                    "ms-auto font-medium font-sans text-secondary-label text-xs tracking-widest",
                )),
            )
            .child(when(
                move || item.with(|item| item.as_ref().is_some_and(|item| item.groups.is_some())),
                || {
                    icon_with_class(
                        Icon::ChevronRight,
                        "-me-0.5 ms-auto size-4 shrink-0 text-muted-foreground/70",
                    )
                },
            )),
    )
}

fn item_icon(item: Memo<Option<Shared<Item>>>) -> impl IntoElement {
    command_palette_title_icon(dynamic(move || {
        let item = item.get()?;
        result_icon(&item).map(|icon| icon_with_class(icon, "size-4 text-icon-muted"))
    }))
}

fn result_icon(item: &Item) -> Option<Icon> {
    if item.thread.is_some() {
        return Some(Icon::MessageSquare);
    }
    if item.value == "browse:up" {
        return Some(Icon::CornerLeftUp);
    }
    if item.value.starts_with("browse:") {
        return Some(Icon::Folder);
    }
    if item.value.starts_with("setting:") {
        return Some(Icon::Settings);
    }
    if item.value.starts_with("theme:") {
        return Some(Icon::Palette);
    }
    match item.value.as_str() {
        "action:new-thread" | "action:new-thread-in" => Some(Icon::SquarePen),
        "new-thread-in:no-project" | "action:new-thread-without-project" => {
            Some(Icon::MessageSquareDashed)
        }
        "action:triage" => Some(Icon::Bug),
        "find-current-thread" | "find-in-browser-page" | "action:search-project-contents" => {
            Some(Icon::TextSearch)
        }
        "action:copy-prompt-link"
        | "action:copy-prompt-link-with-context"
        | "action:copy-thread-reference" => Some(Icon::Link),
        "action:share-thread-with-guest" => Some(Icon::UserPlus),
        "action:link-pull-request" | "action:open-thread-pull-requests" => {
            Some(Icon::GitPullRequest)
        }
        "action:project-pin" => Some(if item.pinned == Some(true) {
            Icon::PinOff
        } else {
            Icon::Pin
        }),
        "action:restart-agent-session" => Some(Icon::RotateCcw),
        "action:open-file-picker" => Some(Icon::FileSearch),
        "action:new-project" => Some(Icon::FolderGit2),
        "action:add-project" | "action:add-project:wsl-folder" => Some(Icon::FolderPlus),
        "action:change-theme" | "action:theme-editor" => Some(Icon::Palette),
        "action:change-appearance"
        | "appearance:system"
        | "action:workspace-focus:1"
        | "action:workspace-focus:-1" => Some(Icon::Monitor),
        "appearance:light" => Some(Icon::Sun),
        "appearance:dark" => Some(Icon::Moon),
        "action:use-in-another-app" => Some(Icon::Plug),
        "action:usage" => Some(Icon::ChartNoAxesColumn),
        "action:settings" | "action:add-project:connect-environment" => Some(Icon::Settings),
        "action:project-settings" => Some(Icon::Folder),
        _ => None,
    }
}
