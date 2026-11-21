//! Port of ProjectActionsSettings.tsx, ProjectActionsList.tsx and projectScriptEditor.tsx.
use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use serde_json::json;
use t3_client::{
    services::settings::service::{
        SettingsWriteAccess,
        project_actions::{ProjectActionEdit, ProjectActionImports},
    },
    workspace::{
        git_panel::scripts::{import_input, importable_scripts, script_keybinding_value},
        project_ops::scripts::ProjectScriptInput,
    },
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    project::{ProjectScript, ProjectScriptIcon},
    t3_project_file::T3ProjectFileScript,
    wire::trim_js,
};

use super::super::shell::{
    RowReset, SettingsRowProps, region, settings_row, settings_section, settings_select,
};
use super::ProjectSettings;
use crate::{
    i18n::{Text, t, tr},
    runtime::tasks::watch,
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        confirm::{
            ConfirmDialogInitialFocus, ConfirmDialogOptions, ConfirmDialogVariant,
            request_confirm_dialog,
        },
        dialog::{
            DialogState, FooterVariant, PopupOptions, dialog, dialog_close, dialog_description,
            dialog_footer, dialog_header, dialog_panel, dialog_popup, dialog_title,
        },
        icon::{Icon, icon_with_class},
        input::{InputSize, text_field},
        menu::{
            ItemOptions, MenuOptions, menu, menu_group_label, menu_item, menu_popup_with,
            menu_separator, menu_trigger,
        },
        switch::{ControlState, Size, switch},
        toast::{ToastOptions, Variant as ToastVariant},
    },
};

const KEY: &str = "defaultProjectScripts";

pub(super) fn project_actions_settings(settings: ProjectSettings) -> impl IntoElement {
    let context = settings.context;
    let imports = signal(ProjectActionImports::Missing);
    let target = context.view().and_then(|view| {
        let target = view.target()?;
        view.scope
            .members
            .iter()
            .find(|member| {
                member.environment_id == target.environment_id
                    && Some(&member.id) == target.project_id.as_ref()
            })
            .cloned()
    });
    if let Some((services, target)) = settings.services.get().zip(target) {
        let mut updates = (services.action_imports)(target);
        watch(async move {
            while let Some(update) = updates.next().await {
                let _ = imports.try_set(update);
            }
        });
    }
    let request = signal(None::<ProjectScript>);
    let open = signal(false);
    let saving = signal(false);
    let show_editor = move |script| {
        batch(|| {
            request.set(script);
            open.set(true);
        })
    };
    let available = move || {
        context.write_access() == SettingsWriteAccess::Writable
            && context.view().is_some_and(|view| !view.targets.is_empty())
    };

    settings_section("project-actions", Text::from("Actions"), view()
        .child(settings_row(SettingsRowProps::titled("", Text::from("Actions"))
            .first()
            .description(|| Text::from("Commands that run in this project's checkout or its worktree, with optional shortcuts."))
            .server(&[KEY])
            .reset(RowReset::new("project actions", || true, move || context.update(json!({ KEY: null }))))
            .control(move || {
                view().app_class(tw("flex flex-row flex-wrap items-center gap-1.5"))
                    .child(region(move || match imports.get() {
                        ProjectActionImports::Available(files) =>
                            importable_scripts(&files, &context.server_settings().default_project_scripts),
                        _ => Vec::new(),
                    }, move |files| {
                        (!files.is_empty()).then(|| import_scripts_menu(settings, files, saving, available))
                    }))
                    .child(button(
                        ButtonContent::IconLabel { icon: Icon::Plus, label: t("Add action") },
                        ButtonVariant::Outline, ButtonSize::Xs, move || saving.get() || !available(),
                        move || show_editor(None),
                    ))
            })))
        .child(region(move || (
            context.mixed(&[KEY]),
            context.server_settings().default_project_scripts.clone(),
        ), move |(mixed, scripts)| {
            if mixed {
                return settings_row(SettingsRowProps::titled("", Text::from("Different actions across checkouts"))
                    .description(|| Text::from("Narrow the selection above to one checkout to edit its list. Adding an action here adds it to every selected checkout.")))
                    .into_element();
            }
            if scripts.is_empty() {
                return text(t("No actions configured.")).app_class(tw("px-3 py-2 text-base text-muted-foreground sm:px-4 sm:text-sm")).into_element();
            }
            let mut rows = view();
            for script in scripts {
                let label = tr("Edit {name}", &[("name", t3_i18n::Value::Text(script.name.0.clone()))]);
                let command = script.command.0.clone();
                rows = rows.child(settings_row(SettingsRowProps::titled("", Text::content(script.name.0.clone()))
                    .description(move || Text::content(command.clone()))
                    .control(move || button(
                        ButtonContent::Icon { icon: Icon::Settings, label: label.clone() },
                        ButtonVariant::GhostMuted, ButtonSize::IconXs, move || saving.get() || !available(),
                        {
                            let script = script.clone();
                            move || show_editor(Some(script.clone()))
                        },
                    ))));
            }
            rows.into_element()
        }))
        .child(region(move || imports.get() == ProjectActionImports::Invalid, |invalid| {
            invalid.then(|| settings_row(SettingsRowProps::titled("", Text::from("t3.json is invalid"))
                .description(|| Text::from("A t3.json exists in this checkout but fails to parse, so every action and icon it declares is ignored. Check the JSON syntax and icon values."))))
        }))
        .child(dialog(open, move |state| {
            dialog_popup(state, PopupOptions { show_close_button: false, ..Default::default() }, move || {
                project_script_editor(settings, state, request.get_untracked(), saving)
            })
        })))
}

