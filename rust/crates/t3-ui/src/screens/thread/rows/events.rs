//! Port of apps/web/src/components/chat/TimelineV2EventRow.tsx V2EventTimelineRow and V2SubagentGroup.

use std::collections::BTreeMap;
use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::services::providers::provider_instance_display_name;
use t3_client::state::sharing::SharedRows;
use t3_client::store::Scope;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::thread_views::threads::subagent_display::{
    format_subagent_display_title, subagent_group_summary, summarize_subagent_statuses,
};
use t3_client::thread_views::timeline_events::{
    self as presentation, AgentTiming, HandoffEndpoint, HandoffEndpoints, active, resolved_id,
};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::OrchestrationV2RemoteTaskChild;
use t3_contracts::orchestration_v2::OrchestrationV2Run;
use t3_contracts::orchestration_v2::{
    AppThreadForkedFrom, OrchestrationV2ModelSwitch, OrchestrationV2ProjectedTurnItem,
    OrchestrationV2Subagent, OrchestrationV2TurnItem as Item,
    OrchestrationV2TurnItemStatus as Status, TurnItemSubagent,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_contracts::server::ServerConfig;
use t3_contracts::settings::TimestampFormat;

use super::work::WorkRowState;
use crate::i18n::{Text, Value, t, tr};
use crate::runtime::{Rt, Shared, rt};
use crate::state::nav::{Navigator, Route, navigator};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::collapsible::{
    CollapsibleHandle, collapsible, collapsible_panel, collapsible_trigger,
};
use crate::ui::icon::{Icon, icon_with_class, provider_icon};
use crate::ui::middle_truncate::{
    Font as TruncateFont, Options as TruncateOptions, middle_truncate_with_font,
};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

pub type TimelineRow = Memo<Option<Shared<MessagesTimelineRow>>>;

#[derive(Clone)]
struct EventRows<T>(SharedRows<T>);

impl<T> PartialEq for EventRows<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug)]
pub struct EventRow {
    pub id: String,
    pub projected_item: Rc<OrchestrationV2ProjectedTurnItem>,
    pub subagents: Option<Vec<Rc<OrchestrationV2ProjectedTurnItem>>>,
    pub resource_summary: bool,
    pub continues_work_log: bool,
}

impl EventRow {
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let MessagesTimelineRowContent::Event {
            projected_item,
            subagents,
            resource_summary,
        } = &row.content
        else {
            return None;
        };
        Some(Self {
            id: row.id.clone(),
            projected_item: projected_item.clone(),
            subagents: subagents.clone(),
            resource_summary: resource_summary.unwrap_or(false),
            continues_work_log: row.continues_work_log.unwrap_or(false),
        })
    }
}

/// Construct once above the virtual list; the context owns navigation and live projection reads.
#[derive(Clone)]
pub struct EventRowContext {
    key: ThreadKey,
    runtime: Rt,
    nav: Navigator,
    agents: Memo<EventRows<OrchestrationV2Subagent>>,
    runs: Memo<EventRows<OrchestrationV2Run>>,
    configs: Signal<BTreeMap<EnvironmentId, Shared<ServerConfig>>>,
    pub state: WorkRowState,
    pub now_ms: Memo<Option<i64>>,
    pub timestamp_format: Memo<TimestampFormat>,
    pub on_toggle_entry: Option<Rc<dyn Fn(String, bool)>>,
}

impl EventRowContext {
    pub fn new(key: ThreadKey, state: WorkRowState) -> Self {
        let runtime = rt();
        let thread = runtime.store.thread(key.clone());
        let agents = computed(move || {
            EventRows(thread.with(|thread| {
                thread
                    .as_ref()
                    .map(|t| t.subagents.clone())
                    .unwrap_or_else(|| Rc::from([]))
            }))
        });
        let runs = computed(move || {
            EventRows(thread.with(|thread| {
                thread
                    .as_ref()
                    .map(|t| t.runs.clone())
                    .unwrap_or_else(|| Rc::from([]))
            }))
        });
        Self {
            agents,
            runs,
            configs: config_sources(runtime),
            key,
            runtime,
            nav: navigator(),
            state,
            // A live duration needs the destination environment's adjusted clock, never device time.
            now_ms: computed(|| None),
            timestamp_format: computed(|| TimestampFormat::Locale),
            on_toggle_entry: None,
        }
    }

    fn open(&self, target: &ThreadKey) {
        self.nav.open(Route::Thread(target.clone()));
    }

    fn config(&self, environment: &EnvironmentId) -> Option<Shared<ServerConfig>> {
        self.configs
            .with(|configs| configs.get(environment).cloned())
    }

