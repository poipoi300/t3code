//! Port of apps/web/src/components/ComposerPromptEditorTiptap.tsx ComposerMentionNodeView and sibling chip node views.

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::Arc,
};

use nsh_base::rich_editor::{
    model::Node,
    view::{EditorSession, NodeView, NodeViewCx, RichEditorPastedFiles},
};
use nsh_core::prelude::*;
use t3_client::{
    services::{assets::format_attachment_size, providers::provider_instance_display_name},
    thread_views::{
        composer::{
            chip_presentation::{PullRequestDisplayState, review_comment_presentation},
            citations,
        },
        user_message::Reference,
    },
    types::ThreadKey,
    view_logic::session::MessageAttachment,
};
use t3_contracts::{
    assistant_citations::AssistantCitation,
    composer_context::{
        ComposerContextRecord, ElementContextRecord, ElementContextSource,
        PreviewAnnotationContextRecord, PullRequestContextMetadata, TerminalContextRecord,
    },
    server::{ServerProvider, ServerProviderSkill},
    wire::trim_js,
};

use super::editor::{
    ComposerEditor, ComposerPromptEditorProps, ComposerSnapshot, composer_prompt_editor_tiptap,
};
use crate::{
    i18n::{Text, Value, t, tr},
    theme::{self, tw},
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        icon::{Icon, icon_with_class, provider_icon},
        popover::{self, Padding, PopoverOptions},
        tooltip::{self, TooltipOptions},
    },
};
use t3_client::thread_views::composer::{
    attachment_intake::{AttachmentItem, UploadState},
    web_drafts::kind_context_id,
};

pub type MentionAction = Rc<dyn Fn(&str)>;
pub type AttachmentAction = Rc<dyn Fn(&str)>;
pub type CitationAction = Rc<dyn Fn(&AssistantCitation)>;

/// Override for containing blocks whose font differs from the user message's `text-sm`.
#[derive(Clone, Copy)]
pub struct ChipFontSize(pub Prop<f32>);

#[derive(Clone, Copy)]
struct SentChip;

fn chip_font_size() -> impl Fn() -> f32 + Copy {
    let size = use_context::<ChipFontSize>().map(|size| size.0);
    move || size.map_or(14.0, |size| size.get())
}

/// Client-owned records and catalogues; this renderer never tokenizes prompt or message text.
#[derive(Clone, Default, PartialEq)]
pub struct ChipData {
    pub records: BTreeMap<String, ComposerContextRecord>,
    pub skills: Vec<ServerProviderSkill>,
    pub providers: Vec<ServerProvider>,
    pub previews: BTreeMap<String, String>,
    pub video_attachments: BTreeSet<String>,
    pub message_attachments: BTreeMap<String, MessageAttachment>,
    pub draft_attachments: Vec<AttachmentItem>,
}

#[derive(Clone)]
pub struct ChipContext {
    pub data: Memo<ChipData>,
    pub open_mention: Option<MentionAction>,
    pub open_attachment: Option<AttachmentAction>,
    pub open_citation: Option<CitationAction>,
    pub open_pull_request: Option<MentionAction>,
}

impl ChipContext {
    pub fn new(data: Memo<ChipData>) -> Self {
        Self {
            data,
            open_mention: None,
            open_attachment: None,
            open_citation: None,
            open_pull_request: None,
        }
    }

    /// Capture the panel handle during construction, never from a press handler.
    pub fn for_thread(data: Memo<ChipData>, key: ThreadKey) -> Self {
        let mut context = Self::new(data);
        if let Some(panel) = use_context::<crate::screens::panel::PanelHost>() {
            context.open_mention = Some(Rc::new(move |path| {
                panel.apply(
                    &key,
                    t3_client::view_logic::shell::panels::Action::OpenFile {
                        path: path.into(),
                        line: None,
                    },
                );
            }));
        }
        context
    }
}

/// Mount at the editor's component boundary; sent rows call `user_message_context_reference_chip`.
pub fn composer_prompt_editor_with_chips(
    editor: ComposerEditor,
    props: ComposerPromptEditorProps,
    context: ChipContext,
    on_change: impl Fn(ComposerSnapshot) + 'static,
    on_paste_files: impl Fn(&RichEditorPastedFiles) + 'static,
) -> impl IntoElement {
    install_composer_chip_views(&editor.session());
    let font = use_context::<ChipFontSize>().unwrap_or_else(|| {
        ChipFontSize(Prop::bind(|| {
            let size = theme::current_tokens()
                .number("font-size-prompt")
                .unwrap_or(14.0);
            if crate::state::layout::layout_class().is_compact()
                && crate::state::layout::pointer() == crate::state::layout::Pointer::Coarse
            {
                size.max(16.0)
            } else {
                size
            }
        }))
    });
    // NodeViews read their context when mounted, under this component's owner.
    crate::ui::popover::with_popup_context(context, move || {
        provide_context(font);
        composer_prompt_editor_tiptap(editor, props, on_change, on_paste_files).into_element()
    })
}

/// Merge with existing code-block views instead of replacing another package's registrations.
pub fn install_composer_chip_views(session: &EditorSession) {
    let mut views = session.node_views();
    for name in [
        "composer-mention",
        "composer-skill",
        "composer-model",
        "composer-citation",
        "composer-context-reference",
    ] {
        views.insert(name, Arc::new(ComposerChipNodeView(name)));
    }
    session.set_node_views(views);
}

struct ComposerChipNodeView(&'static str);

// Dynamic regions can be replaced before mounting; controls create state under their component.
/// Every chip region reads its inputs first and builds here: a theme or translation read during
/// the build must not rebuild the chip and dispose its buttons.
fn chip_scope(build: impl FnOnce() -> AnyElement + 'static) -> AnyElement {
    untracked(|| popover::with_popup_context((), build).into_element())
}

impl NodeView for ComposerChipNodeView {
    fn create(&self, cx: &NodeViewCx, node: &Node) -> AnyElement {
        let node = node.clone();
        let selected = cx.selected();
        let context = use_context::<ChipContext>()
            .unwrap_or_else(|| ChipContext::new(computed(ChipData::default)));
        // The editor may call `create` while tracking; the frame's theme read must not subscribe it.
        untracked(move || view()
            .app_class(tw(
                "relative inline-flex min-w-0 max-w-full select-none items-baseline",
            ))
            // NSH recreates inline NodeViews for changed attributes; avoid another transient region.
            .child(untracked(move || match node.ty().name() {
                "composer-mention" => composer_mention_node_view(&node, context),
                "composer-skill" => composer_skill_node_view(&node, context),
                "composer-model" => composer_model_node_view(&node, context),
                "composer-citation" => composer_citation_node_view(&node, context),
                "composer-context-reference" => {
                    composer_context_reference_node_view(&node, context)
                }
                _ => text(SharedString::from(attr(&node, "source").to_owned())).into_element(),
            }))
            // NSH has no CSS system Highlight colour; the theme's selection accent replaces it.
            .child(
                view()
                    .pointer_events(PointerEvents::None)
                    .app_class(move || {
                        Some(tw(if selected.get() {
                            "absolute inset-0 rounded-sm bg-primary opacity-30"
                        } else {
                            "hidden"
                        }))
                    }),
            )
            .into_element())
    }

    fn update(&self, node: &Node, _: &[nsh_base::rich_editor::state::Decoration]) -> bool {
        node.ty().name() == self.0
    }
}

fn attr<'a>(node: &'a Node, key: &str) -> &'a str {
    node.attrs()
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

