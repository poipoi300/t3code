//! Port of apps/web/src/components/chat/TimelineUserMessageRow.tsx UserMessageRow.

use std::collections::BTreeMap;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::view_logic::session::{MessageAttachment, TimelineEntry};
use t3_contracts::chat_attachment::{ChatAttachment, ChatFileAttachment};
use t3_contracts::composer_context::{ComposerContextRecord, PreviewAnnotationContextRecord};

use crate::i18n::{Value, t, tr};
use crate::runtime::Shared;
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon};
use crate::ui::tooltip::{tooltip, tooltip_create_handle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attribution {
    Own,
    Automation,
    Agent,
    T3Code,
    Guest(String),
}

/// Inputs resolved by the client; attribution, context and clipboard policy are not UI logic.
#[derive(Clone, Debug, PartialEq)]
pub struct UserRow {
    pub id: String,
    pub text: String,
    pub copy_text: String,
    pub prompt_link: Option<String>,
    pub attribution: Attribution,
    pub summary: String,
    pub timestamp: String,
    pub timestamp_tooltip: String,
    pub status: Option<String>,
    pub intent: Option<Intent>,
    pub continues_steer: bool,
    pub grouped_with_previous: bool,
    pub grouped_with_next: bool,
    pub sender_label: String,
    pub can_open_sender: bool,
    pub scheduled_task_title: Option<String>,
    pub has_scheduled_task: bool,
    pub revert_turn_count: Option<usize>,
    pub fresh_context: bool,
    pub model_switch: bool,
    pub workflow_opening: bool,
    pub has_media: bool,
    pub has_audio: bool,
    pub has_files: bool,
    pub media_only: bool,
    pub media_above_text: bool,
    pub file_cards_only: bool,
    pub audio_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent {
    pub promoted: bool,
    pub unread: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FoldState {
    expanded: bool,
    body_expanded: bool,
    content_height: f32,
    lane_width: f32,
}

/// Construct above the virtual list: message-keyed state survives row recycling.
#[derive(Clone, Copy)]
pub struct UserRowState(Signal<BTreeMap<String, FoldState>>);

impl Default for UserRowState {
    fn default() -> Self {
        Self::new()
    }
}

impl UserRowState {
    pub fn new() -> Self {
        Self(signal(BTreeMap::new()))
    }

    fn read(self, id: &str) -> FoldState {
        self.0
            .with(|rows| rows.get(id).copied().unwrap_or_default())
    }

    fn update(self, id: &str, change: impl FnOnce(&mut FoldState)) {
        self.0
            .update(|rows| change(rows.entry(id.to_owned()).or_default()));
    }

    pub fn reveal(self, id: &str) {
        self.update(id, |state| {
            state.expanded = true;
            state.body_expanded = true;
        });
    }
}

pub type RowResolver = Rc<dyn Fn(&TimelineEntry) -> Result<UserRow, String>>;
pub type RowSlot = Rc<dyn Fn(&UserRow) -> AnyElement>;
pub type RowAction = Rc<dyn Fn(&UserRow)>;
pub type RevertAction = Rc<dyn Fn(&UserRow, usize)>;
pub type FileAction = Rc<dyn Fn(&ChatFileAttachment)>;
pub type ImageAction = Rc<dyn Fn(&MessageAttachment)>;
pub type AttachmentRenderer = Rc<dyn Fn(&MessageAttachment) -> AnyElement>;
pub type EntryIconRenderer = Rc<dyn Fn(&str) -> AnyElement>;
pub type IconRenderer = Rc<dyn Fn() -> AnyElement>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMarkdown {
    pub message_id: String,
    pub text: String,
    pub line_breaks: bool,
    pub parse_raw_html: bool,
    pub heading_level_offset: u8,
}

pub type UserMarkdownRenderer = Rc<dyn Fn(UserMarkdown) -> AnyElement>;

/// Slots are the existing TSX dependencies (ChatMarkdown, media, lifecycle and workflow).
/// Absent services stay unsupported; callers must supply permission-checked operations.
#[derive(Clone)]
pub struct UserRowContext {
    pub resolve: RowResolver,
    pub state: UserRowState,
    pub find_active: Signal<bool>,
    pub working: Signal<bool>,
    pub reverting: Signal<bool>,
    pub body: Option<UserMarkdownRenderer>,
    pub media: Option<RowSlot>,
    pub audio: Option<RowSlot>,
    pub files: Option<RowSlot>,
    pub workflow: Option<RowSlot>,
    pub fresh_session: Option<RowSlot>,
    pub model_switch: Option<RowSlot>,
    pub open_sender: Option<RowAction>,
    pub open_automation: Option<RowAction>,
    pub revert: Option<RevertAction>,
    pub copy: Option<RowAction>,
}

impl UserRowContext {
    pub fn new(state: UserRowState, resolve: RowResolver) -> Self {
        Self {
            resolve,
            state,
            find_active: signal(false),
            working: signal(false),
            reverting: signal(false),
            body: None,
            media: None,
            audio: None,
            files: None,
            workflow: None,
            fresh_session: None,
            model_switch: None,
            open_sender: None,
            open_automation: None,
            revert: None,
            copy: None,
        }
    }
}

pub fn user_message_row(
    entry: Memo<Option<Shared<TimelineEntry>>>,
    context: UserRowContext,
) -> impl IntoElement {
    let resolve = context.resolve.clone();
    let row = computed(move || entry.with(|entry| entry.as_ref().map(|entry| resolve(entry))));
    dynamic(move || {
        let row = row.get()?;
        Some(untracked(|| match row {
            Err(reason) => unsupported(&reason).into_element(),
            Ok(row) => {
                let mut root = view();
                for (enabled, slot, label) in [
                    (
                        row.fresh_context,
                        &context.fresh_session,
                        "Fresh session is unavailable.",
                    ),
                    (
                        row.model_switch,
                        &context.model_switch,
                        "Model switch is unavailable.",
                    ),
                ] {
                    if enabled {
                        root =
                            root.child(slot.as_ref().map_or_else(
                                || unsupported(label).into_element(),
                                |slot| slot(&row),
                            ));
                    }
                }
                root.child(user_message_content(Rc::new(row), context.clone()))
                    .into_element()
            }
        }))
    })
}

fn user_message_content(row: Rc<UserRow>, context: UserRowContext) -> AnyElement {
    if row.workflow_opening {
        return context.workflow.as_ref().map_or_else(
            || unsupported("Workflow opening is unavailable.").into_element(),
            |render| render(&row),
        );
    }
    if row.attribution == Attribution::Automation {
        automation_message_row(row, context).into_element()
    } else {
        user_timeline_row(row, context).into_element()
    }
}

fn automation_message_row(row: Rc<UserRow>, context: UserRowContext) -> impl IntoElement {
    let state = context.state;
    let id = row.id.clone();
    let fold = computed(move || state.read(&id));
    let summary = row.scheduled_task_title.clone().unwrap_or_else(|| {
        row.text
            .trim()
            .split('\n')
            .next()
            .unwrap_or_default()
            .trim_end_matches('\r')
            .to_owned()
    });
    let body_row = row.clone();
    let body_context = context.clone();
    let actions_row = row.clone();
    let actions_context = context.clone();
    // NSH has no arbitrary DOM attributes: test_id keeps the stable hook's name, not its CSS semantics.
    view()
        .app_class(tw("group flex flex-col items-end gap-1"))
        .child(row.intent.as_ref().map(user_message_intent_marker))
        .child(
            view()
                .app_class(move || {
                    Some(tw(&cn([
                        Some("min-w-0 max-w-[80%] rounded-2xl bg-message text-message-foreground"),
                        fold.get().expanded.then_some("w-full"),
                    ])))
                })
                .test_id("data-user-message-bubble")
                .label(SharedString::from(t("Automation")))
                .child(folded_message_header(
                    t("Automation"),
                    summary,
                    fold,
                    state,
                    row.id.clone(),
                ))
                .child(dynamic(move || {
                    (fold.get().expanded || body_context.find_active.get()).then(|| {
                        // No nested scroll: the timeline owns scrolling, including expanded runbooks.
                        view()
                            .app_class(move || {
                                Some(tw(&cn([
                                    Some("px-3 pb-3"),
                                    (!fold.get().expanded).then_some("hidden"),
                                ])))
                            })
                            .child(untracked(|| user_message_body(&body_row, &body_context)))
                    })
                })),
        )
        .child(user_message_projected_status(row.status.as_deref()))
        .child(dynamic(move || {
            fold.get().expanded.then(|| {
                untracked(|| {
                    actions_frame(message_actions(&actions_row, &actions_context, true, false))
                })
            })
        }))
}

fn user_timeline_row(row: Rc<UserRow>, context: UserRowContext) -> impl IntoElement {
    let folded = row.attribution == Attribution::Agent;
    let state = context.state;
    let id = row.id.clone();
    let fold = computed(move || state.read(&id));
    let body_row = row.clone();
    let body_context = context.clone();
    let actions_row = row.clone();
    let actions_context = context.clone();
    let floating_row = row.clone();
    let floating_context = context.clone();
    let measure_id = row.id.clone();
    let group_next = row.grouped_with_next;
    let float_actions = move || group_next && pointer() == Pointer::Fine;
    let float_beside = move || float_actions() && fold.get().lane_width >= 592.0;
    // @container queries use the measured row width; sr-only author headings become accessibility labels.
    let mut root = view()
        .app_class(tw("group flex flex-col items-end gap-1"))
        .on_layout(move |bounds: &Bounds<Points>| {
            let width = bounds.size.width.0;
            if state.read(&measure_id).lane_width != width {
                state.update(&measure_id, |state| state.lane_width = width);
            }
        });
    if row.attribution == Attribution::T3Code {
        root = root.child(
            text(t("Sent by T3 Code")).app_class(tw("me-1 text-2xs text-muted-foreground/70")),
        );
    }
    if let Attribution::Guest(name) = &row.attribution {
        root = root.child(text(name.clone()).app_class(tw("me-1 text-2xs text-guest-foreground")));
    }
    if !row.continues_steer {
        root = root.child(row.intent.as_ref().map(user_message_intent_marker));
    }
    if row.media_above_text {
        root = root.child(
            view()
                .app_class(tw("flex max-w-[80%] justify-end"))
                .label(SharedString::from(t("You")))
                .child(row_slot(
                    &context.media,
                    &row,
                    "Media preview is unavailable.",
                )),
        );
    }
    let background = !row.media_only && !row.file_cards_only && !row.audio_only;
    let guest = matches!(row.attribution, Attribution::Guest(_));
    let heading = match &row.attribution {
        Attribution::Agent => row.sender_label.clone(),
        Attribution::Guest(name) => tr("{name} (guest)", &[("name", Value::Text(name.clone()))]),
        _ => t("You"),
    };
    let previous = row.media_above_text || row.grouped_with_previous;
    let next = row.grouped_with_next;
    let mut bubble = view()
        .test_id("data-user-message-bubble")
        .label(SharedString::from(heading))
        .app_class(move || {
            Some(tw(&cn([
                Some("relative min-w-0 max-w-[80%] rounded-2xl text-message-foreground"),
                previous.then_some("rounded-tr-md"),
                next.then_some("rounded-br-md"),
                guest.then_some("bg-guest-surface"),
                (!guest && background).then_some("bg-message"),
                (!folded && background).then_some("p-3"),
                (folded && fold.get().expanded).then_some("w-full"),
            ])))
        });
    if folded {
        bubble = bubble.child(folded_message_header(
            row.sender_label.clone(),
            row.summary.clone(),
            fold,
            state,
            row.id.clone(),
        ));
    }
    bubble = bubble.child(dynamic(move || {
        (!folded || fold.get().expanded || body_context.find_active.get()).then(|| {
            untracked(|| {
                view()
                    .app_class(move || {
                        Some(tw(&cn([
                            folded.then_some("px-3 pb-3"),
                            (folded && !fold.get().expanded).then_some("hidden"),
                        ])))
                    })
                    .child(user_row_body(
                        body_row.clone(),
                        body_context.clone(),
                        !folded,
                    ))
            })
        })
    }));
    // NSH has no cqw: resolve the source's 20cqw - 0.5rem from the measured lane.
    bubble = bubble.child(dynamic(move || float_beside().then(|| {
        untracked(|| view()
            .app_class(tw("absolute end-full top-1/2 me-2 flex w-max -translate-y-1/2 items-center text-xs tabular-nums opacity-0 transition-opacity duration-200 motion-reduce:transition-none focus-within:opacity-100 group-hover:opacity-100"))
            .max_w(move || px((fold.get().lane_width * 0.2 - 8.0).max(0.0)))
            .child(message_actions(&floating_row, &floating_context, false, true)))
    })));
    root.child(bubble)
        .child(user_message_projected_status(row.status.as_deref()))
        .child(dynamic(move || {
            ((!folded || fold.get().expanded) && !float_beside()).then(|| {
                untracked(|| {
                    actions_frame(message_actions(
                        &actions_row,
                        &actions_context,
                        false,
                        float_actions(),
                    ))
                })
            })
        }))
}

fn folded_message_header(
    label: String,
    summary: String,
    fold: Memo<FoldState>,
    state: UserRowState,
    id: String,
) -> impl IntoElement {
    // NSH Button lacks aria-expanded; an expanded prop is needed on the primitive's actual control.
    nsh_base::button().accessibility_label(Some(SharedString::from(label.clone())))
        .hit_slop(move || control_hit_slop(36.0))
        .on_press(move || state.update(&id, |state| state.expanded = !state.expanded))
        .app_class(tw("flex w-full min-w-0 items-center gap-1.5 rounded-2xl px-3 py-2 text-left text-sm focus-visible:outline-2 focus-visible:outline-ring"))
        // CalendarClock and MessagesSquare await UI-ICONS; do not substitute another symbol.
        .child(text(label).app_class(tw("max-w-1/2 shrink-0 truncate text-muted-foreground")))
        .child(text(summary).app_class(tw("min-w-0 flex-1 truncate")))
        .child(view().app_class(move || Some(tw(&cn([
            Some("size-3.5 shrink-0 text-muted-foreground transition-transform motion-reduce:transition-none"),
            fold.get().expanded.then_some("rotate-180"),
        ])))).child(icon(Icon::ChevronDown, 14.0, || palette().muted_foreground)))
}

pub fn user_message_intent_marker(intent: &Intent) -> impl IntoElement + use<> {
    let label = if intent.unread {
        "Steer · waiting to be read"
    } else {
        "Steer"
    };
    let hint = if intent.unread {
        "Waiting for the agent to read this. It reads steers between steps, after a running command finishes."
    } else if intent.promoted {
        "Originally queued, then promoted to steer the active turn"
    } else {
        "Steered the active turn"
    };
    // Redo2 awaits UI-ICONS; no unrelated glyph stands in for it.
    view()
        .app_class(tw(
            "me-1 flex items-center justify-end gap-1 text-xs leading-none text-muted-foreground",
        ))
        .tooltip(tooltip(
            tooltip_create_handle(),
            Default::default(),
            text(t(hint)),
        ))
        .child(text(t(label)))
}

pub fn user_message_projected_status(status: Option<&str>) -> impl IntoElement + use<> {
    let status = status.map(str::to_owned);
    dynamic(move || {
        status.as_deref().filter(|status| !matches!(*status, "completed" | "pending" | "waiting"))
        .map(|status| view().app_class(tw("me-1 flex items-center gap-1.5"))
            .child(text(t(status)).app_class(tw("rounded-full border border-destructive/25 bg-destructive/8 px-1.5 py-0.5 text-3xs font-medium text-destructive"))))
    })
}

fn user_row_body(row: Rc<UserRow>, context: UserRowContext, collapsible: bool) -> impl IntoElement {
    let mut body = view();
    if row.has_media && !row.media_above_text {
        body = body.child(row_slot(
            &context.media,
            &row,
            "Media preview is unavailable.",
        ));
    }
    if row.has_audio {
        body = body.child(row_slot(
            &context.audio,
            &row,
            "Audio preview is unavailable.",
        ));
    }
    if row.has_files {
        body = body.child(row_slot(
            &context.files,
            &row,
            "Attachment preview is unavailable.",
        ));
    }
    if !row.file_cards_only && !row.audio_only {
        body = body.child(collapsible_user_message_body(row, context, collapsible));
    }
    body
}

fn row_slot(slot: &Option<RowSlot>, row: &UserRow, unavailable: &str) -> AnyElement {
    slot.as_ref().map_or_else(
        || unsupported(unavailable).into_element(),
        |render| render(row),
    )
}

fn message_actions(
    row: &UserRow,
    context: &UserRowContext,
    automation: bool,
    float_actions: bool,
) -> impl IntoElement + use<> {
    let mut actions = view().app_class(tw(&cn([
        Some("flex items-center justify-end gap-x-2 gap-y-0.5"),
        Some(if float_actions {
            "flex-wrap"
        } else {
            "shrink-0"
        }),
    ])));
    if (automation && row.has_scheduled_task)
        || (row.attribution == Attribution::Agent && row.can_open_sender)
    {
        let (label, action) = if automation {
            ("Open automation", &context.open_automation)
        } else {
            ("Open thread", &context.open_sender)
        };
        let action = action.clone();
        let row = row.clone();
        let disabled = action.is_none();
        actions = actions.child(button(
            ButtonContent::Label(t(label)),
            ButtonVariant::Link,
            ButtonSize::Xs,
            move || disabled,
            move || {
                if let Some(action) = &action {
                    action(&row);
                }
            },
        ));
    }
    actions = actions.child(
        view()
            .tooltip(tooltip(
                tooltip_create_handle(),
                Default::default(),
                text(t(&row.timestamp_tooltip)),
            ))
            .child(
                text(t(&row.timestamp)).app_class(tw("text-muted-foreground text-xs tabular-nums")),
            ),
    );
    let mut controls = view().app_class(tw(&cn([
        Some("flex items-center gap-0.5"),
        float_actions.then_some("flex-wrap justify-end"),
    ])));
    if let Some(count) = row.revert_turn_count {
        controls = controls.child(revert_user_message_button(row.clone(), count, context));
    }
    if !row.copy_text.is_empty() {
        let row = row.clone();
        let copy = context.copy.clone();
        let disabled = copy.is_none();
        controls = controls.child(
            view()
                .tooltip(tooltip(
                    tooltip_create_handle(),
                    Default::default(),
                    text(t(if disabled {
                        "Copying this message is unavailable."
                    } else {
                        "Copy message"
                    })),
                ))
                .child(button(
                    ButtonContent::Icon {
                        icon: Icon::Copy,
                        label: t("Copy message"),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::Xs,
                    move || disabled,
                    move || {
                        if let Some(copy) = &copy {
                            copy(&row);
                        }
                    },
                )),
        );
    }
    if let Some(link) = &row.prompt_link {
        let link = link.clone();
        // NSH clipboard writes have no acknowledgement: do not claim Copied or suppress retries.
        controls = controls.child(button(
            ButtonContent::Icon {
                icon: Icon::Link,
                label: t("Copy prompt link"),
            },
            ButtonVariant::GhostMuted,
            ButtonSize::Xs,
            || false,
            move || set_clipboard(link.clone()),
        ));
    }
    actions.child(controls)
}

fn actions_frame(actions: impl IntoElement) -> impl IntoElement {
    view().app_class(move || Some(tw(&cn([
        Some("flex w-full max-w-[80%] justify-end pe-1 items-center text-xs tabular-nums opacity-0 transition-opacity duration-200 motion-reduce:transition-none focus-within:opacity-100 group-hover:opacity-100"),
        (pointer() == Pointer::Coarse).then_some("opacity-100"),
    ])))).child(actions)
}

fn revert_user_message_button(
    row: UserRow,
    count: usize,
    context: &UserRowContext,
) -> impl IntoElement + use<> {
    let action = context.revert.clone();
    let missing = action.is_none();
    let working = context.working;
    let reverting = context.reverting;
    view()
        .tooltip(tooltip(
            tooltip_create_handle(),
            Default::default(),
            text(t(if missing {
                "Editing from this message is unavailable."
            } else {
                "Edit from here"
            })),
        ))
        .child(button(
            ButtonContent::Icon {
                icon: Icon::SquarePen,
                label: t("Edit from here"),
            },
            ButtonVariant::Ghost,
            ButtonSize::Xs,
            move || missing || working.get() || reverting.get(),
            move || {
                if !working.get_untracked()
                    && !reverting.get_untracked()
                    && let Some(action) = &action
                {
                    action(&row, count);
                }
            },
        ))
}

fn collapsible_user_message_body(
    row: Rc<UserRow>,
    context: UserRowContext,
    collapsible: bool,
) -> impl IntoElement {
    let state = context.state;
    let id = row.id.clone();
    let fold = computed(move || state.read(&id));
    let has_body = !row.text.trim().is_empty();
    let can_collapse = move || collapsible && has_body && fold.get().content_height >= 352.0;
    let measure_id = row.id.clone();
    let toggle_id = row.id.clone();
    view()
        .child(has_body.then(|| {
            view()
                // NSH has clipping but no CSS mask-image: the web's 1.75rem fade needs an NSH mask primitive.
                .app_class(move || {
                    Some(tw(&cn([
                        Some("relative"),
                        (can_collapse() && !fold.get().body_expanded)
                            .then_some("max-h-44 overflow-hidden"),
                    ])))
                })
                .test_id("data-user-message-body")
                .child(
                    view()
                        .on_layout(move |bounds: &Bounds<Points>| {
                            let height = bounds.size.height.0;
                            if state.read(&measure_id).content_height != height {
                                state.update(&measure_id, |state| state.content_height = height);
                            }
                        })
                        .child(user_message_body(&row, &context)),
                )
        }))
        .child(dynamic(move || {
            can_collapse().then(|| {
                let toggle_id = toggle_id.clone();
                let label = if fold.get().body_expanded {
                    "Show less"
                } else {
                    "Show full message"
                };
                view()
                    .app_class(tw("mt-1.5 flex items-center justify-end gap-2"))
                    .child(view().app_class(tw("-ml-1")).child(button(
                        ButtonContent::Label(t(label)),
                        ButtonVariant::GhostMuted,
                        ButtonSize::Xs,
                        || false,
                        move || {
                            state.update(&toggle_id, |state| {
                                state.body_expanded = !state.body_expanded
                            })
                        },
                    )))
            })
        }))
}

fn user_message_body(row: &UserRow, context: &UserRowContext) -> AnyElement {
    if let Some(body) = &context.body {
        return body(UserMarkdown {
            message_id: row.id.clone(),
            text: row.text.clone(),
            line_breaks: true,
            parse_raw_html: false,
            heading_level_offset: 3,
        });
    }
    // ChatMarkdown's renderer must apply BUBBLE_CHIP_CLASS_NAME's context-chip-message ink to its chips.
    // TL-MARKDOWN owns the user plugins, context chips, line breaks and heading offset.
    // Keep the original text until it exists: neither raw HTML nor context links are executed.
    text(row.text.clone())
        .select_text()
        .app_class(tw("text-sm leading-relaxed text-message-foreground"))
        .into_element()
}

fn unsupported(reason: &str) -> impl IntoElement + use<> {
    text(t(reason)).app_class(tw("text-xs text-muted-foreground"))
}

fn control_hit_slop(control_size: f32) -> EdgeInsets {
    EdgeInsets::all(px(((crate::theme::metrics().hit_target - control_size)
        / 2.0)
        .max(0.0)))
}

/// `downloadable` includes the client's optimistic upload state, absent from ChatFileAttachment.
#[derive(Clone)]
pub struct FilePresentation {
    pub file: ChatFileAttachment,
    pub downloadable: bool,
    pub extension: String,
    pub open: Option<FileAction>,
    pub download: Option<FileAction>,
    pub entry_icon: Option<EntryIconRenderer>,
    pub download_icon: Option<IconRenderer>,
}

pub fn user_file_card(file: FilePresentation) -> impl IntoElement {
    let label = user_message_file_label(&file.file);
    let pasted = file.file.source.as_ref().and_then(Option::as_ref).is_some();
    let size = t3_i18n::display::format_bytes(file.file.size_bytes as f64);
    let details = if !file.downloadable {
        t("Sending…")
    } else if pasted {
        size
    } else {
        format!("{} · {size}", t(&file.extension))
    };
    let entry_icon = view()
        .app_class(tw(
            "flex size-9 shrink-0 items-center justify-center rounded-lg bg-background/60",
        ))
        .child(
            file.entry_icon
                .as_ref()
                .map(|render| render(&file.file.name)),
        );
    let identity = view()
        .app_class(tw("min-w-0 flex-1"))
        .child(text(label.clone()).app_class(tw(
            "block truncate text-sm font-medium group-hover/file:underline",
        )))
        .child(text(details).app_class(tw("block text-xs text-muted-foreground tabular-nums")));
    let identity = if file.downloadable {
        let open = file.open.clone();
        let attachment = file.file.clone();
        nsh_base::button()
            .accessibility_label(Some(SharedString::from(tr("Preview {label}", &[("label", Value::Text(label.clone()))]))))
            .disabled(open.is_none())
            .hit_slop(move || control_hit_slop(36.0))
            .on_press(move || { if let Some(open) = &open { open(&attachment); } })
            .app_class(tw("group/file focus-visible:ring-ring/70 flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-xl text-left focus-visible:ring-2 focus-visible:ring-inset focus-visible:outline-none"))
            .child(entry_icon).child(identity).into_element()
    } else {
        view()
            .app_class(tw("flex min-w-0 flex-1 items-center gap-3"))
            .child(entry_icon)
            .child(identity)
            .into_element()
    };
    let download_label = if file.downloadable {
        tr("Download {label}", &[("label", Value::Text(label))])
    } else {
        t("Sending attachment")
    };
    let downloadable = file.downloadable;
    let download = file.download.clone().filter(|_| downloadable);
    let disabled = download.is_none();
    // ButtonContent has no custom-icon slot; preserve the icon-xs ghost-muted classes here.
    let download_button = nsh_base::button()
        .accessibility_label(Some(SharedString::from(download_label.clone())))
        .disabled(disabled)
        .hit_slop(move || control_hit_slop(24.0))
        .app_class(tw("inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-64 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
        .on_press(move || { if let Some(download) = &download { download(&file.file); } })
        .child(file.download_icon.as_ref().map(|render| render()));
    let hint = if !downloadable {
        t("Sending attachment")
    } else if disabled || file.open.is_none() {
        t("Attachment actions are unavailable.")
    } else {
        download_label
    };
    view().app_class(tw("flex w-64 max-w-full items-center gap-1 rounded-2xl bg-message py-2 ps-2 pe-1 text-message-foreground"))
        .child(identity)
        .child(view().tooltip(tooltip(tooltip_create_handle(), Default::default(), text(hint))).child(download_button))
}

pub fn user_video_attachment(
    name: &str,
    standalone: bool,
    corners: &str,
    player: Option<AnyElement>,
) -> impl IntoElement + use<> {
    // MEDIA_EDGE_CLASS_NAME's after ring is the actual frame border; no pseudo-element is needed.
    let frame = cn([
        Some(if standalone {
            "relative overflow-hidden rounded-2xl ring-1 ring-inset ring-black/9 dark:ring-white/12"
        } else {
            "rounded-lg border border-border/80"
        }),
        standalone.then_some(corners),
    ]);
    view().app_class(tw(&frame)).child(player.unwrap_or_else(|| {
        view().app_class(tw("flex aspect-[4/3] w-full items-center justify-center bg-black px-2 py-3 text-center text-2xs text-white/70"))
            .child(text(name.to_owned())).into_element()
    }))
}

/// The unchipped-file branch of UserTimelineRow, used for files sent beside words.
pub fn user_file_attachment(file: FilePresentation) -> impl IntoElement {
    let label = user_message_file_label(&file.file);
    let pasted = file.file.source.as_ref().and_then(Option::as_ref).is_some();
    let identity = view()
        .app_class(tw("min-w-0 flex-1"))
        .child(text(label.clone()).app_class(tw("block truncate")))
        .child((!file.downloadable || pasted).then(|| {
            text(if file.downloadable {
                t3_i18n::display::format_bytes(file.file.size_bytes as f64)
            } else {
                t("Sending…")
            })
            .app_class(tw("block text-xs text-muted-foreground tabular-nums"))
        }));
    let glyph = file
        .entry_icon
        .as_ref()
        .map(|render| render(&file.file.name));
    if !file.downloadable {
        return view()
            .app_class(tw("flex min-w-0 items-center gap-2 py-1 text-sm"))
            .child(glyph)
            .child(identity)
            .into_element();
    }
    let open = file.open.clone();
    let attachment = file.file.clone();
    let preview = nsh_base::button()
        .accessibility_label(Some(SharedString::from(tr("Preview {label}", &[("label", Value::Text(label.clone()))]))))
        .disabled(open.is_none())
        .hit_slop(move || control_hit_slop(28.0))
        .on_press(move || { if let Some(open) = &open { open(&attachment); } })
        .app_class(tw("focus-visible:ring-ring/70 flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-md py-1 text-left text-sm hover:underline focus-visible:ring-2 focus-visible:ring-inset focus-visible:outline-none"))
        // Eye awaits UI-ICONS, like PierreEntryIcon and Download.
        .child(glyph).child(identity);
    let download = file.download.clone();
    let disabled = download.is_none();
    let download_label = tr("Download {label}", &[("label", Value::Text(label))]);
    let hint = if disabled || file.open.is_none() {
        t("Attachment actions are unavailable.")
    } else {
        download_label.clone()
    };
    view().app_class(tw("flex min-w-0 items-center gap-1"))
        .child(preview)
        .child(view().tooltip(tooltip(tooltip_create_handle(), Default::default(), text(hint)))
            .child(nsh_base::button()
                .accessibility_label(Some(SharedString::from(download_label)))
                .disabled(disabled)
                .hit_slop(move || control_hit_slop(24.0))
                .app_class(tw("inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-64 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                .on_press(move || { if let Some(download) = &download { download(&file.file); } })
                .child(file.download_icon.as_ref().map(|render| render()))))
        .into_element()
}

/// The `mediaBlock` expression in UserTimelineRow; classification and asset URLs come from the client.
pub fn media_block(
    images: &[MessageAttachment],
    videos: Vec<AnyElement>,
    row: &UserRow,
    expand: Option<ImageAction>,
    snapshot_details: Option<AttachmentRenderer>,
) -> impl IntoElement + use<> {
    let standalone = row.media_only || row.media_above_text;
    let lone = standalone && videos.is_empty() && images.len() == 1;
    let collage = standalone && videos.is_empty() && images.len() > 1 && images.iter().all(|image| {
        matches!(&image.attachment, ChatAttachment::Image(image) if image.source.as_ref().and_then(Option::as_ref).is_none())
    });
    let corners = cn([
        row.grouped_with_previous.then_some("rounded-tr-md"),
        (row.media_above_text || row.grouped_with_next).then_some("rounded-br-md"),
    ]);
    let mut grid = view().app_class(tw(&cn([
        Some("grid"),
        Some(if !standalone { "mb-2 max-w-[210px] grid-cols-2 gap-2" }
            else if lone { "max-w-full" }
            else if collage { "relative w-80 max-w-full gap-px overflow-hidden rounded-2xl bg-black/9 dark:bg-white/12" }
            else if images.len() + videos.len() == 1 { "w-80 max-w-full" }
            else { "w-80 max-w-full grid-cols-2 gap-2" }),
        collage.then_some(corners.as_str()),
        collage.then_some(match images.len() {
            2 => "auto-rows-[12rem] grid-cols-2",
            3 => "auto-rows-[calc(6rem-0.5px)] grid-cols-[3fr_2fr]",
            _ => "auto-rows-[6rem] grid-cols-2",
        }),
    ])));
    for (index, attachment) in images.iter().enumerate() {
        let ChatAttachment::Image(image) = &attachment.attachment else {
            grid = grid.child(unsupported("Image preview is unavailable."));
            continue;
        };
        let snapshot = image.source.as_ref().and_then(Option::as_ref).is_some();
        let framed = (snapshot && attachment.preview_url.is_some() && standalone)
            || lone
            || (standalone && !collage);
        let mut frame = view().app_class(tw(&cn([
            Some("bg-background/70"),
            Some(if snapshot && attachment.preview_url.is_some() {
                if standalone { "col-span-2 relative min-h-16 justify-self-end overflow-hidden rounded-2xl" }
                else { "col-span-2 relative h-28 w-52 max-w-full overflow-hidden rounded-lg border border-border/80" }
            } else if lone { "relative overflow-hidden rounded-2xl" }
            else if collage { "overflow-hidden bg-background" }
            else if standalone { "relative aspect-[4/3] overflow-hidden rounded-2xl" }
            else { "aspect-[4/3] overflow-hidden rounded-lg border border-border/80" }),
            lone.then_some(corners.as_str()),
            (collage && images.len() == 3 && index == 0).then_some("row-span-2"),
            (collage && images.len() > 3 && images.len() % 2 == 1 && index == images.len() - 1).then_some("col-span-2"),
        ])));
        if let Some(src) = &attachment.preview_url {
            let label = tr(
                "Preview {name}",
                &[("name", Value::Text(image.name.clone()))],
            );
            let open = expand.clone();
            let selected = attachment.clone();
            frame = frame.child(
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(label)))
                    .disabled(open.is_none())
                    .on_press(move || {
                        if let Some(open) = &open {
                            open(&selected);
                        }
                    })
                    .app_class(tw("block h-full w-full cursor-zoom-in"))
                    // transform-gpu is a Chromium repaint hint; native image isolation needs no class.
                    .child(
                        nsh_core::element::image(nsh_core::element::ImageSrc::from(src.clone()))
                            .label(SharedString::from(image.name.clone()))
                            .app_class(tw(if standalone && snapshot {
                                "block h-auto max-h-48 w-auto min-w-24 max-w-full object-contain"
                            } else if lone {
                                "block h-48 w-auto min-w-24 max-w-full object-cover"
                            } else {
                                "block size-full object-cover"
                            })),
                    ),
            );
        } else {
            frame = frame.child(view().app_class(tw(&cn([
                Some("flex min-h-[72px] items-center justify-center px-2 py-3 text-center text-2xs text-muted-foreground/70"),
                lone.then_some("h-48 w-64 max-w-full"), collage.then_some("h-full"),
            ]))).child(text(image.name.clone())));
        }
        if snapshot && attachment.preview_url.is_some() {
            frame = frame.child(snapshot_details.as_ref().map_or_else(
                || unsupported("Snapshot details are unavailable.").into_element(),
                |render| render(attachment),
            ));
        }
        if framed {
            frame = frame.child(media_edge(if lone { &corners } else { "" }));
        }
        grid = grid.child(frame);
    }
    for video in videos {
        grid = grid.child(video);
    }
    if collage {
        grid = grid.child(media_edge(&corners));
    }
    grid
}

fn media_edge(corners: &str) -> impl IntoElement + use<> {
    // NSH cannot parse rounded-[inherit]: duplicate the frame's 2xl and tightened corners on ::after.
    view().pointer_events(PointerEvents::None).app_class(tw(&cn([
        Some("absolute inset-0 z-10 rounded-2xl ring-1 ring-inset ring-black/9 dark:ring-white/12"),
        Some(corners),
    ])))
}

/// Port of apps/web/src/components/chat/TimelineUserMessageRow.logic.ts userMessageFileLabel.
pub fn user_message_file_label(file: &ChatFileAttachment) -> String {
    if file.source.as_ref().and_then(Option::as_ref).is_some() {
        t("Pasted text")
    } else {
        file.name.clone()
    }
}

/// Port of apps/web/src/components/chat/TimelineUserMessageRow.logic.ts resolvePreviewAnnotationImage.
pub fn resolve_preview_annotation_image<'a>(
    record: &PreviewAnnotationContextRecord,
    records_by_id: &BTreeMap<String, ComposerContextRecord>,
    user_images: &'a [MessageAttachment],
    preview_images: &'a [MessageAttachment],
    annotation_record_ids: &[String],
) -> Option<&'a MessageAttachment> {
    let screenshot = record
        .screenshot_context_id
        .as_ref()
        .and_then(|id| records_by_id.get(&id.0));
    if let Some(ComposerContextRecord::Image(screenshot)) = screenshot
        && let Some(image) = user_images.iter().find(|image| matches!(
            &image.attachment, ChatAttachment::Image(image) if image.id == screenshot.attachment_id.0
        )) { return Some(image); }
    let name = format!("preview-annotation-{}.png", record.annotation_id);
    preview_images
        .iter()
        .find(|image| {
            matches!(
                &image.attachment, ChatAttachment::Image(image) if image.name == name
            )
        })
        .or_else(|| {
            annotation_record_ids
                .iter()
                .position(|id| id == &record.context_id.0)
                .and_then(|index| preview_images.get(index))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn file(source: Option<Value>) -> ChatFileAttachment {
        let mut value = json!({
            "type": "file", "id": "file", "name": "notes.txt", "mimeType": "text/plain", "sizeBytes": 12
        });
        if let Some(source) = source {
            value["source"] = source;
        }
        serde_json::from_value(value).expect("file")
    }

    fn image(id: &str, name: &str) -> MessageAttachment {
        serde_json::from_value(json!({
            "type": "image", "id": id, "name": name, "mimeType": "image/png", "sizeBytes": 12
        }))
        .expect("image")
    }

    fn annotation(screenshot: Option<&str>) -> PreviewAnnotationContextRecord {
        let mut value = json!({
            "version": 1, "contextId": "annotation", "kind": "preview-annotation", "label": "Region",
            "annotationId": "region", "pageUrl": "https://example.test", "pageTitle": null,
            "comment": "Fix this", "targetSummary": "Button", "styleChanges": []
        });
        if let Some(id) = screenshot {
            value["screenshotContextId"] = json!(id);
        }
        serde_json::from_value(value).expect("annotation")
    }

    fn image_record(attachment: &str) -> ComposerContextRecord {
        serde_json::from_value(json!({
            "version": 1, "contextId": "screenshot", "kind": "image", "label": "Screen",
            "attachmentId": attachment, "name": "screen.png", "mimeType": "image/png", "sizeBytes": 12
        })).expect("record")
    }

    #[test]
    fn pasted_text_uses_its_label_while_absent_and_null_sources_keep_the_filename() {
        for source in [None, Some(Value::Null)] {
            assert_eq!(user_message_file_label(&file(source)), "notes.txt");
        }
        assert_eq!(
            user_message_file_label(&file(Some(json!({"_tag": "pasted-text"})))),
            "Pasted text"
        );
    }

    #[test]
    fn a_bound_screenshot_wins_over_both_legacy_fallbacks() {
        let images = [image("bound", "screen.png")];
        let previews = [
            image("position", "old.png"),
            image("named", "preview-annotation-region.png"),
        ];
        let records = BTreeMap::from([("screenshot".into(), image_record("bound"))]);
        let selected = resolve_preview_annotation_image(
            &annotation(Some("screenshot")),
            &records,
            &images,
            &previews,
            &["annotation".into()],
        );
        assert!(std::ptr::eq(selected.expect("bound image"), &images[0]));
    }

    #[test]
    fn unavailable_or_non_image_screenshots_fall_back_to_the_exact_annotation_name() {
        let previews = [
            image("position", "old.png"),
            image("named", "preview-annotation-region.png"),
        ];
        let file_record = serde_json::from_value(json!({
            "version": 1, "contextId": "screenshot", "kind": "file", "label": "file",
            "attachmentId": "bound", "name": "screen.png", "mimeType": "image/png", "sizeBytes": 12
        }))
        .expect("file record");
        for records in [
            BTreeMap::new(),
            BTreeMap::from([("screenshot".into(), image_record("missing"))]),
            BTreeMap::from([("screenshot".into(), file_record)]),
        ] {
            let selected = resolve_preview_annotation_image(
                &annotation(Some("screenshot")),
                &records,
                &[],
                &previews,
                &["annotation".into()],
            );
            assert!(std::ptr::eq(
                selected.expect("named fallback"),
                &previews[1]
            ));
        }
    }

    #[test]
    fn positional_fallback_uses_the_records_index_and_never_defaults_to_the_first_image() {
        let previews = [image("one", "old-1.png"), image("two", "old-2.png")];
        let records = BTreeMap::new();
        let selected = resolve_preview_annotation_image(
            &annotation(None),
            &records,
            &[],
            &previews,
            &["other".into(), "annotation".into()],
        );
        assert!(std::ptr::eq(selected.expect("second image"), &previews[1]));
        for ids in [
            vec![],
            vec!["other".into()],
            vec!["a".into(), "b".into(), "annotation".into()],
        ] {
            assert!(
                resolve_preview_annotation_image(&annotation(None), &records, &[], &previews, &ids)
                    .is_none()
            );
        }
    }
}
