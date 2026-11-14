//! Port of apps/web/src/components/chat/MessagesTimeline.tsx WorktreeSetupTimelineRow and
//! apps/web/src/components/chat/WorktreeSetupCard.tsx WorktreeSetupCard.
//!
//! The snapshot comes from the client's row (`MessagesTimelineRowContent::WorktreeSetup`); the
//! actions are the client's `worktree_setup` service, bound in `runtime/services/thread_ops.rs`.
//! Departures: the running label's shimmer overlay (`live-activity-focus`, a continuous
//! animation) is not drawn, and the running stage's Spinner is the crate's static glyph.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::types::ThreadKey;
use t3_client::view_logic::session::timestamps::format_duration;
use t3_contracts::worktree_setup::{
    WorktreeSetupPhase, WorktreeSetupSnapshot, WorktreeSetupStage, WorktreeSetupStageId,
    WorktreeSetupStageStatus,
};

use crate::i18n::t;
use crate::runtime::Shared;
use crate::runtime::services::thread_ops::{WorktreeSetupBinding, host_worktree_setup};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::middle_truncate::{Options as TruncateOptions, middle_truncate};
use crate::ui::skeleton::{SpinnerSize, Tone, spinner};

use crate::runtime::tasks::watch;

/// The client row's data: `{ kind: "worktree-setup", snapshot, embedded }`.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeSetupRow {
    pub id: String,
    pub snapshot: Rc<WorktreeSetupSnapshot>,
    pub embedded: bool,
}

impl WorktreeSetupRow {
    /// None for other kinds and for a snapshot that does not decode (never shown as another one).
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let MessagesTimelineRowContent::WorktreeSetup { snapshot, embedded } = &row.content else {
            return None;
        };
        Some(Self {
            id: row.id.clone(),
            snapshot: Rc::new(serde_json::from_value(snapshot.clone()).ok()?),
            embedded: *embedded,
        })
    }
}

/// What MessagesTimeline's row context gives this row, per thread, created above the list.
#[derive(Clone)]
pub struct WorktreeSetupContext {
    key: ThreadKey,
    client: Option<WorktreeSetupBinding>,
    /// WorktreeSetupCard's `detailsOpen`, by row id, so it survives recycling.
    details: Signal<BTreeSet<String>>,
    owner: Owner,
    /// ChatView passes `onWorktreeSetupWorkLocally` only while the thread is a local draft.
    pub work_locally: Option<Rc<dyn Fn()>>,
}

impl WorktreeSetupContext {
    /// Reads the host client; without one (the demo, an unavailable host) Cancel and Open
    /// terminal are absent, as the web leaves them out when their callbacks are null.
    pub fn new(key: ThreadKey) -> Self {
        Self::with_client(key, host_worktree_setup())
    }

    pub fn with_client(key: ThreadKey, client: Option<WorktreeSetupBinding>) -> Self {
        Self {
            key,
            client,
            details: signal(BTreeSet::new()),
            owner: Owner::current(),
            work_locally: None,
        }
    }
}

/// Port of MessagesTimeline.tsx WorktreeSetupTimelineRow.
pub fn worktree_setup_timeline_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    context: WorktreeSetupContext,
) -> impl IntoElement {
    let setup = computed(move || {
        row.with(|row| {
            row.as_ref()
                .and_then(|row| WorktreeSetupRow::from_row(&row.0))
        })
    });
    dynamic(move || {
        let setup = setup.get()?;
        let context = context.clone();
        Some(untracked(move || worktree_setup_card(setup, context).into_element()))
    })
}

