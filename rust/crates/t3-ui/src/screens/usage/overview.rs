use super::*;
use crate::{i18n::Text, ui::dialog::*};
use nsh_core::element::ImageSrc;
use std::fmt::Write;
use t3_client::services::usage::overview::{Figures, Overview};
use t3_i18n::display::{format_percent, format_tokens as tokens, format_usd as usd};
fn stat(label: String, value: String) -> AnyElement {
    view()
        .app_class(tw("flex min-w-0 flex-col gap-0.5"))
        .child(text(label).app_class(tw("text-xs text-muted-foreground")))
        .child(text(value).app_class(tw("text-base font-medium text-foreground tabular-nums")))
        .into_element()
}
pub(super) fn meter(fraction: f64) -> AnyElement {
    let part = fraction.clamp(0.0, 1.0) as f32;
    view()
        .app_class(tw(
            "flex h-2 flex-row overflow-hidden rounded-full bg-muted",
        ))
        .child(view().app_class(tw("h-full bg-foreground")).flex_grow(part))
        .child(view().flex_grow(1.0 - part))
        .into_element()
}
fn shares(
    title: String,
    labels: &[&str],
    values: &[f64],
    format: fn(f64) -> String,
) -> Option<AnyElement> {
    let total: f64 = values.iter().filter(|v| **v > 0.0).sum();
    if total <= 0.0 {
        return None;
    }
    let mut bar = view().app_class(tw("flex h-2 flex-row gap-0.5 overflow-hidden rounded-full"));
    let mut legend = view().app_class(tw("flex flex-row flex-wrap gap-4"));
    let mut description = Vec::new();
    for (i, (label, &value)) in labels
        .iter()
        .zip(values)
        .enumerate()
        .filter(|(_, (_, v))| **v > 0.0)
    {
        let label = t(label);
        let amount = format(value);
        let entry = format!("{label} · {amount} · {}", format_percent(value / total, 1));
        description.push(entry.clone());
        bar = bar.child(
            view()
                .app_class(tw("h-full min-w-1 bg-foreground"))
                .flex_grow((value / total) as f32)
                .opacity([0.60, 0.30, 0.72, 1.0, 0.44][i]),
        );
        legend = legend.child(text(entry).app_class(tw("text-xs text-muted-foreground")));
    }
    Some(
        view()
            .app_class(tw("flex flex-col gap-2.5"))
            .child(text(title.clone()))
            .child(
                bar.role(AccessibilityRole::Image)
                    .label(format!("{title}: {}", description.join(", "))),
            )
            .child(legend)
            .into_element(),
    )
}
fn token_share(f: &Figures) -> Option<AnyElement> {
    shares(
        t("Tokens by type"),
        &["Input", "Cache read", "Cache write", "Output"],
        &f.tokens,
        tokens,
    )
}
fn cost_share(f: &Figures) -> Option<AnyElement> {
    let other = f.cost - f.costs.iter().sum::<f64>();
    shares(
        t("Cost by type"),
        &["Input", "Cache read", "Cache write", "Output", "Other"],
        &[
            f.costs[0],
            f.costs[1],
            f.costs[2],
            f.costs[3],
            if other >= 0.005 { other } else { 0.0 },
        ],
        usd,
    )
}
fn speed_share(f: &Figures) -> Option<AnyElement> {
    if f.fast + f.ultrafast <= 0.0 {
        return None;
    }
    shares(
        t("Cost by speed"),
        &["Standard", "Fast", "Ultrafast"],
        &[
            (f.cost - f.fast - f.ultrafast).max(0.0),
            f.fast,
            f.ultrafast,
        ],
        usd,
    )
}
fn value(f: &Figures, is_tokens: bool) -> f64 {
    if is_tokens { f.total_tokens() } else { f.cost }
}
fn path(values: &[f64], max: f64) -> String {
    if values.len() < 2 || max == 0.0 {
        return String::new();
    }
    let dx = 600.0 / (values.len() - 1) as f64;
    let ys: Vec<_> = values.iter().map(|v| 160.0 - 156.0 * v / max).collect();
    let slopes: Vec<_> = ys.windows(2).map(|w| (w[1] - w[0]) / dx).collect();
    let mut tangents = vec![0.0; ys.len()];
    tangents[0] = slopes[0];
    tangents[ys.len() - 1] = slopes[slopes.len() - 1];
    for (i, w) in slopes.windows(2).enumerate() {
        if w[0] * w[1] > 0.0 {
            tangents[i + 1] = (w[0] + w[1]) / 2.0;
        }
    }
    for (i, slope) in slopes.iter().enumerate() {
        if *slope == 0.0 {
            tangents[i] = 0.0;
            tangents[i + 1] = 0.0;
            continue;
        }
        let magnitude = (tangents[i] / slope).hypot(tangents[i + 1] / slope);
        if magnitude > 3.0 {
            tangents[i] *= 3.0 / magnitude;
            tangents[i + 1] *= 3.0 / magnitude;
        }
    }
    let mut line = format!("M0,{:.2}", ys[0]);
    for i in 1..ys.len() {
        let x = i as f64 * dx;
        let _ = write!(
            line,
            " C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            x - 2.0 * dx / 3.0,
            ys[i - 1] + tangents[i - 1] * dx / 3.0,
            x - dx / 3.0,
            ys[i] - tangents[i] * dx / 3.0,
            x,
            ys[i]
        );
    }
    line
}
fn chart(data: &Overview, period: &UsageSummaryInput, is_tokens: bool) -> AnyElement {
    let hourly = metric_hourly(period);
    let title = match (hourly, is_tokens) {
        (true, true) => t("Hourly processed tokens"),
        (true, false) => t("Hourly cost"),
        (false, true) => t("Daily processed tokens"),
        _ => t("Daily cost"),
    };
    let periods = window::periods(period);
    let mut series: Vec<_> = data
        .providers
        .keys()
        .map(|&p| {
            let values: Vec<_> = periods
                .iter()
                .map(|k| {
                    data.periods
                        .get(k)
                        .and_then(|m| m.get(p))
                        .map_or(0.0, |f| value(f, is_tokens))
                })
                .collect();
            (p, values.iter().sum::<f64>(), values)
        })
        .collect();
    series.sort_by(|a, b| b.1.total_cmp(&a.1));
    let peak = series
        .iter()
        .flat_map(|(_, _, v)| v)
        .copied()
        .fold(0.0, f64::max);
    let mut svg_data = String::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 600 160" preserveAspectRatio="none">"#,
    );
    let max = if peak > 0.0 {
        let step = peak / 4.0;
        let mag = 10f64.powf(step.log10().floor());
        let n = step / mag;
        let step = mag
            * (if n > 5.0 {
                10.0
            } else if n > 2.0 {
                5.0
            } else if n > 1.0 {
                2.0
            } else {
                1.0
            });
        (peak / step).ceil() * step
    } else {
        0.0
    };
    for y in [4, 43, 82, 121, 160] {
        let _ = write!(
            svg_data,
            r##"<path d="M0,{y}h600" stroke="#888888" stroke-opacity=".24"/>"##
        );
    }
    let format = if is_tokens { tokens } else { usd };
    let points: Vec<_> = periods
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let values = series
                .iter()
                .map(|(name, _, values)| format!("{name} · {}", format(values[i])))
                .collect::<Vec<_>>();
            (window::period_label(key, period), values.join("  ·  "))
        })
        .collect();
    for (provider, _, values) in series {
        let color = match provider {
            "Claude Code" => "#d97757",
            "Cursor" => "#8b8b8b",
            "OpenCode" => "#5b9bbd",
            "Antigravity" => "#8c7bd1",
            "Pi" => "#4d9f8a",
            "Muse Code" => "#0866ff",
            _ => {
                if dark() {
                    "#efefef"
                } else {
                    "#343434"
                }
            }
        };
        let line = path(&values, max);
        if !line.is_empty() {
            let _ = write!(
                svg_data,
                r#"<path d="{line} L600,160 L0,160 Z" fill="{color}" fill-opacity=".16"/><path d="{line}" stroke="{color}" fill="none" stroke-width="2"/>"#
            );
        }
    }
    svg_data.push_str("</svg>");
    let axis = format(max);
    let hovered = signal(None::<usize>);
    let width = stored(0.0_f32);
    let count = points.len();
    let select = move |x: f32| {
        if width.get() > 0.0 && count > 0 {
            let fraction = (x / width.get()).clamp(0.0, 1.0);
            hovered.set(Some((fraction * (count - 1) as f32).round() as usize));
        }
    };
    let drawing = view()
        .role(AccessibilityRole::Image)
        .label(title.clone())
        .w_full()
        .h(px(160.0))
        .on_layout(move |b: &Bounds<Points>| {
            width.replace(b.size.width.0);
        })
        .on_mouse_move(move |event: &MouseEvent| select(event.position.x.0))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(None))
        .on_touch(move |event: &nsh_core::TouchEvent| {
            if matches!(
                event.phase,
                nsh_core::TouchPhase::Start | nsh_core::TouchPhase::Move
            ) && let Some(p) = event.points.iter().find(|p| p.changed)
            {
                select(p.location.x.0);
            }
        })
        .child(
            svg(ImageSrc::svg_markup(&svg_data))
                .pointer_events(PointerEvents::None)
                .w_full()
                .h_full(),
        )
        .child(region(
            move || hovered.get(),
            move |index| {
                index.and_then(|i| points.get(i)).map(|(period, values)| {
                    view()
                        .pointer_events(PointerEvents::None)
                        .app_class(tw(
                            "absolute left-2 top-2 rounded-md border bg-popover px-2 py-1",
                        ))
                        .child(text(period.clone()))
                        .child(text(values.clone()).app_class(tw("text-xs")))
                })
            },
        ));
    let first = periods
        .first()
        .map(|p| window::period_label(p, period))
        .unwrap_or_default();
    let last = periods
        .last()
        .map(|p| window::period_label(p, period))
        .unwrap_or_default();
    view()
        .app_class(tw("flex min-w-0 flex-1 flex-col gap-2"))
        .child(text(title))
        .child(text(axis).app_class(tw("text-xs text-muted-foreground")))
        .child(drawing)
        .child(
            view()
                .app_class(tw(
                    "flex flex-row justify-between text-xs text-muted-foreground",
                ))
                .child(text(first))
                .child(text(last)),
        )
        .into_element()
}
pub(super) fn overview(
    page: Page,
    data: &Overview,
    period: &UsageSummaryInput,
    is_tokens: bool,
) -> AnyElement {
    let f = &data.totals;
    let format = if is_tokens { tokens } else { usd };
    let mut summary = view()
        .app_class(tw("flex min-w-0 flex-col gap-2"))
        .child(
            text(format(value(f, is_tokens))).app_class(tw("text-4xl font-semibold tabular-nums")),
        )
        .child(text(format!(
            "{} sessions{}",
            data.sessions
                .values()
                .copied()
                .fold(0_u64, u64::saturating_add),
            if is_tokens { "" } else { " · API estimate" }
        )));
    let mut providers: Vec<_> = data.providers.iter().collect();
    providers.sort_by(|a, b| value(b.1, is_tokens).total_cmp(&value(a.1, is_tokens)));
    let total = value(f, is_tokens);
    let fraction = |n: f64| if total > 0.0 { n / total } else { 0.0 };
    if !is_tokens && f.unpriced_records > 0.0 {
        summary = summary.child(text(format!(
            "API estimate excludes {} unpriced records.",
            format_percent(f.unpriced_records / f.records, 1)
        )));
    }
    for (provider, totals) in providers {
        let amount = value(totals, is_tokens);
        let sessions = data.sessions.get(provider).copied().unwrap_or(0);
        let unit = if sessions == 1 {
            t("session")
        } else {
            t("sessions")
        };
        summary = summary.child(
            view()
                .app_class(tw("flex flex-col gap-1"))
                .child(stat(
                    format!("{provider} · {sessions} {unit}"),
                    format!(
                        "{} · {}",
                        format(amount),
                        format_percent(fraction(amount), 1)
                    ),
                ))
                .child(meter(fraction(amount))),
        );
    }
    let mut body = view()
        .app_class(tw("flex flex-col gap-6"))
        .child(
            view()
                .app_class(tw("flex flex-col gap-6 lg:flex-row"))
                .child(summary)
                .child(chart(data, period, is_tokens)),
        )
        .child(text(t("Totals")))
        .child(
            view()
                .app_class(tw("flex flex-row flex-wrap gap-4"))
                .child(stat(t("Processed tokens"), tokens(f.total_tokens())))
                .child(stat(t("Cached input"), tokens(f.tokens[1])))
                .child(stat(t("Uncached input"), tokens(f.tokens[0])))
                .child(stat(t("Output"), tokens(f.tokens[3])))
                .child(stat(t("Cache savings"), usd(f.savings))),
        );
    body = body
        .child(if is_tokens {
            token_share(f)
        } else {
            cost_share(f)
        })
        .child((!is_tokens).then(|| speed_share(f)).flatten());
    let by_time = page.by_time.get_untracked();
    body = body.child(
        view()
            .app_class(tw("flex flex-row items-center justify-between"))
            .child(text(t("Breakdown")))
            .child(controls::choice(
                "Usage breakdown",
                vec![
                    ("model", t("Model")),
                    (
                        "time",
                        if metric_hourly(period) {
                            t("Hour")
                        } else {
                            t("Day")
                        },
                    ),
                ],
                if by_time { "time" } else { "model" },
                move |s| page.by_time.set(s == "time"),
            )),
    );
    if by_time {
        for (key, figures) in data.periods.iter().rev() {
            body = body.child(stat(
                window::period_label(key, period),
                format!(
                    "{} · {}",
                    usd(figures.values().map(|f| f.cost).sum()),
                    tokens(figures.values().map(Figures::total_tokens).sum())
                ),
            ));
            let amounts = figures
                .iter()
                .map(|(name, f)| format!("{name} · {}", format(value(f, is_tokens))))
                .collect::<Vec<_>>()
                .join("  ·  ");
            body = body.child(text(amounts).app_class(tw("text-xs text-muted-foreground")));
        }
    } else {
        let mut models: Vec<_> = data.models.iter().collect();
        models.sort_by(|a, b| {
            value(b.1, is_tokens)
                .total_cmp(&value(a.1, is_tokens))
                .then_with(|| b.1.cost.total_cmp(&a.1.cost))
        });
        for ((provider, model), figures) in models {
            let key = (*provider, model.clone());
            body = body.child(
                view()
                    .app_class(tw(
                        "flex flex-row items-center justify-between gap-2 border-b py-2",
                    ))
                    .child(button(
                        ButtonContent::Label(model.clone()),
                        ButtonVariant::Ghost,
                        ButtonSize::Sm,
                        || false,
                        move || page.model.set(Some(key.clone())),
                    ))
                    .child(text(if figures.cost_unknown() {
                        t("Unpriced")
                    } else {
                        usd(figures.cost)
                    }))
                    .child(text(format_percent(fraction(value(figures, is_tokens)), 1)))
                    .child(text(tokens(figures.total_tokens()))),
            );
        }
    }
    if data.models.is_empty() {
        body = body.child(text(t("No activity in this window.")));
    }
    for notice in &data.notices {
        body = body.child(text(notice.clone()));
    }
    body.into_element()
}
pub(super) fn model_dialog(
    page: Page,
    provider: &str,
    model: &str,
    data: &Overview,
    period: &UsageSummaryInput,
    is_tokens: bool,
    on_price: impl Fn(String) + Copy + 'static,
) -> AnyElement {
    let model = model.to_owned();
    let provider = provider.to_owned();
    let data = data.clone();
    let period = period.clone();
    let open = signal(true);
    dialog(open, move |state| {
        state.on_open_change(move |next| {
            open.set(next);
            if !next {
                page.model.set(None);
            }
        });
        dialog_popup(
            state,
            PopupOptions {
                max_width: MaxWidth::ThreeXl,
                ..Default::default()
            },
            move || {
                let title = model.clone();
                let model = model.clone();
                let f = &data.totals;
                let mut content = view()
                    .app_class(tw("flex flex-col gap-4"))
                    .child(stat(
                        t("Cost"),
                        if f.cost_unknown() {
                            t("Unpriced")
                        } else {
                            usd(f.cost)
                        },
                    ))
                    .child(stat(t("Tokens"), tokens(f.total_tokens())));
                if let Some(rate) = f.per_million() {
                    content = content.child(stat(t("Per 1M tokens"), usd(rate)));
                }
                if let Some(rate) = f.cache_hit() {
                    content = content.child(stat(t("Cache hit"), format_percent(rate, 1)));
                }
                content = content
                    .child(chart(&data, &period, is_tokens || f.cost_unknown()))
                    .child((!f.cost_unknown()).then(|| cost_share(f)).flatten())
                    .child(token_share(f))
                    .child(speed_share(f));
                if f.unpriced_tokens > 0.0 {
                    content = content.child(button(
                        ButtonContent::Label(t("Set price")),
                        ButtonVariant::Default,
                        ButtonSize::Sm,
                        || false,
                        move || on_price(model.clone()),
                    ));
                }
                view()
                    .child(dialog_header(
                        state,
                        view()
                            .child(dialog_title(state, move || Text::content(title.clone())))
                            .child(text(provider.clone())),
                    ))
                    .child(dialog_panel(state, content))
            },
        )
    })
    .into_element()
}
