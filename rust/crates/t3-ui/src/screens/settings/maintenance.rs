//! MaintenanceSettingsPanel, StorageCleanupSections and DatabaseStorageNotice.
mod background;
mod cleanup;
mod database;
mod retention;

use super::{
    setting_controls::reset,
    shell::{
        SettingsContext, SettingsRowProps, region, settings_page_container, settings_row,
        settings_section, use_settings,
    },
};
use crate::{
    i18n::{Text, t},
    runtime::tasks::watch,
    state::nav::SettingsPage,
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        switch::{ControlState, Size, switch},
        toast::{ToastOptions, Variant, toast_manager},
    },
};
use futures::StreamExt;
use nsh_core::prelude::*;
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    services::settings::{
        maintenance::StorageSession,
        scope::{SettingsScopeSearch, settings_scope_environment_label},
        sessions::SettingsSessions,
    },
    types::EnvironmentId,
};
use t3_contracts::settings::{DatabaseStorageStatus, StorageCleanupReport};

#[derive(Clone, PartialEq)]
struct Machine {
    id: EnvironmentId,
    label: String,
    scope_label: String,
    generation: (u64, u64),
    database: bool,
    cleanup: bool,
    run: bool,
    reclaim_allowed: bool,
    run_allowed: bool,
}
#[derive(Clone, PartialEq)]
enum LastRun {
    Loading,
    Failed,
    Ready(Option<Rc<StorageCleanupReport>>),
}
#[derive(Clone, Copy)]
struct Live {
    machine: Stored<Machine>,
    session: Stored<Rc<StorageSession>>,
    database: Signal<Option<DatabaseStorageStatus>>,
    report: Signal<LastRun>,
    requested: Signal<bool>,
}
fn observe(machine: Machine, service: &SettingsSessions, read_reports: bool) -> Option<Live> {
    let (serial, generation, endpoint) = service.session(&machine.id)?;
    if (serial, generation) != machine.generation {
        return None;
    }
    let session = Rc::new(StorageSession(endpoint));
    let database = signal(None);
    if machine.database {
        let mut stream = session.database();
        watch(async move {
            while let Some(next) = stream.next().await {
                database.set(next.ok().flatten());
            }
        });
    }
    let report = signal(LastRun::Loading);
    if read_reports && machine.run {
        let mut stream = session.reports();
        watch(async move {
            while let Some(next) = stream.next().await {
                report.set(match next {
                    Ok(value) => LastRun::Ready(value.map(Rc::new)),
                    Err(_) => LastRun::Failed,
                });
            }
        });
    }
    Some(Live {
        machine: stored(machine),
        session: stored(session),
        database,
        report,
        requested: signal(false),
    })
}

