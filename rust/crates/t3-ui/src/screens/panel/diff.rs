//! Port of apps/web/src/components/DiffPanel.tsx DiffPanel, with DiffPanelShell.tsx in its
//! `embedded` mode (the right panel's only mode) and chat/DiffStatLabel.tsx.
//!
//! The rows are NSH's `DiffView` (the plan's `nsh_base::diff_view`), standing where the web
//! mounts @pierre/diffs' code view: it parses the patch, makes only the rows in view, folds
//! files under their headers, wraps or scrolls lines and highlights each file within a
//! per-frame budget. It is the panel's only scroll container. Selection, queries, file facts
//! and preferences are the client's `workspace::diff_panel`.

use std::{rc::Rc, time::Duration};

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_base::{DiffFile, DiffInline, DiffView, DiffViewApi, DiffViewClasses};
use nsh_core::prelude::*;
use t3_client::{
    ClientError,
    types::ThreadKey,
    workspace::diff_panel::{
        BranchCommitsView, DiffPanelSnapshot,
        comments::{CommentAnnotation, Side},
        files::DiffLineStat,
        logic::{Banner, DiffBody, DiffLabel, LoadingLabel},
    },
};
use t3_contracts::settings::{DiffLayout, TimestampFormat};

use super::{Action, PanelHost, PanelRegistration, SurfaceContext, SurfaceKind};
use crate::i18n::{Value, t, tr};
use crate::runtime::Shared;
use crate::runtime::tasks::TaskOwner;
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::menu::{self, ItemOptions, MenuOptions};
use crate::ui::skeleton::{Shape, skeleton};
use crate::ui::toggle::{self, GroupItem, GroupOptions};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// The client's diff panel service, bound in `runtime/services/diff.rs`.
pub trait DiffPanelBackend {
    fn watch(&self, key: ThreadKey) -> LocalBoxStream<'static, Rc<DiffPanelSnapshot>>;
    fn branch_commits(&self, key: &ThreadKey) -> LocalBoxStream<'static, BranchCommitsView>;
    fn select_scope(
        &self,
        key: &ThreadKey,
        value: &str,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn set_layout(&self, layout: DiffLayout) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn set_ignore_whitespace(&self, ignore: bool);
    fn set_hide_test_files(&self, hide: bool) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn set_file_tree_open(&self, open: bool) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn toggle_file(&self, snapshot: &DiffPanelSnapshot, file_key: &str);
    fn unfold_file(&self, snapshot: &DiffPanelSnapshot, file_key: &str);
    fn toggle_all_files(&self, snapshot: &DiffPanelSnapshot);
    fn refresh(&self, key: &ThreadKey);
    fn draft_comment(
        &self,
        snapshot: &DiffPanelSnapshot,
        side: Side,
        lines: (usize, usize),
    ) -> Option<CommentAnnotation>;
    fn add_review_comment(
        &self,
        key: &ThreadKey,
        snapshot: &DiffPanelSnapshot,
        side: Side,
        lines: (usize, usize),
        text: &str,
    ) -> Result<(), ClientError>;
    fn remove_review_comment(&self, key: &ThreadKey, id: &str) -> Result<(), ClientError>;
}

#[derive(Clone)]
pub struct DiffPanelRuntime(pub Rc<dyn DiffPanelBackend>);

/// The workspace's Diff tab (RightPanelSurfaceContent.tsx's `diff` surface).
pub fn diff_panel_registration(runtime: DiffPanelRuntime, host: PanelHost) -> PanelRegistration {
    PanelRegistration::new(
        SurfaceKind::Diff,
        Prop::new(None),
        move |key| host.apply(&key, Action::Open("diff".into())),
        move |context: SurfaceContext| {
            diff_panel(runtime.clone(), context.thread.clone(), context.visible).into_element()
        },
    )
}

/// Saved preferences report their own failures through the client's error state; a failed
/// write leaves the in-memory choice, as the web's `useLocalStorage` does.
fn run(owner: TaskOwner, future: LocalBoxFuture<'static, Result<(), ClientError>>) {
    owner.spawn(async move {
        if let Err(error) = future.await {
            log::warn!("Diff panel preference was not saved: {error}");
        }
    });
}

pub(crate) fn label_text(label: &DiffLabel) -> String {
    match label {
        DiffLabel::ToolCall => t("Tool call"),
        DiffLabel::LatestTurn => t("Latest turn"),
        DiffLabel::Turn(Some(count)) => tr("Turn {count}", &[("count", Value::Int(*count as i64))]),
        DiffLabel::Turn(None) => tr("Turn {count}", &[("count", Value::Text("?".into()))]),
        DiffLabel::Changes => t("Changes"),
        DiffLabel::Uncommitted => t("Uncommitted"),
        DiffLabel::Commit => t("Commit"),
        DiffLabel::CommitSubject(subject) => subject.clone(),
        DiffLabel::CommitSha(sha) => tr("Commit {sha}", &[("sha", Value::Text(sha.clone()))]),
    }
}

/// `formatShortTimestamp`: the time of day in the user's clock format.
fn short_timestamp(iso: &str, format: &TimestampFormat) -> String {
    use t3_client::view_logic::session::timestamps::resolve_hour12;
    let Some(epoch) = t3_client::time::epoch_ms(iso) else {
        return String::new();
    };
    let settings = locale_settings();
    let zone = match settings.time_zone.as_str() {
        "UTC" | "Etc/UTC" | "GMT" => t3_i18n::TimeZone::Utc,
        name => t3_i18n::TimeZone::Iana(name.into()),
    };
    crate::i18n::formats().date(
        t3_i18n::DateFormat::TimeHm,
        epoch,
        &zone,
        Some(resolve_hour12(format, settings.hour_cycle.twelve_hour())),
    )
}

/// `formatCompactDiffCount`.
pub fn format_compact_diff_count(value: u64) -> String {
    let scaled = |value: f64, unit: &str| {
        if value < 10.0 {
            let rounded = format!("{value:.1}");
            format!("{}{unit}", rounded.strip_suffix(".0").unwrap_or(&rounded))
        } else {
            format!("{}{unit}", value.round())
        }
    };
    match value {
        0..1_000 => value.to_string(),
        1_000..999_500 => scaled(value as f64 / 1e3, "k"),
        999_500..999_500_000 => scaled(value as f64 / 1e6, "m"),
        _ => scaled(value as f64 / 1e9, "b"),
    }
}

/// Port of chat/DiffStatLabel.tsx DiffStatLabel in its `inline` layout.
pub fn diff_stat_label(stat: DiffLineStat, class_name: &str) -> impl IntoElement {
    view()
        .accessible(true)
        .label(SharedString::from(tr(
            "{additions} additions, {deletions} deletions",
            &[
                ("additions", Value::Int(stat.additions as i64)),
                ("deletions", Value::Int(stat.deletions as i64)),
            ],
        )))
        .app_class(tw(&cn([
            Some("inline-flex flex-row items-center gap-1 tabular-nums"),
            Some(class_name),
        ])))
        .child(
            text(format!("+{}", format_compact_diff_count(stat.additions)))
                .accessible(false)
                .app_class(tw("font-mono text-diff-addition-foreground")),
        )
        .child(
            text(format!("-{}", format_compact_diff_count(stat.deletions)))
                .accessible(false)
                .app_class(tw("font-mono text-diff-deletion-foreground")),
        )
}

/// A control with its TooltipPopup, as the TSX wraps each header action.
fn with_tooltip(
    caption: impl Fn() -> String + 'static,
    control: impl IntoElement,
) -> impl IntoElement {
    view()
        .app_class(tw("inline-flex shrink-0"))
        .tooltip(tooltip(
            tooltip_create_handle(),
            TooltipOptions::default(),
            text(move || SharedString::from(caption())),
        ))
        .child(control)
}

fn git_diff_settled(snapshot: &DiffPanelSnapshot) -> bool {
    snapshot.can_refresh
        && !snapshot.refreshing
        && !matches!(
            snapshot.view.body,
            DiffBody::CheckingAccess | DiffBody::AccessDenied(_)
        )
}

/// Port of DiffPanel.tsx DiffPanel.
pub fn diff_panel(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    visible: Memo<bool>,
) -> impl IntoElement {
    let snapshot = signal(None::<Shared<DiffPanelSnapshot>>);
    let answered_at = stored(None);
    // A retained tab keeps the in-flight query when it loses visibility.
    let mut stream = runtime.0.watch(key.clone());
    crate::runtime::tasks::watch(async move {
        while let Some(next) = stream.next().await {
            snapshot.with_untracked(|previous| {
                let same_query = previous.as_ref().filter(|previous| {
                    previous.resolved.query == next.resolved.query
                        && previous.ignore_whitespace == next.ignore_whitespace
                });
                if same_query.is_none() {
                    answered_at.replace(None);
                } else if git_diff_settled(&next)
                    && same_query.is_some_and(|previous| previous.refreshing)
                {
                    answered_at.replace(Some(nsh_core::time::now()));
                }
            });
            if !snapshot.try_set(Some(Shared(next))) {
                return;
            }
        }
    });
    let service = runtime.0.clone();
    let watched = key.clone();
    let settled = computed(move || {
        snapshot.with(|value| value.as_ref().is_some_and(|value| git_diff_settled(value)))
    });
    effect(move || {
        // Keep the web's revalidation floor for an observed answer. A cached
        // result can arrive already settled, without a timestamp in the UI.
        if visible.get()
            && settled.get()
            && answered_at.with(|at| {
                at.is_none_or(|at| {
                    nsh_core::time::now().saturating_duration_since(at) >= Duration::from_secs(10)
                })
            })
        {
            untracked(|| service.refresh(&watched));
        }
    });
    let word_wrap_override = signal(None::<bool>);
    let word_wrap = computed(move || {
        word_wrap_override
            .get()
            .unwrap_or_else(|| snapshot.with(|s| s.as_ref().is_some_and(|s| s.word_wrap)))
    });
    diff_panel_shell(
        diff_panel_header(
            runtime.clone(),
            key.clone(),
            snapshot,
            word_wrap,
            word_wrap_override,
        ),
        diff_panel_body(runtime, key, snapshot, word_wrap),
    )
}

/// Port of DiffPanelShell.tsx DiffPanelShell, `embedded` mode (no Electron drag region).
fn diff_panel_shell(header: impl IntoElement, children: impl IntoElement) -> impl IntoElement {
    view()
        .app_class(tw("flex h-full min-w-0 flex-col bg-background w-full"))
        .child(
            view()
                .app_class(tw(
                    "flex flex-row items-center justify-between gap-2 px-2 h-10 min-h-10 shrink-0 border-b border-border/60 bg-background",
                ))
                .child(header),
        )
        .child(children)
}

fn diff_panel_header(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    word_wrap: Memo<bool>,
    word_wrap_override: Signal<Option<bool>>,
) -> impl IntoElement {
    let read = move |f: &dyn Fn(&DiffPanelSnapshot) -> bool| {
        snapshot.with(|s| s.as_ref().is_some_and(|s| f(s)))
    };
    // The toolbar's toggles are rebuilt by the snapshots their own changes publish.
    let tasks = TaskOwner::here();
    let test_files =
        computed(move || snapshot.with(|s| s.as_ref().map_or(0, |s| s.test_file_count)));
    let header_commit = computed(move || {
        snapshot.with(|s| s.as_ref().and_then(|s| s.resolved.header_commit.clone()))
    });
    let has_files = computed(move || read(&|s| !s.file_keys.is_empty()));
    let refresh_runtime = runtime.clone();
    let refresh_key = key.clone();
    let collapse_runtime = runtime.clone();
    let layout_runtime = runtime.clone();
    let whitespace_runtime = runtime.clone();
    let tests_runtime = runtime.clone();
    let tree_runtime = runtime.clone();
    let all_collapsed = computed(move || {
        snapshot.with(|s| {
            s.as_ref().is_some_and(|s| {
                let keys: Vec<String> = s.shown_files.iter().map(|(k, _)| k.clone()).collect();
                t3_client::workspace::diff_panel::logic::all_files_collapsed(&keys, &s.collapsed)
            })
        })
    });
    view()
        .app_class(tw("flex flex-row min-w-0 flex-1 items-center justify-between gap-2"))
        .child(
            // @container/diff-refs and [-webkit-app-region:no-drag] have no NSH meaning here.
            view()
                .app_class(tw("flex flex-row min-w-0 flex-1 items-center gap-3"))
                .child(diff_scope_menu(runtime.clone(), key.clone(), snapshot))
                .child(dynamic(move || {
                    header_commit.get().map(|sha| {
                        // projectCommitUrl needs the project's repository identity; the
                        // no-URL branch is shown until the client exposes it.
                        text(sha.chars().take(7).collect::<String>())
                            .app_class(tw("shrink-0 font-mono text-xs text-muted-foreground"))
                    })
                }))
                .child(dynamic(move || {
                    let comparison = snapshot.with(|s| s.as_ref().and_then(|s| s.comparison.clone()));
                    comparison.map(|(head, base)| comparison_refs(head, base))
                })),
        )
        .child(
            view()
                .app_class(move || {
                    Some(tw(&cn([
                        Some("flex flex-row shrink-0 items-center gap-1"),
                        header_commit.get().is_some().then_some("gap-0"),
                    ])))
                })
                .child(dynamic(move || {
                    snapshot.with(|s| {
                        s.as_ref()
                            .and_then(|s| s.view.stat)
                            .map(|stat| diff_stat_label(stat, "mr-1 text-2xs"))
                    })
                }))
                .child(dynamic(move || {
                    let can_refresh = read(&|s| s.can_refresh);
                    let runtime = refresh_runtime.clone();
                    let key = refresh_key.clone();
                    can_refresh.then(|| {
                        let refreshing = computed(move || read(&|s| s.refreshing));
                        with_tooltip(
                            move || {
                                if refreshing.get() {
                                    t("Refreshing diff…")
                                } else {
                                    t("Refresh diff")
                                }
                            },
                            // RefreshIcon's spin is a repainting animation; the label says it.
                            untracked(|| {
                                button(
                                    ButtonContent::Icon {
                                        icon: Icon::RefreshCw,
                                        label: if refreshing.get() {
                                            t("Refreshing diff")
                                        } else {
                                            t("Refresh diff")
                                        },
                                    },
                                    ButtonVariant::Ghost,
                                    ButtonSize::IconSm,
                                    || false,
                                    move || runtime.0.refresh(&key),
                                )
                            }),
                        )
                    })
                }))
                .child(dynamic(move || {
                    let runtime = collapse_runtime.clone();
                    has_files.get().then(|| {
                        let collapsed = all_collapsed.get();
                        with_tooltip(
                            move || {
                                if collapsed {
                                    t("Expand all files")
                                } else {
                                    t("Collapse all files")
                                }
                            },
                            button(
                                ButtonContent::Custom {
                                    label: if collapsed {
                                        t("Expand all files")
                                    } else {
                                        t("Collapse all files")
                                    },
                                    child: icon_with_class(
                                        if collapsed {
                                            Icon::ChevronsUpDown
                                        } else {
                                            Icon::ChevronsDownUp
                                        },
                                        "size-3.5",
                                    )
                                    .into_element(),
                                },
                                ButtonVariant::Ghost,
                                ButtonSize::IconSm,
                                || false,
                                move || {
                                    if let Some(s) = snapshot.get_untracked() {
                                        runtime.0.toggle_all_files(&s.0);
                                    }
                                },
                            ),
                        )
                    })
                }))
                .child(toggle::toggle_group(
                    t("Diff layout"),
                    vec![
                        GroupItem::new(
                            "stacked",
                            toggle::Content::Icon {
                                icon: Icon::Rows3,
                                label: t("Stacked diff view"),
                            },
                        )
                        .into(),
                        GroupItem::new(
                            "split",
                            toggle::Content::Icon {
                                icon: Icon::Columns2,
                                label: t("Split diff view"),
                            },
                        )
                        .into(),
                    ],
                    GroupOptions::default(),
                    move || {
                        vec![snapshot.with(|s| match s.as_ref().map(|s| &s.layout) {
                            Some(DiffLayout::Split) => "split".to_owned(),
                            _ => "stacked".to_owned(),
                        })]
                    },
                    || false,
                    move |value| match value.first().map(String::as_str) {
                        Some("stacked") => run(tasks, layout_runtime.0.set_layout(DiffLayout::Stacked)),
                        Some("split") => run(tasks, layout_runtime.0.set_layout(DiffLayout::Split)),
                        _ => {}
                    },
                ))
                .child(with_tooltip(
                    move || {
                        if word_wrap.get() {
                            t("Disable line wrapping")
                        } else {
                            t("Enable line wrapping")
                        }
                    },
                    toggle::toggle(
                        toggle::Content::Icon {
                            icon: Icon::TextWrap,
                            label: t("Enable diff line wrapping"),
                        },
                        toggle::Variant::Ghost,
                        toggle::Size::Sm,
                        move || word_wrap.get(),
                        || false,
                        move |pressed| word_wrap_override.set(Some(pressed)),
                    ),
                ))
                .child(with_tooltip(
                    move || {
                        if read(&|s| s.ignore_whitespace) {
                            t("Show whitespace changes")
                        } else {
                            t("Hide whitespace changes")
                        }
                    },
                    toggle::toggle(
                        toggle::Content::Icon {
                            icon: Icon::Pilcrow,
                            label: t("Hide whitespace changes"),
                        },
                        toggle::Variant::Ghost,
                        toggle::Size::Sm,
                        move || read(&|s| s.ignore_whitespace),
                        || false,
                        move |pressed| whitespace_runtime.0.set_ignore_whitespace(pressed),
                    ),
                ))
                .child(dynamic(move || {
                    let count = test_files.get();
                    let runtime = tests_runtime.clone();
                    (count > 0).then(|| {
                        with_tooltip(
                            move || {
                                if read(&|s| s.hide_test_files) {
                                    tr(
                                        "Show {count, plural, one {# hidden test file} other {# hidden test files}}",
                                        &[("count", Value::Int(count as i64))],
                                    )
                                } else {
                                    t("Hide test files")
                                }
                            },
                            toggle::toggle(
                                toggle::Content::Icon {
                                    icon: Icon::FlaskConicalOff,
                                    label: t("Hide test files"),
                                },
                                toggle::Variant::Ghost,
                                toggle::Size::Sm,
                                move || read(&|s| s.hide_test_files),
                                || false,
                                move |pressed| run(tasks, runtime.0.set_hide_test_files(pressed)),
                            ),
                        )
                    })
                }))
                .child(dynamic(move || {
                    let runtime = tree_runtime.clone();
                    has_files.get().then(|| {
                        with_tooltip(
                            move || {
                                if read(&|s| s.file_tree_open) {
                                    t("Hide file tree")
                                } else {
                                    t("Show file tree")
                                }
                            },
                            toggle::toggle(
                                toggle::Content::Icon {
                                    icon: Icon::FolderTree,
                                    label: t("Show file tree"),
                                },
                                toggle::Variant::Ghost,
                                toggle::Size::Sm,
                                move || read(&|s| s.file_tree_open),
                                || false,
                                move |pressed| run(tasks, runtime.0.set_file_tree_open(pressed)),
                            ),
                        )
                    })
                })),
        )
}

/// The comparison refs beside the scope menu. Changing the target (the TSX's Combobox over
/// `vcs.listRefs`) is not built yet; the target shows as text.
fn comparison_refs(head: String, base: Option<String>) -> impl IntoElement {
    let base = base.unwrap_or_else(|| t("Automatic"));
    view()
        .accessible(true)
        .label(SharedString::from(tr(
            "Comparing {head} against {base}",
            &[
                ("head", Value::Text(head.clone())),
                ("base", Value::Text(base.clone())),
            ],
        )))
        .app_class(tw(
            "flex flex-row min-w-0 max-w-full items-center gap-2 overflow-hidden text-xs text-muted-foreground",
        ))
        .child(
            view()
                .app_class(tw("flex flex-row min-w-12 items-center gap-2 overflow-hidden"))
                .child(text(head).app_class(tw("min-w-0 max-w-48 truncate")))
                .child(icon_with_class(Icon::ArrowRight, "size-3.5 shrink-0 opacity-70")),
        )
        .child(text(base).app_class(tw("min-w-0 max-w-48 truncate")))
}

fn diff_scope_menu(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
) -> impl IntoElement {
    let label = computed(move || {
        snapshot.with(|s| {
            s.as_ref()
                .map_or_else(|| t("Changes"), |s| label_text(&s.resolved.scope_label))
        })
    });
    let value = move || {
        snapshot.with(|s| {
            s.as_ref()
                .map_or_else(String::new, |s| s.resolved.scope_value.clone())
        })
    };
    let open = signal(false);
    let tasks = TaskOwner::here();
    let select = {
        let runtime = runtime.clone();
        let key = key.clone();
        Rc::new(move |value: String| {
            run(tasks, runtime.0.select_scope(&key, &value));
            open.set(false);
        })
    };
    let radio = |value: &'static str, label: String, select: Rc<dyn Fn(String)>| {
        menu::menu_radio_item(
            label.clone(),
            value.into(),
            ItemOptions::default(),
            view()
                .app_class(tw("flex flex-row items-center gap-2"))
                .child(menu::menu_radio_item_indicator())
                .child(text(label)),
            move || select(value.into()),
        )
    };
    let commits_open = signal(false);
    menu::menu(
        // ui/button has no trailing-icon content; the label and chevron are the trigger's own.
        menu::menu_trigger(
            tr("Diff scope: {label}", &[("label", Value::Text(label.get_untracked()))]),
            || false,
            view()
                .app_class(tw(
                    "flex flex-row h-7 sm:h-6 max-w-full min-w-0 items-center gap-1 rounded-md border border-transparent bg-secondary px-2 text-xs text-secondary-foreground",
                ))
                .child(text(move || SharedString::from(label.get())).app_class(tw("truncate")))
                .child(icon_with_class(Icon::ChevronDown, "size-3.5 shrink-0 opacity-70")),
        ),
        MenuOptions {
            open: Some(open),
            align: Align::Start,
            ..Default::default()
        },
        menu::menu_popup(
            view()
                .child(menu::menu_radio_group(
                    t("Diff scope"),
                    value,
                    view()
                        .child(radio("branch", t("Changes"), select.clone()))
                        .child(radio("unstaged", t("Uncommitted"), select.clone()))
                        .child(radio("latest", t("Latest turn"), select.clone())),
                    {
                        let select = select.clone();
                        move |value| select(value)
                    },
                ))
                .child(menu::menu_sub(
                    menu::menu_sub_trigger(t("Turn"), ItemOptions::default(), text(t("Turn"))),
                    || false,
                    menu::menu_sub_popup(turn_rows(snapshot, select.clone())),
                ))
                .child(menu::menu_sub(
                    view()
                        .on_press(move || commits_open.set(true))
                        .child(menu::menu_sub_trigger(
                            t("Commits"),
                            ItemOptions::default(),
                            text(t("Commits")),
                        )),
                    || false,
                    menu::menu_sub_popup(commit_rows(runtime, key, snapshot, commits_open, select)),
                )),
        ),
    )
}

