//! Port of apps/web/src/components/ui/qr-code.tsx QRCodeSvg
//!
//! The encoder is `qrcodegen`, Project Nayuki's Rust edition of the library the web vendors as
//! `packages/shared/src/qrCode.ts` (same segment choice, ECC boost and mask selection), so both
//! apps draw the same modules for the same value. The drawing is the web's own SVG markup, handed
//! to NSH's `svg()` (native drawing on desktop and mobile, an image on the web).
//!
//! Departures, each forced by NSH:
//! - `className` is not taken: a primitive is not restyled, and every web caller passes none.
//!   Layout goes on the parent, as the callers' white `p-1.5` frames already do.
//! - `<title>` becomes the accessibility label of an image; NSH draws SVG markup as a picture and
//!   has no hover title inside it.
//! - The web throws when the value exceeds version 40 and the subtree fails. Here the box keeps
//!   its size and draws nothing, so a caller's layout holds and no unreadable code is shown.

use crate::i18n::Text;
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;
use qrcodegen::{QrCode, QrCodeEcc};

/// The TSX `level` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    #[default]
    L,
    M,
    Q,
    H,
}

impl Level {
    fn ecc(self) -> QrCodeEcc {
        match self {
            Self::L => QrCodeEcc::Low,
            Self::M => QrCodeEcc::Medium,
            Self::Q => QrCodeEcc::Quartile,
            Self::H => QrCodeEcc::High,
        }
    }
}

/// The TSX props other than `value`, with the same defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct QrCodeOptions {
    pub size: f32,
    pub level: Level,
    pub margin_size: u32,
    pub title: Option<Text>,
    pub foreground_color: SharedString,
    pub background_color: SharedString,
}

impl Default for QrCodeOptions {
    fn default() -> Self {
        Self {
            size: 128.0,
            level: Level::L,
            margin_size: 0,
            title: None,
            foreground_color: SharedString::from("#000"),
            background_color: SharedString::from("#fff"),
        }
    }
}

/// `QRCodeSvg`. The value is re-encoded only when it changes (the TSX `memo`).
pub fn qr_code_svg(
    value: impl IntoReactive<SharedString>,
    options: QrCodeOptions,
) -> impl IntoElement {
    let value = value.into_reactive();
    let QrCodeOptions {
        size,
        level,
        margin_size,
        title,
        foreground_color,
        background_color,
    } = options;
    let value = computed(move || value.get());
    let markup = computed(move || {
        value.with(|value| {
            qr_svg_markup(
                value,
                level,
                margin_size,
                &foreground_color,
                &background_color,
            )
            .map(SharedString::from)
        })
    });
    let drawing = svg(move || {
        markup.with(|markup| match markup {
            Some(markup) => ImageSrc::svg_markup(markup),
            None => ImageSrc::svg_markup(EMPTY_SVG),
        })
    })
    .w(px(size))
    .h(px(size))
    // An <svg width height> keeps its size as a flex child (automatic minimum size).
    .flex_shrink_0();
    match title {
        Some(title) => drawing
            .role(AccessibilityRole::Image)
            .label(title.resolve())
            .into_element(),
        None => drawing.accessible(false).into_element(),
    }
}

const EMPTY_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"/>"#;

/// The markup the TSX renders, without the `<title>` (see the module notes); `None` when the
/// value is too long for any QR version.
pub fn qr_svg_markup(
    value: &str,
    level: Level,
    margin_size: u32,
    foreground_color: &str,
    background_color: &str,
) -> Option<String> {
    let code = match QrCode::encode_text(value, level.ecc()) {
        Ok(code) => code,
        Err(error) => {
            log::warn!("QR code not drawn: {error}");
            return None;
        }
    };
    let margin = i64::from(margin_size);
    let view_box_size = i64::from(code.size()) + margin * 2;
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view_box_size} {view_box_size}" shape-rendering="crispEdges"><rect width="{view_box_size}" height="{view_box_size}" fill="{background_color}"/><path d="{}" fill="{foreground_color}"/></svg>"#,
        build_qr_path_data(&code, margin),
    ))
}

