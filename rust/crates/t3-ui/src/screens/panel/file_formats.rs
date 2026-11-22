//! Port of apps/web/src/components/files LargeTextViewer.tsx and VirtualTablePreview.tsx (the
//! delimited-table mode) and NotebookPreview.tsx. Parsing is the client's
//! `workspace::project_files::formats`; this file draws it.
//!
//! Dropped: `contain-strict`, `backdrop-blur` on the sticky header (no NSH form),
//! `scrollbar-gutter-both`, `whitespace-pre*`/`break-words` (NSH text keeps newlines and wraps).

mod binary;
mod image;
mod notebook;

pub(super) use binary::binary_preview;
pub(super) use image::image_preview;

use std::{cell::Cell, rc::Rc};

use nsh_core::prelude::*;
use t3_client::workspace::project_files::{
    assets::AssetReadError,
    formats::{
        CellKind, DelimitedLimits, NOTEBOOK_MAX_BYTES, Notebook, NotebookCell,
        notebook_cell_language, open_notebook, parse_delimited_preview,
    },
};

use super::file_viewer::{PreviewAsset, file_code_chunks, file_surface_loading};
use crate::i18n::{Value, t, tr};
use crate::screens::thread::blocks::code::{CodeHighlightState, code_chunk_row, highlighted_chunk};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};

/// The worker's whole-file table, bounded so one parse per read stays cheap.
const TABLE_LIMITS: DelimitedLimits = DelimitedLimits {
    max_rows: 100_000,
    max_columns: 1024,
    max_cell_length: 2000,
};
const COLUMN_WIDTH: f32 = 160.0;
const ROW_HEADER_WIDTH: f32 = 56.0;

fn count_label(count: usize, one: &str, many: &str) -> String {
    let formatted = crate::i18n::formats().decimal(count as f64);
    if count == 1 {
        tr(one, &[("count", Value::Text(formatted))])
    } else {
        tr(many, &[("count", Value::Text(formatted))])
    }
}

/// Port of DelimitedTablePreview → LargeTextViewer (table mode) → VirtualTablePreview: a sticky
/// header row and virtualised body rows with a row number gutter. Parsed once per read.
pub fn delimited_table_preview(source: &str, delimiter: char) -> impl IntoElement {
    let parsed = parse_delimited_preview(source, delimiter, TABLE_LIMITS);
    let columns = parsed.rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut rows = parsed.rows;
    let header = (!rows.is_empty()).then(|| rows.remove(0));
    let rows = Rc::new(rows);
    let count = rows.len();
    let width = ROW_HEADER_WIDTH + columns as f32 * COLUMN_WIDTH;
    let status = format!(
        "{} · {}",
        count_label(count, "{count} row", "{count} rows"),
        count_label(columns, "{count} column", "{count} columns")
    );
    let body = if header.is_none() {
        view()
            .app_class(tw(
                "flex flex-1 items-center justify-center p-6 text-xs text-muted-foreground",
            ))
            .child(text(t("This file is empty.")))
            .into_element()
    } else {
        let header = header.unwrap_or_default();
        let header_row = view()
            .app_class(tw("relative z-10 flex min-h-[29px] shrink-0 border-b border-border bg-muted/60 font-medium"))
            .child(view().app_class(tw("w-14 shrink-0 border-r border-border bg-background")))
            .children((0..columns).map(|column| {
                text(header.get(column).cloned().unwrap_or_else(|| column_label(column)))
                    .w(px(COLUMN_WIDTH))
                    .app_class(tw("shrink-0 truncate border-r border-border/40 px-3 py-1.5"))
                    .into_element()
            }));
        let list = virtual_list(count, move |at| {
            let rows = rows.clone();
            let index = at.index();
            view()
                .app_class(tw(&cn([
                    Some("flex min-h-[29px] border-b border-border/60"),
                    (index % 2 == 1).then_some("bg-muted/30"),
                ])))
                .child(
                    text(crate::i18n::formats().decimal((index + 1) as f64))
                        .app_class(tw("relative z-1 w-14 shrink-0 border-r border-border bg-background px-2 py-1.5 text-right font-normal text-muted-foreground tabular-nums select-none")),
                )
                .children((0..columns).map(move |column| {
                    text(rows.get(index).and_then(|row| row.get(column)).cloned().unwrap_or_default())
                        .w(px(COLUMN_WIDTH))
                        .app_class(tw("shrink-0 border-r border-border/40 px-3 py-1.5 tabular-nums"))
                        .into_element()
                }))
        });
        scroll()
            .horizontal()
            .app_class(tw("relative min-h-0 flex-1"))
            .child(
                view()
                    .w(px(width))
                    .app_class(tw("flex h-full min-h-0 flex-col text-xs"))
                    .child(header_row)
                    .child(list.app_class(tw("min-h-0 flex-1"))),
            )
            .into_element()
    };
    view()
        .app_class(tw("relative flex min-h-0 flex-1 flex-col"))
        .child(
            text(status)
                .app_class(tw("flex min-h-10 shrink-0 flex-wrap items-center gap-x-1 border-b border-border/60 px-3 py-2 text-xs text-muted-foreground tabular-nums")),
        )
        .child((columns >= TABLE_LIMITS.max_columns).then(|| {
            file_surface_notice(t("Showing the first 1,024 columns. Source and Save retain every column."))
        }))
        .child(body)
}

