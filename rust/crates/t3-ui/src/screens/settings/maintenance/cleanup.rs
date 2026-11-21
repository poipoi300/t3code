use super::*;
use crate::i18n::{Value, tr};
use t3_client::{
    navigation::ignored_selection::format_bytes,
    services::settings::maintenance::{has_rules, number},
};
use t3_contracts::settings::{
    StorageCleanupReportEntryKind as Kind, StorageCleanupReportEntryOutcome as Outcome,
};

pub(super) fn section(context: SettingsContext, machines: Stored<Vec<Live>>) -> AnyElement {
    let machines = stored(
        machines
            .get()
            .into_iter()
            .filter(|m| m.machine.get().run)
            .collect::<Vec<_>>(),
    );
    if machines.with(|items| items.is_empty()) {
        return view().into_element();
    }
    let pending = signal(false);
    let rules = move || {
        context.view().is_some_and(|v| {
            v.targets.iter().any(|target| {
                machines.with(|m| {
                    m.iter()
                        .any(|m| m.machine.with(|e| e.id == target.environment_id))
                }) && has_rules(&target.json)
            })
        })
    };
    let allowed = move || machines.with(|m| m.iter().any(|m| m.session.get().can_clean()));
    let rows = settings_row(
        SettingsRowProps::searchable("storage-delete-now")
            .first()
            .description(move || {
                if !rules() {
                    return Text::from("Turn on a cleanup rule to use this.");
                }
                Text::content(machines.with(|machines| {
                    machines
                        .iter()
                        .map(|m| {
                            let desc = match m.report.get() {
                                LastRun::Failed => t("Could not load the last run."),
                                LastRun::Loading => t("Loading last run…"),
                                LastRun::Ready(None) => t("Cleanup hasn't run yet."),
                                LastRun::Ready(Some(report)) => {
                                    let when = crate::screens::sidebar::row::relative_label(
                                        &report.finished_at,
                                        crate::state::clock::now_ms(),
                                    );
                                    let mut desc = tr(
                                        "Last run {when}. {amount} recovered.",
                                        &[
                                            ("when", Value::Text(when)),
                                            (
                                                "amount",
                                                Value::Text(format_bytes(
                                                    number(&report.bytes_freed) as u64,
                                                )),
                                            ),
                                        ],
                                    );
                                    let failures = number(&report.counts.failed) as i64;
                                    if failures > 0 {
                                        desc.push(' ');
                                        desc.push_str(&tr(
                                            if failures == 1 {
                                                "{count} item couldn't be removed."
                                            } else {
                                                "{count} items couldn't be removed."
                                            },
                                            &[("count", Value::Int(failures))],
                                        ));
                                    }
                                    desc
                                }
                            };
                            if machines.len() > 1 {
                                format!("{}: {desc}", m.machine.get().scope_label)
                            } else {
                                desc
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }))
            })
            .control(move || {
                region(
                    move || pending.get(),
                    move |busy| {
                        action(
                            if busy { "Deleting…" } else { "Delete now" },
                            ButtonSize::Sm,
                            move || busy || !rules() || !allowed(),
                            move || {
                                if pending.get_untracked() || !rules() || !allowed() {
                                    return;
                                }
                                let Some(scope) = context.view() else {
                                    return;
                                };
                                if scope.scope.is_project_scope()
                                    || scope.selected.connected.iter().any(|e| {
                                        e.config.as_ref().is_none_or(|c| {
                                            c.environment.capabilities.storage_cleanup != Some(true)
                                        })
                                    })
                                {
                                    return;
                                }
                                let selected = machines
                                    .get()
                                    .into_iter()
                                    .filter(|m| {
                                        m.session.get().can_clean()
                                            && scope.selected.connected.iter().any(|e| {
                                                e.environment_id == m.machine.get().id
                                                    && e.config.as_ref().is_some_and(|c| {
                                                        c.environment
                                                            .capabilities
                                                            .storage_cleanup_run
                                                            == Some(true)
                                                    })
                                            })
                                    })
                                    .map(|m| (m, m.machine.get().label, m.session.get()))
                                    .collect::<Vec<_>>();
                                if selected.is_empty() {
                                    return;
                                }
                                pending.set(true);
                                context.spawn(async move {
                                    futures::future::join_all(selected.into_iter().map(
                                        |(m, label, session)| async move {
                                            match session.clean().await {
                                                Ok(report) => {
                                                    if let Some(e) = report
                                                        .entries
                                                        .iter()
                                                        .find(|e| e.outcome == Outcome::Failed)
                                                    {
                                                        notice(
                                                            Variant::Error,
                                                            tr(
                                                                "Cleanup failed on {label}",
                                                                &[("label", Value::Text(label))],
                                                            ),
                                                            e.reason.clone(),
                                                        );
                                                    }
                                                    let _ = m.report.try_set(LastRun::Ready(Some(
                                                        Rc::new(report),
                                                    )));
                                                }
                                                Err(err)
                                                    if !matches!(
                                                        err,
                                                        t3_client::ClientError::Cancelled
                                                    ) =>
                                                {
                                                    notice(
                                                        Variant::Error,
                                                        tr(
                                                            "Could not run cleanup on {label}",
                                                            &[("label", Value::Text(label))],
                                                        ),
                                                        err.to_string(),
                                                    )
                                                }
                                                _ => {}
                                            }
                                        },
                                    ))
                                    .await;
                                    let _ = pending.try_set(false);
                                });
                            },
                        )
                    },
                )
            }),
    );
    settings_section(
        "storage-cleanup",
        Text::from("Cleanup"),
        view().child(rows).child(region(
            move || {
                machines.with(|m| {
                    m.iter()
                        .filter_map(|m| match m.report.get() {
                            LastRun::Ready(Some(r)) => Some((m.machine.get().scope_label, r)),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
            },
            move |reports| {
                (!reports.is_empty()).then(|| {
                    let open = signal(false);
                    let summaries = reports
                        .iter()
                        .map(|(label, r)| {
                            let c = &r.counts;
                            let mut s = tr(
                                "{removed} removed, {kept} kept",
                                &[
                                    ("removed", Value::Int(number(&c.removed) as i64)),
                                    ("kept", Value::Int(number(&c.kept) as i64)),
                                ],
                            );
                            if number(&c.failed) > 0. {
                                s.push_str(&tr(
                                    ", {count} failed",
                                    &[("count", Value::Int(number(&c.failed) as i64))],
                                ));
                            }
                            (label.clone(), s)
                        })
                        .collect::<Vec<_>>();
                    let count = context.view().map_or(0, |v| v.selected.connected.len());
                    let summary = if count <= 1 {
                        summaries[0].1.clone()
                    } else if count == summaries.len()
                        && summaries.iter().all(|e| e.1 == summaries[0].1)
                    {
                        tr(
                            "{counts} on every machine",
                            &[("counts", Value::Text(summaries[0].1.clone()))],
                        )
                    } else {
                        summaries
                            .iter()
                            .map(|(label, s)| format!("{label}: {s}"))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    };
                    let reports = stored(reports);
                    view()
                        .child(button(
                            ButtonContent::Label(t("Last run")),
                            ButtonVariant::Ghost,
                            ButtonSize::Sm,
                            || false,
                            move || open.update(|v| *v = !*v),
                        ))
                        .child(text(summary).app_class(tw("text-xs text-muted-foreground")))
                        .child(region(
                            move || open.get(),
                            move |show| {
                                show.then(|| {
                                    let mut list = view();
                                    for (label, r) in reports.get() {
                                        list = list.child(results(label, &r));
                                    }
                                    list
                                })
                            },
                        ))
                })
            },
        )),
    )
    .into_element()
}
fn results(label: String, report: &StorageCleanupReport) -> AnyElement {
    let mut list = view();
    for e in &report.entries {
        let name = match e.kind {
            Kind::Logs => t("Rotated logs"),
            Kind::BrowserArtifacts => t("Browser artifacts"),
            Kind::Worktree => match (&e.thread_id, &e.path) {
                (Some(_), Some(path)) => path
                    .rsplit(['/', '\\'])
                    .find(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| t("Worktrees")),
                (_, path) => path.clone().unwrap_or_else(|| t("Worktrees")),
            },
        };
        let mut lines = vec![e.reason.clone(), format!("{}  {label}", t("Machine"))];
        for (key, value) in [
            ("Project", &e.project_path),
            ("Folder", &e.path),
            ("Thread", &e.thread_title),
        ] {
            if let Some(value) = value {
                lines.push(format!("{}  {value}", t(key)));
            }
        }
        let mut sizes = vec![];
        if e.outcome == Outcome::Removed || (e.outcome == Outcome::Kept && e.bytes.is_some()) {
            if e.kind != Kind::Worktree
                && let Some(n) = &e.files
            {
                let n = number(n) as u64;
                sizes.push(tr(
                    if n == 1 {
                        "{count} file"
                    } else {
                        "{count} files"
                    },
                    &[("count", Value::Int(n as i64))],
                ));
            }
            if let Some(bytes) = &e.bytes {
                sizes.push(format_bytes(number(bytes) as u64));
            }
        }
        let desc = lines.join("\n");
        let size = sizes.join(" · ");
        list = list.child(settings_row(
            SettingsRowProps::titled("", Text::content(name))
                .description(move || Text::content(desc.clone()))
                .control(move || text(size.clone()).app_class(tw("text-xs text-muted-foreground"))),
        ));
    }
    let omitted = number(&report.omitted_count) as i64;
    if omitted > 0 {
        list = list.child(text(tr(
            "{label}: {count} more results.",
            &[
                ("label", Value::Text(label)),
                ("count", Value::Int(omitted)),
            ],
        )));
    } else if report.entries.is_empty() {
        list = list.child(text(tr(
            "{label}: Nothing needed cleanup.",
            &[("label", Value::Text(label))],
        )));
    }
    list.into_element()
}