/// Port of ContextChip.tsx ContextChip. Subtree variables become a direct token colour;
/// SVG selectors live on the glyph. NSH has no DOM attribute or custom clipboard-MIME slot.
fn context_chip_class(kind: &str) -> SharedString {
    let color = match kind {
        "image" => "text-(--context-chip-image)",
        "video" => "text-(--context-chip-video)",
        "file" => "text-(--context-chip-file)",
        "mention" => "text-(--context-chip-mention)",
        "terminal" => "text-(--context-chip-terminal)",
        "element" => "text-(--context-chip-element)",
        "preview-annotation" => "text-(--context-chip-preview-annotation)",
        "review-comment" => "text-(--context-chip-review-comment)",
        "pull-request" => "text-(--context-chip-pull-request)",
        "pr-open" => "text-(--context-chip-pr-open)",
        "pr-draft" => "text-(--context-chip-pr-draft)",
        "pr-merged" => "text-(--context-chip-pr-merged)",
        "pr-closed" => "text-(--context-chip-pr-closed)",
        "skill" => "text-(--context-chip-skill)",
        "model" => "text-(--context-chip-model)",
        "thread" => "text-(--context-chip-thread)",
        "environment" => "text-(--context-chip-environment)",
        "citation" => "text-(--context-chip-citation)",
        "doc-pdf" => "text-(--context-chip-doc-pdf)",
        "doc-word" => "text-(--context-chip-doc-word)",
        "doc-spreadsheet" => "text-(--context-chip-doc-spreadsheet)",
        "doc-slides" => "text-(--context-chip-doc-slides)",
        "doc-epub" => "text-(--context-chip-doc-epub)",
        "invalid" => "text-(--context-chip-invalid)",
        _ => "text-current",
    };
    // Element/hover selectors are resolved on the real control and its label below.
    tw(&theme::cn([
        Some(
            "inline-flex max-w-full items-center font-medium focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-current rounded-sm disabled:cursor-default",
        ),
        Some(color),
    ]))
}

fn interactive_chip_class(kind: &str) -> SharedString {
    let base = context_chip_class(kind);
    // ContextChipLabel's :is(button:enabled,a):hover selector becomes a named native group.
    tw(&theme::cn([
        Some(base.as_str()),
        Some("group/chip cursor-pointer"),
    ]))
}

fn sent_chip_ink(kind: &str) -> impl Fn() -> AdaptiveColor + use<> {
    let accent = format!("context-chip-{kind}");
    move || {
        // The bubble's optional descendant variable has no default token/class in NSH.
        let tokens = theme::current_tokens();
        tokens
            .color("context-chip-message")
            .or_else(|| tokens.color(&accent))
            .unwrap_or_else(|| theme::palette().message_foreground)
    }
}

/// Port of ContextChip.tsx ContextChipLabel; NSH text owns single-line truncation.
pub fn context_chip_label(label: Text) -> impl IntoElement {
    let size = chip_font_size();
    text(move || label.resolve())
        // NSH's box lengths have no em unit; the containing font supplies the 0.2em offset.
        .app_class(tw("block min-w-0 self-baseline overflow-hidden text-ellipsis underline decoration-transparent decoration-1 transition-colors motion-reduce:transition-none group-hover/chip:decoration-current"))
        .underline_offset(move || px(size() * 0.2))
        .number_of_lines(1_u32)
}

fn glyph(which: Icon) -> AnyElement {
    let size = chip_font_size();
    svg(which.asset())
        .app_class(tw("block shrink-0 self-center"))
        .w(move || px(size()))
        .h(move || px(size()))
        .accessible(false)
        .into_element()
}

fn image_chip_thumbnail(url: String) -> AnyElement {
    let size = chip_font_size();
    image(url)
        .app_class(tw("shrink-0 self-center rounded-sm object-cover"))
        .w(move || px(size()))
        .h(move || px(size()))
        .accessible(false)
        .into_element()
}

fn model_glyph(driver: &str, name: &str) -> AnyElement {
    let size = chip_font_size();
    view()
        .app_class(tw("shrink-0 self-center"))
        .w(move || px(size()))
        .h(move || px(size()))
        .child(provider_icon(
            driver,
            name,
            "z-auto size-full shrink-0 self-center",
        ))
        .into_element()
}

/// Port of contextChipParts.tsx ContextChipPopover; content is mounted only while open.
fn context_chip_popover(
    kind: &str,
    icon: AnyElement,
    label: Text,
    accessible_label: Text,
    content: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    context_chip_popover_with_handle(
        popover::popover_create_handle(),
        kind,
        icon,
        label,
        accessible_label,
        content,
    )
}

fn context_chip_popover_with_handle(
    handle: popover::PopoverHandle,
    kind: &str,
    icon: AnyElement,
    label: Text,
    accessible_label: Text,
    content: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    let size = chip_font_size();
    let trigger = nsh_base::button()
        .app_class(interactive_chip_class(kind))
        .when(use_context::<SentChip>().is_some(), |chip| {
            chip.text_color(sent_chip_ink(kind))
        })
        // gap-[0.2em] and leading-[normal] are bound through NSH's typed style API.
        .gap(move || px(size() * 0.2))
        .line_height(nsh_core::LineHeight::Normal)
        .accessibility_label(move || {
            Some(
                tr(
                    "{label}. Show details",
                    &[("label", Value::Text(accessible_label.resolve().to_string()))],
                )
                .into(),
            )
        })
        .on_layout(move |bounds: &Bounds<Points>| handle.measure_anchor(*bounds))
        .child(icon)
        .child(context_chip_label(label));
    popover::popover(
        handle,
        PopoverOptions {
            width: popover::Size::Lg,
            padding: Padding::Compact,
            side: Placement::Top,
            ..Default::default()
        },
        trigger,
        content,
    )
    .into_element()
}

fn shell(
    kind: &str,
    icon: AnyElement,
    label: Text,
    accessible_label: Text,
    hint: Option<Text>,
    action: Option<Rc<dyn Fn()>>,
) -> AnyElement {
    context_chip(
        kind,
        icon,
        context_chip_label(label).into_element(),
        accessible_label,
        hint,
        action,
    )
}

/// Port of ContextChip.tsx ContextChip's element and child order.
fn context_chip(
    kind: &str,
    icon: AnyElement,
    label: AnyElement,
    accessible_label: Text,
    hint: Option<Text>,
    action: Option<Rc<dyn Fn()>>,
) -> AnyElement {
    let size = chip_font_size();
    let control = if let Some(action) = action {
        nsh_base::button()
            .app_class(interactive_chip_class(kind))
            .when(use_context::<SentChip>().is_some(), |chip| {
                chip.text_color(sent_chip_ink(kind))
            })
            .gap(move || px(size() * 0.2))
            .line_height(nsh_core::LineHeight::Normal)
            .accessibility_label(move || Some(accessible_label.resolve()))
            .on_press(move || action())
            .child(icon)
            .child(label)
            .into_element()
    } else {
        view()
            .app_class(context_chip_class(kind))
            .when(use_context::<SentChip>().is_some(), |chip| {
                chip.text_color(sent_chip_ink(kind))
            })
            .gap(move || px(size() * 0.2))
            .line_height(nsh_core::LineHeight::Normal)
            .focusable(true)
            .accessible(true)
            .label(move || accessible_label.resolve())
            .child(icon)
            .child(label)
            .into_element()
    };
    let Some(hint) = hint else {
        return control;
    };
    let target = element_ref();
    let handle = tooltip::tooltip_create_handle();
    target.on_focus_in(move || handle.set_open(true));
    target.on_focus_out(move || handle.set_open(false));
    // ButtonElement has no tooltip slot; the wrapper keeps the original press target.
    view()
        .element_ref(&target)
        .app_class(tw("inline-flex min-w-0 max-w-full"))
        .tooltip(tooltip::tooltip(
            handle,
            TooltipOptions::default(),
            text(move || hint.resolve()),
        ))
        .child(control)
        .into_element()
}

