//! Port of apps/web/src/components/ThreadTerminalDrawer.tsx ThreadTerminalDrawer, with
//! `PersistentThreadTerminals.tsx` PersistentThreadTerminalDrawer's wiring (composed by the
//! client's `TerminalSessions::drawer`) and the drawer's commands from
//! `components/chat/useChatTerminalActions.ts`.
//!
//! Departures, each forced by the tree:
//! - The top edge is NSH's `resize_handle` (native pointer capture and touch hit area) where the
//!   web draws its own `h-1.5 any-pointer-coarse:h-4` band.
//! - The height is capped by the window only: the chat column's height and the composer's
//!   overlay are the thread screen's to report, and it reports neither yet.
//! - A closed drawer unmounts its panes (their sessions detach and resume, with a replay, when it
//!   reopens) where the web keeps them mounted at zero height; hidden threads keep no drawer.
//! - The split panes are flex rows or columns of equal panes where the web uses a CSS grid of
//!   `repeat(n, minmax(0, 1fr))`, and `first:border-*-0` is decided by index.
use futures::StreamExt;
use nsh_core::prelude::*;
use std::rc::Rc;
use t3_client::{
    services::terminal_sessions::{Placement, TerminalDrawerView},
    types::ThreadKey,
    view_logic::session::terminal_drawer::{
        ActionLabel, CloseConfirmation, DrawerDrag, DrawerLayout, DrawerPresentation, GroupKind,
        ResolvedGroup, TerminalLabel, action_label, clamp_drawer_height, close_confirmation,
        present_terminal_drawer, resolve_terminal_drawer_max_height, split_action_label,
    },
};

use super::session::terminal_viewport;
use crate::{
    i18n::{Value, t, tr},
    keys::bindings::{Keybindings, Scope},
    runtime::services::terminal::TerminalServices,
    theme::{cn, tw},
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        confirm::{ConfirmDialogOptions, ConfirmDialogVariant, request_confirm_dialog},
        icon::{Icon, icon_with_class},
        tabs::panel_tab_close_button,
        tooltip::{TooltipOptions, tooltip as hint, tooltip_create_handle},
    },
};

/// A terminal's name for display.
pub fn terminal_label_text(label: &TerminalLabel) -> String {
    match label {
        TerminalLabel::Numbered(number) => {
            tr("Terminal {number}", &[("number", Value::Text(number.clone()))])
        }
        TerminalLabel::Text(text) => text.clone(),
    }
}

fn split_label(label: ActionLabel, vertical: bool) -> String {
    match (label, vertical) {
        (ActionLabel::Limit(max), false) => tr(
            "Split Terminal Horizontally (max {max} per group)",
            &[("max", Value::Int(max as i64))],
        ),
        (ActionLabel::Limit(max), true) => tr(
            "Split Terminal Vertically (max {max} per group)",
            &[("max", Value::Int(max as i64))],
        ),
        (ActionLabel::Shortcut(shortcut), false) => tr(
            "Split Terminal Horizontally ({shortcut})",
            &[("shortcut", Value::Text(shortcut))],
        ),
        (ActionLabel::Shortcut(shortcut), true) => tr(
            "Split Terminal Vertically ({shortcut})",
            &[("shortcut", Value::Text(shortcut))],
        ),
        (ActionLabel::Plain, false) => t("Split Terminal Horizontally"),
        (ActionLabel::Plain, true) => t("Split Terminal Vertically"),
    }
}

fn new_label(label: ActionLabel) -> String {
    match label {
        ActionLabel::Shortcut(shortcut) => {
            tr("New Terminal ({shortcut})", &[("shortcut", Value::Text(shortcut))])
        }
        _ => t("New Terminal"),
    }
}

fn close_label(label: ActionLabel) -> String {
    match label {
        ActionLabel::Shortcut(shortcut) => {
            tr("Close Terminal ({shortcut})", &[("shortcut", Value::Text(shortcut))])
        }
        _ => t("Close Terminal"),
    }
}

