//! FILES-1: headless tests of the workspace file tree, drawn from the client's real tree model.
//! The fake below only answers the model's folder reads and searches, as the server would.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream, StreamExt};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::{
    services::filesystem::ReadAccess,
    types::EnvironmentId,
    workspace::project_files::tree::{FileTreeView, Problem, Reveal, TreeModel},
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString, project::{ProjectEntry, ProjectEntryKind}};
use t3_ui::screens::panel::files_tree::{
    FileBrowserProps, FileTreeHandle, FilesRuntime, FilesService, file_browser_panel,
};

pub fn entry(path: &str, directory: bool) -> ProjectEntry {
    ProjectEntry {
        path: TrimmedNonEmptyString(path.into()),
        kind: if directory { ProjectEntryKind::Directory } else { ProjectEntryKind::File },
        ignored: None,
    }
}

/// `src/` (main.rs, lib/ with mod.rs) and README.md; `fail` refuses folder reads until cleared.
pub fn workspace_listing(directory: &str) -> Vec<ProjectEntry> {
    match directory {
        "" => vec![entry("src", true), entry("README.md", false)],
        "src" => vec![entry("src/main.rs", false), entry("src/lib", true)],
        "src/lib" => vec![entry("src/lib/mod.rs", false)],
        _ => vec![],
    }
}

pub struct FakeTree {
    model: RefCell<TreeModel>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
    fail: Rc<RefCell<bool>>,
    pub lists: RefCell<Vec<String>>,
}
impl FakeTree {
    fn serve(&self) {
        loop {
            let next = self.model.borrow_mut().next_start();
            let Some(directory) = next else { break };
            self.lists.borrow_mut().push(directory.clone());
            let result = if self.fail.replace(false) {
                Err(Problem::server("Workspace root is gone.".into()))
            } else {
                Ok(workspace_listing(&directory))
            };
            self.model.borrow_mut().complete(&directory, result);
        }
        self.listeners.borrow_mut().retain(|l| l.unbounded_send(()).is_ok());
    }
}
impl FileTreeHandle for FakeTree {
    fn view(&self) -> FileTreeView {
        self.model.borrow().view()
    }
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.listeners.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    fn set_expanded(&self, ids: &[SharedString]) {
        self.model.borrow_mut().set_expanded(ids);
        self.serve();
    }
    fn toggle_all(&self) {
        self.model.borrow_mut().toggle_all();
        self.serve();
    }
    fn refresh(&self) {
        self.model.borrow_mut().refresh();
        self.serve();
    }
    fn set_query(&self, query: String) {
        let generation = self.model.borrow_mut().set_query(query, true);
        if let Some(generation) = generation {
            let found = self.model.borrow().search_input(generation).map(|q| {
                ["src/lib/mod.rs", "src/main.rs", "README.md"]
                    .into_iter()
                    .filter(|p| p.to_lowercase().contains(&q.to_lowercase()))
                    .map(|p| entry(p, false))
                    .collect::<Vec<_>>()
            });
            if let Some(found) = found {
                self.model.borrow_mut().complete_search(generation, Ok((found, false)));
            }
        }
        self.serve();
    }
    fn select(&self, path: &str) -> bool {
        let file = self.model.borrow().is_file(path);
        if file {
            self.model.borrow_mut().select_from_tree(path);
        }
        file
    }
    fn load_ancestors(&self, path: &str) {
        self.model.borrow_mut().load_ancestors(path);
        self.serve();
    }
    fn reveal(&self, path: Option<&str>, reveal_id: u64) -> Option<Reveal> {
        let reveal = self.model.borrow_mut().reveal(path, reveal_id);
        if reveal.is_some() {
            self.serve();
        }
        reveal
    }
    fn observe_mutation(&self, mutation: Option<&str>) {
        self.model.borrow_mut().observe_mutation(mutation);
        self.serve();
    }
}

#[derive(Clone, Default)]
pub struct FakeFiles {
    pub trees: Rc<RefCell<BTreeMap<String, Rc<FakeTree>>>>,
    pub fail: Rc<RefCell<bool>>,
    pub preferences: Rc<RefCell<BTreeMap<&'static str, serde_json::Value>>>,
}
impl FilesService for FakeFiles {
    fn tree(&self, _: &EnvironmentId, cwd: &str, _: bool) -> Rc<dyn FileTreeHandle> {
        let tree = self
            .trees
            .borrow_mut()
            .entry(cwd.to_owned())
            .or_insert_with(|| {
                Rc::new(FakeTree {
                    model: RefCell::new(TreeModel::new(vec![])),
                    listeners: RefCell::default(),
                    fail: self.fail.clone(),
                    lists: RefCell::default(),
                })
            })
            .clone();
        tree.serve();
        tree
    }
    fn read_access(&self, _: &EnvironmentId) -> ReadAccess {
        ReadAccess { allowed: true, pending: false, error: None }
    }
    fn access_changes(&self, _: &EnvironmentId) -> LocalBoxStream<'static, ()> {
        futures::stream::pending().boxed_local()
    }
    fn preference(&self, key: &'static str) -> LocalBoxFuture<'static, Option<serde_json::Value>> {
        let value = self.preferences.borrow().get(key).cloned();
        async move { value }.boxed_local()
    }
    fn set_preference(&self, key: &'static str, value: serde_json::Value) {
        self.preferences.borrow_mut().insert(key, value);
    }
}