/// Port of WorktreeSetupCard.tsx WorktreeSetupCard, with the row's callbacks resolved as
/// WorktreeSetupTimelineRow resolves them.
fn worktree_setup_card(row: WorktreeSetupRow, context: WorktreeSetupContext) -> impl IntoElement {
    let snapshot = row.snapshot.clone();
    let running = snapshot.phase == WorktreeSetupPhase::Running;
    let setup_stage = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == WorktreeSetupStageId::SetupScript)
        .cloned();
    let terminal_id = snapshot
        .setup_script
        .as_ref()
        .map(|script| script.terminal_id.0.clone());
    // useWorktreeSetup exposes the terminal only for the setup of the thread being shown.
    let open_terminal = context
        .client
        .clone()
        .zip(terminal_id)
        .filter(|_| snapshot.thread_id == context.key.thread);
    let show_terminal = open_terminal.is_some()
        && setup_stage
            .as_ref()
            .is_some_and(|stage| stage.status != WorktreeSetupStageStatus::Pending);
    let cancel = (!row.embedded).then(|| context.client.clone()).flatten();
    let work_locally = (!row.embedded && running)
        .then(|| context.work_locally.clone())
        .flatten();
    let collapsed = row.embedded && !running;
    let show_header = !row.embedded && !running && snapshot.phase != WorktreeSetupPhase::Done;
    let show_tail = setup_stage.as_ref().is_some_and(|stage| {
        stage.status == WorktreeSetupStageStatus::Failed
            || (stage.status == WorktreeSetupStageStatus::Running && !stage.tail.is_empty())
    });
    // useNowSecond: one tick per second while setup runs; a finished card never ticks.
    let now = running.then(|| {
        let now = signal(unix_ms());
        set_interval(Duration::from_secs(1), move || now.set(unix_ms()));
        now
    });
    let details = context.details;
    let id = row.id.clone();
    let details_open = computed(move || details.with(|open| open.contains(&id)));

    let body = if collapsed {
        collapsed_summary_row(&snapshot).into_element()
    } else {
        let mut stages = view().app_class(tw(if show_header { "pt-1.5" } else { "" }));
        for stage in &snapshot.stages {
            let mut slot = view().child(stage_row(
                stage,
                running,
                snapshot.ended_at.as_ref().map(|at| at.0.as_str()),
                snapshot.setup_script.as_ref().map(|s| s.name.0.as_str()),
                now,
            ));
            if stage.id == WorktreeSetupStageId::SetupScript && show_tail {
                slot = slot.child(output_tail(
                    &stage.tail,
                    stage.status == WorktreeSetupStageStatus::Failed,
                ));
            }
            stages = stages.child(slot);
        }
        stages.into_element()
    };

    let toggle_id = row.id.clone();
    let owner = context.owner;
    let key = context.key.clone();
    let mut actions = view()
        // ml-[calc(--spacing(6)+2px-(--spacing(2)-1px))] at the 4px spacing unit.
        .app_class(tw("mt-0.5 ml-[19px] flex flex-wrap items-center gap-0.5"))
        .child(dynamic(move || {
            let open = details_open.get();
            let toggle_id = toggle_id.clone();
            Some(untracked(move || {
                button(
                    ButtonContent::IconLabel {
                        icon: if open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        label: t("Details"),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::Xs,
                    || false,
                    move || {
                        details.update(|set| {
                            if !set.remove(&toggle_id) {
                                set.insert(toggle_id.clone());
                            }
                        })
                    },
                )
                .into_element()
            }))
        }));
    if show_terminal && let Some((client, terminal)) = open_terminal {
        let (key, setup) = (key.clone(), snapshot.clone());
        actions = actions.child(button(
            ButtonContent::IconLabel {
                icon: Icon::Terminal,
                label: t("Open terminal"),
            },
            ButtonVariant::GhostMuted,
            ButtonSize::Xs,
            || false,
            move || {
                client.0.open_terminal(&key, &setup, &terminal);
            },
        ));
    }
    if let Some(work_locally) = work_locally {
        actions = actions.child(button(
            ButtonContent::IconLabel {
                icon: Icon::Laptop,
                label: t("Work locally"),
            },
            ButtonVariant::GhostMuted,
            ButtonSize::Xs,
            || false,
            move || work_locally(),
        ));
    }
    if running && let Some(client) = cancel {
        let setup = snapshot.clone();
        actions = actions.child(button(
            ButtonContent::IconLabel {
                icon: Icon::X,
                label: t("Cancel"),
            },
            ButtonVariant::GhostMuted,
            ButtonSize::Xs,
            || false,
            move || {
                // ChatView's `void cancelWorktreeSetup()`: the web reports no failure here.
                let cancelled = client.0.cancel(&key, &setup);
                owner.run(|| {
                    watch(async move {
                        let _ = cancelled.await;
                    })
                });
            },
        ));
    }

    let failure = (snapshot.phase == WorktreeSetupPhase::Failed)
        .then(|| snapshot.error.clone())
        .flatten();
    let details_snapshot = snapshot.clone();
    view()
        .role(AccessibilityRole::Region)
        .label(t("Worktree setup"))
        .test_id("worktree-setup")
        .child(show_header.then(|| setup_header_row(&snapshot, now)))
        .child(body)
        .child(failure.map(|error| {
            text(error).app_class(tw("mt-1 ml-8 text-xs text-muted-foreground"))
        }))
        .child(actions)
        .child(dynamic(move || {
            details_open.get().then(|| {
                let snapshot = details_snapshot.clone();
                untracked(move || setup_details(&snapshot))
            })
        }))
}

