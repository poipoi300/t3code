//! PullRequestRoutingSettings.tsx backed by the saved route identities.
use nsh_core::prelude::*;
use t3_client::{
    persistence::RoutingAccess, services::settings::service::source_control::RoutingChoice,
};

use super::super::shell::{SettingsRowProps, settings_row, settings_section};
use super::{Page, region};
use crate::{
    i18n::{Text, t, tr},
    runtime::tasks::watch,
    theme::tw,
    ui::button::{ButtonContent, ButtonSize, ButtonVariant, button},
};

const VALUES: [(&str, &str, RoutingAccess); 3] = [
    ("off", "Off", RoutingAccess::Off),
    ("read", "Read PRs", RoutingAccess::Read),
    ("read-write", "Read and act", RoutingAccess::ReadWrite),
];

pub(super) fn pull_request_routing_settings(page: Page) -> impl IntoElement {
    let choices = signal(Vec::<RoutingChoice>::new());
    let refresh = signal(0u64);
    let saving = signal(false);
    let error = signal(false);
    let services = page.services.get();
    effect(move || {
        let revision = refresh.get();
        let _ = page.context.view().map(|view| {
            view.all_environments
                .iter()
                .map(|entry| entry.environment_id.clone())
                .collect::<Vec<_>>()
        });
        if let Some(services) = services.clone() {
            let request = services.0.routing();
            watch(async move {
                let next = request.await;
                if refresh.try_get() == Some(revision) {
                    let _ = choices.try_set(next);
                }
            });
        }
    });
    let open = signal(false);
    region(
        move || (choices.get(), open.get()),
        move |(choices, expanded)| {
            if choices.len() < 2 {
                return None;
            }
            let summary = choices
                .iter()
                .filter(|entry| entry.permission != RoutingAccess::Off)
                .map(|entry| {
                    format!(
                        "{} {}",
                        entry.label,
                        if entry.permission == RoutingAccess::ReadWrite {
                            t("read and act")
                        } else {
                            t("read PRs")
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let mut rows = view().child(settings_row(
                SettingsRowProps::titled("", Text::from("Pull request routing"))
                    .first()
                    .description(move || {
                        if summary.is_empty() {
                            Text::from("Off")
                        } else {
                            Text::content(summary.clone())
                        }
                    })
                    .control(move || {
                        button(
                            ButtonContent::Label(if expanded {
                                t("Hide details")
                            } else {
                                t("Show details")
                            }),
                            ButtonVariant::Ghost,
                            ButtonSize::Xs,
                            || false,
                            move || open.update(|open| *open = !*open),
                        )
                    }),
            ));
            if expanded {
                rows = rows.child(text(t("Machines you trust here can read PR data through each other's source control access. Enable both machines. Read and act may use broader permissions than the machine that owns them. This applies only to this device."))
                .app_class(tw("px-3 py-2.5 text-xs text-muted-foreground sm:px-4")));
                for entry in choices {
                    let entry = stored(entry);
                    rows = rows.child(settings_row(
                        SettingsRowProps::titled(
                            "",
                            Text::content(entry.with(|entry| entry.label.clone())),
                        )
                        .control(move || {
                            let label = tr(
                                "{environment} pull request routing",
                                &[(
                                    "environment",
                                    t3_i18n::Value::Text(entry.with(|entry| entry.label.clone())),
                                )],
                            );
                            let value = entry.with(|entry| {
                                VALUES
                                    .iter()
                                    .find(|(_, _, permission)| *permission == entry.permission)
                                    .map_or("off", |(key, _, _)| *key)
                            });
                            let mut props =
                                crate::ui::select::SelectProps::new(Text::content(label));
                            props.size = crate::ui::select::Size::Sm;
                            props.value =
                                crate::ui::select::Value::Controlled(Some(value.into())).into();
                            props.items = VALUES
                                .iter()
                                .map(|(key, label, _)| {
                                    crate::ui::select::SelectItem::new(
                                        *key,
                                        Text::client_key(*label).resolve(),
                                    )
                                })
                                .collect::<Vec<_>>()
                                .into();
                            props.disabled = Prop::bind(move || {
                                saving.get() || !entry.with(|entry| entry.available)
                            });
                            crate::ui::select::select(props, move |next| {
                                let Some(permission) = next.and_then(|value| {
                                    VALUES
                                        .iter()
                                        .find(|(key, _, _)| *key == value.as_ref())
                                        .map(|(_, _, permission)| *permission)
                                }) else {
                                    return;
                                };
                                let Some(services) = page.services.get() else {
                                    return;
                                };
                                if saving.get_untracked() {
                                    return;
                                }
                                saving.set(true);
                                error.set(false);
                                let work = services
                                    .0
                                    .set_routing(entry.with(|entry| entry.id.clone()), permission);
                                page.tasks.spawn(async move {
                                    let result = work.await;
                                    let _ = saving.try_set(false);
                                    let _ = error.try_set(result.is_err());
                                    if let Some(revision) = refresh.try_get() {
                                        let _ = refresh.try_set(revision + 1);
                                    }
                                });
                            })
                        }),
                    ));
                }
                rows = rows.child(when(
                    move || error.get(),
                    || {
                        text(t("Could not save pull request routing permission"))
                            .app_class(tw("px-3 py-2 text-xs text-destructive"))
                    },
                ));
            }
            Some(settings_section(
                "pull-request-routing",
                Text::from("Pull request routing"),
                rows,
            ))
        },
    )
}