/// Port of ComposerPromptEditorTiptap.tsx ComposerMentionNodeView.
pub fn composer_mention_node_view(node: &Node, context: ChipContext) -> AnyElement {
    let path = attr(node, "path").to_owned();
    // basenameOfPath intentionally splits only '/', matching the web even for Windows source.
    let label = path.rsplit('/').next().unwrap_or(&path).to_owned();
    mention_chip(path, label, context)
}

fn mention_chip(path: String, label: String, context: ChipContext) -> AnyElement {
    let action = context.open_mention.map(|open| {
        let path = path.clone();
        Rc::new(move || open(&path)) as Rc<dyn Fn()>
    });
    shell(
        document_kind(&path).unwrap_or("mention"),
        // FILES-*: WorkspaceEntryIcon metadata/Pierre glyphs are not exposed by the shared UI yet.
        glyph(Icon::File),
        Text::content(label),
        Text::content(tr("Preview {path}", &[("path", Value::Text(path.clone()))])),
        // FILES-* must supply WorkspaceEntryTooltip; a path-only hint is not its metadata view.
        None,
        action,
    )
}

/// Port of pierre-icons.ts documentChipKindForFileName (presentation only).
fn document_kind(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "pdf" => Some("doc-pdf"),
        "doc" | "docx" | "odt" | "rtf" => Some("doc-word"),
        "xls" | "xlsx" | "ods" | "csv" | "tsv" => Some("doc-spreadsheet"),
        "ppt" | "pptx" | "odp" => Some("doc-slides"),
        "epub" => Some("doc-epub"),
        _ => None,
    }
}

/// Port of ComposerPromptEditorTiptap.tsx ComposerSkillNodeView.
pub fn composer_skill_node_view(node: &Node, context: ChipContext) -> AnyElement {
    let node = node.clone();
    chip_scope(move || composer_skill_node_view_inner(&node, context))
}

fn composer_skill_node_view_inner(node: &Node, context: ChipContext) -> AnyElement {
    let popup = popover::popover_create_handle();
    let name = attr(node, "skillName").to_owned();
    let stored_label = attr(node, "skillLabel").to_owned();
    let stored_description = attr(node, "skillDescription").to_owned();
    let data = context.data;
    let presentation = computed(move || {
        let skill = data.with(|data| {
            data.skills
                .iter()
                .find(|skill| skill.name.0 == name && skill.user_invocable != Some(Some(false)))
                .cloned()
        });
        let label = skill
            .as_ref()
            .map(|skill| {
                optional_string(&skill.display_name)
                    .unwrap_or(&skill.name.0)
                    .to_owned()
            })
            .unwrap_or_else(|| {
                if stored_label.is_empty() {
                    name.clone()
                } else {
                    stored_label.clone()
                }
            });
        let description = skill
            .as_ref()
            .map(|skill| {
                optional_string(&skill.description)
                    .or_else(|| optional_string(&skill.short_description))
                    .unwrap_or_default()
                    .to_owned()
            })
            .unwrap_or_else(|| stored_description.clone());
        (label, description, skill.map(|skill| skill.path.0))
    });
    dynamic(move || {
        let (label, description, path) = presentation.get();
        let instructions = path.and_then(|path| {
            let open = context.open_mention.clone()?;
            Some(Rc::new(move || open(&path)) as Rc<dyn Fn()>)
        });
        chip_scope(move || skill_chip_popover_with_handle(popup, label, description, instructions))
    })
    .into_element()
}

fn optional_string(
    value: &Option<Option<t3_contracts::base_schemas::TrimmedNonEmptyString>>,
) -> Option<&str> {
    value
        .as_ref()?
        .as_ref()
        .map(|value| trim_js(&value.0))
        .filter(|value| !value.is_empty())
}

/// Port of chat/SkillInlineText.tsx SkillChipPopover.
pub fn skill_chip_popover(
    label: String,
    description: String,
    instructions: Option<Rc<dyn Fn()>>,
) -> AnyElement {
    skill_chip_popover_with_handle(
        popover::popover_create_handle(),
        label,
        description,
        instructions,
    )
}

fn skill_chip_popover_with_handle(
    popup: popover::PopoverHandle,
    label: String,
    description: String,
    instructions: Option<Rc<dyn Fn()>>,
) -> AnyElement {
    let accessible = Text::content(tr(
        "Skill {label}",
        &[("label", Value::Text(label.clone()))],
    ));
    context_chip_popover_with_handle(
        popup,
        "skill",
        glyph(Icon::Box),
        Text::content(label.clone()),
        accessible,
        move || {
            view()
                .app_class(tw("space-y-3 p-2 text-sm"))
                .child(text(SharedString::from(label.clone())).app_class(tw("font-medium")))
                .child(text(if description.is_empty() {
                    t("No description is available for this skill.").into()
                } else {
                    SharedString::from(description.clone())
                }))
                .child(instructions.clone().map(|open| {
                    button(
                        ButtonContent::Label(t("View instructions")),
                        ButtonVariant::Outline,
                        ButtonSize::Sm,
                        || false,
                        move || open(),
                    )
                }))
                .into_element()
        },
    )
}

/// Port of ComposerPromptEditorTiptap.tsx ComposerModelNodeView and chat/ModelMentionChip.tsx.
pub fn composer_model_node_view(node: &Node, context: ChipContext) -> AnyElement {
    let node = node.clone();
    chip_scope(move || composer_model_node_view_inner(&node, context))
}

fn composer_model_node_view_inner(node: &Node, context: ChipContext) -> AnyElement {
    let popup = popover::popover_create_handle();
    let instance = attr(node, "instanceId").to_owned();
    let model = attr(node, "model").to_owned();
    let source = attr(node, "source").to_owned();
    let presentation = computed(move || {
        context.data.with(|data| {
            let provider = data
                .providers
                .iter()
                .find(|provider| provider.instance_id.0 == instance)?;
            let option = provider
                .models
                .iter()
                .find(|option| option.slug.0 == model)?;
            Some((
                optional_string(&option.short_name)
                    .unwrap_or(&option.name.0)
                    .to_owned(),
                optional_string(&option.sub_provider)
                    .map(str::to_owned)
                    .unwrap_or_else(|| provider_instance_display_name(provider)),
                provider.driver.0.clone(),
            ))
        })
    });
    dynamic(move || {
        let presentation = presentation.get();
        let source = source.clone();
        chip_scope(move || {
            let Some((label, provider, driver)) = presentation else {
                return text(SharedString::from(source.clone())).into_element();
            };
            let accessible = Text::content(tr(
                "Model {label}",
                &[("label", Value::Text(label.clone()))],
            ));
            let source = source.clone();
            context_chip_popover_with_handle(
                popup,
                "neutral",
                model_glyph(&driver, &provider),
                Text::content(label.clone()),
                accessible,
                move || {
                    view()
                        .app_class(tw("space-y-1 p-2 text-sm"))
                        .child(text(SharedString::from(label.clone())).app_class(tw("font-medium")))
                        .child(
                            text(SharedString::from(provider.clone()))
                                .app_class(tw("text-muted-foreground")),
                        )
                        .child(
                            text(SharedString::from(source.clone()))
                                .app_class(tw("font-mono text-xs text-muted-foreground")),
                        )
                        .into_element()
                },
            )
        })
    })
    .into_element()
}