fn close_tab_label(label: &str, shortcut: Option<&str>) -> String {
    match shortcut {
        Some(shortcut) => tr(
            "Close {label} ({shortcut})",
            &[
                ("label", Value::Text(label.into())),
                ("shortcut", Value::Text(shortcut.into())),
            ],
        ),
        None => tr("Close {label}", &[("label", Value::Text(label.into()))]),
    }
}

/// `lib/terminalCloseConfirm.ts`: the question, title and description on two lines.
fn close_confirmation_message(confirmation: &CloseConfirmation) -> String {
    match confirmation {
        CloseConfirmation::One(label) => format!(
            "{}\n{}",
            tr("Close terminal \"{label}\"?", &[("label", Value::Text(label.clone()))]),
            t("This stops the running process and clears its history.")
        ),
        CloseConfirmation::Many(labels) => format!(
            "{}\n{}",
            tr("Close {count} terminals?", &[("count", Value::Int(labels.len() as i64))]),
            tr(
                "This stops their running processes and clears their histories: {labels}.",
                &[(
                    "labels",
                    Value::Text(
                        labels
                            .iter()
                            .map(|label| format!("\"{label}\""))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                )]
            )
        ),
    }
}

/// The drawer's state and actions, shared by its parts.
#[derive(Clone)]
struct Drawer {
    services: TerminalServices,
    key: ThreadKey,
    view: Signal<Rc<TerminalDrawerView>>,
    presentation: Memo<Rc<DrawerPresentation>>,
}

impl Drawer {
    fn can_operate(&self) -> bool {
        self.view.with(|view| view.can_operate)
    }
    fn label(&self, terminal: &str) -> String {
        terminal_label_text(&self.view.with_untracked(|view| view.label(terminal)))
    }
    fn launch(&self, placement: Placement) {
        if !self.services.0.can_operate(&self.key) {
            return;
        }
        if let Some((_, opened)) = self.services.0.launch(&self.key, placement, None) {
            spawn(async move {
                let _ = opened.await;
            });
        }
    }
    fn split(&self, vertical: bool) {
        if !self.can_operate() || self.presentation.with(|p| p.split_limit_reached) {
            return;
        }
        self.launch(if vertical {
            Placement::SplitVertical
        } else {
            Placement::Split
        });
    }
    fn activate(&self, terminal: &str) {
        self.services.0.activate(&self.key, terminal);
    }
    /// Closes without asking: the shell already ended.
    fn close(&self, terminal: &str) {
        if let Some(closing) = self.services.0.close_terminal(&self.key, terminal) {
            spawn(closing);
        }
    }
    fn confirm_close(&self, terminal: &str) {
        if !self.can_operate() {
            return;
        }
        let Some(confirmation) = close_confirmation(vec![self.label(terminal)]) else {
            return;
        };
        let answer = request_confirm_dialog(
            close_confirmation_message(&confirmation),
            Some(ConfirmDialogOptions {
                variant: ConfirmDialogVariant::Destructive,
                initial_focus: None,
            }),
        );
        let drawer = self.clone();
        let terminal = terminal.to_owned();
        spawn(async move {
            // Without a dialog host the web closes at once, as without its local API.
            let confirmed = match answer {
                Some(answer) => answer.await,
                None => true,
            };
            if confirmed && drawer.services.0.can_operate(&drawer.key) {
                drawer.close(&terminal);
            }
        });
    }
}

/// The drawer of the thread `key`: shown while its drawer is open and its project resolves.
pub fn thread_terminal_drawer(services: TerminalServices, key: ThreadKey) -> impl IntoElement {
    {
        let services = services.clone();
        spawn(async move {
            let _ = services.0.load().await;
        });
    }
    let lease = Rc::new(services.0.drawer(&key, None));
    let state = signal(Rc::new(lease.view()));
    {
        let lease = lease.clone();
        spawn(async move {
            let mut changes = lease.changes();
            while changes.next().await.is_some() {
                let next = lease.view();
                if state.with_untracked(|current| **current != next) {
                    state.set(Rc::new(next));
                }
            }
        });
    }
    let presentation = computed(move || {
        state.with(|view| {
            Rc::new(present_terminal_drawer(
                &view.terminal_ids,
                &view.ui.active_terminal_id,
                &view.ui.terminal_groups,
                &view.ui.active_terminal_group_id,
            ))
        })
    });
    let drawer = Drawer {
        services,
        key,
        view: state,
        presentation,
    };
    let keys = use_context::<Keybindings>();
    if let Some(keys) = keys {
        register_commands(keys, drawer.clone());
    }
    let shown = computed(move || {
        state.with(|view| view.ui.terminal_open && view.project_root.is_some() && view.cwd.is_some())
    });
    let _lease = lease;
    view().app_class(tw("flex shrink-0 flex-col")).child(dynamic(move || {
        let _ = &_lease;
        shown
            .get()
            .then(|| untracked(|| drawer_body(drawer.clone(), keys).into_element()))
    }))
}

/// `useChatTerminalActions.ts`'s drawer chords, while the drawer is mounted.
fn register_commands(keys: Keybindings, drawer: Drawer) {
    let open_if_closed = {
        let drawer = drawer.clone();
        move || {
            if !drawer.view.with_untracked(|view| view.ui.terminal_open) {
                drawer.services.0.set_open(&drawer.key, true);
            }
        }
    };
    let toggle = drawer.clone();
    let (split, split_open) = (drawer.clone(), open_if_closed.clone());
    let (vertical, vertical_open) = (drawer.clone(), open_if_closed.clone());
    let (new, new_open) = (drawer.clone(), open_if_closed);
    let close = drawer;
    keys.register(
        Scope::default()
            .command("terminal.toggle", move |event| {
                let open = toggle.view.with_untracked(|view| view.ui.terminal_open);
                if !open
                    && !toggle.services.0.can_read(&toggle.key)
                    && !toggle.services.0.can_operate(&toggle.key)
                {
                    return false;
                }
                event.claim();
                toggle.services.0.toggle(&toggle.key);
                true
            })
            .command("terminal.split", move |event| {
                if !split.services.0.can_operate(&split.key) {
                    return false;
                }
                event.claim();
                split_open();
                split.launch(Placement::Split);
                true
            })
            .command("terminal.splitVertical", move |event| {
                if !vertical.services.0.can_operate(&vertical.key) {
                    return false;
                }
                event.claim();
                vertical_open();
                vertical.launch(Placement::SplitVertical);
                true
            })
            .command("terminal.new", move |event| {
                if !new.services.0.can_operate(&new.key) {
                    return false;
                }
                event.claim();
                new_open();
                new.launch(Placement::New);
                true
            })
            .command("terminal.close", move |event| {
                if !close.services.0.can_operate(&close.key) {
                    return false;
                }
                event.claim();
                let (open, active) = close
                    .view
                    .with_untracked(|view| (view.ui.terminal_open, view.ui.active_terminal_id.clone()));
                if open {
                    close.confirm_close(&active);
                }
                true
            }),
    );
}

fn drawer_body(drawer: Drawer, keys: Option<Keybindings>) -> impl IntoElement {
    let shortcut = move |command: &'static str| move || keys.and_then(|keys| keys.shortcut_label(command));
    let split_shortcut = shortcut("terminal.split");
    let vertical_shortcut = shortcut("terminal.splitVertical");
    let new_shortcut = shortcut("terminal.new");
    let close_shortcut = shortcut("terminal.close");
    let presentation = drawer.presentation;
    let limit = move || presentation.with(|p| p.split_limit_reached);
    let labels = ActionLabels {
        split: Rc::new(move || split_label(split_action_label(limit(), split_shortcut().as_deref()), false)),
        vertical: Rc::new(move || split_label(split_action_label(limit(), vertical_shortcut().as_deref()), true)),
        new: Rc::new(move || new_label(action_label(new_shortcut().as_deref()))),
        close: Rc::new(move || close_label(action_label(close_shortcut().as_deref()))),
        close_shortcut: Rc::new(close_shortcut),
    };

    let window_height = move || f64::from(window().height.0);
    let drag_height = signal(None::<f64>);
    let drag = stored(None::<DrawerDrag>);
    let state = drawer.view;
    let max_height = move || {
        resolve_terminal_drawer_max_height(&DrawerLayout {
            viewport_height: window_height(),
            ..Default::default()
        })
    };
    let drawer_height = move || clamp_drawer_height(state.with(|v| v.ui.terminal_height), max_height());
    let height = move || px(drag_height.get().unwrap_or_else(drawer_height) as f32);

    let aside = element_ref();
    if let Some(keys) = keys {
        keys.context.focus_within(aside, "terminalFocus");
    }
    let resize_drawer = drawer.clone();
    let resize_handle = nsh_base::resize_handle()
        .axis(nsh_core::geometry::Axis::Vertical)
        .edge(Some(nsh_base::HandleEdge::Leading))
        .accessibility_label(Some(SharedString::from(t("Resize terminal"))))
        .on_drag_start(move || {
            drag.replace(Some(DrawerDrag::start(0.0, drawer_height())));
        })
        .on_drag_move(move |offset: &Points| {
            let moved = drag.with(|drag| {
                drag.as_mut()
                    .and_then(|drag| drag.move_to(f64::from(offset.0), max_height()))
            });
            if let Some(height) = moved {
                drag_height.set(Some(height));
            }
        })
        .on_drag_end(move |offset: &Points| {
            let ended = drag.replace(None);
            drag_height.set(None);
            let Some(mut ended) = ended else { return };
            ended.move_to(f64::from(offset.0), max_height());
            let stored = state.with_untracked(|v| v.ui.terminal_height);
            if let Some(height) = ended.end(stored) {
                resize_drawer.services.0.set_height(&resize_drawer.key, height);
            }
        });

    let empty_drawer = drawer.clone();
    let filled_drawer = drawer.clone();
    let has_terminals = computed(move || presentation.with(|p| !p.terminal_ids.is_empty()));
    view()
        .element_ref(&aside)
        .test_id("thread-terminal-drawer")
        .app_class(tw("relative flex min-w-0 flex-col overflow-hidden bg-background z-45 shrink-0 border-t border-border/80"))
        .h(height)
        .child(resize_handle)
        .child(dynamic(move || {
            if has_terminals.get() {
                let (drawer, labels) = (filled_drawer.clone(), labels.clone());
                untracked(|| terminal_panes(drawer, labels).into_element())
            } else {
                let drawer = empty_drawer.clone();
                let new = labels.new.clone();
                untracked(|| empty_state(drawer, new).into_element())
            }
        }))
}

#[derive(Clone)]
struct ActionLabels {
    split: Rc<dyn Fn() -> String>,
    vertical: Rc<dyn Fn() -> String>,
    new: Rc<dyn Fn() -> String>,
    close: Rc<dyn Fn() -> String>,
    close_shortcut: Rc<dyn Fn() -> Option<String>>,
}

fn empty_state(drawer: Drawer, label: Rc<dyn Fn() -> String>) -> impl IntoElement {
    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-4 py-6 text-center text-sm text-muted-foreground"))
        .child(text(t("No terminal sessions for this thread yet.")))
        .child(dynamic(move || {
            let drawer = drawer.clone();
            let label = label();
            let disabled = drawer.clone();
            untracked(move || {
                button(
                    ButtonContent::Label(label),
                    ButtonVariant::Outline,
                    ButtonSize::Xs,
                    move || !disabled.can_operate(),
                    move || {
                        if drawer.can_operate() {
                            drawer.launch(Placement::New);
                        }
                    },
                )
                .into_element()
            })
        }))
}

fn terminal_panes(drawer: Drawer, labels: ActionLabels) -> impl IntoElement {
    let presentation = drawer.presentation;
    let has_sidebar = computed(move || presentation.with(|p| p.has_sidebar));
    let bar_drawer = drawer.clone();
    let bar_labels = labels.clone();
    let sidebar_drawer = drawer.clone();
    view()
        .app_class(tw("flex min-h-0 w-full flex-1 flex-col"))
        .child(dynamic(move || {
            (!has_sidebar.get()).then(|| {
                let (drawer, labels) = (bar_drawer.clone(), bar_labels.clone());
                untracked(|| floating_actions(drawer, labels).into_element())
            })
        }))
        .child(
            view().app_class(tw("min-h-0 w-full flex-1")).child(
                view()
                    .app_class(move || {
                        Some(tw(&cn([
                            Some("flex h-full min-h-0 bg-(--terminal-background)"),
                            has_sidebar.get().then_some("gap-1.5"),
                        ])))
                    })
                    .child(
                        view()
                            .app_class(tw("min-w-0 flex-1"))
                            .child(terminal_split_panes(drawer)),
                    )
                    .child(dynamic(move || {
                        has_sidebar.get().then(|| {
                            let (drawer, labels) = (sidebar_drawer.clone(), labels.clone());
                            untracked(|| terminal_sidebar(drawer, labels).into_element())
                        })
                    })),
            ),
        )
}

/// Port of TerminalActionButton: an icon button whose label is its tooltip.
fn terminal_action_button(
    label: Rc<dyn Fn() -> String>,
    class: impl Fn() -> String + 'static,
    disabled: impl Fn() -> bool + 'static,
    on_click: impl Fn() + 'static,
    glyph: Icon,
) -> impl IntoElement {
    let disabled = Rc::new(disabled);
    let (dimmed, inert) = (disabled.clone(), disabled);
    let (accessible, tooltip) = (label.clone(), label);
    nsh_base::button()
        .accessibility_label(move || Some(SharedString::from(accessible())))
        .on_press(move || {
            if !inert() {
                on_click();
            }
        })
        .app_class(move || {
            Some(tw(&cn([
                Some(class().as_str()),
                dimmed().then_some("opacity-64 cursor-not-allowed"),
            ])))
        })
        .child(icon_with_class(glyph, "size-3.25"))
        .child(hint(
            tooltip_create_handle(),
            TooltipOptions {
                side: nsh_core::prelude::Placement::Bottom,
                side_offset: 6.0,
                ..Default::default()
            },
            text(move || tooltip()),
        ))
}

fn floating_actions(drawer: Drawer, labels: ActionLabels) -> impl IntoElement {
    let presentation = drawer.presentation;
    let limit = move || presentation.with(|p| p.split_limit_reached);
    let split_class = move || {
        cn([
            Some("p-1 text-foreground/90 transition-colors"),
            Some(if limit() {
                "cursor-not-allowed opacity-64 hover:bg-transparent"
            } else {
                "hover:bg-accent"
            }),
        ])
    };
    let (a, b, c, d) = (drawer.clone(), drawer.clone(), drawer.clone(), drawer.clone());
    let (da, db, dc, dd) = (drawer.clone(), drawer.clone(), drawer.clone(), drawer);
    view()
        .app_class(tw("pointer-events-none absolute right-2 top-2 z-20"))
        .child(
            view()
                .app_class(tw("pointer-events-auto inline-flex items-center overflow-hidden rounded-md border border-border/80 bg-background shadow-xs"))
                .child(terminal_action_button(
                    labels.split.clone(),
                    split_class,
                    move || !da.can_operate(),
                    move || a.split(false),
                    Icon::SquareSplitHorizontal,
                ))
                .child(view().app_class(tw("h-4 w-px bg-border/80")))
                .child(terminal_action_button(
                    labels.vertical.clone(),
                    split_class,
                    move || !db.can_operate(),
                    move || b.split(true),
                    Icon::SquareSplitVertical,
                ))
                .child(view().app_class(tw("h-4 w-px bg-border/80")))
                .child(terminal_action_button(
                    labels.new.clone(),
                    || "p-1 text-foreground/90 transition-colors hover:bg-accent".into(),
                    move || !dc.can_operate(),
                    move || {
                        if c.can_operate() {
                            c.launch(Placement::New);
                        }
                    },
                    Icon::Plus,
                ))
                .child(view().app_class(tw("h-4 w-px bg-border/80")))
                .child(terminal_action_button(
                    labels.close.clone(),
                    || "p-1 text-foreground/90 transition-colors hover:bg-accent".into(),
                    move || !dd.can_operate(),
                    move || {
                        let active = d.presentation.with_untracked(|p| p.active_terminal_id.clone());
                        d.confirm_close(&active);
                    },
                    Icon::Trash2,
                )),
        )
}

#[derive(Clone, PartialEq)]
struct Pane {
    terminal: String,
    label: String,
    active: bool,
    launch: Option<t3_client::services::terminal_sessions::TerminalLaunch>,
}

/// Port of TerminalSplitPanes: the active group's terminals side by side or stacked.
fn terminal_split_panes(drawer: Drawer) -> impl IntoElement {
    let presentation = drawer.presentation;
    let view_signal = drawer.view;
    let panes = computed(move || {
        let presentation = presentation.get();
        view_signal.with(|view| {
            presentation
                .visible_terminal_ids
                .iter()
                .map(|terminal| Pane {
                    terminal: terminal.clone(),
                    label: terminal_label_text(&view.label(terminal)),
                    active: *terminal == presentation.active_terminal_id,
                    launch: view.launch(terminal),
                })
                .collect::<Vec<_>>()
        })
    });
    let vertical = computed(move || presentation.with(|p| p.vertical));
    view()
        .app_class(move || {
            Some(tw(if vertical.get() {
                "flex flex-col h-full w-full min-w-0 gap-0 overflow-hidden"
            } else {
                "flex h-full w-full min-w-0 gap-0 overflow-hidden"
            }))
        })
        .child(each(
            move || panes.get(),
            // A new launch (directory, worktree or environment) is a new session.
            |pane| format!("{}\u{0}{:?}", pane.terminal, pane.launch),
            move |pane, index| {
                let drawer = drawer.clone();
                let first = pane.get_untracked();
                let activate = drawer.clone();
                let terminal = first.terminal.clone();
                let exited = {
                    let drawer = drawer.clone();
                    let terminal = terminal.clone();
                    Rc::new(move || drawer.close(&terminal)) as Rc<dyn Fn()>
                };
                let content = match first.launch {
                    Some(launch) => terminal_viewport(
                        drawer.services.clone(),
                        drawer.key.clone(),
                        terminal.clone(),
                        first.label.clone(),
                        launch,
                        exited,
                    )
                    .into_element(),
                    None => view().into_element(),
                };
                view()
                    .app_class(move || {
                        let (active, first) = (pane.with(|p| p.active), index.get() == 0);
                        Some(tw(&cn([
                            Some("min-h-0 min-w-0 flex-1"),
                            (!first).then_some(if vertical.get() { "border-t" } else { "border-l" }),
                            Some(if active { "border-border" } else { "border-border/70" }),
                        ])))
                    })
                    .on_press_down(move || {
                        if !pane.with_untracked(|p| p.active) {
                            activate.activate(&terminal);
                        }
                    })
                    .child(view().app_class(tw("h-full")).child(content))
            },
        ))
}

fn terminal_sidebar(drawer: Drawer, labels: ActionLabels) -> impl IntoElement {
    let presentation = drawer.presentation;
    let limit = move || presentation.with(|p| p.split_limit_reached);
    let split_class = move |bordered: bool| {
        move || {
            cn([
                Some(if bordered {
                    "inline-flex h-full items-center border-l border-border/70 px-1 text-foreground/90 transition-colors"
                } else {
                    "inline-flex h-full items-center px-1 text-foreground/90 transition-colors"
                }),
                Some(if limit() {
                    "cursor-not-allowed opacity-64 hover:bg-transparent"
                } else {
                    "hover:bg-accent/70"
                }),
            ])
        }
    };
    let (a, b, c, d) = (drawer.clone(), drawer.clone(), drawer.clone(), drawer.clone());
    let (da, db, dc, dd) = (drawer.clone(), drawer.clone(), drawer.clone(), drawer.clone());
    let groups = computed(move || presentation.with(|p| p.groups.clone()));
    let close_shortcut = labels.close_shortcut.clone();
    view()
        .app_class(tw("flex w-36 min-w-36 flex-col border border-border/70 bg-muted/10"))
        .child(
            view()
                .app_class(tw("flex h-[22px] items-stretch justify-end border-b border-border/70"))
                .child(
                    // Sits above the taller touch resize handle so its buttons keep their taps.
                    view()
                        .app_class(tw("inline-flex h-full items-stretch any-pointer-coarse:relative any-pointer-coarse:z-30"))
                        .child(terminal_action_button(
                            labels.split.clone(),
                            split_class(false),
                            move || !da.can_operate(),
                            move || a.split(false),
                            Icon::SquareSplitHorizontal,
                        ))
                        .child(terminal_action_button(
                            labels.vertical.clone(),
                            split_class(true),
                            move || !db.can_operate(),
                            move || b.split(true),
                            Icon::SquareSplitVertical,
                        ))
                        .child(terminal_action_button(
                            labels.new.clone(),
                            || "inline-flex h-full items-center border-l border-border/70 px-1 text-foreground/90 transition-colors hover:bg-accent/70".into(),
                            move || !dc.can_operate(),
                            move || {
                                if c.can_operate() {
                                    c.launch(Placement::New);
                                }
                            },
                            Icon::Plus,
                        ))
                        .child(terminal_action_button(
                            labels.close.clone(),
                            || "inline-flex h-full items-center border-l border-border/70 px-1 text-foreground/90 transition-colors hover:bg-accent/70".into(),
                            move || !dd.can_operate(),
                            move || {
                                let active = d.presentation.with_untracked(|p| p.active_terminal_id.clone());
                                d.confirm_close(&active);
                            },
                            Icon::Trash2,
                        )),
                ),
        )
        .child(
            scroll().app_class(tw("min-h-0 flex-1 px-1 py-1")).child(each(
                move || groups.get(),
                |group| group.id.clone(),
                move |group, _| {
                    terminal_sidebar_group(drawer.clone(), group, close_shortcut.clone())
                },
            )),
        )
}

/// Port of TerminalSidebarGroup.
fn terminal_sidebar_group(
    drawer: Drawer,
    group: ReadSignal<ResolvedGroup>,
    close_shortcut: Rc<dyn Fn() -> Option<String>>,
) -> impl IntoElement {
    let presentation = drawer.presentation;
    let active = move || presentation.with(|p| p.active_terminal_id.clone());
    let is_group_active = move || group.with(|g| g.terminal_ids.contains(&active()));
    let header_drawer = drawer.clone();
    let show_header = computed(move || presentation.with(|p| p.show_group_headers));
    let rows = computed(move || group.with(|g| g.terminal_ids.clone()));
    view()
        .app_class(tw("pb-0.5"))
        .child(dynamic(move || {
            show_header.get().then(|| {
                let drawer = header_drawer.clone();
                untracked(move || {
                    let target = drawer.clone();
                    nsh_base::button()
                        .accessibility_label(move || Some(SharedString::from(group_label(group.with(ResolvedGroup::kind)))))
                        .on_press(move || {
                            let terminal = group.with_untracked(|g| g.active_terminal(&active()).to_owned());
                            target.activate(&terminal);
                        })
                        .app_class(move || {
                            Some(tw(&cn([
                                Some("flex h-[22px] w-full cursor-pointer items-center gap-1 rounded px-1.5 text-2xs"),
                                Some(if is_group_active() {
                                    "bg-accent/50 text-foreground"
                                } else {
                                    "text-muted-foreground hover:bg-accent/40 hover:text-foreground"
                                }),
                            ])))
                        })
                        .child(dynamic(move || {
                            Some(icon_with_class(
                                match group.with(ResolvedGroup::kind) {
                                    GroupKind::Single => Icon::Square,
                                    GroupKind::Stacked => Icon::SquareSplitVertical,
                                    GroupKind::SideBySide => Icon::SquareSplitHorizontal,
                                },
                                "size-3 shrink-0",
                            ))
                        }))
                        .child(
                            text(move || group_label(group.with(ResolvedGroup::kind)))
                                .app_class(tw("min-w-0 flex-1 truncate text-left")),
                        )
                        .child(
                            text(move || group.with(|g| g.terminal_ids.len()).to_string())
                                .app_class(tw("text-muted-foreground/70 text-3xs tabular-nums")),
                        )
                        .into_element()
                })
            })
        }))
        .child(
            view().app_class(tw("flex flex-col gap-0.5")).child(each(
                move || rows.get(),
                |terminal| terminal.clone(),
                move |terminal, _| {
                    terminal_sidebar_row(drawer.clone(), terminal.get_untracked(), close_shortcut.clone())
                },
            )),
        )
}

fn group_label(kind: GroupKind) -> String {
    match kind {
        GroupKind::Single => t("Single"),
        GroupKind::Stacked => t("Stacked"),
        GroupKind::SideBySide => t("Side by side"),
    }
}

fn terminal_sidebar_row(
    drawer: Drawer,
    terminal: String,
    close_shortcut: Rc<dyn Fn() -> Option<String>>,
) -> impl IntoElement {
    let presentation = drawer.presentation;
    let view_signal = drawer.view;
    let id = terminal.clone();
    let is_active = computed(move || presentation.with(|p| p.active_terminal_id == id));
    let id = terminal.clone();
    let label = computed(move || view_signal.with(|view| terminal_label_text(&view.label(&id))));
    let can_operate = computed(move || view_signal.with(|view| view.can_operate));
    let (close_drawer, select_drawer) = (drawer.clone(), drawer);
    let close_id = terminal.clone();
    view()
        .app_class(move || {
            Some(tw(&cn([
                Some("group/tab flex h-6 w-full items-center gap-0.5 rounded-md pr-2 pl-1.5 text-xs"),
                Some(if is_active.get() {
                    "bg-accent text-foreground"
                } else {
                    "text-muted-foreground hover:bg-accent/60 hover:text-foreground"
                }),
            ])))
        })
        .child(dynamic(move || {
            let glyph = icon_with_class(Icon::SquareTerminal, "size-3 shrink-0").into_element();
            if !can_operate.get() {
                return glyph;
            }
            let shortcut = is_active.get().then(|| close_shortcut()).flatten();
            let close_label = close_tab_label(&label.get(), shortcut.as_deref());
            let (drawer, terminal) = (close_drawer.clone(), close_id.clone());
            untracked(move || {
                panel_tab_close_button(
                    glyph,
                    close_label.clone(),
                    move || drawer.confirm_close(&terminal),
                    Some(close_label),
                )
                .into_element()
            })
        }))
        .child(
            nsh_base::button()
                .accessibility_label(move || Some(SharedString::from(label.get())))
                .on_press(move || select_drawer.activate(&terminal))
                .app_class(tw("flex min-w-0 flex-1 cursor-pointer items-center gap-1 text-left"))
                .child(text(move || label.get()).app_class(tw("truncate"))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_web_copy() {
        assert_eq!(terminal_label_text(&TerminalLabel::Numbered("3".into())), "Terminal 3");
        assert_eq!(terminal_label_text(&TerminalLabel::Text("dev".into())), "dev");
        assert_eq!(
            split_label(ActionLabel::Limit(4), false),
            "Split Terminal Horizontally (max 4 per group)"
        );
        assert_eq!(
            split_label(ActionLabel::Shortcut("Ctrl+Shift+D".into()), true),
            "Split Terminal Vertically (Ctrl+Shift+D)"
        );
        assert_eq!(new_label(ActionLabel::Plain), "New Terminal");
        assert_eq!(close_tab_label("Terminal 1", Some("Ctrl+W")), "Close Terminal 1 (Ctrl+W)");
        assert_eq!(
            close_confirmation_message(&CloseConfirmation::One("Terminal 2".into())),
            "Close terminal \"Terminal 2\"?\nThis stops the running process and clears its history."
        );
        assert_eq!(
            close_confirmation_message(&CloseConfirmation::Many(vec!["a".into(), "b".into()])),
            "Close 2 terminals?\nThis stops their running processes and clears their histories: \"a\", \"b\"."
        );
    }
}
