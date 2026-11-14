//! Port of apps/web/src/components/BranchToolbarBranchSelector.tsx BranchToolbarBranchSelector.

use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::ClientError;
use t3_client::workspace::branch::branch_trigger_label;
pub use t3_client::workspace::git_panel::branches::{BranchItem, BranchSelectorState};

use crate::i18n::{Text, Value, t, tr};
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::middle_truncate::{Font, Options, Variant, middle_truncate_with_font};
use crate::ui::popover::dropdown_glass_background;
use crate::ui::switch::{ControlState, Size as SwitchSize, switch};
use crate::ui::toast::{self, ToastOptions};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

const WORKTREE_CHANGE_BLOCKED_MESSAGE: &str = "Stop the current turn to switch worktrees.";

/// The client's branch-selector lease (`GitPanel::branch_selector`), bound by delegation in
/// `runtime/services/git_panel.rs`. Every rule (refs, optimism, grants, worktrees) is the client's.
pub trait BranchSelectorBackend {
    /// The current state first, then every change.
    fn watch(&self) -> LocalBoxStream<'static, Rc<BranchSelectorState>>;
    fn set_query(&self, query: String);
    fn refresh(&self) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn load_next(&self) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn select_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn create_ref(&self, name: String) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn set_start_from_origin(&self, value: bool)
    -> LocalBoxFuture<'static, Result<(), ClientError>>;
}

#[derive(Clone, PartialEq)]
enum PickerItem {
    Ref(BranchItem),
    Create(String),
}

/// `ThreadDetailsPanel`'s `show.branch` (`offersGit && isGitRepo`); ChatView treats a checkout
/// whose status has not arrived as a repository (`liveIsGitRepo ?? … ?? true`).
fn shows_branch(state: &BranchSelectorState) -> bool {
    state.offers_git && state.is_repo != Some(false)
}

/// `resolveBranchTriggerLabel` with the listed ref's remote flag.
fn trigger_label(state: &BranchSelectorState) -> SharedString {
    let remote = state.branch.as_deref().and_then(|branch| {
        state
            .refs
            .iter()
            .find(|r| r.name.0 == branch)
            .map(|r| r.is_remote.flatten() == Some(true))
    });
    let label = branch_trigger_label(
        state.worktree_path.as_deref(),
        state.env_mode,
        state.branch.as_deref(),
        remote,
        state.start_from_origin,
    );
    match state.branch.as_deref().filter(|b| !b.is_empty()) {
        None => Text::from("Select ref").resolve(),
        Some(branch) if label == branch => Text::content(label).resolve(),
        Some(branch) if label.ends_with(" (remote)") => {
            tr("From {branch} (remote)", &[("branch", Value::Text(branch.to_owned()))]).into()
        }
        Some(branch) => tr("From {branch}", &[("branch", Value::Text(branch.to_owned()))]).into(),
    }
}

fn picker_items(state: &BranchSelectorState) -> Vec<PickerItem> {
    let mut items: Vec<_> = state.filtered.iter().cloned().map(PickerItem::Ref).collect();
    if let Some(name) = &state.create_name {
        items.push(PickerItem::Create(name.clone()));
    }
    items
}

/// `branchStatusText`.
fn status_text(state: &BranchSelectorState) -> Option<String> {
    if initial_load_pending(state) {
        Some(t("Loading refs..."))
    } else if state.loading_next {
        Some(t("Loading more refs..."))
    } else if state.next_cursor.is_some() {
        Some(tr(
            "Showing {shown} of {total} refs",
            &[
                ("shown", Value::Int(state.refs.len() as i64)),
                ("total", Value::Int(state.total_count as i64)),
            ],
        ))
    } else {
        None
    }
}

/// `isInitialBranchesLoadPending`: loading with no listed page yet.
fn initial_load_pending(state: &BranchSelectorState) -> bool {
    state.loading && state.refs.is_empty()
}

/// `BranchPickerRefItem`'s badge.
fn ref_badge(item: &BranchItem, project_cwd: Option<&str>) -> Option<&'static str> {
    let reference = &item.reference;
    let secondary_worktree = reference
        .worktree_path
        .as_ref()
        .zip(project_cwd)
        .is_some_and(|(path, cwd)| path.0 != cwd);
    if reference.current {
        Some("current")
    } else if secondary_worktree {
        Some("worktree")
    } else if reference.is_remote.flatten() == Some(true) {
        Some("remote")
    } else if reference.is_default {
        Some("default")
    } else {
        None
    }
}

