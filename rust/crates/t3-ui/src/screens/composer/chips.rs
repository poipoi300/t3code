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
    thread_views::{composer::citations, user_message::Reference},
    types::ThreadKey,
    view_logic::session::MessageAttachment,
};
use t3_contracts::{
    assistant_citations::AssistantCitation,
    composer_context::{
        ComposerContextRecord, ElementContextRecord, ElementContextSource,
        PreviewAnnotationContextRecord, TerminalContextRecord,
    },
    server::{ServerProvider, ServerProviderSkill},
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

pub type MentionAction = Rc<dyn Fn(&str)>;
pub type AttachmentAction = Rc<dyn Fn(&str)>;
pub type CitationAction = Rc<dyn Fn(&AssistantCitation)>;

/// Client-owned records and catalogues; this renderer never tokenizes prompt or message text.
#[derive(Clone, Default, PartialEq)]
pub struct ChipData {
    pub records: BTreeMap<String, ComposerContextRecord>,
    pub skills: Vec<ServerProviderSkill>,
    pub providers: Vec<ServerProvider>,
    pub previews: BTreeMap<String, String>,
    pub video_attachments: BTreeSet<String>,
    pub message_attachments: BTreeMap<String, MessageAttachment>,
}

#[derive(Clone)]
pub struct ChipContext {
    pub data: Memo<ChipData>,
    pub open_mention: Option<MentionAction>,
    pub open_attachment: Option<AttachmentAction>,
    pub open_citation: Option<CitationAction>,
}

impl ChipContext {
    pub fn new(data: Memo<ChipData>) -> Self {
        Self {
            data,
            open_mention: None,
            open_attachment: None,
            open_citation: None,
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
    // NodeViews read their context when mounted, under this component's owner.
    crate::ui::popover::with_popup_context(context, move || {
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
        views.insert(name, Arc::new(ComposerChipNodeView));
    }
    session.set_node_views(views);
}

struct ComposerChipNodeView;

impl NodeView for ComposerChipNodeView {
    fn create(&self, cx: &NodeViewCx, _: &Node) -> AnyElement {
        let node = cx.node();
        let selected = cx.selected();
        let context = use_context::<ChipContext>()
            .unwrap_or_else(|| ChipContext::new(computed(ChipData::default)));
        view()
            .app_class(tw(
                "relative inline-flex min-w-0 max-w-full select-none items-baseline",
            ))
            .child(dynamic(move || {
                let node = node.get();
                let context = context.clone();
                untracked(|| match node.ty().name() {
                    "composer-mention" => composer_mention_node_view(&node, context),
                    "composer-skill" => composer_skill_node_view(&node, context),
                    "composer-model" => composer_model_node_view(&node, context),
                    "composer-citation" => composer_citation_node_view(&node, context),
                    "composer-context-reference" => {
                        composer_context_reference_node_view(&node, context)
                    }
                    _ => text(SharedString::from(attr(&node, "source").to_owned())).into_element(),
                })
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
            .into_element()
    }

    fn update(&self, _: &Node, _: &[nsh_base::rich_editor::state::Decoration]) -> bool {
        true
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
            "inline-flex max-w-full items-center gap-[0.2em] font-medium leading-[normal] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-current rounded-sm disabled:cursor-default",
        ),
        Some(color),
    ]))
}

fn interactive_chip_class(kind: &str) -> SharedString {
    let base = context_chip_class(kind);
    // ContextChipLabel's :is(button:enabled,a):hover selector becomes a named native group.
    tw(&theme::cn([
        Some(&base),
        Some("group/chip cursor-pointer"),
    ]))
}

/// Port of ContextChip.tsx ContextChipLabel; NSH text owns single-line truncation.
pub fn context_chip_label(label: Text) -> impl IntoElement {
    text(move || label.resolve())
        .app_class(tw("block min-w-0 self-baseline overflow-hidden text-ellipsis underline decoration-transparent decoration-1 underline-offset-[0.2em] transition-colors motion-reduce:transition-none group-hover/chip:decoration-current"))
        .number_of_lines(1_u32)
}

fn glyph(which: Icon) -> AnyElement {
    icon_with_class(which, "block size-[1em] shrink-0 self-center").into_element()
}

/// Port of contextChipParts.tsx ContextChipPopover; content is mounted only while open.
fn context_chip_popover(
    kind: &str,
    icon: AnyElement,
    label: Text,
    accessible_label: Text,
    content: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    let handle = popover::popover_create_handle();
    let trigger = nsh_base::button()
        .app_class(interactive_chip_class(kind))
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
    let control = if let Some(action) = action {
        nsh_base::button()
            .app_class(interactive_chip_class(kind))
            .accessibility_label(move || Some(accessible_label.resolve()))
            .on_press(move || action())
            .child(icon)
            .child(label)
            .into_element()
    } else {
        view()
            .app_class(context_chip_class(kind))
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
        Some(Text::content(path)),
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
    let name = attr(node, "skillName").to_owned();
    let stored_label = attr(node, "skillLabel").to_owned();
    let stored_description = attr(node, "skillDescription").to_owned();
    dynamic(move || {
        let skill = context.data.with(|data| {
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
        let instructions = skill.as_ref().and_then(|skill| {
            let open = context.open_mention.clone()?;
            let path = skill.path.0.clone();
            Some(Rc::new(move || open(&path)) as Rc<dyn Fn()>)
        });
        untracked(|| skill_chip_popover(label, description, instructions))
    })
    .into_element()
}

fn optional_string(
    value: &Option<Option<t3_contracts::base_schemas::TrimmedNonEmptyString>>,
) -> Option<&str> {
    value
        .as_ref()?
        .as_ref()
        .map(|value| value.0.trim())
        .filter(|value| !value.is_empty())
}

/// Port of chat/SkillInlineText.tsx SkillChipPopover.
pub fn skill_chip_popover(
    label: String,
    description: String,
    instructions: Option<Rc<dyn Fn()>>,
) -> AnyElement {
    let accessible = Text::content(tr(
        "Skill {label}",
        &[("label", Value::Text(label.clone()))],
    ));
    context_chip_popover(
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
    let instance = attr(node, "instanceId").to_owned();
    let model = attr(node, "model").to_owned();
    let source = attr(node, "source").to_owned();
    dynamic(move || {
        let presentation = context.data.with(|data| {
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
        });
        untracked(|| {
            let Some((label, provider, driver)) = presentation else {
                return text(SharedString::from(source.clone())).into_element();
            };
            let accessible = Text::content(tr(
                "Model {label}",
                &[("label", Value::Text(label.clone()))],
            ));
            let source = source.clone();
            context_chip_popover(
                "neutral",
                provider_icon(&driver, &provider, "z-auto size-[1em] shrink-0 self-center")
                    .into_element(),
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
    dynamic(move || {
        let record = context.data.with(|data| data.records.get(&id).cloned());
        let context = context.clone();
        untracked(|| render_context_reference(&kind, &id, &label, record.as_ref(), context, true))
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
    render_context_reference(
        &reference.kind,
        &reference.context_id,
        &reference.label,
        record,
        context,
        false,
    )
}

/// Fits UserRowContext::context_chip directly; the row supplies its canonical records.
pub fn user_message_chip_renderer(
    context: ChipContext,
) -> crate::screens::thread::rows::user::ContextChipRenderer {
    Rc::new(move |reference, record, row| {
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
        let mut context = context.clone();
        let original = context.data;
        context.data = computed(move || {
            let mut data = original.get();
            data.previews.extend(previews.clone());
            data.video_attachments.extend(videos.clone());
            data.message_attachments.extend(attachments.clone());
            data
        });
        user_message_context_reference_chip(reference, record, context)
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
            let record = record.clone();
            let label = record.label.clone();
            let accessible = Text::content(tr(
                "Review comment, {label}",
                &[("label", Value::Text(label.clone()))],
            ));
            context_chip_popover(
                "review-comment",
                glyph(Icon::MessageCircle),
                Text::content(label),
                accessible,
                move || {
                    view().app_class(tw("space-y-2 overflow-hidden rounded-lg border border-border/70 bg-background/70 p-3"))
                    .child(view().app_class(tw("space-y-1"))
                        .child(text(SharedString::from(record.file_path.0.clone())).app_class(tw("truncate text-xs font-medium text-foreground")))
                        .child(text(SharedString::from(format!("{} · {}", record.section_title, record.range_label))).app_class(tw("text-secondary-label text-2xs"))))
                    .child((!record.text.trim().is_empty()).then(|| { let body = record.text.clone(); crate::screens::thread::markdown::base::chat_markdown(move || body.clone(), || false) }))
                    .child((!record.diff.trim().is_empty()).then(|| source_preview(record.diff.clone(), "diff")))
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
        ComposerContextRecord::Response(record) => {
            assistant_citation_chip(record.citation.clone(), context)
        }
        ComposerContextRecord::Unknown(_) => unresolved_chip(label.to_owned(), draft),
    }
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
                    .child((!element.html_preview.trim().is_empty()).then(|| scroll().app_class(tw("mt-1 max-h-24 rounded bg-muted/60 px-2 py-1.5 text-3xs leading-relaxed")).child(text(SharedString::from(element.html_preview.trim().to_owned())).app_class(tw("font-mono")))))
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
                .child((!record.html_preview.trim().is_empty()).then(|| {
                    view()
                        .app_class(tw(
                            "flex h-40 flex-col overflow-hidden rounded border border-border",
                        ))
                        .child(source_preview(record.html_preview.clone(), "html"))
                }))
                .child((!record.styles.trim().is_empty()).then(|| {
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
    if !record.comment.trim().is_empty() {
        lines.extend([String::new(), record.comment.trim().into()]);
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
            // overflow-auto is a real scroll view; terminal output retains native newlines/tabs.
            .child(scroll().app_class(tw("max-h-80"))
                .child(text(SharedString::from(record.text.clone())).app_class(tw("bg-muted p-3 font-mono text-foreground text-xs leading-relaxed outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                    .focusable(true).label(t("Captured terminal output"))))
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
        Some(t3_contracts::chat_attachment::ChatAttachment::File(file)) => {
            crate::screens::thread::rows::user::user_message_file_label(file)
        }
        Some(t3_contracts::chat_attachment::ChatAttachment::Image(file)) => file.name.clone(),
        _ => name.to_owned(),
    };
    let size = format_attachment_size(bytes);
    let arguments = [
        ("name", Value::Text(name.clone())),
        ("size", Value::Text(size.clone())),
    ];
    let accessible = Text::content(if kind == "image" {
        tr("Image attachment, {name}, {size}", &arguments)
    } else {
        tr("File attachment, {name}, {size}", &arguments)
    });
    let preview = context.data.with(|data| data.previews.get(id).cloned());
    let action = context.open_attachment.map(|open| {
        let id = id.to_owned();
        Rc::new(move || open(&id)) as Rc<dyn Fn()>
    });
    let icon = if kind == "image" {
        preview.map_or_else(
            || glyph(Icon::Image),
            |url| {
                image(url)
                    .app_class(tw(
                        "size-[1em] shrink-0 self-center rounded-sm object-cover",
                    ))
                    .accessible(false)
                    .into_element()
            },
        )
    } else if video {
        glyph(Icon::Film)
    } else {
        glyph(Icon::File)
    };
    context_chip(
        document_kind(&name).unwrap_or(kind),
        icon,
        file_chip_content(&name, &size).into_element(),
        accessible,
        Some(Text::content(format!("{name}\n{size}"))),
        action,
    )
}

/// Port of contextChipParts.tsx FileChipContent and ImageChipButton's label.
fn file_chip_content(name: &str, size: &str) -> impl IntoElement + use<> {
    view().app_class(tw("flex min-w-0 self-baseline items-baseline gap-[0.33em] overflow-hidden text-ellipsis underline decoration-transparent decoration-1 underline-offset-[0.2em] transition-colors motion-reduce:transition-none group-hover/chip:decoration-current"))
        .child(text(SharedString::from(middle_truncate_attachment_name(name, 36))).app_class(tw("min-w-0 max-w-72 overflow-hidden text-ellipsis")).number_of_lines(1_u32))
        .child(text(SharedString::from(size.to_owned())).app_class(tw("shrink-0 text-3xs text-current")))
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
