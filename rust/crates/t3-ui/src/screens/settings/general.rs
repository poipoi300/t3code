//! Port of apps/web/src/components/settings/SettingsPanels.tsx GeneralSettingsPanel (route
//! apps/web/src/routes/_chat.settings.general.tsx) and NotificationSettings.tsx.

use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::services::settings::panels::{
    default_unified_settings, is_project_grouping_enabled, project_grouping_mode_from_toggle,
};

use super::shell::{
    RowReset, SettingsContext, SettingsRowProps, settings_page_container, settings_row,
    settings_section, settings_select, use_settings,
};
use crate::i18n::Text;
use crate::ui::switch::{ControlState, Size as SwitchSize, switch};

/// The settings page body; the shell provides the modal, nav and scope.
pub fn screen() -> AnyElement {
    general_settings_panel().into_element()
}

/// A setting's wire value from the merged settings.
fn value(context: SettingsContext, key: &'static str) -> Value {
    context.setting(key)
}

fn is_default(context: SettingsContext, key: &'static str) -> bool {
    value(context, key) == default_unified_settings()[key]
}

fn reset(context: SettingsContext, key: &'static str, label: &'static str) -> RowReset {
    RowReset::new(
        label,
        move || !is_default(context, key),
        move || context.update(json!({ key: default_unified_settings()[key] })),
    )
}

/// A `Switch` bound to a boolean setting.
fn setting_switch(
    context: SettingsContext,
    key: &'static str,
    label: &'static str,
) -> impl IntoElement {
    switch(
        label,
        ControlState {
            checked: Prop::bind(move || value(context, key).as_bool().unwrap_or(false)),
            ..Default::default()
        },
        SwitchSize::Default,
        move |checked| context.update(json!({ key: checked })),
    )
}

fn switch_row(
    context: SettingsContext,
    id: &'static str,
    key: &'static str,
    description: &'static str,
    reset_label: &'static str,
    switch_label: &'static str,
) -> SettingsRowProps {
    SettingsRowProps::searchable(id)
        .description(move || Text::from(description))
        .reset(reset(context, key, reset_label))
        .control(move || setting_switch(context, key, switch_label))
}

const NOTIFICATION_MODE_LABELS: [(&str, &str); 4] = [
    ("off", "Off"),
    ("notifications", "Notifications only"),
    ("sound", "Sound only"),
    ("notifications-and-sound", "Notifications with sound"),
];

