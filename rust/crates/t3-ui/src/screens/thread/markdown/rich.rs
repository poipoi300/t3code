//! Port of apps/web/src/components/chat/{MarkdownTable,MarkdownMath,MarkdownMermaidCodeBlock,ChatMarkdownMedia}.tsx.

use std::{rc::Rc, time::Duration};

use nsh_base::text_view::{
    Block, ColumnAlign, Document, Inline, Table, TableCell, TextViewClasses,
};
use nsh_core::prelude::*;

use crate::i18n::t;
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;
use crate::ui::menu::{ItemOptions, MenuOptions, menu, menu_item, menu_popup_with};

/// ChatMarkdownMedia.tsx authoredImageSizeStyle, expressed without CSS calc/min strings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AuthoredImageSize {
    pub width: Option<f32>,
    pub max_width: Option<f32>,
    pub max_height: Option<f32>,
    pub aspect_ratio: Option<f32>,
}

pub fn authored_image_size_style(
    width: Option<&str>,
    height: Option<&str>,
    max_height_rem: f32,
) -> Option<AuthoredImageSize> {
    let dimension = |value: Option<&str>| {
        let value = value?.trim();
        let value = if value.to_ascii_lowercase().ends_with("px") {
            &value[..value.len() - 2]
        } else {
            value
        };
        let value = value.trim();
        let parsed = if let Some(digits) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            u64::from_str_radix(digits, 16)
                .ok()
                .map(|value| value as f32)
        } else if let Some(digits) = value
            .strip_prefix("0b")
            .or_else(|| value.strip_prefix("0B"))
        {
            u64::from_str_radix(digits, 2)
                .ok()
                .map(|value| value as f32)
        } else if let Some(digits) = value
            .strip_prefix("0o")
            .or_else(|| value.strip_prefix("0O"))
        {
            u64::from_str_radix(digits, 8)
                .ok()
                .map(|value| value as f32)
        } else {
            value.parse::<f32>().ok()
        };
        parsed.filter(|n| n.is_finite() && *n > 0.0)
    };
    let width = dimension(width);
    let height = dimension(height);
    match (width, height) {
        (Some(width), Some(height)) => Some(AuthoredImageSize {
            width: Some(width),
            aspect_ratio: Some(width / height),
            max_width: Some(
                480.0_f32.min(max_height_rem * nsh_core::geometry::REM_SIZE.0 * width / height),
            ),
            ..Default::default()
        }),
        (Some(width), None) => Some(AuthoredImageSize {
            max_width: Some(width.min(480.0)),
            ..Default::default()
        }),
        (None, Some(height)) => Some(AuthoredImageSize {
            max_height: Some(height.min(480.0)),
            ..Default::default()
        }),
        _ => None,
    }
}