fn browser_fixture(fail_first_read: bool) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let files = FakeFiles::default();
    *files.fail.borrow_mut() = fail_first_read;
    let opened = signal(Vec::<String>::new());
    let lists = signal(0_usize);
    let props = FileBrowserProps {
        environment: EnvironmentId("env".into()),
        cwd: "/repo".into(),
        project_name: "Proj".into(),
        selected_path: computed(|| None),
        reveal_id: computed(|| 0),
        on_open_file: Rc::new(move |path| opened.update(|o| o.push(path))),
        host_folder: false,
        mutation: computed(|| None),
        visible: computed(|| true),
    };
    let panel = file_browser_panel(FilesRuntime(Rc::new(files.clone())), props);
    let tree = files.trees.borrow()["/repo"].clone();
    let mut changes = tree.changes();
    // The fake's reads are counted on screen, where the test reads them.
    spawn(async move {
        while changes.next().await.is_some() {
            if !lists.try_set(tree.lists.borrow().len()) {
                return;
            }
        }
    });
    lists.set(files.trees.borrow()["/repo"].lists.borrow().len());
    view()
        .flex_1()
        .child(text(move || format!("opened:{}", opened.get().join(","))))
        .child(text(move || format!("lists:{}", lists.get())))
        .child(panel)
}

#[component]
fn Browser() -> impl IntoElement {
    browser_fixture(false)
}

#[component]
fn BrokenBrowser() -> impl IntoElement {
    browser_fixture(true)
}

fn start(viewport: (f32, f32), broken: bool) -> Headless {
    let root = if broken { Root::of::<BrokenBrowser>() } else { Root::of::<Browser>() };
    let config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(viewport.0), px(viewport.1)),
        ..EngineConfig::new(root)
    };
    Headless::start(config).expect("file browser starts")
}

#[test]
fn folders_expand_lazily_and_files_open_on_selection() {
    for viewport in [DESKTOP, PHONE] {
        let mut app = start(viewport, false);
        app.wait_for("text:README.md").expect("root listed");
        assert!(!app.screen().shows("main.rs"), "a collapsed folder's rows are not built");
        assert!(app.screen().shows("lists:1"), "only the root is read");
        app.press("text:src").expect("expand src");
        app.wait_for("text:main.rs").expect("src listed");
        assert!(app.screen().shows("lists:2"));
        app.press("text:main.rs").expect("select file");
        app.wait_for("text:opened:src/main.rs").expect("file opened");
        app.press("text:lib").expect("expand a folder, which opens nothing");
        app.wait_for("text:mod.rs").expect("lib listed");
        assert!(app.screen().shows("opened:src/main.rs"));
    }
}

#[test]
fn search_hides_non_matches_and_says_when_nothing_matches() {
    let mut app = start(DESKTOP, false);
    app.wait_for("text:README.md").expect("root listed");
    app.type_text("label:Search Proj files", "mod").expect("search");
    app.wait_for("text:mod.rs").expect("the match and its folders show");
    assert!(!app.screen().shows("README.md"));
    app.type_text("label:Search Proj files", "zzz").expect("search again");
    app.wait_for("text:No matching files.").expect("no matches");
}

#[test]
fn a_failed_folder_read_shows_the_server_reason_and_retries() {
    let mut app = start(DESKTOP, true);
    app.wait_for("text:Workspace root is gone. Click to retry.").expect("error row");
    app.press("role:button[Workspace root is gone. Click to retry.]").expect("retry");
    app.wait_for("text:README.md").expect("listed after retry");
    assert!(!app.screen().shows("Click to retry."));
}

#[test]
fn expand_all_opens_every_folder_and_collapses_them_again() {
    let mut app = start(DESKTOP, false);
    app.wait_for("text:README.md").expect("root listed");
    app.press("role:button[Refresh workspace files]").expect("refresh");
    app.wait_for("text:lists:2").expect("refresh re-reads the root");
    app.press("role:button[Expand all folders]").expect("expand all");
    app.wait_for("label:Collapse all folders").expect("the button now collapses");
    app.wait_for("text:mod.rs").expect("nested folder opened");
    app.press("role:button[Collapse all folders]").expect("collapse all");
    app.wait_for_gone("text:main.rs", std::time::Duration::from_secs(5)).expect("collapsed");
}