    fn live_agent(&self, item: &TurnItemSubagent) -> Option<Shared<OrchestrationV2Subagent>> {
        self.agents.with(|agents| {
            agents
                .0
                .iter()
                .find(|a| a.id == item.subagent_id)
                .cloned()
                .map(Shared)
        })
    }
}

fn config_sources(runtime: Rt) -> Signal<BTreeMap<EnvironmentId, Shared<ServerConfig>>> {
    let store = runtime.store.store();
    let mut changes = store.subscribe();
    let read = move || {
        store
            .snapshot()
            .environments
            .iter()
            .filter_map(|(id, env)| {
                env.config
                    .clone()
                    .map(|config| (id.clone(), Shared(config)))
            })
            .collect()
    };
    let configs = signal(read());
    // Config has no StoreSignals selector yet. Snapshot reads stay in this subscribed task, not bindings.
    spawn(async move {
        while let Some(change) = changes.next().await {
            if change
                .scopes
                .iter()
                .any(|s| matches!(s, Scope::All | Scope::Environment(_)))
            {
                configs.set(read());
            }
        }
    });
    configs
}

pub fn v2_event_timeline_row(row: TimelineRow, context: EventRowContext) -> impl IntoElement {
    dynamic(move || {
        let row = row.with(|row| row.as_ref().and_then(|row| EventRow::from_row(row)))?;
        Some(
            if matches!(row.projected_item.item, Item::Subagent(_))
                && row.subagents.as_ref().map_or(1, Vec::len) > 1
            {
                v2_subagent_group(&row, &context)
            } else {
                // Secret/link request cards require the request service (TL-APPROVAL/C1); no fabricated copy.
                v2_lifecycle_row(&row.projected_item.item, row.resource_summary, &context)
            },
        )
    })
}

/// Port of V2LifecycleRow.tsx V2LifecycleRow.
pub fn v2_lifecycle_row(
    item: &Item,
    resource_summary: bool,
    context: &EventRowContext,
) -> AnyElement {
    match item {
        Item::RunInterruptResult(item) => timeline_system_divider(
            Text::from("Run interrupted"),
            Some(Text::content(item.message.clone())),
            Some(Icon::CircleStop),
            false,
            true,
            None,
        ),
        Item::Handoff(item) => context.runs.with(|runs| {
            presentation::resolve_handoff_endpoints(item, &runs.0)
                .map(|endpoints| {
                    environment_handoff_row(endpoints, item.status == Status::Failed, context)
                })
                .unwrap_or_else(|| view().into_element())
        }),
        Item::Fork(item) => {
            let (label, action, target) = match &item.source {
                AppThreadForkedFrom::Run(source) => (
                    "Forked from conversation",
                    "Open source conversation",
                    Some(source.thread_id.0.as_str()),
                ),
                _ => (
                    "Conversation fork",
                    "Open fork",
                    resolved_id(&item.target_thread_id),
                ),
            };
            let action = target.map(|target| {
                let key = ThreadKey {
                    environment: context.key.environment.clone(),
                    thread: ThreadId(target.into()),
                };
                let context = context.clone();
                (
                    Text::client_key(action.to_owned()),
                    Rc::new(move || context.open(&key)) as Rc<dyn Fn()>,
                )
            });
            timeline_system_divider(
                Text::client_key(label.to_owned()),
                None,
                Some(Icon::GitFork),
                false,
                false,
                action,
            )
        }
        Item::ThreadCreated(item) => {
            let target = resolved_id(&item.target_thread_id).map(|id| ThreadKey {
                environment: context.key.environment.clone(),
                thread: ThreadId(id.into()),
            });
            thread_created_row(item.title.as_deref(), resource_summary, target, context)
        }
        Item::Subagent(item) => subagent_timeline_link(item, context),
        _ => view().into_element(),
    }
}

type DividerAction = (Text, Rc<dyn Fn()>);

/// Port of TimelineSystemDivider.tsx TimelineSystemDivider.
pub fn timeline_system_divider(
    label: Text,
    detail: Option<Text>,
    glyph: Option<Icon>,
    danger: bool,
    wrap_detail: bool,
    action: Option<DividerAction>,
) -> AnyElement {
    timeline_system_divider_content(
        label,
        detail.map(|detail| {
            text(move || detail.resolve())
                .app_class(tw(if wrap_detail {
                    "min-w-0"
                } else {
                    "max-w-80 truncate"
                }))
                .into_element()
        }),
        glyph,
        danger,
        wrap_detail,
        true,
        action,
    )
}