/// `buildQrPathData`: one `h`/`v` rectangle per horizontal run of dark modules.
fn build_qr_path_data(code: &QrCode, margin: i64) -> String {
    use std::fmt::Write;
    let size = code.size();
    let mut commands = String::new();
    for y in 0..size {
        let mut run_start: Option<i32> = None;
        for x in 0..=size {
            let is_dark = x < size && code.get_module(x, y);
            if is_dark {
                run_start.get_or_insert(x);
                continue;
            }
            let Some(start) = run_start.take() else {
                continue;
            };
            let left = i64::from(start) + margin;
            let top = i64::from(y) + margin;
            let _ = write!(commands, "M{left} {top}h{}v1H{left}z", x - start);
        }
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the path back into a module grid, so the test checks what is drawn, not how.
    fn drawn_modules(path: &str, view_box: usize) -> Vec<Vec<bool>> {
        let mut grid = vec![vec![false; view_box]; view_box];
        for command in path.split('z').filter(|part| !part.is_empty()) {
            let (head, rest) = command.trim_start_matches('M').split_once('h').expect("h");
            let (left, top) = head.split_once(' ').expect("x y");
            let (width, tail) = rest.split_once('v').expect("v");
            assert_eq!(tail, format!("1H{left}"));
            let (left, top, width): (usize, usize, usize) = (
                left.parse().unwrap_or(0),
                top.parse().unwrap_or(0),
                width.parse().unwrap_or(0),
            );
            for cell in &mut grid[top][left..left + width] {
                assert!(!*cell, "runs never overlap");
                *cell = true;
            }
        }
        grid
    }

    fn path_of(markup: &str) -> &str {
        let start = markup.find(r#"<path d=""#).expect("path") + 9;
        let end = markup[start..].find('"').expect("end") + start;
        &markup[start..end]
    }

    #[test]
    fn the_path_draws_exactly_the_dark_modules_inside_the_margin() {
        let value = "https://app.t3.codes/pair#token=abc";
        for (level, margin) in [(Level::L, 0), (Level::M, 2), (Level::H, 1)] {
            let code = QrCode::encode_text(value, level.ecc()).expect("encodes");
            let markup = qr_svg_markup(value, level, margin, "#000", "#fff").expect("markup");
            let view_box = code.size() as usize + 2 * margin as usize;
            assert!(markup.contains(&format!(r#"viewBox="0 0 {view_box} {view_box}""#)));
            let grid = drawn_modules(path_of(&markup), view_box);
            for (y, row) in grid.iter().enumerate() {
                for (x, drawn) in row.iter().enumerate() {
                    let module = x
                        .checked_sub(margin as usize)
                        .zip(y.checked_sub(margin as usize))
                        .is_some_and(|(mx, my)| code.get_module(mx as i32, my as i32));
                    assert_eq!(*drawn, module, "module ({x}, {y}) at {level:?}");
                }
            }
        }
    }

    #[test]
    fn levels_choose_the_web_error_correction() {
        let value = "T3 Code";
        let high = QrCode::encode_text(value, Level::H.ecc()).expect("encodes");
        assert_eq!(high.error_correction_level(), QrCodeEcc::High);
        // encodeText boosts the level while the version allows, as qrCode.ts does.
        let low = QrCode::encode_text(value, Level::L.ecc()).expect("encodes");
        assert_eq!(low.error_correction_level(), QrCodeEcc::High);
    }

    #[test]
    fn colours_fill_the_background_and_the_modules() {
        let markup = qr_svg_markup("x", Level::L, 0, "#123456", "#abcdef").expect("markup");
        assert!(markup.contains(r##"<rect width="21" height="21" fill="#abcdef"/>"##));
        assert!(markup.contains(r##"fill="#123456"/></svg>"##));
    }

    #[test]
    fn a_value_too_long_for_any_version_draws_nothing() {
        assert_eq!(
            qr_svg_markup(&"x".repeat(8000), Level::H, 0, "#000", "#fff"),
            None
        );
    }
}