fn turn_rows(
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    select: Rc<dyn Fn(String)>,
) -> impl IntoElement {
    let rows = computed(move || {
        snapshot.with(|s| {
            s.as_ref()
                .map(|s| (s.turns.clone(), s.timestamp_format.clone()))
        })
    });
    let value = move || {
        snapshot.with(|s| {
            s.as_ref()
                .and_then(|s| {
                    s.resolved
                        .selected_turn
                        .as_ref()
                        .map(|t| format!("turn:{}", t.run_id.0))
                })
                .unwrap_or_default()
        })
    };
    let on_change = select.clone();
    menu::menu_radio_group(
        t("Turn"),
        value,
        dynamic(move || {
            let Some((turns, format)) = rows.get() else {
                return Vec::new();
            };
            turns
                .iter()
                .map(|turn| {
                    let value = format!("turn:{}", turn.run_id.0);
                    let label = tr(
                        "Turn {count}",
                        &[("count", Value::Int(turn.turn_count as i64))],
                    );
                    let select = select.clone();
                    let chosen = value.clone();
                    menu::menu_radio_item(
                        label.clone(),
                        value,
                        ItemOptions::default(),
                        view()
                            .app_class(tw("flex flex-row items-center gap-2"))
                            .child(menu::menu_radio_item_indicator())
                            .child(text(label))
                            .child(
                                text(short_timestamp(&turn.completed_at, &format)).app_class(tw(
                                    "ml-auto text-xs tabular-nums text-muted-foreground",
                                )),
                            ),
                        move || select(chosen.clone()),
                    )
                    .into_element()
                })
                .collect()
        }),
        move |value| on_change(value),
    )
}