fn timeline_system_divider_content(
    label: Text,
    detail: Option<AnyElement>,
    glyph: Option<Icon>,
    danger: bool,
    wrap_detail: bool,
    detail_separator: bool,
    action: Option<DividerAction>,
) -> AnyElement {
    // NSH's fragment host uses flex wrapping for inline detail; text preserves newlines without whitespace classes.
    let content = view()
        .app_class(tw(if wrap_detail {
            "min-w-0 flex flex-wrap items-baseline justify-center"
        } else {
            "flex flex-wrap items-center justify-center gap-1.5"
        }))
        .child(
            view()
                .app_class(tw(&cn([
                    Some("inline-flex items-baseline gap-1.5 align-baseline font-medium"),
                    wrap_detail.then_some("shrink-0"),
                ])))
                .child(glyph.map(|glyph| icon_with_class(glyph, "size-3 shrink-0 self-center")))
                .child(text(move || label.resolve())),
        )
        .child(detail.map(|detail| {
            view()
                .app_class(tw(&cn([
                    Some("min-w-0 opacity-70"),
                    Some(if wrap_detail {
                        "ml-1.5 inline-flex items-baseline"
                    } else {
                        "inline-flex items-center"
                    }),
                ])))
                .child(detail_separator.then(|| text(SharedString::from("·\u{a0}"))))
                .child(detail)
        }));
    let center = if let Some((action_label, press)) = action {
        let hint = action_label.clone();
        view().app_class(tw("min-w-0")).tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || hint.resolve())))
            .child(nsh_base::button().app_class(tw(&cn([
            Some("min-w-0 cursor-pointer rounded-full border border-border/70 bg-background px-2.5 py-1 transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"),
            Some(if wrap_detail { "text-center" } else { "flex items-center gap-1.5" }),
        ]))).accessibility_label(move || Some(action_label.resolve())).on_press(move || press()).child(content))
            .into_element()
    } else {
        view()
            .app_class(tw(&cn([
                Some("min-w-0 rounded-full px-2 py-1"),
                Some(if wrap_detail {
                    "text-center"
                } else {
                    "flex flex-wrap items-center justify-center gap-1.5"
                }),
            ])))
            .child(content)
            .into_element()
    };
    view()
        .app_class(tw(&cn([
            Some("flex min-w-0 items-center gap-2 py-2 text-2xs text-muted-foreground"),
            danger.then_some("text-destructive"),
        ])))
        .child(
            view()
                .accessible(false)
                .app_class(tw("h-px flex-1 bg-border/70")),
        )
        .child(center)
        .child(
            view()
                .accessible(false)
                .app_class(tw("h-px flex-1 bg-border/70")),
        )
        .into_element()
}

/// Port of V2LifecycleRow.tsx FreshSessionRow.
pub fn fresh_session_row() -> impl IntoElement {
    timeline_system_divider(
        Text::from("New session"),
        None,
        Some(Icon::Eraser),
        false,
        false,
        None,
    )
}

