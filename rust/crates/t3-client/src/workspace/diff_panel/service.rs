//! The diff panel's composed service: DiffPanel.tsx's hooks (`useCheckpointDiff`,
//! `reviewEnvironment.diffPreview`, `useClientSettings`, `useHideTestFiles`, the file-tree
//! toggle) over the client's store, queries and settings, and diffPanelStore.ts's mutations.

use super::{
    files::{PatchFiles, RenderablePatch, renderable_patch},
    logic::{
        DiffBodyView, DiffQuery, FileAccess, GitScope, PatchState, ResolvedSelection, ScopeAction,
        TurnSummary, ordered_turn_summaries, resolve_body, resolve_selection, scope_action,
    },
};
use crate::{
    ClientError,
    composition::{ClientRuntime, QueryLease},
    connection::Phase,
    persistence::{Record, decode_record},
    platform::{Platform, StorageArea, StorageKey, StorageWrite},
    services::resources::ResourcePolicy,
    thread_views::composer::send::Changes,
    types::ThreadKey,
    view_logic::shell::diff_state::{DiffStore, STORAGE_KEY, Selection},
    workspace::git_panel::GitPanelState,
};
use futures::{
    FutureExt, StreamExt,
    future::{Either, LocalBoxFuture},
    stream::{LocalBoxStream, SelectAll},
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
};
use t3_contracts::{
    base_schemas::{RunId, TrimmedNonEmptyString},
    checkpoint_diff::{
        OrchestrationGetFullThreadDiffInput, OrchestrationGetTurnDiffInput, ThreadTurnDiff,
    },
    review::{
        ReviewBranchCommitsInput, ReviewBranchCommitsResult, ReviewDiffPreviewInput,
        ReviewDiffPreviewInputSourceKind, ReviewDiffPreviewResult, ReviewDiffPreviewSourceKind,
    },
    rpc::{
        WsOrchestrationV2GetFullThreadDiffRpc, WsOrchestrationV2GetTurnDiffRpc,
        WsReviewGetBranchCommitsRpc, WsReviewGetDiffPreviewRpc,
    },
    settings::{DiffLayout, TimestampFormat},
};

const FILE_TREE_KEY: &str = "t3code.diffFileTreeOpen";
const HIDE_TEST_FILES_KEY: &str = "t3code.diffHideTestFiles";

/// Session state beside the shared [`DiffStore`]: the two `useLocalStorage` toggles.
#[derive(Default)]
pub struct DiffPanelState {
    file_tree_open: Cell<bool>,
    hide_test_files: Cell<bool>,
    loaded: Cell<bool>,
    changes: Changes,
}

/// What the panel shows for one thread, recomputed when any input changes.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffPanelSnapshot {
    pub thread_present: bool,
    pub turns: Rc<Vec<TurnSummary>>,
    pub resolved: ResolvedSelection,
    pub view: DiffBodyView,
    /// The patch the view draws: test files left out while they are hidden (a requested file
    /// stays), the source otherwise.
    pub shown_patch: Option<Rc<str>>,
    /// Keys of the shown files, in order, with their paths.
    pub shown_files: Rc<Vec<(String, String)>>,
    /// Keys of every file of the selection.
    pub file_keys: Rc<Vec<String>>,
    pub collapsed: Rc<BTreeSet<String>>,
    pub test_file_count: usize,
    pub layout: DiffLayout,
    pub word_wrap: bool,
    pub timestamp_format: TimestampFormat,
    pub ignore_whitespace: bool,
    pub hide_test_files: bool,
    pub file_tree_open: bool,
    /// The git diff can be refreshed (a git selection of a checkout).
    pub can_refresh: bool,
    pub refreshing: bool,
    /// `comparisonHeadRef` and `comparisonBaseRef`, shown for the branch scope.
    pub comparison: Option<(String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BranchCommitsView {
    pub commits: Option<Rc<ReviewBranchCommitsResult>>,
    pub error: Option<String>,
    pub access: Option<FileAccess>,
}

#[derive(Clone)]
pub struct DiffPanelService<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    shared: Rc<GitPanelState>,
}

