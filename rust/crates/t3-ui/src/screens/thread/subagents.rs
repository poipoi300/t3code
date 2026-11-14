//! Port of apps/web/src/components/chat/SubagentTooltipContent.tsx SubagentTooltipContent.

use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::store::Scope;
use t3_client::thread_views::messages_timeline::MessagesTimelineRow;
use t3_client::thread_views::subagent_inspector::{Speed, inspector_metadata, inspector_traits};
use t3_client::thread_views::threads::subagent_display::{
    MetadataInput, format_subagent_display_title, subagent_detail_preview,
};
use t3_client::thread_views::timeline_events::{self, AgentTiming, resolved_id};
use t3_client::types::ThreadKey;
use t3_contracts::orchestration_v2::{
    OrchestrationV2ProjectedTurnItem, OrchestrationV2Subagent, OrchestrationV2TurnItem,
    OrchestrationV2TurnItemStatus as Status, SubagentOrigin,
};
use t3_contracts::server::ServerConfig;
use t3_contracts::{model_selection::ModelSelection, wire::WireDateTime};

use super::rows::events::{EventRow, EventRowContext, TimelineRow, v2_event_timeline_row};
use crate::i18n::{Text, t};
use crate::runtime::{Shared, rt};
use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon_with_class, provider_icon};
use crate::ui::middle_truncate::{Font, Options, middle_truncate_with_font};
use crate::ui::tooltip::{TooltipOptions, Variant, tooltip, tooltip_create_handle};

#[derive(Clone, Debug)]
pub struct SubagentRow {
    pub event: EventRow,
}

impl SubagentRow {
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let event = EventRow::from_row(row)?;
        matches!(
            event.projected_item.item,
            OrchestrationV2TurnItem::Subagent(_)
        )
        .then_some(Self { event })
    }
}

/// Uses TL-EVENTS' canonical rows, keyed disclosure state and environment-scoped navigation.
pub fn subagent_timeline_row(row: TimelineRow, context: EventRowContext) -> impl IntoElement {
    let filtered = computed(move || row.get().filter(|row| SubagentRow::from_row(row).is_some()));
    v2_event_timeline_row(filtered, context)
}

/// TL-EVENTS passes the canonical projected item so both standalone and grouped links own a hint.
pub fn v2_lifecycle_projected_row(
    parent: ThreadKey,
    item: Rc<OrchestrationV2ProjectedTurnItem>,
    resource_summary: bool,
    context: &EventRowContext,
) -> AnyElement {
    let body = super::rows::events::v2_lifecycle_row(&item.item, resource_summary, context);
    if matches!(item.item, OrchestrationV2TurnItem::Subagent(_)) {
        with_subagent_tooltip(parent, item, body).into_element()
    } else {
        body
    }
}

/// Port of V2LifecycleRow.tsx SubagentTimelineTooltip. Construct only while the hint is open.
pub fn subagent_timeline_tooltip(
    parent: ThreadKey,
    item: Rc<OrchestrationV2ProjectedTurnItem>,
) -> impl IntoElement {
    let thread = rt().store.thread(parent.clone());
    let fallback = item.clone();
    let agent = computed(move || {
        thread.with(|thread| {
            let OrchestrationV2TurnItem::Subagent(item) = &item.item else {
                return None;
            };
            thread
                .as_ref()?
                .subagents
                .iter()
                .find(|agent| agent.id == item.subagent_id)
                .cloned()
                .map(Shared)
        })
    });
    dynamic(move || {
        let source = agent.get().map_or_else(
            || InspectorSource::Item(fallback.clone()),
            |agent| InspectorSource::Live(agent.0),
        );
        untracked(|| inspector_content(parent.clone(), source).into_element())
    })
}

/// Fitted native host: Button does not expose NSH's tooltip behavior.
pub fn with_subagent_tooltip(
    parent: ThreadKey,
    item: Rc<OrchestrationV2ProjectedTurnItem>,
    child: impl IntoElement,
) -> impl IntoElement {
    let handle = tooltip_create_handle();
    view()
        .app_class(tw("w-full min-w-0"))
        .tooltip(tooltip(
            handle,
            TooltipOptions {
                variant: Variant::Glass,
                delay: Some(200.0),
                ..Default::default()
            },
            dynamic(move || {
                handle.is_open().then(|| {
                    untracked(|| {
                        subagent_timeline_tooltip(parent.clone(), item.clone()).into_element()
                    })
                })
            }),
        ))
        .child(child)
}

/// The inspector owns provider/project reads, traits, workspace facts, detail and timing.
pub fn subagent_tooltip_content(
    parent: ThreadKey,
    agent: Rc<OrchestrationV2Subagent>,
) -> impl IntoElement {
    inspector_content(parent, InspectorSource::Live(agent))
}