/// Port of V2LifecycleRow.tsx ModelSwitchRow.
pub fn model_switch_row(
    change: &OrchestrationV2ModelSwitch,
    context: &EventRowContext,
) -> AnyElement {
    let config = context.config(&context.key.environment);
    let providers = config
        .as_ref()
        .map_or(&[][..], |c| c.providers.0.as_slice());
    let Some(visible) = presentation::resolve_model_switch_change(change, providers) else {
        return view().into_element();
    };
    let describe = |selection: &t3_contracts::model_selection::ModelSelection,
                    effort: Option<&str>| {
        let model = providers
            .iter()
            .find(|p| p.instance_id == selection.instance_id)
            .and_then(|p| p.models.iter().find(|m| m.slug.0 == selection.model));
        let driver = providers
            .iter()
            .find(|p| p.instance_id == selection.instance_id)
            .map(|p| p.driver.0.as_str());
        let name = model.map_or_else(
            || selection.model.clone(),
            |m| presentation::display_model_name(m, driver),
        );
        [visible.model_changed.then_some(name.as_str()), effort]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let label = format!(
        "{} → {}",
        describe(
            &change.r#from,
            visible.effort.as_ref().map(|e| e.0.as_str())
        ),
        describe(&change.to, visible.effort.as_ref().map(|e| e.1.as_str()))
    );
    timeline_system_divider(Text::content(label), None, None, false, false, None)
}

/// Port of V2LifecycleRow.tsx EnvironmentHandoffRow.
fn environment_handoff_row(
    endpoints: HandoffEndpoints,
    failed: bool,
    context: &EventRowContext,
) -> AnyElement {
    handoff_row(endpoints, false, failed, context)
}

/// Port of V2LifecycleRow.tsx HandoffRow.
pub fn handoff_row(
    endpoints: HandoffEndpoints,
    pending: bool,
    failed: bool,
    context: &EventRowContext,
) -> AnyElement {
    let mut detail = view().app_class(tw(
        "inline-flex min-w-0 flex-wrap items-center justify-center gap-1.5",
    ));
    let len = endpoints.from.len();
    for (index, endpoint) in endpoints.from.into_iter().enumerate() {
        detail = detail.child(
            view()
                .app_class(tw("inline-flex min-w-0 items-center"))
                .child(handoff_endpoint(endpoint, context))
                .child((index + 1 < len).then(|| {
                    text(SharedString::from(","))
                        .accessible(false)
                        .app_class(tw("ml-0.5"))
                })),
        );
    }
    detail = detail
        .child((len > 0).then(|| icon_with_class(Icon::ArrowRight, "size-3 shrink-0")))
        .child(handoff_endpoint(endpoints.to, context));
    timeline_system_divider_content(
        Text::from(if pending {
            "Context handoff pending"
        } else {
            "Context handoff"
        }),
        Some(detail.into_element()),
        Some(Icon::ArrowRightLeft),
        failed,
        false,
        false,
        None,
    )
}

/// Port of V2LifecycleRow.tsx HandoffEndpoint.
fn handoff_endpoint(endpoint: HandoffEndpoint, context: &EventRowContext) -> AnyElement {
    let config = context.config(&context.key.environment);
    let provider = config.as_ref().and_then(|c| {
        c.providers
            .0
            .iter()
            .find(|p| p.instance_id == endpoint.instance_id)
    });
    let model = endpoint
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let known = model.and_then(|slug| provider?.models.iter().find(|m| m.slug.0 == slug));
    let visual = config
        .as_ref()
        .and_then(|c| presentation::provider_visual(c, &endpoint.instance_id.0));
    let provider_name = visual
        .as_ref()
        .map(|v| v.display_name.clone())
        .or_else(|| provider.map(provider_instance_display_name))
        .unwrap_or_else(|| endpoint.instance_id.0.clone());
    let label = known
        .map(|m| presentation::display_model_name(m, provider.map(|p| p.driver.0.as_str())))
        .or_else(|| model.map(str::to_owned))
        .unwrap_or_else(|| provider_name.clone());
    let driver = provider.map_or(endpoint.instance_id.0.as_str(), |p| p.driver.0.as_str());
    let hint = format!("{provider_name} · {label}");
    view().app_class(tw("inline-flex min-w-0 items-center gap-1.5 rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
        .focusable(true)
        .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(SharedString::from(hint))))
        .child(visual.as_ref().map_or_else(|| provider_icon(driver, &provider_name, "size-3.5").into_element(), |visual| provider_instance_icon(visual, true, true)))
        .child(text(SharedString::from(label)).app_class(tw(&cn([Some("truncate font-medium"), Some(provider_text_class(driver))]))))
        .into_element()
}

fn provider_text_class(driver: &str) -> &'static str {
    match driver {
        "codex" | "openai" | "anthropic" | "openrouter" | "ollama" => "text-black dark:text-white",
        "claudeAgent" => "text-[#d97757]",
        "antigravity" => "text-[#5b87bf]",
        _ => "",
    }
}

/// Port of chat/ProviderInstanceIcon.tsx ProviderInstanceIcon, compact size.
fn provider_instance_icon(
    visual: &presentation::ProviderVisual,
    dim: bool,
    background: bool,
) -> AnyElement {
    // The compact 14px endpoint is below the source's 1.1rem container breakpoint; isolate has no NSH meaning.
    if visual.driver == "openaiCompatible" {
        return view().accessible(false).app_class(tw("relative flex size-3.5 shrink-0 items-center justify-center rounded-md border bg-card text-4xs font-semibold leading-none text-foreground"))
            .child(text(SharedString::from(visual.initials.chars().take(1).collect::<String>()))).into_element();
    }
    let mut root = view()
        .accessible(false)
        .app_class(tw(&cn([
            Some("relative z-30 inline-flex shrink-0 items-center justify-center overflow-visible"),
            (!background).then_some("z-auto"),
        ])))
        .child(provider_icon(
            &visual.driver,
            &visual.display_name,
            &cn([
                Some("size-3.5"),
                (dim && visual.show_badge).then_some("opacity-60"),
            ]),
        ));
    if visual.show_badge {
        let mut badge = view().app_class(tw("pointer-events-none absolute z-10 flex items-center justify-center rounded-full border font-semibold leading-none shadow-sm right-[-0.1875rem] bottom-[-0.1875rem] h-3 min-w-3 px-0.5 text-5xs bg-card text-muted-foreground"))
            .border_color(move || if background { crate::theme::palette().background } else { crate::theme::palette().card });
        if let Some(accent) = visual
            .accent
            .as_ref()
            .and_then(|accent| u32::from_str_radix(&accent[1..], 16).ok())
        {
            let color = AdaptiveColor::rgb(accent, accent);
            let dark = visual.accent.as_ref().is_some_and(|accent| {
                t3_client::services::providers::provider_accent_foreground(accent) == "#000000"
            });
            badge = badge.bg(color).text_color(AdaptiveColor::rgb(
                if dark { 0 } else { 0xffffff },
                if dark { 0 } else { 0xffffff },
            ));
        }
        root = root.child(badge.child(text(SharedString::from(visual.initials.clone()))));
    }
    root.into_element()
}

