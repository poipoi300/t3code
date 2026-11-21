//! Read-only hex preview for the Files surface and attachment viewer.

use std::{fmt::Write as _, rc::Rc};

use nsh_core::prelude::*;
use t3_client::workspace::project_files::assets::AssetReadError;

use super::{asset_failure, asset_preview};
use crate::{
    i18n::{Value, t, tr},
    screens::panel::file_viewer::{PreviewAsset, file_surface_button},
    theme::tw,
    ui::icon::Icon,
};

const MAX_BYTES: usize = 1024 * 1024;
const PAGE_BYTES: usize = 256;
const ROW_BYTES: usize = 8;

pub(crate) fn binary_preview(asset: PreviewAsset) -> impl IntoElement {
    asset_preview(asset, MAX_BYTES, |result| match result {
        Ok(bytes) if bytes.is_empty() => asset_failure(t("The file is empty.")).into_element(),
        Ok(bytes) => hex_view(bytes).into_element(),
        Err(AssetReadError::TooLarge) => {
            asset_failure(t("Preview limited to files under 1 MB.")).into_element()
        }
        Err(AssetReadError::Unavailable) => {
            asset_failure(t("The file could not be loaded. Close it and try again.")).into_element()
        }
    })
}

fn hex_view(bytes: Rc<Vec<u8>>) -> impl IntoElement {
    let pages = bytes.len().div_ceil(PAGE_BYTES);
    let page = signal(0_usize);
    let size = t3_client::services::assets::format_attachment_size(bytes.len() as u64);
    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .child(
            view()
                .app_class(tw("flex min-h-10 shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border/60 px-3 py-1 text-xs text-muted-foreground"))
                .child(text(size))
                .child(
                    view()
                        .app_class(tw("flex items-center gap-1"))
                        .child(file_surface_button(t("Previous page"), Icon::ChevronLeft, move || page.get() == 0, move || page.update(|p| *p = p.saturating_sub(1))))
                        .child(text(move || tr("Page {page} of {pages}", &[
                            ("page", Value::Text((page.get() + 1).to_string())),
                            ("pages", Value::Text(pages.to_string())),
                        ])))
                        .child(file_surface_button(t("Next page"), Icon::ChevronRight, move || page.get() + 1 == pages, move || page.update(|p| *p = (*p + 1).min(pages - 1)))),
                ),
        )
        .child(
            scroll()
                .app_class(tw("min-h-0 flex-1"))
                .child(text(move || {
                    let start = page.get() * PAGE_BYTES;
                    let end = (start + PAGE_BYTES).min(bytes.len());
                    format_hex(&bytes[start..end], start)
                }).app_class(tw("p-3 font-mono text-xs"))),
        )
}

fn format_hex(bytes: &[u8], start: usize) -> String {
    let mut output = String::new();
    for (row, chunk) in bytes.chunks(ROW_BYTES).enumerate() {
        if row > 0 {
            output.push('\n');
        }
        let _ = write!(output, "{:08X}  ", start + row * ROW_BYTES);
        for index in 0..ROW_BYTES {
            if let Some(byte) = chunk.get(index) {
                let _ = write!(output, "{byte:02X} ");
            } else {
                output.push_str("   ");
            }
        }
        output.push(' ');
        for &byte in chunk {
            output.push(if (b' '..=b'~').contains(&byte) {
                char::from(byte)
            } else {
                '.'
            });
        }
    }
    output
}