pub fn markdown_image_copy(alt: &str, src: &str, title: Option<&str>) -> String {
    let alt = alt
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    let destination = if src
        .chars()
        .any(|ch| ch.is_whitespace() || "<>()".contains(ch))
    {
        format!(
            "<{}>",
            src.replace('\\', "\\\\")
                .replace('<', "\\<")
                .replace('>', "\\>")
                .replace('\n', "%0A")
                .replace('\r', "%0D")
        )
    } else {
        src.to_owned()
    };
    let suffix = title
        .map(|title| format!(" \"{}\"", title.replace('\\', "\\\\").replace('"', "\\\"")))
        .unwrap_or_default();
    format!("![{alt}]({destination}{suffix})")
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MarkdownMediaProps {
    pub src: Option<String>,
    pub alt: String,
    pub source_failed: bool,
    pub missing_file_access: bool,
    pub unavailable_detail: Option<crate::i18n::Text>,
    pub original_url: Option<String>,
    pub copy_markdown: Option<String>,
    pub size: Option<AuthoredImageSize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    #[default]
    Image,
    Video,
    Audio,
}

/// Keep by media identity, not by a re-signed URL.
#[derive(Clone, Copy)]
pub struct MarkdownImageState {
    pub loaded_src: Signal<Option<String>>,
    pub failed_src: Signal<Option<String>>,
}

impl Default for MarkdownImageState {
    fn default() -> Self {
        Self {
            loaded_src: signal(None),
            failed_src: signal(None),
        }
    }
}

/// ChatMarkdownMedia.tsx ChatMarkdownImage. NSH reports cached-image loads through on_load too.
pub fn chat_markdown_image(
    props: Memo<MarkdownMediaProps>,
    state: MarkdownImageState,
    standalone: bool,
) -> impl IntoElement {
    let authored_size = computed(move || props.with(|p| p.size));
    let source = computed(move || {
        props
            .with(|p| p.src.clone())
            .or_else(|| state.loaded_src.get())
    });
    let failed = computed(move || {
        props.with(|p| p.source_failed)
            || source
                .get()
                .is_some_and(|src| state.failed_src.get().as_ref() == Some(&src))
    });
    let settled = computed(move || {
        source.get().is_some() && !failed.get() && (!standalone || state.loaded_src.get().is_some())
    });
    view().app_class(tw("max-w-full")).child(dynamic(move || {
        let src = source.get();
        let failed = failed.get();
        let settled = settled.get();
        let size = authored_size.get();
        untracked(|| {
            if !standalone && failed {
                return chat_markdown_image_fallback(props, MediaKind::Image).into_element();
            }
            let frame = view().app_class(tw(&cn([
                Some("relative max-w-full"),
                (!settled && standalone).then_some("aspect-video w-full overflow-hidden bg-muted/60 max-w-[30rem] rounded-lg border border-border/40"),
            ])));
            let frame = media_size(frame, size);
            let frame = if failed {
                frame.role(AccessibilityRole::Alert).child(view()
                    .app_class(tw("flex size-full items-center justify-center p-2 text-center text-xs text-muted-foreground"))
                    .child(chat_markdown_media_unavailable_label(props, MediaKind::Image)))
            } else if let Some(src) = src {
                let loaded = src.clone();
                let rejected = src.clone();
                let img = image(nsh_core::element::ImageSrc::from(src))
                    .label(move || props.with(|p| SharedString::from(p.alt.clone())))
                    .on_load(move |_: &Size<DevicePixels>| { state.loaded_src.set(Some(loaded.clone())); state.failed_src.set(None); })
                    .on_error(move |_: &str| { state.failed_src.set(Some(rejected.clone())); state.loaded_src.set(None); })
                    .app_class(tw(if settled { "h-auto w-auto object-contain max-h-[30rem] max-w-[30rem]" } else { "invisible absolute inset-0 size-full" }))
                    .style_with(move |style| media_size_style(style, size))
                    .when(size.is_some_and(|size| size.width.is_some() && size.aspect_ratio.is_some()), |image| image.w_full().h_full());
                let frame = if settled { frame } else { frame.role(AccessibilityRole::Group).label(t("Loading image")) };
                frame.child(img)
            } else {
                frame.role(AccessibilityRole::Group).label(t("Loading image"))
            };
            frame.into_element()
        })
    }))
}

fn media_size(
    view: nsh_core::element::View,
    size: Option<AuthoredImageSize>,
) -> nsh_core::element::View {
    view.style_with(move |style| media_size_style(style, size))
}

fn media_size_style(
    view: nsh_core::element::StyleRefinement,
    size: Option<AuthoredImageSize>,
) -> nsh_core::element::StyleRefinement {
    let Some(size) = size else {
        return view;
    };
    let view = view.max_w(px(size.max_width.unwrap_or(480.0).min(window().width.0)));
    let view = if let Some(width) = size.width {
        view.w(px(width))
    } else {
        view
    };
    let view = if let Some(height) = size.max_height {
        view.max_h(px(height))
    } else {
        view
    };
    if let Some(ratio) = size.aspect_ratio {
        view.aspect_ratio(ratio)
    } else {
        view
    }
}

fn chat_markdown_media_unavailable_label(
    props: Memo<MarkdownMediaProps>,
    kind: MediaKind,
) -> impl IntoElement {
    view()
        .app_class(tw("inline-flex items-center gap-1.5"))
        .child(crate::ui::icon::icon(Icon::TriangleAlert, 14.0, || {
            crate::theme::palette().muted_foreground
        }))
        .child(text(move || {
            props.with(|p| {
                let label = match kind {
                    MediaKind::Image => t("Image unavailable"),
                    MediaKind::Video => t("Video unavailable"),
                    MediaKind::Audio => t("Audio unavailable"),
                };
                let mut parts = vec![label];
                if !p.alt.is_empty() {
                    parts.push(p.alt.clone());
                }
                if let Some(detail) = &p.unavailable_detail {
                    let detail = detail.resolve();
                    if !detail.is_empty() {
                        parts.push(detail.to_string());
                    }
                }
                SharedString::from(parts.join(" · "))
            })
        }))
}

pub fn chat_markdown_image_fallback(
    props: Memo<MarkdownMediaProps>,
    kind: MediaKind,
) -> impl IntoElement {
    view().app_class(tw("rounded-md border border-border/40 bg-muted/40 px-2 py-1 text-xs text-muted-foreground"))
        .child(chat_markdown_media_unavailable_label(props, kind))
}

/// The asset state comes from the client's guarded AssetService, never a UI permission decision.
pub fn asset_media_props(
    state: &t3_client::services::assets::AssetUrlState,
    fallback: Option<&str>,
    fragment: &str,
    mut props: MarkdownMediaProps,
) -> MarkdownMediaProps {
    use t3_client::services::assets::{AssetUrlState, MISSING_FILE_ACCESS_MESSAGE};
    props.src = match state {
        AssetUrlState::Success { url, .. } => Some(format!("{url}{fragment}")),
        AssetUrlState::Failure { .. } => fallback.map(|url| format!("{url}{fragment}")),
        AssetUrlState::Loading => None,
    };
    props.source_failed = matches!(state, AssetUrlState::Failure { .. }) && fallback.is_none();
    props.missing_file_access = matches!(
        state,
        AssetUrlState::Failure {
            missing_file_access: true
        }
    ) && fallback.is_none();
    props.unavailable_detail = (matches!(
        state,
        AssetUrlState::Failure {
            missing_file_access: true
        }
    ) && fallback.is_none())
    .then(|| crate::i18n::Text::client_key(MISSING_FILE_ACCESS_MESSAGE));
    props
}

pub type RetryMedia = Rc<dyn Fn() -> futures::future::LocalBoxFuture<'static, Result<(), String>>>;

/// ChatMarkdownMedia.tsx ChatMarkdownVideo and MediaVideoPlayer's native-controls path.
pub fn chat_markdown_video(
    props: Memo<MarkdownMediaProps>,
    on_retry: Option<RetryMedia>,
) -> impl IntoElement {
    let failed_src = signal(None::<String>);
    let retrying = signal(false);
    let attempt = signal(0u64);
    let owner = Owner::current();
    view().app_class(tw("w-full max-w-[30rem]"))
      .style_with(move |style| media_size_style(style, props.with(|p| p.size)))
      .child(dynamic(move || {
        let current = props.get();
        let failed = current.src.as_ref().map_or(current.source_failed, |src| failed_src.get().as_ref() == Some(src));
        let epoch = attempt.get();
        let retry = on_retry.clone();
        untracked(|| {
            if failed {
                let can_retry = !current.missing_file_access && (current.src.is_some() || retry.is_some());
                return view().role(AccessibilityRole::Alert)
                    .app_class(tw("flex aspect-video max-h-full w-full flex-col items-center justify-center gap-3 rounded-lg border border-border/40 bg-muted/40 p-4 text-center text-sm text-muted-foreground"))
                    .child(chat_markdown_media_unavailable_label(props, MediaKind::Video))
                    .child(can_retry.then(|| {
                        let retry = retry.clone();
                        // Button's text slot is static; retain both states instead of disposing a pending button prop.
                        view()
                          .child(view().app_class(move || Some(tw(if retrying.get() { "hidden" } else { "flex" })))
                          .child(button(ButtonContent::Label(t("Retry video")), ButtonVariant::Secondary, ButtonSize::Sm, move || retrying.get(), move || {
                            if retrying.get_untracked() { return; }
                            retrying.set(true);
                            let retry = retry.clone();
                            owner.run(|| spawn(async move {
                                let result = if let Some(retry) = retry { retry().await } else { Ok(()) };
                                if result.is_ok() { failed_src.set(None); attempt.update(|n| *n += 1); }
                                retrying.set(false);
                            }));
                          })))
                          .child(view().app_class(move || Some(tw(if retrying.get() { "flex" } else { "hidden" })))
                            .child(button(ButtonContent::Label(t("Retrying…")), ButtonVariant::Secondary, ButtonSize::Sm, || true, || {})))
                    })).into_element();
            }
            if let Some(src) = current.src {
                let rejected = src.clone();
                // NSH supplies native playback controls, but no play/pause or page-visibility events.
                let _ = epoch;
                video(src).test_id("markdown-video")
                    .label(move || props.with(|p| if p.alt.is_empty() { t("Video preview") } else { p.alt.clone() }))
                    .controls(true).paused(true)
                    .on_error(move |_: &str| failed_src.set(Some(rejected.clone())))
                    .app_class(tw("aspect-video max-h-[30rem] w-full bg-black object-contain rounded-lg border border-border/40"))
                    .into_element()
            } else {
                view().role(AccessibilityRole::Group).label(t("Loading video"))
                    .app_class(tw("block aspect-video w-full rounded-lg bg-muted/60"))
                    .into_element()
            }
        })
    }))
}

/// A parsed block's table; completed blocks keep this memo outside recycled cells.
pub fn markdown_table(table: Memo<Table>) -> impl IntoElement {
    view()
        .app_class(tw("w-full max-w-full"))
        .child(dynamic(move || {
            let table = table.get();
            untracked(|| table_grid(&table))
        }))
        .child(markdown_table_actions(table))
}

/// A table row's exact block source, supplied by TL-BLOCKS-1 rather than a whole message.
pub fn markdown_table_block(source: Memo<Rc<str>>) -> impl IntoElement {
    let parsed = computed(move || {
        source.with(|source| {
            nsh_base::text_view::Parser::new()
                .markdown(source)
                .blocks
                .into_iter()
                .find_map(|block| {
                    if let Block::Table(table) = block {
                        Some(table)
                    } else {
                        None
                    }
                })
        })
    });
    view().child(when(
        move || parsed.with(Option::is_some),
        move || untracked(|| markdown_table(computed(move || parsed.get().unwrap_or_default()))),
    ))
}

fn table_grid(table: &Table) -> AnyElement {
    let columns = table.columns();
    if columns == 0 {
        return view().into_element();
    }
    let mut grid = view()
        .role(AccessibilityRole::Table)
        // index.css .chat-markdown table: a shared column layout, with no outer grid border.
        // The block timeline forbids nested scrollers; columns wrap within the lane instead.
        .app_class(tw("grid w-full text-sm"))
        .grid_template_columns(SharedString::from(
            vec!["minmax(0, auto)"; columns].join(" "),
        ));
    for (row_index, row) in std::iter::once(&table.head).chain(&table.rows).enumerate() {
        let header = row_index == 0;
        let mut column = 0;
        for cell in row {
            if column >= columns {
                break;
            }
            let span = cell.span().min(columns - column);
            grid = grid.child(table_cell(
                cell,
                row_index,
                column,
                span,
                columns,
                header,
                table.align.get(column).copied().unwrap_or_default(),
            ));
            column += span;
        }
        for column in column..columns {
            grid = grid.child(table_cell(
                &TableCell::default(),
                row_index,
                column,
                1,
                columns,
                header,
                table.align.get(column).copied().unwrap_or_default(),
            ));
        }
    }
    grid.into_element()
}

fn table_cell(
    cell: &TableCell,
    row: usize,
    column: usize,
    span: usize,
    columns: usize,
    header: bool,
    align: ColumnAlign,
) -> impl IntoElement {
    let content = Document::new(vec![Block::paragraph(cell.content.clone())]);
    // index.css .chat-markdown :is(th,td): first/last padding and the math width exception.
    let math = cell.content.iter().any(has_math);
    view()
        .role(if header { AccessibilityRole::ColumnHeader } else { AccessibilityRole::Cell })
        .row_start(i16::try_from(row + 1).unwrap_or(i16::MAX))
        .col_start(i16::try_from(column + 1).unwrap_or(i16::MAX))
        .col_end(i16::try_from(column + span + 1).unwrap_or(i16::MAX))
        .app_class(tw(&cn([
            Some("min-w-0 px-3 py-[0.45rem] border-b border-contrast-border/60 text-start text-foreground/90"),
            header.then_some("py-[0.55rem] font-semibold"),
            (column == 0).then_some("ps-0"),
            (column + span == columns).then_some("pe-0"),
            (!math).then_some("max-w-[24rem]"),
        ])))
        .child(nsh_base::text_view()
            .document(Some(content))
            .selectable(true)
            .render_math(|formula: &nsh_base::text_view::TextViewMath| {
                let formula = formula.clone();
                markdown_math(computed(move || formula.clone()), false)
            })
            .classes(TextViewClasses { paragraph: Some(tw("m-0")), ..Default::default() })
            .app_class(tw(match align {
                ColumnAlign::Center => "w-full text-center text-foreground/90",
                ColumnAlign::Right => "w-full text-right text-foreground/90",
                ColumnAlign::Left => "w-full text-left text-foreground/90",
                ColumnAlign::None => "w-full text-start text-foreground/90",
            })))
}

fn has_math(inline: &Inline) -> bool {
    match inline {
        Inline::Math { .. } => true,
        Inline::Strong { content }
        | Inline::Emphasis { content }
        | Inline::Strikethrough { content }
        | Inline::Underline { content }
        | Inline::Highlight { content, .. }
        | Inline::Superscript { content }
        | Inline::Subscript { content }
        | Inline::Small { content }
        | Inline::Link { content, .. } => content.iter().any(has_math),
        _ => false,
    }
}

pub fn markdown_table_actions(table: Memo<Table>) -> impl IntoElement {
    let copied = signal(false);
    let open = signal(false);
    let reset = stored(None::<Spawned<()>>);
    let owner = Owner::current();
    let copy = move |csv: bool| {
        let value = table.with(|table| {
            if csv {
                serialize_table_to_csv(table)
            } else {
                serialize_table_to_markdown(table)
            }
        });
        if value.is_empty() {
            return;
        }
        // NSH's clipboard write returns no result, as in MarkdownCodeBlock's default adapter.
        set_clipboard(value);
        copied.set(true);
        owner.run(|| {
            if let Some(timer) = reset.replace(None) {
                timer.cancel();
            }
            reset.replace(Some(set_timeout(Duration::from_millis(1200), move || {
                copied.set(false)
            })));
        });
    };
    view()
        .app_class(tw("mt-0.5 -mx-1 flex items-center justify-end select-none"))
        .child(menu(
            view()
                .app_class(tw("w-fit"))
                .tooltip(crate::ui::tooltip::tooltip(
                    crate::ui::tooltip::tooltip_create_handle(),
                    Default::default(),
                    text(move || {
                        if copied.get() {
                            t("Copied")
                        } else {
                            t("Copy table")
                        }
                    }),
                ))
                .child(dynamic(move || {
                    button(
                        ButtonContent::Icon {
                            icon: if copied.get() {
                                Icon::Check
                            } else {
                                Icon::Copy
                            },
                            label: if copied.get() {
                                t("Copied")
                            } else {
                                t("Copy table")
                            },
                        },
                        ButtonVariant::GhostMuted,
                        ButtonSize::IconXs,
                        || false,
                        move || open.set(!open.get_untracked()),
                    )
                    .into_element()
                })),
            MenuOptions {
                open: Some(open),
                align: Align::End,
                ..Default::default()
            },
            menu_popup_with(move || {
                view()
                    .child(menu_item(
                        t("Copy as Markdown"),
                        ItemOptions::default(),
                        text(t("Copy as Markdown")),
                        move || copy(false),
                    ))
                    .child(menu_item(
                        t("Copy as CSV"),
                        ItemOptions::default(),
                        text(t("Copy as CSV")),
                        move || copy(true),
                    ))
            }),
        ))
}

/// Port of markdown-clipboard.ts serializeTableElementToMarkdown, over the parsed cells.
pub fn serialize_table_to_markdown(table: &Table) -> String {
    let mut lines = Vec::new();
    for row in std::iter::once(&table.head)
        .chain(&table.rows)
        .filter(|row| !row.is_empty())
    {
        lines.push(format!(
            "| {} |",
            row.iter()
                .map(|cell| {
                    inline_markdown(&cell.content)
                        .split('\n')
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim_matches(js_space)
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(" | ")
        ));
        if lines.len() == 1 {
            lines.push(format!(
                "| {} |",
                row.iter()
                    .enumerate()
                    .map(|(column, _)| match table.align.get(column) {
                        Some(ColumnAlign::Center) => ":---:",
                        Some(ColumnAlign::Right) => "---:",
                        _ => "---",
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
    }
    lines.join("\n")
}

/// Port of markdown-clipboard.ts serializeTableElementToCsv.
pub fn serialize_table_to_csv(table: &Table) -> String {
    std::iter::once(&table.head)
        .chain(&table.rows)
        .filter(|row| !row.is_empty())
        .map(|row| {
            row.iter()
                .map(|cell| {
                    let value = inline_csv(&cell.content)
                        .split(js_space)
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if value.contains([',', '"', '\n']) {
                        format!("\"{}\"", value.replace('"', "\"\""))
                    } else {
                        value
                    }
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn inline_csv(content: &[Inline]) -> String {
    content
        .iter()
        .map(|inline| match inline {
            Inline::Math { tex } => math_source(tex, false),
            Inline::Strong { content }
            | Inline::Emphasis { content }
            | Inline::Strikethrough { content }
            | Inline::Underline { content }
            | Inline::Highlight { content, .. }
            | Inline::Superscript { content }
            | Inline::Subscript { content }
            | Inline::Small { content }
            | Inline::Link { content, .. } => inline_csv(content),
            _ => inline.plain_text(),
        })
        .collect()
}

fn inline_markdown(content: &[Inline]) -> String {
    content
        .iter()
        .map(|inline| match inline {
            Inline::Math { tex } => math_source(tex, false),
            Inline::Strong { content } => marked(content, "**"),
            Inline::Emphasis { content } => marked(content, "*"),
            Inline::Strikethrough { content } => marked(content, "~~"),
            Inline::Underline { content }
            | Inline::Highlight { content, .. }
            | Inline::Superscript { content }
            | Inline::Subscript { content }
            | Inline::Small { content } => inline_markdown(content),
            Inline::Code { code } => {
                let code = code.replace('|', "\\|");
                let longest = code.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
                let fence = "`".repeat(longest + 1);
                let pad = if code.starts_with('`') || code.ends_with('`') {
                    " "
                } else {
                    ""
                };
                format!("{fence}{pad}{code}{pad}{fence}")
            }
            Inline::Link { url, content, .. } => {
                let label = inline_markdown(content);
                if url.to_ascii_lowercase().starts_with("https://")
                    || url.to_ascii_lowercase().starts_with("http://")
                {
                    let url = url.replace('|', "\\|");
                    if label.trim() == url {
                        url
                    } else {
                        format!("[{}]({url})", label.trim())
                    }
                } else {
                    label
                }
            }
            Inline::Image(image) => {
                if image.alt.is_empty() || image.src.is_empty() {
                    String::new()
                } else {
                    format!(
                        "![{}]({})",
                        image.alt.replace('|', "\\|"),
                        image.src.replace('|', "\\|")
                    )
                }
            }
            _ => inline.plain_text().replace('|', "\\|"),
        })
        .collect()
}

fn marked(content: &[Inline], marker: &str) -> String {
    let value = inline_markdown(content);
    let core = value.trim();
    if core.is_empty() {
        return value;
    }
    let start = value.len() - value.trim_start().len();
    format!(
        "{}{marker}{core}{marker}{}",
        &value[..start],
        &value[start + core.len()..]
    )
}

pub fn math_source(tex: &str, display: bool) -> String {
    let tex = tex.trim_matches(js_space);
    if display {
        format!("\\[\n{tex}\n\\]")
    } else {
        format!("\\({tex}\\)")
    }
}

/// MarkdownMath's readable fallback while the shared typesetter is unavailable.
pub fn markdown_math(
    formula: Memo<nsh_base::text_view::TextViewMath>,
    focusable: bool,
) -> impl IntoElement {
    let result = view()
        // The block timeline has one scroller; long fallback TeX wraps inside its block.
        .app_class(move || {
            Some(tw(if formula.with(|f| f.display) {
                "my-[0.65rem] block max-w-full"
            } else {
                "inline-block max-w-full align-middle"
            }))
        })
        .child(
            view()
                .app_class(tw("block max-w-full px-0.5 py-0.75"))
                .child(
                    text(move || {
                        formula.with(|f| SharedString::from(math_source(&f.tex, f.display)))
                    })
                    .select_text(),
                ),
        );
    if focusable {
        result.role(AccessibilityRole::Group).label(move || {
            if formula.with(|f| f.display) {
                t("Display equation")
            } else {
                t("Inline equation")
            }
        })
    } else {
        result
    }
}

pub fn math_copy_source(tex: &str, display: bool) -> String {
    let source = math_source(tex, display);
    if display {
        format!("{source}\n\n")
    } else {
        source
    }
}

/// Port of MathTypeset.logic.ts escapeLiteralPercents (used only after a failed typeset).
pub fn escape_literal_percents(tex: &str) -> String {
    let bytes = tex.as_bytes();
    let mut output = String::new();
    let mut copied = 0;
    let mut index = 0;
    while index < tex.len() {
        if bytes[index] == b'\\' {
            if tex[index..].starts_with("\\verb") {
                let starred = bytes.get(index + 5) == Some(&b'*');
                let delimiter_at = index + if starred { 6 } else { 5 };
                if let Some(delimiter) = tex.get(delimiter_at..).and_then(|s| s.chars().next())
                    && (starred || (delimiter != '*' && !delimiter.is_ascii_alphabetic()))
                {
                    let start = delimiter_at + delimiter.len_utf8();
                    let end = tex[start..]
                        .find([delimiter, '\n', '\r'])
                        .map(|at| start + at);
                    let Some(end) = end.filter(|at| tex[*at..].starts_with(delimiter)) else {
                        return tex.to_owned();
                    };
                    index = end + delimiter.len_utf8();
                    continue;
                }
            }
            index += 1;
            if let Some(ch) = tex.get(index..).and_then(|s| s.chars().next()) {
                index += ch.len_utf8();
            }
        } else if bytes[index] == b'%'
            && index > 0
            && !tex[..index].chars().next_back().is_some_and(js_space)
        {
            output.push_str(&tex[copied..index]);
            output.push_str("\\%");
            index += 1;
            copied = index;
        } else if bytes[index] == b'%' {
            index = tex[index..]
                .find('\n')
                .map_or(tex.len(), |at| index + at + 1);
        } else {
            index += tex[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if copied == 0 {
        tex.to_owned()
    } else {
        output.push_str(&tex[copied..]);
        output
    }
}

fn js_space(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// Source fallback for a Mermaid fence, composed with TL-MARKDOWN-1's actual chrome.
/// The shared diagram renderer is a frozen dependency seam; no substitute lexer/layout lives here.
pub fn markdown_mermaid_code_block(
    props: Memo<super::code_block::CodeBlockProps>,
    state: super::code_block::CodeBlockState,
) -> impl IntoElement {
    let complete = computed(move || props.with(|props| !props.is_streaming));
    // While streaming TL-BLOCKS-2 renders the bounded code chunks, never this whole fence.
    view().child(when(complete, move || {
        untracked(|| {
            let code = computed(move || {
                props.with(|props| super::code_block::CodeBlockProps {
                    language: "mermaid".into(),
                    can_wrap: true,
                    diagram: false,
                    run_shell_command_available: false,
                    ..props.clone()
                })
            });
            super::code_block::markdown_code_block(
                code,
                state,
                Default::default(),
                move |wrapped| {
                    text(move || code.with(|p| SharedString::from(p.code.clone())))
                        .select_text()
                        .text_size(move || {
                            px(tokens()
                                .number("font-size-code")
                                .unwrap_or(t3_theme::fonts::DEFAULT_CODE_FONT_SIZE as f32))
                        })
                        .app_class(move || {
                            Some(tw(if wrapped.get() {
                                "px-[0.9rem] py-[0.8rem] font-mono leading-snug whitespace-normal"
                            } else {
                                "px-[0.9rem] py-[0.8rem] font-mono leading-snug whitespace-nowrap"
                            }))
                        })
                        .into_element()
                },
            )
        })
    }))
}

/// Port of MarkdownDetails.tsx. Keep `open` by BlockId; TL-FIND reveals it with `open.set(true)`.
pub fn markdown_details(
    open: Signal<bool>,
    summary: Vec<Inline>,
    children: Document,
) -> impl IntoElement {
    use crate::ui::collapsible::{
        CollapsibleHandle, collapsible, collapsible_panel, collapsible_trigger,
    };
    let label = summary.iter().map(Inline::plain_text).collect::<String>();
    let label = if label.is_empty() {
        crate::i18n::Text::from("Details")
    } else {
        crate::i18n::Text::content(label)
    };
    let content = if summary.is_empty() {
        text(move || t("Details")).into_element()
    } else {
        nsh_base::text_view()
            .document(Some(Document::new(vec![Block::paragraph(summary)])))
            .classes(TextViewClasses {
                paragraph: Some(tw("m-0")),
                ..Default::default()
            })
            .into_element()
    };
    let handle =
        CollapsibleHandle::controlled(move || open.get(), || false, move |value| open.set(value));
    view()
        .app_class(tw("my-2 border-y border-border/60"))
        .child(collapsible(
        view()
            .child(collapsible_trigger(
                handle,
                label,
                view()
                    .app_class(tw("flex items-center gap-2"))
                    .child(
                        view()
                            .app_class(move || {
                                Some(tw(&cn([
                                    Some("size-4 shrink-0 text-muted-foreground"),
                                    open.get().then_some("rotate-90"),
                                    (!reduce_motion()).then_some("transition-transform"),
                                ])))
                            })
                            .child(crate::ui::icon::icon(Icon::ChevronRight, 16.0, || {
                                crate::theme::palette().muted_foreground
                            })),
                    )
                    .child(content),
                "flex w-full items-center gap-2 py-2 text-left text-sm font-medium text-foreground",
            ))
            .child(collapsible_panel(
                handle,
                true,
                view().app_class(tw("pb-3 ps-6 text-foreground/90")).child(
                    nsh_base::text_view()
                        .document(Some(children))
                        .selectable(true),
                ),
            )),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_percent_repair_ports_every_upstream_logic_case() {
        for (source, expected) in [
            (r"\text{3.18% yield}", r"\text{3.18\% yield}"),
            (r"\text{100%% yield}", r"\text{100\%\% yield}"),
            (r"6\% + 2\%", r"6\% + 2\%"),
            (r"x\\%", r"x\\\%"),
            ("x % note\n+ 1", "x % note\n+ 1"),
            ("% note\n\\text{2%}", "% note\n\\text{2\\%}"),
            ("x &= 1 % +9%\n\\text{2%}", "x &= 1 % +9%\n\\text{2\\%}"),
            (r"\verb|50%|+\text{2%}", r"\verb|50%|+\text{2\%}"),
            (r"\verb*%a%b%+c%", r"\verb*%a%b\%+c\%"),
            (r"\verb|50%+\text{2%}", r"\verb|50%+\text{2%}"),
            ("é%\u{feff}%comment", "é\\%\u{feff}%comment"),
        ] {
            assert_eq!(escape_literal_percents(source), expected, "{source}");
        }
    }

    #[test]
    fn formula_copy_preserves_whole_tex_and_display_block_boundaries() {
        assert_eq!(math_copy_source("  |x|  ", false), r"\(|x|\)");
        assert_eq!(math_copy_source(" x\n y ", true), "\\[\nx\n y\n\\]\n\n");
        assert_eq!(math_source(" ", false), r"\(\)");
    }

    #[test]
    fn authored_sizes_accept_px_and_bound_portrait_images_without_losing_ratio() {
        assert_eq!(
            authored_image_size_style(Some("14px"), Some("7PX"), 30.0),
            Some(AuthoredImageSize {
                width: Some(14.0),
                max_width: Some(480.0),
                aspect_ratio: Some(2.0),
                ..Default::default()
            })
        );
        assert_eq!(
            authored_image_size_style(Some("1000"), Some("2000"), 30.0)
                .expect("size")
                .max_width,
            Some(240.0)
        );
        assert_eq!(
            authored_image_size_style(Some("1000"), Some("2000"), 10.0)
                .expect("size")
                .max_width,
            Some(80.0)
        );
        assert_eq!(
            authored_image_size_style(Some(" 2e2px "), None, 30.0)
                .expect("width")
                .max_width,
            Some(200.0)
        );
        assert_eq!(
            authored_image_size_style(None, Some("100px"), 30.0)
                .expect("height")
                .max_height,
            Some(100.0)
        );
        for value in ["14 px", "0x0e", "0b1110", "0o16"] {
            assert_eq!(
                authored_image_size_style(Some(value), None, 30.0)
                    .expect("width")
                    .max_width,
                Some(14.0)
            );
        }
        for value in ["0", "-2", "NaN", "Infinity", "50%", "auto"] {
            assert_eq!(
                authored_image_size_style(Some(value), None, 30.0),
                None,
                "{value}"
            );
        }
    }

    #[test]
    fn media_copy_escapes_titles_alts_and_raw_html_destinations() {
        assert_eq!(
            markdown_image_copy("a[b]\\c", "/a (b)\nc.png", Some("a\"b\\c")),
            "![a\\[b\\]\\\\c](</a (b)%0Ac.png> \"a\\\"b\\\\c\")"
        );
        assert_eq!(
            markdown_image_copy("", "https://example.test/x.png", None),
            "![](https://example.test/x.png)"
        );
    }

    #[test]
    fn asset_failure_and_fallback_keep_permission_explanations_and_urls_distinct() {
        use t3_client::services::assets::AssetUrlState;
        let denied = AssetUrlState::Failure {
            missing_file_access: true,
        };
        let props = asset_media_props(&denied, None, "#page=1", MarkdownMediaProps::default());
        assert!(props.source_failed && props.missing_file_access);
        assert!(props.src.is_none());
        assert_eq!(
            props.unavailable_detail,
            Some(crate::i18n::Text::client_key(
                t3_client::services::assets::MISSING_FILE_ACCESS_MESSAGE
            ))
        );
        let fallback = asset_media_props(
            &denied,
            Some("https://example.test/fallback"),
            "#page=1",
            props,
        );
        assert_eq!(
            fallback.src.as_deref(),
            Some("https://example.test/fallback#page=1")
        );
        assert!(!fallback.source_failed && !fallback.missing_file_access);
        assert_eq!(fallback.unavailable_detail, None);
        let loading = asset_media_props(&AssetUrlState::Loading, Some("fallback"), "", fallback);
        assert_eq!(loading.src, None);
        assert!(!loading.source_failed && !loading.missing_file_access);
    }

    #[test]
    fn table_copy_keeps_formula_pipes_and_escapes_prose_code_and_link_pipes() {
        let table: Table = serde_json::from_value(serde_json::json!({
            "head": [{"content":[{"type":"text","text":"Formula"}]},{"content":[{"type":"text","text":"Note"}]}],
            "align": ["center", "right"],
            "rows": [
                [{"content":[{"type":"math","tex":"|x|"}]},{"content":[{"type":"text","text":"a | b"}]}],
                [{"content":[{"type":"math","tex":"\\|x\\|"}]},{"content":[{"type":"code","code":"\\(c|d\\)"}]}],
                [{"content":[{"type":"link","url":"https://example.com/?q=a|b","content":[{"type":"text","text":"norm "},{"type":"math","tex":"|y|"}]}]},{"content":[]}]
            ]
        })).expect("parsed cells");
        assert_eq!(
            serialize_table_to_markdown(&table),
            "| Formula | Note |\n| :---: | ---: |\n| \\(|x|\\) | a \\| b |\n| \\(\\|x\\|\\) | `\\(c\\|d\\)` |\n| [norm \\(|y|\\)](https://example.com/?q=a\\|b) |  |"
        );
        assert_eq!(
            serialize_table_to_csv(&table),
            "Formula,Note\n\\(|x|\\),a | b\n\\(\\|x\\|\\),\\(c|d\\)\nnorm \\(|y|\\),"
        );
    }

    #[test]
    fn table_copy_keeps_inline_markers_spaces_and_quotes_csv() {
        let table: Table = serde_json::from_value(serde_json::json!({
            "head": [{"content":[{"type":"text","text":"Label"}]}],
            "rows": [
                [{"content":[{"type":"strong","content":[{"type":"text","text":" bold "}]},{"type":"code","code":"`ticks`"}]}],
                [{"content":[{"type":"text","text":"  a, \"b\"\n c  "}]}]
            ]
        })).expect("parsed cells");
        assert_eq!(
            serialize_table_to_markdown(&table),
            "| Label |\n| --- |\n| **bold** `` `ticks` `` |\n| a, \"b\"  c |"
        );
        assert_eq!(
            serialize_table_to_csv(&table),
            "Label\nbold `ticks`\n\"a, \"\"b\"\" c\""
        );
        assert_eq!(serialize_table_to_markdown(&Table::default()), "");
        assert_eq!(serialize_table_to_csv(&Table::default()), "");
    }
}