fn thread_created_row(
    title: Option<&str>,
    resource: bool,
    target: Option<ThreadKey>,
    context: &EventRowContext,
) -> AnyElement {
    let open_label = title
        .map(|title| tr("Open {title}", &[("title", Value::Text(title.into()))]))
        .unwrap_or_else(|| t("Open created thread"));
    let title = title
        .filter(|title| resource || !title.is_empty())
        .map(|title| Text::content(title.to_owned()))
        .unwrap_or_else(|| Text::from("Created thread"));
    let disabled = target.is_none();
    let context = context.clone();
    let action = button(
        ButtonContent::Label(t("Open chat")),
        if resource {
            ButtonVariant::Outline
        } else {
            ButtonVariant::Link
        },
        ButtonSize::Xs,
        move || disabled,
        move || {
            if let Some(target) = &target {
                context.open(target);
            }
        },
    );
    // Button's current API shares visible/accessibility copy; this wrapper carries the specific target label.
    let action = view().label(SharedString::from(open_label)).child(action);
    if resource {
        view()
            .app_class(tw(
                "flex min-w-0 items-center gap-3 rounded-lg border border-border/60 p-3",
            ))
            .child(
                view()
                    .app_class(tw(
                        "flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted/60",
                    ))
                    .child(icon_with_class(
                        Icon::MessageSquare,
                        "size-4 text-secondary-label",
                    )),
            )
            .child(
                text(move || title.resolve())
                    .app_class(tw("min-w-0 flex-1 truncate text-sm font-medium")),
            )
            .child(action)
            .into_element()
    } else {
        // The T3 wordmark is not in the shared icon catalogue; UI-ICONS owns that exact asset.
        // WorkLogRow/WorkLogLine; child selectors are applied on their text and NSH has no cap baseline unit.
        view().app_class(tw("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 text-left transition-colors py-0.5"))
            .child(view().app_class(tw("flex min-h-6 min-w-0 gap-1.5 text-sm leading-relaxed select-none items-center"))
            .child(view().app_class(tw("flex min-w-0 flex-1 items-baseline gap-1.5"))
            .child(view().app_class(tw("relative w-6 shrink-0 leading-0")).child(view().app_class(tw("inline-block size-6 overflow-hidden"))))
            .child(view().app_class(tw("min-w-0 flex-1 text-secondary-label truncate"))
                .child(text(move || {
                    let label = title.resolve();
                    if matches!(title, Text::Key(_)) { label } else { SharedString::from(format!("{} · {label}", t("Created thread"))) }
                })).selectable(false)))
            .child(action)).into_element()
    }
}

fn timing(item: &TurnItemSubagent, live: Option<&OrchestrationV2Subagent>) -> AgentTiming {
    AgentTiming {
        status: live.map_or_else(|| item.status.clone(), |a| a.status.clone()),
        started_at: live
            .and_then(|a| a.started_at.clone())
            .or_else(|| item.started_at.clone()),
        completed_at: live
            .and_then(|a| a.completed_at.clone())
            .or_else(|| item.completed_at.clone()),
    }
}

/// Port of V2LifecycleRow.tsx SubagentElapsed.
fn subagent_elapsed(agent: AgentTiming, context: &EventRowContext) -> impl IntoElement {
    let now = context.now_ms;
    text(move || {
        SharedString::from(if active(&agent.status) {
            now.get()
                .and_then(|now| presentation::subagent_elapsed(&agent, now))
                .unwrap_or_default()
        } else {
            presentation::subagent_elapsed(&agent, 0).unwrap_or_default()
        })
    })
    .app_class(tw("tabular-nums"))
}

fn status_class(status: &Status) -> &'static str {
    match status {
        Status::Pending | Status::Running | Status::Waiting => "bg-info",
        Status::Idle => "bg-muted-foreground/50",
        Status::Completed => "bg-success",
        Status::Failed => "bg-destructive",
        Status::Cancelled | Status::Interrupted => "bg-muted-foreground/60",
    }
}

struct AgentLink<'a> {
    provider_instance_id: &'a IdOrIndex<ProviderInstanceId>,
    driver: &'a str,
    remote: Option<&'a OrchestrationV2RemoteTaskChild>,
    child_thread_id: Option<&'a str>,
    title: Option<&'a str>,
    result: Option<&'a str>,
    progress: Option<&'a str>,
    timing: AgentTiming,
}

