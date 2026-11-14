//! The diff lane's binding: `screens::panel::diff::DiffPanelBackend` over the client's
//! `DiffPanelService`. Mount with `diff::diff_panel_runtime(&client)`.
use crate::screens::panel::diff::{DiffPanelBackend, DiffPanelRuntime};
use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;
use t3_client::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    types::ThreadKey,
    workspace::diff_panel::{
        BranchCommitsView, DiffPanelService, DiffPanelSnapshot,
        comments::{CommentAnnotation, Side},
    },
};
use t3_contracts::settings::DiffLayout;

struct DiffBinding<P: Platform + Clone + 'static>(DiffPanelService<P>);

pub fn diff_panel_runtime<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> DiffPanelRuntime {
    DiffPanelRuntime(Rc::new(DiffBinding(client.diff_panel())))
}

impl<P: Platform + Clone + 'static> DiffPanelBackend for DiffBinding<P> {
    fn watch(&self, key: ThreadKey) -> LocalBoxStream<'static, Rc<DiffPanelSnapshot>> {
        self.0.watch(key)
    }
    fn branch_commits(&self, key: &ThreadKey) -> LocalBoxStream<'static, BranchCommitsView> {
        self.0.branch_commits(key)
    }
    fn select_scope(
        &self,
        key: &ThreadKey,
        value: &str,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.select_scope(key, value)
    }
    fn set_layout(&self, layout: DiffLayout) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.set_layout(layout)
    }
    fn set_ignore_whitespace(&self, ignore: bool) {
        self.0.set_ignore_whitespace(ignore);
    }
    fn set_hide_test_files(&self, hide: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.set_hide_test_files(hide)
    }
    fn set_file_tree_open(&self, open: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.0.set_file_tree_open(open)
    }
    fn toggle_file(&self, snapshot: &DiffPanelSnapshot, file_key: &str) {
        self.0.toggle_file(snapshot, file_key);
    }
    fn unfold_file(&self, snapshot: &DiffPanelSnapshot, file_key: &str) {
        self.0.unfold_file(snapshot, file_key);
    }
    fn toggle_all_files(&self, snapshot: &DiffPanelSnapshot) {
        self.0.toggle_all_files(snapshot);
    }
    fn refresh(&self, key: &ThreadKey) {
        self.0.refresh(key);
    }
    fn draft_comment(
        &self,
        snapshot: &DiffPanelSnapshot,
        side: Side,
        lines: (usize, usize),
    ) -> Option<CommentAnnotation> {
        self.0.draft_comment(snapshot, side, lines)
    }
    fn add_review_comment(
        &self,
        key: &ThreadKey,
        snapshot: &DiffPanelSnapshot,
        side: Side,
        lines: (usize, usize),
        text: &str,
    ) -> Result<(), ClientError> {
        self.0.add_review_comment(key, snapshot, side, lines, text)
    }
    fn remove_review_comment(&self, key: &ThreadKey, id: &str) -> Result<(), ClientError> {
        self.0.remove_review_comment(key, id)
    }
}