fn storage_key(record: &str) -> StorageKey {
    StorageKey::new("diff-panel-state", None, record)
}

fn trimmed(value: &str) -> TrimmedNonEmptyString {
    TrimmedNonEmptyString(value.trim().to_owned())
}

impl<P: Platform + Clone + 'static> DiffPanelService<P> {
    pub(crate) fn new(runtime: ClientRuntime<P>, platform: P, shared: Rc<GitPanelState>) -> Self {
        Self {
            runtime,
            platform,
            shared,
        }
    }

    fn state(&self) -> &DiffPanelState {
        self.shared.diff_panel()
    }

    fn store(&self) -> &RefCell<DiffStore> {
        self.shared.diff_store()
    }

    /// Restores the persisted selection and toggles once; entries made before it finished win.
    async fn load(&self) {
        if self.state().loaded.replace(true) {
            return;
        }
        if let Ok(Some(bytes)) = self
            .platform
            .get(StorageArea::Ordinary, storage_key(STORAGE_KEY))
            .await
            && let Ok(record) =
                decode_record::<serde_json::Value>(&bytes, &storage_key(STORAGE_KEY), 2)
            && let Ok(saved) = DiffStore::decode(&record.payload.to_string())
        {
            let mut store = self.store().borrow_mut();
            for (key, selection) in saved.by_thread {
                store.by_thread.entry(key).or_insert(selection);
            }
            for (key, base) in saved.branch_base {
                store.branch_base.entry(key).or_insert(base);
            }
        }
        for (record, cell) in [
            (FILE_TREE_KEY, &self.state().file_tree_open),
            (HIDE_TEST_FILES_KEY, &self.state().hide_test_files),
        ] {
            if let Ok(Some(bytes)) = self
                .platform
                .get(StorageArea::Ordinary, storage_key(record))
                .await
                && let Ok(value) = decode_record::<bool>(&bytes, &storage_key(record), 1)
            {
                cell.set(value.payload);
            }
        }
        self.changed();
    }

    fn changed(&self) {
        self.state().changes.notify();
        self.shared.diff_changed();
    }

    fn save_selection(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let platform = self.platform.clone();
        let encoded = self.store().borrow().encode();
        async move {
            let value: serde_json::Value = serde_json::from_str(&encoded?)?;
            let record = Record::new("diff-panel-state", None, STORAGE_KEY, 2, value);
            platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: storage_key(STORAGE_KEY),
                        value: Some(serde_json::to_vec(&record)?),
                    }],
                )
                .await?;
            Ok(())
        }
        .boxed_local()
    }

    fn save_toggle(&self, record: &'static str, value: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let platform = self.platform.clone();
        async move {
            let bytes = serde_json::to_vec(&Record::new("diff-panel-state", None, record, 1, value))?;
            platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: storage_key(record),
                        value: Some(bytes),
                    }],
                )
                .await?;
            Ok(())
        }
        .boxed_local()
    }

    /// The menu row `value` chosen in the scope menu (`selectScopeValue`).
    pub fn select_scope(
        &self,
        key: &ThreadKey,
        value: &str,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let turns = self.turns(key);
        let selection = self.selection(key);
        let resolved = resolve_selection(key, &selection, &turns, None);
        match scope_action(value, &resolved, &turns) {
            ScopeAction::Git { unstaged } => self.store().borrow_mut().select_git(key.clone(), unstaged),
            ScopeAction::Commit(sha) => self.store().borrow_mut().select_commit(key.clone(), sha),
            ScopeAction::Turn(run) => self.store().borrow_mut().select_turn(key.clone(), run, None),
            ScopeAction::Nothing => return async { Ok(()) }.boxed_local(),
        }
        self.changed();
        self.save_selection()
    }

    /// `selectTurn`, for a chat link that opens a turn's diff at a file.
    pub fn select_turn(
        &self,
        key: &ThreadKey,
        run: RunId,
        file_path: Option<&str>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.store().borrow_mut().select_turn(key.clone(), run, file_path);
        self.changed();
        self.save_selection()
    }

    /// `selectBranchBaseRef`: `None` is Automatic.
    pub fn select_base_ref(
        &self,
        key: &ThreadKey,
        base_ref: Option<&str>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.store().borrow_mut().select_base(key.clone(), base_ref);
        self.changed();
        self.save_selection()
    }

    /// The panel's whitespace toggle; it overrides the setting for the session.
    pub fn set_ignore_whitespace(&self, ignore: bool) {
        self.store().borrow_mut().ignore_whitespace = Some(ignore);
        self.changed();
    }

    pub fn set_layout(&self, layout: DiffLayout) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let preferences = self.runtime.composer_preferences();
        async move { preferences.update(|settings| settings.diff_layout = layout).await }.boxed_local()
    }

    pub fn set_hide_test_files(&self, hide: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.state().hide_test_files.set(hide);
        self.changed();
        self.save_toggle(HIDE_TEST_FILES_KEY, hide)
    }

    pub fn set_file_tree_open(&self, open: bool) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.state().file_tree_open.set(open);
        self.changed();
        self.save_toggle(FILE_TREE_KEY, open)
    }

    /// `updateCollapsedFileKeys` for the selection's scope.
    pub fn update_collapsed(
        &self,
        snapshot: &DiffPanelSnapshot,
        update: impl FnOnce(&BTreeSet<String>) -> BTreeSet<String>,
    ) {
        let defaults = self.default_collapsed(&snapshot.file_keys);
        let scope = snapshot.resolved.collapse_scope.clone();
        let current = self
            .store()
            .borrow()
            .collapsed
            .get(&scope)
            .cloned()
            .unwrap_or(defaults.clone());
        let next = update(&current);
        if next == current {
            return;
        }
        self.store()
            .borrow_mut()
            .update_collapsed(scope, &defaults, |_| next);
        self.changed();
    }

    /// One file's fold (`toggleDiffFileCollapsed`).
    pub fn toggle_file(&self, snapshot: &DiffPanelSnapshot, file_key: &str) {
        self.update_collapsed(snapshot, |current| {
            let mut next = current.clone();
            if !next.remove(file_key) {
                next.insert(file_key.to_owned());
            }
            next
        });
    }

    /// Unfolds without ever folding (`unfoldDiffFile`, a reveal).
    pub fn unfold_file(&self, snapshot: &DiffPanelSnapshot, file_key: &str) {
        self.update_collapsed(snapshot, |current| {
            let mut next = current.clone();
            next.remove(file_key);
            next
        });
    }

    /// Expand all or collapse all over the shown files (`toggleAllDiffFiles`).
    pub fn toggle_all_files(&self, snapshot: &DiffPanelSnapshot) {
        let keys: Vec<String> = snapshot.shown_files.iter().map(|(key, _)| key.clone()).collect();
        self.update_collapsed(snapshot, |current| super::logic::toggle_all_files(&keys, current));
    }

    /// The refresh button: the git diff is read again.
    pub fn refresh(&self, key: &ThreadKey) {
        let selection = self.selection(key);
        let resolved = resolve_selection(key, &selection, &self.turns(key), None);
        if let Some(input) = self.git_input(key, &resolved)
            && let Some(lease) = self.git_lease(key, input)
        {
            lease.resource().invalidate();
        }
    }

    fn default_collapsed(&self, keys: &[String]) -> BTreeSet<String> {
        if self.runtime.composer_preferences().snapshot().diff_files_collapsed {
            keys.iter().cloned().collect()
        } else {
            BTreeSet::new()
        }
    }

    pub fn selection(&self, key: &ThreadKey) -> Selection {
        self.store()
            .borrow()
            .by_thread
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    fn turns(&self, key: &ThreadKey) -> Vec<TurnSummary> {
        self.runtime
            .store()
            .shared_thread(key)
            .map(|thread| ordered_turn_summaries(thread.checkpoints.iter().map(|c| c.as_ref())))
            .unwrap_or_default()
    }

    /// The thread's checkout: its worktree, else its project's root.
    fn cwd(&self, key: &ThreadKey) -> Option<(bool, Option<String>)> {
        let shell = self.runtime.store().shared_shell(&key.environment)?;
        let thread = shell.threads.iter().find(|t| t.id.0 == key.thread.0)?;
        let cwd = thread.worktree_path.as_ref().map(|p| p.0.clone()).or_else(|| {
            shell
                .projects
                .iter()
                .find(|p| p.id == thread.project_id)
                .map(|p| p.workspace_root.0.clone())
        });
        Some((true, cwd))
    }

    fn ignore_whitespace(&self) -> bool {
        self.store()
            .borrow()
            .ignore_whitespace
            .unwrap_or_else(|| self.runtime.composer_preferences().snapshot().diff_ignore_whitespace)
    }

    fn git_input(&self, key: &ThreadKey, resolved: &ResolvedSelection) -> Option<ReviewDiffPreviewInput> {
        let DiffQuery::Git {
            scope,
            base_ref,
            commit,
        } = &resolved.query
        else {
            return None;
        };
        let cwd = self.cwd(key)?.1?;
        Some(ReviewDiffPreviewInput {
            cwd: trimmed(&cwd),
            base_ref: base_ref.as_deref().map(|b| Some(trimmed(b))),
            commit_sha: commit.as_deref().map(|c| Some(trimmed(c))),
            ignore_whitespace: Some(self.ignore_whitespace()),
            source_kind: commit.is_none().then_some(match scope {
                GitScope::Unstaged => ReviewDiffPreviewInputSourceKind::WorkingTree,
                _ => ReviewDiffPreviewInputSourceKind::BranchRange,
            }),
            tracked_only: None,
            metadata_only: None,
            file: None,
        })
    }

    fn git_lease(&self, key: &ThreadKey, input: ReviewDiffPreviewInput) -> Option<QueryLease<ReviewDiffPreviewResult>> {
        Some(self.runtime.query::<WsReviewGetDiffPreviewRpc>(
            &key.environment,
            input,
            ResourcePolicy::default(),
        ))
    }

    /// `useFilesystemReadAccess` for the git preview: the connected session's grant for it.
    fn file_access(&self, key: &ThreadKey, input: Option<&ReviewDiffPreviewInput>) -> FileAccess {
        if let Some(session) = self.runtime.client(&key.environment).and_then(|c| c.session()) {
            let allowed = input.is_none_or(|input| session.available::<WsReviewGetDiffPreviewRpc>(input));
            return FileAccess {
                allowed,
                pending: false,
                error: None,
            };
        }
        let snapshot = self.runtime.store().snapshot();
        let connection = snapshot
            .environments
            .get(&key.environment)
            .and_then(|e| e.connection.clone());
        let pending = connection
            .as_ref()
            .is_none_or(|c| matches!(c.phase, Phase::Connected | Phase::Connecting | Phase::Backoff));
        FileAccess {
            allowed: false,
            pending,
            error: (!pending).then(|| {
                connection
                    .and_then(|c| c.last_failure.as_ref().map(|f| f.detail.clone()))
                    .unwrap_or_else(|| "This environment is not connected.".into())
            }),
        }
    }

    /// The panel's state for `key`, recomputed whenever the thread, the selection, a setting or
    /// the selected query changes. The selected query runs while the stream is held.
    pub fn watch(&self, key: ThreadKey) -> LocalBoxStream<'static, Rc<DiffPanelSnapshot>> {
        let service = self.clone();
        let mut triggers: SelectAll<LocalBoxStream<'static, ()>> = SelectAll::new();
        triggers.push(
            self.runtime
                .store()
                .subscribe_environment(&key.environment)
                .map(|_| ())
                .boxed_local(),
        );
        triggers.push(self.state().changes.subscribe());
        triggers.push(self.shared.diff_changes());
        triggers.push(self.runtime.composer_preferences().changes());
        let watch = Watch {
            service,
            key,
            triggers,
            active: None,
            parsed: None,
            shown: None,
            last: None,
            started: false,
        };
        futures::stream::unfold(watch, |mut watch| async move {
            if !watch.started {
                watch.started = true;
                watch.service.load().await;
            }
            loop {
                let next = {
                    let changes = match watch.active.as_mut() {
                        Some(active) => Either::Left(active.changes.next()),
                        None => Either::Right(futures::future::pending()),
                    };
                    futures::future::select(watch.triggers.next(), changes).await
                };
                if let Either::Left((None, _)) = next {
                    return None;
                }
                let snapshot = Rc::new(watch.compute());
                if watch.last.as_ref() != Some(&snapshot) {
                    watch.last = Some(snapshot.clone());
                    return Some((snapshot, watch));
                }
            }
        })
        .boxed_local()
    }

    /// The Commits submenu's list, read while the stream is held (`reviewEnvironment.branchCommits`).
    pub fn branch_commits(&self, key: &ThreadKey) -> LocalBoxStream<'static, BranchCommitsView> {
        let Some((_, Some(cwd))) = self.cwd(key) else {
            return futures::stream::once(async { BranchCommitsView::default() }).boxed_local();
        };
        let base = self.store().borrow().branch_base.get(key).cloned().flatten();
        let input = ReviewBranchCommitsInput {
            cwd: trimmed(&cwd),
            base_ref: base.as_deref().map(|b| Some(trimmed(b))),
        };
        let access = self.file_access(key, None);
        if !access.allowed {
            return futures::stream::once(async move {
                BranchCommitsView {
                    access: Some(access),
                    ..Default::default()
                }
            })
            .boxed_local();
        }
        let lease = self.runtime.query::<WsReviewGetBranchCommitsRpc>(
            &key.environment,
            input,
            ResourcePolicy::default(),
        );
        lease
            .resource()
            .observe(self.platform.monotonic_ms())
            .map(move |_| {
                let state = lease.resource().snapshot();
                BranchCommitsView {
                    commits: state.value,
                    error: state.error,
                    access: None,
                }
            })
            .boxed_local()
    }
}