fn agent_link<'a>(
    item: &'a TurnItemSubagent,
    live: Option<&'a OrchestrationV2Subagent>,
) -> AgentLink<'a> {
    AgentLink {
        provider_instance_id: &item.provider_instance_id,
        driver: &item.driver.0,
        remote: item
            .remote_child
            .as_ref()
            .and_then(Option::as_ref)
            .or_else(|| live.and_then(|a| a.remote_child.as_ref().and_then(Option::as_ref))),
        child_thread_id: item.child_thread_id.as_ref().and_then(resolved_id),
        title: item.title.as_deref(),
        result: live
            .and_then(|a| a.result.as_deref())
            .or(item.result.as_deref()),
        progress: live
            .and_then(|a| a.progress.as_ref().and_then(Option::as_deref))
            .or_else(|| item.progress.as_ref().and_then(Option::as_deref)),
        timing: timing(item, live),
    }
}

/// Port of V2LifecycleRow.tsx SubagentNotificationLink. None selects WorkLogRow's existing fallback.
pub fn subagent_notification_link(
    row: &super::work::WorkEntryRow,
    context: &EventRowContext,
) -> Option<AnyElement> {
    let projected = row.entry.projected_item.as_ref()?;
    let Item::Notification(notice) = &projected.item else {
        return None;
    };
    let notification = context.agents.with(|agents| {
        presentation::subagent_notification(notice, &row.entry.created_at, &agents.0)
    })?;
    let agent = notification.agent;
    let data = AgentLink {
        provider_instance_id: &agent.provider_instance_id,
        driver: &agent.driver.0,
        remote: agent.remote_child.as_ref().and_then(Option::as_ref),
        child_thread_id: Some(notification.child_thread_id.0.as_str()),
        title: agent.title.as_deref(),
        result: agent.result.as_deref(),
        progress: agent.progress.as_ref().and_then(Option::as_deref),
        timing: AgentTiming {
            status: agent.status.clone(),
            started_at: agent.started_at.clone(),
            completed_at: agent.completed_at.clone(),
        },
    };
    let event = notification.event.map(|event| {
        (
            event,
            short_timestamp(&row.entry.created_at, &context.timestamp_format.get()),
        )
    });
    Some(subagent_link_content(data, context, event))
}

fn short_timestamp(iso: &str, format: &TimestampFormat) -> String {
    let settings = locale_settings();
    let zone = if matches!(settings.time_zone.as_str(), "UTC" | "Etc/UTC" | "GMT") {
        t3_i18n::TimeZone::Utc
    } else {
        t3_i18n::TimeZone::Iana(settings.time_zone.to_string())
    };
    let Some(at) = t3_client::time::epoch_ms(iso).filter(|_| zone.is_known()) else {
        return String::new();
    };
    crate::i18n::formats().date(
        t3_i18n::DateFormat::TimeHm,
        at,
        &zone,
        Some(t3_client::view_logic::session::timestamps::resolve_hour12(
            format,
            settings.hour_cycle.twelve_hour(),
        )),
    )
}

/// Port of V2LifecycleRow.tsx EnvironmentSubagentAvatar and SubagentAvatar.
fn environment_subagent_avatar(
    item: &AgentLink<'_>,
    status: Option<&Status>,
    context: &EventRowContext,
) -> AnyElement {
    let remote = item.remote;
    let environment = remote
        .map(|r| EnvironmentId(r.environment_id.0.clone()))
        .unwrap_or_else(|| context.key.environment.clone());
    let config = context.config(&environment);
    let visual = config
        .as_ref()
        .and_then(|c| presentation::provider_visual(c, resolved_id(item.provider_instance_id)?));
    let show_badge = visual.as_ref().is_some_and(|v| v.show_badge);
    let provider = config.as_ref().and_then(|c| {
        c.providers
            .0
            .iter()
            .find(|p| resolved_id(item.provider_instance_id) == Some(p.instance_id.0.as_str()))
    });
    let driver = provider
        .map(|p| p.driver.0.as_str())
        .or_else(|| remote.is_none().then_some(item.driver));
    let name = provider
        .map(provider_instance_display_name)
        .unwrap_or_else(|| item.driver.to_owned());
    view().accessible(false).app_class(tw("relative inline-flex size-6 shrink-0 items-center justify-center rounded-full border border-border/70 bg-muted ring-2 ring-background"))
        .child(visual.as_ref().map_or_else(|| driver.map_or_else(|| icon_with_class(Icon::Bot, "size-3.5 text-muted-foreground").into_element(), |driver| provider_icon(driver, &name, "size-3.5").into_element()), |visual| provider_instance_icon(visual, false, false)))
        .child(status.map(|status| view().app_class(tw(&cn([
            Some("absolute size-2 rounded-full ring-2 ring-background"), Some(if show_badge { "-left-px -top-px" } else { "-right-px -bottom-px" }), Some(status_class(status)),
        ])))))
        .into_element()
}