fn unix_ms() -> i64 {
    nsh_core::time::unix_ms() as i64
}

/// WorktreeSetupCard.tsx elapsedMs over `Date.parse`.
fn elapsed_ms(started_at: Option<&str>, end_ms: i64) -> Option<i64> {
    let start = t3_client::time::epoch_ms(started_at?)?;
    Some((end_ms - start).max(0))
}

/// WorktreeSetupCard.tsx Elapsed and LiveElapsed: a finished duration, or the ticking one
/// while setup runs, formatted by the client's `formatDuration`.
fn elapsed_label(
    started_at: Option<&str>,
    ended_at: Option<&str>,
    live: bool,
    now_ms: i64,
) -> Option<String> {
    let started_at = started_at?;
    if let Some(ended_at) = ended_at {
        let end = t3_client::time::epoch_ms(ended_at)?;
        return elapsed_ms(Some(started_at), end).map(|ms| format_duration(ms as f64));
    }
    live.then(|| elapsed_ms(Some(started_at), now_ms))
        .flatten()
        .map(|ms| format_duration(ms as f64))
}

/// The Elapsed slot with the source's `empty:hidden`: no element while there is no duration.
fn elapsed(
    class: &'static str,
    started_at: Option<String>,
    ended_at: Option<String>,
    live: bool,
    now: Option<Signal<i64>>,
) -> impl IntoElement {
    let label = computed(move || {
        let now_ms = match now {
            Some(now) if live && ended_at.is_none() => now.get(),
            _ => 0,
        };
        elapsed_label(started_at.as_deref(), ended_at.as_deref(), live, now_ms)
    });
    dynamic(move || {
        label.get().map(|label| {
            text(move || SharedString::from(label.clone()))
                .app_class(tw(class))
                .into_element()
        })
    })
}

/// Port of WorktreeSetupCard.tsx StageIcon.
fn stage_icon(status: WorktreeSetupStageStatus) -> AnyElement {
    let class = "size-4 shrink-0 stroke-2";
    match status {
        WorktreeSetupStageStatus::Done => icon_with_class(Icon::Check, class).into_element(),
        WorktreeSetupStageStatus::Running => view()
            .app_class(tw("shrink-0"))
            .child(spinner(SpinnerSize::Md, Tone::Current))
            .into_element(),
        WorktreeSetupStageStatus::Failed => icon_with_class(Icon::X, class).into_element(),
        WorktreeSetupStageStatus::Warning => {
            icon_with_class(Icon::CircleAlert, class).into_element()
        }
        WorktreeSetupStageStatus::Skipped => icon_with_class(Icon::Minus, class).into_element(),
        WorktreeSetupStageStatus::Pending => icon_with_class(Icon::Circle, class).into_element(),
    }
}

/// Port of WorktreeSetupCard.tsx stageRowClassName.
fn stage_row_class_name(status: WorktreeSetupStageStatus) -> &'static str {
    match status {
        WorktreeSetupStageStatus::Failed => "text-destructive-foreground",
        WorktreeSetupStageStatus::Warning => "text-warning-foreground",
        WorktreeSetupStageStatus::Pending => "text-secondary-label opacity-40",
        WorktreeSetupStageStatus::Running
        | WorktreeSetupStageStatus::Skipped
        | WorktreeSetupStageStatus::Done => "text-secondary-label",
    }
}

/// Port of packages/contracts/src/worktreeSetup.ts worktreeSetupStageLabel.
fn worktree_setup_stage_label(id: WorktreeSetupStageId) -> String {
    match id {
        WorktreeSetupStageId::Fetch => t("Fetch base branch"),
        WorktreeSetupStageId::Checkout => t("Check out files"),
        WorktreeSetupStageId::Submodules => t("Init submodules"),
        WorktreeSetupStageId::SetupScript => t("Run setup script"),
        WorktreeSetupStageId::Agent => t("Start agent"),
    }
}

