//! DIFF-1/2/3: the diff panel as the right panel mounts it, fed by snapshots the client's own
//! derivations build (selection, body, files, tree, comment annotations).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use futures::channel::mpsc;
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::shell::diff_state::Selection;
use t3_client::workspace::diff_panel::{
    BranchCommitsView, DiffPanelSnapshot, ShownFiles,
    comments::{CommentAnnotation, ExcerptMap, Side},
    files::{RenderablePatch, renderable_patch},
    logic::{FileAccess, PatchState, TurnSummary, resolve_body, resolve_selection},
    tree::{diff_file_tree, diff_file_tree_entries},
};
use t3_contracts::base_schemas::RunId;
use t3_contracts::settings::{DiffLayout, TimestampFormat};
use t3_ui::screens::panel::diff::{DiffPanelBackend, DiffPanelRuntime, diff_panel};

use crate::support::{DESKTOP, PHONE};

pub const PATCH: &str = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n tail\ndiff --git a/src/a.test.ts b/src/a.test.ts\n--- a/src/a.test.ts\n+++ b/src/a.test.ts\n@@ -1 +1 @@\n-x\n+y\n";

pub fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("t1".into()),
    }
}

#[derive(Clone)]
pub struct Inputs {
    pub thread: bool,
    pub repo: bool,
    pub patch: Option<&'static str>,
    pub pending: bool,
    pub collapsed: BTreeSet<String>,
    pub layout: DiffLayout,
    pub file_tree_open: bool,
    pub comments: Vec<CommentAnnotation>,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            thread: true,
            repo: true,
            patch: Some(PATCH),
            pending: false,
            collapsed: BTreeSet::new(),
            layout: DiffLayout::Stacked,
            file_tree_open: false,
            comments: Vec::new(),
        }
    }
}

/// The client's derivations over the inputs, as `DiffPanelService::watch` composes them.
pub fn snapshot(inputs: &Inputs) -> DiffPanelSnapshot {
    let turns = vec![TurnSummary {
        run_id: RunId("r1".into()),
        turn_count: 1,
        completed_at: "2026-11-14T10:00:00.000Z".into(),
    }];
    let selection = Selection::Turn {
        turn_id: RunId("r1".into()),
        file_path: None,
        reveal_request_id: 1,
    };
    let resolved = resolve_selection(&key(), &selection, &turns, None);
    let patch = inputs.patch.map(Rc::<str>::from);
    let state = PatchState {
        pending: inputs.pending,
        answered: patch.is_some(),
        patch: patch.clone(),
        ..Default::default()
    };
    let renderable = patch.as_ref().and_then(renderable_patch);
    let access = FileAccess {
        allowed: true,
        pending: false,
        error: None,
    };
    let view = resolve_body(
        inputs.thread,
        inputs.repo,
        &resolved,
        &turns,
        &access,
        &state,
        renderable.as_ref(),
    );
    let (shown, shown_files, file_keys, tree) = match &renderable {
        Some(RenderablePatch::Files(files)) => {
            let keys = files.file_keys(false);
            let shown_files: Vec<_> = files
                .files
                .iter()
                .zip(&keys)
                .map(|(f, k)| (k.clone(), f.path().to_owned()))
                .collect();
            let indices: Vec<usize> = (0..files.files.len()).collect();
            let shown = Rc::new(ShownFiles {
                map: ExcerptMap::new(&files.files),
                files: files.clone(),
                indices,
            });
            let tree = diff_file_tree(&diff_file_tree_entries(&files.files));
            (Some(shown), shown_files, keys, tree)
        }
        _ => (None, Vec::new(), Vec::new(), Vec::new()),
    };
    DiffPanelSnapshot {
        thread_present: inputs.thread,
        turns: Rc::new(turns),
        view,
        shown_patch: patch.filter(|_| shown.is_some()),
        shown_files: Rc::new(shown_files),
        file_keys: Rc::new(file_keys),
        collapsed: Rc::new(inputs.collapsed.clone()),
        shown,
        annotations: Rc::new(inputs.comments.clone()),
        tree: Rc::new(tree),
        test_file_count: 1,
        layout: inputs.layout.clone(),
        word_wrap: false,
        timestamp_format: TimestampFormat::Locale,
        ignore_whitespace: false,
        hide_test_files: false,
        file_tree_open: inputs.file_tree_open,
        can_refresh: false,
        refreshing: false,
        comparison: None,
        resolved,
    }
}

