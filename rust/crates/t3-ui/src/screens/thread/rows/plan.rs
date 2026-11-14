//! Port of apps/web/src/components/chat/ProposedPlanCard.tsx ProposedPlanCard.

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::Shared;
use crate::screens::thread::blocks::flatten::{Block, BlockId, BlockKind, MessageFlattener};
use crate::screens::thread::markdown::base::chat_markdown;
use crate::screens::thread::markdown::code_block::{
    CodeBlockOptions, CodeBlockProps, CodeBlockState, extract_fence_title, markdown_code_block,
};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;
use crate::ui::input::{InputSize, text_field};
use crate::ui::{badge, dialog, menu, toast, tooltip};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture};
use nsh_base::text_view::{Block as TextBlock, Document, Inline, TextViewClasses};
use nsh_base::text_view::{MarkdownPlugin, MarkdownPlugins, ParseContext, mdast};
use nsh_core::prelude::*;
use std::{collections::BTreeMap, rc::Rc, time::Duration};
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::types::ThreadKey;
use t3_client::view_logic::session::plan_presentation::{
    PlanCodeFence, PlanTextState, PreviewState, plan_code_fence, preview_chunk, preview_code_text,
};
use t3_client::view_logic::session::text::{
    normalize_plan_markdown_for_export, proposed_plan_markdown_filename,
};
use t3_contracts::wire::trim_js;