/// Port of WorktreeSetupCard.tsx headerLabel.
fn header_label(snapshot: &WorktreeSetupSnapshot) -> String {
    match snapshot.phase {
        WorktreeSetupPhase::Running => t("Setting up worktree…"),
        WorktreeSetupPhase::Done => {
            if snapshot
                .stages
                .iter()
                .any(|stage| stage.status == WorktreeSetupStageStatus::Failed)
            {
                t("Worktree ready, setup script failed")
            } else {
                t("Worktree ready")
            }
        }
        WorktreeSetupPhase::Failed => t("Worktree setup failed"),
        WorktreeSetupPhase::Cancelled => t("Worktree setup cancelled"),
    }
}

/// Port of WorktreeSetupCard.tsx SetupHeaderRow (shown only once setup stopped running).
fn setup_header_row(snapshot: &WorktreeSetupSnapshot, now: Option<Signal<i64>>) -> impl IntoElement {
    let running = snapshot.phase == WorktreeSetupPhase::Running;
    let failed = snapshot.phase == WorktreeSetupPhase::Failed;
    let finished_with_failed_stage = snapshot.phase == WorktreeSetupPhase::Done
        && snapshot
            .stages
            .iter()
            .any(|stage| stage.status == WorktreeSetupStageStatus::Failed);
    let tone = if failed {
        "text-destructive-foreground"
    } else if finished_with_failed_stage {
        "text-warning-foreground"
    } else {
        "text-muted-foreground"
    };
    view().app_class(tw("border-b border-border/60 pb-2 pt-1")).child(
        view()
            .app_class(tw(&cn([
                Some("flex h-6 min-w-0 items-baseline gap-2 px-1 text-sm leading-relaxed tabular-nums"),
                Some(tone),
            ])))
            .child(
                view()
                    .app_class(tw("relative min-w-0 shrink overflow-hidden whitespace-nowrap"))
                    .child(text(header_label(snapshot)).app_class(tw("block truncate"))),
            )
            .child(elapsed(
                "ml-auto shrink-0 text-xs text-muted-foreground",
                Some(snapshot.started_at.0.clone()),
                snapshot.ended_at.as_ref().map(|at| at.0.clone()),
                running,
                now,
            )),
    )
}

/// Port of WorkLog.tsx WorkLogRow and WorkLogLine, outside a WorkLogList (`py-0.5`), with
/// the `[&_*]` selectors on the children they select. NSH has no cap-relative baseline:
/// the 24px icon slot is kept as TL-ROWS-WORK-2's port keeps it.
fn work_log_row(
    stage: &str,
    status: &str,
    icon: AnyElement,
    label: AnyElement,
    trailing: Vec<AnyElement>,
) -> impl IntoElement {
    let line = view()
        .app_class(tw("flex min-h-6 min-w-0 gap-1.5 text-sm leading-relaxed select-none items-center"))
        .child(
                    view()
                        .app_class(tw("flex min-w-0 flex-1 items-baseline gap-1.5"))
                        .child(
                            view().app_class(tw("relative w-6 shrink-0 leading-0")).child(
                                view().app_class(tw("size-6 overflow-hidden")).child(
                                    view()
                                        .app_class(tw("flex size-full items-center justify-center"))
                                        .child(icon),
                                ),
                            ),
                        )
                        .child(
                            view()
                                .app_class(tw("min-w-0 flex-1 text-secondary-label truncate"))
                                .child(label),
                        ),
                );
    view()
        .app_class(tw("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 text-left transition-colors py-0.5"))
        .test_id(format!("worktree-setup-stage-{stage}-{status}"))
        .child(trailing.into_iter().fold(line, |line, part| line.child(part)))
}

fn status_name(status: WorktreeSetupStageStatus) -> &'static str {
    match status {
        WorktreeSetupStageStatus::Pending => "pending",
        WorktreeSetupStageStatus::Running => "running",
        WorktreeSetupStageStatus::Done => "done",
        WorktreeSetupStageStatus::Skipped => "skipped",
        WorktreeSetupStageStatus::Warning => "warning",
        WorktreeSetupStageStatus::Failed => "failed",
    }
}

