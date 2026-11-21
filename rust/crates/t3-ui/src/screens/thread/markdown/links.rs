//! Port of apps/web/src/components/chat/{MarkdownFileLink,MarkdownExternalLink,MarkdownThreadLink}.tsx.

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use futures::{StreamExt, stream::LocalBoxStream};

use nsh_base::text_view::{
    Block, CustomNode, Document, Inline, MarkdownPlugin, ParseContext, Parser, TextViewClasses,
    mdast,
};
use nsh_core::element::Renderer;
use nsh_core::prelude::*;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::links::{
    MarkdownFileLinkMeta, is_windows_absolute, resolve_inline_code_file_link_meta,
    resolve_markdown_file_link_meta,
};
use t3_client::view_logic::session::markdown_link_presentation::{
    ThreadLink, external_link_host, favicon_url_for_host, file_link_label,
    is_markdown_file_link_label, leading_external_link_text_length, markdown_file_copy_menu_items,
    parse_thread_link_href, thread_link_candidates, unknown_thread_link_label,
};
use t3_contracts::orchestration_v2::OrchestrationV2ThreadLinkMetadata;

const MARKDOWN_FONT_SIZE: f32 = 14.0;

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::{Shared, rt};
use crate::state::nav::{Route, navigator};
use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::menu::{ItemOptions, MenuOptions, context_menu, menu, menu_item, menu_popup_with};
use crate::ui::tooltip::{TooltipOptions, Variant, tooltip, tooltip_create_handle};

use crate::runtime::tasks::watch;

/// Created above the list; each table's feedback and timer are keyed by the flattener's BlockId.
#[derive(Clone, Copy)]
pub struct MarkdownTablesState {
    tables: Stored<
        BTreeMap<crate::screens::thread::blocks::flatten::BlockId, super::rich::MarkdownTableState>,
    >,
    owner: Owner,
}

impl Default for MarkdownTablesState {
    fn default() -> Self {
        Self {
            tables: stored(BTreeMap::new()),
            owner: Owner::current(),
        }
    }
}

/// Port of MarkdownTable.tsx, with its feedback retained above recycled block cells.
pub fn markdown_table_leaf(
    source: Memo<Rc<str>>,
    id: crate::screens::thread::blocks::flatten::BlockId,
    state: MarkdownTablesState,
) -> impl IntoElement {
    let table = state
        .owner
        .run(|| state.tables.with(|tables| *tables.entry(id).or_default()));
    super::rich::markdown_table_block_with_state(source, table)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadLinkLookup {
    pub settled: bool,
    pub metadata: BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>,
}

/// Bound by runtime/services/thread_ops.rs to the destination client metadata query.
pub trait MarkdownLinkService {
    fn thread_link_metadata(
        &self,
        environment: EnvironmentId,
        threads: Vec<ThreadId>,
    ) -> LocalBoxStream<'static, ThreadLinkLookup>;
}

#[derive(Clone)]
pub struct MarkdownLinkRuntime(pub Rc<dyn MarkdownLinkService>);

fn markdown_link_runtime() -> Option<MarkdownLinkRuntime> {
    use_context::<MarkdownLinkRuntime>().or_else(|| {
        use_context::<crate::runtime::Rt>()
            .and_then(|runtime| runtime.backend().client())
            .map(|client| crate::runtime::services::thread_ops::markdown_links(&client))
    })
}

pub fn markdown_link_context() -> Memo<MarkdownLinkContext> {
    if let Some(context) = use_context::<MarkdownLinkContext>() {
        return computed(move || context.clone());
    }
    let navigation = use_context::<crate::state::nav::Navigator>();
    let runtime = use_context::<crate::runtime::Rt>();
    let thread = computed(move || {
        navigation
            .as_ref()
            .and_then(|navigation| match navigation.top() {
                Route::Thread(key) | Route::ThreadPanel(key, _) => Some(key),
                _ => None,
            })
    });
    let shell = computed(move || {
        thread
            .get()
            .and_then(|key| runtime.map(|runtime| runtime.store.shell(key.environment)))
    });
    computed(move || {
        let Some(thread) = thread.get() else {
            return MarkdownLinkContext::default();
        };
        let cwd = shell.get().and_then(|shell| shell.get()).and_then(|shell| {
            let record = shell
                .threads
                .iter()
                .find(|record| record.id == thread.thread)?;
            record
                .worktree_path
                .as_ref()
                .map(|path| Rc::from(path.0.as_str()))
                .or_else(|| {
                    shell
                        .projects
                        .iter()
                        .find(|project| project.id == record.project_id)
                        .map(|project| Rc::from(project.workspace_root.0.as_str()))
                })
        });
        MarkdownLinkContext {
            environment: Some(thread.environment.clone()),
            thread: Some(thread),
            cwd,
            ..Default::default()
        }
    })
}

/// Message/file context, captured once for a bounded prose block; authored values are content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkdownLinkContext {
    pub environment: Option<EnvironmentId>,
    pub thread: Option<ThreadKey>,
    pub message_id: Option<String>,
    pub cwd: Option<Rc<str>>,
    pub base_dir: Option<Rc<str>>,
}

impl MarkdownLinkContext {
    fn file(&self, href: &str, inline_code: bool) -> Option<MarkdownFileLinkMeta> {
        let base = self.base_dir.as_deref().or(self.cwd.as_deref());
        if inline_code {
            resolve_inline_code_file_link_meta(href, self.cwd.as_deref(), base)
        } else {
            resolve_markdown_file_link_meta(href, self.cwd.as_deref(), base)
        }
    }
}