#[derive(Clone)]
enum InspectorSource {
    Live(Rc<OrchestrationV2Subagent>),
    Item(Rc<OrchestrationV2ProjectedTurnItem>),
}

struct InspectorAgent<'a> {
    title: Option<&'a str>,
    model: Option<&'a str>,
    selection: Option<&'a ModelSelection>,
    child: Option<&'a str>,
    instance: Option<&'a str>,
    driver: &'a str,
    origin: &'a SubagentOrigin,
    status: &'a Status,
    result: Option<&'a str>,
    progress: Option<&'a str>,
    started_at: Option<&'a WireDateTime>,
    completed_at: Option<&'a WireDateTime>,
}

impl InspectorSource {
    fn agent(&self) -> Option<InspectorAgent<'_>> {
        Some(match self {
            Self::Live(a) => InspectorAgent {
                title: a.title.as_deref(),
                model: a.model.as_deref(),
                selection: a.model_selection.as_ref().and_then(Option::as_ref),
                child: a.child_thread_id.as_ref().and_then(resolved_id),
                instance: resolved_id(&a.provider_instance_id),
                driver: &a.driver.0,
                origin: &a.origin,
                status: &a.status,
                result: a.result.as_deref(),
                progress: a.progress.as_ref().and_then(Option::as_deref),
                started_at: a.started_at.as_ref(),
                completed_at: a.completed_at.as_ref(),
            },
            Self::Item(row) => {
                let OrchestrationV2TurnItem::Subagent(a) = &row.item else {
                    return None;
                };
                InspectorAgent {
                    title: a.title.as_deref(),
                    model: None,
                    selection: None,
                    child: a.child_thread_id.as_ref().and_then(resolved_id),
                    instance: resolved_id(&a.provider_instance_id),
                    driver: &a.driver.0,
                    origin: &a.origin,
                    status: &a.status,
                    result: a.result.as_deref(),
                    progress: a.progress.as_ref().and_then(Option::as_deref),
                    started_at: a.started_at.as_ref(),
                    completed_at: a.completed_at.as_ref(),
                }
            }
        })
    }
}

fn inspector_content(parent: ThreadKey, source: InspectorSource) -> impl IntoElement {
    let shell = rt().store.shell(parent.environment.clone());
    let config = environment_config(parent.clone());
    dynamic(move || {
        let shell = shell.get();
        let config = config.get();
        untracked(|| inspector_view(&parent, &source, shell.as_deref(), config.as_deref()))
    })
}