/// Port of NotificationSettings.tsx: choosing a mode with system alerts asks for permission.
fn notification_settings(context: SettingsContext) -> impl IntoElement {
    let permission_message = signal(None::<&'static str>);
    let requesting = signal(false);
    settings_row(
        SettingsRowProps::searchable("thread-notifications")
            .first()
            .description(move || {
                Text::from(permission_message.get().unwrap_or(
                    "System alerts when a thread finishes, fails, or needs input or approval. Applies to this device while T3 Code is open.",
                ))
            })
            .control(move || {
                settings_select(
                    "Thread notifications",
                    "w-full sm:w-56",
                    NOTIFICATION_MODE_LABELS
                        .iter()
                        .map(|(value, label)| (*value, (*label).to_owned()))
                        .collect(),
                    move || value(context, "notificationMode").as_str().unwrap_or("off").to_owned(),
                    move || requesting.get(),
                    move |mode| {
                        permission_message.set(None);
                        if !matches!(mode.as_str(), "notifications" | "notifications-and-sound") {
                            context.update(json!({ "notificationMode": mode }));
                            return;
                        }
                        requesting.set(true);
                        let answer = request_notification_permission();
                        spawn(async move {
                            use nsh_core::notifications::NotificationPermission as P;
                            let permission = answer.await;
                            let _ = requesting.try_set(false);
                            match permission {
                                P::Granted => context.update(json!({ "notificationMode": mode })),
                                P::Unavailable => {
                                    let _ = permission_message.try_set(Some(
                                        "Notifications need a supported browser over HTTPS, or the desktop app. Sound only is still available.",
                                    ));
                                }
                                _ => {
                                    let _ = permission_message.try_set(Some(
                                        "Allow notifications in your browser or system settings, then choose this option again. Sound only is still available.",
                                    ));
                                }
                            }
                        });
                    },
                )
            }),
    )
}

const SIDEBAR_PROJECT_SORT_ORDER_LABELS: [(&str, &str); 3] = [
    ("updated_at", "Last user message"),
    ("created_at", "Created at"),
    ("manual", "Manual"),
];

const QUIT_CONFIRMATION_MODE_LABELS: [(&str, &str); 3] = [
    ("direct", "Direct"),
    ("hold", "Hold"),
    ("double-click", "Double press"),
];

const TIMESTAMP_FORMAT_LABELS: [(&str, &str); 3] = [
    ("locale", "System default"),
    ("12-hour", "12-hour"),
    ("24-hour", "24-hour"),
];

fn labels(table: &[(&'static str, &'static str)]) -> Vec<(&'static str, String)> {
    table
        .iter()
        .map(|(value, label)| (*value, (*label).to_owned()))
        .collect()
}

/// `GeneralSettingsPanel`.
pub fn general_settings_panel() -> impl IntoElement {
    let context = use_settings();
    // `lastEnabledProjectGroupingMode`, read once from storage.
    let last_enabled = signal("repository");
    if let Some(read) = context.last_enabled_grouping_mode() {
        spawn(async move {
            let mode = read.await;
            let _ = last_enabled.try_set(mode);
        });
    }
    let desktop = nsh_core::element::desktop::current_window()
        .state()
        .is_some();
    settings_page_container(
        view()
            .app_class(crate::theme::tw("flex flex-col gap-8"))
            .child(settings_section(
                "notifications",
                Text::from("Notifications"),
                view()
                    .child(notification_settings(context))
                    .child(settings_row(switch_row(
                        context,
                        "in-app-notifications",
                        "inAppNotificationsEnabled",
                        "Show a toast when another thread finishes, fails, or needs input or approval while this app has focus.",
                        "in-app notifications",
                        "In-app notifications",
                    ))),
            ))
            .child(settings_section(
                "sidebar",
                Text::from("Sidebar"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("project-grouping")
                            .first()
                            .description(|| Text::from("Combine matching repositories across environments."))
                            .reset(reset(context, "sidebarProjectGroupingMode", "project grouping"))
                            .control(move || {
                                switch(
                                    "Project grouping",
                                    ControlState {
                                        checked: Prop::bind(move || {
                                            is_project_grouping_enabled(
                                                value(context, "sidebarProjectGroupingMode").as_str().unwrap_or("repository"),
                                            )
                                        }),
                                        ..Default::default()
                                    },
                                    SwitchSize::Default,
                                    move |checked| {
                                        let current = value(context, "sidebarProjectGroupingMode")
                                            .as_str()
                                            .unwrap_or("repository")
                                            .to_owned();
                                        if !checked && current != "separate" {
                                            last_enabled.set(if current == "repository_path" {
                                                "repository_path"
                                            } else {
                                                "repository"
                                            });
                                            context.remember_enabled_grouping_mode(&current);
                                        }
                                        context.update(json!({
                                            "sidebarProjectGroupingMode":
                                                project_grouping_mode_from_toggle(checked, last_enabled.get_untracked())
                                        }));
                                    },
                                )
                            }),
                    ))
                    .child(settings_row(
                        SettingsRowProps::searchable("project-order")
                            .description(|| Text::from("Order of projects in the sidebar project picker and command palette."))
                            .reset(reset(context, "sidebarProjectSortOrder", "project order"))
                            .control(move || {
                                settings_select(
                                    "Project order",
                                    "w-full sm:w-44",
                                    labels(&SIDEBAR_PROJECT_SORT_ORDER_LABELS),
                                    move || value(context, "sidebarProjectSortOrder").as_str().unwrap_or_default().to_owned(),
                                    || false,
                                    move |order| {
                                        if SIDEBAR_PROJECT_SORT_ORDER_LABELS.iter().any(|(v, _)| *v == order) {
                                            context.update(json!({ "sidebarProjectSortOrder": order }));
                                        }
                                    },
                                )
                            }),
                    ))
                    .child(settings_row(switch_row(
                        context,
                        "working-shelf",
                        "sidebarWorkingShelfEnabled",
                        "Fold working and monitoring threads into a Working section. They return to the top of the inbox when they need you. The boring sidebar in Labs has no Working section.",
                        "working section",
                        "Working section",
                    ))),
            ))
            .child(settings_section(
                "confirmations",
                Text::from("Confirmations"),
                view()
                    .child(settings_row(
                        switch_row(
                            context,
                            "unpin-confirmation",
                            "confirmThreadUnpin",
                            "Ask before unpinning a thread from the pinned section.",
                            "unpin confirmation",
                            "Confirm thread unpinning",
                        )
                        .first(),
                    ))
                    .child(settings_row(switch_row(
                        context,
                        "archive-confirmation",
                        "confirmThreadArchive",
                        "Require a second click on the inline archive action before a thread is archived.",
                        "archive confirmation",
                        "Confirm thread archiving",
                    )))
                    .child(settings_row(switch_row(
                        context,
                        "delete-confirmation",
                        "confirmThreadDelete",
                        "Ask before deleting a thread and its chat history.",
                        "delete confirmation",
                        "Confirm thread deletion",
                    )))
                    // `isElectron`: the quit shortcut exists where the app owns a window.
                    .child(desktop.then(|| {
                        settings_row(
                            SettingsRowProps::searchable("quit-confirmation")
                                .description(|| Text::from("Hold mode also quits on two quick presses."))
                                .reset(reset(context, "confirmQuit", "quit shortcut behavior"))
                                .control(move || {
                                    settings_select(
                                        "Quit shortcut behavior",
                                        "w-full sm:w-40",
                                        labels(&QUIT_CONFIRMATION_MODE_LABELS),
                                        move || value(context, "confirmQuit").as_str().unwrap_or_default().to_owned(),
                                        || false,
                                        move |mode| context.update(json!({ "confirmQuit": mode })),
                                    )
                                }),
                        )
                    })),
            ))
            .child(settings_section(
                "display",
                Text::from("Display"),
                view()
                    .child(settings_row(
                        SettingsRowProps::searchable("language")
                            .first()
                            .description(|| Text::from("System default follows your browser or OS language."))
                            .reset(reset(context, "language", "language"))
                            .control(move || {
                                let mut items = vec![(t3_i18n::SYSTEM_LANGUAGE, "System default".to_owned())];
                                // Language names are shown in their own language (content).
                                items.extend(
                                    t3_i18n::SUPPORTED_LOCALES
                                        .iter()
                                        .map(|locale| (*locale, t3_i18n::native_language_name(locale).to_owned())),
                                );
                                settings_select(
                                    "Language",
                                    "w-full sm:w-40",
                                    items,
                                    move || {
                                        let language = value(context, "language").as_str().unwrap_or_default().to_owned();
                                        if t3_i18n::SUPPORTED_LOCALES.contains(&language.as_str()) {
                                            language
                                        } else {
                                            t3_i18n::SYSTEM_LANGUAGE.to_owned()
                                        }
                                    },
                                    || false,
                                    move |language| context.update(json!({ "language": language })),
                                )
                            }),
                    ))
                    .child(settings_row(
                        SettingsRowProps::searchable("time-format")
                            .description(|| Text::from("System default follows your browser or OS clock preference."))
                            .reset(reset(context, "timestampFormat", "time format"))
                            .control(move || {
                                settings_select(
                                    "Timestamp format",
                                    "w-full sm:w-40",
                                    labels(&TIMESTAMP_FORMAT_LABELS),
                                    move || value(context, "timestampFormat").as_str().unwrap_or_default().to_owned(),
                                    || false,
                                    move |format| context.update(json!({ "timestampFormat": format })),
                                )
                            }),
                    )),
            )),
    )
}
