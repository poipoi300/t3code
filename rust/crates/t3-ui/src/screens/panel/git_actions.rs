//! Port of apps/web/src/components/GitActionsControl.tsx GitActionsControl.

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    time::Duration,
};

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::workspace::{
    git_actions::{self as logic, ChangeRequestTerminology, DialogAction, QuickActionKind},
    project_ops::actions::{VcsActionOperation, VcsActionState},
};
use t3_client::{ClientError, types::ThreadKey};
use t3_contracts::{git::*, source_control::*};

use crate::{
    i18n::{Text, Value, t, tr},
    theme::{cn, tw},
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        dialog::{self, ActionVariant, FooterVariant, PopupOptions},
        icon::{Icon, icon_with_class},
        menu::{self, ItemOptions, MenuOptions},
        toast::{self, ToastOptions},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitActionsScope {
    pub thread: ThreadKey,
    pub cwd: String,
    pub draft_id: Option<String>,
}

/// Client projections; the UI does not infer grants, apply events or maintain a VCS cache.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitActionsSnapshot {
    pub status: Option<Rc<VcsStatusResult>>,
    pub status_error: Option<String>,
    pub branch_preview: Option<logic::ChangesTotals>,
    pub action: Rc<VcsActionState>,
    pub can_write: bool,
    pub can_operate: bool,
    pub is_local_draft: bool,
    pub has_server_thread: bool,
    pub latest_run_is_null: bool,
    pub selecting_worktree_base: bool,
    pub project_cloning: bool,
    pub thread_branch: Option<String>,
    pub discovery: Option<Rc<SourceControlDiscoveryResult>>,
    pub offered_providers: Option<BTreeMap<String, bool>>,
    pub publish_providers: Vec<PublishProvider>,
    pub presentation: Option<SourceControlPresentation>,
    pub announce_status_changes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceControlPresentation {
    pub singular: String,
    pub short_label: String,
    pub icon_svg: Option<String>,
}

/// Display fields from sourceControlClients.definitions, including each provider's actual SVG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishProvider {
    pub kind: String,
    pub label: String,
    pub host: String,
    pub path_placeholder: String,
    pub icon_svg: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitActionRequest {
    pub action: GitStackedAction,
    pub commit_message: Option<String>,
    pub feature_branch: bool,
    pub file_paths: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitActionsWatch {
    pub show_actions: bool,
    pub show_changes: bool,
}

/// The frozen runtime adapter must delegate to the client's LiveVcs, SourceControl and
/// VcsActionManager, including current destination/grant checks and metadata persistence.
pub trait GitActionsBackend {
    fn watch(
        &self,
        scope: &GitActionsScope,
        watch: GitActionsWatch,
    ) -> LocalBoxStream<'static, GitActionsSnapshot>;
    fn refresh(&self, scope: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn run_action(
        &self,
        scope: &GitActionsScope,
        request: GitActionRequest,
    ) -> LocalBoxFuture<'static, Result<GitRunStackedActionResult, ClientError>>;
    fn pull(
        &self,
        scope: &GitActionsScope,
    ) -> LocalBoxFuture<'static, Result<VcsPullResult, ClientError>>;
    fn initialize(
        &self,
        scope: &GitActionsScope,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn publish(
        &self,
        scope: &GitActionsScope,
        input: SourceControlPublishRepositoryInput,
    ) -> LocalBoxFuture<'static, Result<SourceControlPublishRepositoryResult, ClientError>>;
    fn sync_branch(
        &self,
        scope: &GitActionsScope,
        branch: Option<String>,
        expected: Option<String>,
        manual: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn open_file(
        &self,
        scope: &GitActionsScope,
        root: String,
        path: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
}

#[derive(Clone)]
pub struct GitActionsRuntime(pub Rc<dyn GitActionsBackend>);

#[derive(Clone)]
pub struct GitActionsControlProps {
    pub scope: Prop<Option<GitActionsScope>>,
    pub compact: Prop<bool>,
    pub show_actions: Prop<bool>,
    pub show_changes: Prop<bool>,
    pub on_open_changes: Option<Rc<dyn Fn()>>,
    pub visible: Prop<bool>,
}

impl GitActionsControlProps {
    pub fn new(scope: Prop<Option<GitActionsScope>>) -> Self {
        Self {
            scope,
            compact: false.into(),
            show_actions: true.into(),
            show_changes: true.into(),
            on_open_changes: None,
            visible: true.into(),
        }
    }
}

#[derive(Clone, Copy)]
struct Control {
    scope: Stored<GitActionsScope>,
    backend: Stored<Option<GitActionsRuntime>>,
    snapshot: Signal<GitActionsSnapshot>,
    commit_open: Signal<bool>,
    message: Signal<String>,
    excluded: Signal<BTreeSet<String>>,
    editing: Signal<bool>,
    publish_open: Signal<bool>,
    pending: Signal<Option<PendingDefaultBranchAction>>,
    success: Signal<Option<InlineSuccess>>,
    initiating: Signal<bool>,
    menu_open: Signal<bool>,
    success_generation: Signal<u64>,
    menu_width: Signal<f32>,
    visible: Prop<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InlineSuccess {
    title: Text,
    description: Option<Text>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingDefaultBranchAction {
    request: GitActionRequest,
    branch: String,
    includes_commit: bool,
}

fn terminology(snapshot: &GitActionsSnapshot) -> ChangeRequestTerminology<'_> {
    ChangeRequestTerminology {
        singular: snapshot
            .presentation
            .as_ref()
            .map_or("pull request", |p| p.singular.as_str()),
        short_label: snapshot
            .presentation
            .as_ref()
            .map_or("PR", |p| p.short_label.as_str()),
    }
}

fn busy(snapshot: &GitActionsSnapshot) -> bool {
    snapshot.action.is_running
        && matches!(
            snapshot.action.operation,
            Some(
                VcsActionOperation::RunChangeRequest
                    | VcsActionOperation::Pull
                    | VcsActionOperation::PublishRepository
            )
        )
}

fn quick(snapshot: &GitActionsSnapshot) -> logic::QuickAction {
    logic::quick_action(
        snapshot.status.as_deref(),
        busy(snapshot),
        snapshot.status.as_ref().is_some_and(|s| s.is_default_ref),
        snapshot
            .status
            .as_ref()
            .is_some_and(|s| s.has_primary_remote),
        &terminology(snapshot),
    )
}

fn progress(snapshot: &GitActionsSnapshot) -> Option<logic::ProgressPresentation> {
    logic::progress_presentation(logic::ProgressInput {
        running: snapshot.action.is_running,
        operation: match snapshot.action.operation {
            Some(VcsActionOperation::RunChangeRequest) => Some("run_change_request"),
            Some(VcsActionOperation::Pull) => Some("pull"),
            _ => None,
        },
        label: snapshot.action.current_label.as_deref(),
        output: snapshot.action.last_output_line.as_deref(),
        phase_started: snapshot.action.phase_started_at_ms,
        hook_started: snapshot.action.hook_started_at_ms,
    })
}

fn progress_status(
    snapshot: &GitActionsSnapshot,
    progress: &logic::ProgressPresentation,
) -> String {
    if let Some(hook) = &snapshot.action.hook_name
        && progress.status == format!("Running {hook}...")
    {
        return tr(
            "Running {hookName}...",
            &[("hookName", Value::Text(hook.clone()))],
        );
    }
    Text::client_key(progress.status.clone())
        .resolve()
        .to_string()
}

fn quiet_empty(scope: &GitActionsScope, snapshot: &GitActionsSnapshot) -> bool {
    let totals = logic::changes_totals(snapshot.status.as_deref(), snapshot.branch_preview.clone());
    (scope.draft_id.is_some() || snapshot.latest_run_is_null)
        && progress(snapshot).is_none()
        && snapshot.status_error.is_none()
        && quick(snapshot).disabled
        && totals.is_none_or(|totals| totals.insertions == 0 && totals.deletions == 0)
}

fn live_branch_update(snapshot: &GitActionsSnapshot) -> Option<Option<String>> {
    if busy(snapshot)
        || snapshot.selecting_worktree_base
        || snapshot.has_server_thread
        || snapshot.project_cloning
    {
        return None;
    }
    logic::live_thread_branch_update(
        snapshot.thread_branch.as_deref(),
        snapshot.status.as_deref(),
    )
}

impl Control {
    fn can_write(self) -> bool {
        self.backend.with(|b| b.is_some()) && self.snapshot.with(|s| s.can_write)
    }
    fn can_change_branch(self) -> bool {
        self.can_write() && self.snapshot.with(|s| s.is_local_draft || s.can_operate)
    }
    fn can_run(self) -> bool {
        self.visible.get()
            && self.can_write()
            && !self.initiating.get()
            && !self.snapshot.with(busy)
    }
    fn reset_commit(self) {
        batch(|| {
            self.commit_open.set(false);
            self.message.set(String::new());
            self.excluded.set(BTreeSet::new());
            self.editing.set(false);
        });
    }
    fn selected_files(self) -> Vec<String> {
        self.snapshot.with(|s| {
            s.status
                .as_ref()
                .map(|s| {
                    s.working_tree
                        .files
                        .iter()
                        .filter(|file| !self.excluded.with(|paths| paths.contains(&file.path)))
                        .map(|file| file.path.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    }
    fn show_error(self, title: &'static str, error: ClientError) {
        if matches!(error, ClientError::Cancelled) {
            return;
        }
        toast::toast_manager().add(toast::stacked_thread_toast(ToastOptions {
            variant: toast::Variant::Error,
            title: title.into(),
            description: Some(Text::content(error.to_string())),
            timeout: None,
            data: toast::ThreadToastData {
                thread_ref: Some(self.scope.get().thread),
                ..Default::default()
            },
            ..Default::default()
        }));
    }
    fn refresh(self) {
        if !self.visible.get_untracked() {
            return;
        }
        if let Some(backend) = self.backend.get() {
            let scope = self.scope.get();
            spawn(async move {
                let _ = backend.0.refresh(&scope).await;
            });
        }
    }
    fn set_success(self, success: InlineSuccess) {
        self.success_generation.update(|g| *g += 1);
        let generation = self.success_generation.get_untracked();
        self.success.set(Some(success));
        set_timeout(
            Duration::from_millis(logic::SUCCESS_VISIBLE_MS),
            move || {
                if self.success_generation.get_untracked() == generation {
                    self.success.set(None);
                }
            },
        );
    }
    fn sync_branch(self, branch: Option<String>, manual: bool) {
        let snapshot = self.snapshot.get_untracked();
        if !snapshot.has_server_thread && !snapshot.is_local_draft {
            return;
        }
        if (snapshot.has_server_thread && !snapshot.can_operate) || snapshot.thread_branch == branch
        {
            return;
        }
        if let Some(backend) = self.backend.get() {
            let scope = self.scope.get();
            spawn(async move {
                if let Err(error) = backend
                    .0
                    .sync_branch(&scope, branch, snapshot.thread_branch, manual)
                    .await
                {
                    self.show_error("Action failed", error);
                }
            });
        }
    }
    fn run(self, request: GitActionRequest, skip_prompt: bool) {
        if !self.can_run() || (request.feature_branch && !self.can_change_branch()) {
            return;
        }
        let status = self.snapshot.get_untracked().status;
        if !skip_prompt
            && !request.feature_branch
            && status.as_ref().is_some_and(|s| {
                s.ref_name.is_some()
                    && logic::requires_default_branch_confirmation(
                        &request.action,
                        s.is_default_ref,
                    )
            })
        {
            let branch = status
                .as_ref()
                .and_then(|s| s.ref_name.as_ref())
                .map(|s| s.0.clone());
            if let Some(branch) = branch {
                let includes_commit = request.action == GitStackedAction::Commit
                    || (matches!(
                        request.action,
                        GitStackedAction::CommitPush | GitStackedAction::CommitPushPr
                    ) && status.as_ref().is_some_and(|s| s.has_working_tree_changes));
                self.pending.set(Some(PendingDefaultBranchAction {
                    request,
                    branch,
                    includes_commit,
                }));
            }
            return;
        }
        let Some(backend) = self.backend.get() else {
            return;
        };
        let scope = self.scope.get();
        let saved_message = request.commit_message.clone();
        self.success.set(None);
        self.initiating.set(true);
        spawn(async move {
            let result = backend.0.run_action(&scope, request).await;
            self.initiating.set(false);
            match result {
                Ok(result) => {
                    if let Some(branch) = logic::thread_branch_update(&result) {
                        self.sync_branch(Some(branch), true);
                    }
                    let created = result.pr.status == GitPrStepStatus::Created;
                    self.set_success(InlineSuccess {
                        title: if created {
                            Text::content(tr(
                                "Created {shortLabel}",
                                &[(
                                    "shortLabel",
                                    Value::Text(
                                        terminology(&self.snapshot.get_untracked())
                                            .short_label
                                            .into(),
                                    ),
                                )],
                            ))
                        } else {
                            Text::client_key(result.toast.title.0)
                        },
                        description: if created {
                            None
                        } else {
                            result
                                .toast
                                .description
                                .flatten()
                                .map(|v| Text::client_key(v.0))
                        },
                    });
                }
                Err(error) => {
                    if let Some(message) = saved_message
                        && self.message.get_untracked().is_empty()
                    {
                        self.message.set(message);
                    }
                    self.show_error("Action failed", error);
                }
            }
        });
    }
    fn quick_action(self) {
        if !self.can_run() {
            return;
        }
        let action = quick(&self.snapshot.get_untracked());
        if action.disabled {
            return;
        }
        match action.kind {
            QuickActionKind::Publish => self.publish_open.set(true),
            QuickActionKind::Run(action) => self.run(
                GitActionRequest {
                    action,
                    commit_message: None,
                    feature_branch: false,
                    file_paths: None,
                },
                false,
            ),
            QuickActionKind::Pull => {
                let Some(backend) = self.backend.get() else {
                    return;
                };
                let scope = self.scope.get();
                self.success.set(None);
                self.initiating.set(true);
                spawn(async move {
                    let result = backend.0.pull(&scope).await;
                    self.initiating.set(false);
                    match result {
                        Err(error) => self.show_error("Pull failed", error),
                        Ok(result) => {
                            let pulled = result.status == VcsPullResultStatus::Pulled;
                            self.set_success(InlineSuccess {
                                title: if pulled {
                                    "Pulled".into()
                                } else {
                                    "Already up to date".into()
                                },
                                description: Some(Text::content(if pulled {
                                    tr(
                                        "Updated {refName} from {upstream}",
                                        &[
                                            ("refName", Value::Text(result.ref_name.0)),
                                            (
                                                "upstream",
                                                Value::Text(
                                                    result
                                                        .upstream_ref
                                                        .map_or_else(|| t("upstream"), |r| r.0),
                                                ),
                                            ),
                                        ],
                                    )
                                } else {
                                    tr(
                                        "{refName} is already synchronized.",
                                        &[("refName", Value::Text(result.ref_name.0))],
                                    )
                                })),
                            });
                        }
                    }
                });
            }
            QuickActionKind::Hint(_) => (),
        }
    }
    fn commit(self, feature_branch: bool) {
        if !self.commit_open.get_untracked()
            || !self.can_run()
            || self.selected_files().is_empty()
            || (feature_branch && !self.can_change_branch())
        {
            return;
        }
        let request = GitActionRequest {
            action: GitStackedAction::Commit,
            commit_message: Some(self.message.get_untracked().trim().to_owned())
                .filter(|m| !m.is_empty()),
            feature_branch,
            file_paths: (!self.excluded.get_untracked().is_empty()).then(|| self.selected_files()),
        };
        self.reset_commit();
        self.run(request, feature_branch);
    }
}

pub fn git_actions_control(props: GitActionsControlProps) -> impl IntoElement {
    let backend = use_context::<GitActionsRuntime>();
    debug_assert!(
        backend.is_some() || use_context::<crate::runtime::Rt>().is_none_or(|rt| rt.backend().client().is_none()),
        "GitActionsRuntime is not provided: app::provide_client_services provides it with the client"
    );
    let backend = stored(backend);
    let props = stored(props);
    let scope = computed(move || props.with(|p| p.scope.get()));
    view().child(dynamic(move || {
        if backend.with(|runtime| runtime.is_none()) {
            return None;
        }
        let scope = scope.get().filter(|s| !s.cwd.is_empty())?;
        Some(untracked(|| mounted_control(scope, backend, props)))
    }))
}

fn mounted_control(
    scope: GitActionsScope,
    backend: Stored<Option<GitActionsRuntime>>,
    props: Stored<GitActionsControlProps>,
) -> AnyElement {
    let control = Control {
        scope: stored(scope),
        backend,
        snapshot: signal(GitActionsSnapshot::default()),
        commit_open: signal(false),
        message: signal(String::new()),
        excluded: signal(BTreeSet::new()),
        editing: signal(false),
        publish_open: signal(false),
        pending: signal(None),
        success: signal(None),
        initiating: signal(false),
        menu_open: signal(false),
        success_generation: signal(0),
        menu_width: signal(0.0),
        visible: props.with(|p| p.visible),
    };
    // Retained details content releases queries when its host detaches it; reopening re-subscribes.
    effect(move || {
        if !control.visible.get() {
            return;
        }
        let watch = props.with(|p| GitActionsWatch {
            show_actions: p.show_actions.get(),
            show_changes: p.show_changes.get(),
        });
        let Some(backend) = backend.get() else {
            return;
        };
        let mut updates = backend.0.watch(&control.scope.get(), watch);
        spawn(async move {
            while let Some(snapshot) = updates.next().await {
                control.snapshot.set(snapshot);
            }
        });
        untracked(|| control.refresh());
    });
    // Synchronizes draft metadata with the external checkout subscription, as the source effect.
    effect(move || {
        let snapshot = control.snapshot.get();
        if control.visible.get()
            && let Some(branch) = live_branch_update(&snapshot)
        {
            untracked(|| control.sync_branch(branch, false));
        }
    });
    effect(move || {
        if control.menu_open.get() {
            untracked(|| control.refresh());
        }
    });
    effect(move || {
        if !control.visible.get() {
            batch(|| {
                control.commit_open.set(false);
                control.publish_open.set(false);
                control.pending.set(None);
                control.menu_open.set(false);
            });
        }
    });
    // NSH's Rust builder has no status/live-region prop; announce changed status explicitly.
    let announcement = computed(move || {
        let snapshot = control.snapshot.get();
        if !control.visible.get() || !snapshot.announce_status_changes {
            return None;
        }
        progress(&snapshot)
            .map(|p| {
                let status = progress_status(&snapshot, &p);
                match p.output {
                    Some(output) => format!("{status} {output}"),
                    None => status,
                }
            })
            .or_else(|| control.success.get().map(|s| s.title.resolve().to_string()))
    });
    effect(move || {
        if let Some(message) = announcement.get() {
            announce_for_accessibility(message);
        }
    });
    let action_mode = computed(move || {
        let snapshot = control.snapshot.get();
        if !control.visible.get() || !props.with(|p| p.show_actions.get()) {
            return 0;
        }
        if snapshot.status.as_ref().is_some_and(|s| !s.is_repo) {
            return 1;
        }
        if quiet_empty(&control.scope.get(), &snapshot)
            || (props.with(|p| p.compact.get())
                && progress(&snapshot).is_none()
                && control.success.get().is_none())
        {
            return 0;
        }
        2
    });
    let changes_visible = computed(move || {
        let snapshot = control.snapshot.get();
        control.visible.get()
            && props.with(|p| p.show_changes.get())
            && snapshot.status.as_ref().is_none_or(|s| s.is_repo)
            && !quiet_empty(&control.scope.get(), &snapshot)
    });
    view()
        .app_class(tw("flex flex-col"))
        .child(dynamic(move || match action_mode.get() {
            1 => Some(untracked(|| initialize_control(control).into_element())),
            2 => Some(untracked(|| action_row(control).into_element())),
            _ => None,
        }))
        .child(when(changes_visible, move || changes_row(control, props)))
        .child(commit_dialog(control))
        .child(default_branch_dialog(control))
        .child(when(
            move || props.with(|p| p.show_actions.get()),
            move || publish_repository_dialog(control),
        ))
        .into_element()
}

// ThreadDetailsControl's feature-owned classes, shared with the details card's rows.
// Child selectors move to icons; coarse after hit-box becomes hit_slop; will-change/text-box drop.
const CONTROL_CLASS: &str = "relative inline-flex shrink-0 cursor-pointer items-center whitespace-nowrap border outline-none transition-[box-shadow,scale] focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 h-8 min-w-0 rounded-lg border-transparent gap-2.5 px-2.5 text-left text-[13px] font-medium text-foreground/80 bg-transparent shadow-none hover:bg-black/[0.035] dark:hover:bg-white/[0.08]";

/// Port of apps/web/src/components/chat/ThreadDetailsControl.tsx ThreadDetailsControl.
pub(crate) fn thread_details_control(
    part: &'static str,
    label: Prop<SharedString>,
    disabled: Prop<bool>,
    multiline: bool,
    children: impl IntoElement,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(move || Some(label.get()))
        .disabled(disabled)
        .on_press(move || {
            if !disabled.get_untracked() {
                on_press();
            }
        })
        .app_class(tw(&cn([
            Some(CONTROL_CLASS),
            Some(match part {
                "primary" => "flex-1 justify-start rounded-e-none",
                "secondary" => "w-8 justify-center rounded-s-none px-0",
                _ => "w-full justify-start",
            }),
            multiline.then_some("h-auto min-h-8 py-1.5 disabled:opacity-100 sm:h-auto"),
        ])))
        .hit_slop(move || {
            EdgeInsets::all(px(
                if crate::state::layout::pointer() == crate::state::layout::Pointer::Coarse {
                    6.0
                } else {
                    0.0
                },
            ))
        })
        .child(children)
}

fn initialize_control(control: Control) -> impl IntoElement {
    thread_details_control(
        "row",
        Prop::bind(move || {
            t(if control.initiating.get() {
                "Initializing..."
            } else {
                "Initialize Git"
            })
            .into()
        }),
        Prop::bind(move || !control.can_run()),
        false,
        view()
            .app_class(tw("flex items-center gap-2.5"))
            .child(icon_with_class(Icon::GitBranchPlus, "size-3.5"))
            .child(
                text(move || {
                    t(if control.initiating.get() {
                        "Initializing..."
                    } else {
                        "Initialize Git"
                    })
                })
                .app_class(tw("ml-0.5")),
            ),
        move || {
            if !control.can_run() {
                return;
            }
            let Some(backend) = control.backend.get() else {
                return;
            };
            let scope = control.scope.get();
            control.initiating.set(true);
            spawn(async move {
                let result = backend.0.initialize(&scope).await;
                control.initiating.set(false);
                if let Err(error) = result {
                    control.show_error("Git initialization failed", error);
                }
            });
        },
    )
}

fn action_row(control: Control) -> impl IntoElement {
    let anchor = element_ref();
    anchor.on_focus_in(move || {
        if !has_menu(control) {
            control.refresh();
        }
    });
    view().element_ref(&anchor).role(AccessibilityRole::Group).label(t("Git actions"))
        .on_mouse_enter(move |_: &MouseEvent| { if !has_menu(control) { control.refresh(); } })
        .on_layout(move |bounds: &Bounds<Points>| control.menu_width.set(bounds.size.width.0))
        .app_class(move || Some(tw(&cn([Some("shrink-0 group/details-split flex w-full items-center rounded-lg hover:bg-black/[0.035] dark:hover:bg-white/[0.08]"),
            control.menu_open.get().then_some("bg-black/[0.035] dark:bg-white/[0.08]"), control.snapshot.with(|s| progress(s).is_some()).then_some("bg-black/[0.035] dark:bg-white/[0.055]")]))))
        .child(dynamic(move || {
            let snapshot = control.snapshot.get();
            if let Some(progress) = progress(&snapshot) {
                let status = progress_status(&snapshot, &progress);
                let label = progress.output.as_ref().map_or_else(|| status.clone(), |output| format!("{status} {output}"));
                return thread_details_control("primary", label.into(), true.into(), true, git_action_progress_button_content(progress, status), || {}).into_element();
            }
            if let Some(success) = control.success.get() { return thread_details_control("primary", success.title.resolve().into(), true.into(), true, git_action_success_button_content(success), || {}).into_element(); }
            let action = quick(&snapshot);
            let reason = if !control.can_write() { Some(t("This connection cannot change source control.")) }
                else { match &action.kind { QuickActionKind::Hint(reason) if action.disabled => Some(Text::client_key(reason.clone()).resolve().to_string()), _ => None } };
            let label = action_label(&action.label, &terminology(&snapshot));
            // NSH's tooltip attaches to the existing control; it cannot reproduce openOnHover popovers.
            view().flex_1().tooltip(crate::ui::tooltip::tooltip_popup(crate::ui::tooltip::Variant::Default, text(reason.unwrap_or_else(|| label.clone()))).into_element()).child(nsh_base::button().accessibility_label(label.clone()).disabled(move || !control.can_run() || quick(&control.snapshot.get()).disabled)
                .on_press(move || control.quick_action())
                .app_class(tw(&cn([Some(CONTROL_CLASS), Some("flex-1 justify-start rounded-e-none")])))
                .child(git_quick_action_icon(&action, &snapshot))
                .child(text(label).app_class(tw("ml-0 min-w-0 truncate")))).into_element()
        }))
        .child(dynamic(move || {
            if let Some(progress) = control.snapshot.with(progress) { return git_action_elapsed_time(progress.started_at).into_element(); }
            untracked(|| view().app_class(tw("flex items-center"))
                .child(view().app_class(move || Some(tw(if control.menu_open.get() { "h-4 w-px shrink-0 bg-border/65" } else { "h-4 w-px shrink-0 bg-transparent group-hover/details-split:bg-border/65" }))))
                .child(menu::menu(thread_details_control("secondary", t("Git action options").into(), Prop::bind(move || control.initiating.get() || control.snapshot.with(busy) || !has_menu(control)), false,
                    icon_with_class(Icon::ChevronDown, "size-4 shrink-0 text-muted-foreground"), || {}), MenuOptions { open: Some(control.menu_open), align: Align::End, width: menu::Width::Content, ..Default::default() },
                    menu::menu_popup_with(move || git_items(control)))).into_element())
        }))
}

fn can_publish(control: Control) -> bool {
    control.can_write()
        && control.snapshot.with(|s| {
            s.status
                .as_ref()
                .is_some_and(|s| s.is_repo && !s.has_primary_remote)
        })
}

fn has_menu(control: Control) -> bool {
    control.snapshot.with(|s| {
        logic::usable_menu(
            &logic::menu_items(
                s.status.as_deref(),
                busy(s),
                s.status.as_ref().is_some_and(|s| s.has_primary_remote),
                &terminology(s),
            ),
            control.can_write(),
            can_publish(control),
            s.status.as_ref().is_some_and(|s| {
                s.ref_name.is_none()
                    || (!s.has_working_tree_changes && s.behind_count > 0 && s.ahead_count == 0)
            }) || s.status_error.is_some(),
        )
    })
}

fn git_items(control: Control) -> impl IntoElement {
    let snapshot = control.snapshot.get();
    let items = logic::menu_items(
        snapshot.status.as_deref(),
        busy(&snapshot),
        snapshot
            .status
            .as_ref()
            .is_some_and(|s| s.has_primary_remote),
        &terminology(&snapshot),
    );
    let rows: Vec<_> = items
        .into_iter()
        .map(|item| {
            let action = item.action;
            let disabled = item.disabled;
            let label = action_label(&item.label, &terminology(&snapshot));
            let reason = menu_action_disabled_reason(&item, &snapshot);
            view()
                .child(menu::menu_item(
                    label.clone(),
                    ItemOptions {
                        disabled: Prop::bind(move || disabled || !control.can_run()),
                        ..Default::default()
                    },
                    view()
                        .app_class(tw("flex items-center gap-2"))
                        .child(git_action_item_icon(action, &snapshot))
                        .child(menu::menu_item_label(label)),
                    move || {
                        let snapshot = control.snapshot.get_untracked();
                        let available = logic::menu_items(
                            snapshot.status.as_deref(),
                            busy(&snapshot),
                            snapshot
                                .status
                                .as_ref()
                                .is_some_and(|s| s.has_primary_remote),
                            &terminology(&snapshot),
                        )
                        .iter()
                        .any(|i| i.action == action && !i.disabled);
                        if !control.can_run() || !available {
                            return;
                        }
                        match action {
                            DialogAction::Commit => {
                                control.excluded.set(BTreeSet::new());
                                control.editing.set(false);
                                control.commit_open.set(true);
                            }
                            DialogAction::Push | DialogAction::CreatePr => control.run(
                                GitActionRequest {
                                    action: if action == DialogAction::Push {
                                        GitStackedAction::Push
                                    } else {
                                        GitStackedAction::CreatePr
                                    },
                                    commit_message: None,
                                    feature_branch: false,
                                    file_paths: None,
                                },
                                false,
                            ),
                        }
                    },
                ))
                // MenuItem has no hover-popover trigger seam; preserve its source reason inline.
                .child(reason.map(|reason| {
                    text(reason).app_class(tw("px-2 py-1.5 text-xs text-muted-foreground"))
                }))
        })
        .collect();
    view().w(move || px((control.menu_width.get() - 10.0).max(0.0))).child(rows)
        .child(when(move || can_publish(control), move || menu::menu_item(t("Publish repository..."), ItemOptions { disabled: Prop::bind(move || !control.can_run()), ..Default::default() },
            view().app_class(tw("flex items-center gap-2")).child(icon_with_class(Icon::CloudUpload, "size-4 shrink-0")).child(menu::menu_item_label(t("Publish repository..."))),
            move || { if control.can_run() && can_publish(control) { control.publish_open.set(true); } })))
        .child(snapshot.status.as_ref().filter(|s| s.ref_name.is_none()).map(|_| text(tr("Detached HEAD: create and check out a branch to enable push and {singular} actions.", &[("singular", Value::Text(Text::client_key(terminology(&snapshot).singular.to_owned()).resolve().to_string()))]))
            .app_class(tw("px-2 py-1.5 text-xs text-warning"))))
        .child(snapshot.status.as_ref().filter(|s| s.ref_name.is_some() && !s.has_working_tree_changes && s.behind_count > 0 && s.ahead_count == 0)
            .map(|_| text(t("Behind upstream. Pull/rebase first.")).app_class(tw("px-2 py-1.5 text-xs text-warning"))))
        .child(snapshot.status_error.map(|error| text(error).app_class(tw("px-2 py-1.5 text-xs text-destructive"))))
}

fn action_label(label: &str, terminology: &ChangeRequestTerminology<'_>) -> String {
    let short = terminology.short_label;
    if label == format!("Create {short}") {
        return tr(
            "Create {shortLabel}",
            &[("shortLabel", Value::Text(short.into()))],
        );
    }
    if label == format!("Commit, push & {short}") {
        return tr(
            "Commit, push & {shortLabel}",
            &[("shortLabel", Value::Text(short.into()))],
        );
    }
    if label == format!("Push & create {short}") {
        return tr(
            "Push & create {shortLabel}",
            &[("shortLabel", Value::Text(short.into()))],
        );
    }
    Text::client_key(label.to_owned()).resolve().to_string()
}

fn menu_action_disabled_reason(
    item: &logic::MenuItem,
    snapshot: &GitActionsSnapshot,
) -> Option<String> {
    if !item.disabled {
        return None;
    }
    if busy(snapshot) {
        return Some(t("Git action in progress."));
    }
    let Some(status) = &snapshot.status else {
        return Some(t("Git status is unavailable."));
    };
    Some(match item.action {
        DialogAction::Commit => t(if !status.has_working_tree_changes {
            "Worktree is clean. Make changes before committing."
        } else {
            "Commit is currently unavailable."
        }),
        DialogAction::Push => t(if status.ref_name.is_none() {
            "Detached HEAD: check out a branch before pushing."
        } else if status.has_working_tree_changes {
            "Commit or stash local changes before pushing."
        } else if status.behind_count > 0 {
            "Branch is behind upstream. Pull/rebase before pushing."
        } else if !status.has_upstream && !status.has_primary_remote {
            "Add an \"origin\" remote before pushing."
        } else if status.ahead_count == 0 {
            "No local commits to push."
        } else {
            "Push is currently unavailable."
        }),
        DialogAction::CreatePr => {
            let args = [(
                "singular",
                Value::Text(
                    Text::client_key(terminology(snapshot).singular.to_owned())
                        .resolve()
                        .to_string(),
                ),
            )];
            if status.ref_name.is_none() {
                tr(
                    "Detached HEAD: check out a branch before creating a {singular}.",
                    &args,
                )
            } else if !status.has_upstream && !status.has_primary_remote {
                tr(
                    "Add an \"origin\" remote before creating a {singular}.",
                    &args,
                )
            } else if status
                .ahead_of_default_count
                .flatten()
                .unwrap_or(status.ahead_count)
                == 0
            {
                tr("No local commits to include in a {singular}.", &args)
            } else if status.behind_count > 0 {
                tr(
                    "Branch is behind upstream. Pull/rebase before creating a {singular}.",
                    &args,
                )
            } else {
                tr("Create {singular} is currently unavailable.", &args)
            }
        }
    })
}

fn source_control_icon(snapshot: &GitActionsSnapshot, classes: &str) -> AnyElement {
    snapshot
        .presentation
        .as_ref()
        .and_then(|p| p.icon_svg.clone())
        .map_or_else(
            || icon_with_class(Icon::GitPullRequest, classes).into_element(),
            |markup| {
                svg(nsh_core::element::ImageSrc::svg_markup(&markup))
                    .app_class(tw(classes))
                    .accessible(false)
                    .into_element()
            },
        )
}

fn git_action_item_icon(action: DialogAction, snapshot: &GitActionsSnapshot) -> impl IntoElement {
    if action == DialogAction::CreatePr {
        return source_control_icon(snapshot, "size-4 shrink-0");
    }
    icon_with_class(
        match action {
            DialogAction::Commit => Icon::GitCommitHorizontal,
            DialogAction::Push => Icon::CloudUpload,
            DialogAction::CreatePr => unreachable!(),
        },
        "size-4 shrink-0",
    )
    .into_element()
}

fn git_quick_action_icon(action: &logic::QuickAction, snapshot: &GitActionsSnapshot) -> AnyElement {
    let glyph = match action.kind {
        QuickActionKind::Publish => Icon::CloudUpload,
        QuickActionKind::Pull => Icon::CloudDownload,
        QuickActionKind::Run(GitStackedAction::Commit) => Icon::GitCommitHorizontal,
        QuickActionKind::Run(GitStackedAction::Push | GitStackedAction::CommitPush) => {
            Icon::CloudUpload
        }
        QuickActionKind::Run(_) => {
            return source_control_icon(snapshot, "size-4 shrink-0 text-muted-foreground");
        }
        QuickActionKind::Hint(_) => match action.label.as_str() {
            "Commit" => Icon::GitCommitHorizontal,
            "Push" => Icon::CloudUpload,
            _ => Icon::Info,
        },
    };
    icon_with_class(glyph, "size-4 shrink-0 text-muted-foreground").into_element()
}

fn git_action_elapsed_time(started_at: Option<u64>) -> AnyElement {
    started_at.map_or_else(
        || view().into_element(),
        |start| running_git_action_elapsed_time(start).into_element(),
    )
}

fn running_git_action_elapsed_time(started_at: u64) -> impl IntoElement {
    let now = signal(nsh_core::time::unix_ms() as u64);
    set_interval(Duration::from_secs(1), move || {
        now.set(nsh_core::time::unix_ms() as u64)
    });
    text(move || logic::format_elapsed(Some(started_at), now.get()).unwrap_or_default())
        .app_class(tw("flex h-8 shrink-0 items-center self-start pe-2.5 text-2xs font-normal tabular-nums text-muted-foreground"))
}

fn git_action_progress_button_content(
    progress: logic::ProgressPresentation,
    status: String,
) -> impl IntoElement {
    // Spinner stays static: continuous repaint is prohibited by the product contract.
    view().role(AccessibilityRole::Group).app_class(tw("grid min-w-0 flex-1 items-center grid-cols-[auto_minmax(0,1fr)] grid-rows-[1.125rem] gap-x-3"))
        .child(icon_with_class(Icon::LoaderCircle, "row-start-1 -mx-0.5 size-4 shrink-0"))
        .child(text(status).app_class(tw("row-start-1 min-w-0 truncate text-left")))
        .child(progress.output.map(|output| view().app_class(tw("col-start-2 grid min-w-0"))
            .child(text(output).app_class(tw("truncate pt-0.5 text-left text-2xs font-normal text-muted-foreground")))))
}

fn git_action_success_button_content(success: InlineSuccess) -> impl IntoElement {
    view().role(AccessibilityRole::Group).app_class(tw("grid min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)] grid-rows-[1.125rem] items-center gap-x-3"))
        .child(icon_with_class(Icon::Check, "size-3.5 shrink-0 text-success"))
        .child(text(success.title.resolve()).app_class(tw("min-w-0 truncate text-left")))
        .child(success.description.map(|description| view().app_class(tw("col-start-2 grid min-w-0"))
            .child(text(description.resolve()).app_class(tw("truncate pt-0.5 text-left text-2xs font-normal text-muted-foreground")))))
}

fn changes_row(control: Control, props: Stored<GitActionsControlProps>) -> impl IntoElement {
    let totals = computed(move || {
        control
            .snapshot
            .with(|s| logic::changes_totals(s.status.as_deref(), s.branch_preview.clone()))
    });
    thread_details_control("row", t("Changes").into(), Prop::bind(move || props.with(|p| p.on_open_changes.is_none())), false,
        view().app_class(tw("flex w-full min-w-0 items-center gap-2.5"))
            .child(icon_with_class(Icon::FileDiff, "size-4 shrink-0 text-muted-foreground"))
            .child(view().app_class(tw("flex min-w-0 flex-1 items-baseline gap-1.5"))
                .child(text(t("Changes")).app_class(tw("shrink-0")))
                .child(text(move || totals.get().map_or_else(String::new, |v| v.base_ref.map_or_else(|| t("Uncommitted"), |base| tr("vs {baseRef}", &[("baseRef", Value::Text(base))])))).app_class(tw("pointer-events-auto min-w-0 truncate font-normal text-muted-foreground")))
                .child(view().app_class(tw("ml-auto flex shrink-0 items-baseline gap-2 ps-2 font-mono tabular-nums"))
                    .child(text(move || format!("+{}", crate::i18n::formats().decimal(totals.get().map_or(0, |t| t.insertions) as f64))).app_class(move || Some(tw(if totals.get().is_some_and(|t| t.insertions > 0) { "text-diff-addition-foreground" } else { "text-muted-foreground" }))))
                    .child(text(move || format!("-{}", crate::i18n::formats().decimal(totals.get().map_or(0, |t| t.deletions) as f64))).app_class(move || Some(tw(if totals.get().is_some_and(|t| t.deletions > 0) { "text-diff-deletion-foreground" } else { "text-muted-foreground" })))))),
        move || { if let Some(open) = props.with(|p| p.on_open_changes.clone()) { open(); } })
}

fn commit_dialog(control: Control) -> impl IntoElement {
    dialog::dialog(control.commit_open, move |state| {
        state.on_open_change(move |open| {
            if open {
                control.commit_open.set(true);
            } else {
                control.reset_commit();
            }
        });
        dialog::dialog_popup(state, PopupOptions::default(), move || {
            view()
            .child(dialog::dialog_header(state, view()
                .child(dialog::dialog_title(state, || "Commit changes".into()))
                .child(dialog::dialog_description(state, || "Review and confirm your commit. Leave the message blank to auto-generate one.".into()))))
            .child(dialog::dialog_panel(state, view()
                .child(commit_review(control))
                .child(view().app_class(tw("space-y-1"))
                    .child(text(t("Commit message (optional)")).app_class(tw("text-sm font-medium")))
                    // UI-FORM has no Textarea yet; the NSH multiline input preserves its contents.
                    .child(nsh_base::input().test_id("git-commit-message").label(t("Commit message (optional)"))
                        .multiline(true).value(move || control.message.get()).on_change(move |value: &str| control.message.set(value.to_owned()))
                        .placeholder(t("Leave empty to auto-generate")).app_class(tw("min-h-16 w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground"))))))
            .child(dialog::dialog_footer(state, FooterVariant::Bare, view().app_class(tw("flex flex-col-reverse gap-2 sm:flex-row sm:justify-end"))
                .child(dialog::dialog_close(state, ButtonContent::Label(t("Cancel")), ButtonVariant::Outline, ButtonSize::Sm, || false))
                .child(dialog::dialog_action(state, ButtonContent::Label(t("Commit on new branch")), ActionVariant::Outline, ButtonSize::Sm, move || !control.can_change_branch() || !control.can_run() || control.selected_files().is_empty(), move || control.commit(true)))
                .child(dialog::dialog_action(state, ButtonContent::Label(t("Commit")), ActionVariant::Default, ButtonSize::Sm, move || !control.can_run() || control.selected_files().is_empty(), move || control.commit(false)))))
        })
    })
}

fn commit_review(control: Control) -> impl IntoElement {
    view().app_class(tw("space-y-3 rounded-xl bg-zinc-25 p-3 text-sm ring-1 ring-black/5 dark:bg-white/[0.035] dark:ring-white/5"))
        .child(view().app_class(tw("grid grid-cols-[auto_1fr] items-center gap-x-2 gap-y-1"))
            .child(text(t("Branch")).app_class(tw("text-muted-foreground")))
            .child(view().app_class(tw("flex items-center justify-between gap-2"))
                .child(text(move || control.snapshot.with(|s| s.status.as_ref().and_then(|s| s.ref_name.as_ref()).map_or_else(|| t("(detached HEAD)"), |n| n.0.clone()))).app_class(tw("font-medium")))
                .child(when(move || control.snapshot.with(|s| s.status.as_ref().is_some_and(|s| s.is_default_ref)), || text(t("Default branch")).app_class(tw("text-right text-warning"))))))
        .child(view().app_class(tw("space-y-1"))
            .child(view().app_class(tw("flex items-center justify-between"))
                .child(view().app_class(tw("flex items-center gap-2"))
                    .child(when(move || control.editing.get(), move || file_checkbox(t("Files"), Prop::bind(move || control.excluded.with(|s| s.is_empty())), Prop::bind(move || !control.excluded.get().is_empty() && !control.selected_files().is_empty()), Prop::bind(move || !control.can_run()), move || {
                        if !control.can_run() { return; }
                        if control.excluded.get_untracked().is_empty() { control.excluded.set(control.snapshot.with(|s| s.status.as_ref().map(|s| s.working_tree.files.iter().map(|f| f.path.clone()).collect()).unwrap_or_default())); }
                        else { control.excluded.set(BTreeSet::new()); }
                    })))
                    .child(text(t("Files")).app_class(tw("text-muted-foreground")))
                    .child(text(move || if !control.editing.get() && !control.excluded.get().is_empty() { tr("({selected} of {total})", &[("selected", Value::Int(control.selected_files().len() as i64)), ("total", Value::Int(control.snapshot.with(|s| s.status.as_ref().map_or(0, |s| s.working_tree.files.len())) as i64))]) } else { String::new() }).app_class(tw("text-muted-foreground"))))
                .child(dynamic(move || button(ButtonContent::Label(t(if control.editing.get() { "Done" } else { "Edit" })), ButtonVariant::Ghost, ButtonSize::Xs, || false, move || control.editing.update(|v| *v = !*v)).into_element())))
            .child(dynamic(move || {
                let files = control.snapshot.with(|s| s.status.as_ref().map(|s| s.working_tree.files.clone()).unwrap_or_default());
                if files.is_empty() { return text(t("none")).app_class(tw("font-medium")).into_element(); }
                view().app_class(tw("space-y-2"))
                    .child(view().app_class(tw("h-44 rounded-lg bg-card ring-1 ring-black/5 dark:bg-white/[0.025] dark:ring-white/5"))
                        .child(scroll().h_full().child(view().app_class(tw("space-y-1 p-1")).child(files.into_iter().map(move |file| commit_file(control, file))))))
                    .child(text(move || control.snapshot.with(|s| {
                        let (mut additions, mut deletions) = (0, 0);
                        if let Some(status) = &s.status { for file in &status.working_tree.files { if !control.excluded.with(|p| p.contains(&file.path)) { additions += file.insertions; deletions += file.deletions; } } }
                        format!("+{} / -{}", crate::i18n::formats().decimal(additions as f64), crate::i18n::formats().decimal(deletions as f64))
                    })).app_class(tw("flex justify-end font-mono"))).into_element()
            })))
}

fn commit_file(control: Control, file: VcsStatusResultWorkingTreeFiles) -> impl IntoElement {
    let path = stored(file.path);
    view().app_class(tw("flex w-full items-center gap-2 rounded-md px-2 py-1 font-mono hover:bg-accent/50"))
        .child(when(move || control.editing.get(), move || file_checkbox(path.get(), Prop::bind(move || !control.excluded.with(|p| p.contains(&path.get()))), false.into(), Prop::bind(move || !control.can_run()), move || {
            if !control.can_run() { return; }
            control.excluded.update(|paths| { if !paths.remove(&path.get()) { paths.insert(path.get()); } });
        })))
        .child(nsh_base::button().accessibility_label(tr("Open {path} in editor", &[("path", Value::Text(path.get()))]))
            .disabled(move || !control.snapshot.with(|s| s.can_operate && s.status.as_ref().is_some_and(|s| s.working_tree.root_path.as_ref().is_some_and(|p| p.is_some()))))
            .on_press(move || {
                if !control.visible.get_untracked() || !control.snapshot.with(|s| s.can_operate) { return; }
                let root = control.snapshot.with(|s| s.status.as_ref().and_then(|s| s.working_tree.root_path.clone().flatten()));
                if let (Some(backend), Some(root)) = (control.backend.get(), root) {
                    let scope = control.scope.get(); let path = path.get();
                    spawn(async move { if let Err(error) = backend.0.open_file(&scope, root, path).await { control.show_error("Unable to open file", error); } });
                }
            })
            .app_class(tw("flex min-w-0 flex-1 items-center justify-between gap-3 text-left enabled:cursor-pointer"))
            // StartTruncatedPath's primitive seam is not available yet.
            .child(text(path.get()).app_class(move || Some(tw(if control.excluded.with(|p| p.contains(&path.get())) { "flex-1 truncate text-muted-foreground" } else { "flex-1 truncate" }))))
            .child(text(move || if control.excluded.with(|p| p.contains(&path.get())) { t("Excluded") } else { format!("+{} / -{}", crate::i18n::formats().decimal(file.insertions as f64), crate::i18n::formats().decimal(file.deletions as f64)) }).app_class(tw("shrink-0"))))
}

fn file_checkbox(
    label: String,
    checked: Prop<bool>,
    mixed: Prop<bool>,
    disabled: Prop<bool>,
    change: impl Fn() + 'static,
) -> impl IntoElement {
    // Checkbox's public label translates its input; the raw control preserves a literal file path.
    nsh_base::checkbox()
        .accessibility_label(Some(SharedString::from(label)))
        .checked(checked)
        .indeterminate(mixed)
        .disabled(disabled)
        .indicator(view().child(dynamic(move || {
            if mixed.get() {
                view().app_class(tw("relative inline-flex size-4.5 sm:size-4 items-center justify-center rounded-[.25rem] border border-input bg-background"))
                    .child(svg(nsh_core::element::ImageSrc::svg_markup(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round"><path d="M5.252 12h13.496"/></svg>"#)).app_class(tw("size-3.5 text-foreground sm:size-3"))).into_element()
            } else { crate::ui::switch::checkbox_mark(checked, crate::ui::switch::Variant::Default).into_element() }
        })))
        .on_change(move |_: &bool| {
            if !disabled.get_untracked() {
                change();
            }
        })
}

fn default_branch_dialog(control: Control) -> impl IntoElement {
    let open = signal(false);
    effect(move || open.set(control.pending.with(|p| p.is_some())));
    dialog::dialog(open, move |state| {
        state.on_open_change(move |next| {
            if !next {
                control.pending.set(None);
            }
        });
        dialog::dialog_popup(state, PopupOptions::default(), move || {
            let copy = computed(move || default_branch_copy(control));
            let unavailable = computed(move || {
                control.snapshot.with(|s| {
                    s.status
                        .as_ref()
                        .is_some_and(|s| !s.has_working_tree_changes)
                })
            });
            view().child(dialog::dialog_header(state, view()
                .child(dialog::dialog_title(state, move || Text::client_key(copy.get().0)))
                .child(text(move || copy.get().1).app_class(tw("text-muted-foreground text-sm")))
                .child(when(unavailable, move || text(move || {
                    let branch = control.pending.get().map_or_else(String::new, |pending| pending.branch);
                    tr("There are no changes to commit, so a feature branch can't be named. Create a branch yourself or continue on {branch}.", &[("branch", Value::Text(branch))])
                }).app_class(tw("text-xs text-muted-foreground"))))))
                .child(dialog::dialog_footer(state, FooterVariant::Bare, view().app_class(tw("flex flex-col-reverse gap-2 sm:flex-row sm:flex-wrap sm:items-center"))
                    .child(dialog::dialog_close(state, ButtonContent::Label(t("Abort")), ButtonVariant::Outline, ButtonSize::Sm, || false))
                    .child(dynamic(move || dialog::dialog_action(state, ButtonContent::Label(copy.get().2), ActionVariant::Outline, ButtonSize::Sm,
                        move || !control.can_run(), move || continue_default(control, false)).into_element()))
                    .child(dialog::dialog_action(state, ButtonContent::Label(t("Check out feature branch & continue")), ActionVariant::Default, ButtonSize::Sm,
                        move || !control.can_run() || !control.can_change_branch() || unavailable.get(), move || {
                            if !unavailable.get_untracked() && control.can_change_branch() { continue_default(control, true); }
                        }))))
        })
    })
}

fn continue_default(control: Control, feature: bool) {
    if !control.can_run() {
        return;
    }
    let pending = control.pending.get_untracked();
    control.pending.set(None);
    if let Some(mut pending) = pending {
        pending.request.feature_branch = feature;
        control.run(pending.request, true);
    }
}

fn default_branch_copy(control: Control) -> (String, String, String) {
    let Some(pending) = control.pending.get() else {
        return (
            "Run action on default branch?".into(),
            String::new(),
            t("Continue"),
        );
    };
    let snapshot = control.snapshot.get();
    let request = pending.request;
    let branch = pending.branch.as_str();
    let includes = pending.includes_commit;
    let terms = terminology(&snapshot);
    let copy = logic::default_branch_dialog_copy(&request.action, branch, includes, &terms);
    let args = [
        ("branch", Value::Text(branch.into())),
        ("shortLabel", Value::Text(terms.short_label.into())),
        (
            "singular",
            Value::Text(
                Text::client_key(terms.singular.to_owned())
                    .resolve()
                    .to_string(),
            ),
        ),
    ];
    let push = matches!(
        request.action,
        GitStackedAction::Push | GitStackedAction::CommitPush
    );
    let description = match (push, includes) {
        (true, true) => tr(
            "This action will commit and push changes on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
            &args,
        ),
        (true, false) => tr(
            "This action will push local commits on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
            &args,
        ),
        (false, true) => tr(
            "This action will commit, push, and create a {singular} on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
            &args,
        ),
        (false, false) => tr(
            "This action will push local commits and create a {singular} on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
            &args,
        ),
    };
    let label = match (push, includes) {
        (true, true) => tr("Commit & push to {branch}", &args),
        (true, false) => tr("Push to {branch}", &args),
        (false, true) => tr("Commit, push & create {shortLabel}", &args),
        (false, false) => tr("Push & create {shortLabel}", &args),
    };
    (copy.title, description, label)
}

#[derive(Clone, Copy)]
struct PublishState {
    provider: Signal<Option<String>>,
    repository: Signal<Option<String>>,
    visibility: Signal<SourceControlRepositoryVisibility>,
    remote: Signal<String>,
    protocol: Signal<SourceControlCloneProtocol>,
    step: Signal<usize>,
    advanced: Signal<bool>,
    error: Signal<Option<String>>,
    result: Signal<Option<SourceControlPublishRepositoryResult>>,
    pending: Signal<bool>,
    generation: Signal<u64>,
}

fn get_publish_provider_readiness(
    kind: &str,
    snapshot: &GitActionsSnapshot,
) -> (bool, Option<Text>) {
    if t3_client::operations::add_project_remote_source_readiness(
        snapshot.discovery.as_deref(),
        kind,
    )
    .ready
    {
        return (true, None);
    }
    let Some(provider) = snapshot
        .discovery
        .as_ref()
        .and_then(|d| d.source_control_providers.iter().find(|p| p.kind.0 == kind))
    else {
        return (
            false,
            Some("Provider status unavailable. Open Settings -> Source control and rescan.".into()),
        );
    };
    if provider.status != SourceControlDiscoveryStatus::Available {
        return (
            false,
            Some(Text::client_key(provider.install_hint.0.clone())),
        );
    }
    if provider.auth.status == SourceControlProviderAuthStatus::Unauthenticated {
        return (
            false,
            Some(match &provider.auth.detail {
                WireOption::Some { value } => Text::content(value.0.clone()),
                WireOption::None => Text::content(tr(
                    "{provider} is not authenticated. Open Settings -> Source control for setup guidance.",
                    &[("provider", Value::Text(provider.label.0.clone()))],
                )),
            }),
        );
    }
    (true, None)
}

impl PublishState {
    fn reset(self) {
        batch(|| {
            self.generation.update(|g| *g += 1);
            self.pending.set(false);
            self.remote.set("origin".into());
            self.repository.set(None);
            self.step.set(0);
            self.advanced.set(false);
            self.error.set(None);
            self.result.set(None);
        });
    }
    fn offered(self, control: Control) -> Vec<PublishProvider> {
        control.snapshot.with(|s| {
            let definitions: Vec<_> = s
                .publish_providers
                .iter()
                .map(|p| (p.kind.as_str(), p.label.as_str()))
                .collect();
            let default_kind = s.publish_providers.first().map_or("", |p| p.kind.as_str());
            t3_client::operations::sort_add_project_provider_sources(
                &definitions,
                default_kind,
                s.offered_providers.as_ref(),
                |kind| get_publish_provider_readiness(kind, s).0,
            )
            .into_iter()
            .filter_map(|kind| s.publish_providers.iter().find(|p| p.kind == kind).cloned())
            .collect()
        })
    }
    fn selected(self, control: Control) -> Option<PublishProvider> {
        let offered = self.offered(control);
        let snapshot = control.snapshot.get();
        let selected = self
            .provider
            .get()
            .and_then(|kind| offered.iter().find(|p| p.kind == kind).cloned());
        selected
            .as_ref()
            .filter(|p| get_publish_provider_readiness(&p.kind, &snapshot).0)
            .cloned()
            .or_else(|| {
                offered
                    .iter()
                    .find(|p| get_publish_provider_readiness(&p.kind, &snapshot).0)
                    .cloned()
            })
            .or(selected)
            .or_else(|| snapshot.publish_providers.first().cloned())
    }
    fn repository(self, control: Control) -> String {
        self.repository.get().unwrap_or_else(|| {
            self.selected(control)
                .and_then(|selected| {
                    control.snapshot.with(|s| {
                        s.discovery
                            .as_ref()
                            .and_then(|d| {
                                d.source_control_providers
                                    .iter()
                                    .find(|p| p.kind.0 == selected.kind)
                            })
                            .and_then(|p| match &p.auth.account {
                                WireOption::Some { value } => Some(format!("{}/", value.0)),
                                WireOption::None => None,
                            })
                    })
                })
                .unwrap_or_default()
        })
    }
    fn can_submit(self, control: Control) -> bool {
        let path = self.repository(control);
        let (owner, name) = path.trim().split_once('/').unwrap_or(("", ""));
        control.can_run()
            && !self.pending.get()
            && !owner.trim().is_empty()
            && !name.trim().is_empty()
            && self.selected(control).is_some_and(|p| {
                self.offered(control)
                    .iter()
                    .any(|offered| offered.kind == p.kind)
                    && get_publish_provider_readiness(&p.kind, &control.snapshot.get()).0
            })
    }
    fn submit(self, control: Control) {
        if !self.can_submit(control) {
            return;
        }
        let (Some(backend), Some(provider)) = (control.backend.get(), self.selected(control))
        else {
            return;
        };
        let scope = control.scope.get();
        let input = SourceControlPublishRepositoryInput {
            cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(scope.cwd.clone()),
            provider: SourceControlProviderKind(provider.kind),
            repository: t3_contracts::base_schemas::TrimmedNonEmptyString(
                self.repository(control).trim().into(),
            ),
            visibility: self.visibility.get(),
            remote_name: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                if self.remote.get().trim().is_empty() {
                    "origin".into()
                } else {
                    self.remote.get().trim().into()
                },
            ))),
            protocol: Some(Some(self.protocol.get())),
        };
        let generation = self.generation.get_untracked();
        self.error.set(None);
        self.pending.set(true);
        spawn(async move {
            let result = backend.0.publish(&scope, input).await;
            if self.generation.get_untracked() != generation {
                return;
            }
            self.pending.set(false);
            match result {
                Ok(result) => {
                    self.result.set(Some(result));
                    self.step.set(2);
                }
                Err(ClientError::Cancelled) => (),
                Err(error) => self.error.set(Some(error.to_string())),
            }
        });
    }
}

fn publish_provider_icon(provider: &PublishProvider, classes: &str) -> impl IntoElement + use<> {
    svg(nsh_core::element::ImageSrc::svg_markup(&provider.icon_svg))
        .app_class(tw(classes))
        .accessible(false)
}

fn publish_repository_dialog(control: Control) -> impl IntoElement {
    let publish = PublishState {
        provider: signal(None),
        repository: signal(None),
        visibility: signal(SourceControlRepositoryVisibility::Private),
        remote: signal("origin".into()),
        protocol: signal(SourceControlCloneProtocol::Ssh),
        step: signal(0),
        advanced: signal(false),
        error: signal(None),
        result: signal(None),
        pending: signal(false),
        generation: signal(0),
    };
    let nav = use_context::<crate::state::nav::Navigator>();
    dialog::dialog(control.publish_open, move |state| {
        state.on_open_change(move |open| {
            control.publish_open.set(open);
            if !open {
                publish.reset();
            }
        });
        // UI-WIZARD is a stub; Dialog supplies focus/dismissal until WizardPopup is available.
        dialog::dialog_popup(state, PopupOptions::default(), move || {
            view()
            .child(dialog::dialog_header(state, view()
                .child(dialog::dialog_title(state, || "Publish repository".into()))
                .child(dialog::dialog_description(state, || "Pick where to host it, then point us at a repo to push to.".into()))
                .child(wizard_steps(control, publish))))
            .child(view().app_class(tw("min-w-0 space-y-4 bg-background/80 px-6 py-5 ring-1 ring-black/5 dark:bg-white/2 dark:ring-white/5"))
                .child(when(move || publish.step.get() == 0, move || publish_provider_panel(control, publish, nav)))
                .child(when(move || publish.step.get() == 1, move || publish_repository_panel(control, publish)))
                .child(when(move || publish.step.get() == 2, move || publish_summary(control, publish))))
            // Every footer button stays mounted and is hidden by step: NSH panics when a press
            // handler disposes its own button (Next and Publish change the step they render for).
            .child(dialog::dialog_footer(state, FooterVariant::Bare, view().app_class(tw("flex flex-col-reverse gap-2 sm:flex-row sm:justify-end"))
                .child(view().app_class(move || Some(tw(if publish.step.get() == 2 { "" } else { "hidden" })))
                    .child(dialog::dialog_close(state, ButtonContent::Label(t("Done")), ButtonVariant::Default, ButtonSize::Default, || false)))
                .child(view().app_class(move || Some(tw(if publish.step.get() == 2 { "hidden" } else { "flex flex-col-reverse gap-2 sm:flex-row" })))
                    .child(view().app_class(move || Some(tw(if publish.step.get() == 0 { "" } else { "hidden" })))
                        .child(button(ButtonContent::Label(t("Cancel")), ButtonVariant::Outline, ButtonSize::Default, move || publish.pending.get(), move || {
                            if !publish.pending.get_untracked() { state.close(); }
                        })))
                    .child(view().app_class(move || Some(tw(if publish.step.get() == 1 { "" } else { "hidden" })))
                        .child(button(ButtonContent::Label(t("Back")), ButtonVariant::Outline, ButtonSize::Default, move || publish.pending.get(), move || {
                            if !publish.pending.get_untracked() { publish.step.set(0); }
                        })))
                    .child(view().app_class(move || Some(tw(if publish.step.get() == 0 { "" } else { "hidden" })))
                        .child(dialog::dialog_action(state, ButtonContent::Label(t("Next")), ActionVariant::Default, ButtonSize::Default,
                            move || !publish.offered(control).iter().any(|p| get_publish_provider_readiness(&p.kind, &control.snapshot.get()).0), move || {
                                if publish.step.get_untracked() == 0 && publish.offered(control).iter().any(|p| get_publish_provider_readiness(&p.kind, &control.snapshot.get()).0) { publish.step.set(1); }
                            })))
                    .child(view().app_class(move || Some(tw(if publish.step.get() == 1 { "" } else { "hidden" })))
                        .child(dialog::dialog_action(state, ButtonContent::Custom {
                                label: t("Publish"),
                                child: text(move || t(if publish.pending.get() { "Publishing..." } else { "Publish" })).into_element(),
                            }, ActionVariant::Default, ButtonSize::Default,
                            move || !publish.can_submit(control), move || {
                                if publish.step.get_untracked() == 1 { publish.submit(control); }
                            }))))))
        })
    })
}

/// Port of apps/web/src/components/ui/wizard.tsx WizardSteps.
fn wizard_steps(control: Control, publish: PublishState) -> impl IntoElement {
    view().role(AccessibilityRole::List).label(t("Setup progress"))
        .app_class(tw("grid auto-cols-fr grid-flow-col gap-1 rounded-xl bg-background p-1 ring-1 ring-black/5 dark:bg-white/4 dark:ring-white/5"))
        .child(["Provider", "Repository", "Summary"].into_iter().enumerate().map(move |(index, key)| {
            nsh_base::button().accessibility_label(tr("{step}, step {number}", &[("step", Value::Text(t(key))), ("number", Value::Int(index as i64 + 1))]))
                .disabled(move || publish.step.get() == 2 || index >= 2 || index > publish.step.get())
                .on_press(move || { if publish.step.get_untracked() != 2 && index < 2 && index <= publish.step.get_untracked() { publish.step.set(index); } })
                .app_class(move || Some(tw(&cn([Some("flex w-full min-w-0 items-center gap-2 rounded-lg px-2.5 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring max-sm:justify-center max-sm:px-2 cursor-pointer hover:bg-card disabled:cursor-default disabled:hover:bg-transparent"),
                    (publish.step.get() == index).then_some("bg-card text-foreground shadow-xs ring-1 ring-inset ring-black/5 hover:bg-card dark:shadow-none dark:ring-white/5")]))))
                .child(view().app_class(move || Some(tw(&cn([Some("grid size-5 shrink-0 place-items-center rounded-full text-sm font-medium ring-1"), Some(if index < publish.step.get() { "bg-primary text-primary-foreground ring-primary" } else if index == publish.step.get() { "bg-primary/10 text-primary ring-primary/30" } else { "bg-card text-muted-foreground ring-black/10 dark:bg-white/5 dark:ring-white/10" })]))))
                    .child(dynamic(move || if index < publish.step.get() { icon_with_class(Icon::Check, "size-4 shrink-0").into_element() } else { text((index + 1).to_string()).into_element() })))
                .child(text(move || {
                    let summary = if index == 0 { publish.selected(control).map(|p| p.label) } else if index == 1 { publish.result.get().map(|r| r.repository.name_with_owner.0) } else { None };
                    if index < publish.step.get() && let Some(summary) = summary { tr("{step}: {summary}", &[("step", Value::Text(t(key))), ("summary", Value::Text(summary))]) } else { t(key) }
                }).app_class(tw("min-w-0 truncate text-sm font-medium max-sm:hidden")))
        }))
}

fn publish_provider_panel(
    control: Control,
    publish: PublishState,
    nav: Option<crate::state::nav::Navigator>,
) -> impl IntoElement {
    view()
        .app_class(tw("space-y-2"))
        .child(text(t("Provider")).app_class(tw("text-xs font-medium text-foreground")))
        .child(
            nsh_base::radio_group()
                .accessibility_label(Some(SharedString::from(t("Provider"))))
                .app_class(tw("grid grid-cols-2"))
                .child(dynamic(move || {
                    let snapshot = control.snapshot.get();
                    publish
                        .offered(control)
                        .into_iter()
                        .map(|provider| {
                            publish_provider_card(control, publish, nav, provider, &snapshot)
                        })
                        .collect::<Vec<_>>()
                })),
        )
}

fn publish_provider_card(
    control: Control,
    publish: PublishState,
    nav: Option<crate::state::nav::Navigator>,
    provider: PublishProvider,
    snapshot: &GitActionsSnapshot,
) -> AnyElement {
    let (ready, hint) = get_publish_provider_readiness(&provider.kind, snapshot);
    let provider = stored(provider);
    if !ready {
        return view().app_class(tw("relative flex cursor-not-allowed items-center gap-3 rounded-lg border border-border bg-background px-3 py-3 text-left opacity-64 dark:border-transparent dark:bg-white/[0.035]"))
            .child(publish_provider_icon(&provider.get(), "size-5 shrink-0 text-muted-foreground"))
            .child(text(provider.get().label).app_class(tw("min-w-0 flex-1 truncate text-sm font-medium text-foreground")))
            .child(view().tooltip(crate::ui::tooltip::tooltip_popup(crate::ui::tooltip::Variant::Default, text(hint.unwrap_or_else(|| Text::from("Open Settings -> Source control to configure this provider.")).resolve())).into_element())
                .child(button(ButtonContent::Label(t("Setup Required")), ButtonVariant::WarningOutline, ButtonSize::Xs, move || nav.is_none(), move || {
                    control.publish_open.set(false); publish.reset();
                    if let Some(nav) = nav { nav.open(crate::state::nav::Route::parse("/settings/source-control")); }
                }))).into_element();
    }
    nsh_base::radio().accessibility_label(Some(SharedString::from(provider.get().label)))
        .checked(move || publish.selected(control).is_some_and(|p| p.kind == provider.get().kind))
        .on_change(move |_: &bool| { if get_publish_provider_readiness(&provider.get().kind, &control.snapshot.get()).0 { publish.provider.set(Some(provider.get().kind)); publish.repository.set(None); } })
        .app_class(move || Some(tw(&cn([Some("relative flex cursor-pointer items-center gap-3 rounded-lg border px-3 py-3 text-left outline-none transition-[background-color,border-color,box-shadow] focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"),
            Some(if publish.selected(control).is_some_and(|p| p.kind == provider.get().kind) { "border-primary bg-background shadow-sm ring-2 ring-inset ring-primary/35 dark:border-transparent dark:bg-primary/10 dark:shadow-none dark:ring-1 dark:ring-primary/30" } else { "border-border bg-background hover:border-foreground/20 hover:bg-muted/50 dark:border-transparent dark:bg-white/[0.035] dark:hover:bg-accent" })]))))
        .child(publish_provider_icon(&provider.get(), "size-5 shrink-0"))
        .child(text(provider.get().label).app_class(tw("min-w-0 flex-1 truncate text-sm font-medium text-foreground"))).into_element()
}

fn publish_repository_panel(control: Control, publish: PublishState) -> impl IntoElement {
    view().app_class(tw("space-y-5"))
        .child(view().app_class(tw("space-y-2"))
            .child(text(t("Repository")).app_class(tw("text-xs font-medium text-foreground")))
            .child(view().app_class(tw("flex items-stretch overflow-hidden rounded-md border border-input bg-background focus-within:outline-2 focus-within:-outline-offset-2 focus-within:outline-ring"))
                .child(view().app_class(tw("flex shrink-0 items-center gap-1.5 border-r border-input bg-muted/50 px-2.5 font-mono text-xs text-muted-foreground"))
                    .child(dynamic(move || publish.selected(control).map(|p| publish_provider_icon(&p, "size-3.5").into_element())))
                    .child(text(move || publish.selected(control).map_or_else(String::new, |p| format!("{}/", p.host)))))
                .child(nsh_base::input().test_id("publish-repository-path").label(t("Repository")).value(move || publish.repository(control))
                    .on_change(move |value: &str| publish.repository.set(Some(value.to_owned()))).on_submit(move |_: &str| publish.submit(control))
                    .placeholder(move || publish.selected(control).map_or_else(String::new, |p| Text::client_key(p.path_placeholder).resolve().to_string()))
                    .disabled(move || publish.pending.get()).app_class(tw("w-full bg-transparent px-3 py-2 font-mono text-sm placeholder:text-muted-foreground/60 focus:outline-none")))))
        .child(view().app_class(tw("space-y-2"))
            .child(text(t("Visibility")).app_class(tw("text-xs font-medium text-foreground")))
            .child(nsh_base::radio_group().accessibility_label(Some(SharedString::from(t("Visibility")))).app_class(tw("grid grid-cols-2"))
                .child([(SourceControlRepositoryVisibility::Private, "Private", "Only invited people", Icon::Lock), (SourceControlRepositoryVisibility::Public, "Public", "Anyone on the web", Icon::Globe)].into_iter().map(move |(visibility, label, description, icon)| {
                    let visibility = stored(visibility);
                    nsh_base::radio().accessibility_label(Some(SharedString::from(t(label)))).checked(move || publish.visibility.get() == visibility.get()).disabled(move || publish.pending.get())
                        .on_change(move |_: &bool| { if !publish.pending.get_untracked() { publish.visibility.set(visibility.get()); } })
                        .app_class(move || Some(tw(&cn([Some("relative flex cursor-pointer items-center gap-3 rounded-lg border px-3 py-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"),
                            Some(if publish.visibility.get() == visibility.get() { "border-primary bg-background shadow-sm ring-2 ring-inset ring-primary/35 dark:border-transparent dark:bg-primary/10 dark:shadow-none dark:ring-1 dark:ring-primary/30" } else { "border-border bg-background hover:border-foreground/20 hover:bg-muted/50 dark:border-transparent dark:bg-white/[0.035] dark:hover:bg-accent" })]))))
                        .child(icon_with_class(icon, "size-4 shrink-0 text-muted-foreground"))
                        .child(view().app_class(tw("min-w-0 flex-1")).child(text(t(label)).app_class(tw("text-sm font-medium text-foreground"))).child(text(t(description)).app_class(tw("text-xs text-muted-foreground"))))
                }))))
        .child(view().child(nsh_base::button().accessibility_label(t("Advanced")).on_press(move || publish.advanced.update(|v| *v = !*v))
            .app_class(tw("flex items-center gap-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground"))
            .child(icon_with_class(Icon::ChevronDown, "size-3.5")).child(text(t("Advanced"))))
            .child(when(move || publish.advanced.get(), move || view().app_class(tw("mt-3 grid gap-3 sm:grid-cols-2"))
                .child(view().app_class(tw("space-y-1.5"))
                    .child(text(t("Remote")).app_class(tw("text-xs font-medium text-foreground")))
                    .child(nsh_base::input().label(t("Remote")).value(move || publish.remote.get()).on_change(move |value: &str| publish.remote.set(value.to_owned())).placeholder(t("origin")).disabled(move || publish.pending.get())
                        .app_class(tw("h-8 w-full rounded-md border border-input bg-background px-3 text-sm"))))
                .child(view().app_class(tw("space-y-1.5"))
                    .child(text(t("Protocol")).app_class(tw("text-xs font-medium text-foreground")))
                    .child(publish_protocol_control(publish))))))
        .child(when(move || publish.pending.get(), move || view().role(AccessibilityRole::Group).app_class(tw("flex items-center gap-2 rounded-md border border-input bg-muted/40 px-3 py-2 text-xs text-muted-foreground dark:border-transparent dark:bg-white/[0.035]"))
            .child(icon_with_class(Icon::LoaderCircle, "size-4"))
            .child(text(move || tr("Publishing repository to {provider}...", &[("provider", Value::Text(publish.selected(control).map_or_else(String::new, |p| p.label)))])))))
        .child(when(move || publish.error.get().is_some() && !publish.pending.get(), move || view().role(AccessibilityRole::Alert).app_class(tw("rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"))
            .child(text(t("Publish failed")).app_class(tw("font-medium"))).child(text(move || publish.error.get().unwrap_or_default()).app_class(tw("mt-0.5 text-destructive/90")))))
}

fn publish_protocol_control(publish: PublishState) -> impl IntoElement {
    use crate::ui::toggle::{Content, GroupItem, GroupOptions, toggle_group};
    toggle_group(
        t("Protocol"),
        vec![
            GroupItem::new("ssh", Content::Label(t("SSH"))).into(),
            GroupItem::new("https", Content::Label(t("HTTPS"))).into(),
        ],
        GroupOptions::default(),
        move || {
            vec![
                if publish.protocol.get() == SourceControlCloneProtocol::Ssh {
                    "ssh".into()
                } else {
                    "https".into()
                },
            ]
        },
        move || publish.pending.get(),
        move |value| {
            if publish.pending.get_untracked() {
                return;
            }
            match value.first().map(String::as_str) {
                Some("ssh") => publish.protocol.set(SourceControlCloneProtocol::Ssh),
                Some("https") => publish.protocol.set(SourceControlCloneProtocol::Https),
                _ => (),
            }
        },
    )
}

fn publish_summary(control: Control, publish: PublishState) -> impl IntoElement {
    dynamic(move || {
        let Some(result) = publish.result.get() else {
            return text(t("Publish result unavailable.")).app_class(tw("rounded-md border border-input bg-background px-3 py-2 text-xs text-muted-foreground dark:border-transparent dark:bg-white/[0.035]")).into_element();
        };
        let pushed = result.status == SourceControlPublishStatus::Pushed;
        let url = result.repository.url.0;
        view().app_class(tw("space-y-4"))
            .child(view().app_class(tw("flex flex-col items-center gap-2 py-1 text-center"))
                .child(view().app_class(tw("grid size-8 place-items-center rounded-full bg-success/15 text-success")).child(icon_with_class(Icon::Check, "size-4")))
                .child(text(t(if pushed { "Repository published" } else { "Repository created" })).app_class(tw("text-sm font-semibold text-foreground")))
                .child(text(if pushed { tr("{branch} is now live on {provider}.", &[("branch", Value::Text(result.branch.0)), ("provider", Value::Text(publish.selected(control).map_or_else(String::new, |p| p.label)))]) }
                    else { tr("Remote \"{remote}\" is set up. Make a commit and push it to share your code.", &[("remote", Value::Text(result.remote_name.0))]) }).app_class(tw("max-w-xs text-xs text-muted-foreground"))))
            .child(view().app_class(tw("flex items-center gap-2 rounded-lg border border-input bg-muted/40 px-3 py-2 dark:border-transparent dark:bg-white/[0.035]"))
                .child(publish.selected(control).map(|p| publish_provider_icon(&p, "size-3.5 shrink-0 text-muted-foreground")))
                .child(text(result.repository.name_with_owner.0).app_class(tw("min-w-0 flex-1 truncate font-mono text-xs text-foreground"))))
            .child(button(ButtonContent::Label(tr("Open on {provider}", &[("provider", Value::Text(publish.selected(control).map_or_else(String::new, |p| p.label)))])), ButtonVariant::Outline, ButtonSize::Sm, || false,
                move || { let url = url.clone(); spawn(async move { let _ = open_url(url).await; }); })).into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use t3_client::types::{EnvironmentId, ThreadId};

    fn snapshot(extra: serde_json::Value) -> GitActionsSnapshot {
        let mut value = json!({"isRepo":true,"hasPrimaryRemote":true,"isDefaultRef":false,"refName":"feature/test", "hasWorkingTreeChanges":false,
            "workingTree":{"files":[],"insertions":0,"deletions":0},"hasUpstream":true,"aheadCount":0,"behindCount":0,"pr":null});
        value
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("overrides").clone());
        GitActionsSnapshot {
            status: Some(Rc::new(serde_json::from_value(value).expect("VCS status"))),
            ..Default::default()
        }
    }

    #[test]
    fn cloning_worktree_base_and_running_actions_defer_automatic_draft_branch_selection() {
        let base = snapshot(json!({"refName":"unborn"}));
        for blocked in [
            GitActionsSnapshot {
                project_cloning: true,
                ..base.clone()
            },
            GitActionsSnapshot {
                selecting_worktree_base: true,
                ..base.clone()
            },
            GitActionsSnapshot {
                has_server_thread: true,
                ..base.clone()
            },
            GitActionsSnapshot {
                action: Rc::new(VcsActionState {
                    is_running: true,
                    operation: Some(VcsActionOperation::RunChangeRequest),
                    ..Default::default()
                }),
                ..base.clone()
            },
        ] {
            assert_eq!(live_branch_update(&blocked), None);
        }
        assert_eq!(
            live_branch_update(&snapshot(json!({"refName":"real-default"}))),
            Some(Some("real-default".into()))
        );
        assert_eq!(
            live_branch_update(&GitActionsSnapshot {
                thread_branch: Some("semantic".into()),
                ..snapshot(json!({"refName":null}))
            }),
            None
        );
        assert_eq!(
            live_branch_update(&GitActionsSnapshot {
                thread_branch: Some("semantic".into()),
                ..snapshot(json!({"refName":"t3code/bda76797"}))
            }),
            None
        );
    }

    #[test]
    fn quiet_drafts_show_errors_changes_and_progress_instead_of_hiding_them() {
        let scope = GitActionsScope {
            thread: ThreadKey {
                environment: EnvironmentId("env".into()),
                thread: ThreadId("thread".into()),
            },
            cwd: "/project".into(),
            draft_id: Some("draft".into()),
        };
        assert!(quiet_empty(&scope, &snapshot(json!({}))));
        assert!(!quiet_empty(
            &scope,
            &GitActionsSnapshot {
                status_error: Some("unsupported".into()),
                ..snapshot(json!({}))
            }
        ));
        assert!(!quiet_empty(
            &scope,
            &snapshot(json!({"hasWorkingTreeChanges":true}))
        ));
        assert!(!quiet_empty(
            &scope,
            &GitActionsSnapshot {
                branch_preview: Some(logic::ChangesTotals {
                    insertions: 1,
                    deletions: 0,
                    base_ref: None
                }),
                ..snapshot(json!({}))
            }
        ));
        assert!(!quiet_empty(
            &scope,
            &GitActionsSnapshot {
                action: Rc::new(VcsActionState {
                    is_running: true,
                    operation: Some(VcsActionOperation::Pull),
                    ..Default::default()
                }),
                ..snapshot(json!({}))
            }
        ));
    }

    #[test]
    fn progress_uses_the_clients_phase_and_hook_output_but_not_unrelated_operations() {
        let mut snapshot = snapshot(json!({}));
        snapshot.action = Rc::new(VcsActionState {
            is_running: true,
            operation: Some(VcsActionOperation::RunChangeRequest),
            current_label: Some("Running source control action".into()),
            last_output_line: Some(" linting ".into()),
            phase_started_at_ms: Some(20),
            hook_started_at_ms: Some(30),
            ..Default::default()
        });
        assert_eq!(
            progress(&snapshot),
            Some(logic::ProgressPresentation {
                status: "Starting source control action...".into(),
                output: Some("linting".into()),
                started_at: Some(30)
            })
        );
        Rc::make_mut(&mut snapshot.action).operation = Some(VcsActionOperation::Pull);
        assert_eq!(
            progress(&snapshot),
            Some(logic::ProgressPresentation {
                status: "Pulling latest changes...".into(),
                output: None,
                started_at: Some(20)
            })
        );
        Rc::make_mut(&mut snapshot.action).operation = Some(VcsActionOperation::RefreshStatus);
        assert_eq!(progress(&snapshot), None);
    }
}
