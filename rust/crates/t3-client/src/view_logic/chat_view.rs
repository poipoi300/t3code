//! Pure decisions from `apps/web/src/components/ChatView.logic.ts`.
use super::context_references::{kind_scoped_id, remove_reference, strip_references};
use crate::view_logic::JsTrim;
#[path = "chat_view/rewind.rs"]
mod rewind;
pub use rewind::*;
#[path = "chat_view/background.rs"]
mod background;
pub use background::derive_thread_pending_background_tasks;
#[path = "chat_view/attachments.rs"]
mod attachments;
use crate::{ClientError, types::ThreadKey};
pub use attachments::*;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
};
use t3_contracts::{
    base_schemas::{EnvironmentId, MessageId, RunId, ThreadId},
    chat_attachment::{ChatAttachment, PROVIDER_SEND_TURN_MAX_ATTACHMENTS},
    ipc::PreviewAnnotationPayloadSchema,
    model_selection::ModelSelection,
    orchestration_v2::{
        OrchestrationV2ProjectedTurnItem, OrchestrationV2RunStatus, OrchestrationV2TurnItem,
    },
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    server::{ServerProvider, ServerProviderAuthStatus, ServerProviderState},
    thread_pull_request::ThreadLinkedPullRequest,
    worktree_setup::{
        WorktreeSetupPhase, WorktreeSetupSnapshot, WorktreeSetupStageId, WorktreeSetupStageStatus,
    },
};