pub struct PlanRow<'a> {
    pub id: &'a str,
    pub markdown: &'a str,
    pub streaming: bool,
}
impl<'a> PlanRow<'a> {
    pub fn from_row(row: &'a MessagesTimelineRow) -> Option<Self> {
        let MessagesTimelineRowContent::ProposedPlan { proposed_plan } = &row.content else {
            return None;
        };
        Some(Self {
            id: proposed_plan
                .get("id")
                .and_then(|id| id.as_str())
                .unwrap_or(&row.id),
            markdown: proposed_plan.get("planMarkdown")?.as_str()?,
            streaming: proposed_plan
                .get("status")
                .and_then(|status| status.as_str())
                == Some("active"),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavePlan {
    pub thread: ThreadKey,
    pub cwd: String,
    pub relative_path: String,
    pub contents: String,
}
/// None is the client's interrupted command outcome; it must not announce a saved file.
pub type WritePlan =
    Rc<dyn Fn(SavePlan) -> LocalBoxFuture<'static, Result<Option<String>, String>>>;
pub type DownloadPlan = Rc<dyn Fn(String, String) -> LocalBoxFuture<'static, Result<(), String>>>;

/// Service plumbing only: the card owns its markdown, menu, form and feedback.
#[derive(Clone)]
pub struct PlanContext {
    pub thread: ThreadKey,
    pub workspace_root: Memo<Option<String>>,
    pub can_write: Memo<bool>,
    pub find_active: Memo<bool>,
    pub write_file: Option<WritePlan>,
    pub download: Option<DownloadPlan>,
    pub state: PlanRowsState,
}
impl PlanContext {
    pub fn new(thread: ThreadKey, state: PlanRowsState) -> Self {
        let client =
            use_context::<crate::runtime::Rt>().and_then(|runtime| runtime.backend().client());
        let can_write = signal(false);
        let (write_file, download) = if let Some(client) = client {
            let service = client.plan_services();
            let mut changes = service.changes(&thread);
            can_write.set(service.can_write(&thread));
            let destination = thread.clone();
            spawn(async move {
                while changes.next().await.is_some() {
                    can_write.set(service.can_write(&destination));
                }
            });
            (
                Some(crate::runtime::services::thread_ops::write_plan(&client)),
                crate::runtime::services::thread_ops::download_plan(&client),
            )
        } else {
            (None, None)
        };
        Self {
            thread,
            state,
            workspace_root: computed(|| None),
            can_write: computed(move || can_write.get()),
            find_active: computed(|| false),
            write_file,
            download,
        }
    }
}

#[derive(Clone, Copy)]
pub struct PlanRowsState {
    plans: Stored<BTreeMap<String, PlanState>>,
    owner: Owner,
}
impl Default for PlanRowsState {
    fn default() -> Self {
        Self {
            plans: stored(BTreeMap::new()),
            owner: Owner::current(),
        }
    }
}
#[derive(Clone, Copy)]
struct PlanState {
    owner: Owner,
    expanded: Signal<bool>,
    copied: Signal<bool>,
    copy_generation: Signal<u64>,
    open: Signal<bool>,
    saving: Signal<bool>,
    last_path: Signal<String>,
    draft_path: Signal<String>,
    render: Stored<PlanRender>,
    keys: Signal<Vec<BlockId>>,
    slots: Stored<Vec<Signal<Shared<PlanLeaf>>>>,
    code_states: Stored<BTreeMap<BlockId, CodeBlockState>>,
    title: Signal<Option<Shared<String>>>,
    collapsible: Signal<bool>,
}
impl PartialEq for PlanState {
    fn eq(&self, other: &Self) -> bool {
        self.expanded == other.expanded
    }
}
impl PlanRowsState {
    fn get(self, id: &str) -> PlanState {
        self.owner.run(|| {
            self.plans.with(|plans| {
                *plans.entry(id.into()).or_insert_with(|| PlanState {
                    owner: self.owner,
                    expanded: signal(false),
                    copied: signal(false),
                    copy_generation: signal(0),
                    open: signal(false),
                    saving: signal(false),
                    last_path: signal(String::new()),
                    draft_path: signal(String::new()),
                    render: stored(PlanRender::new(id)),
                    keys: signal(Vec::new()),
                    slots: stored(Vec::new()),
                    code_states: stored(BTreeMap::new()),
                    title: signal(None),
                    collapsible: signal(false),
                })
            })
        })
    }
}
struct PlanRender {
    flattener: MessageFlattener,
    presentation: PlanTextState,
    leaves: Vec<Rc<PlanLeaf>>,
}
struct PlanLeaf {
    block: Rc<Block>,
    preview: Rc<str>,
    preview_code: Rc<str>,
    preview_state: PreviewState,
    code_first: Option<usize>,
    code_fence: Option<PlanCodeFence>,
}
impl PlanRender {
    fn new(id: &str) -> Self {
        Self {
            flattener: MessageFlattener::new(id),
            presentation: PlanTextState::default(),
            leaves: Vec::new(),
        }
    }
    fn update(&mut self, row: &PlanRow<'_>, state: PlanState, owner: Owner) {
        let source = row.markdown;
        let update = self.presentation.update(source, row.streaming);
        if update.reset {
            self.flattener = MessageFlattener::new(row.id);
            self.leaves.clear();
        }
        state.collapsible.set(update.collapsible);
        state.title.set(update.title.map(Shared));
        let start = update.display_start;
        let mut delta = self.flattener.ingest(row.id, &source[start..]);
        if !row.streaming
            && let Some(finished) = self.flattener.finish()
        {
            if finished.rows.is_empty() {
                delta.document = finished.document;
            } else {
                delta = finished;
            }
        }
        let count = delta.document.block_count();
        state.slots.with(|slots| {
            for block in delta.rows {
                let at = block.id.index;
                let before = at
                    .checked_sub(1)
                    .and_then(|at| self.leaves.get(at))
                    .map_or(PreviewState::default(), |leaf| leaf.preview_state);
                let raw = source[start..]
                    .get(block.source.bytes.clone())
                    .unwrap_or("");
                let (preview, preview_state) = preview_chunk(raw, before);
                let code_first = match &block.kind {
                    BlockKind::Code(code) if code.first => Some(at),
                    BlockKind::Code(_) => at
                        .checked_sub(1)
                        .and_then(|at| self.leaves.get(at))
                        .and_then(|leaf| leaf.code_first),
                    _ => None,
                };
                let code_fence = match &block.kind {
                    BlockKind::Code(code) if code.first => plan_code_fence(raw),
                    BlockKind::Code(_) => at
                        .checked_sub(1)
                        .and_then(|at| self.leaves.get(at))
                        .and_then(|leaf| leaf.code_fence),
                    _ => None,
                };
                let preview_code = match &block.kind {
                    BlockKind::Code(code) => Rc::from(preview_code_text(
                        &preview, code.first, code.last, code_fence,
                    )),
                    _ => Rc::from(""),
                };
                let leaf = Rc::new(PlanLeaf {
                    block,
                    preview: Rc::from(preview),
                    preview_code,
                    preview_state,
                    code_first,
                    code_fence,
                });
                if at == self.leaves.len() {
                    self.leaves.push(leaf.clone());
                } else {
                    self.leaves[at] = leaf.clone();
                }
                if at == slots.len() {
                    slots.push(owner.run(|| signal(Shared(leaf))));
                } else {
                    slots[at].set(Shared(leaf));
                }
            }
            slots.truncate(count);
        });
        self.leaves.truncate(count);
        if state.keys.with_untracked(Vec::len) != count {
            state.keys.set(
                (0..count)
                    .map(|index| BlockId {
                        message_id: row.id.into(),
                        index,
                    })
                    .collect(),
            );
        }
    }
}

/// Port of TimelineAssistantRows.tsx ProposedPlanTimelineRow.
pub fn proposed_plan_timeline_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    context: PlanContext,
) -> impl IntoElement {
    let owner = context.state.owner;
    let state = computed(move || {
        row.with(|row| {
            row.as_ref().and_then(|row| {
                let row = PlanRow::from_row(row)?;
                let state = context.state.get(row.id);
                untracked(|| {
                    state
                        .render
                        .with(|render| render.update(&row, state, owner))
                });
                Some(state)
            })
        })
    });
    view()
        .app_class(tw("min-w-0 px-1 py-0.5"))
        .child(dynamic(move || {
            state
                .get()
                .map(|state| untracked(|| proposed_plan_card(row, state, context.clone())))
        }))
}
fn contents(row: Memo<Option<Shared<MessagesTimelineRow>>>) -> Option<(String, String)> {
    row.with_untracked(|row| {
        let row = PlanRow::from_row(row.as_ref()?)?;
        Some((
            proposed_plan_markdown_filename(row.markdown),
            normalize_plan_markdown_for_export(row.markdown),
        ))
    })
}
fn plan_toast(variant: toast::Variant, title: &'static str, detail: Option<String>) {
    toast::toast_manager().add(toast::stacked_thread_toast(toast::ToastOptions {
        variant,
        title: title.into(),
        description_content: detail.map(|detail| {
            Rc::new(move || text(SharedString::from(detail.clone())).into_element()) as toast::Slot
        }),
        ..Default::default()
    }));
}
fn proposed_plan_card(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    state: PlanState,
    context: PlanContext,
) -> impl IntoElement {
    let saving_context = context.clone();
    let owner = context.state.owner;
    let download = context.download.clone();
    let can_save = context.can_write;
    let root = context.workspace_root;
    let has_writer = context.write_file.is_some();
    let find = context.find_active;
    let popup = menu::menu_popup_with(move || {
        view()
            .child(menu::menu_item(
                t("Copy to clipboard"),
                Default::default(),
                text(move || {
                    if state.copied.get() {
                        t("Copied!")
                    } else {
                        t("Copy to clipboard")
                    }
                }),
                move || {
                    if let Some((_, source)) = contents(row) {
                        set_clipboard(source);
                        state.copied.set(true);
                        let generation = state.copy_generation.get_untracked().wrapping_add(1);
                        state.copy_generation.set(generation);
                        owner.run(move || {
                            set_timeout(Duration::from_secs(2), move || {
                                if state.copy_generation.get_untracked() == generation {
                                    state.copied.set(false);
                                }
                            })
                        });
                    }
                },
            ))
            .child(download.clone().map(|download| {
                menu::menu_item(
                    t("Download as markdown"),
                    Default::default(),
                    text(t("Download as markdown")),
                    move || {
                        if let Some((name, source)) = contents(row) {
                            let download = download.clone();
                            owner.run(|| {
                                spawn(async move {
                                    if let Err(error) = download(name, source).await {
                                        plan_toast(
                                            toast::Variant::Error,
                                            "Could not download plan",
                                            Some(error),
                                        );
                                    }
                                })
                            });
                        }
                    },
                )
            }))
            .child(
                view()
                    .tooltip(plan_write_hint(state, can_save, root, has_writer))
                    .child(menu::menu_item(
                        t("Save to workspace"),
                        menu::ItemOptions {
                            disabled: Prop::bind(move || {
                                !has_writer
                                    || !can_save.get()
                                    || root.get().is_none()
                                    || state.saving.get()
                            }),
                            ..Default::default()
                        },
                        text(t("Save to workspace")),
                        move || {
                            if has_writer
                                && can_save.get_untracked()
                                && root.get_untracked().is_some()
                                && !state.saving.get_untracked()
                            {
                                let last = state.last_path.get_untracked();
                                state.draft_path.set(if last.is_empty() {
                                    contents(row)
                                        .map(|(filename, _)| filename)
                                        .unwrap_or_default()
                                } else {
                                    last
                                });
                                state.open.set(true);
                            }
                        },
                    )),
            )
    });
    view().app_class(tw("rounded-2xl border border-border/80 bg-card/70 p-4 sm:p-5"))
        .child(view().app_class(tw("flex flex-wrap items-center justify-between gap-3"))
            .child(view().app_class(tw("flex min-w-0 items-center gap-2"))
                .child(badge::badge(badge::Props { variant: badge::Variant::Secondary, ..Default::default() }, vec![badge::Child::Text(Text::from("Plan"))]))
                .child(text(move || state.title.get().map_or_else(|| t("Proposed plan"), |title| title.to_string())).role(AccessibilityRole::Header).app_class(tw("truncate text-sm font-medium text-foreground"))))
            .child(menu::menu(button(ButtonContent::Icon { icon: Icon::Ellipsis, label: t("Plan actions") }, ButtonVariant::Outline, ButtonSize::IconXs, || false, || {}), menu::MenuOptions { align: Align::End, ..Default::default() }, popup)))
        .child(view().app_class(tw("mt-4"))
            .child(view().app_class(move || tw(&cn([Some("relative"), (state.collapsible.get() && !state.expanded.get()).then_some("max-h-104 overflow-hidden")])))
                .child(dynamic(move || {
                    let preview = state.collapsible.get() && !state.expanded.get() && !find.get();
                    untracked(|| plan_blocks(state, preview))
                }))
                .child(dynamic(move || (state.collapsible.get() && !state.expanded.get()).then(|| view().accessible(false).app_class(tw("absolute inset-x-0 bottom-0 h-24 bg-linear-to-t from-card/95 via-card/80 to-transparent"))))))
            // Retain both buttons: NSH's OnceInstance holds button props through press commit.
            .child(dynamic(move || state.collapsible.get().then(|| untracked(|| view().app_class(tw("mt-4 flex justify-center"))
                .child(view().app_class(move || tw(if state.expanded.get() { "hidden" } else { "flex" }))
                    .child(button(ButtonContent::Label(t("Expand plan")), ButtonVariant::Outline, ButtonSize::Sm, || false, move || state.expanded.set(true))))
                .child(view().app_class(move || tw(if state.expanded.get() { "flex" } else { "hidden" }))
                    .child(button(ButtonContent::Label(t("Collapse plan")), ButtonVariant::Outline, ButtonSize::Sm, || false, move || state.expanded.set(false)))))))))
        .child(dialog::dialog(state.open, move |dialog| {
            dialog.on_open_change(move |open| if !state.saving.get_untracked() { state.open.set(open); });
            dialog::dialog_popup(dialog, Default::default(), move || untracked(|| save_plan_to_workspace_form(row, state, dialog, saving_context.clone())))
        }))
}
fn plan_blocks(state: PlanState, preview: bool) -> impl IntoElement {
    let plugins = use_context::<MarkdownPlugins>().unwrap_or_default();
    provide_context(plugins.with(PlanHeadingOffset));
    view().child(each(
        move || state.keys.get(),
        |id| id.clone(),
        move |id, _| {
            let slot = state.slots.with(|slots| slots[id.get_untracked().index]);
            let shape = computed(move || {
                slot.with(|leaf| {
                    if matches!(
                        leaf.block.kind,
                        BlockKind::Header | BlockKind::Definition { .. }
                    ) || (preview && leaf.preview.is_empty())
                    {
                        0
                    } else if matches!(leaf.block.kind, BlockKind::Code(_)) {
                        2
                    } else {
                        3
                    }
                })
            });
            dynamic(move || {
                let shape = shape.get();
                untracked(|| match shape {
                    0 => view().into_element(),
                    2 => plan_code_chunk(slot, state, preview),
                    _ => chat_markdown(
                        move || {
                            slot.with(|leaf| {
                                if preview {
                                    leaf.preview.to_string()
                                } else {
                                    leaf.block.text.as_deref().unwrap_or("").to_owned()
                                }
                            })
                        },
                        || false,
                    )
                    .into_element(),
                })
            })
        },
    ))
}
struct PlanHeadingOffset;
impl MarkdownPlugin for PlanHeadingOffset {
    fn name(&self) -> &str {
        "proposed-plan-heading-offset"
    }
    fn block(
        &self,
        node: &mdast::Node,
        cx: &mut ParseContext<'_>,
    ) -> Option<nsh_base::text_view::Block> {
        let mdast::Node::Heading(heading) = node else {
            return None;
        };
        Some(nsh_base::text_view::Block::Heading {
            level: (heading.depth + 3).min(6),
            content: cx.inlines(&heading.children),
            anchor: None,
        })
    }
}
fn plan_code_chunk(slot: Signal<Shared<PlanLeaf>>, state: PlanState, preview: bool) -> AnyElement {
    let initial = slot.get_untracked();
    let BlockKind::Code(code) = &initial.block.kind else {
        return view().into_element();
    };
    let mut id = initial.block.id.clone();
    id.index = initial.code_first.unwrap_or(id.index);
    let copy_index = id.index;
    let code_state = state.owner.run(|| {
        state
            .code_states
            .with(|states| *states.entry(id).or_default())
    });
    let body = move || {
        text(move || {
            slot.with(|leaf| {
                SharedString::from(
                    if preview {
                        leaf.preview_code.as_ref()
                    } else {
                        leaf.block.text.as_deref().unwrap_or("")
                    }
                    .to_owned(),
                )
            })
        })
        .select_text()
        .app_class(move || {
            tw(&cn([
                Some("font-mono"),
                (!code_state.wrap_override.get().unwrap_or(true)).then_some("whitespace-nowrap"),
            ]))
        })
    };
    if !code.first {
        return body().into_element();
    }
    let props = computed(move || {
        slot.with(|leaf| {
            let BlockKind::Code(code) = &leaf.block.kind else {
                return CodeBlockProps::default();
            };
            CodeBlockProps {
                code: if preview {
                    leaf.preview_code.to_string()
                } else {
                    code.text.clone()
                },
                language: code.language.clone().unwrap_or_else(|| "text".into()),
                fence_title: extract_fence_title(code.meta.as_deref()),
                ..Default::default()
            }
        })
    });
    markdown_code_block(
        props,
        code_state,
        CodeBlockOptions {
            copy: Some(Rc::new(move |_| {
                let code = state.render.with(|render| {
                    let mut code = String::new();
                    for leaf in render.leaves.iter().skip(copy_index) {
                        let BlockKind::Code(chunk) = &leaf.block.kind else {
                            break;
                        };
                        if leaf.block.id.index > copy_index && chunk.first {
                            break;
                        }
                        code.push_str(&chunk.text);
                        if chunk.last {
                            break;
                        }
                    }
                    code
                });
                async move {
                    set_clipboard(code);
                    Ok(())
                }
                .boxed_local()
            })),
            ..Default::default()
        },
        move |_| body().into_element(),
    )
    .into_element()
}
fn save_plan_to_workspace_form(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    state: PlanState,
    dialog: dialog::DialogState,
    context: PlanContext,
) -> impl IntoElement {
    let owner = context.state.owner;
    let filename = contents(row).map(|(name, _)| name).unwrap_or_default();
    let path = state.draft_path;
    let can_write = context.can_write;
    let workspace = context.workspace_root;
    let write = context.write_file.clone();
    let has_writer = write.is_some();
    let save = Rc::new(move || {
        if state.saving.get_untracked() || !can_write.get_untracked() {
            return;
        }
        let Some(write) = write.clone() else {
            return;
        };
        let Some(cwd) = workspace.get_untracked() else {
            return;
        };
        let relative_path = trim_js(&path.get_untracked()).to_owned();
        if relative_path.is_empty() {
            plan_toast(toast::Variant::Warning, "Enter a workspace path", None);
            return;
        }
        let Some((_, contents)) = contents(row) else {
            return;
        };
        state.last_path.set(path.get_untracked());
        state.saving.set(true);
        let request = SavePlan {
            thread: context.thread.clone(),
            cwd,
            relative_path,
            contents,
        };
        owner.run(|| {
            spawn(async move {
                let result = write(request).await;
                state.saving.set(false);
                match result {
                    Ok(Some(path)) => {
                        state.open.set(false);
                        plan_toast(
                            toast::Variant::Success,
                            "Plan saved to workspace",
                            Some(path),
                        );
                    }
                    Ok(None) => {}
                    Err(error) => {
                        plan_toast(toast::Variant::Error, "Could not save plan", Some(error))
                    }
                }
            })
        });
    });
    let submit = save.clone();
    view()
        .child(dialog::dialog_header(
            dialog,
            view()
                .child(dialog::dialog_title(dialog, || t("Save plan to workspace")))
                .child(
                    view()
                        .app_class(tw("hidden"))
                        .child(dialog::dialog_description(dialog, move || {
                            tr(
                                "Enter a path relative to {workspace}.",
                                &[(
                                    "workspace",
                                    Value::Text(
                                        workspace.get().unwrap_or_else(|| t("the workspace")),
                                    ),
                                )],
                            )
                        })),
                )
                .child(workspace_description(workspace)),
        ))
        .child(dialog::dialog_panel(
            dialog,
            view()
                .app_class(tw("grid gap-1.5"))
                .child(
                    text(t("Workspace path")).app_class(tw("text-xs font-medium text-foreground")),
                )
                .child(text_field(
                    path,
                    filename,
                    t("Workspace path"),
                    InputSize::Default,
                    move || submit(),
                )),
        ))
        .child(dialog::dialog_footer(
            dialog,
            dialog::FooterVariant::Default,
            view()
                .app_class(tw("flex gap-2"))
                .child(button(
                    ButtonContent::Label(t("Cancel")),
                    ButtonVariant::Outline,
                    ButtonSize::Sm,
                    move || state.saving.get(),
                    move || {
                        if !state.saving.get_untracked() {
                            state.open.set(false);
                        }
                    },
                ))
                .child(
                    view()
                        .app_class(move || tw(if state.saving.get() { "hidden" } else { "flex" }))
                        .tooltip(plan_write_hint(state, can_write, workspace, has_writer))
                        .child(button(
                            ButtonContent::Label(t("Save")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            move || !has_writer || !can_write.get() || state.saving.get(),
                            move || save(),
                        )),
                )
                .child(
                    view()
                        .app_class(move || tw(if state.saving.get() { "flex" } else { "hidden" }))
                        .child(button(
                            ButtonContent::Label(t("Saving...")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            || true,
                            || {},
                        )),
                ),
        ))
}

fn workspace_description(workspace: Memo<Option<String>>) -> impl IntoElement {
    nsh_base::text_view()
        .document(move || {
            let message = tr(
                "Enter a path relative to {workspace}.",
                &[("workspace", Value::Text("\u{fffc}".into()))],
            );
            let (before, after) = message.split_once('\u{fffc}').unwrap_or((&message, ""));
            Some(Document::new(vec![TextBlock::paragraph(vec![
                Inline::text(before),
                Inline::Code {
                    code: workspace.get().unwrap_or_else(|| t("the workspace")),
                },
                Inline::text(after),
            ])]))
        })
        .classes(TextViewClasses {
            code: Some(tw("font-mono")),
            ..Default::default()
        })
        .app_class(tw("text-muted-foreground text-sm"))
}

fn plan_write_hint(
    state: PlanState,
    can_write: Memo<bool>,
    workspace: Memo<Option<String>>,
    has_writer: bool,
) -> impl IntoElement + nsh_core::element::IntoTooltip {
    let hint = tooltip::tooltip_create_handle();
    effect(move || {
        hint.set_disabled(
            has_writer && can_write.get() && workspace.get().is_some() && !state.saving.get(),
        );
    });
    tooltip::tooltip(
        hint,
        Default::default(),
        text(move || {
            if state.saving.get() {
                t("Saving...")
            } else if workspace.get().is_none() {
                t("Workspace path is unavailable")
            } else {
                t("This connection cannot save files.")
            }
        }),
    )
}