/// Port of ComposerPromptEditorTiptap.tsx ComposerCitationNodeView.
pub fn composer_citation_node_view(node: &Node, context: ChipContext) -> AnyElement {
    let Some(citation) = node
        .attrs()
        .get("citation")
        .cloned()
        .and_then(|value| serde_json::from_value::<AssistantCitation>(value).ok())
    else {
        return text(SharedString::from(attr(node, "source").to_owned())).into_element();
    };
    assistant_citation_chip(citation, context)
}

/// Port of chat/AssistantCitationChip.tsx AssistantCitationChip's source link.
pub fn assistant_citation_chip(citation: AssistantCitation, context: ChipContext) -> AnyElement {
    let label = citations::label(&citation);
    let action = context
        .open_citation
        .map(|open| Rc::new(move || open(&citation)) as Rc<dyn Fn()>);
    shell(
        "citation",
        glyph(Icon::Quote),
        Text::content(label.clone()),
        Text::content(tr(
            "View cited assistant text: {label}",
            &[("label", Value::Text(label))],
        )),
        None,
        action,
    )
}

/// Port of ComposerPromptEditorTiptap.tsx ComposerContextReferenceNodeView.
pub fn composer_context_reference_node_view(node: &Node, context: ChipContext) -> AnyElement {
    composer_context_reference_chip(
        attr(node, "kind").into(),
        attr(node, "contextId").into(),
        attr(node, "label").into(),
        context,
    )
}

/// Port of composerContextPresentation.tsx ComposerContextReferenceChip.
pub fn composer_context_reference_chip(
    kind: String,
    id: String,
    label: String,
    context: ChipContext,
) -> AnyElement {
    chip_scope(move || composer_context_reference_chip_inner(kind, id, label, context))
}

fn composer_context_reference_chip_inner(
    kind: String,
    id: String,
    label: String,
    context: ChipContext,
) -> AnyElement {
    dynamic(move || {
        let record = context.data.with(|data| data.records.get(&id).cloned());
        let attachment = context.data.with(|data| {
            data.draft_attachments
                .iter()
                .find(|item| {
                    let item_kind = if item.image { "image" } else { "file" };
                    kind == item_kind && kind_context_id(item_kind, &item.id) == id
                })
                .cloned()
        });
        let context = context.clone();
        let (kind, id, label) = (kind.clone(), id.clone(), label.clone());
        chip_scope(move || {
            attachment.map_or_else(
                || {
                    render_context_reference(
                        &kind,
                        &id,
                        &label,
                        record.as_ref(),
                        context.clone(),
                        true,
                    )
                },
                |item| draft_attachment_chip(item, context.clone()),
            )
        })
    })
    .into_element()
}

/// Port of chat/UserMessageContextChips.tsx UserMessageContextReferenceChip.
/// This is the same renderer used by editor context atoms, with the sent-message fallback copy.
pub fn user_message_context_reference_chip(
    reference: &Reference,
    record: Option<&ComposerContextRecord>,
    context: ChipContext,
) -> AnyElement {
    let reference = reference.clone();
    let record = record.cloned();
    untracked(|| {
        popover::with_popup_context(SentChip, move || {
            render_context_reference(
                &reference.kind,
                &reference.context_id,
                &reference.label,
                record.as_ref(),
                context,
                false,
            )
        })
        .into_element()
    })
}