pub const LAST_INVOKED_SCRIPT_BY_PROJECT_KEY: &str = "t3code:last-invoked-script-by-project";
pub const MAX_HIDDEN_MOUNTED_TERMINAL_THREADS: usize = 10;
pub const ENVIRONMENT_RECONNECT_WARNING_GRACE_MS: u64 = 2_000;
pub type LastInvokedScriptByProject = BTreeMap<t3_contracts::base_schemas::ProjectId, String>;
pub use crate::thread_views::shelves::is_woke_unseen as is_thread_woke_unseen;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RightPanelSurface {
    Preview {
        resource_id: Option<String>,
    },
    Device {
        host_id: String,
        device_id: String,
    },
    PullRequest {
        project_id: t3_contracts::base_schemas::ProjectId,
        repository: String,
        number: u64,
    },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiniPlayerSource {
    Browser { tab_id: String },
    Device { host_id: String, device_id: String },
}

pub fn agent_controlled_browser_close_confirmation(
    surfaces: &[RightPanelSurface],
    agent_controlled_tab_ids: &BTreeSet<String>,
) -> Option<String> {
    let count = surfaces.iter().filter(|surface| matches!(surface,
        RightPanelSurface::Preview { resource_id: Some(id) } if agent_controlled_tab_ids.contains(id))).count();
    match count {
        0 => None,
        1 => Some("Close browser while the agent is using it?\nThe agent is actively controlling this browser. Closing it may interrupt the current browser action.".into()),
        _ => Some(format!("Close {count} browsers while the agent is using them?\nThe agent is actively controlling these browsers. Closing them may interrupt the current browser actions.")),
    }
}

pub fn should_render_preview_mini_player(
    source: Option<&MiniPlayerSource>,
    surface: Option<&RightPanelSurface>,
) -> bool {
    match source {
        None => false,
        Some(MiniPlayerSource::Browser { tab_id }) => !matches!(surface,
            Some(RightPanelSurface::Preview { resource_id: Some(id) }) if id == tab_id),
        Some(MiniPlayerSource::Device { host_id, device_id }) => !matches!(surface,
            Some(RightPanelSurface::Device { host_id: host, device_id: device }) if host == host_id && device == device_id),
    }
}

pub fn should_open_proactive_pull_request(
    previous_target_key: Option<&str>,
    target_key: Option<&str>,
) -> bool {
    target_key.is_some() && target_key != previous_target_key
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProactivePanelObservation {
    pub thread_key: String,
    pub running_turn_id: Option<Option<RunId>>,
    pub target_key: Option<Option<String>>,
    pub user_action_turn_id: Option<RunId>,
    pub user_action_revision: u64,
}

pub fn observe_proactive_panel_user_choice(
    previous: Option<&ProactivePanelObservation>,
    thread_key: String,
    running_turn_id: Option<RunId>,
    user_action_revision: u64,
) -> ProactivePanelObservation {
    let same = previous.filter(|p| p.thread_key == thread_key);
    let new_turn =
        same.is_some_and(|p| running_turn_id.is_some() && running_turn_id != p.user_action_turn_id);
    ProactivePanelObservation {
        thread_key,
        running_turn_id: same.and_then(|p| p.running_turn_id.clone()),
        target_key: same.and_then(|p| p.target_key.clone()),
        user_action_turn_id: running_turn_id
            .or_else(|| same.and_then(|p| p.user_action_turn_id.clone())),
        user_action_revision: if new_turn || same.is_none() {
            user_action_revision
        } else {
            same.map_or(user_action_revision, |p| p.user_action_revision)
        },
    }
}

pub fn should_retarget_thread_pull_request_panel(
    previous: Option<&ThreadLinkedPullRequest>,
    current: Option<&ThreadLinkedPullRequest>,
    surface: Option<&RightPanelSurface>,
) -> bool {
    let (
        Some(previous),
        Some(current),
        Some(RightPanelSurface::PullRequest {
            project_id,
            repository,
            number,
        }),
    ) = (previous, current, surface)
    else {
        return false;
    };
    (previous.project_id != current.project_id
        || previous.repository.0.to_lowercase() != current.repository.0.to_lowercase()
        || previous.number != current.number)
        && *project_id == previous.project_id
        && repository.to_lowercase() == previous.repository.0.to_lowercase()
        && *number == previous.number
}

pub fn should_open_proactive_turn_diff(
    previous_running_turn_id: Option<Option<&RunId>>,
    running_turn_id: Option<&RunId>,
    settled_turn_id: Option<&RunId>,
    turn_completed: bool,
) -> bool {
    running_turn_id.is_none()
        && turn_completed
        && settled_turn_id.is_some()
        && previous_running_turn_id.is_none_or(|previous| previous == settled_turn_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProactiveTurnDiffAction {
    Defer,
    Ignore,
    Open,
}

#[derive(Debug, Clone, Copy)]
pub struct CheckpointDiff<'a> {
    pub status: &'a str,
    pub loaded_file_count: usize,
    pub total_file_count: usize,
    pub additions: u64,
    pub deletions: u64,
}

pub fn resolve_proactive_turn_diff_action(
    checkpoint: Option<CheckpointDiff<'_>>,
    is_git_repo: Option<bool>,
) -> ProactiveTurnDiffAction {
    let Some(checkpoint) = checkpoint.filter(|c| c.status != "missing") else {
        return ProactiveTurnDiffAction::Defer;
    };
    let Some(is_git_repo) = is_git_repo else {
        return ProactiveTurnDiffAction::Defer;
    };
    if !is_git_repo || checkpoint.status != "ready" || checkpoint.loaded_file_count == 0 {
        return ProactiveTurnDiffAction::Ignore;
    }
    if checkpoint.total_file_count >= 3
        || checkpoint.additions.saturating_add(checkpoint.deletions) >= 50
    {
        ProactiveTurnDiffAction::Open
    } else {
        ProactiveTurnDiffAction::Ignore
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SubmissionIntent {
    #[default]
    Foreground,
    Background,
}

pub fn should_dock_draft_hero_for_submission(
    is_draft_hero_state: bool,
    active_thread_key: Option<&str>,
    intent: SubmissionIntent,
) -> bool {
    intent == SubmissionIntent::Foreground && is_draft_hero_state && active_thread_key.is_some()
}

#[derive(Debug, Clone, Copy)]
pub struct TimelineToolActivity<'a> {
    pub is_work: bool,
    pub run_id: Option<&'a RunId>,
    pub is_tool_tone: bool,
    pub has_item_type: bool,
    pub has_request_kind: bool,
    pub command: Option<&'a str>,
}

pub fn should_release_timeline_anchor_for_tool_activity(
    anchor_message_id: Option<&MessageId>,
    live_follow_enabled: bool,
    running_turn_id: Option<&RunId>,
    entries: &[TimelineToolActivity<'_>],
) -> bool {
    anchor_message_id.is_some()
        && live_follow_enabled
        && running_turn_id.is_some()
        && entries.iter().any(|e| {
            e.is_work
                && e.run_id == running_turn_id
                && (e.is_tool_tone
                    || e.has_item_type
                    || e.has_request_kind
                    || e.command.is_some_and(|s| !s.js_trim().is_empty()))
        })
}

pub fn worktree_setup_agent_started(snapshot: &WorktreeSetupSnapshot) -> bool {
    snapshot
        .stages
        .iter()
        .any(|s| s.id == WorktreeSetupStageId::Agent && s.status == WorktreeSetupStageStatus::Done)
}

pub fn resolve_visible_worktree_setup<'a>(
    live: Option<&'a WorktreeSetupSnapshot>,
    recorded: Option<&'a WorktreeSetupSnapshot>,
    turn_started: bool,
    follow_up_sent: bool,
) -> Option<&'a WorktreeSetupSnapshot> {
    let snapshot = newest_setup(live, recorded)?;
    if snapshot.phase == WorktreeSetupPhase::Running {
        return Some(snapshot);
    }
    if follow_up_sent {
        return None;
    }
    if snapshot.phase != WorktreeSetupPhase::Done
        || !turn_started
        || snapshot
            .stages
            .iter()
            .any(|s| s.status == WorktreeSetupStageStatus::Failed)
    {
        Some(snapshot)
    } else {
        None
    }
}

fn newest_setup<'a>(
    latest: Option<&'a WorktreeSetupSnapshot>,
    held: Option<&'a WorktreeSetupSnapshot>,
) -> Option<&'a WorktreeSetupSnapshot> {
    match (latest, held) {
        (Some(latest), Some(held)) if latest.sequence < held.sequence => Some(held),
        (Some(latest), _) => Some(latest),
        (_, held) => held,
    }
}

pub fn resolve_worktree_setup_progress<'a>(
    thread_id: &ThreadId,
    local_preparing: bool,
    run_status: Option<&OrchestrationV2RunStatus>,
    latest: Option<&'a WorktreeSetupSnapshot>,
    held: Option<&'a WorktreeSetupSnapshot>,
) -> (Option<&'a WorktreeSetupSnapshot>, bool) {
    let snapshot = newest_setup(
        latest.filter(|s| &s.thread_id == thread_id),
        held.filter(|s| &s.thread_id == thread_id),
    );
    let preparing = local_preparing
        || run_status == Some(&OrchestrationV2RunStatus::Preparing)
        || snapshot.is_some_and(|s| {
            s.phase == WorktreeSetupPhase::Running && !worktree_setup_agent_started(s)
        });
    (snapshot, preparing)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DraftHeroState {
    pub is_local_draft_thread: bool,
    pub has_timeline_entries: bool,
    pub is_working: bool,
    pub dock_requested: bool,
    pub background_submission_pending: bool,
    pub has_worktree_setup_card: bool,
}

pub fn resolve_draft_hero_state(input: DraftHeroState) -> bool {
    !input.has_worktree_setup_card
        && (input.background_submission_pending
            || (input.is_local_draft_thread
                && !input.has_timeline_entries
                && !input.is_working
                && !input.dock_requested))
}

#[derive(Debug)]
pub struct ThreadTimelineMemory<T> {
    entries: BTreeMap<ThreadKey, Rc<[T]>>,
    order: VecDeque<ThreadKey>,
}

impl<T> Default for ThreadTimelineMemory<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<T> ThreadTimelineMemory<T> {
    pub fn remember_ready(&mut self, key: Option<ThreadKey>, entries: Rc<[T]>) {
        let Some(key) = key.filter(|_| !entries.is_empty()) else {
            return;
        };
        self.entries.insert(key.clone(), entries);
        self.order.retain(|old| old != &key);
        self.order.push_back(key);
        while self.order.len() > 16 {
            if let Some(key) = self.order.pop_front() {
                self.entries.remove(&key);
            }
        }
    }
    pub fn peek(&self, key: Option<&ThreadKey>) -> Option<Rc<[T]>> {
        self.entries.get(key?).cloned()
    }
    pub fn reset(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
    pub fn resolve(
        &self,
        loading: bool,
        key: Option<&ThreadKey>,
        next: Rc<[T]>,
        remembered: Option<Rc<[T]>>,
    ) -> Rc<[T]> {
        if !next.is_empty() {
            return next;
        }
        if loading
            && let Some(entries) = remembered
                .or_else(|| self.peek(key))
                .filter(|entries| !entries.is_empty())
        {
            return entries;
        }
        next
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadMetadataUpdate {
    pub model_selection: Option<ModelSelection>,
    pub branch: Option<String>,
    pub clear_worktree_path: bool,
}

pub fn resolve_thread_metadata_update_for_next_turn(
    current_model: &ModelSelection,
    next_model: Option<&ModelSelection>,
    current_branch: Option<&str>,
    next_branch: Option<&str>,
) -> Option<ThreadMetadataUpdate> {
    let model = next_model.filter(|next| *next != current_model);
    let branch = next_branch.filter(|next| Some(*next) != current_branch);
    if model.is_none() && branch.is_none() {
        return None;
    }
    Some(ThreadMetadataUpdate {
        model_selection: model.cloned(),
        branch: branch.map(str::to_owned),
        clear_worktree_path: branch.is_some(),
    })
}

#[derive(Debug, Clone)]
pub struct ProviderInstanceEntry<'a> {
    pub snapshot: &'a ServerProvider,
    pub is_available: bool,
    pub continuation_group_key: Option<&'a str>,
}

#[derive(Debug)]
pub struct ComposerProviderSelection<'a> {
    pub selected_provider_entry: Option<&'a ProviderInstanceEntry<'a>>,
    pub requested_instance_id: Option<&'a ProviderInstanceId>,
    pub requested_driver_kind: ProviderDriverKind,
    pub locked_continuation_group_key: Option<&'a str>,
    pub unavailable_provider_instance_id: Option<&'a ProviderInstanceId>,
}

pub fn resolve_composer_provider_selection<'a>(
    entries: &'a [ProviderInstanceEntry<'a>],
    candidate_ids: &[Option<&'a ProviderInstanceId>],
    locked_provider: Option<&ProviderDriverKind>,
    locked_instance_id: Option<&'a ProviderInstanceId>,
) -> ComposerProviderSelection<'a> {
    let requested = candidate_ids
        .iter()
        .flatten()
        .copied()
        .find(|id| id.0 != "t3code_no_provider");
    let requested_driver = locked_provider
        .cloned()
        .or_else(|| {
            entries
                .iter()
                .find(|e| Some(&e.snapshot.instance_id) == requested)
                .map(|e| e.snapshot.driver.clone())
        })
        .or_else(|| entries.first().map(|e| e.snapshot.driver.clone()))
        .unwrap_or_else(|| ProviderDriverKind("unconfigured".into()));
    let group = locked_provider.and_then(|_| {
        entries
            .iter()
            .find(|e| Some(&e.snapshot.instance_id) == locked_instance_id)
            .and_then(|e| e.continuation_group_key)
    });
    let exact = locked_provider.is_some_and(|p| p.0 == "antigravity")
        && locked_instance_id.is_some()
        && group.is_none();
    let compatible: Vec<_> = entries
        .iter()
        .filter(|e| {
            locked_provider.is_none_or(|p| p == &e.snapshot.driver)
                && group.is_none_or(|g| g.is_empty() || e.continuation_group_key == Some(g))
                && (!exact || Some(&e.snapshot.instance_id) == locked_instance_id)
        })
        .collect();
    let selectable = |e: &&ProviderInstanceEntry<'_>| e.snapshot.enabled && e.is_available;
    let fallback = |driver: Option<&ProviderDriverKind>| {
        compatible
            .iter()
            .copied()
            .filter(|e| driver.is_none_or(|d| &e.snapshot.driver == d))
            .find(|e| selectable(e) && e.snapshot.status == ServerProviderState::Ready)
            .or_else(|| {
                compatible
                    .iter()
                    .copied()
                    .filter(|e| driver.is_none_or(|d| &e.snapshot.driver == d))
                    .find(|e| selectable(e) && e.snapshot.status != ServerProviderState::Error)
            })
    };
    let selected = candidate_ids
        .iter()
        .find_map(|id| {
            compatible
                .iter()
                .copied()
                .find(|e| Some(&e.snapshot.instance_id) == *id && selectable(e))
        })
        .or_else(|| fallback(Some(&requested_driver)))
        .or_else(|| fallback(None));
    ComposerProviderSelection {
        selected_provider_entry: selected,
        requested_instance_id: requested,
        requested_driver_kind: requested_driver,
        locked_continuation_group_key: group,
        unavailable_provider_instance_id: if selected.is_some() {
            None
        } else if locked_provider.is_some() {
            locked_instance_id.or(requested)
        } else {
            requested
        },
    }
}