/// Port of V2LifecycleRow.tsx SubagentTimelineLink.
fn subagent_timeline_link(item: &TurnItemSubagent, context: &EventRowContext) -> AnyElement {
    let live = context.live_agent(item);
    let live = live.as_ref().map(|a| a.0.as_ref());
    subagent_link_content(agent_link(item, live), context, None)
}

fn subagent_link_content(
    item: AgentLink<'_>,
    context: &EventRowContext,
    event: Option<(presentation::NotificationEvent, String)>,
) -> AnyElement {
    let agent_timing = item.timing.clone();
    let status = event
        .as_ref()
        .and_then(|e| e.0.status.as_ref())
        .unwrap_or(&agent_timing.status);
    let title = item
        .title
        .map(format_subagent_display_title)
        .map(Text::content)
        .unwrap_or_else(|| Text::from("Subagent"));
    let detail = presentation::subagent_timeline_detail(status, item.result, item.progress);
    let remote = item.remote;
    let target = item
        .child_thread_id
        .map(|thread| ThreadKey {
            environment: context.key.environment.clone(),
            thread: ThreadId(thread.into()),
        })
        .or_else(|| {
            let remote = remote?;
            context
                .runtime
                .environments()
                .iter()
                .any(|e| e.id.0 == remote.environment_id.0)
                .then(|| ThreadKey {
                    environment: EnvironmentId(remote.environment_id.0.clone()),
                    thread: ThreadId(remote.thread_id.0.clone()),
                })
        });
    let failed = event.as_ref().map_or(*status == Status::Failed, |e| {
        e.0.status == Some(Status::Failed)
    });
    let detail_present = detail.is_some();
    let label = Text::client_key(
        event
            .as_ref()
            .map_or_else(|| presentation::status_label(status), |e| e.0.label)
            .to_owned(),
    );
    let mut heading = view().app_class(tw("flex items-baseline gap-2")).child(
        text({
            let title = title.clone();
            move || title.resolve()
        })
        .app_class(tw("min-w-0 truncate text-xs font-medium text-foreground")),
    );
    if let Some(remote) = remote {
        let label = remote.label.clone();
        heading = heading.child(
            text(move || tr("on {label}", &[("label", Value::Text(label.clone()))]))
                .app_class(tw("shrink-0 text-xs text-muted-foreground")),
        );
    }
    let body = view()
        .app_class(tw(&cn([
            Some("block text-2xs leading-relaxed"),
            Some(if failed {
                "text-destructive"
            } else {
                "text-muted-foreground"
            }),
        ])))
        .child(match detail {
            None => text({
                let label = label.clone();
                move || label.resolve()
            })
            .into_element(),
            Some(detail) if detail.contains('/') && !detail.contains(' ') => {
                middle_truncate_with_font(
                    move || SharedString::from(detail.clone()),
                    TruncateOptions {
                        show_title: false,
                        ..Default::default()
                    },
                    // NSH has no inherited computed font; theme::tw resolves the source's text-2xs to 11px.
                    || TruncateFont {
                        size: 11.0,
                        ..Default::default()
                    },
                )
                .into_element()
            }
            Some(detail) => text(SharedString::from(detail))
                .app_class(tw("block truncate"))
                .into_element(),
        });
    let content = view().app_class(tw("flex w-full min-w-0 items-center gap-2.5"))
        .child(environment_subagent_avatar(&item, event.as_ref().map_or(Some(status), |e| e.0.status.as_ref()), context))
        .child(view().app_class(tw("min-w-0 flex-1")).child(heading).child(body))
        .child((detail_present && (event.is_some() || *status != Status::Completed)).then(|| text({ let label = label.clone(); move || label.resolve() }).app_class(tw(&cn([
            Some("shrink-0 text-xs"), Some(if failed { "text-destructive" } else { "text-muted-foreground" }),
        ])))))
        .child(view().app_class(tw("shrink-0 text-xs text-muted-foreground tabular-nums")).child(event.map_or_else(|| subagent_elapsed(agent_timing, context).into_element(), |(_,timestamp)| text(SharedString::from(timestamp)).into_element())))
        .child(target.as_ref().map(|_| icon_with_class(Icon::ChevronRight, "size-3.5 shrink-0 text-muted-foreground/60 transition-colors group-hover/subagent:text-foreground")));
    let class =
        "group/subagent flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-1.5 text-left";
    if let Some(target) = target {
        let context = context.clone();
        nsh_base::button().app_class(tw(&cn([
            Some(class), Some("cursor-pointer transition-colors hover:bg-accent/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"),
        ]))).accessibility_label(move || Some(tr("Open {title}", &[("title", Value::Text(title.resolve().to_string()))]).into())).on_press(move || context.open(&target)).child(content).into_element()
    } else {
        view().app_class(tw(class)).child(content).into_element()
    }
}

