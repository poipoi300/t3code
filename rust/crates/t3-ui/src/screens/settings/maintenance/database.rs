use super::*;
use crate::i18n::{Value, tr};
use crate::ui::dialog::{
    ActionVariant, FooterVariant, alert_dialog, alert_dialog_action, alert_dialog_close,
    alert_dialog_description, alert_dialog_footer, alert_dialog_header, alert_dialog_popup,
    alert_dialog_title,
};
use t3_client::services::settings::maintenance::{minutes, number, recommend};

pub(super) fn section(
    context: SettingsContext,
    machines: Stored<Vec<Live>>,
    dismissed: Signal<Option<Vec<String>>>,
) -> AnyElement {
    region(
        move || {
            let dismissed = dismissed.get()?;
            let mut rows: Vec<_> = machines.with(|machines| {
                machines
                    .iter()
                    .enumerate()
                    .filter_map(|(i, live)| {
                        let status = live.database.get()?;
                        (recommend(&status) && !live.machine.with(|m| dismissed.contains(&m.id.0)))
                            .then_some((i, status))
                    })
                    .collect()
            });
            rows.sort_by(|a, b| {
                number(&b.1.reclaimable_bytes).total_cmp(&number(&a.1.reclaimable_bytes))
            });
            (!rows.is_empty()).then_some(rows)
        },
        move |rows| {
            rows.map(|rows| {
                let all = rows.iter().all(|(i, s)| s.can_reclaim == Some(true) && machines.get()[*i].machine.get().reclaim_allowed);

                let mut children = view().child(settings_row(SettingsRowProps::titled("", Text::from("Reclaim freed database space")).first().description(move || {
                    Text::from(if all {
                        "These databases still hold space freed by history cleanup."
                    } else {
                        "These databases still hold space freed by history cleanup. Run the command on each machine to reclaim it. It restarts the background service. Quit the desktop app first if it hosts the environment."
                    })
                })));

                for (i, status) in rows {
                    children = children.child(row(context, machines.get()[i], status, dismissed));
                }
                settings_section("storage-database", Text::from("Database"), children)
            })
        },
    )
}
fn row(
    context: SettingsContext,
    live: Live,
    status: DatabaseStorageStatus,
    dismissed: Signal<Option<Vec<String>>>,
) -> AnyElement {
    let machine = live.machine.get();
    let amount = format!(
        "{:.1} GiB",
        number(&status.reclaimable_bytes) / 1024_f64.powi(3)
    );
    let args = stored(vec![
        ("amount", Value::Text(amount)),
        ("label", Value::Text(machine.label)),
        ("minutes", Value::Int(minutes(&status) as i64)),
    ]);
    let message = move |key| args.with(|args| tr(key, args));
    let direct = status.can_reclaim == Some(true) && machine.reclaim_allowed;
    let title = Text::content(message("{amount} on {label}"));
    let desc = if !direct {
        status.compact_command.clone()
    } else if let Some(failure) = &status.reclaim_failure {
        tr(
            "The last attempt failed: {failure}",
            &[("failure", Value::Text(failure.clone()))],
        )
    } else {
        message("Restarts T3 Code there. It will be offline for about {minutes} min.")
    };
    let failed = status.reclaim_failure.is_some();
    let command = stored(status.compact_command);
    let confirming = signal(false);
    let service = stored(use_context::<SettingsSessions>());
    let reclaim = move || {
        let session = live.session.get();
        let can_reclaim = live
            .database
            .get_untracked()
            .is_some_and(|s| s.can_reclaim == Some(true) && recommend(&s));
        let selected = live.machine.with(|machine| {
            context.view().is_some_and(|scope| {
                !scope.scope.is_project_scope()
                    && scope.selected.connected.iter().any(|e| {
                        e.environment_id == machine.id
                            && e.config.as_ref().is_some_and(|c| {
                                c.environment.capabilities.database_storage == Some(true)
                            })
                    })
            })
        });
        if !selected || !can_reclaim || !session.can_reclaim() {
            return;
        }
        live.requested.set(true);
        let success = message("Reclaiming {amount} on {label}");
        let eta = message("T3 Code there restarts and is back in about {minutes} min.");
        let error = message("Could not reclaim space on {label}");
        context.spawn(async move {
            match session.reclaim().await {
                Ok(()) => notice(Variant::Success, success, eta),
                Err(err) => {
                    let _ = live.requested.try_set(false);
                    if !matches!(err, t3_client::ClientError::Cancelled) {
                        notice(Variant::Error, error, err.to_string());
                    }
                }
            }
        });
    };
    let dismiss = move || {
        let id = live.machine.get().id.0;
        let mut next = dismissed.get_untracked().unwrap_or_default();
        if !next.contains(&id) {
            next.push(id.clone());
        }
        dismissed.set(Some(next));
        if let Some(service) = service.get() {
            context.spawn(async move {
                if let Err(err) = service.dismiss(id.clone()).await {
                    if let Some(Some(mut ids)) = dismissed.try_get() {
                        ids.retain(|v| v != &id);
                        let _ = dismissed.try_set(Some(ids));
                    }
                    notice(Variant::Error, t("Setting not saved"), err.to_string());
                }
            });
        }
    };
    let props = SettingsRowProps::titled("", title).description(move || Text::content(desc.clone())).control(move || {
        let mut controls = view().app_class(tw("flex flex-row flex-wrap items-center gap-1.5"));
        if direct {
            let open = region(move || live.requested.get() && !failed, move |restarting| action(if restarting { "Restarting…" } else { "Reclaim" }, ButtonSize::Xs, move || restarting, move || confirming.set(true)));

            let confirm = alert_dialog(confirming, move |dialog| {
                alert_dialog_popup(dialog, Default::default(), move || {
                    let heading = alert_dialog_title(dialog, move || Text::content(message("Reclaim {amount} on {label}?")));

                    let description = alert_dialog_description(dialog, move || Text::content(message("T3 Code on {label} stops, rebuilds its database, and starts again. It will be offline for about {minutes} min. Running agents stop when it restarts.")));

                    let cancel = alert_dialog_close(dialog, ButtonContent::Label(t("Cancel")), ButtonVariant::Outline, ButtonSize::Default, || false);

                    let accept = alert_dialog_action(
                        dialog,
                        ButtonContent::Label(t("Restart and reclaim")),
                        ActionVariant::Default,
                        ButtonSize::Default,
                        || false,
                        move || {
                            dialog.close();
                            reclaim();
                        },
                    );

                    view().child(alert_dialog_header(dialog, view().child(heading).child(description))).child(alert_dialog_footer(dialog, FooterVariant::Default, view().app_class(tw("flex flex-row gap-2")).child(cancel).child(accept)))
                })
            });
            controls = controls.child(open).child(confirm);
        } else {
            controls = controls.child(action("Copy command", ButtonSize::Xs, || false, move || set_clipboard(command.get())));
        }
        controls.child(button(ButtonContent::Label(t("Dismiss")), ButtonVariant::Ghost, ButtonSize::Xs, || false, dismiss))
    });
    settings_row(props).into_element()
}