/// VirtualTablePreview's `columnLabel`: A, B, … Z, AA, …
fn column_label(column: usize) -> String {
    let mut label = String::new();
    let mut n = column + 1;
    while n > 0 {
        let rem = (n - 1) % 26;
        label.insert(0, char::from(b'A' + rem as u8));
        n = (n - 1) / 26;
    }
    label
}

/// fileSurfaceChrome.tsx FileSurfaceNotice.
fn file_surface_notice(message: String) -> impl IntoElement {
    view()
        .app_class(tw("shrink-0 border-b border-warning/20 bg-warning-surface px-3 py-1.5 text-2xs text-warning-foreground"))
        .child(text(message))
}

/// Port of NotebookPreview, from the notebook's text as the server read it.
pub(super) fn asset_preview(
    asset: PreviewAsset,
    limit: usize,
    render: impl Fn(Result<Rc<Vec<u8>>, AssetReadError>) -> AnyElement + 'static,
) -> impl IntoElement {
    let result = signal(None::<Result<Rc<Vec<u8>>, AssetReadError>>);
    let read =
        asset
            .viewer
            .0
            .asset_bytes(&asset.environment, asset.resource, limit, asset.revision);
    crate::runtime::tasks::watch(async move {
        result.try_set(Some(read.await.map(Rc::new)));
    });
    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col overflow-hidden"))
        .child(dynamic(move || {
            let result = result.get();
            Some(untracked(|| match result {
                Some(result) => render(result),
                None => file_surface_loading().into_element(),
            }))
        }))
}

fn asset_failure(message: String) -> impl IntoElement {
    view()
        .app_class(tw("flex min-h-0 flex-1 items-center justify-center px-6 text-center text-xs leading-relaxed text-destructive"))
        .child(text(message))
}

pub(super) fn notebook_asset_preview(asset: PreviewAsset) -> impl IntoElement {
    asset_preview(asset, NOTEBOOK_MAX_BYTES, |result| match result {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(source) => notebook_preview(
                source.strip_prefix('\u{feff}').unwrap_or(source),
                bytes.len() as u64,
            )
            .into_element(),
            Err(_) => asset_failure(t(
                "Preview supports version 4 notebooks with up to 500 cells.",
            ))
            .into_element(),
        },
        Err(AssetReadError::TooLarge) => {
            asset_failure(t("Notebook preview limited to files under 8 MB.")).into_element()
        }
        Err(AssetReadError::Unavailable) => {
            asset_failure(t("The file could not be loaded. Close it and try again.")).into_element()
        }
    })
}

/// NotebookPreview: notebook input is decoded and bounded by t3-client.
pub fn notebook_preview(source: &str, size_bytes: u64) -> impl IntoElement {
    match open_notebook(source) {
        Err(message) => view()
            .app_class(tw("flex min-h-0 flex-1 items-center justify-center px-6 text-center text-xs leading-relaxed text-destructive"))
            .child(text(crate::i18n::Text::client_key(message).resolve()))
            .into_element(),
        Ok(notebook) => notebook_view(notebook, size_bytes).into_element(),
    }
}