/// Mount once per scoped thread in `VersionControlSlots::branch`; dropping the element drops the
/// lease.
pub fn branch_toolbar_branch_selector(backend: Rc<dyn BranchSelectorBackend>) -> impl IntoElement {
    let backend = stored(backend);
    let state = signal(Rc::new(BranchSelectorState::default()));
    let mut updates = backend.with(|b| b.watch());
    spawn(async move {
        while let Some(next) = updates.next().await {
            state.set(next);
        }
    });
    view()
        .app_class(tw("flex w-full flex-col"))
        .child(when(
            move || state.with(|s| shows_branch(s)),
            move || branch_picker(backend, state),
        ))
}

fn show_branch_error(error: ClientError, failure_title: &'static str) {
    let options = match error {
        ClientError::Cancelled => return,
        ClientError::PermissionDenied { .. } => ToastOptions {
            variant: toast::Variant::Error,
            title: t("Action unavailable"),
            description: Some(t("This connection cannot change the thread's branch.")),
            ..Default::default()
        },
        ClientError::Rejected { message, .. } if message == WORKTREE_CHANGE_BLOCKED_MESSAGE => {
            ToastOptions {
                variant: toast::Variant::Warning,
                title: t(WORKTREE_CHANGE_BLOCKED_MESSAGE),
                ..Default::default()
            }
        }
        error => ToastOptions {
            variant: toast::Variant::Error,
            title: t(failure_title),
            description: Some(error.to_string()),
            ..Default::default()
        },
    };
    toast::toast_manager().add(toast::stacked_thread_toast(options));
}

/// Port of BranchPicker.tsx BranchPicker in panel mode (`align="start" side="bottom"`).
fn branch_picker(
    backend: Stored<Rc<dyn BranchSelectorBackend>>,
    state: Signal<Rc<BranchSelectorState>>,
) -> impl IntoElement {
    let open = signal(false);
    let query = signal(String::new());
    let anchor_width = signal(0.0_f32);
    let available = signal(nsh_core::Size::new(window().width, window().height));
    let search = element_ref();
    let label = computed(move || state.with(|s| trigger_label(s)));
    let disabled = computed(move || state.with(|s| initial_load_pending(s) || s.pending));
    let items = computed(move || state.with(|s| picker_items(s)));
    let select = move |item: PickerItem| {
        open.set(false);
        let backend = backend.get();
        spawn(async move {
            let result = match item {
                PickerItem::Ref(item) => backend
                    .select_ref(item.reference.name.0)
                    .await
                    .map_err(|e| (e, "Failed to switch ref.")),
                PickerItem::Create(name) => backend
                    .create_ref(name)
                    .await
                    .map_err(|e| (e, "Failed to create and switch ref.")),
            };
            if let Err((error, title)) = result {
                show_branch_error(error, title);
            }
        });
    };
    let set_open = move |next: bool| {
        let was_open = open.get_untracked();
        open.set(next);
        if next && !was_open {
            let backend = backend.get();
            spawn(async move {
                let _ = backend.refresh().await;
            });
        }
        if !next && !query.get_untracked().is_empty() {
            query.set(String::new());
            backend.with(|b| b.set_query(String::new()));
        }
    };
    // UI-COMBOBOX's slots are not ported; native Popover supplies outside/Escape/back dismissal.
    // NSH Combobox only supports an inline input trigger, unlike the source's select row.
    nsh_base::popover()
        .open(move || Some(open.get() && !disabled.get()))
        .on_open_change(move |event: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::OpenChange(next, _) = event {
                set_open(*next && !disabled.get_untracked());
            }
        })
        .on_open(move || search.focus())
        .on_position(move |event: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::Position(_, _, room) = event {
                available.set(*room);
            }
        })
        .placement(Placement::Bottom)
        .align(Align::Start)
        .offset(px(4.0))
        .popup_class(tw("z-[130]"))
        .trigger(thread_details_control(label, disabled, open, anchor_width))
        .child(dynamic(move || {
            untracked(|| {
                view()
                    .test_id("branch-picker-popup")
                    .app_class(tw("relative flex max-h-full rounded-lg border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] shadow-[0_16px_40px_-18px_rgb(0_0_0/55%)] dark:shadow-[0_18px_44px_-18px_rgb(0_0_0/80%)] flex flex-col"))
                    .w(move || px(anchor_width.get().min(available.get().width.0).max(0.0)))
                    .max_h(move || px(available.get().height.0.clamp(0.0, 368.0)))
                    .bg(dropdown_glass_background)
                    .border_color(move || tokens().color("contrast-foreground").unwrap_or_default().opacity(0.1))
                    .child(
                        view()
                            .app_class(tw("flex min-w-0 flex-1 flex-col overflow-hidden text-foreground"))
                            .child(combobox_search_input(query, search, move |value| {
                                query.set(value.clone());
                                backend.with(|b| b.set_query(value));
                            }, move || {
                                // autoHighlight puts the first item under Enter.
                                if let Some(item) = items.with_untracked(|items| items.iter().find(|i| !matches!(i, PickerItem::Ref(r) if r.disabled_reason.is_some())).cloned()) {
                                    select(item);
                                }
                            }))
                            .child(
                                view().app_class(tw("flex min-h-0 flex-1 flex-col overflow-hidden"))
                                    .child(when(move || items.with(Vec::is_empty), || text(t("No refs found."))
                                        .app_class(tw("p-2 text-center text-base text-muted-foreground sm:text-sm"))))
                                    .child(view().app_class(tw("relative flex min-h-0 w-full max-h-56 flex-1 flex-col overflow-hidden"))
                                        .child(scroll()
                                            .role(AccessibilityRole::Listbox)
                                            .app_class(tw("min-h-0 max-h-56 overflow-x-hidden ps-1 pe-0 py-2"))
                                            .on_scroll(move |event: &ScrollEvent| {
                                                // LegendList's onEndReached at threshold 0.5.
                                                let remaining = event.content_size.height.0 - event.offset.y.0 - event.size.height.0;
                                                if remaining <= event.size.height.0 * 0.5 && state.with_untracked(|s| s.next_cursor.is_some() && !s.loading_next) {
                                                    let backend = backend.get();
                                                    spawn(async move { let _ = backend.load_next().await; });
                                                }
                                            })
                                            .child(dynamic(move || {
                                                let items = items.get();
                                                let project_cwd = state.with(|s| s.checkout_root.clone());
                                                untracked(|| view().child(items.into_iter().map(|item| picker_row(item, project_cwd.as_deref(), select).into_element()).collect::<Vec<_>>()).into_element())
                                            }))))
                                    .child(when(move || state.with(|s| s.selecting_worktree_base), move || start_from_origin_control(backend, state)))
                                    .child(dynamic(move || state.with(|s| s.filtered.iter().any(|i| i.disabled_reason.as_deref() == Some(WORKTREE_CHANGE_BLOCKED_MESSAGE)))
                                        .then(|| combobox_status(t(WORKTREE_CHANGE_BLOCKED_MESSAGE)).into_element())))
                                    .child(dynamic(move || state.with(|s| status_text(s)).map(|s| combobox_status(s).into_element()))),
                            ),
                    )
                    .into_element()
            })
        }))
}

