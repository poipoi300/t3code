//! ProviderUpdatesAction.tsx and the provider part of UpdateEverythingDialog.tsx.
use crate::{
    i18n::{Text, Value, t, tr},
    runtime::{
        rt,
        tasks::{TaskOwner, watch},
    },
    screens::settings::shell::region,
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        dialog::*,
    },
};
use futures::StreamExt;
use nsh_core::prelude::*;
use std::collections::BTreeMap;
use t3_client::{
    composition::ClientRuntime,
    platform::Platform,
    services::{
        updates::providers::{Candidate, candidates, check, check_targets, update},
        usage::overview::driver_label,
    },
};

pub fn footer() -> AnyElement {
    rt().backend().client().as_ref().map_or_else(
        || view().into_element(),
        |c| footer_for(c, option_env!("T3_APP_VERSION")),
    )
}
pub fn footer_for<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    version: Option<&'static str>,
) -> AnyElement {
    let offers = signal(candidates(client));
    let revisions = signal(0u64);
    let runtime = client.clone();
    let mut changes = client.changes();
    watch(async move {
        while changes.next().await.is_some() {
            offers.set(candidates(&runtime));
            revisions.update(|n| *n += 1);
        }
    });
    let open = signal(false);
    let busy = signal(false);
    let reviewed = signal(Vec::<Candidate>::new());
    let completed = signal(Vec::<Candidate>::new());
    let result = signal(None::<(String, Vec<String>)>);
    let available = computed(move || {
        offers.with(|list| {
            completed.with(|done| {
                list.iter()
                    .filter(|c| !done.contains(c))
                    .cloned()
                    .collect::<Vec<_>>()
            })
        })
    });
    let runtime = client.clone();
    let tasks = TaskOwner::here();
    let sending = client.clone();
    let checking = signal(false);
    let checker = client.clone();
    let check_action = region(
        move || revisions.get(),
        move |_| {
            if check_targets(&checker).is_empty() {
                return None;
            }
            let checker = checker.clone();
            Some(button(
                ButtonContent::Label(t("Check for updates")),
                ButtonVariant::GhostMuted,
                ButtonSize::Xs,
                move || checking.get() || busy.get(),
                move || {
                    if checking.get_untracked() || busy.get_untracked() {
                        return;
                    }
                    checking.set(true);
                    let checker = checker.clone();
                    tasks.spawn(async move {
                        let failures = check(&checker).await;
                        if !failures.is_empty() {
                            result.set(Some((t("Could not check provider updates"), failures)));
                        }
                        checking.set(false);
                    });
                },
            ))
        },
    );
    let confirm = dialog(open, move |state| {
        dialog_popup(state, PopupOptions::default(), move || {
            let rows = reviewed.get_untracked();
            let mut machines = BTreeMap::<_, Vec<String>>::new();
            for c in &rows {
                machines
                    .entry((&c.environment, &c.machine))
                    .or_default()
                    .push(format!("{} · {}", driver_label(&c.driver.0), c.latest));
            }
            let title = if machines.len() == 1 {
                t("Update 1 machine?")
            } else {
                tr(
                    "Update {count} machines?",
                    &[("count", Value::Int(machines.len() as i64))],
                )
            };
            let count = if rows.len() == 1 {
                t("1 provider update")
            } else {
                tr(
                    "{count} provider updates",
                    &[("count", Value::Int(rows.len() as i64))],
                )
            };
            let mut list = view().app_class(tw("flex flex-col gap-3"));
            for ((_, machine), providers) in machines {
                list = list.child(text(machine.clone())).child(
                    text(providers.join(", ")).app_class(tw("text-xs text-muted-foreground")),
                );
            }
            let sending = sending.clone();
            view()
                .child(dialog_header(
                    state,
                    view()
                        .child(dialog_title(state, move || Text::content(title.clone())))
                        .child(dialog_description(state, move || {
                            Text::content(count.clone())
                        })),
                ))
                .child(dialog_panel(state, list))
                .child(
                    view()
                        .app_class(tw("flex flex-row justify-end gap-2"))
                        .child(button(
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Ghost,
                            ButtonSize::Sm,
                            || false,
                            move || open.set(false),
                        ))
                        .child(button(
                            ButtonContent::Label(t("Update")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            move || busy.get(),
                            move || {
                                if busy.get_untracked() {
                                    return;
                                }
                                let rows = rows.clone();
                                let sending = sending.clone();
                                busy.set(true);
                                result.set(None);
                                open.set(false);
                                tasks.spawn(async move {
                                    let replies = futures::future::join_all(
                                        rows.iter().map(|c| update(&sending, c)),
                                    )
                                    .await;
                                    let mut lines = Vec::new();
                                    let mut successes = Vec::new();
                                    for (c, reply) in rows.into_iter().zip(replies) {
                                        match reply {
                                            Ok(message) => {
                                                if let Some(m) = message {
                                                    lines.push(m);
                                                }
                                                successes.push(c);
                                            }
                                            Err(e) => lines.push(format!(
                                                "{} · {}: {e}",
                                                c.machine,
                                                driver_label(&c.driver.0)
                                            )),
                                        }
                                    }
                                    let n = successes.len();
                                    let total = reviewed.with_untracked(Vec::len);
                                    let failures = total - n;
                                    let title = match (n, failures) {
                                        (1, 0) => t("Provider updated"),
                                        (_, 0) => tr(
                                            "{count} providers updated",
                                            &[("count", Value::Int(n as i64))],
                                        ),
                                        (0, 1) => t("Provider update failed"),
                                        (0, _) => t("Provider updates failed"),
                                        _ => tr(
                                            "{failed} of {count} provider updates failed",
                                            &[
                                                ("failed", Value::Int(failures as i64)),
                                                ("count", Value::Int(total as i64)),
                                            ],
                                        ),
                                    };
                                    completed.update(|done| done.extend(successes));
                                    result.set(Some((title, lines)));
                                    busy.set(false);
                                });
                            },
                        )),
                )
        })
    });
    view()
        .app_class(tw("flex min-w-0 flex-col gap-1"))
        .child(region(
            move || revisions.get(),
            move |_| view().children(super::server_update::notices(&runtime, version)),
        ))
        .child(region(
            move || (available.get(), busy.get()),
            move |(offers, working)| {
                (!offers.is_empty() || working).then(|| {
                    button(
                        ButtonContent::Label(if working {
                            t("Updating…")
                        } else {
                            t("Update everything")
                        }),
                        ButtonVariant::GhostMuted,
                        ButtonSize::Xs,
                        move || busy.get(),
                        move || {
                            reviewed.set(offers.clone());
                            open.set(true);
                        },
                    )
                })
            },
        ))
        .child(region(
            move || result.get(),
            move |outcome| {
                outcome.map(|(title, lines)| {
                    view()
                        .child(text(title))
                        .child(text(lines.join("\n")))
                        .child(button(
                            ButtonContent::Label(t("Dismiss")),
                            ButtonVariant::Ghost,
                            ButtonSize::Xs,
                            || false,
                            move || result.set(None),
                        ))
                })
            },
        ))
        .child(check_action)
        .child(confirm)
        .into_element()
}