pub fn get_antigravity_send_block_reason(
    provider: Option<&ServerProvider>,
    model: &str,
) -> Option<&'static str> {
    let provider = provider.filter(|p| p.driver.0 == "antigravity")?;
    if !provider.installed {
        return Some("Install Antigravity in provider settings before sending.");
    }
    if provider.auth.status == ServerProviderAuthStatus::Unauthenticated {
        return Some("Sign in to Antigravity in provider settings before sending.");
    }
    let slug = model.js_trim();
    if slug.is_empty() {
        return Some("Choose an Antigravity model before sending.");
    }
    if provider.auth.status == ServerProviderAuthStatus::Unknown {
        return None;
    }
    if provider.models.is_empty() {
        return Some("Refresh Antigravity models in provider settings before sending.");
    }
    if provider.status == ServerProviderState::Ready
        && slug != t3_contracts::model::ANTIGRAVITY_DEFAULT_MODEL
        && !provider.models.iter().any(|m| {
            m.slug.0 == slug
                || m.aliases
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|a| a.iter().any(|a| a.0 == slug))
        })
    {
        return Some("That Antigravity model is no longer available. Choose another model.");
    }
    None
}

pub fn reconcile_mounted_terminal_thread_ids(
    current: &[String],
    open: &[String],
    active: Option<&str>,
    active_terminal_open: bool,
    max_hidden: Option<usize>,
) -> Vec<String> {
    let mut next: Vec<_> = current
        .iter()
        .filter(|id| Some(id.as_str()) != active && open.contains(id))
        .cloned()
        .collect();
    let cap = max_hidden.unwrap_or(MAX_HIDDEN_MOUNTED_TERMINAL_THREADS);
    // JS slice(-0) retains the entire array; preserve that edge case.
    if cap > 0 && next.len() > cap {
        next.drain(..next.len() - cap);
    }
    if let Some(active) = active.filter(|id| !id.is_empty())
        && active_terminal_open
        && !next.iter().any(|id| id == active)
    {
        next.push(active.into());
    }
    next
}

pub fn resolve_terminal_worktree_path<'a>(
    launch: Option<Option<&'a str>>,
    summary: Option<Option<&'a str>>,
    thread: Option<&'a str>,
) -> Option<&'a str> {
    launch.or(summary).unwrap_or(thread)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentView {
    pub attachment: ChatAttachment,
    pub preview_url: Option<String>,
    pub downloadable: bool,
    pub resource: Option<crate::platform::HostResource>,
}

pub fn collect_user_message_blob_preview_urls(
    is_user: bool,
    attachments: &[AttachmentView],
) -> Vec<&str> {
    if !is_user {
        return Vec::new();
    }
    attachments
        .iter()
        .filter(|a| matches!(a.attachment, ChatAttachment::Image(_)))
        .filter_map(|a| {
            a.preview_url
                .as_deref()
                .filter(|url| url.starts_with("blob:"))
        })
        .collect()
}