/// Port of ui/combobox.tsx ComboboxStatus.
fn combobox_status(content: String) -> impl IntoElement {
    text(content).app_class(tw("px-3 pt-2 pb-2.5 text-muted-foreground text-xs font-medium"))
}

fn picker_row(
    item: PickerItem,
    project_cwd: Option<&str>,
    select: impl Fn(PickerItem) + Copy + 'static,
) -> impl IntoElement {
    match item.clone() {
        PickerItem::Create(name) => combobox_item(
            tr("Create new ref \"{name}\"", &[("name", Value::Text(name.clone()))]),
            false,
            text(tr("Create new ref \"{name}\"", &[("name", Value::Text(name))])).app_class(tw("truncate")).into_element(),
            move || select(item.clone()),
        ),
        PickerItem::Ref(reference) => {
            let name = reference.reference.name.0.clone();
            let badge = ref_badge(&reference, project_cwd);
            let disabled = reference.disabled_reason.is_some();
            combobox_item(
                name.clone(),
                disabled,
                view()
                    .app_class(tw("flex w-full min-w-0 items-center justify-between gap-2"))
                    .child(view().app_class(tw("flex-1 min-w-0")).child(middle_truncate_with_font(
                        move || SharedString::from(name.clone()),
                        Options::default(),
                        || Font { size: 14.0, ..Default::default() },
                    )))
                    .child(badge.map(|badge| text(t(badge)).app_class(tw("shrink-0 text-3xs text-muted-foreground/45"))))
                    .into_element(),
                move || select(item.clone()),
            )
        }
    }
}

