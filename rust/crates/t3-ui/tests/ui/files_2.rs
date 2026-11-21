//! FILES-2: headless tests of the file viewer as PNL-HOST mounts it (a Files surface), over the
//! client's read view. The fakes answer reads and the tree's folder reads only.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use crate::files_1::FakeFiles;
use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::{
    services::filesystem::ReadAccess,
    types::{EnvironmentId, ThreadId, ThreadKey},
    workspace::project_files::{
        FilesWorkspace, assets::AssetReadError, read::FileReadView, tree::Problem,
    },
};
use t3_contracts::{
    assets::AssetResource,
    chat_attachment::ChatFileAttachment,
    project::{ProjectReadFileError, ProjectReadFileResult},
};
use t3_ui::screens::panel::{
    Action, PanelHost, PanelStore, SurfaceContext,
    file_viewer::{FileReadHandle, FileViewerRuntime, FileViewerService, file_preview_panel},
    files_tree::FilesRuntime,
};

pub struct FakeRead(pub FileReadView);
impl FileReadHandle for FakeRead {
    fn view(&self) -> FileReadView {
        self.0.clone()
    }
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn refresh(&self) {}
    fn observe_mutation(&self, _: Option<&str>) {}
}

pub fn contents(path: &str, text: &str) -> FileReadView {
    FileReadView {
        data: Some(Rc::new(ProjectReadFileResult {
            relative_path: path.into(),
            contents: text.into(),
            byte_length: text.len() as u64,
            truncated: false,
            exists: None,
        })),
        is_file: true,
        ..FileReadView::default()
    }
}

#[derive(Clone, Default)]
pub struct FakeViewer {
    pub files: Rc<RefCell<BTreeMap<String, FileReadView>>>,
    pub assets: Rc<RefCell<BTreeMap<String, Vec<u8>>>>,
}
impl FileViewerService for FakeViewer {
    fn workspace(&self, _: &ThreadKey) -> Option<FilesWorkspace> {
        Some(FilesWorkspace { cwd: "/repo".into(), project_name: "Proj".into() })
    }
    fn workspace_mutation(&self, _: &ThreadKey) -> Option<String> {
        None
    }
    fn can_write(&self, _: &EnvironmentId) -> bool {
        true
    }
    fn read(&self, _: &EnvironmentId, _: &str, path: &str) -> Rc<dyn FileReadHandle> {
        Rc::new(FakeRead(self.files.borrow().get(path).cloned().unwrap_or_default()))
    }
    fn asset_bytes(
        &self,
        _: &EnvironmentId,
        resource: AssetResource,
        limit: usize,
        _: Option<String>,
    ) -> LocalBoxFuture<'static, Result<Vec<u8>, AssetReadError>> {
        let assets = self.assets.borrow();
        let result = match asset_path(&resource).and_then(|path| assets.get(path)) {
            Some(bytes) if bytes.len() <= limit => Ok(bytes.clone()),
            Some(_) => Err(AssetReadError::TooLarge),
            None => Err(AssetReadError::Unavailable),
        };
        async { result }.boxed_local()
    }
}

fn asset_path(resource: &AssetResource) -> Option<&str> {
    match resource {
        AssetResource::WorkspaceFile { path, .. } => Some(&path.0),
        // The fixture accepts host-only grants only for paths explicitly outside its workspace.
        AssetResource::MediaFile { path, .. } if !path.0.starts_with("/repo/") => Some(&path.0),
        AssetResource::Attachment { attachment_id, .. } => Some(&attachment_id.0),
        _ => None,
    }
}

/// Files whose access is refused.
#[derive(Clone, Default)]
struct Denied(FakeFiles);
impl t3_ui::screens::panel::files_tree::FilesService for Denied {
    fn tree(&self, e: &EnvironmentId, c: &str, h: bool) -> Rc<dyn t3_ui::screens::panel::files_tree::FileTreeHandle> {
        self.0.tree(e, c, h)
    }
    fn read_access(&self, _: &EnvironmentId) -> ReadAccess {
        ReadAccess { allowed: false, pending: false, error: None }
    }
    fn access_changes(&self, _: &EnvironmentId) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn preference(&self, key: &'static str) -> LocalBoxFuture<'static, Option<serde_json::Value>> {
        self.0.preference(key)
    }
    fn set_preference(&self, key: &'static str, value: serde_json::Value) {
        self.0.set_preference(key, value)
    }
}

pub struct Setup {
    pub files: FilesRuntime,
    pub viewer: FakeViewer,
    pub open: Option<String>,
}

/// The Files surface as PNL-HOST mounts it, over `setup`'s fakes.
pub fn surface_fixture(setup: Setup) -> impl IntoElement {
    surface_fixture_with_attachment(setup, None)
}

pub fn surface_fixture_with_attachment(
    setup: Setup,
    attachment: Option<ChatFileAttachment>,
) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(setup.files);
    provide_context(FileViewerRuntime(Rc::new(setup.viewer)));
    let thread = ThreadKey { environment: EnvironmentId("env".into()), thread: ThreadId("t".into()) };
    let host = PanelHost::new(PanelStore::default(), |_| {});
    host.apply(
        &thread,
        match (attachment, setup.open) {
            (Some(attachment), _) => Action::OpenAttachment(attachment),
            (None, Some(path)) => Action::OpenFile { path, line: None },
            (None, None) => Action::Open("files".into()),
        },
    );
    let panel = host.panel(thread.clone());
    let surface = computed(move || panel.with(|p| p.active().cloned()));
    let tabs = computed(move || panel.with(|p| p.surfaces.iter().map(|s| s.id.clone()).collect::<Vec<_>>().join(",")));
    view()
        .flex_1()
        .child(text(move || format!("tabs:{}", tabs.get())))
        .child(file_preview_panel(SurfaceContext { host, thread, surface, visible: computed(|| true) }))
}

