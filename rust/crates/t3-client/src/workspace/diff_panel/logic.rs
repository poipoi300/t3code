//! DiffPanel.tsx's decisions without the drawing: which turn or git scope is selected, its labels
//! and ids, which query it reads, and which state the panel's body shows.

use super::files::{DiffLineStat, PatchFiles, RenderablePatch};
use crate::{types::ThreadKey, view_logic::shell::diff_state::Selection};
use std::{collections::BTreeSet, rc::Rc};
use t3_contracts::{
    base_schemas::RunId, checkpoint_diff::CHECKPOINT_DIFF_TRUNCATED_MARKER,
    orchestration_v2::OrchestrationV2Checkpoint, orchestration_v2_id_table::IdOrIndex,
};

/// threadCheckpoints.ts `ThreadCheckpointSummary`, as far as the panel reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummary {
    pub run_id: RunId,
    pub turn_count: u64,
    pub completed_at: String,
}

/// `deriveThreadCheckpointSummaries` then DiffPanel.tsx's order: newest turn count first, then
/// latest completion. A run id the projection carries only as an id-table index has no id to
/// select by, so it is left out.
pub fn ordered_turn_summaries<'a>(
    checkpoints: impl IntoIterator<Item = &'a OrchestrationV2Checkpoint>,
) -> Vec<TurnSummary> {
    let mut summaries: Vec<_> = checkpoints
        .into_iter()
        .filter_map(
            |checkpoint| match (&checkpoint.run_id, checkpoint.app_run_ordinal) {
                (Some(IdOrIndex::Id(run_id)), Some(turn_count)) => Some(TurnSummary {
                    run_id: run_id.clone(),
                    turn_count,
                    completed_at: checkpoint.captured_at.0.clone(),
                }),
                _ => None,
            },
        )
        .collect();
    summaries.sort_by(|left, right| {
        right
            .turn_count
            .cmp(&left.turn_count)
            .then_with(|| right.completed_at.cmp(&left.completed_at))
    });
    summaries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitScope {
    Branch,
    Unstaged,
    Commit,
}

/// A label the view translates (`Turn {count}`, `Commit {sha}`) or shows as content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLabel {
    ToolCall,
    LatestTurn,
    /// `None` is the web's `Turn ?`.
    Turn(Option<u64>),
    Changes,
    Uncommitted,
    Commit,
    /// A commit's subject: content.
    CommitSubject(String),
    /// `Commit {sha}` with the short SHA.
    CommitSha(String),
}

/// The section title as AnnotatableCodeView.tsx stores it on a comment (`sectionTitle`), in
/// the web's English: it is data in the composer draft, not a label.
pub fn section_title_text(label: &DiffLabel) -> String {
    match label {
        DiffLabel::ToolCall => "Tool call".into(),
        DiffLabel::LatestTurn => "Latest turn".into(),
        DiffLabel::Turn(Some(count)) => format!("Turn {count}"),
        DiffLabel::Turn(None) => "Turn ?".into(),
        DiffLabel::Changes => "Changes".into(),
        DiffLabel::Uncommitted => "Uncommitted".into(),
        DiffLabel::Commit => "Commit".into(),
        DiffLabel::CommitSubject(subject) => subject.clone(),
        DiffLabel::CommitSha(sha) => format!("Commit {sha}"),
    }
}

/// What the panel reads for a selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffQuery {
    None,
    /// `getFullThreadDiff` when `from` is 0, else `getTurnDiff`.
    Checkpoint {
        from: u64,
        to: u64,
    },
    /// `review.getDiffPreview` for the thread's checkout.
    Git {
        scope: GitScope,
        base_ref: Option<String>,
        commit: Option<String>,
    },
    ToolCall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSelection {
    pub selection: Selection,
    pub selected_turn: Option<TurnSummary>,
    pub latest_turn: Option<TurnSummary>,
    pub git_scope: GitScope,
    pub commit_sha: Option<String>,
    pub base_ref: Option<String>,
    pub file_path: Option<String>,
    pub reveal_request_id: u64,
    pub scope_label: DiffLabel,
    pub section_title: DiffLabel,
    pub section_id: String,
    pub collapse_scope: String,
    /// The value of the scope menu's checked row.
    pub scope_value: String,
    /// The SHA the header links, for a commit selection.
    pub header_commit: Option<String>,
    pub query: DiffQuery,
}