/// Install on the block's Parser and forward render_custom here. Never parse a whole reply
/// per delta: TL-BLOCKS-1 owns block source and document-level reference definitions.
#[derive(Clone, Debug, Default)]
pub struct MarkdownLinkPlugin {
    pub context: MarkdownLinkContext,
}

impl MarkdownPlugin for MarkdownLinkPlugin {
    fn name(&self) -> &str {
        "chat-markdown-links"
    }

    fn inline(&self, node: &mdast::Node, context: &mut ParseContext<'_>) -> Option<Inline> {
        let (href, content, inline_code) = match node {
            mdast::Node::Link(link) => (link.url.as_str(), context.inlines(&link.children), false),
            mdast::Node::InlineCode(code) => {
                (code.value.as_str(), vec![Inline::text(&code.value)], true)
            }
            _ => return None,
        };
        self.link_inline(href, content, inline_code)
            .map(|mut inline| {
                if let mdast::Node::Link(link) = node {
                    set_link_title(&mut inline, link.title.clone());
                }
                inline
            })
    }

    fn render(&self, node: &CustomNode) -> Option<AnyElement> {
        self.render_link(node, None)
    }
}

impl MarkdownLinkPlugin {
    fn link_inline(&self, href: &str, content: Vec<Inline>, inline_code: bool) -> Option<Inline> {
        if !inline_code && parse_thread_link_href(href).is_some() {
            let label = content.iter().map(Inline::plain_text).collect::<String>();
            let mut custom = CustomNode::new("markdown-thread-link").with_attribute("href", href);
            if parse_thread_link_href(&label).is_none() {
                custom = custom.with_text(label);
            }
            return Some(Inline::Custom(custom));
        }
        if let Some(meta) = self.context.file(href, inline_code) {
            let label = content.iter().map(Inline::plain_text).collect::<String>();
            let descriptive = !inline_code && !is_markdown_file_link_label(&label, href);
            return Some(Inline::Custom(
                CustomNode::new("markdown-file-link")
                    .with_attribute("href", href)
                    .with_attribute("inline-code", inline_code.to_string())
                    .with_text(file_link_label(&meta, None))
                    .with_content(if descriptive { content } else { Vec::new() }),
            ));
        }
        if !inline_code && let Some(host) = external_link_host(href) {
            if !content.iter().any(has_link_text) {
                return None;
            }
            if content.first().is_some_and(|inline| matches!(inline, Inline::Custom(node) if node.name == "markdown-external-leading")) { return None; }
            let plain = content
                .iter()
                .all(|item| matches!(item, Inline::Text { .. }))
                .then(|| content.iter().map(Inline::plain_text).collect::<String>());
            let url = if href.starts_with("//") {
                format!("https:{href}")
            } else {
                href.into()
            };
            let label = content.iter().map(Inline::plain_text).collect::<String>();
            return Some(Inline::Custom(
                CustomNode::new("markdown-external-link")
                    .with_attribute("href", url)
                    .with_text(label)
                    .with_content(markdown_external_link_content(
                        &host,
                        plain.as_deref(),
                        content,
                    )),
            ));
        }
        None
    }

    pub(super) fn render_link(
        &self,
        node: &CustomNode,
        images: Option<super::base::MarkdownImageRenderer>,
    ) -> Option<AnyElement> {
        match node.name.as_str() {
            "markdown-external-link" => {
                let content = Document::new(vec![Block::paragraph(node.content.clone())]);
                let plugin = self.clone();
                let href = node.attributes.get("href")?.clone();
                // TextView renders custom nodes while tracking; a theme or label read must not
                // rebuild the link and dispose its props.
                // TextView names a custom inline U+FFFC; the native link supplies the authored name.
                Some(untracked(move || {
                    crate::runtime::services::links::external_link(
                        move || Some(href.clone()),
                        |_| {},
                    )
                    .title(
                        node.attributes
                            .get("title")
                            .cloned()
                            .map(SharedString::from),
                    )
                    .accessibility_label(Some(SharedString::from(node.text.clone())))
                    .app_class(tw("min-w-0 max-w-full text-info-foreground no-underline"))
                    .child(
                        super::base::nested_text_view(images.clone())
                            .document(Some(content))
                            .classes(TextViewClasses {
                                root: Some(tw("text-sm leading-relaxed text-info-foreground")),
                                paragraph: Some(tw("m-0")),
                                ..Default::default()
                            })
                            .render_custom(move |node| {
                                plugin
                                    .render_link(node, images.clone())
                                    .unwrap_or_else(|| view().into_element())
                            }),
                    )
                    .into_element()
                }))
            }
            "markdown-file-link" => {
                let href = node.attributes.get("href")?;
                let meta = self.context.file(
                    href,
                    node.attributes
                        .get("inline-code")
                        .is_some_and(|v| v == "true"),
                )?;
                let chip = markdown_file_link_in_context(meta, Some(self.context.clone()));
                if node.content.is_empty() {
                    return Some(chip.into_element());
                }
                Some(
                    text(SharedString::default())
                        .child(
                            super::base::nested_text_view(images)
                                .document(Some(Document::new(vec![Block::paragraph(
                                    node.content.clone(),
                                )])))
                                .classes(TextViewClasses {
                                    root: Some(tw("text-sm leading-relaxed text-foreground/90")),
                                    paragraph: Some(tw("m-0")),
                                    ..Default::default()
                                }),
                        )
                        .child(text(SharedString::from(" ")))
                        .child(chip)
                        .into_element(),
                )
            }
            "markdown-thread-link" => {
                let Some(environment) = self.context.environment.clone() else {
                    return Some(
                        text(if node.text.is_empty() {
                            SharedString::from(t("Thread"))
                        } else {
                            node.text.clone().into()
                        })
                        .into_element(),
                    );
                };
                Some(
                    markdown_thread_link(
                        environment,
                        parse_thread_link_href(node.attributes.get("href")?)?,
                        (!node.text.is_empty()).then(|| Text::content(node.text.clone())),
                    )
                    .into_element(),
                )
            }
            "markdown-external-leading" => {
                if images.is_some() {
                    return Some(text(SharedString::from(node.text.clone())).into_element());
                }
                let host = node.attributes.get("host")?.clone();
                // Native text retains a single no-break inline box for the favicon and prefix.
                Some(
                    text(SharedString::default())
                        .child(markdown_link_favicon(host))
                        .child(text(SharedString::from(node.text.clone())))
                        .into_element(),
                )
            }
            _ => None,
        }
    }