fn stage_name(id: WorktreeSetupStageId) -> &'static str {
    match id {
        WorktreeSetupStageId::Fetch => "fetch",
        WorktreeSetupStageId::Checkout => "checkout",
        WorktreeSetupStageId::Submodules => "submodules",
        WorktreeSetupStageId::SetupScript => "setup-script",
        WorktreeSetupStageId::Agent => "agent",
    }
}

/// WorktreeSetupCard.tsx StageRow's trailing text: pending shows none, skipped its detail or
/// "skipped", a running checkout its percent, otherwise the stage's detail.
fn stage_trailing(stage: &WorktreeSetupStage) -> Option<String> {
    match stage.status {
        WorktreeSetupStageStatus::Pending => None,
        WorktreeSetupStageStatus::Skipped => {
            Some(stage.detail.clone().unwrap_or_else(|| t("skipped")))
        }
        WorktreeSetupStageStatus::Running
            if stage.id == WorktreeSetupStageId::Checkout && stage.percent.is_some() =>
        {
            stage.percent.map(|percent| format!("{percent}%"))
        }
        _ => stage.detail.clone(),
    }
    .filter(|trailing| !trailing.is_empty())
}

/// Port of WorktreeSetupCard.tsx StageRow.
fn stage_row(
    stage: &WorktreeSetupStage,
    setup_running: bool,
    setup_ended_at: Option<&str>,
    script_name: Option<&str>,
    now: Option<Signal<i64>>,
) -> impl IntoElement {
    let label = match script_name {
        Some(name) if stage.id == WorktreeSetupStageId::SetupScript => name.to_owned(),
        _ => worktree_setup_stage_label(stage.id),
    };
    let pending = stage.status == WorktreeSetupStageStatus::Pending;
    let mut trailing = Vec::new();
    if let Some(detail) = stage_trailing(stage) {
        trailing.push(
            text(detail)
                .app_class(tw("max-w-1/2 min-w-0 truncate text-xs text-muted-foreground tabular-nums"))
                .into_element(),
        );
    }
    if !matches!(
        stage.status,
        WorktreeSetupStageStatus::Skipped | WorktreeSetupStageStatus::Pending
    ) {
        trailing.push(
            elapsed(
                "shrink-0 text-xs text-muted-foreground tabular-nums",
                stage.started_at.as_ref().map(|at| at.0.clone()),
                // A stage interrupted by the setup ending stops with it.
                stage
                    .ended_at
                    .as_ref()
                    .map(|at| at.0.clone())
                    .or_else(|| setup_ended_at.map(str::to_owned)),
                setup_running,
                now,
            )
            .into_element(),
        );
    }
    work_log_row(
        stage_name(stage.id),
        status_name(stage.status),
        view()
            .app_class(tw(&cn([Some("text-icon-muted"), pending.then_some("opacity-40")])))
            .child(stage_icon(stage.status))
            .into_element(),
        text(label)
            .app_class(tw(&cn([Some("relative block truncate"), Some(stage_row_class_name(stage.status))])))
            .into_element(),
        trailing,
    )
}

const OUTPUT_TAIL_LINES: usize = 4;

/// WorktreeSetupCard.tsx OutputTail's four slots: the last four lines, blank slots first.
fn output_tail_slots(lines: &[String]) -> [&str; OUTPUT_TAIL_LINES] {
    std::array::from_fn(|slot| {
        (lines.len() + slot)
            .checked_sub(OUTPUT_TAIL_LINES)
            .and_then(|index| lines.get(index))
            .map_or("", String::as_str)
    })
}

/// Port of WorktreeSetupCard.tsx OutputTail. `pre`'s `overflow-hidden` is the view's clip.
fn output_tail(lines: &[String], failed: bool) -> impl IntoElement {
    let mut pre = view().app_class(tw(&cn([
        Some("mb-1 ml-8 overflow-hidden rounded-md border px-2.5 py-1.5 font-mono text-2xs leading-relaxed select-text"),
        Some(if failed {
            "border-destructive/20 bg-error-surface text-destructive-foreground"
        } else {
            "border-border bg-code text-muted-foreground"
        }),
    ])));
    for line in output_tail_slots(lines) {
        let line = if line.is_empty() { "\u{a0}" } else { line };
        pre = pre.child(text(line.to_owned()).app_class(tw("truncate")));
    }
    pre
}