/// Port of ui/combobox.tsx ComboboxItem with `hideIndicator`.
fn combobox_item(
    label: String,
    disabled: bool,
    content: AnyElement,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    nsh_base::button()
        .role(AccessibilityRole::Option)
        .accessibility_label(Some(SharedString::from(label)))
        .disabled(disabled)
        .on_press(move || {
            if !disabled {
                on_press();
            }
        })
        .app_class(tw("flex min-h-8 w-full cursor-default select-none items-center gap-2 rounded-sm py-1 ps-2 pe-4 text-base outline-none hover:bg-accent hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-64 sm:min-h-7 sm:text-sm"))
        .child(content)
        .into_element()
}

/// BranchPicker's `originControl` row.
fn start_from_origin_control(
    backend: Stored<Rc<dyn BranchSelectorBackend>>,
    state: Signal<Rc<BranchSelectorState>>,
) -> impl IntoElement {
    view()
        .app_class(tw("flex cursor-pointer items-center justify-between gap-3 border-t border-border/60 px-3 py-2 text-xs"))
        .tooltip(tooltip(
            tooltip_create_handle(),
            TooltipOptions::default(),
            text(t("Uses the latest commit of the base branch’s tracked remote branch, even if its name differs. Otherwise uses the same-named branch on origin. Uses your local branch if that remote branch is missing.")).app_class(tw("max-w-72")),
        ))
        .child(
            view()
                .app_class(tw("flex min-w-0 items-center gap-1.5 font-medium text-muted-foreground"))
                .child(icon_with_class(Icon::RefreshCw, "size-3 shrink-0"))
                .child(text(t("Start from remote")).app_class(tw("truncate"))),
        )
        .child(switch(
            "Start worktree from remote",
            ControlState {
                checked: Prop::bind(move || state.with(|s| s.start_from_origin)),
                ..Default::default()
            },
            SwitchSize::Sm,
            move |checked| {
                let backend = backend.get();
                spawn(async move {
                    let _ = backend.set_start_from_origin(checked).await;
                });
            },
        ))
}

/// Port of chat/ThreadDetailsControl.tsx ThreadDetailsControl's select part.
fn thread_details_control(
    label: Memo<SharedString>,
    disabled: Memo<bool>,
    open: Signal<bool>,
    anchor_width: Signal<f32>,
) -> impl IntoElement {
    // Selector classes move to their children; coarse after: becomes native hit slop.
    // text-box cap trimming is unavailable; UI-TRUNCATE owns that declared limitation.
    // NSH cannot parse the source shadow/scale transition list; scale stays 100 on this trigger.
    nsh_base::button()
        .role(AccessibilityRole::Combobox)
        .disabled(move || disabled.get())
        .accessibility_label(move || Some(tr("Branch: {branch}", &[("branch", Value::Text(label.get().to_string()))]).into()))
        .on_layout(move |bounds: &Bounds<Points>| anchor_width.set(bounds.size.width.0))
        .app_class(tw("group/branch-select relative inline-flex shrink-0 cursor-pointer items-center whitespace-nowrap border outline-none transition-shadow focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 h-8 min-w-0 rounded-lg border-transparent text-left text-[13px] font-medium text-foreground/80 w-full justify-start bg-transparent shadow-none hover:!bg-black/[0.035] active:!bg-black/[0.035] dark:hover:!bg-white/[0.08] dark:active:!bg-white/[0.08] gap-0 px-0 max-w-full active:scale-100"))
        .hit_slop(move || EdgeInsets::all(px(if pointer() == Pointer::Coarse { 6.0 } else { 0.0 })))
        .child(
            view()
                .app_class(tw("flex h-full min-w-0 flex-1 items-center rounded-s-lg gap-2.5 px-2.5 text-left hover:bg-black/[0.035] dark:hover:bg-white/[0.08]"))
                .child(icon_with_class(Icon::GitBranch, "size-4 shrink-0 text-muted-foreground"))
                .child(composer_context_label(label)),
        )
        .child(
            view()
                .app_class(tw("rounded-e-lg hover:bg-black/[0.035] dark:hover:bg-white/[0.08] -me-px relative flex h-full w-8 shrink-0 items-center justify-center"))
                // Source select-icon::before is an explicitly positioned first child.
                .child(view().app_class(move || Some(tw(&cn([
                    Some("absolute -left-px top-1/2 h-4 w-px -translate-y-1/2 bg-transparent group-hover/branch-select:bg-border/65"),
                    open.get().then_some("bg-border/65"),
                ])))))
                .child(icon_with_class(Icon::ChevronDown, "size-4 shrink-0 text-muted-foreground opacity-100 me-0")),
        )
}