    /// Decorate a block's resolved document, including reference links; no source scan or parse.
    pub fn decorate_document(&self, document: &mut Document) {
        self.blocks(&mut document.blocks);
        for footnote in &mut document.footnotes {
            self.blocks(&mut footnote.children);
        }
    }

    fn blocks(&self, blocks: &mut [Block]) {
        for block in blocks {
            match block {
                Block::Paragraph { content } | Block::Heading { content, .. } => {
                    self.inlines(content)
                }
                Block::Blockquote { children, .. } => self.blocks(children),
                Block::List { items, .. } => {
                    for item in items {
                        self.blocks(&mut item.children);
                    }
                }
                Block::Table(table) => {
                    for cell in table.head.iter_mut().chain(table.rows.iter_mut().flatten()) {
                        self.inlines(&mut cell.content);
                    }
                }
                Block::Details {
                    summary, children, ..
                } => {
                    self.inlines(summary);
                    self.blocks(children);
                }
                Block::Custom(node) => {
                    self.inlines(&mut node.content);
                    self.blocks(&mut node.children);
                }
                _ => {}
            }
        }
    }

    fn inlines(&self, content: &mut [Inline]) {
        for inline in content {
            let replacement = match inline {
                Inline::Link {
                    url,
                    title,
                    content,
                } => {
                    let title = title.clone();
                    let replacement = self.link_inline(url, content.clone(), false);
                    replacement.map(|mut inline| {
                        set_link_title(&mut inline, title);
                        inline
                    })
                }
                Inline::Code { code } => {
                    self.link_inline(code, vec![Inline::text(code.clone())], true)
                }
                Inline::Strong { content }
                | Inline::Emphasis { content }
                | Inline::Strikethrough { content }
                | Inline::Underline { content }
                | Inline::Highlight { content, .. }
                | Inline::Superscript { content }
                | Inline::Subscript { content }
                | Inline::Small { content } => {
                    self.inlines(content);
                    None
                }
                _ => None,
            };
            if let Some(replacement) = replacement {
                *inline = replacement;
            }
        }
    }
}

fn set_link_title(inline: &mut Inline, title: Option<String>) {
    match inline {
        Inline::Link { title: current, .. } => *current = title,
        Inline::Custom(node) if node.name == "markdown-external-link" => {
            if let Some(title) = title {
                node.attributes.insert("title".into(), title);
            }
        }
        _ => {}
    }
}

/// A bounded prose block's complete renderer. The block owner retains its Rc and identity;
/// resolved cross-block definitions can instead use decorate_document on the flattener's AST.
pub fn markdown_link_block(
    source: Memo<Rc<str>>,
    context: MarkdownLinkContext,
) -> impl IntoElement {
    let plugin = MarkdownLinkPlugin { context };
    let decorate = plugin.clone();
    let document = computed(move || {
        let source = source.get();
        let mut document = Parser::new().markdown(&source);
        decorate.decorate_document(&mut document);
        document
    });
    nsh_base::text_view()
        .document(move || Some(document.get()))
        .render_custom(move |node| plugin.render(node).unwrap_or_else(|| view().into_element()))
        .classes(markdown_link_classes())
        .selectable(true)
        .app_class(tw("w-full text-sm text-foreground/90"))
}

/// `.chat-markdown a:not(.chat-markdown-file-link, :has([data-slot=context-chip-label]))`.
pub fn markdown_link_classes() -> TextViewClasses {
    TextViewClasses {
        link: Some(tw("text-info-foreground no-underline")),
        ..Default::default()
    }
}

/// MarkdownExternalLinkContent: one no-break prefix, followed by native wrapping text.
pub fn markdown_external_link_content(
    host: &str,
    plain_text: Option<&str>,
    children: Vec<Inline>,
) -> Vec<Inline> {
    let (first, rest) = match plain_text.filter(|s| !s.is_empty()) {
        Some(plain) => (Some(plain.to_owned()), Vec::new()),
        None => {
            let mut children = children.into_iter();
            match children.next() {
                Some(Inline::Text { text }) if !text.is_empty() => (Some(text), children.collect()),
                first => {
                    let mut out = vec![Inline::Custom(
                        CustomNode::new("markdown-external-leading").with_attribute("host", host),
                    )];
                    out.extend(first);
                    out.extend(children);
                    return out;
                }
            }
        }
    };
    let first = first.unwrap_or_default();
    let leading = leading_external_link_text_length(&first);
    let mut out = vec![Inline::Custom(
        CustomNode::new("markdown-external-leading")
            .with_attribute("host", host)
            .with_text(&first[..leading]),
    )];
    // NSH wraps text natively; overflow-wrap:anywhere and whitespace hints have no native prop.
    out.extend(breakable_external_link_text(&first[leading..]));
    out.extend(rest);
    out
}