/// Fits UserRowContext::context_chip directly; the row supplies its canonical records.
pub fn user_message_chip_renderer(
    context: ChipContext,
) -> crate::screens::thread::rows::user::ContextChipRenderer {
    Rc::new(move |reference, record, row| {
        if !matches!(
            reference.kind.as_str(),
            "image" | "file" | "preview-annotation"
        ) {
            return user_message_context_reference_chip(reference, record, context.clone());
        }
        let attachments = row
            .message
            .attachments
            .as_ref()
            .map(|files| {
                files
                    .iter()
                    .map(|file| {
                        let id = match &file.attachment {
                            t3_contracts::chat_attachment::ChatAttachment::Image(file) => &file.id,
                            t3_contracts::chat_attachment::ChatAttachment::File(file) => &file.id,
                            t3_contracts::chat_attachment::ChatAttachment::Unknown(file) => {
                                &file.id
                            }
                        };
                        (id.clone(), file.clone())
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let mut previews = BTreeMap::new();
        for image in &row.images {
            if let t3_contracts::chat_attachment::ChatAttachment::Image(file) = &image.attachment
                && let Some(url) = &image.preview_url
            {
                previews.insert(file.id.clone(), url.clone());
            }
        }
        if let Some(ComposerContextRecord::PreviewAnnotation(annotation)) = record {
            let records = row
                .records
                .iter()
                .filter(|record| {
                    annotation
                        .screenshot_context_id
                        .as_ref()
                        .is_some_and(|id| record_id(record) == id.0)
                })
                .map(|record| (record_id(record).to_owned(), record.clone()))
                .collect();
            let annotations = row
                .records
                .iter()
                .filter_map(|record| match record {
                    ComposerContextRecord::PreviewAnnotation(record) => {
                        Some(record.context_id.0.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if let Some(image) =
                crate::screens::thread::rows::user::resolve_preview_annotation_image(
                    annotation,
                    &records,
                    &row.images,
                    &row.preview_images,
                    &annotations,
                )
                && let Some(url) = &image.preview_url
            {
                previews.insert(annotation.context_id.0.clone(), url.clone());
            }
        }
        let videos = row
            .videos
            .iter()
            .filter_map(|file| match &file.attachment {
                t3_contracts::chat_attachment::ChatAttachment::File(file) => Some(file.id.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let context = context.clone();
        let reference = reference.clone();
        let record = record.cloned();
        popover::with_popup_context(SentChip, move || {
            let mut context = context;
            let original = context.data;
            context.data = computed(move || {
                // Media chips need asset metadata, not copies of captured terminal/review text.
                let mut data = original.with(|data| ChipData {
                    previews: data.previews.clone(),
                    video_attachments: data.video_attachments.clone(),
                    message_attachments: data.message_attachments.clone(),
                    ..Default::default()
                });
                data.previews.extend(previews.clone());
                data.video_attachments.extend(videos.clone());
                data.message_attachments.extend(attachments.clone());
                data
            });
            render_context_reference(
                &reference.kind,
                &reference.context_id,
                &reference.label,
                record.as_ref(),
                context,
                false,
            )
        })
        .into_element()
    })
}

fn record_id(record: &ComposerContextRecord) -> &str {
    match record {
        ComposerContextRecord::Image(record) => &record.context_id.0,
        ComposerContextRecord::File(record) => &record.context_id.0,
        ComposerContextRecord::Terminal(record) => &record.context_id.0,
        ComposerContextRecord::Element(record) => &record.context_id.0,
        ComposerContextRecord::PreviewAnnotation(record) => &record.context_id.0,
        ComposerContextRecord::ReviewComment(record) => &record.context_id.0,
        ComposerContextRecord::Mention(record) => &record.context_id.0,
        ComposerContextRecord::Skill(record) => &record.context_id.0,
        ComposerContextRecord::Thread(record) => &record.context_id.0,
        ComposerContextRecord::Environment(record) => &record.context_id.0,
        ComposerContextRecord::Response(record) => &record.context_id.0,
        ComposerContextRecord::Unknown(record) => &record.context_id.0,
    }
}

fn record_kind(record: &ComposerContextRecord) -> &'static str {
    match record {
        ComposerContextRecord::Image(_) => "image",
        ComposerContextRecord::File(_) => "file",
        ComposerContextRecord::Terminal(_) => "terminal",
        ComposerContextRecord::Element(_) => "element",
        ComposerContextRecord::PreviewAnnotation(_) => "preview-annotation",
        ComposerContextRecord::ReviewComment(_) => "review-comment",
        ComposerContextRecord::Mention(_) => "mention",
        ComposerContextRecord::Skill(_) => "skill",
        ComposerContextRecord::Thread(_) => "thread",
        ComposerContextRecord::Environment(_) => "environment",
        ComposerContextRecord::Response(_) => "response",
        ComposerContextRecord::Unknown(_) => "",
    }
}

fn render_context_reference(
    kind: &str,
    id: &str,
    label: &str,
    record: Option<&ComposerContextRecord>,
    context: ChipContext,
    draft: bool,
) -> AnyElement {
    let Some(record) =
        record.filter(|record| record_kind(record) == kind && record_id(record) == id)
    else {
        return unresolved_chip(label.to_owned(), draft);
    };
    match record {
        ComposerContextRecord::Mention(record) => {
            mention_chip(record.path.0.clone(), record.label.clone(), context)
        }
        ComposerContextRecord::Skill(record) => {
            let skill = context.data.with(|data| {
                data.skills
                    .iter()
                    .find(|skill| skill.name.0 == record.name.0)
                    .cloned()
            });
            let description = skill
                .as_ref()
                .and_then(|skill| {
                    optional_string(&skill.description)
                        .or_else(|| optional_string(&skill.short_description))
                })
                .unwrap_or_default()
                .to_owned();
            let instructions = skill.and_then(|skill| {
                context
                    .open_mention
                    .clone()
                    .map(|open| Rc::new(move || open(&skill.path.0)) as Rc<dyn Fn()>)
            });
            skill_chip_popover(record.label.clone(), description, instructions)
        }
        ComposerContextRecord::Terminal(record) => {
            let expired = draft
                && !t3_client::view_logic::chat_view::TerminalContextDraft {
                    id: record.context_id.0.clone(),
                    terminal_label: record.terminal_label.0.clone(),
                    line_start: record.line_start,
                    line_end: record.line_end,
                    text: record.text.clone(),
                }
                .has_text();
            terminal_context_inline_chip_with_expiration(record.clone(), expired)
        }
        ComposerContextRecord::Image(record) => attachment_chip(
            &record.name.0,
            &record.attachment_id.0,
            record.size_bytes,
            "image",
            context,
            draft,
            label,
        ),
        ComposerContextRecord::File(record) => attachment_chip(
            &record.name.0,
            &record.attachment_id.0,
            record.size_bytes,
            "file",
            context,
            draft,
            label,
        ),
        ComposerContextRecord::PreviewAnnotation(record) => {
            let record = record.clone();
            let label = record.label.clone();
            let accessible = Text::content(tr(
                "Preview annotation, {label}",
                &[("label", Value::Text(label.clone()))],
            ));
            context_chip_popover(
                "preview-annotation",
                glyph(Icon::MousePointerClick),
                Text::content(label),
                accessible,
                move || {
                    let screenshot = record
                        .screenshot_context_id
                        .as_ref()
                        .and_then(|id| context.data.with(|data| data.previews.get(&id.0).cloned()))
                        .or_else(|| {
                            context
                                .data
                                .with(|data| data.previews.get(&record.context_id.0).cloned())
                        });
                    view().app_class(tw("overflow-hidden rounded-lg border border-border/70 bg-background/70"))
                    .child(screenshot.map_or_else(|| view().app_class(tw("border-b border-border/70 bg-muted/40 px-3 py-2 text-secondary-label text-xs")).child(text(t("Screenshot unavailable"))).into_element(), |url| image(url).app_class(tw("max-h-64 w-full border-b border-border/70 bg-muted object-contain")).accessible(true).label(t("Annotated preview crop")).into_element()))
                    .child(if draft {
                        view().app_class(tw("px-3 py-2.5 text-sm text-foreground")).child(text(SharedString::from(preview_annotation_tooltip(&record)))).into_element()
                    } else { user_message_preview_annotation_details(&record) })
                    .into_element()
                },
            )
        }
        ComposerContextRecord::Element(record) => {
            let record = record.clone();
            let label = record.label.clone();
            let accessible = Text::content(tr(
                "Browser element, {label}",
                &[("label", Value::Text(label.clone()))],
            ));
            context_chip_popover(
                "element",
                glyph(Icon::MousePointerClick),
                Text::content(label),
                accessible,
                move || user_message_element_details(&record),
            )
        }
        ComposerContextRecord::ReviewComment(record) => {
            let presentation = review_comment_presentation(record);
            if let Some(metadata) = &record.pull_request {
                return pull_request_chip(
                    metadata.clone(),
                    presentation.state,
                    presentation.label,
                    context,
                );
            }
            let record = record.clone();
            let label = presentation.label;
            let accessible = Text::content(if presentation.summary {
                tr(
                    "Pull request, {label}",
                    &[("label", Value::Text(label.clone()))],
                )
            } else {
                tr(
                    "Review comment, {label}",
                    &[("label", Value::Text(label.clone()))],
                )
            });
            context_chip_popover(
                if presentation.summary {
                    "pull-request"
                } else {
                    "review-comment"
                },
                glyph(if presentation.summary {
                    Icon::GitPullRequestArrow
                } else {
                    Icon::MessageCircle
                }),
                Text::content(label),
                accessible,
                move || {
                    view().app_class(tw("space-y-2 overflow-hidden rounded-lg border border-border/70 bg-background/70 p-3"))
                    .child(view().app_class(tw("space-y-1"))
                        .child(text(SharedString::from(record.file_path.0.clone())).app_class(tw("truncate text-xs font-medium text-foreground")))
                        .child(text(SharedString::from(format!("{} · {}", record.section_title, record.range_label))).app_class(tw("text-secondary-label text-2xs"))))
                    .child((!trim_js(&record.text).is_empty()).then(|| { let body = record.text.clone(); crate::screens::thread::markdown::base::chat_markdown(move || body.clone(), || false) }))
                    .child((!trim_js(&record.diff).is_empty()).then(|| source_preview(record.diff.clone(), "diff")))
                    .into_element()
                },
            )
        }
        ComposerContextRecord::Thread(record) => {
            let nav = use_context::<crate::state::nav::Navigator>();
            let key = ThreadKey {
                environment: t3_client::types::EnvironmentId(record.environment_id.0.clone()),
                thread: record.thread_id.clone(),
            };
            let action = nav.map(|nav| {
                Rc::new(move || nav.open(crate::state::nav::Route::Thread(key.clone())))
                    as Rc<dyn Fn()>
            });
            shell(
                "thread",
                glyph(Icon::MessagesSquare),
                Text::content(record.title.clone()),
                Text::content(tr(
                    "Thread, {title}",
                    &[("title", Value::Text(record.title.clone()))],
                )),
                Some(Text::from("Open thread")),
                action,
            )
        }
        ComposerContextRecord::Environment(record) => shell(
            "environment",
            glyph(Icon::Server),
            Text::content(record.label.clone()),
            Text::content(tr(
                "Machine, {label}",
                &[("label", Value::Text(record.label.clone()))],
            )),
            Some(Text::from("T3 Code environment")),
            None,
        ),
        ComposerContextRecord::Response(_) if draft => unresolved_chip(label.to_owned(), true),
        // TL-ROWS-USER/TL-MARKDOWN owns ResponseReplyCard; do not substitute an inline chip.
        ComposerContextRecord::Response(_) => view().into_element(),
        ComposerContextRecord::Unknown(_) => unresolved_chip(label.to_owned(), draft),
    }
}

fn pull_request_state(
    state: Option<PullRequestDisplayState>,
) -> (&'static str, Icon, &'static str, &'static str) {
    match state {
        Some(PullRequestDisplayState::Open) => (
            "pr-open",
            Icon::GitPullRequestArrow,
            "Open",
            "text-emerald-600 dark:text-emerald-300/90",
        ),
        Some(PullRequestDisplayState::Draft) => (
            "pr-draft",
            Icon::GitPullRequestDraft,
            "Draft",
            "text-zinc-500 dark:text-zinc-400/80",
        ),
        Some(PullRequestDisplayState::Merged) => (
            "pr-merged",
            Icon::GitMerge,
            "Merged",
            "text-violet-600 dark:text-violet-300/90",
        ),
        Some(PullRequestDisplayState::Closed) => (
            "pr-closed",
            Icon::GitPullRequestClosed,
            "Closed",
            "text-red-600 dark:text-red-300/90",
        ),
        None => ("pull-request", Icon::GitPullRequestArrow, "", ""),
    }
}

/// Port of contextChipParts.tsx PullRequestChip (metadata tooltip when a preview host is absent).
fn pull_request_chip(
    metadata: PullRequestContextMetadata,
    state: Option<PullRequestDisplayState>,
    label: String,
    context: ChipContext,
) -> AnyElement {
    let (kind, icon, state_label, tone) = pull_request_state(state);
    let accessible = Text::content(tr(
        "Open {state} pull request {label}: {title}",
        &[
            (
                "state",
                Value::Text(Text::client_key(state_label).resolve().to_string()),
            ),
            ("label", Value::Text(label.clone())),
            ("title", Value::Text(metadata.title.clone())),
        ],
    ));
    let action = context.open_pull_request.map(|open| {
        let url = metadata.url.clone();
        Rc::new(move || open(&url)) as Rc<dyn Fn()>
    });
    let chip = context_chip(
        kind,
        glyph(icon),
        context_chip_label(Text::content(label)).into_element(),
        accessible,
        None,
        action,
    );
    view()
        .app_class(tw("inline-flex min-w-0 max-w-full"))
        .tooltip(tooltip::tooltip(
            tooltip::tooltip_create_handle(),
            TooltipOptions::default(),
            pull_request_context_details(metadata, icon, state_label, tone),
        ))
        .child(chip)
        .into_element()
}

/// Port of PullRequestContextDetails.tsx PullRequestContextDetails.
fn pull_request_context_details(
    metadata: PullRequestContextMetadata,
    icon: Icon,
    state: &'static str,
    tone: &'static str,
) -> impl IntoElement {
    view()
        .app_class(tw("max-w-80 space-y-1 overflow-hidden py-0.5 text-left"))
        .child(
            view()
                .app_class(tw("flex items-center gap-1.5 text-xs font-medium"))
                .child(icon_with_class(
                    icon,
                    &theme::cn([Some("size-3.5 shrink-0"), Some(tone)]),
                ))
                .child(
                    text(tr(
                        "Pull request #{number}",
                        &[("number", Value::Int(metadata.number as i64))],
                    ))
                    .app_class(tw("text-foreground")),
                )
                .child(text(Text::client_key(state).resolve()).app_class(tw(tone))),
        )
        .child(text(SharedString::from(metadata.title)).app_class(tw("text-foreground")))
        .child(
            view()
                .app_class(tw(
                    "flex min-w-0 items-center gap-1 text-secondary-label text-3xs",
                ))
                .child(
                    text(SharedString::from(metadata.head_branch))
                        .app_class(tw("truncate font-mono")),
                )
                .child(icon_with_class(Icon::ArrowRight, "size-3 shrink-0"))
                .child(
                    text(SharedString::from(metadata.base_branch))
                        .app_class(tw("truncate font-mono")),
                ),
        )
}

fn source_label(source: Option<&ElementContextSource>) -> Option<String> {
    let source = source?;
    let file = source
        .file_name
        .as_deref()
        .filter(|file| !file.is_empty())?;
    Some(
        source
            .line_number
            .map_or_else(|| file.to_owned(), |line| format!("{file}:{line}")),
    )
}

/// Port of chat/UserMessageContextChips.tsx UserMessagePreviewAnnotationDetails's body.
fn user_message_preview_annotation_details(record: &PreviewAnnotationContextRecord) -> AnyElement {
    let title = record
        .page_title
        .as_deref()
        .map(t3_contracts::wire::trim_js)
        .filter(|title| !title.is_empty())
        .unwrap_or(&record.page_url);
    let title = if title.is_empty() {
        t("Preview annotation")
    } else {
        title.to_owned()
    };
    let elements = record.elements.as_deref().unwrap_or_default();
    view().app_class(tw("min-w-0 px-3 py-2.5"))
        .child(text(SharedString::from(title)).app_class(tw("text-message-foreground text-xs font-medium")))
        .child((!record.comment.is_empty()).then(|| text(SharedString::from(record.comment.clone())).app_class(tw("mt-1 text-sm"))))
        .child(view().app_class(tw("mt-1 flex items-center gap-2 text-secondary-label text-3xs"))
            .child((!record.target_summary.is_empty()).then(|| text(SharedString::from(record.target_summary.clone())).app_class(tw("truncate"))))
            .child((!record.style_changes.is_empty()).then(|| view().app_class(tw("inline-flex shrink-0 items-center gap-1"))
                .child(icon_with_class(Icon::Paintbrush, "size-3"))
                .child(text(SharedString::from(record.style_changes.len().to_string()))))))
        .child((!elements.is_empty()).then(|| view().app_class(tw("mt-2 space-y-2 border-t border-border/60 pt-2"))
            .children(elements.iter().map(|element| {
                let selector = element.selector.as_deref().filter(|selector| !selector.is_empty()).map(str::to_owned).unwrap_or_else(|| format!("<{}>", element.tag_name.0));
                view().app_class(tw("min-w-0"))
                    .child(view().app_class(tw("flex min-w-0 items-center gap-2 text-xs"))
                        .child(text(SharedString::from(selector)).app_class(tw("truncate font-mono text-message-foreground")))
                        .child(source_label(element.source.as_ref()).map(|label| text(SharedString::from(label)).app_class(tw("ml-auto shrink-0 text-secondary-label")))))
                    .child((!trim_js(&element.html_preview).is_empty()).then(|| scroll().app_class(tw("mt-1 max-h-24 rounded bg-muted/60 px-2 py-1.5 text-3xs leading-relaxed")).child(text(SharedString::from(trim_js(&element.html_preview).to_owned())).app_class(tw("font-mono")))))
            }).collect::<Vec<_>>()))).into_element()
}

/// Port of chat/UserMessageContextChips.tsx UserMessageElementDetails.
fn user_message_element_details(record: &ElementContextRecord) -> AnyElement {
    let title = record
        .page_title
        .as_deref()
        .map(t3_contracts::wire::trim_js)
        .filter(|title| !title.is_empty())
        .unwrap_or(&record.page_url)
        .to_owned();
    let selector = record
        .selector
        .as_deref()
        .filter(|selector| !selector.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("<{}>", record.tag_name.0));
    view()
        .app_class(tw(
            "max-w-full overflow-hidden rounded-lg border border-border/70 bg-background/70",
        ))
        .child(
            view()
                .app_class(tw("border-b border-border/70 px-3 py-2.5"))
                .child(
                    text(SharedString::from(title))
                        .app_class(tw("truncate text-message-foreground text-xs font-medium")),
                )
                .child(
                    text(SharedString::from(record.page_url.clone()))
                        .app_class(tw("mt-0.5 truncate text-secondary-label text-3xs")),
                ),
        )
        .child(
            view()
                .app_class(tw("space-y-2 px-3 py-2.5"))
                .child(
                    view()
                        .app_class(tw("flex min-w-0 items-center gap-2 text-xs"))
                        .child(
                            text(SharedString::from(selector))
                                .app_class(tw("truncate font-mono text-message-foreground")),
                        )
                        .child(source_label(record.source.as_ref()).map(|label| {
                            text(SharedString::from(label))
                                .app_class(tw("ml-auto shrink-0 text-secondary-label"))
                        })),
                )
                .child((!trim_js(&record.html_preview).is_empty()).then(|| {
                    view()
                        .app_class(tw(
                            "flex h-40 flex-col overflow-hidden rounded border border-border",
                        ))
                        .child(source_preview(record.html_preview.clone(), "html"))
                }))
                .child((!trim_js(&record.styles).is_empty()).then(|| {
                    view()
                        .app_class(tw(
                            "flex h-32 flex-col overflow-hidden rounded border border-border",
                        ))
                        .child(source_preview(record.styles.clone(), "css"))
                })),
        )
        .into_element()
}

/// Port of composerContextPresentation.tsx previewAnnotationTooltip; records carry captured counts.
fn preview_annotation_tooltip(record: &PreviewAnnotationContextRecord) -> String {
    let mut lines = vec![
        record
            .page_title
            .as_deref()
            .map(t3_contracts::wire::trim_js)
            .filter(|title| !title.is_empty())
            .unwrap_or(&record.page_url)
            .to_owned(),
    ];
    if !trim_js(&record.comment).is_empty() {
        lines.extend([String::new(), trim_js(&record.comment).into()]);
    }
    let elements = record
        .elements
        .as_ref()
        .map_or_else(|| record.element_ids.as_ref().map_or(0, Vec::len), Vec::len);
    let mut targets = Vec::new();
    if elements > 0 {
        targets.push(tr(
            "{count, plural, one {# element} other {# elements}}",
            &[("count", Value::Int(elements as i64))],
        ));
    }
    if let Some(count) = record.region_count.filter(|count| *count > 0) {
        targets.push(tr(
            "{count, plural, one {# region} other {# regions}}",
            &[("count", Value::Int(count as i64))],
        ));
    }
    if let Some(count) = record.stroke_count.filter(|count| *count > 0) {
        targets.push(tr(
            "{count, plural, one {# drawing} other {# drawings}}",
            &[("count", Value::Int(count as i64))],
        ));
    }
    if !record.style_changes.is_empty() {
        targets.push(tr(
            "{count, plural, one {# style change} other {# style changes}}",
            &[("count", Value::Int(record.style_changes.len() as i64))],
        ));
    }
    if !targets.is_empty() {
        lines.extend([String::new(), targets.join(", ")]);
    }
    lines.join("\n")
}

/// Port of contextChipParts.tsx UnresolvedChip and composerContextPresentation's fallback.
fn unresolved_chip(label: String, draft: bool) -> AnyElement {
    let accessible = Text::content(tr(
        "Unavailable context, {label}",
        &[("label", Value::Text(label.clone()))],
    ));
    shell(
        "neutral",
        glyph(Icon::CircleDashed),
        Text::content(label),
        accessible,
        Some(Text::from(if draft {
            "This context is no longer available. Remove it or attach it again."
        } else {
            "This context is no longer available."
        })),
        None,
    )
}

/// Port of chat/TerminalContextInlineChip.tsx TerminalContextInlineChip.
pub fn terminal_context_inline_chip(record: TerminalContextRecord) -> AnyElement {
    terminal_context_inline_chip_with_expiration(record, false)
}

fn terminal_context_inline_chip_with_expiration(
    record: TerminalContextRecord,
    expired: bool,
) -> AnyElement {
    let label = record.label.clone();
    let accessible = Text::content(tr(
        "Terminal excerpt, {label}",
        &[("label", Value::Text(label.clone()))],
    ));
    if expired {
        return shell(
            "invalid",
            glyph(Icon::Terminal),
            Text::content(label.clone()),
            Text::content(tr(
                "Terminal excerpt, {label}, expired",
                &[("label", Value::Text(label.clone()))],
            )),
            Some(Text::content(tr(
                "Terminal context expired. Remove and re-add {label} to include it in your message.",
                &[("label", Value::Text(label))],
            ))),
            None,
        );
    }
    if record.text.is_empty() {
        return shell(
            "terminal",
            glyph(Icon::Terminal),
            Text::content(label),
            accessible,
            None,
            None,
        );
    }
    context_chip_popover(
        "terminal",
        glyph(Icon::Terminal),
        Text::content(label),
        accessible,
        move || {
            view().app_class(tw("overflow-hidden rounded-md border border-border/70 bg-background/80"))
            .child(view().app_class(tw("flex items-center gap-2 border-b border-border/70 px-3 py-2"))
                .child(icon_with_class(Icon::Terminal, "size-4 shrink-0 text-success"))
                .child(text(SharedString::from(record.terminal_label.0.clone())).app_class(tw("min-w-0 truncate text-sm font-medium text-foreground")))
                .child(text(if record.line_start == record.line_end {
                    tr("Line {line}", &[("line", Value::Int(record.line_start as i64))])
                } else {
                    tr("Lines {start}–{end}", &[("start", Value::Int(record.line_start as i64)), ("end", Value::Int(record.line_end as i64))])
                }).app_class(tw("ml-auto shrink-0 text-secondary-label text-xs"))))
            // Text has no tab-size/white-space prop; captured output keeps native wrapping and tab stops.
            .child(scroll().app_class(tw("max-h-80"))
                .child(text(SharedString::from(record.text.clone())).app_class(tw("bg-muted p-3 font-mono text-foreground text-xs leading-relaxed outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                    .select_text().focusable(true).label(t("Captured terminal output"))))
            .into_element()
        },
    )
}

fn source_preview(source: String, _language: &str) -> AnyElement {
    // FILES-* owns ReadOnlySourcePreview; TextView preserves the captured source meanwhile.
    text(SharedString::from(source))
        .select_text()
        .app_class(tw("font-mono text-xs text-foreground"))
        .into_element()
}

fn attachment_chip(
    name: &str,
    id: &str,
    bytes: u64,
    kind: &str,
    context: ChipContext,
    draft: bool,
    fallback_label: &str,
) -> AnyElement {
    if draft {
        return attachment_chip_contents(name, id, bytes, kind, context, true, fallback_label);
    }
    let (name, id, kind, fallback_label) = (
        name.to_owned(),
        id.to_owned(),
        kind.to_owned(),
        fallback_label.to_owned(),
    );
    chip_scope(move || {
        let data = context.data;
        let asset_id = id.clone();
        let asset = computed(move || {
            data.with(|data| {
                (
                    data.message_attachments.get(&asset_id).cloned(),
                    data.previews.get(&asset_id).cloned(),
                    data.video_attachments.contains(&asset_id),
                )
            })
        });
        dynamic(move || {
            asset.with(|_| ());
            let (name, id, kind, fallback_label, context) = (
                name.clone(),
                id.clone(),
                kind.clone(),
                fallback_label.clone(),
                context.clone(),
            );
            chip_scope(move || {
                attachment_chip_contents(&name, &id, bytes, &kind, context, false, &fallback_label)
            })
        })
        .into_element()
    })
}

fn attachment_chip_contents(
    name: &str,
    id: &str,
    bytes: u64,
    kind: &str,
    context: ChipContext,
    draft: bool,
    fallback_label: &str,
) -> AnyElement {
    let attachment = context
        .data
        .with(|data| data.message_attachments.get(id).cloned());
    if !draft
        && !attachment.as_ref().is_some_and(|attachment| {
            matches!(
                (kind, &attachment.attachment),
                (
                    "image",
                    t3_contracts::chat_attachment::ChatAttachment::Image(_)
                ) | (
                    "file",
                    t3_contracts::chat_attachment::ChatAttachment::File(_)
                )
            )
        })
    {
        return unresolved_chip(fallback_label.into(), false);
    }
    let video = context
        .data
        .with(|data| data.video_attachments.contains(id));
    let kind = if video { "video" } else { kind };
    let name = match attachment.as_ref().map(|attachment| &attachment.attachment) {
        Some(t3_contracts::chat_attachment::ChatAttachment::File(file)) if !draft => {
            crate::screens::thread::rows::user::user_message_file_label(file)
        }
        _ => name.to_owned(),
    };
    let size = format_attachment_size(bytes);
    let arguments = [
        ("name", Value::Text(name.clone())),
        ("size", Value::Text(size.clone())),
    ];
    let accessible = Text::content(if kind == "image" {
        tr("Image attachment, {name}, {size}", &arguments)
    } else if video {
        tr("Video attachment, {name}, {size}", &arguments)
    } else {
        tr("File attachment, {name}, {size}", &arguments)
    });
    let preview = context
        .data
        .with(|data| data.previews.get(id).cloned())
        .or_else(|| {
            attachment
                .as_ref()
                .and_then(|attachment| attachment.preview_url.clone())
        });
    let action = context.open_attachment.map(|open| {
        let id = id.to_owned();
        Rc::new(move || open(&id)) as Rc<dyn Fn()>
    });
    let icon = if kind == "image" {
        preview.map_or_else(|| glyph(Icon::Image), image_chip_thumbnail)
    } else if video {
        glyph(Icon::Film)
    } else {
        glyph(Icon::File)
    };
    context_chip(
        if kind == "image" {
            "image"
        } else {
            document_kind(&name).unwrap_or(kind)
        },
        icon,
        file_chip_content(&name, &size, None).into_element(),
        accessible,
        (kind != "image" || draft).then(|| Text::content(format!("{name}\n{size}"))),
        action,
    )
}

/// Port of contextChipParts.tsx FileChipContent and ImageChipButton's label.
fn file_chip_content(name: &str, size: &str, suffix: Option<Text>) -> impl IntoElement + use<> {
    let font = chip_font_size();
    view().app_class(tw("flex min-w-0 self-baseline items-baseline overflow-hidden text-ellipsis underline decoration-transparent decoration-1 transition-colors motion-reduce:transition-none group-hover/chip:decoration-current"))
        .gap(move || px(font() * 0.33))
        .underline_offset(move || px(font() * 0.2))
        .child(text(SharedString::from(middle_truncate_attachment_name(name, 36))).app_class(tw("min-w-0 max-w-72 overflow-hidden text-ellipsis")).number_of_lines(1_u32))
        .child(text(SharedString::from(size.to_owned())).app_class(tw("shrink-0 text-3xs text-current")))
        .child(suffix.map(|suffix| text(move || suffix.resolve()).app_class(tw("text-3xs text-current"))))
}

/// Port of composerContextPresentation.tsx FileContextChip and ImageContextChip.
fn draft_attachment_chip(item: AttachmentItem, context: ChipContext) -> AnyElement {
    let suffix = if item.needs_reattach {
        Some(Text::from("attach again"))
    } else if matches!(item.upload, UploadState::Failed(_)) {
        Some(Text::from("upload failed"))
    } else {
        None
    };
    let size = format_attachment_size(item.size);
    let args = [
        ("name", Value::Text(item.name.clone())),
        ("size", Value::Text(size.clone())),
    ];
    let label = if item.image {
        tr("Image attachment, {name}, {size}", &args)
    } else if item.video && !item.needs_reattach {
        tr("Preview video attachment, {name}, {size}", &args)
    } else {
        tr("File attachment, {name}, {size}", &args)
    };
    let label = if !item.image || item.needs_reattach {
        suffix.as_ref().map_or(label.clone(), |suffix| {
            format!("{label}, {}", suffix.resolve())
        })
    } else {
        label
    };
    let tooltip = match &item.upload {
        _ if item.needs_reattach => tr(
            "{name} was not saved with this draft. Attach it again to send it.",
            &[("name", Value::Text(item.name.clone()))],
        ),
        UploadState::Failed(reason) => format!(
            "{}\n{size}\n\n{}",
            item.name,
            Text::client_key(reason.clone()).resolve()
        ),
        _ => format!("{}\n{size}", item.name),
    };
    let action = (!item.needs_reattach)
        .then_some(context.open_attachment)
        .flatten()
        .map(|open| {
            let id = item.id.clone();
            Rc::new(move || open(&id)) as Rc<dyn Fn()>
        });
    let kind = if item.needs_reattach {
        "neutral"
    } else if !item.image && matches!(item.upload, UploadState::Failed(_)) {
        "invalid"
    } else if item.image {
        "image"
    } else if item.video {
        "video"
    } else {
        document_kind(&item.name).unwrap_or("file")
    };
    let icon = if item.needs_reattach {
        glyph(Icon::CircleDashed)
    } else if item.image {
        item.preview
            .map_or_else(|| glyph(Icon::Image), image_chip_thumbnail)
    } else {
        glyph(if item.video { Icon::Film } else { Icon::File })
    };
    context_chip(
        kind,
        icon,
        file_chip_content(&item.name, &size, suffix).into_element(),
        Text::content(label),
        Some(Text::content(tooltip)),
        action,
    )
}

/// Port of composerInlineChip.ts middleTruncateAttachmentName (Unicode scalar values).
fn middle_truncate_attachment_name(name: &str, max: usize) -> String {
    let characters: Vec<_> = name.chars().collect();
    if characters.len() <= max {
        return name.into();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".into();
    }
    let available = max - 1;
    let suffix = (available - 1).min(if available >= 18 {
        14
    } else {
        available.div_ceil(2)
    });
    format!(
        "{}…{}",
        characters[..available - suffix].iter().collect::<String>(),
        characters[characters.len() - suffix..]
            .iter()
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_truncation_keeps_unicode_scalars_and_the_filename_tail() {
        for (name, limit, expected) in [
            ("abcdef", 0, ""),
            ("abcdef", 1, "…"),
            ("abcdef", 2, "a…"),
            ("abcdef", 4, "a…ef"),
            ("😀雪❄️report", 5, "😀雪…rt"),
            ("short", 36, "short"),
        ] {
            assert_eq!(middle_truncate_attachment_name(name, limit), expected);
        }
        let name = "a".repeat(40) + ".some-extension";
        let truncated = middle_truncate_attachment_name(&name, 36);
        assert_eq!(truncated.chars().count(), 36);
        assert!(truncated.ends_with("some-extension"));
    }
}