/// Port of ComposerContextLabel.tsx ComposerContextLabel with displayMode="panel".
fn composer_context_label(label: Memo<SharedString>) -> impl IntoElement {
    // Hide the duplicate visual value: the combobox's changing label serves browse mode.
    view()
        .app_class(tw("min-w-0 flex-1 overflow-hidden text-left"))
        .accessibility_elements_hidden(true)
        .child(
            view()
                .app_class(tw("block w-full min-w-0 overflow-hidden"))
                .child(middle_truncate_with_font(
                    move || label.get(),
                    Options {
                        variant: Variant::CapAligned,
                        ..Default::default()
                    },
                    || Font {
                        size: 13.0,
                        weight: FontWeight::MEDIUM,
                        ..Default::default()
                    },
                )),
        )
}

/// Port of ui/combobox.tsx ComboboxSearchInput; remove after UI-COMBOBOX lands its slots.
fn combobox_search_input(
    query: Signal<String>,
    search: ElementRef,
    on_change: impl Fn(String) + 'static,
    on_enter: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .app_class(tw("min-w-0 shrink-0 px-3 pt-2.5"))
        .child(
            view().app_class(tw("relative -translate-y-px border-b border-border/70 pb-1.5 transition-colors focus-within:border-ring"))
                .child(view().app_class(tw("pointer-events-none absolute top-1.5 left-0"))
                    .child(icon_with_class(Icon::Search, "size-4 shrink-0 text-muted-foreground/55")))
                .child(nsh_base::input()
                    .test_id("branch-search")
                    .element_ref(&search)
                    .value(move || query.get())
                    .on_change(move |value: &str| on_change(value.to_owned()))
                    .on_submit(move |_: &str| on_enter())
                    .submit_behavior(SubmitBehavior::Submit)
                    .placeholder(move || t("Search refs..."))
                    .label(move || t("Search refs..."))
                    .auto_correct(false)
                    .app_class(tw("flex w-full h-6.5 ps-5 font-sans leading-6.5 rounded-none bg-transparent text-sm"))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_contracts::git::VcsRef;

    fn reference(mut value: serde_json::Value) -> VcsRef {
        let fields = value.as_object_mut().expect("object");
        fields.entry("worktreePath").or_insert(serde_json::Value::Null);
        serde_json::from_value(value).expect("canonical ref")
    }

    fn item(value: serde_json::Value) -> BranchItem {
        BranchItem {
            reference: reference(value),
            disabled_reason: None,
        }
    }

    #[test]
    fn ref_badges_follow_branch_picker_ref_item_precedence() {
        let current = item(serde_json::json!({"name":"main","current":true,"isDefault":true}));
        assert_eq!(ref_badge(&current, Some("/repo")), Some("current"));
        let worktree = item(serde_json::json!({"name":"f","current":false,"isRemote":true,"isDefault":false,"worktreePath":"/w/f"}));
        assert_eq!(ref_badge(&worktree, Some("/repo")), Some("worktree"));
        let same_root = item(serde_json::json!({"name":"f","current":false,"isDefault":false,"worktreePath":"/repo"}));
        assert_eq!(ref_badge(&same_root, Some("/repo")), None);
        let remote = item(serde_json::json!({"name":"origin/f","current":false,"isRemote":true,"isDefault":false}));
        assert_eq!(ref_badge(&remote, Some("/repo")), Some("remote"));
        let default = item(serde_json::json!({"name":"trunk","current":false,"isDefault":true}));
        assert_eq!(ref_badge(&default, None), Some("default"));
    }

    #[test]
    fn status_text_reports_initial_load_then_paging() {
        let mut state = BranchSelectorState {
            loading: true,
            ..Default::default()
        };
        assert_eq!(status_text(&state), Some(t("Loading refs...")));
        state.loading = false;
        state.refs = vec![reference(serde_json::json!({"name":"main","current":true,"isDefault":true}))];
        state.total_count = 120;
        state.next_cursor = Some(100);
        assert_eq!(status_text(&state), Some("Showing 1 of 120 refs".into()));
        state.loading_next = true;
        assert_eq!(status_text(&state), Some(t("Loading more refs...")));
        state.loading_next = false;
        state.next_cursor = None;
        assert_eq!(status_text(&state), None);
    }

    #[test]
    fn the_row_needs_a_repository_that_offers_git() {
        let mut state = BranchSelectorState {
            offers_git: true,
            ..Default::default()
        };
        assert!(shows_branch(&state), "an unknown status counts as a repository");
        state.is_repo = Some(false);
        assert!(!shows_branch(&state));
        state.is_repo = Some(true);
        assert!(shows_branch(&state));
        state.offers_git = false;
        assert!(!shows_branch(&state), "the scratch folder never gets git");
    }
}
