//! FILES-3: the delimited-table and notebook previews, mounted as a Files surface.

use std::rc::Rc;

use crate::files_1::FakeFiles;
use crate::files_2::{FakeViewer, Setup, contents, start_root, surface_fixture};
use nsh_core::prelude::*;
use nsh_engine::Root;
use crate::support::{DESKTOP, PHONE};
use t3_ui::screens::panel::files_tree::FilesRuntime;

fn setup(path: &str, text: &str) -> Setup {
    let viewer = FakeViewer::default();
    viewer.files.borrow_mut().insert(path.into(), contents(path, text));
    Setup { files: FilesRuntime(Rc::new(FakeFiles::default())), viewer, open: Some(path.into()) }
}

#[component]
fn CsvCase() -> impl IntoElement {
    surface_fixture(setup("data.csv", "name,qty
\"Widget, large\",3
Bolt,10
"))
}
#[component]
fn NotebookCase() -> impl IntoElement {
    let notebook = serde_json::json!({
        "nbformat": 4,
        "metadata": {"language_info": {"name": "python"}},
        "cells": [
            {"cell_type": "markdown", "source": "# Results"},
            {"cell_type": "code", "source": "print(42)", "outputs": [{"output_type": "stream", "text": "42
"}]}
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
        app.wait_for("text:2 rows · 2 columns").expect("table status");
        assert!(app.screen().shows("Widget, large"), "a quoted cell keeps its delimiter");
        assert!(app.screen().shows("qty"), "the first row is the header");
        app.press("role:button[Show source]").expect("source");
        app.wait_for("text:/name,qty/").expect("raw text");
        app.press("role:button[Show table]").expect("table again");
        app.wait_for("text:2 rows · 2 columns").expect("table back");
    }
}

#[test]
fn a_notebook_shows_saved_outputs_and_never_runs_code() {
    let mut app = start_root(DESKTOP, Root::of::<NotebookCase>());
    app.wait_for("text:Saved outputs only. Code is never executed.").expect("notice");
    assert!(app.screen().shows("Results"));
    app.wait_for("text:/print\\(42\\)/").expect("code cell source");
    assert!(app.screen().shows("42"));
    assert!(app.screen().shows("python · 2"));
}

#[test]
fn an_unsupported_notebook_says_why() {
    let mut app = start_root(DESKTOP, Root::of::<OldNotebookCase>());
    app.wait_for("text:Preview supports version 4 notebooks with up to 500 cells.").expect("reason");
}