pub fn timeline_has_ephemeral_preview_urls<'a>(
    messages: impl IntoIterator<Item = (bool, bool, &'a [AttachmentView])>,
) -> bool {
    messages
        .into_iter()
        .any(|(is_message, is_user, attachments)| {
            is_message && !collect_user_message_blob_preview_urls(is_user, attachments).is_empty()
        })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceMode {
    #[default]
    Local,
    Worktree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundDraftWorkspaceOptions {
    pub env_mode: WorkspaceMode,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub start_from_origin: bool,
}

pub fn resolve_background_draft_workspace_options(
    env_mode: WorkspaceMode,
    branch: Option<String>,
    start_from_origin: bool,
) -> BackgroundDraftWorkspaceOptions {
    BackgroundDraftWorkspaceOptions {
        env_mode,
        branch,
        worktree_path: None,
        start_from_origin: env_mode == WorkspaceMode::Worktree && start_from_origin,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalContextDraft {
    pub id: String,
    pub terminal_label: String,
    pub line_start: u64,
    pub line_end: u64,
    pub text: String,
}

impl TerminalContextDraft {
    pub fn has_text(&self) -> bool {
        !self
            .text
            .replace("\r\n", "\n")
            .trim_matches('\n')
            .is_empty()
    }
    pub fn label(&self) -> String {
        if self.line_start == self.line_end {
            format!("{} line {}", self.terminal_label, self.line_start)
        } else {
            format!(
                "{} lines {}-{}",
                self.terminal_label, self.line_start, self.line_end
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerSendState<'a> {
    pub trimmed_prompt: String,
    pub sendable_terminal_contexts: Vec<&'a TerminalContextDraft>,
    pub expired_terminal_context_count: usize,
    pub has_sendable_content: bool,
}

pub fn derive_composer_send_state<'a>(
    prompt: &str,
    image_count: usize,
    existing_attachment_count: usize,
    contexts: &'a [TerminalContextDraft],
    element_context_count: usize,
) -> ComposerSendState<'a> {
    let trimmed_prompt = strip_references(prompt).js_trim().to_owned();
    let sendable: Vec<_> = contexts.iter().filter(|c| c.has_text()).collect();
    let has_sendable_content = !trimmed_prompt.is_empty()
        || image_count > 0
        || existing_attachment_count > 0
        || !sendable.is_empty()
        || element_context_count > 0;
    ComposerSendState {
        trimmed_prompt,
        expired_terminal_context_count: contexts.len() - sendable.len(),
        sendable_terminal_contexts: sendable,
        has_sendable_content,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastCopy {
    pub title: String,
    pub description: String,
}

pub fn build_expired_terminal_context_toast_copy(count: f64, empty: bool) -> ToastCopy {
    let singular = count.floor().max(1.0) == 1.0;
    let noun = if singular {
        "Expired terminal context"
    } else {
        "Expired terminal contexts"
    };
    let pronoun = if singular { "it" } else { "them" };
    if empty {
        ToastCopy {
            title: format!("{noun} won't be sent"),
            description: format!(
                "Remove {pronoun} or re-add {pronoun} to include terminal output."
            ),
        }
    } else {
        ToastCopy {
            title: format!("{noun} omitted from message"),
            description: format!("Re-add {pronoun} if you want that terminal output included."),
        }
    }
}

pub fn branch_mismatch_key(
    thread_id: Option<&str>,
    mismatch: Option<(&str, &str)>,
) -> Option<String> {
    let id = thread_id.filter(|id| !id.is_empty())?;
    let (thread, current) = mismatch?;
    Some(format!("{id}:{thread}:{current}"))
}

pub fn should_show_branch_mismatch_banner(
    has_mismatch: bool,
    dismissed: bool,
    composer_has_content: bool,
    shown_for_current: bool,
) -> bool {
    has_mismatch && !dismissed && (composer_has_content || shown_for_current)
}

#[derive(Debug, Default)]
pub struct ChatSessionMemory {
    dismissed_branch_mismatch_keys: BTreeSet<String>,
    checkout_is_repo: BTreeMap<(EnvironmentId, String), bool>,
}

impl ChatSessionMemory {
    pub fn dismiss_branch_mismatch(&mut self, key: String) {
        self.dismissed_branch_mismatch_keys.insert(key);
    }
    pub fn is_branch_mismatch_dismissed(&self, key: Option<&str>) -> bool {
        key.is_some_and(|key| self.dismissed_branch_mismatch_keys.contains(key))
    }
    pub fn remember_checkout_is_repo(
        &mut self,
        environment: EnvironmentId,
        cwd: String,
        is_repo: bool,
    ) {
        self.checkout_is_repo.insert((environment, cwd), is_repo);
    }
    pub fn recall_checkout_is_repo(
        &self,
        environment: &EnvironmentId,
        cwd: Option<&str>,
    ) -> Option<bool> {
        self.checkout_is_repo
            .get(&(environment.clone(), cwd?.into()))
            .copied()
    }
}

pub fn derive_locked_provider(
    has_latest_run: bool,
    item_count: u64,
    has_runtime: bool,
    session_provider: Option<&str>,
    selected_provider: Option<&str>,
    thread_provider: Option<&str>,
    providers: &[ServerProvider],
) -> Option<ProviderDriverKind> {
    if !has_latest_run && item_count == 0 && !has_runtime {
        return None;
    }
    let narrow = |value: &str| {
        let mut chars = value.bytes();
        (value.len() <= 64
            && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'))
        .then(|| ProviderDriverKind(value.into()))
    };
    if let Some(provider) = session_provider.and_then(narrow) {
        return Some(provider);
    }
    let resolve = |value: &str| {
        narrow(
            providers
                .iter()
                .find(|p| p.instance_id.0 == value)
                .map_or(value, |p| p.driver.0.as_str()),
        )
    };
    thread_provider
        .and_then(resolve)
        .or_else(|| selected_provider.and_then(resolve))
}

pub fn get_started_thread_model_change_block_reason(
    has_started_session: bool,
    supports_switching_via_handoff: bool,
    current: &ModelSelection,
    current_instance: Option<&ProviderInstanceId>,
    next: &ModelSelection,
) -> Option<ToastCopy> {
    if !has_started_session
        || current_instance.unwrap_or(&current.instance_id) == &next.instance_id
        || supports_switching_via_handoff
    {
        return None;
    }
    Some(ToastCopy {
        title: "Start a new chat to switch providers".into(),
        description: "This thread does not support switching providers after it has started."
            .into(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Disconnected,
    Connecting,
    Running,
    Ready,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunDispatchView {
    pub run_id: Option<RunId>,
    pub requested_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeDispatchView {
    pub status: Option<String>,
    pub updated_at: Option<String>,
    pub active_run_id: Option<RunId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDispatchSnapshot {
    pub started_at: String,
    pub preparing_worktree: bool,
    pub submission_intent: SubmissionIntent,
    pub latest_user_message_id: Option<MessageId>,
    pub latest_run: RunDispatchView,
    pub runtime: RuntimeDispatchView,
}

pub fn create_local_dispatch_snapshot(
    now_iso: String,
    latest_run: Option<&RunDispatchView>,
    runtime: Option<&RuntimeDispatchView>,
    preparing_worktree: bool,
    latest_user_message_id: Option<MessageId>,
    submission_intent: SubmissionIntent,
) -> LocalDispatchSnapshot {
    LocalDispatchSnapshot {
        started_at: now_iso,
        preparing_worktree,
        submission_intent,
        latest_user_message_id,
        latest_run: latest_run.cloned().unwrap_or_default(),
        runtime: runtime.cloned().unwrap_or_default(),
    }
}

pub struct LocalDispatchClock<'a> {
    pub clocks: &'a crate::view_logic::clock::ServerClocks,
    pub now_epoch_ms: u64,
    pub environment: Option<&'a EnvironmentId>,
    pub active_thread_environment: Option<&'a EnvironmentId>,
}

pub fn create_local_dispatch_snapshot_with_clock(
    clock: LocalDispatchClock<'_>,
    latest_run: Option<&RunDispatchView>,
    runtime: Option<&RuntimeDispatchView>,
    preparing_worktree: bool,
    latest_user_message_id: Option<MessageId>,
    submission_intent: SubmissionIntent,
) -> LocalDispatchSnapshot {
    let now = clock
        .environment
        .or(clock.active_thread_environment)
        .map_or(clock.now_epoch_ms as f64, |environment| {
            clock
                .clocks
                .to_server_clock(environment, clock.now_epoch_ms as f64)
        });
    create_local_dispatch_snapshot(
        crate::time::iso_from_epoch_ms(now as u64),
        latest_run,
        runtime,
        preparing_worktree,
        latest_user_message_id,
        submission_intent,
    )
}

pub fn derive_committed_server_user_message_ids(
    rows: &[OrchestrationV2ProjectedTurnItem],
) -> Vec<t3_contracts::orchestration_v2_id_table::IdOrIndex<MessageId>> {
    let mut ids = Vec::new();
    for row in rows {
        if let OrchestrationV2TurnItem::UserMessage(item) = &row.item
            && !ids.contains(&item.message_id)
        {
            ids.push(item.message_id.clone());
        }
    }
    ids
}

pub struct DispatchAcknowledgement<'a> {
    pub local_dispatch: Option<&'a LocalDispatchSnapshot>,
    pub phase: SessionPhase,
    pub latest_run: Option<&'a RunDispatchView>,
    pub latest_user_message_id: Option<&'a MessageId>,
    pub runtime: Option<&'a RuntimeDispatchView>,
    pub has_pending_approval: bool,
    pub has_pending_user_input: bool,
    pub thread_error: Option<&'a str>,
}

pub fn has_server_acknowledged_local_dispatch(input: DispatchAcknowledgement<'_>) -> bool {
    let Some(local) = input.local_dispatch else {
        return false;
    };
    if input.has_pending_approval
        || input.has_pending_user_input
        || input.thread_error.is_some_and(|e| !e.is_empty())
    {
        return true;
    }
    let latest_user_changed = local.latest_user_message_id.as_ref() != input.latest_user_message_id;
    let runtime_connecting = input
        .runtime
        .and_then(|r| r.status.as_deref())
        .is_some_and(|s| matches!(s, "preparing" | "starting" | "queued"));
    if input.phase == SessionPhase::Connecting || runtime_connecting {
        return input.runtime.and_then(|r| r.status.as_deref()) == Some("queued")
            && latest_user_changed;
    }
    let default_run = RunDispatchView::default();
    let run = input.latest_run.unwrap_or(&default_run);
    let changed = &local.latest_run != run;
    if input.phase == SessionPhase::Running {
        if latest_user_changed {
            return true;
        }
        if !changed || run.started_at.is_none() {
            return false;
        }
        if let Some(active) = input.runtime.and_then(|r| r.active_run_id.as_ref())
            && run.run_id.as_ref() != Some(active)
        {
            return false;
        }
        return true;
    }
    changed
        || local.runtime.status.as_deref() != input.runtime.and_then(|r| r.status.as_deref())
        || local.runtime.updated_at.as_deref()
            != input.runtime.and_then(|r| r.updated_at.as_deref())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FocusedElement<'a> {
    pub tag_name: &'a str,
    pub is_content_editable: bool,
    pub role: Option<&'a str>,
    pub in_keyboard_focus_region: bool,
}

pub fn should_refocus_composer_on_window_focus(element: Option<FocusedElement<'_>>) -> bool {
    let Some(element) = element else {
        return true;
    };
    if element.tag_name == "BODY" {
        return true;
    }
    !matches!(
        element.tag_name,
        "INPUT" | "TEXTAREA" | "SELECT" | "IFRAME" | "WEBVIEW"
    ) && !element.is_content_editable
        && element.role != Some("textbox")
        && !element.in_keyboard_focus_region
}

pub fn merge_direct_preview_annotation(
    images: &[AttachmentView],
    file_count: usize,
    annotations: &[PreviewAnnotationPayloadSchema],
    direct: Option<(&PreviewAnnotationPayloadSchema, Option<&AttachmentView>)>,
) -> (Vec<AttachmentView>, Vec<PreviewAnnotationPayloadSchema>) {
    let Some((annotation, image)) = direct else {
        return (images.to_vec(), annotations.to_vec());
    };
    let already = image.is_some_and(|image| {
        images
            .iter()
            .any(|candidate| attachment_id(candidate) == attachment_id(image))
    });
    let appended = image.is_some()
        && !already
        && images.len().saturating_add(file_count) < PROVIDER_SEND_TURN_MAX_ATTACHMENTS;
    let mut images = images.to_vec();
    if let Some(image) = image.filter(|_| appended) {
        images.push(image.clone());
    }
    let mut annotations = annotations.to_vec();
    if !annotations.iter().any(|a| a.id == annotation.id) {
        let mut annotation = annotation.clone();
        if appended || already {
            if let Some(screenshot) = &mut annotation.screenshot {
                screenshot.data_url.clear();
            }
        } else {
            annotation.screenshot = None;
        }
        annotations.push(annotation);
    }
    (images, annotations)
}

pub fn attachment_id(attachment: &AttachmentView) -> &str {
    match &attachment.attachment {
        ChatAttachment::Image(a) => &a.id,
        ChatAttachment::File(a) => &a.id,
        ChatAttachment::Unknown(a) => &a.id,
    }
}

pub fn is_compact_command_message(is_user: bool, text: &str, attachment_count: usize) -> bool {
    is_user && text.js_trim().eq_ignore_ascii_case("/compact") && attachment_count == 0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DispatchMode {
    #[default]
    Auto,
    Queue,
    Steer,
    Restart,
}

pub struct ComposerTurnInput<'a> {
    pub message_text: &'a str,
    pub send_queues_behind_setup: bool,
    pub is_server_thread: bool,
    pub active_message_count: usize,
    pub send_env_mode: WorkspaceMode,
    pub worktree_path: Option<&'a str>,
    pub active_thread_branch: Option<&'a str>,
    pub resume_compaction_tokens: Option<u64>,
    pub keep_full_history: bool,
    pub phase: SessionPhase,
    pub dispatch_mode: DispatchMode,
    pub fresh_context: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerTurnPlan<'a> {
    pub missing_worktree_base_branch: bool,
    pub base_branch_for_worktree: Option<&'a str>,
    pub compact_before_send: bool,
    pub turn_dispatch_mode: DispatchMode,
    pub should_queue_behind_active_run: bool,
}

pub fn plan_composer_turn(input: ComposerTurnInput<'_>) -> ComposerTurnPlan<'_> {
    let first = !input.send_queues_behind_setup
        && (!input.is_server_thread || input.active_message_count == 0);
    let create = first
        && input.send_env_mode == WorkspaceMode::Worktree
        && input.worktree_path.is_none_or(str::is_empty);
    let compact_command = input
        .message_text
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("/compact"))
        && input.message_text[8..]
            .chars()
            .next()
            .is_none_or(crate::view_logic::is_js_whitespace);
    let compact = !input.fresh_context
        && input.resume_compaction_tokens.is_some()
        && !input.keep_full_history
        && !compact_command;
    let mode = if input.fresh_context
        && matches!(
            input.dispatch_mode,
            DispatchMode::Steer | DispatchMode::Restart
        ) {
        DispatchMode::Queue
    } else {
        input.dispatch_mode
    };
    ComposerTurnPlan {
        missing_worktree_base_branch: create
            && input.active_thread_branch.is_none_or(str::is_empty),
        base_branch_for_worktree: if create {
            input.active_thread_branch
        } else {
            None
        },
        compact_before_send: compact,
        turn_dispatch_mode: if compact
            || (input.send_queues_behind_setup && mode != DispatchMode::Steer)
        {
            DispatchMode::Queue
        } else {
            mode
        },
        should_queue_behind_active_run: compact
            || input.send_queues_behind_setup
            || (input.phase == SessionPhase::Running && mode == DispatchMode::Queue),
    }
}

pub fn bound_attachment_ids(
    uploaded: &[Option<String>],
    local: &[AttachmentView],
) -> Result<Vec<String>, ClientError> {
    uploaded
        .iter()
        .enumerate()
        .map(|(i, id)| {
            id.clone()
                .or_else(|| local.get(i).map(|a| attachment_id(a).to_owned()))
                .ok_or_else(|| {
                    ClientError::InvalidPayload(
                        "missing local attachment for uploaded position".into(),
                    )
                })
        })
        .collect()
}

pub fn is_thread_environment_locked(
    message_count: usize,
    has_runtime: bool,
    active_provider_thread_id: Option<&t3_contracts::base_schemas::ProviderThreadId>,
) -> bool {
    message_count > 0 || has_runtime || active_provider_thread_id.is_some()
}

pub struct ComposerSendContents<'a, Review> {
    pub prompt: &'a str,
    pub images: &'a [AttachmentView],
    pub files: &'a [AttachmentView],
    pub terminal_contexts: &'a [TerminalContextDraft],
    pub sendable_terminal_contexts: &'a [&'a TerminalContextDraft],
    pub preview_annotations: &'a [PreviewAnnotationPayloadSchema],
    pub review_comments: &'a [Review],
    pub thread_contexts: &'a [t3_contracts::composer_context::ThreadContextRecord],
    pub environment_contexts: &'a [t3_contracts::composer_context::EnvironmentContextRecord],
    pub reply_to: Option<&'a t3_contracts::composer_context::ResponseContextRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposerSendSnapshot<Review> {
    pub images: Vec<AttachmentView>,
    pub files: Vec<AttachmentView>,
    pub attachments: Vec<AttachmentView>,
    pub terminal_contexts: Vec<TerminalContextDraft>,
    pub preview_annotations: Vec<PreviewAnnotationPayloadSchema>,
    pub review_comments: Vec<Review>,
    pub thread_contexts: Vec<t3_contracts::composer_context::ThreadContextRecord>,
    pub environment_contexts: Vec<t3_contracts::composer_context::EnvironmentContextRecord>,
    pub reply_to: Option<t3_contracts::composer_context::ResponseContextRecord>,
    pub message_text: String,
    pub draft_text: String,
}

pub fn snapshot_composer_send<Review: Clone>(
    contents: ComposerSendContents<'_, Review>,
) -> ComposerSendSnapshot<Review> {
    let mut text = contents.prompt.to_owned();
    for context in contents.terminal_contexts.iter().filter(|c| {
        !contents
            .sendable_terminal_contexts
            .iter()
            .any(|s| std::ptr::eq(*s, *c))
    }) {
        text = remove_reference(&text, &kind_scoped_id("terminal", &context.id));
    }
    let message_text = text.js_trim().to_owned();
    let draft_text = contents.reply_to.map_or_else(
        || message_text.clone(),
        |reply| {
            remove_reference(&message_text, &reply.context_id.0)
                .js_trim_start()
                .to_owned()
        },
    );
    ComposerSendSnapshot {
        images: contents.images.to_vec(),
        files: contents.files.to_vec(),
        attachments: contents
            .images
            .iter()
            .chain(contents.files)
            .cloned()
            .collect(),
        terminal_contexts: contents
            .sendable_terminal_contexts
            .iter()
            .map(|c| (*c).clone())
            .collect(),
        preview_annotations: contents.preview_annotations.to_vec(),
        review_comments: contents.review_comments.to_vec(),
        thread_contexts: contents.thread_contexts.to_vec(),
        environment_contexts: contents.environment_contexts.to_vec(),
        reply_to: contents.reply_to.cloned(),
        message_text,
        draft_text,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MessageWithContext {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<t3_contracts::composer_context::OrchestrationMessageContext>,
}

pub fn with_message_context(
    text: String,
    context: Option<t3_contracts::composer_context::OrchestrationMessageContext>,
) -> MessageWithContext {
    MessageWithContext { text, context }
}

pub fn optimistic_message_attachments(attachments: &[AttachmentView]) -> Vec<AttachmentView> {
    attachments
        .iter()
        .map(|a| AttachmentView {
            attachment: a.attachment.clone(),
            preview_url: if matches!(a.attachment, ChatAttachment::Image(_)) {
                a.preview_url.clone()
            } else {
                None
            },
            downloadable: false,
            resource: None,
        })
        .collect()
}

pub fn send_title_fallback_labels<Review>(
    snapshot: &ComposerSendSnapshot<Review>,
    review_label: impl FnOnce(&Review) -> String,
    annotation_label: impl FnOnce(&PreviewAnnotationPayloadSchema) -> String,
) -> [Option<String>; 3] {
    [
        snapshot
            .terminal_contexts
            .first()
            .map(TerminalContextDraft::label),
        snapshot
            .review_comments
            .first()
            .map(|review| format!("Review: {}", review_label(review))),
        snapshot.preview_annotations.first().map(annotation_label),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalThreadErrorEntry {
    pub message: Option<String>,
    pub at: u64,
}

pub struct ThreadErrorTarget<'a> {
    pub route_thread: &'a ThreadKey,
    pub route_thread_key: &'a str,
    pub draft_id: Option<&'a str>,
    pub server_thread: Option<&'a ThreadKey>,
}

pub fn set_thread_error(
    target: ThreadErrorTarget<'_>,
    target_thread_id: Option<&ThreadId>,
    message: Option<String>,
    at_epoch_ms: u64,
    server_errors: &mut BTreeMap<String, LocalThreadErrorEntry>,
    draft_errors: &mut BTreeMap<String, LocalThreadErrorEntry>,
) {
    let Some(id) = target_thread_id else {
        return;
    };
    let (errors, key) = if target
        .server_thread
        .is_some_and(|server| server == target.route_thread && &server.thread == id)
    {
        (server_errors, target.route_thread_key.to_owned())
    } else {
        (draft_errors, target.draft_id.unwrap_or(&id.0).to_owned())
    };
    let next = super::errors::sanitize_thread_error_message(message.as_deref()).map(str::to_owned);
    if errors.get(&key).and_then(|e| e.message.as_ref()) == next.as_ref() {
        return;
    }
    errors.insert(
        key,
        LocalThreadErrorEntry {
            message: next,
            at: at_epoch_ms,
        },
    );
}

pub fn model_disabled_reason(
    has_started: bool,
    supports_handoff: bool,
    current: &ModelSelection,
    current_instance: Option<&ProviderInstanceId>,
    next: &ModelSelection,
) -> Option<String> {
    get_started_thread_model_change_block_reason(
        has_started,
        supports_handoff,
        current,
        current_instance,
        next,
    )
    .map(|reason| {
        format!(
            "{} Start a new thread to use this model.",
            reason.description
        )
    })
}

pub fn chat_action_error_message(error_message: Option<&str>) -> &str {
    error_message.unwrap_or("An error occurred.")
}

pub fn release_chat_timeline_anchor(message_id: &mut Option<MessageId>) {
    *message_id = None;
}

#[derive(Debug, Clone, Copy)]
pub enum ArtifactTemplateKind {
    Document,
    Presentation,
    Spreadsheet,
    Site,
    GoogleDocs,
    GoogleSlides,
    GoogleSheets,
    Image,
    Email,
    Slack,
}

pub fn codex_artifact_template_prompt_to_append(
    current_draft: &str,
    kind: ArtifactTemplateKind,
    skill_name: &str,
) -> Option<String> {
    let (before, after) = match kind {
        ArtifactTemplateKind::Document => ("Create a document using this", "about…"),
        ArtifactTemplateKind::Presentation => {
            ("Create a presentation using the", "template about…")
        }
        ArtifactTemplateKind::Spreadsheet => ("Create a spreadsheet using this", "about…"),
        ArtifactTemplateKind::Site => ("Create a Site using this", "about…"),
        ArtifactTemplateKind::GoogleDocs => ("Create a Google Doc using this", "about…"),
        ArtifactTemplateKind::GoogleSlides => {
            ("Create a Google Slides presentation using this", "about…")
        }
        ArtifactTemplateKind::GoogleSheets => ("Create a Google Sheet using this", "about…"),
        ArtifactTemplateKind::Image => ("Create an image using this", "of…"),
        ArtifactTemplateKind::Email => ("Draft an email using this", "about…"),
        ArtifactTemplateKind::Slack => ("Draft a Slack message using this", "about…"),
    };
    let prompt = format!("{before} ${skill_name} {after}");
    let trimmed = current_draft.js_trim_end();
    if let Some(prefix) = trimmed.strip_suffix(&prompt)
        && prefix
            .chars()
            .next_back()
            .is_none_or(crate::view_logic::is_js_whitespace)
    {
        None
    } else {
        Some(prompt)
    }
}

pub fn resolve_selectable_model<'a>(
    provider: &str,
    value: Option<&str>,
    models: &'a [t3_contracts::server::ServerProviderModel],
) -> Option<&'a t3_contracts::server::ServerProviderModel> {
    let value = t3_contracts::wire::trim_js(value?).to_owned();
    if value.is_empty() {
        return None;
    }
    models
        .iter()
        .find(|m| m.slug.0 == value)
        .or_else(|| {
            models
                .iter()
                .find(|m| m.name.0.to_lowercase() == value.to_lowercase())
        })
        .or_else(|| {
            models.iter().find(|m| {
                m.aliases
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|aliases| {
                        aliases
                            .iter()
                            .any(|alias| alias.0.to_lowercase() == value.to_lowercase())
                    })
            })
        })
        .or_else(|| {
            let normalized = t3_contracts::model::MODEL_SLUG_ALIASES_BY_PROVIDER
                .iter()
                .find(|(driver, _)| *driver == provider)
                .and_then(|(_, aliases)| {
                    aliases
                        .iter()
                        .find(|(alias, _)| *alias == value)
                        .map(|(_, slug)| *slug)
                })
                .unwrap_or(&value);
            models.iter().find(|m| m.slug.0 == normalized)
        })
}

pub fn format_outgoing_prompt(
    provider: &str,
    model: Option<&str>,
    models: &[t3_contracts::server::ServerProviderModel],
    effort: Option<&str>,
    text: &str,
) -> String {
    let effort = effort
        .map(t3_contracts::wire::trim_js)
        .filter(|e| !e.is_empty());
    let injected = resolve_selectable_model(provider, model, models)
        .and_then(|m| m.capabilities.as_ref())
        .and_then(|caps| caps.option_descriptors.as_ref())
        .and_then(Option::as_ref)
        .is_some_and(|descriptors| {
            descriptors.iter().any(|d| match d {
                t3_contracts::model::ProviderOptionDescriptor::Select(d)
                    if d.id != "agent" || d.options.iter().any(|o| o.id != "plan") =>
                {
                    d.prompt_injected_values
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|values| {
                            values.iter().any(|value| Some(value.as_str()) == effort)
                        })
                }
                _ => false,
            })
        });
    let text = t3_contracts::wire::trim_js(text);
    let slash_command = text.strip_prefix('/').is_some_and(|rest| {
        let token = rest
            .split(crate::view_logic::is_js_whitespace)
            .next()
            .unwrap_or("");
        !token.is_empty() && !token.contains('/')
    });
    if !text.is_empty()
        && injected
        && effort == Some("ultrathink")
        && !slash_command
        && !text.starts_with("Ultrathink:")
    {
        format!("Ultrathink:\n{text}")
    } else {
        text.into()
    }
}

pub fn start_new_thread_for_project(
    project: Option<&t3_contracts::environment::ScopedProjectRef>,
    start: impl FnOnce(&t3_contracts::environment::ScopedProjectRef),
) -> bool {
    if let Some(project) = project {
        start(project);
        true
    } else {
        false
    }
}

pub fn clone_composer_image_for_retry(
    image: &AttachmentView,
    create_preview: impl FnOnce(&crate::platform::HostResource) -> Result<String, ClientError>,
) -> AttachmentView {
    if image
        .preview_url
        .as_deref()
        .is_some_and(|url| url.starts_with("blob:"))
        && let Some(resource) = &image.resource
        && let Ok(url) = create_preview(resource)
    {
        return AttachmentView {
            preview_url: Some(url),
            ..image.clone()
        };
    }
    image.clone()
}

#[derive(Debug, Clone)]
pub struct DraftThreadInput {
    pub environment_id: EnvironmentId,
    pub project_id: t3_contracts::base_schemas::ProjectId,
    pub created_at: t3_contracts::wire::WireDateTime,
    pub runtime_mode: t3_contracts::provider_policy::RuntimeMode,
    pub interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode,
    pub branch: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
    pub worktree_path: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
}

#[derive(Debug, Clone)]
pub struct ThreadPresentation<RunSummary = RunDispatchView, RuntimeSummary = RuntimeDispatchView> {
    pub environment_id: EnvironmentId,
    pub shell: t3_contracts::orchestration_v2::OrchestrationV2ThreadShell,
    pub latest_run: Option<RunSummary>,
    pub runtime: Option<RuntimeSummary>,
}

#[derive(Debug, Clone)]
pub struct ExecutionPresentation<RunSummary, RuntimeSummary> {
    pub latest_run: Option<(RunId, RunSummary)>,
    pub runtime: Option<(RuntimeDispatchView, RuntimeSummary)>,
}

pub fn build_local_draft_thread(
    thread_id: ThreadId,
    draft: DraftThreadInput,
    fallback_model_selection: ModelSelection,
) -> Result<ThreadPresentation, ClientError> {
    let shell = serde_json::from_value(serde_json::json!({
        "id":thread_id, "projectId":draft.project_id, "title":"New thread", "providerInstanceId":fallback_model_selection.instance_id,
        "modelSelection":fallback_model_selection, "runtimeMode":draft.runtime_mode,"interactionMode":draft.interaction_mode,
        "branch":draft.branch,"worktreePath":draft.worktree_path,"activeProviderThreadId":null,
        "lineage":{"rootThreadId":thread_id,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,
        "createdBy":"user","creationSource":"web","latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,
        "latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,
        "createdAt":draft.created_at,"updatedAt":draft.created_at,"archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null
    }))?;
    Ok(ThreadPresentation {
        environment_id: draft.environment_id,
        shell,
        latest_run: None,
        runtime: None,
    })
}

pub fn present_thread_shell_from_projection<RunSummary, RuntimeSummary>(
    environment_id: EnvironmentId,
    thread_id: &ThreadId,
    projection: &t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection,
    updated_at: t3_contracts::wire::WireDateTime,
    last_visited_at: Option<t3_contracts::wire::WireDateTime>,
    execution: ExecutionPresentation<RunSummary, RuntimeSummary>,
) -> Result<ThreadPresentation<RunSummary, RuntimeSummary>, ClientError> {
    use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};
    let mut thread = serde_json::to_value(&projection.thread)?;
    let pending = projection
        .runtime_requests
        .iter()
        .rfind(|r| r.status == RuntimeRequestStatus::Pending);
    let mut history = Vec::new();
    for provider in projection.provider_threads.iter().filter(|t| {
        t.app_thread_id == Some(IdOrIndex::Id(thread_id.clone())) && t.owner_node_id.is_none()
    }) {
        if !history.contains(&provider.provider_instance_id) {
            history.push(provider.provider_instance_id.clone());
        }
    }
    let goal = projection
        .provider_threads
        .iter()
        .find(|t| Some(&t.id) == projection.thread.active_provider_thread_id.as_ref())
        .and_then(|t| t.goal.as_ref())
        .and_then(Option::as_ref);
    let status = execution
        .runtime
        .as_ref()
        .and_then(|(r, _)| r.status.as_deref())
        .unwrap_or("idle");
    let extra = serde_json::json!({
        "lastVisitedAt":last_visited_at,"latestRunId":execution.latest_run.as_ref().map(|(id, _)| id),
        "activeRunId":execution.runtime.as_ref().and_then(|(r, _)| r.active_run_id.as_ref()),"status":status,
        "pendingRuntimeRequest":pending,"latestVisibleMessage":null,
        "latestUserMessageAt":projection.messages.iter().rfind(|m| m.role == ConversationMessageRole::User).map(|m| &m.updated_at),
        "hasActionableProposedPlan":projection.plans.iter().any(|p| matches!(p, OrchestrationV2PlanArtifact::ProposedPlan(p) if p.status == PlanArtifactProposedPlanStatus::Active)),
        "providerInstanceHistory":history,"goal":goal,"itemCount":projection.turn_items.len(),"visibleItemCount":projection.visible_turn_items.len(),"updatedAt":updated_at
    });
    let object = thread
        .as_object_mut()
        .ok_or_else(|| ClientError::InvalidPayload("thread is not an object".into()))?;
    if let serde_json::Value::Object(extra) = extra {
        object.extend(extra);
    }
    Ok(ThreadPresentation {
        environment_id,
        shell: serde_json::from_value(thread)?,
        latest_run: execution.latest_run.map(|(_, summary)| summary),
        runtime: execution.runtime.map(|(_, summary)| summary),
    })
}

pub fn resolve_send_env_mode(requested: WorkspaceMode, is_git_repo: bool) -> WorkspaceMode {
    if is_git_repo {
        requested
    } else {
        WorkspaceMode::Local
    }
}

pub fn should_show_open_in_picker(
    active_project_name: Option<&str>,
    active_thread_environment_id: &EnvironmentId,
    primary_environment_id: Option<&EnvironmentId>,
    host_environment_id: Option<&EnvironmentId>,
    local_exec: bool,
) -> bool {
    active_project_name.is_some_and(|name| !name.is_empty())
        && (Some(active_thread_environment_id) == primary_environment_id
            || Some(active_thread_environment_id) == host_environment_id
            || !local_exec)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "chat_view_tests.rs"]
mod tests;