/// A backend that records what the panel asks and answers with the next snapshot.
pub struct Fake {
    pub inputs: RefCell<Inputs>,
    pub sinks: RefCell<Vec<mpsc::UnboundedSender<Rc<DiffPanelSnapshot>>>>,
    pub calls: RefCell<Vec<String>>,
}

impl Fake {
    pub fn new(inputs: Inputs) -> Rc<Self> {
        Rc::new(Self {
            inputs: RefCell::new(inputs),
            sinks: RefCell::new(Vec::new()),
            calls: RefCell::new(Vec::new()),
        })
    }
    fn publish(&self) {
        let next = Rc::new(snapshot(&self.inputs.borrow()));
        self.sinks
            .borrow_mut()
            .retain(|sink| sink.unbounded_send(next.clone()).is_ok());
    }
    fn done(&self) -> LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        async { Ok(()) }.boxed_local()
    }
}

impl DiffPanelBackend for Fake {
    fn watch(&self, _: ThreadKey) -> LocalBoxStream<'static, Rc<DiffPanelSnapshot>> {
        let (sink, stream) = mpsc::unbounded();
        let _ = sink.unbounded_send(Rc::new(snapshot(&self.inputs.borrow())));
        self.sinks.borrow_mut().push(sink);
        stream.boxed_local()
    }
    fn branch_commits(&self, _: &ThreadKey) -> LocalBoxStream<'static, BranchCommitsView> {
        futures::stream::empty().boxed_local()
    }
    fn select_scope(
        &self,
        _: &ThreadKey,
        value: &str,
    ) -> LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        self.calls.borrow_mut().push(format!("scope:{value}"));
        self.done()
    }
    fn set_layout(
        &self,
        layout: DiffLayout,
    ) -> LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        self.calls.borrow_mut().push(format!("layout:{layout:?}"));
        self.inputs.borrow_mut().layout = layout;
        self.publish();
        self.done()
    }
    fn set_ignore_whitespace(&self, ignore: bool) {
        self.calls.borrow_mut().push(format!("whitespace:{ignore}"));
    }
    fn set_hide_test_files(
        &self,
        hide: bool,
    ) -> LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        self.calls.borrow_mut().push(format!("tests:{hide}"));
        self.done()
    }
    fn set_file_tree_open(
        &self,
        open: bool,
    ) -> LocalBoxFuture<'static, Result<(), t3_client::ClientError>> {
        self.calls.borrow_mut().push(format!("tree:{open}"));
        self.inputs.borrow_mut().file_tree_open = open;
        self.publish();
        self.done()
    }
    fn toggle_file(&self, _: &DiffPanelSnapshot, file_key: &str) {
        self.calls.borrow_mut().push("toggle".into());
        {
            let mut inputs = self.inputs.borrow_mut();
            if !inputs.collapsed.remove(file_key) {
                inputs.collapsed.insert(file_key.to_owned());
            }
        }
        self.publish();
    }
    fn unfold_file(&self, _: &DiffPanelSnapshot, file_key: &str) {
        self.calls.borrow_mut().push("unfold".into());
        self.inputs.borrow_mut().collapsed.remove(file_key);
        self.publish();
    }
    fn toggle_all_files(&self, snapshot: &DiffPanelSnapshot) {
        self.calls.borrow_mut().push("all".into());
        {
            let mut inputs = self.inputs.borrow_mut();
            if inputs.collapsed.is_empty() {
                inputs.collapsed = snapshot
                    .shown_files
                    .iter()
                    .map(|(k, _)| k.clone())
                    .collect();
            } else {
                inputs.collapsed.clear();
            }
        }
        self.publish();
    }
    fn refresh(&self, _: &ThreadKey) {}
    fn draft_comment(
        &self,
        _: &DiffPanelSnapshot,
        side: Side,
        lines: (usize, usize),
    ) -> Option<CommentAnnotation> {
        Some(CommentAnnotation {
            id: "draft".into(),
            side,
            line: lines.1,
            range_label: "+11".into(),
            text: String::new(),
        })
    }
    fn add_review_comment(
        &self,
        _: &ThreadKey,
        _: &DiffPanelSnapshot,
        side: Side,
        lines: (usize, usize),
        text: &str,
    ) -> Result<(), t3_client::ClientError> {
        self.calls.borrow_mut().push(format!("comment:{text}"));
        self.inputs.borrow_mut().comments.push(CommentAnnotation {
            id: "c1".into(),
            side,
            line: lines.1,
            range_label: "+11".into(),
            text: text.into(),
        });
        self.publish();
        Ok(())
    }
    fn remove_review_comment(&self, _: &ThreadKey, id: &str) -> Result<(), t3_client::ClientError> {
        self.calls.borrow_mut().push(format!("remove:{id}"));
        self.inputs.borrow_mut().comments.retain(|c| c.id != id);
        self.publish();
        Ok(())
    }
}

