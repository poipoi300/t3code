//! Port of apps/web/src/components/sidebar/SidebarThreadRow.tsx SidebarThreadRow.

use std::time::Duration;

use nsh_core::prelude::*;
use t3_client::services::config::timestamp_ms;
use t3_client::services::models::{
    EnvironmentThreadShell, PendingSubagentRequest, format_working_duration_label,
    present_thread_shell, resolve_thread_provider_stack, resolve_thread_working_started_at,
};
use t3_client::view_logic::session::timestamps::{
    format_relative_elapsed, ms_until_relative_time_change, relative_time_elapsed_ms,
};
use t3_client::view_logic::shell::sidebar::presentation::ThreadInput;
use t3_client::view_logic::shell::sidebar::status::{self, Status};
use t3_client::workspace::project_ops::icons::{
    derive_project_identity, project_icon_color_class_name,
};
use t3_contracts::orchestration_project::OrchestrationProjectShell;
use t3_contracts::orchestration_v2::{
    AppThreadSettledOverride, OrchestrationV2ProviderGoalStatus, OrchestrationV2ThreadShell,
};
use t3_contracts::project::{ProjectIconColor, ProjectIconOverride};

use super::{RowVariant, SidebarItem};
use crate::i18n::{Value, t, tr};
use crate::runtime::{Shared, rt};
use crate::state::nav::{Navigator, Route};
use crate::theme::{Scope, cn, palette, tw_with};
use crate::ui::icon::{Icon, icon};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

type Thread = Memo<Option<Shared<OrchestrationV2ThreadShell>>>;
type Project = Memo<Option<Shared<OrchestrationProjectShell>>>;

/// Supplied by the sidebar's selection/draft/snooze stores, never inferred from run state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowExtras {
    pub selected: bool,
    pub unsent_draft: bool,
    pub woke_at: Option<String>,
    pub snooze_wake_label: Option<String>,
    pub local_last_visited_at: Option<String>,
    pub providers: Vec<RowProvider>,
    /// Environment clock converted by t3-client and refreshed by the shared second tick.
    pub server_now_ms: Option<i64>,
    pub pending_subagent_request: Option<PendingSubagentRequest>,
    pub environment_label: Option<String>,
}