/// Port of WorktreeSetupCard.tsx SetupDetails.
fn setup_details(snapshot: &WorktreeSetupSnapshot) -> AnyElement {
    let mut list = view().app_class(tw("mt-1 mb-1.5 ml-8 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs text-muted-foreground"));
    let truncated = |value: String| -> AnyElement {
        view()
            .app_class(tw("min-w-0 font-mono"))
            .child(
                view().app_class(tw("flex")).child(middle_truncate(
                    move || SharedString::from(value.clone()),
                    TruncateOptions::default(),
                )),
            )
            .into_element()
    };
    for (term, value) in [
        (t("Branch"), snapshot.branch.as_ref()),
        (t("Base"), snapshot.base_ref.as_ref()),
        (t("Path"), snapshot.worktree_path.as_ref()),
    ] {
        if let Some(value) = value {
            list = list
                .child(text(term).app_class(tw("text-foreground/80")))
                .child(truncated(value.0.clone()));
        }
    }
    if let Some(script) = &snapshot.setup_script {
        list = list
            .child(text(t("Setup")).app_class(tw("text-foreground/80")))
            .child(text(script.command.0.clone()).app_class(tw("truncate font-mono")));
    }
    list.into_element()
}

/// Port of WorktreeSetupCard.tsx CollapsedSummaryRow (an embedded, finished setup).
fn collapsed_summary_row(snapshot: &WorktreeSetupSnapshot) -> impl IntoElement {
    let status = collapsed_status(snapshot);
    work_log_row(
        "summary",
        status_name(status),
        view()
            .app_class(tw("text-icon-muted"))
            .child(stage_icon(status))
            .into_element(),
        text(header_label(snapshot))
            .app_class(tw(stage_row_class_name(status)))
            .into_element(),
        vec![
            elapsed(
                "shrink-0 text-xs text-muted-foreground tabular-nums",
                Some(snapshot.started_at.0.clone()),
                snapshot.ended_at.as_ref().map(|at| at.0.clone()),
                false,
                None,
            )
            .into_element(),
        ],
    )
}