fn inspector_view(
    parent: &ThreadKey,
    source: &InspectorSource,
    shell: Option<&t3_client::state::sharing::SharedShellSnapshot>,
    config: Option<&ServerConfig>,
) -> AnyElement {
    let Some(agent) = source.agent() else {
        return view().into_element();
    };
    let parent_thread =
        shell.and_then(|shell| shell.threads.iter().find(|t| t.id.0 == parent.thread.0));
    let child_id = agent.child;
    let child = shell.and_then(|shell| {
        shell
            .threads
            .iter()
            .find(|t| Some(t.id.0.as_str()) == child_id)
    });
    let parent_project = shell.and_then(|shell| {
        shell
            .projects
            .iter()
            .find(|p| parent_thread.is_some_and(|t| t.project_id == p.id))
    });
    let child_project = shell.and_then(|shell| {
        shell
            .projects
            .iter()
            .find(|p| child.is_some_and(|t| t.project_id == p.id))
    });
    let instance = agent.instance;
    let provider = config.and_then(|config| {
        config
            .providers
            .0
            .iter()
            .find(|p| Some(p.instance_id.0.as_str()) == instance)
    });
    let selection = agent.selection.or_else(|| {
        (*agent.origin == SubagentOrigin::AppOwned)
            .then(|| child.map(|t| &t.model_selection))
            .flatten()
    });
    let metadata = inspector_metadata(MetadataInput {
        model: agent.model,
        selection,
        provider,
        parent_thread: parent_thread.map(|t| t.as_ref()),
        child_thread: child.map(|t| t.as_ref()),
        parent_project: parent_project.map(|p| p.as_ref()),
        child_project: child_project.map(|p| p.as_ref()),
    });
    let title = child
        .map(|t| Text::content(format_subagent_display_title(&t.title)))
        .or_else(|| {
            agent
                .title
                .map(|title| Text::content(format_subagent_display_title(title)))
        })
        .unwrap_or_else(|| Text::from("Subagent"));
    let traits = inspector_traits(
        selection,
        metadata.model_slug.as_deref(),
        provider,
        instance.unwrap_or_default(),
    );
    let visual = config.and_then(|c| timeline_events::provider_visual(c, instance?));
    let model_label = if metadata.model_slug.as_deref().is_none_or(str::is_empty) {
        Text::client_key(metadata.model_label)
    } else {
        Text::content(metadata.model_label)
    };
    let instance_name = visual
        .as_ref()
        .filter(|visual| visual.show_badge)
        .map(|visual| visual.display_name.clone());
    let driver = provider.map_or(agent.driver, |p| p.driver.0.as_str());
    let mut model = view()
        .app_class(tw(
            "inline-flex min-w-0 items-center gap-1 text-foreground/75",
        ))
        .child(
            text(move || {
                let label = model_label.resolve();
                instance_name.as_ref().map_or_else(
                    || label.clone(),
                    |name| SharedString::from(format!("{label} · {name}")),
                )
            })
            .app_class(tw("min-w-0 truncate")),
        );
    if traits.effort.is_some() || traits.speed.is_some() {
        model = model.child(
            view()
                .app_class(tw("inline-flex shrink-0 items-center gap-1"))
                .child(
                    traits
                        .effort
                        .as_ref()
                        .map(|_| text(SharedString::from(" · "))),
                )
                .child(traits.speed.map(|speed| {
                    view()
                        .label(move || match speed {
                            Speed::Fast => t("Fast mode on"),
                            Speed::Ultrafast => t("Ultrafast mode on"),
                        })
                        .child(match speed {
                            Speed::Fast => icon_with_class(
                                Icon::Zap,
                                "size-3 shrink-0 fill-current opacity-80",
                            )
                            .into_element(),
                            Speed::Ultrafast => ultrafast_icon().into_element(),
                        })
                }))
                .child(traits.effort.map(|effort| text(SharedString::from(effort)))),
        );
    }
    let glyph = if timeline_events::active(agent.status) {
        Icon::CircleDashed
    } else if *agent.status == Status::Failed {
        Icon::CircleX
    } else if *agent.status == Status::Completed {
        Icon::Check
    } else {
        Icon::CircleDashed
    };
    let label = status_key(agent.status);
    let timing = AgentTiming {
        status: agent.status.clone(),
        started_at: agent.started_at.cloned(),
        completed_at: agent.completed_at.cloned(),
    };
    let mut details = view()
        .app_class(tw("grid gap-1.5 pl-0.5 text-xs text-muted-foreground"))
        .child(
            view()
                .app_class(tw("flex min-w-0 items-center gap-2"))
                .child(provider_instance_icon(driver, visual.as_ref()))
                .child(model),
        )
        .child(
            view()
                .app_class(tw("flex min-w-0 items-center justify-between gap-4"))
                .child(
                    view()
                        .app_class(tw(&cn([
                            Some(
                                "inline-flex items-center gap-1 rounded-sm font-medium capitalize",
                            ),
                            Some(status_class(agent.status)),
                        ])))
                        .child(icon_with_class(glyph, "size-3 shrink-0"))
                        .child(text(move || Text::client_key(label).resolve())),
                )
                .child(subagent_elapsed(timing, parent.environment.clone())),
        );
    for (label, value) in metadata.workspace {
        details = details.child(
            view()
                .app_class(tw("flex min-w-0 items-center gap-2"))
                .label(move || Text::client_key(label).resolve())
                .child(icon_with_class(
                    if label == "Branch" {
                        Icon::GitBranch
                    } else {
                        Icon::Folder
                    },
                    "size-3 shrink-0",
                ))
                .child(truncated(value)),
        );
    }
    if let Some(preview) = subagent_detail_preview(agent.status, agent.result, agent.progress) {
        details = details.child(
            view()
                .app_class(tw("flex min-w-0 items-center gap-2"))
                .child(icon_with_class(Icon::Terminal, "size-3 shrink-0"))
                .child(truncated(preview)),
        );
    }
    thread_hover_card(title, details).into_element()
}

/// Port of apps/web/src/components/ThreadHoverCard.tsx ThreadHoverCard.
fn thread_hover_card(title: Text, children: impl IntoElement) -> impl IntoElement {
    view()
        .app_class(tw("flex min-w-0 max-w-80 flex-col gap-2 px-1 py-2"))
        .child(text(move || title.resolve()).app_class(tw(
            "min-w-0 truncate text-xs leading-tight font-medium text-foreground",
        )))
        .child(children)
}