fn notebook_view(notebook: Notebook, size_bytes: u64) -> impl IntoElement {
    const CELLS_PER_PAGE: usize = 20;
    let visible = signal(CELLS_PER_PAGE);
    let total = notebook.cells.len();
    let status = format!(
        "{} · {} · {}",
        count_label(total, "{count} cell", "{count} cells"),
        notebook.language,
        if size_bytes == 0 {
            t("0 bytes")
        } else {
            tr(
                "{size} bytes",
                &[(
                    "size",
                    Value::Text(crate::i18n::formats().decimal(size_bytes as f64)),
                )],
            )
        }
    );
    let language = notebook_cell_language(&notebook.language);
    let budget = Cell::new(notebook::MAX_NOTEBOOK_PIXELS);
    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .child(text(status).app_class(tw(
            "border-b border-border/60 px-3 py-2 text-xs text-muted-foreground tabular-nums",
        )))
        .child(file_surface_notice(t(
            "Saved outputs only. Code is never executed.",
        )))
        .child(
            scroll().app_class(tw("min-h-0 flex-1")).child(
                view()
                    .app_class(tw("mx-auto flex w-full max-w-4xl flex-col gap-5 px-5 py-6"))
                    .child(view().app_class(tw("flex flex-col gap-5")).child(each(
                        move || (0..visible.get().min(total)).collect::<Vec<_>>(),
                        |index| *index,
                        move |index, _| {
                            let index = index.get_untracked();
                            notebook_cell(
                                index,
                                &notebook.cells[index],
                                &notebook.language,
                                &language,
                                &budget,
                            )
                        },
                    )))
                    .child((total == 0).then(|| {
                        text(t("This notebook is empty."))
                            .app_class(tw("text-sm text-muted-foreground"))
                    }))
                    .child(dynamic(move || {
                        let remaining = total.saturating_sub(visible.get());
                        (remaining > 0).then(|| {
                            untracked(|| {
                                view()
                                    .app_class(tw("flex"))
                                    .child(button(
                                        ButtonContent::Label(tr(
                                            "Show more cells ({count} remaining)",
                                            &[(
                                                "count",
                                                Value::Text(
                                                    crate::i18n::formats()
                                                        .decimal(remaining as f64),
                                                ),
                                            )],
                                        )),
                                        ButtonVariant::Outline,
                                        ButtonSize::Sm,
                                        || false,
                                        move || visible.update(|shown| *shown += CELLS_PER_PAGE),
                                    ))
                                    .into_element()
                            })
                        })
                    })),
            ),
        )
}

fn notebook_cell(
    index: usize,
    cell: &NotebookCell,
    notebook_language: &str,
    highlight_language: &str,
    budget: &Cell<u64>,
) -> AnyElement {
    let kind_label = match cell.kind {
        CellKind::Code => notebook_language.to_owned(),
        CellKind::Markdown => "markdown".to_owned(),
        CellKind::Raw => "raw".to_owned(),
    };
    let body = match cell.kind {
        CellKind::Markdown => Some(notebook::notebook_markdown(&cell.source)),
        _ if cell.source.trim().is_empty() => None,
        kind => {
            let language = if kind == CellKind::Code {
                highlight_language.to_owned()
            } else {
                "txt".to_owned()
            };
            Some(highlighted_source(&language, &cell.source).into_element())
        }
    };
    view()
        .label(tr(
            "Cell {index}",
            &[("index", Value::Text((index + 1).to_string()))],
        ))
        .app_class(tw(
            "min-w-0 overflow-hidden rounded-md border border-border/60",
        ))
        .child(text(format!("{kind_label} · {}", index + 1)).app_class(tw(
            "border-b border-border/60 bg-muted/30 px-3 py-1.5 text-xs text-muted-foreground",
        )))
        .child(body)
        .children(
            cell.outputs
                .iter()
                .map(|output| notebook::notebook_output(index, output, budget)),
        )
        .into_element()
}

/// NotebookSource: the cell through the shared tokenizer queue, unvirtualised (cells are
/// bounded to 256,000 characters by the reader).
fn highlighted_source(language: &str, source: &str) -> impl IntoElement {
    let state = CodeHighlightState::new();
    let chunks = file_code_chunks(language, source);
    for chunk in &chunks {
        state.upsert(chunk, false);
    }
    let wrapped = computed(|| false);
    view()
        .app_class(tw("min-w-0 bg-(--code-background)"))
        .children(chunks.into_iter().map(move |chunk| {
            let chunk = computed(move || Some(chunk.clone()));
            let highlighted = highlighted_chunk(chunk, computed(|| false), state.clone());
            code_chunk_row(chunk, highlighted, wrapped).into_element()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_labels_count_like_a_spreadsheet() {
        assert_eq!(column_label(0), "A");
        assert_eq!(column_label(25), "Z");
        assert_eq!(column_label(26), "AA");
        assert_eq!(column_label(701), "ZZ");
    }
}