fn commit_rows(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    open: Signal<bool>,
    select: Rc<dyn Fn(String)>,
) -> impl IntoElement {
    // The open submenu keeps the list it opened with, so rows do not move under the pointer.
    let commits = signal(None::<BranchCommitsView>);
    effect(move || {
        if !open.get() {
            return;
        }
        let mut stream = untracked(|| runtime.0.branch_commits(&key));
        crate::runtime::tasks::watch(async move {
            while let Some(next) = stream.next().await {
                let settled =
                    next.commits.is_some() || next.error.is_some() || next.access.is_some();
                if !commits.try_set(Some(next)) || settled {
                    return;
                }
            }
        });
    });
    let value = move || {
        snapshot.with(|s| {
            s.as_ref()
                .and_then(|s| {
                    s.resolved
                        .commit_sha
                        .as_ref()
                        .map(|sha| format!("commit:{sha}"))
                })
                .unwrap_or_default()
        })
    };
    let on_change = select.clone();
    view()
        .app_class(tw("max-h-80 max-w-96"))
        .child(menu::menu_radio_group(
            t("Commits"),
            value,
            dynamic(move || {
                commits.with(|state| {
                    state.as_ref()
                        .and_then(|v| v.commits.clone())
                        .map(|list| {
                            list.commits
                                .iter()
                                .map(|commit| {
                                    let value = format!("commit:{}", commit.sha);
                                    let chosen = value.clone();
                                    let select = select.clone();
                                    let relative = t3_client::view_logic::session::timestamps::format_relative_elapsed(
                                        t3_client::view_logic::session::timestamps::relative_time_elapsed_ms(
                                            t3_client::time::epoch_ms(&commit.committed_at).unwrap_or(0),
                                            crate::state::clock::now_ms(),
                                        ),
                                    );
                                    let selected = snapshot.with_untracked(|s| {
                                        s.as_ref().and_then(|s| s.resolved.commit_sha.clone())
                                    }) == Some(commit.sha.clone());
                                    view()
                                        .app_class(tw(&cn([
                                            Some("flex flex-row items-center rounded-sm hover:bg-accent"),
                                            selected.then_some("bg-foreground/[0.08]"),
                                        ])))
                                        .child(
                                            view().app_class(tw("min-w-0 flex-1")).child(menu::menu_radio_item(
                                                commit.subject.clone(),
                                                value,
                                                ItemOptions::default(),
                                                view()
                                                    .app_class(tw("flex flex-row items-center gap-3"))
                                                    .child(text(commit.subject.clone()).app_class(tw("min-w-0 flex-1 truncate")))
                                                    .child(
                                                        text(relative_label(&relative))
                                                            .app_class(tw("shrink-0 text-xs tabular-nums text-muted-foreground")),
                                                    ),
                                                move || select(chosen.clone()),
                                            )),
                                        )
                                        .child(
                                            text(commit.sha.chars().take(7).collect::<String>())
                                                .app_class(tw("shrink-0 px-2 font-mono text-xs text-muted-foreground")),
                                        )
                                        .into_element()
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
            }),
            move |value| on_change(value),
        ))
        .child(dynamic(move || {
            let status = commits.with(|state| match state {
                None => Some(t("Loading commits...")),
                Some(state) => {
                    if let Some(error) = &state.error {
                        Some(error.clone())
                    } else if let Some(access) = &state.access {
                        Some(if access.pending {
                            t("Checking file access...")
                        } else {
                            access
                                .error
                                .clone()
                                .unwrap_or_else(|| t("This connection cannot read local diffs."))
                        })
                    } else {
                        match &state.commits {
                            None => Some(t("Loading commits...")),
                            Some(list) if list.commits.is_empty() => Some(t("No commits on this branch.")),
                            Some(list) if list.truncated => Some(tr(
                                "Showing latest {count} commits",
                                &[("count", Value::Int(list.commits.len() as i64))],
                            )),
                            Some(_) => None,
                        }
                    }
                }
            });
            status.map(|status| {
                menu::menu_item(
                    status.clone(),
                    ItemOptions {
                        disabled: true.into(),
                        ..Default::default()
                    },
                    text(status),
                    || {},
                )
            })
        }))
}

/// `formatRelativeTimeLabel`: "5m ago", "just now".
fn relative_label(parts: &t3_client::view_logic::session::timestamps::RelativeTimeParts) -> String {
    match &parts.suffix {
        Some(suffix) => format!("{} {suffix}", parts.value),
        None => parts.value.clone(),
    }
}

fn centered_message(message: String) -> impl IntoElement {
    view()
        .app_class(tw("flex flex-1 items-center justify-center px-5"))
        .child(text(message).app_class(tw("text-center text-xs text-muted-foreground/70")))
}

fn loading_label_text(label: LoadingLabel) -> String {
    t(match label {
        LoadingLabel::ToolCall => "Loading tool call diff...",
        LoadingLabel::Checkpoint => "Loading checkpoint diff...",
        LoadingLabel::Uncommitted => "Loading uncommitted changes...",
        LoadingLabel::Commit => "Loading commit...",
        LoadingLabel::Changes => "Loading changes...",
    })
}

/// Port of DiffPanelShell.tsx DiffFileHeaderSkeleton.
fn diff_file_header_skeleton(title: f32) -> impl IntoElement {
    view()
        .app_class(tw("flex flex-row h-8 items-center gap-2 px-2 pr-3"))
        .child(
            view()
                .app_class(tw("flex size-5 shrink-0 items-center justify-center"))
                .child(skeleton(Shape::Block, px(10.0), px(10.0))),
        )
        .child(skeleton(Shape::Block, px(20.0), px(20.0)))
        .child(skeleton(Shape::Pill, relative(title), px(12.0)))
        .child(
            view()
                .app_class(tw("ml-auto flex flex-row shrink-0 items-center gap-2"))
                .child(skeleton(Shape::Pill, px(20.0), px(12.0)))
                .child(skeleton(Shape::Pill, px(20.0), px(12.0))),
        )
}

fn diff_code_line_skeleton(width: f32) -> impl IntoElement {
    view()
        .app_class(tw("flex flex-row items-center gap-3"))
        .child(skeleton(Shape::Pill, px(20.0), px(10.0)))
        .child(skeleton(Shape::Pill, relative(width), px(10.0)))
}

/// Port of DiffPanelShell.tsx DiffPanelLoadingState: static, with its label announced.
pub fn diff_panel_loading_state(label: String) -> impl IntoElement {
    view()
        .accessible(true)
        .label(SharedString::from(label))
        .app_class(tw("min-h-0 flex-1 overflow-hidden bg-background"))
        .child(diff_file_header_skeleton(0.5))
        .child(
            view()
                .app_class(tw("flex flex-row h-6 items-center gap-2 px-2 pr-3"))
                .child(view().app_class(tw("h-px flex-1 bg-border/40")))
                .child(skeleton(Shape::Pill, px(96.0), px(10.0)))
                .child(view().app_class(tw("h-px flex-1 bg-border/40"))),
        )
        .child(
            view()
                .app_class(tw("flex flex-col gap-2 px-3 py-2"))
                .child(diff_code_line_skeleton(0.66))
                .child(diff_code_line_skeleton(0.8))
                .child(diff_code_line_skeleton(0.6)),
        )
        .child(diff_file_header_skeleton(0.4))
        .child(diff_file_header_skeleton(0.6))
}

fn banner_text(banner: Banner) -> String {
    match banner {
        Banner::CheckingUntracked => t("Showing tracked changes. Checking untracked files..."),
        Banner::UntrackedFailed => {
            t("Showing tracked changes. Untracked changes could not be loaded. Refresh to retry.")
        }
        Banner::Truncated { totals_complete } => {
            let mut message =
                t("This preview exceeds the size limit. Changes shown are incomplete.");
            if totals_complete {
                message.push_str(&t(" Totals include all changes."));
            }
            message
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BodyKind {
    Waiting,
    Message,
    CheckingAccess,
    Main,
}

fn diff_panel_body(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    word_wrap: Memo<bool>,
) -> impl IntoElement {
    let kind = computed(move || {
        snapshot.with(|s| match s.as_ref().map(|s| &s.view.body) {
            None => BodyKind::Waiting,
            Some(
                DiffBody::NoThread
                | DiffBody::NotGitRepo
                | DiffBody::NoTurns
                | DiffBody::AccessDenied(_),
            ) => BodyKind::Message,
            Some(DiffBody::CheckingAccess) => BodyKind::CheckingAccess,
            Some(_) => BodyKind::Main,
        })
    });
    view()
        .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col"))
        .child(dynamic(move || match kind.get() {
            BodyKind::Waiting => view().into_element(),
            BodyKind::CheckingAccess => {
                diff_panel_loading_state(t("Checking file access...")).into_element()
            }
            BodyKind::Message => {
                let message = snapshot.with_untracked(|s| match s.as_ref().map(|s| &s.view.body) {
                    Some(DiffBody::NoThread) => t("Select a thread to inspect turn diffs."),
                    Some(DiffBody::NotGitRepo) => t(
                        "Turn diffs are unavailable because this project is not a git repository.",
                    ),
                    Some(DiffBody::NoTurns) => t("No completed turns yet."),
                    Some(DiffBody::AccessDenied(error)) => error
                        .clone()
                        .unwrap_or_else(|| t("This connection cannot read local diffs.")),
                    _ => String::new(),
                });
                centered_message(message).into_element()
            }
            BodyKind::Main => untracked(|| {
                diff_panel_main(runtime.clone(), key.clone(), snapshot, word_wrap).into_element()
            }),
        }))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MainKind {
    Loading(LoadingLabel),
    Empty(bool),
    Raw,
    Files,
}

fn diff_panel_main(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    word_wrap: Memo<bool>,
) -> impl IntoElement {
    let kind = computed(move || {
        snapshot.with(|s| match s.as_ref().map(|s| &s.view.body) {
            Some(DiffBody::Loading(label)) => MainKind::Loading(*label),
            Some(DiffBody::Raw(_)) => MainKind::Raw,
            Some(DiffBody::Files(_)) => MainKind::Files,
            Some(DiffBody::Empty { no_net_changes }) => MainKind::Empty(*no_net_changes),
            _ => MainKind::Empty(false),
        })
    });
    let banners = computed(move || {
        snapshot.with(|s| {
            s.as_ref()
                .map(|s| s.view.banners.clone())
                .unwrap_or_default()
        })
    });
    let error = computed(move || snapshot.with(|s| s.as_ref().and_then(|s| s.view.error.clone())));
    view()
        .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background"))
        .child(dynamic(move || {
            banners
                .get()
                .into_iter()
                .map(|banner| {
                    text(banner_text(banner))
                        .app_class(tw(
                            "shrink-0 border-b border-border/70 bg-muted/40 px-3 py-1.5 text-2xs text-muted-foreground",
                        ))
                        .into_element()
                })
                .collect::<Vec<_>>()
        }))
        .child(dynamic(move || {
            error.get().map(|error| {
                view()
                    .app_class(tw("px-3"))
                    .child(text(error).app_class(tw("mb-2 text-2xs text-error/80")))
            })
        }))
        .child(dynamic(move || match kind.get() {
            MainKind::Loading(label) => diff_panel_loading_state(loading_label_text(label)).into_element(),
            MainKind::Empty(no_net_changes) => view()
                .app_class(tw("flex h-full items-center justify-center px-3 py-2"))
                .child(
                    text(if no_net_changes {
                        t("No net changes in this selection.")
                    } else {
                        t("No patch available for this selection.")
                    })
                    .app_class(tw("text-xs text-muted-foreground/70")),
                )
                .into_element(),
            MainKind::Raw => untracked(|| raw_patch(snapshot).into_element()),
            MainKind::Files => untracked(|| {
                diff_files(runtime.clone(), key.clone(), snapshot, word_wrap).into_element()
            }),
        }))
}

/// The raw-patch branch. Its `pre` keeps no scroll of its own (no nested scroll container):
/// the one `scroll()` holds the reason and the text.
fn raw_patch(snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>) -> impl IntoElement {
    let raw = computed(move || {
        snapshot.with(|s| match s.as_ref().map(|s| &s.view.body) {
            Some(DiffBody::Raw(text)) => SharedString::from(text.to_string()),
            _ => SharedString::default(),
        })
    });
    scroll().app_class(tw("min-h-0 flex-1 p-2")).child(
        view()
            .app_class(tw("flex flex-col gap-2"))
            .child(
                text(t("Unsupported diff format. Showing raw patch."))
                    .app_class(tw("text-2xs text-muted-foreground/75")),
            )
            .child(
                text(move || raw.get())
                    .select_text()
                    .app_class(tw(
                        "rounded-md border border-border/70 bg-background/70 p-3 font-mono text-2xs leading-relaxed text-muted-foreground/90",
                    )),
            ),
    )
}

/// The files branch: `AnnotatableCodeView` over the shown patch, and DIFF-2's file tree.
fn diff_files(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    word_wrap: Memo<bool>,
) -> impl IntoElement {
    // Rc<str> equality compares pointers first, so an unchanged patch costs no text compare.
    let patch = computed(move || snapshot.with(|s| s.as_ref().and_then(|s| s.shown_patch.clone())));
    let patch_text = computed(move || patch.get().map(|p| SharedString::from(p.to_string())));
    let layout = computed(move || {
        snapshot.with(|s| match s.as_ref().map(|s| &s.layout) {
            Some(DiffLayout::Split) => nsh_base::DiffLayout::Split,
            _ => nsh_base::DiffLayout::Unified,
        })
    });
    let shown = computed(move || snapshot.with(|s| s.as_ref().map(|s| s.shown_files.clone())));
    let collapsed = computed(move || snapshot.with(|s| s.as_ref().map(|s| s.collapsed.clone())));
    let diff = component_ref::<DiffView>();
    let laid_out = signal(0_u64);
    // The client owns which files are folded; the view's own fold state follows it. Runs once
    // per change of the fold set or the files, never per frame or scroll.
    effect(move || {
        laid_out.get();
        let (Some(shown), Some(collapsed)) = (shown.get(), collapsed.get()) else {
            return;
        };
        let Some(api) = diff.api::<DiffViewApi>() else {
            return;
        };
        for file in api.files() {
            let Some((key, path)) = shown.get(file.index as usize) else {
                continue;
            };
            if path.as_str() != file.path.as_ref() {
                continue;
            }
            let want = collapsed.contains(key);
            if want != file.collapsed {
                if want {
                    api.collapse_file(path.clone());
                } else {
                    api.expand_file(path.clone());
                }
            }
        }
    });
    // A file another view asked to reveal (a chat diff link): once the diff holds it, then not
    // again until a new request, so it does not undo what the user did since.
    let revealed = stored(None::<(String, String, u64)>);
    let reveal_runtime = runtime.clone();
    effect(move || {
        laid_out.get();
        let Some(s) = snapshot.get() else {
            return;
        };
        let Some(path) = s.resolved.file_path.clone() else {
            revealed.replace(None);
            return;
        };
        let request = (
            s.resolved.collapse_scope.clone(),
            path.clone(),
            s.resolved.reveal_request_id,
        );
        if revealed.get() == Some(request.clone()) {
            return;
        }
        if super::diff_tree::reveal_diff_file(&reveal_runtime, &s.0, diff, &path) {
            revealed.replace(Some(request));
        }
    });
    let tree_runtime = runtime.clone();
    let header_runtime = runtime.clone();
    let comments = super::review_comments::ReviewComments::new();
    let annotations = computed(move || {
        let draft = comments.draft.get();
        snapshot.with(|s| {
            super::review_comments::view_annotations(s.as_ref().map(|s| &*s.0), draft.as_ref())
        })
    });
    // A draft whose file left the shown patch is dropped (AnnotatableCodeView.tsx's reset).
    let scope =
        computed(move || snapshot.with(|s| s.as_ref().map(|s| s.resolved.collapse_scope.clone())));
    effect(move || {
        scope.get();
        untracked(|| comments.draft.set(None));
    });
    let annotation_runtime = runtime.clone();
    let add_runtime = runtime.clone();
    let annotation_key = key.clone();
    let mut classes = DiffViewClasses::default();
    // diffRendering.ts DIFF_SURFACE_THEME_UNSAFE_CSS: the code surface and the app's diff tints.
    classes.side = Some(tw("bg-(--code-background) text-(--code-foreground)"));
    classes.line_added = Some(tw("bg-diff-addition/30"));
    classes.line_deleted = Some(tw("bg-diff-deletion/30"));
    classes.line_number_added = Some(tw("bg-diff-addition/40"));
    classes.line_number_deleted = Some(tw("bg-diff-deletion/40"));
    view()
        .app_class(tw("flex flex-row min-h-0 flex-1 overflow-hidden"))
        .child(
            view().app_class(tw("min-h-0 min-w-0 flex-1")).child(
                nsh_base::diff_view()
                    .patch(move || patch_text.get().unwrap_or_default())
                    .layout(move || layout.get())
                    .inline(DiffInline::None)
                    .soft_wrap(move || word_wrap.get())
                    .classes(classes)
                    .annotations(move || annotations.get())
                    .render_annotation(move |annotation: &nsh_base::DiffAnnotation| {
                        super::review_comments::comment_annotation(
                            annotation_runtime.clone(),
                            annotation_key.clone(),
                            snapshot,
                            comments,
                            annotation.clone(),
                        )
                    })
                    .render_add_annotation(move |range: &nsh_base::DiffLineRange| {
                        super::review_comments::add_comment_control(
                            add_runtime.clone(),
                            snapshot,
                            comments,
                            *range,
                        )
                    })
                    .render_file_header(move |file: &DiffFile| {
                        diff_file_header(header_runtime.clone(), snapshot, diff, file.clone())
                    })
                    .on_stats_change(move |_: &nsh_base::DiffStats| laid_out.update(|n| *n += 1))
                    .component_ref(&diff)
                    .size_full(),
            ),
        )
        .child(dynamic(move || {
            let open = snapshot.with(|s| s.as_ref().is_some_and(|s| s.file_tree_open));
            open.then(|| {
                super::diff_tree::diff_file_tree_pane(tree_runtime.clone(), snapshot, diff)
            })
        }))
}

/// Pierre's file header with DiffPanel.tsx's prefix (DiffFileCollapseToggle), the path, its
/// suffix (DiffFilePathCopyButton) and its counts. Pressing the header folds the file, as the
/// TSX's header click does.
fn diff_file_header(
    runtime: DiffPanelRuntime,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    diff: ComponentRef<DiffView>,
    file: DiffFile,
) -> impl IntoElement {
    let path = file.path.to_string();
    let key = snapshot.with_untracked(|s| {
        s.as_ref().and_then(|s| {
            s.shown_files
                .get(file.index as usize)
                .filter(|(_, p)| p == &path)
                .map(|(k, _)| k.clone())
        })
    });
    let collapsed = file.collapsed;
    let toggle_key = key.clone();
    let toggle = move || {
        if let (Some(key), Some(s)) = (toggle_key.clone(), snapshot.get_untracked()) {
            runtime.0.toggle_file(&s.0, &key);
        } else if let Some(api) = diff.api::<DiffViewApi>() {
            api.toggle_file(file.path.to_string());
        }
    };
    let press = toggle.clone();
    let copy_path = path.clone();
    let copied = signal(false);
    view()
        .app_class(tw(
            "flex flex-row size-full items-center gap-2 border-b border-border/70 bg-(--code-background) px-2 pr-3 font-sans text-xs",
        ))
        .on_press(press)
        .child(with_tooltip(
            move || if collapsed { t("Expand diff") } else { t("Collapse diff") },
            button(
                ButtonContent::Custom {
                    label: if collapsed {
                        tr("Expand {path}", &[("path", Value::Text(path.clone()))])
                    } else {
                        tr("Collapse {path}", &[("path", Value::Text(path.clone()))])
                    },
                    child: icon_with_class(
                        if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                        "size-4 text-muted-foreground/80",
                    )
                    .into_element(),
                },
                ButtonVariant::Ghost,
                ButtonSize::IconXs,
                || false,
                toggle,
            ),
        ))
        .child(text(path.clone()).select_text().app_class(tw("min-w-0 truncate text-foreground")))
        .child(with_tooltip(
            move || if copied.get() { t("Copied") } else { t("Copy path") },
            button(
                ButtonContent::Custom {
                    label: t("Copy file path"),
                    child: view()
                        .child(dynamic(move || {
                            if copied.get() {
                                icon_with_class(Icon::Check, "size-3 text-success").into_element()
                            } else {
                                icon_with_class(Icon::Copy, "size-3").into_element()
                            }
                        }))
                        .into_element(),
                },
                ButtonVariant::GhostMuted,
                ButtonSize::IconXs,
                || false,
                move || {
                    set_clipboard(copy_path.clone());
                    copied.set(true);
                },
            ),
        ))
        .child(
            view()
                .app_class(tw("ml-auto flex flex-row shrink-0"))
                .child(diff_stat_label(
                    DiffLineStat {
                        additions: u64::from(file.added),
                        deletions: u64::from(file.deleted),
                    },
                    "",
                )),
        )
}

#[cfg(test)]
mod tests {
    use super::format_compact_diff_count;

    #[test]
    fn compact_counts_follow_the_source_thresholds() {
        for (value, label) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1k"),
            (1_260, "1.3k"),
            (12_345, "12k"),
            (999_499, "999k"),
            (999_500, "1m"),
            (2_500_000, "2.5m"),
            (999_500_000, "1b"),
        ] {
            assert_eq!(format_compact_diff_count(value), label, "{value}");
        }
    }
}