fn import_scripts_menu(
    settings: ProjectSettings,
    files: Vec<T3ProjectFileScript>,
    saving: Signal<bool>,
    available: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    menu(
        menu_trigger(t("Import scripts"), move || saving.get() || !available(),
            view().app_class(tw("flex h-6 flex-row items-center gap-1.5 rounded-md px-2 text-xs font-medium hover:bg-accent"))
                .child(text(t("Import scripts")))
                .child(icon_with_class(Icon::ChevronDown, "size-3.5"))),
        MenuOptions { align: Align::End, ..Default::default() },
        menu_popup_with(move || {
            let mut items = view()
                .child(menu_group_label(t("Import from t3.json"), false))
                .child(text(t("Add actions declared by this checkout without editing them first."))
                    .app_class(tw("px-2 pb-2 text-sm text-muted-foreground")))
                .child(menu_separator());
            for file in &files {
                let file = file.clone();
                items = items.child(menu_item(
                    file.name.0.clone(), ItemOptions::default(),
                    view().app_class(tw("min-w-0 flex-1"))
                        .child(text(file.name.0.clone()).app_class(tw("truncate font-medium")))
                        .child(text(file.command.0.clone()).app_class(tw("truncate font-mono text-muted-foreground"))),
                    move || {
                        if saving.get_untracked() || !available() { return; }
                        let (Some(scope), Some(services)) = (settings.context.view(), settings.services.get()) else { return; };
                        let input = import_input(file.clone(), true);
                        let work = (services.edit_action)(scope.search.clone(), ProjectActionEdit::Upsert {
                            id: None, input: input.script, keybinding: input.keybinding,
                        });
                        let toasts = settings.toasts.get();
                        saving.set(true);
                        settings.tasks.spawn(async move {
                            let result = work.await;
                            let _ = saving.try_set(false);
                            let description = match result {
                                Ok(result) if !result.failed => return,
                                Ok(result) => result.toast.and_then(|toast| toast.description.first()
                                    .map(super::super::shell::settings_text)),
                                Err(error) => Some(error.to_string()),
                            };
                            toasts.add(ToastOptions {
                                variant: ToastVariant::Error,
                                title: Text::from("Failed to import action."),
                                description: description.map(Text::content),
                                ..Default::default()
                            });
                        });
                    },
                ));
            }
            items
        }),
    )
}