pub fn screen() -> AnyElement {
    let context = use_settings();
    let service = stored(use_context::<SettingsSessions>());
    let dismissed = signal(None::<Vec<String>>);
    if let Some(session) = service.get() {
        watch(async move {
            dismissed.set(Some(session.dismissed().await.unwrap_or_default()));
        });
    } else {
        dismissed.set(Some(vec![]));
    }
    let updates = view()
        .child(toggle(
            context,
            "provider-update-checks",
            "enableProviderUpdateChecks",
            "provider update checks",
            "Check provider versions",
            "Check installed provider CLIs for newer available versions and fetch model catalog updates. When off, new models may not appear.",
        ))
        .child(toggle(
            context,
            "continue-threads-after-server-update",
            "continueThreadsAfterServerUpdate",
            "continue threads after restarts",
            "Continue threads after restarts",
            "Automatically resume interrupted threads after an update, crash, or machine restart on the selected environments.",
        ));

    let background = view().child(background::row(context)).child(toggle(
        context,
        "keep-awake-while-agents-work",
        "keepAwakeWhileAgentsWork",
        "keep awake while agents work",
        "Keep awake while agents work",
        "Stop the computer from going to sleep while agents are working, and let it sleep once they finish. Closing a laptop lid still puts it to sleep.",
    ));

    let storage = region(
        move || {
            context.revision();
            let scope = context.view()?;
            if scope.scope.is_project_scope() {
                return None;
            }
            let service = service.get()?;
            let machines = scope
                .selected
                .connected
                .iter()
                .filter_map(|e| {
                    let config = e.config.as_ref()?;
                    let (serial, generation, endpoint) = service.session(&e.environment_id)?;
                    let cap = &config.environment.capabilities;
                    let session = StorageSession(endpoint);
                    Some(Machine {
                        id: e.environment_id.clone(),
                        label: e.label.clone(),
                        scope_label: settings_scope_environment_label(e, &scope.selected.connected),
                        generation: (serial, generation),
                        database: cap.database_storage == Some(true),
                        cleanup: cap.storage_cleanup == Some(true),
                        run: cap.storage_cleanup_run == Some(true),
                        reclaim_allowed: session.can_reclaim(),
                        run_allowed: session.can_clean(),
                    })
                })
                .collect::<Vec<_>>();
            let supported = scope.selected.connected.iter().all(|e| {
                e.config.as_ref().is_some_and(|config| {
                    config.environment.capabilities.storage_cleanup == Some(true)
                })
            });
            Some((machines, supported))
        },
        move |selected| {
            selected.map(|(machines, supported)| {
                let live = service
                    .get()
                    .map(|s| {
                        machines
                            .iter()
                            .filter_map(|m| observe(m.clone(), &s, supported))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                let live = stored(live);
                view()
                    .app_class(tw("flex flex-col gap-8"))
                    .child(database::section(context, live, dismissed))
                    .child(retention::section(context, supported, machines))
                    .child(supported.then(|| cleanup::section(context, live)))
            })
        },
    );
    let diagnostics = SettingsRowProps::searchable("diagnostics")
        .first()
        .description(move || {
            Text::from(if single_id(context).is_some() {
                "Inspect processes, resource use, and logs on this environment."
            } else {
                "Inspect processes, resource use, and logs on one environment at a time."
            })
        })
        .control(move || {
            action(
                "View diagnostics",
                ButtonSize::Sm,
                || false,
                move || {
                    context.open_page(
                        SettingsPage::Diagnostics,
                        SettingsScopeSearch {
                            machine: single_id(context),
                            ..Default::default()
                        },
                    )
                },
            )
        });
    let page = view()
        .app_class(tw("flex flex-col gap-8"))
        .child(settings_section("updates", Text::from("Updates"), updates))
        .child(settings_section(
            "background",
            Text::from("Background activity"),
            background,
        ))
        .child(storage)
        .child(settings_section(
            "diagnostics-section",
            Text::from("Diagnostics"),
            settings_row(diagnostics),
        ));
    settings_page_container(page).into_element()
}

fn single_id(context: SettingsContext) -> Option<String> {
    let scope = context.view()?;
    (scope.scope.environment_ids.len() == 1)
        .then(|| {
            scope
                .selected
                .representative
                .as_ref()
                .map(|e| e.environment_id.0.clone())
        })
        .flatten()
}
fn toggle(
    context: SettingsContext,
    id: &'static str,
    key: &'static str,
    reset_label: &'static str,
    label: &'static str,
    description: &'static str,
) -> AnyElement {
    let keys: &'static [&'static str] = match key {
        "enableProviderUpdateChecks" => &["enableProviderUpdateChecks"],
        "continueThreadsAfterServerUpdate" => &["continueThreadsAfterServerUpdate"],
        _ => &["keepAwakeWhileAgentsWork"],
    };
    let offline = move || {
        key == "continueThreadsAfterServerUpdate"
            && context
                .view()
                .is_none_or(|v| v.selected.connected.is_empty())
    };
    settings_row(
        SettingsRowProps::searchable(id)
            .server(keys)
            .description(move || Text::from(description))
            .status(move || {
                offline().then(|| {
                    Text::from(
                        "All selected connected environments must support restart continuation.",
                    )
                })
            })
            .reset(reset(context, key, reset_label))
            .control(move || {
                switch(
                    label,
                    ControlState {
                        checked: Prop::bind(move || {
                            !context.mixed(keys) && context.setting(key) == true
                        }),
                        mixed: Prop::bind(move || context.mixed(keys)),
                        disabled: Prop::bind(offline),
                        ..Default::default()
                    },
                    Size::Default,
                    move |on| context.update(json!({key:on})),
                )
            }),
    )
    .into_element()
}
fn action(
    label: &'static str,
    size: ButtonSize,
    disabled: impl Fn() -> bool + 'static,
    run: impl Fn() + 'static,
) -> AnyElement {
    button(
        ButtonContent::Label(t(label)),
        ButtonVariant::Outline,
        size,
        disabled,
        run,
    )
    .into_element()
}
fn notice(variant: Variant, title: String, description: String) {
    toast_manager().add(ToastOptions {
        variant,
        title: Text::content(title),
        description: Some(Text::content(description)),
        ..Default::default()
    });
}