enum ActiveQuery {
    Checkpoint(QueryLease<ThreadTurnDiff>),
    Git(QueryLease<ReviewDiffPreviewResult>),
}

struct Active {
    identity: String,
    query: ActiveQuery,
    changes: LocalBoxStream<'static, u64>,
    /// The answer's diff as shared text, kept while the answer is the same allocation.
    text: Option<(usize, Rc<str>)>,
}

struct Watch<P: Platform + Clone + 'static> {
    service: DiffPanelService<P>,
    key: ThreadKey,
    triggers: SelectAll<LocalBoxStream<'static, ()>>,
    active: Option<Active>,
    /// The parsed files of the last patch text, by its allocation.
    parsed: Option<(Rc<str>, Option<RenderablePatch>)>,
    /// The shown patch for (files, hidden, requested path).
    shown: Option<(Rc<PatchFiles>, bool, Option<String>, Rc<str>)>,
    last: Option<Rc<DiffPanelSnapshot>>,
    started: bool,
}

impl<P: Platform + Clone + 'static> Watch<P> {
    fn activate(&mut self, identity: String, query: impl FnOnce() -> ActiveQuery) {
        if self.active.as_ref().is_some_and(|a| a.identity == identity) {
            return;
        }
        let query = query();
        let now = self.service.platform.monotonic_ms();
        let changes = match &query {
            ActiveQuery::Checkpoint(lease) => lease.resource().observe(now).boxed_local(),
            ActiveQuery::Git(lease) => lease.resource().observe(now).boxed_local(),
        };
        self.active = Some(Active {
            identity,
            query,
            changes,
            text: None,
        });
    }

    fn shared_text(active: &mut Active, rc: usize, text: &str) -> Rc<str> {
        match &active.text {
            Some((at, shared)) if *at == rc => shared.clone(),
            _ => {
                let shared: Rc<str> = Rc::from(text);
                active.text = Some((rc, shared.clone()));
                shared
            }
        }
    }

    fn renderable(&mut self, patch: &Rc<str>) -> Option<RenderablePatch> {
        if let Some((text, parsed)) = &self.parsed
            && Rc::ptr_eq(text, patch)
        {
            return parsed.clone();
        }
        let parsed = renderable_patch(patch);
        self.parsed = Some((patch.clone(), parsed.clone()));
        parsed
    }

    fn compute(&mut self) -> DiffPanelSnapshot {
        let service = self.service.clone();
        let key = self.key.clone();
        let settings = service.runtime.composer_preferences().snapshot();
        let selection = service.selection(&key);
        let turns = service.turns(&key);
        let checkout = service.cwd(&key);
        let thread_present = checkout.is_some();
        let ignore_whitespace = service.ignore_whitespace();
        let git_input = {
            let resolved = resolve_selection(&key, &selection, &turns, None);
            service.git_input(&key, &resolved)
        };
        let mut patch = PatchState::default();
        let mut commit_subject = None;
        let mut comparison_head = None;
        let mut source_base = None;
        let resolved = resolve_selection(&key, &selection, &turns, None);
        match (&resolved.query, &git_input) {
            (DiffQuery::Checkpoint { from, to }, _) if thread_present => {
                let (from, to) = (*from, *to);
                let identity = format!("checkpoint:{from}:{to}:{ignore_whitespace}");
                let runtime = service.runtime.clone();
                let thread = key.clone();
                self.activate(identity, move || {
                    let ignore = ignore_whitespace.then_some(true);
                    ActiveQuery::Checkpoint(if from == 0 {
                        runtime.query::<WsOrchestrationV2GetFullThreadDiffRpc>(
                            &thread.environment,
                            OrchestrationGetFullThreadDiffInput {
                                thread_id: thread.thread.clone(),
                                to_turn_count: to,
                                ignore_whitespace: ignore,
                            },
                            ResourcePolicy::default(),
                        )
                    } else {
                        runtime.query::<WsOrchestrationV2GetTurnDiffRpc>(
                            &thread.environment,
                            OrchestrationGetTurnDiffInput {
                                from_turn_count: from,
                                to_turn_count: to,
                                thread_id: thread.thread.clone(),
                                ignore_whitespace: ignore,
                            },
                            ResourcePolicy::default(),
                        )
                    })
                });
            }
            (DiffQuery::Git { .. }, Some(input)) if service.file_access(&key, Some(input)).allowed => {
                let identity = format!("git:{}", serde_json::to_string(input).unwrap_or_default());
                let lease = service.git_lease(&key, input.clone());
                if let Some(lease) = lease {
                    self.activate(identity, move || ActiveQuery::Git(lease));
                }
            }
            _ => self.active = None,
        }
        if let Some(active) = self.active.as_mut() {
            match &active.query {
                ActiveQuery::Checkpoint(lease) => {
                    let state = lease.resource().snapshot();
                    patch.pending = state.loading || (state.value.is_none() && state.error.is_none());
                    patch.answered = state.value.is_some();
                    patch.error = state.error.clone();
                    if let Some(value) = &state.value {
                        let rc = Rc::as_ptr(value) as usize;
                        patch.patch = Some(Self::shared_text(active, rc, &value.diff));
                    }
                }
                ActiveQuery::Git(lease) => {
                    let state = lease.resource().snapshot();
                    patch.pending = state.loading || (state.value.is_none() && state.error.is_none());
                    patch.answered = state.value.is_some();
                    patch.error = state.error.clone();
                    if let Some(value) = &state.value {
                        let kind = match resolved.git_scope {
                            GitScope::Unstaged => ReviewDiffPreviewSourceKind::WorkingTree,
                            GitScope::Commit => ReviewDiffPreviewSourceKind::Commit,
                            GitScope::Branch => ReviewDiffPreviewSourceKind::BranchRange,
                        };
                        patch.incomplete = value.incomplete == Some(true);
                        if let Some(source) = value.sources.iter().find(|s| s.kind == kind) {
                            let rc = std::ptr::from_ref(source) as usize;
                            patch.patch = Some(Self::shared_text(active, rc, &source.diff));
                            patch.truncated = source.truncated;
                            patch.source_has_files = source.files.as_ref().is_some_and(|f| !f.is_empty());
                            patch.source_stat = source.files.as_ref().map(|files| {
                                files.iter().fold(Default::default(), |total: super::files::DiffLineStat, file| {
                                    super::files::DiffLineStat {
                                        additions: total.additions + encoded(&file.additions),
                                        deletions: total.deletions + encoded(&file.deletions),
                                    }
                                })
                            });
                            comparison_head = source.head_ref.as_ref().map(|r| r.0.clone());
                            source_base = source.base_ref.as_ref().map(|r| r.0.clone());
                            if resolved.commit_sha.is_some()
                                && source.head_ref.as_ref().map(|r| &r.0) == resolved.commit_sha.as_ref()
                            {
                                commit_subject = source.commit_subject.as_ref().map(|s| s.0.clone());
                            }
                        }
                    }
                }
            }
        }
        let resolved = resolve_selection(&key, &selection, &turns, commit_subject.as_deref());
        let access = service.file_access(&key, git_input.as_ref());
        let renderable = patch.patch.clone().and_then(|p| self.renderable(&p));
        let view = resolve_body(thread_present, true, &resolved, &turns, &access, &patch, renderable.as_ref());
        let hide = service.state().hide_test_files.get();
        let (shown_patch, shown_files, file_keys, test_file_count) = match &view.body {
            super::logic::DiffBody::Files(files) => {
                let keys = files.file_keys(resolved.is_git());
                let is_shown = |path: &str| {
                    !hide
                        || Some(path) == resolved.file_path.as_deref()
                        || !super::changed_file_kind::is_test_changed_file(path)
                };
                let test_count = files
                    .files
                    .iter()
                    .filter(|f| super::changed_file_kind::is_test_changed_file(f.path()))
                    .count();
                let shown_text = match &self.shown {
                    Some((source, h, path, text))
                        if Rc::ptr_eq(source, files) && *h == hide && path == &resolved.file_path =>
                    {
                        text.clone()
                    }
                    _ => {
                        let text = files.joined(|f| is_shown(f.path()));
                        self.shown = Some((files.clone(), hide, resolved.file_path.clone(), text.clone()));
                        text
                    }
                };
                let shown: Vec<_> = files
                    .files
                    .iter()
                    .zip(&keys)
                    .filter(|(f, _)| is_shown(f.path()))
                    .map(|(f, key)| (key.clone(), f.path().to_owned()))
                    .collect();
                (Some(shown_text), Rc::new(shown), Rc::new(keys), test_count)
            }
            _ => (None, Rc::default(), Rc::default(), 0),
        };
        let defaults = service.default_collapsed(&file_keys);
        let collapsed = service
            .store()
            .borrow()
            .collapsed
            .get(&resolved.collapse_scope)
            .cloned()
            .unwrap_or(defaults);
        let can_refresh = resolved.is_git() && git_input.is_some();
        let comparison = (resolved.git_scope == GitScope::Branch && can_refresh && access.allowed).then(|| {
            (
                comparison_head.unwrap_or_else(|| "HEAD".into()),
                source_base.or_else(|| resolved.base_ref.clone()),
            )
        });
        let refreshing = can_refresh && patch.pending;
        DiffPanelSnapshot {
            thread_present,
            turns: Rc::new(turns),
            view,
            shown_patch,
            shown_files,
            file_keys,
            collapsed: Rc::new(collapsed),
            test_file_count,
            layout: settings.diff_layout.clone(),
            word_wrap: settings.word_wrap,
            timestamp_format: settings.timestamp_format.clone(),
            ignore_whitespace,
            hide_test_files: hide,
            file_tree_open: service.state().file_tree_open.get(),
            can_refresh,
            refreshing,
            comparison,
            resolved,
        }
    }
}

fn encoded(value: &t3_contracts::vcs::EncodedNumber) -> u64 {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_f64())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map_or(0, |n| n as u64)
}