fn project_script_editor(
    settings: ProjectSettings,
    dialog: DialogState,
    initial: Option<ProjectScript>,
    saving: Signal<bool>,
) -> impl IntoElement {
    let edited = initial.is_some();
    let id = initial.as_ref().map(|script| script.id.0.clone());
    let initial_shortcut = id
        .as_ref()
        .and_then(|id| {
            let view = settings.context.view()?;
            let target = view.target()?;
            let config = view
                .selected
                .connected
                .iter()
                .find(|environment| environment.environment_id == target.environment_id)?
                .config
                .as_ref()?;
            script_keybinding_value(&config.keybindings, id)
        })
        .unwrap_or_default();
    let shortcut = signal(initial_shortcut.clone());
    let name = signal(
        initial
            .as_ref()
            .map(|script| script.name.0.clone())
            .unwrap_or_default(),
    );
    let command = signal(
        initial
            .as_ref()
            .map(|script| script.command.0.clone())
            .unwrap_or_default(),
    );
    let preview_url = signal(
        initial
            .as_ref()
            .and_then(|script| script.preview_url.as_ref().and_then(Option::as_ref))
            .map(|url| url.0.clone())
            .unwrap_or_default(),
    );
    let icon = signal(
        initial
            .as_ref()
            .map(|script| script.icon.clone())
            .unwrap_or(ProjectScriptIcon::Play),
    );
    let setup = signal(
        initial
            .as_ref()
            .is_some_and(|script| script.run_on_worktree_create),
    );
    let wait = signal(
        initial
            .as_ref()
            .is_some_and(|script| script.r#async == Some(Some(false))),
    );
    let settle = signal(
        initial
            .as_ref()
            .is_some_and(|script| script.run_on_settle == Some(Some(true))),
    );
    let stop = signal(
        initial
            .as_ref()
            .is_some_and(|script| script.stop_on_settle == Some(Some(true))),
    );
    let auto_open = signal(
        initial
            .as_ref()
            .is_some_and(|script| script.auto_open_preview == Some(Some(true))),
    );
    let error = signal(None::<Text>);
    let persist = Rc::new(move |edit: ProjectActionEdit| {
        if saving.get_untracked() {
            return;
        }
        let Some(scope) = settings.context.view() else {
            return;
        };
        let Some(services) = settings.services.get() else {
            return;
        };
        saving.set(true);
        error.set(None);
        let operation = (services.edit_action)(scope.search.clone(), edit);
        settings.tasks.spawn(async move {
            let result = operation.await;
            let _ = saving.try_set(false);
            let message = match result {
                Ok(outcome) if !outcome.failed => {
                    if dialog.open.try_get() == Some(true) {
                        dialog.close();
                    }
                    return;
                }
                Ok(outcome) => outcome
                    .toast
                    .and_then(|toast| {
                        toast
                            .description
                            .first()
                            .map(super::super::shell::settings_text)
                    })
                    .map(Text::content)
                    .unwrap_or_else(|| Text::from("Failed to save project actions")),
                Err(error) => Text::content(error.to_string()),
            };
            let _ = error.try_set(Some(message));
        });
    });
    let save = {
        let id = id.clone();
        let persist = persist.clone();
        move || {
            let name = name.get_untracked();
            let command = command.get_untracked();
            let url = preview_url.get_untracked();
            let nonempty = |value: &str| {
                let value = trim_js(value);
                (!value.is_empty()).then(|| TrimmedNonEmptyString(value.to_owned()))
            };
            let (Some(name), Some(command)) = (nonempty(&name), nonempty(&command)) else {
                return;
            };
            let current_shortcut = shortcut.get_untracked();
            let key = trim_js(&current_shortcut);
            let keybinding = (id.is_none() || key != initial_shortcut)
                .then(|| (!key.is_empty()).then(|| key.to_owned()));
            persist(ProjectActionEdit::Upsert {
                id: id.clone(),
                keybinding,
                input: ProjectScriptInput {
                    name,
                    command,
                    icon: icon.get_untracked(),
                    run_on_worktree_create: setup.get_untracked(),
                    wait_for_setup: wait.get_untracked(),
                    run_on_settle: settle.get_untracked(),
                    stop_on_settle: stop.get_untracked(),
                    preview_url: nonempty(&url),
                    auto_open_preview: auto_open.get_untracked(),
                },
            });
        }
    };
    view()
        .child(dialog_header(
            dialog,
            view()
                .child(dialog_title(dialog, move || {
                    Text::from(if edited { "Edit action" } else { "Add action" })
                }))
                .child(dialog_description(dialog, || {
                    Text::from("Configure a command for this project.")
                })),
        ))
        .child(dialog_panel(
            dialog,
            view()
                .app_class(tw("flex flex-col gap-3"))
                .child(
                    view()
                        .app_class(tw("flex flex-row items-end gap-2"))
                        .child(action_field(name, "Name", "Test"))
                        .child(settings_select(
                            "Icon",
                            "",
                            vec![
                                ("play", Text::from("Play")),
                                ("test", Text::from("Test")),
                                ("lint", Text::from("Lint")),
                                ("configure", Text::from("Configure")),
                                ("build", Text::from("Build")),
                                ("debug", Text::from("Debug")),
                            ],
                            move || json!(icon.get()).as_str().unwrap_or_default().to_owned(),
                            move || saving.get(),
                            move |value| {
                                if let Ok(value) = serde_json::from_value(json!(value)) {
                                    icon.set(value);
                                }
                            },
                        )),
                )
                .child(action_field(shortcut, "Keybinding", "Press shortcut"))
                .child(text(t("Press a shortcut. Use Backspace to clear. Shortcuts are environment-wide. Projects using the same action share its shortcut."))
                    .app_class(tw("text-xs text-muted-foreground")))
                .child(action_field(command, "Command", "bun test"))
                .child(action_field(
                    preview_url,
                    "Preview URL (optional)",
                    "http://localhost:5173",
                ))
                .child(action_switch(
                    "Run when creating a worktree",
                    setup,
                    move || saving.get(),
                ))
                .child(action_switch(
                    "Wait for it to finish before the agent starts",
                    wait,
                    move || saving.get() || !setup.get(),
                ))
                .child(action_switch(
                    "Run in the thread's worktree when the thread settles",
                    settle,
                    move || saving.get(),
                ))
                .child(action_switch(
                    "Stop this action when the thread settles",
                    stop,
                    move || saving.get(),
                ))
                .child(action_switch(
                    "Open preview automatically when this action runs",
                    auto_open,
                    move || saving.get() || preview_url.with(|value| trim_js(value).is_empty()),
                ))
                .child(dynamic(move || {
                    error.get().map(|message| {
                        text(message.resolve()).app_class(tw("text-sm text-destructive"))
                    })
                })),
        ))
        .child(dialog_footer(
            dialog,
            FooterVariant::Bare,
            view()
                .app_class(tw("flex flex-row justify-end gap-2"))
                .child(id.map(move |id| {
                    button(
                        ButtonContent::Label(t("Delete")),
                        ButtonVariant::DestructiveOutline,
                        ButtonSize::Default,
                        move || saving.get(),
                        move || {
                            let message = tr(
                                "Delete action \"{name}\"?",
                                &[("name", t3_i18n::Value::Text(name.get_untracked()))],
                            );
                            let Some(confirmed) = request_confirm_dialog(
                                message,
                                Some(ConfirmDialogOptions {
                                    variant: ConfirmDialogVariant::Destructive,
                                    initial_focus: Some(ConfirmDialogInitialFocus::Cancel),
                                }),
                            ) else {
                                return;
                            };
                            let action = ProjectActionEdit::Remove(id.clone());
                            let persist = persist.clone();
                            settings.tasks.spawn(async move {
                                if confirmed.await {
                                    persist(action);
                                }
                            });
                        },
                    )
                }))
                .child(dialog_close(
                    dialog,
                    ButtonContent::Label(t("Cancel")),
                    ButtonVariant::Outline,
                    ButtonSize::Default,
                    || false,
                ))
                .child(button(
                    ButtonContent::Label(if edited {
                        t("Save changes")
                    } else {
                        t("Save action")
                    }),
                    ButtonVariant::Default,
                    ButtonSize::Default,
                    move || {
                        saving.get()
                            || name.with(|value| trim_js(value).is_empty())
                            || command.with(|value| trim_js(value).is_empty())
                    },
                    save,
                )),
        ))
}

fn action_field(
    value: Signal<String>,
    label: &'static str,
    placeholder: &'static str,
) -> impl IntoElement {
    view()
        .app_class(tw("flex min-w-0 flex-1 flex-col gap-2"))
        .child(text(Text::client_key(label).resolve()).app_class(tw("text-sm font-medium")))
        .child(text_field(
            value,
            Text::client_key(placeholder).resolve().to_string(),
            Text::client_key(label).resolve().to_string(),
            InputSize::Default,
            || {},
        ))
}

fn action_switch(
    label: &'static str,
    value: Signal<bool>,
    disabled: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    view().app_class(tw("flex flex-row items-center justify-between gap-3 rounded-md border border-border/70 px-3 py-2 text-sm"))
        .child(text(Text::client_key(label).resolve()).app_class(tw("min-w-0 flex-1")))
        .child(switch(label, ControlState { checked: value.into(), disabled: Prop::bind(disabled), ..Default::default() },
            Size::Default, move |enabled| value.set(enabled)))
}
