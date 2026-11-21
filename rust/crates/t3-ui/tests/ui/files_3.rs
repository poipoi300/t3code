//! File formats through the Files panel and attachment viewer.

use std::rc::Rc;

use crate::files_1::FakeFiles;
use crate::files_2::{
    FakeViewer, Setup, contents, start_root, surface_fixture, surface_fixture_with_attachment,
};
use crate::support::{DESKTOP, PHONE};
use nsh_core::prelude::*;
use nsh_engine::Root;
use t3_client::workspace::project_files::{read::FileReadView, tree::Problem};
use t3_ui::screens::panel::files_tree::FilesRuntime;

fn setup(path: &str, text: &str) -> Setup {
    let viewer = FakeViewer::default();
    viewer
        .files
        .borrow_mut()
        .insert(path.into(), contents(path, text));
    Setup {
        files: FilesRuntime(Rc::new(FakeFiles::default())),
        viewer,
        open: Some(path.into()),
    }
}

#[component]
fn CsvCase() -> impl IntoElement {
    surface_fixture(setup(
        "data.csv",
        "name,qty
\"Widget, large\",3
Bolt,10
",
    ))
}
#[component]
fn NotebookCase() -> impl IntoElement {
    let notebook = serde_json::json!({
        "nbformat": 4,
        "metadata": {"language_info": {"name": "python"}},
        "cells": [
            {"cell_type": "markdown", "source": "# Results\n\n[Notebook link](https://example.test/link)\n\n![Notebook image](https://example.test/image.png)\n\n<script>notebook-script</script>"},
            {"cell_type": "code", "source": "print(42)", "outputs": [{"output_type": "stream", "text": "42
"},
                {"data": {"image/png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg=="}},
                {"data": {"image/svg+xml": "<svg onload='notebook-svg'/>", "text/plain": "SVG fallback"}},
                {"data": {"text/html": "<script>notebook-output-script</script>"}}
            ]}
        ]
    })
    .to_string();
    surface_fixture(setup("run.ipynb", &notebook))
}
#[component]
fn OldNotebookCase() -> impl IntoElement {
    surface_fixture(setup("old.ipynb", "{\"nbformat\":3,\"cells\":[]}"))
}

#[test]
fn a_csv_file_renders_as_a_table_and_toggles_to_source() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<CsvCase>());
        app.wait_for("text:2 rows · 2 columns")
            .expect("table status");
        assert!(
            app.screen().shows("Widget, large"),
            "a quoted cell keeps its delimiter"
        );
        assert!(app.screen().shows("qty"), "the first row is the header");
        app.press("role:button[Show source]").expect("source");
        app.wait_for("text:/name,qty/").expect("raw text");
        app.press("role:button[Show table]").expect("table again");
        app.wait_for("text:2 rows · 2 columns").expect("table back");
    }
}

#[test]
fn a_notebook_shows_saved_outputs_and_never_runs_code() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<NotebookCase>());
        app.wait_for("text:Saved outputs only. Code is never executed.")
            .expect("notice");
        assert!(app.screen().shows("Results"));
        app.wait_for("text:/print\\(42\\)/")
            .expect("code cell source");
        assert!(app.screen().shows("42"));
        assert!(app.screen().shows("python · 2"));
        app.wait_for("text:Notebook link")
            .expect("plain link label");
        app.wait_for("text:Notebook image")
            .expect("plain image alt text");
        app.wait_for("role:image[Cell 2 output]")
            .expect("saved raster image");
        assert!(app.screen().shows("SVG fallback"));
        assert_eq!(app.screen().count("role:link").expect("links"), 0);
        assert_eq!(
            app.screen()
                .count("role:image[Notebook image]")
                .expect("authored images"),
            0
        );
        assert!(!app.screen().shows("notebook-script"));
        assert!(!app.screen().shows("notebook-output-script"));
    }
}

#[test]
fn an_unsupported_notebook_says_why() {
    let mut app = start_root(DESKTOP, Root::of::<OldNotebookCase>());
    app.wait_for("text:Preview supports version 4 notebooks with up to 500 cells.")
        .expect("reason");
}

fn asset_case(path: &str, bytes: Vec<u8>) -> Setup {
    let viewer = FakeViewer::default();
    viewer
        .assets
        .borrow_mut()
        .insert(format!("/repo/{path}"), bytes);
    let read_error = serde_json::from_value(serde_json::json!({
        "_tag": "ProjectReadFileError",
        "message": "Binary file",
        "failure": "binary_file"
    }))
    .expect("contract error");
    viewer.files.borrow_mut().insert(
        path.into(),
        FileReadView {
            error: Some(Problem::server("Binary file".into())),
            read_error: Some(Rc::new(read_error)),
            ..FileReadView::default()
        },
    );
    Setup {
        files: FilesRuntime(Rc::new(FakeFiles::default())),
        viewer,
        open: Some(path.into()),
    }
}

