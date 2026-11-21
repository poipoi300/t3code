//! UsagePage.tsx and its route modal.
mod controls;
mod overview;
mod preferences;
mod prices;
mod window;
use crate::{
    i18n::t,
    runtime::{
        rt,
        tasks::{TaskOwner, watch},
    },
    screens::settings::shell::region,
    state::{layout::layout_class, nav::navigator, url::UrlState},
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        icon::Icon,
        scroll_area::scroll_area,
    },
};
use futures::StreamExt;
use nsh_core::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_client::{
    composition::{ClientRuntime, usage::UsageTarget},
    platform::Platform,
    services::usage::{
        limits::limits,
        overview::{PROVIDERS, Report, merge},
    },
    types::{EnvironmentId, QueryState},
};
use t3_contracts::usage::{UsageSummary, UsageSummaryInput};
type Reports = BTreeMap<EnvironmentId, QueryState<Rc<UsageSummary>>>;
#[derive(Clone, Copy)]
struct Page {
    metric: Signal<&'static str>,
    days: Signal<u32>,
    selected: Signal<Option<BTreeSet<String>>>,
    hidden: Signal<BTreeSet<String>>,
    model: Signal<Option<(&'static str, String)>>,
    by_time: Signal<bool>,
    refresh: Signal<u64>,
}
impl Page {
    fn new() -> Self {
        Self {
            metric: signal("limits"),
            days: signal(30),
            selected: signal(None),
            hidden: signal(BTreeSet::new()),
            model: signal(None),
            by_time: signal(false),
            refresh: signal(0),
        }
    }
}
pub fn screen() -> AnyElement {
    rt().backend().client().as_ref().map_or_else(
        || modal(text(t("Connect an environment to see limits.")).into_element()),
        screen_for,
    )
}
fn close_action() -> impl Fn() + Copy {
    let nav = navigator();
    let urls = use_context::<UrlState>();
    move || {
        if let Some(urls) = urls {
            urls.close_modal()
        } else {
            nav.back()
        }
    }
}
fn modal(body: AnyElement) -> AnyElement {
    let panel = view()
        .app_class(tw("relative flex size-full min-h-0 min-w-0 flex-col overflow-hidden bg-background text-foreground sm:max-h-[60rem] sm:max-w-[80rem] sm:rounded-2xl sm:border"))
        .child(body);
    let frame = view()
        .pointer_events(PointerEvents::BoxNone)
        .app_class(tw(
            "flex size-full items-center justify-center sm:p-6 lg:p-10",
        ))
        .child(panel);
    let backdrop = view()
        .app_class(tw("absolute inset-0 bg-background/60"))
        .on_press(close_action());
    nsh_base::dialog()
        .role(AccessibilityRole::Dialog)
        .label(t("Usage"))
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .on_cancel(close_action())
        .app_class(tw("w-full h-full"))
        .child(backdrop)
        .child(frame)
        .into_element()
}
pub fn screen_for<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> AnyElement {
    let page = Page::new();
    preferences::bind(client, page);
    let targets = signal(client.usage_targets());
    let mut changes = client.changes();
    let runtime = client.clone();
    watch(async move {
        while changes.next().await.is_some() {
            targets.set(runtime.usage_targets());
        }
    });
    let selected = computed(move || {
        let ids = page.selected.get();
        targets.with(|all| {
            all.iter()
                .filter(|t| ids.as_ref().is_none_or(|ids| ids.contains(&t.id.0)))
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let runtime = client.clone();
    let period = computed(move || {
        let days = page.days.get();
        let _ = page.refresh.get();
        window::input(
            days,
            runtime.usage_clock_ms(),
            locale_settings().time_zone.as_str(),
        )
    });
    let reads = computed(move || {
        selected.with(|all| {
            all.iter()
                .filter(|t| t.connected && t.diagnostics)
                .map(|t| {
                    (
                        t.id.clone(),
                        t.generation,
                        t.config
                            .as_ref()
                            .and_then(|c| c.settings.usage_model_filter.clone()),
                    )
                })
                .collect::<Vec<_>>()
        })
    });
    let reports = signal(Reports::new());
    let runtime = client.clone();
    let limits_tab = computed(move || page.metric.get() == "limits");
    let limit_refresh = controls::limits_refresh(client, selected, limits_tab);
    let handled = stored(0u64);
    let sequence = stored(0_u64);
    let pending = signal(0_usize);
    let previous = stored(None);
    effect(move || {
        let key = sequence.get().wrapping_add(1);
        sequence.replace(key);
        let use_limits = limits_tab.get();
        let reads = reads.get();
        let input = period.get();
        let refresh = page.refresh.get();
        let manual = refresh != handled.get();
        handled.replace(refresh);
        let selection = (reads.clone(), input.clone());
        let keep = !use_limits && previous.with(|old| old.as_ref() == Some(&selection));
        previous.replace((!use_limits).then_some(selection));
        if !keep {
            reports.set(Reports::new());
        }
        pending.set(if use_limits { 0 } else { reads.len() });
        if use_limits {
            return;
        }
        for (id, _, filter) in reads {
            let mut input = input.clone();
            input.model_filter = filter.map(Some);
            let mut stream = runtime.usage_report(&id, input, manual);
            watch(async move {
                while let Some(mut state) = stream.next().await {
                    if sequence.get() != key {
                        break;
                    }
                    reports.update(|r| {
                        if keep && state.value.is_none() && state.error.is_none() {
                            state.value = r.get(&id).and_then(|old| old.value.clone());
                            state.loading = true;
                        }
                        r.insert(id.clone(), state);
                    });
                }
                if sequence.get() == key {
                    pending.update(|n| *n -= 1);
                }
            });
        }
    });
    let prices = signal(false);
    preferences::shortcuts(page, prices);
    let price_model = signal(String::new());
    let on_price = move |model: String| {
        page.model.set(None);
        price_model.set(model);
        prices.set(true);
    };
    let header = region(
        move || {
            (
                layout_class(),
                targets.get(),
                page.metric.get(),
                page.days.get(),
                page.selected.get(),
                page.hidden.get(),
            )
        },
        move |(_, all, metric, days, _, _)| {
            let mut row = view()
                .app_class(tw(
                    "flex min-h-12 min-w-0 flex-row items-center gap-1 border-b px-3",
                ))
                .child(text(t("Usage")).app_class(tw("text-xs text-muted-foreground")))
                .child(controls::separator())
                .child(controls::filters(&all, page, prices))
                .child(view().flex_1());
            if metric != "limits" {
                row = row.child(controls::choice(
                    "Usage period",
                    vec![
                        ("1", t("Past 24h")),
                        ("7", t("7 days")),
                        ("30", t("30 days")),
                        ("90", t("90 days")),
                    ],
                    match days {
                        1 => "1",
                        7 => "7",
                        90 => "90",
                        _ => "30",
                    },
                    move |v| {
                        if let Ok(n) = v.parse() {
                            page.days.set(n);
                        }
                    },
                ));
            }
            row = row.child(controls::choice(
                "Usage metric",
                vec![
                    ("cost", t("Cost")),
                    ("tokens", t("Tokens")),
                    ("limits", t("Limits")),
                ],
                metric,
                move |v| {
                    page.metric.set(match v {
                        "cost" => "cost",
                        "tokens" => "tokens",
                        _ => "limits",
                    })
                },
            ));
            if metric == "limits" {
                let refresh = limit_refresh.clone();
                row = row.child(button(
                    ButtonContent::Icon {
                        icon: Icon::RefreshCw,
                        label: t("Refresh limits"),
                    },
                    ButtonVariant::Ghost,
                    ButtonSize::IconSm,
                    move || refresh.disabled.get(),
                    move || (refresh.run)(false),
                ));
            } else {
                row = row.child(button(
                    ButtonContent::Icon {
                        icon: Icon::RefreshCw,
                        label: t("Refresh usage"),
                    },
                    ButtonVariant::Ghost,
                    ButtonSize::IconSm,
                    move || reads.with(Vec::is_empty) || pending.get() != 0,
                    move || page.refresh.update(|n| *n += 1),
                ));
            }
            row.child(button(
                ButtonContent::Icon {
                    icon: Icon::X,
                    label: t("Close"),
                },
                ButtonVariant::Ghost,
                ButtonSize::IconSm,
                || false,
                close_action(),
            ))
        },
    );
    let runtime = client.clone();
    let content = region(
        move || {
            (
                selected.get(),
                reports.get(),
                page.metric.get(),
                page.hidden.get(),
                period.get(),
                page.by_time.get(),
            )
        },
        move |(all, reports, metric, hidden, period, _)| {
            if all.is_empty() {
                let none = targets.with(Vec::is_empty);
                return text(match (none, metric == "limits") {
                    (true, true) => t("Connect an environment to see limits."),
                    (false, true) => t("Select an environment to see limits."),
                    (true, false) => t("Connect an environment to see usage."),
                    _ => t("Select an environment to see usage."),
                })
                .into_element();
            }
            if metric == "limits" {
                return limit_view(&runtime, &all, &hidden);
            }
            let mut body = view().app_class(tw("flex flex-col gap-5"));
            let summaries = answered(&all, &reports);
            for target in &all {
                let status = if !target.connected {
                    Some(t("This environment is not connected."))
                } else if !target.diagnostics {
                    Some(t(
                        "This connection does not have access to diagnostics and usage.",
                    ))
                } else if let Some(error) = reports.get(&target.id).and_then(|r| r.error.clone()) {
                    Some(error)
                } else if reports
                    .get(&target.id)
                    .is_none_or(|r| r.value.is_none() || r.loading)
                {
                    Some(t("Updating…"))
                } else {
                    None
                };
                if let Some(status) = status {
                    body = body.child(text(if all.len() > 1 {
                        format!("{}: {status}", target.label)
                    } else {
                        status
                    }));
                }
            }
            if !summaries.is_empty() {
                let data = merge(&summaries, &hidden, None, metric_hourly(&period));
                let sharing = !hidden.contains("Codex")
                    && all
                        .iter()
                        .filter_map(|t| t.config.as_ref())
                        .flat_map(|c| &c.providers.0)
                        .any(|p| {
                            matches!(
                                p.auth.status,
                                t3_contracts::server::ServerProviderAuthStatus::Authenticated
                            ) && p.auth.subscription_sharing == Some(Some(true))
                        });
                if sharing {
                    let opener = crate::runtime::services::links::link_opener(&runtime);
                    body = body.child(
                        view()
                            .app_class(tw("flex flex-row items-center justify-between gap-2"))
                            .child(text(t("ChatGPT shared usage")))
                            .child(button(
                                ButtonContent::Label(t("Manage usage")),
                                ButtonVariant::GhostMuted,
                                ButtonSize::Xs,
                                || false,
                                move || opener.open("https://chatgpt.com/#settings/Usage"),
                            )),
                    );
                }
                let dimmed = reports.values().all(|s| s.loading);
                body = body.child(
                    view()
                        .opacity(if dimmed { 0.5 } else { 1.0 })
                        .child(overview::overview(page, &data, &period, metric == "tokens")),
                );
            }
            body.into_element()
        },
    );
    let detail = region(
        move || {
            (
                page.model.get(),
                selected.get(),
                reports.get(),
                period.get(),
                page.metric.get(),
            )
        },
        move |(model, all, reports, period, metric)| {
            model.and_then(|(provider, model)| {
                let data = merge(
                    &answered(&all, &reports),
                    &page.hidden.get_untracked(),
                    Some((provider, &model)),
                    metric_hourly(&period),
                );
                (data.totals.records > 0.0).then(|| {
                    overview::model_dialog(
                        page,
                        provider,
                        &model,
                        &data,
                        &period,
                        metric == "tokens",
                        on_price,
                    )
                })
            })
        },
    );
    let price_dialog =
        prices::dialog_for(client.clone(), selected, prices, price_model, page.refresh);
    modal(
        view()
            .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col"))
            .child(header)
            .child(scroll_area(
                false,
                view()
                    .app_class(tw("mx-auto flex w-full max-w-6xl flex-col gap-8 p-5"))
                    .child(content),
            ))
            .child(detail)
            .child(price_dialog)
            .into_element(),
    )
}
fn metric_hourly(input: &UsageSummaryInput) -> bool {
    input.since_time.is_some()
}
fn answered<'a>(all: &'a [UsageTarget], reports: &'a Reports) -> Vec<Report<'a>> {
    all.iter()
        .filter(|t| t.connected && t.diagnostics)
        .filter_map(|t| {
            Some(Report {
                id: &t.id.0,
                label: &t.label,
                summary: reports.get(&t.id)?.value.as_deref()?,
            })
        })
        .collect()
}
fn limit_view<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    all: &[UsageTarget],
    hidden: &BTreeSet<String>,
) -> AnyElement {
    let data = limits(all, hidden, client.usage_clock_ms());
    let mut body = view().app_class(tw("flex flex-col gap-3"));
    if data.windows.is_empty() && data.links.is_empty() && data.notices.is_empty() {
        return text(t(
            "No provider on the selected environments reports subscription limits.",
        ))
        .into_element();
    }
    let mut last = String::new();
    for w in data.windows {
        if last != w.driver {
            body = body.child(text(w.driver.clone()).app_class(tw("text-sm font-medium")));
            last = w.driver;
        }
        let mut card = view()
            .app_class(tw("flex flex-col gap-2 rounded-lg border p-4"))
            .child(text(w.label))
            .child(
                view()
                    .app_class(tw("flex flex-row items-baseline gap-2"))
                    .child(
                        text(format!("{}%", w.remaining))
                            .app_class(tw("text-3xl font-semibold tabular-nums")),
                    )
                    .child(text(t("left"))),
            );
        let mut segments =
            view().app_class(tw("flex h-2 flex-row gap-0.5 overflow-hidden rounded-full"));
        for member in &w.accounts {
            segments = segments.child(
                view()
                    .flex_grow(member.capacity as f32)
                    .child(overview::meter(member.remaining / 100.0)),
            );
        }
        card = card.child(segments);
        for member in w.accounts {
            let expanded = signal(false);
            let details = region(
                move || expanded.get(),
                move |open| {
                    open.then(|| {
                        view()
                            .app_class(tw("flex flex-col gap-1 px-2 text-xs text-muted-foreground"))
                            .child(member.reset.clone().map(text))
                            .child(
                                member
                                    .time_remaining
                                    .map(|time| text(format!("{time:.0}% time remaining"))),
                            )
                    })
                },
            );
            card = card
                .child(
                    view()
                        .app_class(tw("flex flex-row items-center gap-2"))
                        .child(button(
                            ButtonContent::Label(member.name),
                            ButtonVariant::Ghost,
                            ButtonSize::Xs,
                            || false,
                            move || expanded.update(|v| *v = !*v),
                        ))
                        .child(text(format!("{:.0}%", member.remaining))),
                )
                .child(details);
        }
        for name in w.zero_caps {
            card = card.child(text(format!("{name} · {}", t("$0 cap"))));
        }
        body = body.child(card);
    }
    for (url, (label, accounts)) in data.links {
        body = body.child(text(label)).child(text(accounts.join(", ")));
        if let Some(url) = client.external_link(&url) {
            let opener = crate::runtime::services::links::link_opener(client);
            body = body.child(button(
                ButtonContent::Label(t("Manage usage")),
                ButtonVariant::GhostMuted,
                ButtonSize::Xs,
                || false,
                move || opener.open(url.clone()),
            ));
        }
    }
    for notice in data.notices {
        body = body.child(text(notice));
    }
    body.into_element()
}