thread_local! {
    pub static FAKE: RefCell<Option<Rc<Fake>>> = const { RefCell::new(None) };
}

#[component]
fn PanelFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let fake = FAKE.with(|f| f.borrow().clone()).expect("fake installed");
    let visible = computed(|| true);
    view()
        .w_full()
        .h_full()
        .child(diff_panel(DiffPanelRuntime(fake), key(), visible))
}

pub fn start(window: (f32, f32), inputs: Inputs) -> (Headless, Rc<Fake>) {
    let fake = Fake::new(inputs);
    FAKE.with(|f| *f.borrow_mut() = Some(fake.clone()));
    let app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<PanelFixture>())
    })
    .expect("diff panel fixture starts");
    (app, fake)
}

#[test]
fn a_turn_diff_shows_its_scope_totals_and_file_headers_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let (mut app, _) = start(window, Inputs::default());
        app.wait_for("Latest turn").expect("scope label");
        app.wait_for("src/a.ts").expect("file header");
        app.wait_for("+2").expect("additions total");
        app.wait_for("-2").expect("deletions total");
    }
}

#[test]
fn the_empty_states_follow_the_web_order() {
    let (mut app, _) = start(
        DESKTOP,
        Inputs {
            thread: false,
            ..Default::default()
        },
    );
    app.wait_for("Select a thread to inspect turn diffs.")
        .expect("no thread");
    let (mut app, _) = start(
        DESKTOP,
        Inputs {
            repo: false,
            ..Default::default()
        },
    );
    app.wait_for("Turn diffs are unavailable because this project is not a git repository.")
        .expect("not a repository");
    let (mut app, _) = start(
        DESKTOP,
        Inputs {
            patch: None,
            pending: true,
            ..Default::default()
        },
    );
    app.wait_for("Loading checkpoint diff...")
        .expect("loading label");
    let (mut app, _) = start(
        DESKTOP,
        Inputs {
            patch: Some(""),
            ..Default::default()
        },
    );
    app.wait_for("No net changes in this selection.")
        .expect("empty diff");
}

#[test]
fn folding_a_file_and_the_layout_toggle_go_through_the_client() {
    let (mut app, fake) = start(DESKTOP, Inputs::default());
    app.wait_for("src/a.ts").expect("file header");
    app.press("role:button[Collapse src/a.ts]")
        .expect("fold the file");
    app.wait_for("role:button[Expand src/a.ts]")
        .expect("folded header");
    assert!(fake.calls.borrow().contains(&"toggle".to_owned()));
    app.press("role:button[Collapse all files]")
        .expect("collapse all");
    app.wait_for("role:button[Expand all files]")
        .expect("all folded");
    app.press("role:button[Split diff view]")
        .expect("split layout");
    assert!(fake.calls.borrow().iter().any(|c| c == "layout:Split"));
}

#[test]
fn the_file_tree_lists_the_shown_files_and_reveals_one() {
    let (mut app, fake) = start(DESKTOP, Inputs::default());
    app.wait_for("src/a.ts").expect("diff");
    app.press("role:button[Show file tree]")
        .expect("open the tree");
    app.wait_for("Files").expect("tree header");
    app.wait_for("a.test.ts").expect("tree row");
    assert!(fake.calls.borrow().contains(&"tree:true".to_owned()));
    app.press("a.test.ts").expect("select a file");
    assert!(fake.calls.borrow().contains(&"unfold".to_owned()));
}