fn has_link_text(inline: &Inline) -> bool {
    match inline {
        Inline::Image(_) => false,
        Inline::Strong { content }
        | Inline::Emphasis { content }
        | Inline::Strikethrough { content }
        | Inline::Underline { content }
        | Inline::Highlight { content, .. }
        | Inline::Superscript { content }
        | Inline::Subscript { content }
        | Inline::Small { content } => content.iter().any(has_link_text),
        _ => !inline.plain_text().trim().is_empty(),
    }
}

fn breakable_external_link_text(value: &str) -> Vec<Inline> {
    // No character-per-element splitting: a 16,000-character URL is still one text node.
    if value.is_empty() {
        Vec::new()
    } else {
        vec![Inline::text(value)]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct FailedFaviconHosts(BTreeSet<String>);

fn markdown_link_favicon(host: String) -> impl IntoElement {
    let failed = global::<FailedFaviconHosts>();
    let url = favicon_url_for_host(&host);
    view()
        .app_class(tw("inline-block"))
        .ms(px(MARKDOWN_FONT_SIZE * 0.25))
        .me(px(MARKDOWN_FONT_SIZE * 0.2))
        .accessible(false)
        .child(dynamic(move || {
            if let Some(mark) = source_control_host_icon(&host) {
                return svg(mark)
                    .app_class(tw("inline-block size-[14px]"))
                    .accessible(false)
                    .into_element();
            }
            let host = host.clone();
            let url = url.clone();
            let is_failed = failed.with(|failed| failed.0.contains(&host));
            untracked(move || {
                // NSH has no CSS vertical-align:-0.125em; native inline boxes use its baseline.
                match url.filter(|_| !is_failed) {
                    Some(url) => image(url)
                        .test_id("markdown-link-favicon")
                        .accessible(false)
                        .app_class(tw("inline-block size-[14px] rounded-sm"))
                        .on_error(move |_: &str| {
                            failed.update(|failed| {
                                failed.0.insert(host.clone());
                            })
                        })
                        .into_element(),
                    None => icon_with_class(Icon::Globe, "inline-block size-[14px]").into_element(),
                }
            })
        }))
}

/// MarkdownFileLink without a thread context uses the web's copy-only fallback.
pub fn markdown_file_link(meta: MarkdownFileLinkMeta) -> impl IntoElement {
    markdown_file_link_in_context(meta, None)
}

fn markdown_file_link_in_context(
    meta: MarkdownFileLinkMeta,
    context: Option<MarkdownLinkContext>,
) -> impl IntoElement {
    // Popup state belongs to the mounted chip, outside TextView's rendering computation.
    file_link_chip().content(move |_: &bool| render_file_link_chip(meta.clone(), context.clone()))
}

#[derive(Clone, PartialEq, Props)]
struct FileLinkChipProps {
    content: Prop<Option<Renderer<bool>>>,
}

#[component]
fn FileLinkChip(props: FileLinkChipProps) -> impl IntoElement {
    dynamic(move || {
        let content = props.content.get();
        untracked(|| content.map(|content| content.render("file-chip", false)))
    })
}

fn render_file_link_chip(
    meta: MarkdownFileLinkMeta,
    context: Option<MarkdownLinkContext>,
) -> impl IntoElement {
    let panel = use_context::<crate::screens::panel::PanelHost>();
    let thread = context.as_ref().and_then(|context| context.thread.clone());
    let panel_target = thread
        .zip(panel)
        .filter(|_| meta.workspace_relative_path.is_some());
    let meta = Rc::new(meta);
    let label = file_link_label(&meta, None);
    let accessible = tr(
        "File options for {label}",
        &[("label", Value::Text(label.clone()))],
    );
    let hovered = signal(false);
    let tooltip_path = meta.target_path.clone();
    let popup_meta = meta.clone();
    let make_popup = move || {
        let meta = popup_meta.clone();
        menu_popup_with(move || file_copy_menu(&meta))
    };
    let trigger = if let Some((thread, panel)) = panel_target {
        let target = meta.clone();
        // A workspace file opens in the app's own panel (`on_open`), never outside it.
        crate::runtime::services::links::in_app_link({
            let path = SharedString::from(meta.target_path.clone());
            move || Some(path.clone())
        })
            .direction(LayoutDirection::Ltr)
            .accessibility_label(Some(SharedString::from(label.clone())))
            .gap(px(MARKDOWN_FONT_SIZE * 0.2)).line_height(nsh_core::LineHeight::Normal)
        .app_class(tw("inline-block max-w-full items-center font-medium text-(--context-chip-mention) cursor-pointer focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-current rounded-sm disabled:cursor-default"))
            .child(file_tag_chip_content(&meta.file_path, Text::content(label), hovered))
            .on_open(move |_: &SharedString| open_file_in_panel(panel, &thread, &target))
            .into_element()
    } else {
        // NSH programmatic ContextMenu opens redirect taps to the initial focus; Menu keeps pointer activation.
        menu(nsh_base::button()
            .direction(LayoutDirection::Ltr)
        .accessibility_label(accessible)
        // ContextChip's ink variable is resolved directly; selector rules go on their children.
        // `.chat-markdown-file-link` overrides inline-flex to inline-block in index.css.
        .gap(px(MARKDOWN_FONT_SIZE * 0.2)).line_height(nsh_core::LineHeight::Normal)
        .app_class(tw("inline-block max-w-full items-center font-medium text-(--context-chip-mention) cursor-pointer focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-current rounded-sm disabled:cursor-default select-text"))
        .child(file_tag_chip_content(&meta.file_path, Text::content(label), hovered)),
            MenuOptions { align: Align::Start, ..Default::default() }, make_popup()).into_element()
    };
    view()
        .app_class(tw("inline-block max-w-full"))
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .tooltip(tooltip(
            tooltip_create_handle(),
            TooltipOptions {
                variant: Variant::Code,
                ..Default::default()
            },
            text(SharedString::from(tooltip_path)),
        ))
        .child(context_menu(
            trigger,
            MenuOptions::default(),
            || false,
            make_popup(),
        ))
}

fn open_file_in_panel(
    panel: crate::screens::panel::PanelHost,
    thread: &ThreadKey,
    meta: &MarkdownFileLinkMeta,
) {
    let path = meta
        .workspace_relative_path
        .clone()
        .unwrap_or_else(|| meta.file_path.clone());
    panel.apply(
        thread,
        crate::screens::panel::Action::OpenFile {
            path,
            line: meta.line.map(|line| line as f64),
        },
    );
}

fn file_copy_menu(meta: &MarkdownFileLinkMeta) -> impl IntoElement + use<> {
    markdown_file_copy_menu_items(meta)
        .into_iter()
        .fold(view(), |menu, item| {
            let label = if item.relative {
                t("Copy relative path")
            } else {
                t("Copy full path")
            };
            let value = if is_windows_absolute(&meta.target_path) {
                item.value.replace('/', "\\")
            } else {
                item.value
            };
            menu.child(menu_item(
                label.clone(),
                ItemOptions::default(),
                text(SharedString::from(label)),
                move || {
                    // NSH exposes no write acknowledgement; do not claim a successful copy in a toast.
                    set_clipboard(value.clone());
                },
            ))
        })
}

/// Port of FileTagChip.tsx FileTagChipContent.
fn file_tag_chip_content(
    _path: &str,
    label: Text,
    hovered: Signal<bool>,
) -> impl IntoElement + use<> {
    // Pierre file artwork and metadata are not exposed yet; this is the exact File fallback
    // glyph already in ui/icon.rs, rather than an invented icon or another platform branch.
    text(SharedString::default())
        .writing_direction(WritingDirection::Ltr)
        .child(
            svg(Icon::File.asset())
                .accessible(false)
                .app_class(tw("inline-block shrink-0 self-center"))
                .w(px(MARKDOWN_FONT_SIZE))
                .h(px(MARKDOWN_FONT_SIZE))
                .me(px(MARKDOWN_FONT_SIZE * 0.2)),
        )
        .child(context_chip_label(move || label.resolve(), hovered, true))
}

fn markdown_chip_icon(which: Icon) -> impl IntoElement {
    svg(which.asset())
        .accessible(false)
        .app_class(tw("shrink-0 self-center"))
        .w(px(MARKDOWN_FONT_SIZE))
        .h(px(MARKDOWN_FONT_SIZE))
}

/// Port of ContextChip.tsx ContextChipLabel; native hover state replaces its parent selector.
fn context_chip_label(
    label: impl Fn() -> SharedString + 'static,
    hovered: Signal<bool>,
    inline: bool,
) -> impl IntoElement {
    text(label).number_of_lines(1u32).underline_offset(px(MARKDOWN_FONT_SIZE * 0.2)).app_class(move || Some(tw(&cn([
        Some("block min-w-0 self-baseline text-ellipsis underline decoration-transparent decoration-1 transition-colors motion-reduce:transition-none"),
        inline.then_some("inline"), hovered.get().then_some("decoration-current"),
    ]))))
}

#[derive(Clone, Debug, PartialEq)]
struct ThreadChip {
    key: ThreadKey,
    title: Text,
    project: Option<
        crate::runtime::Shared<t3_contracts::orchestration_project::OrchestrationProjectShell>,
    >,
    missing: bool,
    unknown: Text,
}

type ShellRead = (
    EnvironmentId,
    Option<Shared<t3_client::state::sharing::SharedShellSnapshot>>,
);

fn thread_chip(
    shells: &[ShellRead],
    environment: &EnvironmentId,
    link: &ThreadLink,
    label: Option<&Text>,
    lookup: &ThreadLinkLookup,
) -> ThreadChip {
    let written_environment = link.environment.as_ref().unwrap_or(environment);
    let reachable = shells
        .iter()
        .any(|(environment, _)| environment == written_environment);
    let find = |key: &ThreadKey| {
        shells
            .iter()
            .find(|(environment, _)| *environment == key.environment)
            .and_then(|(_, shell)| shell.as_ref())
            .and_then(|shell| shell.threads.iter().find(|thread| thread.id == key.thread))
            .map(|thread| OrchestrationV2ThreadLinkMetadata {
                title: thread.title.clone(),
                project_id: Some(thread.project_id.clone()),
            })
    };
    let refs: Vec<_> = if reachable && !lookup.settled {
        Vec::new()
    } else {
        shells
            .iter()
            .filter_map(|(environment, shell)| shell.as_ref().map(|shell| (environment, shell)))
            .flat_map(|(environment, shell)| {
                shell.threads.iter().map(|thread| ThreadKey {
                    environment: environment.clone(),
                    thread: thread.id.clone(),
                })
            })
            .collect()
    };
    let resolution =
        t3_client::view_logic::session::markdown_link_presentation::resolve_thread_link(
            link,
            environment,
            reachable,
            lookup.settled,
            &refs,
            find,
            |key| {
                lookup
                    .metadata
                    .get(&key.thread)
                    .cloned()
                    .or_else(|| find(key))
            },
        );
    let unknown = Text::client_key(unknown_thread_link_label(link, environment));
    let title = resolution
        .metadata
        .as_ref()
        .map(|thread| thread.title.trim())
        .filter(|title| !title.is_empty())
        .map(|title| Text::content(title.to_owned()))
        .or_else(|| label.cloned())
        .unwrap_or_else(|| {
            if resolution.missing {
                unknown.clone()
            } else {
                Text::from("Thread")
            }
        });
    let project = resolution
        .metadata
        .and_then(|thread| {
            shells
                .iter()
                .find(|(environment, _)| *environment == resolution.key.environment)
                .and_then(|(_, shell)| shell.as_ref())
                .and_then(|shell| {
                    shell
                        .projects
                        .iter()
                        .find(|project| Some(&project.id) == thread.project_id.as_ref())
                })
                .cloned()
        })
        .map(Shared);
    ThreadChip {
        key: resolution.key,
        title,
        project,
        missing: resolution.missing,
        unknown,
    }
}

pub fn markdown_thread_link(
    environment: EnvironmentId,
    link: ThreadLink,
    label: Option<Text>,
) -> impl IntoElement {
    let runtime = rt();
    let nav = navigator();
    let environments = computed(move || {
        runtime
            .environments()
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>()
    });
    let selectors = computed(move || {
        environments
            .get()
            .into_iter()
            .map(|id| {
                let shell = runtime.store.shell(id.clone());
                (id, shell)
            })
            .collect::<Vec<_>>()
    });
    let lookup = signal(ThreadLinkLookup::default());
    if let Some(service) = markdown_link_runtime() {
        let candidates = thread_link_candidates(&link, &environment);
        let request = computed(move || {
            let written = &candidates[0];
            selectors.with(|selectors| {
                selectors
                    .iter()
                    .find(|(id, _)| *id == written.environment)
                    .filter(|(_, shell)| {
                        !shell.with(|shell| {
                            shell.as_ref().is_some_and(|shell| {
                                shell
                                    .threads
                                    .iter()
                                    .any(|thread| thread.id == written.thread)
                            })
                        })
                    })
                    .map(|_| {
                        (
                            written.environment.clone(),
                            candidates
                                .iter()
                                .map(|key| key.thread.clone())
                                .collect::<Vec<_>>(),
                        )
                    })
            })
        });
        effect(move || {
            let request = request.get();
            lookup.set(ThreadLinkLookup::default());
            if let Some((environment, threads)) = request {
                let mut updates = service.0.thread_link_metadata(environment, threads);
                let task = watch(async move {
                    while let Some(update) = updates.next().await {
                        lookup.set(update);
                    }
                });
                cleanup(move || task.cancel());
            }
        });
    }
    let chip = computed(move || {
        let shells = selectors.with(|selectors| {
            selectors
                .iter()
                .map(|(id, shell)| (id.clone(), shell.get()))
                .collect::<Vec<_>>()
        });
        thread_chip(&shells, &environment, &link, label.as_ref(), &lookup.get())
    });
    let missing = computed(move || chip.with(|chip| chip.missing));
    let project = computed(move || chip.with(|chip| chip.project.clone()));
    dynamic(move || {
        let missing = missing.get();
        untracked(move || {
            if missing {
                let hint = tooltip(
                    tooltip_create_handle(),
                    Default::default(),
                    text(move || chip.with(|chip| chip.unknown.resolve())),
                );
                return view().gap(px(MARKDOWN_FONT_SIZE * 0.2)).line_height(nsh_core::LineHeight::Normal)
        .app_class(tw("inline-flex max-w-full items-center font-medium focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-current rounded-sm"))
                    .role(AccessibilityRole::Group)
                    .focusable(true)
                    .label(move || SharedString::from(tr("Unavailable context, {label}", &[("label", Value::Text(chip.with(|chip| chip.title.resolve().to_string())))])))
                    .tooltip(hint)
                    .child(markdown_chip_icon(Icon::CircleDashed))
                    .child(text(move || chip.with(|chip| chip.title.resolve())))
                    .into_element();
            }
            let hovered = signal(false);
            view().app_class(tw("inline-block max-w-full"))
                .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                .tooltip(tooltip(tooltip_create_handle(), Default::default(), text(move || t("Open thread"))))
                .child(crate::runtime::services::links::in_app_link(move || Some(SharedString::from(Route::Thread(chip.with(|chip| chip.key.clone())).path())))
                    .accessibility_label(move || Some(tr("Thread, {title}", &[("title", Value::Text(chip.with(|chip| chip.title.resolve().to_string())))]).into()))
                    .gap(px(MARKDOWN_FONT_SIZE * 0.2)).line_height(nsh_core::LineHeight::Normal)
        .app_class(tw("inline-flex max-w-full items-center font-medium text-(--context-chip-thread) cursor-pointer focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-current rounded-sm no-underline"))
                    .on_open(move |_: &SharedString| {
                        if !chip.with_untracked(|chip| chip.missing) { nav.open(Route::Thread(chip.with_untracked(|chip| chip.key.clone()))); }
                    })
                    .child(dynamic(move || {
                        project.get().map_or_else(
                            || markdown_chip_icon(Icon::MessagesSquare).into_element(),
                            |project| view().w(px(MARKDOWN_FONT_SIZE)).h(px(MARKDOWN_FONT_SIZE)).child(crate::ui::avatar::project_favicon(
                                move || crate::ui::avatar::ProjectFaviconProject::from_shell(chip.with(|chip| chip.key.environment.clone()), &project),
                                |_| None,
                                "size-full self-center",
                                None,
                            )).into_element(),
                        )
                    }))
                    .child(context_chip_label(move || chip.with(|chip| chip.title.resolve()), hovered, false)))
                .into_element()
        })
    })
}

/// Port of source-control-*/src/shared/icon.ts, as SourceControlIconArtwork draws it.
fn source_control_host_icon(host: &str) -> Option<nsh_core::element::ImageSrc> {
    let markup = match host {
        "github.com" => {
            r###"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none"><path d="M8 0C3.58 0 0 3.58 0 8C0 11.54 2.29 14.53 5.47 15.59C5.87 15.66 6.02 15.42 6.02 15.21C6.02 15.02 6.01 14.39 6.01 13.72C4 14.09 3.48 13.23 3.32 12.78C3.23 12.55 2.84 11.84 2.5 11.65C2.22 11.5 1.82 11.13 2.49 11.12C3.12 11.11 3.57 11.7 3.72 11.94C4.44 13.15 5.59 12.81 6.05 12.6C6.12 12.08 6.33 11.73 6.56 11.53C4.78 11.33 2.92 10.64 2.92 7.58C2.92 6.71 3.23 5.99 3.74 5.43C3.66 5.23 3.38 4.41 3.82 3.31C3.82 3.31 4.49 3.1 6.02 4.13C6.66 3.95 7.34 3.86 8.02 3.86C8.7 3.86 9.38 3.95 10.02 4.13C11.55 3.09 12.22 3.31 12.22 3.31C12.66 4.41 12.38 5.23 12.3 5.43C12.81 5.99 13.12 6.7 13.12 7.58C13.12 10.65 11.25 11.33 9.47 11.53C9.76 11.78 10.01 12.26 10.01 13.01C10.01 14.08 10 14.94 10 15.21C10 15.42 10.15 15.67 10.55 15.59C13.71 14.53 16 11.53 16 8C16 3.58 12.42 0 8 0Z" fill="currentColor" fill-rule="evenodd"/></svg>"###
        }
        "gitlab.com" => {
            r###"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" fill="none"><path d="m31.46 12.78-.04-.12-4.35-11.35A1.14 1.14 0 0 0 25.94.6c-.24 0-.47.1-.66.24-.19.15-.33.36-.39.6l-2.94 9h-11.9l-2.94-9A1.14 1.14 0 0 0 6.07.58a1.15 1.15 0 0 0-1.14.72L.58 12.68l-.05.11a8.1 8.1 0 0 0 2.68 9.34l.02.01.04.03 6.63 4.97 3.28 2.48 2 1.52a1.35 1.35 0 0 0 1.62 0l2-1.52 3.28-2.48 6.67-5h.02a8.09 8.09 0 0 0 2.7-9.36Z" fill="#E24329"/><path d="m31.46 12.78-.04-.12a14.75 14.75 0 0 0-5.86 2.64l-9.55 7.24 6.09 4.6 6.67-5h.02a8.09 8.09 0 0 0 2.67-9.36Z" fill="#FC6D26"/><path d="m9.9 27.14 3.28 2.48 2 1.52a1.35 1.35 0 0 0 1.62 0l2-1.52 3.28-2.48-6.1-4.6-6.07 4.6Z" fill="#FCA326"/><path d="M6.44 15.3a14.71 14.71 0 0 0-5.86-2.63l-.05.12a8.1 8.1 0 0 0 2.68 9.34l.02.01.04.03 6.63 4.97 6.1-4.6-9.56-7.24Z" fill="#FC6D26"/></svg>"###
        }
        "git.cafe" => {
            r###"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" fill="none"><path d="M4.8 4.8h54.4v54.4H4.8z" fill="#a78bfa"/></svg>"###
        }
        "codeberg.org" => {
            r###"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-6 -6 212 212" fill="none"><path d="M58 168 v-98 a50 50 0 0 1 50-50 h20" fill="none" stroke="#ff6600" stroke-width="25"/><path d="M58 168 v-30 a50 50 0 0 1 50-50 h20" fill="none" stroke="#d40000" stroke-width="25"/><path d="M124 20 a18 18 0 1 0 36 0 a18 18 0 1 0 -36 0z" fill="none" stroke="#ff6600" stroke-width="15"/><path d="M124 88 a18 18 0 1 0 36 0 a18 18 0 1 0 -36 0z" fill="none" stroke="#d40000" stroke-width="15"/><path d="M40 180 a18 18 0 1 0 36 0 a18 18 0 1 0 -36 0z" fill="none" stroke="#d40000" stroke-width="15"/></svg>"###
        }
        "bitbucket.org" => {
            r###"<svg xmlns="http://www.w3.org/2000/svg" viewBox="8.4 14.39 2481.29 2231.21" fill="none"><defs><linearGradient id="a" x1="945.1094" y1="1524.8389" x2="944.4923" y2="1524.1893" gradientTransform="matrix(1996.6343 0 0 -1480.3047 -1884485.625 2258195)" gradientUnits="userSpaceOnUse"><stop offset="0.18" stop-color="#0052CC"/><stop offset="1" stop-color="#2684FF"/></linearGradient></defs><path d="M88.92,14.4C45.02,13.83,8.97,48.96,8.41,92.86c-0.06,4.61,0.28,9.22,1.02,13.77l337.48,2048.72 c8.68,51.75,53.26,89.8,105.74,90.24h1619.03c39.38,0.5,73.19-27.9,79.49-66.78l337.49-2071.78c7.03-43.34-22.41-84.17-65.75-91.2 c-4.55-0.74-9.15-1.08-13.76-1.02L88.92,14.4z M1509.99,1495.09H993.24l-139.92-731h781.89L1509.99,1495.09z" fill="#2684FF"/><path d="M2379.27,763.06h-745.5l-125.12,730.42H992.31l-609.67,723.67c19.32,16.71,43.96,26,69.5,26.21h1618.13 c39.35,0.51,73.14-27.88,79.44-66.72L2379.27,763.06z" fill="url(#a)"/></svg>"###
        }
        "origin.cursor.com" => {
            r###"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 466.73 532.09" fill="none"><path d="M457.43,125.94L244.42,2.96c-6.84-3.95-15.28-3.95-22.12,0L9.3,125.94c-5.75,3.32-9.3,9.46-9.3,16.11v247.99c0,6.65,3.55,12.79,9.3,16.11l213.01,122.98c6.84,3.95,15.28,3.95,22.12,0l213.01-122.98c5.75-3.32,9.3-9.46,9.3-16.11v-247.99c0-6.65-3.55-12.79-9.3-16.11h-.01ZM444.05,151.99l-205.63,356.16c-1.39,2.4-5.06,1.42-5.06-1.36v-233.21c0-4.66-2.49-8.97-6.53-11.31L24.87,145.67c-2.4-1.39-1.42-5.06,1.36-5.06h411.26c5.84,0,9.49,6.33,6.57,11.39h-.01Z" fill="currentColor"/></svg>"###
        }
        _ => return None,
    };
    Some(nsh_core::element::ImageSrc::svg_markup(markup))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_plain_and_formatted_external_links_keep_text_formatting_and_bounded_nodes() {
        for length in [128, 16_000] {
            let url = format!("https://github.com/search?q={}", "a".repeat(length));
            let plain = markdown_external_link_content("github.com", Some(&url), Vec::new());
            assert_eq!(plain.len(), 2);
            assert!(
                matches!(&plain[0], Inline::Custom(node) if node.name == "markdown-external-leading" && node.attributes.get("host").map(String::as_str) == Some("github.com"))
            );
            assert_eq!(
                plain.iter().map(Inline::plain_text).collect::<String>(),
                url
            );
            let formatted = markdown_external_link_content(
                "github.com",
                None,
                vec![
                    Inline::text(&url),
                    Inline::Strong {
                        content: vec![Inline::text(String::from(" details"))],
                    },
                ],
            );
            assert_eq!(formatted.len(), 3);
            assert!(matches!(formatted[2], Inline::Strong { .. }));
            assert_eq!(
                formatted.iter().map(Inline::plain_text).collect::<String>(),
                format!("{url} details")
            );
        }
    }

    #[test]
    fn plugin_uses_client_file_rules_and_keeps_authored_text_literal() {
        use nsh_base::text_view::Parser;
        let plugin = MarkdownLinkPlugin {
            context: MarkdownLinkContext {
                environment: Some(EnvironmentId("local".into())),
                cwd: Some(Rc::from("/repo")),
                ..Default::default()
            },
        };
        let document = Parser::new().plugin(plugin).markdown("[entry](src/main.rs:12:3) `AGENTS.md` `script.ts:10` [Settings](https://example.com) [old](t3-thread://v1/provider%3A1)");
        let json = serde_json::to_string(&document).expect("document");
        assert_eq!(json.matches("markdown-file-link").count(), 2);
        assert!(json.contains("markdown-thread-link"));
        assert!(json.contains("provider%3A1"));
        assert!(document.plain_text().contains("Settings"));
        assert!(json.contains("AGENTS.md"));
        let plugin = MarkdownLinkPlugin {
            context: MarkdownLinkContext {
                cwd: Some(Rc::from("/repo")),
                ..Default::default()
            },
        };
        let mut resolved = Parser::new().markdown("[Settings][web] [See **source**][file]\n\n[web]: https://example.com \"Authored title\"\n[file]: src/main.rs:12:3");
        plugin.decorate_document(&mut resolved);
        let Block::Paragraph { content } = &resolved.blocks[0] else {
            panic!("paragraph");
        };
        let Inline::Custom(external) = &content[0] else {
            panic!("external link");
        };
        assert_eq!(external.name, "markdown-external-link");
        assert_eq!(
            external.attributes.get("title").map(String::as_str),
            Some("Authored title")
        );
        assert_eq!(
            external
                .content
                .iter()
                .map(Inline::plain_text)
                .collect::<String>(),
            "Settings"
        );
        let Inline::Custom(file) = &content[2] else {
            panic!("reference became file chip");
        };
        assert_eq!(file.name, "markdown-file-link");
        assert_eq!(file.text, "main.rs · L12:C3");
        assert!(
            matches!(file.content[1], Inline::Strong { .. }),
            "descriptive formatting retained"
        );
        let once = resolved.clone();
        plugin.decorate_document(&mut resolved);
        assert_eq!(
            resolved, once,
            "mounting an already-decorated block never duplicates favicons"
        );
    }
}