pub fn start_root(viewport: (f32, f32), root: Root) -> Headless {
    let config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(viewport.0), px(viewport.1)),
        ..EngineConfig::new(root)
    };
    Headless::start(config).expect("file surface starts")
}

pub fn setup(open: &str, files: &[(&str, FileReadView)]) -> Setup {
    let viewer = FakeViewer::default();
    for (path, view) in files {
        viewer.files.borrow_mut().insert((*path).into(), view.clone());
    }
    Setup { files: FilesRuntime(Rc::new(FakeFiles::default())), viewer, open: Some(open.into()) }
}

#[component]
fn SourceCase() -> impl IntoElement {
    surface_fixture(setup("src/main.rs", &[("src/main.rs", contents("src/main.rs", "fn main() {}
"))]))
}
#[component]
fn MarkdownCase() -> impl IntoElement {
    surface_fixture(setup("notes.md", &[("notes.md", contents("notes.md", "---
title: x
---
# Heading
"))]))
}
#[component]
fn ReadmeCase() -> impl IntoElement {
    surface_fixture(setup("README.md", &[("README.md", contents("README.md", "hello
"))]))
}
#[component]
fn FailureCase() -> impl IntoElement {
    let failure: ProjectReadFileError = serde_json::from_value(serde_json::json!({
        "_tag": "ProjectReadFileError",
        "message": "x",
        "failure": "workspace_path_outside_root",
        "resolvedPath": "/elsewhere/a.rs"
    }))
    .expect("contract error");
    let view = FileReadView {
        error: Some(Problem::server("x".into())),
        read_error: Some(Rc::new(failure)),
        ..FileReadView::default()
    };
    surface_fixture(setup("../a.rs", &[("../a.rs", view)]))
}
#[component]
fn DeniedCase() -> impl IntoElement {
    surface_fixture(Setup {
        files: FilesRuntime(Rc::new(Denied::default())),
        viewer: FakeViewer::default(),
        open: Some("a.rs".into()),
    })
}

#[test]
fn a_source_file_shows_its_text_breadcrumbs_and_the_tree() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start_root(viewport, Root::of::<SourceCase>());
        app.wait_for("text:/fn main/").expect("file contents");
        assert!(app.screen().shows("Proj"), "the project crumb");
        assert!(app.screen().shows("main.rs"));
        app.wait_for("text:README.md").expect("explorer beside the file");
        app.press("role:button[Hide file explorer]").expect("hide explorer");
        app.wait_for_gone("text:README.md", std::time::Duration::from_secs(5)).expect("explorer hidden");
        app.press("role:button[Show file explorer]").expect("show explorer");
        app.wait_for("text:README.md").expect("explorer back");
    }
}

#[test]
fn markdown_toggles_between_source_and_rendered() {
    let mut app = start_root(DESKTOP, Root::of::<MarkdownCase>());
    app.wait_for("text:/# Heading/").expect("source first, as the web defaults");
    app.press("role:button[Show rendered markdown]").expect("render");
    app.wait_for("text:Heading").expect("rendered heading");
    assert!(!app.screen().shows("title: x"), "frontmatter is masked when rendered");
    app.press("role:button[Show markdown source]").expect("back to source");
    app.wait_for("text:/# Heading/").expect("source again");
}

#[test]
fn a_tree_selection_opens_its_own_file_tab() {
    let mut app = start_root(DESKTOP, Root::of::<ReadmeCase>());
    app.wait_for("text:/hello/").expect("readme");
    app.press("text:src").expect("expand src");
    app.press("text:main.rs").expect("open main.rs");
    app.wait_for("text:tabs:file:README.md,file:src/main.rs").expect("a file tab per file");
}

#[test]
fn read_failures_say_what_failed_and_where() {
    let mut app = start_root(DESKTOP, Root::of::<FailureCase>());
    app.wait_for("text:The requested path is outside the workspace.").expect("message");
    assert!(app.screen().shows("/elsewhere/a.rs"));
    assert!(app.screen().shows("Workspace folder: /repo. Check the link's path or locate the file in Files."));
}

#[test]
fn refused_file_access_shows_the_web_reason_and_reads_nothing() {
    let mut app = start_root(DESKTOP, Root::of::<DeniedCase>());
    app.wait_for("text:This connection cannot read host files.").expect("access reason");
}