/// Port of AgentElapsed.tsx: only the visible live label ticks, using the destination's clock.
fn subagent_elapsed(
    agent: AgentTiming,
    environment: t3_client::types::EnvironmentId,
) -> impl IntoElement {
    let live = timeline_events::active(&agent.status);
    let client = rt().backend().client();
    let read = move || {
        client
            .as_ref()?
            .client(&environment)?
            .session()
            .map(|session| session.to_server_clock(nsh_core::time::unix_ms() as f64) as i64)
    };
    let now = signal(if live { read() } else { None });
    if live {
        set_interval(std::time::Duration::from_secs(1), move || now.set(read()));
    }
    text(move || {
        SharedString::from(
            if live {
                now.get()
                    .and_then(|now| timeline_events::subagent_elapsed(&agent, now))
            } else {
                timeline_events::subagent_elapsed(&agent, 0)
            }
            .unwrap_or_default(),
        )
    })
    .app_class(tw("tabular-nums"))
}

/// Port of components/Icons.tsx UltrafastIcon.
fn ultrafast_icon() -> impl IntoElement {
    svg(nsh_core::element::ImageSrc::svg_markup(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><path d="m17 2-10 12h7l-1 8 10-12h-7l1-8Z" opacity="0.4"/><path d="m11 2-10 12h7l-1 8 10-12h-7l1-8Z"/></svg>"#))
        .accessible(false)
        .app_class(tw("size-3 shrink-0 fill-current opacity-80"))
}

/// Port of ProviderInstanceIcon.tsx, the tooltip's badgeContent="none"/12px branch.
fn provider_instance_icon(
    driver: &str,
    visual: Option<&timeline_events::ProviderVisual>,
) -> AnyElement {
    let name = visual.map_or(driver, |v| v.display_name.as_str());
    // At 12px the custom tile's container query hides the protocol mark and second initial.
    let glyph = if driver == "openaiCompatible" {
        view().accessible(false)
            .app_class(tw("relative flex size-3 shrink-0 items-center justify-center rounded-md border bg-card text-4xs font-semibold leading-none text-foreground grayscale opacity-60"))
            .child(text(SharedString::from(t3_client::services::providers::provider_instance_initials(name).chars().take(1).collect::<String>())))
            .into_element()
    } else {
        provider_icon(driver, name, "size-3 shrink-0 grayscale opacity-60").into_element()
    };
    // isolate has no NSH meaning; subtree accent variables become the same typed badge fill.
    let mut root = view()
        .accessible(false)
        .app_class(tw(
            "relative z-30 inline-flex shrink-0 items-center justify-center overflow-visible",
        ))
        .child(glyph);
    if driver != "openaiCompatible"
        && let Some(visual) = visual.filter(|v| v.show_badge)
        && let Some(accent) = visual
            .accent
            .as_ref()
            .and_then(|s| s.strip_prefix('#'))
            .and_then(|s| u32::from_str_radix(s, 16).ok())
    {
        root = root.child(view()
            .app_class(tw("pointer-events-none absolute right-0 bottom-0 z-10 flex h-2 min-w-2 items-center justify-center rounded-full border px-0 text-4xs font-semibold leading-none shadow-sm"))
            .border_color(move || crate::theme::palette().card)
            .bg(AdaptiveColor::rgb(accent, accent)));
    }
    root.into_element()
}

fn truncated(value: String) -> impl IntoElement {
    middle_truncate_with_font(
        move || SharedString::from(value.clone()),
        Options {
            show_title: false,
            ..Default::default()
        },
        // NSH has no computed inherited font; text-xs in the source is 12px.
        || Font {
            size: 12.0,
            ..Default::default()
        },
    )
}

fn environment_config(parent: ThreadKey) -> Memo<Option<Shared<ServerConfig>>> {
    let store = rt().store.store();
    let mut changes = store.subscribe_environment(&parent.environment);
    let read = move || {
        store
            .snapshot()
            .environments
            .get(&parent.environment)
            .and_then(|e| e.config.clone())
            .map(Shared)
    };
    let config = signal(read());
    spawn(async move {
        while let Some(change) = changes.next().await {
            if change
                .scopes
                .iter()
                .any(|scope| matches!(scope, Scope::All | Scope::Environment(_)))
            {
                config.set(read());
            }
        }
    });
    computed(move || config.get())
}

fn status_key(status: &Status) -> &'static str {
    match status {
        Status::Pending => "pending",
        Status::Running => "running",
        Status::Waiting => "waiting",
        Status::Idle => "idle",
        Status::Completed => "completed",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
        Status::Interrupted => "interrupted",
    }
}

fn status_class(status: &Status) -> &'static str {
    match status {
        Status::Pending | Status::Running | Status::Waiting => "text-info",
        Status::Failed => "text-error",
        Status::Completed => "text-success",
        _ => "text-muted-foreground",
    }
}