fn v2_subagent_group(row: &EventRow, context: &EventRowContext) -> AnyElement {
    let members = row
        .subagents
        .clone()
        .unwrap_or_else(|| vec![row.projected_item.clone()]);
    let agents = members
        .iter()
        .filter_map(|p| match &p.item {
            Item::Subagent(item) => Some(item.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let timings = agents
        .iter()
        .map(|item| {
            timing(
                item,
                context.live_agent(item).as_ref().map(|a| a.0.as_ref()),
            )
        })
        .collect::<Vec<_>>();
    let statuses = timings.iter().map(|a| a.status.clone()).collect::<Vec<_>>();
    let summary = subagent_group_summary(&statuses);
    let status_summary = summarize_subagent_statuses(&statuses);
    let group_id = format!("subagent-group:{}", row.id);
    let state = context.state;
    let expanded = computed({
        let group_id = group_id.clone();
        move || state.expanded(&group_id)
    });
    let handle = CollapsibleHandle::controlled(move || expanded.get(), || false, {
        let callback = context.on_toggle_entry.clone();
        let row_id = row.id.clone();
        move |open| {
            if let Some(callback) = &callback {
                callback(row_id.clone(), expanded.get_untracked());
            }
            state.set_expanded(&group_id, open);
        }
    });
    let count = agents.len() as i64;
    let label = tr(
        "{count, plural, one {# subagent} other {# subagents}}",
        &[("count", Value::Int(count))],
    );
    let mut avatars = view()
        .accessible(false)
        .app_class(tw("flex shrink-0 items-center -space-x-1.5"));
    for item in agents.iter().take(3) {
        avatars = avatars.child(environment_subagent_avatar(
            &agent_link(item, None),
            None,
            context,
        ));
    }
    if agents.len() > 3 {
        avatars = avatars.child(text(SharedString::from(format!("+{}", agents.len() - 3))).app_class(tw("relative inline-flex size-6 items-center justify-center rounded-full bg-muted text-3xs font-medium text-muted-foreground ring-2 ring-background")));
    }
    let content = view()
        .app_class(tw("flex w-full min-w-0 items-center gap-2.5"))
        .child(avatars)
        .child(
            view()
                .app_class(tw("min-w-0 flex-1"))
                .child(
                    text(SharedString::from(label.clone()))
                        .app_class(tw("block text-xs font-semibold")),
                )
                .child(text(SharedString::from(status_summary)).app_class(tw(&cn([
                    Some("block truncate text-3xs text-muted-foreground"),
                    summary.active.then_some("text-info"),
                    (!summary.active && summary.failed).then_some("text-destructive"),
                ])))),
        )
        .child(
            view()
                .app_class(tw("shrink-0 text-xs text-muted-foreground tabular-nums"))
                .child(subagent_elapsed(
                    presentation::subagent_group_timing(&timings),
                    context,
                )),
        )
        .child(
            view()
                .app_class(move || {
                    Some(tw(&cn([
                        Some("size-3.5 shrink-0 text-muted-foreground transition-transform"),
                        expanded.get().then_some("rotate-180"),
                    ])))
                })
                .child(icon_with_class(Icon::ChevronDown, "size-3.5")),
        );
    let class =
        "flex w-full min-w-0 items-center gap-2.5 py-2 pr-3.25 text-left transition-opacity";
    let active = summary.active;
    // CollapsibleTrigger's class slot is static; its host binds the source's state-dependent tint.
    let trigger = view()
        .app_class(move || {
            Some(tw(&cn([
                Some("hover:opacity-100"),
                Some(if expanded.get() || active {
                    "text-foreground opacity-100"
                } else {
                    "text-muted-foreground opacity-55"
                }),
            ])))
        })
        .child(collapsible_trigger(
            handle,
            Text::content(label),
            content,
            class,
        ));
    let context = context.clone();
    let panel = dynamic(move || {
        expanded.get().then(|| {
            let mut panel = view().app_class(tw(
                "mt-1 mb-1 rounded-lg border border-border/60 bg-card/30 p-1",
            ));
            for member in &members {
                panel = panel.child(v2_lifecycle_row(&member.item, false, &context));
            }
            panel.into_element()
        })
    });
    collapsible(
        view()
            .app_class(tw(if row.continues_work_log {
                "pb-0"
            } else {
                "pb-2"
            }))
            .child(trigger)
            .child(collapsible_panel(handle, false, panel)),
    )
    .into_element()
}