fn collapsed_status(snapshot: &WorktreeSetupSnapshot) -> WorktreeSetupStageStatus {
    if matches!(
        snapshot.phase,
        WorktreeSetupPhase::Failed | WorktreeSetupPhase::Cancelled
    ) || snapshot.stages.iter().any(|stage| {
        stage.id == WorktreeSetupStageId::SetupScript
            && stage.status == WorktreeSetupStageStatus::Failed
    }) {
        WorktreeSetupStageStatus::Failed
    } else {
        WorktreeSetupStageStatus::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: &str = "2026-10-26T12:00:10.000Z";

    fn now() -> i64 {
        t3_client::time::epoch_ms(NOW).unwrap()
    }

    fn snapshot(phase: &str, ended_at: Option<&str>) -> WorktreeSetupSnapshot {
        serde_json::from_value(json!({"threadId":"thread-1","phase":phase,"startedAt":"2026-10-26T12:00:00.000Z",
            "endedAt":ended_at,"branch":"t3/feature","baseRef":"main","worktreePath":"/repo/.t3/worktrees/feature",
            "setupScript":null,"stages":[
                {"id":"fetch","status":"done","startedAt":"2026-10-26T12:00:00.000Z","endedAt":"2026-10-26T12:00:03.000Z","percent":null,"detail":null,"tail":[]},
                {"id":"checkout","status":"running","startedAt":"2026-10-26T12:00:03.000Z","endedAt":null,"percent":null,"detail":null,"tail":[]}],
            "error":null,"sequence":1}))
        .unwrap()
    }

    fn stage_elapsed(snapshot: &WorktreeSetupSnapshot, index: usize, now: i64) -> Option<String> {
        let stage = &snapshot.stages[index];
        elapsed_label(
            stage.started_at.as_ref().map(|at| at.0.as_str()),
            stage
                .ended_at
                .as_ref()
                .or(snapshot.ended_at.as_ref())
                .map(|at| at.0.as_str()),
            snapshot.phase == WorktreeSetupPhase::Running,
            now,
        )
    }

    /// WorktreeSetupCard.browser.test.tsx: "ticks the running stage while finished stages keep
    /// their duration".
    #[test]
    fn running_stage_ticks_while_finished_stages_keep_their_duration() {
        let running = snapshot("running", None);
        assert_eq!(stage_elapsed(&running, 0, now()).as_deref(), Some("3s"));
        assert_eq!(stage_elapsed(&running, 1, now()).as_deref(), Some("7s"));
        assert_eq!(
            stage_elapsed(&running, 0, now() + 5_000).as_deref(),
            Some("3s")
        );
        assert_eq!(
            stage_elapsed(&running, 1, now() + 5_000).as_deref(),
            Some("12s")
        );
    }

    /// WorktreeSetupCard.browser.test.tsx: "stops a stage interrupted by the setup ending at
    /// the setup's end".
    #[test]
    fn a_stage_interrupted_by_the_setup_ending_stops_at_the_setup_end() {
        let failed = snapshot("failed", Some("2026-10-26T12:00:05.000Z"));
        assert_eq!(stage_elapsed(&failed, 1, now()).as_deref(), Some("2s"));
        assert_eq!(
            stage_elapsed(&failed, 1, now() + 5_000).as_deref(),
            Some("2s")
        );
    }

    #[test]
    fn unparseable_or_missing_starts_show_no_duration() {
        assert_eq!(elapsed_label(None, None, true, now()), None);
        assert_eq!(elapsed_label(Some("soon"), None, true, now()), None);
        assert_eq!(
            elapsed_label(Some("2026-10-26T12:00:00.000Z"), None, false, now()),
            None
        );
        // A start after the clock never shows a negative duration.
        assert_eq!(
            elapsed_label(Some("2026-10-26T12:01:00.000Z"), None, true, now()).as_deref(),
            Some("1ms")
        );
    }

    #[test]
    fn output_tail_keeps_the_last_four_lines_with_blank_slots_first() {
        let lines: Vec<String> = ["a", "b"].map(String::from).to_vec();
        assert_eq!(output_tail_slots(&lines), ["", "", "a", "b"]);
        let lines: Vec<String> = ["1", "2", "3", "4", "5"].map(String::from).to_vec();
        assert_eq!(output_tail_slots(&lines), ["2", "3", "4", "5"]);
    }

    #[test]
    fn trailing_text_follows_the_stage_status() {
        let mut stage = snapshot("running", None).stages[1].clone();
        stage.percent = Some(42);
        assert_eq!(stage_trailing(&stage).as_deref(), Some("42%"));
        stage.status = WorktreeSetupStageStatus::Done;
        stage.detail = Some("main".into());
        assert_eq!(stage_trailing(&stage).as_deref(), Some("main"));
        stage.status = WorktreeSetupStageStatus::Pending;
        assert_eq!(stage_trailing(&stage), None);
        stage.status = WorktreeSetupStageStatus::Skipped;
        stage.detail = None;
        assert_eq!(stage_trailing(&stage).as_deref(), Some("skipped"));
    }

    #[test]
    fn collapsed_summary_fails_for_a_failed_setup_script_or_phase() {
        let mut done = snapshot("done", Some("2026-10-26T12:00:05.000Z"));
        assert_eq!(collapsed_status(&done), WorktreeSetupStageStatus::Done);
        done.stages.push(WorktreeSetupStage {
            id: WorktreeSetupStageId::SetupScript,
            status: WorktreeSetupStageStatus::Failed,
            started_at: None,
            ended_at: None,
            percent: None,
            detail: None,
            tail: Vec::new(),
        });
        assert_eq!(collapsed_status(&done), WorktreeSetupStageStatus::Failed);
        assert_eq!(header_label(&done), "Worktree ready, setup script failed");
        let cancelled = snapshot("cancelled", Some("2026-10-26T12:00:05.000Z"));
        assert_eq!(collapsed_status(&cancelled), WorktreeSetupStageStatus::Failed);
    }

    #[test]
    fn a_snapshot_that_does_not_decode_is_not_a_row() {
        let row: MessagesTimelineRow = serde_json::from_value(json!({
            "id":"setup","createdAt":null,"kind":"worktree-setup","snapshot":{"phase":"running"},"embedded":false
        }))
        .unwrap();
        assert_eq!(WorktreeSetupRow::from_row(&row), None);
    }
}