/// UI-ICONS supplies the branded glyph; the environment's client catalog supplies its identity.
#[derive(Clone, Debug, PartialEq)]
pub struct RowProvider {
    pub instance_id: String,
    pub display_name: String,
    pub glyph: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopStatusIcon {
    Working,
    Waiting,
    Input,
    Request,
    Approval,
    Failed,
    Done,
    Woke,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TopStatus {
    pub label: &'static str,
    pub icon: TopStatusIcon,
    pub class_name: &'static str,
}

/// The component's label precedence, after resolveSidebarThreadStatus.
pub fn top_status(
    status: Status,
    goal_active: bool,
    woke: bool,
    unread: bool,
) -> Option<TopStatus> {
    let (label, icon, class_name) = match status {
        Status::Preparing => ("Setting up", TopStatusIcon::Working, "text-info-foreground"),
        Status::Working => (
            if goal_active { "Goal" } else { "Working" },
            TopStatusIcon::Working,
            "text-info-foreground",
        ),
        Status::Waiting => ("Waiting", TopStatusIcon::Waiting, "text-muted-foreground"),
        Status::Approval => (
            "Approval",
            TopStatusIcon::Approval,
            "text-warning-foreground",
        ),
        Status::Input => (
            "Input",
            TopStatusIcon::Input,
            "text-indigo-600 dark:text-indigo-300",
        ),
        Status::Request => ("Request", TopStatusIcon::Request, "text-guest-foreground"),
        Status::Limited => ("Limited", TopStatusIcon::Failed, "text-warning-foreground"),
        Status::Failed => ("Failed", TopStatusIcon::Failed, "text-error-foreground"),
        Status::Ready if woke => ("Woke", TopStatusIcon::Woke, "text-warning-foreground"),
        Status::Ready if unread => ("Done", TopStatusIcon::Done, "text-success-foreground"),
        Status::Ready => return None,
    };
    Some(TopStatus {
        label,
        icon,
        class_name,
    })
}

#[derive(Clone, Debug, PartialEq)]
struct Presentation {
    status: Status,
    top: Option<TopStatus>,
    unread: bool,
    woke: bool,
    receded: bool,
}

fn presentation(thread: &EnvironmentThreadShell, extras: &RowExtras, active: bool) -> Presentation {
    let input = ThreadInput::from_environment_shell(
        thread,
        extras.local_last_visited_at.as_deref(),
        &timestamp_ms,
    );
    let status = status::resolve_status(&input.status);
    let unread = status::has_unseen_completion(&input.status, &timestamp_ms);
    let woke = extras
        .woke_at
        .as_deref()
        .and_then(timestamp_ms)
        .is_some_and(|wake| {
            thread.source.settled_override != Some(AppThreadSettledOverride::Settled)
                && input
                    .status
                    .last_visited_at
                    .and_then(timestamp_ms)
                    .is_none_or(|visit| visit < wake)
        });
    let goal_active = thread
        .source
        .goal
        .as_ref()
        .and_then(Option::as_ref)
        .is_some_and(|goal| goal.status == OrchestrationV2ProviderGoalStatus::Active);
    Presentation {
        status,
        top: top_status(status, goal_active, woke, unread),
        unread,
        woke,
        receded: status::should_recede(status, unread, woke, active, extras.selected),
    }
}

fn working_duration_label(
    thread: &EnvironmentThreadShell,
    status: Status,
    host_now: Option<i64>,
) -> String {
    let started = match status {
        Status::Preparing | Status::Working => {
            resolve_thread_working_started_at(thread.latest_run.as_ref(), thread.runtime.as_ref())
        }
        Status::Waiting => thread
            .latest_run
            .as_ref()
            .and_then(|run| run.completed_at.as_ref())
            .map(|at| at.0.as_str()),
        _ => None,
    }
    .and_then(timestamp_ms);
    started
        .zip(host_now)
        .map(|(started, now)| {
            t(&format_working_duration_label(
                (now as f64) - (started as f64),
            ))
        })
        .unwrap_or_default()
}

/// compactSidebarTimeLabel(formatRelativeTimeLabel(..)).
pub fn relative_label(iso: &str, now_ms: i64) -> String {
    let Some(date) = timestamp_ms(iso) else {
        return String::new();
    };
    let label = t(&format_relative_elapsed(relative_time_elapsed_ms(date, now_ms)).value);
    compact_sidebar_time_label(&label, &t("just now"), &t("now"))
}

fn compact_sidebar_time_label(label: &str, just_now: &str, now: &str) -> String {
    if label == just_now {
        return now.to_owned();
    }
    label.strip_suffix(" ago").unwrap_or(label).to_owned()
}

fn class(classes: &str) -> SharedString {
    tw_with(Scope::Sidebar, classes, &[])
}

/// RelativeTimeLabel schedules only the next boundary returned by the client.
fn relative_time_label(timestamp: impl Fn() -> Option<String> + 'static) -> impl IntoElement {
    let now = signal(nsh_core::time::unix_ms() as i64);
    let timestamp = computed(timestamp);
    let pending = stored(None::<nsh_core::element::Spawned<()>>);
    effect(move || {
        if let Some(task) = pending.replace(None) {
            task.cancel();
        }
        let at = timestamp.get().as_deref().and_then(timestamp_ms);
        let current = now.get();
        if let Some(at) = at {
            let delay = ms_until_relative_time_change(at, current).max(1) as u64;
            let _ = pending.replace(Some(set_timeout(Duration::from_millis(delay), move || {
                now.set(nsh_core::time::unix_ms() as i64);
            })));
        }
    });
    cleanup(move || {
        if let Some(task) = pending.replace(None) {
            task.cancel();
        }
    });
    text(move || {
        SharedString::from(
            timestamp
                .get()
                .as_deref()
                .map(|at| relative_label(at, now.get()))
                .unwrap_or_default(),
        )
    })
}

pub fn thread_row(item: Memo<Option<SidebarItem>>, nav: Navigator) -> impl IntoElement {
    // SB-LIST has not exposed its draft/selection/wake observations through SidebarItem yet.
    thread_row_with_extras(item, computed(RowExtras::default), nav)
}

pub fn thread_row_with_extras(
    item: Memo<Option<SidebarItem>>,
    extras: Memo<RowExtras>,
    nav: Navigator,
) -> impl IntoElement {
    let runtime = rt();
    let identity = computed(move || item.with(|i| i.as_ref().map(|i| (i.key.clone(), i.variant))));
    dynamic(move || {
        identity.get().map(|(key, _)| {
            untracked(move || {
                let shell = runtime.store.shell(key.environment.clone());
                let id = key.thread;
                let thread = computed(move || {
                    shell.with(|shell| {
                        shell.as_ref().and_then(|shell| {
                            shell
                                .threads
                                .iter()
                                .find(|row| row.id.0 == id.0)
                                .cloned()
                                .map(Shared)
                        })
                    })
                });
                let project = computed(move || {
                    thread.with(|thread| {
                        thread.as_ref().and_then(|thread| {
                            shell.with(|shell| {
                                shell.as_ref().and_then(|shell| {
                                    shell
                                        .projects
                                        .iter()
                                        .find(|p| p.id == thread.project_id)
                                        .cloned()
                                        .map(Shared)
                                })
                            })
                        })
                    })
                });
                sidebar_thread_row(item, thread, project, extras, nav).into_element()
            })
        })
    })
}

pub fn sidebar_thread_row(
    item: Memo<Option<SidebarItem>>,
    thread: Thread,
    project: Project,
    extras: Memo<RowExtras>,
    nav: Navigator,
) -> impl IntoElement {
    let key = computed(move || item.with(|i| i.as_ref().map(|i| i.key.clone())));
    let pending_request = computed(move || extras.with(|extras| extras.pending_subagent_request));
    let model = computed(move || {
        thread.with(|thread| {
            thread.as_ref().and_then(|thread| {
                key.get().map(|key| {
                    present_thread_shell(key.environment, thread.0.clone(), pending_request.get())
                })
            })
        })
    });
    let active = move || {
        let current = nav
            .stack
            .with(|stack| stack.last().and_then(|r| r.thread().cloned()));
        current.is_some() && current == key.get()
    };
    let is_selected = computed(move || extras.with(|extras| extras.selected));
    let has_draft = computed(move || extras.with(|extras| extras.unsent_draft) && !active());
    let observations = computed(move || {
        extras.with(|extras| RowExtras {
            selected: extras.selected,
            woke_at: extras.woke_at.clone(),
            local_last_visited_at: extras.local_last_visited_at.clone(),
            ..Default::default()
        })
    });
    let snooze_wake_label =
        computed(move || extras.with(|extras| extras.snooze_wake_label.clone()));
    let presented = computed(move || {
        model.with(|model| {
            model
                .as_ref()
                .map(|model| presentation(model, &observations.get(), active()))
        })
    });
    let card = move || item.with(|i| i.as_ref().is_some_and(|i| i.variant == RowVariant::Card));
    let title = move || {
        model
            .with(|model| model.as_ref().map(|model| model.title.clone()))
            .unwrap_or_else(|| {
                item.with(|i| i.as_ref().map(|i| i.title.clone()).unwrap_or_default())
            })
    };
    let project_name =
        move || item.with(|i| i.as_ref().map(|i| i.project.clone()).unwrap_or_default());
    let settled = move || {
        thread.with(|thread| {
            thread.as_ref().is_some_and(|thread| {
                thread.settled_override == Some(AppThreadSettledOverride::Settled)
            })
        })
    };
    let surface = move || {
        Some(class(&cn([
        Some("group/sidebar-row relative w-full cursor-pointer overflow-hidden rounded-md text-left outline-none select-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"),
        Some(if active() {
            "bg-sidebar-row-active text-sidebar-foreground light:ring-1 light:ring-inset light:ring-sidebar-border/70"
        } else if is_selected.get() {
            "bg-sidebar-row-selected text-sidebar-foreground light:ring-1 light:ring-inset light:ring-sidebar-border/70"
        } else if has_draft.get() {
            "bg-warning/4 hover:bg-warning/8 text-sidebar-foreground"
        } else if presented.get().is_some_and(|p| p.receded) {
            "text-sidebar-muted-foreground/75 hover:bg-sidebar-row-hover hover:text-sidebar-foreground"
        } else {
            "bg-transparent text-sidebar-foreground hover:bg-sidebar-row-hover"
        }),
        presented.get().filter(|p| p.receded && matches!(p.status, Status::Preparing | Status::Working)).map(|_| "opacity-70 transition-opacity hover:opacity-100 focus-within:opacity-100 motion-reduce:transition-none"),
        (!card()).then_some("flex h-9 items-center gap-2.5 px-2.5"),
    ])))
    };
    let title_element = move || {
        text(move || SharedString::from(title())).app_class(move || {
        let p = presented.get();
        let receded = p.as_ref().is_some_and(|p| p.receded);
        Some(class(&cn([
            Some("min-w-0 flex-1 text-start text-sm transition-opacity motion-reduce:transition-none"),
            Some(if receded { "font-normal" } else { "font-medium" }),
            Some(if card() {
                if receded { "truncate text-foreground/75 group-hover/sidebar-row:text-foreground group-focus-visible/sidebar-row:text-foreground" }
                else if p.as_ref().is_some_and(|p| p.status == Status::Failed && !p.unread && !p.woke) { "truncate text-foreground/95" }
                else { "truncate text-foreground" }
            } else if receded { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-secondary-label" }
            else if active() || p.as_ref().is_some_and(|p| p.woke || matches!(p.status, Status::Input | Status::Request)) { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-foreground" }
            else if p.as_ref().is_some_and(|p| p.unread) { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-muted-foreground" }
            else { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-secondary-label/70" }),
            thread.with(|thread| thread.as_ref().is_some_and(|thread| thread.title_regeneration.as_ref().is_some_and(Option::is_some))).then_some("opacity-55"),
        ])))
    })
    };
    let meta_empty = move || {
        thread.with(|thread| {
            thread.as_ref().is_none_or(|thread| {
                thread.branch.is_none()
                    && thread
                        .linked_pull_request
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_none()
            })
        })
    };
    let age = move || {
        relative_time_label(move || {
            let activity = item.with(|i| i.as_ref().and_then(|i| i.activity_at.clone()));
            if !card() && settled() {
                thread
                    .with(|thread| {
                        thread.as_ref().and_then(|thread| {
                            thread
                                .settled_at
                                .as_ref()
                                .filter(|at| timestamp_ms(&at.0).is_some())
                                .map(|at| at.0.clone())
                        })
                    })
                    .or(activity)
            } else {
                activity
            }
        })
    };
    let details = tooltip_create_handle();
    let card_body = move || {
        view()
        .app_class(move || Some(class(&cn([
            Some("relative z-10 h-[4.875rem] px-(--sidebar-row-content-inset) py-(--sidebar-content-inset)"),
            meta_empty().then_some("h-auto"),
        ]))))
        .child(view().app_class(class("flex h-5 min-w-0 items-center gap-1.5"))
            .child(when(move || has_draft.get(), || {
                view().role(AccessibilityRole::Text).label(t("Unsent draft")).child(icon(Icon::SquarePen, 12.0, move || palette().warning_foreground))
            }))
            .child(project_favicon(project))
            .child(pin_indicator(thread))
            .child(text(move || SharedString::from(project_name())).app_class(class("min-w-0 flex-1 truncate font-normal text-secondary-label text-xs")))
            .child(view().app_class(class("group/sidebar-status-slot relative ml-auto flex h-5 min-w-8 shrink-0 items-stretch justify-end text-xs"))
                // NSH has no focus-visible descendant selector; group hover swaps controls.
                .child(view().app_class(class("pointer-events-none flex items-center self-center justify-self-end tabular-nums text-secondary-label group-any-hover/sidebar-row:opacity-0"))
                    .child(dynamic(move || presented.get().and_then(|p| p.top).map(|top| {
                        view().app_class(class(&cn([Some("inline-flex items-center gap-1 font-medium"), Some(top.class_name)])))
                            .child(top_status_icon(top)).child(text(t(top.label)))
                            .child(text(move || SharedString::from(model.with(|model| model.as_ref().map(|model| working_duration_label(model, presented.get().map_or(Status::Ready, |p| p.status), extras.with(|extras| extras.server_now_ms))).unwrap_or_default()))).app_class(class("tabular-nums")))
                            .into_element()
                    })))
                    .child(when(move || presented.get().is_none_or(|p| p.top.is_none()), age)))
                .child(unavailable_action("Settle thread", true))))
        .child(view().app_class(class("mt-1 flex min-w-0 items-center gap-1.5")).child(title_element())
            .child(when(meta_empty, move || sidebar_provider_stack(model, extras))))
        .child(when(move || !meta_empty(), move || meta_line(thread, model, extras)))
    };
    let slim_body = move || {
        dynamic(move || {
            untracked(|| {
                vec![
        view().app_class(class("flex shrink-0 items-center transition-opacity opacity-40 grayscale group-focus-within/sidebar-row:opacity-100 group-focus-within/sidebar-row:grayscale-0 group-hover/sidebar-row:opacity-100 group-hover/sidebar-row:grayscale-0")).child(project_favicon(project)).into_element(),
        when(move || has_draft.get(), || view().role(AccessibilityRole::Text).label(t("Unsent draft")).child(icon(Icon::SquarePen, 12.0, move || palette().warning_foreground))).into_element(),
        title_element().into_element(), pin_indicator(thread).into_element(),
        view().app_class(class("relative ml-auto flex h-6 min-w-8 shrink-0 items-center justify-end"))
            .child(view().app_class(class("inline-flex justify-end tabular-nums text-secondary-label transition-opacity group-any-hover/sidebar-row:opacity-0 text-xs"))
                .child(when(move || snooze_wake_label.get().is_some(), move || view().app_class(class("inline-flex items-center gap-1 text-xs text-info-foreground tabular-nums")).child(lucide(CLOCK_ICON, 12.0).tint_color(move || palette().info)).child(text(move || SharedString::from(snooze_wake_label.get().unwrap_or_default())))).otherwise(age)))
            .child(unavailable_action(if snooze_wake_label.get().is_some() { "Wake thread now" } else if settled() { "Un-settle thread" } else { "Settle thread" }, false)).into_element(),
    ]
            })
        })
    };
    // list-none/content-visibility/contain-intrinsic-size are native list/virtualisation hints.
    view()
        .app_class(move || Some(class(if card() { "py-0.5" } else { "" })))
        .tooltip(tooltip(
            details,
            TooltipOptions {
                variant: crate::ui::tooltip::Variant::Glass,
                side: Placement::Right,
                align: Align::Start,
                ..Default::default()
            },
            sidebar_thread_tooltip(model, project_name, extras),
        ))
        .child(
            nsh_base::button()
                .test_id(if untracked(card) {
                    "sidebar-row-card"
                } else {
                    "sidebar-row-slim"
                })
                .accessibility_label(move || {
                    Some(SharedString::from(
                        [
                            Some(title()),
                            presented.get().and_then(|p| p.top.map(|s| t(s.label))),
                            Some(project_name()),
                        ]
                        .into_iter()
                        .flatten()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", "),
                    ))
                })
                .selected(active)
                .on_press(move || {
                    if let Some(key) = key.get() {
                        nav.open(Route::Thread(key));
                    }
                })
                .app_class(surface)
                .child(when(card, card_body).otherwise(slim_body)),
        )
}

fn pin_indicator(thread: Thread) -> impl IntoElement {
    when(
        move || {
            thread.with(|thread| {
                thread
                    .as_ref()
                    .is_some_and(|thread| thread.pinned_at.as_ref().is_some_and(Option::is_some))
            })
        },
        || {
            // Read-only pin matches pinningSupported=false until Backend exposes authorized unpin.
            view().role(AccessibilityRole::Text).label(t("Pinned"))
            .child(lucide(r#"<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>"#, 12.0).tint_color(move || palette().muted_foreground.opacity(0.65)))
            .app_class(class("size-3 shrink-0 text-muted-foreground/65"))
        },
    )
}

fn unavailable_action(label: &'static str, card: bool) -> impl IntoElement {
    // Backend has no mutation/permission seam; the disabled control cannot dispatch.
    view().app_class(class("pointer-events-none absolute inset-y-0 right-0 flex items-stretch opacity-0 group-any-hover/sidebar-row:opacity-100"))
        .role(AccessibilityRole::Text).label(t("Thread actions are unavailable on this client."))
        .child(nsh_base::button().disabled(true).accessibility_label(Some(t(label).into()))
            .app_class(class("-mr-1 inline-flex cursor-pointer items-center gap-1 rounded-md bg-transparent px-1.5 text-xs text-muted-foreground hover:text-foreground"))
            .child(if label == "Un-settle thread" { lucide(UNDO_ICON, 14.0).tint_color(move || palette().muted_foreground).into_element() } else if label == "Wake thread now" { lucide(WAKE_ICON, 12.0).tint_color(move || palette().muted_foreground).into_element() } else { icon(Icon::Check, 14.0, move || palette().muted_foreground).into_element() })
            .child(card.then(|| text(t("Settle")))))
}

fn meta_line(
    thread: Thread,
    model: Memo<Option<EnvironmentThreadShell>>,
    extras: Memo<RowExtras>,
) -> impl IntoElement {
    view()
        .app_class(class(
            "mt-0.5 flex min-w-0 items-center gap-1.5 text-secondary-label text-xs",
        ))
        .child(when(
            move || thread.with(|t| t.as_ref().is_some_and(|t| t.worktree_path.is_some())),
            || lucide(WORKTREE_ICON, 14.0).tint_color(move || palette().icon_muted),
        ))
        // UI-TRUNCATE has no MiddleTruncate yet; the tooltip retains the full branch.
        .child(
            view().app_class(class("flex min-w-0 flex-1")).child(
                text(move || {
                    SharedString::from(thread.with(|t| {
                        t.as_ref()
                            .and_then(|t| t.branch.as_ref().map(|b| b.0.clone()))
                            .unwrap_or_default()
                    }))
                })
                .app_class(class("min-w-0 truncate")),
            ),
        )
        // PR status/reconciliation and stack badges need the client's row presentation.
        .child(dynamic(move || {
            thread
                .with(|thread| {
                    thread.as_ref().and_then(|thread| {
                        thread
                            .linked_pull_request
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|pr| pr.number)
                    })
                })
                .map(|number| {
                    nsh_base::button()
                        .disabled(true)
                        .accessibility_label(Some(
                            tr(
                                "PR #{number}, status pending",
                                &[("number", Value::Int(number as i64))],
                            )
                            .into(),
                        ))
                        .app_class(class(
                            "inline-flex shrink-0 items-center gap-1 text-muted-foreground text-xs",
                        ))
                        .child(icon(Icon::Link, 12.0, move || palette().muted_foreground))
                        .child(text(tr(
                            "{number}",
                            &[("number", Value::Int(number as i64))],
                        )))
                        .into_element()
                })
        }))
        .child(
            view()
                .app_class(class(
                    "pointer-events-none ml-auto inline-flex shrink-0 items-center gap-1",
                ))
                .child(sidebar_provider_stack(model, extras)),
        )
}

fn sidebar_provider_stack(
    model: Memo<Option<EnvironmentThreadShell>>,
    extras: Memo<RowExtras>,
) -> impl IntoElement {
    let providers = computed(move || extras.with(|extras| extras.providers.clone()));
    dynamic(move || {
        let model = model.get()?;
        let providers = providers.get();
        let stack = resolve_thread_provider_stack(&model);
        let current = stack.last()?;
        let current_entry = providers
            .iter()
            .find(|entry| entry.instance_id == current.0)?;
        if stack.len() == 1 {
            return Some(
                view()
                    .app_class(class("inline-flex shrink-0 items-center"))
                    .child(
                        image(current_entry.glyph.clone())
                            .app_class(class("size-3.5 opacity-60"))
                            .accessible(false),
                    ),
            );
        }
        Some(
            view()
                .app_class(class(if stack.len() == 1 {
                    "inline-flex shrink-0 items-center"
                } else {
                    "inline-flex shrink-0 items-center gap-1.5"
                }))
                .child(
                    stack
                        .iter()
                        .take(stack.len().saturating_sub(1))
                        .filter_map(|id| providers.iter().find(|entry| entry.instance_id == id.0))
                        .map(|entry| {
                            image(entry.glyph.clone())
                                .app_class(class("size-3 opacity-35 grayscale"))
                                .accessible(false)
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                )
                .child(
                    view()
                        .app_class(class("relative z-10 inline-flex items-center"))
                        .child(
                            image(current_entry.glyph.clone())
                                .app_class(class("size-3.5 opacity-60"))
                                .accessible(false),
                        ),
                ),
        )
    })
}

/// Port of ProjectFavicon.tsx / ProjectMonogram.tsx, with the client's identity algorithm.
fn project_favicon(project: Project) -> impl IntoElement {
    dynamic(move || {
        project.get().map(|project| {
            let identity = derive_project_identity(&project.title.0);
            match project
                .project_icon
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|icon| icon.0.as_ref())
            {
                Some(ProjectIconOverride::Emoji(emoji)) => text(emoji.emoji.0.clone())
                    .app_class(class(
                        "inline-flex size-4 shrink-0 items-center justify-center leading-none",
                    ))
                    .into_element(),
                Some(ProjectIconOverride::Monogram(monogram)) => {
                    project_monogram(&monogram.text.0, &monogram.color).into_element()
                }
                // Authenticated favicon resources and Lucide overrides need Backend/UI-ICONS.
                _ => project_monogram(&identity.monogram, &identity.color).into_element(),
            }
        })
    })
}

fn project_monogram(label: &str, color: &ProjectIconColor) -> impl IntoElement {
    let foreground = project_icon_color_class_name(color);
    let background = foreground
        .replace("text-", "bg-")
        .replace("-600", "-600/14")
        .replace("-400", "-400/14");
    // NSH has no SVG textLength; native mono text keeps the 8.25px lettering.
    // Its fixed 16px tile converts the unsupported 25% corner radius to 4px.
    view().app_class(class("inline-flex size-4 shrink-0 items-center justify-center")).accessible(false)
        .child(view().app_class(class(&cn([Some("size-full overflow-hidden rounded-[4px] font-mono select-none items-center justify-center"), Some(foreground), Some(&background)])))
            .child(text(label.to_owned()).app_class(class("font-mono text-[8.25px] font-bold leading-none"))))
}

fn sidebar_thread_tooltip(
    model: Memo<Option<EnvironmentThreadShell>>,
    project: impl Fn() -> String + 'static,
    extras: Memo<RowExtras>,
) -> impl IntoElement {
    let providers = computed(move || extras.with(|extras| extras.providers.clone()));
    view().app_class(class("flex min-w-0 max-w-80 flex-col gap-2 px-1 py-2"))
        .child(text(move || SharedString::from(model.with(|model| model.as_ref().map(|model| model.title.clone()).unwrap_or_default()))).app_class(class("min-w-0 truncate text-xs leading-tight font-medium text-foreground")))
        // Flex column replaces CSS grid, preserving sibling order and spacing.
        .child(view().app_class(class("flex flex-col gap-1.5 pl-0.5 text-xs text-muted-foreground"))
            .child(text(move || SharedString::from(project())).app_class(class("min-w-0 truncate text-foreground/75")))
            .child(when(move || model.with(|v| v.as_ref().is_some_and(|v| v.source.branch.is_some())), move || {
                view().app_class(class("flex min-w-0 items-center gap-2"))
                    .child(icon(Icon::GitBranch, 12.0, move || palette().muted_foreground))
                    .child(text(move || SharedString::from(model.with(|v| v.as_ref().and_then(|v| v.source.branch.as_ref().map(|b| b.0.clone())).unwrap_or_default()))))
            }))
            .child(when(move || extras.with(|e| e.environment_label.is_some()), move || text(move || SharedString::from(extras.with(|e| e.environment_label.clone().unwrap_or_default()))).app_class(class("min-w-0 truncate text-foreground/75"))))
            .child(dynamic(move || {
                let model = model.get()?;
                let entries = providers.get();
                let stack = resolve_thread_provider_stack(&model);
                let current = stack.last()?;
                let entry = entries.iter().find(|entry| entry.instance_id == current.0)?;
                Some(view().app_class(class("flex min-w-0 items-center gap-2"))
                    .child(image(entry.glyph.clone()).app_class(class("size-3 shrink-0 grayscale opacity-60")).accessible(false))
                    .child(text(model.source.model_selection.model.clone()).app_class(class("min-w-0 truncate text-foreground/75"))))
            }))
            .child(dynamic(move || {
                let model = model.get()?;
                let entries = providers.get();
                let current = model.runtime.as_ref().map_or(&model.source.model_selection.instance_id, |runtime| &runtime.provider_instance_id);
                let previous = model.source.provider_instance_history.as_ref().and_then(Option::as_ref).into_iter().flatten()
                    .filter(|id| *id != current)
                    .map(|id| entries.iter().find(|entry| entry.instance_id == id.0).map_or_else(|| id.0.clone(), |entry| entry.display_name.clone())).collect::<Vec<_>>();
                (!previous.is_empty()).then(|| text(tr("Handed off from {providers}", &[("providers", Value::Text(previous.join(", ")))]))
                    .app_class(class("min-w-0 truncate text-foreground/75")))
            }))
            .child(when(move || model.with(|v| v.as_ref().is_some_and(|v| v.runtime.as_ref().is_some_and(|r| r.last_error.is_some()))), move || {
                text(move || t(if model.with(|v| v.as_ref().and_then(|v| v.runtime.as_ref()).is_some_and(|r| r.last_error_class == Some(t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass::UsageLimit))) { "Usage limit reached" } else { "Error occurred" }))
                    .app_class(move || Some(class(if model.with(|v| v.as_ref().and_then(|v| v.runtime.as_ref()).is_some_and(|r| r.last_error_class == Some(t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass::UsageLimit))) { "min-w-0 truncate text-warning" } else { "min-w-0 truncate text-error" })))
            })))
}

fn lucide(paths: &str, size: f32) -> nsh_core::element::Image {
    image(nsh_core::element::ImageSrc::svg_markup(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{paths}</svg>")))
        .w(px(size)).h(px(size)).flex_shrink_0().accessible(false)
}

/// TOP_STATUS_ICONS; inline Lucide paths until UI-ICONS exposes these variants.
fn top_status_icon(top: TopStatus) -> impl IntoElement {
    let which = top.icon;
    let paths = match which {
        TopStatusIcon::Working => {
            r#"<path d="M10.1 2.182a10 10 0 0 1 3.8 0"/><path d="M13.9 21.818a10 10 0 0 1-3.8 0"/><path d="M17.609 3.721a10 10 0 0 1 2.69 2.7"/><path d="M2.182 13.9a10 10 0 0 1 0-3.8"/><path d="M20.279 17.609a10 10 0 0 1-2.7 2.69"/><path d="M21.818 10.1a10 10 0 0 1 0 3.8"/><path d="M3.721 6.391a10 10 0 0 1 2.7-2.69"/><path d="M6.391 20.279a10 10 0 0 1-2.69-2.7"/>"#
        }
        TopStatusIcon::Waiting => {
            r#"<path d="M10.1 2.18a9.93 9.93 0 0 1 3.8 0"/><path d="M17.6 3.71a9.95 9.95 0 0 1 2.69 2.7"/><path d="M21.82 10.1a9.93 9.93 0 0 1 0 3.8"/><path d="M20.29 17.6a9.95 9.95 0 0 1-2.7 2.69"/><path d="M13.9 21.82a9.94 9.94 0 0 1-3.8 0"/><path d="M6.4 20.29a9.95 9.95 0 0 1-2.69-2.7"/><path d="M2.18 13.9a9.93 9.93 0 0 1 0-3.8"/><path d="M3.71 6.4a9.95 9.95 0 0 1 2.7-2.69"/><circle cx="12" cy="12" r="1"/>"#
        }
        TopStatusIcon::Input => {
            r#"<path d="M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>"#
        }
        TopStatusIcon::Request => {
            r#"<circle cx="12" cy="8" r="5"/><path d="M20 21a8 8 0 0 0-16 0"/>"#
        }
        TopStatusIcon::Approval => {
            r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="M9.1 9a3 3 0 0 1 5.82 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>"#
        }
        TopStatusIcon::Failed => {
            r#"<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/>"#
        }
        TopStatusIcon::Done => r#"<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>"#,
        TopStatusIcon::Woke => {
            r#"<circle cx="12" cy="13" r="8"/><path d="M12 9v4l2 2"/><path d="M5 3 2 6"/><path d="m22 6-3-3"/><path d="M6.38 18.7 4 21"/><path d="M17.64 18.67 20 21"/>"#
        }
    };
    lucide(paths, 16.0).tint_color(move || match which {
        TopStatusIcon::Working => tokens().color("info-foreground").unwrap_or(palette().info),
        TopStatusIcon::Waiting => palette().muted_foreground,
        TopStatusIcon::Input => AdaptiveColor::new(
            tokens()
                .color("indigo-600")
                .unwrap_or(palette().foreground)
                .light,
            tokens()
                .color("indigo-300")
                .unwrap_or(palette().foreground)
                .dark,
        ),
        TopStatusIcon::Request => tokens()
            .color("guest-foreground")
            .unwrap_or(palette().foreground),
        TopStatusIcon::Approval | TopStatusIcon::Woke => palette().warning_foreground,
        TopStatusIcon::Failed if top.class_name == "text-warning-foreground" => {
            palette().warning_foreground
        }
        TopStatusIcon::Failed => palette().error_foreground,
        TopStatusIcon::Done => tokens()
            .color("success-foreground")
            .unwrap_or(palette().success),
    })
}

const UNDO_ICON: &str = r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/>"#;

const WORKTREE_ICON: &str = r#"<path d="M18 19a5 5 0 0 1-5-5v8"/><path d="M9 20H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H20a2 2 0 0 1 2 2v5"/><circle cx="13" cy="12" r="2"/><circle cx="20" cy="19" r="2"/>"#;

const CLOCK_ICON: &str = r#"<path d="M12 6v6l4 2"/><circle cx="12" cy="12" r="10"/>"#;

const WAKE_ICON: &str = r#"<path d="M6.87 6.87a8 8 0 1 0 11.26 11.26"/><path d="M19.9 14.25a8 8 0 0 0-9.15-9.15"/><path d="m22 6-3-3"/><path d="M6.26 18.67 4 21"/><path d="m2 2 20 20"/><path d="M4 4 2 6"/>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::rc::Rc;
    use t3_client::types::EnvironmentId;

    fn thread(extra: Value) -> EnvironmentThreadShell {
        let mut value = json!({
            "id":"thread", "projectId":"project", "title":"Port the row",
            "providerInstanceId":"codex", "modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
            "runtimeMode":"full-access", "interactionMode":"default", "branch":null,"worktreePath":null,
            "lineage":{"rootThreadId":"thread","parentThreadId":null,"relationshipToParent":null},
            "forkedFrom":null,"activeProviderThreadId":null,"latestRunId":null,"activeRunId":null,
            "status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,
            "hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,
            "createdBy":"user","creationSource":"web","createdAt":"2026-11-13T10:00:00.000Z",
            "updatedAt":"2026-11-13T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null
        });
        value
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        present_thread_shell(
            EnvironmentId("host".into()),
            Rc::new(serde_json::from_value(value).expect("shell")),
            None,
        )
    }

    #[test]
    fn status_priority_and_component_labels_match_sidebar_thread_row() {
        for (status, goal, woke, unread, label) in [
            (Status::Preparing, false, true, true, "Setting up"),
            (Status::Working, false, true, true, "Working"),
            (Status::Working, true, true, true, "Goal"),
            (Status::Waiting, false, true, true, "Waiting"),
            (Status::Approval, false, true, true, "Approval"),
            (Status::Input, false, true, true, "Input"),
            (Status::Request, false, true, true, "Request"),
            (Status::Limited, false, true, true, "Limited"),
            (Status::Failed, false, true, true, "Failed"),
            (Status::Ready, false, true, true, "Woke"),
            (Status::Ready, false, false, true, "Done"),
        ] {
            assert_eq!(
                top_status(status, goal, woke, unread).expect("label").label,
                label
            );
        }
        assert!(top_status(Status::Ready, false, false, false).is_none());
    }

    #[test]
    fn bridges_the_clients_status_without_losing_requests_or_usage_limits() {
        for (extra, expected) in [
            (
                json!({"latestRunId":"run","status":"running","blockedOnThreadWait":true}),
                "Waiting",
            ),
            (
                json!({"latestRunId":"run","status":"preparing"}),
                "Setting up",
            ),
            (
                json!({"latestRunId":"run","status":"running","pendingGuestMessageCount":1}),
                "Request",
            ),
            (
                json!({"latestRunId":"run","status":"failed","lastErrorClass":"usage_limit"}),
                "Limited",
            ),
            (
                json!({"latestRunId":"run","status":"running","lastErrorClass":"usage_limit"}),
                "Working",
            ),
            (json!({"latestRunId":"run","status":"failed"}), "Failed"),
        ] {
            assert_eq!(
                presentation(&thread(extra), &RowExtras::default(), false)
                    .top
                    .expect("status")
                    .label,
                expected
            );
        }
        let running = thread(json!({"latestRunId":"run","status":"running"}));
        for (approval, input, expected) in [(false, true, "Input"), (true, true, "Approval")] {
            let mut model = running.clone();
            model.has_pending_user_input = input;
            model.has_pending_approvals = approval;
            assert_eq!(
                presentation(&model, &RowExtras::default(), false)
                    .top
                    .expect("status")
                    .label,
                expected
            );
        }
    }

    #[test]
    fn unseen_completion_obeys_server_watermarks_and_legacy_fallback() {
        for (extra, local, unread) in [
            (json!({}), None, false),
            (json!({}), Some("2026-11-13T10:00:00.000Z"), true),
            (
                json!({"lastVisitedAt":null}),
                Some("2026-11-13T10:00:00.000Z"),
                false,
            ),
            (
                json!({"lastVisitedAt":"2026-11-13T12:00:00.000Z"}),
                Some("2026-11-13T10:00:00.000Z"),
                false,
            ),
            (json!({"lastVisitedAt":"invalid"}), None, true),
        ] {
            let mut base = json!({"latestRunId":"run","status":"completed","latestRunCompletedAt":"2026-11-13T11:00:00.000Z"});
            base.as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            let extras = RowExtras {
                local_last_visited_at: local.map(str::to_owned),
                ..Default::default()
            };
            let result = presentation(&thread(base), &extras, false);
            assert_eq!(result.unread, unread);
            assert_eq!(result.receded, !unread);
        }
    }

    #[test]
    fn completed_background_rosters_only_wait_when_awaited() {
        for (kind, awaited, label, receded) in [
            ("command", false, "Done", false),
            ("monitor", false, "Done", false),
            ("command", true, "Waiting", true),
        ] {
            let mut task = json!({"taskId":"background","kind":kind});
            if awaited {
                task["awaited"] = json!(true);
            }
            let model = thread(
                json!({"latestRunId":"run","status":"completed","latestRunCompletedAt":"2026-11-13T11:00:00.000Z","lastVisitedAt":"2026-11-13T10:00:00.000Z","pendingBackgroundTasks":[task]}),
            );
            let result = presentation(&model, &RowExtras::default(), false);
            assert_eq!(result.top.expect("status").label, label);
            assert_eq!(result.receded, receded);
        }
    }

    #[test]
    fn waking_requires_a_newer_unread_wake_and_never_overrides_settled() {
        let extras = RowExtras {
            woke_at: Some("2026-11-13T11:00:00.000Z".into()),
            ..Default::default()
        };
        for (extra, woke) in [
            (json!({}), true),
            (json!({"lastVisitedAt":"2026-11-13T11:00:00.000Z"}), false),
            (json!({"lastVisitedAt":"invalid"}), true),
            (json!({"settledOverride":"settled"}), false),
        ] {
            assert_eq!(presentation(&thread(extra), &extras, false).woke, woke);
        }
        let quiet = thread(json!({}));
        assert!(presentation(&quiet, &RowExtras::default(), false).receded);
        assert!(!presentation(&quiet, &RowExtras::default(), true).receded);
        assert!(
            !presentation(
                &quiet,
                &RowExtras {
                    selected: true,
                    ..Default::default()
                },
                false
            )
            .receded
        );
    }

    #[test]
    fn duration_uses_host_clock_and_never_invents_an_activity_start() {
        let now = timestamp_ms("2026-11-13T11:05:00.000Z").expect("date");
        let completed = thread(
            json!({"latestRunId":"run","activeRunId":"run","status":"completed","latestRunStartedAt":"2026-11-13T11:00:00.000Z","latestRunCompletedAt":"2026-11-13T11:02:00.000Z"}),
        );
        assert_eq!(
            working_duration_label(&completed, Status::Working, Some(now)),
            ""
        );
        assert_eq!(
            working_duration_label(&completed, Status::Waiting, Some(now)),
            "3m"
        );
        for state in ["queued", "cancelled"] {
            let model = thread(
                json!({"latestRunId":"newer","activeRunId":"older","status":state,"activityRunStatus":"running","activityRunStartedAt":"2026-11-13T11:00:00.000Z"}),
            );
            assert_eq!(
                working_duration_label(&model, Status::Working, Some(now)),
                "5m"
            );
            assert_eq!(working_duration_label(&model, Status::Working, None), "");
            assert_eq!(
                working_duration_label(&model, Status::Working, Some(now - 3_600_000)),
                "0s"
            );
            assert_eq!(
                working_duration_label(&model, Status::Working, Some(now + 3_600_000)),
                "1h 5m"
            );
        }
    }

    #[test]
    fn relative_labels_and_boundary_schedule_use_the_clients_rules() {
        let now = timestamp_ms("2026-11-13T12:10:00.000Z").expect("date");
        for (at, label) in [
            ("2026-11-13T12:05:00.000Z", "5m"),
            ("2026-11-13T10:00:00.000Z", "2h"),
            ("2026-11-13T12:09:59.000Z", "now"),
            ("not a date", ""),
        ] {
            assert_eq!(relative_label(at, now), label);
        }
        assert_eq!(ms_until_relative_time_change(now - 59_999, now), 1);
        assert_eq!(
            ms_until_relative_time_change(now - 3_600_001, now),
            3_599_999
        );
        assert_eq!(
            compact_sidebar_time_label("3d ago", "just now", "now"),
            "3d"
        );
    }
}