impl ResolvedSelection {
    pub fn is_tool_call(&self) -> bool {
        matches!(self.selection, Selection::ToolCall { .. })
    }
    pub fn is_git(&self) -> bool {
        matches!(
            self.selection,
            Selection::Branch { .. } | Selection::Unstaged | Selection::Commit { .. }
        )
    }
    pub fn turn_selected(&self) -> bool {
        matches!(self.selection, Selection::Turn { .. })
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// DiffPanel.tsx's selection block, for one thread. `commit_subject` is the preview's subject
/// once the commit's diff answered.
pub fn resolve_selection(
    key: &ThreadKey,
    selection: &Selection,
    turns: &[TurnSummary],
    commit_subject: Option<&str>,
) -> ResolvedSelection {
    let selected_run = match selection {
        Selection::Turn { turn_id, .. } => Some(turn_id),
        _ => None,
    };
    let git_scope = match selection {
        Selection::Unstaged => GitScope::Unstaged,
        Selection::Commit { .. } => GitScope::Commit,
        _ => GitScope::Branch,
    };
    let commit_sha = match selection {
        Selection::Commit { sha, .. } => Some(sha.clone()),
        _ => None,
    };
    let base_ref = match selection {
        Selection::Branch { base_ref } | Selection::Commit { base_ref, .. } => base_ref.clone(),
        _ => None,
    };
    let (file_path, reveal_request_id) = match selection {
        Selection::Turn {
            file_path,
            reveal_request_id,
            ..
        } => (file_path.clone(), *reveal_request_id),
        _ => (None, 0),
    };
    let selected_turn = selected_run.and_then(|run| {
        turns
            .iter()
            .find(|t| &t.run_id == run)
            .or(turns.first())
            .cloned()
    });
    let latest_turn = turns.first().cloned();
    let tool_call = matches!(selection, Selection::ToolCall { .. });
    let git_label = match (&commit_sha, git_scope) {
        (Some(sha), _) => commit_subject.map_or_else(
            || DiffLabel::CommitSha(short_sha(sha)),
            |s| DiffLabel::CommitSubject(s.into()),
        ),
        (None, GitScope::Unstaged) => DiffLabel::Uncommitted,
        _ => DiffLabel::Changes,
    };
    let same_as_latest =
        selected_turn.as_ref().map(|t| &t.run_id) == latest_turn.as_ref().map(|t| &t.run_id);
    let scope_label = if tool_call {
        DiffLabel::ToolCall
    } else if selected_run.is_none() {
        match &commit_sha {
            Some(_) => {
                commit_subject.map_or(DiffLabel::Commit, |s| DiffLabel::CommitSubject(s.into()))
            }
            None => git_label.clone(),
        }
    } else if same_as_latest {
        DiffLabel::LatestTurn
    } else {
        DiffLabel::Turn(selected_turn.as_ref().map(|t| t.turn_count))
    };
    let section_title = if tool_call {
        DiffLabel::ToolCall
    } else if let Some(turn) = &selected_turn {
        DiffLabel::Turn(Some(turn.turn_count))
    } else {
        git_label
    };
    let section_id = match selection {
        Selection::ToolCall {
            thread_id, item_id, ..
        } => format!("tool-call:{}:{}", thread_id.0, item_id.0),
        _ => match (&selected_turn, &commit_sha) {
            (Some(turn), _) => format!("turn:{}", turn.run_id.0),
            (None, Some(sha)) => format!("commit:{sha}"),
            (None, None) if git_scope == GitScope::Unstaged => "unstaged".into(),
            (None, None) => "branch".into(),
        },
    };
    let commit_value = commit_sha
        .as_ref()
        .map_or_else(String::new, |sha| format!("commit:{sha}"));
    let scope_value = if tool_call {
        String::new()
    } else if selected_run.is_none() {
        if commit_value.is_empty() {
            match git_scope {
                GitScope::Unstaged => "unstaged".into(),
                _ => "branch".into(),
            }
        } else {
            commit_value
        }
    } else if same_as_latest {
        "latest".into()
    } else {
        selected_turn
            .as_ref()
            .map_or_else(String::new, |t| format!("turn:{}", t.run_id.0))
    };
    let query = if tool_call {
        DiffQuery::ToolCall
    } else if let Some(turn) = &selected_turn {
        DiffQuery::Checkpoint {
            from: turn.turn_count.saturating_sub(1),
            to: turn.turn_count,
        }
    } else if selected_run.is_some() {
        DiffQuery::None
    } else {
        DiffQuery::Git {
            scope: git_scope,
            base_ref: base_ref.clone(),
            commit: commit_sha.clone(),
        }
    };
    ResolvedSelection {
        selection: selection.clone(),
        header_commit: selected_run.is_none().then(|| commit_sha.clone()).flatten(),
        collapse_scope: format!("{}:{}:{section_id}", key.environment.0, key.thread.0),
        selected_turn,
        latest_turn,
        git_scope,
        commit_sha,
        base_ref,
        file_path,
        reveal_request_id,
        scope_label,
        section_title,
        section_id,
        scope_value,
        query,
    }
}

/// What the scope menu's `onValueChange` does with a row's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeAction {
    Git { unstaged: bool },
    Commit(String),
    Turn(RunId),
    Nothing,
}

/// `selectScopeValue`.
pub fn scope_action(
    value: &str,
    resolved: &ResolvedSelection,
    turns: &[TurnSummary],
) -> ScopeAction {
    let commit_value = resolved
        .commit_sha
        .as_ref()
        .map(|sha| format!("commit:{sha}"));
    if value == "unstaged" || value == "branch" {
        ScopeAction::Git {
            unstaged: value == "unstaged",
        }
    } else if commit_value.as_deref() == Some(value) {
        ScopeAction::Git { unstaged: false }
    } else if let Some(sha) = value.strip_prefix("commit:") {
        ScopeAction::Commit(sha.into())
    } else if value == "latest" {
        resolved
            .latest_turn
            .as_ref()
            .map_or(ScopeAction::Nothing, |t| {
                ScopeAction::Turn(t.run_id.clone())
            })
    } else {
        turns
            .iter()
            .find(|t| format!("turn:{}", t.run_id.0) == value)
            .map_or(ScopeAction::Nothing, |t| {
                ScopeAction::Turn(t.run_id.clone())
            })
    }
}

/// The state of the selected patch's query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PatchState {
    pub pending: bool,
    pub answered: bool,
    pub error: Option<String>,
    pub patch: Option<Rc<str>>,
    /// The git source was cut at the size limit.
    pub truncated: bool,
    /// The preview lists tracked changes only so far.
    pub incomplete: bool,
    /// The git source's own per-file totals, over every change.
    pub source_stat: Option<DiffLineStat>,
    pub source_has_files: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAccess {
    pub allowed: bool,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadingLabel {
    ToolCall,
    Checkpoint,
    Uncommitted,
    Commit,
    Changes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffBody {
    /// "Select a thread to inspect turn diffs."
    NoThread,
    /// "Turn diffs are unavailable because this project is not a git repository."
    NotGitRepo,
    /// "No completed turns yet."
    NoTurns,
    /// "Checking file access..."
    CheckingAccess,
    /// The access error, else "This connection cannot read local diffs."
    AccessDenied(Option<String>),
    Loading(LoadingLabel),
    /// "No net changes in this selection." or "No patch available for this selection."
    Empty {
        no_net_changes: bool,
    },
    /// "Unsupported diff format. Showing raw patch." over the text.
    Raw(Rc<str>),
    Files(Rc<PatchFiles>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    /// "Showing tracked changes. Checking untracked files..."
    CheckingUntracked,
    /// "Showing tracked changes. Untracked changes could not be loaded. Refresh to retry."
    UntrackedFailed,
    /// "This preview exceeds the size limit. Changes shown are incomplete." and, with
    /// `totals_complete`, " Totals include all changes."
    Truncated { totals_complete: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBodyView {
    pub body: DiffBody,
    pub banners: Vec<Banner>,
    /// Shown above the body when the patch failed and nothing renders.
    pub error: Option<String>,
    pub stat: Option<DiffLineStat>,
}

/// The `DiffPanelShell` children: the first matching state, as the TSX's ternary chain.
pub fn resolve_body(
    thread_present: bool,
    is_git_repo: bool,
    resolved: &ResolvedSelection,
    turns: &[TurnSummary],
    access: &FileAccess,
    state: &PatchState,
    renderable: Option<&RenderablePatch>,
) -> DiffBodyView {
    let view = |body| DiffBodyView {
        body,
        banners: Vec::new(),
        error: None,
        stat: None,
    };
    if !thread_present {
        return view(DiffBody::NoThread);
    }
    if !is_git_repo && !resolved.is_tool_call() {
        return view(DiffBody::NotGitRepo);
    }
    if resolved.turn_selected() && turns.is_empty() {
        return view(DiffBody::NoTurns);
    }
    if resolved.is_git() && !access.allowed {
        return view(if access.pending {
            DiffBody::CheckingAccess
        } else {
            DiffBody::AccessDenied(access.error.clone())
        });
    }
    let git = resolved.is_git();
    let mut banners = Vec::new();
    if state.incomplete {
        banners.push(if state.error.is_some() {
            Banner::UntrackedFailed
        } else {
            Banner::CheckingUntracked
        });
    }
    let truncated = (git && state.truncated)
        || (resolved.selected_turn.is_some()
            && state
                .patch
                .as_ref()
                .is_some_and(|p| p.ends_with(CHECKPOINT_DIFF_TRUNCATED_MARKER)));
    if truncated {
        banners.push(Banner::Truncated {
            totals_complete: git && state.source_has_files && !state.incomplete,
        });
    }
    let error = state.error.clone().filter(|_| renderable.is_none());
    let loading =
        state.pending && (!state.answered || (git && state.incomplete && !state.source_has_files));
    let body = match renderable {
        Some(RenderablePatch::Files(files)) => DiffBody::Files(files.clone()),
        Some(RenderablePatch::Raw(text)) => DiffBody::Raw(text.clone()),
        None if loading => DiffBody::Loading(if resolved.is_tool_call() {
            LoadingLabel::ToolCall
        } else if resolved.selected_turn.is_some() {
            LoadingLabel::Checkpoint
        } else {
            match resolved.git_scope {
                GitScope::Unstaged => LoadingLabel::Uncommitted,
                GitScope::Commit => LoadingLabel::Commit,
                GitScope::Branch => LoadingLabel::Changes,
            }
        }),
        None => DiffBody::Empty {
            no_net_changes: state.patch.as_ref().is_some_and(|p| p.trim().is_empty())
                && !state.incomplete,
        },
    };
    let stat = match &body {
        DiffBody::Files(files) => Some(
            state
                .source_stat
                .filter(|_| git)
                .unwrap_or_else(|| PatchFiles::line_stat(&files.files)),
        ),
        _ => state.source_stat.filter(|_| git && state.source_has_files),
    };
    DiffBodyView {
        body,
        banners,
        error,
        stat,
    }
}

/// `areAllDiffFilesCollapsed`.
pub fn all_files_collapsed(keys: &[String], collapsed: &BTreeSet<String>) -> bool {
    !keys.is_empty() && keys.iter().all(|key| collapsed.contains(key))
}

/// `toggleAllDiffFiles`.
pub fn toggle_all_files(keys: &[String], collapsed: &BTreeSet<String>) -> BTreeSet<String> {
    if all_files_collapsed(keys, collapsed) {
        BTreeSet::new()
    } else {
        keys.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        types::{EnvironmentId, ThreadId},
        view_logic::shell::diff_state::DiffStore,
    };
    use t3_contracts::base_schemas::TurnItemId;

    fn key() -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId("env".into()),
            thread: ThreadId("thread".into()),
        }
    }
    fn turn(id: &str, count: u64, at: &str) -> TurnSummary {
        TurnSummary {
            run_id: RunId(id.into()),
            turn_count: count,
            completed_at: at.into(),
        }
    }
    fn turns() -> Vec<TurnSummary> {
        vec![
            turn("r3", 3, "2026-01-03"),
            turn("r2", 2, "2026-01-02"),
            turn("r1", 1, "2026-01-01"),
        ]
    }
    fn body_of(
        thread: bool,
        repo: bool,
        resolved: &ResolvedSelection,
        turns: &[TurnSummary],
        access: &FileAccess,
        state: &PatchState,
    ) -> DiffBodyView {
        let renderable = state
            .patch
            .as_ref()
            .and_then(super::super::files::renderable_patch);
        resolve_body(
            thread,
            repo,
            resolved,
            turns,
            access,
            state,
            renderable.as_ref(),
        )
    }
    fn access() -> FileAccess {
        FileAccess {
            allowed: true,
            pending: false,
            error: None,
        }
    }

    #[test]
    fn a_tool_call_selection_is_independent_of_checkpoints_and_returns_to_git_changes() {
        let mut store = DiffStore::default();
        let target = Selection::ToolCall {
            thread_id: t3_contracts::base_schemas::ThreadId("child".into()),
            item_id: TurnItemId("edit".into()),
            revision: "1".into(),
        };
        store.by_thread.insert(key(), target.clone());
        store.reconcile_turn(&key(), &[]);
        assert_eq!(store.by_thread.get(&key()), Some(&target));
        store.select_git(key(), false);
        assert_eq!(
            store.by_thread.get(&key()),
            Some(&Selection::Branch { base_ref: None })
        );
        let resolved = resolve_selection(&key(), &target, &turns(), None);
        assert_eq!(resolved.scope_label, DiffLabel::ToolCall);
        assert_eq!(resolved.section_id, "tool-call:child:edit");
        assert_eq!(resolved.scope_value, "");
    }

    #[test]
    fn fold_choices_survive_a_consumer_returning_with_fresh_defaults() {
        let mut store = DiffStore::default();
        let scope = "environment-1:thread-1:branch".to_owned();
        let defaults: BTreeSet<String> = ["src/app.ts".into(), "src/index.ts".into()].into();
        store.update_collapsed(scope.clone(), &defaults, |current| {
            let mut next = current.clone();
            next.remove("src/app.ts");
            next
        });
        assert_eq!(store.collapsed[&scope], ["src/index.ts".to_owned()].into());
        store.update_collapsed(scope.clone(), &BTreeSet::new(), |current| {
            let mut next = current.clone();
            next.remove("src/index.ts");
            next
        });
        store.update_collapsed(scope.clone(), &defaults, Clone::clone);
        assert!(store.collapsed[&scope].is_empty());
    }

    #[test]
    fn turn_selection_labels_ranges_and_menu_values() {
        let selection = Selection::Turn {
            turn_id: RunId("r2".into()),
            file_path: Some("src/a.ts".into()),
            reveal_request_id: 4,
        };
        let resolved = resolve_selection(&key(), &selection, &turns(), None);
        assert_eq!(resolved.scope_label, DiffLabel::Turn(Some(2)));
        assert_eq!(resolved.section_title, DiffLabel::Turn(Some(2)));
        assert_eq!(resolved.query, DiffQuery::Checkpoint { from: 1, to: 2 });
        assert_eq!(resolved.scope_value, "turn:r2");
        assert_eq!(resolved.collapse_scope, "env:thread:turn:r2");
        assert_eq!(
            (resolved.file_path.as_deref(), resolved.reveal_request_id),
            (Some("src/a.ts"), 4)
        );
        let latest = resolve_selection(
            &key(),
            &Selection::Turn {
                turn_id: RunId("gone".into()),
                file_path: None,
                reveal_request_id: 1,
            },
            &turns(),
            None,
        );
        assert_eq!(latest.scope_label, DiffLabel::LatestTurn);
        assert_eq!(latest.query, DiffQuery::Checkpoint { from: 2, to: 3 });
        assert_eq!(latest.scope_value, "latest");
        let first = resolve_selection(&key(), &selection, &[turn("r1", 1, "x")], None);
        assert_eq!(first.query, DiffQuery::Checkpoint { from: 0, to: 1 });
    }

    #[test]
    fn git_and_commit_labels_follow_the_preview_subject() {
        let commit = Selection::Commit {
            sha: "abcdef1234".into(),
            base_ref: Some("main".into()),
        };
        let resolved = resolve_selection(&key(), &commit, &turns(), None);
        assert_eq!(resolved.scope_label, DiffLabel::Commit);
        assert_eq!(
            resolved.section_title,
            DiffLabel::CommitSha("abcdef1".into())
        );
        assert_eq!(resolved.header_commit.as_deref(), Some("abcdef1234"));
        assert_eq!(resolved.scope_value, "commit:abcdef1234");
        let answered = resolve_selection(&key(), &commit, &turns(), Some("Fix it"));
        assert_eq!(
            answered.scope_label,
            DiffLabel::CommitSubject("Fix it".into())
        );
        assert_eq!(
            resolve_selection(&key(), &Selection::Unstaged, &[], None).scope_label,
            DiffLabel::Uncommitted
        );
        let branch = resolve_selection(&key(), &Selection::default(), &[], None);
        assert_eq!(branch.scope_label, DiffLabel::Changes);
        assert_eq!(branch.section_id, "branch");
        assert_eq!(
            branch.query,
            DiffQuery::Git {
                scope: GitScope::Branch,
                base_ref: None,
                commit: None
            }
        );
    }

    #[test]
    fn scope_menu_values_map_to_store_actions() {
        let resolved = resolve_selection(
            &key(),
            &Selection::Commit {
                sha: "abc".into(),
                base_ref: None,
            },
            &turns(),
            None,
        );
        assert_eq!(
            scope_action("unstaged", &resolved, &turns()),
            ScopeAction::Git { unstaged: true }
        );
        assert_eq!(
            scope_action("commit:abc", &resolved, &turns()),
            ScopeAction::Git { unstaged: false }
        );
        assert_eq!(
            scope_action("commit:def", &resolved, &turns()),
            ScopeAction::Commit("def".into())
        );
        assert_eq!(
            scope_action("latest", &resolved, &turns()),
            ScopeAction::Turn(RunId("r3".into()))
        );
        assert_eq!(
            scope_action("turn:r1", &resolved, &turns()),
            ScopeAction::Turn(RunId("r1".into()))
        );
        assert_eq!(
            scope_action("turn:x", &resolved, &turns()),
            ScopeAction::Nothing
        );
    }

    #[test]
    fn body_states_follow_the_ternary_chain() {
        let branch = resolve_selection(&key(), &Selection::default(), &[], None);
        let state = PatchState::default();
        assert_eq!(
            body_of(false, true, &branch, &[], &access(), &state).body,
            DiffBody::NoThread
        );
        assert_eq!(
            body_of(true, false, &branch, &[], &access(), &state).body,
            DiffBody::NotGitRepo
        );
        let turn = resolve_selection(
            &key(),
            &Selection::Turn {
                turn_id: RunId("r".into()),
                file_path: None,
                reveal_request_id: 1,
            },
            &[],
            None,
        );
        assert_eq!(
            body_of(true, true, &turn, &[], &access(), &state).body,
            DiffBody::NoTurns
        );
        let pending = FileAccess {
            allowed: false,
            pending: true,
            error: None,
        };
        assert_eq!(
            body_of(true, true, &branch, &[], &pending, &state).body,
            DiffBody::CheckingAccess
        );
        let denied = FileAccess {
            allowed: false,
            pending: false,
            error: None,
        };
        assert_eq!(
            body_of(true, true, &branch, &[], &denied, &state).body,
            DiffBody::AccessDenied(None)
        );
        let loading = PatchState {
            pending: true,
            ..Default::default()
        };
        assert_eq!(
            body_of(true, true, &branch, &[], &access(), &loading).body,
            DiffBody::Loading(LoadingLabel::Changes)
        );
        let empty = PatchState {
            answered: true,
            patch: Some(Rc::from("")),
            ..Default::default()
        };
        assert_eq!(
            body_of(true, true, &branch, &[], &access(), &empty).body,
            DiffBody::Empty {
                no_net_changes: true
            }
        );
        let failed = PatchState {
            answered: true,
            error: Some("boom".into()),
            ..Default::default()
        };
        let view = body_of(true, true, &branch, &[], &access(), &failed);
        assert_eq!(
            view.body,
            DiffBody::Empty {
                no_net_changes: false
            }
        );
        assert_eq!(view.error.as_deref(), Some("boom"));
    }

    #[test]
    fn truncation_banners_and_totals() {
        let selection = Selection::Turn {
            turn_id: RunId("r3".into()),
            file_path: None,
            reveal_request_id: 1,
        };
        let resolved = resolve_selection(&key(), &selection, &turns(), None);
        let patch: Rc<str> = Rc::from(format!(
            "diff --git a/a b/a\n@@ -1 +1 @@\n-a\n+b{CHECKPOINT_DIFF_TRUNCATED_MARKER}"
        ));
        let state = PatchState {
            answered: true,
            patch: Some(patch),
            ..Default::default()
        };
        let view = body_of(true, true, &resolved, &turns(), &access(), &state);
        assert!(matches!(view.body, DiffBody::Files(_)));
        assert_eq!(
            view.banners,
            vec![Banner::Truncated {
                totals_complete: false
            }]
        );
        assert_eq!(
            view.stat,
            Some(DiffLineStat {
                additions: 1,
                deletions: 1
            })
        );
        let branch = resolve_selection(&key(), &Selection::default(), &[], None);
        let git = PatchState {
            answered: true,
            patch: Some(Rc::from("diff --git a/a b/a\n@@ -1 +1 @@\n-a\n+b\n")),
            truncated: true,
            source_has_files: true,
            source_stat: Some(DiffLineStat {
                additions: 40,
                deletions: 9,
            }),
            ..Default::default()
        };
        let view = body_of(true, true, &branch, &[], &access(), &git);
        assert_eq!(
            view.banners,
            vec![Banner::Truncated {
                totals_complete: true
            }]
        );
        assert_eq!(view.stat.map(|s| s.additions), Some(40));
    }

    #[test]
    fn collapse_all_toggles_between_every_file_and_none() {
        let keys = vec!["a".to_owned(), "b".to_owned()];
        let none = BTreeSet::new();
        assert!(!all_files_collapsed(&keys, &none));
        let all = toggle_all_files(&keys, &none);
        assert!(all_files_collapsed(&keys, &all));
        assert!(toggle_all_files(&keys, &all).is_empty());
        assert!(!all_files_collapsed(&[], &all));
    }
}
