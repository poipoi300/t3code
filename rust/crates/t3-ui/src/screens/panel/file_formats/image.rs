//! Port of PreviewImageContent.tsx using NSH's image and scroll primitives.

use nsh_core::{
    platform::clipboard::{image_mime, image_size},
    prelude::*,
};
use t3_client::workspace::project_files::assets::{AssetReadError, preview_data_url};

use super::{asset_failure, asset_preview};
use crate::{
    i18n::t,
    screens::panel::file_viewer::{PreviewAsset, file_surface_button, file_surface_toggle},
    theme::tw,
    ui::icon::Icon,
};

const IMAGE_MAX_BYTES: usize = 12 * 1024 * 1024;
const MAX_PIXELS: u64 = 32_000_000;
const MAX_SIDE: u32 = 16_384;
const PADDING: f32 = 32.0;
const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 8.0;

/// Validate the actual raster and bound the dimensions before handing bytes to the platform.
pub(super) fn raster_info(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    let mime = image_mime(bytes)?;
    let static_raster = match mime {
        "image/png" => static_png(bytes),
        "image/jpeg" | "image/bmp" => true,
        "image/webp" => simple_webp(bytes),
        // The size of a GIF frame can exceed the logical screen from its header.
        _ => false,
    };
    let (width, height) = image_size(bytes)?;
    (static_raster && bounded_size(width, height)).then_some((mime, width, height))
}

fn bounded_size(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_SIDE
        && height <= MAX_SIDE
        && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

// Simple WebP holds exactly one bounded bitstream and no extended canvas or metadata chunks.
fn simple_webp(bytes: &[u8]) -> bool {
    let Some([a, b, c, d]) = bytes.get(4..8) else {
        return false;
    };
    let riff_len = u32::from_le_bytes([*a, *b, *c, *d]) as usize;
    let Some([b'V', b'P', b'8', b' ' | b'L', a, b, c, d]) = bytes.get(12..20) else {
        return false;
    };
    let chunk_len = u32::from_le_bytes([*a, *b, *c, *d]) as usize;
    riff_len == bytes.len() - 8 && chunk_len.checked_add(chunk_len % 2) == Some(bytes.len() - 20)
}

fn static_png(mut chunks: &[u8]) -> bool {
    let Some(after_signature) = chunks.get(8..) else {
        return false;
    };
    chunks = after_signature;
    while let Some([a, b, c, d, kind @ ..]) = chunks.get(..8) {
        match kind {
            b"acTL" => return false,
            b"IDAT" => return true,
            _ => {}
        }
        let len = u32::from_be_bytes([*a, *b, *c, *d]) as usize;
        let Some(next) = len.checked_add(12).and_then(|end| chunks.get(end..)) else {
            return false;
        };
        chunks = next;
    }
    false
}

pub(crate) fn image_preview(asset: PreviewAsset, name: String) -> impl IntoElement {
    asset_preview(asset, IMAGE_MAX_BYTES, move |result| match result {
        Err(AssetReadError::TooLarge) => {
            asset_failure(t("Preview limited to files under 12 MB.")).into_element()
        }
        Err(AssetReadError::Unavailable) => {
            asset_failure(t("Unable to load workspace image.")).into_element()
        }
        Ok(bytes) => match raster_info(&bytes) {
            Some((mime, width, height)) => {
                preview_image_content(preview_data_url(mime, &bytes), &name, (width, height))
                    .into_element()
            }
            None => asset_failure(t("Unable to load image.")).into_element(),
        },
    })
}

fn preview_image_content(
    source: String,
    name: &str,
    (width, height): (u32, u32),
) -> impl IntoElement {
    let name = SharedString::from(name.to_owned());
    let natural = signal((width, height));
    let viewport = signal(None::<(f32, f32)>);
    let zoom = signal(None::<f32>);
    let failed = signal(false);
    let fit = computed(move || {
        viewport.get().map_or(1.0, |(vw, vh)| {
            let (width, height) = natural.get();
            1.0_f32
                .min((vw - PADDING).max(1.0) / width as f32)
                .min((vh - PADDING).max(1.0) / height as f32)
        })
    });
    let scale = computed(move || zoom.get().unwrap_or_else(|| fit.get()));
    let change = move |factor: f32| {
        zoom.set(Some(
            (scale.get_untracked() * factor).clamp(MIN_SCALE, MAX_SCALE),
        ))
    };
    let content_w = move || {
        px(viewport
            .get()
            .map_or(0.0, |(w, _)| w)
            .max(natural.get().0 as f32 * scale.get() + PADDING))
    };
    let content_h = move || {
        px(viewport
            .get()
            .map_or(0.0, |(_, h)| h)
            .max(natural.get().1 as f32 * scale.get() + PADDING))
    };
    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .child(when(move || failed.get(), || asset_failure(t("Unable to load image."))))
        .child(
            view()
                .app_class(move || Some(tw(if failed.get() { "hidden" } else { "flex min-h-0 flex-1 flex-col" })))
                .child(
                    view()
                        .app_class(tw("flex min-h-10 shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border/60 px-3 py-1 text-xs text-muted-foreground"))
                        .child(text(move || {
                            let (width, height) = natural.get();
                            let formats = crate::i18n::formats();
                            format!("{} × {}", formats.decimal(f64::from(width)), formats.decimal(f64::from(height)))
                        }).app_class(tw("tabular-nums")))
                        .child(
                            view()
                                .app_class(tw("flex items-center gap-1"))
                                .child(file_surface_button(t("Zoom out"), Icon::Minus, move || scale.get() <= MIN_SCALE, move || change(1.0 / 1.25)))
                                .child(text(move || format!("{}%", (scale.get() * 100.0).round())).app_class(tw("min-w-10 text-center tabular-nums")))
                                .child(file_surface_button(t("Zoom in"), Icon::Plus, move || scale.get() >= MAX_SCALE, move || change(1.25)))
                                .child(file_surface_toggle(computed(move || zoom.get().is_none()), |_| t("Fit image"), |_| Icon::Scan, move || zoom.set(None)))
                                .child(file_surface_toggle(computed(move || zoom.get() == Some(1.0)), |_| t("Actual image size"), |_| Icon::Maximize, move || zoom.set(Some(1.0)))),
                        ),
                )
                .child(
                    // NSH scrolls along one axis per native view. Keep the vertical viewport at
                    // the available height and let its child extend in both directions.
                    scroll()
                        .horizontal()
                        .app_class(tw("min-h-0 flex-1"))
                        .on_layout(move |bounds: &Bounds<Points>| viewport.set(Some((bounds.size.width.0, bounds.size.height.0))))
                        .child(
                            scroll()
                                .w(content_w)
                                .h(move || px(viewport.get().map_or(0.0, |(_, h)| h)))
                                .child(
                                    view()
                                        .app_class(tw("flex shrink-0 items-center justify-center p-4"))
                                        .h(content_h)
                                        .child(
                                            image(source)
                                                .role(AccessibilityRole::Image)
                                                .label(name)
                                                .app_class(tw("shrink-0"))
                                                .w(move || px(natural.get().0 as f32 * scale.get()))
                                                .h(move || px(natural.get().1 as f32 * scale.get()))
                                                .on_load(move |size: &Size<DevicePixels>| {
                                                    let width = size.width.0.unsigned_abs();
                                                    let height = size.height.0.unsigned_abs();
                                                    if bounded_size(width, height) {
                                                        natural.set((width, height));
                                                    } else {
                                                        failed.set(true);
                                                    }
                                                })
                                                .on_error(move |_: &str| failed.set(true)),
                                        ),
                                ),
                        ),
                ),
        )
}