#[component]
fn ImageCase() -> impl IntoElement {
    // A 512 × 1 PNG fits at actual size on desktop and must shrink on a phone.
    surface_fixture(asset_case(
        "photo.png",
        vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 2, 0, 0, 0, 0, 1,
            8, 0, 0, 0, 0, 141, 208, 73, 170, 0, 0, 0, 14, 73, 68, 65, 84, 120, 156, 99, 96, 24, 5,
            35, 26, 0, 0, 2, 1, 0, 1, 193, 164, 12, 8, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ],
    ))
}

#[component]
fn SvgCase() -> impl IntoElement {
    surface_fixture(asset_case(
        "photo.svg",
        b"<svg onload='alert(1)'/>".to_vec(),
    ))
}

#[component]
fn BinaryCase() -> impl IntoElement {
    let mut bytes = vec![0; 257];
    bytes[..4].copy_from_slice(b"\x00ABC");
    bytes[256] = 0xff;
    surface_fixture(asset_case("data.zip", bytes))
}

#[component]
fn LargeBinaryCase() -> impl IntoElement {
    surface_fixture(asset_case("large.zip", vec![0; 1024 * 1024 + 1]))
}

#[component]
fn UnknownWorkspaceBinaryCase() -> impl IntoElement {
    surface_fixture(asset_case("/repo/data.dat", b"\x00ABC".to_vec()))
}

#[test]
fn a_raster_preview_fits_and_zooms() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<ImageCase>());
        app.wait_for("text:512 × 1").expect("intrinsic raster size");
        app.press("role:button[Hide file explorer]")
            .expect("full preview width");
        let fitted = if viewport == PHONE {
            "text:/^7[0-9]%$/"
        } else {
            "text:100%"
        };
        app.wait_for(fitted).expect("fitted scale");
        app.press("role:button[Actual image size]")
            .expect("actual size");
        app.wait_for("text:100%").expect("actual scale");
        app.press("role:button[Zoom in]").expect("zoom in");
        app.wait_for("text:125%").expect("zoomed scale");
        app.press("role:button[Fit image]").expect("fit");
        app.wait_for(fitted).expect("fitted scale again");
    }
}

#[test]
fn a_workspace_svg_never_mounts_as_an_image() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<SvgCase>());
        app.wait_for("text:Unable to load image.")
            .expect("unsafe image refused");
        assert_eq!(
            app.screen().count("role:image[photo.svg]").expect("images"),
            0
        );
    }
}

#[test]
fn a_binary_read_failure_opens_a_paged_hex_preview() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<BinaryCase>());
        app.wait_for(r"text:/00 41 42 43[^\n]*\.ABC/")
            .expect("hex bytes and ASCII");
        app.press("role:button[Next page]").expect("next page");
        app.wait_for("text:/00000100.*FF/")
            .expect("last byte on its own page");
        app.press("role:button[Previous page]")
            .expect("previous page");
        app.wait_for("text:/00 41 42 43/")
            .expect("first page again");
    }
}

#[test]
fn a_binary_file_above_the_preview_cap_is_refused() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<LargeBinaryCase>());
        app.wait_for("text:Preview limited to files under 1 MB.")
            .expect("cap notice");
    }
}

#[test]
fn an_unknown_workspace_extension_keeps_the_scoped_read_error() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<UnknownWorkspaceBinaryCase>());
        app.wait_for("text:The file is binary and cannot be displayed as text.")
            .expect("no workspace-scoped asset grant for an unknown extension");
    }
}

#[component]
fn BinaryAttachmentCase() -> impl IntoElement {
    let viewer = FakeViewer::default();
    viewer
        .assets
        .borrow_mut()
        .insert("stored-attachment".into(), b"\x00ABC".to_vec());
    surface_fixture_with_attachment(
        Setup {
            files: FilesRuntime(Rc::new(FakeFiles::default())),
            viewer,
            open: None,
        },
        Some(
            serde_json::from_value(serde_json::json!({
                "type": "file",
                "id": "stored-attachment",
                "name": "sample",
                "mimeType": "Application/ZIP; charset=binary",
                "sizeBytes": 4
            }))
            .expect("attachment contract"),
        ),
    )
}

#[test]
fn an_attachment_tab_previews_the_attachment_by_id() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<BinaryAttachmentCase>());
        app.wait_for("text:Attachment").expect("attachment chrome");
        app.wait_for("text:/00 41 42 43/")
            .expect("signed attachment bytes");
    }
}

#[component]
fn TruncatedNotebookCase() -> impl IntoElement {
    let notebook =
        br##"{"nbformat":4,"cells":[{"cell_type":"markdown","source":"# Loaded in full"}]}"##;
    let path = "large-notebook.ipynb";
    let bytes = [b"\xef\xbb\xbf", notebook.as_slice()].concat();
    let setup = asset_case(path, bytes);
    let mut partial = contents(path, "{\"nbformat\":4,");
    Rc::make_mut(partial.data.as_mut().expect("file data")).truncated = true;
    setup.viewer.files.borrow_mut().insert(path.into(), partial);
    surface_fixture(setup)
}

#[test]
fn a_truncated_workspace_notebook_uses_the_bounded_asset_read() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<TruncatedNotebookCase>());
        app.wait_for("text:Loaded in full")
            .expect("full notebook from the signed asset");
    }
}
